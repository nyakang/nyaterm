use std::path::Path;

use gpui::{
    AppContext as _, Context, Entity, FocusHandle, InteractiveElement as _, IntoElement, Render,
    Styled as _, TestAppContext, VisualTestContext, Window, div,
};
use nyaterm_core::test_support::TestTempDir;
use nyaterm_ui::{NyaDialogWindowExt as _, NyaInputKind, nya_root};

use crate::features::NyaTermApp;
use crate::features::test_support::app_with_visible_local_session;

struct Workspace {
    focus: FocusHandle,
}

impl Render for Workspace {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().track_focus(&self.focus)
    }
}

fn hosted<'a>(
    cx: &'a mut TestAppContext,
    root: &Path,
) -> (Entity<NyaTermApp>, &'a mut VisualTestContext, FocusHandle) {
    // The fixture uses real store and blocking-job threads.
    cx.executor().allow_parking();
    let app = app_with_visible_local_session(cx, root, "windterm-import-fixture");
    app.update(cx, |app, cx| app.sync_component_theme(cx));
    let mut focus = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
        let workspace = cx.new(|cx| Workspace {
            focus: cx.focus_handle(),
        });
        focus = Some(workspace.read(cx).focus.clone());
        nya_root(workspace, window, cx)
    });
    let focus = focus.unwrap();
    cx.update(|window, cx| window.focus(&focus, cx));
    (app, cx, focus)
}

fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
}

#[gpui::test]
fn windterm_password_prompt_masks_focuses_and_cancels_without_writing(cx: &mut TestAppContext) {
    let root = TestTempDir::new("nyaterm-windterm-dialog-cancel");
    let (app, cx, focus) = hosted(cx, root.path());
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.connection_state
                .await_windterm_password(root.join("user.sessions"));
            app.open_windterm_password_dialog(window, cx);
        })
    });
    draw(cx);
    let field = cx.update(|window, cx| {
        let field = app
            .read(cx)
            .existing_text_input("windterm-password.value")
            .unwrap();
        assert!(field.read(cx).is_masked());
        assert_eq!(field.read(cx).input_kind(), NyaInputKind::Secret);
        assert!(field.read(cx).component_focus_handle(cx).is_focused(window));
        field
    });
    cx.simulate_input("fixture master password");
    draw(cx);
    cx.simulate_keystrokes("escape");
    draw(cx);
    cx.update(|window, cx| {
        assert!(!window.has_active_nya_dialog(cx));
        assert!(focus.is_focused(window));
        assert_eq!(field.read(cx).value(cx), "");
        app.update(cx, |app, _| {
            assert!(!app.connection_state.import_path_prompt_active());
            assert!(app.existing_text_input("windterm-password.value").is_none());
            assert!(
                app.store_blocking_client()
                    .request_fn(nyaterm_store::StoreDomain::Connections, |store| store
                        .list_connections())
                    .unwrap()
                    .is_empty()
            );
        });
    });
}

#[gpui::test]
fn windterm_password_prompt_rejects_empty_enter_and_cleans_up_after_submit(
    cx: &mut TestAppContext,
) {
    let root = TestTempDir::new("nyaterm-windterm-dialog-submit");
    std::fs::create_dir_all(root.join("profile/terminal")).unwrap();
    let path = root.join("profile/terminal/user.sessions");
    std::fs::write(
        root.join("profile/user.config"),
        r#"{"application.masterPassword":true,"application.fingerprint":"fingerprint-1"}"#,
    )
    .unwrap();
    std::fs::write(&path, r#"[{"session.protocol":"SSH","session.target":"fixture.example","session.autoLogin":"xCSsthKpnyjEgvx1zOw2rU6noHJj5/EtRQj0+Z4incUd4D9Szbg+1/07c8Sf+IuiVDh7+GQ80spldkp3BHSui4RJyFZkZOaJYKg4gKwGqP0="}]"#).unwrap();
    let (app, cx, _) = hosted(cx, root.path());
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.connection_state.await_windterm_password(path.clone());
            app.open_windterm_password_dialog(window, cx);
        })
    });
    draw(cx);
    cx.simulate_keystrokes("enter");
    draw(cx);
    cx.update(|window, cx| {
        assert!(window.has_active_nya_dialog(cx));
        assert!(app.read(cx).connection_state.awaiting_windterm_password());
    });
    cx.simulate_input(" master-secret ");
    draw(cx);
    let field = app.read_with(cx, |app, _| {
        app.existing_text_input("windterm-password.value").unwrap()
    });
    cx.simulate_keystrokes("enter");
    draw(cx);
    cx.update(|window, cx| {
        assert!(!window.has_active_nya_dialog(cx));
        assert_eq!(field.read(cx).value(cx), "");
        assert!(
            app.read(cx)
                .existing_text_input("windterm-password.value")
                .is_none()
        );
        assert!(!app.read(cx).connection_state.awaiting_windterm_password());
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        cx.run_until_parked();
        if !app.read_with(cx, |app, _| {
            app.connection_state.import_path_prompt_active()
        }) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "WindTerm import did not finish"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    app.update(cx, |app, _| {
        let sessions = app
            .store_blocking_client()
            .request_fn(nyaterm_store::StoreDomain::Connections, |store| {
                store.load_sessions()
            })
            .unwrap();
        assert_eq!(sessions.connections.len(), 1);
        assert_eq!(
            sessions.connections[0]
                .auth
                .as_ref()
                .unwrap()
                .password
                .as_deref(),
            Some(" secret ")
        );
    });
}
