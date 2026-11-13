use super::{TransferPathPart, transfer_path_part_value};
use crate::features::NyaTermApp;
use crate::models::{TransferBrowserContextTarget, TransferBrowserDragSelectionState};
use gpui::{ClickEvent, Context, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Window};
use nyaterm_core::truncate_preview;
use nyaterm_files::sftp::SftpFileEntry;
use std::collections::HashSet;

/// Converts a selection identity key to its display path so raw-path-token is not
/// treated as a user-facing path.
fn browser_display_path_for_identity(entries: &[SftpFileEntry], identity: &str) -> String {
    entries
        .iter()
        .find(|entry| entry.matches_identity(identity))
        .map(|entry| entry.path.clone())
        .unwrap_or_else(|| identity.to_string())
}

fn browser_operation_target_directory(
    backend: nyaterm_files::file_browser::FileBrowserBackendKind,
    selected: &[SftpFileEntry],
    current: &str,
    home: &str,
    fallback: &str,
) -> String {
    if let [entry] = selected {
        return if entry.is_directory() {
            entry.path.clone()
        } else {
            nyaterm_files::file_browser::file_browser_parent(backend, &entry.path)
        };
    }
    [current, home, fallback]
        .into_iter()
        .map(str::trim)
        .find(|path| !path.is_empty())
        .unwrap_or(".")
        .to_string()
}

impl NyaTermApp {
    pub(in crate::features) fn transfer_browser_operation_target_directory(&self) -> String {
        let backend = self
            .session
            .active_file_browser_backend()
            .unwrap_or(nyaterm_files::file_browser::FileBrowserBackendKind::Remote);
        if self.settings.summary().ui_file_explorer_view_mode
            == nyaterm_core::TransferBrowserViewMode::Tree
            && let Some(session) = self.session.active_id()
        {
            let selected = self.transfer.selected_tree_entries(session);
            if selected.len() == 1 {
                return browser_operation_target_directory(
                    backend,
                    &selected,
                    "",
                    "",
                    &self.transfer.normalized_remote_path(),
                );
            }
        }
        let path = self.transfer.browser_view().path.trim();
        let home = self.transfer.browser_view().home_dir.trim();
        browser_operation_target_directory(
            backend,
            &[],
            path,
            home,
            &self.transfer.normalized_remote_path(),
        )
    }

    pub(in crate::features::pages::transfers) fn select_transfer_browser_entry(
        &mut self,
        identity: String,
        cx: &mut Context<Self>,
    ) {
        let display_path =
            browser_display_path_for_identity(self.transfer.browser_view().entries, &identity);
        self.transfer.select_browser_entry(identity);
        self.transfer.set_remote_path(display_path.clone());
        self.shell
            .set_status(format!("selected remote {display_path}"));
        cx.notify();
    }

    pub(in crate::features::pages::transfers) fn select_transfer_browser_entry_from_click(
        &mut self,
        identity: String,
        event: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(self.transfer.browser_view().focus, cx);
        self.transfer.clear_browser_rename_click();
        if self.transfer.consume_browser_selection_click(&identity) {
            self.select_transfer_browser_entry(identity.clone(), cx);
        }
        let modifiers = event.modifiers();
        if event.click_count() >= 2 && !modifiers.modified() {
            self.open_transfer_browser_entry_from_double_click(identity, window, cx);
        }
    }

    pub(in crate::features::pages::transfers) fn open_transfer_browser_entry_from_double_click(
        &mut self,
        identity: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cancel_transfer_browser_pending_rename(cx);
        self.dismiss_transfer_rename_if_open(cx);
        let entry = self
            .transfer
            .browser_view()
            .entries
            .iter()
            .find(|entry| entry.matches_identity(&identity))
            .cloned();
        self.select_transfer_browser_entry(identity, cx);
        if let Some(entry) = entry {
            if entry.is_directory() {
                self.open_transfer_browser_entry_directory(entry, window, cx);
            } else {
                self.open_transfer_default(entry, window, cx);
            }
        }
    }

    pub(in crate::features::pages::transfers) fn handle_transfer_browser_entry_mouse_down(
        &mut self,
        path: String,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(self.transfer.browser_view().focus, cx);
        self.transfer.cancel_browser_selection_click();
        self.transfer
            .arm_browser_rename_click(&path, event.click_count == 1 && !event.modifiers.modified());

        // A plain press on a selected entry preserves the multi-file selection
        // for export. Modifier gestures retain the existing range-selection path.
        let exportable = self.session.active_file_browser_backend()
            == Some(nyaterm_files::file_browser::FileBrowserBackendKind::Local)
            || ((window.supports_virtual_file_drag()
                || window.supports_file_promise_drag()
                || cfg!(target_os = "linux"))
                && self.transfer.browser_view().entries.iter().any(|entry| {
                    entry.matches_identity(&path)
                        && matches!(
                            entry.file_type,
                            nyaterm_files::sftp::SftpFileType::File
                                | nyaterm_files::sftp::SftpFileType::Directory
                        )
                }));
        if exportable
            && !event.modifiers.modified()
            && (self.transfer.browser_view().selected_remote_path.as_deref() == Some(path.as_str())
                || self
                    .transfer
                    .browser_view()
                    .selected_remote_paths
                    .contains(&path))
        {
            self.transfer.clear_browser_drag_selection();
            self.transfer.defer_browser_selection_click(path);
            return;
        }

        let additive = event.modifiers.platform || event.modifiers.control;
        let range_anchor = event
            .modifiers
            .shift
            .then(|| self.transfer.browser_view().selected_remote_path.clone())
            .flatten()
            .or_else(|| {
                event
                    .modifiers
                    .shift
                    .then(|| {
                        self.transfer
                            .browser_view()
                            .selected_remote_paths
                            .iter()
                            .next()
                            .cloned()
                    })
                    .flatten()
            });
        let anchor_path = range_anchor.clone().unwrap_or_else(|| path.clone());
        let base_selection = if additive {
            self.transfer.browser_view().selected_remote_paths.clone()
        } else {
            HashSet::new()
        };

        if let Some(anchor) = range_anchor {
            self.apply_transfer_browser_range(
                anchor,
                path.clone(),
                base_selection.clone(),
                additive,
                cx,
            );
        } else if additive {
            self.toggle_transfer_browser_entry_marked(path.clone(), cx);
        } else {
            self.select_transfer_browser_entry(path.clone(), cx);
        }

        self.transfer
            .set_browser_drag_selection(TransferBrowserDragSelectionState {
                anchor_path,
                last_path: path,
                base_selection,
                additive,
            });
    }

    pub(in crate::features::pages::transfers) fn schedule_transfer_browser_name_rename(
        &mut self,
        path: String,
        event: &ClickEvent,
        cx: &mut Context<Self>,
    ) {
        let modifiers = event.modifiers();
        let was_armed_on_mouse_down = self.transfer.consume_browser_rename_click(&path);
        if !was_armed_on_mouse_down
            || event.click_count() != 1
            || modifiers.modified()
            || self.transfer.rename_dialog_is_open()
        {
            if event.click_count() >= 2 || modifiers.modified() {
                self.cancel_transfer_browser_pending_rename(cx);
            }
            return;
        }

        self.open_transfer_rename_for_path_and_focus(path, cx);
    }

    pub(in crate::features::pages::transfers) fn cancel_transfer_browser_pending_rename(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        if self.transfer.cancel_browser_pending_rename() {
            cx.notify();
        }
    }

    pub(in crate::features::pages::transfers) fn handle_transfer_browser_entry_mouse_move(
        &mut self,
        path: String,
        event: &MouseMoveEvent,
        cx: &mut Context<Self>,
    ) {
        if !event.dragging() {
            self.transfer.clear_browser_drag_selection();
            self.transfer.cancel_browser_selection_click();
            return;
        }

        self.transfer.cancel_browser_pending_rename();

        let Some(mut drag_selection) = self.transfer.browser_view().drag_selection.clone() else {
            return;
        };

        // A move within the pressed row is still a click, not a range gesture.
        // In particular, do not add back an entry just toggled off with Ctrl.
        if drag_selection.last_path == path {
            return;
        }
        self.transfer.cancel_browser_selection_click();
        drag_selection.last_path = path.clone();
        self.transfer
            .set_browser_drag_selection(drag_selection.clone());

        self.apply_transfer_browser_range(
            drag_selection.anchor_path,
            path,
            drag_selection.base_selection,
            drag_selection.additive,
            cx,
        );
    }

    pub(in crate::features::pages::transfers) fn finish_transfer_browser_selection_drag(
        &mut self,
        _event: &MouseUpEvent,
        cx: &mut Context<Self>,
    ) {
        if self.transfer.finish_browser_drag_selection() {
            cx.notify();
        }
    }

    pub(in crate::features::pages::transfers) fn select_transfer_browser_entry_from_context(
        &mut self,
        path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(self.transfer.browser_view().focus, cx);
        if let Some(selected_count) = self.transfer.activate_marked_browser_path(&path) {
            let display_path =
                browser_display_path_for_identity(self.transfer.browser_view().entries, &path);
            self.transfer.set_remote_path(display_path);
            self.shell
                .set_status(format!("{} remote item(s) marked", selected_count));
            cx.notify();
            return;
        }

        self.select_transfer_browser_entry(path, cx);
    }

    pub(in crate::features::pages::transfers) fn prepare_transfer_browser_entry_context_menu(
        &mut self,
        path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dismiss_transfer_rename_if_open(cx);
        self.transfer.close_browser_path_menu();
        self.transfer
            .set_browser_context_target(TransferBrowserContextTarget::Entry(path.clone()));
        self.select_transfer_browser_entry_from_context(path, window, cx);
    }

    pub(in crate::features::pages::transfers) fn prepare_transfer_browser_parent_context_menu(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dismiss_transfer_rename_if_open(cx);
        window.focus(self.transfer.browser_view().focus, cx);
        self.transfer
            .set_browser_context_target(TransferBrowserContextTarget::ParentDirectory);
        self.transfer.clear_browser_selection();
        cx.notify();
    }

    pub(in crate::features::pages::transfers) fn begin_transfer_browser_context_menu(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        self.transfer
            .set_browser_context_target(TransferBrowserContextTarget::CurrentDirectory);
        cx.notify();
    }

    pub(in crate::features::pages::transfers) fn suppress_transfer_browser_context_menu(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        self.transfer
            .set_browser_context_target(TransferBrowserContextTarget::Suppressed);
        cx.notify();
    }

    pub(in crate::features::pages::transfers) fn prepare_transfer_browser_current_context_menu(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dismiss_transfer_rename_if_open(cx);
        if !matches!(
            self.transfer.browser_view().context_target,
            TransferBrowserContextTarget::CurrentDirectory
        ) {
            return;
        }
        window.focus(self.transfer.browser_view().focus, cx);
        self.transfer.clear_browser_selection();
        cx.notify();
    }

    fn apply_transfer_browser_range(
        &mut self,
        anchor_path: String,
        target_path: String,
        base_selection: HashSet<String>,
        additive: bool,
        cx: &mut Context<Self>,
    ) {
        let entries = self.visible_transfer_browser_entries();
        let anchor_index = entries
            .iter()
            .position(|entry| entry.matches_identity(&anchor_path));
        let target_index = entries
            .iter()
            .position(|entry| entry.matches_identity(&target_path));

        let (Some(anchor_index), Some(target_index)) = (anchor_index, target_index) else {
            if additive {
                let active_path = self.transfer.browser_view().selected_remote_path.clone();
                self.transfer
                    .replace_browser_selection(base_selection, active_path);
                cx.notify();
            } else {
                self.select_transfer_browser_entry(target_path, cx);
            }
            return;
        };

        let mut next_selection = if additive {
            base_selection
        } else {
            HashSet::new()
        };
        let start = anchor_index.min(target_index);
        let end = anchor_index.max(target_index);
        for entry in &entries[start..=end] {
            next_selection.insert(entry.identity_key());
        }

        let selected_count = self
            .transfer
            .replace_browser_selection(next_selection, Some(target_path.clone()));
        let display_path = entries[target_index].path.clone();
        self.transfer.set_remote_path(display_path);
        self.shell
            .set_status(format!("{} remote item(s) marked", selected_count));
        cx.notify();
    }

    pub(in crate::features::pages::transfers) fn toggle_transfer_browser_entry_marked(
        &mut self,
        path: String,
        cx: &mut Context<Self>,
    ) {
        let display_path =
            browser_display_path_for_identity(self.transfer.browser_view().entries, &path);
        let selected_count = self.transfer.toggle_browser_path_mark(path.clone());
        self.transfer.set_remote_path(display_path);
        self.shell
            .set_status(format!("{} remote item(s) marked", selected_count));
        cx.notify();
    }

    pub(in crate::features::pages::transfers) fn select_all_visible_transfer_entries(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        let entries = self.visible_transfer_browser_entries();
        let active_path = entries.first().map(SftpFileEntry::identity_key);
        let selected_paths = entries.iter().map(SftpFileEntry::identity_key).collect();
        let selected_count = self
            .transfer
            .replace_browser_selection(selected_paths, active_path.clone());
        if let Some(entry) = entries.first() {
            self.transfer.set_remote_path(entry.path.clone());
        }
        self.shell
            .set_status(format!("{} remote item(s) marked", selected_count));
        cx.notify();
    }

    pub(in crate::features::pages::transfers) fn selected_transfer_path_part(
        &self,
        part: TransferPathPart,
    ) -> Option<String> {
        if self.settings.summary().ui_file_explorer_view_mode
            == nyaterm_core::TransferBrowserViewMode::Tree
        {
            return self
                .selected_transfer_entry()
                .map(|entry| transfer_path_part_value(&entry.path, part));
        }
        let identity = self
            .transfer
            .browser_view()
            .selected_remote_path
            .as_deref()?;
        let path = self
            .transfer
            .browser_view()
            .entries
            .iter()
            .find(|entry| entry.matches_identity(identity))
            .map(|entry| entry.path.as_str())
            .unwrap_or(identity);
        Some(transfer_path_part_value(path, part))
    }

    pub(in crate::features::pages::transfers) fn copy_selected_transfer_path(
        &mut self,
        part: TransferPathPart,
        cx: &mut Context<Self>,
    ) {
        let Some(value) = self.selected_transfer_path_part(part) else {
            self.shell
                .set_status("select a remote item first".to_string());
            self.copy_text_with_feedback("copy-file-path", String::new(), cx);
            cx.notify();
            return;
        };
        self.copy_text_with_feedback("copy-file-path", value.clone(), cx);
        self.shell
            .set_status(format!("copied remote {}", part.label()));
        self.transfer
            .set_browser_status(truncate_preview(&value, 92));
        cx.notify();
    }

    pub(in crate::features::pages::transfers) fn send_selected_transfer_path_to_terminal(
        &mut self,
        part: TransferPathPart,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(value) = self.selected_transfer_path_part(part) else {
            self.shell
                .set_status("select a remote item first".to_string());
            cx.notify();
            return;
        };
        if self.session.active_id().is_none() {
            self.shell
                .set_status("start a session before sending remote path".to_string());
            cx.notify();
            return;
        }
        if self.send_terminal_input(value.clone().into_bytes(), cx) {
            self.shell
                .set_status(format!("sent remote {} to terminal", part.label()));
            self.transfer
                .set_browser_status(truncate_preview(&value, 92));
            self.focus_terminal_input(window, cx);
        }
    }

    pub(in crate::features::pages::transfers) fn selected_transfer_entry(
        &self,
    ) -> Option<SftpFileEntry> {
        if self.settings.summary().ui_file_explorer_view_mode
            == nyaterm_core::TransferBrowserViewMode::Tree
        {
            let session = self.session.active_id()?;
            return self
                .transfer
                .selected_tree_row(session)
                .and_then(|row| row.entry)
                .filter(|entry| {
                    self.transfer
                        .selected_tree_entries(session)
                        .iter()
                        .any(|selected| selected.identity_key() == entry.identity_key())
                });
        }
        let selected = self
            .transfer
            .browser_view()
            .selected_remote_path
            .as_deref()?;
        self.transfer
            .browser_view()
            .entries
            .iter()
            .find(|entry| entry.matches_identity(selected))
            .cloned()
    }

    /// Takes `&mut self` now: it reads the memoised listing, and populating a memo
    /// is a mutation even though nothing observable changes.
    pub(in crate::features) fn selected_transfer_entries(&mut self) -> Vec<SftpFileEntry> {
        if self.settings.summary().ui_file_explorer_view_mode
            == nyaterm_core::TransferBrowserViewMode::Tree
        {
            return self
                .session
                .active_id()
                .map(|session| self.transfer.selected_tree_entries(session))
                .unwrap_or_default();
        }
        if self
            .transfer
            .browser_view()
            .selected_remote_paths
            .is_empty()
        {
            return self.selected_transfer_entry().into_iter().collect();
        }
        // The listing is shared now, so this iterates a slice and clones the few
        // entries that are selected rather than consuming a private vector.
        self.visible_transfer_browser_entries()
            .iter()
            .filter(|entry| {
                self.transfer
                    .browser_view()
                    .selected_remote_paths
                    .contains(&entry.identity_key())
            })
            .cloned()
            .collect()
    }

    pub(in crate::features::pages::transfers) fn start_selected_sftp_download_jobs(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let entries = self.selected_transfer_entries();
        if entries.is_empty() {
            self.report_browser_transfer_error(
                "download",
                &nyaterm_files::file_browser::FileTransferEntryError::NoSelection.into(),
                cx,
            );
            return;
        }
        if self.settings.summary().transfer_ask_save_location {
            let remote_paths = entries
                .into_iter()
                .map(|entry| entry.remote_path())
                .collect::<Vec<_>>();
            self.prompt_transfer_download_directory_and_start(remote_paths, window, cx);
            return;
        }
        let total = entries.len();
        let remote_paths = entries
            .into_iter()
            .map(|entry| entry.remote_path())
            .collect();
        let targets = match self.resolve_transfer_download_targets(remote_paths, None) {
            Ok(targets) => targets,
            Err(error) => {
                self.report_browser_transfer_error("download-target", &error, cx);
                return;
            }
        };
        let mut started = 0;
        let path_options = self.sftp_download_path_options();
        for (remote_path, local_path) in targets {
            if self.start_sftp_download_job_for_target(
                remote_path,
                local_path,
                path_options.clone_for_download_batch(),
                cx,
            ) {
                started += 1;
            }
        }
        if started > 0 {
            self.shell
                .set_status(format!("{started}/{total} remote download job(s) started"));
        }
        cx.notify();
    }

    pub(in crate::features::pages::transfers) fn start_selected_sftp_download_to_directory(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let remote_paths = self
            .selected_transfer_entries()
            .into_iter()
            .map(|entry| entry.remote_path())
            .collect();
        self.prompt_transfer_download_directory_and_start(remote_paths, window, cx);
    }
}

#[cfg(test)]
mod tests {

    use super::{browser_display_path_for_identity, browser_operation_target_directory};
    use nyaterm_files::file_browser::FileBrowserBackendKind;
    use nyaterm_files::sftp::{RemoteFilePath, SftpFileEntry, SftpFileType};

    fn entry(name: &str, path: &str, file_type: SftpFileType) -> SftpFileEntry {
        SftpFileEntry {
            name: name.to_string(),
            path: path.to_string(),
            file_type,
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
    fn operation_target_uses_single_selection_and_falls_back_for_multiple() {
        let directory = entry("logs", "/srv/logs", SftpFileType::Directory);
        let file = entry("app.log", "/srv/logs/app.log", SftpFileType::File);
        assert_eq!(
            browser_operation_target_directory(
                FileBrowserBackendKind::Remote,
                std::slice::from_ref(&directory),
                "/current",
                "/home/user",
                ".",
            ),
            "/srv/logs"
        );
        assert_eq!(
            browser_operation_target_directory(
                FileBrowserBackendKind::Remote,
                std::slice::from_ref(&file),
                "/current",
                "/home/user",
                ".",
            ),
            "/srv/logs"
        );
        assert_eq!(
            browser_operation_target_directory(
                FileBrowserBackendKind::Remote,
                &[directory, file],
                "/current",
                "/home/user",
                ".",
            ),
            "/current"
        );
        assert_eq!(
            browser_operation_target_directory(
                FileBrowserBackendKind::Remote,
                &[],
                "",
                "/home/user",
                ".",
            ),
            "/home/user"
        );
    }

    #[test]
    fn browser_display_path_for_identity_uses_display_path_for_raw_tokens() {
        let entry = SftpFileEntry {
            name: "client-rust.2026-09-11.log".to_string(),
            path: "./rocontrol2/client-rust.2026-09-11.log".to_string(),
            file_type: SftpFileType::File,
            size: Some(1),
            permissions: None,
            owner: String::new(),
            group: String::new(),
            modified_at: None,
            raw_path_token: RemoteFilePath::from_raw(
                "./rocontrol2/client-rust.2026-09-11.log",
                b"./rocontrol2/client-rust.2026-09-11.log",
            )
            .raw_path_token,
            symlink_target_is_directory: false,
        };
        let identity = entry.identity_key();

        assert_eq!(
            browser_display_path_for_identity(std::slice::from_ref(&entry), &identity),
            entry.path
        );
    }
}
