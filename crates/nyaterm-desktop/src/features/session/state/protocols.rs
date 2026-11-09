//! session protocols state and transitions.

use super::{SessionDisconnectUpdate, SessionFeatureState};
use crate::app_shell::session_hub::ssh_connections::SshConnectionLease;
use crate::features::session::trzsz_runtime::TrzszSessionState;
use crate::features::session::zmodem_runtime::ZmodemSessionState;
use nyaterm_files::remote_file::{RemoteFileBackendPreferenceStore, RemoteFileService};
use nyaterm_transport::{SessionEvent, SshMultiplexHandle, SshSessionConfig};
use std::collections::VecDeque;

impl SessionFeatureState {
    pub(in crate::features::session) fn start_zmodem_runtime(
        &mut self,
        session_id: &str,
        transfer: nyaterm_transport::ZmodemTransfer,
    ) -> std::io::Result<nyaterm_transport::zmodem::runtime::ZmodemRuntime> {
        let worker = nyaterm_transport::zmodem::runtime::ZmodemRuntime::spawn(
            self.manager_handle(),
            session_id.into(),
            transfer,
        )?;
        worker.set_wake(self.event_bridge.protocol_wake());
        let bridge_events = self
            .event_bridge
            .begin_protocol_handoff(session_id, worker.consumer_id());
        let mut bootstrap = VecDeque::new();
        let mut retained = VecDeque::new();
        while let Some(event) = self.events.pending.pop_front() {
            let id = match &event {
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
            if id == session_id {
                bootstrap.push_back(event);
            } else {
                retained.push_back(event);
            }
        }
        self.events.pending = retained;
        bootstrap.extend(bridge_events);
        worker.activate(bootstrap);
        Ok(worker)
    }
    pub(in crate::features::session) fn finish_zmodem_runtime(
        &mut self,
        session_id: &str,
        pending: VecDeque<SessionEvent>,
    ) {
        self.events.pending.extend(pending);
        self.event_bridge.finish_protocol_handoff(session_id);
    }
    pub(in crate::features) fn remote_file_service_for_session(
        &mut self,
        session_id: &str,
        config: SshSessionConfig,
        preference_store: std::sync::Arc<dyn RemoteFileBackendPreferenceStore>,
    ) -> anyhow::Result<RemoteFileService> {
        let has_multiplex_key = self
            .metadata(session_id)
            .and_then(|metadata| metadata.ssh_multiplex_key.as_ref())
            .is_some();
        let multiplex = self.ssh_multiplex_handle_for_session(session_id);
        if has_multiplex_key && multiplex.is_none() {
            self.protocols.remote_files.remove(session_id);
            anyhow::bail!("reconnect the SSH session before browsing remote files");
        }
        if let Some(service) = self.protocols.remote_files.get(session_id) {
            return Ok(service.as_ref().clone());
        }
        let service =
            RemoteFileService::with_preference_store(config, multiplex, preference_store)?;
        self.protocols
            .remote_files
            .insert(session_id.to_string(), std::sync::Arc::new(service.clone()));
        Ok(service)
    }
    pub(in crate::features) fn weak_remote_file_service(
        &self,
        session_id: &str,
    ) -> Option<std::sync::Weak<RemoteFileService>> {
        self.protocols
            .remote_files
            .get(session_id)
            .map(std::sync::Arc::downgrade)
    }
    pub(in crate::features) fn remove_remote_file_service(&mut self, session_id: &str) -> bool {
        self.protocols.remote_files.remove(session_id).is_some()
    }
    pub(in crate::features) fn ssh_multiplex_handle_for_session(
        &mut self,
        session_id: &str,
    ) -> Option<SshMultiplexHandle> {
        self.ssh_connection_for_session(session_id)
            .map(|connection| connection.handle())
    }
    pub(in crate::features) fn ssh_connection_for_session(
        &self,
        session_id: &str,
    ) -> Option<SshConnectionLease> {
        let connection = self.protocols.ssh_connections.get(session_id)?;
        if connection.is_closed() {
            return None;
        }
        let key = self.metadata(session_id)?.ssh_multiplex_key.as_deref()?;
        self.ssh_connections.get(key)
    }
    pub(in crate::features) fn has_protocol_runtime_sessions(&self) -> bool {
        !self.protocols.xymodem.is_empty()
            || !self.protocols.zmodem.is_empty()
            || !self.protocols.trzsz.is_empty()
    }
    pub(in crate::features::session) fn xymodem_state(
        &self,
        session_id: &str,
    ) -> Option<&crate::features::session::xymodem_runtime::XymodemSessionState> {
        self.protocols.xymodem.get(session_id)
    }
    pub(in crate::features::session) fn xymodem_states(
        &self,
    ) -> impl Iterator<
        Item = (
            &String,
            &crate::features::session::xymodem_runtime::XymodemSessionState,
        ),
    > {
        self.protocols.xymodem.iter()
    }
    pub(in crate::features::session) fn insert_xymodem_state(
        &mut self,
        session_id: String,
        state: crate::features::session::xymodem_runtime::XymodemSessionState,
    ) {
        self.protocols.xymodem.insert(session_id, state);
    }
    pub(in crate::features::session) fn remove_xymodem_session_runtime(
        &mut self,
        session_id: &str,
    ) -> bool {
        let Some(mut state) = self.protocols.xymodem.remove(session_id) else {
            return false;
        };
        state.stop_worker();
        self.event_bridge.finish_protocol_handoff(session_id);
        true
    }
    pub(in crate::features::session) fn has_zmodem_runtime_sessions(&self) -> bool {
        !self.protocols.zmodem.is_empty()
    }
    pub(in crate::features::session) fn has_trzsz_runtime_sessions(&self) -> bool {
        !self.protocols.trzsz.is_empty()
    }
    #[cfg(test)]
    pub(super) fn protocol_runtime_counts(&self) -> (usize, usize, usize) {
        (
            self.protocols.zmodem.len(),
            self.protocols.trzsz.len(),
            self.protocols.ssh_connections.len(),
        )
    }
    pub(in crate::features::session) fn zmodem_state(
        &self,
        session_id: &str,
    ) -> Option<&ZmodemSessionState> {
        self.protocols.zmodem.get(session_id)
    }
    pub(in crate::features::session) fn zmodem_state_mut(
        &mut self,
        session_id: &str,
    ) -> Option<&mut ZmodemSessionState> {
        self.protocols.zmodem.get_mut(session_id)
    }
    pub(in crate::features::session) fn zmodem_state_mut_or_default(
        &mut self,
        session_id: &str,
    ) -> &mut ZmodemSessionState {
        self.protocols
            .zmodem
            .entry(session_id.to_string())
            .or_default()
    }
    pub(in crate::features::session) fn zmodem_states_mut(
        &mut self,
    ) -> impl Iterator<Item = (&String, &mut ZmodemSessionState)> {
        self.protocols.zmodem.iter_mut()
    }
    pub(in crate::features::session) fn remove_zmodem_session_runtime(
        &mut self,
        session_id: &str,
    ) -> bool {
        let Some(mut state) = self.protocols.zmodem.remove(session_id) else {
            return false;
        };
        state.stop_worker();
        true
    }
    pub(in crate::features::session) fn trzsz_state(
        &self,
        session_id: &str,
    ) -> Option<&TrzszSessionState> {
        self.protocols.trzsz.get(session_id)
    }
    pub(in crate::features::session) fn trzsz_state_mut(
        &mut self,
        session_id: &str,
    ) -> Option<&mut TrzszSessionState> {
        self.protocols.trzsz.get_mut(session_id)
    }
    pub(in crate::features::session) fn trzsz_state_mut_or_default(
        &mut self,
        session_id: &str,
    ) -> &mut TrzszSessionState {
        self.protocols
            .trzsz
            .entry(session_id.to_string())
            .or_default()
    }
    pub(in crate::features::session) fn trzsz_states_mut(
        &mut self,
    ) -> impl Iterator<Item = (&String, &mut TrzszSessionState)> {
        self.protocols.trzsz.iter_mut()
    }
    pub(in crate::features::session) fn remove_trzsz_session_runtime(
        &mut self,
        session_id: &str,
    ) -> bool {
        let Some(mut state) = self.protocols.trzsz.remove(session_id) else {
            return false;
        };
        state.stop_workers();
        true
    }
    pub(in crate::features) fn register_ssh_connection(
        &mut self,
        session_id: String,
        connection: SshConnectionLease,
    ) {
        self.protocols
            .ssh_connections
            .insert(session_id, connection);
    }
    pub(in crate::features) fn mark_session_disconnected(
        &mut self,
        session_id: &str,
    ) -> Option<SessionDisconnectUpdate> {
        let metadata = self.metadata.get_mut(session_id)?;
        let already_disconnected = metadata.disconnected;
        metadata.disconnected = true;
        self.protocols.ssh_connections.remove(session_id);
        Some(SessionDisconnectUpdate {
            already_disconnected,
        })
    }
    pub(in crate::features) fn remove_session_catalog(
        &mut self,
        session_id: &str,
    ) -> Option<String> {
        self.notices.clear(session_id);
        self.event_bridge.release_session(session_id);
        self.remove_xymodem_session_runtime(session_id);
        self.remove_zmodem_session_runtime(session_id);
        self.remove_trzsz_session_runtime(session_id);
        self.remove_remote_file_service(session_id);
        self.protocols.ssh_connections.remove(session_id);
        self.order.retain(|id| id != session_id);
        self.active.history.retain(|id| id != session_id);
        self.start_tab_placements.remove(session_id);
        let multiplex_key = self
            .metadata
            .remove(session_id)
            .and_then(|metadata| metadata.ssh_multiplex_key);
        self.custom_names.remove(session_id);
        self.dynamic_titles.remove(session_id);
        self.cwds.remove(session_id);
        self.tab_colors.remove(session_id);
        self.locked_tabs.remove(session_id);
        if self
            .tab_drag
            .as_ref()
            .is_some_and(|drag| drag.source_id == session_id)
        {
            self.tab_drag = None;
        }
        self.command_history.remove(session_id);
        self.busy_actions.remove(session_id);
        if self.active_id() == Some(session_id) {
            self.clear_active_session();
        }
        multiplex_key
    }
}
