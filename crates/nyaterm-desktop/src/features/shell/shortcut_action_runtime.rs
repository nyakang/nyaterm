use gpui::{Context, Div, InteractiveElement, Stateful, Window};

use crate::features::NyaTermApp;
use crate::models::{BottomPanelMode, NavItem, StartupCommandAction};
use crate::shortcuts::{
    CloseTab, CopySelectedConnections, DuplicateSession, DuplicateSessionWithCommand, LockScreen,
    ManageSyncGroups, MultiplexSsh, MultiplexSshWithCommand, NewLocalTerminal, NewSession, NextTab,
    NextTerminalGroup, OpenChat, OpenNewSessionMenu, OpenSettings, PreviousTab,
    PreviousTerminalGroup, QuickSwitch, RenameFile, ResetZoom, ShortcutId, ShortcutInvocation,
    ShowAllCommands, ShowCommandSuggestions, SwitchToTab, TemporarySshLink, TerminalClear,
    TerminalClearAll, TerminalCommandNext, TerminalCommandPrevious, TerminalCommandSelect,
    TerminalCopy, TerminalFind, TerminalPaste, TerminalPasteSelected, TerminalSelectAll,
    ToggleLeftSidebar, ToggleNativeFullscreen, TogglePaneFocus, ToggleRecording,
    ToggleRightSidebar, ZoomIn, ZoomOut,
};

fn shortcut_interceptor_dispatches(id: ShortcutId) -> bool {
    // GPUI still delivers KeyDown after an interceptor stops propagation.
    // This action must run in the keymap instead, or the terminal sends ^L twice.
    !matches!(
        id,
        ShortcutId::TerminalClear
            | ShortcutId::TerminalClearAll
            | ShortcutId::TerminalCommandPrevious
            | ShortcutId::TerminalCommandNext
            | ShortcutId::TerminalCommandSelect
    )
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
                    if this.security.screen_locked() {
                        return false;
                    }
                    // Interceptors run before GPUI's keymap, so this is the only place
                    // that can keep a key being recorded (Ctrl+W, Ctrl+Shift+W, ...)
                    // from also firing the action already bound to it.
                    if this.settings.keybinding_recording_id().is_some() {
                        if !this.settings.keybinding_focus().is_focused(window) {
                            return false;
                        }
                        this.record_keybinding_keystroke(&event.keystroke, cx);
                        return true;
                    }
                    let window_id = window.window_handle().window_id();
                    let editor_window = this
                        .transfer
                        .editor_window()
                        .is_some_and(|editor| editor.window_id() == window_id);
                    let main_window = this
                        .shell
                        .main_window()
                        .is_some_and(|main| main.window_id() == window_id);
                    if !main_window && !editor_window {
                        return false;
                    }
                    let Some(invocation) = crate::shortcuts::resolve_shortcut_invocation(
                        &event.keystroke,
                        &event.context_stack,
                        cx,
                    ) else {
                        return false;
                    };
                    // The child editor has no workspace keymap. Resolve only its
                    // font commands here, without consulting the main window tab.
                    if editor_window {
                        return this.execute_editor_font_shortcut(invocation.id, cx);
                    }
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

    fn execute_editor_font_shortcut(&mut self, id: ShortcutId, cx: &mut Context<Self>) -> bool {
        if self.security.screen_locked() {
            return false;
        }
        match id {
            ShortcutId::ZoomIn => self.adjust_transfer_internal_editor_font_size(1, cx),
            ShortcutId::ZoomOut => self.adjust_transfer_internal_editor_font_size(-1, cx),
            ShortcutId::ResetZoom => self.reset_transfer_internal_editor_font_size(cx),
            _ => return false,
        }
        true
    }

    pub(in crate::features) fn execute_shortcut_invocation(
        &mut self,
        invocation: ShortcutInvocation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        // Native menu accelerators use the same semantic actions as the keymap.
        // Recheck lifecycle and lock state before either dispatch path executes.
        if cfg!(target_os = "macos")
            && self
                .desktop_controller
                .as_ref()
                .and_then(|controller| controller.upgrade())
                .is_some_and(|controller| {
                    !controller
                        .read(cx)
                        .workspace_menu_commands_ready(self.workspace_id, cx)
                })
        {
            return false;
        }
        use crate::features::layout::title_bar::definition::TitleMenuCommand;
        match invocation.id {
            ShortcutId::TerminalCopy => {
                self.execute_title_menu_command(&TitleMenuCommand::TerminalCopy, window, cx)
            }
            ShortcutId::TerminalPaste => {
                self.execute_title_menu_command(&TitleMenuCommand::TerminalPaste, window, cx)
            }
            ShortcutId::TerminalPasteSelected => {
                if let Some(text) = self.selected_terminal_text() {
                    self.paste_terminal_text(text, window, cx);
                }
            }
            ShortcutId::TerminalFind => {
                self.execute_title_menu_command(&TitleMenuCommand::TerminalFind, window, cx)
            }
            ShortcutId::TerminalClear => {
                self.execute_title_menu_command(&TitleMenuCommand::TerminalClear, window, cx)
            }
            ShortcutId::TerminalClearAll => self.clear_terminal(cx),
            ShortcutId::TerminalCommandPrevious => self.navigate_terminal_command(
                nyaterm_terminal::command_navigation::CommandNavigationAction::Previous,
                cx,
            ),
            ShortcutId::TerminalCommandNext => self.navigate_terminal_command(
                nyaterm_terminal::command_navigation::CommandNavigationAction::Next,
                cx,
            ),
            ShortcutId::TerminalCommandSelect => self.navigate_terminal_command(
                nyaterm_terminal::command_navigation::CommandNavigationAction::Select,
                cx,
            ),
            ShortcutId::TerminalSelectAll => {
                self.execute_title_menu_command(&TitleMenuCommand::TerminalSelectAll, window, cx)
            }
            ShortcutId::ManageSyncGroups => {
                self.execute_title_menu_command(&TitleMenuCommand::ManageSyncGroups, window, cx)
            }
            ShortcutId::ShowCommandSuggestions => self.show_manual_command_suggestions(cx),
            ShortcutId::ToggleRecording => self.toggle_active_session_recording(cx),
            ShortcutId::NewSession => {
                self.execute_title_menu_command(&TitleMenuCommand::NewSession, window, cx)
            }
            ShortcutId::OpenNewSessionMenu => {
                if self.session.active_id().is_some() {
                    let anchor = self
                        .current_terminal_group()
                        .as_deref()
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
            ShortcutId::QuickSwitch => {
                self.execute_title_menu_command(&TitleMenuCommand::QuickSwitch, window, cx)
            }
            ShortcutId::NewLocalTerminal => self.start_local_session(window, cx),
            ShortcutId::CloseTab => self.close_active_session(window, cx),
            ShortcutId::NextTab | ShortcutId::PreviousTab => {
                self.select_relative_session(
                    if invocation.id == ShortcutId::NextTab {
                        1
                    } else {
                        -1
                    },
                    cx,
                );
                self.focus_active_workspace_surface(window, cx);
            }
            ShortcutId::NextTerminalGroup => self.select_relative_terminal_group(1, window, cx),
            ShortcutId::PreviousTerminalGroup => {
                self.select_relative_terminal_group(-1, window, cx)
            }
            ShortcutId::SwitchToTab => {
                let index = invocation.tab_index.unwrap_or(1);
                let index = if index == 9 {
                    self.terminal_group_tab_count().saturating_sub(1)
                } else {
                    index.saturating_sub(1)
                };
                self.select_session_index(index, cx);
                self.focus_active_workspace_surface(window, cx);
            }
            ShortcutId::DuplicateSession => self.duplicate_active_session(window, cx),
            ShortcutId::MultiplexSsh => self.multiplex_active_ssh_session(window, cx),
            ShortcutId::DuplicateSessionWithCommand => self.open_startup_command_dialog(window, cx),
            ShortcutId::MultiplexSshWithCommand => {
                self.open_startup_command_dialog_for(StartupCommandAction::Multiplex, window, cx)
            }
            ShortcutId::ToggleLeftSidebar => {
                self.execute_title_menu_command(&TitleMenuCommand::ToggleLeftSidebar, window, cx)
            }
            ShortcutId::ToggleRightSidebar => {
                self.execute_title_menu_command(&TitleMenuCommand::ToggleRightSidebar, window, cx)
            }
            ShortcutId::TogglePaneFocus => {
                if self.session.active_id().is_some()
                    && !self.active_workspace_tab().is_some_and(|key| {
                        !crate::models::WorkspaceTabId::from_layout_key(&key).is_session()
                    })
                {
                    self.shell
                        .set_pane_focus_mode(!self.shell.pane_focus_mode());
                    self.sync_terminal_frame_snapshot_priority();
                    cx.notify();
                }
            }
            ShortcutId::ToggleNativeFullscreen => {
                window.toggle_fullscreen();
                cx.notify();
            }
            ShortcutId::ZoomIn => {
                self.execute_title_menu_command(&TitleMenuCommand::ZoomIn, window, cx)
            }
            ShortcutId::ZoomOut => {
                self.execute_title_menu_command(&TitleMenuCommand::ZoomOut, window, cx)
            }
            ShortcutId::ResetZoom => {
                self.execute_title_menu_command(&TitleMenuCommand::ResetZoom, window, cx)
            }
            ShortcutId::OpenSettings => {
                self.execute_title_menu_command(&TitleMenuCommand::OpenSettings, window, cx)
            }
            ShortcutId::OpenChat => {
                self.ensure_panel_open(NavItem::AiAssistant, cx);
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
            .on_action(direct_handler!(TerminalClearAll, TerminalClearAll))
            .on_action(direct_handler!(
                TerminalCommandPrevious,
                TerminalCommandPrevious
            ))
            .on_action(direct_handler!(TerminalCommandNext, TerminalCommandNext))
            .on_action(direct_handler!(
                TerminalCommandSelect,
                TerminalCommandSelect
            ))
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
            .on_action(direct_handler!(NextTerminalGroup, NextTerminalGroup))
            .on_action(direct_handler!(
                PreviousTerminalGroup,
                PreviousTerminalGroup
            ))
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
