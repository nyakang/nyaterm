mod policy;
mod view;

#[cfg(test)]
mod tests;

use gpui::{Context, Window};
use nyaterm_core::models::ssh_runtime::SshRuntimeMode;
use nyaterm_remote_desktop::RemoteDesktopViewState;

use crate::features::NyaTermApp;
use crate::models::{HeaderStatusMode, SessionLaunchConfig, WorkspaceTabId};

pub(crate) use policy::SessionToolbarAction;
use policy::{SessionToolbarContext, ToolbarCapabilities};

impl NyaTermApp {
    /// Derive from the focused pane, never from the tab root or a cached toolbar owner.
    pub(in crate::features) fn session_toolbar_context(&self) -> SessionToolbarContext {
        let mut context = SessionToolbarContext {
            session_id: None,
            capabilities: ToolbarCapabilities::default(),
            preferred_mode: HeaderStatusMode::from_setting(
                &self.settings.summary().ui_header_status_mode,
            ),
            visible: self.settings.summary().ui_header_status_visible,
            fullscreen_active: false,
        };
        if self.session.start_has_active_pending()
            || self.session.start_has_active_failed()
            || self
                .active_workspace_tab()
                .is_some_and(|tab| !WorkspaceTabId::from_layout_key(&tab).is_session())
        {
            return context;
        }
        let Some(id) = self.session.active_id() else {
            return context;
        };
        let Some(metadata) = self.session.metadata(id) else {
            return context;
        };
        context.session_id = Some(id.to_string());
        let pending = self.session.session_is_busy(id)
            || self.session.start_reconnect_is_pending(id)
            || self.remote_desktop.start_is_pending(id);
        let local = matches!(metadata.launch_config, SessionLaunchConfig::Local(_));
        let host = local
            || (matches!(metadata.launch_config, SessionLaunchConfig::Ssh(_))
                && metadata.ssh_runtime_mode() == Some(SshRuntimeMode::Standard)
                && metadata
                    .ssh_config
                    .as_ref()
                    .is_some_and(|config| config.remote_stats_enabled()));
        context.capabilities.host = host;
        context.capabilities.accelerators = host && !(local && cfg!(windows));
        context.capabilities.host_available = host && !metadata.disconnected && !pending;
        context.capabilities.disconnected = metadata.disconnected;
        match &metadata.launch_config {
            SessionLaunchConfig::Local(_)
            | SessionLaunchConfig::Ssh(_)
            | SessionLaunchConfig::Telnet(_)
            | SessionLaunchConfig::Serial(_) => {
                context.capabilities.reconnect = Some(!pending);
            }
            SessionLaunchConfig::Rdp(_) | SessionLaunchConfig::Vnc(_) => {
                context.capabilities.reconnect_requires_disconnect = true;
                let state = self.remote_desktop.session_view_state(id);
                let retryable = matches!(
                    state,
                    Some(RemoteDesktopViewState::Failed | RemoteDesktopViewState::Disconnected)
                );
                context.capabilities.disconnected = retryable;
                context.capabilities.reconnect = Some(retryable && !pending);
                if matches!(metadata.launch_config, SessionLaunchConfig::Rdp(_)) {
                    context.fullscreen_active = self.is_remote_desktop_fullscreen(id);
                    context.capabilities.fullscreen = Some(
                        context.fullscreen_active
                            || (self.remote_desktop_fullscreen_supported(id)
                                && state == Some(RemoteDesktopViewState::Connected)
                                && !pending),
                    );
                }
            }
            _ => {}
        }
        context
    }

    pub(in crate::features) fn execute_session_toolbar_action(
        &mut self,
        session_id: &str,
        action: SessionToolbarAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.security.screen_locked()
            || !self
                .session_toolbar_context()
                .accepts_action(session_id, action)
            || self
                .shell
                .main_window()
                .is_some_and(|main| main.window_id() != window.window_handle().window_id())
        {
            return;
        }
        match action {
            SessionToolbarAction::Fullscreen => {
                self.toggle_remote_desktop_fullscreen(session_id, window, cx);
            }
            SessionToolbarAction::Reconnect => {
                self.reconnect_session(session_id.to_string(), window, cx);
            }
        }
    }
}
