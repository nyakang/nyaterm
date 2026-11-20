use crate::models::{ConnectionEditorField, ConnectionEditorSection, ConnectionEditorSelect};
use rust_i18n::t;

use gpui::{
    Context, div,
    prelude::{FluentBuilder, InteractiveElement, ParentElement, Styled},
    px, rgb,
};

use crate::features::{NyaTermApp, connections::ConnectionEditorToggle};
use nyaterm_ui::NyaRadioGroup;

use super::super::super::list::{
    ConnectionEditorRenderContext, connection_editor_select, editor_field, editor_stepper_field,
    editor_switch_row, required,
};
use super::ConnectionEditorSectionContext;

pub(super) fn connection_editor_ftp_section(
    section: ConnectionEditorSectionContext<'_>,
    cx: &mut Context<NyaTermApp>,
) -> gpui::Div {
    let content = match section.editor.section {
        ConnectionEditorSection::Basic => ftp_basic(section, cx),
        ConnectionEditorSection::Charset => ftp_charset(section, cx),
        ConnectionEditorSection::Advanced => ftp_advanced(section, cx),
        _ => div(),
    };
    div()
        .debug_selector(|| "connection-editor-ftp-section".to_string())
        .flex()
        .flex_col()
        .gap_3()
        .child(content)
}

fn ftp_basic(
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
    let anonymous = editor.auth_mode == "none";
    let mut body = div()
        .flex()
        .flex_col()
        .gap_3()
        .child(
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
        .child(super::authentication::login_identity(section, cx))
        .child(
            super::authentication::password_authentication_with_none_label(
                section,
                Some(t!("dialog.ftpAnonymousLogin").as_ref()),
                cx,
            ),
        );
    if anonymous {
        body = body.child(
            div()
                .text_xs()
                .text_color(rgb(palette.text_muted))
                .child(t!("dialog.ftpAnonymousHint")),
        );
    }
    body
}

fn ftp_charset(
    section: ConnectionEditorSectionContext<'_>,
    cx: &mut Context<NyaTermApp>,
) -> gpui::Div {
    let ConnectionEditorSectionContext {
        palette,
        editor,
        fields,
        ..
    } = section;
    let selected: usize = match editor.ftp_filename_encoding_mode.as_str() {
        "utf8" | "utf-8" => 1,
        "custom" => 2,
        _ => 0,
    };
    let custom_enabled = selected == 2;
    div()
        .flex()
        .flex_col()
        .gap_3()
        .child(
            div()
                .text_sm()
                .font_weight(gpui::FontWeight(600.))
                .child(t!("dialog.filenameEncodingTitle")),
        )
        .child(
            NyaRadioGroup::new("connection-editor-ftp-filename-encoding-mode")
                .items([
                    t!("dialog.filenameEncodingAuto"),
                    t!("dialog.filenameEncodingForceUtf8"),
                    t!("dialog.filenameEncodingCustom"),
                ])
                .selected_index(Some(selected))
                .on_select(cx.listener(|this, index, _, cx| {
                    let value = match index {
                        1 => "utf8",
                        2 => "custom",
                        _ => "auto",
                    };
                    this.set_connection_editor_select_value(
                        ConnectionEditorSelect::FtpFilenameEncodingMode,
                        Some(value),
                        cx,
                    );
                })),
        )
        .child(
            div()
                .text_xs()
                .text_color(rgb(palette.text_muted))
                .child(t!("dialog.filenameEncodingAutoHint")),
        )
        .child(
            div()
                .opacity(if custom_enabled { 1.0 } else { 0.55 })
                .child(connection_editor_select(
                    ConnectionEditorRenderContext {
                        palette,
                        fields,
                        cx,
                    },
                    "connection-editor-ftp-filename-encoding",
                    t!("dialog.filenameEncodingLabel"),
                    ConnectionEditorSelect::FtpFilenameEncoding,
                )),
        )
        .child(
            div()
                .text_xs()
                .text_color(rgb(palette.text_muted))
                .child(t!("dialog.filenameEncodingScope")),
        )
        .child(
            div()
                .rounded_md()
                .border_1()
                .border_color(rgb(palette.warning))
                .px_3()
                .py_2()
                .text_xs()
                .text_color(rgb(palette.warning))
                .child(t!("dialog.filenameEncodingWarning")),
        )
}

fn ftp_advanced(
    section: ConnectionEditorSectionContext<'_>,
    cx: &mut Context<NyaTermApp>,
) -> gpui::Div {
    let ConnectionEditorSectionContext {
        palette,
        editor,
        fields,
        ..
    } = section;
    let security_mode = editor.ftp_security_mode.trim().to_ascii_lowercase();
    let ftps_enabled = security_mode == "explicit" || security_mode == "implicit";
    let implicit_ftps = security_mode == "implicit";
    let mut body = div()
        .flex()
        .flex_col()
        .gap_3()
        .child(connection_editor_select(
            ConnectionEditorRenderContext {
                palette,
                fields,
                cx,
            },
            "connection-editor-ftp-passive-mode",
            t!("dialog.ftpPassiveMode"),
            ConnectionEditorSelect::FtpPassiveMode,
        ))
        .child(editor_switch_row(
            palette,
            "connection-editor-ftp-ascii-mode",
            t!("dialog.ftpAsciiMode"),
            editor.ftp_ascii_mode,
            cx.listener(|this, _, _, cx| {
                this.toggle_connection_editor_flag(ConnectionEditorToggle::FtpAsciiMode, cx);
            }),
        ))
        .child(
            div()
                .text_xs()
                .text_color(rgb(palette.text_muted))
                .child(t!("dialog.ftpAsciiModeHint")),
        )
        .child(connection_editor_select(
            ConnectionEditorRenderContext {
                palette,
                fields,
                cx,
            },
            "connection-editor-ftp-security-mode",
            t!("dialog.ftpSecurityMode"),
            ConnectionEditorSelect::FtpSecurityMode,
        ));
    if ftps_enabled {
        let hint = if implicit_ftps {
            t!("dialog.ftpImplicitFtpsHint")
        } else {
            t!("dialog.ftpFtpsHint")
        };
        body = body.child(
            div()
                .text_xs()
                .text_color(rgb(palette.text_muted))
                .child(hint),
        );
    }
    if ftps_enabled {
        body = body
            .child(connection_editor_select(
                ConnectionEditorRenderContext {
                    palette,
                    fields,
                    cx,
                },
                "connection-editor-ftp-tls-version",
                t!("dialog.ftpTlsVersion"),
                ConnectionEditorSelect::FtpTlsVersion,
            ))
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(palette.text_muted))
                    .child(t!("dialog.ftpTlsVersionHint")),
            );
    }
    body.child(editor_field(
        palette,
        t!("dialog.ftpInitialDirectory"),
        ConnectionEditorField::FtpInitialDirectory,
        fields,
        cx,
    ))
    .child(editor_field(
        palette,
        t!("dialog.ftpLocalInitialDirectory"),
        ConnectionEditorField::FtpLocalInitialDirectory,
        fields,
        cx,
    ))
    .child(editor_field(
        palette,
        t!("dialog.ftpConnectTimeout"),
        ConnectionEditorField::FtpConnectTimeout,
        fields,
        cx,
    ))
    .child(
        div()
            .text_xs()
            .text_color(rgb(palette.text_muted))
            .child(t!("dialog.ftpAdvancedHint")),
    )
}
