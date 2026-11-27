use gpui::{IntoElement, SharedString, Stateful, div, prelude::*, px};
use nyaterm_ui::NyaTooltip;

pub(super) fn send_command_control_group(
    _palette: crate::theme::ThemePalette,
    label: impl Into<SharedString>,
    content: impl IntoElement,
) -> impl IntoElement {
    let label = label.into();
    div()
        .id(label.clone())
        .h(px(32.))
        .flex_none()
        .flex()
        .items_center()
        .tooltip(move |window, cx| NyaTooltip::new(label.clone()).compact().build(window, cx))
        .child(content)
}

pub(super) fn send_command_compact_tip(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    content: impl IntoElement,
) -> Stateful<gpui::Div> {
    let label = label.into();
    div()
        .id(id.into())
        .flex_none()
        .flex()
        .items_center()
        .tooltip(move |window, cx| NyaTooltip::new(label.clone()).compact().build(window, cx))
        .child(content)
}
