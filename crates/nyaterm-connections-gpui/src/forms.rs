use gpui::{IntoElement, SharedString, div, prelude::*, px, rgb, svg};
use nyaterm_ui::{NYA_FORM_CONTROL_HEIGHT_PX, NyaSelect, NyaSwitch, ThemePalette};
use std::borrow::Cow;
pub const EDITOR_CONTROL_HEIGHT_PX: f32 = NYA_FORM_CONTROL_HEIGHT_PX;
/// A field's caption, and whether it carries the required marker.
#[derive(Clone, Default)]
pub struct FieldLabel {
    text: SharedString,
    required: bool,
}

impl FieldLabel {
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
}

impl From<&'static str> for FieldLabel {
    fn from(text: &'static str) -> Self {
        Self {
            text: SharedString::from(text),
            required: false,
        }
    }
}

impl From<String> for FieldLabel {
    fn from(text: String) -> Self {
        Self {
            text: SharedString::from(text),
            required: false,
        }
    }
}

impl<'a> From<Cow<'a, str>> for FieldLabel {
    fn from(text: Cow<'a, str>) -> Self {
        Self {
            text: SharedString::from(text),
            required: false,
        }
    }
}

impl From<SharedString> for FieldLabel {
    fn from(text: SharedString) -> Self {
        Self {
            text,
            required: false,
        }
    }
}

/// Mark a caption as required, so it renders with the red asterisk.
pub fn required(label: impl Into<FieldLabel>) -> FieldLabel {
    let mut label = label.into();
    label.required = true;
    label
}

/// The caption above an input or a select.
pub fn field_caption(palette: ThemePalette, label: &FieldLabel) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap_0p5()
        .text_xs()
        .text_color(rgb(palette.text_muted))
        .child(label.text.clone())
        .when(label.required, |this| {
            this.child(div().text_color(rgb(palette.danger)).child("*"))
        })
}

pub fn connection_tree_indent_px(depth: usize) -> f32 {
    if depth == 0 {
        8.
    } else {
        8. + depth as f32 * 16. + 16.
    }
}
pub fn editor_switch_row(
    palette: ThemePalette,
    id: &'static str,
    label: impl Into<SharedString>,
    checked: bool,
    on_click: impl Fn(&bool, &mut gpui::Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    let label: SharedString = label.into();
    div()
        .w_full()
        .min_h(px(EDITOR_CONTROL_HEIGHT_PX))
        .flex_shrink_0()
        .py_1()
        .flex()
        .items_center()
        .justify_between()
        .gap_3()
        .text_xs()
        .text_color(rgb(palette.text))
        .child(div().min_w_0().flex_1().child(label.clone()))
        .child(
            div().flex_shrink_0().child(
                NyaSwitch::new(id)
                    .checked(checked)
                    .tooltip(label)
                    .on_click(on_click),
            ),
        )
}
pub fn icon_action_button(
    palette: ThemePalette,
    id: impl Into<String>,
    label: &'static str,
    tooltip: impl Into<String>,
    on_click: impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    icon_action_button_styled(palette, id, label, tooltip, None, false, on_click)
}
pub fn icon_action_button_styled(
    palette: ThemePalette,
    id: impl Into<String>,
    label: &'static str,
    tooltip: impl Into<String>,
    tint: Option<u32>,
    flip_vertical: bool,
    on_click: impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    // label may be a glyph fallback or an icons/*.svg path.
    let is_svg = label.starts_with("icons/") && label.ends_with(".svg");
    let tooltip = tooltip.into();
    let id = SharedString::from(id.into());
    let base_color = tint.unwrap_or(palette.text_muted);
    let hover_color = tint.unwrap_or(palette.text);
    // `svg()` takes its tint from its *own* computed style — GPUI starts every
    // element from `Style::default()`, so a glyph with no `text_color` of its own
    // resolves to `None` and is skipped entirely at paint time. The parent's
    // `text_color` only reaches real text. Hence the explicit color here, and the
    // group so hover still brightens the icon rather than only the background.
    div()
        .id(id.clone())
        .group(id.clone())
        .size(px(24.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .text_size(px(11.))
        .text_color(rgb(base_color))
        .cursor_pointer()
        .hover(move |this| {
            this.bg(rgb(palette.surface_elevated))
                .text_color(rgb(hover_color))
        })
        .on_click(on_click)
        .tooltip(move |window, cx| nyaterm_ui::NyaTooltip::new(tooltip.clone()).build(window, cx))
        .when(is_svg, |this| {
            let mut icon = svg()
                .size(px(14.))
                .flex_none()
                .path(label)
                .text_color(rgb(base_color))
                .group_hover(id.clone(), move |this| this.text_color(rgb(hover_color)));
            if flip_vertical {
                icon = icon.with_transformation(gpui::Transformation::scale(gpui::size(1., -1.)));
            }
            this.child(icon)
        })
        .when(!is_svg, |this| this.child(label))
}

pub fn select_field(
    palette: ThemePalette,
    id: &'static str,
    label: impl Into<FieldLabel>,
    anchor: Option<gpui::ScrollAnchor>,
    control: NyaSelect,
) -> impl IntoElement {
    let label = label.into();
    div()
        .id(SharedString::from(id))
        .anchor_scroll(anchor)
        .min_w_0()
        .flex_1()
        .flex()
        .flex_col()
        .gap_1()
        .when(!label.is_empty(), |this| {
            this.child(field_caption(palette, &label))
        })
        .child(
            div()
                .id(SharedString::from(format!("{id}-container")))
                .h(px(EDITOR_CONTROL_HEIGHT_PX))
                .min_w_0()
                .child(control),
        )
}
