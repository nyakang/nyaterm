//! session catalog state and transitions.

use super::SessionFeatureState;
use super::start::SessionStartTabPlacement;
use crate::app_shell::session_hub::ssh_connections::SshConnectionLease;
use crate::features::session::notices::SessionNotices;
use crate::features::session::remote_files::RemoteFilesSession;
use crate::features::session::trzsz_runtime::TrzszSessionState;
use crate::features::session::zmodem_runtime::ZmodemSessionState;
use crate::models::{SessionLaunchConfig, SessionRuntimeMetadata};
use gpui::UniformListScrollHandle;
use nyaterm_core::AiExecutionProfile;
use nyaterm_files::file_browser::FileBrowserBackendKind;
use nyaterm_transport::{SessionEvent, SessionInfo, SessionKind, SessionManager, SshSessionConfig};
use std::collections::{HashSet, VecDeque};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SessionTabDragState {
    pub(super) source_id: String,
}

pub(crate) struct SessionCatalogTransferBundle {
    pub(super) entries: Vec<SessionCatalogTransferEntry>,
    pub(super) ordered_ids: Vec<String>,
    pub(super) pending_events: VecDeque<SessionEvent>,
}

pub(super) struct SessionCatalogTransferEntry {
    pub(super) session_id: String,
    pub(super) metadata: SessionRuntimeMetadata,
    pub(super) notices: Option<SessionNotices>,
    pub(super) start_tab_placement: Option<SessionStartTabPlacement>,
    pub(super) custom_name: Option<String>,
    pub(super) dynamic_title: Option<String>,
    pub(super) cwd: Option<String>,
    pub(super) tab_color: Option<u32>,
    pub(super) locked: bool,
    pub(super) command_history: Option<Vec<String>>,
    pub(super) busy_action: Option<String>,
    pub(super) remote_file: Option<RemoteFilesSession>,
    pub(super) ssh_connection: Option<SshConnectionLease>,
    pub(super) xymodem: Option<crate::features::session::xymodem_runtime::XymodemSessionState>,
    pub(super) zmodem: Option<ZmodemSessionState>,
    pub(super) trzsz: Option<TrzszSessionState>,
}

#[derive(Default)]
pub(super) struct ActiveSessionState {
    pub(super) id: Option<String>,
    /// Previously activated sessions, least recent first, never containing `id`.
    /// Closing the active tab hands over to the last live entry so focus returns
    /// to where the user came from instead of the leftmost tab.
    pub(super) history: Vec<String>,
}

pub(super) fn session_info_from_metadata(
    session_id: &str,
    metadata: &SessionRuntimeMetadata,
) -> SessionInfo {
    match &metadata.launch_config {
        SessionLaunchConfig::AdbShell(config)
        | SessionLaunchConfig::AdbFiles(config)
        | SessionLaunchConfig::AdbScreen(config) => SessionInfo {
            id: session_id.to_string(),
            name: match metadata.launch_config {
                SessionLaunchConfig::AdbShell(_) => {
                    std::borrow::Cow::Borrowed(config.name.as_str())
                }
                SessionLaunchConfig::AdbFiles(_) => {
                    rust_i18n::t!("android.filesTitle", name = config.name)
                }
                _ => rust_i18n::t!("android.screenTitle", name = config.name),
            }
            .into_owned(),
            kind: match metadata.launch_config {
                SessionLaunchConfig::AdbShell(_) => SessionKind::AdbShell,
                SessionLaunchConfig::AdbFiles(_) => SessionKind::AdbFiles,
                _ => SessionKind::AdbScreen,
            },
            working_dir: None,
            cols: config.cols,
            rows: config.rows,
        },
        SessionLaunchConfig::Local(config) => SessionInfo {
            id: session_id.to_string(),
            name: config.name.clone(),
            kind: SessionKind::LocalPty,
            working_dir: config.working_dir.clone(),
            cols: config.cols,
            rows: config.rows,
        },
        SessionLaunchConfig::Ssh(config) => SessionInfo {
            id: session_id.to_string(),
            name: config.name.clone(),
            kind: SessionKind::Ssh,
            working_dir: None,
            cols: config.cols,
            rows: config.rows,
        },
        SessionLaunchConfig::Telnet(config) => SessionInfo {
            id: session_id.to_string(),
            name: config.name.clone(),
            kind: if config.raw_tcp {
                SessionKind::RawTcp
            } else {
                SessionKind::Telnet
            },
            working_dir: None,
            cols: config.cols,
            rows: config.rows,
        },
        SessionLaunchConfig::Serial(config) => SessionInfo {
            id: session_id.to_string(),
            name: config.name.clone(),
            kind: SessionKind::Serial,
            working_dir: None,
            cols: 80,
            rows: 24,
        },
        SessionLaunchConfig::Rdp(config) => SessionInfo {
            id: session_id.to_string(),
            name: config.name.clone(),
            kind: SessionKind::Rdp,
            working_dir: None,
            cols: u16::try_from(config.display.width).unwrap_or(u16::MAX),
            rows: u16::try_from(config.display.height).unwrap_or(u16::MAX),
        },
        SessionLaunchConfig::Vnc(config) => SessionInfo {
            id: session_id.to_string(),
            name: config.name.clone(),
            kind: SessionKind::Vnc,
            working_dir: None,
            cols: 80,
            rows: 24,
        },
    }
}

pub(super) const COMMAND_HISTORY_LIMIT: usize = 128;
/// Safety net for the activation stack. Entries are deduplicated and pruned when
/// their session leaves the catalog, so the stack normally stays at or below the
/// live session count; this only bounds ids activated before registration.
pub(super) const ACTIVE_HISTORY_LIMIT: usize = 64;

impl SessionFeatureState {
    pub(crate) fn can_transfer_sessions(&self, session_ids: &[String]) -> Result<(), &'static str> {
        if self.start.has_pending() {
            return Err("wait for the session connection attempt to finish");
        }
        if self.prompts.has_blocking_prompt() {
            return Err("finish the active authentication or security prompt first");
        }
        if session_ids
            .iter()
            .any(|session_id| self.busy_actions.contains_key(session_id))
        {
            return Err("wait for the active session operation to finish");
        }
        if session_ids
            .iter()
            .any(|session_id| !self.metadata.contains_key(session_id))
        {
            return Err("the tab no longer exists");
        }
        Ok(())
    }
    pub(crate) fn pause_sessions_for_transfer(
        &self,
        session_ids: &[String],
    ) -> VecDeque<SessionEvent> {
        self.event_bridge.pause_sessions_for_transfer(session_ids)
    }
    pub(crate) fn resume_sessions_after_failed_transfer(
        &mut self,
        session_ids: &[String],
        bridge_events: VecDeque<SessionEvent>,
    ) {
        self.events.pending.extend(bridge_events);
        for session_id in session_ids {
            self.event_bridge.claim_session(session_id);
        }
    }
    pub(crate) fn detach_sessions_for_transfer(
        &mut self,
        session_ids: &[String],
        bridge_events: VecDeque<SessionEvent>,
    ) -> Option<SessionCatalogTransferBundle> {
        let selected = session_ids.iter().cloned().collect::<HashSet<_>>();
        if selected.is_empty() || selected.iter().any(|id| !self.metadata.contains_key(id)) {
            return None;
        }
        let ordered_ids = self
            .order
            .iter()
            .filter(|id| selected.contains(*id))
            .cloned()
            .collect::<Vec<_>>();
        let mut entries = Vec::with_capacity(session_ids.len());
        for session_id in session_ids {
            let metadata = self.metadata.remove(session_id)?;
            if let Some(files) = self.protocols.remote_files.get_mut(session_id) {
                files.cancel_pending();
            }
            entries.push(SessionCatalogTransferEntry {
                session_id: session_id.clone(),
                notices: self.notices.take(session_id),
                metadata,
                start_tab_placement: self
                    .start_tab_placements
                    .remove(session_id)
                    .map(|p| self.start.resolve_placement(p)),
                custom_name: self.custom_names.remove(session_id),
                dynamic_title: self.dynamic_titles.remove(session_id),
                cwd: self.cwds.remove(session_id),
                tab_color: self.tab_colors.remove(session_id),
                locked: self.locked_tabs.remove(session_id),
                command_history: self.command_history.remove(session_id),
                busy_action: self.busy_actions.remove(session_id),
                remote_file: self.protocols.remote_files.remove(session_id),
                ssh_connection: self.protocols.ssh_connections.remove(session_id),
                xymodem: self.protocols.xymodem.remove(session_id),
                zmodem: self.protocols.zmodem.remove(session_id),
                trzsz: self.protocols.trzsz.remove(session_id),
            });
        }
        self.order.retain(|id| !selected.contains(id));
        self.active.history.retain(|id| !selected.contains(id));
        if self
            .active
            .id
            .as_ref()
            .is_some_and(|id| selected.contains(id))
        {
            self.active.id = None;
        }
        let mut pending_events = VecDeque::new();
        let mut retained_events = VecDeque::new();
        while let Some(event) = self.events.pending.pop_front() {
            let session_id = match &event {
                SessionEvent::Output { session_id, .. }
                | SessionEvent::OutputBoundary { session_id, .. }
                | SessionEvent::OutputDropped { session_id, .. }
                | SessionEvent::CwdChanged { session_id, .. }
                | SessionEvent::CommandContext { session_id, .. }
                | SessionEvent::CommandAccepted { session_id, .. }
                | SessionEvent::Exited { session_id, .. }
                | SessionEvent::CleanupFailed { session_id, .. }
                | SessionEvent::TmuxState { session_id, .. }
                | SessionEvent::Notice { session_id, .. }
                | SessionEvent::Error { session_id, .. } => session_id,
            };
            if selected.contains(session_id) {
                pending_events.push_back(event);
            } else {
                retained_events.push_back(event);
            }
        }
        self.events.pending = retained_events;
        pending_events.extend(bridge_events);
        Some(SessionCatalogTransferBundle {
            entries,
            ordered_ids,
            pending_events,
        })
    }
    pub(crate) fn attach_sessions_from_transfer(
        &mut self,
        bundle: SessionCatalogTransferBundle,
        insert_index: Option<usize>,
    ) {
        let mut ordered_ids = bundle.ordered_ids;
        let insert_index = insert_index
            .unwrap_or(self.order.len())
            .min(self.order.len());
        for entry in bundle.entries {
            let session_id = entry.session_id;
            self.metadata.insert(session_id.clone(), entry.metadata);
            if let Some(notices) = entry.notices {
                self.notices.insert(session_id.clone(), notices);
            }
            if let Some(value) = entry.start_tab_placement {
                self.start_tab_placements.insert(session_id.clone(), value);
            }
            if let Some(value) = entry.custom_name {
                self.custom_names.insert(session_id.clone(), value);
            }
            if let Some(value) = entry.dynamic_title {
                self.dynamic_titles.insert(session_id.clone(), value);
            }
            if let Some(value) = entry.cwd {
                self.cwds.insert(session_id.clone(), value);
            }
            if let Some(value) = entry.tab_color {
                self.tab_colors.insert(session_id.clone(), value);
            }
            if entry.locked {
                self.locked_tabs.insert(session_id.clone());
            }
            if let Some(value) = entry.command_history {
                self.command_history.insert(session_id.clone(), value);
            }
            if let Some(value) = entry.busy_action {
                self.busy_actions.insert(session_id.clone(), value);
            }
            if let Some(value) = entry.remote_file {
                self.protocols
                    .remote_files
                    .insert(session_id.clone(), value);
            }
            if let Some(value) = entry.xymodem {
                self.protocols.xymodem.insert(session_id.clone(), value);
            }
            if let Some(value) = entry.zmodem {
                self.protocols.zmodem.insert(session_id.clone(), value);
            }
            if let Some(value) = entry.trzsz {
                self.protocols.trzsz.insert(session_id.clone(), value);
            }
            if let Some(connection) = entry.ssh_connection {
                self.protocols
                    .ssh_connections
                    .insert(session_id.clone(), connection);
            }
            // Transferred UI events and detector-held bytes precede any output
            // the destination bridge can receive after claiming this session.
            if let Some(state) = self.protocols.zmodem.get(&session_id)
                && let Some(consumer) = state.runtime_consumer()
            {
                self.event_bridge
                    .bind_protocol_consumer(&session_id, consumer);
                state.rebind_wake(self.event_bridge.protocol_wake());
            }
            self.event_bridge.hold_session_for_ui(&session_id);
            self.event_bridge.claim_session(&session_id);
        }
        ordered_ids.retain(|id| self.metadata.contains_key(id));
        self.order
            .splice(insert_index..insert_index, ordered_ids.iter().cloned());
        self.events.pending.extend(bundle.pending_events);
        if let Some(first) = ordered_ids.first() {
            self.select_active_session(first.clone());
        }
    }
    pub(in crate::features) fn manager_handle(&self) -> Arc<SessionManager> {
        Arc::clone(&self.manager)
    }
    pub(in crate::features) fn restore_is_complete(&self) -> bool {
        self.restore.is_complete()
    }
    pub(in crate::features) fn mark_restore_complete(&mut self) -> bool {
        self.restore.mark_complete()
    }
    pub(in crate::features) fn command_history_for(&self, session_id: &str) -> Option<&[String]> {
        self.command_history.get(session_id).map(Vec::as_slice)
    }
    pub(in crate::features) fn active_command_history_snapshot(&self) -> Vec<String> {
        self.active_id()
            .and_then(|session_id| self.command_history_for(session_id))
            .map(<[String]>::to_vec)
            .unwrap_or_default()
    }
    pub(in crate::features) fn active_command_history_entry(&self, index: usize) -> Option<String> {
        let session_id = self.active_id()?;
        self.command_history_for(session_id)?.get(index).cloned()
    }
    pub(in crate::features) fn record_command_history(&mut self, session_id: &str, command: &str) {
        let command = command.trim();
        if command.is_empty() {
            return;
        }
        let history = self
            .command_history
            .entry(session_id.to_string())
            .or_default();
        history.insert(0, command.to_string());
        history.truncate(COMMAND_HISTORY_LIMIT);
    }
    pub(in crate::features) fn migrate_command_history(&mut self, old_id: &str, new_id: &str) {
        if let Some(history) = self.command_history.remove(old_id) {
            self.command_history.insert(new_id.to_string(), history);
        }
    }
    pub(in crate::features) fn remove_command_from_all_history(&mut self, command: &str) {
        for history in self.command_history.values_mut() {
            history.retain(|entry| entry != command);
        }
    }
    pub(in crate::features) fn active_list_scroll(&self) -> &UniformListScrollHandle {
        &self.active_list_scroll
    }
    pub(in crate::features) fn active_search_draft(&self) -> &str {
        &self.active_search_draft
    }
    pub(in crate::features) fn set_active_search_draft(&mut self, draft: String) {
        self.active_search_draft = draft;
    }
    pub(in crate::features) fn busy_action(&self, session_id: &str) -> Option<&str> {
        self.busy_actions.get(session_id).map(String::as_str)
    }
    pub(in crate::features) fn session_is_busy(&self, session_id: &str) -> bool {
        self.busy_actions.contains_key(session_id)
    }
    pub(super) fn begin_busy_action(&mut self, session_id: String, action: &'static str) -> bool {
        if self.busy_actions.contains_key(&session_id) {
            return false;
        }
        self.busy_actions.insert(session_id, action.to_string());
        true
    }
    pub(in crate::features) fn begin_disconnect_action(&mut self, session_id: String) -> bool {
        self.begin_busy_action(session_id, "disconnect")
    }
    pub(in crate::features) fn begin_reconnect_action(&mut self, session_id: String) -> bool {
        self.begin_busy_action(session_id, "reconnect")
    }
    pub(in crate::features) fn finish_busy_action(&mut self, session_id: &str) {
        self.busy_actions.remove(session_id);
    }
    pub(in crate::features) fn retain_busy_actions_for_live_sessions(&mut self) {
        self.busy_actions
            .retain(|id, _| self.metadata.contains_key(id));
    }
    pub(in crate::features) fn active_id(&self) -> Option<&str> {
        self.active.id.as_deref()
    }
    pub(in crate::features) fn active_id_owned(&self) -> Option<String> {
        self.active.id.clone()
    }
    pub(in crate::features) fn active_ssh_config(&self) -> Option<&SshSessionConfig> {
        self.active
            .id
            .as_deref()
            .and_then(|session_id| self.metadata.get(session_id))
            .and_then(|metadata| metadata.ssh_config.as_ref())
    }
    /// A plain local shell: host tools run on this machine, no SSH involved.
    pub(in crate::features) fn is_local_pty(&self, session_id: &str) -> bool {
        self.metadata.get(session_id).is_some_and(|metadata| {
            metadata.ssh_config.is_none()
                && matches!(metadata.launch_config, SessionLaunchConfig::Local(_))
        })
    }
    /// The active session has a machine the host panels can inspect: an SSH
    /// connection, or this machine for a local shell.
    pub(in crate::features) fn active_has_inspectable_host(&self) -> bool {
        self.active_ssh_config().is_some() || self.active_is_local_pty()
    }
    /// Periodic probes may run: this machine for a live local shell, or an SSH
    /// profile that has not disabled remote stats.
    pub(in crate::features) fn active_remote_probes_enabled(&self) -> bool {
        self.active_is_local_pty()
            || self
                .active_ssh_config()
                .is_some_and(|config| config.remote_stats_enabled())
    }
    pub(in crate::features) fn active_is_local_pty(&self) -> bool {
        self.active
            .id
            .as_deref()
            .is_some_and(|id| self.is_local_pty(id) && !self.is_disconnected(id))
    }
    pub(in crate::features) fn active_ssh_config_owned(&self) -> Option<SshSessionConfig> {
        self.active_ssh_config().cloned()
    }
    pub(in crate::features) fn file_browser_backend_for_session(
        &self,
        session_id: &str,
    ) -> Option<FileBrowserBackendKind> {
        if self.is_disconnected(session_id) {
            return None;
        }
        self.file_browser_backend_support_for_session(session_id)
    }
    pub(in crate::features) fn file_browser_backend_support_for_session(
        &self,
        session_id: &str,
    ) -> Option<FileBrowserBackendKind> {
        match self.session_info(session_id)?.kind {
            SessionKind::AdbShell | SessionKind::AdbFiles | SessionKind::AdbScreen => {
                Some(FileBrowserBackendKind::Adb)
            }
            SessionKind::LocalPty => Some(FileBrowserBackendKind::Local),
            SessionKind::Ssh => self
                .metadata(session_id)
                .and_then(|metadata| metadata.ssh_config.as_ref())
                .filter(|config| config.remote_file_browser_enabled())
                .map(|_| FileBrowserBackendKind::Remote),
            _ => None,
        }
    }
    pub(in crate::features) fn active_file_browser_backend(
        &self,
    ) -> Option<FileBrowserBackendKind> {
        self.active_id()
            .and_then(|session_id| self.file_browser_backend_for_session(session_id))
    }
    pub(in crate::features) fn active_ai_execution_profile(&self) -> AiExecutionProfile {
        self.active
            .id
            .as_deref()
            .and_then(|session_id| self.metadata.get(session_id))
            .map(|metadata| metadata.ai_execution_profile)
            .unwrap_or(AiExecutionProfile::SendOnly)
    }
    pub(in crate::features) fn select_active_session(
        &mut self,
        session_id: impl Into<String>,
    ) -> Option<String> {
        let session_id = session_id.into();
        let previous = self.active.id.replace(session_id.clone());
        // Keep the stack free of the session that just became active, then record
        // the one it replaced as the newest fallback.
        self.active.history.retain(|id| *id != session_id);
        if let Some(previous) = previous.as_deref()
            && previous != session_id
        {
            self.active.history.push(previous.to_string());
            if self.active.history.len() > ACTIVE_HISTORY_LIMIT {
                let overflow = self.active.history.len() - ACTIVE_HISTORY_LIMIT;
                self.active.history.drain(..overflow);
            }
        }
        previous
    }
    pub(in crate::features) fn select_active_session_if_none(
        &mut self,
        session_id: impl Into<String>,
    ) -> bool {
        if self.active.id.is_some() {
            return false;
        }
        self.select_active_session(session_id);
        true
    }
    pub(in crate::features) fn clear_active_session(&mut self) -> Option<String> {
        self.active.id.take()
    }
    pub(in crate::features) fn session_order(&self) -> &[String] {
        &self.order
    }
    pub(in crate::features) fn session_order_len(&self) -> usize {
        self.order.len()
    }
    pub(in crate::features) fn session_index(&self, session_id: &str) -> Option<usize> {
        self.order.iter().position(|id| id == session_id)
    }
    pub(in crate::features) fn session_start_tab_placement(
        &self,
        session_id: &str,
    ) -> Option<SessionStartTabPlacement> {
        self.start_tab_placements
            .get(session_id)
            .copied()
            .map(|placement| self.start.resolve_placement(placement))
    }
    pub(in crate::features) fn set_workspace_tab_position(&mut self, key: &str, index: usize) {
        let placement = self
            .start_tab_placements
            .get(key)
            .copied()
            .unwrap_or_else(|| self.start.allocate_tab_placement(index));
        self.start.set_tab_position(placement, index);
        self.start_tab_placements.insert(key.to_string(), placement);
    }
    pub(in crate::features) fn workspace_tab_position(
        &self,
        key: &str,
        fallback_index: usize,
    ) -> (usize, u64) {
        self.session_start_tab_placement(key)
            .map(|placement| (placement.insert_index, placement.request_sequence))
            .unwrap_or((fallback_index, u64::MAX))
    }
    pub(in crate::features) fn metadata(
        &self,
        session_id: &str,
    ) -> Option<&SessionRuntimeMetadata> {
        self.metadata.get(session_id)
    }
    pub(in crate::features) fn metadata_mut(
        &mut self,
        session_id: &str,
    ) -> Option<&mut SessionRuntimeMetadata> {
        self.metadata.get_mut(session_id)
    }
    pub(in crate::features) fn has_session(&self, session_id: &str) -> bool {
        self.metadata.contains_key(session_id)
    }
    pub(in crate::features) fn session_ids(&self) -> impl Iterator<Item = &str> {
        self.metadata.keys().map(String::as_str)
    }
    pub(in crate::features) fn metadata_entries(
        &self,
    ) -> impl Iterator<Item = (&str, &SessionRuntimeMetadata)> {
        self.metadata
            .iter()
            .map(|(session_id, metadata)| (session_id.as_str(), metadata))
    }
    pub(in crate::features) fn register_session_metadata(
        &mut self,
        session_id: &str,
        metadata: SessionRuntimeMetadata,
    ) {
        self.register_session_metadata_for_start(session_id, metadata, None, None);
    }
    pub(in crate::features) fn prepare_session_metadata_for_start(
        &mut self,
        session_id: &str,
        metadata: SessionRuntimeMetadata,
        tab_placement: Option<SessionStartTabPlacement>,
        insert_index: Option<usize>,
    ) {
        if !self.order.iter().any(|id| id == session_id) {
            self.order.push(session_id.to_string());
        }
        self.metadata.insert(session_id.to_string(), metadata);
        if let Some(tab_placement) =
            tab_placement.map(|placement| self.start.resolve_placement(placement))
        {
            self.start_tab_placements
                .insert(session_id.to_string(), tab_placement);
            self.order.retain(|id| id != session_id);
            let target_key = (tab_placement.insert_index, tab_placement.request_sequence);
            let insert_index = self
                .order
                .iter()
                .enumerate()
                .find_map(|(index, existing_id)| {
                    if let Some(existing) = self.session_start_tab_placement(existing_id) {
                        ((existing.insert_index, existing.request_sequence) > target_key)
                            .then_some(index)
                    } else {
                        (index >= tab_placement.insert_index).then_some(index)
                    }
                })
                .unwrap_or(self.order.len());
            self.order.insert(insert_index, session_id.to_string());
        } else if let Some(insert_index) = insert_index {
            self.move_session_to_index(session_id, insert_index);
        }
    }
    pub(in crate::features) fn prepare_provisional_reconnect(
        &mut self,
        session_id: &str,
        metadata: SessionRuntimeMetadata,
    ) {
        self.metadata.insert(session_id.to_string(), metadata);
    }
    pub(in crate::features) fn replace_session_order_id(&mut self, old_id: &str, new_id: &str) {
        if let Some(id) = self.order.iter_mut().find(|id| id.as_str() == old_id) {
            *id = new_id.to_string();
        }
    }
    pub(in crate::features) fn move_session_after(
        &mut self,
        session_id: &str,
        after_session_id: &str,
    ) -> bool {
        if session_id == after_session_id {
            return false;
        }
        let Some(mut session_index) = self.session_index(session_id) else {
            return false;
        };
        let Some(mut after_index) = self.session_index(after_session_id) else {
            return false;
        };
        let session_id = self.order.remove(session_index);
        if session_index < after_index {
            after_index = after_index.saturating_sub(1);
        }
        session_index = (after_index + 1).min(self.order.len());
        self.order.insert(session_index, session_id);
        true
    }
    pub(in crate::features) fn move_session_to_index(
        &mut self,
        session_id: &str,
        index: usize,
    ) -> bool {
        let Some(current_index) = self.session_index(session_id) else {
            return false;
        };
        let session_id = self.order.remove(current_index);
        let index = index.min(self.order.len());
        self.order.insert(index, session_id);
        true
    }
    pub(in crate::features) fn move_session_group_relative(
        &mut self,
        source_ids: &[String],
        target_ids: &[String],
        insert_after: bool,
    ) -> bool {
        let source_ids = source_ids
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        let target_ids = target_ids
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        if source_ids.is_empty() || target_ids.is_empty() || !source_ids.is_disjoint(&target_ids) {
            return false;
        }
        let source_block = self
            .order
            .iter()
            .filter(|id| source_ids.contains(id.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        if source_block.is_empty() {
            return false;
        }
        let mut remaining = self
            .order
            .iter()
            .filter(|id| !source_ids.contains(id.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        let target_positions = remaining
            .iter()
            .enumerate()
            .filter_map(|(index, id)| target_ids.contains(id.as_str()).then_some(index))
            .collect::<Vec<_>>();
        let Some(first_target) = target_positions.first().copied() else {
            return false;
        };
        let insert_index = if insert_after {
            target_positions.last().copied().unwrap_or(first_target) + 1
        } else {
            first_target
        };
        remaining.splice(insert_index..insert_index, source_block);
        if remaining == self.order {
            return false;
        }
        self.order = remaining;
        true
    }
    pub(in crate::features) fn move_session_group_to_end(&mut self, source_ids: &[String]) -> bool {
        let source_ids = source_ids
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        let source_block = self
            .order
            .iter()
            .filter(|id| source_ids.contains(id.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        if source_block.is_empty() {
            return false;
        }
        let mut next = self
            .order
            .iter()
            .filter(|id| !source_ids.contains(id.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        next.extend(source_block);
        if next == self.order {
            return false;
        }
        self.order = next;
        true
    }
    pub(in crate::features) fn ordered_sessions(&self) -> Vec<SessionInfo> {
        let mut ordered = Vec::with_capacity(self.order.len());
        let mut seen = HashSet::with_capacity(self.order.len());
        for session_id in &self.order {
            if !seen.insert(session_id.as_str()) {
                continue;
            }
            if let Some(metadata) = self.metadata.get(session_id) {
                ordered.push(session_info_from_metadata(session_id, metadata));
            }
        }
        for (session_id, metadata) in &self.metadata {
            if seen.insert(session_id.as_str()) {
                ordered.push(session_info_from_metadata(session_id, metadata));
            }
        }
        ordered
    }
    pub(in crate::features) fn session_info(&self, session_id: &str) -> Option<SessionInfo> {
        self.metadata
            .get(session_id)
            .map(|metadata| session_info_from_metadata(session_id, metadata))
    }
    pub(in crate::features) fn display_name_by_info(&self, session: &SessionInfo) -> String {
        self.custom_name(&session.id)
            .filter(|name| !name.trim().is_empty())
            .or_else(|| {
                self.dynamic_title(&session.id)
                    .filter(|name| !name.trim().is_empty())
            })
            .unwrap_or(&session.name)
            .to_string()
    }
    pub(in crate::features) fn display_name(&self, session_id: &str) -> Option<String> {
        self.custom_name(session_id)
            .filter(|name| !name.trim().is_empty())
            .or_else(|| {
                self.dynamic_title(session_id)
                    .filter(|name| !name.trim().is_empty())
            })
            .map(ToOwned::to_owned)
            .or_else(|| self.session_info(session_id).map(|session| session.name))
    }
    pub(in crate::features) fn endpoint(&self, session_id: &str) -> Option<String> {
        let metadata = self.metadata(session_id)?;
        match &metadata.launch_config {
            SessionLaunchConfig::AdbShell(config)
            | SessionLaunchConfig::AdbFiles(config)
            | SessionLaunchConfig::AdbScreen(config) => Some(config.device.serial()),
            SessionLaunchConfig::Local(config) => {
                let shell = config
                    .shell_path
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or("system shell");
                Some(match &config.working_dir {
                    Some(dir) => format!("{shell} in {}", dir.display()),
                    None => shell.to_string(),
                })
            }
            SessionLaunchConfig::Ssh(config) => Some(format!(
                "{}@{}:{}",
                config.username, config.host, config.port
            )),
            SessionLaunchConfig::Telnet(config) => Some(format!("{}:{}", config.host, config.port)),
            SessionLaunchConfig::Serial(config) => Some(format!(
                "{} @ {} {}{}{}",
                config.port_name,
                config.baud_rate,
                config.data_bits,
                config.parity,
                config.stop_bits
            )),
            SessionLaunchConfig::Rdp(config) => Some(format!("{}:{}", config.host, config.port)),
            SessionLaunchConfig::Vnc(config) => Some(format!("{}:{}", config.host, config.port)),
        }
    }
    pub(in crate::features) fn ssh_host(&self, session_id: &str) -> Option<String> {
        let metadata = self.metadata(session_id)?;
        match &metadata.launch_config {
            SessionLaunchConfig::Ssh(config) if !config.host.trim().is_empty() => {
                Some(config.host.clone())
            }
            _ => None,
        }
    }
    pub(in crate::features) fn ssh_address(&self, session_id: &str) -> Option<String> {
        let metadata = self.metadata(session_id)?;
        match &metadata.launch_config {
            SessionLaunchConfig::Ssh(config)
                if !config.username.trim().is_empty() && !config.host.trim().is_empty() =>
            {
                Some(format!(
                    "ssh -p {} {}@{}",
                    config.port, config.username, config.host
                ))
            }
            _ => None,
        }
    }
    pub(in crate::features) fn is_disconnected(&self, session_id: &str) -> bool {
        self.metadata(session_id)
            .is_some_and(|metadata| metadata.disconnected)
    }
    /// Whether a session a configuration replacement could disturb is still
    /// running.
    ///
    /// Deliberately narrow, and only for configuration-replacement blockers
    /// (cloud-sync pulls and backup imports): a tab kept for reconnect
    /// (`disconnected`) holds no backend, and a local shell is spawned by the
    /// local runtime rather than from the connection catalog being rewritten,
    /// so neither one may stop the replacement. RDP/VNC have a separate runtime
    /// owner; its replacement predicate is authoritative for those protocols.
    pub(in crate::features) fn has_live_remote_sessions(&self) -> bool {
        self.metadata.values().any(|metadata| {
            !metadata.disconnected
                && !matches!(
                    metadata.launch_config,
                    crate::models::SessionLaunchConfig::Local(_)
                        | crate::models::SessionLaunchConfig::Rdp(_)
                        | crate::models::SessionLaunchConfig::Vnc(_)
                )
        })
    }
    pub(in crate::features) fn tab_tooltip_lines(&self, session_id: &str) -> Vec<String> {
        let mut lines = Vec::new();
        if let Some(endpoint) = self.endpoint(session_id) {
            lines.push(endpoint);
        }
        if let Some(address) = self.ssh_address(session_id)
            && lines.first().map(String::as_str) != Some(address.as_str())
        {
            lines.push(address);
        }
        if self.is_disconnected(session_id) {
            lines.push("Disconnected — press Enter to reconnect".to_string());
        }
        if let Some(cwd) = self.cwd(session_id)
            && !cwd.trim().is_empty()
        {
            lines.push(format!("cwd {cwd}"));
        }
        lines
    }
    pub(in crate::features) fn live_session_count(&self) -> usize {
        self.metadata
            .values()
            .filter(|metadata| !metadata.disconnected)
            .count()
    }
    /// Who takes over when `session_id` closes: the most recently active surviving
    /// session first, then tab order, then any survivor at all.
    pub(in crate::features) fn next_session_after(&self, session_id: &str) -> Option<String> {
        self.next_live_session_excluding(Some(session_id))
    }
    /// Same handover policy as [`Self::next_session_after`] when the closed session
    /// is already gone from the catalog and there is nothing left to skip.
    pub(in crate::features) fn next_live_session(&self) -> Option<String> {
        self.next_live_session_excluding(None)
    }
    pub(super) fn next_live_session_excluding(&self, skip: Option<&str>) -> Option<String> {
        let known_ids = self.session_ids().collect::<HashSet<_>>();
        self.active
            .history
            .iter()
            .rev()
            .chain(self.order.iter())
            .map(String::as_str)
            .chain(known_ids.iter().copied())
            .find(|candidate| Some(*candidate) != skip && known_ids.contains(*candidate))
            .map(ToOwned::to_owned)
    }
    /// Inverse for sessions revived in place: a tmux pane session adopted
    /// again when the control client reattaches. Returns whether it had been
    /// marked disconnected — the banner text stays in the grid until the next
    /// capture-pane seed repaints it, which is what the caller drives next.
    pub(in crate::features) fn clear_session_disconnected(&mut self, session_id: &str) -> bool {
        let Some(metadata) = self.metadata.get_mut(session_id) else {
            return false;
        };
        let was_disconnected = metadata.disconnected;
        metadata.disconnected = false;
        if was_disconnected {
            self.notices.clear(session_id);
        }
        was_disconnected
    }
    pub(in crate::features) fn custom_name(&self, session_id: &str) -> Option<&str> {
        self.custom_names.get(session_id).map(String::as_str)
    }
    pub(in crate::features) fn set_custom_name(&mut self, session_id: String, name: String) {
        self.custom_names.insert(session_id, name);
    }
    pub(in crate::features) fn dynamic_title(&self, session_id: &str) -> Option<&str> {
        self.dynamic_titles.get(session_id).map(String::as_str)
    }
    pub(in crate::features) fn set_dynamic_title(
        &mut self,
        session_id: &str,
        title: Option<String>,
    ) {
        if let Some(title) = title {
            self.dynamic_titles.insert(session_id.to_string(), title);
        } else {
            self.dynamic_titles.remove(session_id);
        }
    }
    pub(in crate::features) fn cwd(&self, session_id: &str) -> Option<&str> {
        self.cwds.get(session_id).map(String::as_str)
    }
    pub(in crate::features) fn clear_cwd(&mut self, session_id: &str) {
        self.cwds.remove(session_id);
    }
    pub(in crate::features) fn update_cwd(&mut self, session_id: &str, cwd: String) -> bool {
        let changed = self.cwds.get(session_id) != Some(&cwd);
        self.cwds.insert(session_id.to_string(), cwd);
        changed
    }
    pub(in crate::features) fn tab_color(&self, session_id: &str) -> Option<u32> {
        self.tab_colors.get(session_id).copied()
    }
    pub(in crate::features) fn set_tab_color(&mut self, session_id: &str, color: Option<u32>) {
        if let Some(color) = color {
            self.tab_colors.insert(session_id.to_string(), color);
        } else {
            self.tab_colors.remove(session_id);
        }
    }
    pub(in crate::features) fn tab_is_locked(&self, session_id: &str) -> bool {
        self.locked_tabs.contains(session_id)
    }
    pub(in crate::features) fn set_tab_locked(&mut self, session_id: &str, locked: bool) -> bool {
        if locked {
            self.locked_tabs.insert(session_id.to_string())
        } else {
            self.locked_tabs.remove(session_id)
        }
    }
    pub(in crate::features) fn set_tab_drag_source(&mut self, source_id: String) -> bool {
        let next = SessionTabDragState { source_id };
        if self.tab_drag.as_ref() == Some(&next) {
            return false;
        }
        self.tab_drag = Some(next);
        true
    }
    pub(in crate::features) fn tab_drag_source_is(&self, session_id: &str) -> bool {
        self.tab_drag
            .as_ref()
            .is_some_and(|drag| drag.source_id == session_id)
    }
    pub(in crate::features) fn tab_drag_is_pending(&self) -> bool {
        self.tab_drag.is_some()
    }
    pub(in crate::features) fn clear_tab_drag(&mut self) -> bool {
        self.tab_drag.take().is_some()
    }
    pub(in crate::features) fn migrate_session_presentation(&mut self, old_id: &str, new_id: &str) {
        if !self.custom_names.contains_key(new_id)
            && let Some(custom_name) = self.custom_names.remove(old_id)
        {
            self.custom_names.insert(new_id.to_string(), custom_name);
        }
        if let Some(title) = self.dynamic_titles.remove(old_id) {
            self.dynamic_titles.insert(new_id.to_string(), title);
        }
        if let Some(cwd) = self.cwds.remove(old_id) {
            self.cwds.insert(new_id.to_string(), cwd);
        }
        if !self.tab_colors.contains_key(new_id)
            && let Some(color) = self.tab_colors.remove(old_id)
        {
            self.tab_colors.insert(new_id.to_string(), color);
        }
        if !self.locked_tabs.contains(new_id) && self.locked_tabs.remove(old_id) {
            self.locked_tabs.insert(new_id.to_string());
        }
        self.migrate_command_history(old_id, new_id);
    }
    pub(in crate::features) fn migrate_tab_root_presentation(
        &mut self,
        old_root: &str,
        new_root: &str,
    ) {
        if let Some(custom_name) = self.custom_names.remove(old_root) {
            self.custom_names.insert(new_root.to_string(), custom_name);
        }
        if let Some(color) = self.tab_colors.remove(old_root) {
            self.tab_colors.insert(new_root.to_string(), color);
        }
        if self.locked_tabs.remove(old_root) {
            self.locked_tabs.insert(new_root.to_string());
        }
    }
}
