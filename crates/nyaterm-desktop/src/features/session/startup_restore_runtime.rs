use gpui::{Context, Window};
use nyaterm_core::{
    AiExecutionProfile, RestorableOpenTab, RestorablePaneNode, RestorableWorkspacePaneNode,
};
use nyaterm_remote_desktop::{
    RdpClipboardConfig, RdpDisplayConfig, RdpReconnectConfig, RdpSessionConfig, VncClipboardConfig,
    VncDisplayConfig, VncReconnectConfig, VncSecurityConfig, VncSessionConfig, parse_rdp_backend,
    parse_rdp_certificate_policy, parse_rdp_clipboard_mode, parse_vnc_scale_mode,
    parse_vnc_security_mode,
};
use nyaterm_store::{StoreDomain, store_request};
use nyaterm_transport::{LocalSessionConfig, SessionInfo};

use crate::features::{NyaTermApp, session::SavedConnectionStartOptions};
use crate::models::{SessionLaunchConfig, WorkspacePaneNode, WorkspaceSplitDirection};

mod focus;

impl NyaTermApp {
    fn mark_startup_restore_complete(&mut self) {
        self.session.mark_restore_complete();
    }

    /// Mark open tabs (and multi-leaf layout) dirty for a later idle flush.
    ///
    /// Connect/register must not open the config database or rewrite settings on
    /// the UI thread — that path was a major connect-time freeze source.
    pub(in crate::features) fn persist_open_tabs(&mut self) {
        self.workspace_revision = self.workspace_revision.saturating_add(1);
        if !self.settings.summary().startup_restore {
            return;
        }
        self.shell.mark_open_tabs_persist_dirty();
        // Keep multi-leaf layout indexes aligned with the same ordered tab list.
        self.persist_terminal_window_layout();
    }

    /// Force a durable open-tabs write (window close / explicit quit paths).
    pub(in crate::features) fn flush_open_tabs_now(&mut self, cx: &mut Context<Self>) {
        if !self.settings.summary().startup_restore {
            self.shell.clear_session_persistence_dirty();
            return;
        }
        self.shell.mark_session_persistence_dirty();
        self.flush_pending_session_persistence(cx);
    }

    /// Idle plane: snapshot dirty state and write config on a background thread.
    ///
    /// Serialization stays on the UI thread (local maps only). Opening redb and
    /// rewriting settings is never done on the UI tick — that freezes connect
    /// and the first idle frame after connect.
    pub(in crate::features) fn flush_pending_session_persistence(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        if !self.settings.summary().startup_restore {
            self.shell.clear_session_persistence_dirty();
            return;
        }
        if !self.session.restore_is_complete()
            || !self.shell.terminal_windows_restore_is_complete()
            || !self.shell.workspace_layout_restore_is_settled()
        {
            return;
        }
        if !self.shell.terminal_layout_persistence_is_supported()
            || !self.normalize_legacy_terminal_groups()
        {
            return;
        }
        let dirty = self
            .shell
            .pending_session_persistence(self.settings.summary().startup_restore_window_layout);
        if dirty.is_empty() {
            return;
        }

        let tabs = dirty.open_tabs().then(|| self.serialize_open_tabs());
        let layout = if dirty.window_layout() {
            let ordered = self
                .ordered_tab_sessions()
                .into_iter()
                .map(|session| session.id)
                .collect::<Vec<_>>();
            Some(self.shell.serialize_terminal_window_layout(&ordered))
        } else {
            None
        };

        let workspace_layout = if dirty.window_layout() {
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
        let Some(generation) = self.shell.begin_session_persistence(dirty) else {
            return;
        };
        let workspace_id = self.workspace_id;
        let request = store_request(StoreDomain::Sessions, move |store| {
            let mut manifest = store.load_workspace_restore_manifest()?;
            let workspace = if let Some(index) = manifest
                .workspaces
                .iter()
                .position(|workspace| workspace.id == workspace_id)
            {
                &mut manifest.workspaces[index]
            } else {
                manifest
                    .workspaces
                    .push(nyaterm_core::WorkspaceRestoreState::empty(workspace_id));
                manifest
                    .workspaces
                    .last_mut()
                    .expect("workspace was inserted")
            };
            workspace.revision = workspace.revision.saturating_add(1);
            if let Some(tabs) = tabs {
                workspace.sessions.open_tabs = tabs;
            }
            if let Some(layout) = layout {
                workspace.sessions.terminal_window_layout = layout;
            }
            if dirty.window_layout() {
                workspace.sessions.workspace_pane_layout = workspace_layout;
            }
            store.save_workspace_restore_manifest(&manifest)
        });
        let task = match self.store_ui.try_submit(generation, request) {
            Ok(task) => task,
            Err(error) => {
                self.shell
                    .finish_session_persistence(generation, dirty, false);
                let message = format!("session layout save was not queued: {error}");
                self.shell.set_status(message.clone());
                self.settings.update_store_status(message, false);
                return;
            }
        };
        cx.spawn(async move |this, cx| {
            let event = task.await;
            let _ = this.update(cx, move |this, cx| {
                let succeeded = event.outcome.is_ok();
                if this
                    .shell
                    .finish_session_persistence(event.generation, dirty, succeeded)
                    && let Err(error) = event.outcome
                {
                    let message = format!("failed to save session layout: {error}");
                    this.shell.set_status(message.clone());
                    this.settings.update_store_status(message, false);
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(in crate::features) fn serialize_open_tabs(&self) -> Vec<RestorableOpenTab> {
        // Prefer a single Tauri-style open_tabs entry when one pane tree covers every session.
        if let Some(tabs) = self.serialize_open_tabs_as_single_pane_tab() {
            return tabs
                .into_iter()
                .chain(self.session.unsupported_restore_tabs.clone())
                .collect();
        }

        // One strip tab per tab-root; attach RestorablePaneNode when that tab is split.
        self.ordered_tab_sessions()
            .into_iter()
            .map(|session| {
                let mut tab = self.serialize_open_tab_for_session(&session);
                if let Some(root) = self.shell.workspace_pane_root(&session.id)
                    && root.is_split()
                    && let Some(pane_root) = self.workspace_pane_to_restorable_pane(root)
                {
                    tab.root = Some(pane_root);
                    tab.active_pane_id = Some(self.active_pane_for_tab_root(&session.id));
                }
                tab
            })
            .chain(self.session.unsupported_restore_tabs.clone())
            .collect()
    }

    fn serialize_open_tab_for_session(&self, session: &SessionInfo) -> RestorableOpenTab {
        let metadata = self.session.metadata(&session.id);
        let connection_id = metadata.and_then(|meta| meta.source_connection_id.clone());
        let session_type = match metadata.map(|meta| &meta.launch_config) {
            Some(SessionLaunchConfig::Ssh(_)) => "SSH",
            Some(SessionLaunchConfig::Telnet(_)) => "Telnet",
            Some(SessionLaunchConfig::Serial(_)) => "Serial",
            Some(SessionLaunchConfig::Rdp(_)) => "RDP",
            Some(SessionLaunchConfig::Vnc(_)) => "VNC",
            Some(SessionLaunchConfig::Local(_)) | None => "Local",
        }
        .to_string();
        let custom_name = self.session.custom_name(&session.id).map(ToOwned::to_owned);
        let tab_color = self
            .session
            .tab_color(&session.id)
            .map(|color| format!("#{color:06x}"));
        let title = self.session.display_name_by_info(session);
        let mut tab = RestorableOpenTab::with_leaf_root(
            title,
            session_type,
            connection_id,
            custom_name,
            tab_color,
        );
        tab.ssh_runtime_mode = metadata
            .and_then(|meta| meta.ssh_config.as_ref())
            .map(|config| config.effective_runtime_mode().as_str().to_string());
        if let Some(RestorablePaneNode::Leaf {
            ssh_runtime_mode, ..
        }) = tab.root.as_mut()
        {
            *ssh_runtime_mode = tab.ssh_runtime_mode.clone();
        }
        tab.locked = self.tab_tree_is_locked(&session.id);
        tab
    }

    /// When every session is present in a global workspace split, emit one open_tabs
    /// entry whose `root` is a Tauri RestorablePaneNode tree (for interop).
    fn serialize_open_tabs_as_single_pane_tab(&self) -> Option<Vec<RestorableOpenTab>> {
        // Only collapse to one open_tabs entry when exactly one split tree covers every session.
        if self.shell.workspace_pane_roots().len() > 1 {
            return None;
        }
        let root = self
            .shell
            .workspace_pane_roots()
            .values()
            .find(|root| root.is_split())
            .or(self.shell.workspace_split())?;
        if !root.is_split() {
            return None;
        }
        let ordered = self.session.ordered_sessions();
        if ordered.len() < 2 {
            return None;
        }
        let split_ids = root.session_ids();
        if split_ids.len() != ordered.len() {
            return None;
        }
        // Require the pane tree to cover exactly the ordered session set.
        let ordered_set = ordered
            .iter()
            .map(|session| session.id.as_str())
            .collect::<std::collections::HashSet<_>>();
        if split_ids
            .iter()
            .any(|id| !ordered_set.contains(id.as_str()))
        {
            return None;
        }

        let pane_root = self.workspace_pane_to_restorable_pane(root)?;
        let first = ordered.first()?;
        let mut tab = self.serialize_open_tab_for_session(first);
        // Title/type from first leaf; root carries the full tree.
        tab.root = Some(pane_root);
        tab.active_pane_id = self.session.active_id_owned();
        Some(vec![tab])
    }

    fn workspace_pane_to_restorable_pane(
        &self,
        node: &WorkspacePaneNode,
    ) -> Option<RestorablePaneNode> {
        match node {
            WorkspacePaneNode::Leaf { session_id } => {
                let session = self
                    .session
                    .ordered_sessions()
                    .into_iter()
                    .find(|session| &session.id == session_id)?;
                let tab = self.serialize_open_tab_for_session(&session);
                // Use runtime session id as RestorablePane leaf id so active_pane_id roundtrips.
                Some(RestorablePaneNode::Leaf {
                    ssh_runtime_mode: tab.ssh_runtime_mode,
                    id: session_id.clone(),
                    title: tab.title,
                    session_type: tab.session_type,
                    connection_id: tab.connection_id,
                })
            }
            WorkspacePaneNode::Split {
                id,
                direction,
                ratio_percent,
                first,
                second,
            } => {
                let first = self.workspace_pane_to_restorable_pane(first);
                let second = self.workspace_pane_to_restorable_pane(second);
                match (first, second) {
                    (None, None) => None,
                    (Some(only), None) | (None, Some(only)) => Some(only),
                    (Some(first), Some(second)) => {
                        let ratio = (WorkspacePaneNode::clamped_ratio_percent(*ratio_percent)
                            as f64)
                            / 100.0;
                        Some(RestorablePaneNode::Split {
                            id: id.clone(),
                            direction: match direction {
                                WorkspaceSplitDirection::Horizontal => "horizontal".to_string(),
                                WorkspaceSplitDirection::Vertical => "vertical".to_string(),
                            },
                            ratio: ratio.clamp(0.2, 0.8),
                            first: Box::new(first),
                            second: Box::new(second),
                        })
                    }
                }
            }
        }
    }

    pub(in crate::features) fn resume_startup_restore_if_unlocked(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.security.screen_locked() {
            return;
        }
        self.try_restore_open_tabs(window, cx);
        self.pump_startup_restore_queue_if_ready(window, cx);
    }

    pub(in crate::features) fn try_restore_open_tabs(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let should_restore = self
            .stores
            .startup_restore
            .update(cx, |store, _| store.mark_open_tabs_restored());
        if !should_restore {
            return;
        }
        if !self.settings.summary().startup_restore {
            self.mark_startup_restore_complete();
            return;
        }
        if !self.session.ordered_sessions().is_empty() {
            self.mark_startup_restore_complete();
            return;
        }
        let Some(tabs) = self
            .stores
            .startup_restore
            .update(cx, |store, _| store.take_loaded_open_tabs())
        else {
            self.mark_startup_restore_complete();
            return;
        };
        if tabs.is_empty() {
            self.mark_startup_restore_complete();
            return;
        }
        // Expand Tauri per-tab pane trees into a flat restore queue of sessions.
        // Remember every multi-pane root so we can reinstall per-tab trees after connect.
        let mut pending_pane_layouts = Vec::new();
        let mut pending_active_pane_indexes = Vec::new();
        let mut expanded = Vec::new();
        let mut base_index = 0usize;
        for tab in &tabs {
            if !tab.ssh_runtime_modes_supported() {
                // Global layout indexes still refer to the skipped tab. Keep the
                // original manifest intact rather than remapping it incorrectly.
                self.shell.preserve_unread_terminal_layout();
                self.session.unsupported_restore_tabs.push(tab.clone());
                continue;
            }
            if let Some(layout) = tab.workspace_pane_layout_from_root(base_index) {
                pending_pane_layouts.push(layout);
            }
            if let (Some(root), Some(active_pane_id)) =
                (tab.root.as_ref(), tab.active_pane_id.as_ref())
                && let Some(leaf_offset) = root
                    .collect_leaves()
                    .iter()
                    .position(|leaf| &leaf.id == active_pane_id)
            {
                pending_active_pane_indexes.push(base_index + leaf_offset);
            }
            let sessions = tab.expanded_sessions();
            base_index += sessions.len();
            for session in sessions {
                expanded.push(RestorableOpenTab {
                    ssh_runtime_mode: session.ssh_runtime_mode,
                    title: session.title,
                    session_type: session.session_type,
                    connection_id: session.connection_id,
                    custom_name: session.custom_name,
                    tab_color: session.tab_color,
                    locked: session.locked,
                    active_pane_id: None,
                    root: None,
                });
            }
        }
        if expanded.is_empty() {
            self.mark_startup_restore_complete();
            return;
        }
        let queue_len = expanded.len();
        self.stores.startup_restore.update(cx, |store, _| {
            store.clear_pending_layouts();
            for layout in pending_pane_layouts {
                store.push_pending_pane_layout(layout);
            }
            for index in pending_active_pane_indexes {
                store.push_pending_active_pane_index(index);
            }
            store.set_queue(expanded);
            store.request_focus_handoff();
        });
        self.shell
            .set_status(format!("restoring {} workspace tab(s)...", queue_len));
        self.pump_startup_restore_queue(window, cx);
    }

    /// Pump the next queued restore if one may go now.
    ///
    /// The idle plane used to poll this pair of conditions; both change only when a
    /// session start settles, so the session-start drain drives it instead.
    pub(in crate::features) fn pump_startup_restore_queue_if_ready(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let pending_session_start =
            self.session.start_has_pending() || self.remote_desktop.restore_pending.is_some();
        let session_start_busy =
            self.session.start_is_busy() || self.remote_desktop.restore_pending.is_some();
        let should_pump = !self.session.restore_is_complete()
            && self.stores.startup_restore.update(cx, |store, _| {
                store.can_pump_queue(pending_session_start)
                    || store.can_finish_restore(session_start_busy)
            });
        if !should_pump {
            return false;
        }
        self.pump_startup_restore_queue(window, cx);
        true
    }

    pub(in crate::features) fn pump_startup_restore_queue(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.session.restore_is_complete() {
            return;
        }
        if self.session.start_has_pending() || self.remote_desktop.restore_pending.is_some() {
            return;
        }
        let Some(tab) = self
            .stores
            .startup_restore
            .update(cx, |store, _| store.pop_next_tab())
        else {
            if !self.session.start_is_busy() {
                self.finish_startup_restore(cx);
            }
            return;
        };

        let started = self.start_restorable_open_tab(&tab, window, cx);
        if !started {
            // Keep draining sync failures until pending async work or queue empty.
            self.pump_startup_restore_queue(window, cx);
        }
    }

    fn finish_startup_restore(&mut self, cx: &mut Context<Self>) {
        if self.session.restore_is_complete() {
            return;
        }
        self.mark_startup_restore_complete();
        // After all tabs reconnect, attempt multi-leaf then global pane layout restore.
        self.shell.mark_terminal_windows_restore_pending();
        self.shell.set_workspace_pane_layout_restored(false);
        // Prefer stored ui.workspace_pane_layout only when no open_tabs per-tab roots exist.
        // open_tabs[].root maps to per-tab session_pane_roots (Tauri Tab.root).
        let pending_layouts = self
            .stores
            .startup_restore
            .update(cx, |store, _| store.take_pending_pane_layouts());
        let pending_active = self
            .stores
            .startup_restore
            .update(cx, |store, _| store.take_pending_active_pane_indexes());
        if pending_layouts.is_empty() {
            self.try_restore_workspace_pane_layout(cx);
        } else {
            self.shell.set_workspace_pane_layout_restored(true);
            for layout in pending_layouts {
                self.apply_restorable_workspace_pane_layout(layout);
            }
            // Focus last requested active pane leaf if still present.
            if let Some(index) = pending_active.last().copied() {
                let ordered = self
                    .session
                    .ordered_sessions()
                    .into_iter()
                    .map(|session| session.id)
                    .collect::<Vec<_>>();
                if let Some(session_id) = ordered.get(index) {
                    self.activate_session_id(session_id, cx);
                    self.sync_workspace_split_from_active_tab();
                }
            }
        }
        self.try_restore_terminal_window_layout(cx);
        self.normalize_legacy_terminal_groups();
        if self.terminal_windows_is_multi_leaf() {
            self.shell
                .set_status("restored workspace tabs and window layout".to_string());
        } else if !self.shell.workspace_pane_roots().is_empty()
            || self
                .shell
                .workspace_split()
                .is_some_and(|root| root.is_split())
        {
            self.shell
                .set_status("restored workspace tabs and pane layout".to_string());
        } else if !self.session.ordered_sessions().is_empty() {
            self.shell.set_status("restored workspace tabs".to_string());
        }
        if !self.session.ordered_sessions().is_empty() {
            self.persist_open_tabs();
        }
        self.restore_file_manager_after_sessions(cx);
        cx.notify();
    }

    fn start_restorable_open_tab(
        &mut self,
        tab: &RestorableOpenTab,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let custom_name = tab
            .custom_name
            .clone()
            .filter(|value| !value.trim().is_empty());
        let tab_color = parse_restorable_tab_color(tab.tab_color.as_deref());
        let session_type = tab.session_type.to_ascii_lowercase();

        if let Some(connection_id) = tab.connection_id.as_ref().filter(|id| !id.is_empty()) {
            let connection = self
                .connection_state
                .connections()
                .iter()
                .find(|connection| &connection.id == connection_id)
                .cloned();
            let Some(connection) = connection else {
                self.shell.set_status(format!(
                    "restore skipped missing connection {connection_id}"
                ));
                return false;
            };
            // A saved tab may predate a switch to an independent system client.
            if matches!(&connection.config, nyaterm_core::ConnectionType::Rdp { backend, .. } if backend == "mstsc")
            {
                return false;
            }
            if let nyaterm_core::ConnectionType::Rdp {
                host,
                port,
                username,
                domain,
                backend,
                security,
                display,
                clipboard,
                reconnect,
            } = &connection.config
            {
                let session_id = nyaterm_core::uuid();
                let mut config = RdpSessionConfig {
                    relay: None,
                    backend: match parse_rdp_backend(backend) {
                        Ok(backend) => backend,
                        Err(error) => {
                            self.shell.set_status(error.to_string());
                            return false;
                        }
                    },
                    windows_authentication: security.windows_authentication.clone(),
                    name: connection.name.clone(),
                    host: host.clone(),
                    port: *port,
                    username: username.clone(),
                    domain: domain.clone(),
                    password: None,
                    use_nla: security.use_nla,
                    certificate_policy: parse_rdp_certificate_policy(&security.certificate_policy),
                    administrative_session: security.administrative_session,
                    display: RdpDisplayConfig::from(display),
                    clipboard: RdpClipboardConfig {
                        mode: parse_rdp_clipboard_mode(&clipboard.mode),
                    },
                    reconnect: RdpReconnectConfig {
                        enabled: reconnect.enabled,
                        max_attempts: reconnect.max_attempts,
                    },
                };
                config.normalize_backend_settings();
                self.remote_desktop.insert_disconnected(session_id.clone());
                self.register_session(
                    &session_id,
                    crate::models::SessionRuntimeMetadata {
                        ssh_config: None,
                        ssh_multiplex_key: None,
                        source_connection_id: Some(connection.id.clone()),
                        ai_execution_profile: AiExecutionProfile::Disabled,
                        launch_config: SessionLaunchConfig::Rdp(config),
                        disconnected: true,
                    },
                );
                if let Some(name) = custom_name {
                    self.session.set_custom_name(session_id.clone(), name);
                }
                if let Some(color) = tab_color {
                    self.session.set_tab_color(&session_id, Some(color));
                }
                if tab.locked {
                    self.session.set_tab_locked(&session_id, true);
                }
                if self.session.active_id().is_none() {
                    self.activate_session_id(&session_id, cx);
                }
                self.remote_desktop.restore_pending = Some(session_id.clone());
                self.retry_rdp_runtime(&session_id, cx);
                self.settle_remote_desktop_restore(cx);
                return true;
            }
            if let nyaterm_core::ConnectionType::Vnc {
                username,
                host,
                port,
                security,
                display,
                clipboard,
                reconnect,
                shared,
                view_only,
            } = &connection.config
            {
                let session_id = nyaterm_core::uuid();
                let config = VncSessionConfig {
                    username: username.clone(),
                    relay: None,
                    name: connection.name.clone(),
                    host: host.clone(),
                    port: *port,
                    password: None,
                    security: VncSecurityConfig {
                        mode: parse_vnc_security_mode(&security.mode),
                    },
                    display: VncDisplayConfig {
                        scale_mode: parse_vnc_scale_mode(&display.scale_mode),
                    },
                    clipboard: VncClipboardConfig {
                        enabled: clipboard.enabled,
                    },
                    reconnect: VncReconnectConfig {
                        enabled: reconnect.enabled,
                        max_attempts: reconnect.max_attempts,
                    },
                    shared: *shared,
                    view_only: *view_only,
                };
                self.remote_desktop.insert_disconnected(session_id.clone());
                self.register_session(
                    &session_id,
                    crate::models::SessionRuntimeMetadata {
                        ssh_config: None,
                        ssh_multiplex_key: None,
                        source_connection_id: Some(connection.id.clone()),
                        ai_execution_profile: AiExecutionProfile::Disabled,
                        launch_config: SessionLaunchConfig::Vnc(config),
                        disconnected: true,
                    },
                );
                if let Some(name) = custom_name {
                    self.session.set_custom_name(session_id.clone(), name);
                }
                if let Some(color) = tab_color {
                    self.session.set_tab_color(&session_id, Some(color));
                }
                if tab.locked {
                    self.session.set_tab_locked(&session_id, true);
                }
                if self.session.active_id().is_none() {
                    self.activate_session_id(&session_id, cx);
                }
                self.remote_desktop.restore_pending = Some(session_id.clone());
                self.retry_rdp_runtime(&session_id, cx);
                self.settle_remote_desktop_restore(cx);
                return true;
            }
            self.continue_saved_connection_start(
                connection,
                SavedConnectionStartOptions {
                    ssh_runtime_mode: tab
                        .ssh_runtime_mode
                        .as_deref()
                        .and_then(nyaterm_core::models::ssh_runtime::SshRuntimeMode::parse),
                    custom_name,
                    tab_color,
                    locked: tab.locked,
                    ..Default::default()
                },
                cx,
            );
            return true;
        }

        if session_type == "local" || session_type.is_empty() {
            let mut config = LocalSessionConfig::default();
            self.apply_desired_geometry_to_local_config(&mut config);
            self.begin_background_session_start(
                config.name.clone(),
                SessionLaunchConfig::Local(config),
                None,
                AiExecutionProfile::Posix,
                SavedConnectionStartOptions {
                    custom_name,
                    tab_color,
                    locked: tab.locked,
                    ..Default::default()
                },
                cx,
            );
            return true;
        }

        self.shell.set_status(format!(
            "restore skipped unsupported tab {} ({})",
            tab.title, tab.session_type
        ));
        self.shell.preserve_unread_terminal_layout();
        false
    }

    fn apply_restorable_workspace_pane_layout(&mut self, layout: RestorableWorkspacePaneNode) {
        let ordered = self
            .session
            .ordered_sessions()
            .into_iter()
            .map(|session| session.id)
            .collect::<Vec<_>>();
        if ordered.len() < 2 {
            return;
        }
        let Some(restored) = WorkspacePaneNode::restore_layout(&layout, &ordered) else {
            self.shell.preserve_unread_terminal_layout();
            return;
        };
        if !restored.is_split() {
            return;
        }
        // Key the tree by its first leaf so secondary leaves leave the strip.
        let Some(first) = restored.session_ids().into_iter().next() else {
            return;
        };
        // Avoid clobbering an existing distinct per-tab tree for the same root.
        if let Some(existing) = self.shell.workspace_pane_root(&first)
            && existing != &restored
        {
            // Prefer the newly restored tree from open_tabs for this root.
        }
        self.shell
            .insert_workspace_pane_root(first.clone(), restored);
        self.session.select_active_session_if_none(first);
        self.sync_workspace_split_from_active_tab();
        self.shell.set_workspace_pane_layout_restored(true);
        self.shell.show_workspace();
        self.shell
            .set_status("restored pane layout from open_tabs root".to_string());
    }
}

fn parse_restorable_tab_color(value: Option<&str>) -> Option<u32> {
    let raw = value?.trim().trim_start_matches('#');
    if raw.len() != 6 {
        return None;
    }
    u32::from_str_radix(raw, 16).ok()
}

#[cfg(test)]
mod tests {
    use crate::features::NyaTermApp;
    use crate::features::test_support::{
        app_with_visible_local_session, app_with_visible_ssh_session, session_focus_test_window,
    };
    use crate::models::{SessionLaunchConfig, SessionRuntimeMetadata, TabDockEdge, TabDockZone};
    use crate::test_support::{TestAppCleanup, TestConfigDir};
    use gpui::TestAppContext;
    use nyaterm_core::{
        AiExecutionProfile, RestorableOpenTab, RestorableTerminalWindowNode, SavedConnection,
        WorkspaceRestoreManifest, WorkspaceRestoreState,
    };
    use nyaterm_store::ConnectionStore;
    use nyaterm_transport::SshSessionConfig;

    fn register_restored_ssh(app: &mut NyaTermApp, id: &str, connection_id: &str) {
        let config = SshSessionConfig {
            name: id.to_string(),
            host: "example.test".to_string(),
            username: "tester".to_string(),
            ..SshSessionConfig::default()
        };
        app.register_session(
            id,
            SessionRuntimeMetadata {
                ssh_config: Some(config.clone()),
                ssh_multiplex_key: None,
                source_connection_id: Some(connection_id.to_string()),
                ai_execution_profile: AiExecutionProfile::Posix,
                launch_config: SessionLaunchConfig::Ssh(Box::new(config)),
                disconnected: true,
            },
        );
    }

    #[test]
    fn last_ssh_restore_completion_restores_persisted_split_after_restart() {
        let source_root = TestConfigDir::new("nyaterm-ssh-split-before-close");
        let restore_root = TestConfigDir::new("nyaterm-ssh-split-after-restart");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let source = app_with_visible_ssh_session(&mut cx, source_root.path(), "before-left");
        let workspace = source.update(&mut cx, |app, cx| {
            let mut settings = app.settings.summary().clone();
            settings.startup_restore = true;
            settings.startup_restore_window_layout = true;
            app.settings.replace_summary(settings);
            app.session.mark_restore_complete();
            app.shell.set_workspace_pane_layout_restored(true);
            app.shell.complete_terminal_windows_restore();
            app.session
                .metadata_mut("before-left")
                .unwrap()
                .source_connection_id = Some("saved-left".to_string());
            register_restored_ssh(app, "before-right", "saved-right");
            let group = app.shell.terminal_group_for_tab("before-left").unwrap();
            app.dock_tab_on_terminal_window_leaf(
                "before-right".to_string(),
                group,
                TabDockZone::Edge(TabDockEdge::Right),
                cx,
            );
            assert!(app.shell.terminal_windows_is_multi_leaf());
            let mut workspace = WorkspaceRestoreState::empty(app.workspace_id);
            app.capture_workspace_close_snapshot()
                .apply_to(&mut workspace);
            assert_eq!(workspace.sessions.open_tabs.len(), 2);
            assert!(matches!(
                workspace.sessions.terminal_window_layout,
                Some(RestorableTerminalWindowNode::Split { .. })
            ));
            workspace
        });

        let config_dir = restore_root.path().join("config");
        {
            let store = ConnectionStore::open(&config_dir).unwrap();
            let mut settings = store.load_app_settings_summary().unwrap();
            settings.startup_restore = true;
            settings.startup_restore_window_layout = true;
            store.save_general_settings(&settings).unwrap();
            store
                .save_workspace_restore_manifest(&WorkspaceRestoreManifest::single(workspace))
                .unwrap();
        }
        let saved = {
            let store = ConnectionStore::open(&config_dir).unwrap();
            let manifest = store.load_workspace_restore_manifest().unwrap();
            manifest.most_recent().unwrap().sessions.clone()
        };
        assert!(matches!(
            saved.terminal_window_layout,
            Some(RestorableTerminalWindowNode::Split { .. })
        ));

        let restored = app_with_visible_ssh_session(&mut cx, restore_root.path(), "after-left");
        restored.update(&mut cx, |app, cx| {
            app.session
                .metadata_mut("after-left")
                .unwrap()
                .source_connection_id = Some("saved-left".to_string());
            app.stores.startup_restore.update(cx, |store, _| {
                store.mark_open_tabs_restored();
                store.set_loaded_window_layouts(
                    saved.terminal_window_layout,
                    saved.workspace_pane_layout,
                );
                store.set_queue(saved.open_tabs);
                store.request_focus_handoff();
                assert_eq!(
                    store.pop_next_tab().unwrap().connection_id.as_deref(),
                    Some("saved-left")
                );
                assert_eq!(
                    store.pop_next_tab().unwrap().connection_id.as_deref(),
                    Some("saved-right")
                );
            });
            // Inject the final reconnect completion at the session-registration boundary.
            // SSH wire/authentication is independent of the persisted window layout.
            register_restored_ssh(app, "after-right", "saved-right");
            assert!(!app.session.restore_is_complete());
            assert!(app.stores.startup_restore.read(cx).queue_empty());
            assert!(!app.shell.terminal_windows_is_multi_leaf());
        });
        let cx = session_focus_test_window(&mut cx, &restored);
        cx.update(|window, _| window.activate_window());
        cx.run_until_parked();
        cx.update(|window, cx| {
            restored.update(cx, |app, cx| {
                let placement = app.session.start.allocate_tab_placement(2);
                assert!(
                    app.session
                        .start_reserve_saved_connection("preparing", placement)
                );
                assert!(app.session.start_is_busy());
                assert!(!app.session.start_has_pending());
                app.resume_startup_restore_if_unlocked(window, cx);
                app.pump_startup_restore_queue(window, cx);
                assert!(!app.pump_startup_restore_queue_if_ready(window, cx));
                assert!(!app.session.restore_is_complete());
                app.session.start_release_saved_connection("preparing");
                app.remote_desktop.restore_pending = Some("pending-remote-desktop".to_string());
                assert!(!app.pump_startup_restore_queue_if_ready(window, cx));
                assert!(!app.session.restore_is_complete());
                app.remote_desktop.restore_pending = None;
                assert!(app.pump_startup_restore_queue_if_ready(window, cx));
                assert!(app.session.restore_is_complete());
                assert!(app.shell.terminal_windows_restore_is_complete());
                app.reconcile_terminal_windows();
                assert!(app.shell.terminal_windows_is_multi_leaf());
                assert_ne!(
                    app.shell.terminal_group_for_tab("after-left"),
                    app.shell.terminal_group_for_tab("after-right")
                );
                assert!(!app.pump_startup_restore_queue_if_ready(window, cx));
                app.ensure_startup_restore_focus_handoff(window, cx);
            });
            let _ = window.draw(cx);
            window.simulate_next_frame(cx);
            assert!(restored.read(cx).terminal.input_focus().is_focused(window));
        });
    }

    #[test]
    fn startup_restore_skips_a_connection_changed_to_mstsc_without_creating_a_tab() {
        let root = TestConfigDir::new("nyaterm-mstsc-skip-restore");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, root.path(), "existing");
        let saved: SavedConnection = serde_json::from_value(serde_json::json!({
            "id":"mstsc", "name":"External RDP", "type":"rdp", "host":"example.test", "backend":"mstsc"
        })).unwrap();
        let tab: RestorableOpenTab = serde_json::from_value(serde_json::json!({
            "connection_id":"mstsc", "session_type":"RDP"
        }))
        .unwrap();
        let cx = session_focus_test_window(&mut cx, &app);
        cx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.connection_state.replace_loaded(vec![saved], Vec::new());
                let submitted = app.blocking_jobs.metrics().submitted;
                assert!(!app.start_restorable_open_tab(&tab, window, cx));
                assert_eq!(app.blocking_jobs.metrics().submitted, submitted);
                assert_eq!(app.session.session_order(), &["existing"]);
                assert!(!app.remote_desktop.has_sessions());
                assert!(!app.session.start_has_pending());
            });
        });
    }
}
