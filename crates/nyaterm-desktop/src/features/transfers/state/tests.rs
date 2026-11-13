use super::{
    TransferEditorCloseAfterSave, TransferEditorCloseOutcome, TransferEditorSaveOutcome,
    TransferFeatureFocus, TransferFeatureState, TransferPanelState, TransferPathState,
    TransferQueueState,
};
use crate::models::{
    TransferBrowserContextTarget, TransferBrowserNavigationSnapshot,
    TransferBrowserSessionCacheState, TransferBrowserSortColumn, TransferEditorField,
    TransferEditorState, TransferExternalSyncPromptState, TransferJobEvent, TransferJobKind,
    TransferJobResult, TransferJobState, TransferJobStatus, TransferNewFolderState,
    TransferPathPromptKind, TransferPropertiesState, TransferRenameState,
};
use futures::channel::mpsc::unbounded;
use gpui::{ScrollHandle, ScrollStrategy, TestAppContext, UniformListScrollHandle, point, px};
use nyaterm_files::remote_file::{
    RemoteTextDocument, RemoteTextGeneration, RemoteTextMetadata, RemoteTextRevision,
    RemoteTextWriteResult,
};
use nyaterm_files::sftp::{
    SftpFileEntry, SftpFileProperties, SftpFileType, SftpTransferControl, SftpWriteTextResult,
};
use nyaterm_files::sftp_transfer_types::{
    SftpDuplicatePolicy, SftpPathTransferOptions, SftpTransferOptions,
};
use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;

mod browser_navigation;
mod delete_jobs;

fn transfer_focus(cx: &TestAppContext) -> TransferFeatureFocus {
    cx.update(|cx| TransferFeatureFocus {
        tree: cx.focus_handle(),
        queue: cx.focus_handle(),
        browser: cx.focus_handle(),
        editor: cx.focus_handle(),
        preview: cx.focus_handle(),
        external_sync: cx.focus_handle(),
    })
}

fn transfer_state(cx: &TestAppContext) -> TransferFeatureState {
    TransferFeatureState::new(
        ".".to_string(),
        String::new(),
        SftpDuplicatePolicy::Ask,
        180.,
        transfer_focus(cx),
        cx.background_executor.clone(),
    )
}

#[test]
fn dropping_transfer_state_cancels_jobs_and_defers_joining_to_its_cleanup_owner() {
    use std::sync::mpsc;
    use std::time::Duration;

    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = transfer_state(&cx);
    let cleanup = state.worker_cleanup.clone();
    let job = transfer_job("held", "session", TransferJobStatus::Paused, true);
    let control = job.control.as_ref().unwrap().clone();
    control.pause();
    state.enqueue_transfer_job(job);
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (finished_tx, finished_rx) = mpsc::channel();
    state
        .bulk_executor()
        .unwrap()
        .submit_detached("held", move |_| {
            started_tx.send(()).unwrap();
            let released = release_rx.recv_timeout(Duration::from_secs(5)).is_ok();
            finished_tx.send(released).unwrap();
        })
        .unwrap();
    started_rx.recv_timeout(Duration::from_secs(5)).unwrap();

    drop(state);
    assert!(control.is_cancelled());
    assert!(!control.is_paused());
    release_tx.send(()).unwrap();
    cx.foreground_executor().block_test(cleanup.drain());
    assert!(
        finished_rx.try_recv().unwrap(),
        "drop blocked on the worker"
    );
}

#[test]
fn bulk_executor_is_lazy_and_shutdown_wakes_paused_transfers() {
    use std::sync::mpsc;
    use std::time::Duration;
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = transfer_state(&cx);
    assert!(state.bulk_executor.is_none());
    let job = transfer_job("paused", "session", TransferJobStatus::Paused, true);
    let control = job.control.as_ref().unwrap().clone();
    control.pause();
    state.enqueue_transfer_job(job);
    let (started_tx, started_rx) = mpsc::channel();
    let (finished_tx, finished_rx) = mpsc::channel();
    let worker_control = control.clone();
    state
        .bulk_executor()
        .unwrap()
        .submit_detached("paused", move |_| {
            started_tx.send(()).unwrap();
            while worker_control.is_paused() && !worker_control.is_cancelled() {
                std::thread::sleep(Duration::from_millis(5));
            }
            finished_tx.send(()).unwrap();
        })
        .unwrap();
    started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let external_upload = state.external_editor_upload_control();
    let overlapping_upload = state.external_editor_upload_control();
    external_upload.pause();
    let cleanup = state.begin_bulk_shutdown();
    assert!(control.is_cancelled());
    assert!(!control.is_paused());
    assert!(external_upload.is_cancelled());
    assert!(overlapping_upload.is_cancelled());
    let reaper = crate::thread_owner::spawn_joinable("transfer-cleanup-test", cleanup).unwrap();
    finished_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    reaper.join().unwrap();
    assert!(
        state
            .bulk_executor()
            .unwrap()
            .submit_detached("late", |_| {})
            .is_err()
    );
}

#[test]
fn inactive_delete_invalidates_cached_directory_until_its_session_is_restored() {
    use nyaterm_files::file_browser::FileBrowserBackendKind;
    use nyaterm_files::sftp::RemoteFilePath;

    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut transfer = transfer_state(&cx);
    transfer.store_browser_session_cache(
        "inactive".into(),
        TransferBrowserSessionCacheState {
            entries: Arc::new(vec![file_entry("/dir/sub/file")]),
            current_path: "/dir/sub".into(),
            current_raw_path_token: None,
            home_dir: "/home/user".into(),
            history: VecDeque::from(["/dir/sub".into()]),
            history_index: 0,
            visited_history: VecDeque::new(),
        },
    );
    transfer.begin_delete_batch(
        "batch".into(),
        FileBrowserBackendKind::Remote,
        vec![("delete".into(), RemoteFilePath::new("/dir/sub"))],
    );
    let outcome = transfer
        .settle_delete_job("batch", "delete", Ok(()))
        .unwrap();
    transfer.invalidate_delete_batch("inactive", &outcome);
    assert_eq!(transfer.browser.path, ".");
    assert_eq!(
        transfer
            .browser_session_cache("inactive")
            .unwrap()
            .current_path,
        "/dir"
    );
    assert!(transfer.replace_session_id("inactive", "reconnected"));
    assert!(transfer.take_delete_refresh_pending("reconnected"));
    assert!(!transfer.take_delete_refresh_pending("reconnected"));
    assert!(
        transfer
            .settle_delete_job("batch", "delete", Ok(()))
            .is_none()
    );
}

/// Every real input to the derived listing must invalidate the memo, and nothing
/// else may.
///
/// The listing is keyed on the entry `Arc`'s address plus four small values. This
/// walks each one in turn and pins that a repeat read costs nothing while a genuine
/// change costs exactly one recompute.
#[test]
fn each_browser_filter_input_invalidates_the_memo_and_nothing_else_does() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = transfer_state(&cx);
    state.replace_browser_entries_for_test(vec![
        file_entry("/srv/alpha.txt"),
        file_entry("/srv/.hidden"),
        file_entry("/srv/beta.txt"),
    ]);

    let first = state.visible_browser_entries(false);
    assert_eq!(state.browser_filter_recomputes(), 1);
    // Hidden files are filtered out, so the memo is doing real work.
    assert_eq!(first.len(), 2);

    // A repeat read with identical inputs must not recompute.
    let again = state.visible_browser_entries(false);
    assert_eq!(
        state.browser_filter_recomputes(),
        1,
        "a repeat read recomputed"
    );
    assert!(
        Arc::ptr_eq(&first, &again),
        "a memo hit must hand back the same allocation"
    );

    // 1. show_hidden
    let with_hidden = state.visible_browser_entries(true);
    assert_eq!(state.browser_filter_recomputes(), 2);
    assert_eq!(with_hidden.len(), 3);

    // 2. search
    state.set_browser_search("beta".to_string());
    assert_eq!(state.visible_browser_entries(true).len(), 1);
    assert_eq!(state.browser_filter_recomputes(), 3);
    state.set_browser_search(String::new());
    state.visible_browser_entries(true);
    assert_eq!(state.browser_filter_recomputes(), 4);

    // 3. sort column, and 4. direction -- toggling the same column flips direction,
    // so this covers both key fields.
    state.toggle_browser_sort(TransferBrowserSortColumn::Size);
    state.visible_browser_entries(true);
    assert_eq!(state.browser_filter_recomputes(), 5);
    state.toggle_browser_sort(TransferBrowserSortColumn::Size);
    state.visible_browser_entries(true);
    assert_eq!(
        state.browser_filter_recomputes(),
        6,
        "direction is part of the key"
    );

    // 5. the entry list itself, replaced whole
    state.replace_browser_entries_for_test(vec![file_entry("/srv/gamma.txt")]);
    assert_eq!(state.visible_browser_entries(true).len(), 1);
    assert_eq!(state.browser_filter_recomputes(), 7);

    // And still nothing after all of that if nothing moves.
    state.visible_browser_entries(true);
    assert_eq!(state.browser_filter_recomputes(), 7);
}

fn file_entry(path: &str) -> SftpFileEntry {
    SftpFileEntry {
        name: path.rsplit('/').next().unwrap_or(path).to_string(),
        path: path.to_string(),
        file_type: SftpFileType::File,
        size: Some(12),
        permissions: Some(0o640),
        owner: "owner".to_string(),
        group: "group".to_string(),
        modified_at: Some(1),
        raw_path_token: None,
        symlink_target_is_directory: false,
    }
}

#[test]
fn cut_clipboard_only_settles_successful_jobs_from_the_current_generation() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut transfer = transfer_state(&cx);
    transfer.set_file_clipboard(
        "session-a".into(),
        vec![file_entry("/old")],
        true,
        Vec::new(),
    );
    let old_generation = transfer.file_clipboard().unwrap().generation;
    transfer.track_cut_job("old".into(), old_generation, "/old".into());
    transfer.set_file_clipboard(
        "session-b".into(),
        vec![file_entry("/new")],
        true,
        Vec::new(),
    );
    transfer.settle_cut_job("old", true);
    assert_eq!(transfer.file_clipboard().unwrap().entries[0].path, "/new");

    let generation = transfer.file_clipboard().unwrap().generation;
    transfer.track_cut_job("failed".into(), generation, "/new".into());
    transfer.settle_cut_job("failed", false);
    assert!(transfer.file_clipboard().is_some());
    transfer.track_cut_job("succeeded".into(), generation, "/new".into());
    transfer.settle_cut_job("succeeded", true);
    assert!(transfer.file_clipboard().is_none());

    transfer.set_file_clipboard(
        "session-a".into(),
        vec![file_entry("/stale")],
        false,
        Vec::new(),
    );
    transfer.clear_file_clipboard_for_session("session-b");
    assert!(transfer.file_clipboard().is_some());
    transfer.clear_file_clipboard_for_session("session-a");
    assert!(transfer.file_clipboard().is_none());
}

#[test]
fn browser_ctrl_toggle_keeps_active_selection_consistent() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut transfer = transfer_state(&cx);
    transfer.replace_browser_entries_for_test(vec![
        file_entry("/a"),
        file_entry("/b"),
        file_entry("/c"),
    ]);
    transfer.select_browser_entry("/a".into());
    transfer.toggle_browser_path_mark("/b".into());
    transfer.toggle_browser_path_mark("/c".into());

    assert_eq!(transfer.toggle_browser_path_mark("/a".into()), 2);
    assert_eq!(transfer.browser.selected_remote_path.as_deref(), Some("/c"));
    assert!(!transfer.browser.selected_remote_paths.contains("/a"));
    assert_eq!(transfer.toggle_browser_path_mark("/a".into()), 3);
    assert_eq!(transfer.browser.selected_remote_path.as_deref(), Some("/a"));
    assert_eq!(transfer.toggle_browser_path_mark("/a".into()), 2);
    assert_eq!(transfer.browser.selected_remote_path.as_deref(), Some("/b"));
    transfer.toggle_browser_path_mark("/b".into());
    assert_eq!(transfer.browser.selected_remote_path.as_deref(), Some("/c"));
    assert_eq!(transfer.toggle_browser_path_mark("/c".into()), 0);
    assert!(transfer.browser.selected_remote_path.is_none());
}

#[test]
fn browser_deferred_selection_is_cancelled_by_navigation_and_context_menu() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut transfer = transfer_state(&cx);
    transfer.defer_browser_selection_click("/a".into());
    assert!(!transfer.consume_browser_selection_click("/b"));
    assert!(transfer.consume_browser_selection_click("/a"));
    assert!(!transfer.consume_browser_selection_click("/a"));
    transfer.defer_browser_selection_click("/a".into());
    transfer.begin_browser_directory_load("/next".into());
    assert!(!transfer.consume_browser_selection_click("/a"));
    transfer.defer_browser_selection_click("/a".into());
    transfer.set_browser_context_target(TransferBrowserContextTarget::CurrentDirectory);
    assert!(!transfer.consume_browser_selection_click("/a"));
}

#[test]
fn browser_rename_click_requires_selection_before_mouse_down() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut transfer = transfer_state(&cx);

    assert!(!transfer.arm_browser_rename_click("/first.txt", true));
    transfer.select_browser_entry("/first.txt".to_string());
    assert!(!transfer.consume_browser_rename_click("/first.txt"));

    assert!(transfer.arm_browser_rename_click("/first.txt", true));
    let browser = transfer.browser_view();
    assert_eq!(browser.selected_remote_path.as_deref(), Some("/first.txt"));
    assert!(transfer.consume_browser_rename_click("/first.txt"));

    assert!(!transfer.arm_browser_rename_click("/first.txt", false));
    assert!(!transfer.consume_browser_rename_click("/first.txt"));

    transfer.replace_browser_selection(
        HashSet::from(["/first.txt".to_string(), "/second.txt".to_string()]),
        Some("/first.txt".to_string()),
    );
    assert!(!transfer.arm_browser_rename_click("/first.txt", true));
    assert!(!transfer.consume_browser_rename_click("/first.txt"));
}

#[test]
fn browser_context_target_cancels_armed_rename_click() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut transfer = transfer_state(&cx);
    transfer.select_browser_entry("/first.txt".to_string());
    assert!(transfer.arm_browser_rename_click("/first.txt", true));

    transfer.set_browser_context_target(TransferBrowserContextTarget::Entry(
        "/first.txt".to_string(),
    ));

    assert_eq!(
        transfer.browser_view().context_target,
        &TransferBrowserContextTarget::Entry("/first.txt".to_string())
    );
    assert!(!transfer.consume_browser_rename_click("/first.txt"));
}

#[test]
fn browser_external_drop_hover_tracks_overlay_visibility() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut transfer = transfer_state(&cx);

    assert!(!transfer.browser_view().external_drop_hover);
    assert!(transfer.set_browser_external_drop_hover(true));
    assert!(transfer.browser_external_drop_hover_is_pending());
    assert!(transfer.browser_view().external_drop_hover);
    assert!(!transfer.set_browser_external_drop_hover(true));
    assert!(transfer.set_browser_external_drop_hover(false));
    assert!(!transfer.browser_external_drop_hover_is_pending());
    assert!(!transfer.browser_view().external_drop_hover);
}

#[test]
fn browser_entry_context_target_preserves_an_existing_multi_selection() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut transfer = transfer_state(&cx);
    let selected = HashSet::from(["/first.txt".to_string(), "/second.txt".to_string()]);
    transfer.replace_browser_selection(selected.clone(), Some("/second.txt".to_string()));

    transfer.set_browser_context_target(TransferBrowserContextTarget::Entry(
        "/first.txt".to_string(),
    ));

    assert_eq!(transfer.activate_marked_browser_path("/first.txt"), Some(2));
    assert_eq!(transfer.browser.selected_remote_paths, selected);
    assert_eq!(
        transfer.browser.selected_remote_path.as_deref(),
        Some("/first.txt")
    );
}

#[test]
fn browser_entry_context_on_unmarked_path_collapses_to_a_single_selection() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut transfer = transfer_state(&cx);
    let selected = HashSet::from(["/first.txt".to_string(), "/second.txt".to_string()]);
    transfer.replace_browser_selection(selected, Some("/second.txt".to_string()));

    // A right-click on a path that is NOT part of the marked set is not a marked
    // activation, so the caller falls back to a single-entry selection.
    assert_eq!(transfer.activate_marked_browser_path("/third.txt"), None);
    transfer.select_browser_entry("/third.txt".to_string());

    assert_eq!(
        transfer.browser.selected_remote_paths,
        HashSet::from(["/third.txt".to_string()])
    );
    assert_eq!(
        transfer.browser.selected_remote_path.as_deref(),
        Some("/third.txt")
    );
}

#[test]
fn send_to_refreshes_only_a_matching_target_session_cache() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut transfer = transfer_state(&cx);
    transfer.store_browser_session_cache(
        "target".to_string(),
        TransferBrowserSessionCacheState {
            entries: Arc::new(vec![file_entry("/srv/existing.txt")]),
            current_path: "/srv".to_string(),
            current_raw_path_token: None,
            home_dir: "/home/target".to_string(),
            history: VecDeque::from(["/srv".to_string()]),
            history_index: 0,
            visited_history: VecDeque::from(["/srv".to_string()]),
        },
    );

    // A refresh for a directory the cache is not showing leaves it untouched.
    assert!(!transfer.refresh_browser_session_cache_listing(
        "target",
        "/other",
        vec![file_entry("/other/x.txt")],
    ));
    assert_eq!(
        transfer
            .browser_session_cache("target")
            .unwrap()
            .entries
            .len(),
        1
    );

    // A refresh for the cached directory (trailing-slash insensitive) replaces it.
    assert!(transfer.refresh_browser_session_cache_listing(
        "target",
        "/srv/",
        vec![file_entry("/srv/existing.txt"), file_entry("/srv/sent.txt")],
    ));
    assert_eq!(
        transfer
            .browser_session_cache("target")
            .unwrap()
            .entries
            .len(),
        2
    );

    // An unknown session is never created by a refresh.
    assert!(!transfer.refresh_browser_session_cache_listing(
        "missing",
        "/srv",
        vec![file_entry("/srv/x.txt")],
    ));
    assert!(transfer.browser_session_cache("missing").is_none());
}

#[test]
fn browser_navigation_clears_the_rename_click_candidate() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut transfer = transfer_state(&cx);
    transfer.select_browser_entry("/first.txt".to_string());
    assert!(transfer.arm_browser_rename_click("/first.txt", true));

    transfer.begin_browser_directory_load("/next".to_string());

    assert!(!transfer.consume_browser_rename_click("/first.txt"));
    assert_eq!(
        transfer.browser.context_target,
        TransferBrowserContextTarget::CurrentDirectory
    );
}

fn file_properties(path: &str) -> SftpFileProperties {
    SftpFileProperties {
        symlink_target: None,
        name: path.rsplit('/').next().unwrap_or(path).to_string(),
        path: path.to_string(),
        file_type: SftpFileType::File,
        size: Some(12),
        permissions: Some(0o600),
        permissions_symbolic: "rw-------".to_string(),
        owner: "updated-owner".to_string(),
        group: "updated-group".to_string(),
        uid: Some(1000),
        gid: Some(1000),
        modified_at: Some(2),
        accessed_at: Some(3),
        raw_path_token: None,
        symlink_target_is_directory: false,
    }
}

#[test]
fn browser_history_discards_the_forward_branch_and_tracks_visits() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut transfer = transfer_state(&cx);
    transfer.browser.history =
        VecDeque::from(["/three".to_string(), "/two".to_string(), "/one".to_string()]);
    transfer.browser.history_index = 1;

    transfer.record_browser_history("/four".to_string());

    assert_eq!(
        transfer.browser.history,
        VecDeque::from(["/four".to_string(), "/two".to_string(), "/one".to_string(),])
    );
    assert_eq!(transfer.browser.history_index, 0);
    assert_eq!(
        transfer.browser.visited_history.front().map(String::as_str),
        Some("/four")
    );
}

#[test]
fn browser_session_restore_clamps_history_and_clears_interaction() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut transfer = transfer_state(&cx);
    transfer.select_browser_entry("/stale.txt".to_string());
    assert!(
        transfer
            .schedule_browser_pending_rename("/stale.txt")
            .is_some()
    );
    transfer.store_browser_session_cache(
        "session-a".to_string(),
        TransferBrowserSessionCacheState {
            entries: Arc::new(vec![file_entry("/srv/current.txt")]),
            current_path: "/srv".to_string(),
            current_raw_path_token: None,
            home_dir: "/home/test".to_string(),
            history: VecDeque::from(["/srv".to_string()]),
            history_index: 99,
            visited_history: VecDeque::from(["/srv".to_string()]),
        },
    );
    transfer
        .browser
        .horizontal_scroll
        .set_offset(point(px(-24.), px(0.)));

    assert_eq!(
        transfer.restore_browser_session_cache("session-a"),
        Some("/srv".to_string())
    );
    assert!(transfer.browser.pending_rename.is_none());
    let browser = transfer.browser_view();
    assert_eq!(browser.path.as_str(), "/srv");
    assert_eq!(browser.history_index, 0);
    assert!(browser.selected_remote_paths.is_empty());
    assert_eq!(browser.entries.len(), 1);
    assert_eq!(browser.horizontal_scroll.offset().x, px(0.));
}

#[test]
fn browser_session_restore_preserves_the_raw_directory_token() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut transfer = transfer_state(&cx);
    let remote =
        nyaterm_files::sftp::RemoteFilePath::from_raw("/srv/non-utf8-?", b"/srv/non-utf8-\xff");
    transfer.store_browser_session_cache(
        "session-a".to_string(),
        TransferBrowserSessionCacheState {
            entries: Arc::new(Vec::new()),
            current_path: remote.display_path.clone(),
            current_raw_path_token: remote.raw_path_token.clone(),
            home_dir: "/home/test".to_string(),
            history: VecDeque::from([remote.display_path.clone()]),
            history_index: 0,
            visited_history: VecDeque::new(),
        },
    );

    transfer.restore_browser_session_cache("session-a").unwrap();

    assert_eq!(transfer.browser_remote_file_path(), remote);
}

#[test]
fn transfer_moves_sftp_cache_and_invalidates_source_navigation() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut source = transfer_state(&cx);
    let mut target = transfer_state(&cx);
    source.store_browser_session_cache(
        "moved".to_string(),
        TransferBrowserSessionCacheState {
            entries: Arc::new(vec![file_entry("/srv/file.txt")]),
            current_path: "/srv".to_string(),
            current_raw_path_token: Some("raw-path".to_string()),
            home_dir: "/home".to_string(),
            history: VecDeque::from(["/srv".to_string()]),
            history_index: 0,
            visited_history: VecDeque::new(),
        },
    );
    source
        .browser
        .navigation_jobs
        .insert("moved".to_string(), "old-job".to_string());
    let pending = source.prepare_browser_navigation("other", "/srv".to_string());
    source
        .browser
        .pending_navigations
        .insert("old-job".to_string(), pending);

    let bundle = source.detach_sessions_for_transfer(&["moved".to_string()]);
    assert!(!source.retains_transfer_session("moved"));
    assert!(!source.browser.pending_navigations.contains_key("old-job"));
    target.attach_sessions_from_transfer(bundle);
    let cache = target.browser_session_cache("moved").expect("cache moved");
    assert_eq!(cache.current_path, "/srv");
    assert_eq!(cache.current_raw_path_token.as_deref(), Some("raw-path"));
    assert_eq!(cache.entries.len(), 1);
}

#[test]
fn browser_navigation_restores_the_stable_pending_snapshot() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut transfer = transfer_state(&cx);
    transfer.browser.path = "/optimistic".to_string();
    transfer
        .browser
        .navigation_jobs
        .insert("session-a".to_string(), "list-1".to_string());
    let list_scroll = UniformListScrollHandle::new();
    list_scroll.scroll_to_item_strict(3, ScrollStrategy::Top);
    let horizontal_scroll = ScrollHandle::new();
    horizontal_scroll.set_offset(point(px(-24.), px(0.)));
    let stable = TransferBrowserNavigationSnapshot {
        remote_path: "/stable".to_string(),
        browser_path: "/stable".to_string(),
        browser_raw_path_token: None,
        entries: Arc::new(vec![file_entry("/stable/file.txt")]),
        loading: false,
        error: None,
        status: "stable".to_string(),
        history: VecDeque::from(["/stable".to_string()]),
        history_index: 0,
        visited_history: VecDeque::from(["/stable".to_string()]),
        selected_path: None,
        selected_paths: Default::default(),
        list_scroll,
        horizontal_scroll,
    };
    transfer
        .browser
        .pending_navigations
        .insert("list-1".to_string(), stable.clone());

    let rollback = transfer.prepare_browser_navigation("session-a", "/optimistic".to_string());

    assert_eq!(rollback.browser_path, "/stable");
    assert_eq!(transfer.browser.path, "/stable");
    assert_eq!(transfer.browser.list_scroll.logical_scroll_top_index(), 3);
    assert_eq!(transfer.browser.horizontal_scroll.offset().x, px(-24.));
    assert!(!transfer.browser.navigation_jobs.contains_key("session-a"));
    assert!(!transfer.browser.pending_navigations.contains_key("list-1"));
}

#[test]
fn browser_navigation_and_filters_reset_horizontal_scroll() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut transfer = transfer_state(&cx);

    transfer
        .browser
        .horizontal_scroll
        .set_offset(point(px(-24.), px(0.)));
    transfer.set_browser_search("term".to_string());
    assert_eq!(transfer.browser.horizontal_scroll.offset().x, px(0.));

    transfer
        .browser
        .horizontal_scroll
        .set_offset(point(px(-24.), px(0.)));
    transfer.toggle_browser_sort(crate::models::TransferBrowserSortColumn::Modified);
    assert_eq!(transfer.browser.horizontal_scroll.offset().x, px(0.));

    transfer
        .browser
        .horizontal_scroll
        .set_offset(point(px(-24.), px(0.)));
    transfer.begin_browser_directory_load("/next".to_string());
    assert_eq!(transfer.browser.horizontal_scroll.offset().x, px(0.));

    transfer
        .browser
        .horizontal_scroll
        .set_offset(point(px(-24.), px(0.)));
    transfer.reset_browser_for_session(true, None);
    assert_eq!(transfer.browser.horizontal_scroll.offset().x, px(0.));
}

#[test]
fn transfer_session_id_migration_preserves_reconnected_sftp_state() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut transfer = transfer_state(&cx);
    let cache = TransferBrowserSessionCacheState {
        entries: Arc::new(vec![file_entry("/srv/app.txt")]),
        current_path: "/srv".to_string(),
        current_raw_path_token: None,
        home_dir: "/home/nya".to_string(),
        history: VecDeque::from(["/srv".to_string()]),
        history_index: 0,
        visited_history: VecDeque::from(["/srv".to_string()]),
    };
    transfer.store_browser_session_cache("old-session".to_string(), cache);
    transfer.store_browser_session_cache(
        "new-session".to_string(),
        TransferBrowserSessionCacheState {
            entries: Arc::new(vec![file_entry("/stale/file.txt")]),
            current_path: "/stale".to_string(),
            current_raw_path_token: None,
            home_dir: "/stale".to_string(),
            history: VecDeque::from(["/stale".to_string()]),
            history_index: 0,
            visited_history: VecDeque::from(["/stale".to_string()]),
        },
    );
    transfer
        .browser
        .navigation_jobs
        .insert("old-session".to_string(), "sftp-list-1".to_string());
    transfer.browser.pending_navigations.insert(
        "sftp-list-1".to_string(),
        TransferBrowserNavigationSnapshot {
            remote_path: "/srv".to_string(),
            browser_path: "/srv".to_string(),
            browser_raw_path_token: None,
            entries: Arc::new(vec![file_entry("/srv/old.txt")]),
            loading: true,
            error: None,
            status: "listing".to_string(),
            history: VecDeque::from(["/srv".to_string()]),
            history_index: 0,
            visited_history: VecDeque::new(),
            selected_path: None,
            selected_paths: HashSet::new(),
            list_scroll: UniformListScrollHandle::new(),
            horizontal_scroll: ScrollHandle::new(),
        },
    );
    transfer.browser.pending_navigations.insert(
        "orphan-list".to_string(),
        TransferBrowserNavigationSnapshot {
            remote_path: "/tmp".to_string(),
            browser_path: "/tmp".to_string(),
            browser_raw_path_token: None,
            entries: Arc::new(Vec::new()),
            loading: false,
            error: None,
            status: "orphan".to_string(),
            history: VecDeque::new(),
            history_index: 0,
            visited_history: VecDeque::new(),
            selected_path: None,
            selected_paths: HashSet::new(),
            list_scroll: UniformListScrollHandle::new(),
            horizontal_scroll: ScrollHandle::new(),
        },
    );
    transfer.enqueue_transfer_job(TransferJobState {
        id: "download".to_string(),
        session_id: Some("old-session".to_string()),
        kind: TransferJobKind::Download {
            remote_path: "/srv/app.txt".to_string(),
            raw_path_token: None,
            local_path: PathBuf::from("/tmp/app.txt"),
        },
        status: TransferJobStatus::Running,
        detail: "Downloading".to_string(),
        created_at_ms: TransferJobState::now_ms(),
        display_name: String::new(),
        entries: Vec::new(),
        summary: None,
        progress: None,
        control: None,
        speed: Default::default(),
    });
    transfer.enqueue_transfer_job(TransferJobState {
        id: "upload".to_string(),
        session_id: Some("old-session".to_string()),
        kind: TransferJobKind::Upload {
            local_path: PathBuf::from("/tmp/app.txt"),
            remote_path: "/srv/app.txt".to_string(),
        },
        status: TransferJobStatus::Running,
        detail: "Uploading".to_string(),
        created_at_ms: TransferJobState::now_ms(),
        display_name: String::new(),
        entries: Vec::new(),
        summary: None,
        progress: None,
        control: None,
        speed: Default::default(),
    });
    transfer.enqueue_transfer_job(TransferJobState {
        id: "list".to_string(),
        session_id: Some("old-session".to_string()),
        kind: TransferJobKind::ListDir {
            remote_path: "/srv".to_string(),
            select_after: None,
        },
        status: TransferJobStatus::Running,
        detail: "Listing".to_string(),
        created_at_ms: TransferJobState::now_ms(),
        display_name: String::new(),
        entries: Vec::new(),
        summary: None,
        progress: None,
        control: None,
        speed: Default::default(),
    });

    assert!(transfer.replace_session_id("old-session", "new-session"));
    transfer.remove_browser_session_cache("old-session");

    assert!(transfer.has_browser_session_cache("new-session"));
    assert!(!transfer.has_browser_session_cache("old-session"));
    assert_eq!(
        transfer
            .restore_browser_session_cache("new-session")
            .as_deref(),
        Some("/srv")
    );
    assert!(!transfer.browser.navigation_jobs.contains_key("new-session"));
    assert!(!transfer.browser.navigation_jobs.contains_key("old-session"));
    assert!(
        !transfer
            .browser
            .pending_navigations
            .contains_key("sftp-list-1")
    );
    assert!(
        !transfer
            .browser
            .pending_navigations
            .contains_key("orphan-list")
    );
    assert_eq!(
        transfer.transfer_jobs()[0].session_id.as_deref(),
        Some("new-session")
    );
    assert_eq!(
        transfer.transfer_jobs()[1].session_id.as_deref(),
        Some("new-session")
    );
    assert_eq!(
        transfer.transfer_jobs()[2].session_id.as_deref(),
        Some("old-session")
    );
}

#[test]
fn browser_selection_replacement_preserves_the_explicit_active_path() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut transfer = transfer_state(&cx);
    transfer.select_browser_entry("/active".to_string());

    let selected = ["/base".to_string()].into_iter().collect();
    let selected_count = transfer.replace_browser_selection(selected, Some("/active".to_string()));

    assert_eq!(selected_count, 1);
    assert_eq!(
        transfer.browser.selected_remote_path.as_deref(),
        Some("/active")
    );
    assert_eq!(
        transfer.browser.selected_remote_paths,
        ["/base".to_string()].into_iter().collect()
    );
}

fn transfer_queue(cx: &TestAppContext) -> TransferQueueState {
    let (tx, rx) = unbounded();
    let focus = cx.update(|cx| cx.focus_handle());
    TransferQueueState::new(tx, rx, focus)
}

fn transfer_job(
    id: &str,
    session_id: &str,
    status: TransferJobStatus,
    controlled: bool,
) -> TransferJobState {
    TransferJobState {
        id: id.to_string(),
        session_id: Some(session_id.to_string()),
        kind: TransferJobKind::Download {
            remote_path: format!("/remote/{id}"),
            raw_path_token: None,
            local_path: PathBuf::from(format!("/local/{id}")),
        },
        status,
        detail: String::new(),
        created_at_ms: TransferJobState::now_ms(),
        display_name: String::new(),
        entries: Vec::new(),
        summary: None,
        progress: None,
        control: controlled.then(SftpTransferControl::new),
        speed: Default::default(),
    }
}

fn external_sync_prompt(session_id: Option<&str>, job_id: &str) -> TransferExternalSyncPromptState {
    TransferExternalSyncPromptState {
        session_id: session_id.map(str::to_string),
        job_id: job_id.to_string(),
        remote_path: format!("/remote/{job_id}.txt"),
        raw_path_token: None,
        local_path: PathBuf::from(format!("/local/{job_id}.txt")),
    }
}

fn editor_tab(session_id: &str, remote_path: &str) -> TransferEditorState {
    TransferEditorState {
        display: nyaterm_core::InternalEditorDisplay::Window,
        id: TransferEditorState::tab_id(Some(session_id), remote_path),
        session_id: Some(session_id.to_string()),
        remote_path: remote_path.to_string(),
        raw_path_token: None,
        name: remote_path.rsplit('/').next().unwrap().to_string(),
        content: Default::default(),
        search_query: String::new(),
        active_match: 0,
        revision: Some(RemoteTextRevision::from_bytes(
            b"",
            RemoteTextMetadata {
                size: 0,
                modified_at: Some(1),
            },
        )),
        generation: RemoteTextGeneration::next(),
        loading: false,
        saving: false,
        dirty: false,
        conflict: false,
        close_after_save: false,
        reload_confirm: false,
        error: None,
        backup_warning_path: None,
        focused_field: TransferEditorField::Content,
    }
}

#[test]
fn editor_changes_keep_save_snapshots_independent_and_reject_busy_tabs() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut transfer = transfer_state(&cx);
    let mut tab = editor_tab("session-a", "/a.txt");
    tab.content = "original".into();
    let tab_id = tab.id.clone();
    transfer.open_editor_tab(tab);
    let saved = transfer.editor_tab_snapshot(&tab_id).unwrap();
    assert!(!transfer.sync_editor_content(&tab_id, "original"));
    assert!(!transfer.active_editor_tab().unwrap().dirty);
    let mut edited = saved.content.clone();
    edited.push('🙂');
    assert!(transfer.sync_editor_content(&tab_id, edited.clone()));
    assert!(
        transfer
            .active_editor_tab()
            .unwrap()
            .content
            .same_version(&edited)
    );
    assert!(!transfer.sync_editor_content(&tab_id, edited.clone()));
    assert_eq!(saved.content, "original");
    assert!(transfer.begin_editor_tab_save(&tab_id));
    assert!(!transfer.sync_editor_content(&tab_id, "during save"));
    assert_eq!(transfer.active_editor_tab().unwrap().content, "original🙂");
}

#[test]
fn transfer_paths_own_endpoints_policy_and_prompt_admission() {
    let mut paths = TransferPathState::new(
        "  ".to_string(),
        "/tmp/download".to_string(),
        SftpDuplicatePolicy::Ask,
    );

    assert_eq!(paths.normalized_remote_path(), ".");
    assert_eq!(paths.local_path(), "/tmp/download");
    assert_eq!(paths.duplicate_policy(), SftpDuplicatePolicy::Ask);

    paths.set_remote_path("/srv/files");
    paths.set_local_path("/tmp/upload");
    paths.set_duplicate_policy(SftpDuplicatePolicy::Overwrite);
    assert_eq!(paths.remote_path(), "/srv/files");
    assert_eq!(paths.normalized_remote_path(), "/srv/files");
    assert_eq!(paths.local_path(), "/tmp/upload");
    assert_eq!(paths.duplicate_policy(), SftpDuplicatePolicy::Overwrite);

    assert!(paths.begin_prompt(TransferPathPromptKind::UploadFile));
    assert!(!paths.begin_prompt(TransferPathPromptKind::DownloadDirectory));
    assert!(!paths.finish_prompt(TransferPathPromptKind::DownloadDirectory));
    assert!(!paths.begin_prompt(TransferPathPromptKind::UploadDirectory));
    assert!(paths.finish_prompt(TransferPathPromptKind::UploadFile));
    assert!(!paths.finish_prompt(TransferPathPromptKind::UploadFile));
}

#[test]
fn transfer_job_retry_keeps_the_original_policy_and_releases_it_after_success() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut transfer = transfer_state(&cx);
    let initial = transfer.bind_transfer_job_path_options(
        "sftp-upload-1",
        SftpPathTransferOptions::new(
            SftpDuplicatePolicy::Ask,
            None,
            SftpTransferOptions::default(),
        ),
    );
    assert_eq!(initial.duplicate_policy(), SftpDuplicatePolicy::Ask);
    assert!(transfer.has_transfer_job_path_options("sftp-upload-1"));

    // 全局策略已改为 Skip，已有任务仍沿用它启动时的 Ask 策略。
    let retry = transfer.transfer_job_retry_path_options(
        "sftp-upload-1",
        SftpPathTransferOptions::new(
            SftpDuplicatePolicy::Skip,
            None,
            SftpTransferOptions::default().with_max_retries(3),
        ),
    );
    assert_eq!(retry.duplicate_policy(), SftpDuplicatePolicy::Ask);
    assert_eq!(retry.transfer_options().max_retries(), 3);

    transfer.release_transfer_job_path_options("sftp-upload-1");
    assert!(!transfer.has_transfer_job_path_options("sftp-upload-1"));
}

#[test]
fn transfer_panel_owns_height_and_resize_lifecycle() {
    let mut panel = TransferPanelState {
        height: 120.,
        height_resize: None,
    };

    panel.start_height_resize(px(400.));
    assert_eq!(panel.update_height_resize(px(450.)), Some(70.));
    assert_eq!(panel.update_height_resize(px(800.)), Some(60.));
    assert!(panel.finish_height_resize());
    assert!(!panel.finish_height_resize());
    assert!(panel.update_height_resize(px(300.)).is_none());

    panel.start_height_resize(px(400.));
    assert_eq!(panel.update_height_resize(px(-200.)), Some(600.));
    assert!(panel.finish_height_resize());
}

#[test]
fn transfer_file_ops_track_real_rename_input_focus_and_creation_options() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut transfer = transfer_state(&cx);

    transfer.schedule_rename_focus();
    assert!(!transfer.rename_focus_is_pending());
    transfer.open_rename_dialog(TransferRenameState {
        old_path: "/srv/old".to_string(),
        raw_path_token: None,
        initial_name: "old".to_string(),
        value: "old".to_string(),
    });
    transfer.schedule_rename_focus();
    assert!(transfer.rename_focus_is_pending());
    assert_eq!(
        transfer.pending_rename_input_id().as_deref(),
        Some("transfer.rename./srv/old")
    );
    transfer.finish_rename_focus();
    assert!(!transfer.rename_focus_is_pending());
    transfer.schedule_rename_focus();
    transfer.close_rename_dialog();
    assert!(!transfer.rename_dialog_is_open());
    assert!(!transfer.rename_focus_is_pending());
    assert!(transfer.pending_rename_input_id().is_none());
    transfer.close_rename_dialog();
    assert!(!transfer.rename_dialog_is_open());
    assert!(!transfer.rename_focus_is_pending());

    transfer.open_new_folder_dialog(TransferNewFolderState {
        parent_path: "/srv".to_string(),
        value: String::new(),
        mode: 0o755,
        open_after_create: false,
    });
    assert!(transfer.set_new_folder_name("logs".to_string()));
    assert!(transfer.toggle_new_folder_open_after_create());
    assert!(transfer.toggle_new_folder_mode_bit(0o020));
    let folder = transfer
        .new_folder_dialog()
        .expect("new folder dialog should remain open");
    assert_eq!(folder.value, "logs");
    assert!(folder.open_after_create);
    assert_eq!(folder.mode, 0o775);
}

#[test]
fn external_sync_prompts_are_filtered_by_session_and_window_admission() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut transfer = transfer_state(&cx);
    transfer.insert_external_sync_prompt(
        "prompt-a".to_string(),
        external_sync_prompt(Some("session-a"), "job-a"),
    );
    transfer.insert_external_sync_prompt(
        "prompt-b".to_string(),
        external_sync_prompt(Some("session-b"), "job-b"),
    );

    assert_eq!(
        transfer
            .active_external_sync_prompt("session-a")
            .map(|(prompt_id, _)| prompt_id),
        Some("prompt-a".to_string())
    );
    assert!(transfer.begin_external_sync_window_open("prompt-a"));
    assert!(!transfer.begin_external_sync_window_open("prompt-a"));
    assert!(transfer.active_external_sync_prompt("session-a").is_none());
    assert_eq!(
        transfer
            .active_external_sync_prompt("session-b")
            .map(|(prompt_id, _)| prompt_id),
        Some("prompt-b".to_string())
    );
    assert!(!transfer.begin_external_sync_window_open("missing"));

    assert!(transfer.clear_external_sync_window_tracking("prompt-a"));
    assert!(!transfer.external_sync_window_open_is_pending("prompt-a"));
    assert_eq!(
        transfer
            .active_external_sync_prompt("session-a")
            .map(|(prompt_id, _)| prompt_id),
        Some("prompt-a".to_string())
    );

    assert!(transfer.begin_external_sync_window_open("prompt-b"));
    assert!(transfer.dismiss_external_sync_prompt("prompt-b"));
    assert!(transfer.external_sync_prompt("prompt-b").is_none());
    assert!(!transfer.external_sync_window_open_is_pending("prompt-b"));
    assert!(!transfer.dismiss_external_sync_prompt("prompt-b"));
}

#[test]
fn external_sync_upload_resolution_cleans_tracking_and_records_policy() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut transfer = transfer_state(&cx);
    transfer.insert_external_sync_prompt(
        "prompt-a".to_string(),
        external_sync_prompt(Some("session-a"), "job-a"),
    );
    assert!(transfer.begin_external_sync_window_open("prompt-a"));

    let prompt = transfer
        .take_external_sync_prompt_for_upload(
            "prompt-a",
            Some("/remote/job-a.txt\n/local/job-a.txt".to_string()),
        )
        .expect("known prompt should resolve for upload");

    assert_eq!(prompt.job_id, "job-a");
    assert!(transfer.external_sync_prompt("prompt-a").is_none());
    assert!(!transfer.external_sync_window_open_is_pending("prompt-a"));
    assert!(transfer.external_sync_always_uploads("/remote/job-a.txt\n/local/job-a.txt"));
    assert!(
        transfer
            .take_external_sync_prompt_for_upload("prompt-a", None)
            .is_none()
    );
}

#[test]
fn external_sync_session_cleanup_preserves_other_sessions_and_policy() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut transfer = transfer_state(&cx);
    transfer.insert_external_sync_prompt(
        "prompt-a-1".to_string(),
        external_sync_prompt(Some("session-a"), "job-a-1"),
    );
    transfer.insert_external_sync_prompt(
        "prompt-a-2".to_string(),
        external_sync_prompt(Some("session-a"), "job-a-2"),
    );
    transfer.insert_external_sync_prompt(
        "prompt-b".to_string(),
        external_sync_prompt(Some("session-b"), "job-b"),
    );
    transfer.insert_external_sync_prompt(
        "policy-source".to_string(),
        external_sync_prompt(None, "policy-source"),
    );
    transfer.take_external_sync_prompt_for_upload(
        "policy-source",
        Some("persistent-watch-key".to_string()),
    );
    assert!(transfer.begin_external_sync_window_open("prompt-a-1"));
    assert!(transfer.begin_external_sync_window_open("prompt-b"));

    assert_eq!(transfer.clear_external_sync_for_session("session-a"), 2);
    assert!(transfer.external_sync_prompt("prompt-a-1").is_none());
    assert!(transfer.external_sync_prompt("prompt-a-2").is_none());
    assert!(!transfer.external_sync_window_open_is_pending("prompt-a-1"));
    assert!(transfer.external_sync_prompt("prompt-b").is_some());
    assert!(transfer.external_sync_window_open_is_pending("prompt-b"));
    assert!(transfer.external_sync_always_uploads("persistent-watch-key"));
}

#[test]
fn transfer_editor_owns_tab_activation_and_close_confirmation() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut transfer = transfer_state(&cx);
    let tab_a = editor_tab("session-a", "/srv/a.txt");
    let tab_a_id = tab_a.id.clone();
    let tab_b = editor_tab("session-b", "/srv/b.txt");
    let tab_b_id = tab_b.id.clone();

    assert!(!transfer.open_editor_tab(tab_a));
    assert!(!transfer.open_editor_tab(tab_b));
    assert_eq!(
        transfer.active_editor_tab().map(|tab| tab.id.as_str()),
        Some(tab_b_id.as_str())
    );
    assert!(transfer.activate_editor_tab(&tab_a_id));
    transfer.active_editor_tab_mut().unwrap().dirty = true;

    assert_eq!(
        transfer.request_editor_tab_close(&tab_a_id),
        TransferEditorCloseOutcome::ConfirmationRequired
    );
    let workspace = transfer.editor_workspace().unwrap();
    assert!(workspace.close_confirm);
    assert_eq!(
        workspace.pending_close_tab_id.as_deref(),
        Some(tab_a_id.as_str())
    );
    assert!(transfer.cancel_editor_close());
    assert!(!transfer.editor_close_confirmation_is_open());

    transfer.active_editor_tab_mut().unwrap().dirty = false;
    assert_eq!(
        transfer.request_editor_tab_close(&tab_a_id),
        TransferEditorCloseOutcome::Closed
    );
    assert_eq!(
        transfer.active_editor_tab().map(|tab| tab.id.as_str()),
        Some(tab_b_id.as_str())
    );
}

#[test]
fn editor_file_tabs_group_by_session_and_close_back_to_related_tab() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut transfer = transfer_state(&cx);
    let a1 = editor_tab("session-a", "/a1.txt");
    let a2 = editor_tab("session-a", "/a2.txt");
    let b = editor_tab("session-b", "/b.txt");
    let (a1_id, a2_id) = (a1.id.clone(), a2.id.clone());
    transfer.open_editor_tab(a1);
    transfer.open_editor_tab(b);
    transfer.open_editor_tab(a2);
    let workspace = transfer.editor_workspace().unwrap();
    assert_eq!(
        workspace
            .tabs
            .iter()
            .map(|tab| tab.session_id.as_deref())
            .collect::<Vec<_>>(),
        [Some("session-a"), Some("session-a"), Some("session-b")]
    );
    // An older interleaved workspace still returns to the same session.
    transfer.editor.workspace.as_mut().unwrap().tabs.swap(1, 2);
    assert_eq!(
        transfer.request_editor_tab_close(&a2_id),
        TransferEditorCloseOutcome::Closed
    );
    assert_eq!(transfer.active_editor_tab().unwrap().id, a1_id);
}

#[test]
fn transfer_editor_save_completion_closes_requested_tab_atomically() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut transfer = transfer_state(&cx);
    let tab = editor_tab("session-a", "/srv/a.txt");
    let tab_id = tab.id.clone();
    transfer.open_editor_tab(tab);
    assert!(transfer.sync_editor_content(&tab_id, "updated".to_string()));
    assert_eq!(
        transfer.request_editor_tab_close(&tab_id),
        TransferEditorCloseOutcome::ConfirmationRequired
    );
    assert_eq!(
        transfer.prepare_editor_close_after_save(),
        TransferEditorCloseAfterSave::Ready(tab_id.clone())
    );
    assert!(transfer.begin_editor_tab_save(&tab_id));

    assert_eq!(
        transfer.complete_editor_save(
            Some("session-a"),
            "/srv/a.txt",
            SftpWriteTextResult::Saved {
                modified_at: 2,
                size: 7,
            },
        ),
        Some(TransferEditorSaveOutcome::SavedAndClosed)
    );
    assert!(!transfer.editor_has_workspace());
}

#[test]
fn transfer_editor_save_all_waits_for_every_dirty_tab() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut transfer = transfer_state(&cx);
    let tab_a = editor_tab("session-a", "/srv/a.txt");
    let tab_a_id = tab_a.id.clone();
    let tab_b = editor_tab("session-b", "/srv/b.txt");
    let tab_b_id = tab_b.id.clone();
    transfer.open_editor_tab(tab_a);
    transfer.open_editor_tab(tab_b);
    assert!(transfer.sync_editor_content(&tab_a_id, "updated a".to_string()));
    assert!(transfer.sync_editor_content(&tab_b_id, "updated b".to_string()));
    assert_eq!(
        transfer.request_editor_close(),
        TransferEditorCloseOutcome::ConfirmationRequired
    );
    assert_eq!(
        transfer.prepare_editor_close_after_save(),
        TransferEditorCloseAfterSave::All
    );
    assert!(transfer.begin_editor_tab_save(&tab_a_id));
    assert!(transfer.begin_editor_tab_save(&tab_b_id));

    assert_eq!(
        transfer.complete_editor_save(
            Some("session-a"),
            "/srv/a.txt",
            SftpWriteTextResult::Saved {
                modified_at: 2,
                size: 9,
            },
        ),
        Some(TransferEditorSaveOutcome::Saved)
    );
    assert!(transfer.editor_has_workspace());
    assert_eq!(
        transfer.complete_editor_save(
            Some("session-b"),
            "/srv/b.txt",
            SftpWriteTextResult::Saved {
                modified_at: 3,
                size: 9,
            },
        ),
        Some(TransferEditorSaveOutcome::SavedAndClosed)
    );
    assert!(!transfer.editor_has_workspace());
}

#[test]
fn transfer_editor_conflict_and_session_cleanup_preserve_other_tabs() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut transfer = transfer_state(&cx);
    let tab_a = editor_tab("session-a", "/srv/a.txt");
    let tab_a_id = tab_a.id.clone();
    let tab_b = editor_tab("session-b", "/srv/b.txt");
    let tab_b_id = tab_b.id.clone();
    transfer.open_editor_tab(tab_a);
    transfer.open_editor_tab(tab_b);
    assert!(transfer.activate_editor_tab(&tab_a_id));
    assert!(transfer.begin_editor_tab_save(&tab_a_id));
    assert_eq!(
        transfer.complete_editor_save(
            Some("session-a"),
            "/srv/a.txt",
            SftpWriteTextResult::Conflict {
                modified_at: 3,
                size: 9,
            },
        ),
        Some(TransferEditorSaveOutcome::Conflict)
    );
    assert!(transfer.editor_close_confirmation_is_open());

    assert_eq!(transfer.remove_editor_tabs_for_session("session-a"), 1);
    assert_eq!(
        transfer.active_editor_tab().map(|tab| tab.id.as_str()),
        Some(tab_b_id.as_str())
    );
    assert!(!transfer.editor_close_confirmation_is_open());
    assert!(transfer.begin_editor_window_open());
    assert!(!transfer.begin_editor_window_open());
    assert!(transfer.clear_editor_window_tracking());
    assert_eq!(transfer.remove_editor_tabs_for_session("session-b"), 1);
    assert!(!transfer.editor_has_workspace());
}

#[test]
fn transfer_properties_ignore_stale_results_and_close_for_the_owner_session() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut transfer = transfer_state(&cx);
    transfer.open_properties_dialog(TransferPropertiesState {
        symlink_target_value: String::new(),
        session_id: Some("session-a".to_string()),
        entry: file_entry("/srv/file.txt"),
        properties: None,
        mode_value: "0640".to_string(),
        owner_value: String::new(),
        group_value: String::new(),
        recursive: false,
        saving: false,
        error: None,
    });

    assert!(!transfer.complete_properties_load(
        Some("session-b"),
        "/srv/file.txt",
        file_properties("/srv/file.txt"),
        "0600".to_string(),
        "updated-owner".to_string(),
        "updated-group".to_string(),
    ));
    assert!(
        transfer
            .properties_dialog()
            .is_some_and(|state| state.properties.is_none())
    );

    assert!(transfer.complete_properties_load(
        Some("session-a"),
        "/srv/file.txt",
        file_properties("/srv/file.txt"),
        "0600".to_string(),
        "updated-owner".to_string(),
        "updated-group".to_string(),
    ));
    assert_eq!(
        transfer
            .properties_dialog()
            .map(|state| state.owner_value.as_str()),
        Some("updated-owner")
    );
    assert!(transfer.begin_properties_save());
    assert!(!transfer.fail_properties_operation(
        Some("session-b"),
        "/srv/file.txt",
        "stale".to_string(),
    ));
    assert!(
        transfer
            .properties_dialog()
            .is_some_and(|state| state.saving && state.error.is_none())
    );
    assert!(transfer.fail_properties_operation(
        Some("session-a"),
        "/srv/file.txt",
        "denied".to_string(),
    ));
    assert!(
        transfer
            .properties_dialog()
            .is_some_and(|state| { !state.saving && state.error.as_deref() == Some("denied") })
    );
    assert!(!transfer.close_properties_dialog_for_session("session-b"));
    assert!(transfer.close_properties_dialog_for_session("session-a"));
    assert!(transfer.properties_dialog().is_none());

    transfer.open_properties_dialog(TransferPropertiesState {
        symlink_target_value: String::new(),
        session_id: Some("session-a".to_string()),
        entry: file_entry("/srv/file.txt"),
        properties: Some(file_properties("/srv/file.txt")),
        mode_value: "0600".to_string(),
        owner_value: "updated-owner".to_string(),
        group_value: "updated-group".to_string(),
        recursive: false,
        saving: true,
        error: None,
    });
    assert!(!transfer.complete_properties_update(
        Some("session-a"),
        "/srv/other.txt",
        file_properties("/srv/other.txt"),
    ));
    assert!(transfer.complete_properties_update(
        Some("session-a"),
        "/srv/file.txt",
        file_properties("/srv/file.txt"),
    ));
    assert!(transfer.properties_dialog().is_none());
}

#[test]
fn transfer_queue_owns_admission_events_and_job_removal() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut queue = transfer_queue(&cx);
    queue.enqueue(transfer_job(
        "job-1",
        "session-a",
        TransferJobStatus::Completed,
        false,
    ));

    assert_eq!(queue.next_job_id("download"), "download-2");
    assert!(queue.select_job("job-1"));
    assert!(queue.open_job_menu("job-1", px(12.), px(24.)));
    assert_eq!(queue.selected_job_id(), Some("job-1"));
    assert_eq!(
        queue.job_menu().map(|menu| menu.job_id.as_str()),
        Some("job-1")
    );
    assert!(queue.can_delete_job("job-1", Some("session-a")));

    assert!(queue.remove_job("job-1"));
    assert!(queue.jobs().is_empty());
    assert_eq!(queue.selected_job_id(), None);
    assert_eq!(queue.next_job_id("download"), "download-3");

    let mut rx = queue
        .take_event_receiver()
        .expect("the queue holds its receiver until the drain starts");
    let sender = queue.event_sender();
    sender
        .unbounded_send(TransferJobResult {
            id: "missing-job".to_string(),
            event: TransferJobEvent::Started {
                detail: "started".to_string(),
            },
        })
        .expect("queue receiver should remain connected");
    let event = rx.try_recv().expect("queue should receive its typed event");
    assert_eq!(event.id, "missing-job");
    assert!(matches!(event.event, TransferJobEvent::Started { .. }));
}

#[test]
fn pausing_and_resuming_visible_jobs_discard_old_speed_samples() {
    use std::time::{Duration, Instant};

    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut queue = transfer_queue(&cx);
    let mut job = transfer_job("running-a", "session-a", TransferJobStatus::Running, true);
    let start = Instant::now();
    job.speed.record(0, start);
    job.speed.record(2048, start + Duration::from_secs(1));
    assert_eq!(job.speed.bytes_per_second(), 2048.);
    queue.enqueue(job);
    assert_eq!(queue.pause_visible_jobs(Some("session-a")), 1);
    let job = queue.job_mut("running-a").expect("queued");
    assert_eq!(job.speed.bytes_per_second(), 0.);
    job.speed.record(2048, start + Duration::from_secs(2));
    job.speed.record(4096, start + Duration::from_secs(3));
    assert_eq!(queue.resume_visible_jobs(Some("session-a")), 1);
    assert_eq!(
        queue
            .job("running-a")
            .expect("queued")
            .speed
            .bytes_per_second(),
        0.
    );
}

#[test]
fn transfer_queue_batches_are_scoped_to_the_visible_session() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut queue = transfer_queue(&cx);
    queue.enqueue(transfer_job(
        "running-a",
        "session-a",
        TransferJobStatus::Running,
        true,
    ));
    queue.enqueue(transfer_job(
        "running-b",
        "session-b",
        TransferJobStatus::Running,
        true,
    ));
    queue.enqueue(transfer_job(
        "completed-a",
        "session-a",
        TransferJobStatus::Completed,
        false,
    ));
    assert!(queue.open_job_menu("completed-a", px(8.), px(8.)));

    assert_eq!(queue.pause_visible_jobs(Some("session-a")), 1);
    assert_eq!(
        queue.job("running-a").map(|job| job.status),
        Some(TransferJobStatus::Paused)
    );
    assert_eq!(
        queue.job("running-b").map(|job| job.status),
        Some(TransferJobStatus::Running)
    );
    assert_eq!(queue.resume_visible_jobs(Some("session-a")), 1);
    assert_eq!(queue.cancel_visible_jobs(Some("session-a")), 1);
    assert_eq!(
        queue.job("running-a").map(|job| job.status),
        Some(TransferJobStatus::Cancelling)
    );
    assert_eq!(queue.clear_completed_jobs(Some("session-a")), 1);
    assert!(queue.job("completed-a").is_none());
    assert_eq!(queue.selected_job_id(), None);
    assert!(queue.job_menu().is_none());
    assert!(queue.job("running-b").is_some());
    assert_eq!(queue.clear_stopped_jobs(Some("session-b")), 0);
}

#[test]
fn transfer_editor_ignores_stale_load_and_failure_generations() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut transfer = transfer_state(&cx);
    let mut tab = editor_tab("session-a", "/srv/a.txt");
    tab.loading = true;
    let tab_id = tab.id.clone();
    let baseline = tab.revision.clone();
    let stale_generation = tab.generation;
    transfer.open_editor_tab(tab);

    let current_generation = RemoteTextGeneration::next();
    {
        let tab = transfer.active_editor_tab_mut().unwrap();
        tab.generation = current_generation;
        tab.content = "new incarnation".into();
    }
    let stale_revision = RemoteTextRevision::from_bytes(
        b"stale",
        RemoteTextMetadata {
            size: 5,
            modified_at: Some(2),
        },
    );
    assert!(!transfer.complete_editor_load_tab(
        &tab_id,
        stale_generation,
        RemoteTextDocument {
            path: "/srv/a.txt".to_string(),
            content: "stale".to_string(),
            revision: stale_revision,
        },
    ));
    assert!(!transfer.fail_editor_operation_tab(
        &tab_id,
        stale_generation,
        "stale failure".to_string(),
    ));
    {
        let tab = transfer.active_editor_tab().unwrap();
        assert_eq!(tab.content, "new incarnation");
        assert!(tab.loading);
        assert!(tab.error.is_none());
        assert_eq!(tab.revision, baseline);
    }
    assert!(transfer.fail_editor_load_tab(
        &tab_id,
        current_generation,
        "reload failed".to_string(),
    ));
    let tab = transfer.active_editor_tab().unwrap();
    assert!(!tab.loading);
    assert_eq!(tab.revision, baseline);
}

#[test]
fn transfer_editor_conflict_preserves_baseline_and_stale_save_is_ignored() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut transfer = transfer_state(&cx);
    let tab = editor_tab("session-a", "/srv/a.txt");
    let tab_id = tab.id.clone();
    let baseline = tab.revision.clone();
    let stale_generation = tab.generation;
    transfer.open_editor_tab(tab);
    assert!(transfer.sync_editor_content(&tab_id, "local edit".to_string()));
    assert!(transfer.begin_editor_tab_save(&tab_id));
    assert_eq!(
        transfer.complete_editor_save_tab(
            &tab_id,
            stale_generation,
            RemoteTextWriteResult::Conflict,
        ),
        Some(TransferEditorSaveOutcome::Conflict)
    );
    assert_eq!(transfer.active_editor_tab().unwrap().revision, baseline);

    let current_generation = RemoteTextGeneration::next();
    {
        let tab = transfer.active_editor_tab_mut().unwrap();
        tab.generation = current_generation;
        tab.saving = false;
        tab.dirty = true;
        tab.content = "newer edit".into();
    }
    let stale_saved_revision = RemoteTextRevision::from_bytes(
        b"local edit",
        RemoteTextMetadata {
            size: 10,
            modified_at: Some(3),
        },
    );
    assert_eq!(
        transfer.complete_editor_save_tab(
            &tab_id,
            stale_generation,
            RemoteTextWriteResult::Saved {
                revision: stale_saved_revision,
            },
        ),
        None
    );
    let tab = transfer.active_editor_tab().unwrap();
    assert_eq!(tab.content, "newer edit");
    assert!(tab.dirty);
    assert_eq!(tab.revision, baseline);
}

#[test]
fn transfer_editor_cannot_discard_a_save_in_flight() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut transfer = transfer_state(&cx);
    let tab = editor_tab("session-a", "/srv/a.txt");
    let tab_id = tab.id.clone();
    transfer.open_editor_tab(tab);
    assert!(transfer.sync_editor_content(&tab_id, "local edit".to_string()));
    assert_eq!(
        transfer.request_editor_tab_close(&tab_id),
        TransferEditorCloseOutcome::ConfirmationRequired
    );
    assert!(transfer.begin_editor_tab_save(&tab_id));

    assert_eq!(
        transfer.discard_editor(),
        super::TransferEditorDiscardOutcome::Missing
    );
    assert!(transfer.editor_has_workspace());
    assert!(transfer.active_editor_tab().is_some_and(|tab| tab.saving));
}

#[test]
fn transfer_editor_keeps_warning_visible_after_successful_save() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut transfer = transfer_state(&cx);
    let tab = editor_tab("session-a", "/srv/a.txt");
    let tab_id = tab.id.clone();
    let generation = tab.generation;
    transfer.open_editor_tab(tab);
    assert!(transfer.sync_editor_content(&tab_id, "local edit".to_string()));
    assert_eq!(
        transfer.request_editor_tab_close(&tab_id),
        TransferEditorCloseOutcome::ConfirmationRequired
    );
    assert_eq!(
        transfer.prepare_editor_close_after_save(),
        TransferEditorCloseAfterSave::Ready(tab_id.clone())
    );
    assert!(transfer.begin_editor_tab_save(&tab_id));
    let backup_path = "/srv/a.txt.nyaterm-backup-1".to_string();
    let revision = RemoteTextRevision::from_bytes(
        b"local edit",
        RemoteTextMetadata {
            size: 10,
            modified_at: Some(4),
        },
    );
    let outcome = transfer.complete_editor_save_tab(
        &tab_id,
        generation,
        RemoteTextWriteResult::SavedWithBackup {
            revision: revision.clone(),
            backup_path: backup_path.clone(),
        },
    );
    assert!(matches!(
        outcome,
        Some(TransferEditorSaveOutcome::SavedWithWarning(_))
    ));
    let tab = transfer.active_editor_tab().unwrap();
    assert!(!tab.dirty);
    assert!(!tab.saving);
    assert_eq!(tab.revision, Some(revision));
    assert_eq!(
        tab.backup_warning_path.as_deref(),
        Some(backup_path.as_str())
    );
    assert!(!transfer.editor_close_confirmation_is_open());
}

fn preview_tab(session_id: &str, remote_path: &str) -> crate::models::TransferPreviewState {
    crate::models::TransferPreviewState {
        id: crate::models::TransferPreviewState::tab_id_for_remote_path(
            Some(session_id),
            &nyaterm_files::sftp::RemoteFilePath::new(remote_path),
        ),
        session_id: Some(session_id.to_string()),
        remote_path: remote_path.to_string(),
        raw_path_token: None,
        name: remote_path
            .rsplit('/')
            .next()
            .unwrap_or(remote_path)
            .to_string(),
        size: None,
        modified_at: None,
        category: nyaterm_core::PreviewCategory::Text,
        generation: RemoteTextGeneration::next(),
        content: crate::models::PreviewContent::Loading,
        viewport: crate::models::PreviewViewport::default(),
    }
}

#[test]
fn preview_workspace_opens_activates_and_closes_tabs() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut transfer = transfer_state(&cx);
    let first = preview_tab("session", "/a.txt");
    let second = preview_tab("session", "/b.txt");
    let first_id = first.id.clone();
    let second_id = second.id.clone();

    assert!(!transfer.open_preview_tab(first));
    // Re-opening the same tab returns true (already open) rather than duplicating.
    assert!(!transfer.open_preview_tab(second));
    assert!(
        transfer.open_preview_tab(preview_tab("session", "/a.txt")),
        "an already-open path reports already_open"
    );
    assert_eq!(transfer.preview_workspace().unwrap().tabs.len(), 2);

    assert!(transfer.activate_preview_tab(&first_id));
    assert_eq!(
        transfer.active_preview_tab().map(|tab| tab.id.clone()),
        Some(first_id.clone())
    );

    assert_eq!(
        transfer.close_preview_tab(&first_id),
        super::TransferPreviewCloseOutcome::Closed
    );
    assert_eq!(
        transfer.active_preview_tab().map(|tab| tab.id.clone()),
        Some(second_id)
    );
    assert_eq!(
        transfer.close_preview(),
        super::TransferPreviewCloseOutcome::Closed
    );
    assert!(!transfer.preview_has_workspace());
}

#[test]
fn preview_file_tabs_group_by_session_and_close_back_to_related_tab() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut transfer = transfer_state(&cx);
    let a1 = preview_tab("session-a", "/a1.txt");
    let a2 = preview_tab("session-a", "/a2.txt");
    let b = preview_tab("session-b", "/b.txt");
    let (a1_id, a2_id) = (a1.id.clone(), a2.id.clone());
    transfer.open_preview_tab(a1);
    transfer.open_preview_tab(b);
    transfer.open_preview_tab(a2);
    let workspace = transfer.preview_workspace().unwrap();
    assert_eq!(
        workspace
            .tabs
            .iter()
            .map(|tab| tab.session_id.as_deref())
            .collect::<Vec<_>>(),
        [Some("session-a"), Some("session-a"), Some("session-b")]
    );
    transfer.preview.workspace.as_mut().unwrap().tabs.swap(1, 2);
    assert_eq!(
        transfer.close_preview_tab(&a2_id),
        super::TransferPreviewCloseOutcome::Closed
    );
    assert_eq!(transfer.active_preview_tab().unwrap().id, a1_id);
}

#[test]
fn preview_completion_is_dropped_for_a_stale_generation() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut transfer = transfer_state(&cx);
    let tab = preview_tab("session", "/a.txt");
    let tab_id = tab.id.clone();
    let stale_generation = tab.generation;
    transfer.open_preview_tab(tab);

    // Refresh bumps the generation, so the earlier load must be discarded.
    let fresh_generation = transfer.begin_preview_tab_reload(&tab_id).unwrap();
    assert_ne!(stale_generation, fresh_generation);
    assert!(!transfer.complete_preview_tab(
        &tab_id,
        stale_generation,
        crate::models::PreviewContent::Text("stale".to_string()),
    ));
    assert!(matches!(
        transfer.active_preview_tab().map(|tab| &tab.content),
        Some(crate::models::PreviewContent::Loading)
    ));

    // The current generation is applied.
    assert!(transfer.complete_preview_tab(
        &tab_id,
        fresh_generation,
        crate::models::PreviewContent::Text("fresh".to_string()),
    ));
    assert!(matches!(
        transfer.active_preview_tab().map(|tab| &tab.content),
        Some(crate::models::PreviewContent::Text(text)) if text == "fresh"
    ));
}

#[test]
fn preview_tabs_are_removed_when_their_session_ends() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut transfer = transfer_state(&cx);
    transfer.open_preview_tab(preview_tab("session-a", "/a.txt"));
    transfer.open_preview_tab(preview_tab("session-b", "/b.txt"));
    assert_eq!(transfer.remove_preview_tabs_for_session("session-a"), 1);
    assert_eq!(
        transfer
            .active_preview_tab()
            .map(|tab| tab.remote_path.clone()),
        Some("/b.txt".to_string())
    );
    assert_eq!(transfer.remove_preview_tabs_for_session("session-b"), 1);
    assert!(!transfer.preview_has_workspace());
}

#[test]
fn preview_zoom_clamps_and_reset_restores_defaults() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut transfer = transfer_state(&cx);
    transfer.open_preview_tab(preview_tab("session", "/a.txt"));

    assert!(transfer.preview_zoom_active_tab(100.0));
    assert_eq!(
        transfer.active_preview_tab().unwrap().viewport.zoom,
        crate::models::PreviewViewport::MAX_ZOOM
    );
    assert!(transfer.preview_reset_active_viewport());
    assert_eq!(transfer.active_preview_tab().unwrap().viewport.zoom, 1.0);
    assert!(transfer.preview_rotate_active_tab(true));
    assert_eq!(
        transfer
            .active_preview_tab()
            .unwrap()
            .viewport
            .rotation_quarter_turns,
        1
    );
    // Rotating left from a single clockwise turn returns to zero.
    assert!(transfer.preview_rotate_active_tab(false));
    assert_eq!(
        transfer
            .active_preview_tab()
            .unwrap()
            .viewport
            .rotation_quarter_turns,
        0
    );
}

#[test]
fn preview_pdf_navigation_requests_pages_and_applies_under_generation() {
    use std::sync::Arc;
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut transfer = transfer_state(&cx);
    let mut tab = preview_tab("session", "/doc.pdf");
    tab.category = nyaterm_core::PreviewCategory::Pdf;
    tab.content = crate::models::PreviewContent::Pdf(crate::models::PreviewPdfDocument::new(
        Arc::new(Vec::new()),
        5,
    ));
    let generation = tab.generation;
    let tab_id = tab.id.clone();
    transfer.open_preview_tab(tab);

    // The active page (0) needs rendering, so a request is produced once.
    let request = transfer.pdf_page_request_for_active_tab();
    assert!(request.is_some());
    assert_eq!(request.unwrap().page_index, 0);
    // A second call returns nothing because page 0 is now pending.
    assert!(transfer.pdf_page_request_for_active_tab().is_none());

    // Navigating forward clamps within range and requests page 2.
    transfer.preview_next_pdf_page();
    let next = transfer.preview_next_pdf_page();
    assert!(next.is_some());
    assert_eq!(next.unwrap().page_index, 2);

    // A stale generation is rejected.
    let stale = crate::models::PreviewPdfDocument::new(Arc::new(Vec::new()), 5);
    let _ = stale;
    assert!(!transfer.complete_pdf_page(
        &tab_id,
        RemoteTextGeneration::next(),
        2,
        dummy_pdf_page(),
    ));
    // The current generation is applied and clears the pending marker.
    assert!(transfer.complete_pdf_page(&tab_id, generation, 2, dummy_pdf_page()));
}

fn dummy_pdf_page() -> crate::models::PreviewPdfPage {
    use gpui::RenderImage;
    use std::sync::Arc;
    let bytes = vec![0u8, 0, 0, 255];
    let buffer = image::RgbaImage::from_raw(1, 1, bytes.clone()).unwrap();
    crate::models::PreviewPdfPage {
        image: Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])),
        pixels: Arc::new(bytes),
        src_width: 1,
        src_height: 1,
        width: 1,
        height: 1,
    }
}

#[test]
fn promised_retry_keeps_original_source_and_execution_contract_until_released() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut transfer = transfer_state(&cx);
    let source = std::sync::Arc::new(nyaterm_files::remote_file::RemoteFileService::new(
        nyaterm_transport::SshSessionConfig::default(),
    ));
    transfer.bind_promised_download(
        "promise",
        SftpPathTransferOptions::for_promised_download(
            SftpTransferOptions::default().with_max_retries(2),
            nyaterm_files::sftp::SftpFileType::Directory,
        ),
        std::sync::Arc::downgrade(&source),
    );
    let retry = transfer.transfer_job_retry_path_options(
        "promise",
        SftpPathTransferOptions::new(
            SftpDuplicatePolicy::Overwrite,
            None,
            SftpTransferOptions::default().with_max_retries(9),
        ),
    );
    assert!(retry.is_promised_download());
    assert_eq!(retry.duplicate_policy(), SftpDuplicatePolicy::Skip);
    assert_eq!(retry.transfer_options().max_retries(), 2);
    let saved = transfer.promised_download_source("promise").unwrap();
    let nyaterm_files::file_browser::WeakFileBrowserService::Remote(saved) = saved else {
        panic!("wrong source backend")
    };
    assert!(std::sync::Weak::ptr_eq(
        &saved,
        &std::sync::Arc::downgrade(&source)
    ));
    drop(source);
    assert!(
        transfer
            .promised_download_source("promise")
            .unwrap()
            .upgrade()
            .is_none()
    );
    transfer.release_transfer_job_path_options("promise");
    assert!(transfer.promised_download_source("promise").is_none());
}

#[test]
fn file_operation_context_retains_both_sessions_and_migrates_retry_identity() {
    use crate::features::transfers::file_operations::{
        FileOperationContext, FileOperationEndpoint, FileOperationRequest, FileTransferIntent,
        FileTransferOperation,
    };
    use nyaterm_files::sftp::RemoteFilePath;
    let cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = transfer_state(&cx);
    state.enqueue_transfer_job(transfer_job(
        "frozen",
        "source",
        TransferJobStatus::Running,
        true,
    ));
    state.transfer_job_mut("frozen").unwrap().kind = TransferJobKind::SendTo {
        source_path: "/original/file".into(),
        target_session_id: "target".into(),
        target_path: "/target/file".into(),
        target_parent_path: "/target".into(),
    };
    state.track_file_operation(
        "frozen".into(),
        FileOperationContext {
            source_label: "source".into(),
            target_label: "target".into(),
            request: FileOperationRequest::Transfer(FileTransferIntent {
                source: FileOperationEndpoint::Session("source".into()),
                entries: vec![file_entry("/original/file")],
                target: FileOperationEndpoint::Session("target".into()),
                target_directory: RemoteFilePath::from_raw("/target", b"/target"),
                operation: FileTransferOperation::Move,
            }),
        },
    );
    assert!(state.file_operation_references_session("source"));
    assert!(state.file_operation_references_session("target"));
    assert!(state.replace_session_id("target", "reconnected"));
    assert!(matches!(
        &state.transfer_job("frozen").unwrap().kind,
        TransferJobKind::SendTo { target_session_id, .. } if target_session_id == "reconnected"
    ));
    assert!(!state.file_operation_references_session("target"));
    assert!(state.file_operation_references_session("reconnected"));
    state.transfer_job_mut("frozen").unwrap().status = TransferJobStatus::Failed;
    assert!(!state.file_operation_references_session("source"));
    let context = state.file_operation("frozen").unwrap();
    let FileOperationRequest::Transfer(intent) = &context.request else {
        panic!("lost transfer context");
    };
    assert_eq!(
        intent.source,
        FileOperationEndpoint::Session("source".into())
    );
    assert_eq!(
        intent.target,
        FileOperationEndpoint::Session("reconnected".into())
    );
    assert_eq!(intent.entries[0].path, "/original/file");
    assert_eq!(
        intent.target_directory.raw_path().unwrap().unwrap(),
        b"/target"
    );
    assert_eq!(intent.operation, FileTransferOperation::Move);
}

#[test]
fn metadata_file_operations_are_excluded_from_shared_pause() {
    use crate::features::transfers::file_operations::{
        FileMutation, FileMutationIntent, FileOperationContext, FileOperationEndpoint,
        FileOperationRequest,
    };
    use nyaterm_files::sftp::RemoteFilePath;
    let cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = transfer_state(&cx);
    state.enqueue_transfer_job(transfer_job(
        "metadata",
        "source",
        TransferJobStatus::Running,
        true,
    ));
    state.track_file_operation(
        "metadata".into(),
        FileOperationContext {
            source_label: "source".into(),
            target_label: "source".into(),
            request: FileOperationRequest::Mutation(FileMutationIntent {
                endpoint: FileOperationEndpoint::Session("source".into()),
                directory: RemoteFilePath::new("/frozen"),
                mutation: FileMutation::CreateDirectory("new".into()),
            }),
        },
    );
    assert!(!state.supports_bulk_admission("metadata"));
    assert_eq!(state.pause_visible_transfer_jobs(Some("source")), 0);
    assert_eq!(
        state.transfer_job("metadata").unwrap().status,
        TransferJobStatus::Running
    );
}

#[test]
fn session_close_and_window_shutdown_cancel_queued_work_without_starting_it() {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut state = transfer_state(&cx);
    for (id, session) in [("closed", "a"), ("other", "b"), ("copy", "b")] {
        let mut job = transfer_job(id, session, TransferJobStatus::Running, true);
        if id == "copy" {
            job.kind = TransferJobKind::SendTo {
                source_path: "/file".into(),
                target_session_id: "a".into(),
                target_path: "/target".into(),
                target_parent_path: "/".into(),
            };
        }
        state.enqueue_transfer_job(job);
        state.admit_bulk_job(
            id,
            "queued-close-test",
            state.transfer_event_sender(),
            Box::new(|| panic!("cancelled queue entry started")),
        );
    }
    assert_eq!(state.pause_visible_transfer_jobs(None), 3);
    state.cancel_bulk_jobs_for_session("a");
    state.reconcile_bulk_jobs(2, 2);
    assert_eq!(
        state.transfer_job("closed").unwrap().status,
        TransferJobStatus::Cancelling
    );
    assert_eq!(
        state.transfer_job("copy").unwrap().status,
        TransferJobStatus::Cancelling
    );
    assert_eq!(
        state.transfer_job("other").unwrap().status,
        TransferJobStatus::Paused
    );
    let other = state
        .transfer_job("other")
        .unwrap()
        .control
        .clone()
        .unwrap();
    state.begin_bulk_shutdown()();
    assert!(other.is_cancelled());
    assert!(!state.manages_bulk_job("other"));
    assert!(state.admission.shutdown().is_empty());
}
