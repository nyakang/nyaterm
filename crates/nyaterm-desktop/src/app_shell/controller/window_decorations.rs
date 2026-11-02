use gpui::{App, WindowDecorations};
use nyaterm_core::models::window_decoration::WindowDecoration;

use crate::app_shell::DesktopController;

impl DesktopController {
    pub(crate) fn requires_client_decorations(
        &self,
        handle: gpui::AnyWindowHandle,
        cx: &App,
    ) -> bool {
        self.windows.values().any(|entry| {
            entry
                .shell
                .upgrade()
                .and_then(|shell| shell.read(cx).app.clone())
                .is_some_and(|app| app.read(cx).requires_client_decorations(handle))
        })
    }

    #[cfg(test)]
    pub(crate) fn set_decoration_test_process_state(
        &mut self,
        state: gpui::Entity<crate::app_shell::ProcessStateStore>,
    ) {
        self.process_state = Some(state);
    }

    /// The settings owner's draft is the only process-wide appearance preview.
    /// Other workspaces retain their committed settings until Apply succeeds.
    pub(crate) fn effective_window_decorations(&self, cx: &App) -> WindowDecorations {
        let preview = self
            .settings_owners
            .last()
            .and_then(|id| self.windows.get(id))
            .and_then(|entry| entry.shell.upgrade())
            .and_then(|shell| shell.read(cx).app.clone())
            .and_then(|app| app.read(cx).window_decoration_preview());
        let committed = self
            .process_state
            .as_ref()
            .map(|state| &state.read(cx).snapshot().settings.window_decorations);
        match preview.as_ref().or(committed) {
            Some(WindowDecoration::Server) => WindowDecorations::Server,
            _ => WindowDecorations::Client,
        }
    }
}
