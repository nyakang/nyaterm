use std::{cell::Cell, rc::Rc, time::Duration};

use futures::channel::oneshot;
use gpui::{
    AnyWindowHandle, AppContext as _, Context, Entity, IntoElement, Render, TestAppContext,
    VisualTestContext, Window, div,
};
use nyaterm_core::{SavedConnection, test_support::TestTempDir};
use nyaterm_ui::{NyaDialogWindowExt as _, nya_root};

use crate::features::NyaTermApp;
use crate::features::test_support::app_with_visible_local_session;
use crate::models::{ConnectionEditorField, ConnectionPasswordEdit};

type UnlockRequest = (u64, u64, AnyWindowHandle);
type PendingResult = (
    oneshot::Sender<Result<bool, String>>,
    Rc<Cell<Option<bool>>>,
);

struct ReadsApp(Entity<NyaTermApp>);

impl Render for ReadsApp {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _ = self.0.read(cx).shell.main_window();
        div()
    }
}

fn hosted_editor(
    cx: &mut TestAppContext,
    root: &std::path::Path,
) -> (Entity<NyaTermApp>, VisualTestContext, AnyWindowHandle) {
    let app = app_with_visible_local_session(cx, root, "unlock-origin-fixture");
    app.update(cx, |app, cx| app.sync_component_theme(cx));
    let host_app = app.clone();
    let main: AnyWindowHandle = cx
        .add_window(move |window, cx| {
            let host = cx.new(|_| ReadsApp(host_app));
            nya_root(host, window, cx)
        })
        .into();
    let saved: SavedConnection = serde_json::from_value(serde_json::json!({
        "id": "saved",
        "name": "Saved connection",
        "type": "ssh",
        "host": "example.test",
        "username": "tester",
        "auth": { "mode": "password", "password": " saved fixture " }
    }))
    .unwrap();
    main.update(cx, |_, window, cx| {
        app.update(cx, |app, cx| {
            app.shell.remember_main_window(main.downcast().unwrap());
            let mut summary = app.settings.summary().clone();
            summary.has_master_password = true;
            app.settings.replace_summary(summary);
            // Keep the original in the editor so this test exercises window
            // routing without depending on real worker-thread wakeups.
            app.security.complete_unlock();
            app.connection_state.replace_loaded(vec![saved], Vec::new());
            app.open_connection_editor(Some("saved".into()), None, false, window, cx);
        });
        window.open_nya_dialog(cx, |dialog, _, _| dialog.title("Main window dialog"));
    })
    .unwrap();
    cx.run_until_parked();
    let child = app
        .read_with(cx, |app, _| app.connection_state.editor_window_handle())
        .unwrap();
    let mut editor = VisualTestContext::from_window(child.into(), cx);
    draw(&mut editor);
    app.update(cx, |app, _| app.security.lock_secrets());
    (app, editor, main)
}

fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
}

fn begin_reveal(app: &Entity<NyaTermApp>, cx: &mut VisualTestContext) -> UnlockRequest {
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.toggle_connection_password_visibility(window, cx);
            assert!(app.security.unlock_prompt_open());
            let request = app.security.begin_unlock_request().unwrap().0;
            (
                request,
                app.security.unlock_dialog_generation(),
                window.window_handle(),
            )
        })
    })
}

fn pending_result(
    app: &Entity<NyaTermApp>,
    cx: &mut TestAppContext,
    (request, generation, origin): UnlockRequest,
) -> PendingResult {
    let (sender, receiver) = oneshot::channel();
    let delivered = Rc::new(Cell::new(None));
    let result_delivered = delivered.clone();
    app.update(cx, |_, cx| {
        cx.spawn(async move |this, cx| {
            let result = receiver.await.unwrap();
            result_delivered.set(Some(
                NyaTermApp::dispatch_security_unlock_result(
                    &this, cx, request, generation, origin, result,
                )
                .is_ok(),
            ));
        })
        .detach();
    });
    (sender, delivered)
}

fn draw_main(main: AnyWindowHandle, cx: &mut TestAppContext) {
    main.update(cx, |_, window, cx| {
        let _ = window.draw(cx);
    })
    .unwrap();
}

fn assert_saved_password_unchanged(app: &Entity<NyaTermApp>, cx: &TestAppContext) {
    app.read_with(cx, |app, _| {
        let saved = app.connection_state.connection_by_id("saved").unwrap();
        assert_eq!(
            saved.auth.as_ref().unwrap().password.as_deref(),
            Some(" saved fixture ")
        );
        let draft = app.connection_state.active_editor_draft().unwrap();
        assert_eq!(draft.password_edit, ConnectionPasswordEdit::Keep);
        assert_eq!(draft.existing_password.as_deref(), Some(" saved fixture "));
    });
}

#[gpui::test]
fn security_unlock_child_window_failure_then_success_reveals_the_original_password(
    cx: &mut TestAppContext,
) {
    let root = TestTempDir::new("nyaterm-unlock-child-result");
    let (app, mut editor, main) = hosted_editor(cx, root.path());
    let request = begin_reveal(&app, &mut editor);
    draw(&mut editor);
    let (sender, delivered) = pending_result(&app, cx, request);
    draw_main(main, cx);
    sender.send(Ok(false)).unwrap();
    draw(&mut editor);
    assert_eq!(delivered.get(), Some(true));
    editor.update(|window, cx| {
        let app = app.read(cx);
        assert!(!app.security.unlock_busy());
        assert!(app.security.unlock_error().is_some());
        assert!(app.security_secrets_locked());
        let field = app.existing_text_input("security.unlock.password").unwrap();
        assert!(field.read(cx).is_masked());
        assert!(field.read(cx).component_focus_handle(cx).is_focused(window));
        assert!(
            app.connection_state.editor_fields()[&ConnectionEditorField::Password]
                .read(cx)
                .is_masked()
        );
    });
    assert_saved_password_unchanged(&app, cx);
    editor.simulate_keystrokes("r e t r y");
    draw(&mut editor);
    assert_eq!(
        app.read_with(cx, |app, cx| app
            .existing_text_input("security.unlock.password")
            .unwrap()
            .read(cx)
            .value(cx)),
        "retry"
    );

    let request = editor.update(|window, cx| {
        app.update(cx, |app, _| {
            (
                app.security.begin_unlock_request().unwrap().0,
                app.security.unlock_dialog_generation(),
                window.window_handle(),
            )
        })
    });
    let (sender, _) = pending_result(&app, cx, request);
    draw_main(main, cx);
    sender.send(Ok(true)).unwrap();
    draw(&mut editor);
    editor.update(|window, cx| {
        assert!(!window.has_active_nya_dialog(cx));
        let app = app.read(cx);
        assert!(app.security.secrets_unlocked());
        assert!(!app.security.unlock_prompt_open());
        let input = &app.connection_state.editor_fields()[&ConnectionEditorField::Password];
        assert!(!input.read(cx).is_masked());
        assert_eq!(input.read(cx).value(cx), " saved fixture ");
    });
    let (duplicate, _) = pending_result(&app, cx, request);
    duplicate.send(Ok(true)).unwrap();
    draw(&mut editor);
    main.update(cx, |_, window, cx| {
        assert!(window.has_active_nya_dialog(cx))
    })
    .unwrap();
    assert_saved_password_unchanged(&app, cx);
}

#[gpui::test]
fn security_unlock_child_window_reports_rejected_verification_jobs(cx: &mut TestAppContext) {
    let root = TestTempDir::new("nyaterm-unlock-child-rejected");
    let (app, mut editor, main) = hosted_editor(cx, root.path());
    app.update(cx, |app, _| app.blocking_jobs.shutdown());
    editor.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.toggle_connection_password_visibility(window, cx)
        });
    });
    draw(&mut editor);
    editor.update(|window, cx| {
        app.update(cx, |app, cx| app.submit_security_unlock(window, cx));
    });
    draw_main(main, cx);
    draw(&mut editor);
    app.read_with(cx, |app, _| {
        assert!(!app.security.unlock_busy());
        assert!(app.security.unlock_prompt_open());
        assert!(
            app.security
                .unlock_error()
                .unwrap()
                .contains("shutting down")
        );
        assert!(app.security_secrets_locked());
    });
    editor.simulate_keystrokes("r e t r y");
    draw(&mut editor);
    assert_eq!(
        app.read_with(cx, |app, cx| app
            .existing_text_input("security.unlock.password")
            .unwrap()
            .read(cx)
            .value(cx)),
        "retry"
    );
    assert_saved_password_unchanged(&app, cx);
}

#[gpui::test]
fn security_unlock_child_window_retries_in_its_origin_after_a_covering_dialog_closes(
    cx: &mut TestAppContext,
) {
    let root = TestTempDir::new("nyaterm-unlock-child-covered");
    let (app, mut editor, main) = hosted_editor(cx, root.path());
    let request = begin_reveal(&app, &mut editor);
    draw(&mut editor);
    editor.update(|window, cx| {
        window.open_nya_dialog(cx, |dialog, _, _| dialog.title("Covering dialog"));
    });
    let (sender, delivered) = pending_result(&app, cx, request);
    sender.send(Ok(true)).unwrap();
    // Tick runnable work without advancing the retry timer while it is covered.
    for _ in 0..100 {
        if delivered.get().is_some() {
            break;
        }
        cx.executor().tick();
    }
    assert_eq!(delivered.get(), Some(true));
    app.read_with(cx, |app, _| {
        assert!(app.security.unlock_busy());
        assert!(app.security_secrets_locked());
    });
    editor.update(|window, cx| window.close_nya_dialog(cx));
    draw_main(main, cx);
    cx.executor().advance_clock(Duration::from_millis(50));
    draw(&mut editor);
    app.read_with(cx, |app, cx| {
        assert!(app.security.secrets_unlocked());
        assert!(!app.security.unlock_busy());
        assert!(
            !app.connection_state.editor_fields()[&ConnectionEditorField::Password]
                .read(cx)
                .is_masked()
        );
    });
    main.update(cx, |_, window, cx| {
        assert!(window.has_active_nya_dialog(cx))
    })
    .unwrap();
    assert_saved_password_unchanged(&app, cx);
}

#[gpui::test]
fn security_unlock_child_window_ignores_a_cancelled_result_after_a_new_request_starts(
    cx: &mut TestAppContext,
) {
    let root = TestTempDir::new("nyaterm-unlock-child-cancelled");
    let (app, mut editor, main) = hosted_editor(cx, root.path());
    let request = begin_reveal(&app, &mut editor);
    draw(&mut editor);
    let (old_result, delivered) = pending_result(&app, cx, request);
    editor.simulate_keystrokes("escape");
    draw(&mut editor);
    assert!(!app.read_with(cx, |app, _| app.security.unlock_prompt_open()));
    let current = begin_reveal(&app, &mut editor);
    draw_main(main, cx);
    old_result.send(Ok(true)).unwrap();
    draw(&mut editor);
    assert_eq!(delivered.get(), Some(true));
    app.read_with(cx, |app, _| {
        assert!(app.security.unlock_busy());
        assert!(app.security_secrets_locked());
        assert!(app.security.unlock_dialog_matches(current.1, current.2));
    });
    assert_saved_password_unchanged(&app, cx);
    let (result, _) = pending_result(&app, cx, current);
    result.send(Ok(true)).unwrap();
    draw(&mut editor);
    assert!(app.read_with(cx, |app, _| app.security.secrets_unlocked()));
}

#[gpui::test]
fn security_unlock_child_window_ignores_a_result_after_its_dialog_is_replaced(
    cx: &mut TestAppContext,
) {
    let root = TestTempDir::new("nyaterm-unlock-child-replaced");
    let (app, mut editor, main) = hosted_editor(cx, root.path());
    let request = begin_reveal(&app, &mut editor);
    draw(&mut editor);
    let (sender, _) = pending_result(&app, cx, request);
    editor.update(|window, cx| {
        window.close_nya_dialog(cx);
        window.open_nya_dialog(cx, |dialog, _, _| dialog.title("Replacement"));
    });
    draw_main(main, cx);
    sender.send(Ok(true)).unwrap();
    draw(&mut editor);
    assert!(app.read_with(cx, |app, _| app.security_secrets_locked()));
    assert!(!app.read_with(cx, |app, _| app.security.unlock_prompt_open()));
    editor.update(|window, cx| assert!(window.has_active_nya_dialog(cx)));
    assert_saved_password_unchanged(&app, cx);
}

#[gpui::test]
fn security_unlock_child_window_drops_a_result_when_the_origin_window_is_closed(
    cx: &mut TestAppContext,
) {
    let root = TestTempDir::new("nyaterm-unlock-child-closed");
    let (app, mut editor, main) = hosted_editor(cx, root.path());
    let request = begin_reveal(&app, &mut editor);
    draw(&mut editor);
    let (sender, delivered) = pending_result(&app, cx, request);
    editor.update(|window, _| window.remove_window());
    cx.run_until_parked();
    draw_main(main, cx);
    sender.send(Ok(true)).unwrap();
    cx.run_until_parked();
    assert_eq!(delivered.get(), Some(false));
    app.read_with(cx, |app, _| {
        assert!(app.security_secrets_locked());
        assert!(!app.security.unlock_prompt_open());
        assert!(!app.security.unlock_busy());
    });
    main.update(cx, |_, window, cx| {
        assert!(window.has_active_nya_dialog(cx))
    })
    .unwrap();
    assert_saved_password_unchanged(&app, cx);
}
