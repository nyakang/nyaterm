use rust_i18n::t;

use std::borrow::Cow;

use gpui::{App, Context};
use nyaterm_core::packaging;
use nyaterm_ui::{NyaAppMenuBar, NyaDialogWindowExt as _, NyaMenuItem};

use super::definition::{TitleMenuCommand, TitleMenuDefinition, TitleMenuKind};
use crate::features::NyaTermApp;
use crate::models::{
    ActivityBarEntry, ActivityBarZone, HeaderStatusMode, NavItem, PanelOpenMode, PanelSide,
    SmartSplitMode, TitleMenu,
};

impl NyaTermApp {
    pub(crate) fn title_menu_command_enabled(&self, command: &TitleMenuCommand, cx: &App) -> bool {
        if self.security.screen_locked() && *command != TitleMenuCommand::About {
            return false;
        }
        fn find(items: &[TitleMenuDefinition], command: &TitleMenuCommand) -> Option<bool> {
            items.iter().find_map(|item| match &item.kind {
                TitleMenuKind::Action(Some(candidate)) if candidate == command => {
                    Some(!item.disabled)
                }
                TitleMenuKind::Submenu(children) => {
                    find(children, command).map(|enabled| enabled && !item.disabled)
                }
                _ => None,
            })
        }
        [
            TitleMenu::File,
            TitleMenu::View,
            TitleMenu::Terminal,
            TitleMenu::Help,
        ]
        .into_iter()
        .find_map(|menu| find(&self.title_menu_definitions(menu, cx), command))
        .unwrap_or(matches!(
            command,
            TitleMenuCommand::OpenSettings
                | TitleMenuCommand::ToggleLeftSidebar
                | TitleMenuCommand::ToggleRightSidebar
                | TitleMenuCommand::TerminalCopy
                | TitleMenuCommand::TerminalPaste
                | TitleMenuCommand::TerminalFind
                | TitleMenuCommand::TerminalSelectAll
        ))
    }

    pub(crate) fn execute_title_menu_command(
        &mut self,
        command: &TitleMenuCommand,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        if !self.title_menu_command_enabled(command, cx) {
            return;
        }
        if let Some(tab) = self
            .active_workspace_tab()
            .as_deref()
            .map(crate::models::WorkspaceTabId::from_layout_key)
            && !tab.is_session()
        {
            if let crate::models::WorkspaceTabId::FileManager(_) = tab {
                match command {
                    TitleMenuCommand::TerminalCopy
                    | TitleMenuCommand::TerminalPaste
                    | TitleMenuCommand::TerminalFind
                    | TitleMenuCommand::TerminalSelectAll
                    | TitleMenuCommand::TerminalClear
                    | TitleMenuCommand::ToggleBroadcast => return,
                    _ => {}
                }
            }
            if let crate::models::WorkspaceTabId::Editor(id) = tab {
                match command {
                    TitleMenuCommand::TerminalCopy
                    | TitleMenuCommand::TerminalPaste
                    | TitleMenuCommand::TerminalFind
                    | TitleMenuCommand::TerminalSelectAll => {
                        if let Some(view) = self.transfer.workspace_editor_view(&id) {
                            view.update(cx, |view, cx| {
                                view.execute_edit_command(command, window, cx)
                            });
                        }
                        return;
                    }
                    TitleMenuCommand::TerminalClear | TitleMenuCommand::ToggleBroadcast => return,
                    TitleMenuCommand::ZoomIn => {
                        self.adjust_transfer_internal_editor_font_size(1, cx);
                        return;
                    }
                    TitleMenuCommand::ZoomOut => {
                        self.adjust_transfer_internal_editor_font_size(-1, cx);
                        return;
                    }
                    TitleMenuCommand::ResetZoom => {
                        self.reset_transfer_internal_editor_font_size(cx);
                        return;
                    }
                    _ => {}
                }
            }
        }
        match command {
            TitleMenuCommand::OpenFileManager => {
                self.open_file_manager_tab(None, cx);
            }
            TitleMenuCommand::NewWindow => {
                cx.emit(crate::features::AppLifecycleEvent::NewWindowRequested)
            }
            TitleMenuCommand::NewSession => {
                self.prepare_title_menu(cx);
                self.open_connection_editor(None, None, false, window, cx);
            }
            TitleMenuCommand::ImportConfig => {
                window.close_nya_dialog(cx);
                self.open_connection_import_dialog(window, cx);
            }
            TitleMenuCommand::ExportConfig => {
                window.close_nya_dialog(cx);
                self.prompt_encrypted_portable_snapshot_export(window, cx);
            }
            TitleMenuCommand::ZoomIn => self.zoom_terminal_in(cx),
            TitleMenuCommand::ZoomOut => self.zoom_terminal_out(cx),
            TitleMenuCommand::ResetZoom => self.reset_terminal_font_size(cx),
            TitleMenuCommand::QuickSwitch => self.open_quick_switch(window, cx),
            TitleMenuCommand::ToggleActionLinks => self.toggle_terminal_action_links(cx),
            TitleMenuCommand::ToggleTerminalZoom => self.toggle_terminal_zoom_enabled(cx),
            TitleMenuCommand::ToggleBroadcast => self.toggle_broadcast_to_all(cx),
            TitleMenuCommand::TerminalClear => self.send_terminal_clear_screen(cx),
            TitleMenuCommand::RefitTerminals => {
                self.resize_all_known_terminal_surfaces_for_menu(cx)
            }
            TitleMenuCommand::Unsplit => self.unsplit_workspace(cx),
            TitleMenuCommand::Documentation => self.open_documentation(cx),
            TitleMenuCommand::CheckUpdates => {
                self.acknowledge_help_update(cx);
                self.open_update_dialog(window, cx);
            }
            TitleMenuCommand::ViewLogs => self.reveal_log_dir(cx),
            TitleMenuCommand::About => self.open_about(window, cx),
            TitleMenuCommand::SetTheme(theme) => self.update_appearance_theme(theme, cx),
            TitleMenuCommand::SetTerminalTheme(theme) => {
                self.set_terminal_theme(theme.as_deref(), cx)
            }
            TitleMenuCommand::SetLanguage(locale) => self.update_ui_language(locale, cx),
            TitleMenuCommand::HideHeaderStatus => self.open_header_status_hide_confirm(window, cx),
            TitleMenuCommand::SetHeaderStatus(mode) => self.set_header_status_mode(*mode, cx),
            TitleMenuCommand::TogglePanelFloating => {
                let mode = if self.shell.panel_open_mode().is_floating() {
                    PanelOpenMode::Docked
                } else {
                    PanelOpenMode::Floating
                };
                self.set_panel_open_mode(mode, cx);
            }
            TitleMenuCommand::TogglePanelMultiOpen => self.toggle_panel_multi_open(cx),
            TitleMenuCommand::ResetPanelLayout => {
                self.confirm_reset_activity_bar_layout(window, cx)
            }
            TitleMenuCommand::TogglePanelVisibility(id) => {
                if self.shell.activity_bar_layout().is_hidden(id) {
                    self.show_activity_entry(id.clone(), cx);
                } else {
                    self.hide_activity_entry(id.clone(), cx);
                }
            }
            TitleMenuCommand::TogglePadding => self.toggle_terminal_workspace_padding(cx),
            TitleMenuCommand::ToggleLineNumbers => self.toggle_terminal_line_numbers(cx),
            TitleMenuCommand::ToggleTimestamps => self.toggle_terminal_timestamps(cx),
            TitleMenuCommand::SmartSplit(mode) => self.apply_smart_split(*mode, cx),
            TitleMenuCommand::ManageSyncGroups => self.open_sync_groups(window, cx),
            TitleMenuCommand::OpenSettings => {
                self.open_page(NavItem::Settings, cx);
                self.shell.set_status("settings opened".to_string());
                cx.notify();
            }
            TitleMenuCommand::ToggleLeftSidebar => self.toggle_left_sidebar(cx),
            TitleMenuCommand::ToggleRightSidebar => self.toggle_right_inspector(cx),
            TitleMenuCommand::TerminalCopy => self.copy_terminal_selection_or_visible(cx),
            TitleMenuCommand::TerminalPaste => self.paste_from_clipboard(window, cx),
            TitleMenuCommand::TerminalFind => self.open_terminal_search(window, cx),
            TitleMenuCommand::TerminalSelectAll => self.select_all_terminal(cx),
        }
    }

    pub(crate) fn set_desktop_controller(
        &mut self,
        controller: gpui::WeakEntity<crate::app_shell::DesktopController>,
    ) {
        self.desktop_controller = Some(controller);
    }
    pub(crate) fn set_workspace_identity(
        &mut self,
        workspace_id: nyaterm_core::WorkspaceId,
        revision: u64,
    ) {
        self.workspace_id = workspace_id;
        self.workspace_revision = revision;
    }

    pub(crate) fn set_title_menu_bar(&mut self, menu_bar: gpui::Entity<NyaAppMenuBar>) {
        self.shell.set_title_menu_bar(menu_bar);
    }

    pub(crate) fn title_menu_label(&self, menu: TitleMenu) -> Cow<'static, str> {
        t!(menu.i18n_key())
    }

    pub(crate) fn build_title_menu_items(
        &mut self,
        menu: TitleMenu,
        cx: &mut Context<Self>,
    ) -> Vec<NyaMenuItem> {
        self.title_menu_items(menu, cx)
    }

    pub(crate) fn prepare_title_menu(&mut self, cx: &mut Context<Self>) {
        self.shell.close_open_tabs_menu();
        self.shell.close_new_session_menu();
        cx.notify();
    }

    pub(crate) fn acknowledge_help_update(&mut self, cx: &mut Context<Self>) {
        self.update.update(cx, |update, cx| {
            if update.acknowledge_update() {
                cx.notify();
            }
        });
    }

    pub(crate) fn help_update_attention(&self, cx: &App) -> Option<u32> {
        self.update
            .read(cx)
            .update_attention()
            .then(|| self.theme_palette().success)
    }

    pub(crate) fn resize_all_known_terminal_surfaces_for_menu(&mut self, cx: &mut Context<Self>) {
        let changed = self.resize_all_known_terminal_surfaces();
        self.shell.set_status(if changed {
            "terminal sizes reset".to_string()
        } else {
            "terminal sizes already current".to_string()
        });
        cx.notify();
    }

    pub(in crate::features) fn title_menu_items(
        &self,
        menu: TitleMenu,
        cx: &mut Context<Self>,
    ) -> Vec<NyaMenuItem> {
        self.title_menu_definitions(menu, cx)
            .into_iter()
            .map(|item| item.into_window_item(cx))
            .collect()
    }

    pub(crate) fn title_menu_definitions(
        &self,
        menu: TitleMenu,
        cx: &App,
    ) -> Vec<TitleMenuDefinition> {
        match menu {
            TitleMenu::File => self.title_file_menu_items(cx),
            TitleMenu::View => self.title_view_menu_items(cx),
            TitleMenu::Terminal => self.title_terminal_menu_items(cx),
            TitleMenu::Help => self.title_help_menu_items(cx),
        }
    }

    fn title_file_menu_items(&self, _cx: &App) -> Vec<TitleMenuDefinition> {
        vec![
            TitleMenuDefinition::action(t!("menu.newWindow"))
                .icon("icons/window/plus.svg")
                .shortcut("Ctrl+Shift+N")
                .command(TitleMenuCommand::NewWindow),
            TitleMenuDefinition::action(t!("menu.newSession"))
                .icon("icons/conn/add.svg")
                .shortcut(self.display_shortcut_for("tab.newSession", "Ctrl+Shift+T"))
                .command(TitleMenuCommand::NewSession),
            TitleMenuDefinition::action(t!("fileManager.tabTitle"))
                .icon("icons/file-manager.svg")
                .command(TitleMenuCommand::OpenFileManager),
            TitleMenuDefinition::separator(),
            TitleMenuDefinition::action(t!("settings.importConfig"))
                .icon("icons/import.svg")
                .command(TitleMenuCommand::ImportConfig),
            TitleMenuDefinition::action(t!("settings.exportConfig"))
                .icon("icons/menu/export.svg")
                .command(TitleMenuCommand::ExportConfig),
        ]
    }

    fn title_view_menu_items(&self, cx: &App) -> Vec<TitleMenuDefinition> {
        let current_theme = self
            .settings
            .summary()
            .terminal_theme
            .as_deref()
            .unwrap_or("");
        let current_header_status =
            HeaderStatusMode::from_setting(&self.settings.summary().ui_header_status_mode);
        let header_status_visible = self.settings.summary().ui_header_status_visible;
        vec![
            TitleMenuDefinition::submenu(t!("menu.theme"), self.title_theme_menu_items(cx))
                .icon("icons/menu/palette.svg"),
            TitleMenuDefinition::submenu(
                t!("menu.terminalTheme"),
                self.title_terminal_theme_menu_items(current_theme, cx),
            )
            .icon("icons/conn/terminal.svg"),
            TitleMenuDefinition::submenu(t!("menu.language"), self.title_language_menu_items(cx))
                .icon("icons/translation.svg"),
            TitleMenuDefinition::submenu(
                t!("menu.headerStatus"),
                self.title_header_status_menu_items(
                    current_header_status,
                    header_status_visible,
                    cx,
                ),
            )
            .icon("icons/menu/info.svg"),
            TitleMenuDefinition::submenu(t!("menu.panels"), self.title_panels_menu_items(cx))
                .icon("icons/menu/sidebar.svg"),
            TitleMenuDefinition::separator(),
            TitleMenuDefinition::action(t!("menu.zoomIn"))
                .icon("icons/menu/zoom-in.svg")
                .shortcut(self.display_shortcut_for("view.zoomIn", "Ctrl+="))
                .command(TitleMenuCommand::ZoomIn),
            TitleMenuDefinition::action(t!("menu.zoomOut"))
                .icon("icons/menu/zoom-out.svg")
                .shortcut(self.display_shortcut_for("view.zoomOut", "Ctrl+-"))
                .command(TitleMenuCommand::ZoomOut),
            TitleMenuDefinition::action(t!("menu.resetZoom"))
                .icon("icons/menu/reset.svg")
                .shortcut(self.display_shortcut_for("view.resetZoom", "Ctrl+0"))
                .command(TitleMenuCommand::ResetZoom),
        ]
    }

    fn title_terminal_menu_items(&self, cx: &App) -> Vec<TitleMenuDefinition> {
        vec![
            TitleMenuDefinition::action(t!("menu.commandPalette"))
                .icon("icons/fe/search.svg")
                .shortcut(self.display_shortcut_for("tab.quickSwitch", "Ctrl+Shift+S"))
                .command(TitleMenuCommand::QuickSwitch),
            TitleMenuDefinition::separator(),
            TitleMenuDefinition::submenu(
                t!("menu.terminalDisplay"),
                self.title_terminal_display_menu_items(cx),
            )
            .icon("icons/eye.svg"),
            TitleMenuDefinition::action(t!("settings.actionLinks"))
                .icon("icons/conn/flash.svg")
                .checked(self.settings.summary().terminal_action_links_enabled)
                .command(TitleMenuCommand::ToggleActionLinks),
            TitleMenuDefinition::action(t!("settings.terminalZoomEnabled"))
                .icon("icons/menu/zoom-in.svg")
                .checked(self.settings.summary().interaction_terminal_zoom_enabled)
                .command(TitleMenuCommand::ToggleTerminalZoom),
            TitleMenuDefinition::submenu(
                t!("menu.smartSplit"),
                self.title_smart_split_menu_items(cx),
            )
            .icon("icons/menu/split.svg"),
            TitleMenuDefinition::separator(),
            TitleMenuDefinition::submenu(
                t!("menu.syncInput"),
                self.title_sync_input_menu_items(cx),
            )
            .icon("icons/sync.svg"),
            TitleMenuDefinition::separator(),
            TitleMenuDefinition::action(t!("menu.broadcastToAll"))
                .icon("icons/menu/broadcast.svg")
                .checked(self.sync_input.broadcast_to_all())
                .command(TitleMenuCommand::ToggleBroadcast),
            TitleMenuDefinition::separator(),
            TitleMenuDefinition::action(t!("menu.clearTerminal"))
                .icon("icons/fe/delete.svg")
                .shortcut(self.display_shortcut_for("terminal.clear", "Ctrl+L"))
                .command(TitleMenuCommand::TerminalClear),
            TitleMenuDefinition::action(t!("menu.refitTerminals"))
                .icon("icons/menu/fit.svg")
                .command(TitleMenuCommand::RefitTerminals),
            TitleMenuDefinition::action(t!("menu.unsplit"))
                .icon("icons/menu/fit.svg")
                .disabled(
                    !self.terminal_windows_is_multi_leaf()
                        && self.shell.workspace_split().is_none(),
                )
                .command(TitleMenuCommand::Unsplit),
        ]
    }

    fn title_help_menu_items(&self, cx: &App) -> Vec<TitleMenuDefinition> {
        self.title_help_menu_items_for(packaging::updates_managed_externally(), cx)
    }

    /// Builds the Help menu for an explicit update policy so tests can assert both
    /// the ordinary and the externally managed menu.
    fn title_help_menu_items_for(
        &self,
        updates_managed_externally: bool,
        cx: &App,
    ) -> Vec<TitleMenuDefinition> {
        let mut items = vec![
            TitleMenuDefinition::action(t!("menu.documentation"))
                .icon("icons/menu/book.svg")
                .command(TitleMenuCommand::Documentation),
        ];
        // A build whose updates come from an external package manager must not
        // offer an in-app check it cannot apply.
        if !updates_managed_externally {
            let update = self.update.read(cx);
            let has_update = update.has_available_update();
            let success = self.theme_palette().success;
            let mut update_item =
                TitleMenuDefinition::action(t!("menu.checkForUpdates")).icon(if has_update {
                    "icons/menu/upgrade.svg"
                } else {
                    "icons/menu/update.svg"
                });
            if has_update {
                update_item = update_item
                    .icon_color(success)
                    .status(t!("updater.hasNewVersion"), success);
            }
            items.push(update_item.command(TitleMenuCommand::CheckUpdates));
        }
        items.extend([
            TitleMenuDefinition::action(t!("menu.viewLogs"))
                .icon("icons/menu/article.svg")
                .command(TitleMenuCommand::ViewLogs),
            TitleMenuDefinition::separator(),
            TitleMenuDefinition::action(format!("{} NyaTerm", t!("menu.about")))
                .icon("icons/menu/info.svg")
                .command(TitleMenuCommand::About),
        ]);
        items
    }

    fn title_theme_menu_items(&self, _cx: &App) -> Vec<TitleMenuDefinition> {
        let current = self.settings.summary().theme.as_str();
        self.settings
            .theme_options()
            .iter()
            .cloned()
            .map(|(theme, name)| {
                let selected =
                    current == theme || (current == "catppuccin" && theme == "catppuccin-mocha");
                TitleMenuDefinition::action(name)
                    .checked(selected)
                    .command(TitleMenuCommand::SetTheme(theme.to_string()))
            })
            .collect()
    }

    fn title_terminal_theme_menu_items(
        &self,
        current_theme: &str,
        _cx: &App,
    ) -> Vec<TitleMenuDefinition> {
        let mut items = vec![
            TitleMenuDefinition::action(t!("settings.followUiTheme"))
                .checked(current_theme.trim().is_empty())
                .command(TitleMenuCommand::SetTerminalTheme(None)),
        ];
        items.extend(
            self.settings
                .theme_options()
                .iter()
                .cloned()
                .map(|(theme, name)| {
                    TitleMenuDefinition::action(name)
                        .checked(
                            current_theme == theme
                                || (current_theme == "catppuccin" && theme == "catppuccin-mocha"),
                        )
                        .command(TitleMenuCommand::SetTerminalTheme(Some(theme.to_string())))
                }),
        );
        items
    }

    fn title_language_menu_items(&self, _cx: &App) -> Vec<TitleMenuDefinition> {
        let active = crate::i18n::normalize_locale(&self.settings.summary().language);
        crate::i18n::available_locales()
            .into_iter()
            .map(|locale| {
                let checked = locale == active;
                let label = crate::i18n::locale_display_name(&locale);
                TitleMenuDefinition::action(label)
                    .checked(checked)
                    .command(TitleMenuCommand::SetLanguage(locale.to_string()))
            })
            .collect()
    }

    fn title_header_status_menu_items(
        &self,
        current: HeaderStatusMode,
        visible: bool,
        _cx: &App,
    ) -> Vec<TitleMenuDefinition> {
        let mut items = vec![
            TitleMenuDefinition::action(t!("headerStatus.hidden"))
                .checked(!visible)
                .command(TitleMenuCommand::HideHeaderStatus),
        ];
        items.extend(HeaderStatusMode::ALL.into_iter().map(|mode| {
            TitleMenuDefinition::action(t!(mode.i18n_key()))
                .checked(visible && current == mode)
                .command(TitleMenuCommand::SetHeaderStatus(mode))
        }));
        items
    }

    fn title_panels_menu_items(&self, cx: &App) -> Vec<TitleMenuDefinition> {
        let floating = self.shell.panel_open_mode().is_floating();
        let multi_open = self.shell.panel_multi_open();
        vec![
            TitleMenuDefinition::action(t!("panel.floatingMode"))
                .checked(floating)
                .command(TitleMenuCommand::TogglePanelFloating),
            TitleMenuDefinition::action(t!("panel.multiOpen"))
                .checked(multi_open)
                .command(TitleMenuCommand::TogglePanelMultiOpen),
            TitleMenuDefinition::separator(),
            TitleMenuDefinition::submenu(
                t!("activityBar.leftSide"),
                self.title_side_panel_menu_items(PanelSide::Left, cx),
            ),
            TitleMenuDefinition::submenu(
                t!("activityBar.rightSide"),
                self.title_side_panel_menu_items(PanelSide::Right, cx),
            ),
            TitleMenuDefinition::separator(),
            TitleMenuDefinition::action(t!("activityBar.resetLayout"))
                .danger()
                .command(TitleMenuCommand::ResetPanelLayout),
        ]
    }

    fn title_side_panel_menu_items(&self, side: PanelSide, _cx: &App) -> Vec<TitleMenuDefinition> {
        let zones = match side {
            PanelSide::Left => [ActivityBarZone::LeftTop, ActivityBarZone::LeftBottom],
            PanelSide::Right => [ActivityBarZone::RightTop, ActivityBarZone::RightBottom],
        };
        let entries = zones
            .into_iter()
            .flat_map(|zone| self.shell.activity_bar_layout().zone(zone))
            .filter_map(|id| ActivityBarEntry::from_persistence_id(id))
            .filter(|entry| match entry {
                ActivityBarEntry::Panel(item) => !item.opens_settings(),
                ActivityBarEntry::Recording => true,
                ActivityBarEntry::QuickCommands
                | ActivityBarEntry::CommandSend
                | ActivityBarEntry::Lock => false,
            })
            .map(|entry| {
                let id = entry.persistence_id().to_string();
                let label = entry
                    .i18n_key()
                    .map(|key| t!(key).to_string())
                    .unwrap_or_else(|| entry.label().to_string());
                let visible = !self.shell.activity_bar_layout().is_hidden(&id);
                (id, label, visible)
            })
            .collect::<Vec<_>>();
        if entries.is_empty() {
            return vec![TitleMenuDefinition::action(t!("activityBar.noPanels")).disabled(true)];
        }
        entries
            .into_iter()
            .map(|(id, label, visible)| {
                TitleMenuDefinition::action(label)
                    .checked(visible)
                    .command(TitleMenuCommand::TogglePanelVisibility(id.clone()))
            })
            .collect()
    }

    fn title_terminal_display_menu_items(&self, _cx: &App) -> Vec<TitleMenuDefinition> {
        vec![
            TitleMenuDefinition::action(t!("settings.showWorkspacePadding"))
                .checked(self.settings.summary().terminal_show_workspace_padding)
                .command(TitleMenuCommand::TogglePadding),
            TitleMenuDefinition::action(t!("settings.showLineNumbers"))
                .checked(self.settings.summary().terminal_show_line_numbers)
                .command(TitleMenuCommand::ToggleLineNumbers),
            TitleMenuDefinition::action(t!("settings.showTimestamps"))
                .checked(self.settings.summary().terminal_show_timestamps)
                .command(TitleMenuCommand::ToggleTimestamps),
        ]
    }

    fn title_smart_split_menu_items(&self, _cx: &App) -> Vec<TitleMenuDefinition> {
        vec![
            TitleMenuDefinition::action(t!("menu.autoTile"))
                .icon("icons/view-grid.svg")
                .disabled(self.session.session_order_len() < 2)
                .command(TitleMenuCommand::SmartSplit(SmartSplitMode::Auto)),
            TitleMenuDefinition::action(t!("menu.tileHorizontally"))
                .icon("icons/menu/horizontal.svg")
                .disabled(self.session.session_order_len() < 2)
                .command(TitleMenuCommand::SmartSplit(SmartSplitMode::Horizontal)),
            TitleMenuDefinition::action(t!("menu.tileVertically"))
                .icon("icons/menu/vertical.svg")
                .disabled(self.session.session_order_len() < 2)
                .command(TitleMenuCommand::SmartSplit(SmartSplitMode::Vertical)),
        ]
    }

    fn title_sync_input_menu_items(&self, _cx: &App) -> Vec<TitleMenuDefinition> {
        vec![
            TitleMenuDefinition::action(t!("menu.manageGroups"))
                .icon("icons/settings.svg")
                .shortcut(self.display_shortcut_for("terminal.manageSyncGroups", "Ctrl+Shift+G"))
                .command(TitleMenuCommand::ManageSyncGroups),
        ]
    }

    #[cfg(test)]
    pub(crate) fn title_menu_items_for_test(
        &self,
        menu: crate::models::TitleMenu,
        cx: &mut Context<Self>,
    ) -> Vec<NyaMenuItem> {
        self.title_menu_items(menu, cx)
    }

    #[cfg(test)]
    pub(crate) fn help_menu_has_update_command_for_test(
        &self,
        updates_managed_externally: bool,
        cx: &App,
    ) -> bool {
        self.title_help_menu_items_for(updates_managed_externally, cx)
            .iter()
            .any(|item| {
                matches!(
                    item.kind,
                    TitleMenuKind::Action(Some(TitleMenuCommand::CheckUpdates))
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use gpui::{AppContext as _, TestAppContext};
    use nyaterm_core::{AppRuntime, NativeUpdateInfo, RuntimeMode, uuid};
    use nyaterm_ui::NyaMenuItem;

    use crate::entities::{OverlayStore, StartupRestoreStore, UiStoreHandles};
    use crate::features::NyaTermApp;
    use crate::features::layout::title_bar::definition::TitleMenuCommand;
    use crate::features::update::{UpdateCheckKind, UpdateEvent};
    use crate::models::TitleMenu;

    struct MenuCommandWindow;
    impl gpui::Render for MenuCommandWindow {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            gpui::div()
        }
    }

    struct ShortcutCommandWindow {
        app: gpui::Entity<NyaTermApp>,
        focus: gpui::FocusHandle,
    }

    impl gpui::Render for ShortcutCommandWindow {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            use gpui::{InteractiveElement as _, Styled as _};
            let root = gpui::div()
                .id("menu-shortcut-fixture")
                .size_full()
                .key_context(crate::shortcuts::WORKSPACE_KEY_CONTEXT)
                .track_focus(&self.focus);
            self.app
                .update(cx, |app, cx| app.with_shortcut_action_handlers(root, cx))
        }
    }

    #[test]
    fn custom_menu_shortcut_and_native_semantic_action_each_execute_once() {
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = menu_app(&mut cx);
        let before = cx.read_entity(&app, |app, _| app.settings.summary().terminal_font_size);
        cx.update_entity(&app, |app, cx| {
            // Zoom saves appearance settings and reapplies the stored keymap on
            // completion, so the override must also exist in persistent settings.
            let saved = app
                .store_blocking_client()
                .request_fn(nyaterm_store::StoreDomain::Settings, |store| {
                    store.save_keybindings(&std::collections::HashMap::from([(
                        "view.zoomIn".into(),
                        "Ctrl+Alt+U".into(),
                    )]))
                })
                .expect("save custom zoom shortcut");
            app.apply_gpui_settings(saved, cx);
        });
        let window = cx.add_window(|window, cx| {
            let focus = cx.focus_handle();
            window.focus(&focus, cx);
            ShortcutCommandWindow {
                app: app.clone(),
                focus,
            }
        });
        cx.simulate_keystrokes(window.into(), "ctrl-alt-u");
        assert_eq!(
            cx.read_entity(&app, |app, _| app.settings.summary().terminal_font_size),
            before + 1
        );
        cx.dispatch_action(window.into(), crate::shortcuts::ZoomIn);
        assert_eq!(
            cx.read_entity(&app, |app, _| app.settings.summary().terminal_font_size),
            before + 2
        );
        let action = TitleMenuCommand::ZoomIn.native_action();
        cx.read(|cx| {
            assert_eq!(
                cx.key_bindings()
                    .borrow()
                    .bindings_for_action(action.as_ref())
                    .count(),
                1
            )
        });
    }

    #[test]
    fn disabled_terminal_zoom_blocks_menu_commands_and_shortcut_actions() {
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = menu_app(&mut cx);
        cx.update_entity(&app, |app, _| {
            let mut settings = app.settings.summary().clone();
            settings.terminal_font_size = 20;
            settings.interaction_terminal_zoom_enabled = false;
            app.settings.replace_summary(settings);
        });
        let window = cx.add_window(|window, cx| {
            let focus = cx.focus_handle();
            window.focus(&focus, cx);
            ShortcutCommandWindow {
                app: app.clone(),
                focus,
            }
        });
        for command in [
            TitleMenuCommand::ZoomIn,
            TitleMenuCommand::ZoomOut,
            TitleMenuCommand::ResetZoom,
        ] {
            window
                .update(&mut cx, |_, window, cx| {
                    app.update(cx, |app, cx| {
                        app.execute_title_menu_command(&command, window, cx)
                    });
                })
                .unwrap();
        }
        cx.dispatch_action(window.into(), crate::shortcuts::ZoomIn);
        cx.dispatch_action(window.into(), crate::shortcuts::ZoomOut);
        cx.dispatch_action(window.into(), crate::shortcuts::ResetZoom);
        assert_eq!(
            cx.read_entity(&app, |app, _| app.settings.summary().terminal_font_size),
            20
        );
    }

    #[test]
    fn commands_recheck_lock_state_instead_of_executing_a_stale_enabled_menu() {
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = menu_app(&mut cx);
        assert!(cx.read_entity(&app, |app, cx| {
            app.title_menu_command_enabled(&TitleMenuCommand::ToggleLineNumbers, cx)
        }));
        cx.update_entity(&app, |app, _| {
            app.security.activate_screen_lock(String::new())
        });
        let window = cx.add_window(|_, _| MenuCommandWindow);
        window
            .update(&mut cx, |_, window, cx| {
                app.update(cx, |app, cx| {
                    app.execute_title_menu_command(
                        &TitleMenuCommand::ToggleLineNumbers,
                        window,
                        cx,
                    );
                    assert!(!app.settings.summary().terminal_show_line_numbers);
                    assert!(!app.title_menu_command_enabled(&TitleMenuCommand::NewWindow, cx));
                    assert!(app.title_menu_command_enabled(&TitleMenuCommand::About, cx));
                });
            })
            .unwrap();
    }

    fn unique_test_dir() -> PathBuf {
        // A uuid rather than a clock reading: these tests run in parallel and
        // Windows' ~15ms clock granularity lets a nanosecond timestamp repeat,
        // which would share one config dir and so one settings database.
        std::env::temp_dir().join(format!(
            "nyaterm-title-menu-{}-{}",
            std::process::id(),
            uuid()
        ))
    }

    fn menu_app(cx: &mut TestAppContext) -> gpui::Entity<NyaTermApp> {
        let root = unique_test_dir();
        let runtime = AppRuntime::from_parts_for_test(
            RuntimeMode::Portable,
            root.clone(),
            root.join("config"),
            root.join("logs"),
            root.join("cache"),
            None,
        );
        let stores = UiStoreHandles {
            startup_restore: cx.new(|_| StartupRestoreStore::default()),
            overlays: cx.new(|_| OverlayStore::default()),
        };
        cx.new(|cx| NyaTermApp::new(runtime, stores, cx))
    }

    #[test]
    fn custom_title_bar_keeps_tauri_visible_top_level_menus_without_edit() {
        let labels = [
            TitleMenu::File,
            TitleMenu::View,
            TitleMenu::Terminal,
            TitleMenu::Help,
        ]
        .map(TitleMenu::label);

        assert_eq!(labels, ["File", "View", "Terminal", "Help"]);
        assert!(!labels.contains(&"Edit"));
    }

    #[test]
    fn help_menu_keeps_check_command_and_shows_available_status() {
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = menu_app(&mut cx);
        let update = cx.read_entity(&app, |app, _| app.update.clone());
        cx.update_entity(&update, |update, _| {
            let (_, generation) = update.begin_check(UpdateCheckKind::Silent).unwrap();
            update.apply_event(UpdateEvent::Check {
                generation,
                kind: UpdateCheckKind::Silent,
                result: Ok(NativeUpdateInfo {
                    current_version: "2.0.0-preview.2".into(),
                    latest_version: "2.0.0-preview.3".into(),
                    release_date: None,
                    release_notes: None,
                    html_url: None,
                    available: true,
                }),
            });
        });
        let check_labels = ["en", "zh-CN", "zh-TW", "ja", "ko", "fr"]
            .map(|locale| rust_i18n::t!("menu.checkForUpdates", locale = locale).to_string());
        let (item, color) = cx.update_entity(&app, |app, cx| {
            let item = app
                .title_menu_items_for_test(TitleMenu::Help, cx)
                .into_iter()
                .find(|item| check_labels.iter().any(|label| label == item.test_label()));
            (item, app.theme_palette().success)
        });
        // An externally managed build has no in-app check, so the entry is
        // legitimately absent and only its presence is worth asserting.
        let item = match item {
            Some(item) => {
                assert!(
                    !nyaterm_core::packaging::updates_managed_externally(),
                    "externally managed builds must not offer an in-app update entry"
                );
                item
            }
            None => {
                assert!(
                    nyaterm_core::packaging::updates_managed_externally(),
                    "the in-app update entry is missing from an application-managed build"
                );
                return;
            }
        };
        let statuses = ["en", "zh-CN", "zh-TW", "ja", "ko", "fr"]
            .map(|locale| rust_i18n::t!("updater.hasNewVersion", locale = locale).to_string());
        assert!(check_labels.iter().any(|label| label == item.test_label()));
        assert_eq!(
            item.test_presentation().1.as_deref(),
            Some("icons/menu/upgrade.svg")
        );
        assert_eq!(item.test_icon_color(), Some(color));
        assert!(
            item.test_status()
                .is_some_and(|(label, status_color)| status_color == color
                    && statuses.iter().any(|status| status == label))
        );
    }

    #[test]
    fn help_menu_follows_the_packaging_policy_for_the_update_entry() {
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = menu_app(&mut cx);
        for (updates_managed_externally, expected) in [(false, true), (true, false)] {
            let has_update_command = cx.update_entity(&app, |app, cx| {
                app.help_menu_has_update_command_for_test(updates_managed_externally, cx)
            });
            assert_eq!(has_update_command, expected);
        }
    }

    #[test]
    fn title_view_menu_matches_tauri_structure() {
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = menu_app(&mut cx);
        let items = cx.update_entity(&app, |app, cx| {
            app.title_menu_items_for_test(TitleMenu::View, cx)
        });
        let labels = items
            .iter()
            .map(NyaMenuItem::test_label)
            .collect::<Vec<_>>();

        assert!(labels.contains(&"Theme"));
        assert!(labels.contains(&"Terminal Theme"));
        assert!(labels.contains(&"Language"));
        assert!(labels.contains(&"Header Status"));
        assert!(labels.contains(&"Panels"));
        assert!(labels.contains(&"Zoom In"));
        assert!(labels.contains(&"Zoom Out"));
        assert!(labels.contains(&"Reset Zoom"));
    }

    #[test]
    fn title_terminal_menu_matches_tauri_structure_and_default_state() {
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = menu_app(&mut cx);
        let items = cx.update_entity(&app, |app, cx| {
            app.title_menu_items_for_test(TitleMenu::Terminal, cx)
        });
        let labels = items
            .iter()
            .map(NyaMenuItem::test_label)
            .collect::<Vec<_>>();

        assert!(labels.contains(&"Command Palette"));
        assert!(labels.contains(&"Display"));
        assert!(labels.contains(&"Action Links"));
        assert!(labels.contains(&"Enable terminal zoom"));
        assert!(labels.contains(&"Smart Split"));
        assert!(labels.contains(&"Unsplit"));
        assert!(labels.contains(&"Sync Input"));
        assert!(labels.contains(&"Broadcast All"));
        assert!(labels.contains(&"Clear Terminal"));
        assert!(labels.contains(&"Refit Terminal Size"));

        let action_links = items
            .iter()
            .find(|item| item.test_label() == "Action Links")
            .expect("action links item");
        let zoom_enabled = items
            .iter()
            .find(|item| item.test_label() == "Enable terminal zoom")
            .expect("terminal zoom item");
        let unsplit = items
            .iter()
            .find(|item| item.test_label() == "Unsplit")
            .expect("unsplit item");

        assert!(!action_links.test_presentation().4);
        assert!(zoom_enabled.test_presentation().4);
        assert!(unsplit.test_presentation().3);
    }

    #[test]
    fn terminal_display_submenu_has_expected_checks() {
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = menu_app(&mut cx);
        let items = cx.update_entity(&app, |app, cx| {
            app.title_menu_items_for_test(TitleMenu::Terminal, cx)
        });
        let display = items
            .iter()
            .find(|item| item.test_label() == "Display")
            .expect("terminal display");
        let submenu = display.children().expect("terminal display submenu");
        assert_eq!(
            submenu
                .iter()
                .map(NyaMenuItem::test_label)
                .collect::<Vec<_>>(),
            vec!["Show Padding", "Line Numbers", "Show Timestamps"]
        );
        assert!(submenu.iter().all(|item| !item.test_presentation().4));
    }

    #[test]
    fn panels_submenu_exposes_independent_modes_and_dynamic_sides() {
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = menu_app(&mut cx);
        let items = cx.update_entity(&app, |app, cx| {
            app.title_menu_items_for_test(TitleMenu::View, cx)
        });
        let panels = items
            .iter()
            .find(|item| item.test_label() == "Panels")
            .expect("panels submenu");
        let submenu = panels.children().expect("panels submenu children");
        let labels = submenu
            .iter()
            .map(NyaMenuItem::test_label)
            .collect::<Vec<_>>();

        assert!(labels.contains(&"Floating Panels"));
        assert!(labels.contains(&"Multi Open"));
        assert!(labels.contains(&"Left"));
        assert!(labels.contains(&"Right"));
        assert!(labels.contains(&"Reset Activity Bar Layout"));
        let floating = submenu
            .iter()
            .find(|item| item.test_label() == "Floating Panels")
            .expect("floating entry");
        let multi = submenu
            .iter()
            .find(|item| item.test_label() == "Multi Open")
            .expect("multi-open entry");
        assert!(!floating.test_presentation().4);
        assert!(!multi.test_presentation().4);

        let left = submenu
            .iter()
            .find(|item| item.test_label() == "Left")
            .and_then(NyaMenuItem::children)
            .expect("left panel submenu");
        assert!(left.iter().any(|item| item.test_label() == "Files"));
        assert!(left.iter().any(|item| item.test_label() == "Notes"));
        assert!(left.iter().all(|item| item.test_presentation().4));
    }
}
