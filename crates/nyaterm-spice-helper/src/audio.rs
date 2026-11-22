//! Output-only PCM playback. Device discovery never runs on the GLib thread.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SizedSample};

const MAX_RATE: u32 = 192_000;
const BUFFER_MS: usize = 250;
const MAX_PACKET: usize = MAX_RATE as usize * 4;

#[derive(Clone, Copy, PartialEq, Eq)]
struct Format {
    channels: usize,
    rate: u32,
}

struct Pcm {
    samples: VecDeque<[f32; 2]>,
    format: Option<Format>,
    generation: u64,
    phase: f64,
    latency_ms: u32,
    primed: bool,
    volume: [f32; 2],
    server_muted: bool,
    stopped: bool,
}

impl Default for Pcm {
    fn default() -> Self {
        Self {
            samples: VecDeque::with_capacity(MAX_RATE as usize * BUFFER_MS / 1000),
            format: None,
            generation: 0,
            phase: 0.0,
            latency_ms: 0,
            primed: false,
            volume: [1.0; 2],
            server_muted: false,
            stopped: false,
        }
    }
}

impl Pcm {
    fn configure(&mut self, format: Option<Format>) {
        self.format = format;
        self.generation = self.generation.wrapping_add(1);
        self.samples.clear();
        self.phase = 0.0;
        self.primed = false;
    }

    fn append(&mut self, data: &[u8]) -> bool {
        let Some(format) = self.format else {
            return true;
        };
        let frame_bytes = format.channels * 2;
        if data.len() > MAX_PACKET || !data.len().is_multiple_of(frame_bytes) {
            return false;
        }
        let limit = format.rate as usize * BUFFER_MS / 1000;
        let frames = data.len() / frame_bytes;
        let skip = frames.saturating_sub(limit);
        let evict = self
            .samples
            .len()
            .saturating_add(frames - skip)
            .saturating_sub(limit);
        self.samples.drain(..evict.min(self.samples.len()));
        if skip != 0 || evict != 0 {
            self.phase = 0.0;
        }
        for frame in data[skip * frame_bytes..].chunks_exact(frame_bytes) {
            let left = f32::from(i16::from_le_bytes([frame[0], frame[1]])) / 32768.0;
            let right = if format.channels == 2 {
                f32::from(i16::from_le_bytes([frame[2], frame[3]])) / 32768.0
            } else {
                left
            };
            self.samples.push_back([left, right]);
        }
        true
    }

    fn sample(&mut self, output_rate: u32, muted: bool) -> [f32; 2] {
        let Some(format) = self.format else {
            return [0.0; 2];
        };
        if !self.primed {
            if self.samples.len()
                < (u64::from(format.rate) * u64::from(self.latency_ms) / 1000) as usize
            {
                return [0.0; 2];
            }
            self.primed = true;
        }
        let Some(first) = self.samples.front().copied() else {
            self.phase = 0.0;
            self.primed = false;
            return [0.0; 2];
        };
        let second = self.samples.get(1).copied().unwrap_or(first);
        let phase = self.phase as f32;
        let value = std::array::from_fn(|i| {
            if muted || self.server_muted {
                0.0
            } else {
                (first[i] + (second[i] - first[i]) * phase) * self.volume[i]
            }
        });
        self.phase += f64::from(format.rate) / f64::from(output_rate);
        let consume = self.phase.floor() as usize;
        self.phase -= consume as f64;
        self.samples.drain(..consume.min(self.samples.len()));
        value
    }
}

struct Shared {
    pcm: Mutex<Pcm>,
    changed: Condvar,
    muted: AtomicBool,
    ready: AtomicBool,
    failed: AtomicBool,
    device_delay_ms: AtomicU32,
}

pub(crate) struct AudioOutput {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
}

impl AudioOutput {
    pub fn new() -> Self {
        let shared = Arc::new(Shared {
            pcm: Mutex::new(Pcm::default()),
            changed: Condvar::new(),
            muted: AtomicBool::new(false),
            ready: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            device_delay_ms: AtomicU32::new(0),
        });
        let worker_shared = shared.clone();
        let worker = thread::Builder::new()
            .name("spice-audio".into())
            .spawn(move || {
                device_worker(worker_shared);
            })
            .ok();
        if worker.is_none() {
            shared.failed.store(true, Ordering::Release);
        }
        Self { shared, worker }
    }

    pub fn start(&self, channels: i32, rate: i32) -> bool {
        if !(1..=2).contains(&channels) || !(8_000..=MAX_RATE as i32).contains(&rate) {
            return false;
        }
        let mut pcm = self.shared.pcm.lock().unwrap_or_else(|p| p.into_inner());
        pcm.configure(Some(Format {
            channels: channels as usize,
            rate: rate as u32,
        }));
        self.shared.ready.store(false, Ordering::Release);
        drop(pcm);
        self.shared.changed.notify_one();
        true
    }

    pub fn stop(&self) {
        let mut pcm = self.shared.pcm.lock().unwrap_or_else(|p| p.into_inner());
        pcm.configure(None);
        self.shared.ready.store(false, Ordering::Release);
        drop(pcm);
        self.shared.changed.notify_one();
    }

    pub fn push(&self, data: &[u8]) -> bool {
        self.shared
            .pcm
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .append(data)
    }

    pub fn set_muted(&self, muted: bool) {
        self.shared.muted.store(muted, Ordering::Release);
    }

    pub fn set_server_volume(&self, volume: [f32; 2], muted: bool) {
        let mut pcm = self.shared.pcm.lock().unwrap_or_else(|p| p.into_inner());
        pcm.volume = volume;
        pcm.server_muted = muted;
    }

    pub fn set_latency_ms(&self, milliseconds: u32) {
        self.shared
            .pcm
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .latency_ms = milliseconds.min(BUFFER_MS as u32);
    }

    pub fn ready(&self) -> bool {
        self.shared.ready.load(Ordering::Acquire)
    }
    pub fn take_failure(&self) -> bool {
        self.shared.failed.swap(false, Ordering::AcqRel)
    }

    pub fn delay_ms(&self) -> u32 {
        let pcm = self.shared.pcm.lock().unwrap_or_else(|p| p.into_inner());
        pcm.format.map_or(0, |f| {
            (pcm.samples.len() * 1000 / f.rate as usize) as u32
                + self.shared.device_delay_ms.load(Ordering::Relaxed)
        })
    }
}

impl Drop for AudioOutput {
    fn drop(&mut self) {
        self.shared
            .pcm
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .stopped = true;
        self.shared.changed.notify_one();
        // Driver calls are not cancellable. Never hold up helper shutdown on
        // a wedged device; the process supervisor owns the final reap deadline.
        if let Some(worker) = self.worker.take() {
            for _ in 0..20 {
                if worker.is_finished() {
                    let _ = worker.join();
                    return;
                }
                thread::sleep(Duration::from_millis(5));
            }
        }
    }
}

fn device_worker(shared: Arc<Shared>) {
    let mut observed = 0;
    let mut stream = None;
    loop {
        let (generation, format) = {
            let mut pcm = shared.pcm.lock().unwrap_or_else(|p| p.into_inner());
            while !pcm.stopped && pcm.generation == observed {
                pcm = shared.changed.wait(pcm).unwrap_or_else(|p| p.into_inner());
            }
            if pcm.stopped {
                break;
            }
            (pcm.generation, pcm.format)
        };
        observed = generation;
        drop(stream.take());
        if format.is_none() {
            continue;
        }
        match open_device(shared.clone(), generation) {
            Ok(output) => {
                let pcm = shared.pcm.lock().unwrap_or_else(|p| p.into_inner());
                if pcm.generation == generation && !pcm.stopped {
                    stream = Some(output);
                    shared.ready.store(true, Ordering::Release);
                }
            }
            Err(()) => {
                shared.failed.store(true, Ordering::Release);
            }
        }
    }
    shared.ready.store(false, Ordering::Release);
}

fn open_device(shared: Arc<Shared>, generation: u64) -> Result<cpal::Stream, ()> {
    let device = cpal::default_host().default_output_device().ok_or(())?;
    let supported = device.default_output_config().map_err(|_| ())?;
    let config = supported.config();
    if config.channels == 0 || config.channels > 32 || config.sample_rate == 0 {
        return Err(());
    }
    let stream = match supported.sample_format() {
        cpal::SampleFormat::I8 => build::<i8>(&device, config, shared, generation),
        cpal::SampleFormat::I16 => build::<i16>(&device, config, shared, generation),
        cpal::SampleFormat::I32 => build::<i32>(&device, config, shared, generation),
        cpal::SampleFormat::I64 => build::<i64>(&device, config, shared, generation),
        cpal::SampleFormat::U8 => build::<u8>(&device, config, shared, generation),
        cpal::SampleFormat::U16 => build::<u16>(&device, config, shared, generation),
        cpal::SampleFormat::U32 => build::<u32>(&device, config, shared, generation),
        cpal::SampleFormat::U64 => build::<u64>(&device, config, shared, generation),
        cpal::SampleFormat::F32 => build::<f32>(&device, config, shared, generation),
        cpal::SampleFormat::F64 => build::<f64>(&device, config, shared, generation),
        _ => Err(()),
    }?;
    stream.play().map_err(|_| ())?;
    Ok(stream)
}

fn build<T: SizedSample + FromSample<f32>>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    shared: Arc<Shared>,
    generation: u64,
) -> Result<cpal::Stream, ()> {
    let failed = shared.clone();
    let channels = usize::from(config.channels);
    let rate = config.sample_rate;
    device
        .build_output_stream(
            config,
            move |output: &mut [T], info| {
                let timestamp = info.timestamp();
                shared.device_delay_ms.store(
                    timestamp
                        .playback
                        .duration_since(timestamp.callback)
                        .as_millis()
                        .min(1000) as u32,
                    Ordering::Relaxed,
                );
                // Audio callbacks neither allocate nor wait for the network thread.
                let Ok(mut pcm) = shared.pcm.try_lock() else {
                    output.fill(T::from_sample(0.0));
                    return;
                };
                if pcm.generation != generation || pcm.stopped {
                    output.fill(T::from_sample(0.0));
                    return;
                }
                let muted = shared.muted.load(Ordering::Acquire);
                for frame in output.chunks_mut(channels) {
                    let [left, right] = pcm.sample(rate, muted);
                    for (i, sample) in frame.iter_mut().enumerate() {
                        *sample = T::from_sample(if channels == 1 {
                            (left + right) * 0.5
                        } else {
                            match i {
                                0 => left,
                                1 => right,
                                _ => 0.0,
                            }
                        });
                    }
                }
            },
            move |_| {
                failed.ready.store(false, Ordering::Release);
                failed.failed.store(true, Ordering::Release);
            },
            Some(Duration::from_millis(200)),
        )
        .map_err(|_| ())
}

#[cfg(test)]
mod tests {
    use crate::audio::{BUFFER_MS, Format, Pcm};

    #[test]
    fn playback_backlog_is_bounded_and_keeps_latest_audio() {
        let mut pcm = Pcm::default();
        pcm.configure(Some(Format {
            channels: 1,
            rate: 8_000,
        }));
        assert!(pcm.append(&vec![0; 40_000]));
        assert_eq!(pcm.samples.len(), 8_000 * BUFFER_MS / 1000);
        assert!(pcm.append(&i16::MAX.to_le_bytes()));
        assert!(pcm.samples.back().unwrap()[0] > 0.99);
        assert!(!pcm.append(&[1]));
    }

    #[test]
    fn resampling_interpolates_and_muting_consumes_audio() {
        let mut pcm = Pcm::default();
        pcm.configure(Some(Format {
            channels: 1,
            rate: 8_000,
        }));
        assert!(pcm.append(&[0, 0, 0, 64, 0, 0, 0, 0]));
        assert_eq!(pcm.sample(16_000, false), [0.0; 2]);
        assert_eq!(pcm.sample(16_000, false), [0.25; 2]);
        assert_eq!(pcm.sample(16_000, false), [0.5; 2]);
        assert_eq!(pcm.sample(16_000, true), [0.0; 2]);
        assert_eq!(pcm.samples.len(), 2);
        let old = pcm.generation;
        pcm.configure(None);
        assert_ne!(pcm.generation, old);
        assert!(pcm.samples.is_empty());
        assert_eq!(pcm.sample(48_000, false), [0.0; 2]);
    }

    #[test]
    fn guest_volume_and_stereo_order_are_preserved() {
        let mut pcm = Pcm::default();
        pcm.configure(Some(Format {
            channels: 2,
            rate: 48_000,
        }));
        pcm.volume = [0.5, 1.0];
        assert!(pcm.append(&[0, 64, 0, 128]));
        assert_eq!(pcm.sample(48_000, false), [0.25, -1.0]);
        assert_eq!(pcm.sample(48_000, false), [0.0; 2]);
    }

    #[test]
    fn jitter_buffer_rebuilds_after_an_underrun() {
        let mut pcm = Pcm::default();
        pcm.configure(Some(Format {
            channels: 1,
            rate: 8_000,
        }));
        pcm.latency_ms = 1;
        assert!(pcm.append(&[0, 64]));
        assert_eq!(pcm.sample(8_000, false), [0.0; 2]);
        assert_eq!(pcm.samples.len(), 1);
        assert!(pcm.append(&[0, 64].repeat(7)));
        for _ in 0..8 {
            assert_eq!(pcm.sample(8_000, false), [0.5; 2]);
        }
        assert_eq!(pcm.sample(8_000, false), [0.0; 2]);
        assert!(pcm.append(&[0, 64]));
        assert_eq!(pcm.sample(8_000, false), [0.0; 2]);
        assert_eq!(pcm.samples.len(), 1);
    }
}
