mod approvals;
mod history_and_streaming;
mod pickers;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use super::{AiAgentStepPresentation, AiMentionCandidate, AiPanel, AiPanelSnapshot};
use crate::features::ai::panel::transcript::{AiTranscriptRow, AiTranscriptUpdate};
use crate::features::runtime_jobs::{AiAgentStepStatus, AiAgentStepView};
use gpui::{
    AppContext as _, Entity, IntoElement, ParentElement as _, Render, Styled as _, TestAppContext,
    VisualTestContext, div, px,
};
use nyaterm_core::{
    AgentCommandExecutionMode, AiCommandCard, AiMessage, AiMessageRole, AiMode, AiModelConfigItem,
    AiModelSource, AiProviderKind, AiSettings, AppRuntime, RuntimeMode,
};
use nyaterm_ui::NyaInputEvent;

use crate::entities::{OverlayStore, StartupRestoreStore, UiStoreHandles};
use crate::features::{NyaTermApp, runtime_jobs::AiChatJobOutput};
use crate::test_support::TestConfigDir;

fn app(cx: &mut TestAppContext, root: &Path) -> Entity<NyaTermApp> {
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
    cx.new(|cx| NyaTermApp::new(runtime, stores, cx))
}

struct AppHost {
    app: Entity<NyaTermApp>,
}

impl Render for AppHost {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement {
        let app = self.app.read(cx);
        div()
            .w(px(360.))
            .h(px(720.))
            .flex()
            .gap_1()
            .child(
                div().flex_1().min_h_0().overflow_hidden().child(
                    app.ai_panel
                        .clone()
                        .cached(crate::features::layout::cached_panel_style()),
                ),
            )
            .child(
                div().w(px(260.)).min_h_0().overflow_hidden().child(
                    app.connection_panel
                        .clone()
                        .cached(crate::features::layout::cached_panel_style()),
                ),
            )
            .child(
                div().w(px(260.)).min_h_0().overflow_hidden().child(
                    app.transfer_panel
                        .clone()
                        .cached(crate::features::layout::cached_panel_style()),
                ),
            )
            .child(
                div().w(px(260.)).min_h_0().overflow_hidden().child(
                    app.settings_panel
                        .clone()
                        .cached(crate::features::layout::cached_panel_style()),
                ),
            )
    }
}

fn hosted<'a>(
    cx: &'a mut TestAppContext,
    root: &Path,
) -> (Entity<NyaTermApp>, &'a mut VisualTestContext) {
    let app = app(cx, root);
    cx.update_entity(&app, |app, cx| {
        app.sync_component_theme(cx);
        app.flush_ai_panel_snapshot(cx);
        app.flush_connection_panel_snapshot(cx);
        app.flush_transfer_panel_snapshot(cx);
        app.flush_settings_panel_snapshots(cx);
    });
    let host_app = app.clone();
    let (_, vcx) = cx.add_window_view(move |_, _| AppHost {
        app: host_app.clone(),
    });
    let vcx: &mut VisualTestContext = vcx;
    vcx.run_until_parked();
    for _ in 0..3 {
        vcx.update(|window, cx| {
            app.update(cx, |_, cx| cx.notify());
            _ = window.draw(cx);
        });
        vcx.run_until_parked();
    }
    (app, vcx)
}

fn draw(app: &Entity<NyaTermApp>, vcx: &mut VisualTestContext) {
    vcx.update(|window, cx| {
        app.update(cx, |_, cx| cx.notify());
        _ = window.draw(cx);
    });
    vcx.run_until_parked();
}

fn ai_paints(app: &Entity<NyaTermApp>, cx: &mut gpui::App) -> usize {
    app.read(cx).ai_panel.read(cx).paint_count()
}

fn ai_snapshot_sets(app: &Entity<NyaTermApp>, cx: &mut gpui::App) -> usize {
    app.read(cx).ai_panel.read(cx).snapshot_set_count()
}

fn connection_paints(app: &Entity<NyaTermApp>, cx: &mut gpui::App) -> usize {
    app.read(cx).connection_panel.read(cx).paint_count()
}

fn transfer_paints(app: &Entity<NyaTermApp>, cx: &mut gpui::App) -> usize {
    app.read(cx).transfer_panel.read(cx).paint_count()
}

fn settings_paints(app: &Entity<NyaTermApp>, cx: &mut gpui::App) -> usize {
    app.read(cx).settings_panel.read(cx).paint_count()
}

#[test]
fn detected_terminal_error_refreshes_ai_panel_only() {
    let test_dir = TestConfigDir::new("nyaterm-ai-panel");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let (app, vcx) = hosted(&mut cx, test_dir.path());
    let before_snapshots = vcx.update(|_, cx| ai_snapshot_sets(&app, cx));
    let before_ai_paints = vcx.update(|_, cx| ai_paints(&app, cx));
    let before_connection_paints = vcx.update(|_, cx| connection_paints(&app, cx));
    let before_transfer_paints = vcx.update(|_, cx| transfer_paints(&app, cx));
    let before_settings_paints = vcx.update(|_, cx| settings_paints(&app, cx));

    vcx.update(|_, cx| {
        app.update(cx, |app, cx| {
            assert!(app.ai.note_detected_error(
                "session-a".to_string(),
                "permission denied".to_string(),
                Instant::now(),
            ));
            app.defer_ai_panel_snapshot_flush(cx);
        });
    });
    vcx.run_until_parked();

    assert_eq!(
        vcx.update(|_, cx| ai_snapshot_sets(&app, cx)),
        before_snapshots + 1,
        "detected terminal errors should rebuild the AiPanel snapshot"
    );
    draw(&app, vcx);
    assert!(
        vcx.update(|_, cx| ai_paints(&app, cx)) > before_ai_paints,
        "the AiPanel should repaint after its snapshot changes"
    );
    assert_eq!(
        vcx.update(|_, cx| connection_paints(&app, cx)),
        before_connection_paints,
        "AI-owned refreshes must not repaint the connections panel"
    );
    assert_eq!(
        vcx.update(|_, cx| transfer_paints(&app, cx)),
        before_transfer_paints,
        "AI-owned refreshes must not repaint the transfer panel"
    );
    assert_eq!(
        vcx.update(|_, cx| settings_paints(&app, cx)),
        before_settings_paints,
        "AI-owned refreshes must not repaint the settings panel"
    );
}

#[test]
fn repeated_ai_refresh_requests_coalesce() {
    let test_dir = TestConfigDir::new("nyaterm-ai-panel");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let (app, vcx) = hosted(&mut cx, test_dir.path());
    let before = vcx.update(|_, cx| ai_snapshot_sets(&app, cx));

    vcx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.defer_ai_panel_snapshot_flush(cx);
            app.defer_ai_panel_snapshot_flush(cx);
            app.defer_ai_panel_snapshot_flush(cx);
        });
    });
    vcx.run_until_parked();

    let after = vcx.update(|_, cx| ai_snapshot_sets(&app, cx));
    assert_eq!(
        after,
        before + 1,
        "same-cycle refresh requests should build/set one snapshot"
    );

    vcx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.defer_ai_panel_snapshot_flush(cx);
        });
    });
    vcx.run_until_parked();

    assert_eq!(
        vcx.update(|_, cx| ai_snapshot_sets(&app, cx)),
        after + 1,
        "a completed flush must not lock out the next refresh request"
    );

    let after_single = vcx.update(|_, cx| ai_snapshot_sets(&app, cx));
    vcx.update(|_, cx| {
        let panel = app.read(cx).ai_panel.clone();
        panel.update(cx, |panel, cx| {
            panel.with_app(cx, |app, cx| {
                app.defer_ai_panel_snapshot_flush(cx);
            });
        });
    });
    vcx.run_until_parked();

    assert_eq!(
        vcx.update(|_, cx| ai_snapshot_sets(&app, cx)),
        after_single + 1,
        "with_app fallback plus an explicit refresh should still coalesce"
    );
}

#[test]
fn ai_header_running_transition_notifies_root() {
    let test_dir = TestConfigDir::new("nyaterm-ai-panel");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app = app(&mut cx, test_dir.path());
    cx.update_entity(&app, |app, cx| {
        app.sync_component_theme(cx);

        let idle = app.ai_header_presentation();
        assert!(!idle.running);
        let launch = app
            .ai
            .begin_chat_request("inspect".to_string(), AiMode::Ask, None);
        let running = app.ai_header_presentation();
        assert!(running.running);
        assert_ne!(idle, running, "idle -> running should move the header");

        assert!(app.ai.apply_chat_delta(launch.job_id, "hello", None));
        assert_eq!(
            app.ai_header_presentation(),
            running,
            "ordinary streaming deltas must not move the root header projection"
        );

        app.ai
            .finish_chat_job(
                launch.job_id,
                launch.session_id,
                Ok(AiChatJobOutput {
                    native_call: None,
                    mode: AiMode::Ask,
                    text: "done".to_string(),
                    reasoning: None,
                    command_cards: Vec::new(),
                    auto_execute_first: false,
                    approval_note: None,
                }),
            )
            .expect("matching job should finish");
        let finished = app.ai_header_presentation();
        assert!(!finished.running);
        assert_ne!(running, finished, "running -> idle should move the header");

        let cancel_launch = app
            .ai
            .begin_chat_request("cancel me".to_string(), AiMode::Ask, None);
        let cancel_running = app.ai_header_presentation();
        assert!(cancel_running.running);
        app.ai.cancel_chat_and_agent();
        assert!(
            cancel_launch
                .cancel
                .load(std::sync::atomic::Ordering::Relaxed)
        );
        let cancelled = app.ai_header_presentation();
        assert!(!cancelled.running);
        assert_ne!(
            cancel_running, cancelled,
            "cancel should move running back to idle"
        );

        let execution_before = app.ai_header_presentation();
        app.ai
            .set_settings_command_mode(AgentCommandExecutionMode::Auto);
        let execution_after = app.ai_header_presentation();
        assert_ne!(
            execution_before, execution_after,
            "execution mode is part of the root header projection"
        );

        let settings = AiSettings {
            models: vec![
                AiModelConfigItem {
                    supported_reasoning_efforts: None,
                    backend: Default::default(),
                    id: "openai:model-a".to_string(),
                    name: "Model A".to_string(),
                    provider_kind: Some(AiProviderKind::Openai),
                    credential_id: None,
                    enabled: true,
                    source: AiModelSource::Manual,
                    last_seen_at: None,
                },
                AiModelConfigItem {
                    supported_reasoning_efforts: None,
                    backend: Default::default(),
                    id: "openai:model-b".to_string(),
                    name: "Model B".to_string(),
                    provider_kind: Some(AiProviderKind::Openai),
                    credential_id: None,
                    enabled: true,
                    source: AiModelSource::Manual,
                    last_seen_at: None,
                },
            ],
            default_model_id: Some("openai:model-a".to_string()),
            ..AiSettings::default()
        };
        app.ai.replace_settings_config(settings, true);
        let model_before = app.ai_header_presentation();
        app.ai.set_settings_default_model("openai:model-b");
        let model_after = app.ai_header_presentation();
        assert_ne!(
            model_before, model_after,
            "selected model is part of the root header projection"
        );
        assert_eq!(model_after.model_label, "Model B");
    });
}

#[test]
fn unrelated_app_notify_does_not_repaint_cached_ai_panel() {
    let test_dir = TestConfigDir::new("nyaterm-ai-panel");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let (app, vcx) = hosted(&mut cx, test_dir.path());
    let before = vcx.update(|_, cx| ai_paints(&app, cx));
    assert!(
        before > 0,
        "the panel must have painted at least once, or this proves nothing"
    );

    for _ in 0..5 {
        draw(&app, vcx);
    }

    assert_eq!(
        vcx.update(|_, cx| ai_paints(&app, cx)),
        before,
        "unrelated app notifies must not repaint the cached AI panel"
    );
}

#[test]
fn streaming_delta_repaints_ai_panel_without_repainting_sibling_panels() {
    let test_dir = TestConfigDir::new("nyaterm-ai-panel");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let (app, vcx) = hosted(&mut cx, test_dir.path());

    let before = vcx.update(|_, cx| {
        (
            ai_paints(&app, cx),
            connection_paints(&app, cx),
            transfer_paints(&app, cx),
            settings_paints(&app, cx),
        )
    });

    vcx.update(|_, cx| {
        app.update(cx, |app, cx| {
            let launch = app
                .ai
                .begin_chat_request("inspect".to_string(), AiMode::Ask, None);
            assert!(app.ai.apply_chat_delta(launch.job_id, "hello", None));
            app.flush_ai_panel_snapshot(cx);
        });
    });
    vcx.update(|window, cx| {
        _ = window.draw(cx);
    });
    vcx.run_until_parked();

    let after = vcx.update(|_, cx| {
        (
            ai_paints(&app, cx),
            connection_paints(&app, cx),
            transfer_paints(&app, cx),
            settings_paints(&app, cx),
        )
    });
    assert!(after.0 > before.0, "streaming delta must repaint AI panel");
    assert_eq!(after.1, before.1, "connections panel must stay cached");
    assert_eq!(after.2, before.2, "transfers panel must stay cached");
    assert_eq!(after.3, before.3, "settings panel must stay cached");
}

#[test]
fn prompt_subscription_refreshes_snapshot_before_next_paint() {
    let test_dir = TestConfigDir::new("nyaterm-ai-panel");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let (app, vcx) = hosted(&mut cx, test_dir.path());
    let prompt_input = vcx.update(|_, cx| {
        app.read(cx)
            .ai_panel
            .read(cx)
            .snapshot()
            .expect("hosted panel has snapshot")
            .prompt_input
            .clone()
    });

    vcx.update(|_, cx| {
        prompt_input.update(cx, |_, cx| {
            cx.emit(NyaInputEvent::Changed("explain status".to_string()));
        });
        assert_eq!(
            app.read(cx)
                .ai_panel
                .read(cx)
                .snapshot()
                .expect("snapshot remains available")
                .prompt_draft,
            "",
            "the snapshot must wait for the deferred input flush"
        );
    });
    vcx.run_until_parked();

    vcx.update(|window, cx| {
        let snapshot = app.read(cx).ai_panel.read(cx).snapshot().cloned();
        assert_eq!(
            snapshot.expect("deferred flush ran").prompt_draft,
            "explain status"
        );
        _ = window.draw(cx);
    });
}

#[test]
fn message_menu_position_stays_inside_viewport() {
    assert_eq!(
        super::components::ai_message_menu_position(1240., 780., 128., 64., 1280., 800.),
        (1144., 728., 784.)
    );
    assert_eq!(
        super::components::ai_message_menu_position(240., 180., 128., 64., 200., 120.),
        (64., 48., 104.)
    );
}
struct CompactAiHost {
    panel: Entity<AiPanel>,
    height: f32,
}

impl Render for CompactAiHost {
    fn render(&mut self, _: &mut gpui::Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        div()
            .w(px(320.))
            .h(px(self.height))
            .child(self.panel.clone())
    }
}

fn compact_app(cx: &mut TestAppContext, root: &Path) -> Entity<NyaTermApp> {
    let app = app(cx, root);
    cx.update_entity(&app, |app, cx| {
        app.sync_component_theme(cx);
        let mut settings = app.ai.settings_config_cloned();
        settings.enabled = true;
        settings.models = vec![AiModelConfigItem {
            id: "fixture".to_string(),
            name: "A long model name that must truncate in a narrow panel".to_string(),
            provider_kind: Some(AiProviderKind::OpenaiCompatible),
            credential_id: None,
            enabled: true,
            source: AiModelSource::Manual,
            backend: Default::default(),
            last_seen_at: None,
            supported_reasoning_efforts: None,
        }];
        settings.default_model_id = Some("fixture".to_string());
        app.ai.replace_settings_config(settings, false);
        app.flush_ai_panel_snapshot(cx);
    });
    app
}

fn compact_host<'a>(
    cx: &'a mut TestAppContext,
    root: &Path,
) -> (Entity<NyaTermApp>, &'a mut VisualTestContext) {
    let app = compact_app(cx, root);
    let panel = app.read_with(cx, |app, _| app.ai_panel.clone());
    let (_, vcx) = cx.add_window_view(move |_, _| CompactAiHost {
        panel,
        height: 800.,
    });
    let vcx: &mut VisualTestContext = vcx;
    compact_draw(&app, vcx);
    (app, vcx)
}

fn compact_draw(app: &Entity<NyaTermApp>, vcx: &mut VisualTestContext) {
    vcx.run_until_parked();
    for _ in 0..2 {
        vcx.update(|window, cx| {
            app.read(cx)
                .ai_panel
                .clone()
                .update(cx, |_, cx| cx.notify());
            window.refresh();
            _ = window.draw(cx);
        });
        vcx.run_until_parked();
    }
}

fn edit_panel_snapshot(
    app: &Entity<NyaTermApp>,
    vcx: &mut VisualTestContext,
    edit: impl FnOnce(&mut AiPanelSnapshot),
) {
    vcx.update(|_, cx| {
        let panel = app.read(cx).ai_panel.clone();
        panel.update(cx, |panel, cx| {
            let mut snapshot = panel.snapshot().unwrap().clone();
            edit(&mut snapshot);
            panel.set_snapshot(snapshot, cx);
        });
    });
    compact_draw(app, vcx);
}

#[test]
fn narrow_ai_panel_centers_empty_state_and_keeps_compact_controls_inside_composer() {
    let root = TestConfigDir::new("nyaterm-ai-compact");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let (_, vcx) = compact_host(&mut cx, root.path());
    let viewport = vcx.debug_bounds("ai-transcript-viewport").unwrap();
    let empty = vcx.debug_bounds("ai-empty-transcript").unwrap();
    assert!((empty.center().y - viewport.center().y).abs() < px(16.));
    let composer = vcx.debug_bounds("ai-composer").unwrap();
    let prompt = vcx.debug_bounds("ai-prompt").unwrap();
    let mode = vcx.debug_bounds("ai-mode-control").unwrap();
    let model = vcx.debug_bounds("ai-model-control").unwrap();
    let send = vcx.debug_bounds("ai-send-control").unwrap();
    assert!(
        composer.size.height <= px(122.),
        "composer should have just a textarea and one control row"
    );
    assert_eq!(prompt.size.height, px(64.));
    for control in [mode, model, send] {
        assert!(control.top() >= prompt.bottom());
        assert!(control.left() >= composer.left());
        assert!(control.right() <= composer.right());
        assert!((control.center().y - send.center().y).abs() <= px(2.));
    }
    assert!(mode.right() <= model.left());
    assert!(model.right() <= send.left());
}

#[test]
fn mention_picker_reveals_keyboard_selection_beyond_eight_sessions() {
    let root = TestConfigDir::new("nyaterm-ai-mentions");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let (app, vcx) = compact_host(&mut cx, root.path());
    edit_panel_snapshot(&app, vcx, |snapshot| {
        snapshot.mention_open = true;
        snapshot.mention_index = 10;
        snapshot.mention_candidates = (0..12)
            .map(|index| AiMentionCandidate {
                session_id: format!("fixture-{index}"),
                label: format!("Terminal {index}"),
                kind: "Local".to_string(),
                selected: false,
            })
            .collect::<Vec<_>>()
            .into();
    });
    let selected = vcx.debug_bounds("ai-mention-row-10").unwrap();
    let prompt = vcx.debug_bounds("ai-prompt").unwrap();
    let scroll = vcx.update(|_, cx| app.read(cx).ai_panel.read(cx).mention_scroll.clone());
    assert!(
        scroll.offset().y < px(0.),
        "offset={:?}, max={:?}, viewport={:?}, selected={selected:?}",
        scroll.offset(),
        scroll.max_offset(),
        scroll.bounds()
    );
    assert!(selected.top() >= scroll.bounds().top());
    assert!(selected.bottom() <= scroll.bounds().bottom());
    assert!(selected.bottom() <= prompt.top());
    assert!(vcx.debug_bounds("ai-mention-row-11").is_some());
}

fn transcript_message(index: usize) -> Arc<AiMessage> {
    Arc::new(AiMessage {
        id: format!("message-{index}"),
        session_id: "fixture".to_string(),
        role: AiMessageRole::User,
        content: "fixture output\n".repeat(6),
        created_at: "2026-10-02T00:00:00Z".to_string(),
        reasoning_content: None,
        command_cards: Vec::new(),
    })
}

#[test]
fn growing_transcript_follows_bottom_but_preserves_a_readers_scroll_position() {
    let root = TestConfigDir::new("nyaterm-ai-scroll");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let (app, vcx) = compact_host(&mut cx, root.path());
    edit_panel_snapshot(&app, vcx, |snapshot| {
        snapshot.messages = (0..24).map(transcript_message).collect::<Vec<_>>().into();
    });
    let scroll = vcx.update(|_, cx| app.read(cx).ai_panel.read(cx).transcript_scroll.clone());
    assert!(vcx.update(|_, cx| scroll.read(cx).is_following_tail()));
    assert!(vcx.debug_bounds("ai-message-message-23").is_some());
    assert!(vcx.debug_bounds("ai-message-message-0").is_none());
    vcx.update(|_, cx| scroll.update(cx, |state, cx| state.scroll_to_item(4, cx)));
    compact_draw(&app, vcx);
    let anchor = vcx.debug_bounds("ai-message-message-4").unwrap().top();
    edit_panel_snapshot(&app, vcx, |snapshot| {
        snapshot.messages = (0..25).map(transcript_message).collect::<Vec<_>>().into();
    });
    assert!(!vcx.update(|_, cx| scroll.read(cx).is_following_tail()));
    assert_eq!(
        vcx.debug_bounds("ai-message-message-4").unwrap().top(),
        anchor
    );
    vcx.update(|_, cx| scroll.update(cx, |state, cx| state.scroll_to_end(cx)));
    compact_draw(&app, vcx);
    edit_panel_snapshot(&app, vcx, |snapshot| {
        snapshot.messages = (0..26).map(transcript_message).collect::<Vec<_>>().into();
    });
    assert!(vcx.update(|_, cx| scroll.read(cx).is_following_tail()));
    assert!(vcx.debug_bounds("ai-message-message-25").is_some());
}

#[test]
fn prepending_history_preserves_the_visible_message_anchor() {
    let root = TestConfigDir::new("nyaterm-ai-prepend");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let (app, vcx) = compact_host(&mut cx, root.path());
    edit_panel_snapshot(&app, vcx, |snapshot| {
        snapshot.messages = (10..34).map(transcript_message).collect::<Vec<_>>().into();
    });
    let scroll = vcx.update(|_, cx| app.read(cx).ai_panel.read(cx).transcript_scroll.clone());
    vcx.update(|_, cx| scroll.update(cx, |state, cx| state.scroll_to_item(4, cx)));
    compact_draw(&app, vcx);
    let anchor = vcx.debug_bounds("ai-message-message-14").unwrap().top();
    edit_panel_snapshot(&app, vcx, |snapshot| {
        snapshot.messages = (0..34).map(transcript_message).collect::<Vec<_>>().into();
    });
    assert_eq!(
        vcx.debug_bounds("ai-message-message-14").unwrap().top(),
        anchor
    );
    assert!(!vcx.update(|_, cx| scroll.read(cx).is_following_tail()));
}

#[test]
fn streaming_remeasures_the_growing_row_and_preserves_a_readers_anchor() {
    let root = TestConfigDir::new("nyaterm-ai-stream-measure");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let (app, vcx) = compact_host(&mut cx, root.path());
    edit_panel_snapshot(&app, vcx, |snapshot| {
        let mut messages = (0..24).map(transcript_message).collect::<Vec<_>>();
        Arc::make_mut(&mut messages[23]).role = AiMessageRole::Assistant;
        snapshot.messages = messages.into();
        snapshot.streaming_assistant_id = Some("message-23".to_string());
    });
    let scroll = vcx.update(|_, cx| app.read(cx).ai_panel.read(cx).transcript_scroll.clone());
    let previous_height = vcx
        .debug_bounds("ai-message-message-23")
        .unwrap()
        .size
        .height;
    edit_panel_snapshot(&app, vcx, |snapshot| {
        let mut messages = snapshot.messages.to_vec();
        Arc::make_mut(&mut messages[23])
            .content
            .push_str(&"streamed line\n".repeat(6));
        snapshot.messages = messages.into();
    });
    assert!(
        vcx.debug_bounds("ai-message-message-23")
            .unwrap()
            .size
            .height
            > previous_height
    );
    assert!(vcx.update(|_, cx| scroll.read(cx).is_following_tail()));
    vcx.update(|_, cx| scroll.update(cx, |state, cx| state.scroll_to_item(4, cx)));
    compact_draw(&app, vcx);
    let anchor = vcx.debug_bounds("ai-message-message-4").unwrap().top();
    edit_panel_snapshot(&app, vcx, |snapshot| {
        let mut messages = snapshot.messages.to_vec();
        Arc::make_mut(&mut messages[23])
            .content
            .push_str(&"more streamed text\n".repeat(12));
        snapshot.messages = messages.into();
    });
    assert_eq!(
        vcx.debug_bounds("ai-message-message-4").unwrap().top(),
        anchor
    );
    assert!(!vcx.update(|_, cx| scroll.read(cx).is_following_tail()));
}

#[test]
fn switching_conversations_resumes_tail_following_even_with_the_same_message_ids() {
    let root = TestConfigDir::new("nyaterm-ai-scroll-session");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let (app, vcx) = compact_host(&mut cx, root.path());
    edit_panel_snapshot(&app, vcx, |snapshot| {
        snapshot.messages = (0..24).map(transcript_message).collect::<Vec<_>>().into();
    });
    let scroll = vcx.update(|_, cx| app.read(cx).ai_panel.read(cx).transcript_scroll.clone());
    vcx.update(|_, cx| scroll.update(cx, |state, cx| state.scroll_to_item(4, cx)));
    compact_draw(&app, vcx);
    assert!(!vcx.update(|_, cx| scroll.read(cx).is_following_tail()));
    edit_panel_snapshot(&app, vcx, |snapshot| {
        snapshot.current_ai_session_id = "another-conversation".to_string();
    });
    assert!(vcx.update(|_, cx| scroll.read(cx).is_following_tail()));
    assert!(vcx.debug_bounds("ai-message-message-23").is_some());
}

#[test]
fn mixed_transcript_preserves_card_indices_and_remeasures_changed_agent_details() {
    let root = TestConfigDir::new("nyaterm-ai-transcript-projection");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let (app, vcx) = compact_host(&mut cx, root.path());
    let mut previous =
        vcx.update(|_, cx| app.read(cx).ai_panel.read(cx).snapshot().unwrap().clone());
    previous.messages = (0..2).map(transcript_message).collect::<Vec<_>>().into();
    previous.agent_steps = (0..20)
        .map(|step_index| AiAgentStepPresentation {
            step: AiAgentStepView {
                kind: crate::features::ai::presentation::AiAgentStepKind::Command,
                source_message_id: None,
                command_card_id: None,
                exit_code: None,
                step_index,
                status: AiAgentStepStatus::Completed,
                title: "Fixture step".to_string(),
                detail: String::new(),
                thought: None,
                command: None,
                observation: Some("Fixture output".to_string()),
            },
            thought_open: false,
            output_open: false,
        })
        .collect::<Vec<_>>()
        .into();
    previous.command_cards = (0..10)
        .map(|index| AiCommandCard {
            id: format!("card-{index}"),
            title: "Fixture command".to_string(),
            command: "echo fixture".to_string(),
            explanation: String::new(),
            risk_level: None,
            risk_reason: None,
            expected_effect: String::new(),
            rollback: None,
            category: None,
            references: Vec::new(),
            target_terminal_session_id: None,
            target: None,
        })
        .collect::<Vec<_>>()
        .into();
    let old_rows = AiTranscriptRow::project(&previous);
    assert_eq!(old_rows.len(), 26);
    assert!(matches!(
        old_rows[2],
        AiTranscriptRow::AgentStep {
            index: 4,
            step_index: 4
        }
    ));
    assert!(matches!(
        old_rows[25],
        AiTranscriptRow::Command { index: 7, .. }
    ));

    let mut next = previous.clone();
    next.messages = (0..3).map(transcript_message).collect::<Vec<_>>().into();
    let mut steps = next.agent_steps.to_vec();
    steps[19].output_open = true;
    next.agent_steps = steps.into();
    let rows = AiTranscriptRow::project(&next);
    let update = AiTranscriptUpdate::between(Some(&previous), &next, &old_rows, &rows);
    assert_eq!(
        update.splice,
        Some((2..2, 1)),
        "insert before the retained agent and command rows"
    );
    assert_eq!(
        update.remeasure,
        vec![18..19],
        "expanded output changes only its own row height"
    );
    assert!(!update.reset);
}

fn chat_card(id: &str) -> AiCommandCard {
    AiCommandCard {
        id: id.into(),
        title: "Inspect resources".into(),
        command: "echo fixture".into(),
        explanation: String::new(),
        risk_level: Some(nyaterm_core::RiskLevel::Low),
        risk_reason: Some("Read only".into()),
        expected_effect: "Show resources".into(),
        rollback: None,
        category: None,
        references: Vec::new(),
        target_terminal_session_id: Some("terminal-a".into()),
        target: None,
    }
}

fn linked_command_step(status: AiAgentStepStatus) -> AiAgentStepPresentation {
    AiAgentStepPresentation {
        step: AiAgentStepView {
            step_index: 0,
            status,
            kind: crate::features::ai::presentation::AiAgentStepKind::Command,
            source_message_id: Some("message-0".into()),
            command_card_id: Some("agent-fixture".into()),
            exit_code: None,
            title: "Inspect resources".into(),
            detail: String::new(),
            thought: None,
            command: Some("echo fixture".into()),
            observation: None,
        },
        thought_open: false,
        output_open: false,
    }
}

#[test]
fn transcript_deduplicates_ids_and_preserves_unrelated_commands_and_old_text() {
    use crate::features::ai::presentation::{AiAgentStepKind, AiCommandPhase};
    let root = TestConfigDir::new("nyaterm-ai-card-dedup");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let (app, vcx) = compact_host(&mut cx, root.path());
    let mut snapshot =
        vcx.update(|_, cx| app.read(cx).ai_panel.read(cx).snapshot().unwrap().clone());
    let mut message = transcript_message(0);
    let original = "Agent proposed `echo fixture`; legacy note";
    Arc::make_mut(&mut message).content = original.into();
    Arc::make_mut(&mut message).command_cards =
        vec![chat_card("agent-fixture"), chat_card("agent-fixture")];
    snapshot.messages = vec![message].into();
    snapshot.command_cards = vec![
        chat_card("agent-fixture"),
        chat_card("different-id"),
        chat_card("different-id"),
    ]
    .into();
    snapshot.agent_steps = vec![linked_command_step(AiAgentStepStatus::Running)].into();
    let rows = AiTranscriptRow::project(&snapshot);
    assert_eq!(rows.len(), 2);
    assert!(matches!(&rows[1], AiTranscriptRow::Command { id, index: 1 } if id == "different-id"));
    assert_eq!(snapshot.card_owner("agent-fixture"), Some("message-0"));
    assert_eq!(snapshot.messages[0].content, original);
    assert_eq!(
        snapshot.command_phase(&chat_card("agent-fixture")),
        AiCommandPhase::Running
    );
    snapshot.agent_steps = Vec::new().into();
    assert_eq!(
        snapshot.command_phase(&chat_card("agent-fixture")),
        AiCommandPhase::HistoryUnknown
    );
    let mut final_step = linked_command_step(AiAgentStepStatus::Completed);
    final_step.step.kind = AiAgentStepKind::FinalAnswer;
    final_step.step.command_card_id = None;
    snapshot.agent_steps = vec![final_step.clone()].into();
    assert_eq!(AiTranscriptRow::project(&snapshot).len(), 2);
    final_step.step.source_message_id = Some("unrelated-message".into());
    snapshot.agent_steps = vec![final_step].into();
    assert_eq!(
        AiTranscriptRow::project(&snapshot).len(),
        3,
        "unrelated steps must remain visible"
    );
}

#[test]
fn linked_execution_changes_remeasure_only_the_owning_message() {
    let root = TestConfigDir::new("nyaterm-ai-linked-height");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let (app, vcx) = compact_host(&mut cx, root.path());
    let mut previous =
        vcx.update(|_, cx| app.read(cx).ai_panel.read(cx).snapshot().unwrap().clone());
    let mut messages = (0..3).map(transcript_message).collect::<Vec<_>>();
    Arc::make_mut(&mut messages[0]).command_cards = vec![chat_card("agent-fixture")];
    previous.messages = messages.into();
    previous.agent_steps = vec![linked_command_step(AiAgentStepStatus::Running)].into();
    let mut next = previous.clone();
    let mut steps = next.agent_steps.to_vec();
    steps[0].step.status = AiAgentStepStatus::Completed;
    steps[0].step.observation = Some("resource output".into());
    steps[0].output_open = true;
    next.agent_steps = steps.into();
    let update = AiTranscriptUpdate::between(
        Some(&previous),
        &next,
        &AiTranscriptRow::project(&previous),
        &AiTranscriptRow::project(&next),
    );
    assert_eq!(update.remeasure, vec![0..1]);
    assert!(update.splice.is_none() && !update.remeasure_all && !update.reset);
}

#[test]
fn advancing_and_stopping_agent_refreshes_activity_in_retained_rows() {
    let root = TestConfigDir::new("nyaterm-ai-step-activity");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let (app, vcx) = compact_host(&mut cx, root.path());
    let mut previous =
        vcx.update(|_, cx| app.read(cx).ai_panel.read(cx).snapshot().unwrap().clone());
    let mut planning = linked_command_step(AiAgentStepStatus::Planning);
    planning.step.kind = crate::features::ai::presentation::AiAgentStepKind::Planning;
    planning.step.command_card_id = None;
    planning.step.source_message_id = None;
    previous.agent_steps = vec![planning.clone()].into();
    previous.running = true;
    assert!(previous.step_is_active(0));

    let mut next = previous.clone();
    planning.step.step_index = 1;
    next.agent_steps = vec![next.agent_steps[0].clone(), planning].into();
    assert!(!next.step_is_active(0));
    assert!(next.step_is_active(1));
    let update = AiTranscriptUpdate::between(
        Some(&previous),
        &next,
        &AiTranscriptRow::project(&previous),
        &AiTranscriptRow::project(&next),
    );
    assert!(update.splice.is_some());
    assert!(
        !AiTranscriptRow::project(&next)
            .iter()
            .any(|row| { matches!(row, AiTranscriptRow::AgentStep { step_index: 0, .. }) }),
        "earlier auxiliary activity is collapsed"
    );

    previous = next.clone();
    next.running = false;
    assert!(!next.step_is_active(1));
    let update = AiTranscriptUpdate::between(
        Some(&previous),
        &next,
        &AiTranscriptRow::project(&previous),
        &AiTranscriptRow::project(&next),
    );
    assert!(update.splice.is_some() && !update.reset);
    assert!(
        !AiTranscriptRow::project(&next)
            .iter()
            .any(|row| { matches!(row, AiTranscriptRow::AgentStep { .. }) })
    );
    next.agent_history_expanded = true;
    assert_eq!(
        AiTranscriptRow::project(&next)
            .iter()
            .filter(|row| { matches!(row, AiTranscriptRow::AgentStep { .. }) })
            .count(),
        2
    );
}

#[test]
fn command_buttons_and_running_animation_match_execution_phase_in_a_narrow_panel() {
    use crate::features::ai::presentation::{AiAgentStepKind, AiCommandPhase};
    let root = TestConfigDir::new("nyaterm-ai-command-controls");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let (app, vcx) = compact_host(&mut cx, root.path());
    for status in [
        AiAgentStepStatus::NeedsApproval,
        AiAgentStepStatus::Running,
        AiAgentStepStatus::Completed,
        AiAgentStepStatus::Failed,
        AiAgentStepStatus::Rejected,
        AiAgentStepStatus::Cancelled,
    ] {
        let mut step = linked_command_step(status);
        if status == AiAgentStepStatus::Completed {
            step.step.kind = AiAgentStepKind::Observation;
            step.step.exit_code = Some(0);
        }
        let phase = AiCommandPhase::from_step(&step.step);
        edit_panel_snapshot(&app, vcx, |snapshot| {
            let mut message = transcript_message(0);
            Arc::make_mut(&mut message).command_cards = vec![chat_card("agent-fixture")];
            snapshot.messages = vec![message].into();
            snapshot.command_cards = vec![chat_card("agent-fixture")].into();
            snapshot.agent_steps = vec![step].into();
        });
        assert_eq!(
            vcx.debug_bounds("ai-command-run-agent-fixture").is_some(),
            phase.offers_approval()
        );
        assert_eq!(
            vcx.debug_bounds("ai-command-reject-agent-fixture")
                .is_some(),
            phase.offers_approval()
        );
        assert_eq!(
            vcx.debug_bounds("ai-command-insert-agent-fixture")
                .is_some(),
            phase.offers_reuse()
        );
        assert_eq!(
            vcx.debug_bounds("ai-command-save-agent-fixture").is_some(),
            phase.offers_reuse()
        );
        assert!(vcx.debug_bounds("ai-command-copy-agent-fixture").is_some());
        assert_eq!(
            vcx.debug_bounds("ai-running-agent-fixture").is_some(),
            phase == AiCommandPhase::Running
        );
        let card = vcx.debug_bounds("ai-command-agent-fixture").unwrap();
        let viewport = vcx.debug_bounds("ai-transcript-viewport").unwrap();
        assert!(card.left() >= viewport.left() && card.right() <= viewport.right());
    }
    edit_panel_snapshot(&app, vcx, |snapshot| {
        snapshot.agent_steps = Vec::new().into()
    });
    assert!(vcx.debug_bounds("ai-command-run-agent-fixture").is_none());
    assert!(
        vcx.debug_bounds("ai-command-reject-agent-fixture")
            .is_none()
    );
    assert!(
        vcx.debug_bounds("ai-command-insert-agent-fixture")
            .is_some()
    );
}

#[test]
fn reasoning_disclosure_accepts_keyboard_and_persists_when_body_text_starts() {
    let root = TestConfigDir::new("nyaterm-ai-thought-keyboard");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let (app, vcx) = compact_host(&mut cx, root.path());
    let (job, id) = vcx.update(|_, cx| {
        app.update(cx, |app, cx| {
            let launch = app
                .ai
                .begin_chat_request("inspect".into(), AiMode::Ask, None);
            app.ai
                .apply_chat_delta(launch.job_id, "", Some("A useful thought"));
            let id = app.ai.chat_streaming_assistant_id().unwrap().to_string();
            app.flush_ai_panel_snapshot(cx);
            (launch.job_id, id)
        })
    });
    compact_draw(&app, vcx);
    assert!(vcx.update(|_, cx| app.read(cx).ai.expanded_message_thoughts().is_empty()));
    // The first focusable element in this transcript is its reasoning disclosure.
    vcx.update(|window, cx| {
        window.blur(cx);
        window.focus_next(cx);
        window.draw(cx).clear(cx);
    });
    let enter = gpui::Keystroke::parse("enter").unwrap();
    vcx.simulate_event(gpui::KeyDownEvent {
        keystroke: enter.clone(),
        is_held: false,
        prefer_character_input: false,
    });
    vcx.simulate_event(gpui::KeyUpEvent { keystroke: enter });
    compact_draw(&app, vcx);
    assert!(vcx.update(|_, cx| app.read(cx).ai.expanded_message_thoughts().contains(&id)));
    vcx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.ai.apply_chat_delta(job, "Answer", None);
            app.flush_ai_panel_snapshot(cx);
        })
    });
    compact_draw(&app, vcx);
    assert!(vcx.update(|_, cx| app.read(cx).ai.expanded_message_thoughts().contains(&id)));
    let space = gpui::Keystroke::parse("space").unwrap();
    vcx.simulate_event(gpui::KeyDownEvent {
        keystroke: space.clone(),
        is_held: false,
        prefer_character_input: false,
    });
    vcx.simulate_event(gpui::KeyUpEvent { keystroke: space });
    compact_draw(&app, vcx);
    assert!(!vcx.update(|_, cx| app.read(cx).ai.expanded_message_thoughts().contains(&id)));
}

#[test]
fn native_plan_stays_in_request_history_but_questions_remain_visible_and_focused() {
    use nyaterm_core::ai::harness::{
        AgentPlan, AgentPlanTask, AgentQuestion, AgentTaskStatus, AgentToolRegistry,
    };
    let root = TestConfigDir::new("nyaterm-ai-native-disclosure");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let (app, vcx) = compact_host(&mut cx, root.path());
    let user_id = vcx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.ai
                .begin_chat_request("inspect".into(), AiMode::Agent, None);
            let user_id = app.ai.chat_snapshot_messages()[0].id.clone();
            app.ai.begin_native_run(
                serde_json::from_value(serde_json::json!({
                    "mode": "agent", "action": "generate_command", "userInput": "inspect"
                }))
                .unwrap(),
            );
            app.ai
                .update_native_plan(AgentPlan {
                    tasks: vec![AgentPlanTask {
                        id: "inspect".into(),
                        description: "Inspect resources".into(),
                        status: AgentTaskStatus::InProgress,
                        verification: None,
                    }],
                })
                .unwrap();
            app.flush_ai_panel_snapshot(cx);
            user_id
        })
    });
    compact_draw(&app, vcx);
    assert!(vcx.debug_bounds("ai-native-run").is_none());
    vcx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.ai.toggle_execution_group(user_id.clone());
            app.flush_ai_panel_snapshot(cx);
        })
    });
    compact_draw(&app, vcx);
    assert!(vcx.debug_bounds("ai-native-run").is_some());
    vcx.update(|_, cx| app.update(cx, |app, cx| {
        app.ai.toggle_execution_group(user_id.clone());
        let call = AgentToolRegistry::parse(&[], r#"{"action":"request_user_input","arguments":{"questions":[{"id":"service","question":"Which service?"}]}}"#).unwrap();
        app.ai.begin_native_call(call).unwrap();
        app.ai.wait_native_user(vec![AgentQuestion {
            id: "service".into(), question: "Which service?".into(), options: None,
        }]).unwrap();
        app.flush_ai_panel_snapshot(cx);
    }));
    compact_draw(&app, vcx);
    let question = vcx.debug_bounds("ai-native-run").unwrap();
    let viewport = vcx.debug_bounds("ai-transcript-viewport").unwrap();
    assert!(question.top() >= viewport.top());
    assert!(question.bottom() <= viewport.bottom());
    vcx.update(|window, cx| {
        let panel = app.read(cx).ai_panel.read(cx);
        let snapshot = panel.snapshot().unwrap();
        assert!(snapshot.expanded_execution_groups.is_empty());
        assert!(
            snapshot.native_answer_inputs[0]
                .read(cx)
                .component_focus_handle(cx)
                .is_focused(window)
        );
        assert!(
            AiTranscriptRow::project(snapshot)
                .iter()
                .any(|row| { matches!(row, AiTranscriptRow::NativeRun { history: false, .. }) })
        );
    });
}

#[test]
fn execution_disclosures_accept_keyboard_and_mouse_without_following_a_long_script_to_the_end() {
    let root = TestConfigDir::new("nyaterm-ai-execution-interaction");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let (app, vcx) = compact_host(&mut cx, root.path());
    let user_id = vcx.update(|_, cx| {
        app.update(cx, |app, cx| {
            let launch = app
                .ai
                .begin_chat_request("inspect".into(), AiMode::Agent, None);
            let user_id = app.ai.chat_snapshot_messages()[0].id.clone();
            let mut card = chat_card("agent-fixture");
            card.command = "echo fixture\n".repeat(100);
            app.ai
                .finish_chat_job(
                    launch.job_id,
                    launch.session_id,
                    Ok(AiChatJobOutput {
                        native_call: None,
                        mode: AiMode::Agent,
                        text: "Inspect resources".into(),
                        reasoning: None,
                        command_cards: vec![card],
                        auto_execute_first: false,
                        approval_note: None,
                    }),
                )
                .unwrap();
            app.ai.upsert_agent_step(
                0,
                AiAgentStepStatus::Completed,
                crate::features::ai::presentation::AiAgentStepKind::Observation,
                "Observed",
                "Resources are available",
            );
            app.flush_ai_panel_snapshot(cx);
            user_id
        })
    });
    compact_draw(&app, vcx);
    let scroll = vcx.update(|_, cx| app.read(cx).ai_panel.read(cx).transcript_scroll.clone());
    assert!(vcx.update(|_, cx| app.read(cx).ai.expanded_execution_groups().is_empty()));
    vcx.update(|window, cx| {
        window.blur(cx);
        window.focus_next(cx);
        window.draw(cx).clear(cx);
    });
    let enter = gpui::Keystroke::parse("enter").unwrap();
    vcx.simulate_event(gpui::KeyDownEvent {
        keystroke: enter.clone(),
        is_held: false,
        prefer_character_input: false,
    });
    vcx.simulate_event(gpui::KeyUpEvent { keystroke: enter });
    compact_draw(&app, vcx);
    assert!(vcx.update(|_, cx| {
        app.read(cx)
            .ai
            .expanded_execution_groups()
            .contains(&user_id)
    }));
    assert!(vcx.debug_bounds("ai-command-body-agent-fixture").is_none());
    let command_toggle = vcx
        .debug_bounds("ai-command-toggle-agent-fixture-content")
        .unwrap();
    vcx.simulate_click(command_toggle.center(), gpui::Modifiers::default());
    compact_draw(&app, vcx);
    assert!(vcx.update(|_, cx| {
        app.read(cx)
            .ai
            .expanded_command_scripts()
            .contains("agent-fixture")
    }));
    assert!(!vcx.update(|_, cx| scroll.read(cx).is_following_tail()));
    let viewport = vcx.debug_bounds("ai-transcript-viewport").unwrap();
    let command_toggle = vcx
        .debug_bounds("ai-command-toggle-agent-fixture-content")
        .unwrap();
    assert!(command_toggle.top() >= viewport.top());
    assert!(command_toggle.bottom() <= viewport.bottom());
    assert!(
        vcx.debug_bounds("ai-command-body-agent-fixture")
            .unwrap()
            .size
            .height
            > viewport.size.height
    );
    vcx.simulate_click(command_toggle.center(), gpui::Modifiers::default());
    compact_draw(&app, vcx);
    assert!(!vcx.update(|_, cx| {
        app.read(cx)
            .ai
            .expanded_command_scripts()
            .contains("agent-fixture")
    }));
    assert!(vcx.debug_bounds("ai-command-body-agent-fixture").is_none());
    let selector = Box::leak(format!("ai-execution-toggle-{user_id}-content").into_boxed_str());
    let execution_toggle = vcx.debug_bounds(selector).unwrap();
    vcx.simulate_click(execution_toggle.center(), gpui::Modifiers::default());
    compact_draw(&app, vcx);
    assert!(!vcx.update(|_, cx| {
        app.read(cx)
            .ai
            .expanded_execution_groups()
            .contains(&user_id)
    }));
    assert!(
        vcx.debug_bounds("ai-command-toggle-agent-fixture")
            .is_none()
    );
}

#[test]
fn historic_steps_without_a_command_card_fold_their_script_inside_the_request() {
    let root = TestConfigDir::new("nyaterm-ai-historic-command");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let (app, vcx) = compact_host(&mut cx, root.path());
    edit_panel_snapshot(&app, vcx, |snapshot| {
        let mut messages: Vec<_> = (0..3).map(transcript_message).collect();
        for message in &mut messages[1..] {
            Arc::make_mut(message).role = AiMessageRole::Assistant;
        }
        let mut step = linked_command_step(AiAgentStepStatus::Completed);
        step.step.source_message_id = Some("message-1".into());
        step.step.command_card_id = None;
        snapshot.messages = messages.into();
        snapshot.agent_steps = vec![step].into();
        snapshot.expanded_execution_groups = vec!["message-0".into()].into();
    });
    assert!(vcx.debug_bounds("step-command-message-1-0").is_some());
    assert!(vcx.debug_bounds("ai-step-command-body-0").is_none());
    assert!(vcx.debug_bounds("ai-answer-message-2").is_some());
    edit_panel_snapshot(&app, vcx, |snapshot| {
        snapshot.expanded_command_scripts = vec!["step-command-message-1-0".into()].into();
    });
    assert!(vcx.debug_bounds("ai-step-command-body-0").is_some());
    assert!(vcx.debug_bounds("ai-answer-message-2").is_some());
}

#[test]
fn request_disclosure_keeps_the_user_and_final_answer_visible_and_hides_commands() {
    let root = TestConfigDir::new("nyaterm-ai-request-disclosure");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let (app, vcx) = compact_host(&mut cx, root.path());
    edit_panel_snapshot(&app, vcx, |snapshot| {
        let mut messages: Vec<_> = (0..4).map(transcript_message).collect();
        for message in &mut messages[1..] {
            Arc::make_mut(message).role = AiMessageRole::Assistant;
        }
        Arc::make_mut(&mut messages[1]).content = "I will inspect current resource usage.".into();
        let mut card = chat_card("agent-fixture");
        card.command = "echo resource\n".repeat(12);
        card.explanation = "Collect current resource metrics.".into();
        Arc::make_mut(&mut messages[2]).content = card.explanation.clone();
        Arc::make_mut(&mut messages[2]).command_cards = vec![card];
        Arc::make_mut(&mut messages[3]).content = "Resource usage is within normal limits.".into();
        snapshot.messages = messages.into();
        let mut step = linked_command_step(AiAgentStepStatus::Completed);
        step.step.source_message_id = Some("message-2".into());
        step.step.exit_code = Some(0);
        step.step.observation = Some("CPU 2%\nMemory 40%".into());
        snapshot.agent_steps = vec![step].into();
    });
    let collapsed = vcx.update(|_, cx| app.read(cx).ai_panel.read(cx).snapshot().unwrap().clone());
    let rows = AiTranscriptRow::project(&collapsed);
    assert_eq!(rows.len(), 3);
    assert!(matches!(
        &rows[0],
        AiTranscriptRow::Message { index: 0, .. }
    ));
    assert!(matches!(&rows[1], AiTranscriptRow::Execution { group } if group.id == "message-0"));
    assert!(matches!(
        &rows[2],
        AiTranscriptRow::FinalMessage { index: 3, .. }
    ));
    assert!(vcx.debug_bounds("ai-message-message-0").is_some());
    assert!(vcx.debug_bounds("ai-execution-toggle-message-0").is_some());
    assert!(vcx.debug_bounds("ai-answer-message-3").is_some());
    assert!(vcx.debug_bounds("ai-message-message-1").is_none());
    assert!(vcx.debug_bounds("ai-command-body-agent-fixture").is_none());
    edit_panel_snapshot(&app, vcx, |snapshot| {
        snapshot.expanded_execution_groups = vec!["message-0".into()].into();
    });
    assert!(vcx.debug_bounds("ai-message-message-1").is_some());
    assert!(
        vcx.debug_bounds("ai-command-toggle-agent-fixture")
            .is_some()
    );
    assert!(vcx.debug_bounds("ai-command-body-agent-fixture").is_none());
    assert!(vcx.debug_bounds("ai-answer-message-3").is_some());
    edit_panel_snapshot(&app, vcx, |snapshot| {
        snapshot.expanded_command_scripts = vec!["agent-fixture".into()].into();
    });
    assert!(
        vcx.debug_bounds("ai-command-body-agent-fixture")
            .unwrap()
            .size
            .height
            > px(62.)
    );
    assert!(
        vcx.debug_bounds("ai-command-output-agent-fixture")
            .is_some()
    );
    edit_panel_snapshot(&app, vcx, |snapshot| {
        snapshot.expanded_execution_groups = Vec::new().into();
    });
    assert!(vcx.debug_bounds("ai-command-body-agent-fixture").is_none());
    assert!(vcx.debug_bounds("ai-answer-message-3").is_some());
    assert_eq!(AiTranscriptRow::project(&collapsed).len(), 3);
}

#[test]
fn execution_groups_do_not_merge_requests_or_promote_tool_narration_to_final_answers() {
    use crate::features::ai::panel::execution::AiExecutionGroup;
    use crate::features::ai::presentation::{AiAgentStepKind, AiResponsePhase};
    let root = TestConfigDir::new("nyaterm-ai-request-boundaries");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let (app, vcx) = compact_host(&mut cx, root.path());
    let mut snapshot =
        vcx.update(|_, cx| app.read(cx).ai_panel.read(cx).snapshot().unwrap().clone());
    let mut messages: Vec<_> = (0..6).map(transcript_message).collect();
    for index in [1, 2, 4, 5] {
        Arc::make_mut(&mut messages[index]).role = AiMessageRole::Assistant;
    }
    Arc::make_mut(&mut messages[1]).command_cards = vec![chat_card("agent-first")];
    Arc::make_mut(&mut messages[4]).command_cards = vec![chat_card("agent-second")];
    snapshot.messages = messages.into();
    let groups = AiExecutionGroup::project(&snapshot);
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0].id, "message-0");
    assert_eq!(groups[0].final_message, Some(2));
    assert_eq!(groups[1].id, "message-3");
    assert_eq!(groups[1].final_message, Some(5));
    snapshot.expanded_execution_groups = vec!["message-0".into()].into();
    let rows = AiTranscriptRow::project(&snapshot);
    assert!(
        rows.iter()
            .any(|row| matches!(row, AiTranscriptRow::ActivityMessage { index: 1, .. }))
    );
    assert!(
        !rows
            .iter()
            .any(|row| matches!(row, AiTranscriptRow::ActivityMessage { index: 4, .. }))
    );
    let mut step = linked_command_step(AiAgentStepStatus::Running);
    step.step.kind = AiAgentStepKind::ToolProgress;
    step.step.command_card_id = None;
    step.step.source_message_id = Some("message-5".into());
    snapshot.agent_steps = vec![step].into();
    snapshot.running = true;
    snapshot.streaming_assistant_id = Some("message-5".into());
    snapshot.response_phase = AiResponsePhase::ToolArguments;
    let groups = AiExecutionGroup::project(&snapshot);
    assert!(!groups[0].is_live(&snapshot));
    assert!(groups[1].is_live(&snapshot));
    assert_eq!(
        groups[1].final_message, None,
        "tool narration stays in the process"
    );
}

#[test]
fn tool_preparation_belongs_to_the_streaming_message_and_remeasures_it() {
    use crate::features::ai::presentation::{AiAgentStepKind, AiResponsePhase};
    let root = TestConfigDir::new("nyaterm-ai-inline-progress");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let (app, vcx) = compact_host(&mut cx, root.path());
    let mut previous =
        vcx.update(|_, cx| app.read(cx).ai_panel.read(cx).snapshot().unwrap().clone());
    let mut message = transcript_message(0);
    Arc::make_mut(&mut message).role = AiMessageRole::Assistant;
    Arc::make_mut(&mut message).content.clear();
    previous.messages = vec![message].into();
    previous.running = true;
    previous.streaming_assistant_id = Some("message-0".into());
    previous.response_phase = AiResponsePhase::ToolArguments;
    let mut next = previous.clone();
    let mut step = linked_command_step(AiAgentStepStatus::Tool);
    step.step.kind = AiAgentStepKind::ToolProgress;
    step.step.command_card_id = None;
    step.step.source_message_id = None;
    step.step.command = None;
    step.step.title = "Tool tool".into();
    step.step.detail = "Streaming arguments (+2 chars)".into();
    next.agent_steps = vec![step].into();
    let rows = AiTranscriptRow::project(&next);
    assert_eq!(rows.len(), 1, "preparation becomes one execution header");
    assert!(matches!(&rows[0], AiTranscriptRow::Execution { .. }));
    let update = AiTranscriptUpdate::between(
        Some(&previous),
        &next,
        &AiTranscriptRow::project(&previous),
        &rows,
    );
    assert!(update.splice.is_some());
    next.expanded_execution_groups = vec!["message-0".into()].into();
    edit_panel_snapshot(&app, vcx, |snapshot| *snapshot = next.clone());
    assert!(vcx.debug_bounds("ai-thinking-message-0").is_none());
    assert!(vcx.debug_bounds("ai-agent-step-0").is_some());
    assert!(vcx.debug_bounds("ai-step-running-0").is_some());
}

#[test]
fn command_expansion_preserves_results_and_approval_shows_the_complete_script() {
    let root = TestConfigDir::new("nyaterm-ai-script-expansion");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let (app, vcx) = compact_host(&mut cx, root.path());
    edit_panel_snapshot(&app, vcx, |snapshot| {
        let mut message = transcript_message(0);
        Arc::make_mut(&mut message).role = AiMessageRole::Assistant;
        let mut card = chat_card("agent-fixture");
        card.title = "Agent Command".into();
        card.explanation = "Inspect current resource usage".into();
        card.command = "echo resource\n".repeat(12);
        Arc::make_mut(&mut message).content = card.explanation.clone();
        Arc::make_mut(&mut message).command_cards = vec![card];
        snapshot.messages = vec![message].into();
        let mut step = linked_command_step(AiAgentStepStatus::Completed);
        step.step.exit_code = Some(0);
        step.step.observation = Some("CPU 2%\nMemory 40%".into());
        step.step.thought = Some("Inspect current resource usage".into());
        snapshot.agent_steps = vec![step].into();
        snapshot.expanded_execution_groups = vec!["message-0".into()].into();
    });
    assert!(vcx.debug_bounds("ai-answer-message-0").is_none());
    assert!(
        vcx.debug_bounds("ai-command-thought-agent-fixture")
            .is_none()
    );
    assert!(vcx.debug_bounds("ai-command-body-agent-fixture").is_none());
    assert!(
        vcx.debug_bounds("ai-command-toggle-agent-fixture")
            .is_some()
    );
    let previous = vcx.update(|_, cx| app.read(cx).ai_panel.read(cx).snapshot().unwrap().clone());
    edit_panel_snapshot(&app, vcx, |snapshot| {
        snapshot.expanded_command_scripts = vec!["agent-fixture".into()].into();
    });
    assert!(
        vcx.debug_bounds("ai-command-body-agent-fixture")
            .unwrap()
            .size
            .height
            > px(62.)
    );
    let next = vcx.update(|_, cx| app.read(cx).ai_panel.read(cx).snapshot().unwrap().clone());
    let update = AiTranscriptUpdate::between(
        Some(&previous),
        &next,
        &AiTranscriptRow::project(&previous),
        &AiTranscriptRow::project(&next),
    );
    assert_eq!(update.remeasure, vec![1..2]);
    edit_panel_snapshot(&app, vcx, |snapshot| {
        snapshot.expanded_command_scripts = Vec::new().into();
        snapshot.expanded_execution_groups = Vec::new().into();
        let mut steps = snapshot.agent_steps.to_vec();
        steps[0].step.status = AiAgentStepStatus::NeedsApproval;
        steps[0].step.observation = None;
        snapshot.agent_steps = steps.into();
    });
    assert!(
        vcx.debug_bounds("ai-command-body-agent-fixture")
            .unwrap()
            .size
            .height
            > px(62.)
    );
    assert!(
        vcx.debug_bounds("ai-command-script-agent-fixture")
            .is_none()
    );
    assert!(vcx.debug_bounds("ai-command-run-agent-fixture").is_some());
}

#[test]
fn long_commands_and_collapsed_output_fit_light_and_dark_panel_layouts() {
    let root = TestConfigDir::new("nyaterm-ai-command-themes");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let (app, vcx) = compact_host(&mut cx, root.path());
    for theme in ["github-dark", "solarized-light"] {
        let palette = nyaterm_ui::theme_palette(theme);
        vcx.update(|_, cx| {
            nyaterm_ui::apply_component_theme(palette, gpui::font("Arial"), px(12.), cx)
        });
        edit_panel_snapshot(&app, vcx, |snapshot| {
            let mut card = chat_card("agent-fixture");
            card.command = "printf '%s' ".repeat(32);
            let mut message = transcript_message(0);
            Arc::make_mut(&mut message).command_cards = vec![card];
            snapshot.messages = vec![message].into();
            snapshot.chrome.palette = palette;
            let mut step = linked_command_step(AiAgentStepStatus::Completed);
            step.step.kind = crate::features::ai::presentation::AiAgentStepKind::Observation;
            step.step.exit_code = Some(0);
            step.step.observation = Some("resource output\n".repeat(400));
            snapshot.agent_steps = vec![step].into();
        });
        let command = vcx.debug_bounds("ai-command-body-agent-fixture").unwrap();
        let viewport = vcx.debug_bounds("ai-transcript-viewport").unwrap();
        assert!(command.left() >= viewport.left() && command.right() <= viewport.right());
        assert!(
            command.size.height <= px(62.),
            "long command preview must stay bounded"
        );
        assert!(
            vcx.debug_bounds("ai-command-script-agent-fixture")
                .is_some()
        );
        let result = vcx.debug_bounds("ai-command-result-agent-fixture").unwrap();
        assert!(result.top() < command.top(), "results precede the command");
        assert!(
            vcx.debug_bounds("ai-command-output-agent-fixture")
                .is_some()
        );
        for selector in [
            "ai-command-output-agent-fixture-content",
            "ai-command-details-agent-fixture-content",
        ] {
            let content = vcx.debug_bounds(selector).unwrap();
            let card = vcx.debug_bounds("ai-command-agent-fixture").unwrap();
            assert_eq!(content.size.height, px(20.));
            assert!(
                content.left() - card.left() < px(16.),
                "disclosures must align with the card's left padding"
            );
            assert!(
                content.size.width < card.size.width * 0.75,
                "disclosure buttons must keep their content width"
            );
        }
        assert!(vcx.debug_bounds("ai-command-copy-agent-fixture").is_some());
    }
}
