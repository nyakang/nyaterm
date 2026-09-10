use rust_i18n::t;

use gpui::{
    AnyElement, App, ClickEvent, Context, FontWeight, IntoElement, KeyDownEvent, SharedString,
    Window, div, prelude::*, px, rgb, rgba, svg,
};

use crate::features::pages::settings::panel::SettingsPanel;
use crate::models::KeywordHighlightEditorField;
use crate::theme::ThemePalette;
use nyaterm_ui::NyaSwitch;
use nyaterm_ui::NyaTooltip;

use super::super::{
    settings_form_row, settings_form_section, settings_switch, settings_switch_with_enabled,
};
use super::helpers::parse_keyword_swatch;

impl SettingsPanel {
    pub(in crate::features) fn keyword_highlights_settings_section(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let is_dark = self.terminal_theme_is_dark();
        let rules = self.settings.keyword_config().rules.clone();
        let keyword_highlighting_enabled = self.settings.keyword_config().enabled;
        let expanded = self.settings.keyword_highlight_presentation().expanded_id;
        let builtin_ids = nyaterm_core::builtin_keyword_rule_ids();
        let untitled_rule_label = t!("settings.keywordHighlightNewRule");

        settings_form_section(
            palette,
            None,
            None,
            div()
                .flex()
                .flex_col()
                .gap_4()
                .child(settings_form_row(
                    palette,
                    t!("settings.keywordHighlightingExperimental"),
                    Some(SharedString::from(
                        t!("settings.keywordHighlightingExperimentalDesc"),
                    )),
                    settings_switch(
                        palette,
                        "settings-keyword-highlights-enabled",
                        keyword_highlighting_enabled,
                        cx.listener(|this, _, _, cx| {
                            this.toggle_keyword_highlights(cx);
                        }),
                    ),
                ))
                .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_4()
                            .child(div().text_xs().text_color(rgb(palette.text_muted))
                                .child(t!("settings.keywordHighlightLogicalLinesDesc")))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_size(px(12.))
                                            .font_weight(FontWeight(600.))
                                            .text_color(rgb(palette.text))
                                            .child(t!(
                                                "settings.keywordHighlightBuiltinRules",
                                            )),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(rgb(palette.text_muted))
                                            .child(t!(
                                                "settings.keywordHighlightBuiltinNote",
                                            )),
                                    )
                                    .child(
                                        div()
                                            .grid()
                                            .grid_cols(2)
                                            .gap_2()
                                            .children(builtin_ids.iter().map(|id| {
                                                let id = (*id).to_string();
                                                let label =
                                                    nyaterm_core::builtin_keyword_rule_label(&id);
                                                let swatch =
                                                    nyaterm_core::builtin_keyword_rule_swatch(
                                                        &id, is_dark,
                                                    );
                                                let enabled = self
                                                    .settings.keyword_config()
                                                    .builtin_rules
                                                    .get(&id)
                                                    .copied()
                                                    .unwrap_or(true);
                                                let color =
                                                    parse_keyword_swatch(swatch).unwrap_or(0x79c0ff);
                                                let rid = id.clone();
                                                div()
                                                    .rounded_md()
                                                    .border_1()
                                                    .border_color(rgb(palette.border))
                                                    .bg(rgb(palette.bg))
                                                    .px_3()
                                                    .py_2()
                                                    .flex()
                                                    .items_center()
                                                    .justify_between()
                                                    .gap_2()
                                                    .child(
                                                        div()
                                                            .flex()
                                                            .items_center()
                                                            .gap_2()
                                                            .min_w_0()
                                                            .child(
                                                                div()
                                                                    .size(px(10.))
                                                                    .rounded_full()
                                                                    .bg(rgb(color))
                                                                    .border_1()
                                                                    .border_color(rgb(
                                                                        palette.border,
                                                                    )),
                                                            )
                                                            .child(
                                                                div()
                                                                    .text_size(px(12.))
                                                                    .text_color(rgb(
                                                                        palette.text_muted,
                                                                    ))
                                                                    .overflow_hidden()
                                                                    .child(label),
                                                            ),
                                                    )
                                                    .child(settings_switch_with_enabled(
                                                        palette,
                                                        format!(
                                                            "settings-keyword-builtin-{id}"
                                                        ),
                                                        enabled,
                                                        keyword_highlighting_enabled,
                                                        cx.listener(move |this, _, _, cx| {
                                                            this.toggle_keyword_highlight_builtin(
                                                                rid.clone(),
                                                                cx,
                                                            );
                                                        }),
                                                    ))
                                            })),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_3()
                                    .opacity(if keyword_highlighting_enabled {
                                        1.0
                                    } else {
                                        0.5
                                    })
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .gap_2()
                                            .child(
                                                div()
                                                    .text_size(px(12.))
                                                    .font_weight(FontWeight(600.))
                                                    .text_color(rgb(palette.text))
                                                    .child(t!(
                                                        "settings.keywordHighlightRules",
                                                    )),
                                            )
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .gap_1()
                                                    .child(keyword_highlight_action_button(
                                                        palette,
                                                        "settings-keyword-highlights-import",
                                                        "icons/fe/upload.svg",
                                                        t!(
                                                            "settings.keywordHighlightImport",
                                                        ),
                                                        keyword_highlighting_enabled,
                                                        cx.listener(|this, _, _, cx| {
                                                            this.prompt_keyword_highlight_import(cx);
                                                        }),
                                                    ))
                                                    .child(keyword_highlight_action_button(
                                                        palette,
                                                        "settings-keyword-highlights-add",
                                                        "icons/conn/add.svg",
                                                        t!("common.add"),
                                                        keyword_highlighting_enabled,
                                                        cx.listener(|this, _, window, cx| {
                                                            this.add_keyword_highlight_rule(
                                                                window, cx,
                                                            );
                                                        }),
                                                    )),
                                            ),
                                    ),
                            )
                            .when(rules.is_empty(), |this| {
                                this.child(
                                    div()
                                        .rounded_md()
                                        .border_1()
                                        .border_color(rgb(palette.border))
                                        .bg(rgb(palette.input))
                                        .px_4()
                                        .py_6()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .child(
                                            div()
                                                .text_size(px(12.))
                                                .text_color(rgb(palette.text_dimmed))
                                                .child(t!(
                                                    "settings.keywordHighlightNoRules",
                                                )),
                                        ),
                                )
                            })
                            .children(rules.into_iter().map(|rule| {
                                let is_open = expanded.as_deref() == Some(rule.id.as_str());
                                let pattern_count =
                                    rule.patterns.iter().filter(|p| !p.trim().is_empty()).count();
                                let swatch = if is_dark {
                                    rule.color_dark.as_str()
                                } else {
                                    rule.color_light.as_str()
                                };
                                let color = parse_keyword_swatch(swatch).unwrap_or(0x79c0ff);
                                let rule_id = rule.id.clone();
                                let rule_id_toggle = rule.id.clone();
                                let rule_id_delete = rule.id.clone();
                                let patterns_value = rule.patterns.join("\n");
                                let name_control = if is_open && keyword_highlighting_enabled {
                                    let input = self.existing_text_input_box(Self::keyword_highlight_text_input_id(
                                            &rule.id,
                                            KeywordHighlightEditorField::Name,
                                        ), false);
                                    div()
                                        .id(SharedString::from(format!(
                                            "settings-keyword-rule-name-{}",
                                            rule.id
                                        )))
                                        .min_w(px(160.))
                                        .on_click(cx.listener({
                                            let rule_id = rule_id.clone();
                                            move |this, _, window, cx| {
                                                this.focus_keyword_highlight_field(
                                                    rule_id.clone(),
                                                    KeywordHighlightEditorField::Name,
                                                    window,
                                                    cx,
                                                );
                                            }
                                        }))
                                        .child(input)
                                        .into_any_element()
                                } else {
                                    keyword_highlight_static_input(
                                        palette,
                                        &rule.name,
                                        false,
                                        false,
                                        160.,
                                    )
                                };
                                let patterns_control = if is_open && keyword_highlighting_enabled {
                                    let input = self.existing_text_input_box(Self::keyword_highlight_text_input_id(
                                            &rule.id,
                                            KeywordHighlightEditorField::Patterns,
                                        ), true);
                                    div()
                                        .id(SharedString::from(format!(
                                            "settings-keyword-rule-patterns-{}",
                                            rule.id
                                        )))
                                        .font_family(crate::features::shell::gpui_code_font_family())
                                        .on_click(cx.listener({
                                            let rule_id = rule_id.clone();
                                            move |this, _, window, cx| {
                                                this.focus_keyword_highlight_field(
                                                    rule_id.clone(),
                                                    KeywordHighlightEditorField::Patterns,
                                                    window,
                                                    cx,
                                                );
                                            }
                                        }))
                                        .child(input)
                                        .into_any_element()
                                } else {
                                    keyword_highlight_static_input(
                                        palette,
                                        &patterns_value,
                                        true,
                                        true,
                                        0.,
                                    )
                                };
                                let dark_control = if is_open && keyword_highlighting_enabled {
                                    let input = self.existing_text_input_box(Self::keyword_highlight_text_input_id(
                                            &rule.id,
                                            KeywordHighlightEditorField::ColorDark,
                                        ), false);
                                    div()
                                        .id(SharedString::from(format!(
                                            "settings-keyword-rule-dark-{}",
                                            rule.id
                                        )))
                                        .w(px(96.))
                                        .font_family(crate::features::shell::gpui_code_font_family())
                                        .on_click(cx.listener({
                                            let rule_id = rule_id.clone();
                                            move |this, _, window, cx| {
                                                this.focus_keyword_highlight_field(
                                                    rule_id.clone(),
                                                    KeywordHighlightEditorField::ColorDark,
                                                    window,
                                                    cx,
                                                );
                                            }
                                        }))
                                        .child(input)
                                        .into_any_element()
                                } else {
                                    keyword_highlight_static_input(
                                        palette,
                                        &rule.color_dark,
                                        false,
                                        true,
                                        96.,
                                    )
                                };
                                let light_control = if is_open && keyword_highlighting_enabled {
                                    let input = self.existing_text_input_box(Self::keyword_highlight_text_input_id(
                                            &rule.id,
                                            KeywordHighlightEditorField::ColorLight,
                                        ), false);
                                    div()
                                        .id(SharedString::from(format!(
                                            "settings-keyword-rule-light-{}",
                                            rule.id
                                        )))
                                        .w(px(96.))
                                        .font_family(crate::features::shell::gpui_code_font_family())
                                        .on_click(cx.listener({
                                            let rule_id = rule_id.clone();
                                            move |this, _, window, cx| {
                                                this.focus_keyword_highlight_field(
                                                    rule_id.clone(),
                                                    KeywordHighlightEditorField::ColorLight,
                                                    window,
                                                    cx,
                                                );
                                            }
                                        }))
                                        .child(input)
                                        .into_any_element()
                                } else {
                                    keyword_highlight_static_input(
                                        palette,
                                        &rule.color_light,
                                        false,
                                        true,
                                        96.,
                                    )
                                };
                                let palette_dark = nyaterm_core::keyword_highlight_color_palette(true);
                                let palette_light =
                                    nyaterm_core::keyword_highlight_color_palette(false);

                                div()
                                    .id(SharedString::from(format!(
                                        "settings-keyword-rule-{}",
                                        rule.id
                                    )))
                                    .rounded_md()
                                    .border_1()
                                    .border_color(rgb(palette.border))
                                    .bg(rgb(palette.input))
                                    .overflow_hidden()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .id(SharedString::from(format!(
                                                "settings-keyword-rule-header-{}",
                                                rule.id
                                            )))
                                            .px_3()
                                            .py_2()
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .when(keyword_highlighting_enabled, |this| {
                                                this.cursor_pointer()
                                                    .hover(|this| this.bg(rgb(palette.hover)))
                                                    .on_click(cx.listener({
                                                        let rule_id = rule_id.clone();
                                                        move |this, _, _, cx| {
                                                            this.expand_keyword_highlight_rule(
                                                                rule_id.clone(),
                                                                cx,
                                                            );
                                                        }
                                                    }))
                                            })
                                            .child(
                                                div()
                                                    .size(px(10.))
                                                    .rounded_full()
                                                    .bg(rgb(color))
                                                    .border_1()
                                                    .border_color(rgb(palette.border))
                                                    .flex_none(),
                                            )
                                            .child(
                                                div()
                                                    .min_w_0()
                                                    .flex_1()
                                                    .text_size(px(12.))
                                                    .font_weight(FontWeight(600.))
                                                    .text_color(rgb(palette.text))
                                                    .overflow_hidden()
                                                    .child(if rule.name.trim().is_empty() {
                                                        untitled_rule_label.to_string()
                                                    } else {
                                                        rule.name.clone()
                                                    }),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(10.))
                                                    .text_color(rgb(palette.text_muted))
                                                    .child(
                                                        t!(
                                                            "settings.keywordHighlightPatternCount",
                                                            count = pattern_count
                                                        ),
                                                    ),
                                            )
                                            .child(keyword_highlight_rule_switch(
                                                palette,
                                                format!("settings-keyword-rule-enabled-{}", rule.id),
                                                rule.enabled,
                                                keyword_highlighting_enabled,
                                                cx.listener(move |this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.toggle_keyword_highlight_rule(
                                                        rule_id_toggle.clone(),
                                                        cx,
                                                    );
                                                }),
                                            ))
                                            .child(keyword_highlight_icon_button(
                                                palette,
                                                format!("settings-keyword-rule-delete-{}", rule.id),
                                                "icons/fe/delete.svg",
                                                t!("common.delete"),
                                                keyword_highlighting_enabled,
                                                cx.listener(move |this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.remove_keyword_highlight_rule(
                                                        rule_id_delete.clone(),
                                                        cx,
                                                    );
                                                }),
                                            ))
                                            .child(
                                                div()
                                                    .text_size(px(11.))
                                                    .text_color(rgb(palette.text_dimmed))
                                                    .child(
                                                        svg()
                                                            .size(px(14.))
                                                            .flex_none()
                                                            .path(if is_open {
                                                                "icons/chevron-down.svg"
                                                            } else {
                                                                "icons/fe/forward.svg"
                                                            })
                                                            .text_color(rgb(palette.text_dimmed)),
                                                    ),
                                            ),
                                    )
                                    .when(is_open, |this| {
                                        this.child(
                                            div()
                                                .border_t_1()
                                                .border_color(rgb(palette.border))
                                                .bg(rgb(palette.bg))
                                                .px_3()
                                                .py_3()
                                                .flex()
                                                .flex_col()
                                                .gap_3()
                                                .when(keyword_highlighting_enabled, |this| {
                                                    this.track_focus(
                                                        self.settings.keyword_highlight_focus(),
                                                    )
                                                    .on_key_down(cx.listener(
                                                        |this,
                                                         event: &KeyDownEvent,
                                                         window,
                                                         cx| {
                                                            if this
                                                                .handle_keyword_highlight_key_down(
                                                                    event, window, cx,
                                                                )
                                                            {
                                                                cx.stop_propagation();
                                                            }
                                                        },
                                                    ))
                                                })
                                                .child(settings_form_row(
                                                    palette,
                                                    t!(
                                                        "settings.keywordHighlightRuleName",
                                                    ),
                                                    None,
                                                    name_control,
                                                ))
                                                .child(
                                                    div()
                                                        .flex()
                                                        .flex_col()
                                                        .gap_2()
                                                        .child(
                                                            div()
                                                                .text_size(px(11.))
                                                                .text_color(rgb(
                                                                    palette.text_muted,
                                                                ))
                                                                .child(t!(
                                                                    "settings.keywordHighlightRulePatterns",
                                                                )),
                                                        )
                                                        .child(patterns_control),
                                                )
                                                .child(
                                                    div()
                                                        .flex()
                                                        .flex_col()
                                                        .gap_2()
                                                        .child(settings_form_row(
                                                            palette,
                                                            t!(
                                                                "settings.keywordHighlightDarkPalette",
                                                            ),
                                                            None,
                                                            div()
                                                                .flex()
                                                                .items_center()
                                                                .gap_2()
                                                                .child(
                                                                    div()
                                                                        .size(px(20.))
                                                                        .rounded_md()
                                                                        .bg(rgb(
                                                                            parse_keyword_swatch(
                                                                                &rule.color_dark,
                                                                            )
                                                                            .unwrap_or(0x79c0ff),
                                                                        ))
                                                                        .border_1()
                                                                        .border_color(rgb(
                                                                            palette.border,
                                                                        )),
                                                                )
                                                                .child(dark_control),
                                                        ))
                                                        .child(
                                                            div()
                                                                .flex()
                                                                .flex_wrap()
                                                                .gap_1()
                                                                .children(palette_dark.iter().map(
                                                                    |swatch| {
                                                                        let color = parse_keyword_swatch(swatch)
                                                                            .unwrap_or(0x79c0ff);
                                                                        let rid = rule_id.clone();
                                                                        let value = (*swatch).to_string();
                                                                        div()
                                                                            .id(SharedString::from(
                                                                                format!(
                                                                                    "settings-keyword-swatch-dark-{}-{swatch}",
                                                                                    rule.id
                                                                                ),
                                                                            ))
                                                                            .size(px(16.))
                                                                            .rounded_sm()
                                                                            .bg(rgb(color))
                                                                            .border_1()
                                                                            .border_color(rgb(
                                                                                palette.border,
                                                                            ))
                                                                            .when(
                                                                                keyword_highlighting_enabled,
                                                                                |this| {
                                                                                    this.cursor_pointer().on_click(cx.listener(
                                                                                        move |this, _, _, cx| {
                                                                                            this.set_keyword_highlight_rule_color(
                                                                                                rid.clone(),
                                                                                                true,
                                                                                                &value,
                                                                                                cx,
                                                                                            );
                                                                                        },
                                                                                    ))
                                                                                },
                                                                            )
                                                                    },
                                                                )),
                                                        )
                                                        .child(settings_form_row(
                                                            palette,
                                                            t!(
                                                                "settings.keywordHighlightLightPalette",
                                                            ),
                                                            None,
                                                            div()
                                                                .flex()
                                                                .items_center()
                                                                .gap_2()
                                                                .child(
                                                                    div()
                                                                        .size(px(20.))
                                                                        .rounded_md()
                                                                        .bg(rgb(
                                                                            parse_keyword_swatch(
                                                                                &rule.color_light,
                                                                            )
                                                                            .unwrap_or(0x0969da),
                                                                        ))
                                                                        .border_1()
                                                                        .border_color(rgb(
                                                                            palette.border,
                                                                        )),
                                                                )
                                                                .child(light_control),
                                                        ))
                                                        .child(
                                                            div()
                                                                .flex()
                                                                .flex_wrap()
                                                                .gap_1()
                                                                .children(
                                                                    palette_light.iter().map(
                                                                        |swatch| {
                                                                            let color =
                                                                                parse_keyword_swatch(
                                                                                    swatch,
                                                                                )
                                                                                .unwrap_or(
                                                                                    0x0969da,
                                                                                );
                                                                            let rid =
                                                                                rule_id.clone();
                                                                            let value =
                                                                                (*swatch)
                                                                                    .to_string();
                                                                            div()
                                                                                .id(SharedString::from(
                                                                                    format!(
                                                                                        "settings-keyword-swatch-light-{}-{swatch}",
                                                                                        rule.id
                                                                                    ),
                                                                                ))
                                                                                .size(px(16.))
                                                                                .rounded_sm()
                                                                                .bg(rgb(color))
                                                                                .border_1()
                                                                                .border_color(rgb(
                                                                                    palette.border,
                                                                                ))
                                                                                .when(
                                                                                    keyword_highlighting_enabled,
                                                                                    |this| {
                                                                                        this.cursor_pointer().on_click(
                                                                                            cx.listener(
                                                                                                move |this, _, _, cx| {
                                                                                                    this.set_keyword_highlight_rule_color(
                                                                                                        rid.clone(),
                                                                                                        false,
                                                                                                        &value,
                                                                                                        cx,
                                                                                                    );
                                                                                                },
                                                                                            ),
                                                                                        )
                                                                                    },
                                                                                )
                                                                        },
                                                                    ),
                                                                ),
                                                        ),
                                                ),
                                        )
                                    })
                            })),
                ),
        )
    }
}

fn keyword_highlight_static_input(
    palette: ThemePalette,
    value: &str,
    multi_line: bool,
    code_font: bool,
    min_width: f32,
) -> AnyElement {
    div()
        .when(min_width > 0., |this| this.min_w(px(min_width)))
        .when_else(
            multi_line,
            |this| this.h(px(88.)).py_2().items_start(),
            |this| this.h(px(30.)).items_center(),
        )
        .px_2()
        .rounded_sm()
        .border_1()
        .border_color(rgb(palette.border))
        .bg(rgb(palette.input))
        .flex()
        .text_xs()
        .text_color(rgb(palette.text_muted))
        .when(code_font, |this| {
            this.font_family(crate::features::shell::gpui_code_font_family())
        })
        .child(if value.is_empty() { " " } else { value }.to_string())
        .into_any_element()
}

fn keyword_highlight_action_button(
    palette: ThemePalette,
    id: impl Into<String>,
    icon_path: &'static str,
    label: impl Into<SharedString>,
    enabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let label: SharedString = label.into();
    let hover_bg = palette.hover;
    let hover_text = palette.text;

    div()
        .id(SharedString::from(id.into()))
        .h(px(28.))
        .px_3()
        .flex()
        .items_center()
        .rounded_sm()
        .border_1()
        .border_color(rgb(palette.border))
        .bg(rgb(palette.surface_elevated))
        .text_color(rgb(palette.text))
        .text_xs()
        .child(
            svg()
                .size(px(14.))
                .flex_none()
                .path(icon_path)
                .text_color(rgb(palette.text)),
        )
        .child(div().ml_1().child(label))
        .when(enabled, |this| {
            this.cursor_pointer()
                .hover(move |this| this.bg(rgb(hover_bg)).text_color(rgb(hover_text)))
                .on_click(on_click)
        })
}

fn keyword_highlight_icon_button(
    palette: ThemePalette,
    id: impl Into<String>,
    icon_path: &'static str,
    tooltip: impl Into<SharedString>,
    enabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let tooltip: SharedString = tooltip.into();
    let hover_bg = rgba((palette.danger << 8) | 0x18);

    div()
        .id(SharedString::from(id.into()))
        .size(px(28.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_sm()
        .text_color(rgb(palette.danger))
        .child(
            svg()
                .size(px(15.))
                .path(icon_path)
                .text_color(rgb(palette.danger)),
        )
        .tooltip(move |window, cx| NyaTooltip::new(tooltip.clone()).build(window, cx))
        .when(enabled, |this| {
            this.cursor_pointer()
                .hover(move |this| this.bg(hover_bg))
                .on_click(on_click)
        })
}

fn keyword_highlight_rule_switch(
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
