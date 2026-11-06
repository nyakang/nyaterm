use gpui::Context;
use nyaterm_core::AppSettingsSummary;
use nyaterm_store::{StoreDomain, store_request};

use crate::features::NyaTermApp;
use crate::features::settings::SettingsPersistenceDomain;

#[derive(Clone, Copy)]
pub(in crate::features) enum SettingsSaveKind {
    Diagnostics,
    General,
    Interaction,
    ScreenLock,
    HostKey,
    Recording,
    Transfer,
    Terminal,
    QuickCommands,
    Appearance,
    UiLayout,
    Keybindings,
    FileExplorer,
}

impl SettingsSaveKind {
    fn domain(self) -> SettingsPersistenceDomain {
        match self {
            Self::Diagnostics => SettingsPersistenceDomain::Diagnostics,
            Self::General => SettingsPersistenceDomain::General,
            Self::Interaction => SettingsPersistenceDomain::Interaction,
            Self::ScreenLock => SettingsPersistenceDomain::ScreenLock,
            Self::HostKey => SettingsPersistenceDomain::HostKey,
            Self::Recording => SettingsPersistenceDomain::Recording,
            Self::Transfer => SettingsPersistenceDomain::Transfer,
            Self::Terminal => SettingsPersistenceDomain::Terminal,
            Self::QuickCommands => SettingsPersistenceDomain::QuickCommands,
            Self::Appearance => SettingsPersistenceDomain::Appearance,
            Self::UiLayout => SettingsPersistenceDomain::UiLayout,
            Self::Keybindings => SettingsPersistenceDomain::Keybindings,
            Self::FileExplorer => SettingsPersistenceDomain::FileExplorer,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Diagnostics => "diagnostics settings",
            Self::General => "general settings",
            Self::Interaction => "interaction settings",
            Self::ScreenLock => "screen lock settings",
            Self::HostKey => "host key policy",
            Self::Recording => "recording settings",
            Self::Transfer => "transfer settings",
            Self::Terminal => "terminal settings",
            Self::QuickCommands => "quick command UI settings",
            Self::Appearance => "appearance settings",
            Self::UiLayout => "UI layout settings",
            Self::Keybindings => "keybindings",
            Self::FileExplorer => "file explorer settings",
        }
    }
}

impl NyaTermApp {
    pub(crate) fn publish_shared_settings(
        &self,
        settings: AppSettingsSummary,
        cx: &mut Context<Self>,
    ) {
        if let Some(controller) = self.desktop_controller.clone() {
            let source = self.workspace_id;
            cx.defer(move |cx| {
                let _ = controller.update(cx, |controller, cx| {
                    controller.publish_settings(source, settings, cx)
                });
            });
        }
    }

    pub(in crate::features) fn update_ui_language(
        &mut self,
        language: &str,
        cx: &mut Context<Self>,
    ) {
        self.settings.set_language(language);
        self.save_general_settings(cx);
    }

    pub(in crate::features) fn toggle_startup_restore(&mut self, cx: &mut Context<Self>) {
        self.settings.toggle_startup_restore();
        self.save_general_settings(cx);
    }

    pub(in crate::features) fn toggle_startup_restore_window_layout(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        let restore_window_layout = self.settings.toggle_startup_restore_window_layout();
        self.save_general_settings(cx);
        if !restore_window_layout && !self.shell.has_settings_draft() {
            // Clear stored layouts when the user disables restore.
            self.submit_store_request(
                0,
                store_request(StoreDomain::Sessions, |store| {
                    store.save_terminal_window_layout(None)?;
                    store.save_workspace_pane_layout(None)
                }),
                |this, event, cx| {
                    if let Err(error) = event.outcome {
                        let message = format!("failed to clear saved layouts: {error}");
                        this.settings.update_store_status(message.clone(), false);
                        this.shell.set_status(message);
                    }
                    cx.notify();
                },
                cx,
            );
        }
    }

    pub(in crate::features) fn toggle_confirm_on_close(&mut self, cx: &mut Context<Self>) {
        self.settings.toggle_confirm_on_close();
        self.save_general_settings(cx);
    }

    pub(in crate::features) fn toggle_minimize_to_tray(&mut self, cx: &mut Context<Self>) {
        self.settings.toggle_minimize_to_tray();
        self.save_general_settings(cx);
    }

    pub(in crate::features) fn toggle_minimize_to_tray_from_tray(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        // A tray action changes the saved preference even while a settings draft
        // is open. Load the saved summary on the store worker so unrelated draft
        // fields are never persisted by this shortcut.
        self.submit_store_request(
            0,
            nyaterm_store::store_mutation(StoreDomain::Settings, |store| {
                let mut settings = store.load_app_settings_summary()?;
                settings.minimize_to_tray = !settings.minimize_to_tray;
                store.save_general_settings(&settings)
            }),
            |this, event, cx| {
                match event.outcome {
                    Ok(settings) => {
                        if !this.shell.has_settings_draft() {
                            this.apply_gpui_settings(settings.clone(), cx);
                        }
                        this.publish_shared_settings(settings, cx);
                    }
                    Err(error) => {
                        this.settings.update_store_status(
                            format!("tray preference save failed: {error}"),
                            false,
                        );
                        this.shell.set_status("tray preference save failed");
                    }
                }
                cx.notify();
            },
            cx,
        );
    }

    pub(in crate::features) fn set_diagnostics_level(
        &mut self,
        level: &'static str,
        cx: &mut Context<Self>,
    ) {
        if !self.settings.set_diagnostics_level(level) {
            return;
        }
        self.save_diagnostics_settings(cx);
    }

    pub(in crate::features) fn set_diagnostics_retention_days(
        &mut self,
        days: u32,
        cx: &mut Context<Self>,
    ) {
        if !self.settings.set_diagnostics_retention_days(days) {
            return;
        }
        self.save_diagnostics_settings(cx);
    }

    pub(in crate::features) fn set_diagnostics_log_shard_bytes(
        &mut self,
        bytes: u64,
        cx: &mut Context<Self>,
    ) {
        if !self.settings.set_diagnostics_log_shard_bytes(bytes) {
            return;
        }
        // The log layer reads this atomically, so new shards honour the new
        // budget without restarting the process.
        nyaterm_core::diagnostics::set_log_shard_bytes(
            self.settings.summary().diagnostics_log_shard_bytes,
        );
        self.save_diagnostics_settings(cx);
    }

    pub(in crate::features) fn save_diagnostics_settings(&mut self, cx: &mut Context<Self>) {
        if self.defer_settings_domain_persistence(SettingsPersistenceDomain::Diagnostics, cx) {
            return;
        }
        self.queue_settings_save(SettingsSaveKind::Diagnostics, cx);
    }

    pub(in crate::features) fn save_general_settings(&mut self, cx: &mut Context<Self>) {
        if self.defer_settings_domain_persistence(SettingsPersistenceDomain::General, cx) {
            return;
        }
        self.queue_settings_save(SettingsSaveKind::General, cx);
    }

    pub(in crate::features) fn toggle_interaction_copy_on_select(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        self.settings.toggle_interaction_copy_on_select();
        self.save_interaction_settings(cx);
    }

    pub(in crate::features) fn toggle_osc52_clipboard_write(&mut self, cx: &mut Context<Self>) {
        self.settings.toggle_osc52_clipboard_write();
        self.save_interaction_settings(cx);
    }

    pub(in crate::features) fn set_terminal_right_click_action(
        &mut self,
        action: nyaterm_core::TerminalRightClickAction,
        cx: &mut Context<Self>,
    ) {
        self.settings.set_terminal_right_click_action(action);
        self.save_interaction_settings(cx);
    }

    pub(in crate::features) fn toggle_mouse_events_require_alt(&mut self, cx: &mut Context<Self>) {
        self.settings.toggle_mouse_events_require_alt();
        self.save_interaction_settings(cx);
    }

    pub(in crate::features) fn toggle_terminal_zoom_enabled(&mut self, cx: &mut Context<Self>) {
        self.settings.toggle_terminal_zoom_enabled();
        self.save_interaction_settings(cx);
    }

    pub(in crate::features) fn toggle_command_suggestions(&mut self, cx: &mut Context<Self>) {
        let suggestions_enabled = self.settings.toggle_command_suggestions();
        if !suggestions_enabled && !self.shell.has_settings_draft() {
            self.terminal.clear_command_tracking();
        }
        self.save_interaction_settings(cx);
    }

    pub(in crate::features) fn toggle_credential_autofill(&mut self, cx: &mut Context<Self>) {
        let autofill_enabled = self.settings.toggle_credential_autofill();
        if !autofill_enabled {
            self.cancel_credential_autofill(cx);
        }
        self.save_interaction_settings(cx);
    }

    pub(in crate::features) fn set_command_suggestion_min_chars(
        &mut self,
        value: u32,
        cx: &mut Context<Self>,
    ) {
        self.settings.set_command_suggestion_min_chars(value);
        self.save_interaction_settings(cx);
    }

    pub(in crate::features) fn set_command_suggestion_max_chars(
        &mut self,
        value: u32,
        cx: &mut Context<Self>,
    ) {
        self.settings.set_command_suggestion_max_chars(value);
        self.save_interaction_settings(cx);
    }

    pub(in crate::features) fn set_duplicate_session_command_delay(
        &mut self,
        value_ms: u32,
        cx: &mut Context<Self>,
    ) {
        self.settings.set_duplicate_session_command_delay(value_ms);
        self.save_interaction_settings(cx);
    }

    pub(in crate::features) fn toggle_alt_as_meta(&mut self, cx: &mut Context<Self>) {
        self.settings.toggle_alt_as_meta();
        self.save_interaction_settings(cx);
    }

    pub(in crate::features) fn set_interaction_encoding(
        &mut self,
        encoding: &'static str,
        cx: &mut Context<Self>,
    ) {
        self.settings.set_interaction_encoding(encoding);
        self.sync_terminal_encodings_from_settings();
        self.save_interaction_settings(cx);
    }

    /// Apply an edit from the word separators box.
    pub(in crate::features) fn apply_interaction_word_separators(
        &mut self,
        text: String,
        cx: &mut Context<Self>,
    ) {
        self.settings.set_interaction_word_separators(text);
        // Without this, a draft never marks the Interaction domain dirty: applying it
        // writes nothing and cancelling it silently restores the old separators.
        self.save_interaction_settings(cx);
    }

    pub(in crate::features) fn save_interaction_settings(&mut self, cx: &mut Context<Self>) {
        if self.defer_settings_domain_persistence(SettingsPersistenceDomain::Interaction, cx) {
            return;
        }
        self.queue_settings_save(SettingsSaveKind::Interaction, cx);
    }

    pub(in crate::features) fn toggle_startup_lock_enabled(&mut self, cx: &mut Context<Self>) {
        self.settings.toggle_startup_lock_enabled();
        self.save_screen_lock_settings(cx);
    }

    pub(in crate::features) fn toggle_idle_lock_enabled(&mut self, cx: &mut Context<Self>) {
        self.settings.toggle_idle_lock_enabled();
        self.ensure_idle_lock_clock(cx);
        self.security.reset_screen_lock_idle_timer();
        self.save_screen_lock_settings(cx);
    }

    pub(in crate::features) fn set_idle_lock_minutes(
        &mut self,
        value: u32,
        cx: &mut Context<Self>,
    ) {
        self.settings.set_idle_lock_minutes(value);
        self.security.reset_screen_lock_idle_timer();
        self.ensure_idle_lock_clock(cx);
        self.save_screen_lock_settings(cx);
    }

    pub(in crate::features) fn save_screen_lock_settings(&mut self, cx: &mut Context<Self>) {
        if self.defer_settings_domain_persistence(SettingsPersistenceDomain::ScreenLock, cx) {
            return;
        }
        self.queue_settings_save(SettingsSaveKind::ScreenLock, cx);
    }

    pub(in crate::features) fn queue_settings_save(
        &mut self,
        kind: SettingsSaveKind,
        cx: &mut Context<Self>,
    ) {
        if self.shell.has_settings_draft() {
            self.settings.mark_draft_domain_dirty(kind.domain());
            self.settings
                .update_store_status(format!("{} changes staged", kind.label()), true);
            self.request_settings_panel_refresh(cx);
            cx.notify();
            return;
        }
        let Some((generation, snapshot)) = self.settings.queue_persistence(kind.domain()) else {
            self.settings
                .update_store_status(format!("{} changes queued", kind.label()), false);
            cx.notify();
            return;
        };
        self.submit_settings_save(kind, generation, snapshot, cx);
    }

    fn submit_settings_save(
        &mut self,
        kind: SettingsSaveKind,
        generation: u64,
        snapshot: AppSettingsSummary,
        cx: &mut Context<Self>,
    ) {
        let operation = move |store: &nyaterm_store::ConnectionStore| match kind {
            SettingsSaveKind::Diagnostics => store.save_diagnostics_settings(&snapshot),
            SettingsSaveKind::General => store.save_general_settings(&snapshot),
            SettingsSaveKind::Interaction => store.save_interaction_settings(&snapshot),
            SettingsSaveKind::ScreenLock => store.save_screen_lock_settings(&snapshot),
            SettingsSaveKind::HostKey => store.save_host_key_policy(&snapshot.host_key_policy),
            SettingsSaveKind::Recording => store.save_recording_settings(&snapshot),
            SettingsSaveKind::Transfer => store.save_transfer_settings(&snapshot),
            SettingsSaveKind::Terminal => store.save_terminal_settings(&snapshot),
            SettingsSaveKind::QuickCommands => store.save_quick_command_ui_settings(&snapshot),
            SettingsSaveKind::Appearance => store.save_appearance_settings(&snapshot),
            SettingsSaveKind::UiLayout => store.save_ui_layout_settings(&snapshot),
            SettingsSaveKind::Keybindings => store.save_keybindings(&snapshot.keybindings),
            SettingsSaveKind::FileExplorer => store.save_file_explorer_favorite_dirs(&snapshot),
        };
        let request = if matches!(kind, SettingsSaveKind::UiLayout) {
            store_request(StoreDomain::Settings, operation)
        } else {
            nyaterm_store::store_mutation(StoreDomain::Settings, operation)
        };
        let task = match self.store_ui.try_submit(generation, request) {
            Ok(task) => task,
            Err(error) => {
                self.settings
                    .finish_persistence(kind.domain(), generation, false);
                let message = format!("{} save was not queued: {error}", kind.label());
                self.settings.update_store_status(message.clone(), false);
                self.shell.set_status(message);
                cx.notify();
                return;
            }
        };
        self.settings
            .update_store_status(format!("saving {}", kind.label()), false);
        cx.spawn(async move |this, cx| {
            let event = task.await;
            let _ = this.update(cx, |this, cx| {
                let succeeded = event.outcome.is_ok();
                let completion =
                    this.settings
                        .finish_persistence(kind.domain(), event.generation, succeeded);
                if completion.apply_result
                    && let Ok(settings) = event.outcome.as_ref()
                {
                    this.apply_gpui_settings(settings.clone(), cx);
                    this.publish_shared_settings(settings.clone(), cx);
                }
                if completion.report_result {
                    match event.outcome {
                        Ok(_) => {
                            let message = format!("{} saved", kind.label());
                            this.settings.update_store_status(message.clone(), true);
                            this.shell.set_status(message);
                        }
                        Err(error) => {
                            let message = format!("{} save failed: {error}", kind.label());
                            this.settings.update_store_status(message.clone(), false);
                            this.shell.set_status(message);
                        }
                    }
                }
                if let Some((generation, snapshot)) = completion.next {
                    this.submit_settings_save(kind, generation, snapshot, cx);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}
