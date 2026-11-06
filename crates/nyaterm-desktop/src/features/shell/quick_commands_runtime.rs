use gpui::{Context, Window};
use rust_i18n::t;

use crate::features::{NyaTermApp, text_inputs::TextInputSetup};
use crate::models::BottomPanelMode;

impl NyaTermApp {
    pub(in crate::features) fn open_quick_commands_panel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_bottom_panel_mode(BottomPanelMode::QuickCommands);
        self.finish_quick_commands_panel_change(window, cx);
    }

    pub(in crate::features) fn toggle_quick_commands_panel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shell.bottom_panel.toggle_quick_commands();
        self.persist_ui_layout();
        self.finish_quick_commands_panel_change(window, cx);
    }

    fn finish_quick_commands_panel_change(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.shell.bottom_panel_mode() == BottomPanelMode::QuickCommands {
            let search = self.commands.quick_search_draft().to_string();
            let field = self.text_input(
                "quick-command.search",
                &search,
                TextInputSetup::placeholder(t!("quickCommands.search")),
                cx,
            );
            window.focus(&field.read(cx).focus_handle(), cx);
            self.shell.set_status("quick commands opened".to_string());
        } else {
            self.focus_active_workspace_surface(window, cx);
            self.shell.set_status("quick commands hidden".to_string());
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests;
