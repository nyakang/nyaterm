use super::helpers::transfer_browser_search_status;
use crate::features::{NyaTermApp, text_inputs::TextInputSetup};
use crate::models::TransferBrowserSortColumn;
use gpui::{Context, KeyDownEvent, Window};
use nyaterm_files::sftp::{SftpFileEntry, SftpFileType};
use rust_i18n::t;
use std::collections::HashSet;
use std::sync::Arc;

impl NyaTermApp {
    /// The filtered, sorted listing, from the state's memo.
    ///
    /// A thin wrapper for one reason: `show_hidden` lives in settings and the memo
    /// lives in transfer state, so the two borrows have to be sequenced rather than
    /// nested. Everything else about the derivation belongs to the state layer.
    pub(in crate::features::pages::transfers) fn visible_transfer_browser_entries(
        &mut self,
    ) -> Arc<[SftpFileEntry]> {
        let show_hidden = self.settings.summary().ui_file_explorer_show_hidden_files;
        self.transfer.visible_browser_entries(show_hidden)
    }

    pub(in crate::features::pages::transfers) fn toggle_transfer_browser_sort(
        &mut self,
        column: TransferBrowserSortColumn,
        cx: &mut Context<Self>,
    ) {
        self.transfer.toggle_browser_sort(column);
        cx.notify();
    }

    pub(in crate::features) fn apply_transfer_browser_search_input(
        &mut self,
        text: String,
        cx: &mut Context<Self>,
    ) {
        self.mark_user_activity();
        self.transfer.set_browser_search(text);
        let visible = self.visible_transfer_browser_entries().len();
        let status = transfer_browser_search_status(
            self.transfer.browser_view().search.as_str(),
            visible,
            self.transfer.browser_view().entries.len(),
        );
        self.transfer.set_browser_status(status);
        cx.notify();
    }

    pub(in crate::features::pages::transfers) fn focus_transfer_browser_search(
        &mut self,
        initial_text: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(text) = initial_text {
            self.transfer.set_browser_search(text);
            self.forget_text_inputs("transfer.browser.search");
        }
        self.transfer.expand_browser_search();
        let field = self.text_input(
            "transfer.browser.search",
            &self.transfer.browser_view().search.clone(),
            TextInputSetup::placeholder(t!("fileExplorer.searchPlaceholder")),
            cx,
        );
        window.focus(&field.read(cx).focus_handle(), cx);
        field.update(cx, |field, cx| field.focus_with_cursor_at_end(window, cx));
        let visible = self.visible_transfer_browser_entries().len();
        let status = transfer_browser_search_status(
            self.transfer.browser_view().search.as_str(),
            visible,
            self.transfer.browser_view().entries.len(),
        );
        self.transfer.set_browser_status(status);
        cx.notify();
    }

    pub(in crate::features::pages::transfers) fn clear_or_close_transfer_browser_search(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transfer.browser_view().search.is_empty() {
            self.transfer.close_browser_search();
            self.forget_text_inputs("transfer.browser.search");
            self.transfer.set_browser_status("file search closed");
            window.focus(self.transfer.browser_view().focus, cx);
        } else {
            self.transfer.clear_browser_search();
            self.reset_text_input("transfer.browser.search", "", cx);
            self.transfer.set_browser_status("file search cleared");
        }
        cx.notify();
    }
}

pub(super) fn transfer_browser_search_text_for_key(event: &KeyDownEvent) -> Option<String> {
    let keystroke = &event.keystroke;
    if keystroke.modifiers.alt
        || keystroke.modifiers.control
        || keystroke.modifiers.platform
        || keystroke.modifiers.function
    {
        return None;
    }
    keystroke
        .key_char
        .as_deref()
        .filter(|text| !text.is_empty() && !text.chars().any(char::is_control))
        .map(str::to_string)
        .or_else(|| (keystroke.key == "space").then(|| " ".to_string()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct TransferBrowserFooterStats {
    pub selected_file_size: u64,
    pub selected_item_count: usize,
    pub total_file_size: u64,
    pub total_item_count: usize,
}

pub(super) fn transfer_browser_footer_stats(
    all_entries: &[SftpFileEntry],
    visible_entries: &[SftpFileEntry],
    selected_path: Option<&str>,
    selected_paths: &HashSet<String>,
    show_hidden_files: bool,
) -> TransferBrowserFooterStats {
    let totals = all_entries
        .iter()
        .filter(|entry| show_hidden_files || !entry.name.starts_with('.'));
    let mut total_item_count = 0;
    let mut total_file_size = 0u64;
    for entry in totals {
        total_item_count += 1;
        if entry.file_type != SftpFileType::Directory {
            total_file_size = total_file_size.saturating_add(entry.size.unwrap_or(0));
        }
    }

    let is_selected = |entry: &SftpFileEntry| {
        if selected_paths.is_empty() {
            selected_path == Some(entry.identity_key().as_str())
        } else {
            selected_paths.contains(&entry.identity_key())
        }
    };
    let mut selected_item_count = 0;
    let mut selected_file_size = 0u64;
    for entry in visible_entries.iter().filter(|entry| is_selected(entry)) {
        selected_item_count += 1;
        if entry.file_type != SftpFileType::Directory {
            selected_file_size = selected_file_size.saturating_add(entry.size.unwrap_or(0));
        }
    }

    TransferBrowserFooterStats {
        selected_file_size,
        selected_item_count,
        total_file_size,
        total_item_count,
    }
}

#[cfg(test)]
mod tests {
    use super::{transfer_browser_footer_stats, transfer_browser_search_text_for_key};
    use crate::features::transfers::transfer_browser_entry_is_visible;
    use gpui::{KeyDownEvent, Keystroke, Modifiers};
    use nyaterm_files::sftp::{SftpFileEntry, SftpFileType};
    use std::collections::HashSet;

    fn entry(name: &str) -> SftpFileEntry {
        sized_entry(name, SftpFileType::File, 0)
    }

    fn sized_entry(name: &str, file_type: SftpFileType, size: u64) -> SftpFileEntry {
        SftpFileEntry {
            name: name.to_string(),
            path: format!("/tmp/{name}"),
            file_type,
            size: Some(size),
            permissions: None,
            owner: String::new(),
            group: String::new(),
            modified_at: None,
            raw_path_token: None,
            symlink_target_is_directory: false,
        }
    }

    #[test]
    fn hidden_entries_follow_visibility_setting_before_search() {
        let hidden = entry(".env");

        assert!(!transfer_browser_entry_is_visible(&hidden, "", false));
        assert!(!transfer_browser_entry_is_visible(&hidden, "env", false));
        assert!(transfer_browser_entry_is_visible(&hidden, "env", true));
    }

    #[test]
    fn visible_entries_still_follow_case_insensitive_search() {
        let visible = entry("ReleaseNotes.txt");

        assert!(transfer_browser_entry_is_visible(&visible, "notes", false));
        assert!(!transfer_browser_entry_is_visible(
            &visible, "archive", true
        ));
    }

    fn key_event(key: &str, key_char: Option<&str>, modifiers: Modifiers) -> KeyDownEvent {
        KeyDownEvent {
            keystroke: Keystroke {
                modifiers,
                key: key.to_string(),
                key_char: key_char.map(str::to_string),
            },
            is_held: false,
            prefer_character_input: false,
        }
    }

    #[test]
    fn plain_text_keys_can_start_file_search() {
        assert_eq!(
            transfer_browser_search_text_for_key(&key_event("a", Some("A"), Modifiers::default())),
            Some("A".to_string())
        );
        assert_eq!(
            transfer_browser_search_text_for_key(&key_event("space", None, Modifiers::default())),
            Some(" ".to_string())
        );
    }

    #[test]
    fn shortcuts_and_control_characters_do_not_start_file_search() {
        let control = Modifiers {
            control: true,
            ..Modifiers::default()
        };
        assert_eq!(
            transfer_browser_search_text_for_key(&key_event("l", Some("l"), control)),
            None
        );
        assert_eq!(
            transfer_browser_search_text_for_key(&key_event(
                "tab",
                Some("\t"),
                Modifiers::default()
            )),
            None
        );
    }

    #[test]
    fn footer_totals_ignore_search_but_follow_hidden_setting() {
        let entries = vec![
            sized_entry("report.log", SftpFileType::File, 10),
            sized_entry("notes.txt", SftpFileType::File, 20),
            sized_entry(".secret", SftpFileType::File, 30),
        ];
        let visible = vec![entries[0].clone()];

        let hidden =
            transfer_browser_footer_stats(&entries, &visible, None, &HashSet::new(), false);
        assert_eq!(hidden.total_item_count, 2);
        assert_eq!(hidden.total_file_size, 30);

        let shown = transfer_browser_footer_stats(&entries, &visible, None, &HashSet::new(), true);
        assert_eq!(shown.total_item_count, 3);
        assert_eq!(shown.total_file_size, 60);
    }

    #[test]
    fn footer_selected_stats_only_include_visible_items() {
        let entries = vec![
            sized_entry("visible.txt", SftpFileType::File, 12),
            sized_entry("filtered.txt", SftpFileType::File, 24),
            sized_entry("folder", SftpFileType::Directory, 4096),
            sized_entry("empty", SftpFileType::File, 0),
        ];
        let visible = vec![entries[0].clone(), entries[2].clone(), entries[3].clone()];
        let selected = entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect::<HashSet<_>>();

        let stats = transfer_browser_footer_stats(&entries, &visible, None, &selected, false);

        assert_eq!(stats.selected_item_count, 3);
        assert_eq!(stats.selected_file_size, 12);
        assert_eq!(stats.total_item_count, 4);
        assert_eq!(stats.total_file_size, 36);
    }

    #[test]
    fn footer_single_selection_disappears_when_search_hides_it() {
        let selected = sized_entry("selected.txt", SftpFileType::File, 42);
        let visible = sized_entry("visible.txt", SftpFileType::File, 7);
        let entries = vec![selected.clone(), visible.clone()];

        let stats = transfer_browser_footer_stats(
            &entries,
            &[visible],
            Some(&selected.path),
            &HashSet::new(),
            false,
        );

        assert_eq!(stats.selected_item_count, 0);
        assert_eq!(stats.selected_file_size, 0);
        assert_eq!(stats.total_item_count, 2);
    }
}
