use gpui::{
    AnyElement, App, ClickEvent, FontWeight, IntoElement, MouseButton, SharedString, Stateful,
    TitlebarOptions, Window, WindowControlArea, deferred, div, prelude::*, px, rgb, rgba, svg,
};

use super::child_window::ChildWindowChrome;
use crate::theme::ThemePalette;
use nyaterm_ui::{NyaButton, NyaButtonVariant};

pub(in crate::features) fn logo_mark(palette: ThemePalette) -> impl IntoElement {
    div()
        .size(px(22.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .child(super::icons::nyaterm_app_icon(palette, 22.))
}

pub(in crate::features) fn vertical_resize_handle_visual(
    palette: ThemePalette,
    dragging: bool,
    highlighted: bool,
) -> gpui::Div {
    div()
        .relative()
        .w(px(5.))
        .ml(px(-2.))
        .mr(px(-2.))
        .h_full()
        .flex_none()
        .child(
            div()
                .absolute()
                .left(px(2.))
                .top_0()
                .bottom_0()
                .w(px(1.))
                .bg(rgb(palette.border)),
        )
        .child(
            div()
                .absolute()
                .left(px(1.))
                .top_0()
                .bottom_0()
                .w(px(3.))
                .bg(if dragging || highlighted {
                    rgb(palette.primary)
                } else {
                    rgba(0x00000000)
                }),
        )
}

pub(in crate::features) fn horizontal_resize_handle_visual(
    palette: ThemePalette,
    dragging: bool,
    highlighted: bool,
) -> gpui::Div {
    div()
        .relative()
        .h(px(5.))
        .mt(px(-2.))
        .mb(px(-2.))
        .w_full()
        .flex_none()
        .child(
            div()
                .absolute()
                .left_0()
                .right_0()
                .top(px(2.))
                .h(px(1.))
                .bg(rgb(palette.border)),
        )
        .child(
            div()
                .absolute()
                .left_0()
                .right_0()
                .top(px(1.))
                .h(px(3.))
                .bg(if dragging || highlighted {
                    rgb(palette.primary)
                } else {
                    rgba(0x00000000)
                }),
        )
}

pub(in crate::features) fn window_control_button(
    palette: ThemePalette,
    id: &'static str,
    icon_path: &'static str,
    area: WindowControlArea,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let hovered_color = if matches!(area, WindowControlArea::Close) {
        0xffffff
    } else {
        palette.text
    };
    div()
        .id(SharedString::from(id))
        .group(SharedString::from(id))
        .w(px(46.))
        .h_full()
        .flex()
        .items_center()
        .justify_center()
        .text_color(rgb(palette.text_muted))
        .window_control_area(area)
        .cursor_pointer()
        .hover(|this| {
            if matches!(area, WindowControlArea::Close) {
                this.bg(rgb(0xe81123)).text_color(rgb(0xffffff))
            } else {
                this.bg(rgb(palette.hover)).text_color(rgb(palette.text))
            }
        })
        .child(
            svg()
                .size(px(16.))
                .flex_none()
                .path(icon_path)
                .text_color(rgb(palette.text_muted))
                .group_hover(SharedString::from(id), move |this| {
                    this.text_color(rgb(hovered_color))
                }),
        )
        .on_click(on_click)
}

/// The 40px bar every child window draws in place of a native title bar.
///
/// Which controls appear is read off the window itself rather than passed in, so
/// a window cannot end up offering a button the platform will ignore -- or, worse,
/// omitting one it needs: a resizable window with no restore button can still be
/// maximised with `Win`+`Up` and then has no way back.
pub(in crate::features) fn child_window_header(
    palette: ThemePalette,
    title: impl Into<SharedString>,
    icon_path: Option<&'static str>,
    chrome: ChildWindowChrome,
    window: &Window,
    on_close: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let title = title.into();
    let is_maximized = window.is_maximized();
    let minimizable = window.is_minimizable();
    let maximizable = window.is_resizable();
    // A sheet has no title bar: nothing to leave a traffic-light gutter for, and
    // nothing to drag it by.
    let has_title_bar = chrome == ChildWindowChrome::Window;
    div()
        .h(px(40.))
        .flex_none()
        .flex()
        .items_center()
        .border_b_1()
        .border_color(rgb(palette.border))
        .bg(rgb(palette.surface))
        .child(
            div()
                .h_full()
                .min_w_0()
                .flex_1()
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .when(cfg!(target_os = "macos") && has_title_bar, |this| {
                    // Room for the traffic lights AppKit draws over the content.
                    this.pl(px(70.))
                })
                .when(has_title_bar, |this| {
                    this.window_control_area(WindowControlArea::Drag)
                })
                .when_some(icon_path, |this, icon_path| {
                    this.child(
                        svg()
                            .size(px(16.))
                            .flex_none()
                            .path(icon_path)
                            .text_color(rgb(palette.primary)),
                    )
                })
                .child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .text_sm()
                        .font_weight(FontWeight(500.))
                        .text_color(rgb(palette.text))
                        .child(title),
                ),
        )
        .child(
            div()
                .h_full()
                .flex_none()
                .flex()
                .items_center()
                .when(!cfg!(target_os = "macos") && minimizable, |this| {
                    this.child(window_control_button(
                        palette,
                        "child-window-min",
                        "icons/window/minimize.svg",
                        WindowControlArea::Min,
                        |_, window, _| window.minimize_window(),
                    ))
                })
                .when(!cfg!(target_os = "macos") && maximizable, |this| {
                    this.child(window_control_button(
                        palette,
                        "child-window-max",
                        if is_maximized {
                            "icons/window/restore.svg"
                        } else {
                            "icons/window/maximize.svg"
                        },
                        WindowControlArea::Max,
                        |_, window, _| window.zoom_window(),
                    ))
                })
                .when(!cfg!(target_os = "macos"), |this| {
                    this.child(window_control_button(
                        palette,
                        "child-window-close",
                        "icons/window/close.svg",
                        WindowControlArea::Close,
                        on_close,
                    ))
                }),
        )
}

pub(in crate::features) fn child_window_titlebar(
    title: impl Into<SharedString>,
) -> Option<TitlebarOptions> {
    cfg!(target_os = "macos").then(|| TitlebarOptions {
        title: Some(title.into()),
        appears_transparent: true,
        ..Default::default()
    })
}

pub(in crate::features) fn panel_header_with_actions(
    title: impl Into<SharedString>,
    meta: impl Into<SharedString>,
    palette: ThemePalette,
    background: gpui::Rgba,
    actions: Option<AnyElement>,
) -> impl IntoElement {
    // Tauri PanelHeader: min-h-9, uppercase tracked title + dimmed meta/actions.
    let title = title.into();
    let meta = meta.into();
    let show_meta = !meta.is_empty();
    div()
        .h(px(36.))
        .flex_none()
        .flex()
        .items_center()
        .justify_between()
        .gap_3()
        .px_3()
        .border_b_1()
        .border_color(rgb(palette.border))
        .bg(background)
        .child(
            div()
                .min_w_0()
                .flex_1()
                .flex()
                .items_baseline()
                .gap_2()
                .child(
                    div()
                        .text_size(px(11.))
                        .font_weight(FontWeight(700.))
                        .text_color(rgb(palette.text_muted))
                        .child(title.to_uppercase()),
                )
                .when(show_meta, |this| {
                    this.child(
                        div()
                            .min_w_0()
                            .text_size(px(11.))
                            .text_color(rgb(palette.text_muted))
                            .opacity(0.85)
                            .overflow_hidden()
                            .child(meta),
                    )
                }),
        )
        .when_some(actions, |this, actions| {
            this.child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(actions),
            )
        })
}

/// Full-window boundary for modal and outside-click overlays.
///
/// GPUI hitboxes do not implicitly block hitboxes painted behind them. Keep
/// this boundary around every overlay that owns the pointer while it is open.
pub(in crate::features) fn full_window_input_layer(id: impl Into<String>) -> Stateful<gpui::Div> {
    div()
        .id(SharedString::from(id.into()))
        .key_context(crate::shortcuts::MODAL_KEY_CONTEXT)
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
/// matches the window-wide contract documented in `nyaterm_ui::root`: above the
/// main-interface dividers, below `gpui-kit` popups and tooltips.
pub(in crate::features) const APP_OVERLAY_PRIORITY: usize = 1;

/// `full_window_input_layer` lifted above the resize handles.
///
/// Deferring also drops the ancestor content mask, so an overlay positioned near
/// a panel edge is no longer clipped by it.
pub(in crate::features) fn full_window_overlay_layer(
    id: impl Into<String>,
    content: impl IntoElement,
) -> gpui::Deferred {
    deferred(full_window_input_layer(id).child(content)).with_priority(APP_OVERLAY_PRIORITY)
}

/// Lifts an overlay that deliberately owns no input layer above the resize
/// handles. Hover-driven surfaces must not block the pointer underneath them.
pub(in crate::features) fn passive_overlay_layer(content: impl IntoElement) -> gpui::Deferred {
    deferred(content).with_priority(APP_OVERLAY_PRIORITY)
}

/// Dimmed full-area modal shell (Tauri Dialog backdrop + centered card).
/// A dialog centred over whatever hosts it, dimming what is behind it.
///
/// The overlay fills its nearest positioned ancestor, so a dialog belongs to a
/// host that spans the window — the app root — rather than to the panel that
/// owns the state. A panel is often a couple of hundred pixels wide, and a form
/// laid out in there wraps every caption onto its own lines.
pub(in crate::features) fn modal_dialog_shell(
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
pub(in crate::features) fn bounded_dialog_width(
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

pub(in crate::features) fn dialog_action_button(
    _palette: ThemePalette,
    id: impl Into<String>,
    label: impl Into<SharedString>,
    danger: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
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
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Duration;

    use gpui::{
        Context, InteractiveElement as _, IntoElement, Modifiers, MouseButton, ParentElement as _,
        Render, StatefulInteractiveElement as _, Styled as _, TestAppContext, VisualTestContext,
        Window, canvas, deferred, div, point, prelude::FluentBuilder as _, px,
    };

    use super::{
        bounded_dialog_width, full_window_input_layer, horizontal_resize_handle_visual,
        vertical_resize_handle_visual,
    };
    use crate::features::shell::ResizeHandleHoverState;
    use crate::theme::theme_palette;

    struct InputLayerFixture {
        lower_events: Arc<AtomicUsize>,
        backdrop_clicks: Arc<AtomicUsize>,
        child_clicks: Arc<AtomicUsize>,
    }

    struct ResizeHandleFixture {
        hovered: Arc<AtomicBool>,
        rendered_highlight: Arc<AtomicBool>,
        hover_paints: Arc<AtomicUsize>,
        mouse_downs: Arc<AtomicUsize>,
        hover: ResizeHandleHoverState,
    }

    const TEST_HOVER_DELAY: Duration = Duration::from_millis(250);

    impl ResizeHandleFixture {
        fn update_hover(&mut self, hovered: bool, cx: &mut Context<Self>) {
            if !hovered {
                self.hover.leave(&"test-vertical-resize".into());
                self.hovered.store(false, Ordering::SeqCst);
                cx.notify();
                return;
            }
            if self.hovered.swap(true, Ordering::SeqCst) {
                return;
            }
            let id = "test-vertical-resize".into();
            let Some(generation) = self.hover.begin(id) else {
                return;
            };
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(TEST_HOVER_DELAY).await;
                let _ = this.update(cx, |this, cx| {
                    if this
                        .hover
                        .activate(&"test-vertical-resize".into(), generation)
                    {
                        cx.notify();
                    }
                });
            })
            .detach();
        }
    }

    impl Render for ResizeHandleFixture {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let palette = theme_palette("github-dark");
            let hover_paints = self.hover_paints.clone();
            let mouse_downs = self.mouse_downs.clone();
            let vertical_group = "test-vertical-resize";
            let vertical_id = "test-vertical-resize".into();
            let highlighted = self.hover.is_highlighted(&vertical_id);
            self.rendered_highlight.store(highlighted, Ordering::SeqCst);

            div()
                .size(px(120.))
                .flex()
                .items_start()
                .child(
                    div()
                        .w(px(20.))
                        .h(px(40.))
                        .debug_selector(|| "resize-left-content".to_string()),
                )
                .child(deferred(
                    vertical_resize_handle_visual(palette, false, highlighted)
                        .id(vertical_group)
                        .h(px(40.))
                        .debug_selector(|| "vertical-resize-hitbox".to_string())
                        .cursor_col_resize()
                        .on_hover(cx.listener(|this, is_hovered: &bool, _, cx| {
                            this.update_hover(*is_hovered, cx);
                        }))
                        .when(highlighted, |this| {
                            this.child(div().absolute().inset_0().invisible().visible().child(
                                canvas(
                                    |_, _, _| {},
                                    move |_, _, _, _| {
                                        hover_paints.fetch_add(1, Ordering::SeqCst);
                                    },
                                ),
                            ))
                        })
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                this.hover.activate_immediately(vertical_id.clone());
                                mouse_downs.fetch_add(1, Ordering::SeqCst);
                                cx.notify();
                            }),
                        ),
                ))
                .child(
                    div()
                        .w(px(20.))
                        .h(px(40.))
                        .debug_selector(|| "resize-right-content".to_string()),
                )
                .child(
                    div()
                        .ml(px(12.))
                        .w(px(40.))
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .w(px(40.))
                                .h(px(20.))
                                .debug_selector(|| "resize-top-content".to_string()),
                        )
                        .child(deferred(
                            horizontal_resize_handle_visual(palette, false, false)
                                .id("test-horizontal-resize")
                                .debug_selector(|| "horizontal-resize-hitbox".to_string())
                                .cursor_row_resize(),
                        ))
                        .child(
                            div()
                                .w(px(40.))
                                .h(px(20.))
                                .debug_selector(|| "resize-bottom-content".to_string()),
                        ),
                )
        }
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
    fn resize_handles_hover_without_clicking_and_keep_one_pixel_layout_gap(
        cx: &mut TestAppContext,
    ) {
        let hovered = Arc::new(AtomicBool::new(false));
        let rendered_highlight = Arc::new(AtomicBool::new(false));
        let hover_paints = Arc::new(AtomicUsize::new(0));
        let mouse_downs = Arc::new(AtomicUsize::new(0));
        let fixture = ResizeHandleFixture {
            hovered: hovered.clone(),
            rendered_highlight: rendered_highlight.clone(),
            hover_paints: hover_paints.clone(),
            mouse_downs: mouse_downs.clone(),
            hover: ResizeHandleHoverState::default(),
        };
        let (_, cx) = cx.add_window_view(|_, _| fixture);
        let cx: &mut VisualTestContext = cx;
        draw(cx);

        let left = cx.debug_bounds("resize-left-content").unwrap();
        let right = cx.debug_bounds("resize-right-content").unwrap();
        let vertical = cx.debug_bounds("vertical-resize-hitbox").unwrap();
        let top = cx.debug_bounds("resize-top-content").unwrap();
        let bottom = cx.debug_bounds("resize-bottom-content").unwrap();
        let horizontal = cx.debug_bounds("horizontal-resize-hitbox").unwrap();

        assert_eq!(vertical.size.width, px(5.));
        assert_eq!(right.left() - left.right(), px(1.));
        assert_eq!(horizontal.size.height, px(5.));
        assert_eq!(bottom.top() - top.bottom(), px(1.));
        assert_eq!(hover_paints.load(Ordering::SeqCst), 0);

        cx.simulate_mouse_move(vertical.center(), None, Modifiers::default());
        assert!(hovered.load(Ordering::SeqCst));
        assert!(!rendered_highlight.load(Ordering::SeqCst));
        assert_eq!(hover_paints.load(Ordering::SeqCst), 0);

        cx.executor().advance_clock(Duration::from_millis(249));
        cx.run_until_parked();
        draw(cx);
        assert_eq!(hover_paints.load(Ordering::SeqCst), 0);

        cx.simulate_mouse_move(
            point(vertical.center().x, vertical.top() + px(1.)),
            None,
            Modifiers::default(),
        );
        cx.executor().advance_clock(Duration::from_millis(1));
        cx.run_until_parked();
        draw(cx);
        assert!(rendered_highlight.load(Ordering::SeqCst));
        assert!(hover_paints.load(Ordering::SeqCst) > 0);

        cx.simulate_mouse_move(point(px(100.), px(100.)), None, Modifiers::default());
        assert!(!hovered.load(Ordering::SeqCst));
        draw(cx);
        assert!(!rendered_highlight.load(Ordering::SeqCst));

        let paints_after_exit = hover_paints.load(Ordering::SeqCst);
        cx.simulate_mouse_move(vertical.center(), None, Modifiers::default());
        cx.executor().advance_clock(Duration::from_millis(249));
        cx.run_until_parked();
        cx.simulate_mouse_move(point(px(100.), px(100.)), None, Modifiers::default());
        cx.executor().advance_clock(Duration::from_millis(1));
        cx.run_until_parked();
        draw(cx);
        assert_eq!(hover_paints.load(Ordering::SeqCst), paints_after_exit);

        let hitbox_edge = point(vertical.left() + px(0.5), vertical.center().y);
        cx.simulate_mouse_move(hitbox_edge, None, Modifiers::default());
        cx.simulate_mouse_down(hitbox_edge, MouseButton::Left, Modifiers::default());
        draw(cx);
        assert!(rendered_highlight.load(Ordering::SeqCst));
        assert_eq!(mouse_downs.load(Ordering::SeqCst), 1);
        assert!(hover_paints.load(Ordering::SeqCst) > paints_after_exit);
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
