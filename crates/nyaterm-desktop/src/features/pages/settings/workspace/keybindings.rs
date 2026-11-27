use rust_i18n::t;

use gpui::{Context, FontWeight, IntoElement, KeyDownEvent, div, prelude::*, px, rgb};
use nyaterm_ui::{NyaKbd, NyaSearchInput};

use crate::features::pages::settings::panel::SettingsPanel;
use crate::features::pages::settings::settings_button;
use crate::shortcuts::{
    SHORTCUT_CATEGORIES, SHORTCUT_REGISTRY, ShortcutCategory, ShortcutDefinition,
    ShortcutDiagnosticKind, ShortcutNativeStatus, compact_indexed_hotkey_keystrokes_for_display,
    format_hotkey_for_display, hotkey_keystrokes_for_display, shortcut_keys_for,
};

use super::super::settings_form_section;

impl SettingsPanel {
    pub(in crate::features) fn keybindings_settings_section(
        &mut self,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let overrides = self
            .settings
            .summary()
            .keybindings
            .keys()
            .filter(|id| crate::shortcuts::ShortcutId::parse(id).is_some())
            .count();
        let resolved =
            crate::shortcuts::ResolvedKeymap::resolve(&self.settings.summary().keybindings);
        let search = self.settings.keybinding_presentation().search_draft;
        let Some(search_field) = self.existing_text_input("settings.keybindings.search") else {
            debug_assert!(false, "the keybindings search input was never built");
            return div().into_any_element();
        };
        let mut groups = div().flex().flex_col().gap_3();
        for category in SHORTCUT_CATEGORIES {
            groups = groups.child(self.shortcut_category_group(category, &search, &resolved, cx));
        }

        div()
            .id("settings-keybindings-panel")
            .flex()
            .flex_col()
            .gap_3()
            .track_focus(self.settings.keybinding_focus())
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                this.handle_keybinding_key_down(event, cx);
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div().min_w_0().flex_1().child(
                            NyaSearchInput::new("settings-keybindings-search", &search_field)
                                .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                                    if event.keystroke.key == "escape" {
                                        cx.stop_propagation();
                                        this.clear_keybinding_search(cx);
                                    }
                                })),
                        ),
                    )
                    .when(overrides > 0, |this| {
                        this.child(settings_button(
                            "keybindings-reset-all",
                            t!("settings.keybindingsResetAll"),
                            cx.listener(|this, _, _, cx| {
                                this.reset_all_keybindings(cx);
                            }),
                        ))
                    }),
            )
            .child(groups)
            .into_any_element()
    }

    pub(in crate::features) fn shortcut_category_group(
        &mut self,
        category: ShortcutCategory,
        search: &str,
        resolved: &crate::shortcuts::ResolvedKeymap,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let needle = search.trim().to_ascii_lowercase();
        let shortcuts = SHORTCUT_REGISTRY
            .iter()
            .filter(|shortcut| shortcut.category == category)
            .filter(|shortcut| {
                if needle.is_empty() {
                    return true;
                }
                let keys =
                    shortcut_keys_for(shortcut.id.as_str(), &self.settings.summary().keybindings)
                        .unwrap_or_else(|| shortcut.default_keys());
                let display = format_hotkey_for_display(&keys).to_ascii_lowercase();
                t!(shortcut.label_key)
                    .to_ascii_lowercase()
                    .contains(&needle)
                    || shortcut.id.as_str().to_ascii_lowercase().contains(&needle)
                    || display.contains(&needle)
                    || keys.to_ascii_lowercase().contains(&needle)
            })
            .collect::<Vec<_>>();
        if !needle.is_empty() && shortcuts.is_empty() {
            return div().into_any_element();
        }
        let mut rows = div().flex().flex_col().gap_1();
        for shortcut in shortcuts {
            rows = rows.child(self.shortcut_registry_row(shortcut, resolved, cx));
        }

        settings_form_section(palette, Some(t!(category.label_key())), None, rows)
            .into_any_element()
    }

    pub(in crate::features) fn shortcut_registry_row(
        &mut self,
        shortcut: &'static ShortcutDefinition,
        resolved: &crate::shortcuts::ResolvedKeymap,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let _registry_metadata = (
            shortcut.note,
            match shortcut.native_status {
                ShortcutNativeStatus::Supported => "supported",
                ShortcutNativeStatus::Partial => "partial",
                ShortcutNativeStatus::Contextual => "contextual",
            },
        );
        let is_custom = self
            .settings
            .summary()
            .keybindings
            .contains_key(shortcut.id.as_str());
        let interaction = self.settings.keybinding_presentation();
        let is_recording = interaction.recording_id.as_deref() == Some(shortcut.id.as_str());
        let effective_keys =
            shortcut_keys_for(shortcut.id.as_str(), &self.settings.summary().keybindings)
                .unwrap_or_else(|| shortcut.default_keys());
        let issue = if is_recording {
            interaction
                .pending_keys
                .as_deref()
                .and_then(|keys| self.keybinding_conflict_label(keys, shortcut.id.as_str()))
                .map(|name| t!("settings.keybindingsConflict", name = name).into_owned())
        } else {
            resolved
                .diagnostic(shortcut.id)
                .map(|diagnostic| match diagnostic {
                    ShortcutDiagnosticKind::Invalid(error) => format!("Invalid shortcut: {error}"),
                    ShortcutDiagnosticKind::Conflict(id) => {
                        let name = SHORTCUT_REGISTRY
                            .iter()
                            .find(|item| item.id == *id)
                            .map(|item| t!(item.label_key).into_owned())
                            .unwrap_or_else(|| id.as_str().to_string());
                        t!("settings.keybindingsConflict", name = name).into_owned()
                    }
                })
        };
        let custom_label = t!("settings.keybindingsCustom");
        let recording_label = t!("settings.keybindingsRecording");
        let reset_label = t!("settings.keybindingsReset");
        let indexed_hint = t!("settings.keybindingsIndexedHint");
        let shortcut_label = t!(shortcut.label_key);
        let is_switch_to = shortcut.id == crate::shortcuts::ShortcutId::SwitchToTab;
        let displayed_keys = if is_recording {
            interaction.pending_keys.as_deref()
        } else {
            Some(effective_keys.as_str())
        };
        let key_strokes = displayed_keys
            .and_then(hotkey_keystrokes_for_display)
            .map(|strokes| {
                if is_switch_to && !is_recording {
                    compact_indexed_hotkey_keystrokes_for_display(strokes)
                } else {
                    strokes
                }
            });
        let key_color = if issue.is_some() {
            rgb(palette.danger)
        } else if is_recording {
            rgb(palette.link)
        } else {
            rgb(palette.text_muted)
        };
        let key_border = if issue.is_some() {
            rgb(palette.danger)
        } else if is_recording {
            rgb(0x388bfd)
        } else {
            rgb(palette.border)
        };
        let mut key_display = div()
            .flex()
            .flex_wrap()
            .items_center()
            .justify_end()
            .gap_1();
        if let Some(key_strokes) = key_strokes {
            for (index, stroke) in key_strokes.into_iter().enumerate() {
                if index > 0 {
                    key_display = key_display.child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(palette.text_muted))
                            .child("/"),
                    );
                }
                key_display = key_display.child(
                    NyaKbd::new(stroke)
                        .outline()
                        .border_color(key_border)
                        .text_color(key_color),
                );
            }
        } else if let Some(keys) = displayed_keys {
            key_display = key_display.child(
                div()
                    .text_size(px(10.))
                    .font_weight(FontWeight(600.))
                    .text_color(key_color)
                    .child(format_hotkey_for_display(keys)),
            );
        } else {
            key_display = key_display.child(
                div()
                    .text_size(px(10.))
                    .font_weight(FontWeight(600.))
                    .text_color(key_color)
                    .child(recording_label),
            );
        }
        let shortcut_id = shortcut.id.as_str().to_string();
        let reset_shortcut_id = shortcut.id.as_str().to_string();

        div()
            .rounded_md()
            .px_2()
            .py_1()
            .border_1()
            .border_color(if is_recording {
                rgb(0x1f6feb)
            } else {
                rgb(palette.surface_elevated)
            })
            .bg(if is_recording {
                rgb(palette.hover)
            } else {
                rgb(palette.bg)
            })
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .child(
                div().min_w_0().flex_1().flex().flex_col().child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .text_size(px(12.))
                                .font_weight(FontWeight(600.))
                                .text_color(rgb(palette.text))
                                .overflow_hidden()
                                .child(shortcut_label),
                        )
                        .when(is_custom, |this| {
                            this.child(
                                div()
                                    .text_size(px(10.))
                                    .font_weight(FontWeight(600.))
                                    .text_color(rgb(0xbc8cff))
                                    .child(custom_label),
                            )
                        }),
                ),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(key_display)
                    .when_some(issue.clone(), |this, message| {
                        this.child(
                            div()
                                .text_size(px(10.))
                                .text_color(rgb(palette.danger))
                                .child(message),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .when(is_recording, |this| {
                        this.child(settings_button(
                            format!("keybinding-save-{}", shortcut.id.as_str()),
                            t!("common.confirm"),
                            cx.listener(|this, _, _, cx| {
                                this.confirm_keybinding_recording(cx);
                            }),
                        ))
                        .child(settings_button(
                            format!("keybinding-cancel-{}", shortcut.id.as_str()),
                            t!("common.cancel"),
                            cx.listener(|this, _, _, cx| {
                                this.cancel_keybinding_recording(cx);
                            }),
                        ))
                    })
                    .when(!is_recording, |this| {
                        this.child(settings_button(
                            format!("keybinding-record-{}", shortcut.id.as_str()),
                            t!("common.edit"),
                            cx.listener(move |this, _, window, cx| {
                                this.start_keybinding_recording(shortcut_id.clone(), window, cx);
                            }),
                        ))
                    })
                    .when(is_custom && !is_recording, |this| {
                        this.child(settings_button(
                            format!("keybinding-reset-{}", shortcut.id.as_str()),
                            reset_label,
                            cx.listener(move |this, _, _, cx| {
                                this.reset_keybinding(reset_shortcut_id.clone(), cx);
                            }),
                        ))
                    }),
            )
            .when(is_recording && is_switch_to, |this| {
                this.child(
                    div()
                        .w_full()
                        .text_size(px(10.))
                        .text_color(rgb(palette.text_muted))
                        .child(indexed_hint),
                )
            })
    }
}
