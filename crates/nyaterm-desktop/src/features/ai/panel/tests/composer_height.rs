use std::path::Path;
use std::time::Instant;

use gpui::{
    AppContext as _, Entity, MouseButton, MouseMoveEvent, Pixels, ScrollDelta, ScrollEvent,
    TestAppContext, VisualTestContext, point, px,
};

use super::{CompactAiHost, compact_app, compact_draw};
use crate::features::NyaTermApp;
use crate::features::ai::panel::{AiPanel, AiTargetSession};
use crate::test_support::{TestAppCleanup, TestConfigDir};

fn host<'a>(
    cx: &'a mut TestAppContext,
    root: &Path,
    width: f32,
    height: f32,
) -> (
    Entity<NyaTermApp>,
    Entity<CompactAiHost>,
    &'a mut VisualTestContext,
) {
    let app = compact_app(cx, root);
    let panel = app.read_with(cx, |app, _| app.ai_panel.clone());
    let (host, vcx) = cx.add_window_view(move |_, _| CompactAiHost {
        panel,
        width,
        height,
    });
    let vcx: &mut VisualTestContext = vcx;
    settle(&app, vcx);
    (app, host, vcx)
}

fn settle(app: &Entity<NyaTermApp>, vcx: &mut VisualTestContext) {
    // Both the input engine and the panel publish post-layout measurements.
    for _ in 0..3 {
        compact_draw(app, vcx);
    }
}

fn resize(app: &Entity<NyaTermApp>, vcx: &mut VisualTestContext, delta: Pixels) {
    let edge = vcx
        .debug_bounds("ai-composer-resize-handle")
        .unwrap()
        .center();
    // Exercise the panel listener: it must pass measured geometry without
    // reading the panel again while that entity is being updated.
    vcx.simulate_mouse_down(edge, MouseButton::Left, Default::default());
    assert!(app.read_with(vcx, |app, _| app.ai.composer_is_resizing()));
    vcx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.update_ai_composer_resize(
                &MouseMoveEvent {
                    position: point(edge.x, edge.y - delta),
                    pressed_button: Some(MouseButton::Left),
                    modifiers: Default::default(),
                },
                cx,
            );
            app.finish_ai_composer_resize(cx);
        })
    });
    vcx.simulate_mouse_up(edge, MouseButton::Left, Default::default());
    settle(app, vcx);
}

fn assert_controls_visible(vcx: &mut VisualTestContext) {
    let host = vcx.debug_bounds("ai-compact-host").unwrap();
    for id in [
        "ai-composer-controls",
        "ai-model-control",
        "ai-send-control",
    ] {
        let bounds = vcx.debug_bounds(id).unwrap();
        assert!(
            bounds.top() >= host.top() && bounds.bottom() <= host.bottom(),
            "{id}: {bounds:?} outside {host:?}"
        );
        assert!(bounds.left() >= host.left() && bounds.right() <= host.right());
    }
}

#[test]
fn small_panels_cap_the_prompt_and_keep_transcript_and_controls_visible() {
    for width in [309., 480.] {
        let root = TestConfigDir::new("nyaterm-ai-height-cap");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let (app, _, vcx) = host(&mut cx, root.path(), width, 300.);
        resize(&app, vcx, px(1000.));
        assert_controls_visible(vcx);
        let transcript = vcx.debug_bounds("ai-transcript-viewport").unwrap();
        let prompt = vcx.debug_bounds("ai-prompt").unwrap();
        assert!(transcript.size.height >= px(79.5), "{transcript:?}");
        assert!(
            prompt.size.height < px(240.),
            "small panel must cap the prompt: {prompt:?}"
        );
        vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.ai.note_detected_error(
                    "fixture-session".into(),
                    "fixture error".into(),
                    Instant::now(),
                );
                app.flush_ai_panel_snapshot(cx);
            })
        });
        settle(&app, vcx);
        assert_controls_visible(vcx);
        resize(&app, vcx, px(1000.));
        let with_banner = vcx.debug_bounds("ai-prompt").unwrap().size.height;
        assert!(
            with_banner < prompt.size.height,
            "a banner reduces the prompt's available budget"
        );
    }
}

#[test]
fn resizing_the_panel_restores_the_runtime_floor_without_a_second_drag_jump() {
    let root = TestConfigDir::new("nyaterm-ai-height-restore");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let (app, host, vcx) = host(&mut cx, root.path(), 309., 800.);
    resize(&app, vcx, px(1000.));
    assert_eq!(vcx.debug_bounds("ai-prompt").unwrap().size.height, px(480.));
    host.update(vcx, |host, cx| {
        host.height = 280.;
        cx.notify();
    });
    settle(&app, vcx);
    let small = vcx.debug_bounds("ai-prompt").unwrap().size.height;
    assert!(small < px(480.));
    assert_controls_visible(vcx);
    resize(&app, vcx, px(0.));
    assert_eq!(
        app.read_with(vcx, |app, _| app.ai.composer_min_height()),
        480.
    );
    assert_eq!(vcx.debug_bounds("ai-prompt").unwrap().size.height, small);
    host.update(vcx, |host, cx| {
        host.height = 800.;
        cx.notify();
    });
    settle(&app, vcx);
    assert_eq!(vcx.debug_bounds("ai-prompt").unwrap().size.height, px(480.));
    host.update(vcx, |host, cx| {
        host.height = 280.;
        cx.notify();
    });
    settle(&app, vcx);
    resize(&app, vcx, px(-1.));
    assert_eq!(
        vcx.debug_bounds("ai-prompt").unwrap().size.height,
        small - px(1.)
    );
}

#[test]
fn prompt_growth_above_a_manual_floor_and_a_second_drag_are_continuous() {
    let root = TestConfigDir::new("nyaterm-ai-height-natural");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let (app, _, vcx) = host(&mut cx, root.path(), 309., 800.);
    let initial = vcx.debug_bounds("ai-prompt").unwrap().size.height;
    resize(&app, vcx, px(110.) - initial);
    assert_eq!(vcx.debug_bounds("ai-prompt").unwrap().size.height, px(110.));
    vcx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.set_ai_prompt_draft("1\n2\n3\n4\n5\n6", cx)
        })
    });
    settle(&app, vcx);
    let grown = vcx.debug_bounds("ai-prompt").unwrap().size.height;
    assert!(grown > px(110.));
    resize(&app, vcx, px(1.));
    assert_eq!(
        vcx.debug_bounds("ai-prompt").unwrap().size.height,
        grown + px(1.)
    );
    resize(&app, vcx, px(-1.));
    assert_eq!(
        app.read_with(vcx, |app, _| app.ai.composer_min_height()),
        0.
    );
}

#[test]
fn wrapped_context_and_quotes_scroll_before_the_footer_can_be_clipped() {
    let root = TestConfigDir::new("nyaterm-ai-height-context");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let (app, host, vcx) = host(&mut cx, root.path(), 309., 400.);
    // Render a presentation fixture independently of the app's asynchronous
    // snapshot publisher, which correctly replaces synthetic target sessions.
    let panel = vcx.update(|_, cx| {
        let mut snapshot = app.read(cx).ai_panel.read(cx).snapshot().unwrap().clone();
        snapshot.quoted_text = Some("quoted context ".repeat(30));
        snapshot.target_sessions = (0..16)
            .map(|index| AiTargetSession {
                session_id: format!("target-{index}"),
                label: format!("long target session name {index}"),
            })
            .collect::<Vec<_>>()
            .into();
        snapshot.composer_min_height = 480.;
        snapshot.file_action_ready = true;
        let panel = cx.new(|cx| AiPanel::new(app.downgrade(), cx));
        panel.update(cx, |panel, cx| panel.set_snapshot(snapshot, cx));
        panel
    });
    host.update(vcx, |host, cx| {
        host.panel = panel.clone();
        cx.notify();
    });
    settle(&app, vcx);
    for height in [400., 180., 400.] {
        host.update(vcx, |host, cx| {
            host.height = height;
            cx.notify();
        });
        settle(&app, vcx);
        assert_controls_visible(vcx);
        let viewport = vcx.debug_bounds("ai-composer-editable-viewport").unwrap();
        let controls = vcx.debug_bounds("ai-composer-controls").unwrap();
        assert!(viewport.bottom() <= controls.top());
        assert!(viewport.size.height > px(0.));
        let targets = panel.read_with(vcx, |panel, _| {
            panel.snapshot().unwrap().target_sessions.len()
        });
        assert_eq!(targets, 16);
        vcx.simulate_event(ScrollEvent {
            position: viewport.center(),
            // Wheel notches avoid platform-specific trackpad bounce physics.
            delta: ScrollDelta::Lines(point(0., -100.)),
            ..Default::default()
        });
        settle(&app, vcx);
        assert!(
            vcx.debug_bounds("ai-prompt").is_some(),
            "scrolling the body must reveal the editor"
        );
        assert_controls_visible(vcx);
        assert_eq!(
            vcx.debug_bounds("ai-composer-controls").unwrap(),
            controls,
            "body scrolling must not move the footer"
        );
    }
}

#[test]
fn a_small_panel_can_shrink_below_the_natural_six_row_height() {
    let root = TestConfigDir::new("nyaterm-ai-height-six-rows");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let (app, _, vcx) = host(&mut cx, root.path(), 309., 200.);
    vcx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.set_ai_prompt_draft(vec!["line"; 30].join("\n"), cx)
        })
    });
    settle(&app, vcx);
    let prompt = vcx.debug_bounds("ai-prompt").unwrap();
    let natural = app.read_with(vcx, |app, cx| {
        app.ai_panel
            .read(cx)
            .snapshot()
            .unwrap()
            .prompt_input
            .read(cx)
            .auto_grow_height(cx)
            .unwrap()
    });
    assert!(prompt.size.height < natural);
    assert_controls_visible(vcx);
}
