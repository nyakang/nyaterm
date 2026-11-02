use gpui::{Context, Entity, TestAppContext, px};
use nyaterm_core::ai::harness::{AgentRunStatus, AgentToolRegistry};
use nyaterm_core::capabilities::AgentApprovalDecision;
use nyaterm_core::{AiMode, AiPermissionMode};
use nyaterm_mcp_protocol::{RpcError, tool};
use serde_json::{Value, json};
use std::sync::Arc;

use super::{compact_draw, compact_host, edit_panel_snapshot, transcript_message};
use crate::features::NyaTermApp;
use crate::features::ai::panel::transcript::{AiTranscriptRow, AiTranscriptUpdate};
use crate::features::capability_runtime::approvals::NativeCapabilityApproval;
use crate::features::capability_runtime::{
    CapabilityApprovalRequest, CapabilityCaller, CapabilityRequest, PendingCapabilityApproval,
    mcp_grant_key,
};
use crate::test_support::{TestAppCleanup, TestConfigDir};

fn queue_native_approval(
    app: &mut NyaTermApp,
    tool_name: &str,
    arguments: Value,
    allow_session: bool,
    cx: &mut Context<NyaTermApp>,
) -> (
    NativeCapabilityApproval,
    tokio::sync::oneshot::Receiver<Result<Value, RpcError>>,
) {
    app.ai
        .begin_chat_request("Inspect resources".into(), AiMode::Agent, None);
    app.ai.begin_native_run(
        serde_json::from_value(json!({
            "mode":"agent", "action":"generate_command", "userInput":"Inspect resources"
        }))
        .unwrap(),
    );
    let call = AgentToolRegistry::parse(
        &[],
        &json!({"action":tool_name,"arguments":arguments}).to_string(),
    )
    .unwrap();
    let call_id = call.id.clone();
    let (run_id, scope, cancellation) = app.ai.begin_native_call(call).unwrap();
    app.ai
        .set_native_run_status(&run_id, AgentRunStatus::NeedsApproval);
    let ai_session_id = app.ai.chat_session_id().to_string();
    let request_id = format!("native:{run_id}:{call_id}");
    let (reply, receiver) = tokio::sync::oneshot::channel();
    let request = CapabilityRequest {
        connection_id: format!("native:{run_id}"),
        request_id: request_id.clone(),
        generation: String::new(),
        client: "NyaTerm Native Agent".into(),
        permission_mode: AiPermissionMode::Confirm,
        scope,
        tool: tool_name.into(),
        arguments,
        cancellation,
        approved: false,
        approval_decision: None,
        caller: CapabilityCaller::Native {
            run_id: run_id.clone(),
            ai_session_id: ai_session_id.clone(),
            model_risk: None,
        },
        reply,
    };
    app.mcp
        .capabilities
        .queue_approval(PendingCapabilityApproval {
            approval: CapabilityApprovalRequest {
                request_id,
                client: request.client.clone(),
                capability: tool_name.into(),
                target: request.arguments["sessionId"].as_str().map(str::to_string),
                parameter_summary: "/tmp/fixture".into(),
                risk_level: "high".into(),
                destructive: false,
                allow_session,
            },
            grant_key: mcp_grant_key(&request),
            request,
        });
    let view = app
        .mcp
        .capabilities
        .native_approval_requests(&run_id, &ai_session_id)
        .pop()
        .unwrap();
    app.flush_ai_panel_snapshot(cx);
    (view, receiver)
}

fn respond(
    app: &Entity<NyaTermApp>,
    view: &NativeCapabilityApproval,
    decision: AgentApprovalDecision,
    cx: &mut gpui::App,
) {
    app.update(cx, |app, cx| {
        app.respond_to_native_capability_approval(
            &view.run_id,
            &view.ai_session_id,
            &view.approval.request_id,
            decision,
            cx,
        )
    });
}

#[test]
fn native_approval_stays_in_collapsed_history_and_deny_is_keyboard_accessible() {
    let root = TestConfigDir::new("nyaterm-ai-inline-approval-keyboard");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let (app, vcx) = compact_host(&mut cx, root.path());
    let original_focus = vcx.update(|window, cx| window.focused(cx));
    let (_, mut receiver) = vcx.update(|_, cx| {
        app.update(cx, |app, cx| {
            queue_native_approval(
                app,
                tool::TERMINAL_EXECUTE,
                json!({"sessionId":"s","command":"printf '%s' 猫"}),
                false,
                cx,
            )
        })
    });
    compact_draw(&app, vcx);
    assert_eq!(vcx.update(|window, cx| window.focused(cx)), original_focus);
    assert!(vcx.debug_bounds("ai-native-approval").is_some());
    assert!(vcx.debug_bounds("ai-native-approval-command").is_some());
    assert!(vcx.debug_bounds("ai-native-approval-once").is_some());
    assert!(vcx.debug_bounds("ai-native-approval-run").is_none());
    vcx.update(|window, cx| {
        let panel = app.read(cx).ai_panel.read(cx);
        let snapshot = panel.snapshot().unwrap();
        assert!(snapshot.expanded_execution_groups.is_empty());
        assert!(app.read(cx).mcp_pending_approval_requests().is_empty());
        let rows = AiTranscriptRow::project(snapshot);
        assert!(
            rows.iter()
                .any(|r| matches!(r, AiTranscriptRow::Execution { .. }))
        );
        assert_eq!(
            rows.iter()
                .filter(|r| matches!(r, AiTranscriptRow::NativeApproval { .. }))
                .count(),
            1
        );
        // The execution disclosure precedes the authorization buttons.
        window.blur(cx);
        window.focus_next(cx);
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
    assert_eq!(
        receiver.try_recv().unwrap().unwrap_err().code,
        "approval_denied"
    );
    assert!(vcx.debug_bounds("ai-native-approval").is_none());
}

#[test]
fn native_approval_callbacks_reject_scope_switches_cancellation_and_duplicate_clicks() {
    let root = TestConfigDir::new("nyaterm-ai-inline-approval-lifecycle");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let (app, vcx) = compact_host(&mut cx, root.path());
    let (view, mut receiver) = vcx.update(|_, cx| {
        app.update(cx, |app, cx| {
            queue_native_approval(
                app,
                tool::SFTP_READ_TEXT,
                json!({"sessionId":"s","path":"/tmp/fixture"}),
                true,
                cx,
            )
        })
    });
    compact_draw(&app, vcx);
    assert!(vcx.debug_bounds("ai-native-approval-command").is_none());
    assert!(vcx.debug_bounds("ai-native-approval-run").is_some());
    let scope = vcx.update(|_, cx| {
        app.update(cx, |app, cx| {
            let scope = app.ai.active_scope_key().to_string();
            app.ai.switch_scope("terminal:other");
            app.flush_ai_panel_snapshot(cx);
            scope
        })
    });
    vcx.update(|_, cx| respond(&app, &view, AgentApprovalDecision::AllowOnce, cx));
    assert!(receiver.try_recv().is_err());
    compact_draw(&app, vcx);
    assert!(vcx.debug_bounds("ai-native-approval").is_none());
    vcx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.ai.switch_scope(&scope);
            app.flush_ai_panel_snapshot(cx);
        })
    });
    compact_draw(&app, vcx);
    assert!(vcx.debug_bounds("ai-native-approval").is_some());
    let deny = vcx.debug_bounds("ai-native-approval-deny").unwrap();
    vcx.simulate_click(deny.center(), gpui::Modifiers::default());
    compact_draw(&app, vcx);
    assert_eq!(
        receiver.try_recv().unwrap().unwrap_err().code,
        "approval_denied"
    );
    vcx.update(|_, cx| respond(&app, &view, AgentApprovalDecision::AllowOnce, cx));
    assert!(vcx.debug_bounds("ai-native-approval").is_none());

    let (cancelled, mut receiver) = vcx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.cancel_ai_chat(cx);
            queue_native_approval(
                app,
                tool::SFTP_READ_TEXT,
                json!({"sessionId":"s","path":"/tmp/fixture"}),
                true,
                cx,
            )
        })
    });
    vcx.update(|_, cx| app.update(cx, |app, cx| app.cancel_ai_chat(cx)));
    vcx.update(|_, cx| respond(&app, &cancelled, AgentApprovalDecision::AllowOnce, cx));
    compact_draw(&app, vcx);
    assert_eq!(receiver.try_recv().unwrap().unwrap_err().code, "cancelled");
    assert!(vcx.debug_bounds("ai-native-approval").is_none());
}

#[test]
fn native_approval_allow_buttons_consume_requests_and_target_close_revokes_pending_actions() {
    let root = TestConfigDir::new("nyaterm-ai-inline-approval-allow");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let (app, vcx) = compact_host(&mut cx, root.path());
    for selector in ["ai-native-approval-once", "ai-native-approval-run"] {
        let (view, mut receiver) = vcx.update(|_, cx| {
            app.update(cx, |app, cx| {
                app.cancel_ai_chat(cx);
                queue_native_approval(
                    app,
                    tool::SFTP_READ_TEXT,
                    json!({"sessionId":"s","path":"/tmp/fixture"}),
                    true,
                    cx,
                )
            })
        });
        let grant = vcx.update(|_, cx| {
            app.read(cx).mcp.capabilities.pending_approvals[&view.approval.request_id]
                .grant_key
                .clone()
                .unwrap()
        });
        compact_draw(&app, vcx);
        let button = vcx.debug_bounds(selector).unwrap();
        vcx.simulate_click(button.center(), gpui::Modifiers::default());
        compact_draw(&app, vcx);
        // This fixture deliberately has no live target. A valid authorization
        // reaches scope revalidation, instead of executing against a fallback.
        assert_eq!(
            receiver.try_recv().unwrap().unwrap_err().code,
            "scope_denied"
        );
        assert!(vcx.debug_bounds("ai-native-approval").is_none());
        assert_eq!(
            vcx.update(|_, cx| app.read(cx).mcp.capabilities.has_session_grant(&grant)),
            selector == "ai-native-approval-run"
        );
        vcx.update(|_, cx| respond(&app, &view, AgentApprovalDecision::AllowOnce, cx));
    }
    let (view, mut receiver) = vcx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.cancel_ai_chat(cx);
            queue_native_approval(
                app,
                tool::SFTP_READ_TEXT,
                json!({"sessionId":"s","path":"/tmp/fixture"}),
                true,
                cx,
            )
        })
    });
    vcx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.cancel_native_run_for_closed_session("s", cx)
        })
    });
    vcx.update(|_, cx| respond(&app, &view, AgentApprovalDecision::AllowOnce, cx));
    compact_draw(&app, vcx);
    assert_eq!(
        receiver.try_recv().unwrap().unwrap_err().code,
        "target_disconnected"
    );
    assert!(vcx.debug_bounds("ai-native-approval").is_none());
}

#[test]
fn native_approval_shows_complete_scripts_and_preserves_readers_scroll() {
    let root = TestConfigDir::new("nyaterm-ai-inline-approval-layout");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let (app, vcx) = compact_host(&mut cx, root.path());
    let command = "printf '%s' 猫\n".repeat(18);
    let (view, _receiver) = vcx.update(|_, cx| {
        app.update(cx, |app, cx| {
            queue_native_approval(
                app,
                tool::TERMINAL_EXECUTE,
                json!({"sessionId":"s","command":command}),
                false,
                cx,
            )
        })
    });
    compact_draw(&app, vcx);
    let body = vcx.debug_bounds("ai-native-approval-command").unwrap();
    let viewport = vcx.debug_bounds("ai-transcript-viewport").unwrap();
    assert!(body.size.height > px(62.));
    assert!(body.left() >= viewport.left() && body.right() <= viewport.right());
    vcx.update(|_, cx| {
        let snapshot = app.read(cx).ai_panel.read(cx).snapshot().unwrap();
        assert_eq!(
            snapshot.native_approvals[0].command.as_deref(),
            Some(command.as_str())
        );
    });
    edit_panel_snapshot(&app, vcx, |snapshot| {
        snapshot.native_approvals = Arc::from([]);
        snapshot.native_run = None;
        snapshot.messages = (0..24).map(transcript_message).collect::<Vec<_>>().into();
    });
    let scroll = vcx.update(|_, cx| app.read(cx).ai_panel.read(cx).transcript_scroll.clone());
    vcx.update(|_, cx| scroll.update(cx, |state, cx| state.scroll_to_item(4, cx)));
    compact_draw(&app, vcx);
    let anchor = vcx.debug_bounds("ai-message-message-4").unwrap().top();
    let previous = vcx.update(|_, cx| app.read(cx).ai_panel.read(cx).snapshot().unwrap().clone());
    edit_panel_snapshot(&app, vcx, |snapshot| {
        snapshot.native_approvals = vec![view.clone()].into()
    });
    assert!(!vcx.update(|_, cx| scroll.read(cx).is_following_tail()));
    assert_eq!(
        vcx.debug_bounds("ai-message-message-4").unwrap().top(),
        anchor
    );
    let next = vcx.update(|_, cx| app.read(cx).ai_panel.read(cx).snapshot().unwrap().clone());
    let update = AiTranscriptUpdate::between(
        Some(&previous),
        &next,
        &AiTranscriptRow::project(&previous),
        &AiTranscriptRow::project(&next),
    );
    assert!(!update.reset);
    assert!(update.splice.is_some());
}
