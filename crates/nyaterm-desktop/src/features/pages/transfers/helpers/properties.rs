use gpui::{
    AnyElement, FontWeight, IntoElement, ParentElement as _, SharedString, Styled as _, div,
    prelude::FluentBuilder as _, px, rgb,
};
use nyaterm_transport::SftpFileEntry;

use crate::models::TransferPropertiesState;
use crate::theme::ThemePalette;

use super::format_permissions_octal;

pub(in crate::features::pages::transfers) fn symlink_input_row(
    palette: ThemePalette,
    label: impl Into<SharedString>,
    invalid: bool,
    input: AnyElement,
) -> impl IntoElement {
    let label: SharedString = label.into();
    div()
        .flex()
        .items_center()
        .gap_3()
        .child(
            div()
                .w(px(80.))
                .flex_none()
                .text_xs()
                .text_color(rgb(palette.text_muted))
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                // The box draws its own border; a bad name reddens it from here.
                .when(invalid, |this| {
                    this.rounded_sm()
                        .border_1()
                        .border_color(rgb(palette.danger))
                })
                .child(input),
        )
}

pub(in crate::features::pages::transfers) fn property_row(
    palette: ThemePalette,
    label: impl Into<SharedString>,
    value: impl Into<SharedString>,
) -> impl IntoElement {
    let label: SharedString = label.into();
    div()
        .flex()
        .items_start()
        .gap_3()
        .text_xs()
        .child(
            div()
                .w(px(88.))
                .text_color(rgb(palette.text_muted))
                .child(format!("{label}:")),
        )
        .child(
            div()
                .min_w_0()
                .flex_1()
                .font_family(crate::features::shell::gpui_code_font_family())
                .text_color(rgb(palette.text))
                .child(value.into()),
        )
}

pub(in crate::features::pages::transfers) fn property_section_heading(
    palette: ThemePalette,
    label: impl Into<SharedString>,
) -> impl IntoElement {
    let label: SharedString = label.into();
    div()
        .mb_3()
        .text_size(px(10.))
        .font_weight(FontWeight(800.))
        .text_color(rgb(palette.text_muted))
        .child(label.to_ascii_uppercase())
}

pub(in crate::features::pages::transfers) fn transfer_properties_state_from_entry(
    entry: SftpFileEntry,
    session_id: Option<String>,
) -> TransferPropertiesState {
    let mode_value = entry
        .permissions
        .map(format_permissions_octal)
        .unwrap_or_else(|| "0644".to_string());
    TransferPropertiesState {
        symlink_target_value: String::new(),
        session_id,
        owner_value: String::new(),
        group_value: String::new(),
        entry,
        properties: None,
        mode_value,
        recursive: false,
        saving: false,
        error: None,
    }
}

pub(in crate::features::pages::transfers) fn parse_transfer_mode(value: &str) -> Option<u32> {
    let value = value.trim();
    if !(3..=4).contains(&value.len()) || !value.chars().all(|ch| ('0'..='7').contains(&ch)) {
        return None;
    }
    u32::from_str_radix(value, 8).ok()
}

pub(in crate::features::pages::transfers) fn format_owner_group(
    name: &str,
    id: Option<u32>,
) -> String {
    match (name.trim().is_empty(), id) {
        (true, Some(id)) => id.to_string(),
        (true, None) => "-".to_string(),
        (false, Some(id)) => format!("{} [{}]", name.trim(), id),
        (false, None) => name.trim().to_string(),
    }
}
