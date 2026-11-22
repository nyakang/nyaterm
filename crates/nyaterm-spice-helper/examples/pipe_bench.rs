//! Two-process software pipeline, including real OS pipes and consumer ack.
//! Excludes native decode, network, audio and GPU; this is latency, not UI FPS.

use std::io::{self, BufReader, BufWriter, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::Instant;

use nyaterm_remote_desktop::spice::display::SPICE_FRAMEBUFFER_LIMITS;
use nyaterm_remote_desktop::spice::ipc::{
    decode_control, decode_frame, encode_control, encode_frame,
};
use nyaterm_remote_desktop::spice::protocol::SpiceControl;
use nyaterm_remote_desktop::{
    DirtyRect, Framebuffer, Packet, PacketType, read_packet, write_packet, write_packet_into,
};
use nyaterm_spice_helper::frame::{DamageTracker, FRAME_CHUNK_BYTES, SurfaceFormat, SurfaceLayout};
use serde_json::json;

struct Consumer {
    child: Child,
    input: Option<BufWriter<ChildStdin>>,
    output: BufReader<ChildStdout>,
    pipe_capacity: Option<usize>,
}

impl Consumer {
    fn spawn(width: u32, height: u32, displays: usize) -> io::Result<Self> {
        let mut child = Command::new(std::env::current_exe()?)
            .args([
                "--consumer".into(),
                width.to_string(),
                height.to_string(),
                displays.to_string(),
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let input = child.stdin.take().unwrap();
        #[cfg(target_os = "linux")]
        let pipe_capacity = if std::env::var_os("NYATERM_BENCH_DEFAULT_PIPE").is_none() {
            Some(nyaterm_spice_helper::pipe_capacity::configure_frame_pipe(
                &input,
            )?)
        } else {
            None
        };
        #[cfg(not(target_os = "linux"))]
        let pipe_capacity = None;
        let input = Some(BufWriter::with_capacity(256 * 1024, input));
        let output = BufReader::new(child.stdout.take().unwrap());
        Ok(Self {
            child,
            input,
            output,
            pipe_capacity,
        })
    }

    fn send(&mut self, packet: &Packet) -> io::Result<()> {
        write_packet_into(self.input.as_mut().unwrap(), packet)
    }

    fn acknowledge(&mut self, sequence: u64) -> io::Result<()> {
        self.send(&encode_control(&SpiceControl::Ping { sequence })?)?;
        self.input.as_mut().unwrap().flush()?;
        let packet =
            read_packet(&mut self.output)?.ok_or_else(|| io::Error::other("consumer exited"))?;
        if decode_control(&packet)? != (SpiceControl::Pong { sequence }) {
            return Err(io::Error::other("invalid consumer acknowledgement"));
        }
        Ok(())
    }
}

impl Drop for Consumer {
    fn drop(&mut self) {
        self.input.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn consume(width: u32, height: u32, displays: usize) -> Result<(), Box<dyn std::error::Error>> {
    let mut surfaces = (0..displays)
        .map(|_| Framebuffer::new(1, width, height, SPICE_FRAMEBUFFER_LIMITS))
        .collect::<Result<Vec<_>, _>>()?;
    let mut input = BufReader::new(io::stdin());
    let mut output = io::stdout();
    while let Some(packet) = read_packet(&mut input)? {
        match packet.packet_type {
            PacketType::Frame => {
                let (channel, frame) = decode_frame(packet)?;
                surfaces
                    .get_mut(channel as usize)
                    .ok_or("invalid display")?
                    .apply(&frame)?;
            }
            PacketType::Control => {
                let SpiceControl::Ping { sequence } = decode_control(&packet)? else {
                    return Err("unexpected control".into());
                };
                write_packet(
                    &mut output,
                    &encode_control(&SpiceControl::Pong { sequence })?,
                )?;
            }
            _ => return Err("unexpected packet".into()),
        }
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.get(1).is_some_and(|arg| arg == "--consumer") {
        return consume(args[2].parse()?, args[3].parse()?, args[4].parse()?);
    }
    for (name, width, height, displays, sparse) in [
        ("1080p_full", 1920, 1080, 1, false),
        ("4k_full", 3840, 2160, 1, false),
        ("4k_sparse", 3840, 2160, 1, true),
        ("dual_4k_full", 3840, 2160, 2, false),
    ] {
        let layout = SurfaceLayout {
            width,
            height,
            stride: (width * 4) as i32,
            format: SurfaceFormat::Xrgb32,
        };
        let mut pixels = vec![0x42; layout.source_bytes()?];
        let mut consumer = Consumer::spawn(width, height, displays)?;
        consumer.acknowledge(0)?;
        let mut elapsed = Vec::new();
        let mut conversion = Vec::new();
        let mut bytes = 0usize;
        for sequence in 1..=65 {
            pixels[0] = sequence as u8;
            let start = Instant::now();
            let mut conversion_seconds = 0.0;
            let mut frame_bytes = 0;
            for channel in 0..displays {
                let mut damage = DamageTracker::new(width, height)?;
                if sparse {
                    damage.mark(DirtyRect {
                        x: width / 3,
                        y: height / 3,
                        width: 256,
                        height: 128,
                    })?;
                } else {
                    damage.mark_full();
                }
                while let Some(rect) = damage.take_chunk(FRAME_CHUNK_BYTES)? {
                    let conversion_start = Instant::now();
                    let frame = layout.bitmap(&pixels, rect, 1)?;
                    conversion_seconds += conversion_start.elapsed().as_secs_f64();
                    let packet = encode_frame(channel as u32, frame)?;
                    frame_bytes += packet.encoded_len()?;
                    consumer.send(&packet)?;
                }
            }
            consumer.acknowledge(sequence)?;
            if sequence > 5 {
                elapsed.push(start.elapsed().as_secs_f64());
                conversion.push(conversion_seconds);
                bytes += frame_bytes;
            }
        }
        elapsed.sort_by(f64::total_cmp);
        let seconds: f64 = elapsed.iter().sum();
        println!(
            "{}",
            json!({
                "scenario": name, "scope": "CPU conversion + actual OS pipe + second-process framebuffer + acknowledgement; excludes native decode, network, audio, GPU",
                "optimized_build": !cfg!(debug_assertions), "os": std::env::consts::OS,
                "architecture": std::env::consts::ARCH, "iterations": elapsed.len(), "display_count": displays,
                "width": width, "height": height, "mean_ms": seconds * 1000.0 / elapsed.len() as f64,
                "p50_ms": elapsed[elapsed.len() / 2] * 1000.0, "p95_ms": elapsed[elapsed.len() * 95 / 100] * 1000.0,
                "conversion_mean_ms": conversion.iter().sum::<f64>() * 1000.0 / conversion.len() as f64,
                "serialized_mib_per_second": bytes as f64 / 1_048_576.0 / seconds,
                "max_frame_chunk_bytes": FRAME_CHUNK_BYTES,
                "pipe_capacity_bytes": consumer.pipe_capacity,
            })
        );
    }
    Ok(())
}
