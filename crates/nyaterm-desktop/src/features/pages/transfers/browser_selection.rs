use gpui::{
    ClickEvent, ClipboardItem, Context, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Window,
};
use nyaterm_core::truncate_preview;
use nyaterm_transport::SftpFileEntry;

use std::collections::HashSet;

use crate::features::NyaTermApp;
use crate::models::{TransferBrowserContextTarget, TransferBrowserDragSelectionState};

use super::{TransferPathPart, remote_file_name, transfer_path_part_value};

/// Converts a selection identity key to its display path so raw-path-token is not
/// treated as a user-facing path.
fn browser_display_path_for_identity(entries: &[SftpFileEntry], identity: &str) -> String {
    entries
        .iter()
        .find(|entry| entry.matches_identity(identity))
        .map(|entry| entry.path.clone())
        .unwrap_or_else(|| identity.to_string())
}

impl NyaTermApp {
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
        self.transfer
            .arm_browser_rename_click(&path, event.click_count == 1 && !event.modifiers.modified());

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
            return;
        }

        self.transfer.cancel_browser_pending_rename();

        let Some(drag_selection) = self.transfer.browser_view().drag_selection.clone() else {
            return;
        };

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
            cx.notify();
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(value.clone()));
        self.shell
            .set_status(format!("copied remote {}", part.label()));
        self.transfer
            .set_browser_status(truncate_preview(&value, 92));
        cx.notify();
    }

    pub(in crate::features::pages::transfers) fn send_selected_transfer_path_to_terminal(
        &mut self,
        part: TransferPathPart,
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
            cx.notify();
        }
    }

    pub(in crate::features::pages::transfers) fn selected_transfer_entry(
        &self,
    ) -> Option<SftpFileEntry> {
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
            self.shell
                .set_status("mark remote items before downloading".to_string());
            cx.notify();
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
        let base_local_path = if total == 1 {
            self.normalized_transfer_local_path(&entries[0].path)
        } else {
            self.resolved_transfer_download_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("."))
        };
        for entry in entries {
            let local_path = if total == 1 {
                base_local_path.clone()
            } else {
                base_local_path.join(remote_file_name(&entry.path))
            };
            self.start_sftp_download_job_for_target(entry.remote_path(), local_path, window, cx);
        }
        self.shell
            .set_status(format!("{total} remote download job(s) started"));
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use nyaterm_transport::{RemoteFilePath, SftpFileEntry, SftpFileType};

    use super::browser_display_path_for_identity;

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
