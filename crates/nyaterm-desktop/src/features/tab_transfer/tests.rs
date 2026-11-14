use gpui::TestAppContext;
use nyaterm_core::{AiExecutionProfile, MoveTabPlacement, test_support::TestTempDir};
use nyaterm_transport::LocalSessionConfig;

use crate::features::test_support::app_with_visible_local_session;
use crate::models::{
    SessionLaunchConfig, SessionRuntimeMetadata, WorkspacePaneNode, WorkspaceSplitDirection,
};

#[test]
fn tab_transfer_preserves_split_tree_terminal_contents_and_active_session() {
    let root = TestTempDir::new("nyaterm-tab-transfer");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let target_root = TestTempDir::new("nyaterm-tab-transfer-target");
    let source = app_with_visible_local_session(&mut cx, root.path(), "tab");
    let target = app_with_visible_local_session(&mut cx, target_root.path(), "existing");
    let tree = WorkspacePaneNode::Split {
        id: "split".into(),
        direction: WorkspaceSplitDirection::Vertical,
        ratio_percent: 60,
        first: Box::new(WorkspacePaneNode::leaf("tab")),
        second: Box::new(WorkspacePaneNode::leaf("secondary")),
    };
    source.update(&mut cx, |app, _| {
        app.session.register_session_metadata(
            "secondary",
            SessionRuntimeMetadata {
                confirmed_ssh_runtime_mode: None,
                pending_ssh_startup_command: None,
                ssh_config: None,
                ssh_multiplex_key: None,
                source_connection_id: None,
                ai_execution_profile: AiExecutionProfile::Posix,
                launch_config: SessionLaunchConfig::Local(LocalSessionConfig::default()),
                disconnected: false,
            },
        );
        app.terminal
            .seed_session_view("tab".into(), "primary contents".into(), "UTF-8");
        app.terminal
            .seed_session_view("secondary".into(), "secondary contents".into(), "UTF-8");
        app.shell
            .insert_workspace_pane_root("tab".into(), tree.clone());
        app.rebuild_session_tab_owners();
    });
    let bundle = source.update(&mut cx, |app, cx| {
        app.detach_tab_tree_for_transfer("tab", app.workspace_revision(), cx)
            .unwrap()
    });
    assert_eq!(bundle.session_ids, ["tab", "secondary"]);
    target.update(&mut cx, |app, cx| {
        assert!(
            app.attach_tab_tree_from_transfer(bundle, &MoveTabPlacement::Append, cx)
                .is_ok()
        );
        assert_eq!(app.session.active_id(), Some("tab"));
        assert_eq!(app.shell.workspace_pane_root("tab"), Some(&tree));
        assert_eq!(app.terminal.session_output("tab"), Some("primary contents"));
        assert_eq!(
            app.terminal.session_output("secondary"),
            Some("secondary contents")
        );
    });
    source.update(&mut cx, |app, _| {
        assert!(!app.session.has_session("tab"));
        assert!(!app.session.has_session("secondary"));
        assert!(app.shell.workspace_pane_root("tab").is_none());
    });
}

#[test]
fn rejected_transfer_rolls_back_contents_order_and_active_tab() {
    let root = TestTempDir::new("nyaterm-tab-transfer-rollback");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let target_root = TestTempDir::new("nyaterm-tab-transfer-rollback-target");
    let source = app_with_visible_local_session(&mut cx, root.path(), "tab");
    let target = app_with_visible_local_session(&mut cx, target_root.path(), "tab");
    source.update(&mut cx, |app, _| {
        app.terminal
            .seed_session_view("tab".into(), "retained contents".into(), "UTF-8");
    });
    let bundle = source.update(&mut cx, |app, cx| {
        app.detach_tab_tree_for_transfer("tab", app.workspace_revision(), cx)
            .unwrap()
    });
    let (error, bundle) = *target.update(&mut cx, |app, cx| {
        app.attach_tab_tree_from_transfer(bundle, &MoveTabPlacement::Append, cx)
            .err()
            .unwrap()
    });
    assert!(error.contains("already contains"));
    source.update(&mut cx, |app, cx| {
        app.restore_tab_tree_after_failed_transfer(bundle, cx)
            .unwrap();
        assert_eq!(app.session.active_id(), Some("tab"));
        assert_eq!(app.session.session_order(), &["tab"]);
        assert_eq!(
            app.terminal.session_output("tab"),
            Some("retained contents")
        );
    });
}

#[test]
fn tab_transfer_rejects_stale_revision_and_missing_tab_without_mutating_source() {
    let root = TestTempDir::new("nyaterm-tab-transfer-preflight");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let source = app_with_visible_local_session(&mut cx, root.path(), "tab");
    source.update(&mut cx, |app, cx| {
        let revision = app.workspace_revision();
        assert!(
            app.detach_tab_tree_for_transfer("tab", revision + 1, cx)
                .is_err()
        );
        assert!(
            app.detach_tab_tree_for_transfer("missing", revision, cx)
                .is_err()
        );
        assert!(app.session.has_session("tab"));
        assert_eq!(app.session.active_id(), Some("tab"));
    });
}
