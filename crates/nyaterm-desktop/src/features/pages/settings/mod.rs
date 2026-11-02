use std::borrow::Cow;

use gpui::{
    App, ClickEvent, FontWeight, IntoElement, SharedString, Window, div, prelude::*, px, rgb, rgba,
};

use crate::theme::ThemePalette;
use nyaterm_ui::NyaSwitch;

use super::super::NyaTermApp;

mod ai;
mod flush;
mod inputs;
pub(in crate::features) mod panel;
mod security;
mod sync_backup;
mod tab_strip;
mod terminal;
mod transfer;
mod translation;
mod workspace;

// Row controls share one width regardless of their component type. Stacked
// fields and toolbar controls take their width from their own containers.
const SETTINGS_ROW_CONTROL_WIDTH_PX: f32 = 260.;

impl NyaTermApp {
    pub(in crate::features) fn settings_view(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.request_settings_panel_refresh(cx);
        self.settings_panel
            .clone()
            .cached(crate::features::layout::cached_panel_style())
    }
}

pub(super) fn settings_form_section(
    palette: ThemePalette,
    title: Option<Cow<'static, str>>,
    desc: Option<Cow<'static, str>>,
    content: impl IntoElement,
) -> impl IntoElement {
    div()
        .rounded_lg()
        .border_1()
        .border_color(rgba((palette.border << 8) | 0xb3))
        .bg(rgba((palette.surface << 8) | 0x99))
        .overflow_hidden()
        .when(title.is_some() || desc.is_some(), |this| {
            this.child(
                div()
                    .px_4()
                    .py_4()
                    .border_b_1()
                    .border_color(rgba((palette.surface_elevated << 8) | 0x99))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .when_some(title, |this, title| {
                        this.child(
                            div()
                                .text_size(px(14.))
                                .font_weight(FontWeight(600.))
                                .text_color(rgb(palette.text))
                                .child(title),
                        )
                    })
                    .when_some(desc, |this, desc| {
                        this.child(
                            div()
                                .text_size(px(12.))
                                .text_color(rgb(palette.text_dimmed))
                                .child(desc),
                        )
                    }),
            )
        })
        .child(div().px_4().py_4().flex().flex_col().gap_4().child(content))
}

/// Tauri SettingRow: label/desc left, control right.
pub(super) fn settings_form_row(
    palette: ThemePalette,
    label: impl Into<SharedString>,
    desc: Option<SharedString>,
    control: impl IntoElement,
) -> impl IntoElement {
    let label = label.into();
    div()
        .flex()
        .flex_wrap()
        .items_start()
        .justify_between()
        .gap_4()
        .child(
            div()
                .min_w_0()
                .min_w(px(200.))
                .flex_1()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_size(px(14.))
                        .font_weight(FontWeight(500.))
                        .text_color(rgb(palette.text))
                        .child(label),
                )
                .when_some(desc, |this, desc| {
                    this.child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(palette.text_dimmed))
                            .child(desc),
                    )
                }),
        )
        .child(
            div()
                .w(px(SETTINGS_ROW_CONTROL_WIDTH_PX))
                .flex_shrink_0()
                .min_w_0()
                .max_w_full()
                .flex()
                .items_center()
                .justify_end()
                .gap_2()
                .child(control),
        )
}

/// Gives an input a definite width inside a content-sized settings row control slot.
pub(super) fn settings_input_control(input: impl IntoElement) -> gpui::Div {
    div()
        .w(px(SETTINGS_ROW_CONTROL_WIDTH_PX))
        .max_w_full()
        .min_w_0()
        .flex()
        .child(div().min_w_0().flex_1().child(input))
}

/// Keeps an input and its trailing action on one line in a settings row.
pub(super) fn settings_input_action_control(
    width: f32,
    input: impl IntoElement,
    action: impl IntoElement,
) -> impl IntoElement {
    div()
        .w(px(width))
        .max_w_full()
        .min_w_0()
        .flex()
        .items_center()
        .gap_2()
        .child(div().min_w_0().flex_1().child(input))
        .child(div().flex_none().child(action))
}

/// Compact on/off switch control (Tauri SettingSwitch look).
pub(in crate::features::pages) fn settings_switch(
    palette: ThemePalette,
    id: impl Into<String>,
    checked: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    settings_switch_with_enabled(palette, id, checked, true, on_click)
}

pub(super) fn settings_switch_with_enabled(
    _palette: ThemePalette,
    id: impl Into<String>,
    checked: bool,
    enabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    NyaSwitch::new(id.into())
        .checked(checked)
        .disabled(!enabled)
        .on_click(move |_, window, cx| {
            if enabled {
                on_click(&ClickEvent::default(), window, cx);
            }
        })
}
