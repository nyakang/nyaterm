use gpui::{Context, Window};
use rust_i18n::t;

use crate::features::{NyaTermApp, text_inputs::TextInputSetup};
use crate::models::{BottomPanelMode, NavItem, PanelSide, QuickCommandViewMode};

impl NyaTermApp {
    pub(in crate::features) fn quick_commands_is_visible(&self) -> bool {
        match self.commands.quick_view_mode() {
            QuickCommandViewMode::List
            | QuickCommandViewMode::Compact
            | QuickCommandViewMode::Tile => {
                self.shell.bottom_panel_mode() == BottomPanelMode::QuickCommands
            }
            QuickCommandViewMode::CommandBar => self.shell.command_bar_visible(),
            QuickCommandViewMode::Sidebar => self.panel_entry_selected(NavItem::QuickCommands),
        }
    }

    pub(in crate::features) fn close_quick_commands_side_panel(&mut self) {
        self.clear_activity_entry_from_side("quickCmdBar", PanelSide::Left);
        self.clear_activity_entry_from_side("quickCmdBar", PanelSide::Right);
    }

    pub(in crate::features) fn open_quick_commands_panel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.commands.quick_view_mode() {
            QuickCommandViewMode::List
            | QuickCommandViewMode::Compact
            | QuickCommandViewMode::Tile => {
                self.shell.set_command_bar_visible(false);
                self.close_quick_commands_side_panel();
                self.set_bottom_panel_mode(BottomPanelMode::QuickCommands);
                self.finish_quick_commands_panel_change(window, cx);
            }
            QuickCommandViewMode::CommandBar => {
                if self.shell.bottom_panel_mode() == BottomPanelMode::QuickCommands {
                    self.shell.bottom_panel.mode = BottomPanelMode::Hidden;
                }
                self.close_quick_commands_side_panel();
                self.commands.close_quick_command_bar_popup();
                self.shell.set_command_bar_visible(true);
                self.persist_ui_layout();
                self.focus_active_workspace_surface(window, cx);
                self.shell
                    .set_status("quick command bar opened".to_string());
                cx.notify();
            }
            QuickCommandViewMode::Sidebar => {
                if self.shell.bottom_panel_mode() == BottomPanelMode::QuickCommands {
                    self.shell.bottom_panel.mode = BottomPanelMode::Hidden;
                }
                self.shell.set_command_bar_visible(false);
                self.open_quick_commands_sidebar_surface(cx);
                self.persist_ui_layout();
                self.focus_quick_command_search(window, cx);
                self.shell
                    .set_status("quick commands sidebar opened".to_string());
                cx.notify();
            }
        }
    }

    pub(in crate::features) fn toggle_quick_commands_panel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.commands.quick_view_mode() {
            QuickCommandViewMode::List
            | QuickCommandViewMode::Compact
            | QuickCommandViewMode::Tile => {
                self.shell.set_command_bar_visible(false);
                self.close_quick_commands_side_panel();
                self.shell.bottom_panel.toggle_quick_commands();
                self.persist_ui_layout();
                self.finish_quick_commands_panel_change(window, cx);
            }
            QuickCommandViewMode::CommandBar => {
                let next = !self.shell.command_bar_visible();
                self.commands.close_quick_command_bar_popup();
                self.shell.set_command_bar_visible(next);
                self.persist_ui_layout();
                self.focus_active_workspace_surface(window, cx);
                self.shell.set_status(if next {
                    "quick command bar opened".to_string()
                } else {
                    "quick command bar hidden".to_string()
                });
                cx.notify();
            }
            QuickCommandViewMode::Sidebar => {
                if self.panel_entry_selected(NavItem::QuickCommands) {
                    self.close_quick_commands_side_panel();
                    self.persist_ui_layout();
                    self.focus_active_workspace_surface(window, cx);
                    self.shell
                        .set_status("quick commands sidebar hidden".to_string());
                } else {
                    self.open_quick_commands_sidebar_surface(cx);
                    self.persist_ui_layout();
                    self.focus_quick_command_search(window, cx);
                    self.shell
                        .set_status("quick commands sidebar opened".to_string());
                }
                cx.notify();
            }
        }
    }

    pub(in crate::features) fn close_quick_command_bar(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.commands.close_quick_command_bar_popup();
        if self.shell.set_command_bar_visible(false) {
            self.persist_ui_layout();
        }
        self.focus_active_workspace_surface(window, cx);
        self.shell
            .set_status("quick command bar hidden".to_string());
        cx.notify();
    }

    pub(in crate::features) fn open_quick_commands_sidebar_surface(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        let side = self.panel_side_for_item(NavItem::QuickCommands);
        self.ensure_panel_open(NavItem::QuickCommands, cx);
        if !cfg!(target_os = "macos") && !self.shell.panel_is_floating() {
            match side {
                Some(PanelSide::Left) if self.shell.viewport.size.0 < 1024. => {
                    self.shell.panels.mobile_left_open = true;
                }
                Some(PanelSide::Right) if self.shell.viewport.size.0 < 768. => {
                    self.shell.panels.mobile_right_open = true;
                }
                _ => {}
            }
        }
    }

    pub(in crate::features) fn focus_quick_command_search(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let search = self.commands.quick_search_draft().to_string();
        let field = self.text_input(
            "quick-command.search",
            &search,
            TextInputSetup::placeholder(t!("quickCommands.search")),
            cx,
        );
        window.focus(&field.read(cx).focus_handle(), cx);
    }

    fn finish_quick_commands_panel_change(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.shell.bottom_panel_mode() == BottomPanelMode::QuickCommands {
            self.focus_quick_command_search(window, cx);
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
