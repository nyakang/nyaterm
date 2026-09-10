use rust_i18n::t;

use std::collections::HashSet;

use gpui::{
    Context, FontWeight, IntoElement, SharedString, div,
    prelude::{
        FluentBuilder, InteractiveElement, ParentElement, StatefulInteractiveElement, Styled,
    },
    px, rgb, rgba, svg,
};

use nyaterm_core::truncate_preview;
use nyaterm_transport::{SshAlgorithmOption, SshAlgorithmRisk};
use nyaterm_ui::{NyaCheckbox, NyaScrollable, NyaSelect, NyaTabItem, NyaTabs, NyaTooltip};

use crate::features::{NyaTermApp, connections::ConnectionEditorToggle};
use crate::models::{
    ConnectionEditorAdvancedTab, ConnectionEditorField, ConnectionEditorPasswordSource,
    ConnectionEditorSelect, ConnectionEditorSshAlgorithmTab,
};

use super::super::super::list::{
    ConnectionEditorChoice, ConnectionEditorRenderContext, EDITOR_CONTROL_HEIGHT_PX,
    connection_editor_select, editor_field, editor_stepper_field, forwarding_endpoint_editor_field,
    required, toggle_chip,
};

use super::ConnectionEditorSectionContext;

pub(super) struct SshConnectionSectionLabels {
    pub(super) otp: String,
    pub(super) proxy: String,
    pub(super) jump: String,
}

pub(super) struct SshConnectionSectionOptions {
    pub(super) auth: Vec<ConnectionEditorChoice>,
}

fn ssh_advanced_content(
    palette: crate::theme::ThemePalette,
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
    content: impl IntoElement,
) -> impl IntoElement {
    let title: SharedString = title.into();
    let description = description.into();
    div()
        .rounded_md()
        .border_1()
        .border_color(rgb(palette.border))
        .bg(rgb(palette.bg))
        .p_3()
        .flex()
        .flex_col()
        .gap_3()
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight(600.))
                        .text_color(rgb(palette.text))
                        .child(title),
                )
                .child(
                    div()
                        .text_size(px(10.))
                        .text_color(rgb(palette.text_muted))
                        .child(description),
                ),
        )
        .child(content)
}

fn ssh_algorithm_move_button(
    palette: crate::theme::ThemePalette,
    id: String,
    icon: &'static str,
    tooltip: String,
    enabled: bool,
    on_click: impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    let mut button = div()
        .id(SharedString::from(id))
        .size(px(24.))
        .flex_none()
        .rounded_sm()
        .flex()
        .items_center()
        .justify_center()
        .child(
            svg()
                .size(px(13.))
                .path(icon)
                .text_color(rgb(palette.text_muted)),
        )
        .tooltip(move |window, cx| NyaTooltip::new(tooltip.clone()).build(window, cx));
    if enabled {
        button = button
            .cursor_pointer()
            .hover(move |this| this.bg(rgb(palette.hover)))
            .on_click(on_click);
    } else {
        button = button.opacity(0.35);
    }
    button
}

fn ssh_algorithm_list(
    palette: crate::theme::ThemePalette,
    tab: ConnectionEditorSshAlgorithmTab,
    options: &[SshAlgorithmOption],
    selected_values: &[String],
    cx: &mut Context<NyaTermApp>,
) -> impl IntoElement {
    let selected = selected_values.iter().cloned().collect::<HashSet<_>>();
    let option_by_id = options
        .iter()
        .map(|option| (option.id.as_str(), option.risk))
        .collect::<std::collections::HashMap<_, _>>();
    let mut rows = selected_values
        .iter()
        .map(|id| (id.clone(), option_by_id.get(id.as_str()).copied(), true))
        .collect::<Vec<_>>();
    rows.extend(
        options
            .iter()
            .filter(|option| !selected.contains(&option.id))
            .map(|option| (option.id.clone(), Some(option.risk), false)),
    );

    let mut list = div()
        .id(SharedString::from(format!(
            "connection-ssh-algorithm-list-{tab:?}"
        )))
        .max_h(px(224.))
        .overflow_y_scrollbar()
        .flex()
        .flex_col()
        .gap_1();
    for (row_index, (id, risk, enabled)) in rows.into_iter().enumerate() {
        let risk_label = match risk {
            Some(SshAlgorithmRisk::Modern) => t!("dialog.algorithmRiskModern"),
            Some(SshAlgorithmRisk::Legacy) => t!("dialog.algorithmRiskLegacy"),
            Some(SshAlgorithmRisk::Insecure) => t!("dialog.algorithmRiskInsecure"),
            None => t!("dialog.algorithmUnsupported"),
        };
        let risk_color = match risk {
            Some(SshAlgorithmRisk::Modern) => palette.success,
            Some(SshAlgorithmRisk::Legacy) => palette.warning,
            Some(SshAlgorithmRisk::Insecure) | None => palette.danger,
        };
        let selected_index = selected_values.iter().position(|value| value == &id);
        let move_up_enabled = selected_index.is_some_and(|index| index > 0);
        let move_down_enabled =
            selected_index.is_some_and(|index| index + 1 < selected_values.len());
        let checkbox_id = format!("connection-ssh-algorithm-checkbox-{tab:?}-{row_index}");
        let app = cx.weak_entity();
        let checkbox_algorithm = id.clone();
        let checkbox = NyaCheckbox::new(checkbox_id)
            .checked(enabled)
            .disabled(enabled && selected_values.len() <= 1)
            .on_click(move |_, _, cx| {
                let _ = app.update(cx, |app, cx| {
                    app.set_connection_editor_ssh_algorithm_enabled(
                        tab,
                        &checkbox_algorithm,
                        !enabled,
                        cx,
                    );
                });
            });
        let move_up_id = id.clone();
        let move_down_id = id.clone();
        list = list.child(
            div()
                .min_h(px(38.))
                .rounded_sm()
                .border_1()
                .border_color(rgb(palette.border))
                .bg(if enabled {
                    rgba((palette.accent << 8) | 0x18)
                } else {
                    rgba((palette.surface << 8) | 0x18)
                })
                .px_2()
                .py_1()
                .flex()
                .items_center()
                .gap_2()
                .opacity(if enabled { 1.0 } else { 0.65 })
                .child(div().flex_none().child(checkbox))
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .truncate()
                                .text_size(px(10.))
                                .font_family(crate::features::shell::gpui_code_font_family())
                                .text_color(rgb(palette.text))
                                .child(id),
                        )
                        .child(
                            div()
                                .flex_none()
                                .rounded_sm()
                                .border_1()
                                .border_color(rgba((risk_color << 8) | 0x66))
                                .bg(rgba((risk_color << 8) | 0x18))
                                .px_1()
                                .text_size(px(9.))
                                .text_color(rgb(risk_color))
                                .child(risk_label),
                        ),
                )
                .child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(ssh_algorithm_move_button(
                            palette,
                            format!("connection-ssh-algorithm-up-{tab:?}-{row_index}"),
                            "icons/chevron-up.svg",
                            t!("dialog.moveUp").to_string(),
                            move_up_enabled,
                            cx.listener(move |this, _, _, cx| {
                                this.move_connection_editor_ssh_algorithm(tab, &move_up_id, -1, cx);
                            }),
                        ))
                        .child(ssh_algorithm_move_button(
                            palette,
                            format!("connection-ssh-algorithm-down-{tab:?}-{row_index}"),
                            "icons/chevron-down.svg",
                            t!("dialog.moveDown").to_string(),
                            move_down_enabled,
                            cx.listener(move |this, _, _, cx| {
                                this.move_connection_editor_ssh_algorithm(
                                    tab,
                                    &move_down_id,
                                    1,
                                    cx,
                                );
                            }),
                        )),
                ),
        );
    }
    list
}

pub(super) fn connection_editor_ssh_section(
    section: ConnectionEditorSectionContext<'_>,
    labels: SshConnectionSectionLabels,
    options: SshConnectionSectionOptions,
    cx: &mut Context<NyaTermApp>,
) -> gpui::Div {
    let ConnectionEditorSectionContext {
        palette,
        editor,
        fields,
    } = section;
    let SshConnectionSectionLabels {
        otp: otp_label,
        proxy: proxy_label,
        jump: jump_label,
    } = labels;
    let SshConnectionSectionOptions { auth: auth_options } = options;

    let auth_tab_options = auth_options
        .iter()
        .filter_map(|option| option.value.as_ref().map(|value| (value, option)))
        .collect::<Vec<_>>();
    let auth_tab_values: Vec<String> = auth_tab_options
        .iter()
        .map(|(value, _)| (*value).clone())
        .collect::<Vec<_>>();
    let auth_tabs = NyaTabs::new("connection-authentication-tabs")
        .items(
            auth_tab_options
                .iter()
                .map(|(_, option)| NyaTabItem::new(option.label.clone())),
        )
        .selected_index_if_visible(
            auth_tab_options
                .iter()
                .position(|(_, option)| option.selected),
        )
        .on_select(cx.listener(move |this, index: &usize, _, cx| {
            let Some(value) = auth_tab_values.get(*index) else {
                return;
            };
            this.set_connection_editor_select_value(
                ConnectionEditorSelect::Authentication,
                Some(value.as_str()),
                cx,
            );
        }));

    let advanced_tabs = NyaTabs::new("connection-advanced-network-tabs")
        .items([
            NyaTabItem::new(t!("dialog.proxySelect")),
            NyaTabItem::new(t!("dialog.proxyJump")),
            NyaTabItem::new(t!("dialog.twoFactorAuth")),
            NyaTabItem::new(t!("dialog.sshAgentForwardingTab")),
        ])
        .selected_index(match editor.advanced_network_tab {
            ConnectionEditorAdvancedTab::Proxy => 0,
            ConnectionEditorAdvancedTab::JumpHost => 1,
            ConnectionEditorAdvancedTab::TwoFactor => 2,
            ConnectionEditorAdvancedTab::AgentForwarding => 3,
            _ => 0,
        })
        .on_select(cx.listener(|this, index, _, cx| {
            let tab = match *index {
                0 => ConnectionEditorAdvancedTab::Proxy,
                1 => ConnectionEditorAdvancedTab::JumpHost,
                2 => ConnectionEditorAdvancedTab::TwoFactor,
                _ => ConnectionEditorAdvancedTab::AgentForwarding,
            };
            this.set_connection_editor_advanced_tab(tab, cx);
        }));

    let behavior_tabs = NyaTabs::new("connection-advanced-behavior-tabs")
        .items([
            NyaTabItem::new(t!("dialog.commandExecution")),
            NyaTabItem::new(t!("dialog.encodingSettings")),
            NyaTabItem::new("SFTP"),
            NyaTabItem::new(t!("dialog.x11Forwarding")),
            NyaTabItem::new(t!("dialog.backspaceMode")),
        ])
        .selected_index(match editor.advanced_behavior_tab {
            ConnectionEditorAdvancedTab::PostLogin => 0,
            ConnectionEditorAdvancedTab::Terminal => 1,
            ConnectionEditorAdvancedTab::Sftp => 2,
            ConnectionEditorAdvancedTab::X11 => 3,
            ConnectionEditorAdvancedTab::Backspace => 4,
            _ => 0,
        })
        .on_select(cx.listener(|this, index, _, cx| {
            let tab = match *index {
                0 => ConnectionEditorAdvancedTab::PostLogin,
                1 => ConnectionEditorAdvancedTab::Terminal,
                2 => ConnectionEditorAdvancedTab::Sftp,
                3 => ConnectionEditorAdvancedTab::X11,
                _ => ConnectionEditorAdvancedTab::Backspace,
            };
            this.set_connection_editor_advanced_tab(tab, cx);
        }));

    let password_source_tabs = NyaTabs::new("connection-password-source-tabs")
        .items([
            NyaTabItem::new(t!("dialog.askWhenConnecting")),
            NyaTabItem::new(t!("dialog.directPassword")),
            NyaTabItem::new(t!("dialog.savedPassword")),
        ])
        .selected_index(match editor.password_source {
            ConnectionEditorPasswordSource::Ask => 0,
            ConnectionEditorPasswordSource::Direct => 1,
            ConnectionEditorPasswordSource::Saved => 2,
        })
        .on_select(cx.listener(|this, index, _, cx| {
            let source = match *index {
                0 => ConnectionEditorPasswordSource::Ask,
                1 => ConnectionEditorPasswordSource::Direct,
                _ => ConnectionEditorPasswordSource::Saved,
            };
            this.set_connection_editor_password_source(source, cx);
        }));

    div()
        .flex()
        .flex_col()
        .gap_3()
        .child(editor_field(
            palette,
            required(t!("dialog.host")),
            ConnectionEditorField::Host,
            fields,
            cx,
        ))
        .child(editor_stepper_field(
            palette,
            required(t!("dialog.port")),
            ConnectionEditorField::Port,
            fields,
            cx,
        ))
        .child(editor_field(
            palette,
            required(t!("dialog.username")),
            ConnectionEditorField::Username,
            fields,
            cx,
        ))
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(palette.text_muted))
                        .child(t!("dialog.authentication")),
                )
                .child(auth_tabs),
        )
        .when(editor.auth_mode == "none", |this| {
            this.child(
                div()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .bg(rgba((palette.accent << 8) | 0x18))
                    .text_size(px(10.))
                    .text_color(rgb(palette.text_muted))
                    .child(t!("dialog.noAuthenticationDescription")),
            )
        })
        .when(editor.auth_mode == "password", |this| {
            this.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(palette.text_muted))
                            .child(t!("dialog.passwordSource")),
                    )
                    .child(password_source_tabs)
                    .when(
                        editor.password_source == ConnectionEditorPasswordSource::Ask,
                        |this| {
                            this.child(
                                div()
                                    .px_3()
                                    .py_2()
                                    .rounded_md()
                                    .bg(rgba((palette.accent << 8) | 0x18))
                                    .text_size(px(10.))
                                    .text_color(rgb(palette.text_muted))
                                    .child(t!("dialog.askPasswordDescription")),
                            )
                        },
                    )
                    .when(
                        editor.password_source == ConnectionEditorPasswordSource::Direct,
                        |this| {
                            this.child(editor_field(
                                palette,
                                t!("dialog.password"),
                                ConnectionEditorField::Password,
                                fields,
                                cx,
                            ))
                        },
                    )
                    .when(
                        editor.password_source == ConnectionEditorPasswordSource::Saved,
                        |this| {
                            this.child(connection_editor_select(
                                ConnectionEditorRenderContext {
                                    palette,
                                    fields,
                                    cx,
                                },
                                "connection-editor-saved-password",
                                t!("dialog.savedPassword"),
                                ConnectionEditorSelect::SavedPassword,
                            ))
                        },
                    ),
            )
        })
        .when(editor.auth_mode == "agent", |this| {
            this.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .px_3()
                    .py_3()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(palette.border))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_3()
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(FontWeight(600.))
                                            .text_color(rgb(palette.text))
                                            .child(t!("dialog.sshAgentEndpoint")),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(10.))
                                            .text_color(rgb(palette.text_muted))
                                            .child(t!("dialog.sshAgentAuthDesc")),
                                    ),
                            )
                            .child(
                                div()
                                    .w(px(176.))
                                    .flex_none()
                                    .h(px(EDITOR_CONTROL_HEIGHT_PX))
                                    .child(NyaSelect::new(
                                        &fields.select(ConnectionEditorSelect::SshAgentEndpoint),
                                    )),
                            ),
                    )
                    .when(
                        matches!(
                            editor.agent_endpoint,
                            nyaterm_core::SshAgentEndpoint::Environment { .. }
                        ),
                        |this| {
                            this.child(editor_field(
                                palette,
                                t!("dialog.sshAgentEnvironmentVariable"),
                                ConnectionEditorField::AgentEnvironmentVariable,
                                fields,
                                cx,
                            ))
                        },
                    )
                    .when(
                        matches!(
                            editor.agent_endpoint,
                            nyaterm_core::SshAgentEndpoint::UnixSocket { .. }
                        ),
                        |this| {
                            this.child(editor_field(
                                palette,
                                t!("dialog.sshAgentSocketPath"),
                                ConnectionEditorField::AgentUnixSocket,
                                fields,
                                cx,
                            ))
                        },
                    ),
            )
        })
        .when(
            editor.auth_mode == "key" || editor.auth_mode == "certificate",
            |this| {
                this.child(connection_editor_select(
                    ConnectionEditorRenderContext {
                        palette,
                        fields,
                        cx,
                    },
                    "connection-editor-key",
                    t!("dialog.privateKey"),
                    ConnectionEditorSelect::SshKey,
                ))
            },
        )
        .child(
            div()
                .id("connection-editor-advanced-toggle")
                .h(px(28.))
                .flex()
                .items_center()
                .gap_2()
                .text_xs()
                .text_color(rgb(palette.text_muted))
                .cursor_pointer()
                .hover(|this| this.text_color(rgb(palette.text)))
                .child(
                    svg()
                        .size(px(14.))
                        .flex_none()
                        .path(if editor.advanced_open {
                            "icons/chevron-down.svg"
                        } else {
                            "icons/fe/forward.svg"
                        })
                        .text_color(rgb(palette.text_muted)),
                )
                .child(t!("dialog.advancedConfig"))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.toggle_connection_editor_flag(ConnectionEditorToggle::Advanced, cx);
                })),
        )
        .when(editor.advanced_open, |this| {
            this.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(advanced_tabs)
                    .when(
                        editor.advanced_network_tab == ConnectionEditorAdvancedTab::Proxy,
                        |this| {
                            this.child(ssh_advanced_content(
                                palette,
                                t!("dialog.proxySelect"),
                                truncate_preview(&proxy_label, 48),
                                connection_editor_select(
                                    ConnectionEditorRenderContext {
                                        palette,
                                        fields,
                                        cx,
                                    },
                                    "connection-editor-proxy",
                                    t!("dialog.proxySelect"),
                                    ConnectionEditorSelect::Proxy,
                                ),
                            ))
                        },
                    )
                    .when(
                        editor.advanced_network_tab == ConnectionEditorAdvancedTab::JumpHost,
                        |this| {
                            this.child(ssh_advanced_content(
                                palette,
                                t!("dialog.proxyJump"),
                                truncate_preview(&jump_label, 48),
                                connection_editor_select(
                                    ConnectionEditorRenderContext {
                                        palette,
                                        fields,
                                        cx,
                                    },
                                    "connection-editor-jump",
                                    t!("dialog.selectProxyJump"),
                                    ConnectionEditorSelect::ProxyJump,
                                ),
                            ))
                        },
                    )
                    .when(
                        editor.advanced_network_tab == ConnectionEditorAdvancedTab::TwoFactor,
                        |this| {
                            this.child(ssh_advanced_content(
                                palette,
                                t!("dialog.twoFactorAuth"),
                                truncate_preview(&otp_label, 36),
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .child(connection_editor_select(
                                        ConnectionEditorRenderContext {
                                            palette,
                                            fields,
                                            cx,
                                        },
                                        "connection-editor-otp",
                                        t!("dialog.selectOtp"),
                                        ConnectionEditorSelect::Otp,
                                    ))
                                    .child(toggle_chip(
                                        palette,
                                        t!("dialog.autoFillOtp"),
                                        editor.auto_fill_otp,
                                        cx.listener(|this, _, _, cx| {
                                            this.toggle_connection_editor_flag(
                                                ConnectionEditorToggle::AutoFillOtp,
                                                cx,
                                            );
                                        }),
                                    )),
                            ))
                        },
                    )
                    .when(
                        editor.advanced_network_tab == ConnectionEditorAdvancedTab::AgentForwarding,
                        |this| {
                            let endpoints = &editor
                                .agent_forwarding_config
                                .sources
                                .external_agent_endpoints;
                            let endpoint_rows = div()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .children(endpoints.iter().enumerate().map(|(index, endpoint)| {
                                    let remove = cx.listener(move |this, _, _, cx| {
                                        this.remove_connection_editor_agent_endpoint(index, cx);
                                    });
                                    let move_up = cx.listener(move |this, _, _, cx| {
                                        this.move_connection_editor_agent_endpoint(index, -1, cx);
                                    });
                                    let move_down = cx.listener(move |this, _, _, cx| {
                                        this.move_connection_editor_agent_endpoint(index, 1, cx);
                                    });
                                    let select_endpoint = cx.listener(move |this, _, _, cx| {
                                        this.select_connection_editor_agent_endpoint(index, cx);
                                    });
                                    div()
                                        .id(SharedString::from(format!(
                                            "connection-agent-endpoint-{index}"
                                        )))
                                        .flex()
                                        .flex_col()
                                        .gap_1()
                                        .border_1()
                                        .border_color(rgb(palette.border))
                                        .rounded_md()
                                        .p_2()
                                        .on_click(select_endpoint)
                                        .child(
                                            div()
                                                .items_center()
                                                .flex()
                                                .gap_1()
                                                .child(
                                                    div()
                                                        .min_w_0()
                                                        .flex_1()
                                                        .h(px(EDITOR_CONTROL_HEIGHT_PX))
                                                        .child(NyaSelect::new(
                                                            &fields.forwarding_endpoint_select(index),
                                                        )),
                                                )
                                                .child(
                                                    div()
                                                        .flex_none()
                                                        .flex()
                                                        .items_center()
                                                        .gap_1()
                                                        .child(
                                                            div()
                                                                .id(format!(
                                                                    "connection-agent-up-{index}"
                                                                ))
                                                                .px_2()
                                                                .py_1()
                                                                .text_xs()
                                                                .cursor_pointer()
                                                                .child("↑")
                                                                .on_click(move_up),
                                                        )
                                                        .child(
                                                            div()
                                                                .id(format!(
                                                                    "connection-agent-down-{index}"
                                                                ))
                                                                .px_2()
                                                                .py_1()
                                                                .text_xs()
                                                                .cursor_pointer()
                                                                .child("↓")
                                                                .on_click(move_down),
                                                        )
                                                        .child(
                                                            div()
                                                                .id(format!(
                                                                    "connection-agent-remove-{index}"
                                                                ))
                                                                .px_2()
                                                                .py_1()
                                                                .text_xs()
                                                                .cursor_pointer()
                                                                .child("×")
                                                                .on_click(remove),
                                                        ),
                                                ),
                                        )
                                        .when(
                                            matches!(
                                                endpoint,
                                                nyaterm_core::SshAgentEndpoint::Environment { .. }
                                            ),
                                            |this| {
                                                this.child(forwarding_endpoint_editor_field(
                                                    palette,
                                                    t!("dialog.sshAgentEnvironmentVariable"),
                                                    index,
                                                    ConnectionEditorField::AgentForwardingEnvironmentVariable,
                                                    fields,
                                                    cx,
                                                ))
                                            },
                                        )
                                        .when(
                                            matches!(
                                                endpoint,
                                                nyaterm_core::SshAgentEndpoint::UnixSocket { .. }
                                            ),
                                            |this| {
                                                this.child(forwarding_endpoint_editor_field(
                                                    palette,
                                                    t!("dialog.sshAgentSocketPath"),
                                                    index,
                                                    ConnectionEditorField::AgentForwardingSocketPath,
                                                    fields,
                                                    cx,
                                                ))
                                            },
                                        )
                                }));
                            let add_endpoint = cx.listener(|this, _, _, cx| {
                                this.add_connection_editor_agent_endpoint(cx);
                            });
                            let allowlist_fingerprints = match &editor.agent_forwarding_config.policy {
                                nyaterm_core::SshAgentForwardingPolicy::Allowlist { fingerprints } => {
                                    fingerprints.clone()
                                }
                                nyaterm_core::SshAgentForwardingPolicy::All => Vec::new(),
                            };
                            let allowlist_mode = matches!(
                                editor.agent_forwarding_config.policy,
                                nyaterm_core::SshAgentForwardingPolicy::Allowlist { .. }
                            );
                            let external_agent = editor.agent_forwarding_config.sources.external_agent;
                            let stored_keys = editor.agent_forwarding_config.sources.stored_keys;
                            let allowlist_count = allowlist_fingerprints.len();
                            let can_choose_identity = external_agent || stored_keys;
                            let mut choose_identity = div()
                                .id("connection-agent-choose-identity")
                                .h(px(32.))
                                .px_3()
                                .rounded_sm()
                                .border_1()
                                .border_color(rgb(palette.border))
                                .bg(rgb(palette.input))
                                .flex()
                                .items_center()
                                .gap_2()
                                .text_xs()
                                .font_weight(FontWeight(600.))
                                .text_color(rgb(palette.text))
                                .child(
                                    svg()
                                        .size(px(14.))
                                        .path("icons/settings.svg")
                                        .text_color(rgb(palette.text_muted)),
                                )
                                .child(t!("dialog.sshAgentManageAllowlist"));
                            if can_choose_identity {
                                choose_identity = choose_identity
                                    .cursor_pointer()
                                    .hover(|this| this.bg(rgb(palette.hover)))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.set_connection_editor_agent_identity_picker_open(true, cx);
                                        this.refresh_connection_editor_agent_preview(cx);
                                    }));
                            } else {
                                choose_identity = choose_identity.opacity(0.5);
                            }
                            this.child(
                                div()
                                    .rounded_md()
                                    .border_1()
                                    .border_color(rgb(palette.border))
                                    .bg(rgb(palette.bg))
                                    .p_3()
                                    .flex()
                                    .flex_col()
                                    .gap_3()
                                    .child(
                                        div()
                                            .flex()
                                            .items_start()
                                            .justify_between()
                                            .gap_3()
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
                                                            .child(t!("dialog.sshAgentForwarding")),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_xs()
                                                            .text_color(rgb(palette.text_muted))
                                                            .child(t!("dialog.sshAgentForwardingDescription")),
                                                    ),
                                            )
                                            .child(crate::features::pages::settings::settings_switch(
                                                palette,
                                                "connection-agent-forwarding-switch",
                                                editor.agent_forwarding_config.enabled,
                                                cx.listener(|this, _, _, cx| {
                                                    this.toggle_connection_editor_flag(
                                                        ConnectionEditorToggle::AgentForwarding,
                                                        cx,
                                                    );
                                                }),
                                            )),
                                    )
                                    .when(editor.agent_forwarding_config.enabled, |this| {
                                        this.child(
                                            div()
                                                .border_t_1()
                                            .border_color(rgb(palette.border))
                                            .pt_3()
                                            .flex()
                                            .flex_col()
                                            .gap_2()
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .font_weight(FontWeight(600.))
                                                    .text_color(rgb(palette.text))
                                                    .child(t!("dialog.sshAgentForwardingSources")),
                                            )
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .justify_between()
                                                    .gap_3()
                                                    .child(
                                                        div()
                                                            .min_w_0()
                                                            .text_xs()
                                                            .text_color(rgb(palette.text))
                                                            .child(format!(
                                                                "{} ({})",
                                                                t!("dialog.sshAgentExternalSource"),
                                                                t!("dialog.sshAgentEndpointList"),
                                                            )),
                                                    )
                                                    .child(crate::features::pages::settings::settings_switch(
                                                        palette,
                                                        "connection-agent-external-switch",
                                                        external_agent,
                                                        cx.listener(|this, _, _, cx| {
                                                            this.toggle_connection_editor_flag(
                                                                ConnectionEditorToggle::AgentExternal,
                                                                cx,
                                                            );
                                                        }),
                                                    )),
                                            )
                                            .when(external_agent, |this| {
                                                this.child(
                                                    div()
                                                        .rounded_sm()
                                                        .border_1()
                                                        .border_color(rgb(palette.border))
                                                        .bg(rgb(palette.surface))
                                                        .p_2()
                                                        .flex()
                                                        .flex_col()
                                                        .gap_2()
                                                        .child(
                                                            div()
                                                                .flex()
                                                                .items_start()
                                                                .justify_between()
                                                                .gap_2()
                                                                .child(
                                                                    div()
                                                                        .min_w_0()
                                                                        .flex_1()
                                                                        .text_xs()
                                                                        .text_color(rgb(palette.text_muted))
                                                                        .child(t!("dialog.sshAgentEndpointListDescription")),
                                                                )
                                                                .child(
                                                                    div()
                                                                        .id("connection-agent-add-endpoint")
                                                                        .px_2()
                                                                        .py_1()
                                                                        .rounded_sm()
                                                                        .border_1()
                                                                        .border_color(rgb(palette.border))
                                                                        .text_xs()
                                                                        .text_color(rgb(palette.text))
                                                                        .cursor_pointer()
                                                                        .hover(|this| this.bg(rgb(palette.hover)))
                                                                        .child(t!("dialog.sshAgentAddEndpoint"))
                                                                        .on_click(add_endpoint),
                                                                ),
                                                        )
                                                        .when(endpoints.is_empty(), |this| {
                                                            this.child(
                                                                div()
                                                                    .rounded_sm()
                                                                    .border_1()
                                                                    .border_color(rgb(palette.border))
                                                                    .px_2()
                                                                    .py_2()
                                                                    .text_xs()
                                                                    .text_color(rgb(palette.text_muted))
                                                                    .child(t!("dialog.sshAgentEndpointListEmpty")),
                                                            )
                                                        })
                                                        .child(endpoint_rows),
                                                )
                                            })
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .justify_between()
                                                    .gap_3()
                                                    .child(
                                                        div()
                                                            .min_w_0()
                                                            .text_xs()
                                                            .text_color(rgb(palette.text))
                                                            .child(t!("dialog.sshAgentStoredKeysSource")),
                                                    )
                                                    .child(crate::features::pages::settings::settings_switch(
                                                        palette,
                                                        "connection-agent-stored-keys-switch",
                                                        stored_keys,
                                                        cx.listener(|this, _, _, cx| {
                                                            this.toggle_connection_editor_flag(
                                                                ConnectionEditorToggle::AgentStoredKeys,
                                                                cx,
                                                            );
                                                        }),
                                                    )),
                                            ),
                                        )
                                    .child(
                                        div()
                                            .border_t_1()
                                            .border_color(rgb(palette.border))
                                            .pt_3()
                                            .flex()
                                            .flex_col()
                                            .gap_2()
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .font_weight(FontWeight(600.))
                                                    .text_color(rgb(palette.text))
                                                    .child(t!("dialog.sshAgentForwardingPolicy")),
                                            )
                                            .child(
                                                div()
                                                    .w(px(190.))
                                                    .child(connection_editor_select(
                                                        ConnectionEditorRenderContext { palette, fields, cx },
                                                        "connection-editor-ssh-agent-policy",
                                                        "",
                                                        ConnectionEditorSelect::SshAgentForwardingPolicy,
                                                    )),
                                            )
                                            .when(allowlist_mode, |this| {
                                                this.child(
                                                    div()
                                                        .flex()
                                                        .flex_col()
                                                        .gap_2()
                                                        .child(
                                                            div()
                                                                .text_xs()
                                                                .text_color(if allowlist_count == 0 {
                                                                    rgb(palette.warning)
                                                                } else {
                                                                    rgb(palette.text_muted)
                                                                })
                                                                .child(if allowlist_count == 0 {
                                                                    t!("dialog.sshAgentAllowlistEmpty")
                                                                        .to_string()
                                                                } else {
                                                                    format!(
                                                                        "{} {}",
                                                                        allowlist_count,
                                                                        t!("dialog.sshAgentAllowlistCount"),
                                                                    )
                                                                }),
                                                        )
                                                        .child(choose_identity),
                                                )
                                            })
                                    )
                                })
                                    .child(
                                        div()
                                            .border_t_1()
                                            .border_color(rgb(palette.border))
                                            .pt_3()
                                            .text_xs()
                                            .text_color(rgb(palette.warning))
                                            .child(t!("dialog.sshAgentForwardingWarning")),
                                    ),
                            )
                        },
                    )
                    .child(behavior_tabs)
                    .when(
                        editor.advanced_behavior_tab == ConnectionEditorAdvancedTab::PostLogin,
                        |this| {
                            this.child(ssh_advanced_content(
                                palette,
                                t!("dialog.postLoginCommand"),
                                t!("dialog.postLoginCommandDesc"),
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .child(toggle_chip(
                                        palette,
                                        t!("dialog.enabled"),
                                        editor.post_login_enabled,
                                        cx.listener(|this, _, _, cx| {
                                            this.toggle_connection_editor_flag(
                                                ConnectionEditorToggle::PostLogin,
                                                cx,
                                            );
                                        }),
                                    ))
                                    .when(editor.post_login_enabled, |this| {
                                        this.child(editor_field(
                                            palette,
                                            t!("dialog.postLoginCommandContent"),
                                            ConnectionEditorField::PostLoginCommand,
                                            fields,
                                            cx,
                                        ))
                                        .child(
                                            editor_stepper_field(
                                                palette,
                                                t!("dialog.postLoginDelay"),
                                                ConnectionEditorField::PostLoginDelay,
                                                fields,
                                                cx,
                                            ),
                                        )
                                    }),
                            ))
                        },
                    )
                    .when(
                        editor.advanced_behavior_tab == ConnectionEditorAdvancedTab::Terminal,
                        |this| {
                            this.child(ssh_advanced_content(
                                palette,
                                t!("dialog.encodingSettings"),
                                t!("connection.encodingFollowGlobal"),
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_3()
                                    .child(connection_editor_select(
                                        ConnectionEditorRenderContext {
                                            palette,
                                            fields,
                                            cx,
                                        },
                                        "connection-editor-ssh-profile",
                                        t!("dialog.sshProfile"),
                                        ConnectionEditorSelect::SshProfile,
                                    ))
                                    .child(connection_editor_select(
                                        ConnectionEditorRenderContext {
                                            palette,
                                            fields,
                                            cx,
                                        },
                                        "connection-editor-ssh-terminal-type",
                                        t!("dialog.sshTerminalType"),
                                        ConnectionEditorSelect::SshTerminalType,
                                    ))
                                    .child(connection_editor_select(
                                        ConnectionEditorRenderContext {
                                            palette,
                                            fields,
                                            cx,
                                        },
                                        "connection-editor-ssh-encoding",
                                        t!("connection.encoding"),
                                        ConnectionEditorSelect::Encoding,
                                    ))
                                    .when(
                                        editor.ssh_profile
                                            == nyaterm_core::SshProfile::NetworkDevice,
                                        |this| {
                                            this.child(
                                                div()
                                                    .rounded_md()
                                                    .border_1()
                                                    .border_color(rgb(palette.warning))
                                                    .bg(rgba((palette.warning << 8) | 0x18))
                                                    .p_2()
                                                    .text_size(px(10.))
                                                    .text_color(rgb(palette.text_muted))
                                                    .child(t!(
                                                        "dialog.sshNetworkDeviceLimitations",
                                                    )),
                                            )
                                        },
                                    ),
                            ))
                        },
                    )
                    .when(
                        editor.advanced_behavior_tab == ConnectionEditorAdvancedTab::Sftp,
                        |this| {
                            this.child(ssh_advanced_content(
                                palette,
                                t!("dialog.sftpAdvanced"),
                                t!("dialog.sftpAdvancedDesc"),
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .child(toggle_chip(
                                        palette,
                                        t!("dialog.enabled"),
                                        editor.sftp_enabled,
                                        cx.listener(|this, _, _, cx| {
                                            this.toggle_connection_editor_flag(
                                                ConnectionEditorToggle::SftpEnabled,
                                                cx,
                                            );
                                        }),
                                    ))
                                    .child(connection_editor_select(
                                        ConnectionEditorRenderContext {
                                            palette,
                                            fields,
                                            cx,
                                        },
                                        "connection-editor-sftp-cwd",
                                        t!("dialog.sftpCwdFollowMode"),
                                        ConnectionEditorSelect::SftpCwdFollowMode,
                                    ))
                                    .child(editor_stepper_field(
                                        palette,
                                        t!("dialog.sftpShellDetectionTimeout"),
                                        ConnectionEditorField::SftpShellDetectionTimeout,
                                        fields,
                                        cx,
                                    ))
                                    .child(connection_editor_select(
                                        ConnectionEditorRenderContext {
                                            palette,
                                            fields,
                                            cx,
                                        },
                                        "connection-editor-sftp-filename-encoding",
                                        t!("dialog.sftpFilenameEncoding"),
                                        ConnectionEditorSelect::SftpFilenameEncoding,
                                    ))
                                    .child(connection_editor_select(
                                        ConnectionEditorRenderContext { palette, fields, cx },
                                        "connection-editor-sftp-pipeline-depth",
                                        t!("dialog.sftpPipelineDepth"),
                                        ConnectionEditorSelect::SftpPipelineDepth,
                                    )),
                            ))
                        },
                    )
                    .when(
                        editor.advanced_behavior_tab == ConnectionEditorAdvancedTab::X11,
                        |this| {
                            this.child(ssh_advanced_content(
                                palette,
                                t!("dialog.x11Forwarding"),
                                t!("dialog.x11ForwardingDesc"),
                                toggle_chip(
                                    palette,
                                    t!("dialog.enabled"),
                                    editor.x11_forwarding,
                                    cx.listener(|this, _, _, cx| {
                                        this.toggle_connection_editor_flag(
                                            ConnectionEditorToggle::X11,
                                            cx,
                                        );
                                    }),
                                ),
                            ))
                        },
                    )
                    .when(
                        editor.advanced_behavior_tab == ConnectionEditorAdvancedTab::Backspace,
                        |this| {
                            this.child(ssh_advanced_content(
                                palette,
                                t!("dialog.backspaceMode"),
                                t!("dialog.sshBackspaceModeDesc"),
                                connection_editor_select(
                                    ConnectionEditorRenderContext {
                                        palette,
                                        fields,
                                        cx,
                                    },
                                    "connection-editor-backspace",
                                    t!("dialog.backspaceMode"),
                                    ConnectionEditorSelect::Backspace,
                                ),
                            ))
                        },
                    )
                    .child(toggle_chip(
                        palette,
                        t!("dialog.dynamicTabTitle"),
                        editor.dynamic_tab_title,
                        cx.listener(|this, _, _, cx| {
                            this.toggle_connection_editor_flag(
                                ConnectionEditorToggle::DynamicTabTitle,
                                cx,
                            );
                        }),
                    ))
                    .child({
                        let supported = nyaterm_transport::supported_ssh_algorithms();
                        let algorithm_tabs = NyaTabs::new("connection-ssh-algorithm-tabs")
                            .items([
                                NyaTabItem::new(t!("dialog.algorithmKexTab")),
                                NyaTabItem::new(t!("dialog.algorithmCiphersTab")),
                                NyaTabItem::new(t!("dialog.algorithmMacsTab")),
                                NyaTabItem::new(t!("dialog.algorithmHostKeysTab")),
                            ])
                            .selected_index(match editor.ssh_algorithm_tab {
                                ConnectionEditorSshAlgorithmTab::KeyExchange => 0,
                                ConnectionEditorSshAlgorithmTab::Ciphers => 1,
                                ConnectionEditorSshAlgorithmTab::Macs => 2,
                                ConnectionEditorSshAlgorithmTab::HostKeys => 3,
                            })
                            .on_select(cx.listener(|this, index, _, cx| {
                                let tab = match *index {
                                    0 => ConnectionEditorSshAlgorithmTab::KeyExchange,
                                    1 => ConnectionEditorSshAlgorithmTab::Ciphers,
                                    2 => ConnectionEditorSshAlgorithmTab::Macs,
                                    _ => ConnectionEditorSshAlgorithmTab::HostKeys,
                                };
                                this.set_connection_editor_ssh_algorithm_tab(tab, cx);
                            }));
                        let (options, selected_values) = match editor.ssh_algorithm_tab {
                            ConnectionEditorSshAlgorithmTab::KeyExchange => (
                                supported.kex.as_slice(),
                                editor.ssh_algorithm_kex.as_slice(),
                            ),
                            ConnectionEditorSshAlgorithmTab::Ciphers => (
                                supported.ciphers.as_slice(),
                                editor.ssh_algorithm_ciphers.as_slice(),
                            ),
                            ConnectionEditorSshAlgorithmTab::Macs => (
                                supported.macs.as_slice(),
                                editor.ssh_algorithm_macs.as_slice(),
                            ),
                            ConnectionEditorSshAlgorithmTab::HostKeys => (
                                supported.host_keys.as_slice(),
                                editor.ssh_algorithm_host_keys.as_slice(),
                            ),
                        };
                        let mode_description = match editor.ssh_algorithm_mode.as_str() {
                            "secure" => t!("dialog.algorithmModeSecureDesc"),
                            "custom" => t!("dialog.algorithmModeCustomDesc"),
                            _ => t!("dialog.algorithmModeCompatibleDesc"),
                        };
                        ssh_advanced_content(
                            palette,
                            t!("dialog.sshAlgorithms"),
                            t!("dialog.sshAlgorithmsDesc"),
                            div()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .child(connection_editor_select(
                                    ConnectionEditorRenderContext {
                                        palette,
                                        fields,
                                        cx,
                                    },
                                    "connection-editor-ssh-algorithm-mode",
                                    t!("dialog.algorithmMode"),
                                    ConnectionEditorSelect::SshAlgorithmMode,
                                ))
                                .child(
                                    div()
                                        .text_size(px(10.))
                                        .text_color(rgb(palette.text_muted))
                                        .child(mode_description),
                                )
                                .when(editor.ssh_algorithm_mode == "custom", |this| {
                                    this.child(algorithm_tabs).child(ssh_algorithm_list(
                                        palette,
                                        editor.ssh_algorithm_tab,
                                        options,
                                        selected_values,
                                        cx,
                                    ))
                                }),
                        )
                    }),
            )
        })
}
