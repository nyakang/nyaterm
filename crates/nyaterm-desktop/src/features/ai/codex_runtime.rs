//! Desktop job/event adapter; keeps the MCP credential alive through child cleanup.
use super::helper_resolver::resolve_mcp_helper;
use crate::features::mcp::McpEphemeralCredential;
use crate::features::runtime_jobs::AiChatWorkerEvent;
use futures::channel::mpsc::UnboundedSender;
pub(in crate::features) use nyaterm_agent_client::codex::CodexRunResult;
use nyaterm_core::{AiChatRequest, AiSettings};
use std::sync::{Arc, atomic::AtomicBool};

#[allow(clippy::too_many_arguments)]
pub(in crate::features) fn run_codex_app_server(
    settings: &AiSettings,
    request: &AiChatRequest,
    existing_thread_id: Option<&str>,
    stream_tx: Option<&UnboundedSender<AiChatWorkerEvent>>,
    cancel: &Arc<AtomicBool>,
    job_id: u64,
    session_id: &str,
    mcp_credential: McpEphemeralCredential,
) -> Result<CodexRunResult, String> {
    if !settings.codex.enabled {
        return Err("Codex integration is disabled".to_string());
    }
    let mcp_environment = mcp_credential.environment();
    let mcp_helper = resolve_mcp_helper()?;
    let result = nyaterm_agent_client::codex::run_codex_app_server(
        settings,
        request,
        existing_thread_id,
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
        mcp_environment,
        &mcp_helper,
    );
    drop(mcp_credential);
    result
}
