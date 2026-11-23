//! CPU pipeline baseline; deliberately excludes network decode and GPU upload.
//! Run: cargo run --release -p nyaterm-spice-helper --example pipeline_bench

use std::hint::black_box;
use std::io::Cursor;
use std::time::Instant;

use nyaterm_remote_desktop::spice::display::SPICE_FRAMEBUFFER_LIMITS;
use nyaterm_remote_desktop::spice::ipc::{decode_frame, encode_frame};
use nyaterm_remote_desktop::{DirtyRect, Framebuffer, read_packet, write_packet_into};
use nyaterm_spice_helper::frame::{DamageTracker, FRAME_CHUNK_BYTES, SurfaceFormat, SurfaceLayout};
use serde_json::json;

fn main() -> Result<(), Box<dyn std::error::Error>> {
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
        let mut source = vec![0x42; layout.source_bytes()?];
        let mut framebuffers: Vec<_> = (0..displays)
            .map(|_| Framebuffer::new(1, width, height, SPICE_FRAMEBUFFER_LIMITS))
            .collect::<Result<_, _>>()?;
        let mut wire = Vec::with_capacity(FRAME_CHUNK_BYTES + 256);
        let iterations = 60;
        let mut samples = Vec::new();
        let mut total_bytes = 0u64;
        for frame in 0..iterations + 5 {
            source[0] = frame as u8;
            let start = Instant::now();
            let mut bytes = 0;
            for (channel, framebuffer) in framebuffers.iter_mut().enumerate() {
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
                    let event = layout.bitmap(black_box(&source), rect, 1)?;
                    let packet = encode_frame(channel as u32, event)?;
                    wire.clear();
                    write_packet_into(&mut wire, &packet)?;
                    bytes += wire.len() as u64;
                    let packet =
                        read_packet(&mut Cursor::new(black_box(&wire)))?.ok_or("missing frame")?;
                    let (received_channel, event) = decode_frame(packet)?;
                    assert_eq!(received_channel, channel as u32);
                    framebuffer.apply(&event)?;
                    black_box(framebuffer.pixels());
                }
            }
            if frame >= 5 {
                samples.push(start.elapsed().as_secs_f64());
                total_bytes += bytes;
            }
        }
        samples.sort_by(f64::total_cmp);
        let seconds: f64 = samples.iter().sum();
        println!(
            "{}",
            json!({
                "scenario": name,
                "scope": "CPU conversion + binary serialization + in-memory read + framebuffer apply; excludes native decode, OS pipes, GPU",
                "optimized_build": !cfg!(debug_assertions),
                "os": std::env::consts::OS,
                "architecture": std::env::consts::ARCH,
                "iterations": iterations,
                "display_count": displays,
                "width": width, "height": height,
                "mean_ms": seconds * 1000.0 / iterations as f64,
                "p50_ms": samples[samples.len() / 2] * 1000.0,
                "p95_ms": samples[(samples.len() * 95 / 100).min(samples.len() - 1)] * 1000.0,
                "updates_per_second": iterations as f64 / seconds,
                "serialized_mib_per_second": total_bytes as f64 / 1_048_576.0 / seconds,
                "max_frame_chunk_bytes": FRAME_CHUNK_BYTES,
            })
        );
    }
    Ok(())
}
