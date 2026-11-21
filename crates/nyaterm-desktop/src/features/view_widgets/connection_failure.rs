use gpui::{
    App, Context, FocusHandle, FontWeight, Hsla, IntoElement, RenderOnce, SharedString, Window,
    div, prelude::*, px, rgb, svg,
};
use nyaterm_core::truncate_preview;
use nyaterm_transport::SessionKind;
use nyaterm_ui::{NyaButton, NyaButtonVariant, NyaDialogWindowExt as _, NyaScrollable as _};
use rust_i18n::t;

use crate::features::NyaTermApp;
use crate::features::formatting::session_kind_label;
use crate::theme::ThemePalette;

#[derive(Clone)]
pub(in crate::features) enum FailedConnectionTarget {
    Start(String),
    Reconnect(String),
}

impl NyaTermApp {
    pub(in crate::features) fn connection_failure_state(
        &mut self,
        target: Option<FailedConnectionTarget>,
        error: String,
        cancelled: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let (id, name, kind, source, active) = match &target {
            Some(FailedConnectionTarget::Start(id)) => {
                let pending = self
                    .session
                    .start_active_failed()
                    .filter(|_| self.session.active_failed_request_id() == Some(id.as_str()))
                    .map(|failed| &failed.pending);
                (
                    format!("failed-start-{id}"),
                    pending.map(|pending| {
                        pending
                            .custom_name
                            .as_ref()
                            .unwrap_or(&pending.connection_name)
                            .clone()
                    }),
                    pending.map(|pending| pending.kind),
                    pending.and_then(|pending| pending.source_connection_id.clone()),
                    self.session.active_failed_request_id() == Some(id.as_str()),
                )
            }
            Some(FailedConnectionTarget::Reconnect(id)) => (
                format!("failed-reconnect-{id}"),
                self.session.display_name(id),
                self.session.session_info(id).map(|info| info.kind),
                self.session
                    .metadata(id)
                    .and_then(|metadata| metadata.source_connection_id.clone()),
                self.session.active_id() == Some(id.as_str()),
            ),
            None => (
                "failed-connection".into(),
                self.shell.last_connect_failure_name().map(str::to_string),
                None,
                None,
                false,
            ),
        };
        let subtitle = name.map(|name| match kind {
            Some(kind) => format!("{name} · {}", session_kind_label(kind)),
            None => name,
        });
        let mut secondary = Vec::new();
        if let Some(source) = source.filter(|id| {
            self.connection_state
                .connections()
                .iter()
                .any(|connection| &connection.id == id)
        }) {
            secondary.push(
                compact_button(format!("{id}-edit"), t!("dialog.editConnection"))
                    .variant(NyaButtonVariant::Ghost)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.edit_saved_connection_by_id(&source, window, cx);
                    })),
            );
        }
        if let Some(FailedConnectionTarget::Reconnect(session_id)) = &target
            && kind == Some(SessionKind::Serial)
        {
            let session_id = session_id.clone();
            secondary.push(
                compact_button(format!("{id}-device"), t!("serialPrompt.chooseDevice"))
                    .variant(NyaButtonVariant::Ghost)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.open_serial_reconnect_prompt(session_id.clone(), cx);
                    })),
            );
        }
        let retry = target.map(|target| {
            compact_button(format!("{id}-retry"), t!("tabCtx.reconnect"))
                .variant(NyaButtonVariant::Primary)
                .icon("icons/fe/refresh.svg")
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    if window.has_active_nya_dialog(cx)
                        || this.root_overlay_visibility(cx).blocks_workspace_focus()
                    {
                        return;
                    }
                    match &target {
                        FailedConnectionTarget::Start(id) => {
                            this.retry_failed_session_start(id.clone(), window, cx)
                        }
                        FailedConnectionTarget::Reconnect(id) => {
                            if this.session.start_reconnect_failure(id).is_some() {
                                this.reconnect_session(id.clone(), window, cx);
                            }
                        }
                    }
                }))
        });
        ConnectionFailureView {
            id: id.into(),
            palette: self.theme_palette(),
            background: self
                .shell_terminal_surface_color(self.theme_palette().ui_terminal_bg)
                .into(),
            subtitle,
            error,
            cancelled,
            retry,
            secondary,
            allow_focus: active && !self.root_overlay_visibility(cx).blocks_workspace_focus(),
            surface_focus: vec![
                self.terminal.input_focus().clone(),
                self.remote_desktop.focus().clone(),
            ],
        }
    }
}

fn compact_button(id: impl Into<SharedString>, label: impl Into<SharedString>) -> NyaButton {
    NyaButton::new(id, label)
        .small()
        .compact()
        .height(px(28.))
        .text_size(px(12.))
}

#[derive(IntoElement)]
struct ConnectionFailureView {
    id: SharedString,
    palette: ThemePalette,
    background: Hsla,
    subtitle: Option<String>,
    error: String,
    cancelled: bool,
    retry: Option<NyaButton>,
    secondary: Vec<NyaButton>,
    allow_focus: bool,
    surface_focus: Vec<FocusHandle>,
}

impl RenderOnce for ConnectionFailureView {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let focus = window
            .use_keyed_state(
                SharedString::from(format!("{}-focus", self.id)),
                cx,
                |_, cx| cx.focus_handle(),
            )
            .read(cx)
            .clone();
        // Only hand off workspace focus. A background failure must not interrupt an
        // input, dialog, or another pane; redraws preserve the user's chosen control.
        if self.retry.is_some()
            && self.allow_focus
            && !window.has_active_nya_dialog(cx)
            && (window.focused(cx).is_none()
                || self
                    .surface_focus
                    .iter()
                    .any(|handle| handle.is_focused(window)))
        {
            window.focus(&focus, cx);
        }
        let show_error = !self.error.is_empty()
            && (!self.cancelled || self.error != t!("terminal.connectionCancelled").as_ref());
        let palette = self.palette;
        div()
            .id(self.id.clone())
            .debug_selector(|| "failed-workspace-state".into())
            .flex_1()
            .size_full()
            .min_h_0()
            .min_w_0()
            .bg(self.background)
            .child(
                div().size_full().overflow_y_scrollbar().child(
                    div()
                        .min_h_full()
                        .w_full()
                        .p_6()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap_3()
                        .child(
                            svg()
                                .size(px(24.))
                                .path("icons/session/disconnect.svg")
                                .text_color(rgb(if self.cancelled {
                                    palette.text_muted
                                } else {
                                    palette.danger
                                })),
                        )
                        .child(
                            div()
                                .debug_selector(|| "failed-workspace-message".into())
                                .flex()
                                .flex_col()
                                .items_center()
                                .gap_1()
                                .max_w_full()
                                .child(
                                    div()
                                        .text_size(px(15.))
                                        .font_weight(FontWeight(500.))
                                        .text_color(rgb(palette.text))
                                        .child(t!(if self.cancelled {
                                            "terminal.connectionCancelled"
                                        } else {
                                            "terminal.connectionFailed"
                                        })),
                                )
                                .when_some(self.subtitle, |this, subtitle| {
                                    this.child(
                                        div()
                                            .max_w_full()
                                            .text_size(px(12.))
                                            .text_color(rgb(palette.text_dimmed))
                                            .text_center()
                                            .child(subtitle),
                                    )
                                }),
                        )
                        .when(show_error, |this| {
                            this.child(
                                div()
                                    .max_w(px(360.))
                                    .text_center()
                                    .text_size(px(12.))
                                    .text_color(rgb(palette.text_dimmed))
                                    .child(truncate_preview(&self.error, 160)),
                            )
                        })
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .justify_center()
                                .gap_2()
                                .when_some(self.retry, |this, retry| {
                                    this.child(
                                        div()
                                            .debug_selector(|| {
                                                "failed-session-reconnect-target".into()
                                            })
                                            .capture_key_down(|event, window, cx| {
                                                if event.is_held
                                                    && matches!(
                                                        event.keystroke.key.as_str(),
                                                        "enter" | "space"
                                                    )
                                                {
                                                    window.prevent_default();
                                                    cx.stop_propagation();
                                                }
                                            })
                                            .child(retry.track_focus(&focus)),
                                    )
                                })
                                .children(self.secondary),
                        ),
                ),
            )
    }
}
