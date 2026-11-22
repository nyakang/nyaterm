//! CPU baseline for received BGRA damage through immutable upload preparation.
//! Excludes decoder, OS pipes, worker pacing, GPUI/GPU, audio and network.
use std::hint::black_box;
use std::time::Instant;

use crate::spice::presentation::{Command, Displays};
use crate::{DirtyRect, PixelFormat, RemoteFrameEvent};

#[test]
#[ignore = "release-mode performance baseline; run with --ignored --nocapture"]
fn spice_compositor_upload_preparation_baseline() {
    for (name, width, height, channels, sparse) in [
        ("1080p_full", 1920u32, 1080u32, 1u32, false),
        ("4k_full", 3840, 2160, 1, false),
        ("4k_sparse", 3840, 2160, 1, true),
        ("dual_4k_one_visible", 3840, 2160, 2, false),
    ] {
        let mut displays = Displays::default();
        for channel in 0..channels {
            displays
                .apply(Command::Reset {
                    channel,
                    epoch: 1,
                    width,
                    height,
                })
                .unwrap();
            displays
                .surfaces
                .get_mut(&channel)
                .unwrap()
                .set_generation(1);
        }
        let mut samples = Vec::new();
        let mut upload_bytes = 0;
        for iteration in 0..65 {
            let start = Instant::now();
            for channel in 0..channels {
                let rect = if sparse {
                    DirtyRect {
                        x: width / 3,
                        y: height / 3,
                        width: 256,
                        height: 128,
                    }
                } else {
                    DirtyRect {
                        x: 0,
                        y: 0,
                        width,
                        height,
                    }
                };
                let rows = (4 * 1024 * 1024 / (rect.width * 4)).max(1);
                for y in (rect.y..rect.y + rect.height).step_by(rows as usize) {
                    let height = rows.min(rect.y + rect.height - y);
                    displays
                        .apply(Command::Frame(
                            channel,
                            RemoteFrameEvent::Bitmap {
                                epoch: 1,
                                full: false,
                                x: rect.x,
                                y,
                                width: rect.width,
                                height,
                                stride: rect.width * 4,
                                format: PixelFormat::Bgra8,
                                pixels: vec![
                                    iteration as u8;
                                    rect.width as usize * height as usize * 4
                                ]
                                .into(),
                            },
                        ))
                        .unwrap();
                }
            }
            let surface = displays.surfaces.get_mut(&0).unwrap();
            let frame = surface
                .prepare_crop(DirtyRect {
                    x: 0,
                    y: 0,
                    width,
                    height,
                })
                .unwrap();
            let bytes: usize = frame
                .regions
                .iter()
                .map(|region| black_box(&region.pixels).len())
                .sum();
            surface.acknowledge(frame.id);
            if iteration >= 5 {
                samples.push(start.elapsed().as_secs_f64() * 1000.0);
                upload_bytes = bytes;
            }
        }
        samples.sort_by(f64::total_cmp);
        println!(
            "{name}: mean_ms={:.3} p95_ms={:.3} upload_bytes={upload_bytes} samples={}",
            samples.iter().sum::<f64>() / samples.len() as f64,
            samples[(samples.len() as f64 * 0.95).ceil() as usize - 1],
            samples.len()
        );
    }
}
