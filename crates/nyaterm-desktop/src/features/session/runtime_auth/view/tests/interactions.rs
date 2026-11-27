use std::sync::mpsc;
use std::time::{Duration, Instant};

use gpui::{Entity, TestAppContext, VisualTestContext, point, px};
use nyaterm_core::SshKey;
use nyaterm_ui::NyaDialogWindowExt as _;

use super::{authentication, bounds, draw, hosted};
use crate::features::NyaTermApp;
use crate::features::session::runtime_auth::state::{AuthMethod, CredentialResponseSender};
use crate::test_support::{TestAppCleanup, TestConfigDir};

fn click(cx: &mut VisualTestContext, selector: &'static str) {
    let target = bounds(cx, selector).center();
    cx.simulate_click(target, Default::default());
    cx.run_until_parked();
}

fn install_key(app: &Entity<NyaTermApp>, cx: &mut VisualTestContext) {
    app.update(cx, |app, cx| {
        app.security.replace_catalog(
            vec![SshKey {
                id: "saved-key".into(),
                name: "Saved key".into(),
                sort_order: 0,
                key: None,
                cert: None,
                passphrase: None,
                key_file_path: None,
                cert_file_path: None,
                has_key_data: true,
                has_cert_data: false,
            }],
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );
        cx.notify();
    });
    draw(app, cx);
}

fn wait_for_editor_job(app: &Entity<NyaTermApp>, cx: &mut VisualTestContext) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        draw(app, cx);
        if !app.read_with(cx, |app, _| app.security.editor_busy()) {
            break;
        }
        assert!(Instant::now() < deadline, "key editor job did not finish");
        std::thread::sleep(Duration::from_millis(1));
    }
    draw(app, cx);
}

#[test]
fn key_editor_above_authentication_owns_pointer_keyboard_and_cancel() {
    let root = TestConfigDir::new("nyaterm-auth-key-editor-input");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let mut state = authentication(root.path());
    state.authentication.as_mut().unwrap().method = AuthMethod::Key;
    let (sender, responses) = mpsc::channel();
    state.response_tx = CredentialResponseSender::Authentication(sender);
    let (app, cx) = hosted(&mut cx, root.path(), state, (960., 800.));
    install_key(&app, cx);
    click(cx, "ssh-auth-manage-action");
    draw(&app, cx);
    cx.update(|window, cx| assert!(window.has_active_nya_dialog(cx)));

    click(cx, "nya-dialog-action-button");
    draw(&app, cx);
    app.read_with(cx, |app, _| {
        assert!(app.security.key_editor().unwrap().error.is_some())
    });
    click(cx, "security.editor.key-name");
    cx.simulate_input("New key");
    draw(&app, cx);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.session.prompt_restore_credential_focus();
            assert!(!app.drive_pending_focus(window, cx));
            assert!(app.session.prompt_credential_focus_is_pending());
            let field = app.existing_text_input("security.editor.key-name").unwrap();
            assert!(field.read(cx).component_focus_handle(cx).is_focused(window));
        })
    });

    let mode = bounds(cx, "security-key-material-mode");
    cx.simulate_click(
        point(mode.right() - px(20.), mode.center().y),
        Default::default(),
    );
    draw(&app, cx);
    app.read_with(cx, |app, _| {
        assert!(!app.security.key_editor().unwrap().key_content_mode)
    });
    cx.simulate_click(
        point(mode.left() + px(20.), mode.center().y),
        Default::default(),
    );
    draw(&app, cx);
    click(cx, "security.editor.key-data");
    cx.simulate_input("fixture-key-material");
    // A multiline field must keep Enter for editing, not submit either form.
    cx.simulate_keystrokes("enter");
    draw(&app, cx);
    app.read_with(cx, |app, _| {
        let editor = app.security.key_editor().unwrap();
        assert_eq!(editor.name, "New key");
        assert!(editor.key_data.expose_secret().contains('\n'));
        assert!(!app.security.editor_busy());
    });
    assert!(matches!(
        responses.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    click(cx, "nya-dialog-cancel-button");
    draw(&app, cx);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            assert!(!window.has_active_nya_dialog(cx));
            assert!(app.security.key_editor().is_none());
            assert!(app.drive_pending_focus(window, cx));
            assert!(app.select_is_focused("ssh-auth-key", window, cx));
            assert_eq!(app.session.prompt_active_credential().unwrap().id, "layout");
        })
    });
    assert!(matches!(
        responses.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
}

#[test]
fn key_editor_save_failure_keeps_draft_and_success_refreshes_authentication_choices() {
    let root = TestConfigDir::new("nyaterm-auth-key-editor-save");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let mut state = authentication(root.path());
    state.authentication.as_mut().unwrap().method = AuthMethod::Key;
    let (sender, responses) = mpsc::channel();
    state.response_tx = CredentialResponseSender::Authentication(sender);
    let (app, cx) = hosted(&mut cx, root.path(), state, (960., 800.));
    click(cx, "ssh-auth-manage-action");
    draw(&app, cx);
    click(cx, "security.editor.key-name");
    cx.simulate_input("New key");
    let mode = bounds(cx, "security-key-material-mode");
    cx.simulate_click(
        point(mode.right() - px(20.), mode.center().y),
        Default::default(),
    );
    draw(&app, cx);
    click(cx, "security.editor.key-path");
    cx.simulate_input(root.path().join("missing-key").to_str().unwrap());
    click(cx, "nya-dialog-action-button");
    wait_for_editor_job(&app, cx);
    app.read_with(cx, |app, _| {
        let editor = app.security.key_editor().unwrap();
        assert_eq!(editor.name, "New key");
        assert!(editor.error.is_some());
    });
    cx.update(|window, cx| assert!(window.has_active_nya_dialog(cx)));

    cx.simulate_click(
        point(mode.left() + px(20.), mode.center().y),
        Default::default(),
    );
    draw(&app, cx);
    click(cx, "security.editor.key-data");
    cx.simulate_input("fixture-key-material");
    click(cx, "nya-dialog-action-button");
    wait_for_editor_job(&app, cx);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            assert!(app.security.key_editor().is_none());
            assert!(!window.has_active_nya_dialog(cx));
            assert_eq!(app.security.ssh_keys().len(), 1);
            assert_eq!(app.security.ssh_keys()[0].name, "New key");
            let form = app
                .session
                .prompt_active_credential()
                .unwrap()
                .authentication
                .as_ref()
                .unwrap();
            assert!(
                form.key_id.is_none(),
                "saving must not select or submit credentials"
            );
            assert!(app.drive_pending_focus(window, cx));
            assert!(app.select_is_focused("ssh-auth-key", window, cx));
        })
    });
    assert!(matches!(
        responses.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    // Use the real popup and keyboard routing to choose the newly saved key.
    click(cx, "ssh-auth-credential-select");
    draw(&app, cx);
    cx.simulate_keystrokes("down enter");
    draw(&app, cx);
    app.read_with(cx, |app, _| {
        let form = app
            .session
            .prompt_active_credential()
            .unwrap()
            .authentication
            .as_ref()
            .unwrap();
        assert_eq!(
            form.key_id.as_deref(),
            Some(app.security.ssh_keys()[0].id.as_str())
        );
    });
    assert!(matches!(
        responses.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
}

#[test]
fn unlock_above_authentication_resumes_editing_without_restoring_an_expired_prompt() {
    let root = TestConfigDir::new("nyaterm-auth-key-editor-unlock");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let mut state = authentication(root.path());
    let form = state.authentication.as_mut().unwrap();
    form.method = AuthMethod::Key;
    form.key_id = Some("saved-key".into());
    let (app, cx) = hosted(&mut cx, root.path(), state, (960., 800.));
    install_key(&app, cx);
    app.update(cx, |app, _| {
        let mut summary = app.settings.summary().clone();
        summary.has_master_password = true;
        app.settings.replace_summary(summary);
        app.security.lock_secrets();
        // Make the post-unlock secret load fail deterministically.
        app.blocking_jobs.shutdown();
    });
    click(cx, "ssh-auth-manage-action");
    draw(&app, cx);
    click(cx, "security-unlock-input");
    cx.simulate_input("fixture-password");
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            assert!(app.security.unlock_prompt_open());
            assert!(!app.drive_pending_focus(window, cx));
            let generation = app.security.unlock_dialog_generation();
            let request = app.security.begin_unlock_request().unwrap().0;
            app.finish_security_unlock(
                request,
                generation,
                window.window_handle(),
                Ok(true),
                window,
                cx,
            );
        })
    });
    wait_for_editor_job(&app, cx);
    click(cx, "security.editor.key-name");
    cx.simulate_input(" changed");
    app.update(cx, |app, cx| {
        assert!(app.security.key_editor().unwrap().name.contains("changed"));
        app.session.prompts.take_credential();
        cx.notify();
    });
    draw(&app, cx);
    cx.simulate_keystrokes("escape");
    draw(&app, cx);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            assert!(app.security.key_editor().is_none());
            assert!(!window.has_active_nya_dialog(cx));
            assert!(!app.session.prompt_credential_focus_is_pending());
            assert!(!app.drive_pending_focus(window, cx));
        })
    });
}
