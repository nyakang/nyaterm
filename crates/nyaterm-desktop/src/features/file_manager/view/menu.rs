use super::FileManagerNameKind;
use crate::features::file_manager::operations::FileManagerSelection;
use crate::features::file_manager::pane::PaneId;
use crate::features::file_manager::workspace::FileManagerWorkspace;
use crate::features::transfers::file_open::FileOpenMode;
use crate::features::transfers::file_operations::{FileTransferIntent, FileTransferOperation};
use gpui::{App, ClickEvent, ClipboardItem, Context, Window};
use nyaterm_files::file_browser::{
    FileBrowserBackendKind, file_browser_parent, listing::compare_entries,
};
use nyaterm_files::sftp::SftpFileEntry;
use nyaterm_ui::NyaMenuItem;
use rust_i18n::t;

#[derive(Clone, Copy)]
enum PathPart {
    Full,
    Name,
    Parent,
}

fn copied_paths(
    selection: &FileManagerSelection,
    part: PathPart,
    backend: FileBrowserBackendKind,
) -> String {
    selection
        .entries
        .iter()
        .map(|entry| match part {
            PathPart::Full => entry.path.clone(),
            PathPart::Name => entry.name.clone(),
            PathPart::Parent => file_browser_parent(backend, &entry.path),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

impl FileManagerWorkspace {
    fn menu_listener(
        &self,
        pane: PaneId,
        cx: &Context<Self>,
        listener: impl Fn(&mut Self, &ClickEvent, &mut Window, &mut Context<Self>) + 'static,
    ) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
        let endpoint = self.pane(pane).endpoint.clone();
        let generation = self.read_app(cx, |app| {
            endpoint
                .session_id()
                .and_then(|id| app.file_access_generation(id))
        });
        cx.listener(move |this, event, window, cx| {
            if this.pane(pane).endpoint != endpoint || this.pane(pane).disconnected {
                return;
            }
            let current_generation = this.read_app(cx, |app| {
                endpoint
                    .session_id()
                    .and_then(|id| app.file_access_generation(id))
            });
            if generation == current_generation {
                listener(this, event, window, cx);
            }
        })
    }

    pub(super) fn entry_menu(
        &self,
        pane: PaneId,
        entry: SftpFileEntry,
        cx: &mut Context<Self>,
    ) -> Vec<NyaMenuItem> {
        let state = self.pane(pane);
        let mut selection = self.selection(pane);
        selection.entries.sort_by(|left, right| {
            compare_entries(left, right, state.sort_column, state.sort_direction)
        });
        let available = !state.disconnected;
        let has_selection = !selection.entries.is_empty();
        let can_paste =
            available && self.read_app(cx, |app| app.can_paste_transfer_file_clipboard(cx));
        let modifier = if cfg!(target_os = "macos") {
            "Cmd"
        } else {
            "Ctrl"
        };
        let clipboard_item = |cut: bool, paste: bool, label, icon, shortcut| {
            let selection = selection.clone();
            NyaMenuItem::action(label)
                .icon(icon)
                .shortcut(shortcut)
                .disabled(if paste {
                    !can_paste
                } else {
                    !available || !has_selection
                })
                .on_click(self.menu_listener(pane, cx, move |this, _, window, cx| {
                    let selection = selection.clone();
                    this.dispatch_app(window, cx, move |app, _, cx| {
                        if paste {
                            app.paste_file_clipboard(selection.endpoint, selection.directory, cx);
                        } else {
                            app.capture_file_clipboard(
                                selection.endpoint,
                                selection.entries,
                                cut,
                                cx,
                            );
                        }
                    });
                }))
        };
        let rename = self
            .name_menu_item(pane, &selection, FileManagerNameKind::Rename, cx)
            .icon("icons/edit.svg")
            .shortcut("F2");
        let delete_selection = selection.clone();
        let delete = NyaMenuItem::action(t!("fileExplorer.delete"))
            .icon("icons/delete.svg")
            .shortcut("Delete")
            .danger()
            .disabled(!available || !has_selection)
            .on_click(self.menu_listener(pane, cx, move |this, _, window, cx| {
                let id = this.id.clone();
                let selection = delete_selection.clone();
                this.dispatch_app(window, cx, move |app, window, cx| {
                    app.confirm_file_manager_delete_selection(id, selection, window, cx);
                });
            }));
        let mut items = vec![
            NyaMenuItem::action_bar([
                clipboard_item(
                    true,
                    false,
                    t!("menu.cut"),
                    "icons/scissors.svg",
                    format!("{modifier}+X"),
                ),
                clipboard_item(
                    false,
                    false,
                    t!("menu.copy"),
                    "icons/copy.svg",
                    format!("{modifier}+C"),
                ),
                clipboard_item(
                    false,
                    true,
                    t!("menu.paste"),
                    "icons/menu/paste.svg",
                    format!("{modifier}+V"),
                ),
                rename,
                delete,
            ]),
            NyaMenuItem::separator(),
        ];
        for (mode, label, icon, shortcut) in [
            (
                FileOpenMode::Default,
                t!("fileExplorer.cmOpen"),
                "icons/session/folder-open.svg",
                "Enter",
            ),
            (
                FileOpenMode::Preview,
                t!("fileManager.viewFile"),
                "icons/eye.svg",
                "F3",
            ),
            (
                FileOpenMode::Edit,
                t!("fileManager.editFile"),
                "icons/net/edit.svg",
                "F4",
            ),
        ] {
            if entry.is_directory() && !matches!(mode, FileOpenMode::Default) {
                continue;
            }
            let entry = entry.clone();
            let endpoint = state.endpoint.clone();
            items.push(
                NyaMenuItem::action(label)
                    .icon(icon)
                    .shortcut(shortcut)
                    .disabled(!available)
                    .on_click(self.menu_listener(pane, cx, move |this, _, window, cx| {
                        if entry.is_directory() {
                            this.navigate_to(pane, entry.remote_path(), window, cx);
                        } else {
                            let entry = entry.clone();
                            let endpoint = endpoint.clone();
                            this.dispatch_app(window, cx, move |app, window, cx| {
                                app.open_file_for_endpoint(endpoint, entry, mode, window, cx);
                            });
                        }
                    })),
            );
        }
        items.push(NyaMenuItem::separator());
        let target = self.pane(pane.other());
        let side = t!(if pane.other() == PaneId::Left {
            "fileManager.leftSide"
        } else {
            "fileManager.rightSide"
        });
        let target_label = self.read_app(cx, |app| app.endpoint_label(&target.endpoint));
        for (operation, key, icon, shortcut) in [
            (
                FileTransferOperation::Copy,
                "fileManager.copyToPane",
                "icons/copy.svg",
                "F5",
            ),
            (
                FileTransferOperation::Move,
                "fileManager.moveToPane",
                "icons/net/move.svg",
                "F6",
            ),
        ] {
            let intent = FileTransferIntent {
                source: selection.endpoint.clone(),
                entries: selection.entries.clone(),
                target: target.endpoint.clone(),
                target_directory: target.path.clone(),
                operation,
            };
            items.push(
                NyaMenuItem::action(t!(
                    key,
                    side = side.as_ref(),
                    target = target_label.as_str()
                ))
                .icon(icon)
                .shortcut(shortcut)
                .disabled(!available || !has_selection || target.disconnected)
                .on_click(self.menu_listener(
                    pane,
                    cx,
                    move |this, _, window, cx| {
                        let id = this.id.clone();
                        let intent = intent.clone();
                        this.dispatch_app(window, cx, move |app, _, cx| {
                            app.submit_file_transfer(id, intent, cx)
                        });
                    },
                )),
            );
        }
        items.push(NyaMenuItem::separator());
        items.push(
            NyaMenuItem::submenu(
                t!("fileManager.new"),
                vec![
                    self.name_menu_item(pane, &selection, FileManagerNameKind::File, cx)
                        .icon("icons/fe/new-file.svg"),
                    self.name_menu_item(pane, &selection, FileManagerNameKind::Folder, cx)
                        .icon("icons/fe/new-folder.svg")
                        .shortcut("F7"),
                ],
            )
            .icon("icons/plus.svg")
            .disabled(!available),
        );
        let path_items = [
            (PathPart::Full, "fileManager.copyFullPath"),
            (PathPart::Name, "fileManager.copyName"),
            (PathPart::Parent, "fileManager.copyParent"),
        ]
        .into_iter()
        .map(|(part, key)| {
            let backend = self
                .read_app(cx, |app| {
                    Some(app.file_browser_backend_for_endpoint(&selection.endpoint))
                })
                .unwrap_or_else(|| selection.endpoint.kind());
            let value = copied_paths(&selection, part, backend);
            NyaMenuItem::action(t!(key))
                .icon("icons/copy.svg")
                .on_click(move |_, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(value.clone()));
                })
        })
        .collect();
        items.push(
            NyaMenuItem::submenu(t!("fileExplorer.cmCopyPath"), path_items)
                .icon("icons/copy.svg")
                .disabled(!has_selection),
        );
        let properties_selection = selection.clone();
        items.push(
            NyaMenuItem::action(t!("fileExplorer.properties"))
                .icon("icons/menu/info.svg")
                .disabled(!available || selection.entries.len() != 1)
                .on_click(self.menu_listener(pane, cx, move |this, _, window, cx| {
                    let id = this.id.clone();
                    let selection = properties_selection.clone();
                    this.dispatch_app(window, cx, move |app, window, cx| {
                        app.open_file_manager_properties(id, pane, selection, window, cx);
                    });
                })),
        );
        let snapshot = self.read_app(cx, |app| app.plugins.panel.read(cx).catalog(cx));
        for contribution in &snapshot.contributions {
            let declared = snapshot
                .plugins
                .iter()
                .find(|plugin| plugin.id == contribution.plugin_id)
                .and_then(|plugin| plugin.manifest.as_ref())
                .is_some_and(|manifest| {
                    manifest
                        .contributions
                        .file_menu
                        .contains(&contribution.action.id)
                });
            if !declared {
                continue;
            }
            let contribution = contribution.clone();
            let endpoint = state.endpoint.clone();
            let paths: Vec<_> = selection
                .entries
                .iter()
                .map(|entry| entry.path.clone())
                .collect();
            items.push(NyaMenuItem::action(contribution.action.name.clone()).disabled(!available)
                .on_click(self.menu_listener(pane, cx, move |this, _, window, cx| {
                    let contribution = contribution.clone();
                    let endpoint = endpoint.clone();
                    let paths = paths.clone();
                    this.dispatch_app(window, cx, move |app, _, cx| {
                        let session = endpoint.session_id().map(|id| serde_json::json!({ "id": id, "generation": 0,
                            "connection_id": app.session.metadata(id).and_then(|metadata| metadata.source_connection_id.clone()) }));
                        let text = serde_json::json!({ "session": session, "paths": paths }).to_string();
                        app.open_panel(crate::models::NavItem::Plugins, cx);
                        app.plugins.panel.update(cx, |panel, cx| panel.run_contribution(contribution, Some(text), cx));
                    });
                })));
        }
        items
    }

    fn name_menu_item(
        &self,
        pane: PaneId,
        selection: &FileManagerSelection,
        kind: FileManagerNameKind,
        cx: &Context<Self>,
    ) -> NyaMenuItem {
        let label = match kind {
            FileManagerNameKind::Rename => t!("fileExplorer.cmRename"),
            FileManagerNameKind::Folder => t!("fileExplorer.newFolder"),
            FileManagerNameKind::File => t!("fileExplorer.newFile"),
        };
        let disabled = self.pane(pane).disconnected
            || (matches!(kind, FileManagerNameKind::Rename) && selection.entries.len() != 1);
        let selection = selection.clone();
        NyaMenuItem::action(label)
            .disabled(disabled)
            .on_click(self.menu_listener(pane, cx, move |this, _, window, cx| {
                let id = this.id.clone();
                let selection = selection.clone();
                this.dispatch_app(window, cx, move |app, window, cx| {
                    app.open_file_manager_name_target(id, selection, kind, window, cx);
                });
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::{PathPart, copied_paths};
    use crate::features::file_manager::operations::FileManagerSelection;
    use crate::features::file_manager::pane::PaneEndpoint;
    use nyaterm_files::sftp::{RemoteFilePath, SftpFileEntry, SftpFileType};

    fn entry(name: &str, path: &str) -> SftpFileEntry {
        SftpFileEntry {
            name: name.into(),
            path: path.into(),
            file_type: SftpFileType::File,
            size: None,
            permissions: None,
            owner: String::new(),
            group: String::new(),
            modified_at: None,
            raw_path_token: None,
            symlink_target_is_directory: false,
        }
    }

    #[test]
    fn copy_path_parts_preserve_names_order_and_repeated_parent_directories() {
        let mut selection = FileManagerSelection {
            endpoint: PaneEndpoint::Session("remote".into()),
            directory: RemoteFilePath::new("/root/test"),
            entries: vec![entry("config.json", "/root/test/config.json")],
        };
        assert_eq!(
            copied_paths(&selection, PathPart::Full, selection.endpoint.kind()),
            "/root/test/config.json"
        );
        assert_eq!(
            copied_paths(&selection, PathPart::Name, selection.endpoint.kind()),
            "config.json"
        );
        assert_eq!(
            copied_paths(&selection, PathPart::Parent, selection.endpoint.kind()),
            "/root/test"
        );
        selection
            .entries
            .push(entry("中文 file.txt", "/root/test/中文 file.txt"));
        assert_eq!(
            copied_paths(&selection, PathPart::Name, selection.endpoint.kind()),
            "config.json\n中文 file.txt"
        );
        assert_eq!(
            copied_paths(&selection, PathPart::Parent, selection.endpoint.kind()),
            "/root/test\n/root/test"
        );
        selection.entries[0].file_type = SftpFileType::Directory;
        assert_eq!(
            copied_paths(&selection, PathPart::Parent, selection.endpoint.kind()),
            "/root/test\n/root/test"
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn local_copy_parent_understands_windows_drive_and_unc_paths() {
        for (path, parent) in [
            (r"C:\Users\name\config.json", r"C:\Users\name"),
            (
                r"\\server\share\folder\config.json",
                r"\\server\share\folder",
            ),
        ] {
            let selection = FileManagerSelection {
                endpoint: PaneEndpoint::Local,
                directory: RemoteFilePath::new(parent),
                entries: vec![entry("config.json", path)],
            };
            assert_eq!(
                copied_paths(&selection, PathPart::Parent, selection.endpoint.kind()),
                parent
            );
            assert_eq!(
                copied_paths(&selection, PathPart::Full, selection.endpoint.kind()),
                path
            );
        }
    }
}
