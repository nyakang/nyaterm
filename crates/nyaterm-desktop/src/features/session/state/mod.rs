use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use std::time::Instant;

use gpui::{FocusHandle, UniformListScrollHandle};
use nyaterm_core::{AiExecutionProfile, SavedConnection};
use nyaterm_transport::{
    RemoteFileBackendPreferenceStore, RemoteFileService, SessionEvent, SessionInfo, SessionKind,
    SessionManager, SshMultiplexHandle, SshSessionConfig,
};

use crate::features::runtime_jobs::SessionStartResult;
use crate::models::event_wake::{ANY_INTEREST, EventWake};
use crate::models::{
    SessionEventBridge, SessionEventBridgeDrain, SessionEventBridgeStats, SessionLaunchConfig,
    SessionRuntimeMetadata, StartupCommandAction, StartupCommandRequest, TabActionsSubmenu,
    WorkspaceSplitDirection,
};
use crate::temporary_ssh_link::TemporaryLinkProtocol;

use super::SessionProtocolRuntimeState;
use super::auth_runtime::{
    AgentPromptBroker, AgentPromptRequest, CredentialPromptBroker, CredentialPromptRequest,
    CredentialPromptState, HostKeyPromptBroker, HostKeyPromptRequest,
    KeyboardInteractivePromptState, NativeOtpCodePreview, NativeOtpProvider,
    SftpDuplicatePromptBroker, SftpDuplicatePromptState,
};
use super::trzsz_runtime::TrzszSessionState;
use super::zmodem_runtime::ZmodemSessionState;

const COMMAND_HISTORY_LIMIT: usize = 128;
const DEFAULT_DUPLICATE_STARTUP_DELAY_MS: u64 = 500;

pub(in crate::features) struct SessionFeatureState {
    manager: Arc<SessionManager>,
    event_bridge: SessionEventBridge,
    pub(super) start: SessionStartFeatureState,
    restore: SessionRestoreState,
    events: SessionEventQueueState,
    pub(super) prompts: SessionPromptState,
    pub(super) dialogs: SessionDialogState,
    command_history: HashMap<String, Vec<String>>,
    active_search_draft: String,
    /// Shared by the active-sessions list and its scrollbar so both read one
    /// scroll position across re-renders.
    active_list_scroll: UniformListScrollHandle,
    /// Same contract as `active_list_scroll`, for the recording panel list.
    recording_list_scroll: UniformListScrollHandle,
    /// Per-session reconnect/disconnect busy state ("reconnect" | "disconnect").
    busy_actions: HashMap<String, String>,
    active: ActiveSessionState,
    order: Vec<String>,
    metadata: HashMap<String, SessionRuntimeMetadata>,
    start_tab_placements: HashMap<String, SessionStartTabPlacement>,
    custom_names: HashMap<String, String>,
    /// OSC 0/2 titles from the session PTY (fall back when no custom rename).
    dynamic_titles: HashMap<String, String>,
    /// Latest OSC 7 working directories per session.
    cwds: HashMap<String, String>,
    tab_colors: HashMap<String, u32>,
    locked_tabs: HashSet<String>,
    tab_drag: Option<SessionTabDragState>,
    protocols: SessionProtocolRuntimeState,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SessionTabDragState {
    source_id: String,
    target_id: String,
    insert_after: bool,
}

#[derive(Default)]
struct SessionRestoreState {
    complete: bool,
}

#[derive(Default)]
struct SessionEventQueueState {
    pending: VecDeque<SessionEvent>,
}

#[derive(Default)]
struct ActiveSessionState {
    id: Option<String>,
}

pub(in crate::features) struct SessionFeatureFocus {
    pub credential: FocusHandle,
    pub tab_actions: FocusHandle,
    pub color_picker: FocusHandle,
    pub info: FocusHandle,
}

/// Native authentication and transfer prompts tied to the session runtime.
pub(super) struct SessionPromptState {
    /// One wake shared by all four brokers: activation runs all four steps on
    /// any signal, and each is a cheap no-op when its queue is empty.
    wake: EventWake,
    wake_rx: Option<UnboundedReceiver<()>>,
    duplicate_prompts: Arc<SftpDuplicatePromptBroker>,
    active_duplicate_prompt: Option<SftpDuplicatePromptState>,
    host_key_prompts: Arc<HostKeyPromptBroker>,
    active_host_key_prompt: Option<HostKeyPromptRequest>,
    credential_prompts: Arc<CredentialPromptBroker>,
    active_credential_prompt: Option<CredentialPromptState>,
    active_keyboard_interactive_prompt: Option<KeyboardInteractivePromptState>,
    agent_prompts: Arc<AgentPromptBroker>,
    active_agent_prompt: Option<AgentPromptRequest>,
    credential_prompt_focus_pending: bool,
    credential_focus: FocusHandle,
    otp_provider: Arc<NativeOtpProvider>,
}

pub(in crate::features) enum PromptResolution<T> {
    Inactive,
    Changed,
    Ready(T),
}

pub(in crate::features) struct PromptInputTarget {
    pub id: String,
    pub seed: String,
    pub echo: bool,
}

/// Session-scoped overlays, confirmations and editing dialogs.
pub(super) struct SessionDialogState {
    tab_actions_session_id: Option<String>,
    tab_actions_anchor: Option<(f32, f32)>,
    tab_actions_submenu: Option<TabActionsSubmenu>,
    tab_actions_focus: FocusHandle,
    pending_quit_after_close_all: bool,
    pending_window_quit: bool,
    rename_session_id: Option<String>,
    rename_draft: String,
    color_picker_open: bool,
    color_picker_focus: FocusHandle,
    session_info_open: bool,
    session_info_focus: FocusHandle,
    startup_command_open: bool,
    startup_command_action: StartupCommandAction,
    startup_command_draft: String,
    startup_command_delay_ms: u64,
    temporary_ssh_link_open: bool,
    temporary_link_protocol: TemporaryLinkProtocol,
    temporary_ssh_link_draft: String,
    temporary_serial_port_name: String,
    temporary_serial_baud_rate: String,
    temporary_ssh_link_error: Option<&'static str>,
}

pub(in crate::features) enum RenameSessionSubmission {
    Inactive,
    Empty,
    Ready { session_id: String, name: String },
}

pub(in crate::features) struct SessionDisconnectUpdate {
    pub already_disconnected: bool,
    pub multiplex_key: Option<String>,
}

impl SessionFeatureState {
    pub(in crate::features) fn new(
        manager: Arc<SessionManager>,
        event_bridge: SessionEventBridge,
        otp_provider: Arc<NativeOtpProvider>,
        focus: SessionFeatureFocus,
    ) -> Self {
        Self {
            manager,
            event_bridge,
            start: SessionStartFeatureState::new(),
            restore: SessionRestoreState::default(),
            events: SessionEventQueueState::default(),
            prompts: SessionPromptState::new(otp_provider, focus.credential),
            dialogs: SessionDialogState {
                tab_actions_session_id: None,
                tab_actions_anchor: None,
                tab_actions_submenu: None,
                tab_actions_focus: focus.tab_actions,
                pending_quit_after_close_all: false,
                pending_window_quit: false,
                rename_session_id: None,
                rename_draft: String::new(),
                color_picker_open: false,
                color_picker_focus: focus.color_picker,
                session_info_open: false,
                session_info_focus: focus.info,
                startup_command_open: false,
                startup_command_action: StartupCommandAction::Duplicate,
                startup_command_draft: String::new(),
                startup_command_delay_ms: DEFAULT_DUPLICATE_STARTUP_DELAY_MS,
                temporary_ssh_link_open: false,
                temporary_link_protocol: TemporaryLinkProtocol::Ssh,
                temporary_ssh_link_draft: String::new(),
                temporary_serial_port_name: String::new(),
                temporary_serial_baud_rate: "115200".to_string(),
                temporary_ssh_link_error: None,
            },
            command_history: HashMap::new(),
            active_search_draft: String::new(),
            active_list_scroll: UniformListScrollHandle::new(),
            recording_list_scroll: UniformListScrollHandle::new(),
            busy_actions: HashMap::new(),
            active: ActiveSessionState::default(),
            order: Vec::new(),
            metadata: HashMap::new(),
            start_tab_placements: HashMap::new(),
            custom_names: HashMap::new(),
            dynamic_titles: HashMap::new(),
            cwds: HashMap::new(),
            tab_colors: HashMap::new(),
            locked_tabs: HashSet::new(),
            tab_drag: None,
            protocols: SessionProtocolRuntimeState::default(),
        }
    }

    pub(in crate::features) fn manager(&self) -> &SessionManager {
        &self.manager
    }

    pub(in crate::features) fn manager_handle(&self) -> Arc<SessionManager> {
        Arc::clone(&self.manager)
    }

    pub(in crate::features) fn start_has_pending(&self) -> bool {
        self.start.has_pending()
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

    pub(in crate::features) fn start_pending_count(&self) -> usize {
        self.start.pending_count()
    }

    pub(in crate::features) fn start_visible_tab_reservation_count(&self) -> usize {
        self.start.visible_tab_reservation_count()
    }

    pub(in crate::features) fn start_has_cancelled_results(&self) -> bool {
        self.start.has_cancelled_results()
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

    pub(in crate::features) fn start_saved_connection_is_pending(
        &self,
        connection: &SavedConnection,
    ) -> bool {
        self.start.source_connection_is_pending(&connection.id)
    }

    pub(in crate::features) fn start_saved_connection_is_pending_or_preparing(
        &self,
        connection: &SavedConnection,
    ) -> bool {
        self.start_saved_connection_is_pending(connection)
            || self.start.saved_connection_is_preparing(&connection.id)
    }

    pub(in crate::features) fn start_reserve_saved_connection(
        &mut self,
        connection_id: &str,
        placement: SessionStartTabPlacement,
    ) -> bool {
        self.start
            .reserve_saved_connection_start(connection_id, placement)
    }

    pub(in crate::features) fn start_release_saved_connection(&mut self, connection_id: &str) {
        self.start.release_saved_connection_start(connection_id);
        if self.start.visible_tab_reservation_count() == 0 {
            self.clear_start_tab_placements();
        }
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

    pub(in crate::features) fn prompt_duplicate_broker(&self) -> Arc<SftpDuplicatePromptBroker> {
        self.prompts.duplicate_broker()
    }

    pub(in crate::features) fn prompt_otp_provider(&self) -> Arc<NativeOtpProvider> {
        self.prompts.otp_provider()
    }

    pub(in crate::features) fn prompt_credential_focus(&self) -> &FocusHandle {
        self.prompts.credential_focus()
    }

    pub(in crate::features) fn prompt_credential_focus_is_pending(&self) -> bool {
        self.prompts.credential_focus_is_pending()
    }

    pub(in crate::features) fn prompt_finish_credential_focus(&mut self) {
        self.prompts.finish_credential_focus();
    }

    pub(in crate::features) fn prompt_active_duplicate(&self) -> Option<&SftpDuplicatePromptState> {
        self.prompts.active_duplicate()
    }

    pub(in crate::features) fn prompt_active_host_key(&self) -> Option<&HostKeyPromptRequest> {
        self.prompts.active_host_key()
    }

    pub(in crate::features) fn prompt_active_credential(&self) -> Option<&CredentialPromptState> {
        self.prompts.active_credential()
    }

    pub(in crate::features) fn prompt_active_keyboard_interactive(
        &self,
    ) -> Option<&KeyboardInteractivePromptState> {
        self.prompts.active_keyboard_interactive()
    }

    pub(in crate::features) fn prompt_active_agent(&self) -> Option<&AgentPromptRequest> {
        self.prompts.active_agent()
    }

    pub(in crate::features) fn prompt_take_agent_changed(&self) -> bool {
        self.prompts.take_agent_changed()
    }

    pub(in crate::features) fn prompt_reconcile_agent(&mut self) -> bool {
        self.prompts.reconcile_agent()
    }

    pub(in crate::features) fn prompt_has_active_credential(&self) -> bool {
        self.prompts.has_active_credential()
    }

    pub(in crate::features) fn prompt_has_active_keyboard_interactive(&self) -> bool {
        self.prompts.has_active_keyboard_interactive()
    }

    pub(in crate::features) fn prompt_has_active_ssh_auth(&self) -> bool {
        self.prompts.has_active_ssh_auth()
    }

    pub(in crate::features) fn prompt_has_pending_or_active_prompt(&self) -> bool {
        self.prompts.has_pending_or_active_prompt()
    }

    pub(in crate::features) fn prompt_focus_keyboard_interactive_response(
        &mut self,
        prompt_id: &str,
        index: usize,
    ) -> bool {
        self.prompts
            .focus_keyboard_interactive_response(prompt_id, index)
    }

    pub(in crate::features) fn dialog_tab_actions_session_id(&self) -> Option<&str> {
        self.dialogs.tab_actions_session_id()
    }

    pub(in crate::features) fn dialog_tab_actions_anchor(&self) -> Option<(f32, f32)> {
        self.dialogs.tab_actions_anchor()
    }

    pub(in crate::features) fn dialog_tab_actions_submenu(&self) -> Option<TabActionsSubmenu> {
        self.dialogs.tab_actions_submenu()
    }

    pub(in crate::features) fn dialog_tab_actions_focus(&self) -> &FocusHandle {
        self.dialogs.tab_actions_focus()
    }

    pub(in crate::features) fn dialog_close_tab_actions(&mut self) {
        self.dialogs.close_tab_actions();
    }

    pub(in crate::features) fn dialog_select_tab_actions_submenu(
        &mut self,
        submenu: TabActionsSubmenu,
    ) -> bool {
        self.dialogs.select_tab_actions_submenu(submenu)
    }

    pub(in crate::features) fn dialog_should_quit_after_close_all(&self) -> bool {
        self.dialogs.should_quit_after_close_all()
    }

    pub(in crate::features) fn dialog_request_quit_after_close_all(&mut self) {
        self.dialogs.request_quit_after_close_all();
    }

    pub(in crate::features) fn dialog_open_close_all_sessions_confirm(&mut self) {
        self.dialogs.open_close_all_sessions_confirm();
    }

    pub(in crate::features) fn dialog_cancel_close_all_sessions_confirm(&mut self) {
        self.dialogs.cancel_close_all_sessions_confirm();
    }

    pub(in crate::features) fn dialog_take_close_all_sessions_confirm(&mut self) -> bool {
        self.dialogs.take_close_all_sessions_confirm()
    }

    pub(in crate::features) fn dialog_rename_draft(&self) -> &str {
        self.dialogs.rename_draft()
    }

    pub(in crate::features) fn dialog_color_picker_is_open(&self) -> bool {
        self.dialogs.color_picker_is_open()
    }

    pub(in crate::features) fn dialog_color_picker_focus(&self) -> &FocusHandle {
        self.dialogs.color_picker_focus()
    }

    pub(in crate::features) fn dialog_session_info_is_open(&self) -> bool {
        self.dialogs.session_info_is_open()
    }

    pub(in crate::features) fn dialog_session_info_focus(&self) -> &FocusHandle {
        self.dialogs.session_info_focus()
    }

    pub(in crate::features) fn dialog_startup_command_draft(&self) -> &str {
        self.dialogs.startup_command_draft()
    }

    pub(in crate::features) fn dialog_startup_command_delay_ms(&self) -> u64 {
        self.dialogs.startup_command_delay_ms()
    }

    pub(in crate::features) fn dialog_reset_startup_command_delay(&mut self) {
        self.dialogs.reset_startup_command_delay();
    }

    pub(in crate::features) fn dialog_temporary_ssh_link_draft(&self) -> &str {
        self.dialogs.temporary_ssh_link_draft()
    }

    pub(in crate::features) fn dialog_temporary_link_protocol(&self) -> TemporaryLinkProtocol {
        self.dialogs.temporary_link_protocol()
    }

    pub(in crate::features) fn dialog_temporary_serial_port_name(&self) -> &str {
        self.dialogs.temporary_serial_port_name()
    }

    pub(in crate::features) fn dialog_temporary_serial_baud_rate(&self) -> &str {
        self.dialogs.temporary_serial_baud_rate()
    }

    pub(in crate::features) fn dialog_temporary_ssh_link_error(&self) -> Option<&'static str> {
        self.dialogs.temporary_ssh_link_error()
    }

    pub(in crate::features) fn restore_is_complete(&self) -> bool {
        self.restore.is_complete()
    }

    pub(in crate::features) fn mark_restore_complete(&mut self) -> bool {
        self.restore.mark_complete()
    }

    pub(in crate::features) fn configure_event_bridge(
        &self,
        encoding: String,
        scrollback_limit: usize,
    ) {
        self.event_bridge.configure(encoding, scrollback_limit);
    }

    pub(in crate::features) fn route_session_events_to_ui(&self, session_id: &str) {
        self.event_bridge.route_session_to_ui(session_id);
    }

    pub(in crate::features) fn resume_session_direct_output(&self, session_id: &str) {
        self.event_bridge.resume_session_direct_output(session_id);
    }

    pub(in crate::features) fn clear_event_bridge_session(&self, session_id: &str) {
        self.event_bridge.clear_session(session_id);
    }

    pub(in crate::features) fn drain_event_bridge(
        &self,
        max_events: usize,
        max_output_bytes: usize,
    ) -> SessionEventBridgeDrain {
        self.event_bridge
            .drain_events_with_output_budget(max_events, max_output_bytes)
    }

    /// Taken once by `NyaTermApp::start_runtime_data_plane_drain`.
    pub(in crate::features) fn take_event_bridge_wake_receiver(
        &self,
    ) -> Option<UnboundedReceiver<()>> {
        self.event_bridge.take_ui_queue_wake_receiver()
    }

    pub(in crate::features) fn arm_event_bridge_wake(&self) {
        self.event_bridge.arm_ui_queue_wake();
    }

    #[cfg(test)]
    pub(in crate::features) fn push_event_bridge_ui_event_for_test(&self, event: SessionEvent) {
        self.event_bridge.push_ui_event_for_test(event);
    }

    pub(in crate::features) fn harvest_event_bridge_stats(&self) -> SessionEventBridgeStats {
        self.event_bridge.harvest_direct_stats()
    }

    pub(in crate::features) fn event_bridge_has_pending_ui_work(&self) -> bool {
        self.event_bridge.has_pending_ui_work()
    }

    pub(in crate::features) fn event_bridge_queued_event_count(&self) -> usize {
        self.event_bridge.queued_event_count()
    }

    pub(in crate::features) fn event_bridge_source_queued_event_count(&self) -> usize {
        self.event_bridge.source_queued_event_count()
    }

    pub(in crate::features) fn event_bridge_queued_output_bytes(&self) -> usize {
        self.event_bridge.queued_output_bytes()
    }

    pub(in crate::features) fn event_bridge_source_queued_output_bytes(&self) -> usize {
        self.event_bridge.source_queued_output_bytes()
    }

    pub(in crate::features) fn pending_event_count(&self) -> usize {
        self.events.pending.len()
    }

    pub(in crate::features) fn pending_events_are_empty(&self) -> bool {
        self.events.pending.is_empty()
    }

    pub(in crate::features) fn extend_pending_events(
        &mut self,
        events: impl IntoIterator<Item = SessionEvent>,
    ) {
        self.events.pending.extend(events);
    }

    pub(in crate::features) fn pop_pending_event(&mut self) -> Option<SessionEvent> {
        self.events.pending.pop_front()
    }

    pub(in crate::features) fn pending_event_output_bytes(&self) -> usize {
        self.events
            .pending
            .iter()
            .map(|event| match event {
                SessionEvent::Output { data, .. } => data.len(),
                _ => 0,
            })
            .sum()
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

    pub(in crate::features) fn recording_list_scroll(&self) -> &UniformListScrollHandle {
        &self.recording_list_scroll
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

    fn begin_busy_action(&mut self, session_id: String, action: &'static str) -> bool {
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

    pub(in crate::features) fn active_ssh_config_owned(&self) -> Option<SshSessionConfig> {
        self.active_ssh_config().cloned()
    }

    pub(in crate::features) fn active_ssh_file_browser_config(&self) -> Option<&SshSessionConfig> {
        self.active_ssh_config()
    }

    pub(in crate::features) fn active_ssh_multiplex_handle(
        &mut self,
    ) -> Option<SshMultiplexHandle> {
        let session_id = self.active_id_owned()?;
        self.ssh_multiplex_handle_for_session(&session_id)
    }

    pub(in crate::features) fn remote_file_service_for_session(
        &mut self,
        session_id: &str,
        config: SshSessionConfig,
        preference_store: std::sync::Arc<dyn RemoteFileBackendPreferenceStore>,
    ) -> anyhow::Result<RemoteFileService> {
        let multiplex = self.ssh_multiplex_handle_for_session(session_id);
        if let Some(service) = self.protocols.remote_files.get(session_id) {
            // A transfer browser can be opened while the session-start event is
            // still being drained. Rebuild a previously dedicated service once
            // the authenticated shared handle is registered, otherwise SFTP
            // would silently create a second SSH connection.
            if multiplex.is_none() || service.is_multiplexed() {
                return Ok(service.clone());
            }
            self.protocols.remote_files.remove(session_id);
        }
        let service =
            RemoteFileService::with_preference_store(config, multiplex, preference_store)?;
        self.protocols
            .remote_files
            .insert(session_id.to_string(), service.clone());
        Ok(service)
    }

    pub(in crate::features) fn remove_remote_file_service(&mut self, session_id: &str) -> bool {
        self.protocols.remote_files.remove(session_id).is_some()
    }

    pub(in crate::features) fn ssh_multiplex_handle_for_session(
        &mut self,
        session_id: &str,
    ) -> Option<SshMultiplexHandle> {
        let multiplex_key = self.metadata(session_id)?.ssh_multiplex_key.clone()?;
        self.reusable_multiplex_handle(&multiplex_key)
    }

    pub(in crate::features) fn active_ai_execution_profile(&self) -> AiExecutionProfile {
        self.active
            .id
            .as_deref()
            .and_then(|session_id| self.metadata.get(session_id))
            .map(|metadata| metadata.ai_execution_profile)
            .unwrap_or(AiExecutionProfile::SendOnly)
    }

    pub(in crate::features) fn has_protocol_runtime_sessions(&self) -> bool {
        !self.protocols.zmodem.is_empty() || !self.protocols.trzsz.is_empty()
    }

    pub(super) fn has_zmodem_runtime_sessions(&self) -> bool {
        !self.protocols.zmodem.is_empty()
    }

    pub(super) fn has_trzsz_runtime_sessions(&self) -> bool {
        !self.protocols.trzsz.is_empty()
    }

    #[cfg(test)]
    fn protocol_runtime_counts(&self) -> (usize, usize, usize) {
        (
            self.protocols.zmodem.len(),
            self.protocols.trzsz.len(),
            self.protocols.multiplex_handles.len(),
        )
    }

    pub(super) fn zmodem_state(&self, session_id: &str) -> Option<&ZmodemSessionState> {
        self.protocols.zmodem.get(session_id)
    }

    pub(super) fn zmodem_state_mut(&mut self, session_id: &str) -> Option<&mut ZmodemSessionState> {
        self.protocols.zmodem.get_mut(session_id)
    }

    pub(super) fn zmodem_state_mut_or_default(
        &mut self,
        session_id: &str,
    ) -> &mut ZmodemSessionState {
        self.protocols
            .zmodem
            .entry(session_id.to_string())
            .or_default()
    }

    pub(super) fn zmodem_states_mut(
        &mut self,
    ) -> impl Iterator<Item = (&String, &mut ZmodemSessionState)> {
        self.protocols.zmodem.iter_mut()
    }

    pub(super) fn remove_zmodem_session_runtime(&mut self, session_id: &str) -> bool {
        let Some(mut state) = self.protocols.zmodem.remove(session_id) else {
            return false;
        };
        state.stop_worker();
        true
    }

    pub(super) fn trzsz_state(&self, session_id: &str) -> Option<&TrzszSessionState> {
        self.protocols.trzsz.get(session_id)
    }

    pub(super) fn trzsz_state_mut(&mut self, session_id: &str) -> Option<&mut TrzszSessionState> {
        self.protocols.trzsz.get_mut(session_id)
    }

    pub(super) fn trzsz_state_mut_or_default(
        &mut self,
        session_id: &str,
    ) -> &mut TrzszSessionState {
        self.protocols
            .trzsz
            .entry(session_id.to_string())
            .or_default()
    }

    pub(super) fn trzsz_states_mut(
        &mut self,
    ) -> impl Iterator<Item = (&String, &mut TrzszSessionState)> {
        self.protocols.trzsz.iter_mut()
    }

    pub(super) fn remove_trzsz_session_runtime(&mut self, session_id: &str) -> bool {
        let Some(mut state) = self.protocols.trzsz.remove(session_id) else {
            return false;
        };
        state.stop_workers();
        true
    }

    pub(in crate::features) fn register_multiplex_handle(
        &mut self,
        multiplex_key: String,
        handle: SshMultiplexHandle,
    ) {
        self.protocols
            .multiplex_handles
            .insert(multiplex_key, handle);
    }

    pub(in crate::features) fn reusable_multiplex_handle(
        &mut self,
        multiplex_key: &str,
    ) -> Option<SshMultiplexHandle> {
        if self
            .protocols
            .multiplex_handles
            .get(multiplex_key)
            .is_some_and(SshMultiplexHandle::is_closed)
        {
            self.protocols.multiplex_handles.remove(multiplex_key);
        }
        self.protocols.multiplex_handles.get(multiplex_key).cloned()
    }

    pub(in crate::features) fn take_multiplex_handle_if_unreferenced(
        &mut self,
        multiplex_key: &str,
    ) -> Option<SshMultiplexHandle> {
        if self.multiplex_key_is_referenced(multiplex_key) {
            return None;
        }
        self.protocols.multiplex_handles.remove(multiplex_key)
    }

    pub(in crate::features) fn take_multiplex_handle_if_no_other_live_reference(
        &mut self,
        session_id: &str,
        multiplex_key: &str,
    ) -> Option<SshMultiplexHandle> {
        if self.other_live_session_uses_multiplex_key(session_id, multiplex_key) {
            return None;
        }
        self.protocols.multiplex_handles.remove(multiplex_key)
    }

    pub(in crate::features) fn select_active_session(
        &mut self,
        session_id: impl Into<String>,
    ) -> Option<String> {
        let session_id = session_id.into();
        self.active.id.replace(session_id)
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
        self.start_tab_placements.get(session_id).copied()
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

    pub(in crate::features) fn register_session_metadata_for_start(
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
        if let Some(tab_placement) = tab_placement {
            self.start_tab_placements
                .insert(session_id.to_string(), tab_placement);
            self.order.retain(|id| id != session_id);
            let target_key = (tab_placement.insert_index, tab_placement.request_sequence);
            let insert_index = self
                .order
                .iter()
                .enumerate()
                .find_map(|(index, existing_id)| {
                    if let Some(existing) = self.start_tab_placements.get(existing_id) {
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

    pub(in crate::features) fn clear_start_tab_placements(&mut self) {
        self.start_tab_placements.clear();
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

    pub(in crate::features) fn next_session_after(&self, session_id: &str) -> Option<String> {
        let known_ids = self.session_ids().collect::<HashSet<_>>();
        self.order
            .iter()
            .find(|candidate| {
                candidate.as_str() != session_id && known_ids.contains(candidate.as_str())
            })
            .cloned()
            .or_else(|| {
                known_ids
                    .into_iter()
                    .find(|candidate| *candidate != session_id)
                    .map(ToOwned::to_owned)
            })
    }

    pub(in crate::features) fn mark_session_disconnected(
        &mut self,
        session_id: &str,
    ) -> Option<SessionDisconnectUpdate> {
        let metadata = self.metadata.get_mut(session_id)?;
        let already_disconnected = metadata.disconnected;
        metadata.disconnected = true;
        Some(SessionDisconnectUpdate {
            already_disconnected,
            multiplex_key: metadata.ssh_multiplex_key.clone(),
        })
    }

    fn other_live_session_uses_multiplex_key(&self, session_id: &str, multiplex_key: &str) -> bool {
        self.metadata.iter().any(|(id, metadata)| {
            id != session_id
                && !metadata.disconnected
                && metadata.ssh_multiplex_key.as_deref() == Some(multiplex_key)
        })
    }

    fn multiplex_key_is_referenced(&self, multiplex_key: &str) -> bool {
        self.metadata
            .values()
            .any(|metadata| metadata.ssh_multiplex_key.as_deref() == Some(multiplex_key))
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

    pub(in crate::features) fn set_tab_drag_target(
        &mut self,
        source_id: String,
        target_id: String,
        insert_after: bool,
    ) -> bool {
        let next = SessionTabDragState {
            source_id,
            target_id,
            insert_after,
        };
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

    pub(in crate::features) fn tab_drop_after(&self, session_id: &str) -> Option<bool> {
        self.tab_drag
            .as_ref()
            .filter(|drag| drag.target_id == session_id)
            .map(|drag| drag.insert_after)
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

    pub(in crate::features) fn remove_session_catalog(
        &mut self,
        session_id: &str,
    ) -> Option<String> {
        self.remove_zmodem_session_runtime(session_id);
        self.remove_trzsz_session_runtime(session_id);
        self.remove_remote_file_service(session_id);
        self.order.retain(|id| id != session_id);
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
            .is_some_and(|drag| drag.source_id == session_id || drag.target_id == session_id)
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

fn session_info_from_metadata(session_id: &str, metadata: &SessionRuntimeMetadata) -> SessionInfo {
    match &metadata.launch_config {
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

impl SessionRestoreState {
    pub(in crate::features) fn is_complete(&self) -> bool {
        self.complete
    }

    pub(in crate::features) fn mark_complete(&mut self) -> bool {
        if self.complete {
            return false;
        }
        self.complete = true;
        true
    }
}

impl SessionPromptState {
    pub(in crate::features) fn duplicate_broker(&self) -> Arc<SftpDuplicatePromptBroker> {
        Arc::clone(&self.duplicate_prompts)
    }

    pub(in crate::features) fn host_key_broker(&self) -> Arc<HostKeyPromptBroker> {
        Arc::clone(&self.host_key_prompts)
    }

    pub(in crate::features) fn credential_broker(&self) -> Arc<CredentialPromptBroker> {
        Arc::clone(&self.credential_prompts)
    }

    pub(in crate::features) fn agent_broker(&self) -> Arc<AgentPromptBroker> {
        Arc::clone(&self.agent_prompts)
    }

    pub(in crate::features) fn otp_provider(&self) -> Arc<NativeOtpProvider> {
        Arc::clone(&self.otp_provider)
    }

    pub(in crate::features) fn credential_focus(&self) -> &FocusHandle {
        &self.credential_focus
    }

    pub(in crate::features) fn credential_focus_is_pending(&self) -> bool {
        self.credential_prompt_focus_pending
    }

    pub(in crate::features) fn finish_credential_focus(&mut self) {
        self.credential_prompt_focus_pending = false;
    }

    pub(in crate::features) fn active_duplicate(&self) -> Option<&SftpDuplicatePromptState> {
        self.active_duplicate_prompt.as_ref()
    }

    pub(in crate::features) fn active_host_key(&self) -> Option<&HostKeyPromptRequest> {
        self.active_host_key_prompt.as_ref()
    }

    pub(in crate::features) fn active_credential(&self) -> Option<&CredentialPromptState> {
        self.active_credential_prompt.as_ref()
    }

    pub(in crate::features) fn active_keyboard_interactive(
        &self,
    ) -> Option<&KeyboardInteractivePromptState> {
        self.active_keyboard_interactive_prompt.as_ref()
    }

    pub(in crate::features) fn active_agent(&self) -> Option<&AgentPromptRequest> {
        self.active_agent_prompt.as_ref()
    }

    pub(in crate::features) fn has_active_credential(&self) -> bool {
        self.active_credential_prompt.is_some()
    }

    pub(in crate::features) fn has_active_keyboard_interactive(&self) -> bool {
        self.active_keyboard_interactive_prompt.is_some()
    }

    pub(in crate::features) fn has_active_ssh_auth(&self) -> bool {
        self.active_host_key_prompt.is_some()
            || self.active_credential_prompt.is_some()
            || self.active_keyboard_interactive_prompt.is_some()
            || self.active_agent_prompt.is_some()
    }

    pub(in crate::features) fn has_pending_or_active_prompt(&self) -> bool {
        self.has_active_ssh_auth()
            || self.active_duplicate_prompt.is_some()
            || self.host_key_prompts.has_pending()
            || self.credential_prompts.has_pending()
            || self.agent_prompts.has_pending()
            || self.duplicate_prompts.has_pending()
    }

    /// Install one wake across all four brokers, before any of them can be
    /// handed to a transport thread.
    fn new(otp_provider: Arc<NativeOtpProvider>, credential_focus: FocusHandle) -> Self {
        let (wake, wake_rx) = EventWake::new();
        let duplicate_prompts = Arc::new(SftpDuplicatePromptBroker::default());
        let host_key_prompts = Arc::new(HostKeyPromptBroker::default());
        let credential_prompts = Arc::new(CredentialPromptBroker::default());
        let agent_prompts = Arc::new(AgentPromptBroker::default());
        duplicate_prompts.set_wake(wake.clone());
        host_key_prompts.set_wake(wake.clone());
        credential_prompts.set_wake(wake.clone());
        agent_prompts.set_wake(wake.clone());
        Self {
            wake,
            wake_rx: Some(wake_rx),
            duplicate_prompts,
            active_duplicate_prompt: None,
            host_key_prompts,
            active_host_key_prompt: None,
            credential_prompts,
            active_credential_prompt: None,
            active_keyboard_interactive_prompt: None,
            agent_prompts,
            active_agent_prompt: None,
            credential_prompt_focus_pending: false,
            credential_focus,
            otp_provider,
        }
    }

    pub(in crate::features) fn take_wake_receiver(&mut self) -> Option<UnboundedReceiver<()>> {
        self.wake_rx.take()
    }

    /// Declare interest in the next enqueued prompt. See `models::event_wake`:
    /// this must happen before the consumer checks the queues.
    pub(in crate::features) fn arm_wake(&self) {
        self.wake.arm(ANY_INTEREST);
    }

    /// Freeing the single active slot is what lets the next queued prompt in, and
    /// nothing is enqueued at that moment, so the activation pass has to be woken
    /// explicitly. Every `take_*` below does this.
    fn signal_wake(&self) {
        self.wake.signal(ANY_INTEREST);
    }

    pub(in crate::features) fn take_host_key_resolution(
        &mut self,
        request_id: &str,
    ) -> PromptResolution<HostKeyPromptRequest> {
        let Some(request) = self.active_host_key_prompt.take() else {
            return PromptResolution::Inactive;
        };
        if request.id != request_id {
            self.active_host_key_prompt = Some(request);
            return PromptResolution::Changed;
        }
        self.signal_wake();
        PromptResolution::Ready(request)
    }

    pub(in crate::features) fn take_duplicate_resolution(
        &mut self,
        request_id: &str,
    ) -> PromptResolution<SftpDuplicatePromptState> {
        let Some(prompt) = self.active_duplicate_prompt.take() else {
            return PromptResolution::Inactive;
        };
        if prompt.id != request_id {
            self.active_duplicate_prompt = Some(prompt);
            return PromptResolution::Changed;
        }
        self.signal_wake();
        PromptResolution::Ready(prompt)
    }

    pub(in crate::features) fn take_credential(&mut self) -> Option<CredentialPromptState> {
        let state = self.active_credential_prompt.take()?;
        self.credential_prompt_focus_pending = false;
        self.signal_wake();
        Some(state)
    }

    pub(in crate::features) fn take_keyboard_interactive(
        &mut self,
    ) -> Option<KeyboardInteractivePromptState> {
        let state = self.active_keyboard_interactive_prompt.take()?;
        self.credential_prompt_focus_pending = false;
        self.signal_wake();
        Some(state)
    }

    pub(in crate::features) fn take_agent(&mut self) -> Option<AgentPromptRequest> {
        let request = self.active_agent_prompt.take()?;
        self.signal_wake();
        Some(request)
    }

    pub(in crate::features) fn take_agent_changed(&self) -> bool {
        self.agent_prompts.take_changed()
    }

    pub(in crate::features) fn reconcile_agent(&mut self) -> bool {
        if self
            .active_agent_prompt
            .as_ref()
            .is_some_and(AgentPromptRequest::is_resolved)
        {
            self.active_agent_prompt = None;
            return true;
        }
        false
    }

    pub(in crate::features) fn keyboard_interactive_otp_id(&self) -> Option<String> {
        self.active_keyboard_interactive_prompt
            .as_ref()
            .and_then(|state| state.request.otp_id.clone())
    }

    pub(in crate::features) fn keyboard_interactive_otp_code(&self) -> Option<String> {
        self.active_keyboard_interactive_prompt
            .as_ref()
            .and_then(|state| state.otp_code.clone())
    }

    pub(in crate::features) fn apply_keyboard_interactive_otp_result(
        &mut self,
        result: Result<Option<NativeOtpCodePreview>, String>,
        clear_missing_time_step: bool,
    ) -> bool {
        let Some(state) = self.active_keyboard_interactive_prompt.as_mut() else {
            return false;
        };
        match result {
            Ok(Some(preview)) => {
                state.otp_code = Some(preview.code);
                state.otp_type = Some(preview.otp_type);
                state.otp_period = preview.period;
                state.otp_time_step = preview.time_step;
                state.otp_error = None;
                true
            }
            Ok(None) => {
                state.otp_code = None;
                if clear_missing_time_step {
                    state.otp_time_step = None;
                }
                state.otp_error = Some("OTP entry not found".to_string());
                false
            }
            Err(error) => {
                state.otp_code = None;
                state.otp_time_step = None;
                state.otp_error = Some(error);
                false
            }
        }
    }

    pub(in crate::features) fn send_keyboard_interactive_otp_to_response(
        &mut self,
    ) -> Option<(String, String)> {
        let state = self.active_keyboard_interactive_prompt.as_mut()?;
        let code = state.otp_code.clone()?;
        let response = state.responses.first_mut()?;
        *response = code;
        state.focused_index = 0;
        Some((state.id.clone(), response.clone()))
    }

    pub(in crate::features) fn advance_keyboard_interactive_focus(
        &mut self,
        backwards: bool,
    ) -> Option<PromptInputTarget> {
        let state = self.active_keyboard_interactive_prompt.as_mut()?;
        let prompt_count = state.responses.len();
        if prompt_count == 0 {
            return None;
        }
        state.focused_index = if backwards {
            state
                .focused_index
                .checked_sub(1)
                .unwrap_or(prompt_count - 1)
        } else {
            (state.focused_index + 1) % prompt_count
        };
        let index = state.focused_index;
        Some(PromptInputTarget {
            id: format!("ssh.keyboard-interactive.{}.{index}", state.id),
            seed: state.responses[index].clone(),
            echo: state.request.prompts[index].echo,
        })
    }

    pub(in crate::features) fn apply_credential_input(
        &mut self,
        prompt_id: &str,
        text: String,
    ) -> bool {
        let Some(state) = self.active_credential_prompt.as_mut() else {
            return false;
        };
        if state.id != prompt_id {
            return false;
        }
        state.value = text;
        true
    }

    pub(in crate::features) fn apply_keyboard_interactive_input(
        &mut self,
        prompt_id: &str,
        index: usize,
        text: String,
    ) -> bool {
        let Some(state) = self.active_keyboard_interactive_prompt.as_mut() else {
            return false;
        };
        if state.id != prompt_id {
            return false;
        }
        let Some(response) = state.responses.get_mut(index) else {
            return false;
        };
        *response = text;
        state.focused_index = index;
        true
    }

    pub(in crate::features) fn focus_keyboard_interactive_response(
        &mut self,
        prompt_id: &str,
        index: usize,
    ) -> bool {
        let Some(state) = self.active_keyboard_interactive_prompt.as_mut() else {
            return false;
        };
        if state.id != prompt_id || index >= state.responses.len() {
            return false;
        }
        state.focused_index = index;
        true
    }

    pub(in crate::features) fn active_input_target(&self) -> Option<PromptInputTarget> {
        if let Some(state) = self.active_credential_prompt.as_ref() {
            return Some(PromptInputTarget {
                id: format!("ssh.credential.{}", state.id),
                seed: state.value.clone(),
                echo: state.prompt.echo,
            });
        }
        let state = self.active_keyboard_interactive_prompt.as_ref()?;
        let index = (!state.responses.is_empty())
            .then_some(state.focused_index.min(state.responses.len() - 1))?;
        Some(PromptInputTarget {
            id: format!("ssh.keyboard-interactive.{}.{index}", state.id),
            seed: state.responses[index].clone(),
            echo: state.request.prompts[index].echo,
        })
    }

    pub(in crate::features) fn activate_next_host_key(&mut self) -> Option<String> {
        if self.active_host_key_prompt.is_some() || !self.host_key_prompts.has_pending() {
            return None;
        }
        let request = self.host_key_prompts.pop_pending()?;
        let host = request.host_key.host_identifier.clone();
        self.active_host_key_prompt = Some(request);
        Some(host)
    }

    pub(in crate::features) fn activate_next_agent(&mut self) -> Option<String> {
        if self.active_agent_prompt.is_some() || !self.agent_prompts.has_pending() {
            return None;
        }
        let request = self.agent_prompts.pop_pending()?;
        let target = request.target();
        self.active_agent_prompt = Some(request);
        Some(target)
    }

    pub(in crate::features) fn take_next_credential_request(
        &self,
    ) -> Option<CredentialPromptRequest> {
        if self.active_credential_prompt.is_some()
            || self.active_keyboard_interactive_prompt.is_some()
            || !self.credential_prompts.has_pending()
        {
            return None;
        }
        self.credential_prompts.pop_pending()
    }

    pub(in crate::features) fn activate_credential(&mut self, state: CredentialPromptState) {
        self.active_credential_prompt = Some(state);
        self.credential_prompt_focus_pending = true;
    }

    pub(in crate::features) fn activate_keyboard_interactive(
        &mut self,
        state: KeyboardInteractivePromptState,
    ) {
        self.active_keyboard_interactive_prompt = Some(state);
        self.credential_prompt_focus_pending = true;
    }

    pub(in crate::features) fn keyboard_totp_refresh_otp_id(&self, now: u64) -> Option<String> {
        let state = self.active_keyboard_interactive_prompt.as_ref()?;
        if state.otp_type.as_deref() != Some("totp") || state.otp_code.is_none() {
            return None;
        }
        let current_step = now / state.otp_period.max(1);
        if state.otp_time_step == Some(current_step) {
            return None;
        }
        state.request.otp_id.clone()
    }

    pub(in crate::features) fn activate_next_duplicate(&mut self) -> Option<String> {
        if self.active_duplicate_prompt.is_some() || !self.duplicate_prompts.has_pending() {
            return None;
        }
        let request = self.duplicate_prompts.pop_pending()?;
        let target = request.request.target_path.clone();
        self.active_duplicate_prompt = Some(SftpDuplicatePromptState {
            id: request.id,
            request: request.request,
            response_tx: request.response_tx,
        });
        Some(target)
    }
}

impl SessionDialogState {
    pub(in crate::features) fn tab_actions_session_id(&self) -> Option<&str> {
        self.tab_actions_session_id.as_deref()
    }

    pub(in crate::features) fn tab_actions_anchor(&self) -> Option<(f32, f32)> {
        self.tab_actions_anchor
    }

    pub(in crate::features) fn tab_actions_submenu(&self) -> Option<TabActionsSubmenu> {
        self.tab_actions_submenu
    }

    pub(in crate::features) fn tab_actions_focus(&self) -> &FocusHandle {
        &self.tab_actions_focus
    }

    pub(in crate::features) fn open_tab_actions(
        &mut self,
        session_id: String,
        anchor: Option<(f32, f32)>,
    ) {
        self.tab_actions_session_id = Some(session_id);
        self.tab_actions_anchor = anchor;
        self.tab_actions_submenu = None;
    }

    pub(in crate::features) fn close_tab_actions(&mut self) {
        self.tab_actions_session_id = None;
        self.tab_actions_anchor = None;
        self.tab_actions_submenu = None;
    }

    pub(in crate::features) fn select_tab_actions_submenu(
        &mut self,
        submenu: TabActionsSubmenu,
    ) -> bool {
        if self.tab_actions_submenu == Some(submenu) {
            return false;
        }
        self.tab_actions_submenu = Some(submenu);
        true
    }

    pub(in crate::features) fn should_quit_after_close_all(&self) -> bool {
        self.pending_quit_after_close_all
    }

    pub(in crate::features) fn request_quit_after_close_all(&mut self) {
        self.pending_quit_after_close_all = true;
    }

    pub(in crate::features) fn open_close_all_sessions_confirm(&mut self) {
        self.close_tab_actions();
    }

    pub(in crate::features) fn cancel_close_all_sessions_confirm(&mut self) {
        self.pending_quit_after_close_all = false;
        self.pending_window_quit = false;
    }

    pub(in crate::features) fn take_close_all_sessions_confirm(&mut self) -> bool {
        let quit_after = self.pending_quit_after_close_all;
        self.pending_quit_after_close_all = false;
        self.pending_window_quit = false;
        quit_after
    }

    pub(in crate::features) fn rename_draft(&self) -> &str {
        &self.rename_draft
    }

    pub(in crate::features) fn open_rename(&mut self, session_id: String, current_name: &str) {
        self.rename_session_id = Some(session_id);
        self.rename_draft = current_name.chars().take(64).collect();
    }

    pub(in crate::features) fn cancel_rename(&mut self) {
        self.rename_session_id = None;
        self.rename_draft.clear();
    }

    pub(in crate::features) fn take_rename_submission(&mut self) -> RenameSessionSubmission {
        let Some(session_id) = self.rename_session_id.take() else {
            return RenameSessionSubmission::Inactive;
        };
        let name = self
            .rename_draft
            .trim()
            .chars()
            .take(64)
            .collect::<String>();
        self.rename_draft.clear();
        if name.is_empty() {
            self.rename_session_id = Some(session_id);
            return RenameSessionSubmission::Empty;
        }
        RenameSessionSubmission::Ready { session_id, name }
    }

    pub(in crate::features) fn color_picker_is_open(&self) -> bool {
        self.color_picker_open
    }

    pub(in crate::features) fn color_picker_focus(&self) -> &FocusHandle {
        &self.color_picker_focus
    }

    pub(in crate::features) fn close_color_picker(&mut self) {
        self.color_picker_open = false;
    }

    pub(in crate::features) fn session_info_is_open(&self) -> bool {
        self.session_info_open
    }

    pub(in crate::features) fn session_info_focus(&self) -> &FocusHandle {
        &self.session_info_focus
    }

    pub(in crate::features) fn open_session_info(&mut self) {
        self.session_info_open = true;
    }

    pub(in crate::features) fn close_session_info(&mut self) {
        self.session_info_open = false;
    }

    pub(in crate::features) fn startup_command_draft(&self) -> &str {
        &self.startup_command_draft
    }

    pub(in crate::features) fn startup_command_delay_ms(&self) -> u64 {
        self.startup_command_delay_ms
    }

    pub(in crate::features) fn open_startup_command(
        &mut self,
        action: StartupCommandAction,
        delay_ms: u64,
    ) {
        self.startup_command_open = true;
        self.startup_command_action = action;
        self.startup_command_draft.clear();
        self.startup_command_delay_ms = delay_ms.min(60_000);
    }

    pub(in crate::features) fn cancel_startup_command(&mut self) -> StartupCommandAction {
        let action = self.startup_command_action;
        self.startup_command_open = false;
        self.startup_command_action = StartupCommandAction::Duplicate;
        self.startup_command_draft.clear();
        self.startup_command_delay_ms = DEFAULT_DUPLICATE_STARTUP_DELAY_MS;
        action
    }

    pub(in crate::features) fn take_startup_command(
        &mut self,
    ) -> Option<(StartupCommandAction, StartupCommandRequest)> {
        let command = self.startup_command_draft.trim().to_string();
        if command.is_empty() {
            return None;
        }
        let request = StartupCommandRequest {
            command,
            delay_ms: self.startup_command_delay_ms.min(60_000),
        };
        let action = self.startup_command_action;
        self.startup_command_open = false;
        self.startup_command_action = StartupCommandAction::Duplicate;
        self.startup_command_draft.clear();
        Some((action, request))
    }

    pub(in crate::features) fn set_startup_command_delay(&mut self, delay_ms: u64) {
        self.startup_command_delay_ms = delay_ms.min(60_000);
    }

    pub(in crate::features) fn reset_startup_command_delay(&mut self) {
        self.startup_command_delay_ms = 0;
    }

    pub(in crate::features) fn temporary_ssh_link_draft(&self) -> &str {
        &self.temporary_ssh_link_draft
    }

    pub(in crate::features) fn temporary_link_protocol(&self) -> TemporaryLinkProtocol {
        self.temporary_link_protocol
    }

    pub(in crate::features) fn temporary_serial_port_name(&self) -> &str {
        &self.temporary_serial_port_name
    }

    pub(in crate::features) fn temporary_serial_baud_rate(&self) -> &str {
        &self.temporary_serial_baud_rate
    }

    pub(in crate::features) fn temporary_ssh_link_error(&self) -> Option<&'static str> {
        self.temporary_ssh_link_error
    }

    pub(in crate::features) fn open_temporary_ssh_link(&mut self) {
        self.temporary_ssh_link_open = true;
        self.temporary_ssh_link_error = None;
    }

    pub(in crate::features) fn close_temporary_ssh_link(&mut self) {
        self.temporary_ssh_link_open = false;
        self.temporary_link_protocol = TemporaryLinkProtocol::Ssh;
        self.temporary_ssh_link_draft.clear();
        self.temporary_serial_port_name.clear();
        self.temporary_serial_baud_rate = "115200".to_string();
        self.temporary_ssh_link_error = None;
    }

    pub(in crate::features) fn set_temporary_link_protocol(
        &mut self,
        protocol: TemporaryLinkProtocol,
    ) {
        self.temporary_link_protocol = protocol;
        self.temporary_ssh_link_error = None;
    }

    pub(in crate::features) fn reject_temporary_ssh_link(&mut self, error: &'static str) {
        self.temporary_ssh_link_error = Some(error);
    }

    pub(in crate::features) fn apply_temporary_ssh_link(&mut self, text: String) {
        self.temporary_ssh_link_draft = text;
        self.temporary_ssh_link_error = None;
    }

    pub(in crate::features) fn apply_temporary_serial_port_name(&mut self, text: String) {
        self.temporary_serial_port_name = text;
        self.temporary_ssh_link_error = None;
    }

    pub(in crate::features) fn apply_temporary_serial_baud_rate(&mut self, text: String) {
        self.temporary_serial_baud_rate = text.chars().filter(|ch| ch.is_ascii_digit()).collect();
        self.temporary_ssh_link_error = None;
    }

    pub(in crate::features) fn apply_text_input(&mut self, field: &str, text: String) -> bool {
        match field {
            "rename" => self.rename_draft = text,
            "startup-command" => self.startup_command_draft = text,
            _ => return false,
        }
        true
    }
}

pub(in crate::features) struct PendingSessionStart {
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

/// A session start that remains visible after its worker failed.
///
/// Tauri keeps the failed pane in its original tab, so the GPUI shell must
/// retain the pending metadata instead of reducing the failure to a global
/// banner.
pub(in crate::features) struct FailedSessionStart {
    pub pending: PendingSessionStart,
    pub error: String,
}

#[derive(Clone, Copy, Debug)]
pub(in crate::features) struct SessionStartTabPlacement {
    pub(in crate::features) insert_index: usize,
    pub(in crate::features) request_sequence: u64,
    pub(in crate::features) requested_at: Instant,
}

pub(super) struct SessionStartFeatureState {
    tx: UnboundedSender<SessionStartResult>,
    /// Taken once by `NyaTermApp::start_session_start_event_drain`, which owns
    /// delivery from then on. `None` afterwards, so a second start is a no-op.
    rx: Option<UnboundedReceiver<SessionStartResult>>,
    pending: HashMap<String, PendingSessionStart>,
    active_pending: Option<String>,
    failed: HashMap<String, FailedSessionStart>,
    active_failed: Option<String>,
    cancelled: HashSet<String>,
    reconnect_failures: HashMap<String, String>,
    pending_workspace_split: Option<(WorkspaceSplitDirection, String)>,
    preparing_saved_connections: HashMap<String, SessionStartTabPlacement>,
    next_request_sequence: u64,
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
            pending_workspace_split: None,
            preparing_saved_connections: HashMap::new(),
            next_request_sequence: 0,
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

    pub(in crate::features) fn has_failed(&self) -> bool {
        !self.failed.is_empty()
    }

    pub(in crate::features) fn pending_count(&self) -> usize {
        self.pending.len()
    }

    pub(in crate::features) fn visible_tab_reservation_count(&self) -> usize {
        self.pending
            .values()
            .filter(|pending| pending.reconnect_session_id.is_none())
            .count()
            .saturating_add(self.failed.len())
            .saturating_add(self.preparing_saved_connections.len())
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
        placement
    }

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

    pub(in crate::features) fn reserve_saved_connection_start(
        &mut self,
        connection_id: &str,
        placement: SessionStartTabPlacement,
    ) -> bool {
        if self.source_connection_is_pending(connection_id)
            || self.preparing_saved_connections.contains_key(connection_id)
        {
            return false;
        }
        self.preparing_saved_connections
            .insert(connection_id.to_string(), placement);
        true
    }

    pub(in crate::features) fn release_saved_connection_start(&mut self, connection_id: &str) {
        self.preparing_saved_connections.remove(connection_id);
    }

    pub(in crate::features) fn saved_connection_is_preparing(&self, connection_id: &str) -> bool {
        self.preparing_saved_connections.contains_key(connection_id)
    }

    pub(in crate::features) fn register_pending(
        &mut self,
        request_id: String,
        pending: PendingSessionStart,
    ) -> bool {
        if let (Some(connection_id), Some(placement)) = (
            pending.source_connection_id.as_deref(),
            pending.tab_placement,
        ) && self
            .preparing_saved_connections
            .get(connection_id)
            .is_some_and(|preparing| preparing.request_sequence == placement.request_sequence)
        {
            self.preparing_saved_connections.remove(connection_id);
        }
        let reconnecting = pending.reconnect_session_id.is_some();
        self.pending.insert(request_id.clone(), pending);
        if !reconnecting {
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
        &mut self,
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
    }

    pub(in crate::features) fn reconnect_failure(&self, session_id: &str) -> Option<&str> {
        self.reconnect_failures.get(session_id).map(String::as_str)
    }

    pub(in crate::features) fn clear_reconnect_failure(&mut self, session_id: &str) {
        self.reconnect_failures.remove(session_id);
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

    pub(in crate::features) fn close_pending(
        &mut self,
        request_id: &str,
    ) -> Option<PendingSessionStart> {
        let pending = self.pending.remove(request_id)?;
        self.cancelled.insert(request_id.to_string());
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

    fn latest_pending_request_id(&self) -> Option<String> {
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

    fn latest_failed_request_id(&self) -> Option<String> {
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

pub(super) fn pending_session_start_display_name(pending: &PendingSessionStart) -> String {
    pending
        .custom_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or(&pending.connection_name)
        .to_string()
}

pub(super) fn failed_session_start_display_name(failed: &FailedSessionStart) -> String {
    pending_session_start_display_name(&failed.pending)
}

#[cfg(test)]
mod tests;
