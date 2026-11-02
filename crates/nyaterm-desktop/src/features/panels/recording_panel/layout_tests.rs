use gpui::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _,
    TestAppContext, Window, div, px,
};
use nyaterm_core::test_support::TestTempDir;
use nyaterm_transport::{RecordingMode, RecordingStatus, RecordingStatusState};

use crate::features::NyaTermApp;
use crate::features::test_support::app_with_visible_local_session;
use crate::models::SessionLaunchConfig;

struct RecordingPanelHost {
    app: Entity<NyaTermApp>,
    width: f32,
}

impl Render for RecordingPanelHost {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.app.update(cx, |app, cx| {
            let model = app.recording_sessions_panel_model();
            div()
                .w(px(self.width))
                .h(px(420.))
                .child(app.recording_panel(model, cx))
                .into_any_element()
        })
    }
}

#[gpui::test]
fn recording_rows_resize_when_recording_starts_fails_and_stops(cx: &mut TestAppContext) {
    let root = TestTempDir::new("nyaterm-recording-row-height");
    let app = app_with_visible_local_session(cx, root.path(), "fixture");
    cx.update_entity(&app, |app, cx| app.sync_component_theme(cx));
    let host_app = app.clone();
    let (_, vcx) = cx.add_window_view(move |_, _| RecordingPanelHost {
        app: host_app,
        width: 320.,
    });
    for (state, has_path, expected_height) in [
        (None, false, 56.),
        (Some(RecordingStatusState::Recording), true, 74.),
        (Some(RecordingStatusState::Failed), true, 92.),
        (Some(RecordingStatusState::Failed), false, 74.),
        (None, false, 56.),
    ] {
        vcx.update(|window, cx| {
            app.update(cx, |app, cx| {
                if let Some(state) = state {
                    app.recording.apply_status(RecordingStatus {
                        session_id: "fixture".into(),
                        state,
                        mode: RecordingMode::Transcript,
                        file_path: has_path.then(|| root.join("session.log")),
                        input_file_path: None,
                        started_at: None,
                        written_bytes: 1024,
                        queued_bytes: 0,
                        dropped_bytes: 0,
                        last_error: None,
                    });
                } else {
                    app.recording.remove_status("fixture");
                }
                cx.notify();
            });
            window.refresh();
            _ = window.draw(cx);
        });
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            _ = window.draw(cx);
        });
        let row = vcx.debug_bounds("recording-session-row-fixture").unwrap();
        assert_eq!(row.size.height, px(expected_height));
        assert_eq!(
            vcx.debug_bounds("recording-session-path-fixture").is_some(),
            has_path,
        );
        assert_eq!(
            vcx.debug_bounds("recording-session-error-fixture")
                .is_some(),
            state == Some(RecordingStatusState::Failed),
        );
        let summary = vcx
            .debug_bounds("recording-session-summary-fixture")
            .unwrap();
        let toggle = vcx
            .debug_bounds("recording-session-toggle-fixture")
            .unwrap();
        assert!(summary.bottom() < row.bottom());
        assert!(toggle.bottom() < row.bottom());
    }
}

#[gpui::test]
fn recording_panel_keeps_long_names_paths_errors_and_actions_inside_narrow_rows(
    cx: &mut TestAppContext,
) {
    let root = TestTempDir::new("nyaterm-recording-panel-layout");
    let app = app_with_visible_local_session(cx, root.path(), "fixture");
    cx.update_entity(&app, |app, cx| {
        app.sync_component_theme(cx);
        let mut metadata = app.session.metadata("fixture").unwrap().clone();
        if let SessionLaunchConfig::Local(config) = &mut metadata.launch_config {
            config.name = "生产环境主机名称很长\n第二行不应撑开卡片".into();
        }
        for index in 0..30 {
            let id = format!("session-{index:02}");
            app.session.register_session_metadata(&id, metadata.clone());
            app.recording.apply_status(RecordingStatus {
                session_id: id,
                state: if index == 0 {
                    RecordingStatusState::Failed
                } else if index == 1 {
                    RecordingStatusState::Degraded
                } else {
                    RecordingStatusState::Recording
                },
                mode: RecordingMode::Transcript,
                file_path: Some(
                    root.join("很长的录制目录/生产环境服务器/2026-10-22/非常长的日志文件名称.log"),
                ),
                input_file_path: None,
                started_at: None,
                written_bytes: 34567,
                queued_bytes: 0,
                dropped_bytes: 0,
                last_error: Some("无法写入日志文件\n模拟多行错误详情".into()),
            });
        }
    });
    let host_app = app.clone();
    let (host, vcx) = cx.add_window_view(move |_, _| RecordingPanelHost {
        app: host_app,
        width: 280.,
    });
    for width in [280., 350., 520.] {
        vcx.update(|window, cx| {
            host.update(cx, |host, cx| {
                host.width = width;
                cx.notify();
            });
            window.refresh();
            _ = window.draw(cx);
        });
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            _ = window.draw(cx);
        });
        let row = vcx
            .debug_bounds("recording-session-row-session-00")
            .unwrap();
        assert_eq!(row.size.height, px(92.));
        assert!(row.right() <= px(width));
        let name = vcx
            .debug_bounds("recording-session-name-session-00")
            .unwrap();
        let summary = vcx
            .debug_bounds("recording-session-summary-session-00")
            .unwrap();
        let path = vcx
            .debug_bounds("recording-session-path-session-00")
            .unwrap();
        let error = vcx
            .debug_bounds("recording-session-error-session-00")
            .unwrap();
        let toggle = vcx
            .debug_bounds("recording-session-toggle-session-00")
            .unwrap();
        for line in [name, summary, path, error] {
            assert!(line.left() >= row.left());
            assert!(line.right() <= toggle.left());
            assert!(line.bottom() <= row.bottom());
        }
        assert!(name.bottom() <= summary.top());
        assert!(summary.bottom() <= path.top());
        assert!(path.bottom() <= error.top());
        assert_eq!(toggle.size.width, px(28.));
        assert!(toggle.right() < row.right());
        let next_row = vcx
            .debug_bounds("recording-session-row-session-01")
            .unwrap();
        assert!(next_row.top() >= row.bottom());
        let recording_row = vcx
            .debug_bounds("recording-session-row-session-02")
            .unwrap();
        assert_eq!(recording_row.size.height, px(74.));
        // The remaining rows must stay virtualized, rather than overflowing the viewport.
        assert!(
            vcx.debug_bounds("recording-session-row-session-29")
                .is_none()
        );
    }
}
