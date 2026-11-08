use rust_i18n::t;

use gpui::Context;
use nyaterm_core::SavedConnection;
use nyaterm_ui::{NyaDialogWindowExt as _, NyaMenuItem};

use crate::features::NyaTermApp;
use crate::models::ConnectionListContextTarget;

use super::editor::ordered_connection_groups;

impl NyaTermApp {
    fn connection_move_to_group_menu_items(
        &mut self,
        move_ids: Vec<String>,
        cx: &mut Context<Self>,
    ) -> Vec<NyaMenuItem> {
        let ungrouped_ids = move_ids.clone();
        let mut items = vec![
            NyaMenuItem::action(t!("savedConnections.ungroupedConnections"))
                .icon("icons/conn/connect.svg")
                .on_click(cx.listener(move |this, _, _, cx| {
                    if this
                        .resolve_saved_connection_targets(&ungrouped_ids, cx)
                        .is_some()
                    {
                        this.move_connections_into_group(ungrouped_ids.clone(), None, cx);
                    }
                })),
        ];

        let groups = ordered_connection_groups(self.connection_state.groups());
        if !groups.is_empty() {
            items.push(NyaMenuItem::separator());
        }
        for (group, depth) in groups {
            let group_id = group.id.clone();
            let ids = move_ids.clone();
            let label = format!("{:indent$}{}", "", group.name, indent = depth * 2);
            items.push(
                NyaMenuItem::action(label)
                    .icon("icons/conn/folder.svg")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.resolve_saved_connection_targets(&ids, cx).is_some() {
                            this.move_connections_into_group(
                                ids.clone(),
                                Some(group_id.clone()),
                                cx,
                            );
                        }
                    })),
            );
        }
        items
    }

    pub(in crate::features::pages::connections) fn connection_context_menu_items(
        &mut self,
        connection: SavedConnection,
        cx: &mut Context<Self>,
    ) -> Vec<NyaMenuItem> {
        let selected = self.connection_state.selected_connections();
        let target_ids = if selected.len() > 1
            && self
                .connection_state
                .list_contains_selected_id(&connection.id)
        {
            selected.into_iter().map(|item| item.id).collect()
        } else {
            vec![connection.id.clone()]
        };
        self.build_connection_target_menu_items(connection.id, target_ids, true, cx)
    }

    /// Targets are captured when the menu opens, independently of list selection.
    pub(in crate::features) fn connection_target_menu_items(
        &mut self,
        connection_id: String,
        target_ids: Vec<String>,
        cx: &mut Context<Self>,
    ) -> Vec<NyaMenuItem> {
        self.build_connection_target_menu_items(connection_id, target_ids, false, cx)
    }

    fn build_connection_target_menu_items(
        &mut self,
        connection_id: String,
        target_ids: Vec<String>,
        clear_selection_after_copy: bool,
        cx: &mut Context<Self>,
    ) -> Vec<NyaMenuItem> {
        if self
            .connection_state
            .connection_by_id(&connection_id)
            .is_none()
        {
            self.shell.set_status("connection is no longer available");
            cx.notify();
            return Vec::new();
        }
        let selected_count = target_ids.len();
        let connect_label = if selected_count > 1 {
            format!(
                "{} ({selected_count})",
                t!("savedConnections.connectSelected")
            )
        } else {
            t!("savedConnections.connect").to_string()
        };
        let copy_label = if selected_count > 1 {
            t!("savedConnections.copySelected")
        } else {
            t!("savedConnections.copy")
        };
        let connect_ids = target_ids.clone();
        let copy_ids = target_ids.clone();
        let edit_id = connection_id.clone();
        let rename_id = connection_id.clone();
        let move_items = self.connection_move_to_group_menu_items(target_ids.clone(), cx);

        vec![
            NyaMenuItem::action(connect_label)
                .icon("icons/conn/connect.svg")
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.start_saved_connections_by_ids(&connect_ids, window, cx);
                })),
            NyaMenuItem::action(t!("savedConnections.edit"))
                .icon("icons/net/edit.svg")
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.edit_saved_connection_by_id(&edit_id, window, cx);
                })),
            NyaMenuItem::separator(),
            NyaMenuItem::action(t!("savedConnections.rename"))
                .icon("icons/session/rename.svg")
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.edit_saved_connection_by_id(&rename_id, window, cx);
                })),
            NyaMenuItem::action(copy_label)
                .icon("icons/copy.svg")
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.copy_connections_by_ids(&copy_ids, clear_selection_after_copy, cx);
                })),
            NyaMenuItem::submenu(t!("savedConnections.moveToGroup"), move_items)
                .icon("icons/net/move.svg"),
            NyaMenuItem::separator(),
            NyaMenuItem::action(t!("savedConnections.delete"))
                .icon("icons/net/delete.svg")
                .danger()
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.delete_connections_by_ids(&target_ids, window, cx);
                })),
        ]
    }

    pub(in crate::features::pages::connections) fn connection_group_context_menu_items(
        &mut self,
        group_id: String,
        cx: &mut Context<Self>,
    ) -> Vec<NyaMenuItem> {
        let group_id_new = group_id.clone();
        let group_id_folder = group_id.clone();
        let group_id_open = group_id.clone();
        let group_id_edit = group_id.clone();
        let group_id_delete = group_id.clone();
        let total_in_group = self
            .connection_state
            .saved_connections_in_group_tree(&group_id)
            .len();

        let mut items = vec![
            NyaMenuItem::action(t!("savedConnections.newConnection"))
                .icon("icons/conn/add.svg")
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_connection_editor(
                        None,
                        Some(group_id_new.clone()),
                        false,
                        window,
                        cx,
                    );
                })),
            NyaMenuItem::action(t!("savedConnections.newFolder"))
                .icon("icons/fe/new-folder.svg")
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_connection_group_editor(
                        None,
                        Some(group_id_folder.clone()),
                        window,
                        cx,
                    );
                    this.defer_connection_panel_snapshot_flush(cx);
                })),
        ];
        if total_in_group > 0 {
            items.extend([
                NyaMenuItem::separator(),
                NyaMenuItem::action(t!("savedConnections.openAllConnections"))
                    .icon("icons/fe/forward.svg")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_connection_group_open_confirm(group_id_open.clone(), window, cx);
                    })),
            ]);
        }
        items.extend([
            NyaMenuItem::separator(),
            NyaMenuItem::action(t!("savedConnections.renameFolder"))
                .icon("icons/session/rename.svg")
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_connection_group_editor(
                        Some(group_id_edit.clone()),
                        None,
                        window,
                        cx,
                    );
                    this.defer_connection_panel_snapshot_flush(cx);
                })),
            NyaMenuItem::action(t!("savedConnections.deleteFolder"))
                .icon("icons/net/delete.svg")
                .danger()
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_connection_group_delete_confirm(group_id_delete.clone(), window, cx);
                })),
        ]);
        items
    }

    fn connection_selected_menu_items(
        &mut self,
        selected: Vec<SavedConnection>,
        cx: &mut Context<Self>,
    ) -> Vec<NyaMenuItem> {
        let selected_count = selected.len();
        let move_ids = selected
            .into_iter()
            .map(|connection| connection.id)
            .collect::<Vec<_>>();
        let connect_label = if selected_count > 1 {
            format!(
                "{} ({selected_count})",
                t!("savedConnections.connectSelected")
            )
        } else {
            t!("savedConnections.connect").to_string()
        };
        let connect_ids = move_ids.clone();
        let copy_ids = move_ids.clone();
        let delete_ids = move_ids.clone();
        let move_items = self.connection_move_to_group_menu_items(move_ids, cx);

        vec![
            NyaMenuItem::action(connect_label)
                .icon("icons/conn/connect.svg")
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.start_saved_connections_by_ids(&connect_ids, window, cx);
                })),
            NyaMenuItem::action(t!("savedConnections.copySelected"))
                .icon("icons/copy.svg")
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.copy_connections_by_ids(&copy_ids, true, cx);
                })),
            NyaMenuItem::submenu(t!("savedConnections.moveToGroup"), move_items)
                .icon("icons/net/move.svg"),
            NyaMenuItem::action(t!("savedConnections.delete"))
                .icon("icons/net/delete.svg")
                .danger()
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.delete_connections_by_ids(&delete_ids, window, cx);
                })),
        ]
    }

    /// Items for the list's single context menu, chosen by what was right-clicked.
    ///
    /// The list cannot nest a menu per row: one right-click would open the row's
    /// and the list's together, and the one that never receives the click keeps
    /// re-focusing itself, which leaves any dialog opened afterwards unable to
    /// see its own dismiss actions.
    pub(in crate::features::pages::connections) fn connection_list_target_context_menu_items(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Vec<NyaMenuItem> {
        match self.connection_state.list_context_target().clone() {
            ConnectionListContextTarget::List => self.connection_list_context_menu_items(cx),
            ConnectionListContextTarget::Group(group_id) => {
                self.connection_group_context_menu_items(group_id, cx)
            }
            ConnectionListContextTarget::Connection(connection_id) => {
                let Some(connection) = self
                    .connection_state
                    .connection_by_id(&connection_id)
                    .cloned()
                else {
                    // The row went away between the press and the build.
                    return self.connection_list_context_menu_items(cx);
                };
                self.connection_context_menu_items(connection, cx)
            }
        }
    }

    pub(in crate::features::pages::connections) fn connection_list_context_menu_items(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Vec<NyaMenuItem> {
        let selected = self.connection_state.selected_connections();
        let mut items = if selected.is_empty() {
            Vec::new()
        } else {
            let mut items = self.connection_selected_menu_items(selected, cx);
            items.push(NyaMenuItem::separator());
            items
        };
        items.extend([
            NyaMenuItem::action(t!("savedConnections.newConnection"))
                .icon("icons/conn/add.svg")
                .on_click(cx.listener(|this, _, window, cx| {
                    this.open_connection_editor(None, None, false, window, cx);
                })),
            NyaMenuItem::action(t!("savedConnections.newFolder"))
                .icon("icons/fe/new-folder.svg")
                .on_click(cx.listener(|this, _, window, cx| {
                    this.open_connection_group_editor(None, None, window, cx);
                    this.defer_connection_panel_snapshot_flush(cx);
                })),
            NyaMenuItem::separator(),
            NyaMenuItem::action(t!("settings.importConfig"))
                .icon("icons/import.svg")
                .on_click(cx.listener(|this, _, window, cx| {
                    window.close_nya_dialog(cx);
                    this.open_connection_import_dialog(window, cx);
                })),
        ]);
        items
    }

    pub(in crate::features::pages::connections) fn connection_more_menu_items(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Vec<NyaMenuItem> {
        let selected = self.connection_state.selected_connections();
        let mut items = Vec::new();
        if !self.connection_state.groups().is_empty() {
            items.extend([
                NyaMenuItem::action(t!("savedConnections.expandAllFolders"))
                    .icon("icons/conn/unfold-more.svg")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.set_all_connection_groups_expanded(true, cx);
                    })),
                NyaMenuItem::action(t!("savedConnections.collapseAllFolders"))
                    .icon("icons/conn/unfold-less.svg")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.set_all_connection_groups_expanded(false, cx);
                    })),
                NyaMenuItem::separator(),
            ]);
        }
        items.extend([
            NyaMenuItem::action(t!("settings.exportConfig"))
                .icon("icons/menu/export.svg")
                .on_click(cx.listener(|this, _, window, cx| {
                    window.close_nya_dialog(cx);
                    this.prompt_encrypted_portable_snapshot_export(window, cx);
                })),
            NyaMenuItem::action(t!("settings.importConfig"))
                .icon("icons/import.svg")
                .on_click(cx.listener(|this, _, window, cx| {
                    window.close_nya_dialog(cx);
                    this.open_connection_import_dialog(window, cx);
                })),
        ]);
        if !selected.is_empty() {
            items.push(NyaMenuItem::separator());
            items.extend(self.connection_selected_menu_items(selected, cx));
        }
        if !self.connection_state.connections().is_empty() {
            items.extend([
                NyaMenuItem::separator(),
                NyaMenuItem::action(t!("savedConnections.clearAll"))
                    .icon("icons/transfer/clear-all.svg")
                    .danger()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_connections_clear_all_confirm(window, cx);
                    })),
            ]);
        }
        items
    }
}

#[cfg(test)]
mod tests {
    use gpui::{AppContext as _, TestAppContext};
    use nyaterm_core::{AppRuntime, RuntimeMode, SavedConnection, test_support::TestTempDir};
    use nyaterm_ui::NyaMenuItem;

    use crate::entities::{OverlayStore, StartupRestoreStore, UiStoreHandles};
    use crate::features::NyaTermApp;

    #[test]
    fn connection_menus_preserve_selection_scope_and_assets_use_explicit_single_targets() {
        let root = TestTempDir::new("nyaterm-connection-menu-targets");
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let runtime = AppRuntime::from_parts_for_test(
            RuntimeMode::Portable,
            root.path().to_path_buf(),
            root.path().join("config"),
            root.path().join("logs"),
            root.path().join("cache"),
            None,
        );
        let stores = UiStoreHandles {
            startup_restore: cx.new(|_| StartupRestoreStore::default()),
            overlays: cx.new(|_| OverlayStore::default()),
        };
        let app = cx.new(|cx| NyaTermApp::new(runtime, stores, cx));
        app.update(&mut cx, |app, cx| {
            let connections = ["first", "second", "third"].map(|id| {
                serde_json::from_value::<SavedConnection>(serde_json::json!({
                    "id": id, "name": id, "type": "local_terminal",
                }))
                .unwrap()
            });
            app.connection_state
                .replace_loaded(connections.to_vec(), Vec::new());
            let ids = connections
                .iter()
                .map(|item| item.id.clone())
                .collect::<Vec<_>>();
            app.connection_state
                .select_list_connection("first".into(), &ids, false, false);
            app.connection_state
                .select_list_connection("second".into(), &ids, true, false);
            let menu_labels = |items: &[NyaMenuItem]| {
                items
                    .iter()
                    .map(|item| item.test_label().to_string())
                    .collect::<Vec<_>>()
            };
            let mut expected_labels = vec![
                format!("{} (2)", rust_i18n::t!("savedConnections.connectSelected")),
                rust_i18n::t!("savedConnections.edit").to_string(),
                String::new(),
                rust_i18n::t!("savedConnections.rename").to_string(),
                rust_i18n::t!("savedConnections.copySelected").to_string(),
                rust_i18n::t!("savedConnections.moveToGroup").to_string(),
                String::new(),
                rust_i18n::t!("savedConnections.delete").to_string(),
            ];
            let selected = app.connection_context_menu_items(connections[0].clone(), cx);
            assert_eq!(menu_labels(&selected), expected_labels);
            expected_labels[0] = rust_i18n::t!("savedConnections.connect").to_string();
            expected_labels[4] = rust_i18n::t!("savedConnections.copy").to_string();
            let unselected = app.connection_context_menu_items(connections[2].clone(), cx);
            assert_eq!(menu_labels(&unselected), expected_labels);
            let asset = app.connection_target_menu_items("first".into(), vec!["first".into()], cx);
            assert_eq!(menu_labels(&asset), expected_labels);
            assert_eq!(app.connection_state.selected_connections().len(), 2);

            let mut changed = connections[0].clone();
            changed.name = "Latest connection".into();
            app.connection_state
                .replace_loaded(vec![changed], Vec::new());
            let latest = app
                .resolve_saved_connection_targets(&["first".into()], cx)
                .unwrap();
            assert_eq!(latest[0].name, "Latest connection");
            assert!(
                app.resolve_saved_connection_targets(&["first".into(), "second".into()], cx)
                    .is_none()
            );
            assert_eq!(app.shell.status(), "connection is no longer available");
            assert!(
                app.connection_target_menu_items("second".into(), vec!["second".into()], cx)
                    .is_empty()
            );
        });
    }
}
