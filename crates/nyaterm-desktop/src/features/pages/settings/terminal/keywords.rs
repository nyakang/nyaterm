use rust_i18n::t;

use gpui::{
    AnyElement, App, ClickEvent, Context, Entity, FontWeight, IntoElement, KeyDownEvent,
    SharedString, Subscription, Window, div, prelude::*, px, rgb, rgba, svg,
};

use crate::features::pages::settings::panel::SettingsPanel;
use crate::features::view_widgets::keyword_match_option_toggles;
use crate::models::KeywordHighlightEditorField;
use crate::models::KeywordMatchOption;
use crate::theme::ThemePalette;
use nyaterm_core::keyword_highlight_presets::{
    builtin_keyword_rule_ids, builtin_keyword_rule_is_specialized, builtin_keyword_rule_label,
    keyword_highlight_color_palette,
};
use nyaterm_ui::{
    ColorSelect, NYA_FORM_CONTROL_HEIGHT_PX, NyaColorPickerEvent, NyaColorPickerState, NyaSwitch,
};

use super::super::{
    settings_form_row, settings_form_section, settings_switch, settings_switch_with_enabled,
};
use super::helpers::parse_keyword_swatch;

/// Colour-select controls for the custom-rule editor. Keyed by
/// `{rule_id}:{field}` so a control is stable across presentation refreshes.
#[derive(Default)]
pub(in crate::features::pages::settings) struct KeywordColorPickerControls {
    fields: std::collections::HashMap<String, (Entity<NyaColorPickerState>, Subscription, String)>,
}

impl KeywordColorPickerControls {
    fn control_key(rule_id: &str, field: KeywordHighlightEditorField) -> String {
        format!("{}:{}", rule_id, field.input_key())
    }

    fn color_of(
        rule: &nyaterm_core::KeywordHighlightRule,
        field: KeywordHighlightEditorField,
    ) -> &str {
        match field {
            KeywordHighlightEditorField::ColorDark => &rule.color_dark,
            KeywordHighlightEditorField::ColorLight => &rule.color_light,
            KeywordHighlightEditorField::BgDark => &rule.bg_color_dark,
            KeywordHighlightEditorField::BgLight => &rule.bg_color_light,
            _ => "",
        }
    }

    fn featured_colors(is_dark: bool) -> Vec<gpui::Hsla> {
        keyword_highlight_color_palette(is_dark)
            .iter()
            .filter_map(|value| nyaterm_core::themes::parse_color(value))
            .map(|bytes| gpui::Hsla::from(rgba(u32::from_be_bytes(bytes))))
            .collect()
    }

    /// Create/update/discard picker states to match the current rules.
    pub(in crate::features::pages::settings) fn sync(
        &mut self,
        rules: &[nyaterm_core::KeywordHighlightRule],
        window: &mut Window,
        cx: &mut Context<SettingsPanel>,
    ) {
        const FIELDS: [KeywordHighlightEditorField; 4] = [
            KeywordHighlightEditorField::ColorDark,
            KeywordHighlightEditorField::BgDark,
            KeywordHighlightEditorField::ColorLight,
            KeywordHighlightEditorField::BgLight,
        ];
        let mut live = std::collections::HashSet::new();
        for rule in rules {
            for field in FIELDS {
                let key = Self::control_key(&rule.id, field);
                live.insert(key.clone());
                let value = Self::color_of(rule, field).to_string();
                if let Some((picker, _, previous)) = self.fields.get_mut(&key) {
                    if *previous != value {
                        Self::apply_value(picker, &value, window, cx);
                        *previous = value;
                    }
                    continue;
                }
                let picker = cx.new(|cx| {
                    let mut state = NyaColorPickerState::new(window, cx);
                    if let Some(bytes) = nyaterm_core::themes::parse_color(&value) {
                        state = state.default_value(rgba(u32::from_be_bytes(bytes)));
                    }
                    state
                });
                let rule_id = rule.id.clone();
                let subscription = cx.subscribe(&picker, move |panel, _, event, cx| {
                    let NyaColorPickerEvent::Change(color) = event;
                    let value = match color {
                        Some(color) => {
                            let c = gpui::Rgba::from(*color);
                            let channel = |v: f32| (v * 255.).round().clamp(0., 255.) as u8;
                            format!(
                                "#{:02x}{:02x}{:02x}",
                                channel(c.r),
                                channel(c.g),
                                channel(c.b)
                            )
                        }
                        None => String::new(),
                    };
                    panel.with_app(cx, |app, cx| {
                        app.set_keyword_highlight_rule_color_field(
                            rule_id.clone(),
                            field,
                            value.as_str(),
                            cx,
                        );
                    });
                });
                self.fields.insert(key, (picker, subscription, value));
            }
        }
        let stale: Vec<String> = self
            .fields
            .keys()
            .filter(|key| !live.contains(*key))
            .cloned()
            .collect();
        for key in stale {
            self.fields.remove(&key);
        }
    }

    fn apply_value(
        picker: &Entity<NyaColorPickerState>,
        value: &str,
        window: &mut Window,
        cx: &mut Context<SettingsPanel>,
    ) {
        picker.update(cx, |picker, cx| {
            if let Some(bytes) = nyaterm_core::themes::parse_color(value) {
                picker.set_value(rgba(u32::from_be_bytes(bytes)), window, cx);
            } else {
                picker.clear_value(window, cx);
            }
        });
    }

    pub(in crate::features::pages::settings) fn get(
        &self,
        rule_id: &str,
        field: KeywordHighlightEditorField,
    ) -> Option<&Entity<NyaColorPickerState>> {
        self.fields
            .get(&Self::control_key(rule_id, field))
            .map(|(picker, _, _)| picker)
    }
}

impl SettingsPanel {
    /// Render a `ColorSelect` for a rule colour field, or a static swatch when
    /// the rule is collapsed or highlighting is disabled.
    fn keyword_color_select(
        &self,
        rule_id: &str,
        field: KeywordHighlightEditorField,
        value: &str,
        interactive: bool,
        is_dark: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = self.theme_palette();
        if interactive && let Some(picker) = self.keyword_color_pickers.get(rule_id, field) {
            let is_background = matches!(
                field,
                KeywordHighlightEditorField::BgDark | KeywordHighlightEditorField::BgLight
            );
            let label: SharedString = if is_background {
                t!("settings.keywordHighlightBackgroundColor").into()
            } else {
                t!("settings.keywordHighlightTextColor").into()
            };
            let placeholder: SharedString = if is_background {
                t!("settings.keywordHighlightNoBackground").into()
            } else {
                "#rrggbb".into()
            };
            // Both text and background rows use the same fixed width so the
            // two selects line up. The background row spends 28px (clear
            // button) + 8px (gap) of the 260px control slot on the button.
            let color_select = ColorSelect::new(picker)
                .w(px(224.))
                .featured_colors(KeywordColorPickerControls::featured_colors(is_dark))
                .placeholder(placeholder)
                .accessibility_label(label);
            if !is_background {
                return color_select.into_any_element();
            }
            // Background rows get a leading clear button so the user can return
            // to "no background" (inherit terminal). `clear_value()` does not
            // emit a change event, so there is no write-back loop.
            let mut control = div().flex().items_center().gap_2();
            if !value.trim().is_empty() {
                let clear_rule_id = rule_id.to_string();
                control = control.child(keyword_highlight_icon_button(
                    palette,
                    format!("settings-keyword-bg-clear-{}", field.input_key()),
                    "icons/close.svg",
                    t!("settings.keywordHighlightClearBackground"),
                    true,
                    cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.with_app(cx, |app, cx| {
                            app.set_keyword_highlight_rule_color_field(
                                clear_rule_id.clone(),
                                field,
                                "",
                                cx,
                            );
                        });
                        if let Some(picker) = this.keyword_color_pickers.get(&clear_rule_id, field)
                        {
                            picker.update(cx, |picker, cx| picker.clear_value(window, cx));
                        }
                    }),
                ));
            }
            return control.child(color_select).into_any_element();
        }
        keyword_highlight_static_input(palette, value, false, true, 96.).into_any_element()
    }

    pub(in crate::features) fn keyword_highlights_settings_section(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let is_dark = self.terminal_theme_is_dark();
        let rules = self.settings.keyword_config().rules.clone();
        let keyword_highlighting_enabled = self.settings.keyword_config().enabled;
        let line_role_highlights = self.settings.summary().ui_highlight_line_roles;
        let expanded = self.settings.keyword_highlight_presentation().expanded_id;
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
                    t!("settings.keywordHighlighting"),
                    Some(t!("settings.keywordHighlightMasterHint").into()),
                    settings_switch(
                        palette,
                        "settings-keyword-highlights-enabled",
                        keyword_highlighting_enabled,
                        cx.listener(|this, _, _, cx| {
                            this.toggle_keyword_highlights(cx);
                        }),
                    ),
                ))
                .child(settings_form_row(
                    palette,
                    t!("settings.lineRoleHighlighting"),
                    Some(SharedString::from(
                        t!("settings.lineRoleHighlightingHint").to_string(),
                    )),
                    div()
                        .debug_selector(|| "settings-line-role-highlights-control".into())
                        .child(settings_switch_with_enabled(
                            palette,
                            "settings-line-role-highlights",
                            line_role_highlights,
                            keyword_highlighting_enabled,
                            cx.listener(|this, _, _, cx| {
                                this.toggle_line_role_highlights(cx);
                            }),
                        )),
                ))
                .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_4()
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
                                    .child(self.keyword_highlight_builtin_group(false, cx))
                                    .child(self.keyword_highlight_builtin_group(true, cx)),
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
                                    let toggle_rule_id = rule.id.clone();
                                    let panel = cx.entity().downgrade();
                                    let input = self.existing_text_input_box_with(
                                        Self::keyword_highlight_text_input_id(
                                            &rule.id,
                                            KeywordHighlightEditorField::Patterns,
                                        ),
                                        true,
                                        |shell| {
                                            shell.bottom_right(keyword_match_option_toggles(
                                                palette,
                                                rule.case_sensitive,
                                                rule.whole_word,
                                                rule.use_regex,
                                                std::rc::Rc::new(
                                                    move |option: KeywordMatchOption,
                                                          _window: &mut Window,
                                                          cx: &mut App| {
                                                        let toggle_rule_id = toggle_rule_id.clone();
                                                        panel
                                                            .update(cx, |panel, cx| {
                                                                panel.toggle_keyword_highlight_rule_match_option(
                                                                    toggle_rule_id,
                                                                    option,
                                                                    cx,
                                                                );
                                                            })
                                                            .ok();
                                                    },
                                                ),
                                            ))
                                        },
                                    );
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
                                let dark_text_control = self.keyword_color_select(
                                    &rule.id,
                                    KeywordHighlightEditorField::ColorDark,
                                    &rule.color_dark,
                                    is_open && keyword_highlighting_enabled,
                                    is_dark,
                                    cx,
                                );
                                let dark_bg_control = self.keyword_color_select(
                                    &rule.id,
                                    KeywordHighlightEditorField::BgDark,
                                    &rule.bg_color_dark,
                                    is_open && keyword_highlighting_enabled,
                                    is_dark,
                                    cx,
                                );
                                let light_text_control = self.keyword_color_select(
                                    &rule.id,
                                    KeywordHighlightEditorField::ColorLight,
                                    &rule.color_light,
                                    is_open && keyword_highlighting_enabled,
                                    is_dark,
                                    cx,
                                );
                                let light_bg_control = self.keyword_color_select(
                                    &rule.id,
                                    KeywordHighlightEditorField::BgLight,
                                    &rule.bg_color_light,
                                    is_open && keyword_highlighting_enabled,
                                    is_dark,
                                    cx,
                                );

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
                                                        .child(
                                                            div()
                                                                .text_size(px(11.))
                                                                .font_weight(FontWeight(600.))
                                                                .text_color(rgb(palette.text_muted))
                                                                .child(t!(
                                                                    "settings.keywordHighlightDarkPalette",
                                                                )),
                                                        )
                                                        .child(settings_form_row(
                                                            palette,
                                                            t!("settings.keywordHighlightTextColor"),
                                                            None,
                                                            dark_text_control,
                                                        ))
                                                        .child(settings_form_row(
                                                            palette,
                                                            t!("settings.keywordHighlightBackgroundColor"),
                                                            None,
                                                            dark_bg_control,
                                                        ))
                                                        .child(
                                                            div()
                                                                .text_size(px(11.))
                                                                .font_weight(FontWeight(600.))
                                                                .text_color(rgb(palette.text_muted))
                                                                .mt_2()
                                                                .child(t!(
                                                                    "settings.keywordHighlightLightPalette",
                                                                )),
                                                        )
                                                        .child(settings_form_row(
                                                            palette,
                                                            t!("settings.keywordHighlightTextColor"),
                                                            None,
                                                            light_text_control,
                                                        ))
                                                        .child(settings_form_row(
                                                            palette,
                                                            t!("settings.keywordHighlightBackgroundColor"),
                                                            None,
                                                            light_bg_control,
                                                        ))
                                                        .child(
                                                            div()
                                                                .flex()
                                                                .flex_col()
                                                                .gap_1()
                                                                .mt_2()
                                                                .child(
                                                                    div()
                                                                        .text_size(px(11.))
                                                                        .text_color(rgb(palette.text_muted))
                                                                        .child(t!("settings.keywordHighlightPreview")),
                                                                )
                                                                .child(
                                                                    div()
                                                                        .h(px(30.))
                                                                        .flex()
                                                                        .items_center()
                                                                        .justify_center()
                                                                        .rounded_md()
                                                                        .border_1()
                                                                        .border_color(rgb(palette.border))
                                                                        .bg(rgb(
                                                                            if is_dark {
                                                                                parse_keyword_swatch(&rule.bg_color_dark)
                                                                            } else {
                                                                                parse_keyword_swatch(&rule.bg_color_light)
                                                                            }
                                                                            .unwrap_or(palette.input),
                                                                        ))
                                                                        .text_color(rgb(
                                                                            parse_keyword_swatch(
                                                                                if is_dark {
                                                                                    &rule.color_dark
                                                                                } else {
                                                                                    &rule.color_light
                                                                                },
                                                                            )
                                                                            .unwrap_or(if is_dark {
                                                                                0x79c0ff
                                                                            } else {
                                                                                0x0969da
                                                                            }),
                                                                        ))
                                                                        .font_family(crate::features::shell::gpui_code_font_family())
                                                                        .child(t!("settings.keywordHighlightPreviewSample")),
                                                                ),
                                                        ),
                                                ),
                                        )
                                    })
                            })),
                ),
        )
    }

    fn keyword_highlight_builtin_group(
        &self,
        specialized: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let enabled = self.settings.keyword_config().enabled
            && (!specialized || self.settings.summary().ui_highlight_line_roles);
        let title = if specialized {
            t!("settings.keywordHighlightSpecializedRules")
        } else {
            t!("settings.keywordHighlightGeneralRules")
        };
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_size(px(12.))
                    .font_weight(FontWeight(600.))
                    .text_color(rgb(palette.text))
                    .child(title),
            )
            .when(specialized, |group| {
                group.child(
                    div()
                        .text_size(px(11.))
                        .text_color(rgb(palette.text_muted))
                        .child(t!("settings.keywordHighlightSpecializedHint")),
                )
            })
            .child(
                div().grid().grid_cols(2).gap_2().children(
                    builtin_keyword_rule_ids()
                        .iter()
                        .copied()
                        .filter(|id| builtin_keyword_rule_is_specialized(id) == specialized)
                        .map(|id| {
                            let swatch = self.settings.builtin_keyword_swatch(id);
                            let checked = self
                                .settings
                                .keyword_config()
                                .builtin_rules
                                .get(id)
                                .copied()
                                .unwrap_or(true);
                            let color = parse_keyword_swatch(&swatch).unwrap_or(0x79c0ff);
                            div()
                                .debug_selector(move || {
                                    format!("settings-keyword-builtin-row-{id}")
                                })
                                .min_w_0()
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
                                        .opacity(if enabled { 1.0 } else { 0.5 })
                                        .child(
                                            div()
                                                .size(px(10.))
                                                .flex_none()
                                                .rounded_full()
                                                .bg(rgb(color))
                                                .border_1()
                                                .border_color(rgb(palette.border)),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(12.))
                                                .text_color(rgb(palette.text_muted))
                                                .overflow_hidden()
                                                .child(builtin_keyword_rule_label(id)),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex_none()
                                        .debug_selector(move || {
                                            format!("settings-keyword-builtin-control-{id}")
                                        })
                                        .child(settings_switch_with_enabled(
                                            palette,
                                            format!("settings-keyword-builtin-{id}"),
                                            checked,
                                            enabled,
                                            cx.listener(move |this, _, _, cx| {
                                                this.toggle_keyword_highlight_builtin(
                                                    id.to_string(),
                                                    cx,
                                                );
                                            }),
                                        )),
                                )
                        }),
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
            |this| this.h(px(NYA_FORM_CONTROL_HEIGHT_PX)).items_center(),
        )
        .px_2()
        .rounded_sm()
        .border_1()
        .border_color(rgb(palette.border))
        .bg(rgb(palette.input_surface()))
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
    _palette: ThemePalette,
    id: impl Into<String>,
    icon_path: &'static str,
    label: impl Into<SharedString>,
    enabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    nyaterm_ui::NyaButton::new(id.into(), label)
        .form_control()
        .icon(icon_path)
        .disabled(!enabled)
        .on_click(on_click)
}

fn keyword_highlight_icon_button(
    palette: ThemePalette,
    id: impl Into<String>,
    icon_path: &'static str,
    tooltip: impl Into<SharedString>,
    enabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    nyaterm_ui::NyaIconButton::new(id.into(), icon_path)
        .form_control()
        .color(rgb(palette.danger))
        .tooltip(tooltip)
        .disabled(!enabled)
        .on_click(on_click)
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
