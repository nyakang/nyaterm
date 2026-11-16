use super::idempotency::Lookup;
use super::{CapabilityRequest, CapabilityRuntimeState, connection_type_name, to_value};
use crate::features::NyaTermApp;
use crate::features::mcp::server::rpc_failure;
use crate::features::session::SavedConnectionStartOptions;
use gpui::Context;
use nyaterm_core::capabilities::{CapabilityScope, CapabilitySession};
use nyaterm_core::models::connection::AiExecutionProfile;
use nyaterm_core::models::sessions::SavedConnection;
use nyaterm_mcp_protocol::{RpcError, SessionOpenArgs, SessionOpenMode, SessionOpenResult};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

pub(super) struct SessionOpenContext {
    request_id: String,
    owner: String,
    cancellation: CancellationToken,
    scope: CapabilityScope,
    generation: String,
    require_command_capture: bool,
}

impl SessionOpenContext {
    pub(super) fn from_request(
        request: &CapabilityRequest,
        cancellation: CancellationToken,
    ) -> Self {
        Self {
            request_id: request.request_id.clone(),
            owner: request.connection_id.clone(),
            cancellation,
            scope: request.scope.clone(),
            generation: request.generation.clone(),
            require_command_capture: false,
        }
    }

    pub(super) fn for_terminal_start(
        request: &CapabilityRequest,
        cancellation: CancellationToken,
    ) -> Self {
        Self {
            require_command_capture: true,
            ..Self::from_request(request, cancellation)
        }
    }
}

pub(super) enum SessionOpenReply {
    Rpc(oneshot::Sender<Result<Value, RpcError>>),
    TerminalStart(oneshot::Sender<Result<SessionOpenResult, RpcError>>),
}

impl SessionOpenReply {
    fn complete(self, result: Result<SessionOpenResult, RpcError>) {
        match self {
            Self::Rpc(reply) => {
                let _ = reply.send(result.and_then(to_value));
            }
            Self::TerminalStart(reply) => {
                let _ = reply.send(result);
            }
        }
    }
}

impl From<oneshot::Sender<Result<Value, RpcError>>> for SessionOpenReply {
    fn from(reply: oneshot::Sender<Result<Value, RpcError>>) -> Self {
        Self::Rpc(reply)
    }
}

struct Waiter {
    request_id: String,
    owner: String,
    cancellation: CancellationToken,
    // A retry recovers the same operation, but cannot inherit its original scope.
    recovery_scope: Option<CapabilityScope>,
    reply: SessionOpenReply,
}
struct OpenOperation {
    connection: SavedConnection,
    owner: String,
    reuse: bool,
    scope: CapabilityScope,
    generation: String,
    started: bool,
    require_command_capture: bool,
    waiters: Vec<Waiter>,
}
#[derive(Default)]
pub(super) struct SessionOpenState {
    operations: HashMap<String, OpenOperation>,
    queue: VecDeque<String>,
}
impl SessionOpenState {
    pub fn pending(&self) -> bool {
        !self.operations.is_empty()
    }
    pub fn prune_waiters(&mut self) {
        for operation in self.operations.values_mut() {
            operation.waiters.retain(|w| !w.cancellation.is_cancelled());
        }
    }
    pub fn cancel_request(&mut self, request: &str) {
        for op in self.operations.values_mut() {
            op.waiters.retain(|w| {
                if w.request_id == request {
                    w.cancellation.cancel();
                    false
                } else {
                    true
                }
            });
        }
    }
    pub fn disconnect(&mut self, owner: &str) {
        for op in self.operations.values_mut() {
            op.waiters.retain(|w| {
                if w.owner == owner {
                    w.cancellation.cancel();
                    false
                } else {
                    true
                }
            });
        }
    }
}
impl CapabilityRuntimeState {
    pub(in crate::features) fn complete_session_open_success(
        &mut self,
        operation_id: &str,
        session_id: String,
        sessions: &[CapabilitySession],
    ) {
        let Some(op) = self.session_opens.operations.remove(operation_id) else {
            return;
        };
        let opened = SessionOpenResult {
            session_id: session_id.clone(),
            connection_id: op.connection.id.clone(),
            name: op.connection.name.clone(),
            r#type: connection_type_name(&op.connection.config)
                .unwrap_or("unknown")
                .into(),
            connected: true,
        };
        self.ledger.finish(operation_id, to_value(opened.clone()));
        self.session_opens.queue.retain(|id| id != operation_id);
        for waiter in op.waiters {
            if !waiter.cancellation.is_cancelled() {
                let permitted = waiter
                    .recovery_scope
                    .as_ref()
                    .is_none_or(|scope| scope.resolve(sessions).require(&session_id).is_ok());
                let reply = if permitted {
                    Ok(opened.clone())
                } else {
                    Err(rpc_failure(
                        "scope_denied",
                        "The previous result is outside the current scope.",
                    ))
                };
                waiter.reply.complete(reply);
            }
        }
    }
    pub(in crate::features) fn complete_session_open_failure(
        &mut self,
        operation_id: &str,
        _error: &str,
    ) {
        let Some(op) = self.session_opens.operations.remove(operation_id) else {
            return;
        };
        // Transport/authentication errors can contain host or credential context.
        let error = rpc_failure(
            "execution_failed",
            "The saved connection could not be opened.",
        );
        self.ledger.finish(operation_id, Err(error.clone()));
        self.session_opens.queue.retain(|id| id != operation_id);
        for waiter in op.waiters {
            if !waiter.cancellation.is_cancelled() {
                waiter.reply.complete(Err(error.clone()));
            }
        }
    }
}
impl NyaTermApp {
    pub(super) fn dispatch_mcp_session_open(
        &mut self,
        request: CapabilityRequest,
        cx: &mut Context<Self>,
    ) {
        let args: SessionOpenArgs =
            serde_json::from_value(request.arguments.clone()).expect("validated session open");
        let scope = request.scope.resolve(&self.mcp_live_sessions());
        let Some(connection) = self
            .connection_state
            .connection_by_id(&args.connection_id)
            .cloned()
        else {
            let _ = request.reply.send(Err(rpc_failure(
                "invalid_argument",
                "The saved connection does not exist.",
            )));
            return;
        };
        if connection_type_name(&connection.config).is_none() {
            let _ = request.reply.send(Err(rpc_failure(
                "permission_denied",
                "MCP cannot open graphical connections.",
            )));
            return;
        }
        let canonical = serde_json::to_value(&args).expect("session arguments");
        let lookup = self.mcp.capabilities.ledger.begin(
            &request.connection_id,
            &request.tool,
            args.idempotency_key.as_deref(),
            &canonical,
        );
        let operation_id = match lookup {
            Ok(Lookup::Complete(result)) => {
                let result = result.and_then(|value| {
                    let session_id = value
                        .get("sessionId")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    scope.require(session_id).map_err(|_| {
                        rpc_failure(
                            "scope_denied",
                            "The previous result is outside the current scope.",
                        )
                    })?;
                    Ok(value)
                });
                let _ = request.reply.send(result);
                return;
            }
            Ok(Lookup::New(id)) => id,
            Ok(Lookup::Pending(id)) => id,
            Err(error) => {
                let _ = request.reply.send(Err(error));
                return;
            }
        };
        self.ensure_terminal_execution_clock(cx);
        // A retry must join its own operation before considering other sessions.
        if self
            .mcp
            .capabilities
            .session_opens
            .operations
            .contains_key(&operation_id)
        {
            self.mcp
                .capabilities
                .session_opens
                .operations
                .get_mut(&operation_id)
                .unwrap()
                .waiters
                .push(Waiter {
                    request_id: request.request_id,
                    owner: request.connection_id,
                    cancellation: request.cancellation,
                    recovery_scope: Some(request.scope),
                    reply: request.reply.into(),
                });
            return;
        }
        let context = SessionOpenContext::from_request(&request, request.cancellation.clone());
        self.queue_mcp_session_open(
            context,
            connection,
            args.mode == SessionOpenMode::ReuseOrCreate,
            operation_id,
            request.reply.into(),
            cx,
        );
    }

    pub(super) fn queue_mcp_session_open(
        &mut self,
        context: SessionOpenContext,
        connection: SavedConnection,
        reuse: bool,
        operation_id: String,
        reply: SessionOpenReply,
        cx: &mut Context<Self>,
    ) {
        let scope = context.scope.resolve(&self.mcp_live_sessions());
        if reuse {
            let mut candidates = self
                .session
                .ordered_sessions()
                .into_iter()
                .filter(|s| {
                    scope.session_ids.contains(&s.id)
                        && !self.session.is_disconnected(&s.id)
                        && !self.mcp.capabilities.terminal_executions.busy(&s.id)
                        && self.session.metadata(&s.id).is_some_and(|m| {
                            m.source_connection_id.as_deref() == Some(&connection.id)
                                && m.ai_execution_profile
                                    != nyaterm_core::AiExecutionProfile::Disabled
                                && (!context.require_command_capture
                                    || supports_command_capture(
                                        self.ai_execution_profile_for_session(&s.id),
                                    ))
                        })
                })
                .map(|s| s.id)
                .collect::<Vec<_>>();
            candidates.sort_by_key(|id| {
                (
                    scope.default_session_id.as_ref() != Some(id),
                    self.session.active_id() != Some(id.as_str()),
                    id.clone(),
                )
            });
            if let Some(id) = candidates.into_iter().next() {
                let opened = SessionOpenResult {
                    session_id: id,
                    connection_id: connection.id,
                    name: connection.name,
                    r#type: connection_type_name(&connection.config).unwrap().into(),
                    connected: true,
                };
                self.mcp
                    .capabilities
                    .ledger
                    .finish(&operation_id, to_value(opened.clone()));
                reply.complete(Ok(opened));
                return;
            }
        }
        let mut id = operation_id.clone();
        if reuse
            && let Some((existing, _)) =
                self.mcp
                    .capabilities
                    .session_opens
                    .operations
                    .iter()
                    .find(|(_, op)| {
                        op.owner == context.owner
                            && op.reuse
                            && op.scope == context.scope
                            && op.connection.id == connection.id
                            && op.require_command_capture == context.require_command_capture
                    })
        {
            id = existing.clone();
            self.mcp.capabilities.ledger.alias(&operation_id, &id);
        }
        let state = &mut self.mcp.capabilities.session_opens;
        if !state.operations.contains_key(&id) && state.operations.len() >= 64 {
            let error = rpc_failure("resource_limit", "Too many pending session starts.");
            self.mcp.capabilities.ledger.finish(&id, Err(error.clone()));
            reply.complete(Err(error));
            return;
        }
        let waiter = Waiter {
            request_id: context.request_id,
            owner: context.owner.clone(),
            cancellation: context.cancellation,
            recovery_scope: None,
            reply,
        };
        if let Some(op) = state.operations.get_mut(&id) {
            op.waiters.push(waiter);
        } else {
            state.queue.push_back(id.clone());
            state.operations.insert(
                id,
                OpenOperation {
                    connection,
                    owner: context.owner,
                    reuse,
                    scope: context.scope,
                    generation: context.generation,
                    started: false,
                    require_command_capture: context.require_command_capture,
                    waiters: vec![waiter],
                },
            );
        }
        self.poll_mcp_session_opens(cx);
        self.ensure_terminal_execution_clock(cx);
    }
    pub(super) fn poll_mcp_session_opens(&mut self, cx: &mut Context<Self>) {
        self.mcp.capabilities.session_opens.prune_waiters();
        let queued = self
            .mcp
            .capabilities
            .session_opens
            .queue
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        for id in queued {
            let Some(op) = self.mcp.capabilities.session_opens.operations.get(&id) else {
                continue;
            };
            if op.waiters.is_empty() {
                let pending_request = self
                    .session
                    .start_pending_entries()
                    .find(|(_, pending)| {
                        pending.mcp_open_operation_id.as_deref() == Some(id.as_str())
                    })
                    .map(|(request_id, _)| request_id.clone());
                self.mcp
                    .capabilities
                    .complete_session_open_failure(&id, "cancelled before start");
                if let Some(request_id) = pending_request {
                    self.close_pending_session_start(request_id, cx);
                }
                continue;
            }
            if op.started {
                if !self.session.start_mcp_operation_is_pending(&id) {
                    self.mcp
                        .capabilities
                        .complete_session_open_failure(&id, "start ended without delivery");
                }
                continue;
            }
            if !self.mcp.generation_matches(&op.generation) {
                self.mcp
                    .capabilities
                    .complete_session_open_failure(&id, "authorization expired");
                continue;
            }
            if op.reuse {
                let scope = op.scope.resolve(&self.mcp_live_sessions());
                let mut candidates = self
                    .session
                    .ordered_sessions()
                    .into_iter()
                    .filter(|s| {
                        scope.session_ids.contains(&s.id)
                            && !self.session.is_disconnected(&s.id)
                            && !self.mcp.capabilities.terminal_executions.busy(&s.id)
                            && self.session.metadata(&s.id).is_some_and(|m| {
                                m.source_connection_id.as_deref() == Some(&op.connection.id)
                                    && m.ai_execution_profile
                                        != nyaterm_core::AiExecutionProfile::Disabled
                                    && (!op.require_command_capture
                                        || supports_command_capture(
                                            self.ai_execution_profile_for_session(&s.id),
                                        ))
                            })
                    })
                    .map(|s| s.id)
                    .collect::<Vec<_>>();
                candidates.sort_by_key(|sid| {
                    (
                        scope.default_session_id.as_ref() != Some(sid),
                        self.session.active_id() != Some(sid.as_str()),
                        sid.clone(),
                    )
                });
                if let Some(session_id) = candidates.into_iter().next() {
                    let sessions = self.mcp_live_sessions();
                    self.mcp
                        .capabilities
                        .complete_session_open_success(&id, session_id, &sessions);
                    continue;
                }
            }
            if self.session.saved_connection_is_starting(&op.connection.id) {
                continue;
            }
            let op = self
                .mcp
                .capabilities
                .session_opens
                .operations
                .get_mut(&id)
                .unwrap();
            op.started = true;
            let connection = op.connection.clone();
            self.continue_saved_connection_start(
                connection,
                SavedConnectionStartOptions {
                    mcp_open_operation_id: Some(id),
                    // An MCP caller cannot answer a port dialog.
                    suppress_serial_prompt: true,
                    ..Default::default()
                },
                cx,
            );
        }
    }
}

fn supports_command_capture(profile: AiExecutionProfile) -> bool {
    matches!(
        profile,
        AiExecutionProfile::Posix | AiExecutionProfile::Powershell | AiExecutionProfile::Cmd
    )
}

#[cfg(test)]
mod tests {
    use super::super::idempotency::Lookup;
    use super::{CapabilityRuntimeState, OpenOperation, Waiter};
    use nyaterm_core::capabilities::CapabilityScope;
    use nyaterm_core::models::sessions::SavedConnection;
    use serde_json::json;
    use tokio::sync::oneshot;
    use tokio_util::sync::CancellationToken;
    #[test]
    fn pending_recovery_rechecks_each_waiter_scope_without_losing_the_operation_result() {
        use nyaterm_core::capabilities::CapabilitySession;
        let mut state = CapabilityRuntimeState::default();
        let args = json!({"connectionId":"saved"});
        let Lookup::New(id) = state
            .ledger
            .begin("owner", "session_open", Some("key"), &args)
            .unwrap()
        else {
            panic!()
        };
        let mut receivers = Vec::new();
        let waiters = [
            None,
            Some(CapabilityScope::AllSessions),
            Some(CapabilityScope::explicit(
                ["different-session".into()],
                None,
            )),
            Some(CapabilityScope::current_window("old-window")),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, recovery_scope)| {
            let (reply, receiver) = oneshot::channel();
            receivers.push(receiver);
            Waiter {
                request_id: index.to_string(),
                owner: "owner".into(),
                cancellation: CancellationToken::new(),
                recovery_scope,
                reply: reply.into(),
            }
        })
        .collect();
        state.session_opens.operations.insert(
            id.clone(),
            OpenOperation {
                connection: serde_json::from_value(
                    json!({"id":"saved","name":"saved","type":"ssh","host":"localhost"}),
                )
                .unwrap(),
                owner: "owner".into(),
                reuse: false,
                scope: CapabilityScope::AllSessions,
                generation: "generation".into(),
                started: true,
                require_command_capture: false,
                waiters,
            },
        );
        state.complete_session_open_success(
            &id,
            "created".into(),
            &[CapabilitySession {
                id: "created".into(),
                owner_window_label: Some("new-window".into()),
                live: true,
            }],
        );
        for (index, receiver) in receivers.iter_mut().enumerate() {
            let result = receiver.try_recv().unwrap();
            if index < 2 {
                assert_eq!(result.unwrap()["sessionId"], "created");
            } else {
                assert_eq!(result.unwrap_err().code, "scope_denied");
            }
        }
        assert!(matches!(
            state
                .ledger
                .begin("owner", "session_open", Some("key"), &args)
                .unwrap(),
            Lookup::Complete(Ok(_))
        ));
    }
    #[test]
    fn ui_completion_cannot_finish_mcp_operation_and_cancelled_waiter_does_not_lose_result() {
        let mut state = CapabilityRuntimeState::default();
        let connection: SavedConnection = serde_json::from_value(
            json!({"id":"saved","name":"same name","type":"ssh","host":"localhost"}),
        )
        .unwrap();
        let args = json!({"connectionId":"saved"});
        let Lookup::New(id) = state
            .ledger
            .begin("owner", "session_open", Some("key"), &args)
            .unwrap()
        else {
            panic!()
        };
        let Lookup::New(alias) = state
            .ledger
            .begin("owner", "session_open", Some("alias"), &args)
            .unwrap()
        else {
            panic!()
        };
        state.ledger.alias(&alias, &id);
        let (reply, mut receiver) = oneshot::channel();
        let cancellation = CancellationToken::new();
        state.session_opens.operations.insert(
            id.clone(),
            OpenOperation {
                connection,
                owner: "owner".into(),
                reuse: true,
                scope: CapabilityScope::AllSessions,
                generation: "generation".into(),
                started: true,
                require_command_capture: false,
                waiters: vec![Waiter {
                    request_id: "rpc".into(),
                    owner: "owner".into(),
                    cancellation: cancellation.clone(),
                    recovery_scope: None,
                    reply: reply.into(),
                }],
            },
        );
        state.complete_session_open_success("", "ui-session".into(), &[]);
        assert!(state.session_opens.pending());
        cancellation.cancel();
        state.session_opens.prune_waiters();
        state.complete_session_open_success(&id, "mcp-session".into(), &[]);
        assert!(receiver.try_recv().is_err());
        for key in ["key", "alias"] {
            let Lookup::Complete(Ok(value)) = state
                .ledger
                .begin("owner", "session_open", Some(key), &args)
                .unwrap()
            else {
                panic!()
            };
            assert_eq!(value["sessionId"], "mcp-session");
        }
        assert!(!state.session_opens.pending());
    }
    #[gpui::test]
    fn mcp_session_open_rejects_deferred_serial_hints_without_opening_a_device(
        cx: &mut gpui::TestAppContext,
    ) {
        use super::super::terminal_execution::TerminalExecutionPolicy;
        use crate::features::capability_runtime::{CapabilityCaller, CapabilityRequest};
        use crate::features::test_support::app_with_visible_local_session;
        use crate::test_support::{TestAppCleanup, TestConfigDir};
        use gpui::AppContext as _;
        use nyaterm_core::AiPermissionMode;

        cx.executor().allow_parking();
        let _cleanup = TestAppCleanup::new(cx);
        let directory = TestConfigDir::new("nyaterm-mcp-deferred-serial");
        let app = app_with_visible_local_session(cx, directory.path(), "existing");
        let credential = cx.update_entity(&app, |app, _| {
            app.create_ephemeral_mcp_credential(
                vec!["existing".into()],
                Some("existing".into()),
                AiPermissionMode::FullAccess,
                TerminalExecutionPolicy {
                    background: false,
                    max_lines: 0,
                },
            )
            .unwrap()
        });
        let generation = credential
            .environment()
            .remove("NYATERM_MCP_GENERATION")
            .unwrap();
        for (index, prompt) in [
            json!({"port": true}),
            json!({"baud_rate": true}),
            json!({"port": true, "baud_rate": true}),
        ]
        .into_iter()
        .enumerate()
        {
            cx.update_entity(&app, |app, cx| {
                let saved: SavedConnection = serde_json::from_value(json!({
                    "id": "board", "name": "Board", "type": "serial",
                    "port_name": "COM7", "baud_rate": 115200, "connect_prompt": prompt
                })).unwrap();
                app.connection_state.replace_loaded(vec![saved], Vec::new());
                let submitted = app.blocking_jobs.metrics().submitted;
                let (reply, mut receiver) = oneshot::channel();
                app.dispatch_mcp_session_open(CapabilityRequest {
                    terminal_plan: None, extended_results: true, resolved_session_id: None,
                    connection_id: "owner".into(), request_id: format!("serial-request-{index}"),
                    generation: generation.clone(), client: "fixture".into(),
                    permission_mode: AiPermissionMode::FullAccess,
                    scope: CapabilityScope::explicit(vec!["existing".into()], Some("existing".into())),
                    tool: "session_open".into(),
                    arguments: json!({"connectionId": "board", "idempotencyKey": format!("serial-{index}")}),
                    cancellation: CancellationToken::new(), approved: true, approval_decision: None,
                    caller: CapabilityCaller::Mcp, reply,
                }, cx);
                assert_eq!(receiver.try_recv().unwrap().unwrap_err().code, "execution_failed");
                assert!(!app.mcp.capabilities.session_opens.pending());
                assert!(!app.session.start_has_pending());
                assert_eq!(app.session.session_order(), &["existing"]);
                assert_eq!(app.blocking_jobs.metrics().submitted, submitted);
            });
        }
        drop(credential);
    }

    #[gpui::test]
    fn reuse_uses_source_identity_and_scope_and_default_open_still_creates(
        cx: &mut gpui::TestAppContext,
    ) {
        use super::super::terminal_execution::TerminalExecutionPolicy;
        use crate::features::capability_runtime::{CapabilityCaller, CapabilityRequest};
        use crate::features::test_support::app_with_visible_local_session;
        use crate::test_support::TestConfigDir;
        use gpui::AppContext as _;
        use nyaterm_core::AiPermissionMode;
        cx.executor().allow_parking();
        let directory = TestConfigDir::new("nyaterm-session-open-contract");
        let app = app_with_visible_local_session(cx, directory.path(), "existing");
        let credential = cx.update_entity(&app, |app, _cx| {
            let saved:SavedConnection=serde_json::from_value(json!({"id":"saved","name":"same name","type":"local_terminal","shell_args":"'unterminated"})).unwrap();
            app.connection_state.replace_loaded(vec![saved],Vec::new());
            let mut metadata=app.session.metadata("existing").unwrap().clone();metadata.source_connection_id=Some("saved".into());app.session.register_session_metadata("existing",metadata);
            app.create_ephemeral_mcp_credential(vec!["existing".into()],Some("existing".into()),AiPermissionMode::FullAccess,TerminalExecutionPolicy{background:false,max_lines:0}).unwrap()
        });
        let generation = credential
            .environment()
            .remove("NYATERM_MCP_GENERATION")
            .unwrap();
        for (index, owner, mode, source, expected) in [
            (0, "owner-a", Some("reuse_or_create"), "saved", true),
            (1, "owner-b", Some("reuse_or_create"), "saved", true),
            (
                2,
                "owner-a",
                Some("reuse_or_create"),
                "different-saved-id",
                false,
            ),
            (3, "owner-a", None, "saved", false),
        ] {
            cx.update_entity(&app, |app, cx| {
                let mut metadata = app.session.metadata("existing").unwrap().clone();
                metadata.source_connection_id = Some(source.into());
                app.session.register_session_metadata("existing", metadata);
                let mut arguments =
                    json!({"connectionId":"saved","idempotencyKey":format!("key-{index}")});
                if let Some(mode) = mode {
                    arguments["mode"] = json!(mode);
                }
                let (reply, mut receiver) = oneshot::channel();
                app.dispatch_mcp_session_open(
                    CapabilityRequest {
                        terminal_plan: None,
                        extended_results: true,
                        resolved_session_id: None,
                        connection_id: owner.into(),
                        request_id: format!("request-{index}"),
                        generation: generation.clone(),
                        client: "fixture".into(),
                        permission_mode: AiPermissionMode::FullAccess,
                        scope: CapabilityScope::explicit(
                            vec!["existing".into()],
                            Some("existing".into()),
                        ),
                        tool: "session_open".into(),
                        arguments,
                        cancellation: CancellationToken::new(),
                        approved: true,
                        approval_decision: None,
                        caller: CapabilityCaller::Mcp,
                        reply,
                    },
                    cx,
                );
                let result = receiver.try_recv().unwrap();
                assert_eq!(result.is_ok(), expected);
                if expected {
                    assert_eq!(result.unwrap()["sessionId"], "existing");
                }
                assert!(!app.session.saved_connection_is_starting("saved"));
            });
        }
        drop(credential);
    }
}
