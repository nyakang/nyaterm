use super::{file_entry, transfer_state};
use crate::models::{TransferBrowserSortColumn, TransferBrowserSortDirection};
use crate::test_support::TestAppCleanup;
use gpui::{ScrollStrategy, TestAppContext};
use nyaterm_files::sftp::RemoteFilePath;
use std::collections::HashSet;

#[test]
fn navigation_selection_uses_visible_sorted_rows_and_the_actual_parent_offset() {
    let cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    for (path, parent_count) in [("/srv", 1), ("/", 0)] {
        for direction in [
            TransferBrowserSortDirection::Ascending,
            TransferBrowserSortDirection::Descending,
        ] {
            for with_token in [false, true] {
                let mut state = transfer_state(&cx);
                state.begin_browser_directory_load(path.into());
                state.browser.search = "item".into();
                state.browser.sort_column = TransferBrowserSortColumn::Size;
                state.browser.sort_direction = direction;
                let mut entries = (0..2000)
                    .map(|i| {
                        let mut entry =
                            file_entry(&format!("{}/item-{i:02}", path.trim_end_matches('/')));
                        entry.size = Some(i);
                        if with_token {
                            entry.raw_path_token =
                                RemoteFilePath::from_raw(&entry.path, entry.path.as_bytes())
                                    .raw_path_token;
                        }
                        entry
                    })
                    .collect::<Vec<_>>();
                let target = entries[1500].clone();
                entries.push(file_entry(&format!("{path}/.item-hidden")));
                entries.push(file_entry(&format!("{path}/another")));
                state.replace_browser_entries_for_test(entries);
                // Exercise both identity-based requests and legacy display paths.
                for requested in [target.path.clone(), target.identity_key()] {
                    assert_eq!(
                        state.restore_browser_navigation_selection(&requested, false),
                        Some(target.path.clone())
                    );
                    assert_eq!(
                        state.browser.selected_remote_path,
                        Some(target.identity_key())
                    );
                    assert_eq!(
                        state.browser.selected_remote_paths,
                        HashSet::from([target.identity_key()])
                    );
                    let scroll = state
                        .browser
                        .list_scroll
                        .0
                        .borrow()
                        .deferred_scroll_to_item
                        .unwrap();
                    let index = if direction == TransferBrowserSortDirection::Ascending {
                        1500
                    } else {
                        499
                    };
                    assert_eq!(scroll.item_index, index + parent_count);
                    assert_eq!(scroll.strategy, ScrollStrategy::Nearest);
                }
            }
        }
    }
}

#[test]
fn missing_filtered_or_hidden_navigation_targets_never_queue_a_reveal() {
    let cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    for (target, search, show_hidden) in [
        ("/srv/missing", "", true),
        ("/srv/item", "other", true),
        ("/srv/.hidden", "", false),
    ] {
        let mut state = transfer_state(&cx);
        state.begin_browser_directory_load("/srv".into());
        state.browser.search = search.into();
        state.replace_browser_entries_for_test(vec![
            file_entry("/srv/item"),
            file_entry("/srv/.hidden"),
        ]);
        state.select_browser_entry("/srv/old".into());
        assert!(
            state
                .restore_browser_navigation_selection(target, show_hidden)
                .is_none()
        );
        assert!(state.browser.selected_remote_path.is_none());
        assert!(state.browser.selected_remote_paths.is_empty());
        assert!(
            state
                .browser
                .list_scroll
                .0
                .borrow()
                .deferred_scroll_to_item
                .is_none()
        );
        assert_eq!(state.browser.search, search);
        state.browser.search.clear();
        state.visible_browser_entries(true);
        assert!(state.browser.selected_remote_path.is_none());
        assert!(
            state
                .browser
                .list_scroll
                .0
                .borrow()
                .deferred_scroll_to_item
                .is_none()
        );
    }
}

#[test]
fn raw_identity_disambiguates_entries_that_share_a_display_path() {
    let cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let mut state = transfer_state(&cx);
    state.begin_browser_directory_load("/srv".into());
    let mut first = file_entry("/srv/unknown");
    first.raw_path_token = RemoteFilePath::from_raw(&first.path, b"/srv/\xff").raw_path_token;
    let mut second = first.clone();
    second.raw_path_token = RemoteFilePath::from_raw(&second.path, b"/srv/\xfe").raw_path_token;
    state.replace_browser_entries_for_test(vec![first.clone(), second.clone()]);
    assert!(
        state
            .restore_browser_navigation_selection(&first.path, true)
            .is_none()
    );
    assert!(
        state
            .browser
            .list_scroll
            .0
            .borrow()
            .deferred_scroll_to_item
            .is_none()
    );
    assert_eq!(
        state.restore_browser_navigation_selection(&second.identity_key(), true),
        Some(second.path.clone())
    );
    assert_eq!(
        state.browser.selected_remote_path,
        Some(second.identity_key())
    );
    assert_ne!(
        state.browser.selected_remote_path,
        Some(first.identity_key())
    );
}
