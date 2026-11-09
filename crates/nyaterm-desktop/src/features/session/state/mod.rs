//! Authoritative session state and focused transition modules.

use self::catalog::{ActiveSessionState, SessionTabDragState};
use self::dialogs::SessionDialogState;
use self::events::SessionEventQueueState;
use self::prompts::SessionPromptState;
use self::restore::SessionRestoreState;
use self::start::{DEFAULT_DUPLICATE_STARTUP_DELAY_MS, SessionStartFeatureState};
use crate::app_shell::session_hub::ssh_connections::SshConnectionPool;
use crate::features::session::SessionProtocolRuntimeState;
use crate::features::session::auth_runtime::NativeOtpProvider;
use crate::features::session::notices::SessionNoticeState;
use crate::models::{SessionEventBridge, SessionRuntimeMetadata, StartupCommandAction};
use crate::temporary_ssh_link::TemporaryLinkProtocol;
use gpui::{FocusHandle, ScrollHandle, UniformListScrollHandle};
use nyaterm_transport::SessionManager;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

mod catalog;
mod dialogs;
mod events;
mod prompts;
mod protocols;
mod restore;
mod start;

pub(crate) use catalog::SessionCatalogTransferBundle;
pub(in crate::features) use dialogs::RenameSessionSubmission;
pub(in crate::features) use prompts::PromptResolution;
pub(in crate::features) use start::{
    PendingReconnectCompletion, PendingSessionStart, SavedConnectionStartOptions,
    SessionStartEndReason, SessionStartEventRequest, SessionStartTabPlacement,
    StartTabActionSnapshot,
};

pub(in crate::features) struct SessionFeatureState {
    pub(in crate::features) unsupported_restore_tabs: Vec<nyaterm_core::RestorableOpenTab>,
    pub(in crate::features) notices: SessionNoticeState,
    manager: Arc<SessionManager>,
    ssh_connections: SshConnectionPool,
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

pub(in crate::features) struct SessionFeatureFocus {
    pub credential: FocusHandle,
    pub tab_actions: FocusHandle,
    pub color_picker: FocusHandle,
    pub info: FocusHandle,
}

pub(in crate::features) struct SessionDisconnectUpdate {
    pub already_disconnected: bool,
}

#[cfg(test)]
mod tests;

mod tab_actions;

impl SessionFeatureState {
    pub(in crate::features) fn ssh_connection_pool(&self) -> SshConnectionPool {
        self.ssh_connections.clone()
    }

    #[cfg(test)]
    pub(in crate::features) fn shutdown_workers(&mut self) {
        self.take_shutdown()();
    }

    pub(in crate::features) fn take_shutdown(&mut self) -> impl FnOnce() + Send + 'static {
        // Reject late worker results so their pending-session guards clean up
        // before the background shutdown barrier, even if the window is gone.
        self.start.tx.close_channel();
        let pending_ids = self.start.pending.keys().cloned().collect::<Vec<_>>();
        for id in pending_ids {
            self.start.close_pending(&id);
        }
        let mut protocols = std::mem::take(&mut self.protocols);
        let bridge_shutdown = self.event_bridge.take_shutdown();
        move || {
            bridge_shutdown();
            protocols.shutdown_workers();
        }
    }

    pub(in crate::features) fn new(
        manager: Arc<SessionManager>,
        ssh_connections: SshConnectionPool,
        event_bridge: SessionEventBridge,
        otp_provider: Arc<NativeOtpProvider>,
        focus: SessionFeatureFocus,
    ) -> Self {
        Self {
            unsupported_restore_tabs: Vec::new(),
            manager,
            ssh_connections,
            event_bridge,
            start: SessionStartFeatureState::new(),
            restore: SessionRestoreState::default(),
            events: SessionEventQueueState::default(),
            prompts: SessionPromptState::new(otp_provider, focus.credential),
            dialogs: SessionDialogState {
                tab_actions_target: None,
                tab_actions_start_snapshot: None,
                tab_actions_anchor: None,
                tab_actions_submenu: None,
                tab_actions_focus: focus.tab_actions,
                tab_actions_restore_focus: None,
                pending_quit_after_close_all: false,
                pending_window_quit: false,
                rename_target: None,
                rename_dialog_handle: None,
                rename_draft: String::new(),
                color_picker_open: false,
                color_picker_focus: focus.color_picker,
                session_info_open: false,
                session_info_focus: focus.info,
                session_info_offset: (0., 0.),
                session_info_drag: None,
                session_info_scroll: ScrollHandle::new(),
                session_info_dialog_height: 0.,
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
            busy_actions: HashMap::new(),
            notices: SessionNoticeState::default(),
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
}

pub(super) use start::{failed_session_start_display_name, pending_session_start_display_name};
