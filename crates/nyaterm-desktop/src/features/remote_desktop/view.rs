use rust_i18n::t;

use std::borrow::Cow;

use gpui::{
    Bounds, Context, CursorStyle, FontWeight, IntoElement, KeyDownEvent, KeyUpEvent,
    ModifiersChangedEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    NavigationDirection, Pixels, Point, ScrollDelta, ScrollWheelEvent, SharedString, Size, canvas,
    div, prelude::*, px, rgb,
};
use nyaterm_remote_desktop::{
    CertificatePromptReason, CursorShape, DisplayScaleMode, DisplayTransform, LogicalPoint,
    LogicalRect, LogicalSize, RdpCertificateResponse, RdpPointerButton, RemoteDesktopViewState,
    VncScaleMode,
};

use crate::features::NyaTermApp;
use crate::widgets::small_button;

use super::runtime::format_remote_desktop_error;
use super::state::RdpCertificatePrompt;

impl NyaTermApp {
    pub(in crate::features) fn remote_desktop_view(
        &mut self,
        session_id: String,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let palette = self.theme_palette();
        let Some(session) = self.remote_desktop.sessions.get(&session_id) else {
            return self
                .rdp_status_view(
                    "Remote Desktop",
                    "The RDP runtime is no longer available for this tab.",
                )
                .into_any_element();
        };
        if let Some(request) = session.certificate_request.clone() {
            return self
                .rdp_certificate_view(session_id, request, cx)
                .into_any_element();
        }
        if let Some(error) = session.error.clone()
            && matches!(session.state, RemoteDesktopViewState::Failed)
        {
            let retry_id = session_id.clone();
            let close_id = session_id.clone();
            return div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_3()
                .bg(rgb(0x000000))
                .child(
                    div()
                        .text_size(px(14.))
                        .font_weight(FontWeight(600.))
                        .text_color(rgb(palette.danger))
                        .child(t!("remoteDesktop.connectionFailed")),
                )
                .child(
                    div()
                        .max_w(px(520.))
                        .px_6()
                        .text_center()
                        .text_size(px(12.))
                        .text_color(rgb(palette.text_dimmed))
                        .child(format_remote_desktop_error(&error)),
                )
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(small_button(
                            palette,
                            format!("rdp-retry-{session_id}"),
                            t!("remoteDesktop.retry"),
                            cx.listener(move |this, _, _, cx| {
                                this.retry_rdp_runtime(&retry_id, cx);
                                cx.notify();
                            }),
                        ))
                        .child(small_button(
                            palette,
                            format!("rdp-close-{session_id}"),
                            t!("remoteDesktop.close"),
                            cx.listener(move |this, _, _, cx| {
                                this.close_session(close_id.clone(), cx);
                            }),
                        )),
                )
                .into_any_element();
        }
        if matches!(session.state, RemoteDesktopViewState::Disconnected) {
            let retry_id = session_id.clone();
            let close_id = session_id.clone();
            return div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_3()
                .bg(rgb(0x000000))
                .child(
                    div()
                        .text_size(px(14.))
                        .font_weight(FontWeight(600.))
                        .text_color(rgb(palette.text))
                        .child(t!("remoteDesktop.disconnected")),
                )
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(small_button(
                            palette,
                            format!("rdp-disconnected-retry-{session_id}"),
                            t!("remoteDesktop.retry"),
                            cx.listener(move |this, _, _, cx| {
                                this.retry_rdp_runtime(&retry_id, cx);
                                cx.notify();
                            }),
                        ))
                        .child(small_button(
                            palette,
                            format!("rdp-disconnected-close-{session_id}"),
                            t!("remoteDesktop.close"),
                            cx.listener(move |this, _, _, cx| {
                                this.close_session(close_id.clone(), cx);
                            }),
                        )),
                )
                .into_any_element();
        }
        let (Some(framebuffer), Some(texture)) = (session.framebuffer.as_ref(), session.texture)
        else {
            let detail = match session.state {
                RemoteDesktopViewState::Connecting => t!("remoteDesktop.connecting"),
                RemoteDesktopViewState::Disconnecting => {
                    Cow::Borrowed("Disconnecting Remote Desktop session...")
                }
                RemoteDesktopViewState::Disconnected => {
                    Cow::Borrowed("Remote Desktop session is disconnected.")
                }
                _ => t!("remoteDesktop.waitingFrame"),
            };
            return self
                .rdp_status_view("Remote Desktop", detail)
                .into_any_element();
        };
        let remote_size = (framebuffer.width(), framebuffer.height());
        let scale_mode = self
            .session
            .metadata(&session_id)
            .and_then(|metadata| match &metadata.launch_config {
                crate::models::SessionLaunchConfig::Vnc(config) => Some(config.display.scale_mode),
                _ => None,
            })
            .unwrap_or(VncScaleMode::Fit);
        let cursor_shape = session.cursor_shape.clone();
        let cursor_position = session.cursor_position;
        let cursor_visible = session.cursor_visible;
        let cursor_texture = session.cursor_texture;
        let app = cx.entity();
        let surface_focus = self.remote_desktop.focus().clone();
        let input = self
            .remote_desktop
            .inputs
            .entry(session_id.clone())
            .or_insert_with(|| {
                cx.new(|_| {
                    super::input::RemoteDesktopInput::new(
                        app.downgrade(),
                        session_id.clone(),
                        surface_focus.clone(),
                    )
                })
            })
            .clone();
        let input_down = input.clone();
        let input_up = input.clone();

        let hide_native_cursor = remote_cursor_hides_native(cursor_visible, cursor_shape.as_ref());
        let remote_cursor_texture_visible = cursor_visible
            && cursor_texture.is_some()
            && cursor_shape.as_ref().is_some_and(|shape| {
                shape.width > 0 && shape.height > 0 && !shape.pixels.is_empty()
            });
        let viewport_session_id = session_id.clone();
        let canvas =
            canvas(
                move |bounds, window, cx| {
                    let app = app.clone();
                    let session_id = viewport_session_id.clone();
                    let scale_factor = window.scale_factor();
                    window.defer(cx, move |_, cx| {
                        app.update(cx, |this, _| {
                            this.update_rdp_viewport(&session_id, bounds, scale_factor);
                        });
                    });
                    remote_desktop_image_bounds(bounds, remote_size.0, remote_size.1, scale_mode)
                },
                move |_bounds, image_bounds, window, _cx| {
                    let _ = window.paint_dynamic_texture(image_bounds, texture);
                    if let (Some(cursor), Some(cursor_texture)) = (&cursor_shape, cursor_texture)
                        && cursor_visible
                        && remote_size.0 > 0
                        && remote_size.1 > 0
                    {
                        let scale_x = f32::from(image_bounds.size.width) / remote_size.0 as f32;
                        let scale_y = f32::from(image_bounds.size.height) / remote_size.1 as f32;
                        let cursor_bounds = Bounds::new(
                            Point::new(
                                image_bounds.origin.x
                                    + px((cursor_position.x as f32 - cursor.hotspot.x as f32)
                                        * scale_x),
                                image_bounds.origin.y
                                    + px((cursor_position.y as f32 - cursor.hotspot.y as f32)
                                        * scale_y),
                            ),
                            Size::new(
                                px(cursor.width as f32 * scale_x),
                                px(cursor.height as f32 * scale_y),
                            ),
                        );
                        let _ = window.paint_dynamic_texture(cursor_bounds, cursor_texture);
                    }
                },
            )
            .size_full();

        let move_id = session_id.clone();
        let left_down_id = session_id.clone();
        let right_down_id = session_id.clone();
        let middle_down_id = session_id.clone();
        let left_up_id = session_id.clone();
        let right_up_id = session_id.clone();
        let middle_up_id = session_id.clone();
        let left_up_out_id = session_id.clone();
        let right_up_out_id = session_id.clone();
        let middle_up_out_id = session_id.clone();
        let back_down_id = session_id.clone();
        let forward_down_id = session_id.clone();
        let back_up_id = session_id.clone();
        let forward_up_id = session_id.clone();
        let scroll_id = session_id.clone();
        let key_down_id = session_id.clone();
        let key_up_id = session_id.clone();
        let modifiers_id = session_id.clone();
        let focus = surface_focus;
        div()
            .id(format!("rdp-surface-{session_id}"))
            .size_full()
            .relative()
            .overflow_hidden()
            .bg(rgb(0x000000))
            .cursor(if hide_native_cursor {
                CursorStyle::Hidden
            } else {
                CursorStyle::Arrow
            })
            .track_focus(&focus)
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                if input_down.update(cx, |input, _| input.consumes_key(event)) {
                    return;
                }

                if this.send_rdp_key_down(
                    &key_down_id,
                    &event.keystroke.key,
                    event.keystroke.key_char.as_deref(),
                    event.is_held,
                    event.keystroke.modifiers,
                ) {
                    cx.stop_propagation();
                    this.mark_user_activity();
                }
            }))
            .on_modifiers_changed(
                cx.listener(move |this, event: &ModifiersChangedEvent, _, cx| {
                    if this.send_remote_modifier_state(
                        &modifiers_id,
                        event.modifiers,
                        Some(event.capslock.on),
                    ) {
                        cx.stop_propagation();
                        this.mark_user_activity();
                    }
                }),
            )
            .on_key_up(cx.listener(move |this, event: &KeyUpEvent, _, cx| {
                if input_up.update(cx, |input, _| input.consumes_key_up(&event.keystroke.key)) {
                    return;
                }

                if this.send_rdp_key_up(&key_up_id, &event.keystroke.key) {
                    cx.stop_propagation();
                    this.mark_user_activity();
                }
            }))
            .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
                if this.send_rdp_pointer(&move_id, event.position, None, false) {
                    cx.stop_propagation();
                    this.mark_user_activity();
                    if remote_cursor_texture_visible {
                        cx.notify();
                    }
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    window.focus(this.remote_desktop.focus(), cx);
                    this.activate_workspace_pane(left_down_id.clone(), cx);
                    if this.send_rdp_pointer(
                        &left_down_id,
                        event.position,
                        Some(RdpPointerButton::Left),
                        true,
                    ) {
                        this.mark_user_activity();
                    }
                    cx.stop_propagation();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    window.focus(this.remote_desktop.focus(), cx);
                    this.activate_workspace_pane(right_down_id.clone(), cx);
                    if this.send_rdp_pointer(
                        &right_down_id,
                        event.position,
                        Some(RdpPointerButton::Right),
                        true,
                    ) {
                        this.mark_user_activity();
                    }
                    cx.stop_propagation();
                }),
            )
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    window.focus(this.remote_desktop.focus(), cx);
                    this.activate_workspace_pane(middle_down_id.clone(), cx);
                    if this.send_rdp_pointer(
                        &middle_down_id,
                        event.position,
                        Some(RdpPointerButton::Middle),
                        true,
                    ) {
                        this.mark_user_activity();
                    }
                    cx.stop_propagation();
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseUpEvent, _, cx| {
                    if this.send_rdp_pointer(
                        &left_up_id,
                        event.position,
                        Some(RdpPointerButton::Left),
                        false,
                    ) {
                        this.mark_user_activity();
                    }
                    cx.stop_propagation();
                }),
            )
            .on_mouse_up(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseUpEvent, _, cx| {
                    if this.send_rdp_pointer(
                        &right_up_id,
                        event.position,
                        Some(RdpPointerButton::Right),
                        false,
                    ) {
                        this.mark_user_activity();
                    }
                    cx.stop_propagation();
                }),
            )
            .on_mouse_up(
                MouseButton::Middle,
                cx.listener(move |this, event: &MouseUpEvent, _, cx| {
                    if this.send_rdp_pointer(
                        &middle_up_id,
                        event.position,
                        Some(RdpPointerButton::Middle),
                        false,
                    ) {
                        this.mark_user_activity();
                    }
                    cx.stop_propagation();
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseUpEvent, _, _cx| {
                    if this.send_rdp_pointer(
                        &left_up_out_id,
                        event.position,
                        Some(RdpPointerButton::Left),
                        false,
                    ) {
                        this.mark_user_activity();
                    }
                }),
            )
            .on_mouse_up_out(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseUpEvent, _, _cx| {
                    if this.send_rdp_pointer(
                        &right_up_out_id,
                        event.position,
                        Some(RdpPointerButton::Right),
                        false,
                    ) {
                        this.mark_user_activity();
                    }
                }),
            )
            .on_mouse_up_out(
                MouseButton::Middle,
                cx.listener(move |this, event: &MouseUpEvent, _, _cx| {
                    if this.send_rdp_pointer(
                        &middle_up_out_id,
                        event.position,
                        Some(RdpPointerButton::Middle),
                        false,
                    ) {
                        this.mark_user_activity();
                    }
                }),
            )
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Back),
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    if this.send_rdp_pointer(
                        &back_down_id,
                        event.position,
                        Some(RdpPointerButton::X1),
                        true,
                    ) {
                        cx.stop_propagation();
                        this.mark_user_activity();
                    }
                }),
            )
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Forward),
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    if this.send_rdp_pointer(
                        &forward_down_id,
                        event.position,
                        Some(RdpPointerButton::X2),
                        true,
                    ) {
                        cx.stop_propagation();
                        this.mark_user_activity();
                    }
                }),
            )
            .on_mouse_up(
                MouseButton::Navigate(NavigationDirection::Back),
                cx.listener(move |this, event: &MouseUpEvent, _, cx| {
                    if this.send_rdp_pointer(
                        &back_up_id,
                        event.position,
                        Some(RdpPointerButton::X1),
                        false,
                    ) {
                        cx.stop_propagation();
                        this.mark_user_activity();
                    }
                }),
            )
            .on_mouse_up(
                MouseButton::Navigate(NavigationDirection::Forward),
                cx.listener(move |this, event: &MouseUpEvent, _, cx| {
                    if this.send_rdp_pointer(
                        &forward_up_id,
                        event.position,
                        Some(RdpPointerButton::X2),
                        false,
                    ) {
                        cx.stop_propagation();
                        this.mark_user_activity();
                    }
                }),
            )
            .on_scroll_wheel(
                cx.listener(move |this, event: &ScrollWheelEvent, window, cx| {
                    let line_height = f32::from(window.line_height()).max(1.0);
                    let (delta_x, delta_y) = match event.delta {
                        ScrollDelta::Pixels(delta) => (
                            f32::from(delta.x) / line_height,
                            f32::from(delta.y) / line_height,
                        ),
                        ScrollDelta::Lines(delta) => (delta.x, delta.y),
                    };
                    if (delta_x != 0.0 || delta_y != 0.0)
                        && this.send_remote_wheel(&scroll_id, event.position, delta_x, delta_y)
                    {
                        cx.stop_propagation();
                        this.mark_user_activity();
                    }
                }),
            )
            .child(canvas)
            .child(input)
            .into_any_element()
    }

    fn rdp_certificate_view(
        &mut self,
        session_id: String,
        prompt: RdpCertificatePrompt,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let reject_id = session_id.clone();
        let once_id = session_id.clone();
        let remember_id = session_id.clone();
        let changed = matches!(prompt.reason, CertificatePromptReason::Changed { .. });
        let title = if changed {
            t!("remoteDesktop.certificateChangedTitle")
        } else {
            t!("remoteDesktop.certificateTitle")
        };
        let remember_label = if changed {
            t!("remoteDesktop.replaceCertificate")
        } else {
            t!("remoteDesktop.trustAndRemember")
        };
        let risk_detail = match &prompt.reason {
            CertificatePromptReason::FirstUse => String::new(),
            CertificatePromptReason::Changed {
                previous_fingerprint,
                presented_fingerprint,
            } => format!(
                "{}\n{}: {}\n{}: {}",
                t!("remoteDesktop.certificateChangedWarning"),
                t!("remoteDesktop.previousFingerprint"),
                previous_fingerprint,
                t!("remoteDesktop.presentedFingerprint"),
                presented_fingerprint,
            ),
        };
        let request = prompt.request;
        let certificate_detail = format!(
            "{}:{}\nSHA-256 {}\nSubject: {}\nIssuer: {}\nValid: {} to {}",
            request.host,
            request.port,
            request.sha256_fingerprint,
            request.subject.as_deref().unwrap_or("unknown"),
            request.issuer.as_deref().unwrap_or("unknown"),
            request.valid_from.as_deref().unwrap_or("unknown"),
            request.valid_to.as_deref().unwrap_or("unknown")
        );
        let detail = if risk_detail.is_empty() {
            certificate_detail
        } else {
            format!("{risk_detail}\n\n{certificate_detail}")
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_3()
            .px_6()
            .bg(rgb(0x000000))
            .child(
                div()
                    .text_size(px(15.))
                    .font_weight(FontWeight(600.))
                    .text_color(rgb(palette.warning))
                    .child(title),
            )
            .child(
                div()
                    .max_w(px(620.))
                    .text_size(px(12.))
                    .text_color(rgb(palette.text_dimmed))
                    .child(detail),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(small_button(
                        palette,
                        format!("rdp-cert-reject-{session_id}"),
                        t!("remoteDesktop.reject"),
                        cx.listener(move |this, _, _, cx| {
                            this.resolve_rdp_certificate(
                                &reject_id,
                                RdpCertificateResponse::Reject,
                                cx,
                            );
                            cx.notify();
                        }),
                    ))
                    .when(!changed, |this| {
                        this.child(small_button(
                            palette,
                            format!("rdp-cert-once-{session_id}"),
                            t!("remoteDesktop.trustOnce"),
                            cx.listener(move |this, _, _, cx| {
                                this.resolve_rdp_certificate(
                                    &once_id,
                                    RdpCertificateResponse::TrustOnce,
                                    cx,
                                );
                                cx.notify();
                            }),
                        ))
                    })
                    .child(small_button(
                        palette,
                        format!("rdp-cert-remember-{session_id}"),
                        remember_label,
                        cx.listener(move |this, _, _, cx| {
                            this.resolve_rdp_certificate(
                                &remember_id,
                                RdpCertificateResponse::TrustAndRemember,
                                cx,
                            );
                            cx.notify();
                        }),
                    )),
            )
    }

    fn rdp_status_view(
        &self,
        title: &'static str,
        detail: impl Into<SharedString>,
    ) -> impl IntoElement {
        let detail: SharedString = detail.into();
        let palette = self.theme_palette();
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_2()
            .bg(rgb(0x000000))
            .child(
                div()
                    .text_size(px(14.))
                    .font_weight(FontWeight(600.))
                    .text_color(rgb(palette.text))
                    .child(title),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(palette.text_dimmed))
                    .child(detail),
            )
    }
}

fn remote_desktop_image_bounds(
    viewport: Bounds<Pixels>,
    remote_width: u32,
    remote_height: u32,
    scale_mode: VncScaleMode,
) -> Bounds<Pixels> {
    let mode = match scale_mode {
        VncScaleMode::Fit => DisplayScaleMode::Fit,
        VncScaleMode::Stretch => DisplayScaleMode::Stretch,
        VncScaleMode::Actual => DisplayScaleMode::Actual,
    };
    let transform = DisplayTransform::new(
        LogicalRect {
            origin: LogicalPoint {
                x: f32::from(viewport.origin.x),
                y: f32::from(viewport.origin.y),
            },
            size: LogicalSize {
                width: f32::from(viewport.size.width),
                height: f32::from(viewport.size.height),
            },
        },
        remote_width,
        remote_height,
        mode,
    );
    transform.map_or(viewport, |transform| {
        let image = transform.image_bounds();
        Bounds::new(
            Point::new(px(image.origin.x), px(image.origin.y)),
            Size::new(px(image.size.width), px(image.size.height)),
        )
    })
}

fn remote_cursor_hides_native(visible: bool, shape: Option<&CursorShape>) -> bool {
    !visible
        || shape
            .is_some_and(|shape| shape.width > 0 && shape.height > 0 && !shape.pixels.is_empty())
}

#[cfg(test)]
mod tests {
    use nyaterm_remote_desktop::{CursorShape, RemotePoint};

    use super::remote_cursor_hides_native;

    fn shape(width: u32, height: u32) -> CursorShape {
        CursorShape {
            shape_id: 1,
            width,
            height,
            hotspot: RemotePoint { x: 0, y: 0 },
            pixels: vec![0; width.saturating_mul(height).saturating_mul(4) as usize],
        }
    }

    #[test]
    fn native_cursor_tracks_remote_default_bitmap_and_hidden_states() {
        assert!(!remote_cursor_hides_native(true, None));
        assert!(!remote_cursor_hides_native(true, Some(&shape(0, 0))));
        assert!(remote_cursor_hides_native(true, Some(&shape(2, 2))));
        assert!(remote_cursor_hides_native(false, None));
        assert!(remote_cursor_hides_native(false, Some(&shape(2, 2))));
    }
}
