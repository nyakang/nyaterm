use gpui::{
    AnyElement, App, ClickEvent, IntoElement, SharedString, Window, WindowControlArea, div,
    prelude::*, px,
};
pub(in crate::features) use nyaterm_ui::presentation::{
    conversation_panel_header_with_actions, panel_header_with_actions,
};

use super::child_window::ChildWindowChrome;
use crate::theme::ThemePalette;
use crate::window_chrome::WindowChrome;

pub(in crate::features) fn logo_mark(palette: ThemePalette) -> impl IntoElement {
    div()
        .size(px(22.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .child(nyaterm_ui::icons::nyaterm_app_icon(palette, 22.))
}

pub(in crate::features) use nyaterm_ui::resize_handle::{
    horizontal_resize_handle_borderless, horizontal_resize_handle_visual,
    vertical_resize_handle_visual,
};

pub(in crate::features) fn window_control_button(
    palette: ThemePalette,
    id: &'static str,
    icon_path: &'static str,
    area: WindowControlArea,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    window_control_button_with_area(palette, id, icon_path, area, true, on_click)
}

fn window_control_button_with_area(
    palette: ThemePalette,
    id: &'static str,
    icon_path: &'static str,
    area: WindowControlArea,
    native_control: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    nyaterm_ui::window_chrome::window_control_button(
        palette,
        id,
        icon_path,
        area,
        native_control,
        on_click,
    )
}

fn toggle_child_window_zoom(window: &mut Window, _cx: &mut App) {
    #[cfg(target_os = "windows")]
    if window.is_maximized()
        && let Ok(handle) = raw_window_handle::HasWindowHandle::window_handle(window)
        && let raw_window_handle::RawWindowHandle::Win32(handle) = handle.as_raw()
    {
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::ShowWindow(
                handle.hwnd.get() as _,
                windows_sys::Win32::UI::WindowsAndMessaging::SW_RESTORE,
            );
        }
        // The native restore can complete before GPUI's queued resize callback.
        // Synchronize its viewport now and force a full redraw of the child window.
        window.bounds_changed(_cx);
        return;
    }

    window.zoom_window();
}

pub(in crate::features) use nyaterm_ui::overlay::{
    APP_OVERLAY_PRIORITY, bounded_dialog_width, dialog_action_button, full_window_input_layer,
    full_window_overlay_layer, modal_dialog_shell, modal_overlay_layer, passive_overlay_layer,
    regional_overlay_layer, selectable_overlay_layer,
};

/// The 40px child-window header, omitted when Linux supplies a system title bar.
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
    window: &mut Window,
    cx: &mut App,
    on_close: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let policy = WindowChrome::child(window, chrome == ChildWindowChrome::Sheet);
    nyaterm_ui::window_chrome::child_window_header(
        palette,
        title,
        icon_path,
        nyaterm_ui::window_chrome::ChildWindowPresentation {
            show_child_header: policy.show_child_header,
            minimize: policy.minimize,
            maximize: policy.maximize,
            close: policy.close,
            restore: policy.restore,
            native_button_gutter: policy.native_button_gutter,
            drag: policy.drag,
        },
        window,
        cx,
        nyaterm_ui::window_chrome::ChildWindowActions {
            on_close,
            on_zoom: toggle_child_window_zoom,
        },
    )
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

    use super::{horizontal_resize_handle_visual, vertical_resize_handle_visual};
    use crate::features::shell::ResizeHandleHoverState;
    use crate::theme::theme_palette;

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

    fn draw(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| {
            _ = window.draw(cx);
        });
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
}
