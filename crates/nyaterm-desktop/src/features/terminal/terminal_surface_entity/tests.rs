use nyaterm_terminal_gpui::geometry::terminal_visual_scroll_offset_px;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::entities::{OverlayStore, StartupRestoreStore, UiStoreHandles};
use crate::features::NyaTermApp;
use crate::features::terminal::terminal_surface::{
    TerminalOverviewMarker, TerminalOverviewMarkerKind,
};
use crate::models::TerminalBufferCellPos;
use crate::models::{TerminalProtocolState, TerminalSelection};
use crate::terminal::{TerminalLineDecorations, compile_terminal_keyword_highlighter};
use gpui::prelude::*;
use gpui::{
    AppContext, Context, Entity, Render, ScrollDelta, ScrollEvent, TestAppContext, Window, bounds,
    div, point, px, size,
};
use nyaterm_core::{AppRuntime, RuntimeMode, uuid};
use nyaterm_terminal::{TerminalScreen, TerminalSnapshot};
use nyaterm_terminal_gpui::{
    NyaTerminalLayoutCache, precompute_terminal_keyword_highlights, terminal_keyword_rules_key,
};

use super::{
    TERMINAL_KEYWORD_HIGHLIGHT_PREFETCH_VIEWPORTS,
    TERMINAL_KEYWORD_HIGHLIGHT_PRESSURE_PREFETCH_VIEWPORTS,
    TERMINAL_KEYWORD_HIGHLIGHT_PRESSURE_THROTTLE, TERMINAL_SURFACE_RETAINED_ROW_BYTE_LIMIT,
    TERMINAL_SURFACE_RETAINED_ROW_LIMIT, TerminalKeywordHighlightRequestKey,
    TerminalPaintedHitTestGeometry, TerminalScrollVisualState, TerminalSurface,
    TerminalSurfaceFrameSnapshot, TerminalSurfaceLocalScrollResult, TerminalVisualScrollGeometry,
    empty_terminal_keyword_rules, terminal_effective_visual_scroll_offset_px,
    terminal_gutter_metrics, terminal_keyword_highlight_prefetch_rows,
    terminal_keyword_highlight_prefetch_viewports, terminal_keyword_highlight_pressure_delay,
    terminal_keyword_highlight_request_key, terminal_keyword_highlight_visible_rows,
    terminal_line_number_digits, terminal_retained_row_window_distance,
    terminal_snapshot_absolute_window, terminal_snapshot_anchor_row_for_display_offset,
    terminal_snapshot_covers_display_offset, terminal_snapshot_row_estimated_bytes,
    terminal_surface_fractional_prefetch_offset, terminal_surface_synthesized_window_extra_rows,
    terminal_surface_text_first_repaint_ready, terminal_surface_visible_rows_for_viewport,
};

struct TerminalSurfaceLayoutTestView {
    surface: Entity<TerminalSurface>,
    mounted: bool,
}

#[test]
fn ime_native_bounds_follow_fractional_horizontal_scroll_on_soft_wrapped_lines() {
    use crate::features::test_support::app_with_visible_local_session;
    use crate::test_support::TestConfigDir;
    use gpui::EntityInputHandler;
    use nyaterm_terminal_gpui::unwrapped::TerminalUnwrappedLayout;

    let dir = TestConfigDir::new("nyaterm-ime-horizontal-anchor");
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, dir.path(), "s1");
    let surface = cx.update_entity(&app, |app, cx| app.ensure_terminal_surface("s1", cx));
    let (_, vcx) = cx.add_window_view(|_, _| TerminalSurfaceLayoutTestView {
        surface: surface.clone(),
        mounted: true,
    });
    let mut screen = TerminalScreen::new(20, 4);
    screen.set_logical_line_snapshots(true);
    screen.advance(&[b'x'; 45]);
    let snapshot = Arc::new(screen.viewport_snapshot(0));
    let grid = bounds(point(px(30.), px(50.)), size(px(160.), px(72.)));
    vcx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.terminal
                .layout
                .session_surface_bounds
                .insert("s1".into(), grid);
            window.focus(&app.terminal.input.focus, cx);
            app.replace_and_mark_text_in_range(None, "中", Some(0..0), window, cx);
        });
        for offset in [29.5, 35., 40.] {
            surface.update(cx, |surface, _| {
                // The same painted mapping consumed by grid rendering and hit
                // testing, including a cursor on the third physical segment.
                surface.painted_hit_test_snapshot = Some(snapshot.clone());
                surface.painted_hit_test_geometry = Some(TerminalPaintedHitTestGeometry {
                    grid_bounds: Some(grid),
                    display_offset: 0,
                    viewport_anchor_row: 0,
                    snapshot_rows: snapshot.row_count(),
                    viewport_rows: 4,
                    visual_y_offset: 0.,
                    cell_width: 8.,
                    cell_height: 18.,
                    revision: 0,
                });
                surface.painted_unwrapped =
                    TerminalUnwrappedLayout::new(&snapshot, 0, 4, offset).map(Arc::new);
            });
            app.update(cx, |app, cx| {
                let anchor = app.terminal_ime_anchor_bounds(grid, cx);
                assert_eq!(anchor.left(), grid.left() + px((45. - offset) * 8.));
                assert_eq!(anchor.top(), grid.top());
                let native = app.bounds_for_range(0..0, grid, window, cx).unwrap();
                assert_eq!(native.left(), anchor.left());
                assert_eq!(native.top(), anchor.top());
            });
        }
    });
}

#[test]
fn row_stripes_repaint_grid_and_gutter_together_and_respect_protocol_modes() {
    let mut screen = TerminalScreen::new(40, 3);
    screen.advance(b"first\r\nsecond");
    let snapshot = Arc::new(screen.viewport_snapshot(0));
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let surface = cx.new(|_| TerminalSurface::new("row-stripes"));
    cx.update_entity(&surface, |surface, _| {
        surface.show_line_numbers = true;
        surface.show_timestamps = true;
        surface.apply_frame_snapshot(TerminalSurfaceFrameSnapshot::new(
            snapshot,
            TerminalScrollVisualState {
                history: screen.history_state(),
                session_id: "row-stripes".into(),
                scroll_offset: 0,
                scroll_residual_lines: 0.0,
                display_offset: 0,
                scrollback_len: 0,
                viewport_rows: 3,
                has_new_while_scrolled: false,
                performance_overlay: None,
                skipped_output_chars: 0,
            },
        ));
        assert!(surface.set_zebra_stripes(true, true, None));
        assert!(!surface.set_zebra_stripes(true, true, None));
    });
    let window = cx.open_window(size(px(600.), px(240.)), {
        let surface = surface.clone();
        move |_, _| TerminalSurfaceLayoutTestView {
            surface,
            mounted: true,
        }
    });
    for (protocol, enabled, by_row, expected) in [
        (TerminalProtocolState::default(), true, true, 2),
        (
            TerminalProtocolState {
                alternate_screen: true,
                ..Default::default()
            },
            true,
            true,
            0,
        ),
        (
            TerminalProtocolState {
                mouse_reporting: true,
                ..Default::default()
            },
            true,
            true,
            0,
        ),
        (TerminalProtocolState::default(), false, true, 0),
        (TerminalProtocolState::default(), true, false, 0),
        (TerminalProtocolState::default(), true, true, 2),
    ] {
        cx.update_entity(&surface, |surface, cx| {
            surface.set_protocol_state(protocol);
            surface.set_zebra_stripes(enabled, by_row, None);
            cx.notify();
        });
        cx.update_window(window.into(), |_, window, cx| {
            window.draw(cx).clear(cx);
            let palette = crate::theme::theme_palette("github-dark");
            let color = gpui::Background::from(gpui::rgba((palette.terminal_fg << 8) | 0x0f));
            let stripes = window
                .painted_quads()
                .into_iter()
                .filter(|quad| quad.background == color)
                .collect::<Vec<_>>();
            assert_eq!(stripes.len(), expected);
            if expected == 2 {
                assert_eq!(stripes[0].bounds.top(), stripes[1].bounds.top());
                assert_eq!(stripes[0].bounds.size.height, stripes[1].bounds.size.height);
            }
        })
        .unwrap();
    }
}

#[test]
fn window_tint_and_transparent_surface_keep_explicit_and_inverse_ansi_backgrounds_opaque() {
    use crate::features::shell::window_material::ShellBackground;

    struct TintedTerminal {
        surface: Entity<TerminalSurface>,
        tint: gpui::Rgba,
    }

    impl Render for TintedTerminal {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .flex()
                .bg(self.tint)
                .child(self.surface.clone())
        }
    }

    let mut screen = TerminalScreen::new(40, 3);
    screen.advance(b"\x1b[48;2;18;52;86mA\x1b[0m\x1b[38;2;171;205;239;7mB\x1b[0m");
    let snapshot = Arc::new(screen.viewport_snapshot(0));
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let surface = cx.new(|_| TerminalSurface::new("ansi-backgrounds"));
    cx.update_entity(&surface, |surface, _| {
        surface.apply_frame_snapshot(
            TerminalSurfaceFrameSnapshot::new(
                snapshot,
                TerminalScrollVisualState {
                    history: screen.history_state(),
                    session_id: "ansi-backgrounds".into(),
                    scroll_offset: 0,
                    scroll_residual_lines: 0.,
                    display_offset: 0,
                    scrollback_len: 0,
                    viewport_rows: 3,
                    has_new_while_scrolled: false,
                    performance_overlay: None,
                    skipped_output_chars: 0,
                },
            )
            .with_presentation(false, false, "block"),
        );
    });
    let rendered_surface = surface.clone();
    let (host, vcx) = cx.add_window_view(move |_, _| TintedTerminal {
        surface: rendered_surface,
        tint: gpui::rgba(0),
    });
    for native_transparent in [false, true] {
        for opacity in [0, 20, 100] {
            let background = ShellBackground {
                native_transparent,
                wallpaper: false,
                content_opacity: 0,
                window_opacity: opacity,
            };
            vcx.update(|window, cx| {
                surface.update(cx, |surface, cx| {
                    surface.set_background_transparent(background.transparent_surfaces());
                    cx.notify();
                });
                host.update(cx, |host, cx| {
                    host.tint = background.root_color(0x010203);
                    cx.notify();
                });
                let _ = window.draw(cx);
                for color in [0x123456, 0xabcdef] {
                    assert!(
                        window.painted_quads().iter().any(|quad| {
                            quad.background == gpui::Background::from(gpui::rgb(color))
                        }),
                        "ANSI background {color:06x} must remain fully opaque"
                    );
                }
            });
        }
    }
}

#[test]
fn overview_marker_buckets_reuse_arc_until_key_or_height_changes() {
    let mut surface = TerminalSurface::new("session");
    assert!(
        surface.set_overview_markers(
            vec![TerminalOverviewMarker {
                absolute_line: 50,
                kind: TerminalOverviewMarkerKind::SelectedOccurrence,
            }]
            .into(),
            100,
            7,
        )
    );

    let initial = surface.overview_marker_buckets_for_track_height(200);
    let reused = surface.overview_marker_buckets_for_track_height(200);
    let resized = surface.overview_marker_buckets_for_track_height(240);

    assert!(Arc::ptr_eq(&initial, &reused));
    assert!(!Arc::ptr_eq(&initial, &resized));
    assert!(!resized.is_empty());
}

impl Render for TerminalSurfaceLayoutTestView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .min_w_0()
            .min_h_0()
            .when(self.mounted, |view| view.child(self.surface.clone()))
    }
}

fn rendered_terminal_grid_bounds(show_line_numbers: bool) -> gpui::Bounds<gpui::Pixels> {
    rendered_terminal_wrapping_grid_bounds(
        show_line_numbers,
        false,
        nyaterm_core::models::settings::TerminalWrapMode::Character,
    )
}

fn rendered_terminal_wrapping_grid_bounds(
    show_line_numbers: bool,
    show_markers: bool,
    mode: nyaterm_core::models::settings::TerminalWrapMode,
) -> gpui::Bounds<gpui::Pixels> {
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let surface = cx.new(|_| TerminalSurface::new("session"));
    let mut screen = TerminalScreen::default();
    screen
        .set_logical_line_snapshots(mode == nyaterm_core::models::settings::TerminalWrapMode::None);
    let snapshot = Arc::new(screen.viewport_snapshot(0));
    let rows = snapshot.row_count();
    let history = snapshot.history;
    cx.update_entity(&surface, |surface, _cx| {
        surface.show_line_numbers = show_line_numbers;
        surface.show_wrap_markers = show_markers;
        surface.wrap_mode = mode;
        surface.apply_frame_snapshot(
            TerminalSurfaceFrameSnapshot::new(
                snapshot,
                TerminalScrollVisualState {
                    history,
                    session_id: "session".to_string(),
                    scroll_offset: 0,
                    scroll_residual_lines: 0.0,
                    display_offset: 0,
                    scrollback_len: 0,
                    viewport_rows: rows,
                    has_new_while_scrolled: false,
                    performance_overlay: None,
                    skipped_output_chars: 0,
                },
            )
            .with_presentation(false, true, "block"),
        );
    });
    let window = cx.open_window(size(px(400.0), px(240.0)), {
        let surface = surface.clone();
        move |_window, _cx| TerminalSurfaceLayoutTestView {
            surface,
            mounted: true,
        }
    });

    cx.update_window(window.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
    })
    .unwrap();
    cx.run_until_parked();

    cx.read_entity(&surface, |surface, _cx| {
        let geometry = surface.painted_hit_test_geometry.expect("painted geometry");
        geometry.grid_bounds.expect("painted grid bounds")
    })
}

#[test]
fn trailing_marker_lane_preserves_grid_origin_and_wrap_mode_keeps_the_same_grid_size() {
    use nyaterm_core::models::settings::TerminalWrapMode;
    let plain = rendered_terminal_wrapping_grid_bounds(false, false, TerminalWrapMode::Character);
    let markers = rendered_terminal_wrapping_grid_bounds(false, true, TerminalWrapMode::Character);
    let unwrapped = rendered_terminal_wrapping_grid_bounds(false, true, TerminalWrapMode::None);
    assert_eq!(markers.left(), plain.left());
    assert_eq!(plain.right() - markers.right(), px(8.0));
    assert_eq!(plain.size.width - markers.size.width, px(8.0));
    assert_eq!(markers, unwrapped);
    let numbered = rendered_terminal_wrapping_grid_bounds(true, false, TerminalWrapMode::Character);
    let numbered_markers =
        rendered_terminal_wrapping_grid_bounds(true, true, TerminalWrapMode::Character);
    assert_eq!(numbered.left(), numbered_markers.left());
    assert_eq!(numbered.right() - numbered_markers.right(), px(8.0));
}

#[test]
fn horizontal_offset_is_local_to_each_surface_and_history_preserves_manual_position() {
    use nyaterm_core::models::settings::TerminalWrapMode;
    let mut screen = TerminalScreen::new(4, 2);
    screen.set_logical_line_snapshots(true);
    screen.advance(b"abcdefghijklmnop\r\na\r\nb\r\nc");
    let mut first = TerminalSurface::new("first");
    let second = TerminalSurface::new("second");
    first.wrap_mode = TerminalWrapMode::None;
    first.viewport_rows = 2;
    first.scroll_offset = 1;
    first.display_offset = 1;
    first
        .horizontal_scroll
        .set_offset(point(px(-64.0), px(0.0)));
    let snapshot = screen.viewport_snapshot(1);
    let layout = first.unwrapped_layout(&snapshot).unwrap();
    assert_eq!(layout.horizontal_offset, 8.0);
    assert_eq!(f32::from(second.horizontal_scroll.offset().x), 0.0);
    assert_eq!(f32::from(first.horizontal_scroll.offset().x), -64.0);
}

#[test]
fn unwrapped_wheel_input_scrolls_only_the_requested_axis_once() {
    use nyaterm_core::models::settings::TerminalWrapMode;

    struct ScrollFixture {
        surface: Entity<TerminalSurface>,
    }

    impl Render for ScrollFixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let surface = self.surface.clone();
            div()
                .w(px(400.))
                .h(px(64.))
                .flex()
                // Exercise the actual measuring layer and terminal handler
                // without an app/transport worker replacing the test frames.
                .on_scroll(move |event, _, cx| {
                    surface.update(cx, |surface, cx| {
                        surface.handle_unwrapped_scroll(event, cx);
                    });
                })
                .child(self.surface.clone())
        }
    }

    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut screen = TerminalScreen::new(40, 4);
    screen.set_logical_line_snapshots(true);
    screen.advance(
        format!("{}\r\n", "0123456789".repeat(20))
            .repeat(20)
            .as_bytes(),
    );
    let offset = screen.scrollback_len();
    let snapshot = Arc::new(screen.viewport_snapshot_with_window(offset, 0, offset));
    let surface = cx.new(|_| TerminalSurface::new("scroll-axes"));
    surface.update(&mut cx, |surface, _| {
        surface.wrap_mode = TerminalWrapMode::None;
        surface.apply_frame_snapshot(TerminalSurfaceFrameSnapshot::new(
            snapshot,
            TerminalScrollVisualState {
                history: screen.history_state(),
                session_id: "scroll-axes".into(),
                scroll_offset: offset,
                scroll_residual_lines: 0.0,
                display_offset: offset,
                scrollback_len: offset,
                viewport_rows: 4,
                has_new_while_scrolled: false,
                performance_overlay: None,
                skipped_output_chars: 0,
            },
        ));
        surface
            .horizontal_scroll
            .set_offset(point(px(-32.), px(0.)));
    });
    let (_, vcx) = cx.add_window_view({
        let surface = surface.clone();
        move |_, _| ScrollFixture { surface }
    });
    vcx.update(|window, cx| {
        _ = window.draw(cx);
        let surface = surface.read(cx);
        assert!(surface.painted_unwrapped.is_some());
        assert!(surface.horizontal_scroll.max_offset().x > px(0.));
    });
    let mut scroll = |delta, shift| {
        let before = vcx.update(|_, cx| {
            let surface = surface.read(cx);
            (surface.scroll_offset, surface.horizontal_scroll.offset().x)
        });
        vcx.simulate_event(ScrollEvent {
            position: point(px(20.), px(20.)),
            delta,
            modifiers: gpui::Modifiers {
                shift,
                ..Default::default()
            },
            ..Default::default()
        });
        let after = vcx.update(|window, cx| {
            _ = window.draw(cx);
            let surface = surface.read(cx);
            (surface.scroll_offset, surface.horizontal_scroll.offset().x)
        });
        (before, after)
    };
    for delta in [
        ScrollDelta::Lines(point(0., -1.)),
        ScrollDelta::Pixels(point(px(0.), px(-16.))),
        ScrollDelta::Pixels(point(px(-1.), px(-16.))),
    ] {
        let (before, after) = scroll(delta, false);
        assert!(after.0 < before.0, "vertical input must scroll history");
        assert_eq!(after.1, before.1, "vertical input must preserve X");
    }
    let (before, after) = scroll(ScrollDelta::Pixels(point(px(-32.), px(0.))), false);
    assert_eq!(after.0, before.0);
    assert_eq!(after.1, before.1 - px(32.));
    let (before, after) = scroll(ScrollDelta::Lines(point(0., -1.)), true);
    assert_eq!(after.0, before.0);
    assert_eq!(after.1, before.1 - px(16.));
}

#[test]
fn switching_to_no_wrap_does_not_replay_cursor_reveals_from_character_mode() {
    use nyaterm_core::models::settings::TerminalWrapMode;
    let mut screen = TerminalScreen::new(4, 2);
    screen.advance(b"abcdefghijk");
    let mut surface = TerminalSurface::new("session");
    surface.is_active = true;
    surface.snapshot = Some(Arc::new(screen.snapshot()));
    let input_at = Instant::now();
    assert!(!surface.reveal_input_cursor(Some(input_at)));
    assert!(surface.horizontal_reveal.is_none());
    surface.wrap_mode = TerminalWrapMode::None;
    assert!(!surface.reveal_input_cursor(Some(input_at)));
    assert!(surface.horizontal_reveal.is_none());
    assert!(surface.reveal_input_cursor(Some(input_at + Duration::from_millis(1))));
    assert!(surface.horizontal_reveal.is_some());
}

#[test]
fn painted_hit_test_grid_bounds_match_the_rendered_grid_layout() {
    let grid_bounds = rendered_terminal_grid_bounds(false);

    let rail = nyaterm_terminal_gpui::command_blocks::COMMAND_RAIL_WIDTH;
    assert_eq!(grid_bounds.origin, point(px(rail), px(0.0)));
    assert_eq!(grid_bounds.size.height, px(240.0));
}

#[test]
fn painted_hit_test_grid_bounds_start_after_the_rendered_gutter() {
    let snapshot = TerminalScreen::default().viewport_snapshot(0);
    let expected_gutter =
        terminal_gutter_metrics(8.0, false, 10, true, terminal_line_number_digits(&snapshot))
            .total_width();
    let grid_bounds = rendered_terminal_grid_bounds(true);

    assert_eq!(grid_bounds.origin, point(px(expected_gutter), px(0.0)));
    assert_eq!(grid_bounds.size.height, px(240.0));
}

#[test]
fn clear_all_paints_input_at_top_and_keeps_host_rows_for_hit_testing() {
    let mut screen = TerminalScreen::new(40, 12);
    screen.advance(b"\x1b[12;1Hroot# draft");
    screen.clear_except_input();
    for older_rows in [0, 16] {
        let snapshot = Arc::new(screen.viewport_snapshot_with_window(0, older_rows, 16));
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let surface = cx.new(|_| TerminalSurface::new("session"));
        cx.update_entity(&surface, |surface, _cx| {
            surface.show_line_numbers = true;
            surface.show_timestamps = true;
            surface.apply_frame_snapshot(TerminalSurfaceFrameSnapshot::new(
                snapshot.clone(),
                TerminalScrollVisualState {
                    history: screen.history_state(),
                    session_id: "session".to_string(),
                    scroll_offset: 0,
                    scroll_residual_lines: 0.0,
                    display_offset: 0,
                    scrollback_len: 0,
                    viewport_rows: 12,
                    has_new_while_scrolled: false,
                    performance_overlay: None,
                    skipped_output_chars: 0,
                },
            ));
        });
        let window = cx.open_window(size(px(600.0), px(240.0)), {
            let surface = surface.clone();
            move |_window, _cx| TerminalSurfaceLayoutTestView {
                surface,
                mounted: true,
            }
        });
        cx.update_window(window.into(), |_, window, cx| {
            window.draw(cx).clear(cx);
        })
        .unwrap();
        let geometry = cx.read_entity(&surface, |surface, _cx| {
            surface.painted_hit_test_geometry.expect("painted geometry")
        });
        let anchor = snapshot.cursor.row - 11;
        assert_eq!(geometry.viewport_anchor_row, anchor);
        let cursor_y = geometry.visual_y_offset + snapshot.cursor.row as f32 * geometry.cell_height;
        assert_eq!(cursor_y, 0.0);
        let clicked_snapshot_row = ((geometry.cell_height * 0.5 - geometry.visual_y_offset)
            / geometry.cell_height)
            .floor() as usize;
        assert_eq!(clicked_snapshot_row - geometry.viewport_anchor_row, 11);
        assert_eq!(snapshot.line(clicked_snapshot_row), Some("root# draft"));
    }
}

#[test]
fn enriched_snapshot_keeps_existing_rows_at_the_same_pixel_height() {
    let mut screen = TerminalScreen::default();
    screen.advance_decoded_text(&terminal_test_output_lines(50));
    let plain = Arc::new(screen.viewport_snapshot(0));
    let enriched = Arc::new(screen.viewport_snapshot_with_window(0, 32, 32));
    assert!(enriched.row_count() > plain.row_count());
    let viewport_rows = plain.viewport_rows;
    let scrollback_len = plain.scrollback_len;
    let (plain_geometry, enriched_geometry) = {
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let surface = cx.new(|_| {
            let mut surface = TerminalSurface::new("session");
            surface.cell_height = 21.12;
            surface
        });
        let apply_snapshot = |cx: &mut TestAppContext, snapshot: Arc<TerminalSnapshot>| {
            cx.update_entity(&surface, |surface, cx| {
                surface.apply_frame_snapshot(TerminalSurfaceFrameSnapshot::new(
                    snapshot,
                    TerminalScrollVisualState {
                        history: screen.history_state(),
                        session_id: "session".to_string(),
                        scroll_offset: 0,
                        scroll_residual_lines: 0.0,
                        display_offset: 0,
                        scrollback_len,
                        viewport_rows,
                        has_new_while_scrolled: false,
                        performance_overlay: None,
                        skipped_output_chars: 0,
                    },
                ));
                cx.notify();
            });
        };
        apply_snapshot(&mut cx, plain.clone());
        let window = cx.open_window(size(px(400.0), px(685.0)), {
            let surface = surface.clone();
            move |_window, _cx| TerminalSurfaceLayoutTestView {
                surface,
                mounted: true,
            }
        });
        let draw = |cx: &mut TestAppContext| {
            cx.update_window(window.into(), |_, window, cx| {
                window.draw(cx).clear(cx);
            })
            .unwrap();
            cx.run_until_parked();
            cx.read_entity(&surface, |surface, _| {
                surface.painted_hit_test_geometry.expect("painted geometry")
            })
        };
        let plain_geometry = draw(&mut cx);
        apply_snapshot(&mut cx, enriched.clone());
        let enriched_geometry = draw(&mut cx);
        (plain_geometry, enriched_geometry)
    };
    let bottom_absolute_row = plain.total_rows - 1;
    let row_y = |snapshot: &TerminalSnapshot, geometry: TerminalPaintedHitTestGeometry| {
        let start = snapshot.total_rows - snapshot.row_count();
        geometry.visual_y_offset + (bottom_absolute_row - start) as f32 * geometry.cell_height
    };
    assert_eq!(plain_geometry.cell_height, 21.0);
    assert_eq!(enriched_geometry.cell_height, 21.0);
    assert_eq!(
        row_y(&plain, plain_geometry),
        row_y(&enriched, enriched_geometry)
    );
}

#[test]
fn real_window_draw_renders_visible_inactive_but_skips_unmounted_surface() {
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let layout_cache = Arc::new(Mutex::new(NyaTerminalLayoutCache::default()));
    let mut screen = TerminalScreen::default();
    screen.advance_decoded_text(&terminal_test_output_lines(32));
    let snapshot = Arc::new(screen.viewport_snapshot(0));
    let viewport_rows = snapshot.row_count().max(1);
    let history = snapshot.history;
    let surface = cx.new({
        let layout_cache = Arc::clone(&layout_cache);
        let snapshot = Arc::clone(&snapshot);
        move |_| {
            let mut surface = TerminalSurface::new("session");
            assert!(surface.set_layout_cache(layout_cache));
            assert!(
                surface.apply_frame_snapshot(
                    TerminalSurfaceFrameSnapshot::new(
                        snapshot,
                        TerminalScrollVisualState {
                            history,
                            session_id: "session".to_string(),
                            scroll_offset: 0,
                            scroll_residual_lines: 0.0,
                            display_offset: 0,
                            scrollback_len: 8,
                            viewport_rows,
                            has_new_while_scrolled: false,
                            performance_overlay: None,
                            skipped_output_chars: 0,
                        },
                    )
                    .with_presentation(false, false, "block"),
                )
            );
            surface
        }
    });
    let window = cx.open_window(size(px(400.0), px(viewport_rows as f32 * 16.0)), {
        let surface = surface.clone();
        move |_window, _cx| TerminalSurfaceLayoutTestView {
            surface,
            mounted: true,
        }
    });
    let host = window.root(&mut cx).expect("terminal fixture root view");
    let draw = |cx: &mut TestAppContext| {
        cx.update_window(window.into(), |_, window, cx| {
            window.draw(cx).clear(cx);
        })
        .unwrap();
        cx.run_until_parked();
    };

    draw(&mut cx);
    let first_draw = layout_cache.lock().unwrap().stats();
    assert!(first_draw.shape_calls > 0);
    assert!(first_draw.shape_calls <= viewport_rows as u64 + 1);

    let before_repeated = first_draw;
    cx.update_entity(&surface, |surface, cx| {
        assert!(
            !surface.apply_frame_snapshot(
                TerminalSurfaceFrameSnapshot::new(
                    Arc::clone(&snapshot),
                    TerminalScrollVisualState {
                        history,
                        session_id: "session".to_string(),
                        scroll_offset: 0,
                        scroll_residual_lines: 0.0,
                        display_offset: 0,
                        scrollback_len: 8,
                        viewport_rows,
                        has_new_while_scrolled: false,
                        performance_overlay: None,
                        skipped_output_chars: 0,
                    },
                )
                .with_presentation(false, false, "block"),
            )
        );
        cx.notify();
    });
    draw(&mut cx);
    let repeated_delta = layout_cache
        .lock()
        .unwrap()
        .stats()
        .delta_since(before_repeated);
    assert!(repeated_delta.hits > 0);
    assert_eq!(repeated_delta.shape_calls, 0);

    cx.update_entity(&host, |host, cx| {
        host.mounted = false;
        cx.notify();
    });
    draw(&mut cx);
    let before_hidden = layout_cache.lock().unwrap().stats();
    let painted_revision_before_hidden = cx.read_entity(&surface, |surface, _cx| {
        surface
            .painted_hit_test_geometry
            .expect("visible surface should have painted geometry")
            .revision
    });

    screen.advance_decoded_text("hidden surface update\n");
    let hidden_snapshot = Arc::new(screen.viewport_snapshot(0));
    cx.update_entity(&surface, |surface, cx| {
        assert!(
            surface.apply_frame_snapshot(
                TerminalSurfaceFrameSnapshot::new(
                    hidden_snapshot,
                    TerminalScrollVisualState {
                        history,
                        session_id: "session".to_string(),
                        scroll_offset: 0,
                        scroll_residual_lines: 0.0,
                        display_offset: 0,
                        scrollback_len: 9,
                        viewport_rows,
                        has_new_while_scrolled: false,
                        performance_overlay: None,
                        skipped_output_chars: 0,
                    },
                )
                .with_presentation(false, false, "block"),
            )
        );
        cx.notify();
    });
    draw(&mut cx);

    let hidden_delta = layout_cache
        .lock()
        .unwrap()
        .stats()
        .delta_since(before_hidden);
    assert_eq!(hidden_delta, Default::default());
    cx.read_entity(&surface, |surface, _cx| {
        assert_eq!(
            surface
                .painted_hit_test_geometry
                .expect("hidden surface should retain its last painted geometry")
                .revision,
            painted_revision_before_hidden
        );
        assert!(surface.revision > painted_revision_before_hidden);
    });

    let before_remount = layout_cache.lock().unwrap().stats();
    cx.update_entity(&host, |host, cx| {
        host.mounted = true;
        cx.notify();
    });
    draw(&mut cx);
    let remount_delta = layout_cache
        .lock()
        .unwrap()
        .stats()
        .delta_since(before_remount);
    assert!(remount_delta.shape_calls > 0);
    cx.read_entity(&surface, |surface, _cx| {
        assert_eq!(
            surface
                .painted_hit_test_geometry
                .expect("remounted surface should paint")
                .revision,
            surface.revision
        );
    });
}

#[test]
fn painted_hit_test_state_uses_the_latest_grid_bounds() {
    let mut surface = TerminalSurface::new("session");
    let snapshot = Arc::new(TerminalScreen::default().viewport_snapshot(0));
    surface.painted_hit_test_snapshot = Some(snapshot);
    surface.painted_hit_test_geometry = Some(TerminalPaintedHitTestGeometry {
        grid_bounds: None,
        display_offset: 0,
        viewport_anchor_row: 0,
        snapshot_rows: 24,
        viewport_rows: 24,
        visual_y_offset: 0.0,
        cell_width: 8.0,
        cell_height: 16.0,
        revision: 1,
    });
    let grid_bounds = bounds(point(px(87.0), px(31.0)), size(px(640.0), px(384.0)));

    assert!(surface.set_painted_hit_test_grid_bounds(grid_bounds));
    assert!(!surface.set_painted_hit_test_grid_bounds(grid_bounds));
    assert_eq!(
        surface
            .painted_hit_test_state()
            .map(|(geometry, _)| geometry.grid_bounds),
        Some(Some(grid_bounds))
    );
}

macro_rules! apply_test_frame_snapshot {
    (
        $surface:expr,
        $snapshot:expr,
        $scroll_offset:expr,
        $scroll_residual_lines:expr,
        $display_offset:expr,
        $scrollback_len:expr,
        $viewport_rows:expr,
        $has_new_while_scrolled:expr,
        $performance_overlay:expr,
        $skipped_output_chars:expr,
        $has_action_link_decorations:expr,
        $show_cursor:expr,
        $cursor_style:expr $(,)?
    ) => {{
        let snapshot = $snapshot;
        let history = snapshot.history;
        $surface.apply_frame_snapshot(
            TerminalSurfaceFrameSnapshot::new(
                snapshot,
                TerminalScrollVisualState {
                    history,
                    session_id: "session".to_string(),
                    scroll_offset: $scroll_offset,
                    scroll_residual_lines: $scroll_residual_lines,
                    display_offset: $display_offset,
                    scrollback_len: $scrollback_len,
                    viewport_rows: $viewport_rows,
                    has_new_while_scrolled: false,
                    performance_overlay: None,
                    skipped_output_chars: 0,
                },
            )
            .with_output_state(
                $has_new_while_scrolled,
                $performance_overlay,
                $skipped_output_chars,
            )
            .with_presentation(
                $has_action_link_decorations,
                $show_cursor,
                $cursor_style,
            ),
        )
    }};
}

fn terminal_test_output_lines(count: usize) -> String {
    (0..count)
        .map(|index| format!("line {index:03}\n"))
        .collect::<String>()
}

#[test]
fn empty_terminal_keyword_rules_reuses_arc() {
    let first = empty_terminal_keyword_rules();
    let second = empty_terminal_keyword_rules();

    assert!(first.is_empty());
    assert!(Arc::ptr_eq(&first, &second));
}

fn terminal_test_retained_live_snapshot(count: usize) -> (TerminalSnapshot, usize) {
    let mut screen = TerminalScreen::default();
    screen.advance_decoded_text(&terminal_test_output_lines(count));
    let base = screen.viewport_snapshot(0);
    let viewport_rows = base.row_count().max(1);
    let older = screen.viewport_snapshot(viewport_rows);
    let retained_older_rows = older.row_count().min(viewport_rows);
    let mut snapshot = base;
    if retained_older_rows == 0 || snapshot.cols == 0 {
        return (snapshot, viewport_rows);
    }

    let older_start = older.row_count().saturating_sub(retained_older_rows);
    let mut rows = older.rows()[older_start..].to_vec();
    rows.extend(snapshot.rows().iter().cloned());
    snapshot.row_data = rows.into();
    (snapshot, viewport_rows)
}

#[test]
fn retained_row_cache_refreshes_only_changed_snapshot_rows() {
    let mut screen = TerminalScreen::default();
    screen.advance_decoded_text(&terminal_test_output_lines(80));
    let snapshot = screen.viewport_snapshot(0);
    let mut surface = TerminalSurface::new("session");

    assert_eq!(
        surface.remember_retained_snapshot_rows(&snapshot),
        snapshot.row_count()
    );
    assert_eq!(surface.remember_retained_snapshot_rows(&snapshot), 0);

    let mut metadata_changed = snapshot.clone();
    let rows = Arc::make_mut(&mut metadata_changed.row_data);
    Arc::make_mut(&mut rows[0]).timestamp_ms = Some(42);
    assert_eq!(
        surface.remember_retained_snapshot_rows(&metadata_changed),
        1
    );
}

#[test]
fn retained_row_cache_resets_when_numbering_base_changes() {
    // A reflow or local clear can repin the numbering origin. Rows cached
    // against the old base must never be stitched into the new window, or the
    // gutter restarts mid-screen while content keeps counting.
    let mut screen = TerminalScreen::default();
    screen.advance_decoded_text(&terminal_test_output_lines(80));
    let snapshot = Arc::new(screen.viewport_snapshot(0));
    let rows = snapshot.row_count();
    let scrollback_len = snapshot.scrollback_len;
    let mut surface = TerminalSurface::new("session");
    apply_test_frame_snapshot!(
        surface,
        snapshot.clone(),
        0,
        0.0,
        0,
        scrollback_len,
        rows,
        false,
        None,
        0,
        false,
        false,
        "block",
    );
    assert!(!surface.retained_snapshots.is_empty());

    let mut rebased = (*snapshot).clone();
    rebased.line_number_base = snapshot.line_number_base.map(|base| base + 1);
    let rebased = Arc::new(rebased);
    apply_test_frame_snapshot!(
        surface,
        rebased.clone(),
        0,
        0.0,
        0,
        scrollback_len,
        rows,
        false,
        None,
        0,
        false,
        false,
        "block",
    );

    assert_eq!(surface.retained_snapshots.len(), 1);
    assert!(Arc::ptr_eq(
        surface.retained_snapshots.first().unwrap(),
        &rebased
    ));
    assert_eq!(surface.retained_rows.len(), rows);
}

#[test]
fn retained_row_cache_respects_estimated_byte_budget() {
    let mut screen = TerminalScreen::default();
    screen.advance_decoded_text(&terminal_test_output_lines(80));
    let snapshot = screen.viewport_snapshot(0);
    let row_bytes = snapshot
        .row(0)
        .map(terminal_snapshot_row_estimated_bytes)
        .expect("snapshot row");
    let max_bytes = row_bytes.saturating_mul(2);
    let mut surface = TerminalSurface::new("session");

    surface.remember_retained_snapshot_rows_with_limits(&snapshot, usize::MAX, max_bytes);

    assert!(surface.retained_row_estimated_bytes <= max_bytes);
    assert!(surface.retained_rows.len() < snapshot.row_count());
}

#[test]
fn retained_row_window_distance_prefers_rows_outside_the_target() {
    assert_eq!(terminal_retained_row_window_distance(50, 60, 100), 10);
    assert_eq!(terminal_retained_row_window_distance(59, 60, 100), 1);
    assert_eq!(terminal_retained_row_window_distance(70, 60, 100), 0);
    assert_eq!(terminal_retained_row_window_distance(99, 60, 100), 0);
    assert_eq!(terminal_retained_row_window_distance(105, 60, 100), 6);
}

#[test]
fn retained_row_cache_follows_upward_scroll_target() {
    let mut screen = TerminalScreen::new(40, 8);
    screen.set_scrollback_limit(400);
    screen.advance_decoded_text(&terminal_test_output_lines(400));

    let viewport_rows = 8;
    let live = screen.viewport_snapshot_with_window(0, 16, 16);
    let scrollback_len = live.scrollback_len;
    let real_total_rows = scrollback_len.saturating_add(viewport_rows);

    let mut surface = TerminalSurface::new("upward-scroll-retention");
    // Keep fewer rows than the total scrollback so eviction is exercised on
    // every upward step, as it is on a terminal with a long scrollback.
    let max_rows = 48;
    surface.remember_retained_snapshot_rows_with_limits(&live, max_rows, usize::MAX);

    let mut covered = 0usize;
    let mut total = 0usize;
    let mut offset = 8usize;
    while offset <= scrollback_len.min(320) {
        let snapshot = screen.viewport_snapshot_with_window(offset, 16, 16);
        surface.remember_retained_snapshot_rows_with_limits(&snapshot, max_rows, usize::MAX);
        let desired_end = real_total_rows.saturating_sub(offset);
        let desired_start = desired_end.saturating_sub(viewport_rows);
        total += 1;
        if surface.retained_rows_cover_absolute_range(desired_start, desired_end) {
            covered += 1;
        }
        offset += 16;
    }

    println!("retained upward scroll coverage: {covered}/{total}");
    assert_eq!(
        covered, total,
        "the retained row cache must keep the upward scroll target covered"
    );
}

#[test]
#[ignore = "measurement: run with --ignored --nocapture on a 12k-line scrollback"]
fn large_scrollback_upward_scroll_coverage_measurement() {
    const VIEWPORT_ROWS: usize = 50;
    const MAX_RETAINED_ROWS: usize = 4096;
    const EXTRA_ROWS: usize = 192;
    const SCROLLBACK_LINES: usize = 12_000;

    let mut screen = TerminalScreen::new(200, VIEWPORT_ROWS as u16);
    screen.set_scrollback_limit(SCROLLBACK_LINES + VIEWPORT_ROWS);
    screen.advance_decoded_text(&terminal_test_output_lines(SCROLLBACK_LINES));

    let live = screen.viewport_snapshot_with_window(0, EXTRA_ROWS, EXTRA_ROWS);
    let scrollback_len = live.scrollback_len;
    let real_total_rows = scrollback_len.saturating_add(VIEWPORT_ROWS);
    let mut surface = TerminalSurface::new("large-scrollback-measurement");
    surface.remember_retained_snapshot_rows_with_limits(&live, MAX_RETAINED_ROWS, usize::MAX);

    // Reference model of the previous eviction policy (always drop the smallest
    // absolute row), used only to quantify the improvement.
    let mut old_rows = BTreeMap::new();
    if let Some((start, end)) = terminal_snapshot_absolute_window(&live) {
        for abs_row in start..end {
            old_rows.insert(abs_row, ());
        }
    }

    let mut new_covered = 0usize;
    let mut old_covered = 0usize;
    let mut total = 0usize;
    let mut offset = 0usize;
    while offset.saturating_add(8) <= scrollback_len {
        offset += 8;
        let snapshot = screen.viewport_snapshot_with_window(offset, EXTRA_ROWS, EXTRA_ROWS);
        surface.remember_retained_snapshot_rows_with_limits(
            &snapshot,
            MAX_RETAINED_ROWS,
            usize::MAX,
        );
        if let Some((start, end)) = terminal_snapshot_absolute_window(&snapshot) {
            for abs_row in start..end {
                old_rows.insert(abs_row, ());
            }
        }
        while old_rows.len() > MAX_RETAINED_ROWS {
            let Some(oldest) = old_rows.keys().next().copied() else {
                break;
            };
            old_rows.remove(&oldest);
        }

        let desired_end = real_total_rows.saturating_sub(offset);
        let desired_start = desired_end.saturating_sub(VIEWPORT_ROWS);
        total += 1;
        if surface.retained_rows_cover_absolute_range(desired_start, desired_end) {
            new_covered += 1;
        }
        if (desired_start..desired_end).all(|abs_row| old_rows.contains_key(&abs_row)) {
            old_covered += 1;
        }
    }
    println!(
        "12k scrollback upward scroll coverage: old={old_covered}/{total} new={new_covered}/{total}"
    );
}

#[test]
fn synthesized_scroll_window_uses_bounded_overscan() {
    assert_eq!(terminal_surface_synthesized_window_extra_rows(8), 32);
    assert_eq!(terminal_surface_synthesized_window_extra_rows(40), 80);
    assert_eq!(terminal_surface_synthesized_window_extra_rows(200), 192);
}

#[test]
fn visual_scroll_offset_tracks_target_display_and_residual() {
    assert_eq!(terminal_visual_scroll_offset_px(0, 0, 0.0, 16.0), 0.0);
    assert_eq!(terminal_visual_scroll_offset_px(1, 0, 0.25, 16.0), 20.0);
    assert_eq!(terminal_visual_scroll_offset_px(0, 1, -0.25, 16.0), -20.0);
    assert_eq!(terminal_visual_scroll_offset_px(4, 4, 0.5, 20.0), 10.0);
    assert_eq!(terminal_visual_scroll_offset_px(20, 0, 0.0, 16.0), 320.0);
    assert_eq!(
        terminal_effective_visual_scroll_offset_px(TerminalVisualScrollGeometry {
            snapshot_pending: true,
            target_offset: 1,
            displayed_offset: 0,
            residual_lines: 0.25,
            viewport_anchor_row: 1,
            snapshot_rows: 4,
            viewport_rows: 2,
            cell_height: 16.0,
        }),
        16.0
    );
    assert_eq!(
        terminal_effective_visual_scroll_offset_px(TerminalVisualScrollGeometry {
            snapshot_pending: false,
            target_offset: 1,
            displayed_offset: 0,
            residual_lines: 0.25,
            viewport_anchor_row: 0,
            snapshot_rows: 1,
            viewport_rows: 1,
            cell_height: 16.0,
        }),
        20.0
    );
}

#[test]
fn text_first_repaint_waits_for_cached_target_text() {
    let state = TerminalScrollVisualState {
        history: Default::default(),
        session_id: "session".to_string(),
        scroll_offset: 4,
        scroll_residual_lines: 0.0,
        display_offset: 4,
        scrollback_len: 20,
        viewport_rows: 8,
        has_new_while_scrolled: false,
        performance_overlay: None,
        skipped_output_chars: 0,
    };

    assert!(!terminal_surface_text_first_repaint_ready(
        &state, false, false
    ));
    assert!(terminal_surface_text_first_repaint_ready(
        &state, false, true
    ));
    assert!(!terminal_surface_text_first_repaint_ready(
        &state, true, true
    ));
}

#[test]
fn keyword_highlight_request_key_tracks_snapshot_and_rules() {
    let mut screen = TerminalScreen::default();
    screen.advance_decoded_text("alpha\nbeta\n");
    let snapshot = screen.viewport_snapshot(0);
    let rules = vec![nyaterm_core::ResolvedKeywordHighlightRule {
        id: "alpha".to_string(),
        name: "Alpha".to_string(),
        patterns: vec!["alpha".to_string()],
        color: "#ff0000".to_string(),
        enabled: true,
    }];
    let rules_key = terminal_keyword_rules_key(&rules);
    let rows = 0..snapshot.row_count();
    let key = terminal_keyword_highlight_request_key(&snapshot, rules_key, rows.clone());

    assert_eq!(
        key,
        terminal_keyword_highlight_request_key(&snapshot, rules_key, rows.clone())
    );
    assert_ne!(
        key,
        terminal_keyword_highlight_request_key(
            &snapshot,
            rules_key.saturating_add(1),
            rows.clone(),
        )
    );
    assert_ne!(
        key,
        terminal_keyword_highlight_request_key(&snapshot, rules_key, 1..snapshot.row_count())
    );

    let mut scrolled_snapshot = snapshot.clone();
    scrolled_snapshot.display_offset = scrolled_snapshot.display_offset.saturating_add(1);
    assert_ne!(
        key,
        terminal_keyword_highlight_request_key(&scrolled_snapshot, rules_key, rows.clone())
    );

    let mut edited_snapshot = snapshot.clone();
    let row_data = Arc::make_mut(&mut edited_snapshot.row_data);
    let row = Arc::make_mut(&mut row_data[0]);
    row.signature = row.signature.wrapping_add(1);
    assert_ne!(
        key,
        terminal_keyword_highlight_request_key(&edited_snapshot, rules_key, rows)
    );

    let visible_rows = 0..1;
    let visible_key =
        terminal_keyword_highlight_request_key(&snapshot, rules_key, visible_rows.clone());
    let mut edited_outside_visible_rows = snapshot.clone();
    let rows = Arc::make_mut(&mut edited_outside_visible_rows.row_data);
    let row = Arc::make_mut(&mut rows[1]);
    row.signature = row.signature.wrapping_add(1);
    assert_eq!(
        visible_key,
        terminal_keyword_highlight_request_key(
            &edited_outside_visible_rows,
            rules_key,
            visible_rows,
        )
    );
}

#[test]
fn keyword_highlight_request_key_tracks_wrapped_row_context() {
    let mut snapshot = TerminalScreen::new(3, 4).snapshot();
    {
        let rows = Arc::make_mut(&mut snapshot.row_data);
        Arc::make_mut(&mut rows[0]).signature = 41;
        Arc::make_mut(&mut rows[1]).signature = 42;
    }
    let rules_key = 7;
    let plain_key = terminal_keyword_highlight_request_key(&snapshot, rules_key, 1..2);

    let mut wrapped = snapshot.clone();
    {
        let rows = Arc::make_mut(&mut wrapped.row_data);
        Arc::make_mut(&mut rows[1]).wrapped = true;
    }
    let wrapped_key = terminal_keyword_highlight_request_key(&wrapped, rules_key, 1..2);
    let expanded_key = terminal_keyword_highlight_request_key(&wrapped, rules_key, 0..2);

    assert_ne!(plain_key, wrapped_key);
    assert_eq!(wrapped_key, expanded_key);
    assert_eq!(wrapped_key.row_start, 0);
    assert_eq!(wrapped_key.row_end, 2);
}

#[test]
fn keyword_highlight_request_key_tracks_osc133_without_text_changes() {
    let mut screen = TerminalScreen::new(40, 2);
    screen.advance(b"ERROR");
    let output = screen.snapshot();
    let before = terminal_keyword_highlight_request_key(&output, 7, 0..1);
    screen.advance(b"\r\x1b]133;B\x07");
    let input = screen.snapshot();
    assert_eq!(
        output.row(0).unwrap().signature,
        input.row(0).unwrap().signature
    );
    assert_eq!(
        output.row(0).unwrap().revision,
        input.row(0).unwrap().revision
    );
    assert_ne!(
        before,
        terminal_keyword_highlight_request_key(&input, 7, 0..1)
    );
    let mut shifted = input.clone();
    let rows = Arc::make_mut(&mut shifted.row_data);
    Arc::make_mut(&mut rows[0]).shell_input_columns = Some((2, 40));
    assert_ne!(
        terminal_keyword_highlight_request_key(&input, 7, 0..1),
        terminal_keyword_highlight_request_key(&shifted, 7, 0..1)
    );
}

#[test]
fn keyword_highlight_visible_rows_bound_retained_scroll_work() {
    let mut screen = TerminalScreen::new(80, 24);
    screen.advance_decoded_text(&terminal_test_output_lines(240));
    let snapshot = screen.viewport_snapshot_with_window(80, 64, 64);
    let viewport_rows = snapshot.viewport_rows;
    let rows = terminal_keyword_highlight_visible_rows(
        &snapshot,
        80,
        viewport_rows,
        snapshot.scrollback_len,
    );
    let anchor = terminal_snapshot_anchor_row_for_display_offset(
        &snapshot,
        80,
        viewport_rows,
        snapshot.scrollback_len,
    );

    assert!(rows.contains(&anchor));
    assert!(rows.len() <= viewport_rows.saturating_add(2));
    assert!(snapshot.row_count() > rows.len());
    assert_ne!(
        rows,
        terminal_keyword_highlight_visible_rows(
            &snapshot,
            81,
            viewport_rows,
            snapshot.scrollback_len,
        )
    );
}

#[test]
fn keyword_highlight_prefetch_covers_two_viewports_of_local_scroll() {
    let mut screen = TerminalScreen::new(80, 24);
    screen.advance_decoded_text(&terminal_test_output_lines(240));
    let snapshot = screen.viewport_snapshot_with_window(80, 64, 64);
    let viewport_rows = snapshot.viewport_rows;
    let rows = terminal_keyword_highlight_prefetch_rows(
        &snapshot,
        80,
        viewport_rows,
        snapshot.scrollback_len,
        2,
    );

    for display_offset in 80..=80usize.saturating_add(viewport_rows.saturating_mul(2)) {
        let visible_rows = terminal_keyword_highlight_visible_rows(
            &snapshot,
            display_offset,
            viewport_rows,
            snapshot.scrollback_len,
        );
        assert!(rows.start <= visible_rows.start);
        assert!(rows.end >= visible_rows.end);
    }
    assert!(rows.len() <= viewport_rows.saturating_mul(5).saturating_add(2));
    assert!(snapshot.row_count() > rows.len());
}

#[test]
fn keyword_highlight_prefetch_uses_visible_window_under_output_pressure() {
    let mut screen = TerminalScreen::new(80, 24);
    screen.advance_decoded_text(&terminal_test_output_lines(240));
    let snapshot = screen.viewport_snapshot_with_window(80, 64, 64);
    let viewport_rows = snapshot.viewport_rows;

    let rows = terminal_keyword_highlight_prefetch_rows(
        &snapshot,
        80,
        viewport_rows,
        snapshot.scrollback_len,
        0,
    );
    let visible_rows = terminal_keyword_highlight_visible_rows(
        &snapshot,
        80,
        viewport_rows,
        snapshot.scrollback_len,
    );

    assert_eq!(rows, visible_rows);
    assert!(rows.len() <= viewport_rows.saturating_add(2));
}

#[test]
fn keyword_highlight_prefetch_viewports_follow_output_pressure() {
    assert_eq!(
        terminal_keyword_highlight_prefetch_viewports(false),
        TERMINAL_KEYWORD_HIGHLIGHT_PREFETCH_VIEWPORTS
    );
    assert_eq!(
        terminal_keyword_highlight_prefetch_viewports(true),
        TERMINAL_KEYWORD_HIGHLIGHT_PRESSURE_PREFETCH_VIEWPORTS
    );
}

#[test]
fn keyword_highlight_pressure_delay_uses_throttle_window() {
    assert_eq!(
        terminal_keyword_highlight_pressure_delay(false, Some(Duration::ZERO)),
        None
    );
    assert_eq!(terminal_keyword_highlight_pressure_delay(true, None), None);
    assert_eq!(
        terminal_keyword_highlight_pressure_delay(true, Some(Duration::ZERO)),
        Some(TERMINAL_KEYWORD_HIGHLIGHT_PRESSURE_THROTTLE)
    );
    assert_eq!(
        terminal_keyword_highlight_pressure_delay(
            true,
            Some(TERMINAL_KEYWORD_HIGHLIGHT_PRESSURE_THROTTLE + Duration::from_millis(1)),
        ),
        None
    );
}

#[test]
fn keyword_highlight_deferred_task_coalesces_latest_snapshot_under_pressure() {
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let surface = cx.new(|_| TerminalSurface::new("session"));
    let mut first_screen = TerminalScreen::default();
    first_screen.advance_decoded_text("alpha\n");
    let first_snapshot = Arc::new(first_screen.viewport_snapshot(0));
    let mut second_screen = TerminalScreen::default();
    second_screen.advance_decoded_text("beta\n");
    let second_snapshot = Arc::new(second_screen.viewport_snapshot(0));
    let rules = Arc::new(vec![nyaterm_core::ResolvedKeywordHighlightRule {
        id: "beta".to_string(),
        name: "Beta".to_string(),
        patterns: vec!["beta".to_string()],
        color: "#ff0000".to_string(),
        enabled: true,
    }]);

    cx.update_entity(&surface, |surface, cx| {
        surface.keyword_rules = rules.clone();
        apply_test_frame_snapshot!(
            surface,
            first_snapshot.clone(),
            0,
            0.0,
            0,
            first_snapshot.scrollback_len,
            first_snapshot.viewport_rows,
            false,
            None,
            0,
            false,
            false,
            "block",
        );
        surface.keyword_highlight_last_started_at = Some(Instant::now());
        surface.schedule_keyword_highlights(false, true, cx);
        assert!(surface.keyword_highlights.is_none());
        assert!(surface.keyword_highlight_deferred_task.is_some());
        let first_key = surface.keyword_highlight_pending_key;

        apply_test_frame_snapshot!(
            surface,
            second_snapshot.clone(),
            0,
            0.0,
            0,
            second_snapshot.scrollback_len,
            second_snapshot.viewport_rows,
            false,
            None,
            0,
            false,
            false,
            "block",
        );
        surface.schedule_keyword_highlights(false, true, cx);
        assert!(surface.keyword_highlight_deferred_task.is_some());
        assert_ne!(surface.keyword_highlight_pending_key, first_key);
        surface.keyword_highlight_last_started_at = None;
    });

    cx.executor()
        .advance_clock(TERMINAL_KEYWORD_HIGHLIGHT_PRESSURE_THROTTLE);
    cx.run_until_parked();

    cx.read_entity(&surface, |surface, _| {
        assert!(surface.keyword_highlight_deferred_task.is_none());
        assert!(surface.keyword_highlight_task.is_none());
        let highlights = surface
            .keyword_highlights
            .as_ref()
            .expect("deferred task should publish latest highlights");
        assert!(highlights.matches_snapshot_rows(
            second_snapshot.as_ref(),
            surface.palette,
            0..second_snapshot.row_count(),
        ));
        assert_eq!(highlights.range_count(), 1);
    });
}

#[test]
fn cancelling_pending_keyword_highlights_keeps_published_highlights() {
    let mut screen = TerminalScreen::default();
    screen.advance_decoded_text("alpha\nbeta\n");
    let snapshot = screen.viewport_snapshot(0);
    let rules = vec![nyaterm_core::ResolvedKeywordHighlightRule {
        id: "alpha".to_string(),
        name: "Alpha".to_string(),
        patterns: vec!["alpha".to_string()],
        color: "#ff0000".to_string(),
        enabled: true,
    }];
    let rules = Arc::new(rules);
    let highlighter = Arc::new(compile_terminal_keyword_highlighter(
        rules.as_ref(),
        nyaterm_terminal_gpui::TerminalKeywordHighlightConfig {
            line_roles: true,
            ansi: nyaterm_ui::theme_palette("github-dark").terminal_ansi,
        },
    ));
    let highlights = Arc::new(precompute_terminal_keyword_highlights(
        &snapshot,
        highlighter.as_ref(),
        nyaterm_ui::theme_palette("github-dark"),
        None,
    ));
    let pending_key = terminal_keyword_highlight_request_key(
        &snapshot,
        terminal_keyword_rules_key(&rules),
        0..snapshot.row_count(),
    );
    let mut surface = TerminalSurface::new("session");
    surface.keyword_highlight_generation = 41;
    surface.keyword_highlight_pending_key = Some(pending_key);
    surface.keyword_highlighter_rules = Some(rules);
    surface.keyword_highlighter = Some(highlighter);
    surface.keyword_highlights = Some(highlights.clone());

    surface.cancel_pending_keyword_highlights();

    assert_eq!(surface.keyword_highlight_generation, 42);
    assert!(surface.keyword_highlight_pending_key.is_none());
    assert!(surface.keyword_highlight_task.is_none());
    assert!(
        surface
            .keyword_highlights
            .as_ref()
            .is_some_and(|stored| Arc::ptr_eq(stored, &highlights))
    );
    assert!(surface.keyword_highlighter_rules.is_some());
    assert!(surface.keyword_highlighter.is_some());
}

#[test]
fn transient_empty_keyword_rules_keep_pending_highlights() {
    let mut surface = TerminalSurface::new("session");
    let pending_key = TerminalKeywordHighlightRequestKey {
        rules_key: 7,
        display_offset: 0,
        row_start: 0,
        row_end: 1,
        line_signatures_key: 9,
    };
    surface.keyword_rules = Arc::new(Vec::new());
    surface.keyword_highlight_generation = 41;
    surface.keyword_highlight_pending_key = Some(pending_key);
    surface.keyword_highlighter_rules = Some(Arc::new(Vec::new()));
    surface.keyword_highlighter = Some(Arc::new(compile_terminal_keyword_highlighter(
        &[],
        nyaterm_terminal_gpui::TerminalKeywordHighlightConfig {
            line_roles: true,
            ansi: nyaterm_ui::theme_palette("github-dark").terminal_ansi,
        },
    )));
    surface.keyword_highlights = Some(Arc::new(precompute_terminal_keyword_highlights(
        &TerminalScreen::default().viewport_snapshot(0),
        surface.keyword_highlighter.as_ref().unwrap().as_ref(),
        nyaterm_ui::theme_palette("github-dark"),
        None,
    )));

    assert!(surface.handle_empty_keyword_rules_for_highlights(false));
    assert_eq!(surface.keyword_highlight_generation, 41);
    assert_eq!(surface.keyword_highlight_pending_key, Some(pending_key));
    assert!(surface.keyword_highlights.is_some());
    assert!(surface.keyword_highlighter.is_some());
}

#[test]
fn configured_empty_keyword_rules_clear_pending_highlights() {
    let mut surface = TerminalSurface::new("session");
    let pending_key = TerminalKeywordHighlightRequestKey {
        rules_key: 7,
        display_offset: 0,
        row_start: 0,
        row_end: 1,
        line_signatures_key: 9,
    };
    surface.keyword_rules = Arc::new(Vec::new());
    surface.keyword_highlight_generation = 41;
    surface.keyword_highlight_pending_key = Some(pending_key);
    surface.keyword_highlighter_rules = Some(Arc::new(Vec::new()));
    surface.keyword_highlighter = Some(Arc::new(compile_terminal_keyword_highlighter(
        &[],
        nyaterm_terminal_gpui::TerminalKeywordHighlightConfig {
            line_roles: true,
            ansi: nyaterm_ui::theme_palette("github-dark").terminal_ansi,
        },
    )));
    surface.keyword_highlights = Some(Arc::new(precompute_terminal_keyword_highlights(
        &TerminalScreen::default().viewport_snapshot(0),
        surface.keyword_highlighter.as_ref().unwrap().as_ref(),
        nyaterm_ui::theme_palette("github-dark"),
        None,
    )));

    assert!(surface.handle_empty_keyword_rules_for_highlights(true));
    assert_eq!(surface.keyword_highlight_generation, 42);
    assert!(surface.keyword_highlight_pending_key.is_none());
    assert!(surface.keyword_highlights.is_none());
    assert!(surface.keyword_highlighter.is_none());
    assert!(surface.keyword_highlighter_rules.is_none());
}

#[test]
fn cached_keyword_highlighter_reuses_equal_rule_sets() {
    let cached_rules = Arc::new(vec![nyaterm_core::ResolvedKeywordHighlightRule {
        id: "alpha".to_string(),
        name: "Alpha".to_string(),
        patterns: vec!["alpha".to_string()],
        color: "#ff0000".to_string(),
        enabled: true,
    }]);
    let equivalent_rules = Arc::new(cached_rules.as_ref().clone());
    assert!(!Arc::ptr_eq(&cached_rules, &equivalent_rules));
    let highlighter = Arc::new(compile_terminal_keyword_highlighter(
        cached_rules.as_ref(),
        nyaterm_terminal_gpui::TerminalKeywordHighlightConfig {
            line_roles: true,
            ansi: nyaterm_ui::theme_palette("github-dark").terminal_ansi,
        },
    ));
    let mut surface = TerminalSurface::new("session");
    surface.keyword_highlighter_rules = Some(cached_rules);
    surface.keyword_highlighter = Some(highlighter.clone());

    assert!(
        surface
            .cached_keyword_highlighter_for_rules(&equivalent_rules)
            .is_some_and(|cached| Arc::ptr_eq(&cached, &highlighter))
    );
}

#[test]
fn applying_identical_scroll_visual_state_is_a_noop() {
    let snapshot = Arc::new(TerminalScreen::default().viewport_snapshot(0));
    let rows = snapshot.row_count();
    let mut surface = TerminalSurface::new("session");
    apply_test_frame_snapshot!(
        surface, snapshot, 0, 0.0, 0, 10, rows, false, None, 0, false, false, "block",
    );
    let state = TerminalScrollVisualState {
        history: surface.history,
        session_id: "session".to_string(),
        scroll_offset: 0,
        scroll_residual_lines: 0.0,
        display_offset: 0,
        scrollback_len: 10,
        viewport_rows: rows,
        has_new_while_scrolled: false,
        performance_overlay: None,
        skipped_output_chars: 0,
    };
    let revision_before = surface.revision;

    assert!(!surface.apply_scroll_visual_state(state));
    assert_eq!(surface.revision, revision_before);
}

#[test]
fn repeated_pending_scroll_visual_state_does_not_need_repaint() {
    let snapshot = Arc::new(TerminalScreen::default().viewport_snapshot(0));
    let rows = snapshot.row_count();
    let mut surface = TerminalSurface::new("session");
    apply_test_frame_snapshot!(
        surface, snapshot, 0, 0.0, 0, 10, rows, false, None, 0, false, false, "block",
    );
    let state = TerminalScrollVisualState {
        history: surface.history,
        session_id: "session".to_string(),
        scroll_offset: 1,
        scroll_residual_lines: 0.0,
        display_offset: 1,
        scrollback_len: 10,
        viewport_rows: rows,
        has_new_while_scrolled: false,
        performance_overlay: None,
        skipped_output_chars: 0,
    };

    assert!(surface.scroll_visual_state_needs_repaint(&state));
    assert!(!surface.apply_scroll_visual_state(state.clone()));

    assert!(!surface.scroll_visual_state_needs_repaint(&state));
    assert!(surface.scroll_snapshot_pending);
}

#[test]
fn scroll_without_target_snapshot_preserves_stale_paint_state() {
    let snapshot = Arc::new(TerminalScreen::default().viewport_snapshot(0));
    let rows = snapshot.row_count();
    let mut surface = TerminalSurface::new("session");
    surface.keyword_rules = Arc::new(vec![nyaterm_core::ResolvedKeywordHighlightRule {
        id: "test".to_string(),
        name: "test".to_string(),
        patterns: vec!["test".to_string()],
        color: "#ff0000".to_string(),
        enabled: true,
    }]);
    surface.set_decorations_and_keywords(
        vec![TerminalLineDecorations {
            link_ranges: vec![(1, 3)],
            ..TerminalLineDecorations::default()
        }],
        surface.keyword_rules.clone(),
        true,
        "block",
    );

    apply_test_frame_snapshot!(
        surface, snapshot, 0, 0.0, 0, 10, rows, false, None, 0, true, true, "block",
    );
    surface.update_scroll_chrome_without_snapshot(&TerminalScrollVisualState {
        history: surface.history,
        session_id: "session".to_string(),
        scroll_offset: 1,
        scroll_residual_lines: 0.0,
        display_offset: 1,
        scrollback_len: 10,
        viewport_rows: rows,
        has_new_while_scrolled: false,
        performance_overlay: None,
        skipped_output_chars: 0,
    });

    assert_eq!(surface.display_offset, 0);
    assert!(surface.scroll_snapshot_pending);
    assert!(!surface.keyword_rules.is_empty());
    assert_eq!(surface.decorations[0].link_ranges, vec![(1, 3)]);
    assert!(surface.has_action_link_decorations);
    assert_eq!(
        terminal_effective_visual_scroll_offset_px(TerminalVisualScrollGeometry {
            snapshot_pending: surface.scroll_snapshot_pending,
            target_offset: surface.scroll_offset,
            displayed_offset: surface.display_offset,
            residual_lines: surface.scroll_residual_lines,
            viewport_anchor_row: 0,
            snapshot_rows: rows,
            viewport_rows: rows,
            cell_height: 16.0,
        },),
        0.0
    );
}

#[test]
fn degraded_empty_decorations_preserve_stale_paint_state() {
    let snapshot = Arc::new(TerminalScreen::default().viewport_snapshot(0));
    let rows = snapshot.row_count();
    let mut surface = TerminalSurface::new("session");

    apply_test_frame_snapshot!(
        surface, snapshot, 0, 0.0, 0, 10, rows, false, None, 0, false, true, "block",
    );
    surface.set_decorations_and_keywords(
        vec![TerminalLineDecorations {
            search_ranges: vec![(2, 5)],
            link_ranges: vec![(1, 3)],
            ..TerminalLineDecorations::default()
        }],
        Arc::new(Vec::new()),
        true,
        "block",
    );

    surface.set_decorations_and_keywords_preserving_stale(
        Vec::new(),
        Arc::new(Vec::new()),
        true,
        "beam",
        true,
    );

    assert_eq!(surface.decorations[0].search_ranges, vec![(2, 5)]);
    assert_eq!(surface.decorations[0].link_ranges, vec![(1, 3)]);
    assert_eq!(surface.cursor_style, "beam");

    surface.set_decorations_and_keywords_preserving_stale(
        vec![TerminalLineDecorations {
            active_search_ranges: vec![(6, 8)],
            ..TerminalLineDecorations::default()
        }],
        Arc::new(Vec::new()),
        true,
        "underline",
        true,
    );

    assert!(surface.decorations[0].search_ranges.is_empty());
    assert_eq!(surface.decorations[0].active_search_ranges, vec![(6, 8)]);
    assert_eq!(surface.cursor_style, "underline");
}

#[test]
fn empty_decorations_clear_stale_links_when_preserve_not_allowed() {
    let snapshot = Arc::new(TerminalScreen::default().viewport_snapshot(0));
    let rows = snapshot.row_count();
    let mut surface = TerminalSurface::new("session");

    apply_test_frame_snapshot!(
        surface, snapshot, 0, 0.0, 0, 10, rows, false, None, 0, true, true, "block",
    );
    surface.set_decorations_and_keywords(
        vec![TerminalLineDecorations {
            link_ranges: vec![(1, 3)],
            ..TerminalLineDecorations::default()
        }],
        Arc::new(Vec::new()),
        true,
        "block",
    );

    assert!(surface.set_decorations_and_keywords_preserving_stale(
        Vec::new(),
        Arc::new(Vec::new()),
        true,
        "block",
        false,
    ));

    assert!(surface.decorations.is_empty());
}

#[test]
fn restored_keyword_rules_report_paint_detail_change_without_new_frame() {
    let snapshot = Arc::new(TerminalScreen::default().viewport_snapshot(0));
    let rows = snapshot.row_count();
    let mut surface = TerminalSurface::new("session");
    let rules = Arc::new(vec![nyaterm_core::ResolvedKeywordHighlightRule {
        id: "alpha".to_string(),
        name: "Alpha".to_string(),
        patterns: vec!["alpha".to_string()],
        color: "#ff0000".to_string(),
        enabled: true,
    }]);
    let decorations = vec![TerminalLineDecorations {
        search_ranges: vec![(1, 5)],
        ..TerminalLineDecorations::default()
    }];

    assert!(apply_test_frame_snapshot!(
        surface,
        snapshot.clone(),
        0,
        0.0,
        0,
        10,
        rows,
        false,
        None,
        0,
        false,
        true,
        "block",
    ));
    assert!(surface.set_decorations_and_keywords(
        decorations.clone(),
        Arc::new(Vec::new()),
        true,
        "block",
    ));

    assert!(!apply_test_frame_snapshot!(
        surface, snapshot, 0, 0.0, 0, 10, rows, false, None, 0, false, true, "block",
    ));
    assert!(surface.set_decorations_and_keywords(decorations, rules.clone(), true, "block",));
    assert_eq!(surface.keyword_rules.as_ref(), rules.as_ref());
    assert!(!surface.set_decorations_and_keywords(
        surface.decorations.to_vec(),
        rules,
        true,
        "block",
    ));
}

#[test]
fn pending_scroll_uses_retained_rows_before_target_snapshot_arrives() {
    assert_eq!(
        terminal_effective_visual_scroll_offset_px(TerminalVisualScrollGeometry {
            snapshot_pending: true,
            target_offset: 9,
            displayed_offset: 0,
            residual_lines: 0.0,
            viewport_anchor_row: 12,
            snapshot_rows: 40,
            viewport_rows: 20,
            cell_height: 16.0,
        }),
        144.0
    );
    assert_eq!(
        terminal_effective_visual_scroll_offset_px(TerminalVisualScrollGeometry {
            snapshot_pending: true,
            target_offset: 40,
            displayed_offset: 0,
            residual_lines: 0.0,
            viewport_anchor_row: 12,
            snapshot_rows: 40,
            viewport_rows: 20,
            cell_height: 16.0,
        }),
        192.0
    );
    assert_eq!(
        terminal_effective_visual_scroll_offset_px(TerminalVisualScrollGeometry {
            snapshot_pending: true,
            target_offset: 0,
            displayed_offset: 6,
            residual_lines: -3.0,
            viewport_anchor_row: 12,
            snapshot_rows: 40,
            viewport_rows: 20,
            cell_height: 16.0,
        }),
        -128.0
    );
}

#[test]
fn pending_scroll_with_live_retained_rows_aligns_target_viewport() {
    let (snapshot, viewport_rows) = terminal_test_retained_live_snapshot(160);
    let target_offset = viewport_rows;
    let anchor = terminal_snapshot_anchor_row_for_display_offset(
        &snapshot,
        0,
        viewport_rows,
        snapshot.scrollback_len,
    );
    assert!(anchor >= viewport_rows);

    let cell_h = 16.0;
    let visual_y_offset =
        terminal_effective_visual_scroll_offset_px(TerminalVisualScrollGeometry {
            snapshot_pending: true,
            target_offset,
            displayed_offset: 0,
            residual_lines: 0.0,
            viewport_anchor_row: anchor,
            snapshot_rows: snapshot.row_count(),
            viewport_rows,
            cell_height: cell_h,
        }) - anchor as f32 * cell_h;
    let target_anchor = terminal_snapshot_anchor_row_for_display_offset(
        &snapshot,
        target_offset,
        viewport_rows,
        snapshot.scrollback_len,
    );

    assert_eq!(visual_y_offset, -(target_anchor as f32) * cell_h);
}

#[test]
fn gutter_visible_rows_follow_visual_scroll_window() {
    assert_eq!(
        terminal_surface_visible_rows_for_viewport(20, 200, 0.0, 16.0),
        0..21
    );
    assert_eq!(
        terminal_surface_visible_rows_for_viewport(20, 200, -160.0, 16.0),
        9..31
    );
    assert_eq!(
        terminal_surface_visible_rows_for_viewport(20, 200, 80.0, 16.0),
        0..16
    );
}

#[test]
fn gutter_visible_rows_clamp_large_retained_windows() {
    let rows = terminal_surface_visible_rows_for_viewport(40, 1200, -8000.0, 16.0);

    assert!(rows.start >= 499);
    assert!(rows.end <= 542);
    assert!(rows.len() <= 43);
}

#[test]
fn matching_scroll_snapshot_clears_pending_visual_freeze() {
    let snapshot = Arc::new(TerminalScreen::default().viewport_snapshot(0));
    let rows = snapshot.row_count();
    let mut surface = TerminalSurface::new("session");

    apply_test_frame_snapshot!(
        surface,
        snapshot.clone(),
        0,
        0.0,
        0,
        10,
        rows,
        false,
        None,
        0,
        true,
        true,
        "block",
    );
    surface.update_scroll_chrome_without_snapshot(&TerminalScrollVisualState {
        history: surface.history,
        session_id: "session".to_string(),
        scroll_offset: 1,
        scroll_residual_lines: 0.0,
        display_offset: 1,
        scrollback_len: 10,
        viewport_rows: rows,
        has_new_while_scrolled: false,
        performance_overlay: None,
        skipped_output_chars: 0,
    });
    assert!(surface.scroll_snapshot_pending);

    apply_test_frame_snapshot!(
        surface, snapshot, 1, 0.0, 1, 10, rows, false, None, 0, true, true, "block",
    );

    assert!(!surface.scroll_snapshot_pending);
    assert_eq!(surface.display_offset, 1);
    assert_eq!(
        terminal_effective_visual_scroll_offset_px(TerminalVisualScrollGeometry {
            snapshot_pending: surface.scroll_snapshot_pending,
            target_offset: surface.scroll_offset,
            displayed_offset: surface.display_offset,
            residual_lines: surface.scroll_residual_lines,
            viewport_anchor_row: 0,
            snapshot_rows: rows,
            viewport_rows: rows,
            cell_height: 16.0,
        },),
        0.0
    );
}

#[test]
fn repeated_pending_scroll_chrome_update_is_noop() {
    let mut screen = TerminalScreen::default();
    screen.advance_decoded_text(&terminal_test_output_lines(80));
    let snapshot = Arc::new(screen.viewport_snapshot(0));
    let rows = snapshot.row_count();
    let scrollback_len = snapshot.scrollback_len;
    let mut surface = TerminalSurface::new("session");

    apply_test_frame_snapshot!(
        surface,
        snapshot,
        0,
        0.0,
        0,
        scrollback_len,
        rows,
        false,
        None,
        0,
        false,
        false,
        "block",
    );

    assert!(
        surface.update_scroll_chrome_without_snapshot(&TerminalScrollVisualState {
            history: screen.history_state(),
            session_id: "session".to_string(),
            scroll_offset: 1,
            scroll_residual_lines: 0.0,
            display_offset: 1,
            scrollback_len,
            viewport_rows: rows,
            has_new_while_scrolled: false,
            performance_overlay: None,
            skipped_output_chars: 0,
        })
    );
    assert!(surface.scroll_snapshot_pending);
    let revision_before_repeat = surface.revision;

    assert!(
        !surface.update_scroll_chrome_without_snapshot(&TerminalScrollVisualState {
            history: screen.history_state(),
            session_id: "session".to_string(),
            scroll_offset: 1,
            scroll_residual_lines: 0.0,
            display_offset: 1,
            scrollback_len,
            viewport_rows: rows,
            has_new_while_scrolled: false,
            performance_overlay: None,
            skipped_output_chars: 0,
        })
    );
    assert_eq!(surface.revision, revision_before_repeat);
    assert!(surface.scroll_snapshot_pending);
}

#[test]
fn identical_frame_snapshot_update_is_noop() {
    let snapshot = Arc::new(TerminalScreen::default().viewport_snapshot(0));
    let rows = snapshot.row_count();
    let scrollback_len = snapshot.scrollback_len;
    let mut surface = TerminalSurface::new("session");

    assert!(apply_test_frame_snapshot!(
        surface,
        snapshot.clone(),
        0,
        0.0,
        0,
        scrollback_len,
        rows,
        false,
        None,
        0,
        false,
        false,
        "block",
    ));
    let revision_before_repeat = surface.revision;

    assert!(!apply_test_frame_snapshot!(
        surface,
        snapshot,
        0,
        0.0,
        0,
        scrollback_len,
        rows,
        false,
        None,
        0,
        false,
        false,
        "block",
    ));
    assert_eq!(surface.revision, revision_before_repeat);
}

#[test]
fn local_surface_scroll_state_reuses_covering_snapshot_immediately() {
    let (snapshot, rows) = terminal_test_retained_live_snapshot(80);
    let snapshot = Arc::new(snapshot);
    let scrollback_len = snapshot.scrollback_len;
    let mut surface = TerminalSurface::new("session");

    apply_test_frame_snapshot!(
        surface,
        snapshot,
        0,
        0.0,
        0,
        scrollback_len,
        rows,
        false,
        None,
        0,
        false,
        false,
        "block",
    );
    let text_updated = surface.apply_scroll_visual_state(TerminalScrollVisualState {
        history: surface.history,
        session_id: "session".to_string(),
        scroll_offset: 1,
        scroll_residual_lines: 0.25,
        display_offset: 1,
        scrollback_len,
        viewport_rows: rows,
        has_new_while_scrolled: true,
        performance_overlay: None,
        skipped_output_chars: 7,
    });

    assert!(text_updated);
    assert!(!surface.scroll_snapshot_pending);
    assert_eq!(surface.display_offset, 1);
    assert_eq!(surface.scroll_residual_lines, 0.25);
    assert!(surface.has_new_while_scrolled);
    assert_eq!(surface.skipped_output_chars, 7);
}

#[test]
fn promoting_current_scroll_window_does_not_retain_it_again() {
    let (snapshot, rows) = terminal_test_retained_live_snapshot(80);
    let snapshot = Arc::new(snapshot);
    let scrollback_len = snapshot.scrollback_len;
    let mut surface = TerminalSurface::new("session");
    surface.snapshot = Some(snapshot.clone());
    surface.history = snapshot.history;

    assert!(surface.promote_snapshot_covering_display_offset(1, rows, scrollback_len));
    assert!(surface.retained_snapshots.is_empty());
    assert!(surface.retained_rows.is_empty());
    assert!(Arc::ptr_eq(surface.snapshot.as_ref().unwrap(), &snapshot));
}

#[test]
fn local_surface_wheel_updates_visual_state_before_app_sync() {
    let (snapshot, rows) = terminal_test_retained_live_snapshot(80);
    let snapshot = Arc::new(snapshot);
    let scrollback_len = snapshot.scrollback_len;
    let mut surface = TerminalSurface::new("session");

    apply_test_frame_snapshot!(
        surface,
        snapshot,
        0,
        0.0,
        0,
        scrollback_len,
        rows,
        false,
        None,
        0,
        false,
        false,
        "block",
    );

    assert!(surface.can_handle_scroll_wheel_locally());
    assert_eq!(
        surface.apply_local_scroll_wheel_visual_state(0.35),
        Some(TerminalSurfaceLocalScrollResult {
            generation: 1,
            visual_changed: true,
            needs_text_snapshot: false,
        })
    );
    assert_eq!(surface.scroll_offset, 0);
    assert!((surface.scroll_residual_lines - 0.35).abs() < f32::EPSILON * 8.0);
    assert_eq!(surface.display_offset, 0);

    assert_eq!(
        surface.apply_local_scroll_wheel_visual_state(0.70),
        Some(TerminalSurfaceLocalScrollResult {
            generation: 2,
            visual_changed: true,
            needs_text_snapshot: false,
        })
    );
    assert_eq!(surface.scroll_offset, 1);
    assert!((surface.scroll_residual_lines - 0.05).abs() < f32::EPSILON * 8.0);
    assert_eq!(surface.display_offset, 1);
    assert!(!surface.scroll_snapshot_pending);
}

#[test]
fn local_surface_fractional_wheel_keeps_live_text_window_stable() {
    let snapshot = Arc::new(TerminalScreen::default().viewport_snapshot(0));
    let rows = snapshot.row_count();
    let mut surface = TerminalSurface::new("session");

    apply_test_frame_snapshot!(
        surface, snapshot, 0, 0.0, 0, 10, rows, false, None, 0, false, false, "block",
    );

    let result = surface
        .apply_local_scroll_wheel_visual_state(0.60)
        .expect("local scroll result");

    assert!(result.visual_changed);
    assert!(!result.needs_text_snapshot);
    assert_eq!(surface.scroll_offset, 0);
    assert!((surface.scroll_residual_lines - 0.60).abs() < f32::EPSILON * 8.0);
    assert_eq!(surface.display_offset, 0);
    assert!(!surface.scroll_snapshot_pending);
}

#[test]
fn local_surface_fractional_scroll_counts_as_visual_scroll_for_cursor() {
    let snapshot = Arc::new(TerminalScreen::default().viewport_snapshot(0));
    let rows = snapshot.row_count();
    let mut surface = TerminalSurface::new("session");

    apply_test_frame_snapshot!(
        surface,
        snapshot.clone(),
        0,
        0.0,
        0,
        10,
        rows,
        false,
        None,
        0,
        false,
        true,
        "block",
    );
    assert!(surface.show_cursor);

    surface
        .apply_local_scroll_wheel_visual_state(0.60)
        .expect("local scroll result");
    assert!(surface.visual_scroll_active());
    assert!(!surface.show_cursor);

    surface.set_decorations_and_keywords(Vec::new(), Arc::new(Vec::new()), true, "block");
    assert!(!surface.show_cursor);

    apply_test_frame_snapshot!(
        surface, snapshot, 0, 0.60, 0, 10, rows, false, None, 0, false, true, "block",
    );
    assert!(!surface.show_cursor);
}

#[test]
fn selection_visual_update_preserves_existing_decorations() {
    let snapshot = Arc::new(TerminalScreen::default().viewport_snapshot(0));
    let rows = snapshot.row_count();
    let mut surface = TerminalSurface::new("session");

    apply_test_frame_snapshot!(
        surface,
        snapshot.clone(),
        0,
        0.0,
        0,
        10,
        rows,
        false,
        None,
        0,
        false,
        true,
        "block",
    );
    surface.set_decorations_and_keywords(
        vec![TerminalLineDecorations {
            link_ranges: vec![(1, 3)],
            ..TerminalLineDecorations::default()
        }],
        Arc::new(Vec::new()),
        true,
        "block",
    );
    let static_decorations = surface.decorations.clone();
    let selection = TerminalSelection::from_range(
        TerminalBufferCellPos::new(0, 2),
        TerminalBufferCellPos::new(1, usize::MAX),
    );

    assert!(surface.set_selection_visual(Some(selection)));
    assert_eq!(surface.selection_visual, Some(selection));
    assert_eq!(surface.decorations[0].link_ranges, vec![(1, 3)]);
    assert!(Arc::ptr_eq(&surface.decorations, &static_decorations));
    assert!(!surface.set_selection_visual(Some(selection)));

    assert!(surface.set_selection_visual(None));
    assert_eq!(surface.decorations[0].link_ranges, vec![(1, 3)]);
    assert!(Arc::ptr_eq(&surface.decorations, &static_decorations));
}

#[test]
#[ignore = "performance benchmark; run manually with --ignored --nocapture"]
fn dense_action_link_selection_drag_benchmark() {
    let mut surface = TerminalSurface::new("bench");
    let decorations = (0..5000)
        .map(|_| TerminalLineDecorations {
            link_ranges: vec![(1, 8), (12, 24), (30, 42)],
            ..TerminalLineDecorations::default()
        })
        .collect::<Vec<_>>();
    surface.set_decorations_and_keywords(decorations, Arc::new(Vec::new()), false, "block");
    let static_decorations = surface.decorations.clone();
    let started = Instant::now();
    let mut static_arc_replacements = 0usize;
    for col in 0..10_000 {
        surface.set_selection_visual(Some(TerminalSelection::from_range(
            TerminalBufferCellPos::new(2, 0),
            TerminalBufferCellPos::new(4, col % 80),
        )));
        static_arc_replacements +=
            usize::from(!Arc::ptr_eq(&surface.decorations, &static_decorations));
    }
    eprintln!(
        "dense action-link drag benchmark: updates=10000 total={:?} static_arc_replacements={static_arc_replacements}",
        started.elapsed()
    );
}

#[test]
#[ignore = "performance benchmark; run manually with --ignored --nocapture"]
fn overview_marker_fast_scroll_benchmark() {
    let mut surface = TerminalSurface::new("bench");
    let markers = (0..2000)
        .map(|index| TerminalOverviewMarker {
            absolute_line: index * 50,
            kind: TerminalOverviewMarkerKind::SelectedOccurrence,
        })
        .collect::<Vec<_>>();
    surface.set_overview_markers(markers.into(), 100_000, 9);
    let first = surface.overview_marker_buckets_for_track_height(800);
    let started = Instant::now();
    let mut bucket_arc_rebuilds = 0usize;
    for _ in 0..10_000 {
        let buckets = surface.overview_marker_buckets_for_track_height(800);
        bucket_arc_rebuilds += usize::from(!Arc::ptr_eq(&first, &buckets));
    }
    eprintln!(
        "overview marker scroll benchmark: paints=10000 total={:?} bucket_arc_rebuilds={bucket_arc_rebuilds}",
        started.elapsed()
    );
}

#[test]
fn empty_selection_press_and_release_preserve_action_link_decorations() {
    let snapshot = Arc::new(TerminalScreen::default().viewport_snapshot(0));
    let rows = snapshot.row_count();
    let mut surface = TerminalSurface::new("session");

    apply_test_frame_snapshot!(
        surface, snapshot, 0, 0.0, 0, 10, rows, false, None, 0, true, true, "block",
    );
    let decorations = vec![TerminalLineDecorations {
        selected_occurrence_ranges: vec![(0, 1)],
        search_ranges: vec![(1, 2)],
        active_search_ranges: vec![(2, 3)],
        link_ranges: vec![(3, 7)],
    }];
    surface.set_decorations_and_keywords(decorations.clone(), Arc::new(Vec::new()), true, "block");
    let static_decorations = surface.decorations.clone();
    let revision = surface.revision;
    let anchor = TerminalSelection::with_anchor(TerminalBufferCellPos::new(0, 4));

    // MouseDown records an empty anchor as O(1) dynamic state.
    assert!(surface.set_selection_visual(Some(anchor)));
    assert_eq!(surface.selection_visual, Some(anchor));
    assert_eq!(surface.decorations.as_ref(), decorations.as_slice());
    assert!(Arc::ptr_eq(&surface.decorations, &static_decorations));
    assert_eq!(surface.revision, revision.saturating_add(1));

    // MouseUp clears the empty selection and must leave the same underlines in place.
    assert!(surface.set_selection_visual(None));
    assert_eq!(surface.selection_visual, None);
    assert_eq!(surface.decorations.as_ref(), decorations.as_slice());
    assert!(Arc::ptr_eq(&surface.decorations, &static_decorations));
    assert_eq!(surface.revision, revision.saturating_add(2));
}

#[test]
fn local_surface_fractional_scroll_prefetches_adjacent_snapshot() {
    assert_eq!(
        terminal_surface_fractional_prefetch_offset(0, 0.35, 10),
        Some(1)
    );
    assert_eq!(
        terminal_surface_fractional_prefetch_offset(4, 0.35, 10),
        Some(5)
    );
    assert_eq!(
        terminal_surface_fractional_prefetch_offset(4, -0.35, 10),
        Some(3)
    );
    assert_eq!(
        terminal_surface_fractional_prefetch_offset(0, -0.35, 10),
        None
    );
    assert_eq!(
        terminal_surface_fractional_prefetch_offset(10, 0.35, 10),
        None
    );
    assert_eq!(
        terminal_surface_fractional_prefetch_offset(1, f32::NAN, 10),
        None
    );
}

#[test]
fn local_surface_snapshot_requests_are_deduped_until_covered() {
    let snapshot = Arc::new(TerminalScreen::default().viewport_snapshot(0));
    let rows = snapshot.row_count();
    let mut surface = TerminalSurface::new("session");

    apply_test_frame_snapshot!(
        surface, snapshot, 0, 0.0, 0, 10, rows, false, None, 0, false, false, "block",
    );

    assert_eq!(
        surface.scroll_snapshot_request_offsets_to_enqueue(vec![1, 1, 2]),
        vec![1, 2]
    );
    assert_eq!(
        surface.scroll_snapshot_request_offsets_to_enqueue(vec![1, 2]),
        Vec::<usize>::new()
    );

    let mut screen = TerminalScreen::default();
    screen.advance_decoded_text(&terminal_test_output_lines(80));
    let covering = Arc::new(screen.viewport_snapshot(1));
    let scrollback_len = covering.scrollback_len;
    apply_test_frame_snapshot!(
        surface,
        covering,
        1,
        0.0,
        1,
        scrollback_len,
        rows,
        false,
        None,
        0,
        false,
        false,
        "block",
    );

    assert!(!surface.pending_scroll_snapshot_offsets.contains(&1));
    assert_eq!(
        surface.scroll_snapshot_request_offsets_to_enqueue(vec![1, 3]),
        vec![3]
    );
}

#[test]
fn local_surface_snapshot_request_dedupe_resets_when_scrollback_changes() {
    let snapshot = Arc::new(TerminalScreen::default().viewport_snapshot(0));
    let rows = snapshot.row_count();
    let mut surface = TerminalSurface::new("session");

    apply_test_frame_snapshot!(
        surface, snapshot, 0, 0.0, 0, 10, rows, false, None, 0, false, false, "block",
    );
    assert_eq!(
        surface.scroll_snapshot_request_offsets_to_enqueue(vec![4]),
        vec![4]
    );
    assert_eq!(
        surface.scroll_snapshot_request_offsets_to_enqueue(vec![4]),
        Vec::<usize>::new()
    );

    surface.update_scroll_chrome_without_snapshot(&TerminalScrollVisualState {
        history: surface.history,
        session_id: "session".to_string(),
        scroll_offset: 5,
        scroll_residual_lines: 0.0,
        display_offset: 5,
        scrollback_len: 11,
        viewport_rows: rows,
        has_new_while_scrolled: true,
        performance_overlay: None,
        skipped_output_chars: 0,
    });

    assert_eq!(
        surface.scroll_snapshot_request_offsets_to_enqueue(vec![4, 5]),
        vec![4, 5]
    );
}

#[test]
fn local_surface_wheel_consumes_edge_noop_without_visual_sync() {
    let snapshot = Arc::new(TerminalScreen::default().viewport_snapshot(0));
    let rows = snapshot.row_count();
    let mut surface = TerminalSurface::new("session");

    let frame_applied = apply_test_frame_snapshot!(
        surface, snapshot, 0, 0.0, 0, 0, rows, false, None, 0, false, false, "block",
    );
    assert!(frame_applied);

    assert_eq!(
        surface.apply_local_scroll_wheel_visual_state(-0.5),
        Some(TerminalSurfaceLocalScrollResult {
            generation: 0,
            visual_changed: false,
            needs_text_snapshot: false,
        })
    );
    assert_eq!(surface.scroll_offset, 0);
    assert_eq!(surface.scroll_residual_lines, 0.0);
    assert_eq!(surface.scroll_interaction_generation, 0);
    assert!(!surface.scroll_snapshot_pending);
}

#[test]
fn local_surface_wheel_flags_missing_text_snapshot_immediately() {
    let snapshot = Arc::new(TerminalScreen::default().viewport_snapshot(0));
    let rows = snapshot.row_count();
    let mut surface = TerminalSurface::new("session");

    apply_test_frame_snapshot!(
        surface, snapshot, 0, 0.0, 0, 10, rows, false, None, 0, false, false, "block",
    );

    let result = surface
        .apply_local_scroll_wheel_visual_state(4.0)
        .expect("local scroll result");

    assert!(result.visual_changed);
    assert!(result.needs_text_snapshot);
    assert_eq!(surface.scroll_offset, 4);
    assert_eq!(surface.display_offset, 0);
    assert!(surface.scroll_snapshot_pending);
}

#[test]
fn pending_local_surface_scroll_survives_stale_full_frame_paint() {
    let (snapshot, rows) = terminal_test_retained_live_snapshot(80);
    let snapshot = Arc::new(snapshot);
    let scrollback_len = snapshot.scrollback_len;
    let mut surface = TerminalSurface::new("session");

    apply_test_frame_snapshot!(
        surface,
        snapshot.clone(),
        0,
        0.0,
        0,
        scrollback_len,
        rows,
        false,
        None,
        0,
        false,
        false,
        "block",
    );
    let result = surface
        .apply_local_scroll_wheel_visual_state(1.0)
        .expect("local scroll result");
    assert!(result.visual_changed);
    assert_eq!(surface.scroll_offset, 1);
    assert_eq!(surface.display_offset, 1);
    assert!(surface.remember_pending_local_scroll_sync(
        surface.current_scroll_visual_state(),
        result.generation,
    ));

    let frame_applied = apply_test_frame_snapshot!(
        surface,
        snapshot,
        0,
        0.0,
        0,
        scrollback_len,
        rows,
        true,
        None,
        0,
        false,
        true,
        "block",
    );

    assert!(!frame_applied);
    assert_eq!(surface.scroll_offset, 1);
    assert_eq!(surface.display_offset, 1);
    assert!(surface.has_new_while_scrolled);
    assert!(!surface.show_cursor);
    assert!(!surface.scroll_snapshot_pending);
}

#[test]
fn local_surface_wheel_respects_protocol_scroll_modes() {
    let mut surface = TerminalSurface::new("session");
    assert!(surface.can_handle_scroll_wheel_locally());

    let mouse_reporting = TerminalProtocolState {
        mouse_reporting: true,
        ..TerminalProtocolState::default()
    };
    surface.set_protocol_state(mouse_reporting);
    assert!(!surface.can_handle_scroll_wheel_locally());

    let alternate_scroll = TerminalProtocolState {
        alternate_screen: true,
        alternate_scroll: true,
        ..TerminalProtocolState::default()
    };
    surface.set_protocol_state(alternate_scroll);
    assert!(!surface.can_handle_scroll_wheel_locally());
}

#[test]
fn protocol_scroll_wheel_defers_parent_update_until_surface_lease_is_released() {
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let root = std::env::temp_dir().join(format!(
        "nyaterm-terminal-scroll-wheel-{}-{}",
        std::process::id(),
        uuid()
    ));
    let runtime = AppRuntime::from_parts_for_test(
        RuntimeMode::Portable,
        root.clone(),
        root.join("config"),
        root.join("logs"),
        root.join("cache"),
        None,
    );
    let stores = UiStoreHandles {
        startup_restore: cx.new(|_| StartupRestoreStore::default()),
        overlays: cx.new(|_| OverlayStore::default()),
    };
    let app = cx.new(|cx| NyaTermApp::new(runtime, stores, cx));
    let session_id = "session";
    let output = terminal_test_output_lines(80);
    let surface = cx.update_entity(&app, |app, cx| {
        app.session.select_active_session(session_id);
        app.terminal
            .seed_session_view(session_id.to_string(), output.clone(), "UTF-8");
        let bounds = bounds(point(px(0.), px(0.)), size(px(400.), px(240.)));
        assert!(app.remember_terminal_surface_bounds_for_session(Some(session_id), bounds,));
        app.ensure_terminal_surface(session_id, cx)
    });

    let mut screen = TerminalScreen::default();
    screen.advance_decoded_text(&output);
    let snapshot = Arc::new(screen.viewport_snapshot(0));
    let viewport_rows = snapshot.row_count().max(1);
    let scrollback_len = snapshot.scrollback_len;
    let bounds = bounds(point(px(0.), px(0.)), size(px(400.), px(240.)));
    let protocol = TerminalProtocolState {
        mouse_reporting: true,
        ..TerminalProtocolState::default()
    };
    cx.update_entity(&surface, |surface, _| {
        surface.set_protocol_state(protocol);
        surface.apply_frame_snapshot(TerminalSurfaceFrameSnapshot::new(
            snapshot.clone(),
            TerminalScrollVisualState {
                history: screen.history_state(),
                session_id: session_id.to_string(),
                scroll_offset: 0,
                scroll_residual_lines: 0.0,
                display_offset: 0,
                scrollback_len,
                viewport_rows,
                has_new_while_scrolled: false,
                performance_overlay: None,
                skipped_output_chars: 0,
            },
        ));
        surface.painted_hit_test_geometry = Some(TerminalPaintedHitTestGeometry {
            grid_bounds: Some(bounds),
            display_offset: 0,
            viewport_anchor_row: 0,
            snapshot_rows: snapshot.row_count(),
            viewport_rows,
            visual_y_offset: 0.0,
            cell_width: 8.0,
            cell_height: 16.0,
            revision: surface.revision,
        });
        surface.painted_hit_test_snapshot = Some(snapshot);
    });
    cx.update_entity(&app, |app, _| {
        app.terminal
            .view
            .views
            .get_mut(session_id)
            .expect("seeded terminal view")
            .protocol_state = protocol;
    });

    // The event callback holds the surface update lease; protocol wheel handling
    // must wait until the callback returns before reading the geometry.
    cx.update_entity(&surface, |surface, cx| {
        surface.handle_scroll_wheel(
            &ScrollEvent {
                position: point(px(8.), px(8.)),
                delta: ScrollDelta::Lines(point(0., 1.)),
                ..ScrollEvent::default()
            },
            cx,
        );
    });
    cx.run_until_parked();

    assert_eq!(
        cx.read_entity(&surface, |surface, _| surface.protocol_state),
        protocol
    );
}

#[test]
fn returning_to_live_paints_latest_last_row_before_app_scroll_sync() {
    assert_return_to_live_paints_latest_last_row(false, false, false);
}

#[test]
fn fractional_pixels_returning_to_live_paint_latest_last_row_before_app_scroll_sync() {
    assert_return_to_live_paints_latest_last_row(true, false, false);
}

#[test]
fn returning_to_live_during_output_growth_paints_latest_last_row_before_app_scroll_sync() {
    assert_return_to_live_paints_latest_last_row(false, true, false);
}

#[test]
fn returning_to_live_without_a_worker_frame_keeps_painted_text_and_geometry_until_ready() {
    assert_return_to_live_paints_latest_last_row(false, false, true);
}

#[test]
fn fractional_pixels_returning_to_live_without_a_worker_frame_keep_geometry_until_ready() {
    assert_return_to_live_paints_latest_last_row(true, false, true);
}

fn assert_return_to_live_paints_latest_last_row(
    fractional: bool,
    grow_history: bool,
    live_missing: bool,
) {
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let root = std::env::temp_dir().join(format!("nyaterm-live-scroll-{}", uuid()));
    let runtime = AppRuntime::from_parts_for_test(
        RuntimeMode::Portable,
        root.clone(),
        root.join("config"),
        root.join("logs"),
        root.join("cache"),
        None,
    );
    let stores = UiStoreHandles {
        startup_restore: cx.new(|_| StartupRestoreStore::default()),
        overlays: cx.new(|_| OverlayStore::default()),
    };
    let app = cx.new(|cx| NyaTermApp::new(runtime, stores, cx));
    let output = terminal_test_output_lines(80);
    let mut screen = TerminalScreen::default();
    screen.advance_decoded_text(&output);
    screen.advance_decoded_text("\r");
    let initial_offset = usize::from(!fractional);
    let old = Arc::new(screen.viewport_snapshot_with_window(initial_offset, 32, 32));
    if grow_history {
        screen.advance_decoded_text("\r\nnew output\r\n");
    }
    screen.advance_decoded_text("latest unsubmitted input");
    let latest = Arc::new(screen.viewport_snapshot(0));
    assert!(
        latest
            .rows()
            .last()
            .unwrap()
            .text
            .contains("latest unsubmitted input")
    );
    if !grow_history {
        assert_eq!(old.history, latest.history);
        assert!(terminal_snapshot_covers_display_offset(
            &old,
            0,
            latest.viewport_rows,
            latest.scrollback_len,
        ));
    }
    let surface = cx.update_entity(&app, |app, cx| {
        app.session.select_active_session("session");
        app.terminal
            .seed_session_view("session".to_string(), output, "UTF-8");
        let view = app.terminal.view.views.get_mut("session").unwrap();
        view.frame_snapshot = (!live_missing).then(|| latest.clone());
        view.scroll_offset = initial_offset;
        app.sync_terminal_surface_paint("session", cx);
        app.ensure_terminal_surface("session", cx)
    });
    cx.update_entity(&surface, |surface, _| {
        // Exercise painting without the production bounds tracker requesting a
        // worker resize for the synthetic test window.
        surface.app = None;
        surface.apply_frame_snapshot(TerminalSurfaceFrameSnapshot::new(
            old.clone(),
            TerminalScrollVisualState {
                history: old.history,
                session_id: "session".to_string(),
                scroll_offset: initial_offset,
                scroll_residual_lines: if fractional { 0.6 } else { 0.0 },
                display_offset: initial_offset,
                scrollback_len: old.scrollback_len,
                viewport_rows: latest.viewport_rows,
                has_new_while_scrolled: true,
                performance_overlay: None,
                skipped_output_chars: 0,
            },
        ));
    });
    let window = cx.open_window(size(px(640.0), px(600.0)), {
        let surface = surface.clone();
        move |_window, _cx| TerminalSurfaceLayoutTestView {
            surface,
            mounted: true,
        }
    });
    cx.update_window(window.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
    })
    .unwrap();
    let old_geometry = cx.read_entity(&surface, |surface, _| {
        surface.painted_hit_test_geometry.unwrap()
    });
    cx.read_entity(&app, |app, _| {
        assert_eq!(
            app.terminal_live_surface_frame("session").is_none(),
            live_missing
        );
    });
    cx.update_entity(&surface, |surface, cx| {
        surface.set_app(app.clone(), true);
        surface.handle_scroll_wheel(
            &ScrollEvent {
                delta: if fractional {
                    ScrollDelta::Pixels(point(px(0.), px(-0.6 * surface.cell_height)))
                } else {
                    ScrollDelta::Lines(point(0., -1.))
                },
                ..ScrollEvent::default()
            },
            cx,
        );
        surface.app = None;
    });
    for phase in ["immediate", "pending full frame", "app sync", "full frame"] {
        if phase == "app sync" {
            cx.update_entity(&app, |app, _| {
                app.terminal
                    .view
                    .views
                    .get_mut("session")
                    .unwrap()
                    .frame_snapshot = Some(latest.clone());
            });
            cx.update_entity(&surface, |surface, cx| {
                surface.flush_local_scroll_app_sync(app.clone(), cx);
            });
        } else if phase == "full frame" || phase == "pending full frame" {
            cx.update_entity(&app, |app, cx| {
                app.sync_terminal_surface_paint("session", cx);
            });
        }
        cx.update_window(window.into(), |_, window, cx| {
            window.draw(cx).clear(cx);
        })
        .unwrap();
        cx.read_entity(&surface, |surface, _| {
            let (geometry, painted) = surface.painted_hit_test_state().unwrap();
            if live_missing && (phase == "immediate" || phase == "pending full frame") {
                assert!(Arc::ptr_eq(&painted, &old));
                assert_eq!(geometry.display_offset, old_geometry.display_offset);
                assert_eq!(geometry.visual_y_offset, old_geometry.visual_y_offset);
                assert!(surface.scroll_snapshot_pending);
                assert!(!surface.show_cursor);
                return;
            }
            assert!(
                painted
                    .rows()
                    .last()
                    .unwrap()
                    .text
                    .contains("latest unsubmitted input"),
                "last row regressed during {phase}",
            );
            assert_eq!(geometry.display_offset, 0, "{phase}");
            let last_row_y =
                geometry.visual_y_offset + (painted.row_count() - 1) as f32 * geometry.cell_height;
            assert_eq!(
                last_row_y,
                (latest.viewport_rows - 1) as f32 * geometry.cell_height,
                "last row moved during {phase}",
            );
        });
    }
}

#[test]
fn stale_local_scroll_sync_does_not_overwrite_the_app_scroll_target() {
    let root = crate::test_support::TestConfigDir::new("nyaterm-stale-local-scroll");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app = crate::features::test_support::app_with_visible_local_session(
        &mut cx,
        root.path(),
        "session",
    );
    let surface = cx.update_entity(&app, |app, cx| {
        app.terminal.seed_session_view(
            "session".to_string(),
            terminal_test_output_lines(80),
            "UTF-8",
        );
        app.terminal
            .view
            .views
            .get_mut("session")
            .unwrap()
            .scroll_offset = 3;
        app.ensure_terminal_surface("session", cx)
    });
    cx.update_entity(&surface, |surface, cx| {
        let mut stale = surface.current_scroll_visual_state();
        stale.scroll_offset = 0;
        stale.display_offset = 0;
        surface.remember_pending_local_scroll_sync(stale, 1);
        surface.scroll_interaction_generation = 2;
        surface.flush_local_scroll_app_sync(app.clone(), cx);
    });
    cx.read_entity(&app, |app, _| {
        assert_eq!(app.terminal.view.views["session"].scroll_offset, 3);
    });
}

#[test]
fn reversing_local_scroll_before_app_sync_keeps_the_latest_wheel_target() {
    let root = crate::test_support::TestConfigDir::new("nyaterm-reverse-local-scroll");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app = crate::features::test_support::app_with_visible_local_session(
        &mut cx,
        root.path(),
        "session",
    );
    let mut screen = TerminalScreen::default();
    screen.advance_decoded_text(&terminal_test_output_lines(80));
    let live = Arc::new(screen.viewport_snapshot_with_window(0, 32, 0));
    let surface = cx.update_entity(&app, |app, cx| {
        app.terminal
            .view
            .views
            .get_mut("session")
            .unwrap()
            .frame_snapshot = Some(live.clone());
        app.sync_terminal_surface_paint("session", cx);
        app.ensure_terminal_surface("session", cx)
    });
    let wheel = |cx: &mut TestAppContext, lines| {
        cx.update_entity(&surface, |surface, cx| {
            surface.handle_scroll_wheel(
                &ScrollEvent {
                    delta: ScrollDelta::Lines(point(0., lines)),
                    ..ScrollEvent::default()
                },
                cx,
            );
        });
    };
    wheel(&mut cx, 1.0);
    cx.update_entity(&app, |app, cx| {
        // The app still thinks it is at live until the local timer fires.
        assert_eq!(app.terminal.view.views["session"].scroll_offset, 0);
        app.notify_terminal_scroll_position_only("session", cx);
        app.notify_terminal_scroll_visual_only("session", cx);
    });
    cx.read_entity(&surface, |surface, _| {
        assert_eq!(surface.scroll_offset, 1);
        assert!(!surface.show_cursor);
    });
    wheel(&mut cx, -1.0);
    cx.update_entity(&surface, |surface, cx| {
        surface.flush_local_scroll_app_sync(app.clone(), cx);
        assert_eq!(surface.scroll_offset, 0);
        assert!(Arc::ptr_eq(surface.snapshot.as_ref().unwrap(), &live));
    });
}

#[test]
fn local_surface_scroll_app_sync_is_frame_coalesced() {
    let mut surface = TerminalSurface::new("session");
    let first = TerminalScrollVisualState {
        history: surface.history,
        session_id: "session".to_string(),
        scroll_offset: 1,
        scroll_residual_lines: 0.25,
        display_offset: 1,
        scrollback_len: 10,
        viewport_rows: 4,
        has_new_while_scrolled: false,
        performance_overlay: None,
        skipped_output_chars: 0,
    };
    let mut second = first.clone();
    second.scroll_offset = 2;
    second.display_offset = 2;

    assert!(surface.remember_pending_local_scroll_sync(first, 1));
    assert!(!surface.remember_pending_local_scroll_sync(second, 2));

    let pending = surface
        .pending_local_scroll_sync
        .as_ref()
        .expect("pending sync");
    assert!(surface.local_scroll_sync_armed);
    assert_eq!(pending.generation, 2);
    assert_eq!(pending.state.scroll_offset, 2);
    assert_eq!(pending.state.display_offset, 2);
}

#[test]
fn local_surface_scroll_state_marks_pending_when_snapshot_missing() {
    let snapshot = Arc::new(TerminalScreen::default().viewport_snapshot(0));
    let rows = snapshot.row_count();
    let mut surface = TerminalSurface::new("session");

    apply_test_frame_snapshot!(
        surface, snapshot, 0, 0.0, 0, 10, rows, false, None, 0, false, false, "block",
    );
    let text_updated = surface.apply_scroll_visual_state(TerminalScrollVisualState {
        history: surface.history,
        session_id: "session".to_string(),
        scroll_offset: 4,
        scroll_residual_lines: 0.0,
        display_offset: 4,
        scrollback_len: 10,
        viewport_rows: rows,
        has_new_while_scrolled: false,
        performance_overlay: None,
        skipped_output_chars: 0,
    });

    assert!(!text_updated);
    assert!(surface.scroll_snapshot_pending);
    assert_eq!(surface.display_offset, 0);
    assert_eq!(surface.scroll_offset, 4);
}

#[test]
fn local_surface_scroll_state_promotes_retained_snapshot_window() {
    let mut screen = TerminalScreen::default();
    screen.advance_decoded_text(&terminal_test_output_lines(120));
    let first_offset = 8;
    let second_offset = 30;
    let first_snapshot = Arc::new(screen.viewport_snapshot(first_offset));
    let second_snapshot = Arc::new(screen.viewport_snapshot(second_offset));
    let rows = first_snapshot.row_count().max(1);
    let scrollback_len = first_snapshot.scrollback_len;
    let mut surface = TerminalSurface::new("session");

    apply_test_frame_snapshot!(
        surface,
        first_snapshot.clone(),
        first_offset,
        0.0,
        first_offset,
        scrollback_len,
        rows,
        false,
        None,
        0,
        false,
        false,
        "block",
    );
    apply_test_frame_snapshot!(
        surface,
        second_snapshot,
        second_offset,
        0.0,
        second_offset,
        scrollback_len,
        rows,
        false,
        None,
        0,
        true,
        false,
        "block",
    );
    assert_eq!(
        surface.snapshot.as_ref().unwrap().display_offset,
        second_offset
    );
    assert!(surface.has_action_link_decorations);

    let text_updated = surface.apply_scroll_visual_state(TerminalScrollVisualState {
        history: screen.history_state(),
        session_id: "session".to_string(),
        scroll_offset: first_offset,
        scroll_residual_lines: 0.0,
        display_offset: first_offset,
        scrollback_len,
        viewport_rows: rows,
        has_new_while_scrolled: false,
        performance_overlay: None,
        skipped_output_chars: 0,
    });

    assert!(text_updated);
    assert!(!surface.scroll_snapshot_pending);
    assert_eq!(surface.display_offset, first_offset);
    assert!(surface.has_action_link_decorations);
    assert_eq!(
        surface.snapshot.as_ref().unwrap().display_offset,
        first_offset
    );
    assert!(Arc::ptr_eq(
        surface.snapshot.as_ref().unwrap(),
        &first_snapshot
    ));
}

#[test]
fn retained_snapshot_promotion_preserves_dynamic_absolute_selection() {
    let mut screen = TerminalScreen::default();
    screen.advance_decoded_text(&terminal_test_output_lines(120));
    let live_snapshot = Arc::new(screen.viewport_snapshot(0));
    let history_offset = 8;
    let history_snapshot = Arc::new(screen.viewport_snapshot(history_offset));
    let rows = live_snapshot.row_count().max(1);
    let scrollback_len = live_snapshot.scrollback_len;
    let live_start = live_snapshot
        .total_rows
        .saturating_sub(live_snapshot.display_offset)
        .saturating_sub(live_snapshot.row_count());
    let selected_line = live_start.saturating_add(2);
    let history_start = history_snapshot
        .total_rows
        .saturating_sub(history_snapshot.display_offset)
        .saturating_sub(history_snapshot.row_count());
    let live_row = selected_line.saturating_sub(live_start);
    let history_row = selected_line.saturating_sub(history_start);
    assert_ne!(live_row, history_row);
    assert!(history_row < history_snapshot.row_count());

    let mut surface = TerminalSurface::new("session");
    apply_test_frame_snapshot!(
        surface,
        history_snapshot.clone(),
        history_offset,
        0.0,
        history_offset,
        scrollback_len,
        rows,
        false,
        None,
        0,
        false,
        false,
        "block",
    );
    apply_test_frame_snapshot!(
        surface,
        live_snapshot,
        0,
        0.0,
        0,
        scrollback_len,
        rows,
        false,
        None,
        0,
        false,
        false,
        "block",
    );
    let selection = TerminalSelection::from_range(
        TerminalBufferCellPos::new(selected_line, 2),
        TerminalBufferCellPos::new(selected_line, 5),
    );
    assert!(surface.set_selection_visual(Some(selection)));
    let static_decorations = surface.decorations.clone();

    assert!(
        surface.apply_scroll_visual_state(TerminalScrollVisualState {
            history: screen.history_state(),
            session_id: "session".to_string(),
            scroll_offset: history_offset,
            scroll_residual_lines: 0.0,
            display_offset: history_offset,
            scrollback_len,
            viewport_rows: rows,
            has_new_while_scrolled: false,
            performance_overlay: None,
            skipped_output_chars: 0,
        })
    );

    assert!(Arc::ptr_eq(
        surface.snapshot.as_ref().unwrap(),
        &history_snapshot
    ));
    assert!(Arc::ptr_eq(&surface.decorations, &static_decorations));
    assert_eq!(surface.selection_visual, Some(selection));
}

#[test]
fn prefetched_snapshot_is_retained_without_changing_current_paint() {
    let mut screen = TerminalScreen::default();
    screen.advance_decoded_text(&terminal_test_output_lines(160));
    let live_snapshot = Arc::new(screen.viewport_snapshot(0));
    let viewport_rows = live_snapshot.viewport_rows.max(1);
    let scrollback_len = live_snapshot.scrollback_len;
    let prefetch_offset = viewport_rows.saturating_mul(2).min(scrollback_len);
    let prefetched_snapshot = Arc::new(screen.viewport_snapshot_with_window(
        prefetch_offset,
        viewport_rows.saturating_mul(2),
        viewport_rows.saturating_mul(2),
    ));
    let mut surface = TerminalSurface::new("session");

    apply_test_frame_snapshot!(
        surface,
        live_snapshot.clone(),
        0,
        0.0,
        0,
        scrollback_len,
        viewport_rows,
        false,
        None,
        0,
        false,
        true,
        "block",
    );
    let revision = surface.revision;

    assert!(surface.retain_prefetched_snapshot(prefetched_snapshot.clone()));
    assert_eq!(surface.revision, revision);
    assert_eq!(surface.display_offset, 0);
    assert!(Arc::ptr_eq(
        surface.snapshot.as_ref().unwrap(),
        &live_snapshot
    ));
    assert!(surface.has_snapshot_covering_display_offset(
        prefetch_offset,
        viewport_rows,
        scrollback_len
    ));

    let mut refreshed_prefetch = prefetched_snapshot.as_ref().clone();
    let rows = Arc::make_mut(&mut refreshed_prefetch.row_data);
    Arc::make_mut(&mut rows[0]).text = "refreshed".to_string();
    assert!(surface.retain_prefetched_snapshot(Arc::new(refreshed_prefetch)));
    assert_eq!(
        surface
            .retained_snapshots
            .last()
            .and_then(|snapshot| snapshot.line(0)),
        Some("refreshed")
    );
}

#[test]
fn local_surface_scroll_state_synthesizes_cross_window_on_hot_path() {
    let mut screen = TerminalScreen::default();
    screen.advance_decoded_text(&terminal_test_output_lines(160));
    let live_snapshot = Arc::new(screen.viewport_snapshot(0));
    let rows = live_snapshot.row_count().max(2);
    let older_offset = rows;
    let target_offset = rows / 2;
    let older_snapshot = Arc::new(screen.viewport_snapshot(older_offset));
    let scrollback_len = live_snapshot.scrollback_len;
    let mut surface = TerminalSurface::new("session");

    apply_test_frame_snapshot!(
        surface,
        live_snapshot,
        0,
        0.0,
        0,
        scrollback_len,
        rows,
        false,
        None,
        0,
        false,
        false,
        "block",
    );
    apply_test_frame_snapshot!(
        surface,
        older_snapshot,
        older_offset,
        0.0,
        older_offset,
        scrollback_len,
        rows,
        false,
        None,
        0,
        false,
        false,
        "block",
    );

    assert!(surface.snapshot.as_ref().is_some_and(|snapshot| {
        !terminal_snapshot_covers_display_offset(snapshot, target_offset, rows, scrollback_len)
    }));

    let text_updated = surface.apply_scroll_visual_state(TerminalScrollVisualState {
        history: screen.history_state(),
        session_id: "session".to_string(),
        scroll_offset: target_offset,
        scroll_residual_lines: 0.0,
        display_offset: target_offset,
        scrollback_len,
        viewport_rows: rows,
        has_new_while_scrolled: false,
        performance_overlay: None,
        skipped_output_chars: 0,
    });

    let snapshot = surface.snapshot.as_ref().expect("retained snapshot");
    assert!(text_updated);
    assert!(!surface.scroll_snapshot_pending);
    assert_eq!(surface.scroll_offset, target_offset);
    assert_eq!(surface.display_offset, target_offset);
    assert!(terminal_snapshot_covers_display_offset(
        snapshot,
        target_offset,
        rows,
        scrollback_len
    ));
}

#[test]
fn local_surface_scroll_state_synthesizes_row_cache_on_hot_path() {
    let mut screen = TerminalScreen::default();
    screen.advance_decoded_text(&terminal_test_output_lines(160));
    let live_snapshot = Arc::new(screen.viewport_snapshot(0));
    let rows = live_snapshot.row_count().max(2);
    let older_offset = rows;
    let target_offset = rows / 2;
    let older_snapshot = Arc::new(screen.viewport_snapshot(older_offset));
    let scrollback_len = live_snapshot.scrollback_len;
    let mut surface = TerminalSurface::new("session");

    apply_test_frame_snapshot!(
        surface,
        live_snapshot,
        0,
        0.0,
        0,
        scrollback_len,
        rows,
        false,
        None,
        0,
        false,
        false,
        "block",
    );
    apply_test_frame_snapshot!(
        surface,
        older_snapshot,
        older_offset,
        0.0,
        older_offset,
        scrollback_len,
        rows,
        false,
        None,
        0,
        false,
        false,
        "block",
    );
    surface.retained_snapshots.clear();
    assert!(!surface.retained_rows.is_empty());

    let text_updated = surface.apply_scroll_visual_state(TerminalScrollVisualState {
        history: screen.history_state(),
        session_id: "session".to_string(),
        scroll_offset: target_offset,
        scroll_residual_lines: 0.0,
        display_offset: target_offset,
        scrollback_len,
        viewport_rows: rows,
        has_new_while_scrolled: false,
        performance_overlay: None,
        skipped_output_chars: 0,
    });

    let snapshot = surface.snapshot.as_ref().expect("retained snapshot");
    assert!(text_updated);
    assert!(!surface.scroll_snapshot_pending);
    assert_eq!(surface.scroll_offset, target_offset);
    assert_eq!(surface.display_offset, target_offset);
    assert!(terminal_snapshot_covers_display_offset(
        snapshot,
        target_offset,
        rows,
        scrollback_len
    ));
}

#[test]
fn retained_rows_can_synthesize_snapshot_when_no_retained_window_covers_target() {
    let mut screen = TerminalScreen::default();
    screen.advance_decoded_text(&terminal_test_output_lines(160));
    let live_snapshot = Arc::new(screen.viewport_snapshot(0));
    let rows = live_snapshot.row_count().max(2);
    let older_offset = rows;
    let target_offset = rows / 2;
    let older_snapshot = Arc::new(screen.viewport_snapshot(older_offset));
    let scrollback_len = live_snapshot.scrollback_len;
    let mut surface = TerminalSurface::new("session");

    apply_test_frame_snapshot!(
        surface,
        live_snapshot,
        0,
        0.0,
        0,
        scrollback_len,
        rows,
        false,
        None,
        0,
        false,
        false,
        "block",
    );
    apply_test_frame_snapshot!(
        surface,
        older_snapshot,
        older_offset,
        0.0,
        older_offset,
        scrollback_len,
        rows,
        false,
        None,
        0,
        false,
        false,
        "block",
    );
    surface.retained_snapshots.clear();

    assert!(
        surface
            .retained_snapshot_covering_display_offset(target_offset, rows, scrollback_len)
            .is_none()
    );
    assert!(surface.has_snapshot_covering_display_offset(target_offset, rows, scrollback_len));
    let synthesized = surface
        .snapshot_covering_display_offset(target_offset, rows, scrollback_len)
        .expect("retained rows should synthesize the target viewport");
    assert!(synthesized.row_count() > rows);
    assert!(synthesized.display_offset <= target_offset);
    assert!(terminal_snapshot_covers_display_offset(
        synthesized.as_ref(),
        target_offset,
        rows,
        scrollback_len
    ));
    assert!(terminal_snapshot_covers_display_offset(
        synthesized.as_ref(),
        target_offset.saturating_sub(1),
        rows,
        scrollback_len
    ));
    assert!(terminal_snapshot_covers_display_offset(
        synthesized.as_ref(),
        target_offset.saturating_add(1).min(scrollback_len),
        rows,
        scrollback_len
    ));
}

#[test]
fn local_surface_scroll_state_does_not_synthesize_across_gap() {
    let mut screen = TerminalScreen::default();
    screen.advance_decoded_text(&terminal_test_output_lines(200));
    let live_snapshot = Arc::new(screen.viewport_snapshot(0));
    let rows = live_snapshot.row_count().max(2);
    let far_offset = rows.saturating_mul(2);
    let target_offset = rows;
    let far_snapshot = Arc::new(screen.viewport_snapshot(far_offset));
    let scrollback_len = live_snapshot.scrollback_len;
    let mut surface = TerminalSurface::new("session");

    apply_test_frame_snapshot!(
        surface,
        live_snapshot,
        0,
        0.0,
        0,
        scrollback_len,
        rows,
        false,
        None,
        0,
        false,
        false,
        "block",
    );
    apply_test_frame_snapshot!(
        surface,
        far_snapshot,
        far_offset,
        0.0,
        far_offset,
        scrollback_len,
        rows,
        false,
        None,
        0,
        false,
        false,
        "block",
    );

    let text_updated = surface.apply_scroll_visual_state(TerminalScrollVisualState {
        history: screen.history_state(),
        session_id: "session".to_string(),
        scroll_offset: target_offset,
        scroll_residual_lines: 0.0,
        display_offset: target_offset,
        scrollback_len,
        viewport_rows: rows,
        has_new_while_scrolled: false,
        performance_overlay: None,
        skipped_output_chars: 0,
    });

    assert!(!surface.has_snapshot_covering_display_offset(target_offset, rows, scrollback_len));
    assert!(!text_updated);
    assert!(surface.scroll_snapshot_pending);
    assert_eq!(surface.display_offset, far_offset);
}

#[test]
fn retained_snapshot_stays_valid_when_output_growth_reanchors_offset() {
    let mut screen = TerminalScreen::default();
    screen.advance_decoded_text(&terminal_test_output_lines(80));
    let old_display_offset = 6;
    let snapshot = Arc::new(screen.viewport_snapshot(old_display_offset));
    let rows = snapshot.row_count();
    let old_scrollback_len = snapshot.scrollback_len;
    let growth = 3;
    let new_display_offset = old_display_offset + growth;
    let new_scrollback_len = old_scrollback_len + growth;

    assert!(terminal_snapshot_covers_display_offset(
        snapshot.as_ref(),
        old_display_offset,
        rows,
        old_scrollback_len
    ));
    assert!(terminal_snapshot_covers_display_offset(
        snapshot.as_ref(),
        new_display_offset,
        rows,
        new_scrollback_len
    ));
    assert_eq!(
        terminal_snapshot_anchor_row_for_display_offset(
            snapshot.as_ref(),
            old_display_offset,
            rows,
            old_scrollback_len
        ),
        terminal_snapshot_anchor_row_for_display_offset(
            snapshot.as_ref(),
            new_display_offset,
            rows,
            new_scrollback_len
        )
    );
}

#[test]
fn output_growth_scroll_position_reuse_does_not_mark_snapshot_pending() {
    let mut screen = TerminalScreen::default();
    screen.advance_decoded_text(&terminal_test_output_lines(80));
    let old_display_offset = 6;
    let snapshot = Arc::new(screen.viewport_snapshot(old_display_offset));
    let rows = snapshot.row_count();
    let old_scrollback_len = snapshot.scrollback_len;
    let growth = 3;
    let new_display_offset = old_display_offset + growth;
    let new_scrollback_len = old_scrollback_len + growth;
    let mut surface = TerminalSurface::new("session");

    apply_test_frame_snapshot!(
        surface,
        snapshot,
        old_display_offset,
        0.0,
        old_display_offset,
        old_scrollback_len,
        rows,
        false,
        None,
        0,
        false,
        false,
        "block",
    );
    assert!(
        surface
            .snapshot_covering_display_offset(new_display_offset, rows, new_scrollback_len)
            .is_some()
    );

    surface.update_scroll_position_without_snapshot(&TerminalScrollVisualState {
        history: screen.history_state(),
        session_id: "session".to_string(),
        scroll_offset: new_display_offset,
        scroll_residual_lines: 0.0,
        display_offset: new_display_offset,
        scrollback_len: new_scrollback_len,
        viewport_rows: rows,
        has_new_while_scrolled: true,
        performance_overlay: None,
        skipped_output_chars: 0,
    });

    assert!(!surface.scroll_snapshot_pending);
    assert_eq!(surface.display_offset, new_display_offset);
    assert!(surface.has_new_while_scrolled);
    assert_eq!(
        terminal_effective_visual_scroll_offset_px(TerminalVisualScrollGeometry {
            snapshot_pending: surface.scroll_snapshot_pending,
            target_offset: surface.scroll_offset,
            displayed_offset: surface.display_offset,
            residual_lines: surface.scroll_residual_lines,
            viewport_anchor_row: 0,
            snapshot_rows: rows,
            viewport_rows: rows,
            cell_height: 16.0,
        },),
        0.0
    );
}

#[test]
fn scroll_position_reuse_preserves_stale_decorations_until_recomputed() {
    let mut screen = TerminalScreen::default();
    screen.advance_decoded_text(&terminal_test_output_lines(80));
    let old_display_offset = 6;
    let snapshot = Arc::new(screen.viewport_snapshot(old_display_offset));
    let rows = snapshot.row_count();
    let old_scrollback_len = snapshot.scrollback_len;
    let growth = 3;
    let new_display_offset = old_display_offset + growth;
    let new_scrollback_len = old_scrollback_len + growth;
    let mut surface = TerminalSurface::new("session");

    apply_test_frame_snapshot!(
        surface,
        snapshot,
        old_display_offset,
        0.0,
        old_display_offset,
        old_scrollback_len,
        rows,
        false,
        None,
        0,
        true,
        true,
        "block",
    );
    surface.set_decorations_and_keywords(
        vec![TerminalLineDecorations {
            search_ranges: vec![(2, 5)],
            link_ranges: vec![(1, 3)],
            ..TerminalLineDecorations::default()
        }],
        Arc::new(Vec::new()),
        true,
        "block",
    );

    surface.update_scroll_position_without_snapshot(&TerminalScrollVisualState {
        history: screen.history_state(),
        session_id: "session".to_string(),
        scroll_offset: new_display_offset,
        scroll_residual_lines: 0.0,
        display_offset: new_display_offset,
        scrollback_len: new_scrollback_len,
        viewport_rows: rows,
        has_new_while_scrolled: true,
        performance_overlay: None,
        skipped_output_chars: 0,
    });

    assert_eq!(surface.display_offset, new_display_offset);
    assert_eq!(surface.decorations[0].search_ranges, vec![(2, 5)]);
    assert_eq!(surface.decorations[0].link_ranges, vec![(1, 3)]);
    assert!(surface.has_action_link_decorations);
}

#[test]
fn surface_retained_scroll_state_resets_when_scrollback_shrinks() {
    let mut old_screen = TerminalScreen::default();
    old_screen.advance_decoded_text(&terminal_test_output_lines(120));
    let old_offset = 12;
    let old_snapshot = Arc::new(old_screen.viewport_snapshot(old_offset));
    let old_rows = old_snapshot.row_count();
    let mut surface = TerminalSurface::new("session");

    apply_test_frame_snapshot!(
        surface,
        old_snapshot,
        old_offset,
        0.0,
        old_offset,
        old_screen.scrollback_len(),
        old_rows,
        false,
        None,
        0,
        true,
        false,
        "block",
    );
    assert_eq!(surface.retained_snapshots.len(), 1);
    assert!(surface.has_action_link_decorations);

    let mut new_screen = TerminalScreen::default();
    new_screen.advance_decoded_text("after clear\n");
    let new_snapshot = Arc::new(new_screen.viewport_snapshot(0));
    let new_rows = new_snapshot.row_count();
    apply_test_frame_snapshot!(
        surface,
        new_snapshot.clone(),
        0,
        0.0,
        0,
        new_screen.scrollback_len(),
        new_rows,
        false,
        None,
        0,
        false,
        false,
        "block",
    );

    assert_eq!(surface.retained_snapshots.len(), 1);
    assert_eq!(surface.retained_snapshots[0].display_offset, 0);
    assert_eq!(
        surface.retained_snapshots[0].total_rows,
        new_snapshot.total_rows
    );
    assert!(!surface.has_action_link_decorations);
}

#[test]
fn surface_retained_scroll_state_resets_when_viewport_rows_change() {
    let mut screen = TerminalScreen::default();
    screen.advance_decoded_text(&terminal_test_output_lines(120));
    let old_offset = 12;
    let old_snapshot = Arc::new(screen.viewport_snapshot(old_offset));
    let old_rows = old_snapshot.row_count().max(2);
    let new_snapshot = Arc::new(screen.viewport_snapshot(0));
    let new_viewport_rows = old_rows - 1;
    let mut surface = TerminalSurface::new("session");

    apply_test_frame_snapshot!(
        surface,
        old_snapshot,
        old_offset,
        0.0,
        old_offset,
        screen.scrollback_len(),
        old_rows,
        false,
        None,
        0,
        true,
        false,
        "block",
    );
    assert_eq!(surface.retained_snapshots.len(), 1);
    assert!(!surface.retained_rows.is_empty());

    apply_test_frame_snapshot!(
        surface,
        new_snapshot,
        0,
        0.0,
        0,
        screen.scrollback_len(),
        new_viewport_rows,
        false,
        None,
        0,
        false,
        false,
        "block",
    );

    assert_eq!(surface.retained_snapshots.len(), 1);
    assert_eq!(surface.retained_snapshots[0].display_offset, 0);
    assert_eq!(surface.viewport_rows, new_viewport_rows);
    assert!(!surface.has_action_link_decorations);
}

#[test]
fn full_history_rotation_keeps_surviving_rows_and_converts_old_prefetch() {
    let mut screen = TerminalScreen::new(16, 3);
    screen.set_scrollback_limit(8);
    screen.advance(b"0\r\n1\r\n2\r\n3\r\n4\r\n5\r\n6\r\n7\r\n8\r\n9\r\n10\r\n");
    let old = Arc::new(screen.viewport_snapshot(5));
    let mut surface = TerminalSurface::new("session");
    apply_test_frame_snapshot!(
        surface,
        old.clone(),
        5,
        0.0,
        5,
        8,
        3,
        false,
        None,
        0,
        false,
        false,
        "block"
    );
    let old_rows = surface.retained_rows.clone();
    screen.advance(b"11\r\n12\r\n");
    let fresh = Arc::new(screen.viewport_snapshot(0));
    apply_test_frame_snapshot!(
        surface,
        fresh.clone(),
        0,
        0.0,
        0,
        8,
        3,
        false,
        None,
        0,
        false,
        false,
        "block"
    );
    assert_eq!(surface.retained_snapshots.len(), 2);
    for (key, row) in old_rows {
        assert!(Arc::ptr_eq(surface.retained_row(key - 2).unwrap(), &row));
    }
    assert!(surface.retain_prefetched_snapshot(old));
    assert!(!surface.has_snapshot_covering_display_offset(5, 3, 8));
    assert_eq!(surface.snapshot.as_ref().unwrap().history, fresh.history);
}

#[test]
fn ordinary_history_growth_keeps_retained_absolute_rows() {
    let mut screen = TerminalScreen::new(16, 3);
    screen.set_scrollback_limit(32);
    screen.advance(b"0\r\n1\r\n2\r\n3\r\n4\r\n5\r\n6\r\n7\r\n");
    let old = Arc::new(screen.viewport_snapshot(4));
    let mut surface = TerminalSurface::new("session");
    apply_test_frame_snapshot!(
        surface,
        old,
        4,
        0.0,
        4,
        screen.scrollback_len(),
        3,
        false,
        None,
        0,
        false,
        false,
        "block"
    );
    let retained = surface.retained_rows.clone();
    screen.advance(b"8\r\n9\r\n");
    let fresh = Arc::new(screen.viewport_snapshot(0));
    apply_test_frame_snapshot!(
        surface,
        fresh,
        0,
        0.0,
        0,
        screen.scrollback_len(),
        3,
        false,
        None,
        0,
        false,
        false,
        "block"
    );
    assert_eq!(surface.retained_snapshots.len(), 2);
    for (key, row) in retained {
        assert!(Arc::ptr_eq(surface.retained_rows.get(&key).unwrap(), &row));
    }
}

#[test]
fn pending_local_scroll_reanchors_across_full_history_rotation() {
    let mut screen = TerminalScreen::new(16, 3);
    screen.set_scrollback_limit(8);
    screen.advance(b"0\r\n1\r\n2\r\n3\r\n4\r\n5\r\n6\r\n7\r\n8\r\n9\r\n10\r\n");
    let old = Arc::new(screen.viewport_snapshot_with_window(3, 5, 3));
    let mut surface = TerminalSurface::new("session");
    apply_test_frame_snapshot!(
        surface, old, 3, 0.0, 3, 8, 3, false, None, 0, false, false, "block"
    );
    let result = surface.apply_local_scroll_wheel_visual_state(1.0).unwrap();
    assert_eq!(surface.scroll_offset, 4);
    surface.remember_pending_local_scroll_sync(
        surface.current_scroll_visual_state(),
        result.generation,
    );
    let reading = screen.viewport_snapshot(4).line(0).unwrap().to_string();
    screen.advance(b"11\r\n");
    let fresh = Arc::new(screen.viewport_snapshot_with_window(4, 4, 4));
    apply_test_frame_snapshot!(
        surface, fresh, 4, 0.0, 4, 8, 3, true, None, 0, false, false, "block"
    );
    assert_eq!(surface.scroll_offset, 5);
    assert_eq!(surface.display_offset, 5);
    let snapshot = surface.snapshot.as_ref().unwrap();
    let anchor = terminal_snapshot_anchor_row_for_display_offset(snapshot, 5, 3, 8);
    assert_eq!(snapshot.line(anchor), Some(reading.as_str()));
}

fn install_keyword_test_frame(surface: &mut TerminalSurface, snapshot: Arc<TerminalSnapshot>) {
    let viewport_rows = snapshot.viewport_rows;
    let scrollback_len = snapshot.scrollback_len;
    let history = snapshot.history;
    surface.apply_frame_snapshot(TerminalSurfaceFrameSnapshot::new(
        snapshot,
        TerminalScrollVisualState {
            history,
            session_id: "highlight-test".into(),
            scroll_offset: 0,
            scroll_residual_lines: 0.,
            display_offset: 0,
            scrollback_len,
            viewport_rows,
            has_new_while_scrolled: false,
            performance_overlay: None,
            skipped_output_chars: 0,
        },
    ));
}

#[test]
fn applying_and_painting_a_frame_does_not_compile_or_parse_keywords() {
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut screen = TerminalScreen::new(60, 4);
    screen.advance(
        "\u{2003}\u{2003}\u{2003}\u{2003}-rw-r--r-- 1 root root 123 Oct 30 文件.txt".as_bytes(),
    );
    let surface = cx.new(|_| TerminalSurface::new("highlight-test"));
    cx.update_entity(&surface, |surface, _| {
        surface.keyword_rules =
            Arc::new(nyaterm_core::keyword_highlight_presets::get_builtin_keyword_rules(true));
        install_keyword_test_frame(surface, Arc::new(screen.snapshot()));
        assert!(surface.keyword_highlighter.is_none());
        assert!(surface.keyword_highlights.is_none());
    });
    let window = cx.open_window(size(px(600.), px(120.)), {
        let surface = surface.clone();
        move |_, _| TerminalSurfaceLayoutTestView {
            surface,
            mounted: true,
        }
    });
    cx.update_window(window.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.read_entity(&surface, |surface, _| {
        assert!(surface.keyword_highlighter.is_none());
        assert!(surface.keyword_highlights.is_none());
        assert!(surface.keyword_highlight_task.is_none());
    });
}

#[test]
fn output_highlight_preferences_follow_draft_cancel_and_settings_reload_on_open_terminals() {
    use crate::features::test_support::app_with_visible_local_session;
    use crate::test_support::TestConfigDir;

    for original_line_roles in [false, true] {
        let dir = TestConfigDir::new("nyaterm-highlight-settings-restore");
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "s1");
        let surfaces = cx.update_entity(&app, |app, cx| {
            let mut settings = app.settings.summary().clone();
            settings.ui_highlight_line_roles = original_line_roles;
            app.apply_gpui_settings(settings, cx);
            let mut keywords = app.settings.keyword_config().clone();
            keywords.enabled = true;
            keywords
                .builtin_rules
                .insert("builtin-net-address".into(), false);
            app.settings.replace_keyword_config(keywords);
            app.invalidate_paint_theme_caches();
            [
                app.ensure_terminal_surface("s1", cx),
                app.ensure_terminal_surface("s2", cx),
            ]
        });
        cx.update_entity(&app, |app, cx| {
            app.begin_settings_draft(cx);
            app.toggle_line_role_highlights(cx);
            app.toggle_keyword_highlights(cx);
            assert!(app.resolved_keyword_highlight_rules().is_empty());
            app.toggle_line_role_highlights(cx);
            assert_eq!(
                app.settings.summary().ui_highlight_line_roles,
                !original_line_roles
            );
            app.toggle_keyword_highlights(cx);
            assert!(!app.resolved_keyword_highlight_rules().is_empty());
            assert!(!app.settings.keyword_config().builtin_rules["builtin-net-address"]);
            for surface in &surfaces {
                assert_eq!(
                    surface.read(cx).keyword_highlight_config().line_roles,
                    !original_line_roles
                );
            }
            app.cancel_settings(cx);
            assert!(app.settings.keyword_config().enabled);
            assert!(!app.settings.keyword_config().builtin_rules["builtin-net-address"]);
            for surface in &surfaces {
                assert_eq!(
                    surface.read(cx).keyword_highlight_config().line_roles,
                    original_line_roles
                );
            }

            let mut reloaded = app.settings.summary().clone();
            reloaded.ui_highlight_line_roles = !original_line_roles;
            app.apply_gpui_settings(reloaded, cx);
            for surface in &surfaces {
                assert_eq!(
                    surface.read(cx).keyword_highlight_config().line_roles,
                    !original_line_roles
                );
            }
        });
    }
}

#[test]
fn line_role_changes_reject_the_in_flight_configuration_and_recompute() {
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let surface = cx.new(|_| TerminalSurface::new("highlight-test"));
    let mut screen = TerminalScreen::new(80, 4);
    screen.advance(b"-rw-r--r-- 1 root root 123 Oct 30 10:00 a.txt");
    cx.update_entity(&surface, |surface, cx| {
        surface.keyword_rules =
            Arc::new(nyaterm_core::keyword_highlight_presets::get_builtin_keyword_rules(true));
        install_keyword_test_frame(surface, Arc::new(screen.snapshot()));
        surface.schedule_keyword_highlights(true, false, cx);
        assert!(surface.keyword_highlight_task.is_some());
        for enabled in [false, true, false] {
            surface.set_line_roles(enabled, cx);
        }
        assert!(surface.keyword_highlights.is_none());
        assert!(surface.keyword_highlighter.is_none());
        assert!(
            surface.keyword_highlight_task.is_some(),
            "cancellation retains the single worker owner"
        );
    });
    cx.run_until_parked();
    cx.read_entity(&surface, |surface, _| {
        let config = surface.keyword_highlight_config();
        assert!(!config.line_roles);
        assert_eq!(
            surface.keyword_highlighter.as_ref().unwrap().config(),
            config
        );
        assert_eq!(
            surface.keyword_highlights.as_ref().unwrap().rules_key(),
            nyaterm_terminal_gpui::terminal_keyword_highlight_config_key(
                &surface.keyword_rules,
                config
            )
        );
        assert!(surface.keyword_highlight_task.is_none());
        assert!(surface.keyword_highlight_pending_key.is_none());
    });
}

#[test]
fn output_frames_coalesce_without_restarting_the_running_worker() {
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let surface = cx.new(|_| TerminalSurface::new("highlight-test"));
    let mut screen = TerminalScreen::new(60, 4);
    screen.advance(b"ERROR initial");
    cx.update_entity(&surface, |surface, cx| {
        surface.keyword_rules =
            Arc::new(nyaterm_core::keyword_highlight_presets::get_builtin_keyword_rules(true));
        install_keyword_test_frame(surface, Arc::new(screen.snapshot()));
        surface.schedule_keyword_highlights(true, false, cx);
        let generation = surface.keyword_highlight_generation;
        for index in 0..128 {
            screen.advance(format!("\r\nERROR output {index}").as_bytes());
            install_keyword_test_frame(surface, Arc::new(screen.snapshot()));
            surface.schedule_keyword_highlights(true, false, cx);
            assert_eq!(surface.keyword_highlight_generation, generation);
        }
    });
    cx.run_until_parked();
    cx.read_entity(&surface, |surface, _| {
        let snapshot = surface.snapshot.as_ref().unwrap();
        assert!(
            surface
                .keyword_highlights
                .as_ref()
                .unwrap()
                .matches_snapshot(snapshot, surface.palette)
        );
        assert!(surface.keyword_highlight_task.is_none());
        assert!(surface.keyword_highlight_pending_key.is_none());
    });
}

#[test]
fn changing_only_the_palette_invalidates_compiled_and_published_highlights() {
    let rules = Arc::new(nyaterm_core::keyword_highlight_presets::get_builtin_keyword_rules(true));
    let mut surface = TerminalSurface::new("palette-test");
    surface.keyword_rules = rules.clone();
    surface.keyword_highlighter_rules = Some(rules.clone());
    surface.keyword_highlighter = Some(Arc::new(compile_terminal_keyword_highlighter(
        &rules,
        surface.keyword_highlight_config(),
    )));
    surface.palette.terminal_ansi[12] ^= 0x00ffff;
    assert!(
        surface
            .cached_keyword_highlighter_for_rules(&rules)
            .is_none()
    );
    surface.invalidate_keyword_highlight_config();
    assert!(surface.keyword_highlighter.is_none());
    assert!(surface.keyword_highlights.is_none());
}

#[test]
fn full_history_repeated_rotation_keeps_text_selection_and_row_allocations() {
    for limit in [128, 5000, 100000] {
        assert_history_rotation_reuses_rows(limit);
    }
}

fn assert_history_rotation_reuses_rows(limit: usize) {
    let mut screen = TerminalScreen::new(32, 4);
    screen.set_scrollback_limit(limit);
    for index in 0..limit + 6 {
        screen.advance_decoded_text(&format!("line {index}\r\n"));
    }
    let original = Arc::new(screen.viewport_snapshot_with_window(40, 8, 8));
    let mut surface = TerminalSurface::new("session");
    apply_test_frame_snapshot!(
        surface,
        original.clone(),
        40,
        0.0,
        40,
        limit,
        4,
        false,
        None,
        0,
        false,
        false,
        "block"
    );
    let start = original.total_rows - original.display_offset - original.row_count();
    let selection = TerminalSelection::from_range(
        TerminalBufferCellPos::new(start + 8, 1),
        TerminalBufferCellPos::new(start + 9, 3),
    );
    surface.set_selection_visual(Some(selection));
    let cache_keys: Vec<_> = surface.retained_rows.keys().copied().collect();
    for delta in 1..=20 {
        screen.advance(b"new line\r\n");
        let state = TerminalScrollVisualState {
            history: screen.history_state(),
            session_id: "session".into(),
            scroll_offset: 40 + delta,
            scroll_residual_lines: 0.0,
            display_offset: 40 + delta,
            scrollback_len: limit,
            viewport_rows: 4,
            has_new_while_scrolled: true,
            performance_overlay: None,
            skipped_output_chars: 0,
        };
        surface.apply_scroll_visual_state(state);
        let retained = surface.snapshot.as_ref().unwrap();
        assert!(Arc::ptr_eq(&original.row_data, &retained.row_data));
        assert_eq!(
            retained.line(8),
            screen.viewport_snapshot(40 + delta).line(0)
        );
        assert_eq!(surface.selection_visual, selection.after_eviction(delta));
        assert_eq!(surface.retained_row_origin, delta);
        assert_eq!(
            surface.retained_rows.keys().copied().collect::<Vec<_>>(),
            cache_keys
        );
        assert!(!surface.scroll_snapshot_pending);
        assert!(surface.retained_rows.len() <= TERMINAL_SURFACE_RETAINED_ROW_LIMIT);
        assert!(surface.retained_row_estimated_bytes <= TERMINAL_SURFACE_RETAINED_ROW_BYTE_LIMIT);
    }
}

#[test]
fn late_oldest_prefetch_supplies_retained_rows_without_committing_its_stale_window() {
    let mut screen = TerminalScreen::new(16, 3);
    screen.set_scrollback_limit(128);
    screen.advance(&b"history\r\n".repeat(134));
    let paint = Arc::new(screen.viewport_snapshot(128));
    let prefetch = screen.viewport_snapshot_with_window(128, 0, 16);
    let mut surface = TerminalSurface::new("session");
    apply_test_frame_snapshot!(
        surface,
        paint.clone(),
        128,
        0.0,
        128,
        128,
        3,
        false,
        None,
        0,
        false,
        false,
        "block"
    );
    screen.advance(b"new\r\nnew\r\n");
    let state = TerminalScrollVisualState {
        history: screen.history_state(),
        session_id: "session".into(),
        scroll_offset: 128,
        scroll_residual_lines: 0.0,
        display_offset: 128,
        scrollback_len: 128,
        viewport_rows: 3,
        has_new_while_scrolled: true,
        performance_overlay: None,
        skipped_output_chars: 0,
    };
    surface.apply_scroll_visual_state(state.clone());
    assert!(Arc::ptr_eq(surface.snapshot.as_ref().unwrap(), &paint));
    assert!(!surface.retain_prefetched_snapshot(Arc::new(prefetch.clone())));
    assert!(surface.retain_surviving_prefetched_rows(&prefetch));
    surface.apply_scroll_visual_state(state);
    let snapshot = surface.snapshot.as_ref().unwrap();
    assert_eq!(snapshot.history, screen.history_state());
    assert!(Arc::ptr_eq(&snapshot.row_data[0], &prefetch.row_data[2]));
    assert!(!surface.scroll_snapshot_pending);
}

#[test]
fn late_oldest_logical_frame_paints_only_retained_rows_with_its_source_index() {
    let mut screen = TerminalScreen::new(16, 3);
    screen.set_scrollback_limit(128);
    screen.set_logical_line_snapshots(true);
    screen.advance(&b"history\r\n".repeat(134));
    let snapshot = Arc::new(screen.viewport_snapshot_with_window(128, 0, 16));
    let mut surface = TerminalSurface::new("session");
    surface.wrap_mode = nyaterm_core::models::settings::TerminalWrapMode::None;
    screen.advance(b"new\r\nnew\r\n");
    let state = TerminalScrollVisualState {
        history: screen.history_state(),
        session_id: "session".into(),
        scroll_offset: 128,
        scroll_residual_lines: 0.0,
        display_offset: 128,
        scrollback_len: 128,
        viewport_rows: 3,
        has_new_while_scrolled: true,
        performance_overlay: None,
        skipped_output_chars: 0,
    };
    assert!(surface.apply_retained_logical_frame(snapshot.clone(), &state));
    assert_eq!(surface.history, state.history);
    assert_eq!(surface.display_offset, 126);
    assert!(Arc::ptr_eq(surface.snapshot.as_ref().unwrap(), &snapshot));
    assert!(!surface.has_snapshot_covering_display_offset(128, 3, 128));
}

#[test]
fn full_history_missing_target_keeps_last_paint_and_its_decorations() {
    let mut screen = TerminalScreen::new(16, 3);
    screen.set_scrollback_limit(8);
    screen.advance(b"0\r\n1\r\n2\r\n3\r\n4\r\n5\r\n6\r\n7\r\n8\r\n9\r\n10\r\n");
    let old = Arc::new(screen.viewport_snapshot(8));
    let mut surface = TerminalSurface::new("session");
    apply_test_frame_snapshot!(
        surface,
        old.clone(),
        8,
        0.0,
        8,
        8,
        3,
        false,
        None,
        0,
        false,
        false,
        "block"
    );
    surface.decorations = Arc::from(vec![TerminalLineDecorations {
        link_ranges: vec![(0, 1)],
        ..Default::default()
    }]);
    let decorations = surface.decorations.clone();
    screen.advance(b"11\r\n");
    surface.apply_scroll_visual_state(TerminalScrollVisualState {
        history: screen.history_state(),
        session_id: "session".into(),
        scroll_offset: 8,
        scroll_residual_lines: 0.0,
        display_offset: 8,
        scrollback_len: 8,
        viewport_rows: 3,
        has_new_while_scrolled: true,
        performance_overlay: None,
        skipped_output_chars: 0,
    });
    assert!(Arc::ptr_eq(surface.snapshot.as_ref().unwrap(), &old));
    assert!(Arc::ptr_eq(&surface.decorations, &decorations));
    assert!(!surface.has_snapshot_covering_display_offset(8, 3, 8));
}

#[test]
fn full_history_mixed_live_prefetch_synthesizes_only_surviving_rows() {
    let mut screen = TerminalScreen::new(32, 4);
    screen.set_scrollback_limit(128);
    for index in 0..134 {
        screen.advance_decoded_text(&format!("line {index}\r\n"));
    }
    let original = Arc::new(screen.viewport_snapshot_with_window(20, 8, 20));
    let reading = screen.viewport_snapshot(20).line(0).unwrap().to_string();
    let mut surface = TerminalSurface::new("session");
    apply_test_frame_snapshot!(
        surface,
        original.clone(),
        20,
        0.0,
        20,
        128,
        4,
        false,
        None,
        0,
        false,
        false,
        "block"
    );
    screen.advance(b"new line\r\n");
    surface.apply_scroll_visual_state(TerminalScrollVisualState {
        history: screen.history_state(),
        session_id: "session".into(),
        scroll_offset: 21,
        scroll_residual_lines: 0.0,
        display_offset: 21,
        scrollback_len: 128,
        viewport_rows: 4,
        has_new_while_scrolled: true,
        performance_overlay: None,
        skipped_output_chars: 0,
    });
    let fresh = surface.snapshot.as_ref().unwrap();
    assert_eq!(fresh.history, screen.history_state());
    let anchor = terminal_snapshot_anchor_row_for_display_offset(fresh, 21, 4, 128);
    assert_eq!(fresh.line(anchor), Some(reading.as_str()));
    assert!(
        original
            .rows()
            .iter()
            .any(|row| Arc::ptr_eq(row, &fresh.row_data[anchor]))
    );
}

#[test]
fn full_history_no_wrap_wait_keeps_long_line_frame_and_source_index() {
    let mut screen = TerminalScreen::new(8, 3);
    screen.set_scrollback_limit(128);
    screen.set_logical_line_snapshots(true);
    screen.advance("x".repeat(800).as_bytes());
    screen.advance(b"\r\n");
    for _ in 0..35 {
        screen.advance(b"tail\r\n");
    }
    let old = Arc::new(screen.viewport_snapshot(50));
    let mut surface = TerminalSurface::new("session");
    surface.wrap_mode = nyaterm_core::models::settings::TerminalWrapMode::None;
    apply_test_frame_snapshot!(
        surface,
        old.clone(),
        50,
        0.0,
        50,
        128,
        3,
        false,
        None,
        0,
        false,
        false,
        "block"
    );
    screen.advance(b"new\r\n");
    surface.apply_scroll_visual_state(TerminalScrollVisualState {
        history: screen.history_state(),
        session_id: "session".into(),
        scroll_offset: 51,
        scroll_residual_lines: 0.0,
        display_offset: 51,
        scrollback_len: 128,
        viewport_rows: 3,
        has_new_while_scrolled: true,
        performance_overlay: None,
        skipped_output_chars: 0,
    });
    assert!(Arc::ptr_eq(surface.snapshot.as_ref().unwrap(), &old));
    assert!(surface.retained_rows.is_empty());
    assert!(!surface.has_snapshot_covering_display_offset(51, 3, 128));
    assert!(surface.unwrapped_layout(&old).is_some());
}
