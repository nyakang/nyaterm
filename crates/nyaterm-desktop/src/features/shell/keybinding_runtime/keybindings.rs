use std::collections::HashMap;

use gpui::{Context, KeyDownEvent, Window};

use crate::features::NyaTermApp;
use crate::shortcuts::event_to_hotkey_string;

impl NyaTermApp {
    /// First display chord for empty-workspace / UI labels (Tauri-style chips).
    pub(in crate::features) fn display_shortcut_for(&self, id: &str, fallback: &str) -> String {
        use crate::shortcuts::{format_hotkey_for_display, shortcut_keys_for};
        let raw = shortcut_keys_for(id, &self.settings.summary().keybindings)
            .unwrap_or_else(|| fallback.to_string());
        let display = format_hotkey_for_display(&raw);
        display
            .split(" / ")
            .next()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(fallback)
            .to_string()
    }
    pub(in crate::features) fn start_keybinding_recording(
        &mut self,
        shortcut_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(shortcut_id) = crate::shortcuts::ShortcutId::parse(&shortcut_id) else {
            self.shell.set_status("unknown shortcut".to_string());
            cx.notify();
            return;
        };
        self.settings.begin_keybinding_recording(shortcut_id);
        self.shell.set_status("recording shortcut".to_string());
        window.focus(self.settings.keybinding_focus(), cx);
        cx.notify();
    }

    pub(in crate::features) fn cancel_keybinding_recording(&mut self, cx: &mut Context<Self>) {
        self.settings.cancel_keybinding_recording();
        self.shell
            .set_status("shortcut recording cancelled".to_string());
        cx.notify();
    }

    pub(in crate::features) fn confirm_keybinding_recording(&mut self, cx: &mut Context<Self>) {
        let Some(shortcut_id) = self.settings.keybinding_recording_id() else {
            self.shell
                .set_status("no shortcut recording is active".to_string());
            cx.notify();
            return;
        };
        let Some(binding) = self.settings.pending_keybinding().cloned() else {
            self.shell
                .set_status("press a shortcut before saving".to_string());
            cx.notify();
            return;
        };

        self.settings.finish_keybinding_recording();
        let keys = binding.canonical();
        if let Some(conflict) = self.keybinding_conflict_label(&keys, shortcut_id.as_str()) {
            self.settings.begin_keybinding_recording(shortcut_id);
            self.settings.set_pending_keybinding(Some(binding));
            let message =
                rust_i18n::t!("settings.shortcutConflictMessage", shortcut = conflict).to_string();
            self.shell.set_status(message.clone());
            self.notify_operation(
                "shortcut-conflict",
                nyaterm_ui::notification::NyaNotificationKind::Warning,
                message,
                cx,
            );
            cx.notify();
            return;
        }
        let mut keybindings = self.settings.summary().keybindings.clone();
        if crate::shortcuts::is_default_binding(shortcut_id, &binding) {
            keybindings.remove(shortcut_id.as_str());
        } else {
            keybindings.insert(shortcut_id.as_str().to_string(), keys);
        }
        self.save_keybindings(
            keybindings,
            format!("shortcut {} saved", shortcut_id.as_str()),
            cx,
        );
    }

    pub(in crate::features) fn reset_keybinding(
        &mut self,
        shortcut_id: String,
        cx: &mut Context<Self>,
    ) {
        let mut keybindings = self.settings.summary().keybindings.clone();
        keybindings.remove(&shortcut_id);
        self.save_keybindings(keybindings, format!("shortcut {shortcut_id} reset"), cx);
    }

    pub(in crate::features) fn reset_all_keybindings(&mut self, cx: &mut Context<Self>) {
        let mut keybindings = self.settings.summary().keybindings.clone();
        crate::shortcuts::reset_known_overrides(&mut keybindings);
        self.save_keybindings(keybindings, "all shortcuts reset".to_string(), cx);
    }

    fn save_keybindings(
        &mut self,
        keybindings: HashMap<String, String>,
        success_message: String,
        cx: &mut Context<Self>,
    ) {
        crate::shortcuts::rebuild_keymap(&keybindings, cx);
        self.settings.set_keybindings(keybindings.clone());
        if self.defer_settings_persistence(cx) {
            self.settings.finish_keybinding_recording();
            self.shell
                .set_status(success_message.replace("saved", "staged"));
            return;
        }
        self.settings.finish_keybinding_recording();
        self.shell.set_status(success_message);
        self.queue_settings_save(crate::features::settings::SettingsSaveKind::Keybindings, cx);
        cx.notify();
    }

    pub(in crate::features) fn handle_keybinding_key_down(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) {
        cx.stop_propagation();
        let Some(recording_id) = self.settings.keybinding_recording_id() else {
            return;
        };
        match event.keystroke.key.as_str() {
            "escape" => {
                self.cancel_keybinding_recording(cx);
                return;
            }
            "enter" if self.settings.pending_keybinding().is_some() => {
                self.confirm_keybinding_recording(cx);
                return;
            }
            _ => {}
        }

        let Some(keys) = event_to_hotkey_string(event) else {
            return;
        };
        let Ok(binding) = crate::shortcuts::ShortcutBinding::parse(&keys) else {
            return;
        };
        if recording_id == crate::shortcuts::ShortcutId::SwitchToTab
            && !crate::shortcuts::is_indexed_shortcut_template(&keys)
        {
            self.settings.set_pending_keybinding(None);
            self.shell
                .set_status("tab switch shortcut must end with number 1".to_string());
            cx.notify();
            return;
        }
        self.settings.set_pending_keybinding(Some(binding));
        self.shell
            .set_status("shortcut captured; press Enter or Save".to_string());
        cx.notify();
    }

    pub(in crate::features) fn keybinding_conflict_label(
        &self,
        pending_keys: &str,
        exclude_id: &str,
    ) -> Option<String> {
        let pending = crate::shortcuts::ShortcutBinding::parse(pending_keys).ok()?;
        let exclude = crate::shortcuts::ShortcutId::parse(exclude_id)?;
        let conflict = crate::shortcuts::conflicting_shortcut(
            &pending,
            exclude,
            &self.settings.summary().keybindings,
        )?;
        let definition = crate::shortcuts::SHORTCUT_REGISTRY
            .iter()
            .find(|item| item.id == conflict)?;
        Some(rust_i18n::t!(definition.label_key).into_owned())
    }

    pub(in crate::features) fn apply_keybinding_search(
        &mut self,
        text: String,
        cx: &mut Context<Self>,
    ) {
        self.settings.set_keybinding_search(text);
        cx.notify();
    }
}
