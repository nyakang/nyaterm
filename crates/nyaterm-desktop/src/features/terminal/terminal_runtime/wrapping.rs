use gpui::Context;
use nyaterm_core::models::settings::TerminalWrapMode;

use crate::features::NyaTermApp;

impl NyaTermApp {
    /// Apply a settings preview, rollback or reload even when no terminal is visible.
    pub(in crate::features) fn sync_terminal_wrap_mode(&mut self, cx: &mut Context<Self>) {
        let mode = self.settings.summary().terminal_wrap_mode;
        let previous = self.terminal.view.frame_pipeline.layout_generation();
        let generation = self
            .terminal
            .view
            .frame_pipeline
            .set_logical_line_snapshots(mode == TerminalWrapMode::None);
        if previous == generation {
            return;
        }
        for (id, view) in &mut self.terminal.view.views {
            if let Some(surface) = self.terminal.view.surfaces.get(id)
                && !surface.read(cx).wrap_layout_pending()
            {
                let (history, offset) =
                    surface.update(cx, |surface, _| surface.begin_wrap_layout_change());
                view.scroll_offset = view.history_state().reanchor_offset(history, offset);
            }
            view.invalidate_wrap_layout();
        }
        if mode == TerminalWrapMode::None {
            self.clear_terminal_block_selection(cx);
        }
        self.terminal.view.scroll_delta_residuals.clear();
        if let Some(view) = self
            .session
            .active_id()
            .and_then(|id| self.terminal.view.views.get(id))
        {
            self.terminal.view.scroll_offset = view.scroll_offset;
        }
        let ids = self
            .visible_terminal_session_ids()
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        for id in ids {
            self.request_terminal_live_snapshot(&id);
            if let Some(offset) = self
                .terminal
                .view
                .views
                .get(&id)
                .map(|view| view.scroll_offset)
                .filter(|offset| *offset > 0)
            {
                self.request_terminal_frame_snapshot_for_user_scroll(&id, offset);
            }
            self.notify_terminal_surface_only(Some(&id), cx);
        }
    }
}

#[cfg(test)]
mod tests;
