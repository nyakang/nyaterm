//! Desktop job/event adapter; keeps the MCP credential alive through child cleanup.
use super::helper_resolver::resolve_mcp_helper;
use crate::features::mcp::McpEphemeralCredential;
use crate::features::runtime_jobs::AiChatWorkerEvent;
use futures::channel::mpsc::UnboundedSender;
pub(in crate::features) use nyaterm_agent_client::claude_code::ClaudeCodeRunResult;
use nyaterm_core::{AiChatRequest, AiSettings};
use std::sync::{Arc, atomic::AtomicBool};

#[allow(clippy::too_many_arguments)]
pub(in crate::features) fn run_claude_code(
    settings: &AiSettings,
    request: &AiChatRequest,
    stream_tx: Option<&UnboundedSender<AiChatWorkerEvent>>,
    cancel: &Arc<AtomicBool>,
    job_id: u64,
    session_id: &str,
    mcp_credential: McpEphemeralCredential,
    mut on_session_id: impl FnMut(&str) -> Result<(), String>,
) -> Result<ClaudeCodeRunResult, String> {
    if !settings.claude_code.enabled {
        return Err("Claude Code integration is disabled".to_string());
    }
    let environment = mcp_credential.environment();
    let mcp_helper = resolve_mcp_helper()?;
    let result = nyaterm_agent_client::claude_code::run_claude_code(
        settings,
        request,
        &mut |text_delta, reasoning_delta| {
            if let Some(tx) = stream_tx {
                let _ = tx.unbounded_send(AiChatWorkerEvent::Delta {
                    job_id,
                    session_id: session_id.to_string(),
                    text_delta,
                    reasoning_delta,
                });
            }
        },
        cancel,
        environment,
        &mcp_helper,
        &mut on_session_id,
    );
    drop(mcp_credential);
    result
}
