use std::borrow::Cow;

use gpui::{IntoElement, SharedString, div, prelude::*, px, rgb, rgba, svg};
use nyaterm_ui::{NyaDropdownMenu, NyaMenuItem};

pub(super) struct QuickCommandRowPresentation<'a> {
    pub command_id: &'a str,
    pub show_badge: bool,
    pub execution_mode: &'a str,
    /// Localized badge text. This helper is a free function with no translator of
    /// its own, so the row supplies the string it already looked up.
    pub badge_label: Cow<'static, str>,
}

pub(super) struct QuickCommandRowHandlers<OnRun, OnDetails> {
    pub on_run: OnRun,
    pub on_details: OnDetails,
    pub menu_items: Vec<NyaMenuItem>,
}

pub(super) fn quick_command_row_actions<OnRun, OnDetails>(
    palette: crate::theme::ThemePalette,
    presentation: QuickCommandRowPresentation<'_>,
    handlers: QuickCommandRowHandlers<OnRun, OnDetails>,
) -> impl IntoElement
where
    OnRun: Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
    OnDetails: Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
{
    let QuickCommandRowPresentation {
        command_id,
        show_badge,
        execution_mode,
        badge_label,
    } = presentation;
    let QuickCommandRowHandlers {
        on_run,
        on_details,
        menu_items,
    } = handlers;
    // Tauri renderCommandActions: optional badge + Send + Details + More menu.
    div()
        .flex()
        .items_center()
        .gap_1()
        .flex_none()
        .when(show_badge, |this| {
            this.child(quick_command_execution_badge(
                palette,
                execution_mode,
                badge_label,
            ))
        })
        .child(quick_command_action_icon_button(
            palette,
            format!("quick-command-run-{command_id}"),
            "icons/send.svg",
            on_run,
        ))
        .child(quick_command_action_icon_button(
            palette,
            format!("quick-command-detail-{command_id}"),
            "icons/eye.svg",
            on_details,
        ))
        .child(
            NyaDropdownMenu::new(format!("quick-command-more-{command_id}"))
                .icon("icons/session/more.svg")
                .icon_size(px(14.))
                .min_width(px(148.))
                .items(menu_items),
        )
}

/// Tauri's execution-mode badge: an outline chip carrying the mode's own glyph and
/// its localized name, rather than a colored `exec` / `append` tag.
fn quick_command_execution_badge(
    palette: crate::theme::ThemePalette,
    execution_mode: &str,
    label: impl Into<SharedString>,
) -> impl IntoElement {
    let label: SharedString = label.into();
    let icon = match execution_mode {
        "segmented" => "icons/send.svg",
        "append" => "icons/keyboard-return.svg",
        _ => "icons/conn/flash.svg",
    };
    div()
        .flex_none()
        .max_w(px(104.))
        .px(px(6.))
        .flex()
        .items_center()
        .gap_1()
        .rounded_md()
        .border_1()
        .border_color(rgba((palette.border << 8) | 0x66))
        .bg(rgba((palette.bg << 8) | 0x59))
        .text_size(px(10.))
        .line_height(px(16.))
        .text_color(rgb(palette.text_muted))
        .child(
            svg()
                .size(px(12.))
                .flex_none()
                .path(icon)
                .text_color(rgb(palette.text_muted)),
        )
        .child(div().min_w_0().truncate().child(label))
}

fn quick_command_action_icon_button(
    palette: crate::theme::ThemePalette,
    id: impl Into<String>,
    icon_path: &'static str,
    on_click: impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    div()
        .id(SharedString::from(id.into()))
        .size(px(26.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .text_color(rgb(palette.text_muted))
        .cursor_pointer()
        .hover(|this| {
            this.bg(rgb(palette.surface_elevated))
                .text_color(rgb(palette.text))
        })
        .child(
            svg()
                .size(px(14.))
                .flex_none()
                .path(icon_path)
                .text_color(rgb(palette.text_muted)),
        )
        .on_click(on_click)
}
