//! Shared modal input boundaries and overlay layout.
use crate::{NyaButton, NyaButtonVariant, ThemePalette};
use gpui::{
    App, ClickEvent, IntoElement, MouseButton, SharedString, Stateful, Window, deferred, div,
    prelude::*, px, rgb, rgba,
};

pub const MODAL_KEY_CONTEXT: &str = "NyaModal";

/// Full-window boundary for modal and outside-click overlays.
///
/// GPUI hitboxes do not implicitly block hitboxes painted behind them. Keep
/// this boundary around every overlay that owns the pointer while it is open.
pub fn full_window_input_layer(id: impl Into<String>) -> Stateful<gpui::Div> {
    div()
        .id(SharedString::from(id.into()))
        .key_context(MODAL_KEY_CONTEXT)
        .absolute()
        .inset_0()
        .occlude()
        .on_any_mouse_down(|_, _, cx| cx.stop_propagation())
        .on_mouse_up(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_up(MouseButton::Middle, |_, _, cx| cx.stop_propagation())
        .on_mouse_up(MouseButton::Right, |_, _, cx| cx.stop_propagation())
        .on_mouse_move(|_, _, cx| cx.stop_propagation())
}

/// Deferred priority for the app's own overlays.
///
/// The resize handles are `deferred()` at the default priority `0`, and deferred
/// draws paint after the whole normal tree, so a handle would otherwise draw its
/// border line across an overlay and win hit-testing inside its 5px band. `1`
/// matches the window-wide contract documented in `crate::root`: above the
/// main-interface dividers, below `gpui-kit` popups and tooltips.
pub const APP_OVERLAY_PRIORITY: usize = 1;

/// `full_window_input_layer` lifted above the resize handles.
///
/// Deferring also drops the ancestor content mask, so an overlay positioned near
/// a panel edge is no longer clipped by it.
pub fn full_window_overlay_layer(
    id: impl Into<String>,
    content: impl IntoElement,
) -> gpui::Deferred {
    deferred(crate::nya_modal_overlay(
        full_window_input_layer(id).child(content),
    ))
    .with_priority(APP_OVERLAY_PRIORITY)
}

/// Overlay layer that keeps the window's text-selection listeners alive.
///
/// [`full_window_input_layer`] stops mouse propagation, and window-level mouse
/// listeners run in paint order, so its `stop_propagation` also starves a
/// selectable text painted inside the overlay. `occlude()` already keeps the
/// pointer away from the elements behind the layer.
pub fn selectable_overlay_layer(
    id: impl Into<String>,
    content: impl IntoElement,
) -> gpui::Deferred {
    deferred(crate::nya_modal_overlay(
        div()
            .id(SharedString::from(id.into()))
            .key_context(MODAL_KEY_CONTEXT)
            .absolute()
            .inset_0()
            .occlude()
            .child(content),
    ))
    .with_priority(APP_OVERLAY_PRIORITY)
}

/// Positioning shells do not own input; only menu controls contribute regions.
pub fn regional_overlay_layer(_id: impl Into<String>, content: impl IntoElement) -> gpui::Deferred {
    deferred(crate::nya_region_overlay(content)).with_priority(APP_OVERLAY_PRIORITY)
}

/// Lifts a modal that already supplies its own input boundary. In particular,
/// the lock screen must let Windows caption presses reach the window procedure.
pub fn modal_overlay_layer(content: impl IntoElement) -> gpui::Deferred {
    deferred(crate::nya_modal_overlay(content)).with_priority(APP_OVERLAY_PRIORITY)
}

/// Lifts an overlay that deliberately owns no input layer above the resize
/// handles. Hover-driven surfaces must not block the pointer underneath them.
pub fn passive_overlay_layer(content: impl IntoElement) -> gpui::Deferred {
    deferred(crate::nya_passive_overlay(content)).with_priority(APP_OVERLAY_PRIORITY)
}

/// Dimmed full-area modal shell (Tauri Dialog backdrop + centered card).
/// A dialog centred over whatever hosts it, dimming what is behind it.
///
/// The overlay fills its nearest positioned ancestor, so a dialog belongs to a
/// host that spans the window — the app root — rather than to the panel that
/// owns the state. A panel is often a couple of hundred pixels wide, and a form
/// laid out in there wraps every caption onto its own lines.
pub fn modal_dialog_shell(
    palette: ThemePalette,
    background: gpui::Rgba,
    id: impl Into<String>,
    width: f32,
    content: impl IntoElement,
) -> impl IntoElement {
    full_window_input_layer(id)
        .bg(rgba(0x00000080))
        .flex()
        .items_center()
        .justify_center()
        .p_3()
        .child(
            div()
                .w(px(width))
                .max_w_full()
                .max_h_full()
                .rounded_md()
                .border_1()
                .border_color(rgb(palette.border))
                .bg(background)
                .shadow_lg()
                .child(content),
        )
}

/// Bounds a dialog to the available viewport while preserving the historical
/// fallback to the preferred width when GPUI has not published a finite size.
pub fn bounded_dialog_width(
    viewport_width: f32,
    horizontal_inset: f32,
    minimum_width: f32,
    preferred_width: f32,
) -> f32 {
    let available_width = viewport_width - horizontal_inset;
    if available_width.is_nan() || available_width > preferred_width {
        preferred_width
    } else if available_width < minimum_width {
        minimum_width
    } else {
        available_width
    }
}

pub fn dialog_action_button(
    _palette: ThemePalette,
    id: impl Into<String>,
    label: impl Into<SharedString>,
    danger: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> NyaButton {
    let label: SharedString = label.into();
    let variant = if danger {
        NyaButtonVariant::Danger
    } else {
        NyaButtonVariant::Primary
    };

    NyaButton::new(id.into(), label)
        .variant(variant)
        .small()
        .compact()
        .on_click(on_click)
}

#[cfg(test)]
mod tests {
    use super::{bounded_dialog_width, full_window_input_layer};
    use gpui::{
        Context, IntoElement, Modifiers, MouseButton, Render, TestAppContext, VisualTestContext,
        Window, div, point, prelude::*, px,
    };
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    struct InputLayerFixture {
        lower_events: Arc<AtomicUsize>,
        backdrop_clicks: Arc<AtomicUsize>,
        child_clicks: Arc<AtomicUsize>,
    }

    struct SelectableInputLayerFixture {
        background_events: Arc<AtomicUsize>,
        overlay_clicks: Arc<AtomicUsize>,
    }

    impl Render for SelectableInputLayerFixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let down = self.background_events.clone();
            let up = self.background_events.clone();
            let moved = self.background_events.clone();
            let scrolled = self.background_events.clone();
            let clicked = self.overlay_clicks.clone();
            div()
                .size_full()
                .relative()
                .on_any_mouse_down(move |_, _, _| {
                    down.fetch_add(1, Ordering::SeqCst);
                })
                .on_mouse_up(MouseButton::Left, move |_, _, _| {
                    up.fetch_add(1, Ordering::SeqCst);
                })
                .on_mouse_move(move |_, _, _| {
                    moved.fetch_add(1, Ordering::SeqCst);
                })
                .on_scroll(move |_, _, _| {
                    scrolled.fetch_add(1, Ordering::SeqCst);
                })
                .child(super::selectable_overlay_layer(
                    "selectable-test-layer",
                    div()
                        .id("selectable-test-content")
                        .size_full()
                        .on_click(move |_, _, _| {
                            clicked.fetch_add(1, Ordering::SeqCst);
                        }),
                ))
        }
    }

    #[gpui::test]
    fn selectable_layer_occludes_background_pointer_and_scroll_handlers(cx: &mut TestAppContext) {
        let background_events = Arc::new(AtomicUsize::new(0));
        let overlay_clicks = Arc::new(AtomicUsize::new(0));
        let (_, cx) = cx.add_window_view(|_, _| SelectableInputLayerFixture {
            background_events: background_events.clone(),
            overlay_clicks: overlay_clicks.clone(),
        });
        draw(cx);
        let position = point(px(12.), px(12.));
        cx.simulate_mouse_move(position, None, Modifiers::none());
        cx.simulate_click(position, Modifiers::none());
        cx.simulate_event(gpui::ScrollEvent {
            position,
            delta: gpui::ScrollDelta::Lines(point(0., -3.)),
            modifiers: Modifiers::none(),
            touch_phase: gpui::TouchPhase::Moved,
            source: gpui::ScrollSource::Pointer,
        });
        assert_eq!(overlay_clicks.load(Ordering::SeqCst), 1);
        assert_eq!(background_events.load(Ordering::SeqCst), 0);
    }

    impl Render for InputLayerFixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let lower_down = self.lower_events.clone();
            let lower_move = self.lower_events.clone();
            let lower_up = self.lower_events.clone();
            let root_down = self.lower_events.clone();
            let root_move = self.lower_events.clone();
            let root_up = self.lower_events.clone();
            let backdrop_clicks = self.backdrop_clicks.clone();
            let child_clicks = self.child_clicks.clone();
            div()
                .size_full()
                .relative()
                .on_any_mouse_down(move |_, _, _| {
                    root_down.fetch_add(1, Ordering::SeqCst);
                })
                .on_mouse_move(move |_, _, _| {
                    root_move.fetch_add(1, Ordering::SeqCst);
                })
                .on_mouse_up(MouseButton::Left, move |_, _, _| {
                    root_up.fetch_add(1, Ordering::SeqCst);
                })
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .on_any_mouse_down(move |_, _, _| {
                            lower_down.fetch_add(1, Ordering::SeqCst);
                        })
                        .on_mouse_move(move |_, _, _| {
                            lower_move.fetch_add(1, Ordering::SeqCst);
                        })
                        .on_mouse_up(MouseButton::Left, move |_, _, _| {
                            lower_up.fetch_add(1, Ordering::SeqCst);
                        }),
                )
                .child(
                    full_window_input_layer("test-input-layer")
                        .on_click(move |_, _, _| {
                            backdrop_clicks.fetch_add(1, Ordering::SeqCst);
                        })
                        .child(
                            div()
                                .id("test-overlay-child")
                                .debug_selector(|| "test-overlay-child".to_string())
                                .absolute()
                                .left(px(100.))
                                .top(px(100.))
                                .size(px(40.))
                                .on_click(move |_, _, cx| {
                                    cx.stop_propagation();
                                    child_clicks.fetch_add(1, Ordering::SeqCst);
                                }),
                        ),
                )
        }
    }

    fn draw(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| {
            _ = window.draw(cx);
        });
    }

    #[test]
    fn dialog_width_bounds_available_space_and_keeps_non_finite_fallback() {
        assert_eq!(bounded_dialog_width(1280., 32., 280., 448.), 448.);
        assert_eq!(bounded_dialog_width(400., 32., 280., 448.), 368.);
        assert_eq!(bounded_dialog_width(200., 32., 280., 448.), 280.);
        assert_eq!(bounded_dialog_width(f32::NAN, 32., 280., 448.), 448.);
    }

    #[gpui::test]
    fn full_window_input_layer_blocks_lower_pointer_events_and_keeps_overlay_clicks(
        cx: &mut TestAppContext,
    ) {
        let lower_events = Arc::new(AtomicUsize::new(0));
        let backdrop_clicks = Arc::new(AtomicUsize::new(0));
        let child_clicks = Arc::new(AtomicUsize::new(0));
        let fixture = InputLayerFixture {
            lower_events: lower_events.clone(),
            backdrop_clicks: backdrop_clicks.clone(),
            child_clicks: child_clicks.clone(),
        };
        let (_, cx) = cx.add_window_view(|_, _| fixture);
        let cx: &mut VisualTestContext = cx;
        draw(cx);

        cx.simulate_mouse_move(point(px(12.), px(12.)), None, Modifiers::default());
        cx.simulate_click(point(px(12.), px(12.)), Modifiers::default());
        assert_eq!(backdrop_clicks.load(Ordering::SeqCst), 1);
        assert_eq!(lower_events.load(Ordering::SeqCst), 0);

        let child = cx
            .debug_bounds("test-overlay-child")
            .expect("overlay child should be rendered");
        cx.simulate_click(child.center(), Modifiers::default());
        assert_eq!(child_clicks.load(Ordering::SeqCst), 1);
        assert_eq!(backdrop_clicks.load(Ordering::SeqCst), 1);
        assert_eq!(lower_events.load(Ordering::SeqCst), 0);
    }
}
