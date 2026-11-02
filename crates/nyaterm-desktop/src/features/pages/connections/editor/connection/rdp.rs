use crate::models::ConnectionEditorSection;
use rust_i18n::t;

use gpui::{
    Context, FontWeight, IntoElement, SharedString, div,
    prelude::{FluentBuilder, InteractiveElement, ParentElement, Styled},
    px, rgb,
};
use nyaterm_ui::NyaSwitch;

use crate::features::{NyaTermApp, connections::ConnectionEditorToggle};
use crate::models::{ConnectionEditorField, ConnectionEditorSelect};

use super::super::super::list::{
    ConnectionEditorRenderContext, connection_editor_select, editor_field, editor_stepper_field,
    required,
};
use super::ConnectionEditorSectionContext;

fn rdp_switch_row(
    palette: crate::theme::ThemePalette,
    id: &'static str,
    label: impl Into<SharedString>,
    description: impl Into<SharedString>,
    checked: bool,
    on_click: impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    let label: SharedString = label.into();
    let description: SharedString = description.into();
    div()
        .rounded_md()
        .border_1()
        .border_color(rgb(palette.border))
        .px_3()
        .py_2()
        .flex()
        .items_start()
        .justify_between()
        .gap_3()
        .child(
            div()
                .min_w_0()
                .flex_1()
                .child(div().text_xs().font_weight(FontWeight(500.)).child(label))
                .child(
                    div()
                        .mt_1()
                        .text_size(px(10.))
                        .text_color(rgb(palette.text_muted))
                        .child(description),
                ),
        )
        .child(
            NyaSwitch::new(id)
                .checked(checked)
                .on_click(move |_, window, cx| {
                    on_click(&gpui::ClickEvent::default(), window, cx);
                }),
        )
}

pub(super) fn connection_editor_rdp_section(
    section: ConnectionEditorSectionContext<'_>,
    cx: &mut Context<NyaTermApp>,
) -> gpui::Div {
    let content = match section.editor.section {
        ConnectionEditorSection::Basic => div()
            .flex()
            .flex_col()
            .gap_3()
            .child(rdp_basic(section, cx))
            .child(rdp_authentication(section, cx)),
        ConnectionEditorSection::Security => rdp_security(section, cx),
        ConnectionEditorSection::Network => rdp_network(section, cx),
        ConnectionEditorSection::Display => rdp_display(section, cx),
        ConnectionEditorSection::Session => rdp_session(section, cx),
        _ => div(),
    };
    div()
        .debug_selector(|| "connection-editor-rdp-section".to_string())
        .flex()
        .flex_col()
        .gap_3()
        .child(content)
}

fn rdp_basic(
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

fn rdp_authentication(
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
                .flex()
                .when(!narrow, |this| this.flex_wrap())
                .when(narrow, |this| this.flex_col())
                .gap_3()
                .child(div().min_w(px(180.)).flex_1().child(editor_field(
                    palette,
                    required(t!("dialog.username")),
                    ConnectionEditorField::Username,
                    fields,
                    cx,
                )))
                .child(div().min_w(px(160.)).flex_1().child(editor_field(
                    palette,
                    t!("dialog.rdpDomain"),
                    ConnectionEditorField::Domain,
                    fields,
                    cx,
                ))),
        )
        .child(super::authentication::password_authentication(section, cx))
}

fn rdp_security(
    section: ConnectionEditorSectionContext<'_>,
    cx: &mut Context<NyaTermApp>,
) -> gpui::Div {
    let ConnectionEditorSectionContext {
        palette,
        editor,
        fields,
        ..
    } = section;
    div()
        .flex()
        .flex_col()
        .gap_3()
        .flex()
        .flex_col()
        .gap_3()
        .child(rdp_switch_row(
            palette,
            "connection-rdp-use-nla",
            t!("dialog.rdpUseNla"),
            t!("dialog.rdpUseNlaDesc"),
            editor.rdp_security.use_nla,
            cx.listener(|this, _, _, cx| {
                this.toggle_connection_editor_flag(ConnectionEditorToggle::RdpUseNla, cx)
            }),
        ))
        .child(rdp_switch_row(
            palette,
            "connection-rdp-administrative-session",
            t!("dialog.rdpAdministrativeSession"),
            t!("dialog.rdpAdministrativeSessionDesc"),
            editor.rdp_security.administrative_session,
            cx.listener(|this, _, _, cx| {
                this.toggle_connection_editor_flag(
                    ConnectionEditorToggle::RdpAdministrativeSession,
                    cx,
                )
            }),
        ))
        .child(connection_editor_select(
            ConnectionEditorRenderContext {
                palette,
                fields,
                cx,
            },
            "connection-editor-rdp-certificate-policy",
            if matches!(editor.rdp_backend.as_str(), "windows" | "active-x") {
                t!("dialog.rdpWindowsAuthentication")
            } else {
                t!("dialog.rdpCertificatePolicy")
            },
            ConnectionEditorSelect::RdpCertificatePolicy,
        ))
}

fn rdp_network(
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
        .flex()
        .flex_col()
        .gap_3()
        .child(connection_editor_select(
            ConnectionEditorRenderContext {
                palette,
                fields,
                cx,
            },
            "connection-editor-rdp-proxy",
            t!("dialog.proxySelect"),
            ConnectionEditorSelect::Proxy,
        ))
        .child(connection_editor_select(
            ConnectionEditorRenderContext {
                palette,
                fields,
                cx,
            },
            "connection-editor-rdp-jump",
            t!("dialog.proxyJump"),
            ConnectionEditorSelect::ProxyJump,
        ))
}

fn rdp_display(
    section: ConnectionEditorSectionContext<'_>,
    cx: &mut Context<NyaTermApp>,
) -> gpui::Div {
    let ConnectionEditorSectionContext {
        palette,
        editor,
        fields,
        ..
    } = section;
    div()
        .flex()
        .flex_col()
        .gap_3()
        .flex()
        .flex_col()
        .gap_3()
        .child(connection_editor_select(
            ConnectionEditorRenderContext {
                palette,
                fields,
                cx,
            },
            "connection-editor-rdp-display-mode",
            t!("dialog.rdpDisplayMode"),
            ConnectionEditorSelect::RdpDisplayMode,
        ))
        .when(editor.rdp_display.mode == "fixed", |this| {
            this.child(
                div()
                    .flex()
                    .gap_3()
                    .child(div().min_w_0().flex_1().child(editor_stepper_field(
                        palette,
                        t!("dialog.rdpWidth"),
                        ConnectionEditorField::RdpDisplayWidth,
                        fields,
                        cx,
                    )))
                    .child(div().min_w_0().flex_1().child(editor_stepper_field(
                        palette,
                        t!("dialog.rdpHeight"),
                        ConnectionEditorField::RdpDisplayHeight,
                        fields,
                        cx,
                    ))),
            )
        })
        .child(
            div()
                .text_xs()
                .text_color(rgb(palette.text_muted))
                .child(t!("dialog.rdpGraphicsPipelineUnavailable")),
        )
        .child(rdp_switch_row(
            palette,
            "connection-rdp-display-wallpaper",
            t!("dialog.rdpWallpaper"),
            t!("dialog.rdpWallpaperDesc"),
            editor.rdp_display.display_wallpaper,
            cx.listener(|this, _, _, cx| {
                this.toggle_connection_editor_flag(ConnectionEditorToggle::RdpDisplayWallpaper, cx)
            }),
        ))
        .child(rdp_switch_row(
            palette,
            "connection-rdp-display-themes",
            t!("dialog.rdpThemes"),
            t!("dialog.rdpThemesDesc"),
            editor.rdp_display.display_themes,
            cx.listener(|this, _, _, cx| {
                this.toggle_connection_editor_flag(ConnectionEditorToggle::RdpDisplayThemes, cx)
            }),
        ))
        .child(rdp_switch_row(
            palette,
            "connection-rdp-font-smoothing",
            t!("dialog.rdpFontSmoothing"),
            t!("dialog.rdpFontSmoothingDesc"),
            editor.rdp_display.enable_font_smoothing,
            cx.listener(|this, _, _, cx| {
                this.toggle_connection_editor_flag(ConnectionEditorToggle::RdpFontSmoothing, cx)
            }),
        ))
        .child(rdp_switch_row(
            palette,
            "connection-rdp-desktop-composition",
            t!("dialog.rdpDesktopComposition"),
            t!("dialog.rdpDesktopCompositionDesc"),
            editor.rdp_display.enable_desktop_composition,
            cx.listener(|this, _, _, cx| {
                this.toggle_connection_editor_flag(
                    ConnectionEditorToggle::RdpDesktopComposition,
                    cx,
                )
            }),
        ))
        .child(rdp_switch_row(
            palette,
            "connection-rdp-disable-full-window-drag",
            t!("dialog.rdpDisableFullWindowDrag"),
            t!("dialog.rdpDisableFullWindowDragDesc"),
            editor.rdp_display.disable_full_window_drag,
            cx.listener(|this, _, _, cx| {
                this.toggle_connection_editor_flag(ConnectionEditorToggle::RdpFullWindowDrag, cx)
            }),
        ))
        .child(rdp_switch_row(
            palette,
            "connection-rdp-disable-menu-animations",
            t!("dialog.rdpDisableMenuAnimations"),
            t!("dialog.rdpDisableMenuAnimationsDesc"),
            editor.rdp_display.disable_menu_animations,
            cx.listener(|this, _, _, cx| {
                this.toggle_connection_editor_flag(ConnectionEditorToggle::RdpMenuAnimations, cx)
            }),
        ))
        .child(rdp_switch_row(
            palette,
            "connection-rdp-disable-cursor-shadow",
            t!("dialog.rdpDisableCursorShadow"),
            t!("dialog.rdpDisableCursorShadowDesc"),
            editor.rdp_display.disable_cursor_shadow,
            cx.listener(|this, _, _, cx| {
                this.toggle_connection_editor_flag(ConnectionEditorToggle::RdpCursorShadow, cx)
            }),
        ))
        .child(rdp_switch_row(
            palette,
            "connection-rdp-disable-cursor-blinking",
            t!("dialog.rdpDisableCursorBlinking"),
            t!("dialog.rdpDisableCursorBlinkingDesc"),
            editor.rdp_display.disable_cursor_blinking,
            cx.listener(|this, _, _, cx| {
                this.toggle_connection_editor_flag(ConnectionEditorToggle::RdpCursorBlinking, cx)
            }),
        ))
}

fn rdp_session(
    section: ConnectionEditorSectionContext<'_>,
    cx: &mut Context<NyaTermApp>,
) -> gpui::Div {
    let ConnectionEditorSectionContext {
        palette,
        editor,
        fields,
        ..
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
            "connection-editor-rdp-backend",
            t!("dialog.rdpBackend"),
            ConnectionEditorSelect::RdpBackend,
        ))
        .child(
            div()
                .text_xs()
                .text_color(rgb(palette.text_muted))
                .child(t!("dialog.rdpBackendDesc")),
        )
        .child(connection_editor_select(
            ConnectionEditorRenderContext {
                palette,
                fields,
                cx,
            },
            "connection-editor-rdp-clipboard-mode",
            t!("dialog.rdpClipboard"),
            ConnectionEditorSelect::RdpClipboardMode,
        ))
        .flex()
        .flex_col()
        .gap_3()
        .child(rdp_switch_row(
            palette,
            "connection-rdp-auto-reconnect",
            t!("dialog.rdpAutoReconnect"),
            t!("dialog.rdpAutoReconnectDesc"),
            editor.rdp_reconnect.enabled,
            cx.listener(|this, _, _, cx| {
                this.toggle_connection_editor_flag(ConnectionEditorToggle::RdpReconnect, cx)
            }),
        ))
        .child(editor_stepper_field(
            palette,
            t!("dialog.rdpReconnectAttempts"),
            ConnectionEditorField::RdpReconnectAttempts,
            fields,
            cx,
        ))
}
