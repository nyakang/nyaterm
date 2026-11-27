use gpui::{Context, TestAppContext};
use nyaterm_core::models::settings::TerminalWrapMode;

use crate::features::NyaTermApp;
use crate::features::test_support::app_with_visible_local_session;
use crate::test_support::{TestAppCleanup, TestConfigDir};

fn finish_frames(app: &mut NyaTermApp, cx: &mut Context<NyaTermApp>) {
    for _ in 0..16 {
        app.terminal
            .view
            .frame_pipeline
            .finish_pending_output()
            .unwrap();
        app.drain_terminal_frame_events(cx);
        if app.terminal.view.frame_pipeline.queued_event_count() == 0
            && app.terminal.view.pending_frame_events.is_empty()
            && app.terminal.view.frame_pipeline.queued_command_count() == 0
        {
            return;
        }
    }
    panic!("terminal frames did not settle");
}

#[test]
fn wrap_preview_replaces_idle_live_and_history_frames_and_cancel_restores_them() {
    for offset in [0, 80] {
        let dir = TestConfigDir::new("nyaterm-wrap-preview");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "s1");
        app.update(&mut cx, |app, cx| {
            app.begin_settings_draft(cx);
            app.terminal
                .view
                .views
                .get_mut("s1")
                .unwrap()
                .screen
                .resize(12, 4);
            app.terminal
                .view
                .frame_pipeline
                .ensure_session("s1", "UTF-8", 1000);
            app.terminal.view.frame_pipeline.resize_session("s1", 12, 4);
            app.terminal.view.frame_pipeline.submit_output(
                "s1",
                "一二三四五六七八九十abcdef\r\n".repeat(100).into_bytes(),
                "UTF-8",
                1000,
            );
            finish_frames(app, cx);
            app.terminal.view.views.get_mut("s1").unwrap().scroll_offset = offset;
            app.terminal.view.scroll_offset = offset;
            if offset > 0 {
                app.request_terminal_frame_snapshot_for_user_scroll("s1", offset);
                finish_frames(app, cx);
            }
            app.notify_terminal_surface_only(Some("s1"), cx);
            let history = app.terminal.view.views["s1"].history_state();
            let surface = app.terminal.view.surfaces["s1"].clone();
            let original = surface
                .read(cx)
                .snapshot_covering_display_offset(offset, 4, history.scrollback_len)
                .unwrap();
            assert!(original.logical_lines.is_none());
            app.set_terminal_wrap_mode(TerminalWrapMode::None, cx);
            assert!(surface.read(cx).wrap_layout_pending());
            assert!(
                surface
                    .read(cx)
                    .snapshot_covering_display_offset(offset, 4, history.scrollback_len)
                    .is_none()
            );
            finish_frames(app, cx);
            let view = &app.terminal.view.views["s1"];
            assert_eq!(view.history_state(), history);
            assert_eq!(view.scroll_offset, offset);
            assert!(!surface.read(cx).wrap_layout_pending());
            let replacement = surface
                .read(cx)
                .snapshot_covering_display_offset(offset, 4, history.scrollback_len)
                .unwrap();
            assert!(replacement.logical_lines.is_some());
            let index = replacement.logical_lines.as_ref().unwrap();
            let anchor = index.physical_row(
                index
                    .logical_row(history.scrollback_len.saturating_sub(offset))
                    .min(index.logical_rows().saturating_sub(4)),
            );
            app.cancel_settings(cx);
            finish_frames(app, cx);
            let view = &app.terminal.view.views["s1"];
            let expected_offset = if offset == 0 {
                0
            } else {
                history.scrollback_len.saturating_sub(anchor)
            };
            assert_eq!(view.scroll_offset, expected_offset);
            let restored = surface
                .read(cx)
                .snapshot_covering_display_offset(expected_offset, 4, history.scrollback_len)
                .unwrap();
            assert!(restored.logical_lines.is_none());
            assert_eq!(restored.history, history);
        });
    }
}

#[test]
fn wrap_reload_updates_worker_and_same_mode_is_noop() {
    let dir = TestConfigDir::new("nyaterm-wrap-reload");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, dir.path(), "s1");
    app.update(&mut cx, |app, cx| {
        app.begin_settings_draft(cx);
        let initial = app.terminal.view.frame_pipeline.layout_generation();
        app.set_terminal_wrap_mode(TerminalWrapMode::Character, cx);
        assert_eq!(
            app.terminal.view.frame_pipeline.layout_generation(),
            initial
        );
        let mut settings = app.settings.summary().clone();
        settings.terminal_wrap_mode = TerminalWrapMode::None;
        app.apply_gpui_settings(settings, cx);
        let changed = app.terminal.view.frame_pipeline.layout_generation();
        assert_ne!(initial, changed);
        app.sync_terminal_wrap_mode(cx);
        assert_eq!(
            app.terminal.view.frame_pipeline.layout_generation(),
            changed
        );
        app.cancel_settings(cx);
    });
}

#[test]
fn wrap_change_in_a_hidden_tab_refreshes_when_shown_without_output() {
    let dir = TestConfigDir::new("nyaterm-hidden-wrap");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let app = app_with_visible_local_session(&mut cx, dir.path(), "s1");
    app.update(&mut cx, |app, cx| {
        app.begin_settings_draft(cx);
        app.terminal
            .view
            .frame_pipeline
            .submit_output("s1", vec![b'x'; 200], "UTF-8", 1000);
        finish_frames(app, cx);
        app.notify_terminal_surface_only(Some("s1"), cx);
        let surface = app.terminal.view.surfaces["s1"].clone();
        app.shell.hide_workspace_session("s1");
        assert!(app.visible_terminal_session_ids().is_empty());
        app.sync_terminal_frame_snapshot_priority();
        app.set_terminal_wrap_mode(TerminalWrapMode::None, cx);
        finish_frames(app, cx);
        assert!(app.terminal.view.views["s1"].frame_snapshot.is_none());
        assert!(surface.read(cx).wrap_layout_pending());
        app.shell.reveal_workspace_session("s1");
        app.sync_terminal_frame_snapshot_priority();
        app.notify_terminal_surface_only(Some("s1"), cx);
        finish_frames(app, cx);
        let view = &app.terminal.view.views["s1"];
        let history = view.history_state();
        assert!(!surface.read(cx).wrap_layout_pending());
        let snapshot = surface
            .read(cx)
            .snapshot_covering_display_offset(0, history.viewport_rows, history.scrollback_len)
            .unwrap();
        assert!(snapshot.logical_lines.is_some());
    });
}
