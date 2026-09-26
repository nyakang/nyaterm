use std::sync::Arc;

use gpui::{Context, KeyDownEvent, Window};
use nyaterm_transport::{FileBrowserBackendKind, RemoteFilePath, SftpFileEntry, file_browser_join};

use crate::features::NyaTermApp;
use crate::models::{
    TransferBrowserContextTarget, TransferJobEvent, TransferJobKind, TransferJobOutput,
    TransferJobResult, TransferJobState, TransferJobStatus,
};

impl NyaTermApp {
    pub(super) fn request_missing_expanded_tree_listings(&mut self, cx: &mut Context<Self>) {
        if self.settings.summary().ui_file_explorer_view_mode
            != nyaterm_core::TransferBrowserViewMode::Tree
        {
            return;
        }
        let Some(session) = self.session.active_id_owned() else {
            return;
        };
        for path in self.transfer.missing_expanded_tree_paths(&session) {
            self.request_transfer_tree_listing(path, cx);
        }
    }
    fn request_transfer_tree_listing(&mut self, path: RemoteFilePath, cx: &mut Context<Self>) {
        let Some(session_id) = self.session.active_id_owned() else {
            return;
        };
        let Some(backend) = self.session.active_file_browser_backend() else {
            return;
        };
        let service = match self.active_file_browser_service() {
            Ok(service) => service,
            Err(_) => return,
        };
        let Some(generation) = self
            .transfer
            .begin_tree_request(&session_id, backend, path.clone())
        else {
            return;
        };
        let id = self.transfer.next_transfer_job_id("sftp-tree");
        self.transfer.enqueue_transfer_job(TransferJobState {
            id: id.clone(),
            session_id: Some(session_id),
            kind: TransferJobKind::ListTree {
                path: path.clone(),
                generation,
            },
            status: TransferJobStatus::Running,
            detail: String::new(),
            created_at_ms: TransferJobState::now_ms(),
            display_name: String::new(),
            entries: Vec::new(),
            summary: None,
            progress: None,
            control: None,
            speed: Default::default(),
        });
        let tx = self.transfer.transfer_event_sender();
        self.submit_transfer_blocking_job("sftp-tree-listing", id.clone(), tx.clone(), move || {
            let result = service
                .list_dir_path(&path)
                .map(TransferJobOutput::TreeEntries)
                .map_err(|error| error.to_string());
            let _ = tx.unbounded_send(TransferJobResult {
                id,
                event: TransferJobEvent::Finished(result),
            });
        });
        cx.notify();
    }

    pub(in crate::features) fn reveal_transfer_tree_current_path(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        if self.settings.summary().ui_file_explorer_view_mode
            != nyaterm_core::TransferBrowserViewMode::Tree
        {
            return;
        }
        let Some(session_id) = self.session.active_id_owned() else {
            return;
        };
        let Some(backend) = self.session.active_file_browser_backend() else {
            return;
        };
        let mut path = self.transfer.browser_remote_file_path();
        if backend == FileBrowserBackendKind::Remote && !path.display_path.starts_with('/') {
            let home = self.transfer.browser_view().home_dir;
            if home.starts_with('/') && path.raw_path_token.is_none() {
                path = RemoteFilePath::new(if path.display_path == "." {
                    home.to_string()
                } else {
                    file_browser_join(backend, home, &path.display_path)
                });
            }
        }
        let missing = self.transfer.reveal_tree_path(&session_id, backend, path);
        for path in missing {
            self.request_transfer_tree_listing(path, cx);
        }
    }

    pub(in crate::features) fn select_transfer_tree_row(
        &mut self,
        key: String,
        additive: bool,
        range: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(session) = self.session.active_id_owned() {
            self.transfer
                .select_tree_row(&session, key, additive, range);
        }
        self.transfer.tree_focus().focus(window, cx);
        cx.notify();
    }

    pub(in crate::features) fn expand_transfer_tree_row(
        &mut self,
        key: &str,
        expand: Option<bool>,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.session.active_id_owned() else {
            return;
        };
        if let Some(path) = self.transfer.toggle_tree_node(&session, key, expand) {
            self.request_transfer_tree_listing(path, cx);
        }
        cx.notify();
    }

    pub(in crate::features) fn prepare_transfer_tree_entry_context_menu(
        &mut self,
        key: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.session.active_id_owned() else {
            return;
        };
        self.dismiss_transfer_rename_if_open(cx);
        self.transfer.close_browser_path_menu();
        self.transfer
            .set_browser_context_target(TransferBrowserContextTarget::Entry(key.clone()));
        self.transfer.select_tree_context_row(&session, key);
        self.transfer.tree_focus().focus(window, cx);
        cx.notify();
    }

    pub(in crate::features) fn prepare_transfer_tree_current_context_menu(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.session.active_id_owned() else {
            return;
        };
        self.dismiss_transfer_rename_if_open(cx);
        self.transfer
            .set_browser_context_target(TransferBrowserContextTarget::CurrentDirectory);
        self.transfer.clear_tree_selection(&session);
        self.transfer.tree_focus().focus(window, cx);
        cx.notify();
    }

    pub(in crate::features) fn navigate_transfer_tree_row(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.session.active_id_owned() else {
            return;
        };
        let Some(row) = self.transfer.selected_tree_row(&session) else {
            return;
        };
        if row.directory {
            if let Some(entry) = row.entry {
                self.open_transfer_browser_entry_directory(entry, window, cx);
            } else {
                self.open_transfer_browser_directory(row.path.display_path, window, cx);
            }
        } else if let Some(entry) = row.entry {
            self.open_transfer_default(entry, window, cx);
        }
    }

    pub(in crate::features) fn handle_transfer_tree_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.session.active_id_owned() else {
            return;
        };
        let row = self.transfer.selected_tree_row(&session);
        let modified_for_select_all = (event.keystroke.modifiers.platform
            || event.keystroke.modifiers.control)
            && !event.keystroke.modifiers.alt
            && !event.keystroke.modifiers.shift;
        if modified_for_select_all && event.keystroke.key.eq_ignore_ascii_case("a") {
            self.transfer.select_all_tree_rows(&session);
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if modified_for_select_all {
            match event.keystroke.key.to_ascii_lowercase().as_str() {
                "c" => self.capture_transfer_file_clipboard(false, cx),
                "x" => self.capture_transfer_file_clipboard(true, cx),
                "v" => self.paste_transfer_file_clipboard(window, cx),
                _ => {}
            }
            if matches!(
                event.keystroke.key.to_ascii_lowercase().as_str(),
                "c" | "x" | "v"
            ) {
                cx.stop_propagation();
                return;
            }
        }
        let unmodified = !event.keystroke.modifiers.alt
            && !event.keystroke.modifiers.control
            && !event.keystroke.modifiers.platform
            && !event.keystroke.modifiers.shift;
        match event.keystroke.key.as_str() {
            "up" if unmodified => self.transfer.move_tree_selection(&session, -1),
            "down" if unmodified => self.transfer.move_tree_selection(&session, 1),
            "left" if unmodified => {
                if let Some(row) = row {
                    if row.expanded {
                        self.expand_transfer_tree_row(&row.key, Some(false), cx);
                    } else {
                        self.transfer.select_tree_parent(&session, &row.key);
                    }
                }
            }
            "right" if unmodified => {
                if let Some(row) = row {
                    if row.expandable && !row.expanded {
                        self.expand_transfer_tree_row(&row.key, Some(true), cx);
                    } else {
                        self.transfer.move_tree_selection(&session, 1);
                    }
                }
            }
            "enter" if unmodified => self.navigate_transfer_tree_row(window, cx),
            "delete" if unmodified && !self.selected_transfer_entries().is_empty() => {
                self.open_selected_transfer_delete_dialog(window, cx)
            }
            key if unmodified && key.eq_ignore_ascii_case("f2") => {
                let entries = self.selected_transfer_entries();
                if let [entry] = entries.as_slice() {
                    self.open_transfer_rename_for_path_and_focus(entry.path.clone(), cx);
                }
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// Apply already-fetched write listings and invalidate just affected parents.
    pub(super) fn update_transfer_tree_from_output(
        &mut self,
        session: &str,
        output: &TransferJobOutput,
    ) {
        let Some(backend) = self
            .session
            .file_browser_backend_support_for_session(session)
        else {
            return;
        };
        let seed = |app: &mut Self, path: &str, entries: &[SftpFileEntry]| {
            let path = RemoteFilePath::new(path);
            app.transfer.invalidate_tree_path(session, &path, false);
            app.transfer
                .seed_tree_listing(session, backend, path, Arc::new(entries.to_vec()));
        };
        match output {
            TransferJobOutput::CwdSynced {
                remote_path,
                entries,
            } => seed(self, remote_path, entries),
            TransferJobOutput::Renamed {
                old_path,
                parent_path,
                entries,
                ..
            } => {
                self.transfer
                    .invalidate_tree_path(session, &RemoteFilePath::new(old_path), true);
                seed(self, parent_path, entries);
            }
            TransferJobOutput::Moved {
                old_path,
                new_path,
                parent_path,
                entries,
            } => {
                self.transfer
                    .invalidate_tree_path(session, &RemoteFilePath::new(old_path), true);
                let parent = nyaterm_transport::file_browser_parent(backend, new_path);
                self.transfer
                    .invalidate_tree_path(session, &RemoteFilePath::new(parent), false);
                seed(self, parent_path, entries);
            }
            TransferJobOutput::Deleted {
                remote_path,
                parent_path,
                entries,
            } => {
                self.transfer.invalidate_tree_path(
                    session,
                    &RemoteFilePath::new(remote_path),
                    true,
                );
                seed(self, parent_path, entries);
            }
            TransferJobOutput::CreatedDirectory {
                remote_path,
                parent_path,
                entries,
                open_after_create,
            } => {
                if *open_after_create {
                    self.transfer.invalidate_tree_path(
                        session,
                        &RemoteFilePath::new(parent_path),
                        false,
                    );
                    seed(self, remote_path, entries);
                } else {
                    seed(self, parent_path, entries);
                }
            }
            TransferJobOutput::Uploaded {
                parent_path,
                entries,
                ..
            }
            | TransferJobOutput::CreatedFile {
                parent_path,
                entries,
                ..
            }
            | TransferJobOutput::CreatedSymlink {
                parent_path,
                entries,
                ..
            }
            | TransferJobOutput::PropertiesUpdated {
                parent_path,
                entries,
                ..
            } => seed(self, parent_path, entries),
            TransferJobOutput::Sent {
                source_path,
                source_parent_path,
                source_entries,
                target_session_id,
                target_parent_path,
                entries,
                ..
            } => {
                if let Some(parent) = source_parent_path {
                    self.transfer.invalidate_tree_path(
                        session,
                        &RemoteFilePath::new(source_path),
                        true,
                    );
                    self.transfer.invalidate_tree_path(
                        session,
                        &RemoteFilePath::new(parent),
                        false,
                    );
                    if let Some(source_entries) = source_entries {
                        seed(self, parent, source_entries);
                    }
                }
                let Some(target_backend) = self
                    .session
                    .file_browser_backend_support_for_session(target_session_id)
                else {
                    return;
                };
                self.transfer.invalidate_tree_path(
                    target_session_id,
                    &RemoteFilePath::new(target_parent_path),
                    false,
                );
                self.transfer.seed_tree_listing(
                    target_session_id,
                    target_backend,
                    RemoteFilePath::new(target_parent_path),
                    Arc::new(entries.clone()),
                );
            }
            _ => {}
        }
    }
}
