use std::collections::HashMap;

use gpui::{Context, KeyDownEvent, Window};

use crate::features::NyaTermApp;
use crate::shortcuts::keystroke_to_hotkey_string;

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
        if self.defer_settings_domain_persistence(
            crate::features::settings::SettingsPersistenceDomain::Keybindings,
            cx,
        ) {
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
        self.record_keybinding_keystroke(&event.keystroke, cx);
    }

    /// Feed one keystroke to the active shortcut recording. Repeating the same
    /// keystroke is harmless, so the keystroke interceptor and the panel's own
    /// key listener may both deliver it.
    pub(in crate::features) fn record_keybinding_keystroke(
        &mut self,
        keystroke: &gpui::Keystroke,
        cx: &mut Context<Self>,
    ) {
        let Some(recording_id) = self.settings.keybinding_recording_id() else {
            return;
        };
        match keystroke.key.as_str() {
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

        let Some(keys) = keystroke_to_hotkey_string(keystroke) else {
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

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui::{
        Context, Entity, IntoElement, KeyBinding, Render, TestAppContext, Window, actions, div,
        prelude::*,
    };
    use nyaterm_core::test_support::TestTempDir;

    use crate::features::NyaTermApp;
    use crate::features::test_support::app_with_visible_local_session;

    actions!(keybinding_recording_tests, [BoundToCtrlW]);

    struct RecorderHost {
        app: Entity<NyaTermApp>,
        fired: Rc<Cell<usize>>,
    }

    impl Render for RecorderHost {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let fired = self.fired.clone();
            let app = self.app.clone();
            div()
                .size_full()
                .track_focus(self.app.read(cx).settings.keybinding_focus())
                .on_action(move |_: &BoundToCtrlW, _, _| fired.set(fired.get() + 1))
                .on_key_down(move |event, _, cx| {
                    app.update(cx, |app, cx| app.handle_keybinding_key_down(event, cx));
                })
        }
    }

    /// GPUI runs keymap actions before key listeners, so recording a shortcut that
    /// is already bound (e.g. Ctrl+W closing the settings window) used to fire the
    /// bound action instead of being captured.
    #[test]
    fn recording_captures_a_shortcut_that_is_already_bound() {
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let root = TestTempDir::new("nyaterm-keybinding-recording");
        let app = app_with_visible_local_session(&mut cx, root.path(), "local");
        cx.update(|cx| cx.bind_keys([KeyBinding::new("ctrl-w", BoundToCtrlW, None)]));
        let fired = Rc::new(Cell::new(0usize));
        let host_fired = fired.clone();
        let host_app = app.clone();
        let (_, cx) = cx.add_window_view(move |window, cx| {
            let host = cx.new(|_| RecorderHost {
                app: host_app,
                fired: host_fired,
            });
            nyaterm_ui::nya_root(host, window, cx)
        });
        let handle = cx.update(|window, _| window.window_handle().downcast().unwrap());
        app.update(cx, |app, cx| {
            app.shell.remember_main_window(handle);
            app.ensure_shortcut_interceptor(cx);
        });
        cx.update(|window, cx| {
            let _ = window.draw(cx);
            app.update(cx, |app, cx| {
                app.start_keybinding_recording("tab.close".to_string(), window, cx);
            });
        });
        cx.run_until_parked();

        cx.simulate_keystrokes("ctrl-w");
        cx.run_until_parked();

        assert_eq!(
            fired.get(),
            0,
            "the bound action must not run while recording"
        );
        app.read_with(cx, |app, _| {
            assert!(app.settings.keybinding_recording_id().is_some());
            assert_eq!(
                app.settings
                    .pending_keybinding()
                    .map(|binding| binding.canonical()),
                Some("ctrl+w".to_string())
            );
        });
    }
}
