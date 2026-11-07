use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;

use gpui::{
    AppContext as _, Entity, IntoElement, ParentElement as _, Render, Styled as _, TestAppContext,
    VisualTestContext, div, px,
};
use nyaterm_core::{AiAgentKind, AiMode};

use super::{
    CompactAiHost, ai_snapshot_sets, app, compact_draw, compact_host, edit_panel_snapshot,
    transcript_message,
};
use crate::features::NyaTermApp;
use crate::test_support::TestConfigDir;

struct HistoryHost {
    app: Entity<NyaTermApp>,
}

#[test]
fn closing_a_terminal_cancels_its_external_agent_without_cancelling_other_chats() {
    for kind in [AiAgentKind::ClaudeCode, AiAgentKind::Codex] {
        let root = TestConfigDir::new("nyaterm-ai-close-external-agent");
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx, root.path());
        cx.update_entity(&app, |app, cx| {
            app.ai.switch_visible_scope("terminal:closed");
            app.ai.set_chat_run_mode(AiMode::Agent, kind);
            let closed = app
                .ai
                .begin_chat_request("inspect".into(), AiMode::Agent, None);
            app.ai.switch_visible_scope("terminal:other");
            let other = app.ai.begin_chat_request("other".into(), AiMode::Ask, None);

            app.remove_session_state("closed", cx);

            assert!(closed.cancel.load(Ordering::Acquire));
            assert!(!other.cancel.load(Ordering::Acquire));
            assert_eq!(app.ai.active_scope_key(), "terminal:other");
            app.ai.switch_scope("terminal:closed");
            assert!(!app.ai.chat_or_agent_is_running());
            assert!(!app.ai.apply_chat_delta(closed.job_id, "late", None));
            app.ai.switch_scope("terminal:other");
            app.ai.cancel_chat_and_agent();
        });
    }
}

#[test]
fn opening_the_current_history_entry_keeps_live_message_ids_and_draft() {
    let root = TestConfigDir::new("nyaterm-ai-current-history");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app = app(&mut cx, root.path());
    cx.update_entity(&app, |app, cx| {
        app.ai.switch_visible_scope("terminal:fixture");
        let launch = app
            .ai
            .begin_chat_request("question".into(), AiMode::Ask, None);
        app.ai.apply_chat_delta(launch.job_id, "live answer", None);
        app.ai.cancel_chat_and_agent();
        app.ai.set_chat_prompt_draft("unsent".into());
        app.ai.toggle_history();
        let title = app.ai_header_presentation().conversation_title;
        let id = app.ai.chat_session_id().to_string();
        let messages = app.ai.chat_messages().to_vec();
        app.load_ai_session_messages(id.clone(), cx);
        assert_eq!(app.ai.chat_session_id(), id);
        assert_eq!(app.ai.chat_messages(), messages);
        assert_eq!(app.ai.chat_prompt_draft(), "unsent");
        assert!(!app.ai.history_is_open());
        assert!(!app.ai.history_is_pending());
        assert_eq!(app.ai_header_presentation().conversation_title, title);
    });
}

impl Render for HistoryHost {
    fn render(
        &mut self,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement {
        self.app.update(cx, |app, cx| {
            div().w(px(309.)).h(px(575.)).child(app.side_panel_stack(
                crate::models::PanelSide::Right,
                window,
                cx,
            ))
        })
    }
}

#[test]
fn successful_history_load_restores_focus_from_the_inner_search_field() {
    let root = TestConfigDir::new("nyaterm-ai-history-load-focus");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let (app, vcx) = compact_host(&mut cx, root.path());
    vcx.update(|window, cx| {
        let field = app
            .read(cx)
            .ai_panel
            .read(cx)
            .snapshot()
            .unwrap()
            .prompt_input
            .clone();
        window.focus(&field.read(cx).focus_handle(), cx);
    });
    vcx.run_until_parked();
    let original_focus = vcx.update(|window, cx| window.focused(cx).unwrap());
    let (source, job) = vcx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.ai.remember_history_focus(window.focused(cx));
            app.ai.toggle_history();
            app.flush_ai_panel_snapshot(cx);
            let field = app.existing_text_input("ai.history-search").unwrap();
            window.focus(&field.read(cx).focus_handle(), cx);
            let source = app.ai.chat_session_id().to_string();
            let job = app.ai.begin_history_operation("load").unwrap();
            (source, job)
        })
    });
    compact_draw(&app, vcx);
    vcx.update(|window, cx| {
        assert_ne!(window.focused(cx), Some(original_focus.clone()));
        app.update(cx, |app, cx| {
            assert!(app.ai.finish_history_message_load(
                job,
                &source,
                "saved".into(),
                Ok(Vec::new()),
                "loaded".into()
            ));
            app.flush_ai_panel_snapshot(cx);
        });
    });
    compact_draw(&app, vcx);
    vcx.update(|window, cx| {
        assert_eq!(app.read(cx).ai.chat_session_id(), "saved");
        assert!(!app.read(cx).ai.history_is_open());
        assert_eq!(window.focused(cx), Some(original_focus));
        assert!(
            app.read(cx)
                .existing_text_input("ai.history-search")
                .is_none()
        );
    });
}

#[test]
fn history_header_preserves_conversation_and_gives_the_list_a_visible_viewport() {
    let root = TestConfigDir::new("nyaterm-ai-history-header");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app = app(&mut cx, root.path());
    let original = cx.update_entity(&app, |app, cx| {
        app.sync_component_theme(cx);
        app.ensure_panel_open(crate::models::NavItem::AiAssistant, cx);
        let source = app.ai.chat_session_id().to_string();
        let job = app.ai.begin_history_operation("fixture").unwrap();
        app.ai.finish_history_message_load(
            job,
            &source,
            source.clone(),
            Ok((0..24)
                .map(|index| (*transcript_message(index)).clone())
                .collect()),
            "fixture".into(),
        );
        let launch = app
            .ai
            .begin_chat_request("fixture question".to_string(), AiMode::Ask, None);
        app.ai
            .apply_chat_delta(launch.job_id, "fixture answer", None);
        app.ai.set_chat_prompt_draft("unsent draft".to_string());
        app.flush_ai_panel_snapshot(cx);
        (
            app.ai.active_scope_key().to_string(),
            app.ai.chat_session_id().to_string(),
            app.ai.chat_messages().to_vec(),
        )
    });
    let host_app = app.clone();
    let (_, vcx) = cx.add_window_view(move |_, _| HistoryHost { app: host_app });
    let vcx: &mut VisualTestContext = vcx;
    compact_draw(&app, vcx);
    let scroll = vcx.update(|_, cx| app.read(cx).ai_panel.read(cx).transcript_scroll.clone());
    vcx.update(|_, cx| scroll.update(cx, |state, cx| state.scroll_to_item(4, cx)));
    compact_draw(&app, vcx);
    let anchor = vcx.debug_bounds("ai-message-message-4").unwrap().top();
    let button = vcx.debug_bounds("ai-header-history-toggle").unwrap();
    vcx.update(|window, cx| {
        let field = app
            .read(cx)
            .ai_panel
            .read(cx)
            .snapshot()
            .unwrap()
            .prompt_input
            .clone();
        let focus = field.read(cx).focus_handle();
        window.focus(&focus, cx);
    });
    vcx.run_until_parked();
    let previous_focus = vcx.update(|window, cx| window.focused(cx).unwrap());
    for _ in 0..3 {
        vcx.simulate_click(button.center(), gpui::Modifiers::default());
        compact_draw(&app, vcx);
        vcx.update(|_, cx| {
            let state = &app.read(cx).ai;
            assert!(state.history_is_open());
            assert_eq!(state.active_scope_key(), original.0);
            assert_eq!(state.chat_session_id(), original.1);
            assert_eq!(state.chat_messages(), original.2);
            assert_eq!(state.chat_prompt_draft(), "unsent draft");
            assert!(state.chat_or_agent_is_running());
        });
        let viewport = vcx.debug_bounds("ai-history-viewport").unwrap();
        assert!(
            viewport.size.height > px(32.),
            "history body collapsed: {viewport:?}"
        );
        vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.apply_ai_history_search("no match".into(), cx)
            })
        });
        compact_draw(&app, vcx);
        vcx.simulate_click(button.center(), gpui::Modifiers::default());
        compact_draw(&app, vcx);
        vcx.update(|window, cx| {
            let state = &app.read(cx).ai;
            assert!(!state.history_is_open());
            assert_eq!(state.chat_session_id(), original.1);
            assert_eq!(state.chat_messages(), original.2);
            assert_eq!(state.chat_prompt_draft(), "unsent draft");
            assert!(state.chat_or_agent_is_running());
            assert_eq!(window.focused(cx), Some(previous_focus.clone()));
        });
        assert_eq!(
            vcx.debug_bounds("ai-message-message-4").unwrap().top(),
            anchor
        );
        assert!(!vcx.update(|_, cx| scroll.read(cx).is_following_tail()));
    }
}

#[test]
fn snapshot_refresh_does_not_switch_or_create_a_conversation() {
    let root = TestConfigDir::new("nyaterm-ai-snapshot-scope");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app = app(&mut cx, root.path());
    cx.update_entity(&app, |app, cx| {
        app.ai.switch_visible_scope("terminal:fixture");
        let launch = app
            .ai
            .begin_chat_request("question".into(), AiMode::Ask, None);
        app.ai.apply_chat_delta(launch.job_id, "answer", None);
        let session = app.ai.chat_session_id().to_string();
        let messages = app.ai.chat_messages().to_vec();
        app.ai.toggle_history();
        app.flush_ai_panel_snapshot(cx);
        assert_eq!(app.ai.active_scope_key(), "terminal:fixture");
        assert_eq!(app.ai.chat_session_id(), session);
        assert_eq!(app.ai.chat_messages(), messages);
    });
}

#[test]
fn history_exposes_all_two_hundred_sessions_and_scrolls_in_a_short_panel() {
    for theme in ["github-dark", "github-light"] {
        let root = TestConfigDir::new("nyaterm-ai-history-list");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let (app, vcx) = compact_host(&mut cx, root.path());
        // Resize the actual viewport, rather than merely the shell's projection.
        vcx.update(|window, cx| {
            window.resize(gpui::size(px(320.), px(280.)));
            window
                .root::<CompactAiHost>()
                .unwrap()
                .unwrap()
                .update(cx, |host, cx| {
                    host.height = 280.;
                    cx.notify();
                });
            app.update(cx, |app, cx| {
                app.settings.set_appearance_theme(theme.to_string());
                app.sync_component_theme(cx);
            });
        });
        edit_panel_snapshot(&app, vcx, |snapshot| {
            snapshot.history_open = true;
            snapshot.history_sessions = (0..200)
                .map(|index| nyaterm_core::AiSession {
                    id: format!("history-{index}"),
                    title: format!("Fixture {index}"),
                    agent_kind: Default::default(),
                    scope: Default::default(),
                    connection_id: None,
                    created_at: "2000-01-01T00:00:00Z".into(),
                    updated_at: "2000-01-01T00:00:00Z".into(),
                    external_session_id: None,
                    backend_metadata: None,
                })
                .collect::<Vec<_>>()
                .into();
        });
        let scroll = vcx.update(|_, cx| app.read(cx).ai_panel.read(cx).history_scroll.clone());
        assert!(scroll.max_offset().y > px(0.));
        scroll.set_offset(gpui::point(px(0.), -scroll.max_offset().y));
        compact_draw(&app, vcx);
        let viewport = vcx.debug_bounds("ai-history-viewport").unwrap();
        let last = vcx.debug_bounds("ai-history-session-history-199").unwrap();
        assert!(last.bottom() <= viewport.bottom());
        assert!(last.top() >= viewport.top());
    }
}

#[test]
fn content_cache_parses_only_changed_messages_and_ignores_a_replaced_session() {
    let root = TestConfigDir::new("nyaterm-ai-content-cache");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let (app, vcx) = compact_host(&mut cx, root.path());
    edit_panel_snapshot(&app, vcx, |snapshot| {
        snapshot.messages = (0..2000).map(transcript_message).collect::<Vec<_>>().into();
    });
    let parses = vcx.update(|_, cx| app.read(cx).ai_panel.read(cx).content.parses);
    assert_eq!(parses, 2000);
    edit_panel_snapshot(&app, vcx, |snapshot| {
        snapshot.prompt_draft = "draft".into();
        snapshot.history_query = "query".into();
    });
    assert_eq!(
        vcx.update(|_, cx| app.read(cx).ai_panel.read(cx).content.parses),
        parses
    );
    edit_panel_snapshot(&app, vcx, |snapshot| {
        let mut messages = snapshot.messages.to_vec();
        Arc::make_mut(&mut messages[1999]).content =
            "# Answer\n\n**Bold** and `code`.\n".repeat(1024);
        Arc::make_mut(&mut messages[1999]).reasoning_content = Some("Reasoning\n".repeat(1024));
        snapshot.messages = messages.into();
    });
    assert_eq!(
        vcx.update(|_, cx| app.read(cx).ai_panel.read(cx).content.parses),
        parses + 1
    );
    vcx.update(|_, cx| {
        let panel = app.read(cx).ai_panel.clone();
        panel.update(cx, |panel, cx| {
            for text in ["obsolete", "latest"] {
                let mut snapshot = panel.snapshot().unwrap().clone();
                let mut messages = snapshot.messages.to_vec();
                Arc::make_mut(&mut messages[1999]).content = text.into();
                snapshot.messages = messages.into();
                panel.set_snapshot(snapshot, cx);
            }
        });
    });
    compact_draw(&app, vcx);
    assert_eq!(
        vcx.update(|_, cx| app
            .read(cx)
            .ai_panel
            .read(cx)
            .prepared_message("message-1999")
            .unwrap()
            .display
            .clone()),
        "latest"
    );
    vcx.update(|_, cx| {
        let panel = app.read(cx).ai_panel.clone();
        panel.update(cx, |panel, cx| {
            let mut snapshot = panel.snapshot().unwrap().clone();
            let mut messages = snapshot.messages.to_vec();
            Arc::make_mut(&mut messages[1999]).content = "obsolete".into();
            snapshot.messages = messages.into();
            panel.set_snapshot(snapshot.clone(), cx);
            snapshot.current_ai_session_id = "replacement".into();
            snapshot.messages = vec![transcript_message(1)].into();
            panel.set_snapshot(snapshot, cx);
        });
    });
    compact_draw(&app, vcx);
    vcx.update(|_, cx| {
        let panel = app.read(cx).ai_panel.read(cx);
        assert!(panel.prepared_message("message-1999").is_none());
        assert_eq!(
            panel.prepared_message("message-1").unwrap().display,
            "fixture output\n".repeat(6)
        );
    });
}

#[test]
fn streaming_bursts_coalesce_without_losing_text_and_background_sessions_do_not_flush() {
    use crate::features::runtime_jobs::AiChatWorkerEvent;
    use std::time::Duration;
    let root = TestConfigDir::new("nyaterm-ai-stream-burst");
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let (app, vcx) = compact_host(&mut cx, root.path());
    let launch = vcx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.start_ai_chat_event_drain(cx);
            let source = app.ai.chat_session_id().to_string();
            let job = app.ai.begin_history_operation("fixture").unwrap();
            app.ai.finish_history_message_load(
                job,
                &source,
                "fixture".into(),
                Ok((0..1998)
                    .map(|index| (*transcript_message(index)).clone())
                    .collect()),
                "fixture".into(),
            );
            let launch = app
                .ai
                .begin_chat_request("question".into(), AiMode::Ask, None);
            app.flush_ai_panel_snapshot(cx);
            launch
        })
    });
    vcx.run_until_parked();
    let before = vcx.update(|_, cx| ai_snapshot_sets(&app, cx));
    let started = Instant::now();
    for _ in 0..1000 {
        launch
            .tx
            .unbounded_send(AiChatWorkerEvent::Delta {
                job_id: launch.job_id,
                session_id: launch.session_id.clone(),
                text_delta: "x".into(),
                reasoning_delta: Some("r".into()),
            })
            .unwrap();
    }
    vcx.run_until_parked();
    assert_eq!(vcx.update(|_, cx| ai_snapshot_sets(&app, cx)), before);
    vcx.executor().advance_clock(Duration::from_millis(33));
    vcx.run_until_parked();
    assert_eq!(vcx.update(|_, cx| ai_snapshot_sets(&app, cx)), before + 1);
    vcx.update(|_, cx| {
        let state = &app.read(cx).ai;
        let message = state.chat_messages().last().unwrap();
        assert_eq!(message.content, "x".repeat(1000));
        assert_eq!(
            message.reasoning_content.as_deref(),
            Some("r".repeat(1000).as_str())
        );
    });
    eprintln!(
        "AI burst: 2000 messages, 1000 deltas, 1 snapshot, elapsed {:?}",
        started.elapsed()
    );
    vcx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.ai.switch_visible_scope("terminal:background");
            app.ai
                .begin_chat_request("background".into(), AiMode::Ask, None);
            app.flush_ai_panel_snapshot(cx);
        })
    });
    let before = vcx.update(|_, cx| ai_snapshot_sets(&app, cx));
    launch
        .tx
        .unbounded_send(AiChatWorkerEvent::Delta {
            job_id: launch.job_id,
            session_id: launch.session_id.clone(),
            text_delta: "behind".into(),
            reasoning_delta: None,
        })
        .unwrap();
    vcx.run_until_parked();
    vcx.executor().advance_clock(Duration::from_millis(33));
    vcx.run_until_parked();
    assert_eq!(vcx.update(|_, cx| ai_snapshot_sets(&app, cx)), before);
    vcx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.ai.switch_visible_scope("unbound:");
            app.flush_ai_panel_snapshot(cx);
            assert!(
                app.ai
                    .chat_messages()
                    .last()
                    .unwrap()
                    .content
                    .ends_with("behind")
            );
        })
    });
    let before = vcx.update(|_, cx| ai_snapshot_sets(&app, cx));
    launch
        .tx
        .unbounded_send(AiChatWorkerEvent::Delta {
            job_id: launch.job_id,
            session_id: launch.session_id.clone(),
            text_delta: "final".into(),
            reasoning_delta: None,
        })
        .unwrap();
    launch
        .tx
        .unbounded_send(AiChatWorkerEvent::Finished(Box::new(
            crate::features::runtime_jobs::AiChatJobResult {
                job_id: launch.job_id,
                session_id: launch.session_id.clone(),
                result: Err("fixture failure".into()),
            },
        )))
        .unwrap();
    vcx.run_until_parked();
    assert_eq!(vcx.update(|_, cx| ai_snapshot_sets(&app, cx)), before + 1);
    vcx.executor().advance_clock(Duration::from_millis(33));
    vcx.run_until_parked();
    assert_eq!(vcx.update(|_, cx| ai_snapshot_sets(&app, cx)), before + 1);
}

#[test]
#[ignore = "synthetic performance comparison; run explicitly with --ignored --nocapture"]
fn compare_immediate_and_coalesced_stream_updates_for_two_thousand_messages() {
    for coalesced in [false, true] {
        let root = TestConfigDir::new("nyaterm-ai-stream-profile");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let (app, vcx) = compact_host(&mut cx, root.path());
        let launch = vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                let source = app.ai.chat_session_id().to_string();
                let job = app.ai.begin_history_operation("fixture").unwrap();
                app.ai.finish_history_message_load(
                    job,
                    &source,
                    "fixture".into(),
                    Ok((0..1998)
                        .map(|index| (*transcript_message(index)).clone())
                        .collect()),
                    "fixture".into(),
                );
                let launch = app
                    .ai
                    .begin_chat_request("fixture".into(), AiMode::Ask, None);
                app.flush_ai_panel_snapshot(cx);
                launch
            })
        });
        compact_draw(&app, vcx);
        let before = vcx.update(|_, cx| {
            (
                ai_snapshot_sets(&app, cx),
                app.read(cx).ai_panel.read(cx).content.parses,
            )
        });
        let started = Instant::now();
        for _ in 0..1000 {
            vcx.update(|_, cx| {
                app.update(cx, |app, cx| {
                    app.ai
                        .apply_chat_delta(launch.job_id, "**fixture** ", Some("reasoning "));
                    if coalesced {
                        app.schedule_ai_stream_refresh(cx);
                    } else {
                        app.flush_ai_panel_snapshot(cx);
                    }
                })
            });
            vcx.run_until_parked();
        }
        if coalesced {
            vcx.executor()
                .advance_clock(std::time::Duration::from_millis(33));
        }
        compact_draw(&app, vcx);
        let after = vcx.update(|_, cx| {
            (
                ai_snapshot_sets(&app, cx),
                app.read(cx).ai_panel.read(cx).content.parses,
            )
        });
        let expected = if coalesced { 1 } else { 1000 };
        assert_eq!(after.0 - before.0, expected);
        assert_eq!(after.1 - before.1, expected);
        eprintln!(
            "AI scheduling profile: coalesced={coalesced}, messages=2000, deltas=1000, snapshots={}, parses={}, elapsed={:?}",
            after.0 - before.0,
            after.1 - before.1,
            started.elapsed()
        );
    }
}
