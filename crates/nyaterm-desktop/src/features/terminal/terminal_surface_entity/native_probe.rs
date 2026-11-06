//! Opt-in native-window measurements over the real surface and background scheduler.
use super::{TerminalScrollVisualState, TerminalSurface, TerminalSurfaceFrameSnapshot};
use gpui::profiler::{FrameEvent, FrameTimingCollector, set_trace_enabled};
use gpui::{
    App, AppContext, Bounds, Context, Entity, IntoElement, ParentElement, Render, Styled, Window,
    WindowBounds, WindowOptions, div, font, px, size,
};
use nyaterm_terminal::{TerminalScreen, TerminalSnapshot};
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

const COLS: u16 = 120;
const ROWS: u16 = 40;
const WARMUP: usize = 40;
const SAMPLES: usize = 300;

struct Probe {
    surface: Entity<TerminalSurface>,
}
impl Render for Probe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.surface.clone())
    }
}

fn install(surface: &mut TerminalSurface, snapshot: Arc<TerminalSnapshot>) {
    let offset = snapshot.display_offset;
    let state = TerminalScrollVisualState {
        history: snapshot.history,
        session_id: "synthetic-highlight-probe".into(),
        scroll_offset: offset,
        scroll_residual_lines: 0.,
        display_offset: offset,
        scrollback_len: snapshot.scrollback_len,
        viewport_rows: snapshot.viewport_rows,
        has_new_while_scrolled: false,
        performance_overlay: None,
        skipped_output_chars: 0,
    };
    surface.apply_frame_snapshot(TerminalSurfaceFrameSnapshot::new(snapshot, state));
}

fn distribution(mut values: Vec<u64>) -> serde_json::Value {
    if values.is_empty() {
        return serde_json::json!({"count": 0});
    }
    values.sort_unstable();
    let percentile = |p: usize| values[(values.len() - 1) * p / 100];
    serde_json::json!({"count": values.len(), "p50_us": percentile(50), "p95_us": percentile(95), "p99_us": percentile(99), "max_us": values[values.len()-1]})
}

#[test]
#[ignore = "manual native GPU benchmark; run one test per process with --nocapture"]
fn keyword_highlight_native_probe() {
    crate::preload_i18n().expect("preload fixture UI");
    let case = std::env::var("NYATERM_HIGHLIGHT_PROBE_CASE").unwrap_or_else(|_| "typing".into());
    assert!(matches!(case.as_str(), "typing" | "output" | "scroll"));
    let roles = std::env::var("NYATERM_HIGHLIGHT_PROBE_ROLES").is_ok_and(|value| value == "1");
    let theme =
        std::env::var("NYATERM_HIGHLIGHT_PROBE_THEME").unwrap_or_else(|_| "github-dark".into());
    let screenshot = std::env::var("NYATERM_HIGHLIGHT_PROBE_SCREENSHOT").ok();
    let palette = nyaterm_ui::theme_palette(&theme);
    set_trace_enabled(true);
    gpui_platform::application().run(move |cx: &mut App| {
        nyaterm_ui::apply_component_theme(palette, font("Consolas"), px(12.), cx);
        let surface = cx.new(|_| {
            let mut surface = TerminalSurface::new("synthetic-highlight-probe");
            surface.line_roles = roles; // HISTORICAL ROLE ADAPTER
            surface.palette = palette;
            surface.cell_width = 8.;
            surface.cell_height = 16.;
            surface.font_family = "Consolas".into();
            surface.font_size = 12.;
            surface.keyword_rules = Arc::new(nyaterm_core::keyword_highlight_presets::merge_keyword_highlight_rules_for_paint_with_ansi(&[], &std::collections::HashMap::new(), theme != "github-light", palette.terminal_ansi));
            surface
        });
        let host_surface = surface.clone();
        let window = cx.open_window(WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(960.), px(640.)), cx))),
            show: true, focus: false, inactive_frame_interval: None, ..Default::default()
        }, move |_, cx| cx.new(|_| Probe { surface: host_surface })).expect("native GPU window");
        cx.spawn(async move |cx| {
            let mut screen = TerminalScreen::new(COLS, ROWS);
            for index in 0..2200 {
                screen.advance(format!("2026-11-06 10:00:00 INFO worker={index} ERROR 2001:db8::1 128MiB\r\n").as_bytes());
            }
            let mut collector = FrameTimingCollector::new();
            let mut handler_us = Vec::new();
            let mut draw_us = Vec::new();
            let mut submit_us = Vec::new();
            let mut frame_to_present = Vec::new();
            let mut latest_draw_start = None;
            let mut pending = VecDeque::new();
            let mut highlight_ready_us = Vec::new();
            let mut highlight_pending = None;
            let started = Instant::now();
            for step in 0..WARMUP + SAMPLES {
                cx.background_executor().timer(Duration::from_millis(16)).await;
                assert!(started.elapsed() < Duration::from_secs(60), "native probe timed out");
                for event in collector.collect_unseen() {
                    match event {
                        FrameEvent::Draw(timing) => {
                            latest_draw_start = Some(timing.draw_start);
                            if step >= WARMUP { draw_us.push(timing.draw_duration().as_micros() as u64); }
                        },
                        FrameEvent::Present(timing) => {
                            if step >= WARMUP { submit_us.push(timing.present_duration().as_micros() as u64); }
                            while pending.front().is_some_and(|&(at, _)| latest_draw_start.is_some_and(|draw| at <= draw)) {
                                let (at, record): (Instant, bool) = pending.pop_front().unwrap();
                                if record { frame_to_present.push(timing.present_end.duration_since(at).as_micros() as u64); }
                            }
                        }
                    }
                }
                surface.read_with(cx, |surface, _| {
                    if let Some(at) = highlight_pending {
                        let ready = surface.keyword_highlights.as_ref().zip(surface.snapshot.as_ref()).is_some_and(|(highlights, snapshot)| highlights.matches_snapshot_rows(snapshot, surface.palette, 0..snapshot.row_count()));
                        if ready {
                            if step >= WARMUP { highlight_ready_us.push(Instant::now().duration_since(at).as_micros() as u64); }
                            highlight_pending = None;
                        }
                    }
                });
                if case != "scroll" {
                    screen.advance(b"\x1b]133;C\x07\r\n");
                    for line in 0..16 {
                        screen.advance(format!("INFO batch={step} row={line} 10.0.0.1 128MiB\r\n").as_bytes());
                    }
                    if case == "typing" {
                        screen.advance(format!("\x1b]133;A\x07$ \x1b]133;B\x07echo 中文{step} --option=\"value\"").as_bytes());
                    }
                }
                let snapshot = Arc::new(screen.viewport_snapshot(if case == "scroll" { step % 1000 } else { 0 }));
                let at = Instant::now();
                window.update(cx, |_, _, cx| {
                    surface.update(cx, |surface, cx| {
                        install(surface, snapshot);
                        surface.schedule_keyword_highlights(true, case != "scroll", cx);
                        cx.notify();
                    });
                }).unwrap();
                if step >= WARMUP { handler_us.push(at.elapsed().as_micros() as u64); }
                pending.push_back((at, step >= WARMUP));
                highlight_pending = Some(at);
            }
            // Include settling time after output stops; do not cancel unfinished work.
            let settle_started = Instant::now();
            loop {
                cx.background_executor().timer(Duration::from_millis(2)).await;
                let ready = surface.read_with(cx, |surface, _| surface.keyword_highlight_task.is_none() && surface.keyword_highlight_deferred_task.is_none());
                if ready { break; }
                assert!(settle_started.elapsed() < Duration::from_secs(5), "highlight worker did not settle");
            }
            let settle_us = settle_started.elapsed().as_micros() as u64;
            assert!(surface.read_with(cx, |surface, _| surface.keyword_highlights.as_ref().zip(surface.snapshot.as_ref()).is_some_and(|(highlights, snapshot)| highlights.matches_snapshot_rows(snapshot, surface.palette, 0..snapshot.row_count()))), "final snapshot did not converge");
            let image = window.update(cx, |_, window, _| screenshot.as_ref().map(|_| window.render_to_image().expect("native screenshot"))).unwrap();
            if let Some(image) = image {
                let path = screenshot.unwrap();
                cx.background_executor().spawn(async move { image.save(path).expect("save native fixture"); }).await;
            }
            println!("{}", serde_json::json!({
                "case": case, "roles": roles, "theme": theme, "samples": SAMPLES,
                "handler": distribution(handler_us), "draw": distribution(draw_us),
                "platform_submit_cpu": distribution(submit_us),
                "synthetic_frame_to_present": distribution(frame_to_present),
                "highlight_ready_poll": distribution(highlight_ready_us), "final_settle_us": settle_us,
                "fixture_rows_per_second": if case == "scroll" { 0.0 } else { 16.0 * (WARMUP + SAMPLES) as f64 / started.elapsed().as_secs_f64() },
                "note": "synthetic terminal frames through real TerminalSurface and native GPUI; 16ms highlight polling; presentation submission is not physical keyboard-to-display latency"
            }));
            cx.update(|cx| cx.quit());
        }).detach();
    });
}
