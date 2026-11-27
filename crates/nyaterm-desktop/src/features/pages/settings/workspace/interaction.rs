use rust_i18n::t;

use std::borrow::Cow;

use gpui::{Context, FontWeight, IntoElement, SharedString, div, prelude::*, px, rgb};
use nyaterm_ui::NyaSelectOption;

use crate::features::{pages::settings::panel::SettingsPanel, shell::TAB_MOUSE_ACTIONS};
use crate::theme::ThemePalette;

use super::super::{settings_form_row, settings_form_section, settings_switch};

impl SettingsPanel {
    pub(in crate::features) fn interaction_settings_section(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        // Built before the form, which reads `self` throughout: creating the
        // box needs it mutably.
        let word_separators_input =
            self.existing_text_input_box("settings.interaction.word-separators", false);
        let double_action = self
            .settings
            .summary()
            .interaction_tab_double_click_action
            .clone();
        let middle_action = self
            .settings
            .summary()
            .interaction_tab_middle_click_action
            .clone();
        let right_action = self
            .settings
            .summary()
            .interaction_tab_right_click_action
            .clone();
        let _delay_ms = self
            .settings
            .summary()
            .interaction_duplicate_session_command_delay_ms;
        let _min_chars = self
            .settings
            .summary()
            .interaction_command_suggestion_min_chars;
        let _max_chars = self
            .settings
            .summary()
            .interaction_command_suggestion_max_chars;
        let suggestions_enabled = self
            .settings
            .summary()
            .interaction_command_suggestions_enabled;

        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(settings_form_section(
                palette,
                Some(t!("settings.interactionClipboardMouse")),
                None,
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(settings_form_row(
                        palette,
                        t!("settings.copyOnSelect"),
                        None,
                        settings_switch(
                            palette,
                            "interaction-copy-select",
                            self.settings.summary().interaction_copy_on_select,
                            cx.listener(|this, _, _, cx| {
                                this.toggle_interaction_copy_on_select(cx);
                            }),
                        ),
                    ))
                    .child(settings_form_row(
                        palette,
                        t!("settings.freeTypeMode"),
                        Some(SharedString::from(t!("settings.freeTypeModeDesc"))),
                        settings_switch(
                            palette,
                            "interaction-free-type",
                            self.settings.summary().interaction_free_type_enabled,
                            cx.listener(|this, _, _, cx| {
                                this.toggle_interaction_free_type(cx);
                            }),
                        ),
                    ))
                    .child(settings_form_row(
                        palette,
                        t!("settings.allowOsc52ClipboardWrite"),
                        Some(SharedString::from(t!(
                            "settings.allowOsc52ClipboardWriteDesc"
                        ))),
                        settings_switch(
                            palette,
                            "interaction-osc52-clipboard",
                            self.settings
                                .summary()
                                .interaction_allow_osc52_clipboard_write,
                            cx.listener(|this, _, _, cx| {
                                this.toggle_osc52_clipboard_write(cx);
                            }),
                        ),
                    ))
                    .child(settings_form_row(
                        palette,
                        t!("settings.terminalRightClickAction"),
                        None,
                        self.settings_select_control(
                            "settings.interaction.terminal-right-click-action",
                            vec![
                                NyaSelectOption::new("none", t!("settings.terminalRightClickNone")),
                                NyaSelectOption::new("menu", t!("settings.terminalRightClickMenu")),
                                NyaSelectOption::new(
                                    "paste",
                                    t!("settings.terminalRightClickPaste"),
                                ),
                            ],
                            self.settings
                                .summary()
                                .interaction_terminal_right_click_action
                                .compat_value(),
                            false,
                            cx,
                        ),
                    ))
                    .child(settings_form_row(
                        palette,
                        t!("settings.mouseEventsRequireAlt"),
                        Some(SharedString::from(t!("settings.mouseEventsRequireAltDesc"))),
                        settings_switch(
                            palette,
                            "interaction-mouse-requires-alt",
                            self.settings.summary().interaction_mouse_events_require_alt,
                            cx.listener(|this, _, _, cx| this.toggle_mouse_events_require_alt(cx)),
                        ),
                    ))
                    .child(settings_form_row(
                        palette,
                        t!("settings.disableAlternateScreen"),
                        Some(SharedString::from(t!(
                            "settings.disableAlternateScreenDesc"
                        ))),
                        settings_switch(
                            palette,
                            "interaction-disable-alternate-screen",
                            self.settings.summary().interaction_disable_alternate_screen,
                            cx.listener(|this, _, _, cx| this.toggle_disable_alternate_screen(cx)),
                        ),
                    ))
                    .child(settings_form_row(
                        palette,
                        t!("settings.alternateScroll"),
                        Some(SharedString::from(t!("settings.alternateScrollDesc"))),
                        settings_switch(
                            palette,
                            "interaction-alternate-scroll",
                            self.settings.summary().interaction_alternate_scroll,
                            cx.listener(|this, _, _, cx| {
                                this.toggle_interaction_alternate_scroll(cx)
                            }),
                        ),
                    )),
            ))
            .child(settings_form_section(
                palette,
                Some(t!("settings.interactionTerminalPage")),
                None,
                settings_form_row(
                    palette,
                    t!("settings.terminalZoomEnabled"),
                    Some(SharedString::from(t!("settings.terminalZoomEnabledDesc"))),
                    settings_switch(
                        palette,
                        "interaction-terminal-zoom",
                        self.settings.summary().interaction_terminal_zoom_enabled,
                        cx.listener(|this, _, _, cx| {
                            this.toggle_terminal_zoom_enabled(cx);
                        }),
                    ),
                ),
            ))
            .child(settings_form_section(
                palette,
                Some(t!("settings.interactionCommandInput")),
                None,
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(settings_form_row(
                        palette,
                        t!("settings.commandSuggestions"),
                        Some(SharedString::from(t!("settings.commandSuggestionsDesc"))),
                        settings_switch(
                            palette,
                            "interaction-cmd-suggestions",
                            suggestions_enabled,
                            cx.listener(|this, _, _, cx| {
                                this.toggle_command_suggestions(cx);
                            }),
                        ),
                    ))
                    .child(settings_form_row(
                        palette,
                        t!("settings.credentialAutofill"),
                        Some(SharedString::from(t!("settings.credentialAutofillDesc"))),
                        settings_switch(
                            palette,
                            "interaction-credential-autofill",
                            self.settings
                                .summary()
                                .interaction_credential_autofill_enabled,
                            cx.listener(|this, _, _, cx| {
                                this.toggle_credential_autofill(cx);
                            }),
                        ),
                    ))
                    .when(suggestions_enabled, |this| {
                        this.child(settings_form_row(
                            palette,
                            t!("settings.commandSuggestionsMinChars"),
                            None,
                            self.existing_number_input_box(
                                "settings.number.command-suggestion-min-chars",
                            ),
                        ))
                        .child(settings_form_row(
                            palette,
                            t!("settings.commandSuggestionsMaxChars"),
                            None,
                            self.existing_number_input_box(
                                "settings.number.command-suggestion-max-chars",
                            ),
                        ))
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(rgb(palette.text_dimmed))
                                .child(t!("settings.commandSuggestionsLengthDesc")),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(settings_field_meta(
                                palette,
                                t!("settings.wordSeparators"),
                                t!("settings.wordSeparatorsDesc"),
                            ))
                            .child(div().w_full().max_w(px(640.)).child(word_separators_input)),
                    )
                    .child(settings_form_row(
                        palette,
                        t!("settings.duplicateSessionCommandDelay"),
                        Some(SharedString::from(t!(
                            "settings.duplicateSessionCommandDelayDesc"
                        ))),
                        self.existing_number_input_box(
                            "settings.number.duplicate-session-command-delay",
                        ),
                    ))
                    .child(settings_form_row(
                        palette,
                        t!("settings.altAsMeta"),
                        Some(SharedString::from(t!("settings.altAsMetaDesc"))),
                        settings_switch(
                            palette,
                            "interaction-alt-meta",
                            self.settings.summary().interaction_alt_as_meta,
                            cx.listener(|this, _, _, cx| {
                                this.toggle_alt_as_meta(cx);
                            }),
                        ),
                    )),
            ))
            .child(settings_form_section(
                palette,
                Some(t!("settings.tabMouseActions")),
                None,
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(self.tab_mouse_action_settings_field(
                        TabMouseActionPresentation {
                            label: t!("settings.tabDoubleClickAction"),
                            description: None,
                            id: "settings.interaction.tab-double",
                            current: &double_action,
                        },
                        cx,
                    ))
                    .child(self.tab_mouse_action_settings_field(
                        TabMouseActionPresentation {
                            label: t!("settings.tabMiddleClickAction"),
                            description: None,
                            id: "settings.interaction.tab-middle",
                            current: &middle_action,
                        },
                        cx,
                    ))
                    .child(self.tab_mouse_action_settings_field(
                        TabMouseActionPresentation {
                            label: t!("settings.tabRightClickAction"),
                            description: Some(t!("settings.tabRightClickActionDesc")),
                            id: "settings.interaction.tab-right",
                            current: &right_action,
                        },
                        cx,
                    )),
            ))
    }

    fn tab_mouse_action_settings_field(
        &mut self,
        presentation: TabMouseActionPresentation<'_>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let TabMouseActionPresentation {
            label,
            description,
            id,
            current,
        } = presentation;
        let selected = normalized_tab_mouse_action(current).to_string();
        let options = TAB_MOUSE_ACTIONS
            .iter()
            .map(|action| NyaSelectOption::new(*action, t!(tab_mouse_action_i18n_key(action))))
            .collect();

        self.settings_select_field(
            id,
            label,
            description.map(SharedString::from),
            (options, selected, false),
            cx,
        )
    }
}

struct TabMouseActionPresentation<'a> {
    label: Cow<'static, str>,
    description: Option<Cow<'static, str>>,
    id: &'static str,
    current: &'a str,
}

fn settings_field_meta(
    palette: ThemePalette,
    label: impl Into<SharedString>,
    desc: impl Into<SharedString>,
) -> impl IntoElement {
    let label: SharedString = label.into();
    let desc: SharedString = desc.into();
    div()
        .min_w_0()
        .child(
            div()
                .text_size(px(13.))
                .font_weight(FontWeight(500.))
                .text_color(rgb(palette.text))
                .child(label),
        )
        .child(
            div()
                .mt_1()
                .text_size(px(11.))
                .text_color(rgb(palette.text_dimmed))
                .child(desc),
        )
}

fn normalized_tab_mouse_action(action: &str) -> &'static str {
    TAB_MOUSE_ACTIONS
        .iter()
        .copied()
        .find(|candidate| *candidate == action)
        .unwrap_or("none")
}

fn tab_mouse_action_i18n_key(action: &str) -> &'static str {
    match action {
        "rename_tab" => "tabCtx.rename",
        "copy_tab_name" => "tabCtx.copyName",
        "copy_server_ip" => "tabCtx.copyIp",
        "duplicate_session" => "tabCtx.duplicate",
        "multiplex_ssh" => "tabCtx.multiplexSsh",
        "reconnect_session" => "tabCtx.reconnect",
        "disconnect_session" => "tabCtx.disconnect",
        "close_tab" => "tabCtx.close",
        _ => "settings.tabMouseActionNone",
    }
}
