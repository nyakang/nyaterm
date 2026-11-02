//! Controlled, reusable selects keyed by stable feature ids.
//!
//! Persisted feature state remains authoritative for the selected value. The
//! component entity owns focus and popup state, and emits only committed value
//! changes back to the application.

use std::collections::HashMap;

use gpui::{
    App, AppContext, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, Styled as _, Subscription, Window, div, px,
};
use nyaterm_core::{
    AiAgentKind, AiPermissionMode, CodexThreadMode, ExistingFileBehavior, ExternalMcpSessionScope,
    McpTerminalDisplayMode, RecordingMode, RecordingRotationPolicy, RiskLevel,
};
use nyaterm_transport::SftpDuplicatePolicy;
use nyaterm_ui::{
    NYA_FORM_CONTROL_HEIGHT_PX, NyaSelect, NyaSelectEvent, NyaSelectOption, NyaSelectState,
};

use super::NyaTermApp;
use super::ai::AiFullAccessSetting;
use crate::features::shell::TabMouseActionTarget;
use crate::models::{ConnectionEditorSelect, HeaderStatusMode};
use crate::send_command::{
    SendCommandDataType, SendCommandLineEnding, SendCommandMode, SendCommandTarget,
};
use crate::temporary_ssh_link::TemporaryLinkProtocol;

pub(in crate::features) const FOLLOW_UI_THEME_VALUE: &str = "__nya_follow_ui_theme__";
pub(in crate::features) const NO_SELECTION_VALUE: &str = "__nya_no_selection__";
pub(in crate::features) const PENDING_CONNECTION_GROUP_VALUE: &str =
    "__nya_pending_connection_group__";

fn ai_permission_mode(value: &str) -> Option<AiPermissionMode> {
    match value {
        "observer" => Some(AiPermissionMode::Observer),
        "confirm" => Some(AiPermissionMode::Confirm),
        "auto" => Some(AiPermissionMode::Auto),
        "full_access" => Some(AiPermissionMode::FullAccess),
        _ => None,
    }
}

#[derive(Default)]
pub(in crate::features) struct SelectRegistry {
    fields: HashMap<SharedString, Entity<NyaSelectState>>,
    subscriptions: HashMap<SharedString, Subscription>,
}

impl SelectRegistry {
    pub(in crate::features) fn field(&self, id: &SharedString) -> Option<Entity<NyaSelectState>> {
        self.fields.get(id).cloned()
    }

    pub(in crate::features) fn insert_field(
        &mut self,
        id: SharedString,
        field: Entity<NyaSelectState>,
    ) {
        self.fields.insert(id, field);
    }
}

impl NyaTermApp {
    pub(in crate::features) fn dismiss_connection_editor_menus(&mut self, cx: &mut Context<Self>) {
        let fields = self
            .selects
            .fields
            .iter()
            .filter(|(id, _)| {
                id.starts_with("connection-editor-") && id.as_ref() != "connection-editor-kind"
            })
            .map(|(_, field)| field.clone())
            .collect::<Vec<_>>();
        for field in fields {
            field.update(cx, |field, cx| field.dismiss_menu(cx));
        }
    }

    pub(in crate::features) fn select_entity<I>(
        &mut self,
        id: I,
        options: Vec<NyaSelectOption>,
        selected_value: Option<String>,
        disabled: bool,
        cx: &mut Context<Self>,
    ) -> Entity<NyaSelectState>
    where
        I: Into<SharedString>,
    {
        let id = id.into();
        let select = if let Some(select) = self.selects.fields.get(&id) {
            select.clone()
        } else {
            let select = cx.new(|cx| {
                NyaSelectState::new(cx, options.clone(), selected_value.clone()).disabled(disabled)
            });
            let subscription_id = id.clone();
            let subscription =
                cx.subscribe(
                    &select,
                    move |app: &mut NyaTermApp, _, event, cx| match event {
                        NyaSelectEvent::Changed(value) => {
                            app.on_select_changed(&subscription_id, value.as_deref(), cx);
                        }
                    },
                );
            self.selects.fields.insert(id.clone(), select.clone());
            self.selects.subscriptions.insert(id, subscription);
            select
        };

        select.update(cx, |select, cx| {
            select.set_options(options, cx);
            select.set_selected_value(selected_value, cx);
            select.set_disabled(disabled, cx);
        });
        select
    }

    pub(in crate::features) fn select_is_focused(
        &self,
        id: &str,
        window: &Window,
        cx: &App,
    ) -> bool {
        self.selects
            .fields
            .get(id)
            .is_some_and(|select| select.read(cx).is_focused(window, cx))
    }

    pub(in crate::features) fn select_menu_is_focused(
        &self,
        id: &str,
        window: &Window,
        cx: &App,
    ) -> bool {
        self.selects
            .fields
            .get(id)
            .is_some_and(|select| select.read(cx).is_menu_focused(window, cx))
    }

    pub(in crate::features) fn focus_select(
        &self,
        id: &str,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        let Some(select) = self.selects.fields.get(id) else {
            return false;
        };
        let focus = select.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        true
    }

    pub(in crate::features) fn select_with_prefix_is_focused(
        &self,
        prefix: &str,
        window: &Window,
        cx: &App,
    ) -> bool {
        self.selects.fields.iter().any(|(id, select)| {
            id.as_ref().starts_with(prefix) && select.read(cx).is_focused(window, cx)
        })
    }

    pub(in crate::features) fn select_menu_with_prefix_is_focused(
        &self,
        prefix: &str,
        window: &Window,
        cx: &App,
    ) -> bool {
        self.selects.fields.iter().any(|(id, select)| {
            id.as_ref().starts_with(prefix) && select.read(cx).is_menu_focused(window, cx)
        })
    }

    /// A select with no chrome of its own, for a control strip that draws its own.
    pub(in crate::features) fn form_select_control<I>(
        &mut self,
        id: I,
        options: Vec<NyaSelectOption>,
        selected_value: Option<String>,
        disabled: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<I>
    where
        I: Into<SharedString>,
    {
        let id = id.into();
        let select = self.select_entity(id.clone(), options, selected_value, disabled, cx);

        div()
            .id(id)
            .w_full()
            .h(px(NYA_FORM_CONTROL_HEIGHT_PX))
            .child(NyaSelect::new(&select))
    }

    pub(in crate::features) fn on_select_changed(
        &mut self,
        id: &str,
        value: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let Some(value) = value else {
            return;
        };

        if self.change_runtime_auth_select(id, value, cx) {
            return;
        }

        if id == "connection-editor-kind" {
            if let Some(kind) = [
                crate::models::ConnectionKindTab::Ssh,
                crate::models::ConnectionKindTab::Local,
                crate::models::ConnectionKindTab::Telnet,
                crate::models::ConnectionKindTab::Serial,
                crate::models::ConnectionKindTab::Rdp,
                crate::models::ConnectionKindTab::Vnc,
            ]
            .into_iter()
            .find(|kind| kind.label() == value)
            {
                self.set_connection_editor_kind(kind, cx);
            }
            return;
        }

        if let Some(index) = id
            .strip_prefix("connection-editor-ssh-agent-forwarding-endpoint-")
            .and_then(|index| index.parse::<usize>().ok())
        {
            self.set_connection_editor_agent_endpoint_type(index, value, cx);
            return;
        }

        if let Some(credential_id) = id.strip_prefix("ai-provider-protocol.") {
            self.set_ai_provider_protocol(credential_id.to_string(), value.to_string(), cx);
            return;
        }
        match id {
            "remote.stats.network" => self.select_stats_network_interface(value, cx),
            "appearance-ui-theme" => self.update_appearance_theme(value, cx),
            "appearance-terminal-theme" => {
                self.set_terminal_theme((value != FOLLOW_UI_THEME_VALUE).then_some(value), cx)
            }
            "appearance-minimum-contrast" => self.set_minimum_contrast_ratio(value, cx),
            "appearance-window-decorations" => self.set_window_decorations(
                nyaterm_core::models::window_decoration::WindowDecoration::from(value.to_owned()),
                cx,
            ),
            "appearance-background-fit" => self.set_background_image_fit(value, cx),
            "appearance-window-material" => self.set_window_material(
                nyaterm_core::models::window_material::WindowMaterial::from(value.to_owned()),
                cx,
            ),
            "appearance-terminal-font-weight" => {
                if let Ok(weight) = value.parse() {
                    self.set_terminal_font_weight(weight, cx);
                }
            }
            "appearance-terminal-font-weight-bold" => {
                if let Ok(weight) = value.parse() {
                    self.set_terminal_font_weight_bold(weight, cx);
                }
            }
            "appearance-cursor-style" => self.set_cursor_style(value, cx),
            "settings.general.language" => self.update_ui_language(value, cx),
            "settings.general.header-status" => {
                if value == "hidden" {
                    self.set_header_status_visible(false, cx);
                } else {
                    self.set_header_status_mode(HeaderStatusMode::from_setting(value), cx);
                }
            }
            "settings.general.diagnostics-level" => match value {
                "warn" => self.set_diagnostics_level("warn", cx),
                "debug" => self.set_diagnostics_level("debug", cx),
                _ => self.set_diagnostics_level("info", cx),
            },
            "settings.general.diagnostics-retention" => {
                if let Ok(days) = value.parse::<u32>() {
                    self.set_diagnostics_retention_days(days, cx);
                }
            }
            "settings.terminal.keep-alive-mode" => match value {
                "strict" => self.set_terminal_keep_alive_mode("strict", cx),
                "disabled" => self.set_terminal_keep_alive_mode("disabled", cx),
                _ => self.set_terminal_keep_alive_mode("compatible", cx),
            },
            "settings.interaction.default-encoding" => {
                match nyaterm_core::character_encoding::CharacterEncoding::parse(value) {
                    Ok(encoding) => self.set_interaction_encoding(encoding.label(), cx),
                    Err(error) => {
                        self.shell
                            .set_status(crate::features::terminal::encoding_error_text(error));
                        cx.notify();
                    }
                }
            }
            "settings.interaction.tab-double" => {
                self.set_tab_mouse_action(TabMouseActionTarget::Double, value, cx);
            }
            "settings.interaction.tab-middle" => {
                self.set_tab_mouse_action(TabMouseActionTarget::Middle, value, cx);
            }
            "settings.interaction.tab-right" => {
                self.set_tab_mouse_action(TabMouseActionTarget::Right, value, cx);
            }
            "settings.transfer.duplicate-strategy" => {
                self.update_transfer_duplicate_policy(
                    SftpDuplicatePolicy::from_legacy_value(value),
                    cx,
                );
            }
            "settings.transfer.editor-type" => match value {
                "internal" => self.update_transfer_editor_type("internal", cx),
                _ => self.update_transfer_editor_type("external", cx),
            },
            "settings.transfer.internal-editor-display" => self.update_internal_editor_display(
                nyaterm_core::InternalEditorDisplay::from_compat_value(value),
                cx,
            ),
            "settings.interaction.terminal-right-click-action" => self
                .set_terminal_right_click_action(
                    nyaterm_core::TerminalRightClickAction::from_compat_value(value),
                    cx,
                ),
            "settings.recording.default-mode" => match value {
                "raw" => self.set_recording_default_mode(RecordingMode::Raw, cx),
                _ => self.set_recording_default_mode(RecordingMode::Transcript, cx),
            },
            "settings.recording.rotation" => match value {
                "daily" => self.set_recording_rotation(RecordingRotationPolicy::Daily, cx),
                "size" => {
                    let bytes = match self.settings.summary().recording_rotation {
                        RecordingRotationPolicy::Size { max_bytes } => max_bytes,
                        _ => 50 * 1024 * 1024,
                    };
                    self.set_recording_rotation(
                        RecordingRotationPolicy::Size { max_bytes: bytes },
                        cx,
                    );
                }
                _ => self.set_recording_rotation(RecordingRotationPolicy::Session, cx),
            },
            "settings.recording.existing-file" => match value {
                "append" => {
                    self.set_recording_existing_file_behavior(ExistingFileBehavior::Append, cx)
                }
                "overwrite" => {
                    self.set_recording_existing_file_behavior(ExistingFileBehavior::Overwrite, cx)
                }
                _ => {
                    self.set_recording_existing_file_behavior(ExistingFileBehavior::Unique, cx);
                }
            },
            "settings.security.host-key-policy" => match value {
                "strict" => self.update_host_key_policy("strict", cx),
                "accept" => self.update_host_key_policy("accept", cx),
                _ => self.update_host_key_policy("prompt", cx),
            },
            "settings.translation.target-language" => {
                self.translation.select_target_language(value);
                self.save_translation_settings(cx);
            }
            "network-tunnel-editor-type" => self.set_network_tunnel_type(value, cx),
            "network-tunnel-editor-connection" => self.set_network_tunnel_connection(
                (value != NO_SELECTION_VALUE).then(|| value.to_string()),
                cx,
            ),
            "network-tunnel-editor-group" => self.set_network_tunnel_group(
                (value != NO_SELECTION_VALUE).then(|| value.to_string()),
                cx,
            ),
            "network-proxy-editor-protocol" => self.set_network_proxy_protocol(value, cx),
            "network-proxy-editor-group" => self.set_network_proxy_group(
                (value != NO_SELECTION_VALUE).then(|| value.to_string()),
                cx,
            ),
            "bottom-command-data-select" => {
                let data_type = match value {
                    "hex" => SendCommandDataType::Hex,
                    _ => SendCommandDataType::Text,
                };
                self.set_send_command_data_type(data_type, cx);
            }
            "bottom-command-mode-select" => {
                let mode = match value {
                    "byte" => SendCommandMode::Byte,
                    "packet" => SendCommandMode::Packet,
                    "character" => SendCommandMode::Character,
                    _ => SendCommandMode::Line,
                };
                self.set_send_command_mode(mode, cx);
            }
            "bottom-command-target-select" => {
                let target = match value {
                    "all" => SendCommandTarget::AllCompatible,
                    value if value.starts_with("group:") => {
                        SendCommandTarget::Group(value[6..].to_string())
                    }
                    _ => SendCommandTarget::Current,
                };
                self.set_send_command_target(target, cx);
            }
            "bottom-command-eol-select" => {
                let line_ending = match value {
                    "none" => SendCommandLineEnding::None,
                    "cr" => SendCommandLineEnding::Cr,
                    "lf" => SendCommandLineEnding::Lf,
                    _ => SendCommandLineEnding::Crlf,
                };
                self.set_send_command_line_ending(line_ending, cx);
            }
            "temporary-link-protocol" => {
                let protocol = match value {
                    "telnet" => TemporaryLinkProtocol::Telnet,
                    "serial" => TemporaryLinkProtocol::Serial,
                    _ => TemporaryLinkProtocol::Ssh,
                };
                self.set_temporary_link_protocol(protocol, cx);
            }
            "temporary-link-serial-port" => {
                self.apply_temporary_serial_port_name(value.to_string(), cx);
            }
            "cloud-provider-select" => self.update_cloud_sync_provider(value, cx),
            "cloud-conflict-branch" => {
                self.cloud_sync.select_conflict_branch(value);
                self.request_settings_panel_refresh(cx);
                cx.notify();
            }
            "ai-proxy-mode" | "ai-proxy-protocol" => {
                self.ai.set_proxy_selection(id, value);
                cx.notify();
            }
            "ai-smart-risk" => {
                let risk = match value {
                    "low" => Some(RiskLevel::Low),
                    "medium" => Some(RiskLevel::Medium),
                    "high" => Some(RiskLevel::High),
                    "critical" => Some(RiskLevel::Critical),
                    _ => None,
                };
                if let Some(risk) = risk {
                    self.update_ai_smart_auto_execute_max_risk(risk, cx);
                }
            }
            "ai-default-agent" => {
                let kind = match value {
                    "codex" => AiAgentKind::Codex,
                    "claude_code" => AiAgentKind::ClaudeCode,
                    _ => AiAgentKind::Nyaterm,
                };
                self.set_ai_default_agent(kind, cx);
            }
            "ai-external-permission" => {
                if let Some(mode) = ai_permission_mode(value) {
                    self.request_ai_permission_mode(AiFullAccessSetting::ExternalAgent, mode, cx);
                }
            }
            "ai-codex-default-model" => {
                self.ai.settings_config_mut().codex.default_model =
                    (value != "__none__").then(|| value.to_string());
                self.reset_text_input(
                    "ai.input.codex-default-model",
                    if value == "__none__" { "" } else { value },
                    cx,
                );
                self.persist_ai_settings_now(cx);
            }
            "ai-codex-permission" => {
                if let Some(mode) = ai_permission_mode(value) {
                    self.request_ai_permission_mode(AiFullAccessSetting::Codex, mode, cx);
                }
            }
            "ai-claude-permission" => {
                if let Some(mode) = ai_permission_mode(value) {
                    self.request_ai_permission_mode(AiFullAccessSetting::ClaudeCode, mode, cx);
                }
            }
            "ai-mcp-permission" => {
                if let Some(mode) = ai_permission_mode(value) {
                    self.request_ai_permission_mode(AiFullAccessSetting::McpHost, mode, cx);
                }
            }
            "ai-codex-thread-mode" => self.set_ai_codex_thread_mode(
                if value == "ephemeral" {
                    CodexThreadMode::Ephemeral
                } else {
                    CodexThreadMode::Persistent
                },
                cx,
            ),
            "ai-mcp-session-scope" => self.set_ai_mcp_scope(
                if value == "all_sessions" {
                    ExternalMcpSessionScope::AllSessions
                } else {
                    ExternalMcpSessionScope::CurrentWindow
                },
                cx,
            ),
            "ai-mcp-terminal-display-mode" => match value {
                "inline" => {
                    self.set_ai_mcp_terminal_display_mode(McpTerminalDisplayMode::Inline, cx)
                }
                "silent" => {
                    self.set_ai_mcp_terminal_display_mode(McpTerminalDisplayMode::Silent, cx)
                }
                _ => {}
            },
            id if id.starts_with("quick-command.variable.") => {
                if let Some(index) = id
                    .strip_prefix("quick-command.variable.")
                    .and_then(|index| index.parse::<usize>().ok())
                {
                    self.apply_quick_command_variable(index, value.to_string(), cx);
                }
            }
            id if id.starts_with("connection-editor-") => {
                let select = match id {
                    "connection-editor-group-select" => ConnectionEditorSelect::Group,
                    "connection-editor-saved-password" => ConnectionEditorSelect::SavedPassword,
                    "connection-editor-account" => ConnectionEditorSelect::Account,
                    "connection-editor-ssh-key" => ConnectionEditorSelect::SshKey,
                    "connection-editor-otp" => ConnectionEditorSelect::Otp,
                    "connection-editor-proxy" => ConnectionEditorSelect::Proxy,
                    "connection-editor-proxy-jump" => ConnectionEditorSelect::ProxyJump,
                    "connection-editor-backspace" => ConnectionEditorSelect::Backspace,
                    "connection-editor-encoding" => ConnectionEditorSelect::Encoding,
                    "connection-editor-sftp-cwd-follow" => {
                        ConnectionEditorSelect::SftpCwdFollowMode
                    }
                    "connection-editor-sftp-pipeline-depth" => {
                        ConnectionEditorSelect::SftpPipelineDepth
                    }
                    "connection-editor-sftp-filename-encoding" => {
                        ConnectionEditorSelect::SftpFilenameEncoding
                    }
                    "connection-editor-ssh-algorithm-mode" => {
                        ConnectionEditorSelect::SshAlgorithmMode
                    }
                    "connection-editor-ssh-agent-policy" => {
                        ConnectionEditorSelect::SshAgentForwardingPolicy
                    }
                    "connection-editor-ssh-agent-endpoint" => {
                        ConnectionEditorSelect::SshAgentEndpoint
                    }
                    "connection-editor-ssh-profile" => ConnectionEditorSelect::SshProfile,
                    "connection-editor-ssh-terminal-type" => {
                        ConnectionEditorSelect::SshTerminalType
                    }
                    "connection-editor-rdp-backend" => ConnectionEditorSelect::RdpBackend,
                    "connection-editor-rdp-certificate-policy" => {
                        ConnectionEditorSelect::RdpCertificatePolicy
                    }
                    "connection-editor-rdp-display-mode" => ConnectionEditorSelect::RdpDisplayMode,
                    "connection-editor-rdp-clipboard-mode" => {
                        ConnectionEditorSelect::RdpClipboardMode
                    }
                    "connection-editor-vnc-security-mode" => {
                        ConnectionEditorSelect::VncSecurityMode
                    }
                    "connection-editor-vnc-scale-mode" => ConnectionEditorSelect::VncScaleMode,
                    "connection-editor-recording-mode" => ConnectionEditorSelect::RecordingMode,
                    "connection-editor-telnet-enter-mode" => {
                        ConnectionEditorSelect::TelnetEnterMode
                    }
                    "connection-editor-shell" => ConnectionEditorSelect::Shell,
                    "connection-editor-serial-port" => ConnectionEditorSelect::SerialPort,
                    "connection-editor-baud-rate" => ConnectionEditorSelect::BaudRate,
                    "connection-editor-data-bits" => ConnectionEditorSelect::DataBits,
                    "connection-editor-parity" => ConnectionEditorSelect::Parity,
                    "connection-editor-flow-control" => ConnectionEditorSelect::FlowControl,
                    "connection-editor-stop-bits" => ConnectionEditorSelect::StopBits,
                    _ => return,
                };
                if select == ConnectionEditorSelect::Group
                    && value == PENDING_CONNECTION_GROUP_VALUE
                {
                    return;
                }
                self.set_connection_editor_select_value(
                    select,
                    (value != NO_SELECTION_VALUE).then_some(value),
                    cx,
                );
            }
            _ => {
                let font = id
                    .strip_prefix("appearance-terminal-font-")
                    .and_then(|index| index.parse::<usize>().ok())
                    .map(|index| (true, index))
                    .or_else(|| {
                        id.strip_prefix("appearance-ui-font-")
                            .and_then(|index| index.parse::<usize>().ok())
                            .map(|index| (false, index))
                    });
                if let Some((terminal, index)) = font {
                    self.set_appearance_font_stack_entry(terminal, index, value.to_string(), cx);
                }
            }
        }
    }
}
