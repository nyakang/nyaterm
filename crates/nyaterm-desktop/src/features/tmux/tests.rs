use gpui::TestAppContext;
use nyaterm_core::test_support::TestTempDir;
use nyaterm_transport::tmux::{TmuxStatePayload, TmuxUiNode, TmuxWindowInfo};

use crate::features::test_support::app_with_visible_local_session;
use crate::models::{WorkspacePaneNode, WorkspaceSplitDirection};

fn state(host: &str, pane: u32) -> TmuxStatePayload {
    TmuxStatePayload {
        control_session_id: host.into(),
        exited: false,
        windows: vec![TmuxWindowInfo {
            id: "@0".into(),
            name: "window".into(),
            index: 0,
            active: true,
            tree: Some(TmuxUiNode::Pane {
                pane_id: format!("%{pane}"),
                session_id: format!("{host}::pane{pane}"),
                name: "pane".into(),
            }),
        }],
        active_window_id: Some("@0".into()),
        active_pane_session_id: Some(format!("{host}::pane{pane}")),
    }
}

#[test]
fn tmux_state_refresh_detach_and_reattach_preserve_manual_siblings_and_other_tabs() {
    let root = TestTempDir::new("nyaterm-tmux-workspace");
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "host");
    app.update(&mut cx, |app, cx| {
        app.session.mark_restore_complete();
        let metadata = app.session.metadata("host").unwrap().clone();
        for id in ["manual", "unrelated"] {
            app.session.register_session_metadata(id, metadata.clone());
            app.terminal.seed_session_view(id.into(), String::new(), "UTF-8");
        }
        app.shell.insert_workspace_pane_root("host".into(), WorkspacePaneNode::Split {
            id: "manual-divider".into(), direction: WorkspaceSplitDirection::Vertical, ratio_percent: 37,
            first: Box::new(WorkspacePaneNode::leaf("host")),
            second: Box::new(WorkspacePaneNode::leaf("manual")),
        });
        for pane in [0, 1] {
            app.handle_session_tmux_state_event("host".into(), state("host", pane), cx);
            let tree = app.shell.workspace_pane_root("host").unwrap();
            assert_eq!(tree.session_ids(), vec![format!("host::pane{pane}"), "manual".into()]);
            assert!(matches!(tree, WorkspacePaneNode::Split { id, ratio_percent: 37, .. } if id == "manual-divider"));
            let mut exit = state("host", pane);
            exit.exited = true;
            exit.windows.clear();
            app.handle_session_tmux_state_event("host".into(), exit, cx);
            assert_eq!(app.shell.workspace_pane_root("host").unwrap().session_ids(), vec!["host", "manual"]);
            assert!(app.session.has_session("unrelated"));
            assert!(app.shell.workspace.hidden_sessions.contains(&format!("host::pane{pane}")));
        }
    });
}

#[test]
fn detaching_one_tmux_host_does_not_steal_another_hosts_active_pane() {
    let root = TestTempDir::new("nyaterm-tmux-focus");
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, root.path(), "host");
    app.update(&mut cx, |app, cx| {
        let metadata = app.session.metadata("host").unwrap().clone();
        app.session.register_session_metadata("other", metadata);
        app.terminal
            .seed_session_view("other".into(), String::new(), "UTF-8");
        app.handle_session_tmux_state_event("host".into(), state("host", 0), cx);
        app.handle_session_tmux_state_event("other".into(), state("other", 1), cx);
        app.session.select_active_session("other::pane1");
    });
    cx.run_until_parked();
    app.update(&mut cx, |app, cx| {
        let mut exit = state("host", 0);
        exit.exited = true;
        exit.windows.clear();
        app.handle_session_tmux_state_event("host".into(), exit, cx);
    });
    cx.run_until_parked();
    app.read_with(&cx, |app, _| {
        assert_eq!(app.session.active_id(), Some("other::pane1"))
    });
}
