mod local;
mod rdp;
mod recording;
mod serial;
mod ssh;
mod telnet;
mod vnc;

use rust_i18n::t;

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use gpui::{
    AnyElement, App, Bounds, Context, Entity, FontWeight, IntoElement, KeyDownEvent, Pixels, Point,
    SharedString, anchored, deferred, div,
    prelude::{
        FluentBuilder, InteractiveElement, ParentElement, StatefulInteractiveElement, Styled,
    },
    px, rgb, rgba, svg,
};
use nyaterm_core::{ConnectionType, Group, SavedConnection, natural_compare, truncate_preview};
use nyaterm_ui::{
    NyaCheckbox, NyaInput, NyaPopover, NyaScrollArea, NyaScrollable, NyaSelectOption,
    NyaSelectState, NyaTabItem, NyaTabs,
};

use self::local::connection_editor_local_section;
use self::rdp::connection_editor_rdp_section;
use self::recording::connection_editor_recording_section;
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
    view_widgets::modal_dialog_shell, view_widgets::themed_icon,
};
use crate::models::{
    ConnectionEditorField, ConnectionEditorSelect, ConnectionEditorState, ConnectionKindTab,
};

#[derive(Clone, Copy)]
struct ConnectionEditorSectionContext<'a> {
    palette: crate::theme::ThemePalette,
    editor: &'a ConnectionEditorState,
    fields: &'a ConnectionEditorFields,
}

impl NyaTermApp {
    pub(in crate::features) fn connection_editor_panel(
        &mut self,
        editor: ConnectionEditorState,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.connection_editor_surface(editor, false, cx)
    }

    pub(in crate::features) fn connection_editor_window_view(
        &mut self,
        editor: ConnectionEditorState,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.connection_editor_surface(editor, true, cx)
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
                );
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
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.connection_state
            .ensure_editor_forwarding_endpoint_fields(cx);
        let palette = self.theme_palette();
        let title = if editor.id.is_some() {
            t!("dialog.editConnection")
        } else {
            t!("dialog.newConnection")
        };
        let local_label = t!("dialog.localTerminal");
        let serial_label = t!("dialog.serial");
        let name_label = t!("dialog.connectionName");
        let description_label = t!("dialog.description");
        let group_title = t!("dialog.group");
        let cancel_label = t!("common.cancel");
        let save_label = t!("common.save");
        let none_label = t!("dialog.none");
        let icon_label = t!("dialog.icon");
        let icon_auto_detect_label = t!("dialog.iconAutoDetect");
        let icon_auto_detect_hint = t!("dialog.iconAutoDetectTooltip");
        let icon_auto_detect = editor.icon_auto_detect;
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
            .password_id
            .as_deref()
            .and_then(|id| {
                self.security
                    .passwords()
                    .iter()
                    .find(|password| password.id == id)
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
            editor.password_id.is_none(),
        )];
        password_options.extend(self.security.passwords().iter().map(|password| {
            ConnectionEditorChoice::new(
                Some(password.id.clone()),
                password.name.clone(),
                editor.password_id.as_deref() == Some(password.id.as_str()),
            )
        }));
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
        let encoding_options = ["global", "UTF-8", "GBK", "GB2312", "GB18030"]
            .into_iter()
            .map(|value| {
                let label = if value == "global" {
                    t!("connection.encodingFollowGlobal").to_string()
                } else {
                    value.to_string()
                };
                ConnectionEditorChoice::new(
                    Some(value.to_string()),
                    label,
                    editor.encoding == value,
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
        let rdp_certificate_options = [
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
        let sftp_filename_encoding_options = ["terminal", "UTF-8", "GBK", "GB2312", "GB18030"]
            .into_iter()
            .map(|value| {
                let label = if value == "terminal" {
                    t!("dialog.sftpFilenameEncodingFollowTerminal").to_string()
                } else {
                    value.to_string()
                };
                ConnectionEditorChoice::new(
                    Some(value.to_string()),
                    label,
                    editor.sftp_filename_encoding == value,
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
        let mut selects = HashMap::new();
        for (select_key, choices, placeholder) in [
            (
                ConnectionEditorSelect::SavedPassword,
                password_options.as_slice(),
                password_label.clone(),
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
                t!("dialog.selectSerialPort").to_string(),
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
                ConnectionEditorSelect::StopBits,
                stop_bits_options.as_slice(),
                String::new(),
            ),
        ] {
            selects.insert(
                select_key,
                self.connection_editor_select_entity(select_key, choices, placeholder, cx),
            );
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
        let fields = ConnectionEditorFields::new(
            self.connection_state.editor_fields().clone(),
            self.connection_state.editor_number_fields().clone(),
            selects,
            forwarding_endpoint_selects,
            forwarding_endpoint_fields,
        );
        let icon_key = editor.icon.as_deref();
        let icon_def = resolve_connection_icon(icon_key, editor.kind.label());
        let icon_picker_open = self.connection_state.editor_icon_picker_is_open();
        let group_select_open = self.connection_state.editor_group_select_is_open();
        let agent_identity_picker_open =
            self.connection_state.editor_agent_identity_picker_is_open();
        let icon_picker_bg = if native_window {
            rgb(palette.surface)
        } else {
            self.shell_surface_color(palette.surface)
        };
        let validation_error = self.connection_editor_validation_error(&editor);
        let save_enabled = validation_error.is_none();
        let editor_focus = self.connection_state.editor_focus_handle();
        let section_context = ConnectionEditorSectionContext {
            palette,
            editor: &editor,
            fields: &fields,
        };
        let mut icon_grid = div().grid().grid_cols(7).gap_1();
        for icon_key in CONNECTION_ICON_OPTIONS.iter().copied() {
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
                    .child(themed_icon(palette, icon, false, 16.))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.set_connection_editor_icon(Some(icon_key), cx);
                    })),
            );
        }
        for record in self.connection_state.custom_icons.records.clone() {
            let Some(image) = self
                .connection_state
                .custom_icons
                .images
                .get(&record.id)
                .cloned()
            else {
                continue;
            };
            let id = record.id.clone();
            icon_grid = icon_grid.child(
                nyaterm_ui::NyaButton::new(format!("custom-icon-{}", record.id), "")
                    .tooltip(record.name)
                    .content(gpui::img(image).size(px(20.)))
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
            .occlude()
            .w(px(232.))
            .p_2()
            .rounded_md()
            .border_1()
            .border_color(rgb(palette.border))
            .bg(icon_picker_bg)
            .shadow_lg()
            .child(
                div()
                    .max_h(px(280.))
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
            // Only SSH reports a remote system, so the toggle would be
            // inert on the other kinds.
            .when(
                editor.kind == ConnectionKindTab::Ssh
                    && editor.ssh_profile == nyaterm_core::SshProfile::Standard,
                |this| {
                    this.child(
                        div()
                            .id("connection-editor-icon-auto-detect")
                            .mt_2()
                            .pt_2()
                            .border_t_1()
                            .border_color(rgb(palette.border))
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .cursor_pointer()
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
                            .child(crate::features::pages::settings::settings_switch(
                                palette,
                                "connection-editor-icon-auto-detect-switch",
                                icon_auto_detect,
                                cx.listener(move |this, _, _, cx| {
                                    this.set_connection_editor_icon_auto_detect(
                                        !icon_auto_detect,
                                        cx,
                                    );
                                }),
                            ))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.set_connection_editor_icon_auto_detect(!icon_auto_detect, cx);
                            })),
                    )
                },
            );
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

        let card = div()
            .id(SharedString::from("connection-editor-panel"))
            .w_full()
            .when(native_window, |this| this.size_full())
            .when(!native_window, |this| this.max_h(px(640.)))
            .flex()
            .flex_col()
            // No blanket focus grab here: it existed to keep the old label-div
            // inputs "focused", and would now steal focus back from whichever
            // field the pointer just landed on, since click follows mouse-down.
            .track_focus(&editor_focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if this.connection_editor_select_is_focused(window, cx) {
                    return;
                }
                if this.handle_connection_editor_key_down(event, window, cx) {
                    cx.stop_propagation();
                }
            }))
            .when(!native_window, |this| {
                this.child(
                    div()
                        .px_4()
                        .pt_4()
                        .text_size(px(15.))
                        .font_weight(FontWeight(700.))
                        .text_color(rgb(palette.text))
                        .child(title),
                )
            })
            .child(
                div()
                    .flex_none()
                    .px_4()
                    .pt_4()
                    .when(!native_window, |this| this.pt_3())
                    .child(
                        NyaTabs::new("connection-kind-tabs")
                            .items([
                                NyaTabItem::new("SSH"),
                                NyaTabItem::new(local_label),
                                NyaTabItem::new("Telnet"),
                                NyaTabItem::new(serial_label),
                                NyaTabItem::new("RDP"),
                                NyaTabItem::new("VNC"),
                            ])
                            .selected_index(match editor.kind {
                                ConnectionKindTab::Ssh => 0,
                                ConnectionKindTab::Local => 1,
                                ConnectionKindTab::Telnet => 2,
                                ConnectionKindTab::Serial => 3,
                                ConnectionKindTab::Rdp => 4,
                                ConnectionKindTab::Vnc => 5,
                            })
                            .on_select(cx.listener(|this, index, _, cx| {
                                let kind = match *index {
                                    0 => ConnectionKindTab::Ssh,
                                    1 => ConnectionKindTab::Local,
                                    2 => ConnectionKindTab::Telnet,
                                    3 => ConnectionKindTab::Serial,
                                    4 => ConnectionKindTab::Rdp,
                                    _ => ConnectionKindTab::Vnc,
                                };
                                this.set_connection_editor_kind(kind, cx);
                            })),
                    ),
            )
            .child(
                div()
                    .id("connection-editor-scroll")
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .on_scroll_wheel(cx.listener(|this, _: &gpui::ScrollWheelEvent, window, cx| {
                        if this.connection_editor_select_menu_is_focused(window, cx) {
                            cx.stop_propagation();
                        }
                    }))
                    .p_4()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_end()
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
                                div().min_w(px(192.)).max_w(px(288.)).flex_1().child(
                                    connection_editor_group_control(
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
                                    ),
                                ),
                            ),
                    )
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
                    .when(
                        matches!(editor.kind, ConnectionKindTab::Rdp | ConnectionKindTab::Vnc),
                        |this| {
                            this.child(crate::features::pages::connections::list::connection_editor_select(
                                crate::features::pages::connections::list::ConnectionEditorRenderContext {
                                    palette,
                                    fields: &fields,
                                    cx,
                                },
                                "remote-desktop-proxy",
                                t!("dialog.proxySelect"),
                                ConnectionEditorSelect::Proxy,
                            ))
                            .child(
                                crate::features::pages::connections::list::connection_editor_select(
                                    crate::features::pages::connections::list::ConnectionEditorRenderContext {
                                        palette,
                                        fields: &fields,
                                        cx,
                                    },
                                    "remote-desktop-jump",
                                    t!("dialog.proxyJump"),
                                    ConnectionEditorSelect::ProxyJump,
                                ),
                            )
                        },
                    )
                    .when(editor.kind == ConnectionKindTab::Rdp, |this| {
                        this.child(connection_editor_rdp_section(section_context, cx))
                    })
                    .when(editor.kind == ConnectionKindTab::Vnc, |this| {
                        this.child(connection_editor_vnc_section(section_context, cx))
                    })
                    .when(
                        !matches!(editor.kind, ConnectionKindTab::Rdp | ConnectionKindTab::Vnc),
                        |this| this.child(connection_editor_recording_section(section_context, cx)),
                    )
                    .child(connection_description_field(
                        palette,
                        description_label,
                        &fields,
                        cx,
                    ))
                    .when_some(editor.error.clone(), |this, error| {
                        this.child(
                            div()
                                .text_size(px(12.))
                                .text_color(rgb(palette.danger))
                                .child(error),
                        )
                    }),
            )
            .child(
                div()
                    .h(px(52.))
                    .flex_none()
                    .border_t_1()
                    .border_color(rgb(palette.border))
                    .bg(rgb(palette.section_header))
                    .px_5()
                    .py_3()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .text_size(px(10.))
                            .text_color(rgb(palette.danger))
                            .child(validation_error.unwrap_or_default()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(connection_editor_footer_button(
                                palette,
                                "connection-editor-close",
                                cancel_label,
                                false,
                                true,
                                cx.listener(|this, _, _, cx| {
                                    this.close_connection_editor(cx);
                                }),
                            ))
                            .child(connection_editor_footer_button(
                                palette,
                                "connection-editor-save",
                                save_label,
                                true,
                                save_enabled,
                                cx.listener(|this, _, window, cx| {
                                    this.save_connection_editor(window, cx);
                                }),
                            )),
                    ),
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
            });
        if native_window {
            surface.into_any_element()
        } else {
            modal_dialog_shell(
                palette,
                self.shell_surface_color(palette.bg),
                "connection-editor-modal",
                560.,
                surface,
            )
            .into_any_element()
        }
    }
}

impl NyaTermApp {
    fn connection_editor_select_is_focused(&self, window: &gpui::Window, cx: &gpui::App) -> bool {
        connection_editor_select_keys()
            .into_iter()
            .any(|select| self.select_is_focused(connection_editor_select_id(select), window, cx))
            || self.select_with_prefix_is_focused(
                "connection-editor-ssh-agent-forwarding-endpoint-",
                window,
                cx,
            )
    }

    fn connection_editor_select_menu_is_focused(
        &self,
        window: &gpui::Window,
        cx: &gpui::App,
    ) -> bool {
        connection_editor_select_keys().into_iter().any(|select| {
            self.select_menu_is_focused(connection_editor_select_id(select), window, cx)
        }) || self.select_menu_with_prefix_is_focused(
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

fn connection_editor_select_keys() -> [ConnectionEditorSelect; 29] {
    [
        ConnectionEditorSelect::Group,
        ConnectionEditorSelect::SavedPassword,
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
    }
}

fn connection_editor_select_is_searchable(select: ConnectionEditorSelect) -> bool {
    matches!(
        select,
        ConnectionEditorSelect::SavedPassword
            | ConnectionEditorSelect::SshKey
            | ConnectionEditorSelect::Otp
            | ConnectionEditorSelect::Proxy
            | ConnectionEditorSelect::ProxyJump
    )
}

fn connection_editor_select_search_placeholder(select: ConnectionEditorSelect) -> Option<String> {
    match select {
        ConnectionEditorSelect::SavedPassword => Some(t!("dialog.selectPassword").to_string()),
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
                // Fixed: in a flex column the box would otherwise shrink to
                // whatever space the form had left, cutting a row in half.
                .h(px(72.))
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
                .py_2()
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
    palette: crate::theme::ThemePalette,
    id: &'static str,
    label: impl Into<SharedString>,
    primary: bool,
    enabled: bool,
    on_click: impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    let label: SharedString = label.into();
    let background = if primary {
        palette.primary
    } else {
        palette.surface_elevated
    };
    let text = if primary {
        palette.on_primary
    } else {
        palette.text
    };
    div()
        .id(id)
        .h(px(28.))
        .px_3()
        .flex()
        .items_center()
        .rounded_sm()
        .border_1()
        .border_color(if primary {
            rgb(palette.primary)
        } else {
            rgb(palette.border)
        })
        .bg(rgb(background))
        .text_color(rgb(text))
        .text_xs()
        .opacity(if enabled { 1.0 } else { 0.45 })
        .when(enabled, |this| {
            this.cursor_pointer()
                .hover(move |this| this.opacity(0.86))
                .on_click(on_click)
        })
        .child(label)
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
    use nyaterm_core::Group;

    use super::{connection_editor_group_menu_options, ordered_connection_groups};
    use crate::features::selects::PENDING_CONNECTION_GROUP_VALUE;
    use crate::models::{
        ConnectionEditorAdvancedTab, ConnectionEditorField, ConnectionEditorPasswordSource,
        ConnectionEditorState, ConnectionEditorTelnetTab, ConnectionKindTab,
    };

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
            rdp_security: Default::default(),
            rdp_display: Default::default(),
            rdp_clipboard: Default::default(),
            rdp_reconnect: Default::default(),
            rdp_advanced_tab: crate::models::ConnectionEditorRdpTab::Security,
            vnc_security: Default::default(),
            vnc_display: Default::default(),
            vnc_clipboard: Default::default(),
            vnc_reconnect: Default::default(),
            vnc_shared: true,
            vnc_view_only: false,
            password_source: ConnectionEditorPasswordSource::Ask,
            password_id: None,
            password: nyaterm_core::SecretString::default(),
            existing_password: None,
            key_id: None,
            otp_id: None,
            auto_fill_otp: false,
            proxy_id: None,
            proxy_jump_id: None,
            x11_forwarding: false,
            dynamic_tab_title: false,
            agent_endpoint: Default::default(),
            agent_forwarding_config: Default::default(),
            agent_allow_all_confirmed: false,
            agent_forwarding_endpoint_index: 0,
            agent_preview: None,
            agent_preview_loading: false,
            backspace_mode: "del".to_string(),
            encoding: "global".to_string(),
            ssh_profile: Default::default(),
            terminal_type: None,
            sftp_enabled: true,
            sftp_cwd_follow_mode: "shell_integration".to_string(),
            sftp_shell_detection_timeout_ms: "3000".to_string(),
            sftp_pipeline_depth: None,
            sftp_extra: Default::default(),
            sftp_filename_encoding: "terminal".to_string(),
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
            telnet_auto_login_timeout_ms: "60000".to_string(),
            telnet_auto_login_username_prompt_regex: String::new(),
            telnet_auto_login_password_prompt_regex: String::new(),
            telnet_auto_login_success_prompt_regex: String::new(),
            telnet_auto_login_failure_prompt_regex: String::new(),
            telnet_auto_login_max_retries: "0".to_string(),
            post_login_enabled: false,
            post_login_command: String::new(),
            post_login_delay_ms: "0".to_string(),
            advanced_open: false,
            advanced_network_tab: ConnectionEditorAdvancedTab::Proxy,
            advanced_behavior_tab: ConnectionEditorAdvancedTab::PostLogin,
            telnet_advanced_tab: ConnectionEditorTelnetTab::Input,
            connect_after_save: false,
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
}
