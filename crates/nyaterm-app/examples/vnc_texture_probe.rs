//! Synthetic native GPU upload probe. Does not connect to or capture user sessions.
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::profiler::{FrameEvent, FrameTimingCollector, set_trace_enabled};
use gpui::{
    App, AppContext, Bounds, Context, DevicePixels, DynamicTexture, DynamicTextureUpdate,
    IntoElement, OwnedTextureBytes, ParentElement, Render, Styled, Window, WindowBounds,
    WindowOptions, canvas, div, point, px, size,
};
use serde_json::json;

struct Probe {
    texture: DynamicTexture,
}

impl Render for Probe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let texture = self.texture;
        div().size_full().child(
            canvas(
                |bounds, _, _| bounds,
                move |_, bounds, window, _| {
                    window.paint_dynamic_texture(bounds, texture).unwrap();
                },
            )
            .size_full(),
        )
    }
}

fn distribution(mut samples: Vec<u64>) -> serde_json::Value {
    samples.sort_unstable();
    if samples.is_empty() {
        return json!({"samples": 0});
    }
    json!({"samples": samples.len(), "p50_us": samples[samples.len()/2],
        "p95_us": samples[(samples.len()-1)*95/100], "max_us": samples[samples.len()-1]})
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    let width: u32 = args.get(1).map_or(1920, |s| s.parse().unwrap());
    let height: u32 = args.get(2).map_or(1080, |s| s.parse().unwrap());
    let column = args.get(3).is_some_and(|s| s == "column");
    let owned = args.get(4).is_none_or(|s| s == "owned");
    let samples: usize = args.get(5).map_or(120, |s| s.parse().unwrap());
    assert!(width >= 256 && height >= 256 && width <= 7680 && height <= 4320 && samples > 0);
    // Preparation stays outside GPUI; repeated immutable uploads isolate atlas-copy cost.
    let initial: OwnedTextureBytes =
        Arc::new([0, 0, 0, 255].repeat(width as usize * height as usize));
    let patch_width = if column { 128 } else { width };
    let patch: OwnedTextureBytes =
        Arc::new([0x10, 0x80, 0xf0, 255].repeat(patch_width as usize * height as usize));
    let dimensions = size(DevicePixels(width as i32), DevicePixels(height as i32));
    let region = Bounds::new(
        point(DevicePixels(0), DevicePixels(0)),
        size(
            DevicePixels(patch_width as i32),
            DevicePixels(height as i32),
        ),
    );
    set_trace_enabled(true);
    gpui_platform::application().run(move |cx: &mut App| {
        let window = cx.open_window(WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(width as f32), px(height as f32)), cx))),
            show: true, focus: true, inactive_frame_interval: None, ..Default::default()
        }, |window, cx| {
            let texture = if owned { window.create_dynamic_texture_owned(dimensions, initial).unwrap() }
                else { window.create_dynamic_texture(dimensions, initial.as_ref().as_ref(), width * 4).unwrap() };
            cx.new(|_| Probe { texture })
        }).expect("native GPU window");
        cx.spawn(async move |cx| {
            let mut collector = FrameTimingCollector::new();
            let mut upload_cpu = Vec::new();
            let mut draws = Vec::new();
            let mut presents = Vec::new();
            let started = Instant::now();
            for iteration in 0..samples + 30 {
                cx.background_executor().timer(Duration::from_millis(16)).await;
                let elapsed = window.update(cx, |probe, window, cx| {
                    let started = Instant::now();
                    if owned {
                        window.update_dynamic_texture_owned_batch(probe.texture, vec![DynamicTextureUpdate {
                            bounds: region, bytes: Arc::clone(&patch), bytes_per_row: patch_width * 4,
                        }]).unwrap();
                    } else {
                        window.update_dynamic_texture(probe.texture, region, patch.as_ref().as_ref(), patch_width * 4).unwrap();
                    }
                    let elapsed = started.elapsed().as_micros() as u64;
                    cx.notify();
                    elapsed
                }).unwrap();
                for event in collector.collect_unseen() {
                    if iteration >= 30 {
                        match event {
                            FrameEvent::Draw(timing) => draws.push(timing.draw_duration().as_micros() as u64),
                            FrameEvent::Present(timing) => presents.push(timing.present_duration().as_micros() as u64),
                        }
                    }
                }
                if iteration >= 30 { upload_cpu.push(elapsed); }
                assert!(started.elapsed() < Duration::from_secs(60), "native probe timed out");
            }
            // Readback is outside timing samples. Verify channel order and untouched pixels.
            let (image, gpu) = window.update(cx, |_, window, _| (window.render_to_image().unwrap(), window.gpu_specs())).unwrap();
            let y = image.height()/2;
            let x = ((u64::from(image.width()) * u64::from(patch_width/2)) / u64::from(width)) as u32;
            assert_eq!(image.get_pixel(x, y).0, [0xf0, 0x80, 0x10, 255]);
            if column { assert_eq!(image.get_pixel(image.width()/2, y).0, [0, 0, 0, 255]); }
            println!("{}", json!({"mode": if owned { "owned" } else { "borrowed" },
                "fixture": if column { "column" } else { "full" }, "width": width, "height": height,
                "gpu": gpu, "upload_cpu": distribution(upload_cpu), "draw_cpu": distribution(draws),
                "present_cpu": distribution(presents), "readback_valid": true,
                "note": "synthetic native window; CPU submission timing is not display latency; excludes helper and composition"}));
            cx.update(|cx| cx.quit());
        }).detach();
    });
}
