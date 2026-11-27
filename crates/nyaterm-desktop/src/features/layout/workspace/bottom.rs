use gpui::{Context, div, prelude::*, px};

use crate::features::NyaTermApp;
use crate::models::BottomPanelMode;

impl NyaTermApp {
    pub(in crate::features) fn bottom_panel_view(
        &mut self,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let resizable_panel = match self.shell.bottom_panel_mode() {
            BottomPanelMode::QuickCommands if self.commands.quick_view_mode().is_bottom_panel() => {
                let palette = self.theme_palette();
                Some(
                    div()
                        .h(px(self.shell.quick_commands_height().clamp(36., 520.)))
                        .flex_none()
                        .bg(self.shell_surface_color(palette.surface))
                        .child(self.quick_commands_panel(cx))
                        .into_any_element(),
                )
            }
            BottomPanelMode::CommandSend => {
                Some(self.bottom_command_send_bar(cx).into_any_element())
            }
            BottomPanelMode::QuickCommands | BottomPanelMode::Hidden => None,
        };
        let command_bar = (self.commands.quick_view_mode().is_command_bar()
            && self.shell.command_bar_visible())
        .then(|| self.quick_command_bar_view(cx).into_any_element());

        match (resizable_panel, command_bar) {
            (None, None) => div().into_any_element(),
            (Some(panel), None) => panel,
            (None, Some(bar)) => bar,
            (Some(panel), Some(bar)) => div()
                .flex_none()
                .w_full()
                .flex()
                .flex_col()
                .child(panel)
                .child(bar)
                .into_any_element(),
        }
    }
}
