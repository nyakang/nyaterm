use gpui::{App, ClickEvent, IntoElement, SharedString, Window, div, prelude::*, px, rgb, svg};

use nyaterm_ui::NyaTooltip;
use nyaterm_ui::ThemePalette;

pub(crate) fn usage_color(palette: ThemePalette, ratio: f64) -> gpui::Hsla {
    if ratio >= 0.9 {
        rgb(0xfb7185).into()
    } else if ratio >= 0.7 {
        rgb(palette.warning).into()
    } else {
        rgb(palette.primary).into()
    }
}

pub(super) fn process_usage_color(palette: ThemePalette, ratio: f64) -> gpui::Hsla {
    if ratio >= 0.9 {
        rgb(palette.danger).into()
    } else if ratio >= 0.7 {
        rgb(palette.warning).into()
    } else {
        rgb(palette.text).into()
    }
}

pub(crate) fn compact_remote_svg_button(
    palette: ThemePalette,
    id: impl Into<String>,
    icon_path: &'static str,
    tooltip: impl Into<String>,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let tooltip = tooltip.into();
    div()
        .id(SharedString::from(id.into()))
        .size(px(28.))
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
        .tooltip(move |window, cx| NyaTooltip::new(tooltip.clone()).build(window, cx))
        .child(
            svg()
                .size(px(16.))
                .flex_none()
                .path(icon_path)
                .text_color(rgb(palette.text_muted)),
        )
        .on_click(on_click)
}
