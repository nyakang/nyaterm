use gpui::Context;
use nyaterm_core::{
    AiSettings, AppSettingsSummary, KeywordHighlightConfig, RestorableOpenTab,
    RestorableTerminalWindowNode, RestorableWorkspacePaneNode, TranslationSettings,
};
use nyaterm_store::{
    StoreDomain, StoreEvent, StoreRequest, StoreSubmitError, StoreTask, store_request,
};

use super::NyaTermApp;
use crate::features::settings::SettingsPersistenceDomain;

struct ShutdownPersistenceSnapshot {
    settings: AppSettingsSummary,
    settings_domains: Vec<SettingsPersistenceDomain>,
    keyword_highlights: KeywordHighlightConfig,
    ai_settings: Option<AiSettings>,
    translation_settings: Option<TranslationSettings>,
    session: Option<ShutdownSessionSnapshot>,
}

struct ShutdownSessionSnapshot {
    open_tabs: Vec<RestorableOpenTab>,
    terminal_layout: Option<RestorableTerminalWindowNode>,
    workspace_layout: Option<RestorableWorkspacePaneNode>,
}

impl NyaTermApp {
    pub(crate) fn shutdown_blocking_jobs(&mut self) {
        self.update.download_cancel.cancel();
        self.remote_desktop.routes.clear();
        self.remote_desktop.prepared_routes.clear();
        self.shell.system_tray = None;
        self.shutdown_remote_desktop_workers();
        self.session.shutdown_workers();
        self.terminal.shutdown_workers();
        self.recording.shutdown_worker();
        self.transfer.shutdown_external_editor_watchers();
        self.blocking_jobs.shutdown();
    }

    pub(crate) fn report_shutdown_retry_required(&mut self, cx: &mut Context<Self>) {
        let message =
            "storage is available again; retry the failed save, then close NyaTerm".to_string();
        self.settings.update_store_status(message.clone(), false);
        self.shell.set_status(message);
        cx.notify();
    }

    pub(in crate::features) fn submit_store_request<R>(
        &mut self,
        generation: u64,
        request: R,
        apply: impl FnOnce(&mut Self, StoreEvent<R::Response>, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> bool
    where
        R: StoreRequest,
    {
        match self.store_ui.try_submit(generation, request) {
            Ok(task) => {
                cx.spawn(async move |this, cx| {
                    let event = task.await;
                    let _ = this.update(cx, |this, cx| {
                        apply(this, event, cx);
                        // Every async reply lands here, after the whole handler body
                        // has run. Handlers that mutate list state *after* swapping
                        // the catalog therefore still flush fresh state, which a
                        // flush inside `apply_loaded_sessions` could not promise.
                        this.flush_connection_panel_snapshot(cx);
                        this.flush_transfer_panel_snapshot(cx);
                    });
                })
                .detach();
                true
            }
            Err(error) => {
                let message = format!("storage request was not queued: {error}");
                self.settings.update_store_status(message.clone(), false);
                self.shell.set_status(message);
                cx.notify();
                false
            }
        }
    }

    pub(in crate::features) fn store_blocking_client(&self) -> nyaterm_store::StoreBlockingClient {
        self.store_blocking.clone()
    }

    pub(crate) fn submit_shutdown_persistence(
        &mut self,
    ) -> Result<StoreTask<()>, StoreSubmitError> {
        // A UI-layout change still inside its debounce window has no dirty settings
        // domain yet, so fold it in here or quitting inside that window loses it.
        // The session half below is re-serialized unconditionally and needs no
        // equivalent.
        if self.shell.take_ui_layout_persist_pending() {
            self.settings
                .mark_persistence_dirty(SettingsPersistenceDomain::UiLayout);
        }
        let settings_domains = self.settings.dirty_persistence_domains();
        let settings = self.settings.summary().clone();
        let keyword_highlights = self.settings.keyword_config().clone();
        let ai_settings = self
            .ai
            .settings_persistence_is_dirty()
            .then(|| self.ai.pending_settings());
        let translation_settings = self
            .translation
            .settings_persistence_is_dirty()
            .then(|| self.translation.pending_settings());
        let session = if settings.startup_restore {
            let open_tabs = self.serialize_open_tabs();
            let ordered = self
                .ordered_tab_sessions()
                .into_iter()
                .map(|session| session.id)
                .collect::<Vec<_>>();
            let terminal_layout = settings
                .startup_restore_window_layout
                .then(|| self.terminal.serialize_terminal_window_layout(&ordered))
                .flatten();
            let workspace_layout = if settings.startup_restore_window_layout {
                self.sync_workspace_split_from_active_tab();
                let ordered = self
                    .session
                    .ordered_sessions()
                    .into_iter()
                    .map(|session| session.id)
                    .collect::<Vec<_>>();
                self.shell
                    .workspace_split()
                    .as_ref()
                    .filter(|root| root.is_split())
                    .and_then(|root| root.serialize_layout(&ordered))
                    .or_else(|| {
                        self.shell
                            .workspace_pane_roots()
                            .values()
                            .find(|root| root.is_split())
                            .and_then(|root| root.serialize_layout(&ordered))
                    })
            } else {
                None
            };
            Some(ShutdownSessionSnapshot {
                open_tabs,
                terminal_layout,
                workspace_layout,
            })
        } else {
            None
        };
        let snapshot = ShutdownPersistenceSnapshot {
            settings,
            settings_domains,
            keyword_highlights,
            ai_settings,
            translation_settings,
            session,
        };
        self.store_ui.try_submit_shutdown(
            u64::MAX - 1,
            store_request(StoreDomain::Shutdown, move |store| {
                for domain in snapshot.settings_domains {
                    match domain {
                        SettingsPersistenceDomain::Diagnostics => {
                            store.save_diagnostics_settings(&snapshot.settings)?;
                        }
                        SettingsPersistenceDomain::General => {
                            store.save_general_settings(&snapshot.settings)?;
                        }
                        SettingsPersistenceDomain::Interaction => {
                            store.save_interaction_settings(&snapshot.settings)?;
                        }
                        SettingsPersistenceDomain::ScreenLock => {
                            store.save_screen_lock_settings(&snapshot.settings)?;
                        }
                        SettingsPersistenceDomain::HostKey => {
                            store.save_host_key_policy(&snapshot.settings.host_key_policy)?;
                        }
                        SettingsPersistenceDomain::Recording => {
                            store.save_recording_settings(&snapshot.settings)?;
                        }
                        SettingsPersistenceDomain::Transfer => {
                            store.save_transfer_settings(&snapshot.settings)?;
                        }
                        SettingsPersistenceDomain::Terminal => {
                            store.save_terminal_settings(&snapshot.settings)?;
                        }
                        SettingsPersistenceDomain::QuickCommands => {
                            store.save_quick_command_ui_settings(&snapshot.settings)?;
                        }
                        SettingsPersistenceDomain::Appearance => {
                            store.save_appearance_settings(&snapshot.settings)?;
                        }
                        SettingsPersistenceDomain::UiLayout => {
                            store.save_ui_layout_settings(&snapshot.settings)?;
                        }
                        SettingsPersistenceDomain::Keybindings => {
                            store.save_keybindings(&snapshot.settings.keybindings)?;
                        }
                        SettingsPersistenceDomain::FileExplorer => {
                            store.save_file_explorer_favorite_dirs(&snapshot.settings)?;
                        }
                    }
                }
                store.save_keyword_highlights(&snapshot.keyword_highlights)?;
                if let Some(settings) = snapshot.ai_settings {
                    store.save_ai_settings(settings)?;
                }
                if let Some(settings) = snapshot.translation_settings {
                    store.save_translation_settings(settings)?;
                }
                if let Some(session) = snapshot.session {
                    store.save_open_tabs(&session.open_tabs)?;
                    store.save_terminal_window_layout(session.terminal_layout.as_ref())?;
                    store.save_workspace_pane_layout(session.workspace_layout.as_ref())?;
                }
                Ok(())
            }),
        )
    }
}
