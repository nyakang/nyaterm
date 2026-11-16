use super::TerminalExecutionState;
use crate::features::NyaTermApp;
use crate::features::capability_runtime::CapabilityRequest;
use crate::features::capability_runtime::session_open::{SessionOpenContext, SessionOpenReply};
use crate::features::mcp::server::rpc_failure;
use gpui::Context;
use nyaterm_core::models::connection::{AiExecutionProfile, ConnectionType, SshProfile};
use nyaterm_core::network_cli_prompt::resolve_ssh_execution_profile;
use nyaterm_mcp_protocol::{RpcError, SessionOpenResult, TerminalStartArgs};
use std::time::{Duration, Instant};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

#[cfg(test)]
mod tests;

pub(super) struct PendingInlineStart {
    request: CapabilityRequest,
    args: TerminalStartArgs,
    deadline: Instant,
    opening: CancellationToken,
    receiver: oneshot::Receiver<Result<SessionOpenResult, RpcError>>,
}

impl TerminalExecutionState {
    pub(super) fn has_pending_inline_starts(&self) -> bool {
        !self.pending_inline_starts.is_empty()
    }

    pub(super) fn pending_inline_start_count(&self, owner: Option<&str>) -> usize {
        self.pending_inline_starts
            .values()
            .filter(|pending| owner.is_none_or(|owner| pending.request.connection_id == owner))
            .count()
    }

    pub(super) fn cancel_pending_inline_starts(&mut self) {
        for pending in self.pending_inline_starts.values() {
            pending.request.cancellation.cancel();
            pending.opening.cancel();
        }
    }
}

impl NyaTermApp {
    fn validate_inline_saved_target(
        &self,
        request: &CapabilityRequest,
        connection_id: &str,
    ) -> Result<(), RpcError> {
        self.authorize_saved_target(request, connection_id)?;
        let connection = self
            .connection_state
            .connection_by_id(connection_id)
            .ok_or_else(|| {
                rpc_failure(
                    "invalid_argument",
                    "The saved SSH connection does not exist.",
                )
            })?;
        let ConnectionType::Ssh {
            ai_execution_profile,
            ..
        } = &connection.config
        else {
            return Err(rpc_failure(
                "invalid_argument",
                "Inline connection jobs require SSH.",
            ));
        };
        if !matches!(
            resolve_ssh_execution_profile(
                *ai_execution_profile,
                connection.ssh_profile == SshProfile::NetworkDevice,
                ""
            ),
            AiExecutionProfile::Posix | AiExecutionProfile::Powershell | AiExecutionProfile::Cmd
        ) {
            return Err(rpc_failure(
                "permission_denied",
                "Inline jobs require a supported command-capture shell.",
            ));
        }
        Ok(())
    }

    pub(super) fn start_connection_inline(
        &mut self,
        request: CapabilityRequest,
        args: TerminalStartArgs,
        operation_id: String,
        cx: &mut Context<Self>,
    ) {
        let connection_id = args
            .target
            .as_ref()
            .and_then(|target| target.connection_id())
            .expect("connection target");
        let preparation = (|| {
            if request.cancellation.is_cancelled() {
                return Err(rpc_failure(
                    "cancelled",
                    "The start was cancelled before submission.",
                ));
            }
            self.validate_inline_saved_target(&request, connection_id)?;
            if !self
                .mcp
                .capabilities
                .terminal_executions
                .can_register_job(&request.connection_id)
            {
                return Err(rpc_failure("resource_limit", "The job registry is full."));
            }
            let connection = self
                .connection_state
                .connection_by_id(connection_id)
                .cloned()
                .ok_or_else(|| {
                    rpc_failure(
                        "invalid_argument",
                        "The saved SSH connection does not exist.",
                    )
                })?;
            Ok(connection)
        })();
        let connection = match preparation {
            Ok(connection) => connection,
            Err(error) => {
                self.complete_terminal_start(request, &operation_id, Err(error), cx);
                return;
            }
        };
        let opening = request.cancellation.child_token();
        let context = SessionOpenContext::for_terminal_start(&request, opening.clone());
        let deadline = Instant::now() + Duration::from_millis(args.timeout_ms.unwrap_or(300_000));
        let (reply, receiver) = oneshot::channel();
        self.mcp
            .capabilities
            .terminal_executions
            .pending_inline_starts
            .insert(
                operation_id,
                PendingInlineStart {
                    request,
                    args,
                    deadline,
                    opening,
                    receiver,
                },
            );
        // The opener owns transport preparation; this continuation owns the command.
        self.queue_mcp_session_open(
            context,
            connection,
            true,
            nyaterm_core::uuid(),
            SessionOpenReply::TerminalStart(reply),
            cx,
        );
        self.ensure_terminal_execution_clock(cx);
    }

    pub(super) fn poll_connection_inline_starts(&mut self, cx: &mut Context<Self>) {
        let ids = self
            .mcp
            .capabilities
            .terminal_executions
            .pending_inline_starts
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for id in ids {
            let pending = &self
                .mcp
                .capabilities
                .terminal_executions
                .pending_inline_starts[&id];
            let error = if pending.request.cancellation.is_cancelled() {
                Some(rpc_failure(
                    "cancelled",
                    "The start was cancelled before submission.",
                ))
            } else if Instant::now() >= pending.deadline {
                Some(rpc_failure(
                    "execution_failed",
                    "The saved connection start timed out.",
                ))
            } else if !self.mcp.generation_matches(&pending.request.generation) {
                Some(rpc_failure(
                    "authentication_failed",
                    "MCP credential expired.",
                ))
            } else {
                let connection_id = pending
                    .args
                    .target
                    .as_ref()
                    .and_then(|target| target.connection_id())
                    .expect("connection target");
                self.validate_inline_saved_target(&pending.request, connection_id)
                    .err()
            };
            let result = if let Some(error) = error {
                Some(Err(error))
            } else {
                let pending = self
                    .mcp
                    .capabilities
                    .terminal_executions
                    .pending_inline_starts
                    .get_mut(&id)
                    .unwrap();
                match pending.receiver.try_recv() {
                    Ok(result) => Some(result),
                    Err(oneshot::error::TryRecvError::Empty) => None,
                    Err(oneshot::error::TryRecvError::Closed) => Some(Err(rpc_failure(
                        "execution_failed",
                        "The saved connection could not be opened.",
                    ))),
                }
            };
            let Some(result) = result else { continue };
            let mut pending = self
                .mcp
                .capabilities
                .terminal_executions
                .pending_inline_starts
                .remove(&id)
                .unwrap();
            pending.opening.cancel();
            let result = result.and_then(|opened| {
                pending
                    .request
                    .scope
                    .resolve(&self.mcp_live_sessions())
                    .require(&opened.session_id)
                    .map_err(|_| {
                        rpc_failure("scope_denied", "The opened terminal is outside scope.")
                    })?;
                let remaining = pending.deadline.saturating_duration_since(Instant::now());
                let remaining_ms = remaining.as_millis().try_into().unwrap_or(u64::MAX);
                if remaining_ms == 0 {
                    return Err(rpc_failure(
                        "execution_failed",
                        "The saved connection start timed out.",
                    ));
                }
                pending.args.target = None;
                pending.args.session_id = Some(opened.session_id.clone());
                pending.args.timeout_ms = Some(remaining_ms);
                self.start_terminal_job(
                    &pending.request,
                    pending.args,
                    opened.session_id,
                    id.clone(),
                    cx,
                )
            });
            self.complete_terminal_start(pending.request, &id, result, cx);
        }
    }
}
