use super::browser_filter::transfer_browser_search_text_for_key;
use gpui::{Context, KeyDownEvent, Window};

use crate::features::NyaTermApp;

impl NyaTermApp {
    fn transfer_browser_file_shortcuts_available(&self) -> bool {
        let browser = self.transfer.browser_view();
        self.session
            .active_file_browser_backend()
            .is_some_and(|backend| {
                backend != nyaterm_files::file_browser::FileBrowserBackendKind::Local
            })
            && !browser.path_editing
            && !browser.search_expanded
            && !self.transfer.rename_dialog_is_open()
            && !self.transfer.path_prompt_is_open()
    }

    pub(in crate::features) fn upload_transfer_browser_files_shortcut(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        if self.transfer_browser_file_shortcuts_available() {
            self.close_transfer_browser_upload_menu(cx);
            self.prompt_transfer_browser_upload_path(
                crate::models::TransferPathPromptKind::UploadFile,
                cx,
            );
        }
    }

    pub(in crate::features) fn download_transfer_browser_files_shortcut(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transfer_browser_file_shortcuts_available()
            && !self.selected_transfer_entries().is_empty()
        {
            self.start_selected_sftp_download_jobs(window, cx);
        }
    }

    pub(in crate::features::pages::transfers) fn handle_transfer_browser_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.mark_user_activity();
        if event.keystroke.key == "escape" && self.close_open_transfer_browser_menu(cx) {
            cx.stop_propagation();
            return;
        }
        // The path field owns all editing keys, including clipboard shortcuts.
        if self.transfer.browser_view().path_editing || self.transfer.browser_view().search_expanded
        {
            return;
        }
        let keystroke = &event.keystroke;
        let modified_for_location = (keystroke.modifiers.platform || keystroke.modifiers.control)
            && !keystroke.modifiers.alt
            && !keystroke.modifiers.shift;
        if modified_for_location && keystroke.key.eq_ignore_ascii_case("l") {
            cx.stop_propagation();
            self.begin_transfer_browser_path_edit(window, cx);
            return;
        }

        if self.settings.summary().ui_file_explorer_view_mode
            == nyaterm_core::TransferBrowserViewMode::Tree
        {
            self.handle_transfer_tree_key_down(event, window, cx);
            return;
        }

        if modified_for_location {
            match keystroke.key.to_ascii_lowercase().as_str() {
                "c" => self.capture_transfer_file_clipboard(false, cx),
                "x" => self.capture_transfer_file_clipboard(true, cx),
                "v" => self.paste_transfer_file_clipboard(window, cx),
                _ => {}
            }
            if matches!(keystroke.key.to_ascii_lowercase().as_str(), "c" | "x" | "v") {
                cx.stop_propagation();
                return;
            }
        }

        // Once the search field owns focus, editing keys must not fall through to
        // the browser shortcuts below. In particular, printable keys would
        // otherwise recreate the field from a one-character seed on every press.
        if self.transfer.browser_view().search_expanded {
            return;
        }

        if !self.transfer.rename_dialog_is_open()
            && let Some(text) = transfer_browser_search_text_for_key(event)
        {
            cx.stop_propagation();
            self.focus_transfer_browser_search(Some(text), window, cx);
            return;
        }

        let modified_for_select_all = (keystroke.modifiers.platform || keystroke.modifiers.control)
            && !keystroke.modifiers.alt
            && !keystroke.modifiers.shift;

        if modified_for_select_all && keystroke.key.eq_ignore_ascii_case("a") {
            cx.stop_propagation();
            self.select_all_visible_transfer_entries(cx);
            return;
        }

        let unmodified = !keystroke.modifiers.alt
            && !keystroke.modifiers.control
            && !keystroke.modifiers.platform
            && !keystroke.modifiers.shift;

        if unmodified && keystroke.key.eq_ignore_ascii_case("enter") {
            cx.stop_propagation();
            if let Some(entry) = self.selected_transfer_entry() {
                if entry.is_directory() {
                    self.open_transfer_browser_entry_directory(entry, window, cx);
                } else {
                    self.open_transfer_default(entry, window, cx);
                }
            } else {
                self.shell
                    .set_status("select a remote item before opening".to_string());
                cx.notify();
            }
            return;
        }

        if unmodified && keystroke.key.eq_ignore_ascii_case("backspace") {
            cx.stop_propagation();
            self.open_transfer_parent_directory(window, cx);
            return;
        }

        if unmodified && keystroke.key.eq_ignore_ascii_case("f5") {
            cx.stop_propagation();
            self.refresh_transfer_browser(window, cx);
            return;
        }

        if keystroke.key == "delete" && unmodified && !self.selected_transfer_entries().is_empty() {
            cx.stop_propagation();
            self.open_selected_transfer_delete_dialog(window, cx);
        }
    }

    /// Close whichever browser popup menu is open; false when none was.
    fn close_open_transfer_browser_menu(&mut self, cx: &mut Context<Self>) -> bool {
        let view = self.transfer.browser_view();
        let (upload, favorites, path) = (
            view.upload_menu.is_some(),
            view.favorites_menu.is_some(),
            view.path_menu.is_some(),
        );
        if upload {
            self.close_transfer_browser_upload_menu(cx);
        } else if favorites {
            self.close_transfer_browser_favorites_menu(cx);
        } else if path {
            self.close_transfer_browser_path_menu(cx);
        } else if self.transfer.transfer_job_menu().is_some() {
            self.close_transfer_job_menu(cx);
        } else {
            return false;
        }
        true
    }
}
