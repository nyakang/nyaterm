use rust_i18n::t;

use gpui::Context;
use nyaterm_transport::SftpFileEntry;
use nyaterm_ui::NyaMenuItem;

use crate::features::NyaTermApp;
use crate::models::{TransferBrowserContextTarget, TransferPathPromptKind};

use super::TransferPathPart;

impl NyaTermApp {
    pub(in crate::features::pages::transfers) fn transfer_browser_context_menu_items(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Vec<NyaMenuItem> {
        match self.transfer.browser_view().context_target.clone() {
            TransferBrowserContextTarget::CurrentDirectory => {
                self.transfer_browser_current_context_menu_items(cx)
            }
            TransferBrowserContextTarget::ParentDirectory => {
                self.transfer_browser_parent_context_menu_items(cx)
            }
            TransferBrowserContextTarget::Entry(path) => {
                if self.transfer.rename_dialog_is_open() {
                    return Vec::new();
                }
                let Some(entry) = self
                    .transfer
                    .browser_view()
                    .entries
                    .iter()
                    .find(|entry| entry.matches_identity(&path))
                    .cloned()
                else {
                    return Vec::new();
                };
                self.transfer_browser_entry_context_menu_items(entry, cx)
            }
            TransferBrowserContextTarget::Suppressed => Vec::new(),
        }
    }

    pub(in crate::features::pages::transfers) fn transfer_browser_current_context_menu_items(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Vec<NyaMenuItem> {
        use super::context_menu_policy::{
            TransferContextMenuAction as Action, TransferContextMenuNode as Node,
            transfer_context_action_visible_for_backend,
            transfer_current_directory_context_menu_policy,
        };

        let policy = transfer_current_directory_context_menu_policy();
        let local_backend = self.session.active_file_browser_backend()
            == Some(nyaterm_transport::FileBrowserBackendKind::Local);
        let mut items = Vec::with_capacity(policy.len());
        for node in policy {
            if let Node::Action(action) = node {
                let backend = if local_backend {
                    nyaterm_transport::FileBrowserBackendKind::Local
                } else {
                    nyaterm_transport::FileBrowserBackendKind::Remote
                };
                if !transfer_context_action_visible_for_backend(action, backend) {
                    continue;
                }
            }
            let item = match node {
                Node::Separator => NyaMenuItem::separator(),
                Node::Action(Action::Refresh) => NyaMenuItem::action(t!("fileExplorer.cmRefresh"))
                    .icon("icons/fe/refresh.svg")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.refresh_transfer_browser(window, cx);
                        this.defer_transfer_panel_snapshot_flush(cx);
                    })),
                Node::Action(Action::Upload) => NyaMenuItem::submenu(
                    t!("fileExplorer.cmUpload"),
                    vec![
                        NyaMenuItem::action(t!("fileExplorer.upload"))
                            .icon("icons/fe/upload.svg")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.prompt_transfer_browser_upload_path(
                                    TransferPathPromptKind::UploadFile,
                                    cx,
                                );
                            })),
                        NyaMenuItem::action(t!("fileExplorer.uploadFolder"))
                            .icon("icons/fe/upload-folder.svg")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.prompt_transfer_browser_upload_path(
                                    TransferPathPromptKind::UploadDirectory,
                                    cx,
                                );
                            })),
                    ],
                )
                .icon("icons/fe/upload.svg"),
                Node::Action(Action::NewFile) => NyaMenuItem::action(t!("fileExplorer.newFile"))
                    .icon("icons/fe/new-file.svg")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_transfer_new_file_dialog(window, cx);
                    })),
                Node::Action(Action::NewFolder) => {
                    NyaMenuItem::action(t!("fileExplorer.newFolder"))
                        .icon("icons/fe/new-folder.svg")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_transfer_new_folder_dialog(window, cx);
                        }))
                }
                Node::Action(Action::NewSymlink) => {
                    NyaMenuItem::action(t!("fileExplorer.newSymlink"))
                        .icon("icons/conn/symlink.svg")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_transfer_new_symlink_dialog(window, cx);
                        }))
                }
                Node::Action(Action::CopyDirectoryPath) => {
                    NyaMenuItem::action(t!("fileExplorer.cmCopyDirPath"))
                        .icon("icons/copy.svg")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.copy_current_transfer_browser_path(cx);
                        }))
                }
                Node::Action(Action::SendDirectoryPath) => {
                    NyaMenuItem::action(t!("fileExplorer.cmTerminalDirPath"))
                        .icon("icons/fe/send-path.svg")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.send_current_transfer_browser_path_to_terminal(cx);
                        }))
                }
                Node::Action(Action::Properties) => {
                    NyaMenuItem::action(t!("fileExplorer.cmProperties"))
                        .icon("icons/menu/info.svg")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_current_transfer_browser_properties(window, cx);
                        }))
                }
                // The current-directory policy only emits the nodes handled above;
                // anything else would be a policy/handler drift, so skip it.
                Node::Action(_) => continue,
            };
            items.push(item);
        }
        items
    }

    pub(in crate::features::pages::transfers) fn transfer_browser_parent_context_menu_items(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Vec<NyaMenuItem> {
        use super::context_menu_policy::{
            TransferContextMenuAction as Action, TransferContextMenuNode as Node,
            transfer_parent_directory_context_menu_policy,
        };

        let policy = transfer_parent_directory_context_menu_policy();
        let mut items = Vec::with_capacity(policy.len());
        for node in policy {
            let item = match node {
                Node::Separator => NyaMenuItem::separator(),
                Node::Action(Action::GoUp) => NyaMenuItem::action(t!("fileExplorer.goUp"))
                    .icon("icons/fe/up.svg")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_transfer_parent_directory(window, cx);
                        this.defer_transfer_panel_snapshot_flush(cx);
                    })),
                Node::Action(Action::Refresh) => NyaMenuItem::action(t!("fileExplorer.cmRefresh"))
                    .icon("icons/fe/refresh.svg")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.refresh_transfer_browser(window, cx);
                        this.defer_transfer_panel_snapshot_flush(cx);
                    })),
                Node::Action(_) => continue,
            };
            items.push(item);
        }
        items
    }

    pub(in crate::features::pages::transfers) fn transfer_browser_entry_context_menu_items(
        &mut self,
        entry: SftpFileEntry,
        cx: &mut Context<Self>,
    ) -> Vec<NyaMenuItem> {
        use super::context_menu_policy::{
            TransferContextMenuAction as Action, TransferContextMenuNode as Node,
            TransferEntryMenuCapabilities, transfer_context_action_visible_for_backend,
            transfer_entry_context_menu_policy,
        };

        let ai_actions = self.enabled_transfer_file_ai_actions_for_entry(&entry);
        let send_targets = self.transfer_send_to_targets();
        let policy = transfer_entry_context_menu_policy(TransferEntryMenuCapabilities {
            is_directory: entry.is_directory(),
            show_open_internal: self.show_transfer_open_internal_menu_entry(&entry),
            show_open_external: self.show_transfer_open_external_menu_entry(&entry),
            show_preview: self.show_transfer_preview_menu_entry(&entry),
            has_ai_actions: !ai_actions.is_empty(),
            has_send_targets: !send_targets.is_empty(),
        });
        let local_backend = self.session.active_file_browser_backend()
            == Some(nyaterm_transport::FileBrowserBackendKind::Local);
        let mut items = Vec::with_capacity(policy.len());

        for node in policy {
            if let Node::Action(action) = node {
                let backend = if local_backend {
                    nyaterm_transport::FileBrowserBackendKind::Local
                } else {
                    nyaterm_transport::FileBrowserBackendKind::Remote
                };
                if !transfer_context_action_visible_for_backend(action, backend) {
                    continue;
                }
            }
            let item = match node {
                Node::Separator => NyaMenuItem::separator(),
                Node::Action(Action::Open) => NyaMenuItem::action(t!("fileExplorer.cmOpen"))
                    .icon("icons/session/folder-open.svg")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_selected_transfer_default(window, cx);
                    })),
                Node::Action(Action::Preview) => NyaMenuItem::action(t!("fileExplorer.cmPreview"))
                    .icon("icons/eye.svg")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_selected_transfer_preview(window, cx);
                    })),
                Node::Action(Action::OpenInternal) => {
                    NyaMenuItem::action(t!("fileExplorer.cmOpenInternalEditor"))
                        .icon("icons/net/edit.svg")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_selected_transfer_editor(window, cx);
                        }))
                }
                Node::Action(Action::OpenExternal) => {
                    NyaMenuItem::action(t!("fileExplorer.cmOpenExternalEditor"))
                        .icon("icons/net/edit.svg")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_selected_transfer_external(window, cx);
                        }))
                }
                Node::Action(Action::Refresh) => NyaMenuItem::action(t!("fileExplorer.cmRefresh"))
                    .icon("icons/fe/refresh.svg")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.refresh_transfer_browser(window, cx);
                        this.defer_transfer_panel_snapshot_flush(cx);
                    })),
                Node::Action(Action::Upload) => NyaMenuItem::submenu(
                    t!("fileExplorer.cmUpload"),
                    vec![
                        NyaMenuItem::action(t!("fileExplorer.upload"))
                            .icon("icons/fe/upload.svg")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.prompt_transfer_browser_upload_path(
                                    TransferPathPromptKind::UploadFile,
                                    cx,
                                );
                            })),
                        NyaMenuItem::action(t!("fileExplorer.uploadFolder"))
                            .icon("icons/fe/upload-folder.svg")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.prompt_transfer_browser_upload_path(
                                    TransferPathPromptKind::UploadDirectory,
                                    cx,
                                );
                            })),
                    ],
                )
                .icon("icons/fe/upload.svg"),
                Node::Action(Action::Download) => {
                    NyaMenuItem::action(t!("fileExplorer.cmDownload"))
                        .icon("icons/fe/download.svg")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.start_selected_sftp_download_jobs(window, cx);
                            this.defer_transfer_panel_snapshot_flush(cx);
                        }))
                }
                Node::Action(Action::Rename) => NyaMenuItem::action(t!("fileExplorer.cmRename"))
                    .icon("icons/session/rename.svg")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_transfer_rename_dialog(window, cx);
                        this.defer_transfer_panel_snapshot_flush(cx);
                    })),
                Node::Action(Action::Move) => NyaMenuItem::action(t!("fileExplorer.cmMove"))
                    .icon("icons/net/move.svg")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_transfer_move_dialog_for_selection(window, cx);
                    })),
                Node::Action(Action::Delete) => NyaMenuItem::action(t!("fileExplorer.cmDelete"))
                    .icon("icons/net/delete.svg")
                    .danger()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_selected_transfer_delete_dialog(window, cx);
                    })),
                Node::Action(Action::SendTo) => {
                    let send_items = send_targets
                        .iter()
                        .cloned()
                        .map(|target| {
                            let session_id = target.session_id.clone();
                            let mut item = NyaMenuItem::action(target.label.clone())
                                .icon("icons/send.svg")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.start_send_selected_transfers_to_session(
                                        session_id.clone(),
                                        window,
                                        cx,
                                    );
                                    this.defer_transfer_panel_snapshot_flush(cx);
                                }));
                            if let Some(meta) = target.meta {
                                item = item.shortcut(meta);
                            }
                            item
                        })
                        .collect::<Vec<_>>();
                    NyaMenuItem::submenu(t!("fileExplorer.cmSendTo"), send_items)
                        .icon("icons/send.svg")
                }
                Node::Action(Action::AddToFavorites) => {
                    let favorite_path = entry.path.clone();
                    NyaMenuItem::action(t!("fileExplorer.addToFavorites"))
                        .icon("icons/fe/star.svg")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.add_transfer_browser_favorite_path(favorite_path.clone(), cx);
                            this.defer_transfer_panel_snapshot_flush(cx);
                        }))
                }
                Node::Action(Action::CopyPath) => {
                    NyaMenuItem::action(t!("fileExplorer.cmCopyPath"))
                        .icon("icons/copy.svg")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.copy_selected_transfer_path(TransferPathPart::Full, cx);
                        }))
                }
                Node::Action(Action::CopyName) => {
                    NyaMenuItem::action(t!("fileExplorer.cmCopyName"))
                        .icon("icons/copy.svg")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.copy_selected_transfer_path(TransferPathPart::Name, cx);
                        }))
                }
                Node::Action(Action::CopyDirectoryPath) => {
                    NyaMenuItem::action(t!("fileExplorer.cmCopyDirPath"))
                        .icon("icons/copy.svg")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.copy_selected_transfer_path(TransferPathPart::Directory, cx);
                        }))
                }
                Node::Action(Action::SendPath) => {
                    NyaMenuItem::action(t!("fileExplorer.cmTerminalPath"))
                        .icon("icons/fe/send-path.svg")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.send_selected_transfer_path_to_terminal(
                                TransferPathPart::Full,
                                cx,
                            );
                        }))
                }
                Node::Action(Action::SendName) => {
                    NyaMenuItem::action(t!("fileExplorer.cmTerminalName"))
                        .icon("icons/fe/send-path.svg")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.send_selected_transfer_path_to_terminal(
                                TransferPathPart::Name,
                                cx,
                            );
                        }))
                }
                Node::Action(Action::SendDirectoryPath) => {
                    NyaMenuItem::action(t!("fileExplorer.cmTerminalDirPath"))
                        .icon("icons/fe/send-path.svg")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.send_selected_transfer_path_to_terminal(
                                TransferPathPart::Directory,
                                cx,
                            );
                        }))
                }
                Node::Action(Action::Ai) => {
                    let ai_items = ai_actions
                        .iter()
                        .cloned()
                        .map(|action| {
                            let ai_entry = entry.clone();
                            NyaMenuItem::action(action.name.clone()).on_click(cx.listener(
                                move |this, _, window, cx| {
                                    this.start_transfer_file_ai_action(
                                        ai_entry.clone(),
                                        action.clone(),
                                        window,
                                        cx,
                                    );
                                },
                            ))
                        })
                        .collect();
                    NyaMenuItem::submenu("AI", ai_items).icon("icons/ai.svg")
                }
                Node::Action(Action::Properties) => {
                    NyaMenuItem::action(t!("fileExplorer.cmProperties"))
                        .icon("icons/menu/info.svg")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_selected_transfer_properties(window, cx);
                        }))
                }
                Node::Action(
                    Action::GoUp | Action::NewFile | Action::NewFolder | Action::NewSymlink,
                ) => continue,
            };
            items.push(item);
        }
        items
    }
}
