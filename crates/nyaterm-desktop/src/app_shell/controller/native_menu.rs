//! Process-wide native menu projection and routing. Business state stays in its feature owner.
use gpui::{App, Context, Menu, MenuItem, OsAction, SystemMenuType};
use nyaterm_core::WorkspaceId;
use nyaterm_core::app_identity::AppFlavor;
use nyaterm_ui::{NyaCopy, NyaCut, NyaPaste, NyaRedo, NyaSelectAll, NyaUndo};
use rust_i18n::t;

use crate::app_shell::{
    AppShellLifecycle, DesktopController, DesktopControllerGlobal, NativeHide, NativeHideOthers,
    NativeNewWindow, NativeQuit, NativeShowAll,
};
use crate::features::layout::title_bar::definition::{
    TitleMenuCommand, TitleMenuDefinition, TitleMenuKind, UnavailableTitleMenuCommand,
};
use crate::models::TitleMenu;

#[cfg(test)]
mod tests;

#[derive(Default)]
pub(super) struct NativeMenuState {
    pending: bool,
    installed: Option<NativeMenuProjection>,
    keymap_revision: u64,
}

impl NativeMenuState {
    fn begin_refresh(&mut self) -> bool {
        if self.pending {
            return false;
        }
        self.pending = true;
        true
    }

    fn accept(&mut self, projection: &NativeMenuProjection) -> bool {
        self.pending = false;
        if self.installed.as_ref() == Some(projection) {
            return false;
        }
        self.installed = Some(projection.clone());
        true
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct NativeMenuProjection {
    workspace: Option<WorkspaceId>,
    main_active: bool,
    ready: bool,
    locked: bool,
    keymap_revision: u64,
    menus: Vec<(String, Vec<TitleMenuDefinition>)>,
    edit_labels: Vec<String>,
    app_labels: Vec<String>,
}

impl NativeMenuProjection {
    fn new(
        workspace: Option<WorkspaceId>,
        main_active: bool,
        ready: bool,
        locked: bool,
        keymap_revision: u64,
        app: Option<&crate::features::NyaTermApp>,
        cx: &App,
    ) -> Self {
        let menus = [
            TitleMenu::File,
            TitleMenu::View,
            TitleMenu::Terminal,
            TitleMenu::Help,
        ]
        .into_iter()
        .map(|menu| {
            let mut items = app.map_or_else(Vec::new, |app| app.title_menu_definitions(menu, cx));
            if menu == TitleMenu::Help {
                items.retain(|item| {
                    !matches!(
                        item.kind,
                        TitleMenuKind::Action(Some(TitleMenuCommand::About))
                    )
                });
                if items
                    .last()
                    .is_some_and(|item| matches!(item.kind, TitleMenuKind::Separator))
                {
                    items.pop();
                }
            }
            if menu == TitleMenu::View {
                items.extend([
                    TitleMenuDefinition::separator(),
                    TitleMenuDefinition::action(t!("tray.settings"))
                        .command(TitleMenuCommand::OpenSettings),
                    TitleMenuDefinition::action(t!("settings.shortcutLabels.toggleLeftSidebar"))
                        .command(TitleMenuCommand::ToggleLeftSidebar),
                    TitleMenuDefinition::action(t!("settings.shortcutLabels.toggleRightSidebar"))
                        .command(TitleMenuCommand::ToggleRightSidebar),
                ]);
            }
            if menu == TitleMenu::Terminal {
                items.extend([
                    TitleMenuDefinition::separator(),
                    TitleMenuDefinition::action(t!("menu.copy"))
                        .command(TitleMenuCommand::TerminalCopy),
                    TitleMenuDefinition::action(t!("menu.paste"))
                        .command(TitleMenuCommand::TerminalPaste),
                    TitleMenuDefinition::action(t!("documentEditor.find"))
                        .command(TitleMenuCommand::TerminalFind),
                    TitleMenuDefinition::action(t!("menu.selectAll"))
                        .command(TitleMenuCommand::TerminalSelectAll),
                    TitleMenuDefinition::action(t!("settings.shortcutLabels.localMode"))
                        .command(TitleMenuCommand::TerminalLocalMode),
                ]);
            }
            (t!(menu.i18n_key()).to_string(), items)
        })
        .collect();
        Self {
            workspace,
            main_active,
            ready,
            locked,
            keymap_revision,
            menus,
            edit_labels: [
                "menu.edit",
                "menu.undo",
                "menu.redo",
                "menu.cut",
                "menu.copy",
                "menu.paste",
                "menu.selectAll",
            ]
            .map(|key| t!(key).to_string())
            .to_vec(),
            app_labels: [
                "menu.about",
                "menu.services",
                "menu.hide",
                "menu.hideOthers",
                "menu.showAll",
                "menu.exit",
            ]
            .map(|key| t!(key).to_string())
            .to_vec(),
        }
    }

    fn command_enabled(&self, command: &TitleMenuCommand) -> bool {
        self.ready
            && (!self.locked || *command == TitleMenuCommand::About)
            && (self.main_active || !command.needs_main_window())
    }

    fn into_menus(self, flavor: AppFlavor) -> Vec<Menu> {
        fn convert(item: TitleMenuDefinition, projection: &NativeMenuProjection) -> MenuItem {
            match item.kind {
                TitleMenuKind::Separator => MenuItem::separator(),
                TitleMenuKind::Submenu(children) => MenuItem::submenu(
                    Menu::new(item.label)
                        .items(children.into_iter().map(|item| convert(item, projection)))
                        .disabled(item.disabled || !projection.ready || projection.locked),
                ),
                TitleMenuKind::Action(command) => {
                    let mut name = item.label.to_string();
                    if let Some((status, _)) = item.status {
                        name.push_str(&format!(" ({status})"));
                    }
                    let disabled = item.disabled
                        || command
                            .as_ref()
                            .is_none_or(|cmd| !projection.command_enabled(cmd));
                    MenuItem::Action {
                        name: name.into(),
                        action: command.map_or_else(
                            || Box::new(gpui::NoAction) as Box<dyn gpui::Action>,
                            |command| {
                                if disabled {
                                    Box::new(UnavailableTitleMenuCommand(command))
                                } else {
                                    command.native_action()
                                }
                            },
                        ),
                        os_action: None,
                        checked: item.checked,
                        disabled,
                    }
                }
            }
        }
        let name = flavor.display_name();
        let app = Menu::new(name).items([
            MenuItem::Action {
                name: format!("{} {name}", self.app_labels[0]).into(),
                action: if self.ready {
                    Box::new(TitleMenuCommand::About)
                } else {
                    Box::new(UnavailableTitleMenuCommand(TitleMenuCommand::About))
                },
                os_action: None,
                checked: false,
                disabled: !self.ready,
            },
            if self.ready && !self.locked {
                MenuItem::os_submenu(self.app_labels[1].clone(), SystemMenuType::Services)
            } else {
                MenuItem::submenu(Menu::new(self.app_labels[1].clone()).disabled(true))
            },
            MenuItem::separator(),
            MenuItem::action(format!("{} {name}", self.app_labels[2]), NativeHide),
            MenuItem::action(self.app_labels[3].clone(), NativeHideOthers),
            MenuItem::action(self.app_labels[4].clone(), NativeShowAll),
            MenuItem::separator(),
            MenuItem::action(format!("{} {name}", self.app_labels[5]), NativeQuit),
        ]);
        let edit_disabled = self.locked || !self.ready;
        let edit = Menu::new(self.edit_labels[0].clone())
            .items(
                [
                    MenuItem::os_action(self.edit_labels[1].clone(), NyaUndo, OsAction::Undo),
                    MenuItem::os_action(self.edit_labels[2].clone(), NyaRedo, OsAction::Redo),
                    MenuItem::separator(),
                    MenuItem::os_action(self.edit_labels[3].clone(), NyaCut, OsAction::Cut),
                    MenuItem::os_action(self.edit_labels[4].clone(), NyaCopy, OsAction::Copy),
                    MenuItem::os_action(self.edit_labels[5].clone(), NyaPaste, OsAction::Paste),
                    MenuItem::os_action(
                        self.edit_labels[6].clone(),
                        NyaSelectAll,
                        OsAction::SelectAll,
                    ),
                ]
                .into_iter()
                .map(|item| {
                    if edit_disabled {
                        match item {
                            MenuItem::Action { name, .. } => {
                                MenuItem::action(name, gpui::NoAction).disabled(true)
                            }
                            other => other,
                        }
                    } else {
                        item
                    }
                }),
            )
            .disabled(edit_disabled);
        let mut edit = Some(edit);
        let mut menus = vec![app];
        for (index, (label, items)) in self.menus.iter().enumerate() {
            if index == 1 {
                menus.push(edit.take().expect("one Edit menu"));
            }
            menus.push(
                Menu::new(label.clone())
                    .items(items.clone().into_iter().map(|item| convert(item, &self))),
            );
        }
        menus
    }
}

pub(crate) fn request_refresh(cx: &mut App) {
    if !cfg!(target_os = "macos") {
        return;
    }
    if let Some(global) = cx.try_global::<DesktopControllerGlobal>() {
        let controller = global.0.downgrade();
        cx.defer(move |cx| {
            let _ = controller.update(cx, |controller, cx| {
                controller.request_native_menu_refresh(cx)
            });
        });
    }
}

pub(crate) fn keymap_changed(cx: &mut App) {
    if !cfg!(target_os = "macos") {
        return;
    }
    if let Some(global) = cx.try_global::<DesktopControllerGlobal>() {
        let controller = global.0.downgrade();
        cx.defer(move |cx| {
            let _ = controller.update(cx, |controller, cx| {
                controller.native_menu.keymap_revision =
                    controller.native_menu.keymap_revision.wrapping_add(1);
                controller.request_native_menu_refresh(cx);
            });
        });
    }
}

pub(crate) fn init(cx: &mut App) {
    if !cfg!(target_os = "macos") {
        return;
    }
    cx.bind_keys([
        gpui::KeyBinding::new("cmd-q", NativeQuit, None),
        gpui::KeyBinding::new("cmd-h", NativeHide, None),
        gpui::KeyBinding::new("cmd-alt-h", NativeHideOthers, None),
    ]);
    cx.on_action(|command: &TitleMenuCommand, cx| {
        route(command.clone(), cx);
    });
    cx.on_action(|_: &NativeNewWindow, cx| route(TitleMenuCommand::NewWindow, cx));
    cx.on_action(|_: &NativeHide, cx| {
        cx.hide();
    });
    cx.on_action(|_: &NativeHideOthers, cx| {
        cx.hide_other_apps();
    });
    cx.on_action(|_: &NativeShowAll, cx| {
        cx.unhide_other_apps();
    });
    cx.on_action(|_: &NativeQuit, cx| {
        if let Some(global) = cx.try_global::<DesktopControllerGlobal>() {
            let controller = global.0.clone();
            controller.update(cx, |controller, cx| controller.request_quit(cx));
        }
    });
    // Window handlers consume these actions on a main window. Global handlers provide
    // the application-level fallback while a secondary window is focused.
    cx.on_action(|_: &crate::shortcuts::NewSession, cx| route(TitleMenuCommand::NewSession, cx));
    cx.on_action(|_: &crate::shortcuts::OpenSettings, cx| {
        route(TitleMenuCommand::OpenSettings, cx)
    });
    cx.on_window_closed(|cx, _| request_refresh(cx)).detach();
}

fn route(command: TitleMenuCommand, cx: &mut App) {
    let Some(global) = cx.try_global::<DesktopControllerGlobal>() else {
        return;
    };
    let controller = global.0.clone();
    // App actions may be dispatched during a window update. Route on the next effect
    // so neither the controller nor the target window is borrowed recursively.
    cx.defer(move |cx| {
        let target = controller.update(cx, |controller, cx| {
            controller.prepare_native_title_command(&command, cx)
        });
        // Feature commands (for example opening Settings) can call back into the
        // controller. Release its entity borrow before entering the feature owner.
        if let Some((handle, app)) = target {
            let _ = handle.update(cx, |_, window, cx| {
                app.update(cx, |app, cx| {
                    app.execute_title_menu_command(&command, window, cx)
                });
            });
        }
        request_refresh(cx);
    });
}

impl DesktopController {
    pub(crate) fn workspace_menu_commands_ready(&self, workspace: WorkspaceId, cx: &App) -> bool {
        !self.screen_locked
            && !self.process_quitting
            && !self.closing_workspaces.contains(&workspace)
            && self
                .windows
                .get(&workspace)
                .and_then(|entry| entry.shell.upgrade())
                .is_some_and(|shell| matches!(shell.read(cx).lifecycle, AppShellLifecycle::Ready))
    }

    pub(crate) fn request_native_menu_refresh(&mut self, cx: &mut Context<Self>) {
        if !cfg!(target_os = "macos") || !self.native_menu.begin_refresh() {
            return;
        }
        let controller = cx.weak_entity();
        cx.defer(move |cx| {
            let _ = controller.update(cx, |controller, cx| {
                controller.native_menu.pending = false;
                let projection = controller.native_menu_projection(cx);
                if controller.native_menu.accept(&projection) {
                    cx.set_menus(projection.into_menus(AppFlavor::current()));
                }
            });
        });
    }

    fn native_menu_projection(&self, cx: &App) -> NativeMenuProjection {
        // The native menu belongs to the active main window. Secondary windows use
        // the latest surviving main window only for application-level operations.
        let workspace = selected_workspace(
            self.most_recent_workspace_id,
            cx.active_window(),
            self.windows.iter().map(|(id, entry)| (*id, entry.handle)),
        );
        let entry = workspace.and_then(|id| self.windows.get(&id));
        let shell = entry.and_then(|entry| entry.shell.upgrade());
        let shell = shell.as_ref().map(|shell| shell.read(cx));
        let app = shell
            .as_ref()
            .and_then(|shell| shell.app.as_ref())
            .map(|app| app.read(cx));
        let ready = shell.is_some_and(|shell| matches!(shell.lifecycle, AppShellLifecycle::Ready))
            && !self.process_quitting
            && workspace.is_some_and(|id| !self.closing_workspaces.contains(&id));
        let main_active = entry.is_some_and(|entry| {
            cx.active_window()
                .is_some_and(|active| active == entry.handle)
        });
        NativeMenuProjection::new(
            workspace,
            main_active,
            ready,
            self.screen_locked,
            self.native_menu.keymap_revision,
            app,
            cx,
        )
    }

    fn prepare_native_title_command(
        &mut self,
        command: &TitleMenuCommand,
        cx: &mut Context<Self>,
    ) -> Option<(
        gpui::AnyWindowHandle,
        gpui::Entity<crate::features::NyaTermApp>,
    )> {
        let projection = self.native_menu_projection(cx);
        if !projection.command_enabled(command) {
            return None;
        }
        if *command == TitleMenuCommand::NewWindow {
            let _ = self.open_workspace(
                nyaterm_core::OpenWorkspaceRequest {
                    layout_source_workspace_id: projection.workspace,
                    activation: None,
                    activate: true,
                },
                cx,
            );
            return None;
        }
        let entry = projection.workspace.and_then(|id| self.windows.get(&id))?;
        let handle = entry.handle;
        let shell = entry.shell.upgrade()?;
        let app = shell.read(cx).app.clone()?;
        Some((handle, app))
    }
}

fn selected_workspace(
    recent: Option<WorkspaceId>,
    active: Option<gpui::AnyWindowHandle>,
    windows: impl IntoIterator<Item = (WorkspaceId, gpui::AnyWindowHandle)>,
) -> Option<WorkspaceId> {
    let mut fallback = None;
    for (id, handle) in windows {
        if Some(handle) == active {
            return Some(id);
        }
        if Some(id) == recent {
            fallback = Some(id);
        }
    }
    fallback
}
