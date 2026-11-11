//! Isolated scrcpy 5.0.1 video and control runtime.
mod decoder;
mod protocol;

use std::collections::{HashSet, VecDeque};
use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use nyaterm_android::ipc::{Control, Frame, PROTOCOL_VERSION, Packet, read_packet, write_packet};

struct OutputState {
    controls: VecDeque<Control>,
    latest: Option<Frame>,
    in_flight: Option<u64>,
    sequence: u64,
    closed: bool,
    visible: bool,
    epoch: u64,
    pixels: Vec<Vec<u8>>,
}

impl Default for OutputState {
    fn default() -> Self {
        Self {
            controls: VecDeque::new(),
            latest: None,
            in_flight: None,
            sequence: 0,
            closed: false,
            visible: true,
            epoch: 0,
            pixels: Vec::new(),
        }
    }
}

#[derive(Default)]
struct Output {
    state: Mutex<OutputState>,
    changed: Condvar,
}

impl Output {
    fn control(&self, control: Control) {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.controls.len() >= 32 {
            state.controls.pop_front();
        }
        state.controls.push_back(control);
        self.changed.notify_one();
    }
    fn frame(&self, mut frame: Frame) {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if !state.visible {
            if state.pixels.len() < 2 {
                state.pixels.push(frame.pixels);
            }
            return;
        }
        state.sequence = state.sequence.wrapping_add(1).max(1);
        frame.sequence = state.sequence;
        state.latest = Some(frame);
        self.changed.notify_one();
    }
    fn acknowledge(&self, sequence: u64) {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.in_flight == Some(sequence) {
            state.in_flight = None;
            self.changed.notify_one();
        }
    }
    fn presentation(&self, visible: bool) {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.visible != visible {
            state.epoch = state.epoch.wrapping_add(1);
        }
        state.visible = visible;
        if !visible
            && let Some(frame) = state.latest.take()
            && state.pixels.len() < 2
        {
            state.pixels.push(frame.pixels);
        }
        self.changed.notify_all();
    }
    fn credit(&self) -> Option<u64> {
        let state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        (state.visible && !state.closed && state.in_flight.is_none() && state.latest.is_none())
            .then_some(state.epoch)
    }
    fn take_pixels(&self) -> Vec<u8> {
        self.state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .pixels
            .pop()
            .unwrap_or_default()
    }
    fn close(&self) {
        self.state.lock().unwrap_or_else(|p| p.into_inner()).closed = true;
        self.changed.notify_all();
    }
}

fn output_loop(output: Arc<Output>) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    loop {
        let packet = {
            let mut state = output.state.lock().unwrap_or_else(|p| p.into_inner());
            loop {
                if let Some(control) = state.controls.pop_front() {
                    break Packet::Control(control);
                }
                if state.in_flight.is_none()
                    && let Some(frame) = state.latest.take()
                {
                    state.in_flight = Some(frame.sequence);
                    break Packet::Frame(frame);
                }
                if state.closed {
                    return Ok(());
                }
                state = output
                    .changed
                    .wait(state)
                    .unwrap_or_else(|p| p.into_inner());
            }
        };
        write_packet(&mut stdout, &packet)?;
        if let Packet::Frame(frame) = packet {
            let mut state = output.state.lock().unwrap_or_else(|p| p.into_inner());
            if state.pixels.len() < 2 {
                state.pixels.push(frame.pixels);
            }
        }
    }
}

type Geometry = Arc<Mutex<(u32, u32, u64)>>;

fn connect_video(endpoint: SocketAddr) -> io::Result<TcpStream> {
    if !endpoint.ip().is_loopback() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "video endpoint must be local",
        ));
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(mut stream) = TcpStream::connect_timeout(&endpoint, Duration::from_secs(1)) {
            stream.set_read_timeout(Some(Duration::from_secs(1)))?;
            let mut dummy = [1];
            if stream.read_exact(&mut dummy).is_ok() && dummy == [0] {
                stream.set_read_timeout(None)?;
                stream.set_nodelay(true)?;
                return Ok(stream);
            }
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "scrcpy server startup",
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

struct PresentationReader<'a, F> {
    socket: &'a mut TcpStream,
    tick: F,
}
impl<F: FnMut() -> io::Result<()>> Read for PresentationReader<'_, F> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        loop {
            (self.tick)()?;
            match self.socket.read(buffer) {
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                    ) =>
                {
                    continue;
                }
                result => return result,
            }
        }
    }
}

fn present_cached(
    decoder: &mut Option<decoder::Decoder>,
    output: &Output,
    busy: &Mutex<Option<Instant>>,
    presented: &mut (u64, u64),
    binding_generation: u64,
    generation: u64,
) -> io::Result<()> {
    let Some(epoch) = output.credit() else {
        return Ok(());
    };
    let Some(decoder) = decoder else {
        return Ok(());
    };
    let signature = (decoder.serial(), epoch);
    if decoder.serial() == 0 || signature == *presented {
        return Ok(());
    }
    *busy.lock().unwrap_or_else(|p| p.into_inner()) = Some(Instant::now());
    let frame = decoder.present(binding_generation, generation, output.take_pixels());
    *busy.lock().unwrap_or_else(|p| p.into_inner()) = None;
    if let Some(frame) = frame.map_err(io::Error::other)? {
        output.frame(frame);
        *presented = signature;
    }
    Ok(())
}

fn video_loop(
    mut video: TcpStream,
    output: Arc<Output>,
    geometry: Geometry,
    busy: Arc<Mutex<Option<Instant>>>,
    binding_generation: u64,
) -> io::Result<()> {
    let mut name = [0; 64];
    video.read_exact(&mut name)?;
    let mut codec = [0; 4];
    video.read_exact(&mut codec)?;
    if &codec != b"h264" {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "expected H.264"));
    }
    video.set_read_timeout(Some(Duration::from_millis(50)))?;
    let mut decoder = None;
    let mut presented = (0, 0);
    let mut config = Vec::new();
    let mut generation = 0;
    loop {
        let packet = protocol::read_video(&mut PresentationReader {
            socket: &mut video,
            tick: || {
                present_cached(
                    &mut decoder,
                    &output,
                    &busy,
                    &mut presented,
                    binding_generation,
                    generation,
                )
            },
        })?;
        match packet {
            protocol::VideoPacket::Session { width, height } => {
                generation += 1;
                presented = (0, 0);
                config.clear();
                *geometry.lock().unwrap_or_else(|p| p.into_inner()) = (width, height, generation);
                *busy.lock().unwrap_or_else(|p| p.into_inner()) = Some(Instant::now());
                decoder = Some(decoder::Decoder::new().map_err(io::Error::other)?);
                *busy.lock().unwrap_or_else(|p| p.into_inner()) = None;
                output.control(Control::Ready {
                    width,
                    height,
                    generation,
                });
            }
            protocol::VideoPacket::Media {
                config: is_config,
                pts,
                bytes,
            } => {
                if is_config {
                    if config.len() + bytes.len() > protocol::MAX_MEDIA_BYTES {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "codec config limit",
                        ));
                    }
                    config.extend_from_slice(&bytes);
                    continue;
                }
                let mut packet = std::mem::take(&mut config);
                packet.extend_from_slice(&bytes);
                if packet.len() > protocol::MAX_MEDIA_BYTES {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "codec packet limit",
                    ));
                }
                let decoder = decoder.as_mut().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "session header missing")
                })?;
                *busy.lock().unwrap_or_else(|p| p.into_inner()) = Some(Instant::now());
                let result = decoder.packet(&packet, pts);
                *busy.lock().unwrap_or_else(|p| p.into_inner()) = None;
                result.map_err(io::Error::other)?;
            }
        }
    }
}

struct Inputs {
    keys: HashSet<u32>,
    touch: Option<Control>,
    socket: TcpStream,
    geometry: Geometry,
}

impl Inputs {
    fn send(&mut self, control: &Control) -> io::Result<()> {
        if let Control::Touch {
            width,
            height,
            generation,
            ..
        }
        | Control::Scroll {
            width,
            height,
            generation,
            ..
        } = control
        {
            let current = *self.geometry.lock().unwrap_or_else(|p| p.into_inner());
            if current != (*width, *height, *generation) {
                self.release()?;
                return Ok(());
            }
        }
        let bytes = protocol::encode(control)?;
        self.socket.write_all(&bytes)?;
        match control {
            Control::Key {
                action: 0, keycode, ..
            } => {
                self.keys.insert(*keycode);
            }
            Control::Key {
                action: 1, keycode, ..
            } => {
                self.keys.remove(keycode);
            }
            Control::Touch { action: 0 | 2, .. } => self.touch = Some(control.clone()),
            Control::Touch { action: 1 | 3, .. } => self.touch = None,
            _ => {}
        }
        Ok(())
    }
    fn release(&mut self) -> io::Result<()> {
        let socket = &mut self.socket;
        for keycode in self.keys.drain() {
            socket.write_all(&protocol::encode(&Control::Key {
                action: 1,
                keycode,
                repeat: 0,
                metastate: 0,
            })?)?;
        }
        if let Some(Control::Touch {
            x,
            y,
            width,
            height,
            generation,
            ..
        }) = self.touch.take()
        {
            socket.write_all(&protocol::encode(&Control::Touch {
                action: 3,
                x,
                y,
                width,
                height,
                generation,
            })?)?;
        }
        Ok(())
    }
}

impl Drop for Inputs {
    fn drop(&mut self) {
        let _ = self.release();
        let _ = self.socket.shutdown(Shutdown::Both);
    }
}

fn run() -> io::Result<()> {
    ffmpeg_next::init().map_err(io::Error::other)?;
    ffmpeg_next::util::log::set_level(ffmpeg_next::util::log::Level::Quiet);
    write_packet(
        &mut io::stdout().lock(),
        &Packet::Control(Control::Hello {
            protocol: PROTOCOL_VERSION,
            version: env!("CARGO_PKG_VERSION").into(),
        }),
    )?;
    let mut stdin = io::stdin().lock();
    let Some(Packet::Control(Control::Connect {
        binding,
        endpoint,
        settings,
    })) = read_packet(&mut stdin)?
    else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "expected connect",
        ));
    };
    settings
        .validate()
        .map_err(|message| io::Error::new(io::ErrorKind::InvalidInput, message))?;
    let video = connect_video(endpoint)?;
    let control = TcpStream::connect_timeout(&endpoint, Duration::from_secs(5))?;
    control.set_nodelay(true)?;
    control.set_write_timeout(Some(Duration::from_millis(500)))?;
    let output = Arc::new(Output::default());
    let geometry = Arc::new(Mutex::new((0, 0, 0)));
    let busy = Arc::new(Mutex::new(None));
    let stopped = Arc::new(AtomicBool::new(false));
    let worker_output = Arc::clone(&output);
    let writer = nyaterm_core::runtime::worker::spawn("android-helper-output".into(), move || {
        output_loop(worker_output)
    })?;
    let video_shutdown = video.try_clone()?;
    let worker_output = Arc::clone(&output);
    let worker_geometry = Arc::clone(&geometry);
    let worker_busy = Arc::clone(&busy);
    let video_stopped = Arc::clone(&stopped);
    let video_worker =
        nyaterm_core::runtime::worker::spawn("android-helper-video".into(), move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                video_loop(
                    video,
                    Arc::clone(&worker_output),
                    worker_geometry,
                    worker_busy,
                    binding.generation,
                )
            }));
            let code = match result {
                Ok(Ok(())) => "VideoEnded",
                Ok(Err(_)) => "VideoFailed",
                Err(_) => "DecoderPanic",
            };
            if !video_stopped.load(Ordering::Acquire) {
                worker_output.control(Control::Fatal { code: code.into() });
            }
        })?;
    let clipboard_shutdown = control.try_clone()?;
    let mut reader = control.try_clone()?;
    let clipboard_output = Arc::clone(&output);
    let clipboard_worker =
        nyaterm_core::runtime::worker::spawn("android-helper-clipboard".into(), move || {
            while let Ok(message) = protocol::read_device_message(&mut reader) {
                if let Some(message) = message {
                    clipboard_output.control(message);
                }
            }
        })?;
    let worker_busy = Arc::clone(&busy);
    let worker_stopped = Arc::clone(&stopped);
    let worker_output = Arc::clone(&output);
    let watchdog =
        nyaterm_core::runtime::worker::spawn("android-helper-watchdog".into(), move || {
            while !worker_stopped.load(Ordering::Acquire) {
                let hung = worker_busy
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .is_some_and(|started| started.elapsed() > Duration::from_secs(5));
                if hung {
                    worker_output.control(Control::Fatal {
                        code: "DecoderHung".into(),
                    });
                    std::thread::sleep(Duration::from_millis(100));
                    std::process::exit(1);
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        })?;
    let mut inputs = Inputs {
        keys: HashSet::new(),
        touch: None,
        socket: control,
        geometry,
    };
    let result = (|| {
        while let Some(packet) = read_packet(&mut stdin)? {
            let Packet::Control(control) = packet else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "unexpected frame",
                ));
            };
            match control {
                Control::Ping { sequence } => output.control(Control::Pong { sequence }),
                Control::FrameAck { sequence } => output.acknowledge(sequence),
                Control::Presentation { visible } => output.presentation(visible),
                Control::ReleaseInputs => inputs.release()?,
                Control::Disconnect => {
                    inputs.release()?;
                    break;
                }
                Control::Key { .. }
                | Control::Touch { .. }
                | Control::Scroll { .. }
                | Control::Text { .. }
                | Control::Paste { .. }
                | Control::Rotate => {
                    if let Err(code) = control.validate_input() {
                        output.control(Control::InputRejected {
                            operation: control.input_operation().expect("input variant"),
                            code,
                        });
                    } else if let Err(error) = inputs.send(&control) {
                        if matches!(
                            error.kind(),
                            io::ErrorKind::InvalidData | io::ErrorKind::InvalidInput
                        ) {
                            output.control(Control::InputRejected {
                                operation: control.input_operation().expect("input variant"),
                                code: nyaterm_android::ipc::InputRejection::InvalidInput,
                            });
                        } else {
                            return Err(error);
                        }
                    }
                }
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "unexpected control",
                    ));
                }
            }
        }
        Ok(())
    })();
    drop(inputs);
    stopped.store(true, Ordering::Release);
    let _ = video_shutdown.shutdown(Shutdown::Both);
    let _ = clipboard_shutdown.shutdown(Shutdown::Both);
    let _ = video_worker.join();
    let _ = clipboard_worker.join();
    let _ = watchdog.join();
    output.control(Control::Disconnected);
    output.close();
    let _ = writer.join();
    result
}

fn main() {
    // Panic output is deliberately redacted; the parent receives a typed fatal error.
    std::panic::set_hook(Box::new(|_| {}));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(run));
    if !matches!(result, Ok(Ok(()))) {
        let code = if result.is_err() {
            "HelperPanic"
        } else {
            "HelperFailed"
        };
        let _ = write_packet(
            &mut io::stdout().lock(),
            &Packet::Control(Control::Fatal { code: code.into() }),
        );
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use crate::{Output, OutputState};
    use nyaterm_android::ipc::Frame;
    #[test]
    fn presentation_credit_replaces_pending_frames_without_unbounded_backlog() {
        let output = Output::default();
        for n in 0..1000 {
            output.frame(Frame {
                sequence: 0,
                binding_generation: 1,
                generation: 1,
                pts: n,
                width: 1,
                height: 1,
                stride: 4,
                pixels: vec![0; 4],
            });
        }
        let mut state = output.state.lock().unwrap();
        assert_eq!(state.latest.as_ref().unwrap().pts, 999);
        assert_eq!(state.sequence, 1000);
        state.in_flight = Some(1000);
        drop(state);
        output.acknowledge(7);
        assert_eq!(output.state.lock().unwrap().in_flight, Some(1000));
        output.acknowledge(1000);
        assert_eq!(output.state.lock().unwrap().in_flight, None);
        let _: OutputState = Default::default();
    }
    #[test]
    fn hidden_screen_decodes_without_conversion_and_resumes_cached_static_frame() {
        ffmpeg_next::init().unwrap();
        let mut decoder = Some(crate::decoder::Decoder::new().unwrap());
        decoder
            .as_mut()
            .unwrap()
            .packet(include_bytes!("../tests/fixtures/black.h264"), 1)
            .unwrap();
        assert!(decoder.as_ref().unwrap().serial() > 0);
        let output = Output::default();
        let busy = std::sync::Mutex::new(None);
        let mut presented = (0, 0);
        output.presentation(false);
        for _ in 0..100 {
            crate::present_cached(&mut decoder, &output, &busy, &mut presented, 1, 1).unwrap();
        }
        assert_eq!(decoder.as_ref().unwrap().conversions, 0);
        assert!(output.state.lock().unwrap().latest.is_none());
        output.presentation(true);
        crate::present_cached(&mut decoder, &output, &busy, &mut presented, 1, 1).unwrap();
        assert_eq!(decoder.as_ref().unwrap().conversions, 1);
        assert_eq!(output.state.lock().unwrap().latest.as_ref().unwrap().pts, 1);
        for _ in 0..100 {
            crate::present_cached(&mut decoder, &output, &busy, &mut presented, 1, 1).unwrap();
        }
        assert_eq!(decoder.as_ref().unwrap().conversions, 1);
        output.presentation(false);
        output.presentation(true);
        crate::present_cached(&mut decoder, &output, &busy, &mut presented, 1, 1).unwrap();
        assert_eq!(decoder.as_ref().unwrap().conversions, 2);
        assert!(output.state.lock().unwrap().pixels.len() <= 2);
    }
}
