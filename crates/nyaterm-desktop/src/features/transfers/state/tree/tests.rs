use super::TransferTreeState;
use nyaterm_files::file_browser::FileBrowserBackendKind;
use nyaterm_files::sftp::{RemoteFilePath, SftpFileEntry, SftpFileType};
use std::sync::Arc;

fn directory(name: &str, path: &str) -> SftpFileEntry {
    SftpFileEntry {
        name: name.into(),
        path: path.into(),
        file_type: SftpFileType::Directory,
        size: None,
        permissions: None,
        owner: String::new(),
        group: String::new(),
        modified_at: None,
        raw_path_token: None,
        symlink_target_is_directory: false,
    }
}

fn file(name: &str, path: &str) -> SftpFileEntry {
    SftpFileEntry {
        name: name.into(),
        path: path.into(),
        file_type: SftpFileType::File,
        size: Some(1),
        permissions: None,
        owner: String::new(),
        group: String::new(),
        modified_at: None,
        raw_path_token: None,
        symlink_target_is_directory: false,
    }
}

fn cached_tree() -> TransferTreeState {
    let mut tree = TransferTreeState::default();
    let backend = FileBrowserBackendKind::Remote;
    tree.seed(
        "a",
        backend,
        RemoteFilePath::new("/"),
        Arc::new(vec![
            directory("shell", "/shell"),
            directory("target", "/target"),
            directory("other", "/other"),
        ]),
    );
    tree.seed(
        "a",
        backend,
        RemoteFilePath::new("/shell"),
        Arc::new(vec![file("one", "/shell/one")]),
    );
    tree.reveal("a", backend, RemoteFilePath::new("/shell"));
    tree.presentation(Some("a"), true);
    tree.select("a", "/shell/one".into(), false, false);
    tree
}

#[test]
fn failed_navigation_restores_tree_interaction_without_discarding_new_caches() {
    let mut tree = cached_tree();
    let backend = FileBrowserBackendKind::Remote;
    let expanded = tree.sessions["a"].expanded.clone();
    tree.sessions["a"]
        .scroll
        .scroll_to_item(1, gpui::ScrollStrategy::Top);
    let deferred = tree.sessions["a"]
        .scroll
        .0
        .borrow()
        .deferred_scroll_to_item
        .unwrap()
        .item_index;
    tree.begin_navigation("a", backend, "nav".into());
    tree.reveal("a", backend, RemoteFilePath::new("/target"));
    tree.presentation(Some("a"), true);
    tree.seed(
        "a",
        backend,
        RemoteFilePath::new("/other"),
        Arc::new(vec![file("new", "/other/new")]),
    );
    assert!(tree.settle_navigation("a", "nav", false));
    assert_eq!(tree.sessions["a"].expanded, expanded);
    assert_eq!(tree.sessions["a"].focused.as_deref(), Some("/shell/one"));
    assert_eq!(tree.sessions["a"].anchor.as_deref(), Some("/shell/one"));
    assert!(tree.sessions["a"].selected.contains("/shell/one"));
    assert_eq!(
        tree.sessions["a"]
            .scroll
            .0
            .borrow()
            .deferred_scroll_to_item
            .unwrap()
            .item_index,
        deferred
    );
    assert!(tree.sessions["a"].nodes["/other"].entries.is_some());
}

#[derive(Clone, Copy, Debug)]
enum ManualAction {
    Select,
    ContextSelect,
    SelectAll,
    Clear,
    Move,
    Parent,
    Expand,
    Collapse,
}

#[test]
fn every_manual_tree_interaction_retires_navigation_positioning_on_success_and_failure() {
    for success in [true, false] {
        for action in [
            ManualAction::Select,
            ManualAction::ContextSelect,
            ManualAction::SelectAll,
            ManualAction::Clear,
            ManualAction::Move,
            ManualAction::Parent,
            ManualAction::Expand,
            ManualAction::Collapse,
        ] {
            let mut tree = cached_tree();
            tree.begin_navigation("a", FileBrowserBackendKind::Remote, "nav".into());
            tree.reveal(
                "a",
                FileBrowserBackendKind::Remote,
                RemoteFilePath::new("/target"),
            );
            tree.presentation(Some("a"), true);
            match action {
                ManualAction::Select => tree.select("a", "/other".into(), false, false),
                ManualAction::ContextSelect => tree.select_context("a", "/other".into()),
                ManualAction::SelectAll => tree.select_all("a"),
                ManualAction::Clear => tree.clear_selection("a"),
                ManualAction::Move => tree.move_selection("a", -1),
                ManualAction::Parent => tree.select_parent("a", "/shell/one"),
                ManualAction::Expand => {
                    tree.toggle("a", "/other", Some(true));
                }
                ManualAction::Collapse => {
                    tree.toggle("a", "/target", Some(false));
                }
            }
            let state = &tree.sessions["a"];
            let expected = (
                state.focused.clone(),
                state.anchor.clone(),
                state.selected.clone(),
                state.expanded.clone(),
            );
            assert!(
                !tree.settle_navigation("a", "nav", success),
                "{action:?}, success={success}"
            );
            let state = &tree.sessions["a"];
            assert_eq!(
                (
                    state.focused.clone(),
                    state.anchor.clone(),
                    state.selected.clone(),
                    state.expanded.clone()
                ),
                expected
            );
        }
    }
}

#[test]
fn superseded_navigation_cannot_restore_over_the_latest_checkpoint() {
    let mut tree = cached_tree();
    let backend = FileBrowserBackendKind::Remote;
    tree.begin_navigation("a", backend, "old".into());
    tree.reveal("a", backend, RemoteFilePath::new("/target"));
    assert!(tree.settle_navigation("a", "old", false));
    tree.begin_navigation("a", backend, "latest".into());
    tree.reveal("a", backend, RemoteFilePath::new("/other"));
    assert!(!tree.settle_navigation("a", "old", false));
    assert_eq!(tree.sessions["a"].focused.as_deref(), Some("/other"));
    assert!(tree.settle_navigation("a", "latest", false));
    assert_eq!(tree.sessions["a"].focused.as_deref(), Some("/shell/one"));
}

#[test]
fn repeated_deep_reveals_submit_one_listing_and_keep_only_latest_ancestor_demand() {
    for depth in [1, 16, 63] {
        let mut tree = TransferTreeState::default();
        let backend = FileBrowserBackendKind::Remote;
        let root = RemoteFilePath::new("/");
        tree.reveal("a", backend, RemoteFilePath::new("/old"));
        let generation = tree.begin_request("a", backend, root.clone()).unwrap();
        tree.track_listing("a".into(), backend, root.clone(), generation, "root".into());
        for index in 0..100 {
            let path = format!("/branch-{index}{}", "/child".repeat(depth - 1));
            tree.reveal("a", backend, RemoteFilePath::new(path));
            assert!(tree.next_listing_path("a", None).is_none());
            assert!(tree.next_listing_path("another-session", None).is_none());
            assert_eq!(
                tree.sessions["a"]
                    .nodes
                    .values()
                    .filter(|node| node.pending.is_some())
                    .count(),
                1
            );
        }
        assert!(tree.finish_listing("root"));
        assert!(tree.complete(
            "a",
            &root,
            generation,
            Ok(vec![directory("branch-99", "/branch-99")])
        ));
        assert_eq!(
            tree.next_listing_path("a", None).unwrap().display_path,
            "/branch-99"
        );
    }
}

#[test]
fn cancelled_listing_keeps_its_slot_until_completion_and_rejects_old_results() {
    let mut tree = cached_tree();
    let backend = FileBrowserBackendKind::Remote;
    let target = RemoteFilePath::new("/target");
    tree.reveal("a", backend, target.clone());
    let generation = tree.begin_request("a", backend, target.clone()).unwrap();
    tree.track_listing(
        "a".into(),
        backend,
        target.clone(),
        generation,
        "old".into(),
    );
    tree.reveal("a", backend, RemoteFilePath::new("/other/child"));
    assert!(tree.sessions["a"].nodes["/target"].pending.is_none());
    assert!(tree.next_listing_path("a", None).is_none());
    assert!(!tree.finish_listing("unrelated"));
    assert!(tree.finish_listing("old"));
    assert!(!tree.complete("a", &target, generation, Ok(vec![])));
    assert_eq!(
        tree.next_listing_path("a", None).unwrap().display_path,
        "/other"
    );
}

#[test]
fn manual_expansion_precedes_ancestors_and_ancestor_failure_waits_for_explicit_retry() {
    let mut tree = cached_tree();
    let backend = FileBrowserBackendKind::Remote;
    let target = RemoteFilePath::new("/target");
    tree.reveal("a", backend, RemoteFilePath::new("/target/child"));
    tree.presentation(Some("a"), true);
    tree.toggle("a", "/other", Some(true));
    assert_eq!(
        tree.next_listing_path("a", None).unwrap().display_path,
        "/other"
    );
    tree.seed(
        "a",
        backend,
        RemoteFilePath::new("/other"),
        Arc::new(vec![]),
    );
    assert_eq!(tree.next_listing_path("a", None).unwrap(), target);
    let generation = tree.begin_request("a", backend, target.clone()).unwrap();
    tree.track_listing(
        "a".into(),
        backend,
        target.clone(),
        generation,
        "denied".into(),
    );
    tree.finish_listing("denied");
    assert!(tree.complete("a", &target, generation, Err("permission denied".into())));
    assert!(tree.next_listing_path("a", None).is_none());
    assert!(!tree.sessions["a"].scroll_to_selection);
    tree.toggle("a", "/target", Some(true));
    assert_eq!(tree.next_listing_path("a", None).unwrap(), target);
    tree.toggle("a", "/target", Some(false));
    assert!(tree.next_listing_path("a", None).is_none());
}

#[test]
fn navigation_excludes_equivalent_utf8_raw_path_from_tree_requests() {
    let mut tree = cached_tree();
    tree.reveal(
        "a",
        FileBrowserBackendKind::Remote,
        RemoteFilePath::new("/target"),
    );
    let navigation_path = RemoteFilePath::from_raw("/target", b"/target");
    assert!(
        tree.next_listing_path("a", Some(&navigation_path))
            .is_none()
    );
}

#[cfg(windows)]
#[test]
fn failed_cross_drive_navigation_restores_root_and_cached_listing() {
    let mut tree = TransferTreeState::default();
    let backend = FileBrowserBackendKind::Local;
    tree.seed(
        "a",
        backend,
        RemoteFilePath::new(r"C:\"),
        Arc::new(vec![directory("old", r"C:\old")]),
    );
    tree.reveal("a", backend, RemoteFilePath::new(r"C:\"));
    tree.presentation(Some("a"), true);
    tree.begin_navigation("a", backend, "nav".into());
    tree.reveal("a", backend, RemoteFilePath::new(r"D:\target"));
    tree.presentation(Some("a"), true);
    assert!(tree.settle_navigation("a", "nav", false));
    let view = tree.presentation(Some("a"), true);
    assert_eq!(view.rows[0].path.display_path, r"C:\");
    assert!(view.rows.iter().any(|row| row.label == "old"));
}

#[cfg(windows)]
#[test]
fn wsl_case_distinct_nodes_keep_independent_listings_and_invalidation() {
    let mut tree = TransferTreeState::default();
    let backend = FileBrowserBackendKind::Local;
    let root = r"\\wsl.localhost\Ubuntu";
    let upper = RemoteFilePath::new(format!(r"{root}\Project"));
    let lower = RemoteFilePath::new(format!(r"{root}\project"));
    tree.seed(
        "wsl",
        backend,
        RemoteFilePath::new(root),
        Arc::new(vec![
            directory("Project", &upper.display_path),
            directory("project", &lower.display_path),
        ]),
    );
    tree.reveal("wsl", backend, RemoteFilePath::new(root));
    let view = tree.presentation(Some("wsl"), true);
    let upper_key = view
        .rows
        .iter()
        .find(|row| row.label == "Project")
        .unwrap()
        .key
        .clone();
    let lower_key = view
        .rows
        .iter()
        .find(|row| row.label == "project")
        .unwrap()
        .key
        .clone();
    assert_ne!(upper_key, lower_key);

    let upper_request = tree.begin_request("wsl", backend, upper.clone()).unwrap();
    let lower_request = tree.begin_request("wsl", backend, lower.clone()).unwrap();
    tree.invalidate("wsl", &upper, false);
    assert!(!tree.complete("wsl", &upper, upper_request, Ok(vec![])));
    assert!(tree.complete("wsl", &lower, lower_request, Ok(vec![])));
}

#[test]
fn reconnect_keeps_cached_tree_listings_but_retires_old_connection_requests() {
    let mut tree = TransferTreeState::default();
    let backend = FileBrowserBackendKind::Remote;
    tree.seed(
        "old",
        backend,
        RemoteFilePath::new("/"),
        Arc::new(vec![directory("pending", "/pending")]),
    );
    let path = RemoteFilePath::new("/pending");
    let generation = tree.begin_request("old", backend, path.clone()).unwrap();
    tree.reveal("old", backend, path.clone());
    tree.replace_session("old", "new");
    assert!(!tree.complete("old", &path, generation, Ok(vec![])));
    assert!(!tree.complete("new", &path, generation, Ok(vec![])));
    assert!(
        tree.presentation(Some("new"), true)
            .rows
            .iter()
            .any(|row| row.label == "pending")
    );
    assert!(tree.begin_request("new", backend, path).is_some());
}

#[test]
fn invalidation_rejects_old_results_and_preserves_other_parents() {
    let mut tree = TransferTreeState::default();
    tree.seed(
        "a",
        FileBrowserBackendKind::Remote,
        RemoteFilePath::new("/"),
        Arc::new(vec![directory("one", "/one"), directory("two", "/two")]),
    );
    let old = tree
        .begin_request(
            "a",
            FileBrowserBackendKind::Remote,
            RemoteFilePath::new("/one"),
        )
        .unwrap();
    let two = tree
        .begin_request(
            "a",
            FileBrowserBackendKind::Remote,
            RemoteFilePath::new("/two"),
        )
        .unwrap();
    tree.invalidate("a", &RemoteFilePath::new("/one"), false);
    assert!(!tree.complete("a", &RemoteFilePath::new("/one"), old, Ok(vec![])));
    assert!(tree.complete("a", &RemoteFilePath::new("/two"), two, Ok(vec![])));
    let new = tree
        .begin_request(
            "a",
            FileBrowserBackendKind::Remote,
            RemoteFilePath::new("/one"),
        )
        .unwrap();
    assert_ne!(old, new);
}

#[test]
fn session_scope_raw_identity_and_keyboard_selection_are_independent() {
    let mut tree = TransferTreeState::default();
    let mut first = directory("same", "/same");
    first.raw_path_token = RemoteFilePath::from_raw("/same", b"/\xff").raw_path_token;
    let mut second = first.clone();
    second.raw_path_token = RemoteFilePath::from_raw("/same", b"/\xfe").raw_path_token;
    tree.seed(
        "a",
        FileBrowserBackendKind::Remote,
        RemoteFilePath::new("/"),
        Arc::new(vec![first, second]),
    );
    tree.reveal(
        "a",
        FileBrowserBackendKind::Remote,
        RemoteFilePath::new("/"),
    );
    let view = tree.presentation(Some("a"), false);
    assert_eq!(view.rows.len(), 3);
    assert_ne!(view.rows[1].key, view.rows[2].key);
    tree.move_selection("a", 1);
    assert_eq!(tree.selected_row("a").unwrap().depth, 1);
    assert_eq!(tree.presentation(Some("b"), false).rows.len(), 0);
    tree.remove_session("a");
    assert!(tree.presentation(Some("a"), false).rows.is_empty());
}

#[test]
fn reveal_missing_ancestors_and_seed_supersede_pending_listings() {
    let mut tree = TransferTreeState::default();
    let pending = tree
        .begin_request(
            "a",
            FileBrowserBackendKind::Remote,
            RemoteFilePath::new("/a/b"),
        )
        .unwrap();
    tree.seed(
        "a",
        FileBrowserBackendKind::Remote,
        RemoteFilePath::new("/a/b"),
        Arc::new(vec![]),
    );
    assert!(!tree.complete(
        "a",
        &RemoteFilePath::new("/a/b"),
        pending,
        Ok(vec![directory("old", "/a/b/old")])
    ));
    let missing = tree.reveal(
        "a",
        FileBrowserBackendKind::Remote,
        RemoteFilePath::new("/a/b"),
    );
    assert_eq!(
        missing
            .iter()
            .map(|p| p.display_path.as_str())
            .collect::<Vec<_>>(),
        vec!["/", "/a"]
    );
    let view = tree.presentation(Some("a"), false);
    assert_eq!(view.focused.as_deref(), Some("/a/b"));
    assert!(tree.sessions["a"].scroll_to_selection);
    tree.seed(
        "a",
        FileBrowserBackendKind::Remote,
        RemoteFilePath::new("/"),
        Arc::new(vec![directory("a", "/a")]),
    );
    let view = tree.presentation(Some("a"), false);
    assert_eq!(view.focused.as_deref(), Some("/a/b"));
    assert!(tree.sessions["a"].scroll_to_selection);
    tree.seed(
        "a",
        FileBrowserBackendKind::Remote,
        RemoteFilePath::new("/a"),
        Arc::new(vec![directory("b", "/a/b")]),
    );
    tree.presentation(Some("a"), false);
    assert!(!tree.sessions["a"].scroll_to_selection);
}

#[test]
fn manual_selection_cancels_a_reveal_waiting_for_ancestors() {
    let mut tree = TransferTreeState::default();
    let backend = FileBrowserBackendKind::Remote;
    tree.reveal("a", backend, RemoteFilePath::new("/a/b"));
    tree.seed(
        "a",
        backend,
        RemoteFilePath::new("/"),
        Arc::new(vec![directory("a", "/a"), directory("other", "/other")]),
    );
    tree.presentation(Some("a"), false);
    tree.select("a", "/other".into(), false, false);
    tree.seed(
        "a",
        backend,
        RemoteFilePath::new("/a"),
        Arc::new(vec![directory("b", "/a/b")]),
    );
    let view = tree.presentation(Some("a"), false);
    assert_eq!(view.focused.as_deref(), Some("/other"));
    assert!(!tree.sessions["a"].scroll_to_selection);
}

#[test]
fn initial_relative_remote_directory_uses_filesystem_root() {
    let mut tree = TransferTreeState::default();
    let missing = tree.reveal(
        "a",
        FileBrowserBackendKind::Remote,
        RemoteFilePath::new("."),
    );
    assert_eq!(missing, vec![RemoteFilePath::new("/")]);
    let view = tree.presentation(Some("a"), false);
    assert_eq!(view.rows.len(), 1);
    assert_eq!(view.rows[0].label, "/");
    assert_eq!(view.rows[0].path, RemoteFilePath::new("/"));
}

#[test]
fn selection_excludes_root_supports_ranges_and_directory_symlinks_do_not_expand() {
    let mut symlink = file("linked-dir", "/linked-dir");
    symlink.file_type = SftpFileType::Symlink;
    symlink.symlink_target_is_directory = true;
    let mut tree = TransferTreeState::default();
    tree.seed(
        "a",
        FileBrowserBackendKind::Remote,
        RemoteFilePath::new("/"),
        Arc::new(vec![
            directory("folder", "/folder"),
            file("one.txt", "/one.txt"),
            symlink,
        ]),
    );
    tree.reveal(
        "a",
        FileBrowserBackendKind::Remote,
        RemoteFilePath::new("/"),
    );
    let view = tree.presentation(Some("a"), false);
    let root = view.rows.iter().find(|row| row.root).unwrap();
    let folder = view.rows.iter().find(|row| row.label == "folder").unwrap();
    let linked = view
        .rows
        .iter()
        .find(|row| row.label == "linked-dir")
        .unwrap();
    assert!(folder.expandable);
    assert!(linked.directory);
    assert!(!linked.expandable);
    assert!(tree.toggle("a", &linked.key, Some(true)).is_none());

    tree.select("a", root.key.clone(), false, false);
    assert!(tree.selected_entries("a").is_empty());
    tree.select("a", folder.key.clone(), false, false);
    tree.select("a", linked.key.clone(), false, true);
    assert_eq!(tree.selected_entries("a").len(), 2);
    tree.select_all("a");
    assert_eq!(tree.selected_entries("a").len(), 3);
}

#[test]
fn hiding_hidden_files_clears_their_tree_selection() {
    let mut tree = TransferTreeState::default();
    tree.seed(
        "a",
        FileBrowserBackendKind::Remote,
        RemoteFilePath::new("/"),
        Arc::new(vec![file(".secret", "/.secret")]),
    );
    tree.reveal(
        "a",
        FileBrowserBackendKind::Remote,
        RemoteFilePath::new("/"),
    );
    let shown = tree.presentation(Some("a"), true);
    let hidden = shown
        .rows
        .iter()
        .find(|row| row.label == ".secret")
        .unwrap();
    tree.select("a", hidden.key.clone(), false, false);
    assert_eq!(tree.selected_entries("a").len(), 1);

    let view = tree.presentation(Some("a"), false);
    assert!(!view.rows.iter().any(|row| row.label == ".secret"));
    assert!(tree.selected_entries("a").is_empty());
}
