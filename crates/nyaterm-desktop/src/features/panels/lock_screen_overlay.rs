use rust_i18n::t;

use gpui::{
    Context, FontWeight, IntoElement, KeyDownEvent, MouseButton, SharedString, Window,
    WindowControlArea, div, prelude::*, px, rgb, rgba, svg,
};
use nyaterm_ui::NyaInput;

use crate::features::view_widgets::{nyaterm_app_icon, window_control_button};
use crate::features::{NyaTermApp, text_inputs::TextInputSetup};
use crate::widgets::small_button;

impl NyaTermApp {
    pub(in crate::features) fn lock_screen_overlay(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let password_draft = self.security.screen_lock_password_draft().to_string();
        let password_input = self.text_input(
            "lock-screen.password",
            &password_draft,
            TextInputSetup::masked(),
            cx,
        );
        let password_focus = password_input.read(cx).focus_handle();
        let overlay_focus = if self.settings.summary().has_master_password {
            password_focus.clone()
        } else {
            self.security.screen_lock_focus().clone()
        };
        let lock_status = if self.security.screen_lock_status().trim().is_empty() {
            if self.settings.summary().has_master_password {
                t!("lockScreen.passwordPlaceholder").to_string()
            } else {
                t!("settings.masterPasswordRequired").to_string()
            }
        } else {
            self.security.screen_lock_status().to_string()
        };
        let status_is_error = lock_status == t!("lockScreen.wrongPassword")
            || lock_status.starts_with(t!("lockScreen.unlockFailed").as_ref());

        div()
            .id(SharedString::from("lock-screen-overlay"))
            .key_context(crate::shortcuts::SCREEN_LOCKED_KEY_CONTEXT)
            .absolute()
            .top_0()
            .bottom_0()
            .left_0()
            .right_0()
            .flex()
            .flex_col()
            .bg(rgba(0x000000d9))
            .text_color(rgb(0xffffff))
            .track_focus(self.security.screen_lock_focus())
            .on_click(move |_, window, cx| window.focus(&overlay_focus, cx))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if this.handle_lock_key_down(event, cx) {
                    cx.stop_propagation();
                }
            }))
            .when(!cfg!(target_os = "macos"), |this| {
                this.child(
                    div()
                        .h(px(40.))
                        .flex()
                        .items_center()
                        .text_color(rgb(0xb6bfcc))
                        .child(
                            div()
                                .h_full()
                                .flex_1()
                                .window_control_area(WindowControlArea::Drag)
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(Self::handle_title_bar_mouse_down),
                                ),
                        )
                        .child(window_control_button(
                            palette,
                            "lock-window-min",
                            "icons/window/minimize.svg",
                            WindowControlArea::Min,
                            |_, window, _| window.minimize_window(),
                        ))
                        .child(window_control_button(
                            palette,
                            "lock-window-max",
                            if window.is_maximized() {
                                "icons/window/restore.svg"
                            } else {
                                "icons/window/maximize.svg"
                            },
                            WindowControlArea::Max,
                            |_, window, _| window.zoom_window(),
                        ))
                        .child(window_control_button(
                            palette,
                            "lock-window-close",
                            "icons/window/close.svg",
                            WindowControlArea::Close,
                            cx.listener(|this, _, window, cx| {
                                this.handle_window_close_request(window, cx);
                            }),
                        )),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .id("lock-screen-content")
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap_5()
                            .on_click(|_, _, cx| cx.stop_propagation())
                            .child(
                                div()
                                    .relative()
                                    .child(
                                        div()
                                            .size(px(82.))
                                            .rounded_lg()
                                            .shadow_lg()
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .child(nyaterm_app_icon(palette, 82.)),
                                    )
                                    .child(
                                        div()
                                            .absolute()
                                            .right(px(-6.))
                                            .bottom(px(-6.))
                                            .size(px(28.))
                                            .rounded_full()
                                            .border_2()
                                            .border_color(rgb(0x030508))
                                            .bg(rgb(0x1f2937))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .child(
                                                svg()
                                                    .size(px(16.))
                                                    .path("icons/lock.svg")
                                                    .text_color(rgb(0xffffff)),
                                            ),
                                    ),
                            )
                            .child(
                                div()
                                    .text_xl()
                                    .font_weight(FontWeight(800.))
                                    .child(t!("lockScreen.title")),
                            )
                            .child(
                                div()
                                    .max_w(px(360.))
                                    .text_center()
                                    .text_sm()
                                    .line_height(px(20.))
                                    .text_color(rgb(palette.text_muted))
                                    .child(t!("lockScreen.message")),
                            )
                            .when(self.settings.summary().has_master_password, |this| {
                                this.child(
                                    div()
                                        .w(px(280.))
                                        .flex()
                                        .flex_col()
                                        .gap_2()
                                        .child(
                                            div()
                                                .id(SharedString::from("lock-password-input"))
                                                .relative()
                                                .h(px(42.))
                                                .px_3()
                                                .flex()
                                                .items_center()
                                                .rounded_md()
                                                .border_1()
                                                .border_color(if status_is_error {
                                                    rgb(0x8b2d2d)
                                                } else {
                                                    rgb(palette.border)
                                                })
                                                .bg(rgb(palette.input))
                                                .font_family(
                                                    crate::features::shell::gpui_code_font_family(),
                                                )
                                                .text_sm()
                                                .text_color(rgb(palette.text))
                                                .cursor_text()
                                                .on_click(move |_, window, cx| {
                                                    window.focus(&password_focus, cx);
                                                })
                                                .child(
                                                    div()
                                                        .min_w_0()
                                                        .flex_1()
                                                        .overflow_hidden()
                                                        .child(NyaInput::new(&password_input)),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .text_center()
                                                .text_xs()
                                                .text_color(if status_is_error {
                                                    rgb(palette.danger)
                                                } else {
                                                    rgb(palette.text_muted)
                                                })
                                                .child(lock_status.clone()),
                                        ),
                                )
                            })
                            .when(!self.settings.summary().has_master_password, |this| {
                                this.child(
                                    div()
                                        .text_center()
                                        .text_xs()
                                        .text_color(rgb(palette.text_muted))
                                        .child(lock_status.clone()),
                                )
                            })
                            .child(small_button(
                                palette,
                                "lock-screen-unlock",
                                t!("lockScreen.unlock"),
                                cx.listener(|this, _, _, cx| {
                                    this.submit_lock_unlock(cx);
                                }),
                            )),
                    ),
            )
    }
}
