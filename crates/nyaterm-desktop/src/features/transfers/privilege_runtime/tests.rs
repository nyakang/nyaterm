use super::PASSWORD_INPUT;
use crate::features::NyaTermApp;
use crate::features::session::remote_files::tests::{Connection, connection};
use crate::features::session::remote_files::{FileAccessPurpose, FilePrivilegeStatus};
use crate::features::test_support::app_with_visible_ssh_session;
use crate::models::{TransferJobKind, TransferJobState, TransferJobStatus};
use gpui::{
    AppContext as _, Context, Entity, IntoElement as _, Render, TestAppContext, VisualTestContext,
    Window, div,
};
use nyaterm_core::test_support::TestTempDir;
use nyaterm_ui::{NyaDialogWindowExt as _, NyaInputShell, NyaInputState, nya_root};

struct Host;
impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        div()
    }
}
fn hosted<'a>(
    cx: &'a mut TestAppContext,
    root: &std::path::Path,
    connection: &Connection,
) -> (Entity<NyaTermApp>, &'a mut VisualTestContext) {
    let app = app_with_visible_ssh_session(cx, root, "sudo-test");
    app.update(cx, |app, cx| {
        app.sync_component_theme(cx);
        let metadata = app.session.metadata_mut("sudo-test").unwrap();
        metadata.ssh_config = Some(connection.config.clone());
        metadata.ssh_multiplex_key = Some("sudo-test-key".into());
        let lease = app
            .session
            .ssh_connection_pool()
            .register("sudo-test-key".into(), connection.handle.clone());
        app.session
            .register_ssh_connection("sudo-test".into(), lease);
    });
    let (_, cx) = cx.add_window_view(|window, cx| {
        let host = cx.new(|_| Host);
        nya_root(host, window, cx)
    });
    (app, cx)
}
fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
}

#[gpui::test]
fn cancelling_sudo_prompt_clears_password_and_restores_the_underlying_field(
    cx: &mut TestAppContext,
) {
    let root = TestTempDir::new("nyaterm-sudo-prompt");
    let connection = connection();
    let (app, cx) = hosted(cx, root.path(), &connection);
    let original = cx.update(|window, cx| {
        let field = cx.new(|cx| NyaInputState::new(cx, "unsaved draft"));
        let input = field.clone();
        app.update(cx, |app, cx| {
            app.open_form_dialog(
                (
                    "Editor".into(),
                    400.,
                    "Save".into(),
                    move |_, _, _| NyaInputShell::new("sudo-original", &input).into_any_element(),
                    |_, _, _| false,
                    |_, _| {},
                ),
                window,
                cx,
            );
        });
        window.focus(&field.read(cx).focus_handle(), cx);
        field
    });
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.toggle_file_sudo("sudo-test".into(), window, cx)
        })
    });
    draw(cx);
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            let field = app.existing_text_input(PASSWORD_INPUT).unwrap();
            assert!(field.read(cx).is_masked());
            field.update(cx, |field, cx| {
                field.set_content_silent("synthetic fixture input", cx)
            });
        })
    });
    cx.simulate_keystrokes("escape");
    draw(cx);
    cx.update(|window, cx| {
        app.read_with(cx, |app, _| {
            assert!(app.transfer.sudo_prompt.is_none());
            assert!(app.existing_text_input(PASSWORD_INPUT).is_none());
            assert_eq!(
                app.file_sudo_status("sudo-test"),
                FilePrivilegeStatus::Normal
            );
        });
        assert!(window.has_active_nya_dialog(cx));
        assert_eq!(original.read(cx).value(cx), "unsaved draft");
        assert!(
            original
                .read(cx)
                .component_focus_handle(cx)
                .is_focused(window)
        );
    });
}

#[gpui::test]
fn active_queued_paused_and_cancelling_file_jobs_block_privilege_changes(cx: &mut TestAppContext) {
    let root = TestTempDir::new("nyaterm-sudo-busy");
    let connection = connection();
    let (app, cx) = hosted(cx, root.path(), &connection);
    cx.update(|_, cx| {
        app.update(cx, |app, _| {
            app.transfer.enqueue_transfer_job(TransferJobState {
                id: "active".into(),
                session_id: Some("sudo-test".into()),
                kind: TransferJobKind::Download {
                    remote_path: "/file".into(),
                    raw_path_token: None,
                    local_path: "/unused".into(),
                },
                status: TransferJobStatus::Running,
                detail: String::new(),
                display_name: String::new(),
                created_at_ms: 0,
                entries: Vec::new(),
                summary: None,
                progress: None,
                control: None,
                speed: Default::default(),
            });
            for status in [
                TransferJobStatus::Running,
                TransferJobStatus::Queued,
                TransferJobStatus::Paused,
                TransferJobStatus::Cancelling,
            ] {
                app.transfer.transfer_job_mut("active").unwrap().status = status;
                assert!(app.file_sudo_busy("sudo-test"));
            }
            app.transfer.transfer_job_mut("active").unwrap().status = TransferJobStatus::Completed;
            assert!(!app.file_sudo_busy("sudo-test"));
            let guard = app.transfer.begin_external_file_task("sudo-test");
            assert!(app.file_sudo_busy("sudo-test"));
            drop(guard);
            assert!(!app.file_sudo_busy("sudo-test"));
        })
    });
}

#[gpui::test]
fn locking_cancels_inflight_file_authorization_and_late_success_is_ignored(
    cx: &mut TestAppContext,
) {
    let root = TestTempDir::new("nyaterm-sudo-lock");
    let connection = connection();
    let (app, cx) = hosted(cx, root.path(), &connection);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.toggle_file_sudo("sudo-test".into(), window, cx);
            app.remote_file_service_for_session(
                "sudo-test",
                connection.config.clone(),
                FileAccessPurpose::Automation,
            )
            .unwrap();
            let (generation, lease) = app
                .session
                .begin_file_sudo(
                    "sudo-test",
                    connection.handle.clone(),
                    Some("test-only".into()),
                )
                .unwrap();
            app.transfer.sudo_prompt.as_mut().unwrap().generation = Some(generation);
            let candidate = nyaterm_files::remote_file::RemoteFileService::with_sudo(
                connection.config.clone(),
                connection.handle.clone(),
                lease.clone(),
            )
            .unwrap();
            app.apply_shared_screen_lock(true, window, cx);
            assert!(lease.is_revoked());
            assert!(app.existing_text_input(PASSWORD_INPUT).is_none());
            app.finish_file_sudo_prompt("sudo-test".into(), generation, Ok(candidate), window, cx);
            assert_eq!(
                app.file_sudo_status("sudo-test"),
                FilePrivilegeStatus::Normal
            );
        })
    });
    draw(cx);
}

#[gpui::test]
fn privilege_switch_retires_both_panes_and_preserves_dirty_document_revision(
    cx: &mut TestAppContext,
) {
    use crate::features::file_manager::{
        FileManagerSeed, FileManagerWorkspace, PaneEndpoint, PaneId,
    };
    use nyaterm_files::remote_file::{RemoteTextMetadata, RemoteTextRevision};
    let root = TestTempDir::new("nyaterm-sudo-pane-cache");
    let connection = connection();
    let (app, cx) = hosted(cx, root.path(), &connection);
    cx.update(|_, cx| {
        let manager = cx.new(|cx| {
            FileManagerWorkspace::new(
                app.clone(),
                "sudo-manager".into(),
                FileManagerSeed {
                    left: PaneEndpoint::Session("sudo-test".into()),
                    left_path: None,
                    right: PaneEndpoint::Session("sudo-test".into()),
                    right_path: None,
                },
                cx,
            )
        });
        let generations = manager
            .read(cx)
            .pane_ids_for_session("sudo-test")
            .into_iter()
            .map(|id| (id, manager.read(cx).pane(id).generation))
            .collect::<Vec<_>>();
        app.update(cx, |app, cx| {
            app.file_manager
                .insert("sudo-manager".into(), manager.clone());
            let mut document = crate::features::test_support::editor_document(
                "sudo-test",
                "/dirty",
                nyaterm_core::InternalEditorDisplay::Window,
            );
            document.dirty = true;
            document.content = "unsaved content".into();
            let revision = RemoteTextRevision::from_bytes(
                b"original",
                RemoteTextMetadata {
                    size: 8,
                    modified_at: Some(1),
                },
            );
            document.revision = Some(revision.clone());
            app.transfer.open_editor_tab(document);
            app.invalidate_file_privilege_views("sudo-test", cx);
            let document = &app.transfer.editor_documents()[0];
            assert!(document.dirty);
            assert_eq!(document.content.as_str(), "unsaved content");
            assert_eq!(document.revision.as_ref(), Some(&revision));
        });
        assert_eq!(generations.len(), 2);
        for (id, before) in generations {
            assert!(matches!(id, PaneId::Left | PaneId::Right));
            assert_ne!(manager.read(cx).pane(id).generation, before);
            assert!(manager.read(cx).pane(id).entries.is_empty());
        }
    });
}
