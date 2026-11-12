//! session start state and transitions.

use super::SessionFeatureState;
use crate::features::runtime_jobs::SessionStartResult;
use crate::models::{SessionLaunchConfig, StartupCommandRequest, WorkspaceSplitDirection};
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use nyaterm_core::models::android::AndroidViewKind;
use nyaterm_core::{AiExecutionProfile, ConnectionType, SavedConnection, uuid};
use nyaterm_transport::SessionKind;
use std::collections::{HashMap, HashSet};
use std::time::Instant;

pub(in crate::features) struct PendingSessionStart {
    /// Preparation has no worker result to reap when its callback is cancelled.
    pub preparing: bool,
    pub(in crate::features) mcp_open_operation_id: Option<String>,
    /// Retains an external override before a saved connection's config is built.
    pub requested_ssh_runtime_mode: Option<nyaterm_core::models::ssh_runtime::SshRuntimeMode>,
    pub attempt: nyaterm_transport::connection_attempt::ConnectionAttempt,
    pub connection_name: String,
    pub launch_config: Option<SessionLaunchConfig>,
    pub requested_at: Instant,
    pub kind: SessionKind,
    pub ai_execution_profile: AiExecutionProfile,
    pub custom_name: Option<String>,
    pub tab_color: Option<u32>,
    pub locked: bool,
    pub after_session_id: Option<String>,
    pub insert_index: Option<usize>,
    pub seed_output: Option<String>,
    pub startup_command: Option<StartupCommandRequest>,
    pub multiplex_key: Option<String>,
    pub source_connection_id: Option<String>,
    pub workspace_split: Option<(WorkspaceSplitDirection, String)>,
    pub tab_placement: Option<SessionStartTabPlacement>,
    /// Existing pane being replaced by this request, when this is a reconnect.
    pub reconnect_session_id: Option<String>,
}

impl PendingSessionStart {
    pub(in crate::features) fn android_view_kind(&self) -> Option<AndroidViewKind> {
        match self.kind {
            SessionKind::AdbShell => Some(AndroidViewKind::Shell),
            SessionKind::AdbFiles => Some(AndroidViewKind::Files),
            SessionKind::AdbScreen => Some(AndroidViewKind::Screen),
            _ => None,
        }
    }

    pub(in crate::features) fn ssh_runtime_mode(
        &self,
    ) -> Option<nyaterm_core::models::ssh_runtime::SshRuntimeMode> {
        self.requested_ssh_runtime_mode.or_else(|| {
            self.launch_config.as_ref().and_then(|config| match config {
                SessionLaunchConfig::Ssh(config) => Some(config.effective_runtime_mode()),
                _ => None,
            })
        })
    }
}

pub(in crate::features) struct PendingReconnectCompletion {
    pub(in crate::features) mcp_open_operation_id: Option<String>,
    pub old_id: String,
    pub new_id: String,
    pub connection_name: String,
    pub kind: SessionKind,
    pub source_connection_id: Option<String>,
    pub workspace_split: Option<(WorkspaceSplitDirection, String)>,
    pub startup_command: Option<StartupCommandRequest>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::features) enum SessionStartEndReason {
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::features) struct StartTabActionSnapshot {
    pub request_id: String,
    pub reason: Option<SessionStartEndReason>,
}

/// A failed or cancelled start retains its metadata in the original tab for retry.
pub(in crate::features) struct FailedSessionStart {
    pub pending: PendingSessionStart,
    pub error: String,
    pub reason: SessionStartEndReason,
}

#[derive(Clone, Copy, Debug)]
pub(in crate::features) struct SessionStartTabPlacement {
    pub(in crate::features) insert_index: usize,
    pub(in crate::features) request_sequence: u64,
    pub(in crate::features) requested_at: Instant,
}

pub(in crate::features::session) struct SessionStartFeatureState {
    pub(super) tx: UnboundedSender<SessionStartResult>,
    /// Taken once by `NyaTermApp::start_session_start_event_drain`, which owns
    /// delivery from then on. `None` afterwards, so a second start is a no-op.
    pub(super) rx: Option<UnboundedReceiver<SessionStartResult>>,
    pub(super) pending: HashMap<String, PendingSessionStart>,
    pub(super) active_pending: Option<String>,
    pub(super) failed: HashMap<String, FailedSessionStart>,
    pub(super) active_failed: Option<String>,
    pub(super) cancelled: HashSet<String>,
    pub(super) reconnect_failures: HashMap<String, String>,
    pub(super) reconnect_completions: HashMap<String, PendingReconnectCompletion>,
    pub(super) pending_workspace_split: Option<(WorkspaceSplitDirection, String)>,
    pub(super) next_request_sequence: u64,
    // Positions are resolved by sequence, including in callbacks holding an old
    // options snapshot. A drag must survive credential loading and worker completion.
    // Allocation time disambiguates completed tabs imported from another
    // workspace, whose local request sequences can overlap ours.
    pub(super) placements: HashMap<(u64, Instant), SessionStartTabPlacement>,
}

pub(in crate::features) enum SessionStartEventRequest {
    Cancelled,
    Pending {
        pending: Option<Box<PendingSessionStart>>,
        was_active: bool,
    },
}

#[derive(Clone, Default)]
pub(in crate::features) struct SavedConnectionStartOptions {
    pub android_view: Option<nyaterm_core::models::android::AndroidViewKind>,
    pub(in crate::features) mcp_open_operation_id: Option<String>,
    pub ssh_runtime_mode: Option<nyaterm_core::models::ssh_runtime::SshRuntimeMode>,
    pub custom_name: Option<String>,
    pub tab_color: Option<u32>,
    pub locked: bool,
    pub after_session_id: Option<String>,
    pub insert_index: Option<usize>,
    pub seed_output: Option<String>,
    pub startup_command: Option<StartupCommandRequest>,
    pub reconnect_session_id: Option<String>,
    pub workspace_split: Option<(WorkspaceSplitDirection, String)>,
    pub tab_placement: Option<SessionStartTabPlacement>,
    pub request_id: Option<String>,
}

impl SessionStartFeatureState {
    pub(in crate::features) fn new() -> Self {
        let (tx, rx) = unbounded();
        Self {
            tx,
            rx: Some(rx),
            pending: HashMap::new(),
            active_pending: None,
            failed: HashMap::new(),
            active_failed: None,
            cancelled: HashSet::new(),
            reconnect_failures: HashMap::new(),
            reconnect_completions: HashMap::new(),
            pending_workspace_split: None,
            next_request_sequence: 0,
            placements: HashMap::new(),
        }
    }

    pub(in crate::features) fn sender(&self) -> UnboundedSender<SessionStartResult> {
        self.tx.clone()
    }

    pub(in crate::features) fn take_event_receiver(
        &mut self,
    ) -> Option<UnboundedReceiver<SessionStartResult>> {
        self.rx.take()
    }

    pub(in crate::features) fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    pub(in crate::features) fn is_busy(&self) -> bool {
        !self.pending.is_empty()
    }

    pub(in crate::features) fn has_failed(&self) -> bool {
        !self.failed.is_empty()
    }

    #[cfg(test)]
    pub(in crate::features) fn pending_count(&self) -> usize {
        self.pending.len()
    }

    pub(in crate::features) fn visible_tab_reservation_count(&self) -> usize {
        self.pending
            .values()
            .filter(|pending| pending.reconnect_session_id.is_none())
            .count()
            .saturating_add(self.failed.len())
    }

    pub(in crate::features) fn allocate_tab_placement(
        &mut self,
        insert_index: usize,
    ) -> SessionStartTabPlacement {
        let placement = SessionStartTabPlacement {
            insert_index,
            request_sequence: self.next_request_sequence,
            requested_at: Instant::now(),
        };
        self.next_request_sequence = self.next_request_sequence.saturating_add(1);
        self.placements.insert(
            (placement.request_sequence, placement.requested_at),
            placement,
        );
        placement
    }

    pub(in crate::features) fn resolve_placement(
        &self,
        placement: SessionStartTabPlacement,
    ) -> SessionStartTabPlacement {
        self.placements
            .get(&(placement.request_sequence, placement.requested_at))
            .copied()
            .unwrap_or(placement)
    }

    pub(in crate::features) fn set_tab_position(
        &mut self,
        placement: SessionStartTabPlacement,
        index: usize,
    ) {
        self.placements.insert(
            (placement.request_sequence, placement.requested_at),
            SessionStartTabPlacement {
                insert_index: index,
                ..placement
            },
        );
    }

    pub(in crate::features) fn request_can_start(
        &self,
        options: &SavedConnectionStartOptions,
    ) -> bool {
        options.request_id.as_ref().is_none_or(|id| {
            self.pending
                .get(id)
                .is_some_and(|pending| pending.preparing)
        })
    }

    pub(in crate::features) fn begin_preparation(
        &mut self,
        request_id: String,
        mut pending: PendingSessionStart,
    ) {
        pending.preparing = true;
        self.register_pending(request_id, pending);
    }

    pub(in crate::features) fn begin_saved_preparation(
        &mut self,
        connection: &SavedConnection,
        options: &SavedConnectionStartOptions,
    ) -> String {
        if let Some(id) = &options.request_id {
            return id.clone();
        }
        let (kind, ai_execution_profile) = match &connection.config {
            ConnectionType::LocalTerminal {
                ai_execution_profile,
                ..
            } => (SessionKind::LocalPty, *ai_execution_profile),
            ConnectionType::Ssh {
                ai_execution_profile,
                ..
            } => (SessionKind::Ssh, *ai_execution_profile),
            ConnectionType::Telnet {
                ai_execution_profile,
                raw_tcp_cli,
                ..
            } => (
                if *raw_tcp_cli {
                    SessionKind::RawTcp
                } else {
                    SessionKind::Telnet
                },
                *ai_execution_profile,
            ),
            ConnectionType::Serial {
                ai_execution_profile,
                ..
            } => (SessionKind::Serial, *ai_execution_profile),
            ConnectionType::Adb { .. } => {
                match options.android_view.unwrap_or(AndroidViewKind::Shell) {
                    AndroidViewKind::Shell => (SessionKind::AdbShell, AiExecutionProfile::SendOnly),
                    AndroidViewKind::Files => (SessionKind::AdbFiles, AiExecutionProfile::Disabled),
                    AndroidViewKind::Screen => {
                        (SessionKind::AdbScreen, AiExecutionProfile::Disabled)
                    }
                }
            }
            ConnectionType::Rdp { .. } => (SessionKind::Rdp, AiExecutionProfile::Disabled),
            ConnectionType::Vnc { .. } => (SessionKind::Vnc, AiExecutionProfile::Disabled),
        };
        let id = uuid();
        self.begin_preparation(
            id.clone(),
            PendingSessionStart {
                preparing: true,
                mcp_open_operation_id: options.mcp_open_operation_id.clone(),
                requested_ssh_runtime_mode: options.ssh_runtime_mode,
                attempt: Default::default(),
                connection_name: connection.name.clone(),
                launch_config: None,
                requested_at: options
                    .tab_placement
                    .map(|p| p.requested_at)
                    .unwrap_or_else(Instant::now),
                kind,
                ai_execution_profile,
                custom_name: options.custom_name.clone(),
                tab_color: options.tab_color,
                locked: options.locked,
                after_session_id: options.after_session_id.clone(),
                insert_index: options.insert_index,
                seed_output: options.seed_output.clone(),
                startup_command: options.startup_command.clone(),
                multiplex_key: None,
                source_connection_id: Some(connection.id.clone()),
                workspace_split: options.workspace_split.clone(),
                tab_placement: options.tab_placement,
                reconnect_session_id: options.reconnect_session_id.clone(),
            },
        );
        id
    }

    pub(in crate::features) fn active_failed_request_id(&self) -> Option<&str> {
        self.active_failed.as_deref()
    }

    pub(in crate::features) fn failed(&self, request_id: &str) -> Option<&FailedSessionStart> {
        self.failed.get(request_id)
    }

    pub(in crate::features) fn set_failed_error(&mut self, request_id: &str, error: String) {
        if let Some(failed) = self.failed.get_mut(request_id) {
            failed.error = error;
        }
    }

    pub(in crate::features) fn take_failed_for_retry(
        &mut self,
        request_id: &str,
    ) -> Option<FailedSessionStart> {
        let failed = self.failed.remove(request_id)?;
        if self.active_failed.as_deref() == Some(request_id) {
            self.active_failed = None;
        }
        Some(failed)
    }

    #[cfg(test)]
    pub(in crate::features) fn has_cancelled_results(&self) -> bool {
        !self.cancelled.is_empty()
    }

    pub(in crate::features) fn has_active_pending(&self) -> bool {
        self.active_pending.is_some()
    }

    pub(in crate::features) fn has_active_failed(&self) -> bool {
        self.active_failed.is_some()
    }

    pub(in crate::features) fn request_is_active(&self, request_id: &str) -> bool {
        self.active_pending.as_deref() == Some(request_id)
            || self.active_failed.as_deref() == Some(request_id)
    }

    pub(in crate::features) fn pending_entries(
        &self,
    ) -> impl Iterator<Item = (&String, &PendingSessionStart)> {
        self.pending.iter()
    }

    pub(in crate::features) fn failed_entries(
        &self,
    ) -> impl Iterator<Item = (&String, &FailedSessionStart)> {
        self.failed.iter()
    }

    pub(in crate::features) fn source_connection_is_pending(&self, connection_id: &str) -> bool {
        self.pending
            .values()
            .any(|pending| pending.source_connection_id.as_deref() == Some(connection_id))
    }

    #[cfg(test)]
    pub(in crate::features) fn saved_connection_is_preparing(&self, connection_id: &str) -> bool {
        self.pending.values().any(|pending| {
            pending.preparing && pending.source_connection_id.as_deref() == Some(connection_id)
        })
    }

    pub(in crate::features) fn mcp_operation_is_pending(&self, operation_id: &str) -> bool {
        self.pending
            .values()
            .any(|pending| pending.mcp_open_operation_id.as_deref() == Some(operation_id))
            || self
                .reconnect_completions
                .values()
                .any(|pending| pending.mcp_open_operation_id.as_deref() == Some(operation_id))
    }

    pub(in crate::features) fn attempt(
        &self,
        request_id: &str,
    ) -> nyaterm_transport::connection_attempt::ConnectionAttempt {
        self.pending
            .get(request_id)
            .map(|pending| pending.attempt.clone())
            .unwrap_or_default()
    }

    pub(in crate::features) fn register_pending(
        &mut self,
        request_id: String,
        mut pending: PendingSessionStart,
    ) -> bool {
        if pending.reconnect_session_id.is_none() && pending.tab_placement.is_none() {
            pending.tab_placement =
                Some(self.allocate_tab_placement(pending.insert_index.unwrap_or(usize::MAX)));
        }
        let existing = self.pending.get(&request_id);
        let promotion = existing.is_some();
        if let Some(existing) = existing {
            pending.attempt = existing.attempt.clone();
            pending.requested_at = existing.requested_at;
            pending.custom_name = existing.custom_name.clone();
            pending.tab_color = existing.tab_color;
            pending.locked = existing.locked;
            pending.tab_placement = existing.tab_placement;
        }
        let reconnecting = pending.reconnect_session_id.is_some();
        self.pending.insert(request_id.clone(), pending);
        if !reconnecting && !promotion {
            self.active_pending = Some(request_id);
            self.active_failed = None;
        }
        reconnecting
    }

    pub(in crate::features) fn take_event_request(
        &mut self,
        request_id: &str,
    ) -> SessionStartEventRequest {
        if self.cancelled.remove(request_id) {
            return SessionStartEventRequest::Cancelled;
        }
        let was_active = self.active_pending.as_deref() == Some(request_id);
        let pending = self.pending.remove(request_id);
        if was_active {
            self.active_pending = None;
        }
        SessionStartEventRequest::Pending {
            pending: pending.map(Box::new),
            was_active,
        }
    }

    pub(in crate::features) fn complete_success(
        &self,
        was_active: bool,
        no_active_session: bool,
    ) -> bool {
        was_active
            || (self.active_pending.is_none() && self.active_failed.is_none() && no_active_session)
    }

    pub(in crate::features) fn record_failure(
        &mut self,
        request_id: String,
        pending: Option<Box<PendingSessionStart>>,
        error: String,
        was_active: bool,
        reconnect_session_exists: bool,
    ) -> bool {
        let reconnect_session_id = pending
            .as_ref()
            .and_then(|pending| pending.reconnect_session_id.clone());
        if let Some(session_id) = reconnect_session_id {
            if reconnect_session_exists {
                self.reconnect_failures.insert(session_id, error);
            }
            return true;
        }
        if let Some(pending) = pending {
            self.failed.insert(
                request_id.clone(),
                FailedSessionStart {
                    pending: *pending,
                    error,
                    reason: SessionStartEndReason::Failed,
                },
            );
            if was_active {
                self.active_failed = Some(request_id);
            }
        }
        false
    }

    pub(in crate::features) fn clear_active_selection(&mut self) {
        self.active_pending = None;
        self.active_failed = None;
    }

    pub(in crate::features) fn reconnect_is_pending(&self, session_id: &str) -> bool {
        self.pending
            .values()
            .any(|pending| pending.reconnect_session_id.as_deref() == Some(session_id))
            || self
                .reconnect_completions
                .values()
                .any(|completion| completion.old_id == session_id)
    }

    pub(in crate::features) fn reconnect_failure(&self, session_id: &str) -> Option<&str> {
        self.reconnect_failures.get(session_id).map(String::as_str)
    }

    pub(in crate::features) fn clear_reconnect_failure(&mut self, session_id: &str) {
        self.reconnect_failures.remove(session_id);
    }

    pub(in crate::features) fn set_reconnect_failure(&mut self, session_id: String, error: String) {
        self.reconnect_failures.insert(session_id, error);
    }

    pub(in crate::features) fn record_reconnect_completion(
        &mut self,
        completion: PendingReconnectCompletion,
    ) {
        self.reconnect_completions
            .insert(completion.new_id.clone(), completion);
    }

    pub(in crate::features) fn take_reconnect_completion(
        &mut self,
        new_id: &str,
    ) -> Option<PendingReconnectCompletion> {
        self.reconnect_completions.remove(new_id)
    }

    pub(in crate::features) fn set_pending_workspace_split(
        &mut self,
        direction: WorkspaceSplitDirection,
        source_session_id: String,
    ) {
        self.pending_workspace_split = Some((direction, source_session_id));
    }

    pub(in crate::features) fn take_pending_workspace_split(
        &mut self,
    ) -> Option<(WorkspaceSplitDirection, String)> {
        self.pending_workspace_split.take()
    }

    pub(in crate::features) fn pending_display_name(&self) -> Option<String> {
        self.active_pending
            .as_deref()
            .and_then(|request_id| self.pending.get(request_id))
            .or_else(|| {
                self.pending
                    .values()
                    .filter(|pending| pending.reconnect_session_id.is_none())
                    .min_by(|left, right| {
                        left.requested_at
                            .cmp(&right.requested_at)
                            .then_with(|| left.connection_name.cmp(&right.connection_name))
                    })
            })
            .map(pending_session_start_display_name)
    }

    pub(in crate::features) fn active_failed(&self) -> Option<&FailedSessionStart> {
        self.active_failed
            .as_deref()
            .and_then(|request_id| self.failed.get(request_id))
    }

    pub(in crate::features) fn failed_display_name(&self) -> Option<String> {
        self.active_failed()
            .or_else(|| {
                self.failed.values().min_by(|left, right| {
                    left.pending
                        .requested_at
                        .cmp(&right.pending.requested_at)
                        .then_with(|| {
                            left.pending
                                .connection_name
                                .cmp(&right.pending.connection_name)
                        })
                })
            })
            .map(failed_session_start_display_name)
    }

    pub(in crate::features) fn select_pending(&mut self, request_id: &str) -> bool {
        if !self.pending.contains_key(request_id) {
            return false;
        }
        self.active_pending = Some(request_id.to_string());
        self.active_failed = None;
        true
    }

    /// Cancel a worker but retain its metadata for retry under the same tab identity.
    pub(in crate::features) fn cancel_pending(
        &mut self,
        request_id: &str,
        message: String,
    ) -> bool {
        let was_active = self.active_pending.as_deref() == Some(request_id);
        let Some(pending) = self.close_pending(request_id) else {
            return false;
        };
        self.failed.insert(
            request_id.to_string(),
            FailedSessionStart {
                pending,
                error: message,
                reason: SessionStartEndReason::Cancelled,
            },
        );
        if was_active {
            self.active_pending = None;
            self.active_failed = Some(request_id.to_string());
        }
        true
    }

    pub(in crate::features) fn close_pending(
        &mut self,
        request_id: &str,
    ) -> Option<PendingSessionStart> {
        let pending = self.pending.remove(request_id)?;
        pending.attempt.cancel();
        if !pending.preparing {
            self.cancelled.insert(request_id.to_string());
        }
        if self.active_pending.as_deref() == Some(request_id) {
            self.active_pending = self.latest_pending_request_id();
            if self.active_pending.is_none() {
                self.active_failed = self.latest_failed_request_id();
            }
        }
        Some(pending)
    }

    pub(in crate::features) fn select_failed(&mut self, request_id: &str) -> bool {
        if !self.failed.contains_key(request_id) {
            return false;
        }
        self.active_failed = Some(request_id.to_string());
        self.active_pending = None;
        true
    }

    pub(in crate::features) fn close_failed(
        &mut self,
        request_id: &str,
    ) -> Option<FailedSessionStart> {
        let failed = self.failed.remove(request_id)?;
        if self.active_failed.as_deref() == Some(request_id) {
            self.active_failed = None;
            self.active_pending = self.latest_pending_request_id();
            if self.active_pending.is_none() {
                self.active_failed = self.latest_failed_request_id();
            }
        }
        Some(failed)
    }

    pub(in crate::features) fn pending_status_source(&self) -> Option<(String, Instant)> {
        self.pending
            .values()
            .min_by(|left, right| {
                left.requested_at
                    .cmp(&right.requested_at)
                    .then_with(|| left.connection_name.cmp(&right.connection_name))
            })
            .map(|pending| (pending.connection_name.clone(), pending.requested_at))
    }

    pub(super) fn latest_pending_request_id(&self) -> Option<String> {
        self.pending
            .iter()
            .filter(|(_, pending)| pending.reconnect_session_id.is_none())
            .max_by(|(left_id, left), (right_id, right)| {
                left.requested_at
                    .cmp(&right.requested_at)
                    .then_with(|| left_id.cmp(right_id))
            })
            .map(|(request_id, _)| request_id.clone())
    }

    pub(super) fn latest_failed_request_id(&self) -> Option<String> {
        self.failed
            .iter()
            .max_by(|(left_id, left), (right_id, right)| {
                left.pending
                    .requested_at
                    .cmp(&right.pending.requested_at)
                    .then_with(|| left_id.cmp(right_id))
            })
            .map(|(request_id, _)| request_id.clone())
    }
}

pub(in crate::features::session) fn pending_session_start_display_name(
    pending: &PendingSessionStart,
) -> String {
    pending
        .custom_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or(&pending.connection_name)
        .to_string()
}

pub(in crate::features::session) fn failed_session_start_display_name(
    failed: &FailedSessionStart,
) -> String {
    pending_session_start_display_name(&failed.pending)
}

pub(super) const DEFAULT_DUPLICATE_STARTUP_DELAY_MS: u64 = 500;

impl SessionFeatureState {
    pub(in crate::features) fn start_request_can_start(
        &self,
        options: &SavedConnectionStartOptions,
    ) -> bool {
        self.start.request_can_start(options)
    }

    pub(in crate::features) fn begin_saved_start_preparation(
        &mut self,
        connection: &SavedConnection,
        options: &SavedConnectionStartOptions,
    ) -> String {
        self.start.begin_saved_preparation(connection, options)
    }

    pub(in crate::features) fn start_attempt(
        &self,
        request_id: &str,
    ) -> nyaterm_transport::connection_attempt::ConnectionAttempt {
        self.start.attempt(request_id)
    }

    pub(in crate::features) fn complete_start_preparation(&mut self, request_id: &str) {
        self.start.take_event_request(request_id);
    }

    pub(in crate::features) fn clear_active_start_selection(&mut self) {
        self.start.clear_active_selection();
    }
    pub(in crate::features) fn external_start_is_busy(&self) -> bool {
        self.start.is_busy()
    }
    pub(in crate::features) fn start_is_busy(&self) -> bool {
        self.start.is_busy()
    }
    pub(in crate::features) fn start_has_pending(&self) -> bool {
        self.start.has_pending()
    }
    #[cfg(test)]
    pub(in crate::features) fn simulate_pending_start(
        &mut self,
        id: &str,
        pending: PendingSessionStart,
    ) {
        self.start.register_pending(id.to_string(), pending);
    }
    #[cfg(test)]
    pub(in crate::features) fn simulate_start_failure(&mut self, id: &str) {
        if let SessionStartEventRequest::Pending {
            pending,
            was_active,
        } = self.start.take_event_request(id)
        {
            self.start.record_failure(
                id.to_string(),
                pending,
                "failed".to_string(),
                was_active,
                false,
            );
        }
    }
    pub(in crate::features) fn start_has_failed(&self) -> bool {
        self.start.has_failed()
    }
    pub(in crate::features) fn start_pending_display_name(&self) -> Option<String> {
        self.start.pending_display_name()
    }
    pub(in crate::features) fn start_active_failed(&self) -> Option<&FailedSessionStart> {
        self.start.active_failed()
    }
    pub(in crate::features) fn start_failed_display_name(&self) -> Option<String> {
        self.start.failed_display_name()
    }
    pub(in crate::features) fn start_pending_status_source(&self) -> Option<(String, Instant)> {
        self.start.pending_status_source()
    }
    pub(in crate::features) fn start_visible_tab_reservation_count(&self) -> usize {
        self.start.visible_tab_reservation_count()
    }
    pub(in crate::features) fn start_has_active_pending(&self) -> bool {
        self.start.has_active_pending()
    }
    pub(in crate::features) fn start_has_active_failed(&self) -> bool {
        self.start.has_active_failed()
    }
    pub(in crate::features) fn start_request_is_active(&self, request_id: &str) -> bool {
        self.start.request_is_active(request_id)
    }
    pub(in crate::features) fn start_pending_entries(
        &self,
    ) -> impl Iterator<Item = (&String, &PendingSessionStart)> {
        self.start.pending_entries()
    }
    pub(in crate::features) fn start_failed_entries(
        &self,
    ) -> impl Iterator<Item = (&String, &FailedSessionStart)> {
        self.start.failed_entries()
    }
    pub(in crate::features) fn saved_connection_is_starting(&self, id: &str) -> bool {
        self.start.source_connection_is_pending(id)
    }
    pub(in crate::features) fn start_mcp_operation_is_pending(&self, id: &str) -> bool {
        self.start.mcp_operation_is_pending(id)
    }
    #[cfg(test)]
    pub(in crate::features) fn simulate_saved_preparation(
        &mut self,
        id: &str,
        placement: SessionStartTabPlacement,
    ) -> String {
        let connection: SavedConnection = serde_json::from_value(serde_json::json!({
            "id": id, "name": "preparing", "type": "ssh", "host": "127.0.0.1"
        }))
        .unwrap();
        self.start.begin_saved_preparation(
            &connection,
            &SavedConnectionStartOptions {
                tab_placement: Some(placement),
                ..Default::default()
            },
        )
    }
    #[cfg(test)]
    pub(in crate::features) fn simulate_close_preparation(&mut self, id: &str) {
        self.start.close_pending(id);
    }
    pub(in crate::features) fn start_reconnect_is_pending(&self, session_id: &str) -> bool {
        self.start.reconnect_is_pending(session_id)
    }
    pub(in crate::features) fn start_reconnect_failure(&self, session_id: &str) -> Option<&str> {
        self.start.reconnect_failure(session_id)
    }
    pub(in crate::features) fn start_set_pending_workspace_split(
        &mut self,
        direction: WorkspaceSplitDirection,
        source_session_id: String,
    ) {
        self.start
            .set_pending_workspace_split(direction, source_session_id);
    }
    pub(in crate::features) fn start_take_pending_workspace_split(
        &mut self,
    ) -> Option<(WorkspaceSplitDirection, String)> {
        self.start.take_pending_workspace_split()
    }
    pub(in crate::features) fn resolve_start_tab_placement(
        &self,
        placement: SessionStartTabPlacement,
    ) -> SessionStartTabPlacement {
        self.start.resolve_placement(placement)
    }
    pub(in crate::features) fn set_start_tab_position(
        &mut self,
        placement: SessionStartTabPlacement,
        index: usize,
    ) {
        self.start.set_tab_position(placement, index);
    }
    pub(in crate::features) fn active_failed_request_id(&self) -> Option<&str> {
        self.start.active_failed_request_id()
    }
}
