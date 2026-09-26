use gpui::{Context, Div, InteractiveElement, Stateful, Window};

use crate::features::NyaTermApp;
use crate::models::{BottomPanelMode, NavItem, StartupCommandAction};
use crate::shortcuts::{
    CloseTab, CopySelectedConnections, DuplicateSession, DuplicateSessionWithCommand, LockScreen,
    ManageSyncGroups, MultiplexSsh, MultiplexSshWithCommand, NewLocalTerminal, NewSession, NextTab,
    OpenChat, OpenNewSessionMenu, OpenSettings, PreviousTab, QuickSwitch, RenameFile, ResetZoom,
    ShortcutId, ShortcutInvocation, ShowAllCommands, ShowCommandSuggestions, SwitchToTab,
    TemporarySshLink, TerminalClear, TerminalCopy, TerminalFind, TerminalPaste,
    TerminalPasteSelected, TerminalSelectAll, ToggleLeftSidebar, ToggleNativeFullscreen,
    TogglePaneFocus, ToggleRecording, ToggleRightSidebar, ZoomIn, ZoomOut,
};

fn shortcut_interceptor_dispatches(id: ShortcutId) -> bool {
    // GPUI still delivers KeyDown after an interceptor stops propagation.
    // This action must run in the keymap instead, or the terminal sends ^L twice.
    id != ShortcutId::TerminalClear
}

impl NyaTermApp {
    pub(in crate::features) fn ensure_shortcut_interceptor(&mut self, cx: &mut Context<Self>) {
        if self.shell.runtime.shortcut_interceptor.is_some() || self.shell.main_window().is_none() {
            return;
        }
        let app = cx.weak_entity();
        let subscription = cx.intercept_keystrokes(move |event, window, cx| {
            let handled = app
                .update(cx, |this, cx| {
                    if this.settings.keybinding_recording_id().is_some()
                        || this.shell.main_window().is_none_or(|main| {
                            main.window_id() != window.window_handle().window_id()
                        })
                    {
                        return false;
                    }
                    let Some(invocation) = crate::shortcuts::resolve_shortcut_invocation(
                        &event.keystroke,
                        &event.context_stack,
                        cx,
                    ) else {
                        return false;
                    };
                    if !shortcut_interceptor_dispatches(invocation.id) {
                        return false;
                    }
                    this.execute_shortcut_invocation(invocation, window, cx)
                })
                .unwrap_or(false);
            if handled {
                cx.stop_propagation();
            }
        });
        self.shell.runtime.shortcut_interceptor = Some(subscription);
    }

    pub(in crate::features) fn execute_shortcut_invocation(
        &mut self,
        invocation: ShortcutInvocation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        match invocation.id {
            ShortcutId::TerminalCopy => self.copy_terminal_selection_or_visible(cx),
            ShortcutId::TerminalPaste => self.paste_from_clipboard(window, cx),
            ShortcutId::TerminalPasteSelected => {
                if let Some(text) = self.selected_terminal_text() {
                    self.paste_terminal_text(text, window, cx);
                }
            }
            ShortcutId::TerminalFind => self.open_terminal_search(window, cx),
            ShortcutId::TerminalClear => self.send_terminal_clear_screen(cx),
            ShortcutId::TerminalSelectAll => self.select_all_terminal(cx),
            ShortcutId::ManageSyncGroups => self.open_sync_groups(window, cx),
            ShortcutId::ShowCommandSuggestions => self.show_manual_command_suggestions(cx),
            ShortcutId::ToggleRecording => self.toggle_active_session_recording(cx),
            ShortcutId::NewSession => self.open_connection_editor(None, None, false, window, cx),
            ShortcutId::OpenNewSessionMenu => {
                if self.session.active_id().is_some() {
                    let anchor = self
                        .shell
                        .focused_terminal_leaf()
                        .map(|id| {
                            crate::features::shell::NewSessionMenuAnchor::TerminalLeaf(
                                id.to_string(),
                            )
                        })
                        .unwrap_or(crate::features::shell::NewSessionMenuAnchor::MainTabStrip);
                    self.open_new_session_menu(anchor, cx);
                }
            }
            ShortcutId::TemporarySshLink => self.open_temporary_ssh_link_dialog(window, cx),
            ShortcutId::QuickSwitch => self.open_quick_switch(window, cx),
            ShortcutId::NewLocalTerminal => self.start_local_session(window, cx),
            ShortcutId::CloseTab => self.close_active_session(cx),
            ShortcutId::NextTab => self.select_relative_session(1, cx),
            ShortcutId::PreviousTab => self.select_relative_session(-1, cx),
            ShortcutId::SwitchToTab => {
                let index = invocation.tab_index.unwrap_or(1);
                let index = if index == 9 {
                    self.session.ordered_sessions().len().saturating_sub(1)
                } else {
                    index.saturating_sub(1)
                };
                self.select_session_index(index, cx);
            }
            ShortcutId::DuplicateSession => self.duplicate_active_session(window, cx),
            ShortcutId::MultiplexSsh => self.multiplex_active_ssh_session(window, cx),
            ShortcutId::DuplicateSessionWithCommand => self.open_startup_command_dialog(window, cx),
            ShortcutId::MultiplexSshWithCommand => {
                self.open_startup_command_dialog_for(StartupCommandAction::Multiplex, window, cx)
            }
            ShortcutId::ToggleLeftSidebar => self.toggle_left_sidebar(cx),
            ShortcutId::ToggleRightSidebar => self.toggle_right_inspector(cx),
            ShortcutId::TogglePaneFocus => {
                if self.session.active_id().is_some() {
                    self.shell
                        .set_pane_focus_mode(!self.shell.pane_focus_mode());
                    cx.notify();
                }
            }
            ShortcutId::ToggleNativeFullscreen => {
                window.toggle_fullscreen();
                cx.notify();
            }
            ShortcutId::ZoomIn => self.zoom_terminal_in(cx),
            ShortcutId::ZoomOut => self.zoom_terminal_out(cx),
            ShortcutId::ResetZoom => self.reset_terminal_font_size(cx),
            ShortcutId::OpenSettings => {
                self.open_page(NavItem::Settings, cx);
                self.shell.set_status("settings opened".to_string());
                cx.notify();
            }
            ShortcutId::OpenChat => {
                self.ensure_panel_open(NavItem::AiAssistant);
                window.focus(self.ai.chat_focus(), cx);
                self.shell.set_status("AI panel focused".to_string());
                cx.notify();
            }
            ShortcutId::ShowAllCommands => {
                self.set_bottom_panel_mode(BottomPanelMode::QuickCommands);
                self.shell.set_status("quick commands opened".to_string());
                cx.notify();
            }
            ShortcutId::RenameFile => {
                if self.selected_transfer_entries().len() == 1
                    && self.session.active_file_browser_backend().is_some()
                    && !self.transfer.rename_dialog_is_open()
                {
                    self.open_transfer_rename_dialog(window, cx);
                }
            }
            ShortcutId::CopySelectedConnections => self.copy_selected_connections(cx),
            ShortcutId::LockScreen => self.lock_app(window, cx),
        }
        true
    }

    pub(in crate::features) fn with_shortcut_action_handlers(
        &mut self,
        root: Stateful<Div>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        macro_rules! direct_handler {
            ($action:ty, $id:ident) => {
                cx.listener(|this, _: &$action, window, cx| {
                    this.execute_shortcut_invocation(
                        ShortcutInvocation {
                            id: ShortcutId::$id,
                            tab_index: None,
                        },
                        window,
                        cx,
                    );
                    cx.stop_propagation();
                })
            };
        }

        root.on_action(direct_handler!(TerminalCopy, TerminalCopy))
            .on_action(direct_handler!(TerminalPaste, TerminalPaste))
            .on_action(direct_handler!(
                TerminalPasteSelected,
                TerminalPasteSelected
            ))
            .on_action(direct_handler!(TerminalFind, TerminalFind))
            .on_action(direct_handler!(TerminalClear, TerminalClear))
            .on_action(direct_handler!(TerminalSelectAll, TerminalSelectAll))
            .on_action(direct_handler!(ManageSyncGroups, ManageSyncGroups))
            .on_action(direct_handler!(
                ShowCommandSuggestions,
                ShowCommandSuggestions
            ))
            .on_action(direct_handler!(ToggleRecording, ToggleRecording))
            .on_action(direct_handler!(NewSession, NewSession))
            .on_action(direct_handler!(OpenNewSessionMenu, OpenNewSessionMenu))
            .on_action(direct_handler!(TemporarySshLink, TemporarySshLink))
            .on_action(direct_handler!(QuickSwitch, QuickSwitch))
            .on_action(direct_handler!(NewLocalTerminal, NewLocalTerminal))
            .on_action(direct_handler!(CloseTab, CloseTab))
            .on_action(direct_handler!(NextTab, NextTab))
            .on_action(direct_handler!(PreviousTab, PreviousTab))
            .on_action(cx.listener(|this, action: &SwitchToTab, window, cx| {
                this.execute_shortcut_invocation(
                    ShortcutInvocation {
                        id: ShortcutId::SwitchToTab,
                        tab_index: Some(action.index),
                    },
                    window,
                    cx,
                );
                cx.stop_propagation();
            }))
            .on_action(direct_handler!(DuplicateSession, DuplicateSession))
            .on_action(direct_handler!(MultiplexSsh, MultiplexSsh))
            .on_action(direct_handler!(
                DuplicateSessionWithCommand,
                DuplicateSessionWithCommand
            ))
            .on_action(direct_handler!(
                MultiplexSshWithCommand,
                MultiplexSshWithCommand
            ))
            .on_action(direct_handler!(ToggleLeftSidebar, ToggleLeftSidebar))
            .on_action(direct_handler!(ToggleRightSidebar, ToggleRightSidebar))
            .on_action(direct_handler!(TogglePaneFocus, TogglePaneFocus))
            .on_action(direct_handler!(
                ToggleNativeFullscreen,
                ToggleNativeFullscreen
            ))
            .on_action(direct_handler!(ZoomIn, ZoomIn))
            .on_action(direct_handler!(ZoomOut, ZoomOut))
            .on_action(direct_handler!(ResetZoom, ResetZoom))
            .on_action(direct_handler!(OpenSettings, OpenSettings))
            .on_action(direct_handler!(OpenChat, OpenChat))
            .on_action(direct_handler!(ShowAllCommands, ShowAllCommands))
            .on_action(direct_handler!(RenameFile, RenameFile))
            .on_action(direct_handler!(
                CopySelectedConnections,
                CopySelectedConnections
            ))
            .on_action(direct_handler!(LockScreen, LockScreen))
    }
}

#[cfg(test)]
mod tests {
    use crate::shortcuts::ShortcutId;

    use super::shortcut_interceptor_dispatches;

    #[test]
    fn terminal_clear_is_dispatched_only_by_the_keymap() {
        assert!(!shortcut_interceptor_dispatches(ShortcutId::TerminalClear));
        assert!(shortcut_interceptor_dispatches(ShortcutId::TerminalFind));
    }
}
