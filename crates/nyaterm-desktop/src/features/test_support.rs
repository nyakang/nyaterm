use super::NyaTermApp;
use crate::entities::{OverlayStore, StartupRestoreStore, UiStoreHandles};
use crate::models::{SessionLaunchConfig, SessionRuntimeMetadata};
use gpui::{
    AppContext as _, Entity, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    Styled as _, TestAppContext, VisualTestContext, div,
};
use nyaterm_core::{AiExecutionProfile, AppRuntime, RuntimeMode};
use nyaterm_transport::{LocalSessionConfig, SshSessionConfig};
use std::path::Path;

struct SessionFocusHost {
    app: Entity<NyaTermApp>,
}

impl Render for SessionFocusHost {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement {
        let app = self.app.read(cx);
        div()
            .size_full()
            .child(div().track_focus(app.terminal.input_focus()))
            .child(div().track_focus(app.remote_desktop.focus()))
            .child(div().track_focus(app.session.prompt_credential_focus()))
            .child(div().track_focus(app.session.dialog_tab_actions_focus()))
    }
}

pub(crate) fn session_focus_test_window<'a>(
    cx: &'a mut TestAppContext,
    app: &Entity<NyaTermApp>,
) -> &'a mut VisualTestContext {
    let app = app.clone();
    let (_, cx) = cx.add_window_view(move |_, _| SessionFocusHost { app });
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    cx
}

/// Build a visible local terminal session for GPUI tests.
pub(crate) fn app_with_visible_local_session(
    cx: &mut gpui::TestAppContext,
    root: &Path,
    session_id: &str,
) -> gpui::Entity<NyaTermApp> {
    app_with_visible_session(
        cx,
        root,
        session_id,
        SessionLaunchConfig::Local(LocalSessionConfig {
            name: "Local session".to_string(),
            ..LocalSessionConfig::default()
        }),
        None,
    )
}

/// Build a visible SSH terminal session for GPUI tests.
///
/// Unlike a local shell this session holds a backend, so tests that need a
/// session which configuration replacement must not disturb use this one.
pub(crate) fn app_with_visible_ssh_session(
    cx: &mut gpui::TestAppContext,
    root: &Path,
    session_id: &str,
) -> gpui::Entity<NyaTermApp> {
    let config = SshSessionConfig {
        name: session_id.to_string(),
        host: "example.test".to_string(),
        username: "tester".to_string(),
        ..SshSessionConfig::default()
    };
    app_with_visible_session(
        cx,
        root,
        session_id,
        SessionLaunchConfig::Ssh(Box::new(config.clone())),
        Some(config),
    )
}

fn app_with_visible_session(
    cx: &mut gpui::TestAppContext,
    root: &Path,
    session_id: &str,
    launch_config: SessionLaunchConfig,
    ssh_config: Option<SshSessionConfig>,
) -> gpui::Entity<NyaTermApp> {
    let runtime = AppRuntime::from_parts_for_test(
        RuntimeMode::Portable,
        root.to_path_buf(),
        root.join("config"),
        root.join("logs"),
        root.join("cache"),
        None,
    );
    let stores = UiStoreHandles {
        startup_restore: cx.new(|_| StartupRestoreStore::default()),
        overlays: cx.new(|_| OverlayStore::default()),
    };
    let app = cx.new(|cx| NyaTermApp::new(runtime, stores, cx));
    cx.update_entity(&app, |app, _| {
        let mut summary = app.settings.summary().clone();
        summary.cursor_blink = true;
        app.settings.replace_summary(summary);
        app.session.register_session_metadata(
            session_id,
            SessionRuntimeMetadata {
                ssh_config,
                ssh_multiplex_key: None,
                source_connection_id: None,
                ai_execution_profile: AiExecutionProfile::Posix,
                launch_config,
                disconnected: false,
            },
        );
        app.session.select_active_session(session_id);
        app.terminal
            .seed_session_view(session_id.to_string(), String::new(), "UTF-8");
        app.shell.show_workspace();
        assert!(
            !app.visible_terminal_session_ids().is_empty(),
            "the fixture must have a visible session"
        );
    });
    app
}

/// A loaded editor document without starting file I/O.
pub(crate) fn editor_document(
    session: &str,
    path: &str,
    display: nyaterm_core::InternalEditorDisplay,
) -> crate::models::TransferEditorState {
    crate::models::TransferEditorState {
        display,
        id: crate::models::TransferEditorState::tab_id(Some(session), path),
        session_id: Some(session.to_string()),
        remote_path: path.to_string(),
        raw_path_token: None,
        name: path.rsplit(['/', '\\']).next().unwrap_or(path).to_string(),
        content: "original".into(),
        search_query: String::new(),
        active_match: 0,
        revision: None,
        generation: nyaterm_files::remote_file::RemoteTextGeneration::next(),
        loading: false,
        saving: false,
        dirty: false,
        conflict: false,
        close_after_save: false,
        reload_confirm: false,
        error: None,
        backup_warning_path: None,
        focused_field: crate::models::TransferEditorField::Content,
    }
}
