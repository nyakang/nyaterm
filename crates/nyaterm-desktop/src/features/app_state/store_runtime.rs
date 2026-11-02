use gpui::{AppContext as _, Context, Task};
use nyaterm_core::{
    AiSettings, AppSettingsSummary, KeywordHighlightConfig, TranslationSettings,
    WorkspaceRestoreState, WorkspaceSessionState, WorkspaceUiState,
};
use nyaterm_store::{
    StoreDomain, StoreEvent, StoreRequest, StoreSubmitError, StoreTask, store_request,
};

use super::NyaTermApp;
use crate::features::settings::SettingsPersistenceDomain;

struct ShutdownPersistenceSnapshot {
    settings: AppSettingsSummary,
    settings_domains: Vec<SettingsPersistenceDomain>,
    keyword_highlights: Option<KeywordHighlightConfig>,
    ai_settings: Option<AiSettings>,
    translation_settings: Option<TranslationSettings>,
    workspace: Option<WorkspaceCloseSnapshot>,
}

pub(crate) struct WorkspaceCloseSnapshot {
    pub(crate) workspace_id: nyaterm_core::WorkspaceId,
    sessions: Option<WorkspaceSessionState>,
    ui: WorkspaceUiState,
}

impl WorkspaceCloseSnapshot {
    pub(crate) fn apply_to(self, workspace: &mut WorkspaceRestoreState) {
        workspace.revision = workspace.revision.saturating_add(1);
        if let Some(mut sessions) = self.sessions {
            sessions.extra = std::mem::take(&mut workspace.sessions.extra);
            workspace.sessions = sessions;
        }
        let mut ui = self.ui;
        ui.extra = std::mem::take(&mut workspace.ui.extra);
        workspace.ui = ui;
    }
}

impl NyaTermApp {
    pub(crate) fn capture_workspace_ui_state(&self) -> WorkspaceUiState {
        let bottom_panel_mode = match self.shell.bottom_panel_mode() {
            crate::models::BottomPanelMode::QuickCommands => "quick_commands",
            crate::models::BottomPanelMode::CommandSend => "command_send",
            crate::models::BottomPanelMode::Hidden => "hidden",
        };
        WorkspaceUiState {
            left_panel_width: self.shell.left_panel_width().round().clamp(160., 720.) as u32,
            right_panel_width: self.shell.right_panel_width().round().clamp(200., 720.) as u32,
            bottom_panel_height: self.shell.quick_commands_height().round().clamp(60., 600.) as u32,
            transfer_panel_height: self.transfer.panel_height().round().clamp(60., 600.) as u32,
            serial_send_panel_height: self.shell.command_send_height().round() as u32,
            bottom_panel_mode: bottom_panel_mode.to_string(),
            active_left_panel: self
                .shell
                .active_left_panel()
                .map(|item| item.persistence_id().to_string()),
            active_right_panel: self
                .shell
                .active_right_panel()
                .map(|item| item.persistence_id().to_string()),
            left_panel_collapsed: self.shell.left_panel_collapsed(),
            right_panel_collapsed: self.shell.right_panel_collapsed(),
            panel_multi_open: self.shell.panel_multi_open(),
            panel_open_mode: self.shell.panel_open_mode().as_setting().to_string(),
            left_open_panels: self.shell.left_open_panels().to_vec(),
            right_open_panels: self.shell.right_open_panels().to_vec(),
            panel_stack_sizes: self
                .shell
                .panel_stack_sizes()
                .iter()
                .filter_map(|(key, value)| {
                    let scaled = (*value * 1000.).round();
                    (scaled.is_finite() && scaled > 0.).then(|| (key.clone(), scaled as u32))
                })
                .collect(),
            current_page: self.shell.selected_nav().persistence_id().to_string(),
            extra: Default::default(),
        }
    }

    pub(crate) fn capture_workspace_close_snapshot(&mut self) -> WorkspaceCloseSnapshot {
        let settings = self.settings.summary().clone();
        let sessions = if settings.startup_restore {
            let open_tabs = self.serialize_open_tabs();
            let ordered = self
                .ordered_tab_sessions()
                .into_iter()
                .map(|session| session.id)
                .collect::<Vec<_>>();
            let terminal_window_layout = settings
                .startup_restore_window_layout
                .then(|| self.shell.serialize_terminal_window_layout(&ordered))
                .flatten();
            let workspace_pane_layout = if settings.startup_restore_window_layout {
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
            Some(WorkspaceSessionState {
                open_tabs,
                terminal_window_layout,
                workspace_pane_layout,
                extra: Default::default(),
            })
        } else {
            None
        };
        WorkspaceCloseSnapshot {
            workspace_id: self.workspace_id,
            sessions,
            ui: self.capture_workspace_ui_state(),
        }
    }

    pub(crate) fn report_close_save_failed(&mut self, error: String, cx: &mut Context<Self>) {
        let message = format!("Could not save before closing: {error}");
        self.settings.update_store_status(message.clone(), false);
        self.shell.set_status(message);
        cx.notify();
    }

    pub(crate) fn shutdown_workspace_sessions(&mut self) {
        for session in self.session.ordered_sessions() {
            let _ = self.session.manager().close(&session.id);
        }
    }

    pub(crate) fn shutdown_blocking_jobs(&mut self, cx: &mut Context<Self>) -> Task<()> {
        self.plugins.shutdown(cx);
        self.ai.invalidate_provider_jobs();
        self.ai.cancel_chat_and_agent();
        let agent_worker = self.ai.shutdown_agent_management_worker();
        let stop_jobs = self.blocking_jobs.begin_shutdown();
        let stop_transfers = self.transfer.begin_bulk_shutdown();
        let ssh_connections = self.session.ssh_connection_pool();
        self.remote_desktop.routes.clear();
        self.remote_desktop.prepared_routes.clear();
        self.shutdown_remote_desktop_workers();
        #[cfg(windows)]
        let native_hosts = self.remote_desktop.take_native_hosts_for_shutdown();
        let session_shutdown = self.session.take_shutdown();
        let frame_worker = self.terminal.take_frame_shutdown();
        let recording_shutdown = self.recording.take_shutdown();
        let stop_editor_watchers = self.transfer.begin_external_editor_shutdown();
        cx.background_spawn(async move {
            #[cfg(windows)]
            for host in native_hosts {
                host.shutdown();
            }
            stop_editor_watchers.await;
            session_shutdown();
            if let Some(shutdown) = frame_worker {
                shutdown();
            }
            if let Some(shutdown) = recording_shutdown {
                for error in shutdown() {
                    tracing::warn!(%error, "recording shutdown failed");
                }
            }
            if let Some(worker) = agent_worker {
                let _ = worker.join();
            }
            stop_jobs();
            stop_transfers();
            ssh_connections.finish_disconnects();
        })
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
                        let shared_domain = event
                            .outcome
                            .is_ok()
                            .then(|| {
                                crate::app_shell::SharedStateDomain::from_store_domain(event.domain)
                            })
                            .flatten();
                        apply(this, event, cx);
                        // Every async reply lands here, after the whole handler body
                        // has run. Handlers that mutate list state *after* swapping
                        // the catalog therefore still flush fresh state, which a
                        // flush inside `apply_loaded_sessions` could not promise.
                        this.flush_connection_panel_snapshot(cx);
                        this.flush_transfer_panel_snapshot(cx);
                        if let Some(domain) = shared_domain {
                            this.request_shared_state_refresh(domain, cx);
                        }
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

    pub(crate) fn request_shared_state_refresh(
        &self,
        domain: crate::app_shell::SharedStateDomain,
        cx: &mut Context<Self>,
    ) {
        if let Some(controller) = self.desktop_controller.clone() {
            cx.defer(move |cx| {
                let _ = controller.update(cx, |controller, cx| {
                    controller.request_shared_state_refresh(domain, cx)
                });
            });
        }
    }

    pub(crate) fn replace_shared_snapshot(
        &self,
        snapshot: nyaterm_store::BootstrapSnapshot,
        domain: crate::app_shell::SharedStateDomain,
        cx: &mut Context<Self>,
    ) {
        if let Some(controller) = self.desktop_controller.clone() {
            let registration = self
                .cloud_sync
                .operations
                .followup("cloud-sync-shared-refresh");
            cx.defer(move |cx| {
                let _ = controller.update(cx, |controller, cx| {
                    controller.replace_shared_snapshot(snapshot, domain, cx)
                });
                drop(registration);
            });
        }
    }

    pub(in crate::features) fn store_blocking_client(&self) -> nyaterm_store::StoreBlockingClient {
        self.store_blocking.clone()
    }

    pub(crate) fn submit_shutdown_persistence(
        &mut self,
        preserve_workspace: bool,
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
        let keyword_highlights = self
            .settings
            .keyword_persistence_dirty()
            .then(|| self.settings.keyword_config().clone());
        let ai_settings = self
            .ai
            .settings_persistence_is_dirty()
            .then(|| self.ai.pending_settings());
        let translation_settings = self
            .translation
            .settings_persistence_is_dirty()
            .then(|| self.translation.pending_settings());
        let workspace = preserve_workspace.then(|| self.capture_workspace_close_snapshot());
        let snapshot = ShutdownPersistenceSnapshot {
            settings,
            settings_domains,
            keyword_highlights,
            ai_settings,
            translation_settings,
            workspace,
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
                if let Some(keyword_highlights) = snapshot.keyword_highlights {
                    store.save_keyword_highlights(&keyword_highlights)?;
                }
                if let Some(settings) = snapshot.ai_settings {
                    store.save_ai_settings(settings)?;
                }
                if let Some(settings) = snapshot.translation_settings {
                    store.save_translation_settings(settings)?;
                }
                if let Some(close_snapshot) = snapshot.workspace {
                    let mut manifest = store.load_workspace_restore_manifest()?;
                    let workspace_index = manifest
                        .workspaces
                        .iter()
                        .position(|workspace| workspace.id == close_snapshot.workspace_id);
                    let workspace = if let Some(index) = workspace_index {
                        &mut manifest.workspaces[index]
                    } else {
                        manifest
                            .workspaces
                            .push(WorkspaceRestoreState::empty(close_snapshot.workspace_id));
                        manifest
                            .workspaces
                            .last_mut()
                            .expect("workspace was inserted")
                    };
                    close_snapshot.apply_to(workspace);
                    store.save_workspace_restore_manifest(&manifest)?;
                }
                Ok(())
            }),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, mpsc};
    use std::time::Duration;

    use gpui::{AppContext as _, TestAppContext};
    use nyaterm_core::test_support::TestTempDir;

    use crate::features::test_support::app_with_visible_local_session;

    #[gpui::test]
    fn workspace_shutdown_keeps_gpui_updates_responsive_while_a_request_finishes(
        cx: &mut TestAppContext,
    ) {
        let directory = TestTempDir::new("nyaterm-workspace-worker-shutdown");
        let app = app_with_visible_local_session(cx, directory.path(), "shutdown-fixture");
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        cx.update_entity(&app, |app, _| {
            app.blocking_jobs
                .submit_detached("held-provider-request", move |_| {
                    started_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                })
                .unwrap();
        });
        started_rx.recv().unwrap();
        let timed_out = Arc::new(AtomicBool::new(false));
        let watchdog_timeout = Arc::clone(&timed_out);
        let (responsive_tx, responsive_rx) = mpsc::channel();
        let watchdog = std::thread::spawn(move || {
            if responsive_rx.recv_timeout(Duration::from_secs(2)).is_err() {
                watchdog_timeout.store(true, Ordering::Release);
            }
            release_tx.send(()).unwrap();
        });
        let cleanup = cx.update_entity(&app, |app, cx| app.shutdown_blocking_jobs(cx));
        cx.update_entity(&app, |_, _| {
            responsive_tx.send(()).unwrap();
        });
        watchdog.join().unwrap();
        cleanup.detach();
        cx.run_until_parked();
        assert!(
            !timed_out.load(Ordering::Acquire),
            "shutdown must yield the UI thread before waiting"
        );
    }
}
