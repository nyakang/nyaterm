use std::cell::Cell;
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    AppContext as _, Context, Entity, IntoElement, Modifiers, MouseButton, Render, Subscription,
    TestAppContext, VisualTestContext, Window, div, point, prelude::*, px, size,
};
use nyaterm_core::{AiPermissionMode, CapabilityScope};
use nyaterm_mcp_protocol::{RpcError, tool};
use nyaterm_ui::{NyaDialogWindowExt as _, init_components, nya_root};
use serde_json::{Value, json};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::features::NyaTermApp;
use crate::features::capability_runtime::terminal_execution::TerminalExecutionPolicy;
use crate::features::capability_runtime::{
    CapabilityApprovalRequest, CapabilityCaller, CapabilityRequest, PendingCapabilityApproval,
    mcp_grant_key,
};
use crate::features::mcp::McpApprovalDecision;
use crate::features::test_support::app_with_visible_local_session;
use crate::test_support::{TestAppCleanup, TestConfigDir};

struct ApprovalHost {
    app: Entity<NyaTermApp>,
    lower_presses: Rc<Cell<usize>>,
    _subscription: Subscription,
}

impl Render for ApprovalHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.app
            .update(cx, |app, cx| app.schedule_mcp_approval_dialog(window, cx));
        let lower_presses = self.lower_presses.clone();
        div()
            .size_full()
            .track_focus(self.app.read(cx).terminal.input_focus())
            .on_mouse_down(MouseButton::Left, move |_, _, _| {
                lower_presses.set(lower_presses.get() + 1);
            })
    }
}

fn host<'a>(
    cx: &'a mut TestAppContext,
    path: &Path,
) -> (
    Entity<NyaTermApp>,
    &'a mut VisualTestContext,
    Rc<Cell<usize>>,
) {
    cx.update(init_components);
    let app = app_with_visible_local_session(cx, path, "s");
    cx.update_entity(&app, |app, cx| app.sync_component_theme(cx));
    let host_app = app.clone();
    let presses = Rc::new(Cell::new(0));
    let lower_presses = presses.clone();
    let (_, vcx) = cx.add_window_view(move |window, cx| {
        let content = cx.new(|cx| ApprovalHost {
            app: host_app.clone(),
            lower_presses,
            _subscription: cx.observe(&host_app, |_, _, cx| cx.notify()),
        });
        nya_root(content, window, cx)
    });
    vcx.update(|window, cx| {
        let focus = app.read(cx).terminal.input_focus().clone();
        window.focus(&focus, cx);
    });
    draw(vcx);
    (app, vcx, presses)
}

fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    for _ in 0..3 {
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
        cx.run_until_parked();
    }
}

fn settle(cx: &mut VisualTestContext) {
    cx.executor().advance_clock(Duration::from_millis(500));
    draw(cx);
}

fn press_key(cx: &mut VisualTestContext, key: &'static str) {
    let keystroke = gpui::Keystroke::parse(key).unwrap();
    cx.simulate_event(gpui::KeyDownEvent {
        keystroke: keystroke.clone(),
        is_held: false,
        prefer_character_input: false,
    });
    cx.simulate_event(gpui::KeyUpEvent { keystroke });
    draw(cx);
}

fn queue(
    app: &Entity<NyaTermApp>,
    cx: &mut VisualTestContext,
    id: &str,
    command: &str,
    allow_session: bool,
) -> oneshot::Receiver<Result<Value, RpcError>> {
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            let (reply, receiver) = oneshot::channel();
            let request = CapabilityRequest {
                extended_results: true,
                resolved_session_id: None,
                request_id: id.into(),
                connection_id: "external-fixture".into(),
                generation: "fixture-generation".into(),
                client: "fixture client".into(),
                permission_mode: AiPermissionMode::Confirm,
                scope: CapabilityScope::AllSessions,
                tool: tool::TERMINAL_EXECUTE.into(),
                arguments: json!({"sessionId":"s", "command":command}),
                cancellation: CancellationToken::new(),
                approved: false,
                approval_decision: None,
                caller: CapabilityCaller::Mcp,
                reply,
            };
            app.mcp
                .capabilities
                .queue_approval(PendingCapabilityApproval {
                    approval: CapabilityApprovalRequest {
                        request_id: id.into(),
                        client: request.client.clone(),
                        capability: request.tool.clone(),
                        target: Some("s".into()),
                        parameter_summary: "truncated summary…".into(),
                        risk_level: "high".into(),
                        destructive: !allow_session,
                        allow_session,
                    },
                    grant_key: mcp_grant_key(&request),
                    request,
                });
            cx.notify();
            receiver
        })
    })
}

fn current_id(app: &Entity<NyaTermApp>, cx: &mut VisualTestContext) -> Option<String> {
    cx.update(|_, cx| {
        app.read(cx)
            .mcp
            .approval_dialog
            .current
            .as_ref()
            .map(|current| current.request_id.clone())
    })
}

#[test]
fn mcp_approval_backdrop_escape_and_blur_preserve_the_request_and_block_lower_input() {
    let directory = TestConfigDir::new("nyaterm-mcp-approval-dismissal");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let (app, vcx, lower_presses) = host(&mut cx, directory.path());
    let mut reply = queue(&app, vcx, "one", "echo fixture", true);
    draw(vcx);
    vcx.simulate_click(point(px(8.), px(80.)), Modifiers::default());
    press_key(vcx, "escape");
    vcx.deactivate_window();
    vcx.update(|window, cx| window.blur(cx));
    draw(vcx);
    assert_eq!(lower_presses.get(), 0);
    assert_eq!(current_id(&app, vcx).as_deref(), Some("one"));
    assert!(matches!(
        reply.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    // Confirm dispatched from a non-button focus must not approve or dismiss.
    vcx.update(|window, cx| {
        let text = app
            .read(cx)
            .mcp
            .approval_dialog
            .current
            .as_ref()
            .unwrap()
            .text
            .clone();
        text.focus(window, cx);
    });
    press_key(vcx, "enter");
    draw(vcx);
    assert_eq!(current_id(&app, vcx).as_deref(), Some("one"));
    assert!(matches!(
        reply.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    assert!(vcx.debug_bounds("mcp-approval-session").is_some());
}

#[test]
fn mcp_approval_initial_keyboard_focus_denies_and_restores_the_previous_focus() {
    let directory = TestConfigDir::new("nyaterm-mcp-approval-keyboard");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let (app, vcx, _) = host(&mut cx, directory.path());
    let previous = vcx.update(|window, cx| window.focused(cx));
    let mut reply = queue(&app, vcx, "one", "echo fixture", false);
    draw(vcx);
    assert!(vcx.debug_bounds("mcp-approval-session").is_none());
    let deny_focus = vcx.update(|window, cx| window.focused(cx));
    press_key(vcx, "tab");
    assert_ne!(vcx.update(|window, cx| window.focused(cx)), deny_focus);
    press_key(vcx, "shift-tab");
    assert_eq!(vcx.update(|window, cx| window.focused(cx)), deny_focus);
    press_key(vcx, "enter");
    settle(vcx);
    assert_eq!(
        reply.try_recv().unwrap().unwrap_err().code,
        "approval_denied"
    );
    assert!(current_id(&app, vcx).is_none());
    assert_eq!(vcx.update(|window, cx| window.focused(cx)), previous);
}

#[test]
fn mcp_approval_full_script_is_copyable_and_the_footer_stays_inside_a_small_viewport() {
    let directory = TestConfigDir::new("nyaterm-mcp-approval-long-script");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let (app, vcx, _) = host(&mut cx, directory.path());
    vcx.simulate_resize(size(px(420.), px(360.)));
    let command = format!(
        "printf '%s' '<猫>&\"'\n{}\n{}",
        "hostname\n".repeat(60),
        "x".repeat(600)
    );
    let _reply = queue(&app, vcx, "long", &command, true);
    draw(vcx);
    vcx.update(|_, cx| {
        let text = app
            .read(cx)
            .mcp
            .approval_dialog
            .current
            .as_ref()
            .unwrap()
            .text
            .clone();
        text.select_all(cx);
        assert_eq!(text.selected_text(cx), command);
    });
    let footer = vcx.debug_bounds("mcp-approval-footer").unwrap();
    let once = vcx.debug_bounds("mcp-approval-once").unwrap();
    assert!(footer.origin.y >= px(0.) && footer.bottom() <= px(360.));
    assert!(footer.origin.x >= px(0.) && footer.right() <= px(420.));
    assert!(footer.contains(&once.center()));
    assert!(
        vcx.debug_bounds("mcp-approval-parameters")
            .unwrap()
            .size
            .width
            <= px(420.)
    );
    vcx.simulate_event(gpui::ScrollEvent {
        position: footer.center() - point(px(0.), px(80.)),
        delta: gpui::ScrollDelta::Pixels(point(px(0.), px(-200.))),
        modifiers: Modifiers::default(),
        touch_phase: gpui::TouchPhase::Moved,
        source: gpui::ScrollSource::Pointer,
    });
    draw(vcx);
    assert_eq!(vcx.debug_bounds("mcp-approval-footer"), Some(footer));
}

#[test]
fn mcp_approval_fifo_and_duplicate_callbacks_do_not_consume_the_next_request() {
    let directory = TestConfigDir::new("nyaterm-mcp-approval-fifo");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let (app, vcx, _) = host(&mut cx, directory.path());
    let mut first = queue(&app, vcx, "one", "echo first", true);
    let mut second = queue(&app, vcx, "two", "echo second", true);
    draw(vcx);
    assert_eq!(current_id(&app, vcx).as_deref(), Some("one"));
    let first_handle = vcx.update(|_, cx| {
        app.read(cx)
            .mcp
            .approval_dialog
            .current
            .as_ref()
            .unwrap()
            .handle
            .clone()
    });
    let deny = vcx.debug_bounds("mcp-approval-deny").unwrap().center();
    vcx.simulate_click(deny, Modifiers::default());
    settle(vcx);
    assert_eq!(
        first.try_recv().unwrap().unwrap_err().code,
        "approval_denied"
    );
    assert_eq!(current_id(&app, vcx).as_deref(), Some("two"));
    vcx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.decide_presented_mcp_approval(
                "one",
                &first_handle,
                McpApprovalDecision::AllowSession,
                window,
                cx,
            );
        })
    });
    assert!(matches!(
        second.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    assert!(vcx.update(|_, cx| app.read(cx).mcp.capabilities.session_grants.is_empty()));

    let deny = vcx.debug_bounds("mcp-approval-deny").unwrap().center();
    vcx.simulate_click(deny, Modifiers::default());
    settle(vcx);
    let mut replacement = queue(&app, vcx, "one", "echo replacement", true);
    draw(vcx);
    vcx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.decide_presented_mcp_approval(
                "one",
                &first_handle,
                McpApprovalDecision::AllowSession,
                window,
                cx,
            );
        });
    });
    assert!(matches!(
        replacement.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    assert_eq!(current_id(&app, vcx).as_deref(), Some("one"));
}

#[test]
fn mcp_approval_allow_buttons_dispatch_once_and_only_session_choice_adds_a_grant() {
    for (key, session_grant) in [("once", false), ("session", true)] {
        let directory = TestConfigDir::new("nyaterm-mcp-approval-allow");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let (app, vcx, _) = host(&mut cx, directory.path());
        let mut reply = queue(&app, vcx, "one", "echo fixture", true);
        // Dispatch a harmless synchronous capability, so the button path is
        // exercised without starting a shell or depending on saved credentials.
        vcx.update(|_, cx| {
            app.update(cx, |app, _| {
                app.mcp.ephemeral_generations.lock().unwrap().insert(
                    "fixture-generation".into(),
                    TerminalExecutionPolicy {
                        background: false,
                        max_lines: 100,
                    },
                );
                let pending = app
                    .mcp
                    .capabilities
                    .pending_approvals
                    .get_mut("one")
                    .unwrap();
                pending.request.tool = tool::SESSION_GET.into();
                pending.request.arguments = json!({"sessionId":"s"});
                pending.approval.capability = tool::SESSION_GET.into();
                pending.grant_key = mcp_grant_key(&pending.request);
            })
        });
        draw(vcx);
        let selector = if key == "once" {
            "mcp-approval-once"
        } else {
            "mcp-approval-session"
        };
        let button = vcx.debug_bounds(selector).unwrap().center();
        let handle = vcx.update(|_, cx| {
            app.read(cx)
                .mcp
                .approval_dialog
                .current
                .as_ref()
                .unwrap()
                .handle
                .clone()
        });
        vcx.simulate_click(button, Modifiers::default());
        settle(vcx);
        assert!(reply.try_recv().unwrap().is_ok());
        assert!(current_id(&app, vcx).is_none());
        vcx.update(|window, cx| {
            app.update(cx, |app, cx| {
                app.decide_presented_mcp_approval(
                    "one",
                    &handle,
                    McpApprovalDecision::AllowSession,
                    window,
                    cx,
                );
                assert_eq!(
                    app.mcp.capabilities.session_grants.len(),
                    usize::from(session_grant)
                );
                assert!(app.mcp.capabilities.pending_approvals.is_empty());
            })
        });
    }
}

#[test]
fn mcp_approval_expiration_under_another_dialog_does_not_close_that_dialog() {
    let directory = TestConfigDir::new("nyaterm-mcp-approval-covered");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let (app, vcx, _) = host(&mut cx, directory.path());
    let _reply = queue(&app, vcx, "one", "echo fixture", true);
    draw(vcx);
    let other = vcx.update(|window, cx| {
        window.open_tracked_nya_dialog(cx, |dialog, _, _| dialog.title("Other"))
    });
    draw(vcx);
    vcx.update(|window, cx| {
        app.update(cx, |app, cx| {
            let handle = app
                .mcp
                .approval_dialog
                .current
                .as_ref()
                .unwrap()
                .handle
                .clone();
            app.handle_pending_mcp_cancel("one", cx);
            app.decide_presented_mcp_approval(
                "one",
                &handle,
                McpApprovalDecision::AllowSession,
                window,
                cx,
            );
        })
    });
    draw(vcx);
    assert!(vcx.update(|window, cx| other.is_top(window, cx)));
    assert!(vcx.update(|_, cx| app.read(cx).mcp.capabilities.session_grants.is_empty()));
    vcx.update(|window, cx| window.close_nya_dialog(cx));
    settle(vcx);
    assert!(current_id(&app, vcx).is_none());
    assert!(!vcx.update(|window, cx| window.has_active_nya_dialog(cx)));
}

#[test]
fn mcp_approval_waits_for_other_dialogs_and_survives_lock_without_a_decision() {
    let directory = TestConfigDir::new("nyaterm-mcp-approval-lock");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let (app, vcx, _) = host(&mut cx, directory.path());
    vcx.update(|window, cx| window.open_nya_dialog(cx, |dialog, _, _| dialog.title("Existing")));
    let mut reply = queue(&app, vcx, "one", "echo fixture", true);
    draw(vcx);
    assert!(current_id(&app, vcx).is_none());
    vcx.update(|window, cx| window.close_nya_dialog(cx));
    settle(vcx);
    assert_eq!(current_id(&app, vcx).as_deref(), Some("one"));
    vcx.update(|window, cx| {
        app.update(cx, |app, cx| app.apply_shared_screen_lock(true, window, cx))
    });
    settle(vcx);
    assert!(current_id(&app, vcx).is_none());
    assert!(!vcx.update(|window, cx| window.has_active_nya_dialog(cx)));
    assert!(matches!(
        reply.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    vcx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.apply_shared_screen_lock(false, window, cx)
        })
    });
    draw(vcx);
    assert_eq!(current_id(&app, vcx).as_deref(), Some("one"));
    vcx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.handle_mcp_disconnect("external-fixture", cx)
        })
    });
    settle(vcx);
    assert!(current_id(&app, vcx).is_none());
    assert_eq!(reply.try_recv().unwrap().unwrap_err().code, "cancelled");
}

#[test]
fn mcp_approval_waits_for_modal_children_and_withdraws_revoked_credentials() {
    let directory = TestConfigDir::new("nyaterm-mcp-approval-revocation");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let (app, vcx, _) = host(&mut cx, directory.path());
    vcx.update(|_, cx| {
        app.update(cx, |app, cx| {
            assert!(app.connection_state.begin_editor_window_open());
            cx.notify();
        })
    });
    let mut reply = queue(&app, vcx, "one", "echo fixture", true);
    draw(vcx);
    assert!(current_id(&app, vcx).is_none());
    vcx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.connection_state.clear_editor_window_pending();
            cx.notify();
        })
    });
    draw(vcx);
    assert_eq!(current_id(&app, vcx).as_deref(), Some("one"));
    vcx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.mcp.ephemeral_generations.lock().unwrap().insert(
                "fixture-generation".into(),
                TerminalExecutionPolicy {
                    background: false,
                    max_lines: 100,
                },
            );
            let pending = app
                .mcp
                .capabilities
                .pending_approvals
                .remove("one")
                .unwrap();
            app.dispatch_capability_request(pending.request, cx);
            app.mcp.capabilities.revoke_generation("fixture-generation");
            cx.notify();
        })
    });
    settle(vcx);
    assert!(current_id(&app, vcx).is_none());
    assert_eq!(reply.try_recv().unwrap().unwrap_err().code, "cancelled");
}

#[test]
fn mcp_approval_cancelled_request_id_cannot_authorize_a_replacement_before_redraw() {
    let directory = TestConfigDir::new("nyaterm-mcp-approval-reused-id");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    let (app, vcx, _) = host(&mut cx, directory.path());
    let _reply = queue(&app, vcx, "one", "echo original", true);
    draw(vcx);
    let old_handle = vcx.update(|_, cx| {
        app.read(cx)
            .mcp
            .approval_dialog
            .current
            .as_ref()
            .unwrap()
            .handle
            .clone()
    });
    vcx.update(|_, cx| app.update(cx, |app, cx| app.handle_pending_mcp_cancel("one", cx)));
    let mut replacement = queue(&app, vcx, "one", "echo replacement", true);
    // The previous card may still be painted while events replace its request.
    vcx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.decide_presented_mcp_approval(
                "one",
                &old_handle,
                McpApprovalDecision::AllowSession,
                window,
                cx,
            );
            assert!(app.mcp.capabilities.session_grants.is_empty());
        })
    });
    assert!(matches!(
        replacement.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    settle(vcx);
    assert!(!old_handle.is_active());
    assert_eq!(current_id(&app, vcx).as_deref(), Some("one"));
}
