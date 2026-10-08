//! Native, synthetic renderer probe. See docs/terminal-rendering.md.
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui::profiler::{FrameEvent, FrameTimingCollector, set_trace_enabled};
use gpui::{
    App, AppContext, Bounds, Context, Element, ElementId, GlobalElementId, InspectorElementId,
    IntoElement, LayoutId, ParentElement, Pixels, Render, Styled, Window, WindowBounds,
    WindowOptions, div, px, rgb, size,
};
use nyaterm_terminal::{TerminalScreen, TerminalSnapshot};
use nyaterm_terminal_gpui::{NyaTerminalElement, NyaTerminalLayoutCache, TerminalLineDecorations};
use serde_json::json;

const COLS: u16 = 120;
const ROWS: u16 = 40;
const CELL_W: f32 = 8.;
const CELL_H: f32 = 16.;
const WARMUP: usize = 60;
const SAMPLES: usize = 240;

#[derive(Default)]
struct Measurements {
    recording: bool,
    element: Vec<(u64, u64)>,
}

struct MeasuredTerminal {
    inner: NyaTerminalElement,
    measurements: Rc<RefCell<Measurements>>,
    prepaint_us: u64,
}

impl IntoElement for MeasuredTerminal {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for MeasuredTerminal {
    type RequestLayoutState = <NyaTerminalElement as Element>::RequestLayoutState;
    type PrepaintState = <NyaTerminalElement as Element>::PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        self.inner.request_layout(id, inspector, window, cx)
    }
    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let started = Instant::now();
        let plan = self
            .inner
            .prepaint(id, inspector, bounds, layout, window, cx);
        self.prepaint_us = started.elapsed().as_micros() as u64;
        plan
    }
    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        layout: &mut Self::RequestLayoutState,
        plan: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let started = Instant::now();
        self.inner
            .paint(id, inspector, bounds, layout, plan, window, cx);
        let paint_us = started.elapsed().as_micros() as u64;
        let mut measurements = self.measurements.borrow_mut();
        if measurements.recording {
            measurements.element.push((self.prepaint_us, paint_us));
        }
    }
}

struct Probe {
    snapshot: Arc<TerminalSnapshot>,
    cache: Arc<Mutex<NyaTerminalLayoutCache>>,
    measurements: Rc<RefCell<Measurements>>,
    running: Rc<Cell<bool>>,
    cell_graphics: bool,
    cold: bool,
    scale: Option<f32>,
}

impl Render for Probe {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        if let Some(scale) = self.scale {
            window.set_scale_factor(scale);
        }
        if self.running.get() {
            window.request_animation_frame();
        }
        if self.cold {
            self.cache.lock().unwrap().clear();
        }
        let palette = nyaterm_ui::theme_palette("github-dark");
        let inner = NyaTerminalElement::new(
            Arc::clone(&self.snapshot),
            Arc::new(Vec::new()),
            Vec::<TerminalLineDecorations>::new(),
            false,
            "block",
            CELL_W,
            CELL_H,
            palette,
            "Consolas".into(),
            12.,
            400.,
            700.,
        )
        .with_layout_cache(Arc::clone(&self.cache))
        .with_cell_glyph_rendering(self.cell_graphics);
        div()
            .size_full()
            .bg(rgb(palette.terminal_bg))
            .child(MeasuredTerminal {
                inner,
                measurements: Rc::clone(&self.measurements),
                prepaint_us: 0,
            })
    }
}

fn fixture(kind: &str, cell_graphics: bool) -> Arc<TerminalSnapshot> {
    let mut screen = TerminalScreen::new(COLS, ROWS);
    for row in 0..ROWS {
        screen.advance(format!("\x1b[{};1H", row + 1).as_bytes());
        let text = match kind {
            "shade" => "▓".repeat(usize::from(COLS)),
            "blocks" => "█▀▄─".repeat(usize::from(COLS) / 4),
            "text" => format!(
                "row {row:02}  user@nyaterm  0123456789  CPU 25%  memory 512 MiB  /synthetic/path"
            ),
            "mixed" => {
                format!("│ row {row:02}  CPU ▓▓▓▓░░░░  █▀▄  ───────  user@nyaterm  512 MiB │")
            }
            "decorations" => match row % 4 {
                0 => "TEXT\x1b[4;31m        \x1b[0m",
                1 => "\x1b[9;31m█▀──\x1b[0m",
                2 => "█\u{301}▀\u{fe0f} 中X",
                _ => "╔═╦═╗ ░▒▓ █▀▄",
            }
            .into(),
            _ => panic!("fixture must be text, mixed, blocks, shade, or decorations"),
        };
        screen.advance(text.as_bytes());
    }
    let snapshot = screen.snapshot();
    if kind == "decorations" {
        assert!(
            snapshot.cell(0, 4).unwrap().style.underline,
            "fixture must retain underlined cells"
        );
        if cell_graphics {
            assert!(
                snapshot
                    .row(0)
                    .unwrap()
                    .styled_spans
                    .iter()
                    .any(|span| span.style.underline),
                "current Snapshot must retain styled tails; rebuild with isolated target directories"
            );
        }
    }
    Arc::new(snapshot)
}

fn distribution(values: impl Iterator<Item = u64>) -> serde_json::Value {
    let mut values: Vec<_> = values.collect();
    values.sort_unstable();
    if values.is_empty() {
        return json!(null);
    }
    let percentile =
        |percent: usize| values[(values.len() * percent).div_ceil(100).saturating_sub(1)];
    json!({"count": values.len(), "p50_us": percentile(50), "p95_us": percentile(95), "max_us": values[values.len() - 1]})
}

fn verify_decoration_scene(window: &Window) {
    let scale = window.scale_factor();
    let quads = window.painted_quads();
    let strikes = window.painted_underlines();
    assert_eq!(strikes.len(), usize::from(ROWS) / 4);
    for row in (0..ROWS).step_by(4) {
        // GPUI snaps half-pixel edges toward zero (all probe coordinates are positive).
        let underline_y = (((f32::from(row) * CELL_H + CELL_H - 2.) * scale) - 0.5).ceil();
        assert!(
            quads.iter().any(|quad| {
                quad.bounds.origin.x.0 == 4. * CELL_W * scale
                    && quad.bounds.origin.y.0 == underline_y
                    && quad.bounds.size.width.0 == 8. * CELL_W * scale
            }),
            "trailing underline missing on row {row}, scale={scale}, quads={:?}",
            quads
                .iter()
                .filter(|quad| quad.bounds.size.height.0 <= 3. && quad.bounds.size.width.0 > 20.)
                .map(|quad| quad.bounds)
                .collect::<Vec<_>>()
        );
        let top = f32::from(row + 1) * CELL_H * scale;
        let glyph = quads
            .iter()
            .find(|quad| {
                quad.bounds.origin.x.0 == 0.
                    && quad.bounds.origin.y.0 == top
                    && quad.bounds.size.width.0 == CELL_W * scale
                    && quad.bounds.size.height.0 == CELL_H * scale
            })
            .expect("full-block glyph quad");
        let strike = strikes
            .iter()
            .find(|strike| {
                strike.bounds.origin.y.0 >= top && strike.bounds.origin.y.0 < top + CELL_H * scale
            })
            .expect("strikethrough above full-block glyph");
        assert!(
            strike.order > glyph.order,
            "strike must paint after the overlapping glyph"
        );
    }
}

fn main() {
    // Separate processes avoid sharing the text/sprite atlas between A and B.
    let args: Vec<String> = std::env::args().skip(1).collect();
    assert!(
        (2..=5).contains(&args.len()),
        "usage: renderer_probe <cell|font> <fixture> [hot|cold] [native|scale] [screenshot.png]"
    );
    assert!(matches!(args[0].as_str(), "cell" | "font"));
    let cell_graphics = args[0] == "cell";
    let kind = args[1].clone();
    let cold = match args.get(2).map(String::as_str).unwrap_or("hot") {
        "hot" => false,
        "cold" => true,
        _ => panic!("cache must be hot or cold"),
    };
    let scale: Option<f32> = args
        .get(3)
        .filter(|scale| scale.as_str() != "native")
        .map(|scale| scale.parse().expect("numeric scale"));
    assert!(scale.is_none_or(|scale| scale.is_finite() && (1.0..=2.0).contains(&scale)));
    let screenshot_path = args.get(4).cloned();
    let snapshot = fixture(&kind, cell_graphics);
    let cache = Arc::new(Mutex::new(NyaTerminalLayoutCache::default()));
    let measurements = Rc::new(RefCell::new(Measurements::default()));
    let running = Rc::new(Cell::new(true));
    set_trace_enabled(true);
    gpui_platform::application().run(move |cx: &mut App| {
        let window = cx.open_window(WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(f32::from(COLS) * CELL_W), px(f32::from(ROWS) * CELL_H)), cx))),
            show: true, focus: false, inactive_frame_interval: None, ..Default::default()
        }, |window, cx| {
            if let Some(scale) = scale {
                let ratio = scale / window.scale_factor();
                window.resize(size(px(f32::from(COLS) * CELL_W * ratio), px(f32::from(ROWS) * CELL_H * ratio)));
            }
            cx.new(|_| Probe { snapshot, cache: Arc::clone(&cache), measurements: Rc::clone(&measurements), running: Rc::clone(&running), cell_graphics, cold, scale })
        }).expect("native GPU window");
        cx.spawn(async move |cx| {
            let mut collector = FrameTimingCollector::new();
            let mut warmup_draws = 0;
            let mut draws = Vec::new();
            let mut presents = Vec::new();
            let mut dirty_to_present = Vec::new();
            let mut last_dirty = None;
            let deadline = Instant::now() + Duration::from_secs(30);
            while warmup_draws < WARMUP {
                cx.background_executor().timer(Duration::from_millis(10)).await;
                assert!(Instant::now() < deadline, "native warmup timed out");
                warmup_draws += collector.collect_unseen().iter().filter(|event| matches!(event, FrameEvent::Draw(_))).count();
            }
            measurements.borrow_mut().recording = true;
            while draws.len() < SAMPLES || measurements.borrow().element.len() < SAMPLES {
                cx.background_executor().timer(Duration::from_millis(10)).await;
                assert!(Instant::now() < deadline, "native frame loop timed out");
                for event in collector.collect_unseen() {
                    match event {
                        FrameEvent::Draw(timing) => {
                            draws.push(timing.draw_duration().as_micros() as u64);
                            last_dirty = timing.dirty_at;
                        }
                        FrameEvent::Present(timing) => {
                            presents.push(timing.present_duration().as_micros() as u64);
                            if let Some(dirty) = last_dirty.take() {
                                dirty_to_present.push(timing.present_end.duration_since(dirty).as_micros() as u64);
                            }
                        }
                    }
                }
            }
            running.set(false);
            measurements.borrow_mut().recording = false;
            // Native readback is outside measured frames. Encode/write on a
            // background thread, never inside the renderer or GPUI callback.
            let (image, actual_scale, gpu) = window.update(cx, |_, window, _| {
                (screenshot_path.as_ref().map(|_| window.render_to_image().expect("native GPU readback")), window.scale_factor(), window.gpu_specs())
            }).unwrap();
            if let Some(image) = image {
                let path = screenshot_path.unwrap();
                let required_width = (f32::from(COLS) * CELL_W * actual_scale).round() as u32;
                let required_height = (f32::from(ROWS) * CELL_H * actual_scale).round() as u32;
                assert!(image.width() >= required_width && image.height() >= required_height, "capture clipped the fixture: {}x{}, expected at least {required_width}x{required_height}", image.width(), image.height());
                cx.background_executor().spawn(async move { image.save(path).expect("save probe screenshot"); }).await;
            }
            if cell_graphics && kind == "decorations" {
                window.update(cx, |_, window, _| verify_decoration_scene(window)).unwrap();
            }
            let measurements = measurements.borrow();
            println!("{}", json!({
                "mode": if cell_graphics { "cell" } else { "font-control" }, "fixture": kind,
                "cache": if cold { "cold" } else { "hot" }, "cols": COLS, "rows": ROWS,
                "font": "Consolas", "font_size": 12, "cell_width": CELL_W, "cell_height": CELL_H,
                "scale": actual_scale, "forced_scale": scale, "gpu": gpu,
                "prepaint": distribution(measurements.element.iter().take(SAMPLES).map(|sample| sample.0)),
                "paint": distribution(measurements.element.iter().take(SAMPLES).map(|sample| sample.1)),
                "draw": distribution(draws.into_iter().take(SAMPLES)),
                "platform_submit_cpu": distribution(presents.into_iter().take(SAMPLES)),
                "dirty_to_present": distribution(dirty_to_present.into_iter().take(SAMPLES)),
                "layout_cache": format!("{:?}", cache.lock().unwrap().stats()),
                "note": "synthetic native window; submission is CPU time, not GPU execution or display latency; font-control is not a historical build"
            }));
            cx.update(|cx| cx.quit());
        }).detach();
    });
}
