use crate::models::ConnectionEditorSection;
use rust_i18n::t;

use gpui::{
    Context, FontWeight, IntoElement, SharedString, div,
    prelude::{FluentBuilder, InteractiveElement, ParentElement, Styled},
    px, rgb,
};

use crate::features::{NyaTermApp, connections::ConnectionEditorToggle};
use crate::models::{ConnectionEditorField, ConnectionEditorSelect};
use nyaterm_ui::NyaSwitch;

use super::super::super::list::{
    ConnectionEditorRenderContext, connection_editor_select, editor_field, editor_stepper_field,
    required,
};

use super::{ConnectionEditorSectionContext, recording::connection_editor_recording_section};

fn telnet_switch_row(
    palette: crate::theme::ThemePalette,
    id: &'static str,
    label: impl Into<SharedString>,
    description: impl Into<SharedString>,
    checked: bool,
    enabled: bool,
    on_click: impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    let label: SharedString = label.into();
    let description: SharedString = description.into();
    div()
        .rounded_md()
        .border_1()
        .border_color(rgb(palette.border))
        .bg(rgb(palette.bg))
        .px_3()
        .py_2()
        .opacity(if enabled { 1.0 } else { 0.55 })
        .flex()
        .items_start()
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
                        .font_weight(FontWeight(500.))
                        .text_color(rgb(palette.text))
                        .child(label),
                )
                .child(
                    div()
                        .text_size(px(10.))
                        .text_color(rgb(palette.text_muted))
                        .child(description),
                ),
        )
        .child(
            div().mt(px(2.)).flex_none().child(
                NyaSwitch::new(id)
                    .checked(checked)
                    .disabled(!enabled)
                    .on_click(move |_, window, cx| {
                        if enabled {
                            on_click(&gpui::ClickEvent::default(), window, cx);
                        }
                    }),
            ),
        )
}

pub(super) fn connection_editor_telnet_section(
    section: ConnectionEditorSectionContext<'_>,
    cx: &mut Context<NyaTermApp>,
) -> gpui::Div {
    let content = match section.editor.section {
        ConnectionEditorSection::Basic => div()
            .flex()
            .flex_col()
            .gap_3()
            .child(telnet_basic(section, cx))
            .child(telnet_authentication(section, cx)),
        ConnectionEditorSection::Terminal => telnet_terminal(section, cx),
        ConnectionEditorSection::Network => telnet_network(section, cx),
        ConnectionEditorSection::Compatibility => telnet_compatibility(section, cx),
        ConnectionEditorSection::AutoLogin => telnet_auto_login(section, cx),
        _ => div(),
    };
    div()
        .debug_selector(|| "connection-editor-telnet-section".to_string())
        .flex()
        .flex_col()
        .gap_3()
        .child(content)
}

fn telnet_network(
    section: ConnectionEditorSectionContext<'_>,
    cx: &mut Context<NyaTermApp>,
) -> gpui::Div {
    let ConnectionEditorSectionContext {
        palette, fields, ..
    } = section;
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
            "connection-editor-telnet-proxy",
            t!("dialog.proxySelect"),
            ConnectionEditorSelect::Proxy,
        ))
        .child(connection_editor_select(
            ConnectionEditorRenderContext {
                palette,
                fields,
                cx,
            },
            "connection-editor-telnet-jump",
            t!("dialog.proxyJump"),
            ConnectionEditorSelect::ProxyJump,
        ))
        .child(
            div()
                .text_xs()
                .text_color(rgb(palette.text_muted))
                .child(t!("dialog.telnetNetworkRouteDescription")),
        )
        .child(
            div()
                .text_xs()
                .text_color(rgb(palette.text_muted))
                .child(t!("dialog.telnetNetworkPlaintextDescription")),
        )
}

fn telnet_basic(
    section: ConnectionEditorSectionContext<'_>,
    cx: &mut Context<NyaTermApp>,
) -> gpui::Div {
    let ConnectionEditorSectionContext {
        palette,
        fields,
        narrow,
        ..
    } = section;
    div().flex().flex_col().gap_3().child(
        div()
            .flex()
            .when(narrow, |this| this.flex_col())
            .gap_3()
            .child(div().min_w_0().flex_1().child(editor_field(
                palette,
                required(t!("dialog.host")),
                ConnectionEditorField::Host,
                fields,
                cx,
            )))
            .child(
                div()
                    .when(!narrow, |this| this.w(px(150.)).flex_none())
                    .when(narrow, |this| this.w_full())
                    .child(editor_stepper_field(
                        palette,
                        required(t!("dialog.port")),
                        ConnectionEditorField::Port,
                        fields,
                        cx,
                    )),
            ),
    )
}

fn telnet_authentication(
    section: ConnectionEditorSectionContext<'_>,
    cx: &mut Context<NyaTermApp>,
) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap_3()
        .child(super::authentication::login_identity(section, cx))
        .child(super::authentication::password_authentication(section, cx))
}

fn telnet_terminal(
    section: ConnectionEditorSectionContext<'_>,
    cx: &mut Context<NyaTermApp>,
) -> gpui::Div {
    let ConnectionEditorSectionContext {
        palette,
        fields,
        narrow,
        ..
    } = section;
    div()
        .flex()
        .flex_col()
        .gap_3()
        .child(
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
                        .text_xs()
                        .font_weight(FontWeight(600.))
                        .child(t!("dialog.telnetInputBehavior")),
                )
                .child(
                    div()
                        .grid()
                        .grid_cols(if narrow { 1 } else { 2 })
                        .gap_2()
                        .child(connection_editor_select(
                            ConnectionEditorRenderContext {
                                palette,
                                fields,
                                cx,
                            },
                            "connection-editor-telnet-backspace",
                            t!("dialog.backspaceMode"),
                            ConnectionEditorSelect::Backspace,
                        ))
                        .child(connection_editor_select(
                            ConnectionEditorRenderContext {
                                palette,
                                fields,
                                cx,
                            },
                            "connection-editor-telnet-enter-mode",
                            t!("dialog.telnetEnterMode"),
                            ConnectionEditorSelect::TelnetEnterMode,
                        )),
                ),
        )
        .child(
            div()
                .rounded_md()
                .border_1()
                .border_color(rgb(palette.border))
                .bg(rgb(palette.bg))
                .p_3()
                .flex()
                .flex_col()
                .gap_3()
                .child(connection_editor_select(
                    ConnectionEditorRenderContext {
                        palette,
                        fields,
                        cx,
                    },
                    "connection-editor-telnet-encoding",
                    t!("connection.encoding"),
                    ConnectionEditorSelect::Encoding,
                ))
                .child(connection_editor_recording_section(section, cx)),
        )
}

fn telnet_compatibility(
    section: ConnectionEditorSectionContext<'_>,
    cx: &mut Context<NyaTermApp>,
) -> gpui::Div {
    let ConnectionEditorSectionContext {
        palette,
        editor,
        fields,
        narrow,
        ..
    } = section;
    div().flex().flex_col().gap_3().child(
        div()
            .rounded_md()
            .border_1()
            .border_color(rgb(palette.border))
            .bg(rgb(palette.bg))
            .p_3()
            .flex()
            .flex_col()
            .gap_3()
            .child(telnet_switch_row(
                palette,
                "connection-telnet-raw-tcp",
                t!("dialog.telnetRawTcpCli"),
                t!("dialog.telnetRawTcpCliDesc"),
                editor.raw_tcp_cli,
                true,
                cx.listener(|this, _, _, cx| {
                    this.toggle_connection_editor_flag(ConnectionEditorToggle::RawTcp, cx);
                }),
            ))
            .child(
                div()
                    .w_full()
                    .opacity(if editor.raw_tcp_cli { 0.55 } else { 1.0 })
                    .child(editor_stepper_field(
                        palette,
                        t!("dialog.telnetKeepAliveInterval"),
                        ConnectionEditorField::TelnetKeepAliveInterval,
                        fields,
                        cx,
                    ))
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(palette.text_muted))
                            .child(t!("dialog.telnetKeepAliveIntervalDesc")),
                    ),
            )
            .child(
                div()
                    .grid()
                    .grid_cols(if narrow { 1 } else { 2 })
                    .gap_2()
                    .child(telnet_switch_row(
                        palette,
                        "connection-telnet-local-echo",
                        t!("dialog.telnetLocalEcho"),
                        t!("dialog.telnetLocalEchoDesc"),
                        editor.local_echo,
                        true,
                        cx.listener(|this, _, _, cx| {
                            this.toggle_connection_editor_flag(
                                ConnectionEditorToggle::LocalEcho,
                                cx,
                            );
                        }),
                    ))
                    .child(telnet_switch_row(
                        palette,
                        "connection-telnet-local-line-edit",
                        t!("dialog.telnetLocalLineEdit"),
                        t!("dialog.telnetLocalLineEditDesc"),
                        editor.local_line_edit,
                        true,
                        cx.listener(|this, _, _, cx| {
                            this.toggle_connection_editor_flag(
                                ConnectionEditorToggle::LocalLineEdit,
                                cx,
                            );
                        }),
                    ))
                    .child(telnet_switch_row(
                        palette,
                        "connection-telnet-force-character",
                        t!("dialog.telnetForceCharAtATime"),
                        t!("dialog.telnetForceCharAtATimeDesc"),
                        editor.force_character_at_a_time,
                        true,
                        cx.listener(|this, _, _, cx| {
                            this.toggle_connection_editor_flag(
                                ConnectionEditorToggle::ForceCharacterAtATime,
                                cx,
                            );
                        }),
                    ))
                    .child(telnet_switch_row(
                        palette,
                        "connection-telnet-send-naws",
                        t!("dialog.telnetSendNaws"),
                        t!("dialog.telnetSendNawsDesc"),
                        editor.send_naws,
                        !editor.raw_tcp_cli,
                        cx.listener(|this, _, _, cx| {
                            this.toggle_connection_editor_flag(
                                ConnectionEditorToggle::SendNaws,
                                cx,
                            );
                        }),
                    ))
                    .child(telnet_switch_row(
                        palette,
                        "connection-telnet-send-sga",
                        t!("dialog.telnetSendSga"),
                        t!("dialog.telnetSendSgaDesc"),
                        editor.send_sga,
                        !editor.raw_tcp_cli,
                        cx.listener(|this, _, _, cx| {
                            this.toggle_connection_editor_flag(ConnectionEditorToggle::SendSga, cx);
                        }),
                    )),
            ),
    )
}

fn telnet_auto_login(
    section: ConnectionEditorSectionContext<'_>,
    cx: &mut Context<NyaTermApp>,
) -> gpui::Div {
    let ConnectionEditorSectionContext {
        palette,
        editor,
        fields,
        narrow,
        ..
    } = section;
    div().flex().flex_col().gap_3().child(
        div()
            .mt_3()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight(600.))
                    .text_color(rgb(palette.text))
                    .child(t!("dialog.telnetAutoLogin")),
            )
            .child(
                div()
                    .grid()
                    .grid_cols(if narrow { 1 } else { 2 })
                    .gap_2()
                    .child(telnet_switch_row(
                        palette,
                        "connection-telnet-auto-login-enabled",
                        t!("dialog.enabled"),
                        t!("dialog.telnetAutoLoginDesc"),
                        editor.telnet_auto_login_enabled,
                        true,
                        cx.listener(|this, _, _, cx| {
                            this.toggle_connection_editor_flag(
                                ConnectionEditorToggle::TelnetAutoLoginEnabled,
                                cx,
                            );
                        }),
                    ))
                    .child(telnet_switch_row(
                        palette,
                        "connection-telnet-auto-login-wake",
                        t!("dialog.telnetAutoLoginWakeEnter"),
                        t!("dialog.telnetAutoLoginWakeEnterDesc"),
                        editor.telnet_auto_login_send_wake_enter,
                        editor.telnet_auto_login_enabled,
                        cx.listener(|this, _, _, cx| {
                            this.toggle_connection_editor_flag(
                                ConnectionEditorToggle::TelnetAutoLoginSendWakeEnter,
                                cx,
                            );
                        }),
                    )),
            )
            .child(
                div()
                    .grid()
                    .grid_cols(if narrow { 1 } else { 2 })
                    .gap_2()
                    .child(editor_stepper_field(
                        palette,
                        t!("dialog.telnetAutoLoginTimeout"),
                        ConnectionEditorField::TelnetAutoLoginTimeout,
                        fields,
                        cx,
                    ))
                    .child(editor_stepper_field(
                        palette,
                        t!("dialog.telnetAutoLoginMaxRetries"),
                        ConnectionEditorField::TelnetAutoLoginMaxRetries,
                        fields,
                        cx,
                    )),
            )
            .child(editor_field(
                palette,
                t!("dialog.telnetAutoLoginUsernamePrompt"),
                ConnectionEditorField::TelnetAutoLoginUsernamePrompt,
                fields,
                cx,
            ))
            .child(editor_field(
                palette,
                t!("dialog.telnetAutoLoginPasswordPrompt"),
                ConnectionEditorField::TelnetAutoLoginPasswordPrompt,
                fields,
                cx,
            ))
            .child(editor_field(
                palette,
                t!("dialog.telnetAutoLoginSuccessPrompt"),
                ConnectionEditorField::TelnetAutoLoginSuccessPrompt,
                fields,
                cx,
            ))
            .child(editor_field(
                palette,
                t!("dialog.telnetAutoLoginFailurePrompt"),
                ConnectionEditorField::TelnetAutoLoginFailurePrompt,
                fields,
                cx,
            )),
    )
}
