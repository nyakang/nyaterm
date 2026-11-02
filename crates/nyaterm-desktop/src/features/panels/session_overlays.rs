use rust_i18n::t;

use gpui::{
    AnyElement, Context, FontWeight, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent,
    ParentElement as _, SharedString, Styled as _, div, prelude::*, px, rgb, rgba,
};
use nyaterm_core::truncate_preview;
use nyaterm_ui::{NyaNumberInputOptions, NyaScrollable as _, NyaSelectableText};

use crate::features::{NyaTermApp, text_inputs::TextInputSetup};
use crate::widgets::small_button;

use super::TAB_PRESET_COLORS;

const SESSION_INFO_DIALOG_WIDTH: f32 = 900.;
const SESSION_INFO_VIEWPORT_MARGIN: f32 = 8.;

fn session_info_dialog_width(viewport_width: f32) -> f32 {
    // Keep room to drag the dialog; a 900px cap only applies on wide windows.
    (viewport_width * 0.8).clamp(0., SESSION_INFO_DIALOG_WIDTH)
}

/// Constrain the measured dialog; before prepaint use its maximum height.
fn session_info_drag_limits(viewport: (f32, f32), dialog_height: f32) -> (f32, f32) {
    let (viewport_width, viewport_height) = viewport;
    let fallback_height = (viewport_height - 32.).max(0.);
    let dialog_height = if dialog_height > 0. {
        dialog_height
    } else {
        fallback_height
    };
    (
        ((viewport_width - session_info_dialog_width(viewport_width)) / 2.
            - SESSION_INFO_VIEWPORT_MARGIN)
            .max(0.),
        ((viewport_height - dialog_height) / 2. - SESSION_INFO_VIEWPORT_MARGIN).max(0.),
    )
}

impl NyaTermApp {
    pub(in crate::features) fn constrain_session_info_to_viewport(&mut self) {
        let limits = session_info_drag_limits(
            self.shell.viewport_size(),
            self.session.dialog_session_info_dialog_height(),
        );
        self.session.dialog_constrain_session_info(limits);
    }

    pub(in crate::features) fn rename_session_dialog_content(
        &mut self,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let rename_draft = self.session.dialog_rename_draft().to_string();
        self.text_input_box(
            "session.rename",
            &rename_draft,
            TextInputSetup::placeholder(t!("tabCtx.renamePlaceholder")),
            cx,
        )
        .into_any_element()
    }

    pub(in crate::features) fn tab_color_picker_overlay(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let active_color = self
            .session
            .active_id()
            .and_then(|session_id| self.session.tab_color(session_id));
        let mut swatches = div().mt_3().grid().grid_cols(6).gap_2();
        for (name, color) in TAB_PRESET_COLORS {
            let selected = active_color == Some(color);
            swatches = swatches.child(
                div()
                    .id(SharedString::from(format!("tab-color-{name}")))
                    .size(px(28.))
                    .rounded_full()
                    .border_2()
                    .border_color(if selected {
                        rgb(0xffffff)
                    } else {
                        rgb(0x1f2937)
                    })
                    .bg(rgb(color))
                    .cursor_pointer()
                    .hover(|this| this.border_color(rgb(palette.text)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.set_active_session_tab_color(Some(color), cx);
                    })),
            );
        }

        div()
            .id(SharedString::from("tab-color-overlay"))
            .absolute()
            .top_0()
            .bottom_0()
            .left_0()
            .right_0()
            .bg(rgba(0x00000080))
            .flex()
            .items_center()
            .justify_center()
            .track_focus(self.session.dialog_color_picker_focus())
            .on_click(cx.listener(|this, _, window, cx| {
                window.focus(this.session.dialog_color_picker_focus(), cx);
                cx.notify();
            }))
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                cx.stop_propagation();
                if event.keystroke.key == "escape" {
                    this.close_tab_color_picker(cx);
                }
            }))
            .child(
                div()
                    .id(SharedString::from("tab-color-dialog"))
                    .w(px(300.))
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(palette.border))
                    .bg(self.shell_surface_color(palette.bg))
                    .shadow_lg()
                    .p_4()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight(800.))
                            .text_color(rgb(palette.text))
                            .child(t!("tabCtx.setColor")),
                    )
                    .child(swatches)
                    .child(
                        div()
                            .mt_3()
                            .text_xs()
                            .text_color(rgb(palette.text_muted))
                            .child(t!("tabCtx.colorHint")),
                    )
                    .child(
                        div()
                            .mt_4()
                            .flex()
                            .items_center()
                            .justify_end()
                            .gap_2()
                            .child(small_button(
                                palette,
                                "tab-color-reset",
                                t!("tabCtx.resetColor"),
                                cx.listener(|this, _, _, cx| {
                                    this.set_active_session_tab_color(None, cx);
                                }),
                            ))
                            .child(small_button(
                                palette,
                                "tab-color-cancel",
                                t!("common.cancel"),
                                cx.listener(|this, _, _, cx| {
                                    this.close_tab_color_picker(cx);
                                }),
                            )),
                    ),
            )
    }

    pub(in crate::features) fn session_info_overlay(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let details = self.active_session_info_details().unwrap_or_default();
        let title = details
            .iter()
            .find(|(label, _)| *label == t!("sessionInfo.name"))
            .map(|(_, value)| value.clone())
            .unwrap_or_else(|| t!("tabCtx.sessionInfo").to_string());
        let (viewport_width, viewport_height) = self.shell.viewport_size();
        let dialog_width = session_info_dialog_width(viewport_width);
        // A wide report uses two columns; otherwise the body scrolls.
        let columns = if dialog_width >= 760. { 2 } else { 1 };
        let mut rows = div()
            .id(SharedString::from("session-info-rows"))
            .debug_selector(|| "session-info-rows".to_string())
            .mt_3()
            .w_full()
            .grid()
            .grid_cols(columns)
            .gap_x_4()
            .gap_y_1();
        if details.is_empty() {
            rows = rows.child(
                div()
                    .col_span(columns)
                    .rounded_sm()
                    .border_1()
                    .border_color(rgb(palette.border))
                    .bg(rgb(palette.input))
                    .p_3()
                    .text_sm()
                    .text_color(rgb(palette.text_muted))
                    .child(t!("tabCtx.noSessionDetails")),
            );
        } else {
            for (index, (label, value)) in details.iter().enumerate() {
                if value.is_empty() {
                    // Empty value marks a section header (RDP connection report), so
                    // the dialog reads like the Windows "connection quality" sheet.
                    rows = rows.child(
                        div()
                            .col_span(columns)
                            .mt_2()
                            .text_sm()
                            .font_weight(FontWeight(700.))
                            .text_color(rgb(palette.text))
                            .child(NyaSelectableText::new(
                                SharedString::from(format!("session-info-section-{index}")),
                                label.to_string(),
                            )),
                    );
                } else {
                    // Selectable text rather than painted rows: the values are what
                    // users copy, and window selection can span all of them at once.
                    rows = rows.child(
                        div()
                            .flex()
                            .gap_2()
                            .text_xs()
                            .debug_selector(move || format!("session-info-row-{index}"))
                            .child(div().flex_none().text_color(rgb(palette.text_muted)).child(
                                NyaSelectableText::new(
                                    SharedString::from(format!("session-info-label-{index}")),
                                    label.to_string(),
                                ),
                            ))
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .text_color(rgb(palette.text))
                                    .child(NyaSelectableText::new(
                                        SharedString::from(format!("session-info-value-{index}")),
                                        value.clone(),
                                    )),
                            ),
                    );
                }
            }
        }

        self.constrain_session_info_to_viewport();
        let offset = self.session.dialog_session_info_offset();
        let dialog_max_height = (viewport_height - 32.).max(0.);
        // Header, footer, the rows' top margin and the dialog's padding/border
        // stay outside the scrolling body. Reserve their fixed height so the
        // dialog can keep a content-driven height and still stay in the viewport.
        let body_max_height = (dialog_max_height - 160.).max(0.);
        let scroll = self.session.dialog_session_info_scroll();

        div()
            .on_children_prepainted(cx.processor(
                |this, bounds: Vec<gpui::Bounds<gpui::Pixels>>, _, cx| {
                    let Some(dialog) = bounds.first() else {
                        return;
                    };
                    this.session
                        .dialog_set_session_info_dialog_height(f32::from(dialog.size.height));
                    let previous = this.session.dialog_session_info_offset();
                    this.constrain_session_info_to_viewport();
                    if this.session.dialog_session_info_offset() != previous {
                        // A narrower window can wrap the header and change its
                        // measured height. Repaint only when that moves the dialog.
                        cx.notify();
                    }
                },
            ))
            .id(SharedString::from("session-info-overlay"))
            .absolute()
            .top_0()
            .bottom_0()
            .left_0()
            .right_0()
            .bg(rgba(0x00000080))
            .flex()
            .items_center()
            .justify_center()
            .track_focus(self.session.dialog_session_info_focus())
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                cx.stop_propagation();
                if event.keystroke.key == "escape" {
                    this.close_active_session_info(cx);
                }
            }))
            .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
                if !this.session.dialog_session_info_drag_active() {
                    return;
                }
                if !event.dragging() {
                    this.session.dialog_end_session_info_drag();
                    cx.notify();
                    return;
                }
                let pointer = (f32::from(event.position.x), f32::from(event.position.y));
                let limits = session_info_drag_limits(
                    this.shell.viewport_size(),
                    this.session.dialog_session_info_dialog_height(),
                );
                if this.session.dialog_drag_session_info(pointer, limits) {
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.session.dialog_session_info_drag_active() {
                        this.session.dialog_end_session_info_drag();
                        cx.notify();
                    }
                }),
            )
            .child(
                div()
                    .id(SharedString::from("session-info-dialog"))
                    .debug_selector(|| "session-info-dialog".to_string())
                    .w(px(dialog_width))
                    .flex_none()
                    .max_h(px(dialog_max_height))
                    .relative()
                    .left(px(offset.0))
                    .top(px(offset.1))
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(palette.border))
                    .bg(self.shell_surface_color(palette.bg))
                    .shadow_lg()
                    .overflow_hidden()
                    .p_4()
                    .child(
                        div()
                            .id(SharedString::from("session-info-header"))
                            .debug_selector(|| "session-info-header".to_string())
                            .flex()
                            .items_start()
                            .gap_3()
                            .cursor(gpui::CursorStyle::OpenHand)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                                    this.session.dialog_begin_session_info_drag((
                                        f32::from(event.position.x),
                                        f32::from(event.position.y),
                                    ));
                                    cx.stop_propagation();
                                }),
                            )
                            .child(
                                div()
                                    .size(px(10.))
                                    .mt_1()
                                    .rounded_full()
                                    .bg(rgb(palette.success)),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight(800.))
                                            .text_color(rgb(palette.text))
                                            .child(t!("tabCtx.sessionInfo")),
                                    )
                                    .child(
                                        div()
                                            .mt_1()
                                            .text_xs()
                                            .text_color(rgb(palette.text_muted))
                                            .child(truncate_preview(&title, 56)),
                                    ),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation()
                                    })
                                    .child(small_button(
                                        palette,
                                        "session-info-header-close",
                                        t!("common.close"),
                                        cx.listener(|this, _, _, cx| {
                                            this.close_active_session_info(cx)
                                        }),
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .id(SharedString::from("session-info-body-frame"))
                            .debug_selector(|| "session-info-body-frame".to_string())
                            .relative()
                            .max_h(px(body_max_height))
                            .child(
                                div()
                                    .id(SharedString::from("session-info-body"))
                                    .max_h(px(body_max_height))
                                    .overflow_y_scroll()
                                    .track_scroll(&scroll)
                                    .child(rows),
                            )
                            // A caller-owned handle instead of `overflow_y_scrollbar`:
                            // that wrapper is height:100%, which a content-sized dialog
                            // cannot resolve against without inflating itself.
                            .vertical_scrollbar(&scroll),
                    )
                    .child(
                        div()
                            .id(SharedString::from("session-info-footer"))
                            .debug_selector(|| "session-info-footer".to_string())
                            .mt_4()
                            .flex()
                            .items_center()
                            .justify_end()
                            .gap_2()
                            .child(small_button(
                                palette,
                                "session-info-copy",
                                t!("common.copyToClipboard"),
                                cx.listener(|this, _, _, cx| {
                                    this.copy_active_session_info(cx);
                                }),
                            ))
                            .child(small_button(
                                palette,
                                "session-info-close",
                                t!("common.close"),
                                cx.listener(|this, _, _, cx| {
                                    this.close_active_session_info(cx);
                                }),
                            )),
                    ),
            )
    }

    pub(in crate::features) fn startup_command_dialog_content(
        &mut self,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = self.theme_palette();
        let command_draft = self.session.dialog_startup_command_draft().to_string();
        let command_input = self
            .text_input_box(
                "session.startup-command",
                &command_draft,
                TextInputSetup::placeholder(t!("tabCtx.commandRequired")),
                cx,
            )
            .into_any_element();
        let delay_ms = self.session.dialog_startup_command_delay_ms();

        div()
            .min_w_0()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight(600.))
                    .text_color(rgb(palette.text_muted))
                    .child(t!("tabCtx.commandInput")),
            )
            .child(command_input)
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight(600.))
                    .text_color(rgb(palette.text_muted))
                    .child(t!("tabCtx.commandDelay")),
            )
            .child(
                div()
                    .h(px(36.))
                    .px_3()
                    .rounded_sm()
                    .border_1()
                    .border_color(rgb(palette.border))
                    .bg(rgb(palette.input))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .child(
                        div().w(px(180.)).child(
                            self.number_input_box(
                                "session.number.startup-delay",
                                delay_ms.to_string().as_str(),
                                NyaNumberInputOptions::default()
                                    .range(0.0, 60_000.0)
                                    .step(100.0)
                                    .suffix("ms"),
                                cx,
                            ),
                        ),
                    )
                    .child(div().flex().items_center().gap_2().child(small_button(
                        palette,
                        "startup-delay-zero",
                        "0",
                        cx.listener(|this, _, _, cx| {
                            this.session.dialog_reset_startup_command_delay();
                            this.reset_number_input("session.number.startup-delay", "0", cx);
                            cx.notify();
                        }),
                    ))),
            )
            .when(command_draft.trim().is_empty(), |this| {
                this.child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(palette.danger))
                        .child(t!("tabCtx.commandRequired")),
                )
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use gpui::{
        AppContext as _, Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext, px,
        size,
    };
    use nyaterm_remote_desktop::RdpSessionConfig;

    use crate::features::NyaTermApp;
    use crate::features::test_support::app_with_visible_local_session;
    use crate::models::SessionLaunchConfig;
    use crate::test_support::{TestAppCleanup, TestConfigDir};

    fn rdp_session_app(cx: &mut TestAppContext, root: &Path) -> Entity<NyaTermApp> {
        let app = app_with_visible_local_session(cx, root, "remote");
        cx.update_entity(&app, |app, cx| {
            app.sync_component_theme(cx);
            app.session.metadata_mut("remote").unwrap().launch_config =
                SessionLaunchConfig::Rdp(RdpSessionConfig {
                    relay: None,
                    backend: Default::default(),
                    windows_authentication: None,
                    name: "103".into(),
                    host: "10.168.10.103".into(),
                    port: 3389,
                    username: String::new(),
                    domain: String::new(),
                    password: None,
                    use_nla: true,
                    certificate_policy: Default::default(),
                    administrative_session: false,
                    display: Default::default(),
                    clipboard: Default::default(),
                    reconnect: Default::default(),
                });
        });
        app
    }

    fn host_app(
        cx: &mut TestAppContext,
        app: Entity<NyaTermApp>,
        window_height: f32,
    ) -> &mut VisualTestContext {
        host_app_sized(cx, app, 760., window_height)
    }

    fn host_app_sized(
        cx: &mut TestAppContext,
        app: Entity<NyaTermApp>,
        window_width: f32,
        window_height: f32,
    ) -> &mut VisualTestContext {
        let window = cx.open_window(
            size(px(window_width), px(window_height)),
            move |window, cx| nyaterm_ui::nya_root(app, window, cx),
        );
        let _ = window.root(cx).expect("test window root");
        VisualTestContext::from_window(*std::ops::Deref::deref(&window), cx).into_mut()
    }

    fn draw(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.refresh();
            _ = window.draw(cx);
        });
        cx.run_until_parked();
    }

    fn open_session_info(cx: &mut VisualTestContext, app: &Entity<NyaTermApp>) {
        cx.update(|window, cx| {
            app.update(cx, |app, cx| app.open_active_session_info(window, cx));
        });
        draw(cx);
    }

    #[test]
    fn drag_limits_follow_the_measured_dialog_height() {
        let (horizontal, vertical) = super::session_info_drag_limits((760., 560.), 480.);
        assert!((horizontal - 68.).abs() < 0.01);
        assert!((vertical - 32.).abs() < 0.01);

        let (_, short_dialog) = super::session_info_drag_limits((760., 560.), 160.);
        assert!((short_dialog - 192.).abs() < 0.01);

        // Before the first prepaint there is no measured height; fall back to
        // the dialog's maximum height so the first drag cannot escape.
        let (_, unmeasured) = super::session_info_drag_limits((760., 560.), 0.);
        assert!((unmeasured - 8.).abs() < 0.01);
    }

    #[test]
    fn session_info_dialog_stays_inside_the_viewport_when_dragged_far() {
        let root = TestConfigDir::new("nyaterm-session-info-drag-bounds");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = TestAppCleanup::new(&cx);
        let app = rdp_session_app(&mut cx, root.path());
        let vcx = host_app(&mut cx, app.clone(), 360.);
        draw(vcx);

        open_session_info(vcx, &app);
        let header = vcx.debug_bounds("session-info-header").expect("header");
        let start = header.center();

        vcx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
        // A tall report used to get a limit based on a fixed 160px height, so
        // dragging to the bottom edge pushed the footer outside the window.
        vcx.simulate_mouse_move(
            gpui::point(start.x, px(359.)),
            MouseButton::Left,
            Modifiers::none(),
        );
        draw(vcx);
        let dragged_down = vcx
            .debug_bounds("session-info-dialog")
            .expect("dragged dialog");
        assert!(
            f32::from(dragged_down.top()) >= 0.,
            "dialog escaped the top edge: {dragged_down:?}"
        );
        assert!(
            f32::from(dragged_down.bottom()) <= 360. + 1.,
            "dialog escaped the bottom edge: {dragged_down:?}"
        );

        // And the header must remain reachable after dragging back up.
        vcx.simulate_mouse_move(
            gpui::point(start.x, px(1.)),
            MouseButton::Left,
            Modifiers::none(),
        );
        draw(vcx);
        let dragged_up = vcx
            .debug_bounds("session-info-dialog")
            .expect("dragged dialog");
        assert!(
            f32::from(dragged_up.top()) >= 0.,
            "dialog escaped the top edge: {dragged_up:?}"
        );
        assert!(
            f32::from(dragged_up.bottom()) <= 360. + 1.,
            "dialog escaped the bottom edge: {dragged_up:?}"
        );
        vcx.simulate_mouse_up(
            gpui::point(start.x, px(1.)),
            MouseButton::Left,
            Modifiers::none(),
        );
    }

    #[test]
    fn session_info_dialog_fits_its_content_and_drags_from_the_header() {
        let root = TestConfigDir::new("nyaterm-session-info-layout");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = TestAppCleanup::new(&cx);
        let app = rdp_session_app(&mut cx, root.path());
        let vcx = host_app(&mut cx, app.clone(), 560.);
        draw(vcx);

        open_session_info(vcx, &app);
        let dialog = vcx.debug_bounds("session-info-dialog").expect("dialog");
        let footer = vcx.debug_bounds("session-info-footer").expect("footer");
        let rows = vcx.debug_bounds("session-info-rows").expect("rows");

        assert!(
            f32::from(dialog.size.height) <= 560. - 32. + 1.,
            "dialog outgrew the viewport: {:?}",
            dialog
        );
        let footer_gap = f32::from(dialog.bottom()) - f32::from(footer.bottom());
        assert!(
            footer_gap <= 24.,
            "dialog left {footer_gap}px of empty space below its footer: {dialog:?}"
        );
        assert!(
            rows.size.height > footer.top() - rows.top(),
            "the RDP report should overflow the dialog body: rows {:?}, dialog {:?}",
            rows,
            dialog
        );

        let body = vcx
            .debug_bounds("session-info-body-frame")
            .expect("body frame");
        let scroll = vcx.update(|_, cx| app.read(cx).session.dialog_session_info_scroll());
        assert_eq!(scroll.offset().y, px(0.));
        vcx.simulate_event(gpui::ScrollWheelEvent {
            position: body.center(),
            delta: gpui::ScrollDelta::Lines(gpui::point(0., -3.)),
            modifiers: Modifiers::none(),
            touch_phase: gpui::TouchPhase::Moved,
        });
        draw(vcx);
        assert!(
            scroll.offset().y < px(0.),
            "the report should scroll inside the capped body: {:?}",
            scroll.offset()
        );

        let header = vcx.debug_bounds("session-info-header").expect("header");
        let start = header.center();
        let (dx, dy) = (48., 24.);
        vcx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
        vcx.simulate_mouse_move(
            gpui::point(start.x + px(dx), start.y + px(dy)),
            MouseButton::Left,
            Modifiers::none(),
        );
        draw(vcx);
        let dragged = vcx
            .debug_bounds("session-info-dialog")
            .expect("dragged dialog");
        assert!(
            (f32::from(dragged.origin.x) - f32::from(dialog.origin.x) - dx).abs() < 2.,
            "dialog did not follow the pointer horizontally: {dialog:?} -> {dragged:?}"
        );
        assert!(
            (f32::from(dragged.origin.y) - f32::from(dialog.origin.y) - dy).abs() < 2.,
            "dialog did not follow the pointer vertically: {dialog:?} -> {dragged:?}"
        );
        vcx.simulate_mouse_up(
            gpui::point(start.x + px(dx), start.y + px(dy)),
            MouseButton::Left,
            Modifiers::none(),
        );
        draw(vcx);

        vcx.update(|_, cx| {
            app.update(cx, |app, cx| app.close_active_session_info(cx));
        });
        draw(vcx);
        open_session_info(vcx, &app);
        let reopened = vcx
            .debug_bounds("session-info-dialog")
            .expect("reopened dialog");
        assert!(
            (f32::from(reopened.origin.x) - f32::from(dialog.origin.x)).abs() < 2.
                && (f32::from(reopened.origin.y) - f32::from(dialog.origin.y)).abs() < 2.,
            "reopening should center the dialog again: {dialog:?} vs {reopened:?}"
        );
    }

    #[test]
    fn wide_session_info_uses_two_columns_and_fits_the_report() {
        let root = TestConfigDir::new("nyaterm-session-info-wide");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = TestAppCleanup::new(&cx);
        let app = rdp_session_app(&mut cx, root.path());
        let vcx = host_app_sized(&mut cx, app.clone(), 1400., 800.);
        draw(vcx);

        open_session_info(vcx, &app);
        let dialog = vcx.debug_bounds("session-info-dialog").expect("dialog");
        let rows = vcx.debug_bounds("session-info-rows").expect("rows");
        let body = vcx.debug_bounds("session-info-body-frame").expect("body");
        assert!(
            f32::from(dialog.size.width) >= 880.,
            "a wide window should use the two-column dialog: {dialog:?}"
        );
        assert!(
            f32::from(rows.size.height) <= f32::from(body.size.height) + 1.,
            "two columns should fit the report: rows {:?}, body {:?}",
            rows,
            body
        );
    }

    #[test]
    fn short_session_info_stays_content_sized() {
        let root = TestConfigDir::new("nyaterm-session-info-short");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, root.path(), "terminal");
        cx.update_entity(&app, |app, cx| app.sync_component_theme(cx));
        let vcx = host_app(&mut cx, app.clone(), 560.);
        draw(vcx);

        open_session_info(vcx, &app);
        let dialog = vcx.debug_bounds("session-info-dialog").expect("dialog");
        let footer = vcx.debug_bounds("session-info-footer").expect("footer");
        assert!(
            f32::from(dialog.size.height) < 420.,
            "a short report should not stretch the dialog: {dialog:?}"
        );
        let footer_gap = f32::from(dialog.bottom()) - f32::from(footer.bottom());
        assert!(
            footer_gap <= 24.,
            "dialog left {footer_gap}px of empty space below its footer: {dialog:?}"
        );
    }

    #[test]
    fn session_info_values_can_be_selected_and_copied() {
        let root = TestConfigDir::new("nyaterm-session-info-selection");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = TestAppCleanup::new(&cx);
        let app = rdp_session_app(&mut cx, root.path());
        let vcx = host_app(&mut cx, app.clone(), 700.);
        draw(vcx);

        open_session_info(vcx, &app);
        let value = vcx
            .debug_bounds("session-info-row-4")
            .expect("endpoint row");
        let start = gpui::point(value.left() + px(2.), value.center().y);
        let end = gpui::point(value.right() - px(2.), value.center().y);
        // Establish the hover hitbox before pressing; selection only begins on a
        // TextView the window considers hovered.
        vcx.simulate_mouse_move(start, None, Modifiers::none());
        vcx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
        vcx.simulate_mouse_move(end, MouseButton::Left, Modifiers::none());
        vcx.simulate_mouse_up(end, MouseButton::Left, Modifiers::none());
        draw(vcx);

        let copy_shortcut = if cfg!(target_os = "macos") {
            "cmd-c"
        } else {
            "ctrl-c"
        };
        vcx.simulate_keystrokes(copy_shortcut);
        draw(vcx);
        let copied = vcx
            .update(|_, cx| cx.read_from_clipboard())
            .and_then(|item| item.text());
        assert!(
            copied
                .as_deref()
                .is_some_and(|text| text.contains("10.168.10.103:3389")),
            "selecting the endpoint and pressing {copy_shortcut} should copy it, got {copied:?}"
        );
    }

    #[test]
    fn session_info_fits_a_narrow_viewport_and_reclamps_after_resize() {
        let root = TestConfigDir::new("nyaterm-session-info-narrow-resize");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = rdp_session_app(&mut cx, root.path());
        let vcx = host_app(&mut cx, app.clone(), 560.);
        draw(vcx);
        open_session_info(vcx, &app);
        let header = vcx.debug_bounds("session-info-header").unwrap();
        vcx.simulate_mouse_down(header.center(), MouseButton::Left, Modifiers::none());
        vcx.simulate_mouse_move(
            gpui::point(px(750.), px(550.)),
            MouseButton::Left,
            Modifiers::none(),
        );
        vcx.simulate_mouse_up(
            gpui::point(px(750.), px(550.)),
            MouseButton::Left,
            Modifiers::none(),
        );
        draw(vcx);
        for (width, height) in [(400., 300.), (320., 220.), (760., 560.)] {
            vcx.simulate_resize(size(px(width), px(height)));
            draw(vcx);
            let dialog = vcx.debug_bounds("session-info-dialog").unwrap();
            let header = vcx.debug_bounds("session-info-header").unwrap();
            assert!(f32::from(dialog.left()) >= 0.);
            assert!(f32::from(dialog.right()) <= width + 1., "{dialog:?}");
            assert!(f32::from(dialog.top()) >= 0.);
            assert!(f32::from(dialog.bottom()) <= height + 1., "{dialog:?}");
            assert!(header.top() >= dialog.top() && header.bottom() <= dialog.bottom());
        }
        vcx.update(|_, cx| app.update(cx, |app, cx| app.close_active_session_info(cx)));
        draw(vcx);
        open_session_info(vcx, &app);
        vcx.update(|_, cx| {
            assert_eq!(app.read(cx).session.dialog_session_info_offset(), (0., 0.));
            assert!(!app.read(cx).session.dialog_session_info_drag_active());
            assert_eq!(
                app.read(cx).session.dialog_session_info_scroll().offset().y,
                px(0.)
            );
        });
    }
}
