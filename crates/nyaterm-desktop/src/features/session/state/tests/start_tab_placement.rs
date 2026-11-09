use std::time::Instant;

use gpui::{AppContext as _, Context, Entity, TestAppContext};
use nyaterm_core::InternalEditorDisplay;
use nyaterm_transport::{SessionInfo, SessionKind};

use super::pending;
use crate::features::NyaTermApp;
use crate::features::runtime_jobs::{SessionStartResult, SessionStartSuccess};
use crate::features::session::{SavedConnectionStartOptions, SessionStartTabPlacement};
use crate::features::shell::session_tabs::SessionStripTabId;
use crate::features::test_support::{app_with_visible_local_session, editor_document};
use crate::models::{TerminalWindowNode, WorkspaceSplitDirection, WorkspaceTabId};
use crate::test_support::TestConfigDir;

fn app_with_three_tabs(cx: &mut TestAppContext, root: &TestConfigDir) -> Entity<NyaTermApp> {
    let app = app_with_visible_local_session(cx, root.path(), "a");
    cx.update_entity(&app, |app, _| {
        let metadata = app.session.metadata("a").unwrap().clone();
        app.session.register_session_metadata("b", metadata.clone());
        app.session.register_session_metadata("c", metadata);
        app.shell
            .restore_terminal_window_tree(Some(TerminalWindowNode::Leaf {
                id: "group".into(),
                tab_ids: vec!["a".into(), "b".into(), "c".into()],
                active_tab_id: Some("a".into()),
            }));
    });
    app
}

fn reserve_copy(app: &mut NyaTermApp, source: &str, request_id: &str) -> SessionStartTabPlacement {
    let options = app.prepare_session_start_options(SavedConnectionStartOptions {
        after_session_id: Some(source.into()),
        ..Default::default()
    });
    let placement = options.tab_placement.unwrap();
    let mut request = pending(request_id);
    request.after_session_id = options.after_session_id;
    request.tab_placement = Some(placement);
    request.launch_config = Some(app.session.metadata(source).unwrap().launch_config.clone());
    app.session
        .start
        .register_pending(request_id.into(), request);
    placement
}

fn complete_start(
    app: &mut NyaTermApp,
    request_id: &str,
    session_id: &str,
    cx: &mut Context<NyaTermApp>,
) {
    let config = app
        .session
        .start_pending_entries()
        .find(|(id, _)| *id == request_id)
        .and_then(|(_, pending)| pending.launch_config.clone())
        .unwrap();
    let now = Instant::now();
    app.apply_session_start_event(
        SessionStartResult {
            request_id: request_id.into(),
            connection_name: "copy".into(),
            kind: SessionKind::LocalPty,
            worker_started_at: now,
            worker_finished_at: now,
            result: Ok(SessionStartSuccess {
                session_info: SessionInfo {
                    id: session_id.into(),
                    name: "copy".into(),
                    kind: SessionKind::LocalPty,
                    working_dir: None,
                    cols: 80,
                    rows: 24,
                },
                multiplex_handle: None,
                launch_config: Some(config),
                cleanup: None,
            }),
        },
        cx,
    );
}

fn strip_keys(app: &NyaTermApp, group: &str) -> Vec<String> {
    app.session_strip_tabs(&Some(group.into()))
        .iter()
        .map(|tab| tab.id.layout_key())
        .collect()
}

#[test]
fn copied_session_replaces_its_reserved_slot_after_first_middle_or_last_source() {
    for (source, index) in [("a", 1), ("b", 2), ("c", 3)] {
        let root = TestConfigDir::new("nyaterm-copy-tab-position");
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app_with_three_tabs(&mut cx, &root);
        cx.update_entity(&app, |app, cx| {
            let placement = reserve_copy(app, source, "copy");
            let key = SessionStripTabId::Start(placement.request_sequence).layout_key();
            let mut expected = vec!["a".into(), "b".into(), "c".into()];
            expected.insert(index, key);
            assert_eq!(strip_keys(app, "group"), expected);
            complete_start(app, "copy", "connected", cx);
            expected[index] = "connected".into();
            assert_eq!(strip_keys(app, "group"), expected);
            assert_eq!(app.workspace_tab_ids(), expected);
            assert_eq!(app.session.active_id(), Some("connected"));
        });
    }
}

#[test]
fn copying_uses_the_source_group_visible_order_including_editor_and_pending_tabs() {
    let root = TestConfigDir::new("nyaterm-copy-group-position");
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app = app_with_three_tabs(&mut cx, &root);
    cx.update_entity(&app, |app, cx| {
        let document = editor_document("b", "/editor.txt", InternalEditorDisplay::Workspace);
        let editor = WorkspaceTabId::Editor(document.id.clone()).layout_key();
        app.transfer.open_editor_tab(document);
        app.shell
            .restore_terminal_window_tree(Some(TerminalWindowNode::Split {
                id: "split".into(),
                direction: WorkspaceSplitDirection::Vertical,
                ratio_percent: 50,
                first: Box::new(TerminalWindowNode::Leaf {
                    id: "left".into(),
                    tab_ids: vec!["a".into()],
                    active_tab_id: Some("a".into()),
                }),
                second: Box::new(TerminalWindowNode::Leaf {
                    id: "right".into(),
                    tab_ids: vec![editor.clone(), "b".into(), "c".into()],
                    active_tab_id: Some("b".into()),
                }),
            }));
        app.shell.workspace.focused_group = Some("left".into());
        let earlier = app.session.start.allocate_tab_placement(0);
        let mut request = pending("earlier");
        request.tab_placement = Some(earlier);
        app.session
            .start
            .register_pending("earlier".into(), request);
        app.shell
            .reserve_terminal_start_group(earlier.request_sequence, "right".into());
        let earlier_key = SessionStripTabId::Start(earlier.request_sequence).layout_key();
        let placement = reserve_copy(app, "b", "copy");
        assert_eq!(placement.insert_index, 3);
        assert_eq!(
            app.shell.terminal_start_group(placement.request_sequence),
            Some("right")
        );
        let copy_key = SessionStripTabId::Start(placement.request_sequence).layout_key();
        assert_eq!(
            strip_keys(app, "right"),
            [
                earlier_key.clone(),
                editor.clone(),
                "b".into(),
                copy_key,
                "c".into()
            ]
        );
        app.activate_session_id("a", cx);
        complete_start(app, "copy", "connected", cx);
        assert_eq!(
            strip_keys(app, "right"),
            [
                earlier_key,
                editor,
                "b".into(),
                "connected".into(),
                "c".into()
            ]
        );
        assert_eq!(strip_keys(app, "left"), ["a"]);
        assert_eq!(app.session.active_id(), Some("a"));
    });
}

#[test]
fn concurrent_copies_keep_reserved_order_for_every_worker_completion_order() {
    for completions in [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ] {
        let root = TestConfigDir::new("nyaterm-concurrent-copy-position");
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app_with_three_tabs(&mut cx, &root);
        cx.update_entity(&app, |app, cx| {
            let requests = ["first", "second", "third"];
            let keys = requests.map(|id| {
                let placement = reserve_copy(app, "b", id);
                SessionStripTabId::Start(placement.request_sequence).layout_key()
            });
            let mut expected = strip_keys(app, "group");
            for index in completions {
                complete_start(app, requests[index], requests[index], cx);
                let slot = expected.iter().position(|key| key == &keys[index]).unwrap();
                expected[slot] = requests[index].into();
                assert_eq!(
                    strip_keys(app, "group"),
                    expected,
                    "completion order {completions:?}"
                );
            }
            assert_eq!(app.workspace_tab_ids(), expected);
        });
    }
}

#[test]
fn starts_with_the_same_reserved_index_keep_request_order_on_reverse_completion() {
    let root = TestConfigDir::new("nyaterm-copy-request-order");
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app = app_with_three_tabs(&mut cx, &root);
    cx.update_entity(&app, |app, cx| {
        for id in ["first", "second"] {
            let placement = app.session.start.allocate_tab_placement(1);
            let mut request = pending(id);
            request.tab_placement = Some(placement);
            request.launch_config = Some(app.session.metadata("a").unwrap().launch_config.clone());
            app.session.start.register_pending(id.into(), request);
            app.shell
                .reserve_terminal_start_group(placement.request_sequence, "group".into());
        }
        complete_start(app, "second", "second", cx);
        complete_start(app, "first", "first", cx);
        assert_eq!(app.workspace_tab_ids(), ["a", "first", "second", "b", "c"]);
    });
}

#[test]
fn completing_a_dragged_copy_preserves_its_latest_slot() {
    for index in [0, 3] {
        let root = TestConfigDir::new("nyaterm-dragged-copy-position");
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app_with_three_tabs(&mut cx, &root);
        cx.update_entity(&app, |app, cx| {
            let placement = reserve_copy(app, "b", "copy");
            let key = SessionStripTabId::Start(placement.request_sequence).layout_key();
            let mut expected = vec!["a".into(), "b".into(), "c".into()];
            expected.insert(index, key.clone());
            app.commit_session_strip_order(&Some("group".into()), &expected, &key, cx);
            complete_start(app, "copy", "connected", cx);
            expected[index] = "connected".into();
            assert_eq!(app.workspace_tab_ids(), expected);
        });
    }
}

#[test]
fn closing_the_source_before_copy_completion_keeps_the_remaining_reserved_slot() {
    let root = TestConfigDir::new("nyaterm-closed-copy-source");
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app = app_with_three_tabs(&mut cx, &root);
    cx.update_entity(&app, |app, cx| {
        reserve_copy(app, "b", "copy");
        app.session.remove_session_catalog("b");
        app.reconcile_terminal_windows();
        complete_start(app, "copy", "connected", cx);
        assert_eq!(app.workspace_tab_ids(), ["a", "connected", "c"]);
    });
}

#[test]
fn closing_the_destination_group_before_completion_uses_the_display_fallback_group() {
    let root = TestConfigDir::new("nyaterm-closed-copy-group");
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app = app_with_three_tabs(&mut cx, &root);
    cx.update_entity(&app, |app, cx| {
        reserve_copy(app, "b", "copy");
        app.session.remove_session_catalog("b");
        app.session.remove_session_catalog("c");
        app.shell
            .restore_terminal_window_tree(Some(TerminalWindowNode::Leaf {
                id: "remaining".into(),
                tab_ids: vec!["a".into()],
                active_tab_id: Some("a".into()),
            }));
        complete_start(app, "copy", "connected", cx);
        assert_eq!(strip_keys(app, "remaining"), ["a", "connected"]);
        assert_eq!(
            app.shell.terminal_group_for_tab("connected"),
            Some("remaining".into())
        );
    });
}
