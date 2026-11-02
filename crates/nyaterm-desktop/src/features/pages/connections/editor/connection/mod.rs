use crate::models::ConnectionEditorFocusTarget;
mod authentication;
mod layout;
mod local;
mod rdp;
mod recording;
mod serial;
mod ssh;
mod tags;
mod telnet;
mod vnc;

use rust_i18n::t;

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use gpui::{
    AnyElement, App, Bounds, Context, Entity, FontWeight, IntoElement, Pixels, Point, SharedString,
    anchored, deferred, div,
    prelude::{
        FluentBuilder, InteractiveElement, ParentElement, StatefulInteractiveElement, Styled,
    },
    px, rgb, rgba, svg,
};
use nyaterm_core::{ConnectionType, Group, SavedConnection, natural_compare, truncate_preview};
use nyaterm_ui::{
    NyaCheckbox, NyaInput, NyaPopover, NyaScrollArea, NyaScrollable, NyaSelectOption,
    NyaSelectState, NyaSwitch, NyaTooltip,
};

use self::local::connection_editor_local_section;
use self::rdp::connection_editor_rdp_section;
use self::serial::connection_editor_serial_section;
use self::ssh::{
    SshConnectionSectionLabels, SshConnectionSectionOptions, connection_editor_ssh_section,
};
use self::telnet::connection_editor_telnet_section;
use self::vnc::connection_editor_vnc_section;
use super::super::list::{
    ConnectionEditorChoice, ConnectionEditorFields, EDITOR_CONTROL_HEIGHT_PX, editor_field,
};
use crate::features::selects::{NO_SELECTION_VALUE, PENDING_CONNECTION_GROUP_VALUE};
use crate::features::{
    NyaTermApp, icons::CONNECTION_ICON_OPTIONS, icons::DEFAULT_CONNECTION_ICON,
    icons::resolve_connection_icon, text_inputs::ORDINARY_INPUT_SHELL_PADDING_X_PX,
    text_inputs::ordinary_input_focus_ring, text_inputs::ordinary_input_shell_border_color,
    view_widgets::full_window_input_layer, view_widgets::modal_dialog_shell,
    view_widgets::themed_icon,
};
use crate::models::{
    ConnectionEditorCredentialOverlay, ConnectionEditorField, ConnectionEditorSection,
    ConnectionEditorSelect, ConnectionEditorState, ConnectionKindTab,
};

#[derive(Clone, Copy)]
struct ConnectionEditorSectionContext<'a> {
    palette: crate::theme::ThemePalette,
    editor: &'a ConnectionEditorState,
    fields: &'a ConnectionEditorFields,
    baud_popover_open: bool,
    narrow: bool,
}

impl NyaTermApp {
    pub(in crate::features) fn connection_editor_panel(
        &mut self,
        editor: ConnectionEditorState,
        viewport: (f32, f32),
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.connection_editor_surface(editor, false, viewport, cx)
    }

    pub(in crate::features) fn connection_editor_window_view(
        &mut self,
        editor: ConnectionEditorState,
        viewport: (f32, f32),
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.connection_editor_surface(editor, true, viewport, cx)
    }

    fn connection_editor_select_entity(
        &mut self,
        select_key: ConnectionEditorSelect,
        choices: &[ConnectionEditorChoice],
        placeholder: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) -> Entity<NyaSelectState> {
        let options = Self::connection_editor_select_options(choices);
        let selected_value = choices.iter().find(|choice| choice.selected).map(|choice| {
            choice
                .value
                .clone()
                .unwrap_or_else(|| NO_SELECTION_VALUE.to_string())
        });
        let select = self.select_entity(
            connection_editor_select_id(select_key),
            options,
            selected_value,
            false,
            cx,
        );
        let searchable = connection_editor_select_is_searchable(select_key);
        let search_placeholder = connection_editor_select_search_placeholder(select_key);
        select.update(cx, |select, cx| {
            select.set_placeholder(placeholder, cx);
            select.set_searchable(searchable, cx);
            select.set_search_placeholder(search_placeholder, cx);
        });
        select
    }

    fn connection_editor_select_options(
        choices: &[ConnectionEditorChoice],
    ) -> Vec<NyaSelectOption> {
        choices
            .iter()
            .map(|choice| {
                let mut option = NyaSelectOption::new(
                    choice
                        .value
                        .clone()
                        .unwrap_or_else(|| NO_SELECTION_VALUE.to_string()),
                    choice.label.clone(),
                )
                .disabled(choice.disabled);
                if let Some(search_text) = choice.search_text.as_ref() {
                    option = option.search_text(search_text.clone());
                }
                if let Some(subtitle) = choice.subtitle.as_ref() {
                    option = option.subtitle(subtitle.clone());
                }
                option
            })
            .collect()
    }

    pub(super) fn ssh_agent_endpoint_value(
        endpoint: &nyaterm_core::SshAgentEndpoint,
    ) -> &'static str {
        match endpoint {
            nyaterm_core::SshAgentEndpoint::Auto => "auto",
            nyaterm_core::SshAgentEndpoint::Environment { .. } => "environment",
            nyaterm_core::SshAgentEndpoint::UnixSocket { .. } => "unix_socket",
            nyaterm_core::SshAgentEndpoint::WindowsOpenSsh => "windows_openssh",
            nyaterm_core::SshAgentEndpoint::Pageant => "pageant",
        }
    }

    pub(super) fn ssh_agent_endpoint_choices(
        endpoint: &nyaterm_core::SshAgentEndpoint,
    ) -> Vec<ConnectionEditorChoice> {
        let selected = Self::ssh_agent_endpoint_value(endpoint);
        let mut choices = vec![ConnectionEditorChoice::new(
            Some("auto".to_string()),
            t!("dialog.sshAgentAuto"),
            selected == "auto",
        )];
        if cfg!(unix) {
            choices.extend([
                ConnectionEditorChoice::new(
                    Some("environment".to_string()),
                    t!("dialog.sshAgentEnvironment"),
                    selected == "environment",
                ),
                ConnectionEditorChoice::new(
                    Some("unix_socket".to_string()),
                    t!("dialog.sshAgentUnixSocket"),
                    selected == "unix_socket",
                ),
            ]);
        }
        if cfg!(windows) {
            choices.extend([
                ConnectionEditorChoice::new(
                    Some("windows_openssh".to_string()),
                    t!("dialog.sshAgentWindowsOpenSsh"),
                    selected == "windows_openssh",
                ),
                ConnectionEditorChoice::new(
                    Some("pageant".to_string()),
                    t!("dialog.sshAgentPageant"),
                    selected == "pageant",
                ),
            ]);
        }
        if !nyaterm_core::ssh_agent_endpoint_supported_on_current_platform(endpoint) {
            let label = match endpoint {
                nyaterm_core::SshAgentEndpoint::Environment { .. } => {
                    t!("dialog.sshAgentEnvironment")
                }
                nyaterm_core::SshAgentEndpoint::UnixSocket { .. } => {
                    t!("dialog.sshAgentUnixSocket")
                }
                nyaterm_core::SshAgentEndpoint::Pageant => t!("dialog.sshAgentPageant"),
                nyaterm_core::SshAgentEndpoint::WindowsOpenSsh => {
                    t!("dialog.sshAgentWindowsOpenSsh")
                }
                nyaterm_core::SshAgentEndpoint::Auto => t!("dialog.sshAgentAuto"),
            };
            choices.push(ConnectionEditorChoice::new(
                Some(selected.to_string()),
                format!("{} ({})", label, t!("dialog.sshAgentUnavailableOnPlatform")),
                true,
            ));
        }
        choices
    }

    fn connection_editor_surface(
        &mut self,
        editor: ConnectionEditorState,
        native_window: bool,
        viewport: (f32, f32),
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.connection_state
            .ensure_editor_forwarding_endpoint_fields(cx);
        self.connection_state.sync_editor_password_input(cx);
        let password_disabled =
            editor.kind == ConnectionKindTab::Vnc && editor.vnc_security.mode == "none";
        if let Some(input) = self
            .connection_state
            .editor_fields()
            .get(&ConnectionEditorField::Password)
        {
            input.update(cx, |state, cx| state.set_disabled(password_disabled, cx));
        }
        for (field, disabled) in [
            (
                ConnectionEditorField::SftpShellDetectionTimeout,
                editor.kind == ConnectionKindTab::Ssh && !editor.sftp_enabled,
            ),
            (
                ConnectionEditorField::RdpReconnectAttempts,
                editor.kind == ConnectionKindTab::Rdp && !editor.rdp_reconnect.enabled,
            ),
            (
                ConnectionEditorField::VncReconnectAttempts,
                editor.kind == ConnectionKindTab::Vnc && !editor.vnc_reconnect.enabled,
            ),
        ] {
            if let Some(entity) = self
                .connection_state
                .editor_number_fields()
                .get(&field)
                .cloned()
            {
                entity.update(cx, |entity, cx| entity.set_disabled(disabled, cx));
            }
        }
        let palette = self.theme_palette();
        let title = if editor.id.is_some() {
            t!("dialog.editConnection")
        } else {
            t!("dialog.newConnection")
        };
        let name_label = t!("dialog.connectionName");
        let description_label = t!("dialog.description");
        let group_title = t!("dialog.group");
        let none_label = t!("dialog.none");
        let icon_label = t!("dialog.icon");
        let icon_auto_detect_label = t!("dialog.iconAutoDetect");
        let icon_auto_detect_hint = t!("dialog.iconAutoDetectTooltip");
        let icon_auto_detect = editor.icon_auto_detect;
        let icon_auto_detect_enabled = editor.kind == ConnectionKindTab::Ssh
            && editor.ssh_profile == nyaterm_core::SshProfile::Standard;
        let icon_auto_detect_hint = if editor.kind != ConnectionKindTab::Ssh
            || editor.ssh_profile == nyaterm_core::SshProfile::NetworkDevice
        {
            t!("dialog.iconAutoDetectNetworkDeviceTooltip").to_string()
        } else {
            icon_auto_detect_hint.to_string()
        };
        let group_options = connection_editor_group_menu_options(
            self.connection_state.groups(),
            &editor,
            none_label.clone(),
            t!("dialog.newGroup"),
        );
        let group_parent_id = if editor.pending_group_name.is_some() {
            editor.pending_group_parent_id.as_deref()
        } else {
            editor.group_id.as_deref()
        };
        let group_parent_hint = group_parent_id
            .and_then(|id| connection_group_path_label(self.connection_state.groups(), id))
            .map(|path| t!("dialog.newGroupParentHint", group = path).to_string())
            .unwrap_or_else(|| t!("dialog.newGroupRootHint").to_string());
        let key_label = editor
            .key_id
            .as_deref()
            .and_then(|id| {
                self.security
                    .ssh_keys()
                    .iter()
                    .find(|key| key.id == id)
                    .map(|key| key.name.clone())
            })
            .unwrap_or_else(|| none_label.to_string());
        let password_label = editor
            .account_id
            .as_ref()
            .filter(|_| {
                matches!(
                    editor.kind,
                    ConnectionKindTab::Ssh | ConnectionKindTab::Telnet
                )
            })
            .or(editor.password_id.as_ref())
            .and_then(|id| {
                self.security
                    .passwords()
                    .iter()
                    .find(|password| password.id == id.as_str())
                    .map(|password| password.name.clone())
            })
            .unwrap_or_else(|| t!("dialog.selectPassword").to_string());
        let otp_label = editor
            .otp_id
            .as_deref()
            .and_then(|id| {
                self.security
                    .otp_entries()
                    .iter()
                    .find(|entry| entry.id == id)
                    .map(|entry| {
                        if entry.issuer.is_empty() {
                            entry.username.clone()
                        } else if entry.username.is_empty() {
                            entry.issuer.clone()
                        } else {
                            format!("{} ({})", entry.issuer, entry.username)
                        }
                    })
            })
            .unwrap_or_else(|| t!("dialog.noOtp").to_string());
        let proxy_label = editor
            .proxy_id
            .as_deref()
            .and_then(|id| {
                self.tunnel_state
                    .proxies()
                    .iter()
                    .find(|proxy| proxy.id == id)
                    .map(|proxy| {
                        if proxy.protocol == "proxycommand" {
                            let command = proxy.command.as_deref().unwrap_or("").trim();
                            if command.is_empty() {
                                format!("{} · {}", proxy.name, proxy.protocol.to_ascii_uppercase())
                            } else {
                                format!(
                                    "{} · {} {}",
                                    proxy.name,
                                    proxy.protocol.to_ascii_uppercase(),
                                    truncate_preview(command, 18)
                                )
                            }
                        } else {
                            format!(
                                "{} · {} {}:{}",
                                proxy.name,
                                proxy.protocol.to_ascii_uppercase(),
                                proxy.host,
                                proxy.port
                            )
                        }
                    })
            })
            .unwrap_or_else(|| t!("dialog.noProxy").to_string());
        let jump_label = editor
            .proxy_jump_id
            .as_deref()
            .and_then(|id| {
                self.connection_state
                    .connections()
                    .iter()
                    .find(|connection| connection.id == id)
                    .map(|connection| connection.name.clone())
            })
            .unwrap_or_else(|| t!("dialog.noProxyJump").to_string());
        let auth_options = [
            ("none", t!("dialog.noAuthentication")),
            ("password", t!("dialog.password")),
            ("key", t!("dialog.privateKey")),
            ("agent", t!("dialog.sshAgent")),
        ]
        .into_iter()
        .map(|(value, label)| {
            ConnectionEditorChoice::new(Some(value.to_string()), label, editor.auth_mode == value)
        })
        .collect::<Vec<_>>();
        let mut key_options = vec![ConnectionEditorChoice::new(
            None,
            none_label.clone(),
            editor.key_id.is_none(),
        )];
        key_options.extend(self.security.ssh_keys().iter().map(|key| {
            let search_text = [Some(key.name.as_str()), key.key_file_path.as_deref()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" ");
            let subtitle = key
                .key_file_path
                .clone()
                .or_else(|| key.cert_file_path.clone());
            let mut choice = ConnectionEditorChoice::new(
                Some(key.id.clone()),
                key.name.clone(),
                editor.key_id.as_deref() == Some(key.id.as_str()),
            )
            .search_text(search_text);
            if let Some(subtitle) = subtitle {
                choice = choice.subtitle(subtitle);
            }
            choice
        }));
        let mut password_options = vec![ConnectionEditorChoice::new(
            None,
            none_label.clone(),
            editor.account_id.is_none() && editor.password_id.is_none(),
        )];
        password_options.extend(self.security.passwords().iter().map(|password| {
            ConnectionEditorChoice::new(
                Some(password.id.clone()),
                password.name.clone(),
                if matches!(
                    editor.kind,
                    ConnectionKindTab::Ssh | ConnectionKindTab::Telnet
                ) {
                    editor.account_id.as_deref() == Some(password.id.as_str())
                } else {
                    editor.password_id.as_deref() == Some(password.id.as_str())
                },
            )
            .subtitle(password.username.clone())
        }));
        let reference = if matches!(
            editor.kind,
            ConnectionKindTab::Ssh | ConnectionKindTab::Telnet
        ) {
            editor.account_id.as_ref()
        } else {
            editor.password_id.as_ref()
        };
        if let Some(id) = reference.filter(|id| {
            !self
                .security
                .passwords()
                .iter()
                .any(|entry| entry.id == **id)
        }) {
            password_options.push(ConnectionEditorChoice::new(
                Some(id.clone()),
                t!("passwordManager.missingAccount"),
                true,
            ));
        }
        let mut account_options = password_options.clone();
        if let Some(choice) = account_options
            .first_mut()
            .filter(|choice| choice.value.is_none())
        {
            choice.label = t!("authForm.manualIdentity").to_string();
        }
        for choice in account_options
            .iter_mut()
            .filter(|choice| choice.value.is_some())
        {
            let Some(id) = choice.value.as_deref() else {
                continue;
            };
            let Some(account) = self
                .security
                .passwords()
                .iter()
                .find(|entry| entry.id == id)
            else {
                continue;
            };
            let username = account.username.trim();
            let username = if username.is_empty() {
                t!("authForm.accountUsernameFallback").to_string()
            } else {
                username.to_string()
            };
            let password_status = if account.has_password {
                t!("authForm.passwordSaved")
            } else {
                t!("authForm.passwordNotSaved")
            };
            choice.subtitle = Some(format!("{username} · {password_status}"));
        }
        let account_placeholder = if editor.account_id.is_none() {
            t!("authForm.manualIdentity").to_string()
        } else {
            password_label.clone()
        };
        let mut otp_options = vec![ConnectionEditorChoice::new(
            None,
            t!("dialog.noOtp"),
            editor.otp_id.is_none(),
        )];
        otp_options.extend(self.security.otp_entries().iter().map(|entry| {
            let label = if entry.issuer.is_empty() {
                entry.username.clone()
            } else if entry.username.is_empty() {
                entry.issuer.clone()
            } else {
                format!("{} ({})", entry.issuer, entry.username)
            };
            let subtitle = [
                entry.otp_type.to_ascii_uppercase(),
                entry.algorithm.clone(),
                entry.digits.to_string(),
            ]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" · ");
            let search_text = [
                entry.issuer.as_str(),
                entry.username.as_str(),
                entry.otp_type.as_str(),
                entry.algorithm.as_str(),
            ]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
            ConnectionEditorChoice::new(
                Some(entry.id.clone()),
                label,
                editor.otp_id.as_deref() == Some(entry.id.as_str()),
            )
            .search_text(search_text)
            .subtitle(subtitle)
        }));
        let mut proxy_options = vec![ConnectionEditorChoice::new(
            None,
            t!("dialog.noProxy"),
            editor.proxy_id.is_none(),
        )];
        proxy_options.extend(self.tunnel_state.proxies().iter().map(|proxy| {
            let protocol = proxy.protocol.to_ascii_uppercase();
            let subtitle = if proxy.protocol == "proxycommand" {
                [protocol.as_str(), proxy.command.as_deref().unwrap_or("")]
                    .into_iter()
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
                    .join(" · ")
            } else {
                [
                    format!("{} {}:{}", protocol, proxy.host, proxy.port),
                    proxy.username.clone().unwrap_or_default(),
                ]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join(" · ")
            };
            let search_text = [
                proxy.name.clone(),
                proxy.protocol.clone(),
                proxy.host.clone(),
                proxy.port.to_string(),
                proxy.username.clone().unwrap_or_default(),
                proxy.command.clone().unwrap_or_default(),
            ]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
            ConnectionEditorChoice::new(
                Some(proxy.id.clone()),
                proxy.name.clone(),
                editor.proxy_id.as_deref() == Some(proxy.id.as_str()),
            )
            .search_text(search_text)
            .subtitle(subtitle)
        }));
        let mut jump_options = vec![ConnectionEditorChoice::new(
            None,
            t!("dialog.noProxyJump"),
            editor.proxy_jump_id.is_none(),
        )];
        jump_options.extend(
            self.connection_state
                .connections()
                .iter()
                .filter(|connection| matches!(connection.config, ConnectionType::Ssh { .. }))
                .filter(|connection| editor.id.as_deref() != Some(connection.id.as_str()))
                .filter(|connection| {
                    !connection_proxy_jump_would_cycle(
                        self.connection_state.connections(),
                        editor.id.as_deref(),
                        connection,
                    )
                })
                .map(|connection| {
                    let (host, port, username) = match &connection.config {
                        ConnectionType::Ssh {
                            host,
                            port,
                            username,
                            ..
                        } => (host.as_str(), *port, username.as_str()),
                        _ => ("", 22, ""),
                    };
                    let group_path = connection
                        .group_id
                        .as_deref()
                        .and_then(|id| {
                            connection_group_path_label(self.connection_state.groups(), id)
                        })
                        .unwrap_or_default();
                    let host_port = format!("{host}:{port}");
                    let subtitle = [group_path.as_str(), host_port.as_str(), username]
                        .into_iter()
                        .filter(|part| !part.is_empty())
                        .collect::<Vec<_>>()
                        .join(" · ");
                    let search_text = [
                        connection.name.as_str(),
                        host,
                        username,
                        group_path.as_str(),
                    ]
                    .into_iter()
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ");
                    ConnectionEditorChoice::new(
                        Some(connection.id.clone()),
                        connection.name.clone(),
                        editor.proxy_jump_id.as_deref() == Some(connection.id.as_str()),
                    )
                    .search_text(search_text)
                    .subtitle(subtitle)
                }),
        );
        let backspace_options = [
            ("del", t!("dialog.backspaceDel")),
            ("ctrl-h", t!("dialog.backspaceCtrlH")),
        ]
        .into_iter()
        .map(|(value, label)| {
            ConnectionEditorChoice::new(
                Some(value.to_string()),
                label,
                match value {
                    "ctrl-h" => {
                        matches!(editor.backspace_mode.as_str(), "ctrl-h" | "bs" | "ctrl_h")
                    }
                    _ => !matches!(editor.backspace_mode.as_str(), "ctrl-h" | "bs" | "ctrl_h"),
                },
            )
        })
        .collect::<Vec<_>>();
        let telnet_enter_options = [
            ("crlf", "CRLF (\\r\\n)"),
            ("cr", "CR (\\r)"),
            ("lf", "LF (\\n)"),
        ]
        .into_iter()
        .map(|(value, label)| {
            ConnectionEditorChoice::new(
                Some(value.to_string()),
                label,
                editor.telnet_enter_mode == value,
            )
        })
        .collect::<Vec<_>>();
        let sftp_pipeline_options = std::iter::once(ConnectionEditorChoice::new(
            None,
            t!("dialog.sftpPipelineAutomatic"),
            editor.sftp_pipeline_depth.is_none(),
        ))
        .chain((4..=64).map(|depth| {
            ConnectionEditorChoice::new(
                Some(depth.to_string()),
                depth.to_string(),
                editor.sftp_pipeline_depth == Some(depth),
            )
        }))
        .collect::<Vec<_>>();
        let encoding_options = std::iter::once("global")
            .chain(
                nyaterm_core::character_encoding::CharacterEncoding::ALL
                    .into_iter()
                    .map(|encoding| encoding.label()),
            )
            .map(|value| {
                let label = if value == "global" {
                    t!("connection.encodingFollowGlobal").to_string()
                } else {
                    value.to_string()
                };
                ConnectionEditorChoice::new(
                    Some(value.to_string()),
                    label,
                    (value == "global"
                        && (editor.encoding.trim().is_empty()
                            || editor.encoding.trim().eq_ignore_ascii_case("global")))
                        || editor.encoding == value
                        || nyaterm_core::character_encoding::CharacterEncoding::parse(
                            &editor.encoding,
                        )
                        .is_ok_and(|encoding| encoding.label() == value),
                )
            })
            .collect::<Vec<_>>();
        let ssh_profile_options = [
            ("standard", t!("dialog.sshProfileStandard")),
            ("network_device", t!("dialog.sshProfileNetworkDevice")),
        ]
        .into_iter()
        .map(|(value, label)| {
            ConnectionEditorChoice::new(
                Some(value.to_string()),
                label,
                matches!(
                    (editor.ssh_profile, value),
                    (nyaterm_core::SshProfile::Standard, "standard")
                        | (nyaterm_core::SshProfile::NetworkDevice, "network_device")
                ),
            )
        })
        .collect::<Vec<_>>();
        let effective_terminal =
            nyaterm_core::resolve_ssh_terminal_type(editor.ssh_profile, editor.terminal_type);
        let mut ssh_terminal_options = vec![ConnectionEditorChoice::new(
            None,
            t!(
                "dialog.sshTerminalTypeDefault",
                value = effective_terminal.as_str()
            ),
            editor.terminal_type.is_none(),
        )];
        ssh_terminal_options.extend(
            ["xterm-256color", "xterm", "vt100", "vt220", "ansi", "linux"]
                .into_iter()
                .map(|value| {
                    ConnectionEditorChoice::new(
                        Some(value.to_string()),
                        value,
                        editor.terminal_type.is_some() && effective_terminal.as_str() == value,
                    )
                })
                .collect::<Vec<_>>(),
        );
        let mut rdp_backend_options = vec![ConnectionEditorChoice::new(
            Some("builtin".into()),
            t!("dialog.rdpBackendBuiltin"),
            matches!(editor.rdp_backend.as_str(), "builtin" | "internal" | "auto"),
        )];
        let mut windows_backend = ConnectionEditorChoice::new(
            Some("windows".into()),
            t!("dialog.rdpBackendWindows"),
            matches!(editor.rdp_backend.as_str(), "windows" | "active-x"),
        );
        windows_backend.disabled = !cfg!(windows);
        rdp_backend_options.push(windows_backend);
        if !matches!(
            editor.rdp_backend.as_str(),
            "auto" | "builtin" | "internal" | "windows" | "active-x"
        ) {
            rdp_backend_options.push(ConnectionEditorChoice::new(
                Some(editor.rdp_backend.clone()),
                t!("dialog.rdpBackendUnsupported"),
                true,
            ));
        }
        let mut rdp_certificate_options = [
            ("prompt", t!("dialog.rdpCertificatePrompt")),
            ("strict", t!("dialog.rdpCertificateStrict")),
            ("accept-temporarily", t!("dialog.rdpCertificateTemporary")),
        ]
        .into_iter()
        .map(|(value, label)| {
            let selected = match editor.rdp_security.certificate_policy.as_str() {
                "strict" | "reject_changed" | "reject-changed" => value == "strict",
                "insecure" | "accept-temporarily" => value == "accept-temporarily",
                _ => value == "prompt",
            };
            ConnectionEditorChoice::new(Some(value.to_string()), label, selected)
        })
        .collect::<Vec<_>>();
        if matches!(editor.rdp_backend.as_str(), "windows" | "active-x") {
            rdp_certificate_options = vec![
                ConnectionEditorChoice::new(
                    None,
                    t!("dialog.rdpWindowsAuthenticationRequired"),
                    editor.rdp_security.windows_authentication.is_none(),
                ),
                ConnectionEditorChoice::new(
                    Some("system".into()),
                    t!("dialog.rdpWindowsAuthentication"),
                    editor.rdp_security.windows_authentication.as_deref() == Some("system"),
                ),
            ];
        }
        let rdp_display_options = [
            ("fit-window", t!("dialog.rdpDisplayFitWindow")),
            ("fixed", t!("dialog.rdpDisplayFixed")),
        ]
        .into_iter()
        .map(|(value, label)| {
            let current = match editor.rdp_display.mode.as_str() {
                "fixed" | "native" => "fixed",
                _ => "fit-window",
            };
            ConnectionEditorChoice::new(Some(value.to_string()), label, current == value)
        })
        .collect::<Vec<_>>();
        let rdp_clipboard_options = [
            ("text-only", t!("dialog.rdpClipboardTextOnly")),
            ("text-and-files", t!("dialog.rdpClipboardTextAndFiles")),
            ("disabled", t!("dialog.disabled")),
        ]
        .into_iter()
        .map(|(value, label)| {
            ConnectionEditorChoice::new(
                Some(value.to_string()),
                label,
                editor.rdp_clipboard.mode == value,
            )
        })
        .collect::<Vec<_>>();
        let vnc_security_options = [
            ("auto", t!("dialog.vncSecurityAuto")),
            ("none", t!("dialog.vncSecurityNone")),
            ("vnc-auth", t!("dialog.vncSecurityPassword")),
        ]
        .into_iter()
        .map(|(value, label)| {
            ConnectionEditorChoice::new(
                Some(value.to_string()),
                label,
                editor.vnc_security.mode == value,
            )
        })
        .collect::<Vec<_>>();
        let vnc_scale_options = [
            ("fit", t!("dialog.vncScaleFit")),
            ("stretch", t!("dialog.vncScaleStretch")),
            ("actual", t!("dialog.vncScaleActual")),
        ]
        .into_iter()
        .map(|(value, label)| {
            ConnectionEditorChoice::new(
                Some(value.to_string()),
                label,
                editor.vnc_display.scale_mode == value,
            )
        })
        .collect::<Vec<_>>();
        let recording_mode = editor
            .recording
            .as_ref()
            .and_then(|settings| settings.mode)
            .unwrap_or(nyaterm_core::RecordingMode::Transcript);
        let recording_mode_options = [
            ("transcript", t!("dialog.recordingModeTranscript")),
            ("raw", t!("dialog.recordingModeRaw")),
        ]
        .into_iter()
        .map(|(value, label)| {
            ConnectionEditorChoice::new(
                Some(value.to_string()),
                label,
                matches!(
                    (recording_mode, value),
                    (nyaterm_core::RecordingMode::Transcript, "transcript")
                        | (nyaterm_core::RecordingMode::Raw, "raw")
                ),
            )
        })
        .collect::<Vec<_>>();
        let sftp_cwd_options = [
            ("off", t!("dialog.sftpCwdFollowOff")),
            (
                "shell_integration",
                t!("dialog.sftpCwdFollowShellIntegration"),
            ),
            ("rc_file", t!("dialog.sftpCwdFollowRcFile")),
        ]
        .into_iter()
        .map(|(value, label)| {
            ConnectionEditorChoice::new(
                Some(value.to_string()),
                label,
                editor.sftp_cwd_follow_mode == value,
            )
        })
        .collect::<Vec<_>>();
        let sftp_filename_encoding_options = std::iter::once("terminal")
            .chain(
                nyaterm_core::character_encoding::CharacterEncoding::ALL
                    .into_iter()
                    .map(|encoding| encoding.label()),
            )
            .map(|value| {
                let label = if value == "terminal" {
                    t!("dialog.sftpFilenameEncodingFollowTerminal").to_string()
                } else {
                    value.to_string()
                };
                ConnectionEditorChoice::new(
                    Some(value.to_string()),
                    label,
                    (value == "terminal"
                        && (editor.sftp_filename_encoding.trim().is_empty()
                            || ["terminal", "global"].iter().any(|follow| {
                                editor
                                    .sftp_filename_encoding
                                    .trim()
                                    .eq_ignore_ascii_case(follow)
                            })))
                        || editor.sftp_filename_encoding == value
                        || nyaterm_core::character_encoding::CharacterEncoding::parse(
                            &editor.sftp_filename_encoding,
                        )
                        .is_ok_and(|encoding| encoding.label() == value),
                )
            })
            .collect::<Vec<_>>();
        let ssh_algorithm_mode_options = [
            ("compatible", t!("dialog.algorithmModeCompatible")),
            ("secure", t!("dialog.algorithmModeSecure")),
            ("custom", t!("dialog.algorithmModeCustom")),
        ]
        .into_iter()
        .map(|(value, label)| {
            ConnectionEditorChoice::new(
                Some(value.to_string()),
                label,
                editor.ssh_algorithm_mode == value,
            )
        })
        .collect::<Vec<_>>();
        let mut serial_port_options = Vec::new();
        if !editor.serial_port.is_empty()
            && !self
                .connection_state
                .serial_ports()
                .contains(&editor.serial_port)
        {
            serial_port_options.push(ConnectionEditorChoice::new(
                Some(editor.serial_port.clone()),
                editor.serial_port.clone(),
                true,
            ));
        }
        serial_port_options.extend(self.connection_state.serial_ports().iter().map(|port| {
            ConnectionEditorChoice::new(
                Some(port.clone()),
                port.clone(),
                editor.serial_port == *port,
            )
        }));
        let baud_options = [
            "9600", "19200", "38400", "57600", "115200", "230400", "460800", "921600",
        ]
        .into_iter()
        .map(|value| {
            ConnectionEditorChoice::new(
                Some(value.to_string()),
                value.to_string(),
                editor.baud_rate == value,
            )
        })
        .collect::<Vec<_>>();
        let data_bits_options = ["5", "6", "7", "8"]
            .into_iter()
            .map(|value| {
                ConnectionEditorChoice::new(
                    Some(value.to_string()),
                    value.to_string(),
                    editor.data_bits == value,
                )
            })
            .collect::<Vec<_>>();
        let parity_options = [
            ("none", t!("dialog.parityNone")),
            ("odd", t!("dialog.parityOdd")),
            ("even", t!("dialog.parityEven")),
            ("mark", t!("dialog.parityMark")),
            ("space", t!("dialog.paritySpace")),
        ]
        .into_iter()
        .map(|(value, label)| {
            ConnectionEditorChoice::new(Some(value.to_string()), label, editor.parity == value)
        })
        .collect::<Vec<_>>();
        let flow_control_options = [
            ("none", t!("dialog.serialFlowControlNone")),
            ("software", t!("dialog.serialFlowControlSoftware")),
            ("hardware", t!("dialog.serialFlowControlHardware")),
        ]
        .into_iter()
        .map(|(value, label)| {
            ConnectionEditorChoice::new(
                Some(value.to_string()),
                label,
                editor.flow_control.as_str() == value,
            )
        })
        .collect::<Vec<_>>();
        let stop_bits_options = ["1", "1.5", "2"]
            .into_iter()
            .map(|value| {
                ConnectionEditorChoice::new(
                    Some(value.to_string()),
                    value.to_string(),
                    editor.stop_bits == value,
                )
            })
            .collect::<Vec<_>>();
        let policy_options = [
            ConnectionEditorChoice::new(
                Some("allowlist".to_string()),
                t!("dialog.sshAgentPolicyAllowlist"),
                matches!(
                    editor.agent_forwarding_config.policy,
                    nyaterm_core::SshAgentForwardingPolicy::Allowlist { .. }
                ),
            ),
            ConnectionEditorChoice::new(
                Some("all".to_string()),
                t!("dialog.sshAgentPolicyAll"),
                matches!(
                    editor.agent_forwarding_config.policy,
                    nyaterm_core::SshAgentForwardingPolicy::All
                ),
            ),
        ];
        let ssh_agent_endpoint_options = Self::ssh_agent_endpoint_choices(&editor.agent_endpoint);
        let shell_label = match editor.shell_path.as_str() {
            "powershell.exe" => t!("dialog.shellPowerShell"),
            "cmd.exe" => t!("dialog.shellCmd"),
            "bash" => t!("dialog.shellBash"),
            "wsl.exe" => t!("dialog.shellWsl"),
            "wt.exe" => t!("dialog.shellWindowsTerminal"),
            _ => t!("dialog.shellCustom"),
        };
        let shell_options = [
            ("powershell.exe", t!("dialog.shellPowerShell")),
            ("cmd.exe", t!("dialog.shellCmd")),
            ("bash", t!("dialog.shellBash")),
            ("wsl.exe", t!("dialog.shellWsl")),
            ("wt.exe", t!("dialog.shellWindowsTerminal")),
        ]
        .into_iter()
        .map(|(value, label)| {
            ConnectionEditorChoice::new(Some(value.to_string()), label, editor.shell_path == value)
        })
        .collect::<Vec<_>>();
        let serial_port_placeholder = if self.connection_state.serial_ports_loading() {
            t!("dialog.loadingSerialPorts").to_string()
        } else if let Some(error) = self.connection_state.serial_ports_error() {
            format!("{}: {error}", t!("dialog.serialPortsLoadFailed"))
        } else if self.connection_state.serial_ports().is_empty() {
            t!("dialog.noSerialPortsFound").to_string()
        } else {
            t!("dialog.selectSerialPort").to_string()
        };
        let mut selects = HashMap::new();
        for (select_key, choices, placeholder) in [
            (
                ConnectionEditorSelect::SavedPassword,
                password_options.as_slice(),
                password_label.clone(),
            ),
            (
                ConnectionEditorSelect::Account,
                account_options.as_slice(),
                account_placeholder,
            ),
            (
                ConnectionEditorSelect::SshKey,
                key_options.as_slice(),
                key_label.clone(),
            ),
            (
                ConnectionEditorSelect::Otp,
                otp_options.as_slice(),
                otp_label.clone(),
            ),
            (
                ConnectionEditorSelect::Proxy,
                proxy_options.as_slice(),
                proxy_label.clone(),
            ),
            (
                ConnectionEditorSelect::ProxyJump,
                jump_options.as_slice(),
                jump_label.clone(),
            ),
            (
                ConnectionEditorSelect::Backspace,
                backspace_options.as_slice(),
                String::new(),
            ),
            (
                ConnectionEditorSelect::Encoding,
                encoding_options.as_slice(),
                t!("connection.encodingFollowGlobal").to_string(),
            ),
            (
                ConnectionEditorSelect::SftpCwdFollowMode,
                sftp_cwd_options.as_slice(),
                String::new(),
            ),
            (
                ConnectionEditorSelect::SftpPipelineDepth,
                sftp_pipeline_options.as_slice(),
                t!("dialog.sftpPipelineAutomatic").to_string(),
            ),
            (
                ConnectionEditorSelect::SftpFilenameEncoding,
                sftp_filename_encoding_options.as_slice(),
                t!("dialog.sftpFilenameEncodingFollowTerminal").to_string(),
            ),
            (
                ConnectionEditorSelect::SshAlgorithmMode,
                ssh_algorithm_mode_options.as_slice(),
                String::new(),
            ),
            (
                ConnectionEditorSelect::SshAgentEndpoint,
                ssh_agent_endpoint_options.as_slice(),
                t!("dialog.sshAgentAuto").to_string(),
            ),
            (
                ConnectionEditorSelect::SshAgentForwardingPolicy,
                policy_options.as_slice(),
                t!("dialog.sshAgentPolicyAllowlist").to_string(),
            ),
            (
                ConnectionEditorSelect::SshProfile,
                ssh_profile_options.as_slice(),
                String::new(),
            ),
            (
                ConnectionEditorSelect::SshTerminalType,
                ssh_terminal_options.as_slice(),
                String::new(),
            ),
            (
                ConnectionEditorSelect::RdpBackend,
                rdp_backend_options.as_slice(),
                String::new(),
            ),
            (
                ConnectionEditorSelect::RdpCertificatePolicy,
                rdp_certificate_options.as_slice(),
                String::new(),
            ),
            (
                ConnectionEditorSelect::RdpDisplayMode,
                rdp_display_options.as_slice(),
                String::new(),
            ),
            (
                ConnectionEditorSelect::RdpClipboardMode,
                rdp_clipboard_options.as_slice(),
                String::new(),
            ),
            (
                ConnectionEditorSelect::VncSecurityMode,
                vnc_security_options.as_slice(),
                String::new(),
            ),
            (
                ConnectionEditorSelect::VncScaleMode,
                vnc_scale_options.as_slice(),
                String::new(),
            ),
            (
                ConnectionEditorSelect::RecordingMode,
                recording_mode_options.as_slice(),
                String::new(),
            ),
            (
                ConnectionEditorSelect::TelnetEnterMode,
                telnet_enter_options.as_slice(),
                "CR (\\r)".to_string(),
            ),
            (
                ConnectionEditorSelect::Shell,
                shell_options.as_slice(),
                shell_label.to_string(),
            ),
            (
                ConnectionEditorSelect::SerialPort,
                serial_port_options.as_slice(),
                serial_port_placeholder,
            ),
            (
                ConnectionEditorSelect::BaudRate,
                baud_options.as_slice(),
                t!("dialog.customBaudRate").to_string(),
            ),
            (
                ConnectionEditorSelect::DataBits,
                data_bits_options.as_slice(),
                String::new(),
            ),
            (
                ConnectionEditorSelect::Parity,
                parity_options.as_slice(),
                String::new(),
            ),
            (
                ConnectionEditorSelect::FlowControl,
                flow_control_options.as_slice(),
                String::new(),
            ),
            (
                ConnectionEditorSelect::StopBits,
                stop_bits_options.as_slice(),
                String::new(),
            ),
        ] {
            let select = self.connection_editor_select_entity(select_key, choices, placeholder, cx);
            let disabled = matches!(
                select_key,
                ConnectionEditorSelect::SftpCwdFollowMode
                    | ConnectionEditorSelect::SftpFilenameEncoding
                    | ConnectionEditorSelect::SftpPipelineDepth
            ) && !editor.sftp_enabled
                || select_key == ConnectionEditorSelect::SavedPassword && password_disabled
                || select_key == ConnectionEditorSelect::SerialPort
                    && self.connection_state.serial_ports_loading();
            select.update(cx, |select, cx| select.set_disabled(disabled, cx));
            selects.insert(select_key, select);
        }
        let forwarding_endpoint_selects = editor
            .agent_forwarding_config
            .sources
            .external_agent_endpoints
            .iter()
            .enumerate()
            .map(|(index, endpoint)| {
                let choices = Self::ssh_agent_endpoint_choices(endpoint);
                let options = Self::connection_editor_select_options(&choices);
                let selected_value = Some(Self::ssh_agent_endpoint_value(endpoint).to_string());
                let select = self.select_entity(
                    format!("connection-editor-ssh-agent-forwarding-endpoint-{index}"),
                    options,
                    selected_value,
                    false,
                    cx,
                );
                (index, select)
            })
            .collect();
        let forwarding_endpoint_fields = self
            .connection_state
            .editor_forwarding_endpoint_fields()
            .clone();
        let mut fields = ConnectionEditorFields::new(
            self.connection_state.editor_fields().clone(),
            self.connection_state.editor_number_fields().clone(),
            selects,
            forwarding_endpoint_selects,
            forwarding_endpoint_fields,
        );
        let targets = self
            .connection_state
            .editor_fields()
            .keys()
            .chain(self.connection_state.editor_number_fields().keys())
            .copied()
            .map(ConnectionEditorFocusTarget::Field)
            .chain(
                connection_editor_select_keys()
                    .into_iter()
                    .map(ConnectionEditorFocusTarget::Select),
            )
            .chain(
                self.connection_state
                    .editor_forwarding_endpoint_fields()
                    .keys()
                    .map(
                        |(index, field)| ConnectionEditorFocusTarget::ForwardingEndpoint {
                            index: *index,
                            field: *field,
                        },
                    ),
            )
            .collect::<Vec<_>>();
        fields.anchors = targets
            .into_iter()
            .map(|target| (target, self.connection_state.editor_anchor(target)))
            .collect();
        let scroll = self.connection_state.editor_scroll();
        let heading_anchor = self
            .connection_state
            .editor_anchor(ConnectionEditorFocusTarget::Heading);
        let account_id = if matches!(
            editor.kind,
            ConnectionKindTab::Ssh | ConnectionKindTab::Telnet
        ) {
            editor.account_id.as_deref()
        } else {
            editor.password_id.as_deref()
        };
        fields.account = account_id
            .and_then(|id| {
                self.security
                    .passwords()
                    .iter()
                    .find(|entry| entry.id == id)
            })
            .cloned();
        fields.account_password_locked =
            self.settings.summary().has_master_password && !self.security.secrets_unlocked();
        fields.errors = editor.field_errors.clone();
        let icon_key = editor.icon.as_deref();
        let icon_def = resolve_connection_icon(icon_key, editor.kind.label());
        let icon_picker_open = self.connection_state.editor_icon_picker_is_open();
        let group_select_open = self.connection_state.editor_group_select_is_open();
        let agent_identity_picker_open =
            self.connection_state.editor_agent_identity_picker_is_open();
        let credential_overlay = self.connection_state.editor_credential_overlay();
        let baud_popover_open = self.connection_state.editor_baud_popover_is_open();
        let validation_error = self.connection_editor_validation_error(&editor);
        let save_enabled = !self.connection_state.editor_save_in_flight();
        let width = if native_window {
            viewport.0
        } else {
            (viewport.0 - 26.).min(900.)
        };
        let height = if native_window {
            viewport.1
        } else {
            (viewport.1 - 26.).min(680.)
        };
        let wide = width >= 760.;
        let navigation_width = if wide { 180. } else { 128. };
        let narrow = width - navigation_width < 500.;
        let protocol_select = self.select_entity(
            "connection-editor-kind",
            [
                ConnectionKindTab::Ssh,
                ConnectionKindTab::Local,
                ConnectionKindTab::Telnet,
                ConnectionKindTab::Serial,
                ConnectionKindTab::Rdp,
                ConnectionKindTab::Vnc,
            ]
            .into_iter()
            .map(|kind| {
                NyaSelectOption::new(
                    kind.label(),
                    match kind {
                        ConnectionKindTab::Local => t!("dialog.localTerminal"),
                        ConnectionKindTab::Serial => t!("dialog.serial"),
                        _ => Cow::Borrowed(kind.label()),
                    },
                )
            })
            .collect(),
            Some(editor.kind.label().to_string()),
            false,
            cx,
        );
        let section_focus = self.connection_state.editor_section_focus();
        let section_context = ConnectionEditorSectionContext {
            palette,
            editor: &editor,
            fields: &fields,
            baud_popover_open,
            narrow,
        };
        let custom_icons = self
            .connection_state
            .custom_icons
            .records
            .iter()
            .filter_map(|record| {
                self.connection_state
                    .custom_icons
                    .images
                    .get(&record.id)
                    .cloned()
                    .map(|image| (record.clone(), image))
            })
            .collect::<Vec<_>>();
        let icon_rows = (CONNECTION_ICON_OPTIONS.len() + custom_icons.len()).div_ceil(7);
        // Every icon cell is 28 px, with a 4 px gap between rows.
        let icon_grid_height = icon_rows * 28 + icon_rows.saturating_sub(1) * 4;
        let icon_viewport_height = icon_grid_height.min(280) as f32;
        let mut icon_grid = div()
            .grid()
            .grid_cols(7)
            .gap_1()
            .pr_4()
            .debug_selector(|| "connection-editor-icon-grid".to_string());
        for (index, icon_key) in CONNECTION_ICON_OPTIONS.iter().copied().enumerate() {
            let icon = resolve_connection_icon(Some(icon_key), editor.kind.label());
            let selected = editor.icon.as_deref().unwrap_or(DEFAULT_CONNECTION_ICON) == icon_key;
            icon_grid = icon_grid.child(
                div()
                    .id(SharedString::from(format!("connection-icon-{icon_key}")))
                    .size(px(28.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_sm()
                    .cursor_pointer()
                    .bg(if selected {
                        rgba((palette.primary << 8) | 0x26)
                    } else {
                        rgba(0x00000000)
                    })
                    .border_1()
                    .border_color(if selected {
                        rgb(palette.primary)
                    } else {
                        rgba(0x00000000)
                    })
                    .hover(|this| this.bg(rgb(palette.hover)))
                    .when(index == 6, |this| {
                        this.debug_selector(|| "connection-editor-icon-rightmost".to_string())
                    })
                    .child(themed_icon(palette, icon, false, 16.))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.set_connection_editor_icon(Some(icon_key), cx);
                    })),
            );
        }
        for (index, (record, image)) in custom_icons.into_iter().enumerate() {
            let id = record.id.clone();
            let selected = editor.icon.as_deref() == Some(record.id.as_str());
            icon_grid = icon_grid.child(
                div()
                    .id(SharedString::from(format!("custom-icon-{}", record.id)))
                    .size(px(28.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_sm()
                    .cursor_pointer()
                    .bg(if selected {
                        rgba((palette.primary << 8) | 0x26)
                    } else {
                        rgba(0x00000000)
                    })
                    .border_1()
                    .border_color(if selected {
                        rgb(palette.primary)
                    } else {
                        rgba(0x00000000)
                    })
                    .hover(|this| this.bg(rgb(palette.hover)))
                    .when(index == 0, |this| {
                        this.debug_selector(|| "connection-editor-custom-icon-first".to_string())
                    })
                    .tooltip(move |window, cx| {
                        NyaTooltip::new(record.name.clone()).build(window, cx)
                    })
                    .child(
                        div()
                            .size(px(16.))
                            .when(index == 0, |this| {
                                this.debug_selector(|| {
                                    "connection-editor-custom-image-first".to_string()
                                })
                            })
                            .child(gpui::img(image).size(px(16.))),
                    )
                    .on_click(cx.listener(move |app, _, _, cx| {
                        app.set_connection_editor_icon(Some(&id), cx)
                    })),
            );
        }
        let custom_selected = editor
            .icon
            .as_ref()
            .filter(|id| {
                self.connection_state
                    .custom_icons
                    .records
                    .iter()
                    .any(|record| &record.id == *id)
            })
            .cloned();
        let selected_image = editor
            .icon
            .as_ref()
            .and_then(|id| self.connection_state.custom_icons.images.get(id))
            .cloned();
        let icon_picker_trigger = div()
            .id("connection-editor-icon-trigger")
            .size(px(32.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_sm()
            .border_1()
            .border_color(if icon_picker_open {
                rgb(palette.primary)
            } else {
                rgb(palette.border)
            })
            .bg(rgb(palette.input))
            .cursor_pointer()
            .hover(|this| this.bg(rgb(palette.hover)))
            .child(if let Some(image) = selected_image {
                gpui::img(image).size(px(17.)).into_any_element()
            } else {
                themed_icon(palette, icon_def, false, 17.).into_any_element()
            });
        let icon_picker_content = div()
            .debug_selector(|| "connection-editor-icon-popover-content".to_string())
            .occlude()
            .flex()
            .flex_col()
            // Seven 28 px icons plus gaps and a separate 16 px scrollbar track.
            .w(px(256.))
            .p_2()
            .rounded_md()
            .border_1()
            .border_color(rgb(palette.border))
            .bg(rgb(palette.surface))
            .shadow_lg()
            .child(
                div()
                    .h(px(icon_viewport_height))
                    .overflow_y_scrollbar()
                    .child(icon_grid),
            )
            .child(
                nyaterm_ui::NyaButton::new("import-custom-icon", t!("dialog.importCustomIcon"))
                    .on_click(cx.listener(|app, _, window, cx| {
                        app.import_connection_custom_icon(window, cx)
                    })),
            )
            .when_some(custom_selected, |this, id| {
                this.child(
                    nyaterm_ui::NyaButton::new("delete-custom-icon", t!("dialog.deleteCustomIcon"))
                        .on_click(cx.listener(move |app, _, _, cx| {
                            app.delete_connection_custom_icon(id.clone(), cx)
                        })),
                )
            })
            .when(editor.kind == ConnectionKindTab::Ssh, |this| {
                this.child(
                    div()
                        .id("connection-editor-icon-auto-detect")
                        .debug_selector(|| "connection-editor-icon-auto-detect".to_string())
                        .mt_2()
                        .pt_2()
                        .border_t_1()
                        .border_color(rgb(palette.border))
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .opacity(if icon_auto_detect_enabled { 1.0 } else { 0.55 })
                        .when(icon_auto_detect_enabled, |this| {
                            this.cursor_pointer()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.set_connection_editor_icon_auto_detect(
                                        !icon_auto_detect,
                                        cx,
                                    );
                                }))
                        })
                        .when(!icon_auto_detect_enabled, |this| {
                            let tooltip = icon_auto_detect_hint.clone();
                            this.tooltip(move |window, cx| {
                                NyaTooltip::new(tooltip.clone()).build(window, cx)
                            })
                        })
                        .child(
                            div()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(palette.text))
                                        .child(icon_auto_detect_label),
                                )
                                .child(
                                    div()
                                        .text_size(px(10.))
                                        .text_color(rgb(palette.text_dimmed))
                                        .child(icon_auto_detect_hint),
                                ),
                        )
                        .child(
                            NyaSwitch::new("connection-editor-icon-auto-detect-switch")
                                .checked(icon_auto_detect)
                                .disabled(!icon_auto_detect_enabled),
                        ),
                )
            });
        let icon_picker = div()
            .flex_none()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(palette.text_muted))
                    .child(icon_label),
            )
            .child(
                NyaPopover::new(
                    "connection-editor-icon-popover",
                    icon_picker_trigger,
                    icon_picker_content,
                )
                .appearance(false)
                .open(icon_picker_open)
                .on_open_change(cx.listener(|this, open, _, cx| {
                    this.set_connection_icon_picker_open(*open, cx);
                })),
            );

        let content = div()
            .id(SharedString::from(format!(
                "connection-editor-scroll-{}",
                editor.section.id()
            )))
            .debug_selector(|| "connection-editor-scroll".to_string())
            .relative()
            .min_w_0()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&scroll)
            .on_scroll_wheel(cx.listener(|this, _: &gpui::ScrollWheelEvent, window, cx| {
                if this.connection_editor_select_menu_is_focused(window, cx) {
                    cx.stop_propagation();
                }
            }))
            .p(px(20.))
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .id("connection-editor-section-heading")
                    .anchor_scroll(Some(heading_anchor))
                    .track_focus(&section_focus)
                    .tab_index(-1)
                    .debug_selector(|| "connection-editor-section-title".to_string())
                    .text_size(px(15.))
                    .font_weight(FontWeight(600.))
                    .child(editor.section.label()),
            )
            .when(editor.section == ConnectionEditorSection::Basic, |this| {
                this.child(
                    div()
                        .flex()
                        .when(!narrow, |this| this.flex_wrap())
                        .when(narrow, |this| this.flex_col())
                        .items_end()
                        .when(narrow, |this| this.items_stretch())
                        .gap_3()
                        .child(icon_picker)
                        .child(div().min_w(px(192.)).flex_1().child(editor_field(
                            palette,
                            name_label,
                            ConnectionEditorField::Name,
                            &fields,
                            cx,
                        )))
                        .child(
                            div()
                                .min_w(px(192.))
                                .when(!narrow, |this| this.max_w(px(288.)))
                                .flex_1()
                                .child(connection_editor_group_control(
                                    ConnectionEditorGroupControlArgs {
                                        palette,
                                        label: group_title,
                                        display_label: connection_editor_group_display_label(
                                            self.connection_state.groups(),
                                            &editor,
                                            none_label,
                                            t!("dialog.newGroup"),
                                        ),
                                        select_open: group_select_open,
                                        trigger_bounds: self
                                            .connection_state
                                            .editor_group_select_trigger_bounds(),
                                        parent_hint: group_parent_hint.clone(),
                                        choices: &group_options,
                                        fields: &fields,
                                    },
                                    cx,
                                )),
                        ),
                )
            })
            .when(editor.kind == ConnectionKindTab::Ssh, |this| {
                this.child(connection_editor_ssh_section(
                    section_context,
                    SshConnectionSectionLabels {
                        otp: otp_label.clone(),
                        proxy: proxy_label.clone(),
                        jump: jump_label.clone(),
                    },
                    SshConnectionSectionOptions { auth: auth_options },
                    cx,
                ))
            })
            .when(editor.kind == ConnectionKindTab::Local, |this| {
                this.child(connection_editor_local_section(section_context, cx))
            })
            .when(editor.kind == ConnectionKindTab::Telnet, |this| {
                this.child(connection_editor_telnet_section(section_context, cx))
            })
            .when(editor.kind == ConnectionKindTab::Serial, |this| {
                this.child(connection_editor_serial_section(section_context, cx))
            })
            .when(editor.kind == ConnectionKindTab::Rdp, |this| {
                this.child(connection_editor_rdp_section(section_context, cx))
            })
            .when(editor.kind == ConnectionKindTab::Vnc, |this| {
                this.child(connection_editor_vnc_section(section_context, cx))
            })
            .when(editor.section == ConnectionEditorSection::Basic, |this| {
                this.child(tags::connection_tags_field(palette, &editor, &fields, cx))
            })
            .when(editor.section == ConnectionEditorSection::Basic, |this| {
                this.child(connection_description_field(
                    palette,
                    description_label,
                    &fields,
                    cx,
                ))
            });
        let card = layout::connection_editor_frame(
            layout::ConnectionEditorFrame {
                palette,
                editor: &editor,
                title,
                content: content.into_any_element(),
                navigation_focus: self.connection_state.editor_navigation_focus(),
                editor_focus: self.connection_state.editor_focus_handle(),
                scroll,
                native_window,
                height,
                wide,
                navigation_width,
                protocol_select,
                validation_error,
                save_enabled,
            },
            cx,
        );
        let surface = div()
            .relative()
            .w_full()
            .when(native_window, |this| this.size_full())
            .overflow_hidden()
            .bg(rgb(palette.bg))
            .child(card)
            .when(agent_identity_picker_open, |this| {
                this.child(connection_editor_agent_identity_picker(
                    palette, &editor, cx,
                ))
            })
            .when_some(credential_overlay, |this, overlay| {
                this.child(self.connection_editor_credential_manager(overlay, cx))
            });
        if native_window {
            surface.into_any_element()
        } else {
            modal_dialog_shell(
                palette,
                self.shell_surface_color(palette.bg),
                "connection-editor-modal",
                900.,
                surface,
            )
            .into_any_element()
        }
    }

    fn connection_editor_credential_manager(
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

impl NyaTermApp {
    pub(in crate::features) fn focus_connection_editor_select(
        &self,
        select: ConnectionEditorSelect,
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) -> bool {
        self.focus_select(connection_editor_select_id(select), window, cx)
    }

    pub(super) fn connection_editor_select_is_focused(
        &self,
        window: &gpui::Window,
        cx: &gpui::App,
    ) -> bool {
        self.select_is_focused("connection-editor-kind", window, cx)
            || connection_editor_select_keys().into_iter().any(|select| {
                self.select_is_focused(connection_editor_select_id(select), window, cx)
            })
            || self.select_with_prefix_is_focused(
                "connection-editor-ssh-agent-forwarding-endpoint-",
                window,
                cx,
            )
    }

    pub(super) fn connection_editor_select_menu_is_focused(
        &self,
        window: &gpui::Window,
        cx: &gpui::App,
    ) -> bool {
        self.select_menu_is_focused("connection-editor-kind", window, cx)
            || connection_editor_select_keys().into_iter().any(|select| {
                self.select_menu_is_focused(connection_editor_select_id(select), window, cx)
            })
            || self.select_menu_with_prefix_is_focused(
                "connection-editor-ssh-agent-forwarding-endpoint-",
                window,
                cx,
            )
    }
}

fn connection_editor_agent_identity_picker(
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

fn connection_editor_select_keys() -> [ConnectionEditorSelect; 32] {
    [
        ConnectionEditorSelect::Group,
        ConnectionEditorSelect::SavedPassword,
        ConnectionEditorSelect::Account,
        ConnectionEditorSelect::SshKey,
        ConnectionEditorSelect::Otp,
        ConnectionEditorSelect::Proxy,
        ConnectionEditorSelect::ProxyJump,
        ConnectionEditorSelect::Backspace,
        ConnectionEditorSelect::Encoding,
        ConnectionEditorSelect::SftpCwdFollowMode,
        ConnectionEditorSelect::SftpPipelineDepth,
        ConnectionEditorSelect::SftpFilenameEncoding,
        ConnectionEditorSelect::SshAlgorithmMode,
        ConnectionEditorSelect::SshAgentEndpoint,
        ConnectionEditorSelect::SshAgentForwardingPolicy,
        ConnectionEditorSelect::SshProfile,
        ConnectionEditorSelect::SshTerminalType,
        ConnectionEditorSelect::RdpBackend,
        ConnectionEditorSelect::RdpCertificatePolicy,
        ConnectionEditorSelect::RdpDisplayMode,
        ConnectionEditorSelect::RdpClipboardMode,
        ConnectionEditorSelect::VncSecurityMode,
        ConnectionEditorSelect::VncScaleMode,
        ConnectionEditorSelect::RecordingMode,
        ConnectionEditorSelect::TelnetEnterMode,
        ConnectionEditorSelect::Shell,
        ConnectionEditorSelect::SerialPort,
        ConnectionEditorSelect::BaudRate,
        ConnectionEditorSelect::DataBits,
        ConnectionEditorSelect::Parity,
        ConnectionEditorSelect::StopBits,
        ConnectionEditorSelect::FlowControl,
    ]
}

fn connection_editor_select_id(select: ConnectionEditorSelect) -> &'static str {
    match select {
        ConnectionEditorSelect::Authentication => {
            unreachable!("authentication uses connection-specific segmented tabs")
        }
        ConnectionEditorSelect::SshAgentEndpoint => "connection-editor-ssh-agent-endpoint",
        ConnectionEditorSelect::SshAgentForwardingPolicy => "connection-editor-ssh-agent-policy",
        ConnectionEditorSelect::Group => "connection-editor-group-select",
        ConnectionEditorSelect::SavedPassword => "connection-editor-saved-password",
        ConnectionEditorSelect::Account => "connection-editor-account",
        ConnectionEditorSelect::SshKey => "connection-editor-ssh-key",
        ConnectionEditorSelect::Otp => "connection-editor-otp",
        ConnectionEditorSelect::Proxy => "connection-editor-proxy",
        ConnectionEditorSelect::ProxyJump => "connection-editor-proxy-jump",
        ConnectionEditorSelect::Backspace => "connection-editor-backspace",
        ConnectionEditorSelect::Encoding => "connection-editor-encoding",
        ConnectionEditorSelect::SftpCwdFollowMode => "connection-editor-sftp-cwd-follow",
        ConnectionEditorSelect::SftpPipelineDepth => "connection-editor-sftp-pipeline-depth",
        ConnectionEditorSelect::SftpFilenameEncoding => "connection-editor-sftp-filename-encoding",
        ConnectionEditorSelect::SshAlgorithmMode => "connection-editor-ssh-algorithm-mode",
        ConnectionEditorSelect::SshProfile => "connection-editor-ssh-profile",
        ConnectionEditorSelect::SshTerminalType => "connection-editor-ssh-terminal-type",
        ConnectionEditorSelect::RdpBackend => "connection-editor-rdp-backend",
        ConnectionEditorSelect::RdpCertificatePolicy => "connection-editor-rdp-certificate-policy",
        ConnectionEditorSelect::RdpDisplayMode => "connection-editor-rdp-display-mode",
        ConnectionEditorSelect::RdpClipboardMode => "connection-editor-rdp-clipboard-mode",
        ConnectionEditorSelect::VncSecurityMode => "connection-editor-vnc-security-mode",
        ConnectionEditorSelect::VncScaleMode => "connection-editor-vnc-scale-mode",
        ConnectionEditorSelect::RecordingMode => "connection-editor-recording-mode",
        ConnectionEditorSelect::TelnetEnterMode => "connection-editor-telnet-enter-mode",
        ConnectionEditorSelect::Shell => "connection-editor-shell",
        ConnectionEditorSelect::SerialPort => "connection-editor-serial-port",
        ConnectionEditorSelect::BaudRate => "connection-editor-baud-rate",
        ConnectionEditorSelect::DataBits => "connection-editor-data-bits",
        ConnectionEditorSelect::Parity => "connection-editor-parity",
        ConnectionEditorSelect::StopBits => "connection-editor-stop-bits",
        ConnectionEditorSelect::FlowControl => "connection-editor-flow-control",
    }
}

fn connection_editor_select_is_searchable(select: ConnectionEditorSelect) -> bool {
    matches!(
        select,
        ConnectionEditorSelect::SavedPassword
            | ConnectionEditorSelect::Account
            | ConnectionEditorSelect::SshKey
            | ConnectionEditorSelect::Otp
            | ConnectionEditorSelect::Proxy
            | ConnectionEditorSelect::ProxyJump
    )
}

fn connection_editor_select_search_placeholder(select: ConnectionEditorSelect) -> Option<String> {
    match select {
        ConnectionEditorSelect::SavedPassword => Some(t!("dialog.selectPassword").to_string()),
        ConnectionEditorSelect::Account => Some(t!("dialog.account").to_string()),
        ConnectionEditorSelect::SshKey => Some(t!("dialog.privateKey").to_string()),
        ConnectionEditorSelect::Otp => Some(t!("dialog.searchOtpEntries").to_string()),
        ConnectionEditorSelect::Proxy => Some(t!("network.searchProxies").to_string()),
        ConnectionEditorSelect::ProxyJump => Some(t!("network.searchConnections").to_string()),
        ConnectionEditorSelect::SshAgentForwardingPolicy => None,
        _ => None,
    }
}

fn connection_group_path_label(groups: &[Group], group_id: &str) -> Option<String> {
    let mut parts = Vec::new();
    let mut next = Some(group_id);
    let mut seen = HashSet::new();
    while let Some(id) = next {
        if !seen.insert(id.to_string()) {
            break;
        }
        let group = groups.iter().find(|group| group.id == id)?;
        parts.push(group.name.clone());
        next = group.parent_id.as_deref();
    }
    parts.reverse();
    Some(parts.join(" / "))
}

struct ConnectionEditorGroupControlArgs<'a> {
    palette: crate::theme::ThemePalette,
    label: Cow<'static, str>,
    display_label: String,
    select_open: bool,
    trigger_bounds: Option<Bounds<Pixels>>,
    parent_hint: String,
    choices: &'a [ConnectionEditorGroupMenuOption],
    fields: &'a ConnectionEditorFields,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ConnectionEditorGroupMenuOption {
    value: Option<String>,
    label: String,
    depth: usize,
    selected: bool,
    disabled: bool,
    none_option: bool,
}

fn connection_editor_group_control(
    args: ConnectionEditorGroupControlArgs<'_>,
    cx: &mut Context<NyaTermApp>,
) -> impl IntoElement {
    let ConnectionEditorGroupControlArgs {
        palette,
        label,
        display_label,
        select_open,
        trigger_bounds,
        parent_hint,
        choices,
        fields,
    } = args;
    let new_group_entity = fields.get(&ConnectionEditorField::NewGroupName);
    let new_group_field = new_group_entity.cloned();
    let new_group_focused = new_group_entity.is_some_and(|field| field.read(cx).has_focus());
    let can_add = new_group_entity.is_some_and(|field| !field.read(cx).value(cx).trim().is_empty());
    let popup_position = trigger_bounds.map(connection_editor_group_popup_position);
    let popup_width = trigger_bounds.map(|bounds| bounds.size.width.max(px(192.)));
    let app = cx.entity();

    div()
        .id("connection-editor-group")
        .relative()
        .min_w_0()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .text_xs()
                .text_color(rgb(palette.text_muted))
                .child(label),
        )
        .child(
            div()
                .on_children_prepainted({
                    let app = app.clone();
                    move |bounds, _, cx| {
                        let Some(bounds) = bounds.into_iter().next() else {
                            return;
                        };
                        app.update(cx, |this, cx| {
                            this.set_connection_group_select_trigger_bounds(bounds, cx);
                        });
                    }
                })
                .id("connection-editor-group-row")
                .h(px(EDITOR_CONTROL_HEIGHT_PX))
                .min_w_0()
                .flex()
                .items_center()
                .child(
                    div()
                        .id("connection-editor-group-trigger")
                        .h(px(EDITOR_CONTROL_HEIGHT_PX))
                        .min_w_0()
                        .flex_1()
                        .px_2()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .rounded_sm()
                        .border_1()
                        .border_color(if select_open {
                            rgb(palette.primary)
                        } else {
                            rgb(palette.border)
                        })
                        .bg(rgb(palette.input))
                        .cursor_pointer()
                        .hover(|this| this.bg(rgb(palette.hover)))
                        .child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .truncate()
                                .text_xs()
                                .text_color(rgb(palette.text))
                                .child(display_label),
                        )
                        .child(
                            svg()
                                .size(px(14.))
                                .path("icons/chevron-down.svg")
                                .text_color(rgb(palette.text_muted)),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.toggle_connection_group_select(cx);
                        })),
                ),
        )
        .when(select_open, |this| {
            let Some(popup_position) = popup_position else {
                return this;
            };
            let popup_width = popup_width.unwrap_or(px(192.));
            this.child(
                deferred(
                    anchored()
                        .snap_to_window_with_margin(px(8.))
                        .position(popup_position)
                        .child(
                            div()
                                .occlude()
                                .w(popup_width)
                                .rounded_md()
                                .border_1()
                                .border_color(rgb(palette.border))
                                .bg(rgb(palette.surface_elevated))
                                .shadow_lg()
                                .overflow_hidden()
                                .flex()
                                .flex_col()
                                .child(
                                    div().flex_none().w_full().max_h(px(192.)).bg(rgb(
                                        palette.surface_elevated,
                                    )).child(
                                        NyaScrollArea::new("connection-editor-group-options")
                                            .max_h(px(192.))
                                            .child(connection_editor_group_option_list(
                                                palette, choices, cx,
                                            )),
                                    ),
                                )
                                .child(
                                    div()
                                        .id("connection-editor-new-group-footer")
                                        .flex_none()
                                        .p_2()
                                        .border_t_1()
                                        .border_color(rgb(palette.border))
                                        .flex()
                                        .flex_col()
                                        .gap_1()
                                        .child(
                                            div()
                                                .h(px(32.))
                                                .min_w_0()
                                                .flex()
                                                .items_center()
                                                .gap_2()
                                                .child(
                                                    div()
                                                        .id("connection-editor-new-group-input")
                                                        .h(px(32.))
                                                        .min_w_0()
                                                        .flex_1()
                                                        .px(px(ORDINARY_INPUT_SHELL_PADDING_X_PX))
                                                        .flex()
                                                        .items_center()
                                                        .rounded_sm()
                                                        .border_1()
                                                        .border_color(
                                                            ordinary_input_shell_border_color(
                                                                palette,
                                                                new_group_focused,
                                                            ),
                                                        )
                                                        .when(new_group_focused, |this| {
                                                            this.shadow(
                                                                ordinary_input_focus_ring(palette),
                                                            )
                                                        })
                                                        .bg(rgb(palette.input))
                                                        .children(new_group_field.map(|field| {
                                                            div()
                                                                .min_w_0()
                                                                .flex_1()
                                                                .text_xs()
                                                                .child(NyaInput::new(&field))
                                                        })),
                                                )
                                                .child(
                                                    div()
                                                        .id("connection-editor-new-group-add")
                                                        .size(px(32.))
                                                        .flex_none()
                                                        .flex()
                                                        .items_center()
                                                        .justify_center()
                                                        .rounded_sm()
                                                        .border_1()
                                                        .border_color(rgb(palette.border))
                                                        .bg(rgb(palette.input))
                                                        .text_color(rgb(palette.text_muted))
                                                        .opacity(if can_add { 1.0 } else { 0.45 })
                                                        .when(can_add, |this| {
                                                            this.cursor_pointer()
                                                                .hover(|this| {
                                                                    this.bg(rgb(palette.hover))
                                                                })
                                                                .on_click(cx.listener(
                                                                    |this, _, _, cx| {
                                                                        this.commit_connection_editor_new_group(cx);
                                                                    },
                                                                ))
                                                        })
                                                        .child(
                                                            svg()
                                                                .size(px(15.))
                                                                .path("icons/conn/add.svg")
                                                                .text_color(rgb(
                                                                    palette.text_muted,
                                                                )),
                                                        ),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .min_w_0()
                                                .truncate()
                                                .text_size(px(10.))
                                                .text_color(rgb(palette.text_dimmed))
                                                .child(parent_hint),
                                        ),
                                )
                                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                    this.close_connection_group_select(cx);
                                })),
                        ),
                )
                .with_priority(1),
            )
        })
}

fn connection_editor_group_option_list(
    palette: crate::theme::ThemePalette,
    choices: &[ConnectionEditorGroupMenuOption],
    cx: &mut Context<NyaTermApp>,
) -> impl IntoElement {
    choices
        .iter()
        .fold(div().w_full().flex().flex_col(), |list, choice| {
            let selected = choice.selected;
            let value = choice.value.clone();
            let disabled = choice.disabled;
            let none_option = choice.none_option;
            let padding_left = px(12. + (choice.depth as f32 * 16.));
            let row_id = value
                .as_deref()
                .map(|id| format!("connection-editor-group-option-{id}"))
                .unwrap_or_else(|| "connection-editor-group-option-none".to_string());
            list.child(
                div()
                    .id(SharedString::from(row_id))
                    .w_full()
                    .h(px(28.))
                    .pl(padding_left)
                    .pr_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .bg(if selected {
                        rgba((palette.primary << 8) | 0x26)
                    } else {
                        rgba(0x00000000)
                    })
                    .text_color(if selected {
                        rgb(palette.primary)
                    } else if none_option || disabled {
                        rgb(palette.text_muted)
                    } else {
                        rgb(palette.text)
                    })
                    .when(!disabled, |this| {
                        this.cursor_pointer()
                            .hover(|this| this.bg(rgb(palette.hover)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.set_connection_editor_select_value(
                                    ConnectionEditorSelect::Group,
                                    value.as_deref(),
                                    cx,
                                );
                            }))
                    })
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .truncate()
                            .text_xs()
                            .child(choice.label.clone()),
                    ),
            )
        })
}

fn connection_editor_group_popup_position(bounds: Bounds<Pixels>) -> Point<Pixels> {
    Point {
        x: bounds.origin.x,
        y: bounds.origin.y + bounds.size.height + px(4.),
    }
}

fn connection_editor_group_display_label(
    groups: &[Group],
    editor: &ConnectionEditorState,
    none_label: impl Into<SharedString>,
    new_group_label: impl Into<SharedString>,
) -> String {
    let none_label: SharedString = none_label.into();
    let new_group_label: SharedString = new_group_label.into();
    if let Some(name) = editor.pending_group_name.as_deref() {
        return format!("{name} ({new_group_label})");
    }
    editor
        .group_id
        .as_deref()
        .and_then(|id| connection_group_path_label(groups, id))
        .unwrap_or_else(|| none_label.to_string())
}

fn connection_editor_group_menu_options(
    groups: &[Group],
    editor: &ConnectionEditorState,
    none_label: impl Into<SharedString>,
    new_group_label: impl Into<SharedString>,
) -> Vec<ConnectionEditorGroupMenuOption> {
    let none_label: SharedString = none_label.into();
    let new_group_label: SharedString = new_group_label.into();
    let mut options = vec![ConnectionEditorGroupMenuOption {
        value: None,
        label: none_label.to_string(),
        depth: 0,
        selected: editor.group_id.is_none() && editor.pending_group_name.is_none(),
        disabled: false,
        none_option: true,
    }];

    if let Some(name) = editor.pending_group_name.as_deref() {
        options.push(ConnectionEditorGroupMenuOption {
            value: Some(PENDING_CONNECTION_GROUP_VALUE.to_string()),
            label: format!("{name} ({new_group_label})"),
            depth: 0,
            selected: true,
            disabled: true,
            none_option: false,
        });
    }

    options.extend(
        ordered_connection_groups(groups)
            .into_iter()
            .map(|(group, depth)| {
                let selected = editor.pending_group_name.is_none()
                    && editor.group_id.as_deref() == Some(group.id.as_str());
                ConnectionEditorGroupMenuOption {
                    value: Some(group.id.clone()),
                    label: group.name,
                    depth,
                    selected,
                    disabled: false,
                    none_option: false,
                }
            }),
    );

    options
}

pub(in crate::features::pages::connections) fn ordered_connection_groups(
    groups: &[Group],
) -> Vec<(Group, usize)> {
    let group_ids = groups
        .iter()
        .map(|group| group.id.clone())
        .collect::<HashSet<_>>();
    let mut children = HashMap::<Option<String>, Vec<Group>>::new();
    for group in groups {
        let parent_id = group
            .parent_id
            .clone()
            .filter(|parent_id| parent_id != &group.id && group_ids.contains(parent_id));
        children.entry(parent_id).or_default().push(group.clone());
    }
    for siblings in children.values_mut() {
        siblings.sort_by(|left, right| {
            left.sort_order
                .cmp(&right.sort_order)
                // Same rule as the panel tree, so the dropdown and the list agree.
                .then_with(|| natural_compare(&left.name, &right.name))
                .then_with(|| left.id.cmp(&right.id))
        });
    }

    fn append_group(
        group: Group,
        depth: usize,
        children: &HashMap<Option<String>, Vec<Group>>,
        visited: &mut HashSet<String>,
        ordered: &mut Vec<(Group, usize)>,
    ) {
        if !visited.insert(group.id.clone()) {
            return;
        }
        let group_id = group.id.clone();
        ordered.push((group, depth));
        for child in children.get(&Some(group_id)).cloned().unwrap_or_default() {
            append_group(child, depth + 1, children, visited, ordered);
        }
    }

    let mut ordered = Vec::with_capacity(groups.len());
    let mut visited = HashSet::new();
    for group in children.get(&None).cloned().unwrap_or_default() {
        append_group(group, 0, &children, &mut visited, &mut ordered);
    }
    let mut remaining = groups
        .iter()
        .filter(|group| !visited.contains(&group.id))
        .cloned()
        .collect::<Vec<_>>();
    remaining.sort_by(|left, right| {
        left.sort_order
            .cmp(&right.sort_order)
            .then_with(|| {
                left.name
                    .to_ascii_lowercase()
                    .cmp(&right.name.to_ascii_lowercase())
            })
            .then_with(|| left.id.cmp(&right.id))
    });
    for group in remaining {
        append_group(group, 0, &children, &mut visited, &mut ordered);
    }
    ordered
}

fn connection_description_field(
    palette: crate::theme::ThemePalette,
    label: impl Into<SharedString>,
    fields: &ConnectionEditorFields,
    cx: &App,
) -> impl IntoElement {
    let label: SharedString = label.into();
    let entity = fields.get(&ConnectionEditorField::Description);
    let handle = entity.map(|field| field.read(cx).focus_handle());
    let focused = entity.is_some_and(|field| field.read(cx).has_focus());
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .text_xs()
                .text_color(rgb(palette.text_muted))
                .child(label),
        )
        .child(
            div()
                .id("connection-editor-description")
                .debug_selector(|| "connection-editor-description".to_string())
                // Fixed: in a flex column the box would otherwise shrink to
                // whatever space the form had left, cutting a row in half.
                .h(px(56.))
                .flex_none()
                .min_w_0()
                .overflow_hidden()
                .rounded_sm()
                .border_1()
                .border_color(ordinary_input_shell_border_color(palette, focused))
                .when(focused, |this| {
                    this.shadow(ordinary_input_focus_ring(palette))
                })
                .bg(rgb(palette.input))
                .px(px(ORDINARY_INPUT_SHELL_PADDING_X_PX))
                // Textarea supplies its own top padding; adding shell padding
                // here pushes the first line toward the center of this short box.
                .cursor_text()
                .when_some(handle, |this, handle| {
                    this.on_click(move |_, window, cx| {
                        window.focus(&handle, cx);
                    })
                })
                .children(entity.map(NyaInput::new)),
        )
}

fn connection_editor_footer_button(
    _palette: crate::theme::ThemePalette,
    id: &'static str,
    label: impl Into<SharedString>,
    primary: bool,
    enabled: bool,
    disabled_tooltip: Option<String>,
    on_click: impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    let mut button = nyaterm_ui::NyaButton::new(id, label)
        .height(px(36.))
        .variant(if primary {
            nyaterm_ui::NyaButtonVariant::Primary
        } else {
            nyaterm_ui::NyaButtonVariant::Secondary
        })
        .disabled(!enabled)
        .on_click(on_click);
    if !enabled && let Some(tooltip) = disabled_tooltip {
        button = button.tooltip(tooltip);
    }
    div().debug_selector(move || id.to_string()).child(button)
}

fn connection_proxy_jump_would_cycle(
    connections: &[SavedConnection],
    current_id: Option<&str>,
    candidate: &SavedConnection,
) -> bool {
    let Some(current_id) = current_id else {
        return false;
    };
    let mut seen = HashSet::new();
    let mut next_id = Some(candidate.id.clone());
    while let Some(id) = next_id {
        if id == current_id || !seen.insert(id.clone()) {
            return true;
        }
        next_id = connections
            .iter()
            .find(|connection| connection.id == id)
            .and_then(|connection| connection.network.as_ref())
            .and_then(|network| network.proxy_jump_id.clone());
    }
    false
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, path::Path, sync::Arc};

    use gpui::{
        AppContext as _, Entity, InteractiveElement as _, IntoElement, Modifiers,
        ParentElement as _, Render, RenderImage, ScrollDelta, ScrollWheelEvent, Styled as _,
        TestAppContext, VisualTestContext, div, point, px,
    };
    use nyaterm_core::{AppRuntime, Group, RuntimeMode, models::sessions::ConnectionCustomIcon};

    use super::{connection_editor_group_menu_options, ordered_connection_groups};
    use crate::entities::{OverlayStore, StartupRestoreStore, UiStoreHandles};
    use crate::features::NyaTermApp;
    use crate::features::selects::PENDING_CONNECTION_GROUP_VALUE;
    use crate::models::{
        ConnectionEditorCredentialOverlay, ConnectionEditorField, ConnectionEditorForwardingTab,
        ConnectionEditorPasswordSource, ConnectionEditorState, ConnectionKindTab,
    };
    use crate::test_support::TestConfigDir;

    struct EditorHost {
        app: Entity<NyaTermApp>,
        width: f32,
        height: f32,
        font_size: f32,
        native_window: bool,
    }

    struct AlgorithmListHost {
        app: Entity<NyaTermApp>,
        tab: crate::models::ConnectionEditorSshAlgorithmTab,
    }

    impl Render for AlgorithmListHost {
        fn render(
            &mut self,
            _window: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let tab = self.tab;
            let list = self.app.update(cx, |app, cx| {
                let supported = nyaterm_transport::supported_ssh_algorithms();
                let (options, selected) = match tab {
                    crate::models::ConnectionEditorSshAlgorithmTab::KeyExchange => {
                        (&supported.kex, &supported.compatible.kex)
                    }
                    crate::models::ConnectionEditorSshAlgorithmTab::Ciphers => {
                        (&supported.ciphers, &supported.compatible.ciphers)
                    }
                    crate::models::ConnectionEditorSshAlgorithmTab::Macs => {
                        (&supported.macs, &supported.compatible.macs)
                    }
                    crate::models::ConnectionEditorSshAlgorithmTab::HostKeys => {
                        (&supported.host_keys, &supported.compatible.host_keys)
                    }
                };
                super::ssh::ssh_algorithm_list(app.theme_palette(), tab, options, selected, cx)
                    .into_any_element()
            });
            div().w(px(340.)).text_size(px(12.)).child(list)
        }
    }

    impl Render for EditorHost {
        fn render(
            &mut self,
            _window: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let editor = self.app.read(cx).connection_state.active_editor_draft();
            let editor = self.app.update(cx, |app, cx| {
                editor
                    .map(|editor| {
                        if self.native_window {
                            app.connection_editor_window_view(editor, (self.width, self.height), cx)
                        } else {
                            app.connection_editor_panel(editor, (self.width, self.height), cx)
                        }
                    })
                    .unwrap_or_else(|| div().into_any_element())
            });
            div()
                .debug_selector(|| "connection-editor-test-host".to_string())
                .w(px(self.width))
                .h(px(self.height))
                .text_size(px(self.font_size))
                .overflow_hidden()
                .child(editor)
        }
    }

    fn test_app(cx: &mut TestAppContext, root: &Path) -> Entity<NyaTermApp> {
        let runtime = AppRuntime::from_parts_for_test(
            RuntimeMode::Portable,
            root.to_path_buf(),
            root.join("config"),
            root.join("logs"),
            root.join("cache"),
            None,
        );
        let stores = UiStoreHandles {
            startup_restore: cx.new(|_| StartupRestoreStore::default()),
            overlays: cx.new(|_| OverlayStore::default()),
        };
        cx.new(|cx| NyaTermApp::new(runtime, stores, cx))
    }

    fn hosted_editor<'a>(
        cx: &'a mut TestAppContext,
        root: &Path,
        width: f32,
        height: f32,
        font_size: f32,
    ) -> (Entity<NyaTermApp>, &'a mut VisualTestContext) {
        let app = test_app(cx, root);
        cx.update_entity(&app, |app, cx| {
            app.sync_component_theme(cx);
            app.connection_state.begin_editor(editor(None, None));
            app.connection_state.build_editor_fields(cx);
        });
        let host_app = app.clone();
        let (_, vcx) = cx.add_window_view(move |_, _| EditorHost {
            app: host_app,
            width,
            height,
            font_size,
            native_window: false,
        });
        let vcx: &mut VisualTestContext = vcx;
        vcx.run_until_parked();
        for _ in 0..3 {
            draw_editor(&app, vcx);
        }
        (app, vcx)
    }

    fn draw_editor(app: &Entity<NyaTermApp>, vcx: &mut VisualTestContext) {
        vcx.update(|window, cx| {
            app.update(cx, |_, cx| cx.notify());
            _ = window.draw(cx);
            window.simulate_next_frame(cx);
        });
        vcx.run_until_parked();
    }

    #[test]
    fn ssh_algorithm_lists_keep_rows_full_height_and_scroll_inside_the_viewport() {
        use crate::models::ConnectionEditorSshAlgorithmTab;

        for tab in [
            ConnectionEditorSshAlgorithmTab::KeyExchange,
            ConnectionEditorSshAlgorithmTab::Ciphers,
            ConnectionEditorSshAlgorithmTab::Macs,
            ConnectionEditorSshAlgorithmTab::HostKeys,
        ] {
            let test_dir = TestConfigDir::new("nyaterm-ssh-algorithm-list-scroll");
            let mut cx = TestAppContext::single();
            let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
            let app = test_app(&mut cx, test_dir.path());
            cx.update_entity(&app, |app, cx| app.sync_component_theme(cx));
            let host_app = app.clone();
            let (_, vcx) = cx.add_window_view(move |_, _| AlgorithmListHost { app: host_app, tab });
            let vcx: &mut VisualTestContext = vcx;
            draw_editor(&app, vcx);

            let viewport = vcx
                .debug_bounds("connection-ssh-algorithm-viewport")
                .unwrap();
            let first = vcx.debug_bounds("connection-ssh-algorithm-row-0").unwrap();
            let second = vcx.debug_bounds("connection-ssh-algorithm-row-1").unwrap();
            assert_eq!(first.size.height, px(36.));
            assert_eq!(second.top() - first.top(), px(36.));
            assert!(viewport.size.height <= px(226.));
            let overlay = vcx.debug_bounds("scrollbar-overlay").unwrap();
            assert!(overlay.size.height >= viewport.size.height - px(2.));

            vcx.simulate_event(ScrollWheelEvent {
                position: viewport.center(),
                delta: ScrollDelta::Pixels(point(px(0.), px(-108.))),
                ..Default::default()
            });
            draw_editor(&app, vcx);
            let after = vcx.debug_bounds("connection-ssh-algorithm-row-0").unwrap();
            if tab == ConnectionEditorSshAlgorithmTab::Macs {
                assert_eq!(after.top(), first.top(), "short lists should not scroll");
            } else {
                assert!(after.top() < first.top(), "{tab:?} must scroll its content");
            }
            assert_eq!(
                vcx.debug_bounds("connection-ssh-algorithm-viewport")
                    .unwrap(),
                viewport
            );
            assert_eq!(vcx.debug_bounds("scrollbar-overlay").unwrap(), overlay);
        }
    }

    fn protocol_selectors(kind: ConnectionKindTab) -> (&'static str, &'static str) {
        match kind {
            ConnectionKindTab::Ssh => (
                "connection-editor-ssh-section",
                "connection-editor-ssh-advanced",
            ),
            ConnectionKindTab::Local => (
                "connection-editor-local-section",
                "connection-editor-local-advanced",
            ),
            ConnectionKindTab::Telnet => (
                "connection-editor-telnet-section",
                "connection-editor-telnet-advanced",
            ),
            ConnectionKindTab::Serial => (
                "connection-editor-serial-section",
                "connection-editor-serial-advanced",
            ),
            ConnectionKindTab::Rdp => (
                "connection-editor-rdp-section",
                "connection-editor-rdp-advanced",
            ),
            ConnectionKindTab::Vnc => (
                "connection-editor-vnc-section",
                "connection-editor-vnc-advanced",
            ),
        }
    }

    fn group(id: &str, name: &str, parent_id: Option<&str>, sort_order: i32) -> Group {
        Group {
            id: id.to_string(),
            name: name.to_string(),
            parent_id: parent_id.map(ToOwned::to_owned),
            sort_order,
            created_at_ms: None,
            updated_at_ms: None,
        }
    }

    fn editor(group_id: Option<&str>, pending_group_name: Option<&str>) -> ConnectionEditorState {
        ConnectionEditorState {
            id: None,
            kind: ConnectionKindTab::Ssh,
            name: String::new(),
            description: String::new(),
            tags: Vec::new(),
            new_tag: String::new(),
            icon: None,
            icon_auto_detect: false,
            recording: None,
            group_id: group_id.map(ToOwned::to_owned),
            new_group_name: String::new(),
            pending_group_name: pending_group_name.map(ToOwned::to_owned),
            pending_group_parent_id: group_id.map(ToOwned::to_owned),
            host: String::new(),
            port: "22".to_string(),
            username: String::new(),
            domain: String::new(),
            auth_mode: "password".to_string(),
            rdp_backend: "builtin".to_string(),
            rdp_security: Default::default(),
            rdp_display: Default::default(),
            rdp_clipboard: Default::default(),
            rdp_reconnect: Default::default(),

            vnc_security: Default::default(),
            vnc_display: Default::default(),
            vnc_clipboard: Default::default(),
            vnc_reconnect: Default::default(),
            vnc_shared: true,
            vnc_view_only: false,
            password_source: ConnectionEditorPasswordSource::Ask,
            password_id: None,
            account_id: None,
            username_source: nyaterm_core::ConnectionUsernameSource::Connection,
            password_source_touched: false,
            password_source_demoted: false,
            password_edit: crate::models::ConnectionPasswordEdit::Replace,
            field_errors: Default::default(),
            password: nyaterm_core::SecretString::default(),
            existing_password: None,
            existing_password_locked: false,
            key_id: None,
            otp_id: None,
            auto_fill_otp: false,
            proxy_id: None,
            proxy_jump_id: None,
            host_key_alias: None,
            x11_forwarding: false,
            dynamic_tab_title: false,
            agent_endpoint: Default::default(),
            agent_forwarding_config: Default::default(),
            agent_allow_all_confirmed: false,
            agent_forwarding_endpoint_index: 0,
            agent_preview: None,
            agent_preview_loading: false,
            backspace_mode: "del".to_string(),
            encoding: String::new(),
            ssh_profile: Default::default(),
            terminal_type: None,
            sftp_enabled: true,
            sftp_compatibility_mode: false,
            sftp_cwd_follow_mode: "shell_integration".to_string(),
            sftp_shell_detection_timeout_ms: "3000".to_string(),
            sftp_pipeline_depth: None,
            sftp_extra: Default::default(),
            sftp_filename_encoding: String::new(),
            ssh_algorithm_mode: "compatible".to_string(),
            ssh_algorithm_kex: Vec::new(),
            ssh_algorithm_ciphers: Vec::new(),
            ssh_algorithm_macs: Vec::new(),
            ssh_algorithm_host_keys: Vec::new(),
            ssh_algorithm_tab: crate::models::ConnectionEditorSshAlgorithmTab::KeyExchange,
            shell_path: String::new(),
            shell_args: String::new(),
            working_dir: String::new(),
            serial_port: String::new(),
            baud_rate: "115200".to_string(),
            data_bits: "8".to_string(),
            parity: "none".to_string(),
            flow_control: Default::default(),
            stop_bits: "1".to_string(),
            raw_tcp_cli: false,
            telnet_enter_mode: "cr".to_string(),
            local_echo: false,
            local_line_edit: false,
            force_character_at_a_time: false,
            send_naws: true,
            send_sga: true,
            telnet_auto_login_enabled: true,
            telnet_auto_login_send_wake_enter: true,
            telnet_keep_alive_interval_secs:
                nyaterm_core::models::connection::DEFAULT_TELNET_KEEP_ALIVE_INTERVAL_SECS
                    .to_string(),
            telnet_auto_login_timeout_ms: "60000".to_string(),
            telnet_auto_login_username_prompt_regex: String::new(),
            telnet_auto_login_password_prompt_regex: String::new(),
            telnet_auto_login_success_prompt_regex: String::new(),
            telnet_auto_login_failure_prompt_regex: String::new(),
            telnet_auto_login_max_retries: "0".to_string(),
            post_login_enabled: false,
            post_login_command: String::new(),
            post_login_delay_ms: "0".to_string(),

            forwarding_tab: ConnectionEditorForwardingTab::Agent,

            connect_after_save: false,
            section: crate::models::ConnectionEditorSection::Basic,
            focused_field: ConnectionEditorField::Name,
            error: None,
        }
    }

    #[test]
    fn connection_editor_group_select_orders_tree_and_keeps_orphans_visible() {
        let groups = vec![
            group("child", "Child", Some("parent"), 0),
            group("orphan", "Orphan", Some("missing"), 2),
            group("parent", "Parent", None, 1),
        ];

        let ordered = ordered_connection_groups(&groups)
            .into_iter()
            .map(|(group, depth)| (group.id, depth))
            .collect::<Vec<_>>();

        assert_eq!(
            ordered,
            vec![
                ("parent".to_string(), 0),
                ("child".to_string(), 1),
                ("orphan".to_string(), 0),
            ]
        );
    }

    #[test]
    fn connection_editor_group_menu_options_keep_depth_labels_and_pending_group() {
        let groups = vec![
            group("child", "Child", Some("parent"), 0),
            group("parent", "Parent", None, 1),
        ];
        let editor = editor(Some("parent"), Some("Staging"));

        let options = connection_editor_group_menu_options(&groups, &editor, "None", "New Group");
        let values = options
            .iter()
            .map(|option| {
                (
                    option.value.as_deref(),
                    option.label.as_str(),
                    option.depth,
                    option.selected,
                    option.disabled,
                    option.none_option,
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(
            values,
            vec![
                (None, "None", 0, false, false, true),
                (
                    Some(PENDING_CONNECTION_GROUP_VALUE),
                    "Staging (New Group)",
                    0,
                    true,
                    true,
                    false
                ),
                (Some("parent"), "Parent", 0, false, false, false),
                (Some("child"), "Child", 1, false, false, false),
            ]
        );
    }

    #[test]
    fn connection_editor_group_menu_options_select_existing_group_without_pending_group() {
        let groups = vec![
            group("child", "Child", Some("parent"), 0),
            group("parent", "Parent", None, 1),
        ];
        let editor = editor(Some("child"), None);

        let selected = connection_editor_group_menu_options(&groups, &editor, "None", "New Group")
            .into_iter()
            .filter(|option| option.selected)
            .map(|option| option.value)
            .collect::<Vec<_>>();

        assert_eq!(selected, vec![Some("child".to_string())]);
    }

    fn visible_password_eye(
        app: &Entity<NyaTermApp>,
        vcx: &mut VisualTestContext,
    ) -> gpui::Bounds<gpui::Pixels> {
        app.read_with(vcx, |app, _| app.connection_state.editor_scroll())
            .set_offset(point(px(0.), px(-10000.)));
        draw_editor(app, vcx);
        draw_editor(app, vcx);
        vcx.debug_bounds("nya-input-mask-toggle").unwrap()
    }

    #[gpui::test]
    fn saved_connection_password_supports_typing_paste_clear_and_locked_delete(
        cx: &mut TestAppContext,
    ) {
        use crate::models::ConnectionPasswordEdit;
        let test_dir = TestConfigDir::new("nyaterm-single-password-input");
        let (app, vcx) = hosted_editor(cx, test_dir.path(), 640., 900., 12.);
        vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                let mut draft = editor(None, None);
                draft.id = Some("saved".into());
                draft.password_source = ConnectionEditorPasswordSource::Direct;
                draft.password_edit = ConnectionPasswordEdit::Keep;
                draft.password = "old".into();
                draft.existing_password = Some("old".into());
                app.connection_state.begin_editor(draft);
                app.connection_state.build_editor_fields(cx);
            })
        });
        draw_editor(&app, vcx);
        assert!(
            vcx.debug_bounds("connection-editor-field-Password")
                .is_some()
        );
        vcx.update(|window, cx| {
            let input =
                &app.read(cx).connection_state.editor_fields()[&ConnectionEditorField::Password];
            assert_eq!(input.read(cx).value(cx), "old");
            assert!(input.read(cx).is_masked());
            window.focus(&input.read(cx).component_focus_handle(cx), cx);
        });
        draw_editor(&app, vcx);
        let eye = visible_password_eye(&app, vcx);
        vcx.simulate_click(eye.center(), Modifiers::default());
        draw_editor(&app, vcx);
        assert!(app.read_with(vcx, |app, cx| {
            !app.connection_state.editor_fields()[&ConnectionEditorField::Password]
                .read(cx)
                .is_masked()
                && !app.security.unlock_prompt_open()
        }));
        let eye = visible_password_eye(&app, vcx);
        vcx.simulate_click(eye.center(), Modifiers::default());
        draw_editor(&app, vcx);
        vcx.simulate_keystrokes("secondary-a n e w");
        draw_editor(&app, vcx);
        assert!(vcx.update(|_, cx| {
            let draft = app.read(cx).connection_state.active_editor_draft().unwrap();
            draft.password_edit == ConnectionPasswordEdit::Replace
                && draft.password.expose_secret() == "new"
        }));
        vcx.simulate_keystrokes("tab");
        draw_editor(&app, vcx);
        assert!(vcx.update(|window, cx| {
            let input =
                &app.read(cx).connection_state.editor_fields()[&ConnectionEditorField::Password];
            !input.read(cx).component_focus_handle(cx).is_focused(window)
        }));
        vcx.simulate_keystrokes("shift-tab");
        draw_editor(&app, vcx);
        assert!(vcx.update(|window, cx| {
            let input =
                &app.read(cx).connection_state.editor_fields()[&ConnectionEditorField::Password];
            input.read(cx).component_focus_handle(cx).is_focused(window)
        }));
        vcx.update(|_, cx| {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(" pasted ".into()))
        });
        vcx.simulate_keystrokes("secondary-a secondary-v");
        draw_editor(&app, vcx);
        assert!(vcx.update(|_, cx| {
            app.read(cx)
                .connection_state
                .active_editor_draft()
                .unwrap()
                .password
                .expose_secret()
                == " pasted "
        }));
        vcx.simulate_keystrokes("secondary-a backspace");
        draw_editor(&app, vcx);
        assert_eq!(
            vcx.update(|_, cx| app
                .read(cx)
                .connection_state
                .active_editor_draft()
                .unwrap()
                .password_edit),
            ConnectionPasswordEdit::Remove
        );

        for key in ["backspace", "delete"] {
            vcx.update(|_, cx| {
                app.update(cx, |app, cx| app.connection_state.clear_editor_secrets(cx))
            });
            draw_editor(&app, vcx);
            vcx.update(|window, cx| {
                let input = &app.read(cx).connection_state.editor_fields()
                    [&ConnectionEditorField::Password];
                assert!(input.read(cx).value(cx).is_empty());
                assert!(
                    app.read(cx)
                        .connection_state
                        .editor_password_can_delete_stored()
                );
                window.focus(&input.read(cx).component_focus_handle(cx), cx);
            });
            draw_editor(&app, vcx);
            let keystroke = gpui::Keystroke::parse(key).unwrap();
            vcx.simulate_event(gpui::KeyDownEvent {
                keystroke: keystroke.clone(),
                is_held: false,
                prefer_character_input: false,
            });
            vcx.simulate_event(gpui::KeyUpEvent { keystroke });
            draw_editor(&app, vcx);
            assert_eq!(
                vcx.update(|_, cx| app
                    .read(cx)
                    .connection_state
                    .active_editor_draft()
                    .unwrap()
                    .password_edit),
                ConnectionPasswordEdit::Remove
            );
        }
        vcx.simulate_keystrokes("n e w");
        draw_editor(&app, vcx);
        assert_eq!(
            vcx.update(|_, cx| app
                .read(cx)
                .connection_state
                .active_editor_draft()
                .unwrap()
                .password_edit),
            ConnectionPasswordEdit::Replace
        );
    }

    #[gpui::test]
    fn password_eye_unlock_failure_cancel_and_success_preserve_the_new_draft(
        cx: &mut TestAppContext,
    ) {
        use crate::models::ConnectionPasswordEdit;
        let test_dir = TestConfigDir::new("nyaterm-password-eye-unlock");
        let app = test_app(cx, test_dir.path());
        app.update(cx, |app, cx| {
            app.sync_component_theme(cx);
            let mut summary = app.settings.summary().clone();
            summary.has_master_password = true;
            app.settings.replace_summary(summary);
            app.security.lock_secrets();
            let mut draft = editor(None, None);
            draft.password_edit = ConnectionPasswordEdit::Replace;
            draft.password = " new draft ".into();
            draft.existing_password = Some("old".into());
            app.connection_state.begin_editor(draft);
            app.connection_state.build_editor_fields(cx);
        });
        let host_app = app.clone();
        let (_, vcx) = cx.add_window_view(move |window, cx| {
            let content = cx.new(|_| EditorHost {
                app: host_app,
                width: 640.,
                height: 900.,
                font_size: 12.,
                native_window: false,
            });
            nyaterm_ui::nya_root(content, window, cx)
        });
        let vcx: &mut VisualTestContext = vcx;
        draw_editor(&app, vcx);
        app.update(vcx, |app, _| {
            let mut summary = app.settings.summary().clone();
            summary.has_master_password = true;
            app.settings.replace_summary(summary);
            app.security.lock_secrets();
        });
        let eye = visible_password_eye(&app, vcx);
        vcx.simulate_click(eye.center(), Modifiers::default());
        draw_editor(&app, vcx);
        assert!(app.read_with(vcx, |app, _| app.security.unlock_prompt_open()));
        vcx.update(|window, cx| {
            app.update(cx, |app, cx| {
                let request = app.security.begin_unlock_request().unwrap().0;
                let generation = app.security.unlock_dialog_generation();
                app.finish_security_unlock(
                    request,
                    generation,
                    window.window_handle(),
                    Ok(false),
                    window,
                    cx,
                );
                assert!(app.security.unlock_error().is_some());
                assert_eq!(
                    app.connection_state
                        .active_editor_draft()
                        .unwrap()
                        .password
                        .expose_secret(),
                    " new draft "
                );
            })
        });
        draw_editor(&app, vcx);
        vcx.simulate_keystrokes("escape");
        draw_editor(&app, vcx);
        assert!(!app.read_with(vcx, |app, _| app.security.unlock_prompt_open()));
        assert_eq!(
            app.read_with(vcx, |app, _| app
                .connection_state
                .active_editor_draft()
                .unwrap()
                .password_edit),
            ConnectionPasswordEdit::Replace
        );

        let eye = visible_password_eye(&app, vcx);
        vcx.simulate_click(eye.center(), Modifiers::default());
        draw_editor(&app, vcx);
        vcx.update(|window, cx| {
            app.update(cx, |app, cx| {
                let request = app.security.begin_unlock_request().unwrap().0;
                let generation = app.security.unlock_dialog_generation();
                app.finish_security_unlock(
                    request,
                    generation,
                    window.window_handle(),
                    Ok(true),
                    window,
                    cx,
                );
            })
        });
        draw_editor(&app, vcx);
        vcx.update(|_, cx| {
            let input =
                &app.read(cx).connection_state.editor_fields()[&ConnectionEditorField::Password];
            assert!(!input.read(cx).is_masked());
            assert_eq!(input.read(cx).value(cx), " new draft ");
        });
        // Hiding must work even if the security owner has become locked.
        vcx.update(|_, cx| app.update(cx, |app, _| app.security.lock_secrets()));
        let eye = visible_password_eye(&app, vcx);
        vcx.simulate_click(eye.center(), Modifiers::default());
        draw_editor(&app, vcx);
        assert!(!app.read_with(vcx, |app, _| app.security.unlock_prompt_open()));
        assert!(app.read_with(vcx, |app, cx| {
            app.connection_state.editor_fields()[&ConnectionEditorField::Password]
                .read(cx)
                .is_masked()
        }));
    }

    #[gpui::test]
    fn account_password_input_rejects_typing_and_paste_and_unlink_restores_editing(
        cx: &mut TestAppContext,
    ) {
        use crate::models::{ConnectionEditorSelect, ConnectionPasswordEdit};
        let test_dir = TestConfigDir::new("nyaterm-account-readonly-password");
        let (app, vcx) = hosted_editor(cx, test_dir.path(), 640., 900., 12.);
        for kind in [
            ConnectionKindTab::Ssh,
            ConnectionKindTab::Telnet,
            ConnectionKindTab::Rdp,
            ConnectionKindTab::Vnc,
        ] {
            vcx.update(|_, cx| {
                app.update(cx, |app, cx| {
                    let mut draft = editor(None, None);
                    draft.kind = kind;
                    draft.password_edit = ConnectionPasswordEdit::Keep;
                    if matches!(kind, ConnectionKindTab::Ssh | ConnectionKindTab::Telnet) {
                        draft.account_id = Some("account".into());
                    } else {
                        draft.password_id = Some("account".into());
                    }
                    draft.password_source = ConnectionEditorPasswordSource::Saved;
                    app.connection_state.begin_editor(draft);
                    app.connection_state.build_editor_fields(cx);
                    let token = app.connection_state.begin_editor_password_reveal().unwrap();
                    app.connection_state.complete_editor_password_reveal(
                        token,
                        Ok(Some("account secret".into())),
                        cx,
                    );
                })
            });
            draw_editor(&app, vcx);
            vcx.update(|window, cx| {
                let input = &app.read(cx).connection_state.editor_fields()
                    [&ConnectionEditorField::Password];
                window.focus(&input.read(cx).component_focus_handle(cx), cx);
                cx.write_to_clipboard(gpui::ClipboardItem::new_string("paste".into()));
            });
            draw_editor(&app, vcx);
            vcx.simulate_keystrokes("secondary-a x secondary-v backspace delete");
            draw_editor(&app, vcx);
            assert_eq!(
                app.read_with(vcx, |app, cx| app.connection_state.editor_fields()
                    [&ConnectionEditorField::Password]
                    .read(cx)
                    .value(cx)),
                "account secret"
            );
            vcx.update(|_, cx| {
                app.update(cx, |app, cx| {
                    let select =
                        if matches!(kind, ConnectionKindTab::Ssh | ConnectionKindTab::Telnet) {
                            ConnectionEditorSelect::Account
                        } else {
                            ConnectionEditorSelect::SavedPassword
                        };
                    app.set_connection_editor_select_value(select, None, cx);
                })
            });
            draw_editor(&app, vcx);
            vcx.update(|window, cx| {
                let input = &app.read(cx).connection_state.editor_fields()
                    [&ConnectionEditorField::Password];
                assert!(input.read(cx).value(cx).is_empty());
                window.focus(&input.read(cx).component_focus_handle(cx), cx);
            });
            draw_editor(&app, vcx);
            vcx.simulate_keystrokes("n e w");
            draw_editor(&app, vcx);
            assert_eq!(
                app.read_with(vcx, |app, _| app
                    .connection_state
                    .active_editor_draft()
                    .unwrap()
                    .password
                    .expose_secret()
                    .to_owned()),
                "new"
            );
        }
    }

    #[gpui::test]
    fn post_login_editor_preserves_edits_across_navigation_and_saves_only_with_shortcut(
        cx: &mut TestAppContext,
    ) {
        use crate::features::connections::ConnectionEditorToggle;
        use crate::models::ConnectionEditorSection;

        let test_dir = TestConfigDir::new("nyaterm-post-login-editor");
        let (app, vcx) = hosted_editor(cx, test_dir.path(), 640., 720., 12.);
        vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                let mut draft = editor(None, None);
                draft.post_login_enabled = true;
                draft.post_login_command = "echo hello".into();
                app.connection_state.begin_editor(draft);
                app.connection_state.build_editor_fields(cx);
                app.set_connection_editor_section(ConnectionEditorSection::PostLogin, cx);
            });
        });
        draw_editor(&app, vcx);
        assert_eq!(
            vcx.debug_bounds("connection-editor-post-login-command")
                .unwrap()
                .size
                .height,
            px(168.)
        );
        let input = vcx.update(|window, cx| {
            let input = app.read(cx).connection_state.editor_fields()
                [&ConnectionEditorField::PostLoginCommand]
                .clone();
            input.update(cx, |input, cx| input.focus_with_cursor_at_end(window, cx));
            input
        });
        vcx.simulate_keystrokes("enter shift-enter");
        draw_editor(&app, vcx);
        vcx.update(|_, cx| {
            let draft = app.read(cx).connection_state.active_editor_draft().unwrap();
            assert_eq!(draft.section, ConnectionEditorSection::PostLogin);
            assert!(draft.field_errors.is_empty());
            assert_eq!(draft.post_login_command, "echo hello\n\n");
        });
        for section in [
            ConnectionEditorSection::Basic,
            ConnectionEditorSection::PostLogin,
        ] {
            vcx.update(|_, cx| {
                app.update(cx, |app, cx| app.set_connection_editor_section(section, cx))
            });
            draw_editor(&app, vcx);
        }
        for _ in 0..2 {
            vcx.update(|_, cx| {
                app.update(cx, |app, cx| {
                    app.toggle_connection_editor_flag(ConnectionEditorToggle::PostLogin, cx)
                })
            });
            draw_editor(&app, vcx);
        }
        vcx.update(|window, cx| {
            assert_eq!(
                input.entity_id(),
                app.read(cx).connection_state.editor_fields()
                    [&ConnectionEditorField::PostLoginCommand]
                    .entity_id()
            );
            assert_eq!(input.read(cx).value(cx), "echo hello\n\n");
            input.update(cx, |input, cx| input.focus_with_cursor_at_end(window, cx));
        });
        vcx.simulate_keystrokes(if cfg!(target_os = "macos") {
            "cmd-enter"
        } else {
            "ctrl-enter"
        });
        draw_editor(&app, vcx);
        vcx.update(|_, cx| {
            let draft = app.read(cx).connection_state.active_editor_draft().unwrap();
            assert_eq!(draft.post_login_command, "echo hello\n\n");
            assert!(
                draft
                    .field_errors
                    .contains_key(&ConnectionEditorField::Host),
                "save must validate the connection"
            );
            assert_eq!(draft.section, ConnectionEditorSection::Basic);
        });
    }

    #[gpui::test]
    fn connection_editor_enter_creates_inline_tags_and_keeps_input_focus(cx: &mut TestAppContext) {
        let test_dir = TestConfigDir::new("nyaterm-connection-inline-tags");
        let (app, vcx) = hosted_editor(cx, test_dir.path(), 640., 720., 12.);

        for id in [None, Some("saved-local")] {
            vcx.update(|_, cx| {
                app.update(cx, |app, cx| {
                    let mut draft = editor(None, None);
                    draft.id = id.map(ToOwned::to_owned);
                    draft.kind = ConnectionKindTab::Local;
                    app.connection_state.begin_editor(draft);
                    app.connection_state.build_editor_fields(cx);
                    cx.notify();
                });
            });
            draw_editor(&app, vcx);
            let tags = vcx
                .debug_bounds("connection-editor-tags")
                .expect("tag field");
            let description = vcx
                .debug_bounds("connection-editor-description")
                .expect("description field");
            assert!(tags.bottom() < description.top());
            vcx.update(|window, cx| {
                let input =
                    &app.read(cx).connection_state.editor_fields()[&ConnectionEditorField::NewTag];
                window.focus(&input.read(cx).component_focus_handle(cx), cx);
            });
            draw_editor(&app, vcx);
            vcx.simulate_keystrokes("d o c k e r enter");
            draw_editor(&app, vcx);
            vcx.update(|window, cx| {
                let state = &app.read(cx).connection_state;
                let draft = state
                    .active_editor_draft()
                    .expect("Enter keeps the editor open");
                assert_eq!(draft.tags, ["docker"]);
                assert!(draft.new_tag.is_empty());
                let input = &state.editor_fields()[&ConnectionEditorField::NewTag];
                assert_eq!(input.read(cx).value(cx), "");
                assert!(input.read(cx).component_focus_handle(cx).is_focused(window));
            });
            vcx.simulate_keystrokes("g p u enter");
            draw_editor(&app, vcx);
            let tags_after = vcx
                .debug_bounds("connection-editor-tags")
                .expect("inline tags");
            assert_eq!(tags.size.height, tags_after.size.height);
            vcx.update(|_, cx| {
                assert_eq!(
                    app.read(cx)
                        .connection_state
                        .active_editor_draft()
                        .unwrap()
                        .tags,
                    ["docker", "gpu"]
                );
            });
        }
    }

    #[gpui::test]
    fn serial_custom_baud_accepts_pointer_focus_and_keyboard_edits(cx: &mut TestAppContext) {
        let test_dir = TestConfigDir::new("nyaterm-serial-custom-baud-input");
        let (app, vcx) = hosted_editor(cx, test_dir.path(), 640., 720., 12.);

        for (id, seed) in [(None, "115200"), (Some("saved-serial"), "76800")] {
            vcx.update(|_, cx| {
                app.update(cx, |app, cx| {
                    let mut draft = editor(None, None);
                    draft.id = id.map(ToOwned::to_owned);
                    draft.kind = ConnectionKindTab::Serial;
                    draft.baud_rate = seed.to_string();
                    app.connection_state.begin_editor(draft);
                    app.connection_state.build_editor_fields(cx);
                    app.set_connection_editor_section(
                        crate::models::ConnectionEditorSection::Serial,
                        cx,
                    );
                    app.set_connection_editor_baud_popover_open(true, cx);
                    cx.notify();
                });
            });
            for _ in 0..3 {
                draw_editor(&app, vcx);
            }
            let bounds = vcx
                .debug_bounds("connection-editor-custom-baud")
                .expect("custom baud input should render");

            let input = vcx.update(|_, cx| {
                app.read(cx).connection_state.editor_number_fields()
                    [&ConnectionEditorField::BaudRate]
                    .clone()
            });
            assert!(
                input.read_with(vcx, |input, _| input.component_state().is_some()),
                "the popup must render the numeric input entity"
            );
            assert_eq!(input.read_with(vcx, |input, cx| input.value(cx)), seed);
            assert!(bounds.size.width > px(0.));
            assert!(bounds.size.height >= px(32.));
            vcx.simulate_click(bounds.center(), Modifiers::default());
            draw_editor(&app, vcx);
            assert!(vcx.update(|window, cx| {
                input.read(cx).component_focus_handle(cx).is_focused(window)
            }));
            vcx.simulate_keystrokes("secondary-a 2 5 0 0 0 0");
            draw_editor(&app, vcx);
            assert_eq!(input.read_with(vcx, |input, cx| input.value(cx)), "250000");
            assert!(vcx.update(|_, cx| {
                let state = &app.read(cx).connection_state;
                assert_eq!(state.active_editor_draft().unwrap().baud_rate, "250000");
                state.editor_baud_popover_is_open()
            }));

            let apply = vcx
                .debug_bounds("connection-editor-apply-custom-baud")
                .expect("apply custom baud button should render");
            vcx.simulate_click(apply.center(), Modifiers::default());
            draw_editor(&app, vcx);
            assert!(vcx.update(|_, cx| {
                let state = &app.read(cx).connection_state;
                assert_eq!(state.active_editor_draft().unwrap().baud_rate, "250000");
                !state.editor_baud_popover_is_open()
            }));
        }
    }

    #[gpui::test]
    fn connection_editor_icon_picker_scrolls_custom_icons_without_moving_actions(
        cx: &mut TestAppContext,
    ) {
        let test_dir = TestConfigDir::new("nyaterm-connection-icon-picker-scroll");
        let (app, vcx) = hosted_editor(cx, test_dir.path(), 640., 720., 12.);
        vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.set_connection_icon_picker_open(true, cx);
            });
        });
        for _ in 0..3 {
            draw_editor(&app, vcx);
        }
        let grid_without_custom = vcx
            .debug_bounds("connection-editor-icon-grid")
            .expect("default icons should render inside the picker");
        let popup_without_custom = vcx
            .debug_bounds("connection-editor-icon-popover-content")
            .expect("icon picker should be open");

        let records = (0..14)
            .map(|index| ConnectionCustomIcon {
                id: format!("test-icon-{index}"),
                name: format!("Test icon {index}"),
                data_url: String::new(),
                created_at_ms: 0,
                updated_at_ms: 0,
            })
            .collect::<Vec<_>>();
        vcx.update(|_, cx| {
            app.update(cx, |app, _| {
                let icon = Arc::new(RenderImage::new(vec![image::Frame::new(
                    image::RgbaImage::from_pixel(1, 1, image::Rgba([255, 0, 0, 255])),
                )]));
                let images = records
                    .iter()
                    .map(|record| (record.id.clone(), icon.clone()))
                    .collect::<HashMap<_, _>>();
                app.connection_state.custom_icons.records = records[..7].to_vec();
                app.connection_state.custom_icons.images = Arc::new(images);
            });
        });
        for _ in 0..3 {
            draw_editor(&app, vcx);
        }
        let popup_with_seven_custom = vcx
            .debug_bounds("connection-editor-icon-popover-content")
            .expect("icon picker should grow with seven custom icons");
        let first_custom_icon = vcx
            .debug_bounds("connection-editor-custom-icon-first")
            .expect("first custom icon should render");
        let first_custom_image = vcx
            .debug_bounds("connection-editor-custom-image-first")
            .expect("image inside first custom icon should render");
        assert_eq!(first_custom_image.left() - first_custom_icon.left(), px(6.));
        assert_eq!(
            popup_with_seven_custom.size.height - popup_without_custom.size.height,
            px(32.)
        );

        vcx.update(|_, cx| {
            app.update(cx, |app, _| {
                app.connection_state.custom_icons.records = records;
            });
        });
        for _ in 0..3 {
            draw_editor(&app, vcx);
        }

        let grid_before = vcx
            .debug_bounds("connection-editor-icon-grid")
            .expect("icon grid should render inside the picker");
        let popup_before = vcx
            .debug_bounds("connection-editor-icon-popover-content")
            .expect("icon picker should be open");
        let rightmost_icon = vcx
            .debug_bounds("connection-editor-icon-rightmost")
            .expect("rightmost icon should render clear of the scrollbar");
        let auto_detect_before = vcx
            .debug_bounds("connection-editor-icon-auto-detect")
            .expect("auto-detect action should render below the icon grid");
        assert_eq!(grid_without_custom.size.height, px(220.));
        assert!(grid_before.size.height > px(280.));
        assert!(popup_before.right() - rightmost_icon.right() >= px(25.));
        assert_eq!(
            popup_before.size.height - popup_without_custom.size.height,
            px(60.)
        );
        vcx.simulate_event(ScrollWheelEvent {
            position: point(grid_before.left() + px(20.), grid_before.top() + px(20.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(-48.))),
            ..Default::default()
        });
        draw_editor(&app, vcx);

        let grid_after = vcx
            .debug_bounds("connection-editor-icon-grid")
            .expect("icon grid should remain scrollable");
        let popup_after = vcx
            .debug_bounds("connection-editor-icon-popover-content")
            .expect("icon picker should remain open");
        let auto_detect_after = vcx
            .debug_bounds("connection-editor-icon-auto-detect")
            .expect("auto-detect action should remain below the icon grid");
        assert!(
            grid_after.top() < grid_before.top(),
            "grid: {grid_before:?} -> {grid_after:?}, popup: {popup_before:?} -> {popup_after:?}, auto-detect: {auto_detect_before:?} -> {auto_detect_after:?}"
        );
        assert_eq!(popup_after.top(), popup_before.top());
        assert_eq!(auto_detect_after.top(), auto_detect_before.top());
    }

    #[gpui::test]
    fn vnc_credential_manager_blocks_editor_clicks_and_closes(cx: &mut TestAppContext) {
        let test_dir = TestConfigDir::new("nyaterm-vnc-credential-overlay");
        let (app, vcx) = hosted_editor(cx, test_dir.path(), 640., 720., 12.);
        vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.set_connection_editor_kind(ConnectionKindTab::Vnc, cx);
                app.set_connection_editor_section(
                    crate::models::ConnectionEditorSection::Basic,
                    cx,
                );
                app.connection_state
                    .set_editor_password_source(ConnectionEditorPasswordSource::Saved);
                app.set_connection_editor_credential_overlay(
                    Some(ConnectionEditorCredentialOverlay::Passwords),
                    cx,
                );
            });
        });
        draw_editor(&app, vcx);
        assert_eq!(
            vcx.update(|_, cx| app.read(cx).connection_state.editor_credential_overlay()),
            Some(ConnectionEditorCredentialOverlay::Passwords)
        );

        let editor_close = vcx
            .debug_bounds("connection-editor-close")
            .expect("editor close button should render under the overlay");
        vcx.simulate_click(editor_close.center(), Modifiers::default());
        draw_editor(&app, vcx);
        assert_eq!(
            vcx.update(|_, cx| app.read(cx).connection_state.editor_credential_overlay()),
            Some(ConnectionEditorCredentialOverlay::Passwords)
        );
        assert!(vcx.update(|_, cx| {
            app.read(cx)
                .connection_state
                .active_editor_draft()
                .is_some()
        }));

        let overlay_close = vcx
            .debug_bounds("connection-editor-credential-close")
            .expect("credential manager close button should render");
        vcx.simulate_click(overlay_close.center(), Modifiers::default());
        draw_editor(&app, vcx);
        assert_eq!(
            vcx.update(|_, cx| app.read(cx).connection_state.editor_credential_overlay()),
            None
        );
        assert!(
            vcx.debug_bounds("connection-editor-credential-overlay")
                .is_none()
        );
    }

    #[gpui::test]
    fn account_dropdown_escape_closes_the_menu_before_the_connection_editor(
        cx: &mut TestAppContext,
    ) {
        use crate::models::ConnectionEditorSelect;
        let test_dir = TestConfigDir::new("nyaterm-account-dropdown-escape");
        let (app, vcx) = hosted_editor(cx, test_dir.path(), 640., 720., 12.);
        vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                let account = nyaterm_core::SavedPassword {
                    id: "identity".into(),
                    name: "Identity".into(),
                    username: "root".into(),
                    password: None,
                    has_password: false,
                    sort_order: 0,
                };
                app.security
                    .replace_catalog(Vec::new(), Vec::new(), vec![account], Vec::new());
                app.set_connection_editor_section(
                    crate::models::ConnectionEditorSection::Basic,
                    cx,
                );
            })
        });
        draw_editor(&app, vcx);
        vcx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.focus_connection_editor_select(ConnectionEditorSelect::Account, window, cx)
            })
        });
        draw_editor(&app, vcx);
        vcx.simulate_keystrokes("enter");
        draw_editor(&app, vcx);
        assert!(vcx.update(|window, cx| {
            app.read(cx)
                .connection_editor_select_menu_is_focused(window, cx)
        }));
        vcx.simulate_keystrokes("escape");
        draw_editor(&app, vcx);
        assert!(vcx.update(|window, cx| {
            let app = app.read(cx);
            app.connection_state.active_editor_draft().is_some()
                && app.connection_editor_select_is_focused(window, cx)
                && !app.connection_editor_select_menu_is_focused(window, cx)
        }));
        vcx.simulate_keystrokes("escape");
        assert!(vcx.update(|_, cx| {
            app.read(cx)
                .connection_state
                .active_editor_draft()
                .is_none()
        }));
    }

    #[gpui::test]
    fn vnc_no_authentication_hides_password_controls_without_losing_the_draft(
        cx: &mut TestAppContext,
    ) {
        let test_dir = TestConfigDir::new("nyaterm-vnc-auth-draft");
        let (app, vcx) = hosted_editor(cx, test_dir.path(), 640., 720., 12.);
        vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.set_connection_editor_kind(ConnectionKindTab::Vnc, cx);
                app.set_connection_editor_section(
                    crate::models::ConnectionEditorSection::Basic,
                    cx,
                );
                app.connection_state
                    .set_editor_password_source(ConnectionEditorPasswordSource::Direct);
                app.connection_state.set_editor_field_text(
                    ConnectionEditorField::Password,
                    "retained fixture".into(),
                );
                app.set_connection_editor_select_value(
                    crate::models::ConnectionEditorSelect::VncSecurityMode,
                    Some("none"),
                    cx,
                );
            })
        });
        draw_editor(&app, vcx);
        assert!(vcx.debug_bounds("connection-password-controls").is_none());
        vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.set_connection_editor_select_value(
                    crate::models::ConnectionEditorSelect::VncSecurityMode,
                    Some("auto"),
                    cx,
                );
                assert_eq!(
                    app.connection_state
                        .active_editor_draft()
                        .unwrap()
                        .password
                        .expose_secret(),
                    "retained fixture"
                );
            })
        });
        draw_editor(&app, vcx);
        assert!(vcx.debug_bounds("connection-password-controls").is_some());
    }

    #[test]
    fn account_dialog_returns_to_the_same_connection_and_preserves_its_drafts() {
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        use crate::models::ConnectionEditorSelect;
        use nyaterm_core::models::credentials::PasswordUpdate;
        let test_dir = TestConfigDir::new("nyaterm-account-dialog-return");
        let app = test_app(&mut cx, test_dir.path());
        cx.update_entity(&app, |app, cx| {
            app.sync_component_theme(cx);
            let mut draft = editor(None, None);
            draft.host = "example.com".into();
            draft.username = "root".into();
            app.connection_state.begin_editor(draft);
            app.connection_state.build_editor_fields(cx);
            app.set_connection_editor_section(crate::models::ConnectionEditorSection::Basic, cx);
        });
        let host_app = app.clone();
        let (_, vcx) = cx.add_window_view(move |window, cx| {
            let content = cx.new(|_| EditorHost {
                app: host_app,
                width: 640.,
                height: 720.,
                font_size: 12.,
                native_window: false,
            });
            nyaterm_ui::nya_root(content, window, cx)
        });
        let vcx: &mut VisualTestContext = vcx;
        draw_editor(&app, vcx);
        vcx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.open_connection_account_editor(None, window, cx);
                app.security
                    .apply_editor_input("pw-name", "Deployment".into());
                app.security
                    .apply_editor_input("pw-username", "deploy".into());
                app.security
                    .apply_editor_input("pw-value", " account fixture ".into());
            })
        });
        draw_editor(&app, vcx);
        vcx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.focus_text_input_if_present("security.editor.pw-name", window, cx);
            })
        });
        draw_editor(&app, vcx);
        vcx.simulate_keystrokes("tab");
        draw_editor(&app, vcx);
        assert!(vcx.update(|window, cx| {
            app.read(cx)
                .existing_text_input("security.editor.pw-username")
                .unwrap()
                .read(cx)
                .component_focus_handle(cx)
                .is_focused(window)
        }));
        vcx.simulate_keystrokes("shift-tab");
        draw_editor(&app, vcx);
        assert!(vcx.update(|window, cx| {
            app.read(cx)
                .existing_text_input("security.editor.pw-name")
                .unwrap()
                .read(cx)
                .component_focus_handle(cx)
                .is_focused(window)
        }));
        vcx.simulate_keystrokes("enter");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        while vcx.update(|_, cx| app.read(cx).security.editor_busy()) {
            assert!(
                std::time::Instant::now() < deadline,
                "account save did not complete"
            );
            vcx.run_until_parked();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        draw_editor(&app, vcx);
        let id = vcx.update(|_, cx| {
            let app = app.read(cx);
            assert!(app.security.password_editor().is_none());
            let draft = app
                .connection_state
                .active_editor_draft()
                .expect("parent stays open");
            assert_eq!(draft.username, "root");
            assert_eq!(draft.password_source, ConnectionEditorPasswordSource::Saved);
            assert_eq!(draft.host, "example.com");
            draft.account_id.unwrap()
        });
        assert!(vcx.debug_bounds("connection-account-username").is_some());
        assert!(
            vcx.update(|window, cx| app.read(cx).connection_editor_select_is_focused(window, cx))
        );
        assert!(
            vcx.debug_bounds("connection-editor-saved-password")
                .is_none()
        );
        vcx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.focus_connection_editor_select(ConnectionEditorSelect::Account, window, cx);
                app.open_connection_account_editor(Some(id), window, cx);
                assert!(matches!(
                    app.security.password_editor().unwrap().password_update,
                    PasswordUpdate::Keep
                ));
                app.set_account_password_update(
                    PasswordUpdate::Replace("replacement fixture".into()),
                    cx,
                );
                app.set_account_password_update(PasswordUpdate::Keep, cx);
                assert!(
                    app.existing_text_input("security.editor.pw-value")
                        .is_none()
                );
                app.set_account_password_update(PasswordUpdate::Remove, cx);
                app.set_account_password_update(PasswordUpdate::Keep, cx);
            })
        });
        draw_editor(&app, vcx);
        vcx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.focus_text_input_if_present("security.editor.pw-name", window, cx);
            })
        });
        draw_editor(&app, vcx);
        vcx.simulate_keystrokes("escape");
        draw_editor(&app, vcx);
        assert!(vcx.update(|_, cx| app.read(cx).security.password_editor().is_none()));
        assert!(
            vcx.update(|window, cx| app.read(cx).connection_editor_select_is_focused(window, cx))
        );
        assert!(vcx.update(|_, cx| {
            app.read(cx)
                .connection_state
                .active_editor_draft()
                .is_some()
        }));
    }

    #[test]
    fn connection_editor_categories_fit_wide_and_compact_viewports() {
        use crate::models::ConnectionEditorSection;
        let mut field_selectors = std::collections::HashMap::<_, &'static str>::new();
        for native_window in [false, true] {
            for (width, height, font_size) in [
                (900., 680., 12.),
                (785., 680., 12.),
                (784., 680., 12.),
                (760., 680., 12.),
                (759., 680., 12.),
                (420., 480., 24.),
            ] {
                let test_dir = TestConfigDir::new("nyaterm-connection-editor-layout");
                let mut cx = TestAppContext::single();
                let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
                let (app, vcx) = hosted_editor(&mut cx, test_dir.path(), width, height, font_size);
                vcx.update(|window, cx| {
                    let host = window.root::<EditorHost>().unwrap().unwrap();
                    host.update(cx, |host, cx| {
                        host.native_window = native_window;
                        cx.notify();
                    });
                });
                for kind in [
                    ConnectionKindTab::Ssh,
                    ConnectionKindTab::Local,
                    ConnectionKindTab::Telnet,
                    ConnectionKindTab::Serial,
                    ConnectionKindTab::Rdp,
                    ConnectionKindTab::Vnc,
                ] {
                    vcx.update(|_, cx| {
                        app.update(cx, |app, cx| app.set_connection_editor_kind(kind, cx))
                    });
                    vcx.update(|_, cx| {
                        app.update(cx, |app, cx| {
                            let mut draft = app.connection_state.active_editor_draft().unwrap();
                            draft.auth_mode = "agent".into();
                            draft.agent_forwarding_config.enabled = true;
                            draft.agent_forwarding_config.sources.external_agent = true;
                            draft.agent_forwarding_config.sources.stored_keys = true;
                            draft
                                .agent_forwarding_config
                                .sources
                                .external_agent_endpoints = vec![
                                nyaterm_core::SshAgentEndpoint::Environment {
                                    variable: "SSH_AUTH_SOCK".into(),
                                },
                                nyaterm_core::SshAgentEndpoint::UnixSocket {
                                    path: "/tmp/agent.sock".into(),
                                },
                            ];
                            draft.post_login_enabled = true;
                            draft.ssh_algorithm_mode = "custom".into();
                            draft.rdp_display.mode = "custom".into();
                            app.connection_state.begin_editor(draft);
                            app.connection_state.build_editor_fields(cx);
                        })
                    });
                    for section in ConnectionEditorSection::for_kind(kind) {
                        vcx.update(|_, cx| {
                            app.update(cx, |app, cx| {
                                app.set_connection_editor_section(*section, cx)
                            })
                        });
                        draw_editor(&app, vcx);
                        let host = vcx.debug_bounds("connection-editor-test-host").unwrap();
                        let panel = vcx.debug_bounds("connection-editor-panel").unwrap();
                        let scroll = vcx.debug_bounds("connection-editor-scroll").unwrap();
                        let body = vcx.debug_bounds(protocol_selectors(kind).0).unwrap();
                        if *section == ConnectionEditorSection::Basic
                            && matches!(
                                kind,
                                ConnectionKindTab::Ssh
                                    | ConnectionKindTab::Telnet
                                    | ConnectionKindTab::Rdp
                                    | ConnectionKindTab::Vnc
                            )
                        {
                            let host = vcx.debug_bounds("connection-editor-field-Host").unwrap();
                            let username = vcx
                                .debug_bounds("connection-editor-field-Username")
                                .unwrap();
                            let description =
                                vcx.debug_bounds("connection-editor-description").unwrap();
                            assert!(host.bottom() <= username.top(), "{kind:?}");
                            assert!(username.bottom() <= description.top(), "{kind:?}");
                        }
                        assert!(
                            panel.left() >= host.left() && panel.right() <= host.right(),
                            "{kind:?}/{section:?}: {panel:?} in {host:?}"
                        );
                        assert!(panel.top() >= host.top() && panel.bottom() <= host.bottom());
                        assert!(scroll.size.height > px(0.));
                        assert!(
                            body.left() >= scroll.left() && body.right() <= scroll.right(),
                            "{kind:?}/{section:?}: {body:?} in {scroll:?}"
                        );
                        let field_keys = vcx.update(|_, cx| {
                            let state = &app.read(cx).connection_state;
                            state
                                .editor_fields()
                                .keys()
                                .chain(state.editor_number_fields().keys())
                                .copied()
                                .collect::<Vec<_>>()
                        });
                        for field in field_keys {
                            // GPUI's test selector API requires static strings. Intern each
                            // field name once for this matrix rather than once per frame.
                            let selector = field_selectors.entry(field).or_insert_with(|| {
                                Box::leak(
                                    format!("connection-editor-field-{field:?}").into_boxed_str(),
                                )
                            });
                            if let Some(bounds) = vcx.debug_bounds(selector) {
                                assert!(bounds.size.width > px(0.));
                                assert!(
                                    bounds.left() >= scroll.left()
                                        && bounds.right() <= scroll.right(),
                                    "{kind:?}/{section:?}/{field:?} at {width}: {bounds:?} in {scroll:?}"
                                );
                            }
                        }
                        let navigation = vcx.debug_bounds("connection-editor-navigation").unwrap();
                        assert!(navigation.right() <= scroll.left());
                        assert_eq!(navigation.top(), scroll.top());
                        assert_eq!(navigation.size.height, scroll.size.height);
                        for selector in ["connection-editor-save", "connection-editor-close"] {
                            let button = vcx.debug_bounds(selector).unwrap();
                            assert!(
                                button.left() >= panel.left() && button.right() <= panel.right()
                            );
                            assert!(
                                button.top() >= panel.top() && button.bottom() <= panel.bottom()
                            );
                        }
                    }
                }
            }
        }
    }
    #[gpui::test]
    fn category_navigation_preserves_input_entities_and_keyboard_focus(cx: &mut TestAppContext) {
        use crate::models::ConnectionEditorSection;
        let test_dir = TestConfigDir::new("nyaterm-editor-navigation-entities");
        let (app, vcx) = hosted_editor(cx, test_dir.path(), 900., 680., 12.);
        let original = vcx.update(|_, cx| {
            app.read(cx).connection_state.editor_fields()[&ConnectionEditorField::Name].clone()
        });
        let component = original.read_with(vcx, |input, _| input.component_state().unwrap());
        for width in [420., 759., 760., 900., 420.] {
            vcx.update(|window, cx| {
                window
                    .root::<EditorHost>()
                    .unwrap()
                    .unwrap()
                    .update(cx, |host, cx| {
                        host.width = width;
                        host.height = 480.;
                        cx.notify();
                    });
            });
            vcx.simulate_resize(gpui::size(px(width), px(480.)));
            draw_editor(&app, vcx);
            assert!(vcx.debug_bounds("connection-editor-navigation").is_some());
        }
        vcx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.connection_state
                    .set_editor_field_error(ConnectionEditorField::Name, "retained error".into());
                window.focus(&app.connection_state.editor_navigation_focus(), cx);
            });
        });
        vcx.simulate_keystrokes("down");
        draw_editor(&app, vcx);
        assert!(vcx.update(|window, cx| {
            let state = &app.read(cx).connection_state;
            assert_eq!(
                state.active_editor_draft().unwrap().section,
                ConnectionEditorSection::Network
            );
            state.editor_navigation_focus().is_focused(window)
        }));
        vcx.simulate_keystrokes("tab");
        draw_editor(&app, vcx);
        assert!(vcx.update(|window, cx| {
            !app.read(cx)
                .connection_state
                .editor_navigation_focus()
                .is_focused(window)
        }));
        vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.set_connection_editor_section(ConnectionEditorSection::Basic, cx)
            })
        });
        draw_editor(&app, vcx);
        vcx.update(|_, cx| {
            let state = &app.read(cx).connection_state;
            let current = &state.editor_fields()[&ConnectionEditorField::Name];
            assert_eq!(current.entity_id(), original.entity_id());
            assert_eq!(
                current.read(cx).component_state().unwrap().entity_id(),
                component.entity_id()
            );
            assert_eq!(
                state.active_editor_draft().unwrap().error.as_deref(),
                Some("retained error")
            );
        });
    }

    #[gpui::test]
    fn save_reveals_focuses_and_scrolls_to_an_invalid_hidden_field(cx: &mut TestAppContext) {
        use crate::models::ConnectionEditorSection;
        let test_dir = TestConfigDir::new("nyaterm-editor-validation-navigation");
        let (app, vcx) = hosted_editor(cx, test_dir.path(), 420., 480., 24.);
        vcx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.connection_state
                    .set_editor_field_text(ConnectionEditorField::Host, "example.test".into());
                app.connection_state
                    .set_editor_field_text(ConnectionEditorField::Username, "root".into());
                app.connection_state.set_editor_field_text(
                    ConnectionEditorField::SftpShellDetectionTimeout,
                    "invalid".into(),
                );
                app.save_connection_editor(window, cx);
            })
        });
        for _ in 0..6 {
            draw_editor(&app, vcx);
        }
        vcx.update(|window, cx| {
            let state = &app.read(cx).connection_state;
            let draft = state.active_editor_draft().unwrap();
            assert_eq!(draft.section, ConnectionEditorSection::Sftp);
            assert!(
                draft
                    .field_errors
                    .contains_key(&ConnectionEditorField::SftpShellDetectionTimeout)
            );
            let focus = state
                .editor_fields()
                .get(&ConnectionEditorField::SftpShellDetectionTimeout)
                .map(|input| input.read(cx).component_focus_handle(cx))
                .or_else(|| {
                    state
                        .editor_number_fields()
                        .get(&ConnectionEditorField::SftpShellDetectionTimeout)
                        .map(|input| input.read(cx).component_focus_handle(cx))
                })
                .unwrap();
            assert!(focus.is_focused(window));
        });
        let field = vcx
            .debug_bounds("connection-editor-field-SftpShellDetectionTimeout")
            .unwrap();
        let viewport = vcx.debug_bounds("connection-editor-scroll").unwrap();
        assert!(
            field.top() >= viewport.top() && field.top() < viewport.bottom(),
            "{field:?} in {viewport:?}"
        );
    }

    #[gpui::test]
    fn save_returns_to_basic_and_scrolls_to_an_invalid_authentication_field(
        cx: &mut TestAppContext,
    ) {
        use crate::models::ConnectionEditorSection;
        let test_dir = TestConfigDir::new("nyaterm-editor-auth-validation");
        let (app, vcx) = hosted_editor(cx, test_dir.path(), 420., 480., 24.);
        vcx.update(|window, cx| {
            app.update(cx, |app, cx| {
                let mut draft = app.connection_state.active_editor_draft().unwrap();
                draft.host = "example.test".into();
                draft.username = "root".into();
                draft.auth_mode = "agent".into();
                draft.agent_endpoint = nyaterm_core::SshAgentEndpoint::UnixSocket {
                    path: String::new(),
                };
                app.connection_state.begin_editor(draft);
                app.connection_state.build_editor_fields(cx);
                app.set_connection_editor_section(ConnectionEditorSection::Network, cx);
                app.save_connection_editor(window, cx);
            });
        });
        for _ in 0..6 {
            draw_editor(&app, vcx);
        }
        vcx.update(|window, cx| {
            let state = &app.read(cx).connection_state;
            let draft = state.active_editor_draft().unwrap();
            assert_eq!(draft.section, ConnectionEditorSection::Basic);
            assert!(
                draft
                    .field_errors
                    .contains_key(&ConnectionEditorField::AgentUnixSocket)
            );
            let input = &state.editor_fields()[&ConnectionEditorField::AgentUnixSocket];
            assert!(input.read(cx).component_focus_handle(cx).is_focused(window));
        });
        let field = vcx
            .debug_bounds("connection-editor-field-AgentUnixSocket")
            .unwrap();
        let viewport = vcx.debug_bounds("connection-editor-scroll").unwrap();
        assert!(
            field.top() >= viewport.top() && field.top() < viewport.bottom(),
            "{field:?} in {viewport:?}"
        );
    }

    #[gpui::test]
    fn agent_identity_picker_escape_preserves_the_connection_editor(cx: &mut TestAppContext) {
        let test_dir = TestConfigDir::new("nyaterm-editor-agent-picker-escape");
        let (app, vcx) = hosted_editor(cx, test_dir.path(), 900., 680., 12.);
        vcx.update(|window, cx| {
            app.update(cx, |app, cx| {
                let input = &app.connection_state.editor_fields()[&ConnectionEditorField::Name];
                window.focus(&input.read(cx).component_focus_handle(cx), cx);
                app.set_connection_editor_agent_identity_picker_open(true, cx);
            });
        });
        draw_editor(&app, vcx);
        vcx.simulate_keystrokes("escape");
        draw_editor(&app, vcx);
        vcx.update(|_, cx| {
            let state = &app.read(cx).connection_state;
            assert!(!state.editor_agent_identity_picker_is_open());
            assert!(state.editor_has_draft());
        });
    }

    #[gpui::test]
    fn invalid_hidden_baud_rate_opens_its_picker_and_focuses_the_custom_input(
        cx: &mut TestAppContext,
    ) {
        use crate::models::ConnectionEditorSection;
        let test_dir = TestConfigDir::new("nyaterm-editor-baud-validation");
        let (app, vcx) = hosted_editor(cx, test_dir.path(), 420., 480., 24.);
        vcx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.set_connection_editor_kind(ConnectionKindTab::Serial, cx);
                app.connection_state
                    .set_editor_field_text(ConnectionEditorField::SerialPort, "COM1".into());
                app.connection_state
                    .set_editor_field_text(ConnectionEditorField::BaudRate, "0".into());
                app.save_connection_editor(window, cx);
            });
        });
        for _ in 0..6 {
            draw_editor(&app, vcx);
        }
        vcx.update(|window, cx| {
            let state = &app.read(cx).connection_state;
            assert_eq!(
                state.active_editor_draft().unwrap().section,
                ConnectionEditorSection::Serial
            );
            assert!(state.editor_baud_popover_is_open());
            let input = &state.editor_number_fields()[&ConnectionEditorField::BaudRate];
            assert!(input.read(cx).component_focus_handle(cx).is_focused(window));
        });
    }
}
