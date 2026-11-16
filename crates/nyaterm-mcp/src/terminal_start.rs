//! Forwarder-local waiting; host arguments and idempotency remain unchanged.

use std::time::Duration;

use nyaterm_mcp_protocol::{ToolContractError, tool, validate_tool_result};
use serde_json::{Value, json};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::bridge::BridgeClient;

const MAX_WAIT_MS: u64 = 30_000;
const DEFAULT_WAIT_MS: u64 = 15_000;

pub(super) const WAIT_DESCRIPTION: &str = "Actual ssh_exec jobs wait up to 15s by default; inline_pty returns immediately. Set waitMs: 0 to opt out or a positive budget (capped at 30000ms). Waiting excludes approvals/registration. Timeout or poll errors return the last valid result; the job continues. Read remaining output with terminal_poll offset: nextOffset; eof is not exit evidence.";

#[derive(Default)]
pub(super) struct StartWait(Option<u64>);

impl StartWait {
    pub(super) fn take(name: &str, arguments: &mut Value) -> Result<Self, &'static str> {
        if name != tool::TERMINAL_START {
            return Ok(Self::default());
        }
        let requested = arguments
            .as_object_mut()
            .and_then(|object| object.remove("waitMs"));
        match requested {
            None | Some(Value::Null) => Ok(Self::default()),
            Some(value) => value
                .as_u64()
                .map(|wait| Self(Some(wait.min(MAX_WAIT_MS))))
                .ok_or("waitMs must be a nonnegative integer or null"),
        }
    }

    fn wait_ms(&self, start: &Value) -> u64 {
        self.0.unwrap_or_else(|| {
            if start["backend"] == "ssh_exec" {
                DEFAULT_WAIT_MS
            } else {
                0
            }
        })
    }

    /// The caller validates start first; only valid polls can replace it.
    pub(super) async fn wait_for_result(
        &self,
        bridge: &BridgeClient,
        start: Value,
        cancellation: CancellationToken,
    ) -> Value {
        let wait_ms = self.wait_ms(&start);
        let Some(job_id) = start["jobId"].as_str().map(str::to_owned) else {
            return start;
        };
        if wait_ms == 0 {
            return start;
        }
        let deadline = Instant::now() + Duration::from_millis(wait_ms);
        let mut backoff = Duration::from_millis(150);
        let mut latest = start;
        loop {
            if is_transient(&latest) {
                tokio::select! {
                    biased;
                    _ = cancellation.cancelled() => return latest,
                    _ = tokio::time::sleep_until((Instant::now() + backoff).min(deadline)) => {}
                }
            }
            if cancellation.is_cancelled() || Instant::now() >= deadline {
                return latest;
            }
            // A terminal start response still needs one poll to fetch its output.
            // Every poll reads from the original offset; never accumulate pages
            // or let a malformed host response redirect us to another job.
            match bridge
                .call_with_deadline(
                    tool::TERMINAL_POLL,
                    json!({ "jobId": job_id }),
                    cancellation.clone(),
                    deadline,
                )
                .await
            {
                Ok(value)
                    if validate_tool_result(tool::TERMINAL_POLL, &value).is_ok()
                        && value["jobId"].as_str() == Some(job_id.as_str()) =>
                {
                    latest = value;
                }
                _ => return latest,
            }
            if !is_transient(&latest) {
                return latest;
            }
            backoff = (backoff * 2).min(Duration::from_millis(1200));
        }
    }
}

fn is_transient(value: &Value) -> bool {
    matches!(
        value["status"].as_str(),
        Some("starting" | "running" | "cancelling")
    )
}

pub(super) fn input_schema(name: &str, targets: bool) -> Result<Value, ToolContractError> {
    let mut schema = nyaterm_mcp_protocol::tool_input_schema_for_features(name, targets)?;
    if name == tool::TERMINAL_START {
        schema["properties"]["waitMs"] = json!({
            "type": ["integer", "null"], "minimum": 0,
            "description": "Forwarder-local wait budget in milliseconds, capped at 30000. Omitted or null: 15000 for the actual ssh_exec backend, 0 for inline_pty. Zero disables waiting; positive values wait for either backend. Excludes approvals and registration."
        });
    }
    Ok(schema)
}

pub(super) fn result_schema(name: &str, targets: bool) -> Option<Value> {
    let start = nyaterm_mcp_protocol::terminal_job_result_schema_for_features(name, targets)?;
    if name != tool::TERMINAL_START {
        return Some(start);
    }
    let poll = nyaterm_mcp_protocol::terminal_job_result_schema_for_features(
        tool::TERMINAL_POLL,
        targets,
    )?;
    // Shared schemas use root-relative references. Hoist their definitions into
    // the new root rather than nesting them under anyOf and breaking those refs.
    let mut definitions = serde_json::Map::new();
    let mut variants = Vec::new();
    for mut schema in [start, poll] {
        if let Some(Value::Object(defs)) = schema.as_object_mut()?.remove("$defs") {
            definitions.extend(defs);
        }
        if let Some(Value::Array(alternatives)) = schema.as_object_mut()?.remove("anyOf") {
            variants.extend(alternatives);
        } else {
            variants.push(schema);
        }
    }
    Some(json!({ "type": "object", "anyOf": variants, "$defs": definitions }))
}

#[cfg(test)]
mod tests;
