use gpui::{FontWeight, IntoElement, SharedString, div, prelude::*, px, rgb, svg};
use nyaterm_transport::SessionKind;
use nyaterm_ui::NyaNumberInputOptions;

use crate::features::NyaTermApp;

/// A captioned box hosting one of the security editors' inputs.
///
/// `id` is the stable part of the field's registry key, so an edit finds its
/// way back to the right draft without the editor tracking a focused field.
pub(super) fn security_editor_field(
    app: &mut NyaTermApp,
    id: &'static str,
    label: impl Into<SharedString>,
    value: String,
    setup: crate::features::text_inputs::TextInputSetup,
    cx: &mut gpui::Context<NyaTermApp>,
) -> gpui::AnyElement {
    let label: SharedString = label.into();
    let field_id = format!("security.editor.{id}");
    let field = app.text_input(field_id.clone(), &value, setup.clone(), cx);
    let busy = app.security.editor_busy();
    field.update(cx, |field, cx| {
        field.set_disabled(busy, cx);
    });
    app.text_input_field(field_id, label, &value, setup, cx)
        .into_any_element()
}

pub(super) fn security_number_editor_field(
    app: &mut NyaTermApp,
    id: &'static str,
    label: impl Into<SharedString>,
    value: String,
    setup: NyaNumberInputOptions,
    cx: &mut gpui::Context<NyaTermApp>,
) -> gpui::AnyElement {
    let label: SharedString = label.into();
    let palette = app.theme_palette();
    let field_id = format!("security.editor.{id}");
    let field = app.number_input(field_id.clone(), &value, setup.clone(), cx);
    let busy = app.security.editor_busy();
    field.update(cx, |field, cx| {
        field.set_disabled(busy, cx);
    });
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
        .child(app.number_input_box(field_id, &value, setup, cx))
        .into_any_element()
}

pub(super) fn security_type_chip(
    palette: crate::theme::ThemePalette,
    label: &'static str,
    selected: bool,
    disabled: bool,
    on_click: impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    div()
        .id(SharedString::from(format!("security-type-{label}")))
        .h(px(22.))
        .px_2()
        .flex()
        .items_center()
        .rounded_sm()
        .text_size(px(10.))
        .font_weight(FontWeight(700.))
        .when(!disabled, |this| this.cursor_pointer())
        .text_color(if selected {
            rgb(palette.success)
        } else {
            rgb(palette.text_muted)
        })
        .bg(if selected {
            rgb(0x12261a)
        } else {
            rgb(palette.surface_elevated)
        })
        .when(!disabled, |this| {
            this.hover(|this| this.bg(rgb(palette.border)))
        })
        .when(disabled, |this| this.opacity(0.5))
        .child(label)
        .on_click(move |event, window, cx| {
            if !disabled {
                on_click(event, window, cx);
            }
        })
}

pub(super) fn session_action_svg_button(
    palette: crate::theme::ThemePalette,
    id: impl Into<String>,
    icon_path: &'static str,
    tooltip: impl Into<String>,
    enabled: bool,
    on_click: impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    // Tauri ActiveSessions action icons: h-7 ghost.
    let tooltip = tooltip.into();
    let id = SharedString::from(id.into());
    let icon_color = if enabled {
        palette.text_muted
    } else {
        palette.text_dimmed
    };
    div()
        .id(id.clone())
        .group(id.clone())
        .size(px(28.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .text_color(rgb(icon_color))
        .when(enabled, |this| {
            this.cursor_pointer().hover(|this| {
                this.bg(rgb(palette.surface_elevated))
                    .text_color(rgb(palette.text))
            })
        })
        .when(!enabled, |this| this.opacity(0.4))
        .child(
            svg()
                .size(px(16.))
                .flex_none()
                .path(icon_path)
                .text_color(rgb(icon_color))
                .when(enabled, |this| {
                    this.group_hover(id, move |this| this.text_color(rgb(palette.text)))
                }),
        )
        .tooltip(move |window, cx| nyaterm_ui::NyaTooltip::new(tooltip.clone()).build(window, cx))
        .on_click(move |event, window, cx| {
            if enabled {
                on_click(event, window, cx);
            }
        })
}

pub(super) fn format_otp_code_display(code: &str) -> String {
    let trimmed = code.trim();
    if trimmed.is_empty() || trimmed == "------" {
        return "------".to_string();
    }
    let digits: String = trimmed.chars().filter(|c| !c.is_whitespace()).collect();
    digits
        .as_bytes()
        .chunks(3)
        .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

pub(super) fn session_kind_icon_path(kind: SessionKind) -> &'static str {
    match kind {
        SessionKind::Ssh => "icons/conn/server.svg",
        SessionKind::Telnet | SessionKind::RawTcp => "icons/conn/telnet.svg",
        SessionKind::Serial => "icons/conn/serial.svg",
        SessionKind::LocalPty => "icons/conn/terminal.svg",
        SessionKind::Rdp => "icons/conn/rdp.svg",
        SessionKind::Vnc => "icons/conn/vnc.svg",
    }
}
