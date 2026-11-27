use crate::models::HeaderStatusMode;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SessionToolbarAction {
    Fullscreen,
    Reconnect,
}

/// Support is independent of connection readiness and of probe results.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct ToolbarCapabilities {
    pub host: bool,
    pub accelerators: bool,
    pub host_available: bool,
    pub fullscreen: Option<bool>,
    pub reconnect: Option<bool>,
    pub disconnected: bool,
    pub reconnect_requires_disconnect: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::features) struct SessionToolbarContext {
    pub(super) session_id: Option<String>,
    pub(super) capabilities: ToolbarCapabilities,
    pub(super) preferred_mode: HeaderStatusMode,
    pub(super) visible: bool,
    pub(super) fullscreen_active: bool,
}

impl SessionToolbarContext {
    pub(super) fn supports_mode(&self, mode: HeaderStatusMode) -> bool {
        match mode {
            HeaderStatusMode::Session | HeaderStatusMode::DateTime => true,
            HeaderStatusMode::Resources | HeaderStatusMode::Host => self.capabilities.host,
            HeaderStatusMode::Gpu | HeaderStatusMode::Npu => self.capabilities.accelerators,
        }
    }

    pub(super) fn mode_enabled(&self, mode: HeaderStatusMode) -> bool {
        self.supports_mode(mode)
            && (matches!(mode, HeaderStatusMode::Session | HeaderStatusMode::DateTime)
                || self.capabilities.host_available)
    }

    pub(in crate::features) fn effective_mode(&self) -> HeaderStatusMode {
        if self.mode_enabled(self.preferred_mode) {
            self.preferred_mode
        } else {
            HeaderStatusMode::Session
        }
    }

    pub(in crate::features) fn displays(&self, mode: HeaderStatusMode) -> bool {
        self.visible && self.effective_mode() == mode
    }

    pub(super) fn action_enabled(&self, action: SessionToolbarAction) -> Option<bool> {
        match action {
            SessionToolbarAction::Fullscreen => self.capabilities.fullscreen,
            SessionToolbarAction::Reconnect => self.capabilities.reconnect,
        }
    }

    pub(super) fn accepts_action(&self, session_id: &str, action: SessionToolbarAction) -> bool {
        self.session_id.as_deref() == Some(session_id) && self.action_enabled(action) == Some(true)
    }

    pub(super) fn shortcut_actions(&self, width: f32) -> Vec<SessionToolbarAction> {
        if width < 1024. {
            return Vec::new();
        }
        [
            SessionToolbarAction::Fullscreen,
            SessionToolbarAction::Reconnect,
        ]
        .into_iter()
        .filter(|action| {
            self.action_enabled(*action).is_some()
                && (*action == SessionToolbarAction::Fullscreen
                    || (self.capabilities.disconnected
                        && self.action_enabled(*action) == Some(true)))
        })
        .collect()
    }
}
