//! Connection editor credentials presentation.

use crate::features::NyaTermApp;
use crate::features::view_widgets::{full_window_input_layer, modal_dialog_shell};
use crate::models::{
    ConnectionEditorCredentialOverlay, ConnectionEditorSelect, ConnectionEditorState,
};
use gpui::prelude::*;
use gpui::{AnyElement, Context, FontWeight, SharedString, div, px, rgb, rgba};
use nyaterm_core::truncate_preview;
use nyaterm_ui::{NyaCheckbox, NyaScrollArea, NyaScrollable};
use rust_i18n::t;

impl NyaTermApp {
    pub(in crate::features::pages::connections::editor) fn connection_editor_credential_manager(
        &mut self,
        overlay: ConnectionEditorCredentialOverlay,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = self.theme_palette();
        let (title, add_label) = match overlay {
            ConnectionEditorCredentialOverlay::Passwords => {
                (t!("dialog.managePasswords"), t!("passwordManager.add"))
            }
            ConnectionEditorCredentialOverlay::Keys => {
                (t!("dialog.manageKeys"), t!("securityAuth.addKey"))
            }
        };
        let entries = match overlay {
            ConnectionEditorCredentialOverlay::Passwords => self
                .security
                .passwords()
                .iter()
                .map(|entry| (entry.id.clone(), entry.name.clone()))
                .collect::<Vec<_>>(),
            ConnectionEditorCredentialOverlay::Keys => self
                .security
                .ssh_keys()
                .iter()
                .map(|entry| (entry.id.clone(), entry.name.clone()))
                .collect::<Vec<_>>(),
        };
        let mut rows = div().flex().flex_col();
        if entries.is_empty() {
            rows = rows.child(
                div()
                    .p_4()
                    .text_xs()
                    .text_color(rgb(palette.text_muted))
                    .child(match overlay {
                        ConnectionEditorCredentialOverlay::Passwords => {
                            t!("passwordManager.noPasswords")
                        }
                        ConnectionEditorCredentialOverlay::Keys => t!("securityAuth.noKeys"),
                    }),
            );
        } else {
            let count = entries.len();
            for (index, (id, name)) in entries.into_iter().enumerate() {
                let edit_id = id.clone();
                let delete_id = id.clone();
                rows = rows.child(
                    div()
                        .min_h(px(42.))
                        .px_3()
                        .py_2()
                        .when(index + 1 < count, |this| {
                            this.border_b_1().border_color(rgb(palette.border))
                        })
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .truncate()
                                .text_xs()
                                .text_color(rgb(palette.text))
                                .child(truncate_preview(&name, 40)),
                        )
                        .child(
                            nyaterm_ui::NyaIconButton::new(
                                format!("connection-editor-credential-edit-{id}"),
                                "icons/edit.svg",
                            )
                            .tooltip(t!("common.edit"))
                            .on_click(cx.listener(
                                move |this, _, window, cx| match overlay {
                                    ConnectionEditorCredentialOverlay::Passwords => {
                                        this.open_connection_account_editor(
                                            Some(edit_id.clone()),
                                            window,
                                            cx,
                                        );
                                    }
                                    ConnectionEditorCredentialOverlay::Keys => {
                                        this.open_security_key_editor(
                                            Some(edit_id.clone()),
                                            window,
                                            cx,
                                        );
                                    }
                                },
                            )),
                        )
                        .child(
                            nyaterm_ui::NyaIconButton::new(
                                format!("connection-editor-credential-delete-{id}"),
                                "icons/delete.svg",
                            )
                            .tooltip(t!("common.delete"))
                            .on_click(cx.listener(
                                move |this, _, window, cx| match overlay {
                                    ConnectionEditorCredentialOverlay::Passwords => {
                                        this.request_delete_security_password(
                                            delete_id.clone(),
                                            window,
                                            cx,
                                        );
                                    }
                                    ConnectionEditorCredentialOverlay::Keys => {
                                        this.request_delete_security_key(
                                            delete_id.clone(),
                                            window,
                                            cx,
                                        );
                                    }
                                },
                            )),
                        ),
                );
            }
        }

        full_window_input_layer("connection-editor-credential-overlay")
            .bg(rgba(0x00000088))
            .flex()
            .items_center()
            .justify_center()
            .p_4()
            .child(
                div()
                    .w_full()
                    .max_w(px(460.))
                    .max_h(px(500.))
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(palette.border))
                    .bg(rgb(palette.surface_elevated))
                    .shadow_lg()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .h(px(48.))
                            .flex_none()
                            .px_3()
                            .flex()
                            .items_center()
                            .gap_2()
                            .border_b_1()
                            .border_color(rgb(palette.border))
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .text_sm()
                                    .font_weight(FontWeight(600.))
                                    .child(title),
                            )
                            .child(
                                nyaterm_ui::NyaButton::new(
                                    "connection-editor-credential-add",
                                    add_label,
                                )
                                .on_click(cx.listener(
                                    move |this, _, window, cx| match overlay {
                                        ConnectionEditorCredentialOverlay::Passwords => {
                                            this.open_connection_account_editor(None, window, cx);
                                        }
                                        ConnectionEditorCredentialOverlay::Keys => {
                                            this.open_security_key_editor(None, window, cx);
                                        }
                                    },
                                )),
                            )
                            .child(
                                div()
                                    .debug_selector(|| {
                                        "connection-editor-credential-close".to_string()
                                    })
                                    .child(
                                        nyaterm_ui::NyaIconButton::new(
                                            "connection-editor-credential-close",
                                            "icons/close.svg",
                                        )
                                        .tooltip(t!("common.close"))
                                        .on_click(
                                            cx.listener(move |this, _, window, cx| {
                                                this.set_connection_editor_credential_overlay(
                                                    None, cx,
                                                );
                                                let select = match overlay {
                                                ConnectionEditorCredentialOverlay::Passwords => {
                                                    ConnectionEditorSelect::SavedPassword
                                                }
                                                ConnectionEditorCredentialOverlay::Keys => {
                                                    ConnectionEditorSelect::SshKey
                                                }
                                            };
                                                this.focus_connection_editor_select(
                                                    select, window, cx,
                                                );
                                            }),
                                        ),
                                    ),
                            ),
                    )
                    .child(div().flex_1().min_h_0().overflow_y_scrollbar().child(rows)),
            )
            .into_any_element()
    }
}

pub(super) fn connection_editor_agent_identity_picker(
    palette: crate::theme::ThemePalette,
    editor: &ConnectionEditorState,
    cx: &mut Context<NyaTermApp>,
) -> AnyElement {
    let allowlist_fingerprints: &[String] = match &editor.agent_forwarding_config.policy {
        nyaterm_core::SshAgentForwardingPolicy::Allowlist { fingerprints } => fingerprints,
        nyaterm_core::SshAgentForwardingPolicy::All => &[],
    };
    let mut identity_list = div().flex().flex_col().gap_2();
    if let Some(preview) = editor.agent_preview.as_ref() {
        identity_list =
            identity_list.child(div().text_xs().text_color(rgb(palette.text_muted)).child(
                format!(
                    "{} {}{}",
                    preview.identities.len(),
                    t!("dialog.sshAgentPreviewIdentityCount"),
                    if preview.truncated {
                        format!(" · {}", t!("dialog.sshAgentPreviewTruncated"))
                    } else {
                        String::new()
                    }
                ),
            ));
        for (index, identity) in preview.identities.iter().enumerate() {
            let fingerprint = identity.fingerprint.clone();
            let selected = allowlist_fingerprints
                .iter()
                .any(|value| value == &fingerprint);
            let source = match identity.source.as_str() {
                "external_agent" => t!("dialog.sshAgentExternalSource").to_string(),
                "stored_key" => t!("dialog.sshAgentStoredKeysSource").to_string(),
                _ => identity.source.clone(),
            };
            identity_list = identity_list.child(
                div()
                    .id(SharedString::from(format!(
                        "connection-agent-identity-picker-row-{index}"
                    )))
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(palette.border))
                    .bg(rgb(palette.surface))
                    .px_3()
                    .py_2()
                    .flex()
                    .items_start()
                    .gap_2()
                    .child(
                        NyaCheckbox::new(SharedString::from(format!(
                            "connection-agent-identity-picker-checkbox-{index}"
                        )))
                        .checked(selected)
                        .on_click(cx.listener(
                            move |this, _: &bool, _, cx| {
                                this.toggle_connection_editor_agent_allowlist_fingerprint(
                                    &fingerprint,
                                    cx,
                                );
                            },
                        )),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight(600.))
                                    .text_color(rgb(palette.text))
                                    .child(if identity.comment.is_empty() {
                                        "(anonymous)".to_string()
                                    } else {
                                        identity.comment.clone()
                                    }),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .font_family(crate::features::shell::gpui_code_font_family())
                                    .text_size(px(10.))
                                    .text_color(rgb(palette.text_muted))
                                    .child(identity.fingerprint.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(rgb(palette.text_dimmed))
                                    .child(source),
                            ),
                    ),
            );
        }
        for (index, error) in preview.endpoint_errors.iter().enumerate() {
            identity_list = identity_list.child(
                div()
                    .id(SharedString::from(format!(
                        "connection-agent-identity-picker-error-{index}"
                    )))
                    .text_xs()
                    .text_color(rgb(palette.warning))
                    .child(format!(
                        "{} #{}: {} ({})",
                        t!("dialog.sshAgentPreviewError"),
                        error.custom_endpoint_index + 1,
                        error.endpoint_type,
                        match error.code {
                            nyaterm_transport::SshAgentEndpointPreviewErrorCode::ConnectFailed => {
                                t!("dialog.sshAgentEndpointConnectFailed")
                            }
                            nyaterm_transport::SshAgentEndpointPreviewErrorCode::IdentityEnumerationFailed => {
                                t!("dialog.sshAgentEndpointIdentityEnumerationFailed")
                            }
                        }
                    )),
            );
        }
        if preview.identities.is_empty() && preview.endpoint_errors.is_empty() {
            identity_list = identity_list.child(
                div()
                    .py_4()
                    .text_xs()
                    .text_color(rgb(palette.text_muted))
                    .text_center()
                    .child(t!("dialog.sshAgentEndpointListEmpty")),
            );
        }
    } else {
        identity_list = identity_list.child(
            div()
                .py_6()
                .text_xs()
                .text_color(rgb(palette.text_muted))
                .text_center()
                .child(if editor.agent_preview_loading {
                    t!("dialog.sshAgentPreviewLoading")
                } else {
                    t!("dialog.sshAgentPreviewRefresh")
                }),
        );
    }

    let refresh_label = if editor.agent_preview_loading {
        t!("dialog.sshAgentPreviewLoading")
    } else {
        t!("dialog.sshAgentPreviewRefresh")
    };
    let picker_card = div()
        .id("connection-agent-identity-picker-card")
        .w(px(560.))
        .max_w_full()
        .max_h_full()
        .rounded_md()
        .border_1()
        .border_color(rgb(palette.border))
        .bg(rgb(palette.bg))
        .shadow_lg()
        .p_4()
        .flex()
        .flex_col()
        .gap_3()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_3()
                .child(
                    div()
                        .flex_1()
                        .text_lg()
                        .font_weight(FontWeight(700.))
                        .text_color(rgb(palette.text))
                        .text_center()
                        .child(t!("dialog.sshAgentIdentityPickerTitle")),
                )
                .child(
                    div()
                        .id("connection-agent-identity-picker-close")
                        .size(px(28.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_sm()
                        .text_lg()
                        .text_color(rgb(palette.text_muted))
                        .cursor_pointer()
                        .hover(|this| this.bg(rgb(palette.hover)))
                        .child("×")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.set_connection_editor_agent_identity_picker_open(false, cx);
                        })),
                ),
        )
        .child(
            div()
                .text_sm()
                .text_color(rgb(palette.text_muted))
                .text_center()
                .child(t!("dialog.sshAgentIdentityPickerDescription")),
        )
        .child(
            NyaScrollArea::new("connection-agent-identity-picker-list")
                .max_h(px(320.))
                .child(identity_list),
        )
        .child(
            div()
                .flex()
                .gap_2()
                .child(
                    div()
                        .id("connection-agent-identity-picker-refresh")
                        .h(px(36.))
                        .flex_1()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_sm()
                        .border_1()
                        .border_color(rgb(palette.border))
                        .bg(rgb(palette.input))
                        .text_sm()
                        .text_color(if editor.agent_preview_loading {
                            rgb(palette.text_muted)
                        } else {
                            rgb(palette.text)
                        })
                        .child(refresh_label)
                        .when(!editor.agent_preview_loading, |this| {
                            this.cursor_pointer()
                                .hover(|this| this.bg(rgb(palette.hover)))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.refresh_connection_editor_agent_preview(cx);
                                }))
                        }),
                )
                .child(
                    div()
                        .id("connection-agent-identity-picker-done")
                        .h(px(36.))
                        .flex_1()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_sm()
                        .bg(rgb(palette.primary))
                        .text_sm()
                        .text_color(rgb(palette.on_primary))
                        .cursor_pointer()
                        .hover(|this| this.bg(rgb(palette.primary_hover)))
                        .child(t!("dialog.sshAgentIdentityPickerDone"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.set_connection_editor_agent_identity_picker_open(false, cx);
                        })),
                ),
        )
        .into_any_element();

    modal_dialog_shell(
        palette,
        rgb(palette.bg),
        "connection-agent-identity-picker",
        560.,
        picker_card,
    )
    .into_any_element()
}
