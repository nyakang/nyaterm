//! Side-effect results outlive RPC waiters. Expired keys are never replayed.
use nyaterm_mcp_protocol::RpcError;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub(super) enum Lookup {
    New(String),
    Pending(String),
    Complete(Result<Value, RpcError>),
}
struct Entry {
    fingerprint: [u8; 32],
    operation: String,
    result: Option<Result<Value, RpcError>>,
    completed: Option<Instant>,
    pinned: bool,
}
#[derive(Default)]
pub(super) struct IdempotencyLedger {
    entries: HashMap<(String, String, String), Entry>,
}
impl IdempotencyLedger {
    pub fn contains(&self, owner: &str, tool: &str, key: &str) -> bool {
        self.entries
            .contains_key(&(owner.into(), tool.into(), key.into()))
    }
    pub fn begin(
        &mut self,
        owner: &str,
        tool: &str,
        key: Option<&str>,
        arguments: &Value,
    ) -> Result<Lookup, RpcError> {
        let operation = nyaterm_core::uuid();
        let Some(key) = key else {
            return Ok(Lookup::New(operation));
        };
        let key = (owner.into(), tool.into(), key.into());
        let fingerprint: [u8; 32] =
            Sha256::digest(serde_json::to_vec(arguments).expect("JSON value")).into();
        if let Some(entry) = self.entries.get(&key) {
            if entry.fingerprint != fingerprint {
                return Err(super::super::mcp::server::rpc_failure(
                    "idempotency_conflict",
                    "The key belongs to different operation parameters.",
                ));
            }
            if !entry.pinned
                && entry
                    .completed
                    .is_some_and(|at| at.elapsed() >= Duration::from_secs(600))
            {
                return Err(super::super::mcp::server::rpc_failure(
                    "idempotency_expired",
                    "The operation result expired. It will not be executed again with this key.",
                ));
            }
            return Ok(if let Some(result) = &entry.result {
                Lookup::Complete(result.clone())
            } else {
                Lookup::Pending(entry.operation.clone())
            });
        }
        if self.entries.len() >= 1024
            || self.entries.keys().filter(|(id, _, _)| id == owner).count() >= 256
        {
            return Err(super::super::mcp::server::rpc_failure(
                "resource_limit",
                "The client idempotency ledger is full.",
            ));
        }
        self.entries.insert(
            key,
            Entry {
                fingerprint,
                operation: operation.clone(),
                result: None,
                completed: None,
                pinned: false,
            },
        );
        Ok(Lookup::New(operation))
    }
    pub fn finish(&mut self, operation: &str, result: Result<Value, RpcError>) {
        for entry in self
            .entries
            .values_mut()
            .filter(|entry| entry.operation == operation)
        {
            entry.result = Some(result.clone());
            entry.completed = Some(Instant::now());
        }
    }
    pub fn requires_cleanup(&self) -> bool {
        self.entries.values().any(|entry| entry.result.is_some())
    }
    pub fn cleanup(&mut self) {
        for entry in self.entries.values_mut() {
            if !entry.pinned
                && entry
                    .completed
                    .is_some_and(|at| at.elapsed() >= Duration::from_secs(600))
            {
                entry.result = None;
            }
        }
    }
    pub fn forget_expired_owner(&mut self, owner: &str) {
        // Called only after the Host has irreversibly expired this issued identity.
        self.entries.retain(|(id, _, _), _| id != owner);
    }
    pub fn alias(&mut self, operation: &str, original: &str) {
        for entry in self
            .entries
            .values_mut()
            .filter(|e| e.operation == operation)
        {
            entry.operation = original.into();
        }
    }
    pub fn pin(&mut self, operation: &str, pinned: bool) {
        for entry in self
            .entries
            .values_mut()
            .filter(|e| e.operation == operation)
        {
            if entry.pinned && !pinned {
                entry.completed = Some(Instant::now());
            }
            entry.pinned = pinned;
        }
    }
}
#[cfg(test)]
mod tests {
    use super::{IdempotencyLedger, Lookup};
    use serde_json::json;
    #[test]
    fn expired_keys_keep_capacity_protection_instead_of_allowing_replay() {
        let mut ledger = IdempotencyLedger::default();
        let arguments = json!({"connectionId":"saved"});
        for index in 0..256 {
            let Lookup::New(operation) = ledger
                .begin(
                    "owner",
                    "session_open",
                    Some(&index.to_string()),
                    &arguments,
                )
                .unwrap()
            else {
                panic!()
            };
            ledger.finish(&operation, Ok(json!({"sessionId": "created"})));
        }
        for entry in ledger.entries.values_mut() {
            entry.completed = Some(std::time::Instant::now() - std::time::Duration::from_secs(601));
        }
        ledger.cleanup();
        assert!(
            matches!(ledger.begin("owner", "session_open", Some("new"), &arguments), Err(error) if error.code == "resource_limit")
        );
        assert!(
            matches!(ledger.begin("owner", "session_open", Some("0"), &arguments), Err(error) if error.code == "idempotency_expired")
        );
        assert!(matches!(
            ledger
                .begin("other-owner", "session_open", Some("0"), &arguments)
                .unwrap(),
            Lookup::New(_)
        ));
    }
    #[test]
    fn retries_join_results_and_expired_or_conflicting_keys_never_reexecute() {
        let mut ledger = IdempotencyLedger::default();
        let args = json!({"connectionId":"a"});
        let Lookup::New(id) = ledger
            .begin("owner", "session_open", Some("key"), &args)
            .unwrap()
        else {
            panic!()
        };
        assert!(
            matches!(ledger.begin("owner","session_open",Some("key"),&args).unwrap(),Lookup::Pending(other) if other==id)
        );
        ledger.finish(&id, Ok(json!({"sessionId":"created"})));
        assert!(matches!(
            ledger
                .begin("owner", "session_open", Some("key"), &args)
                .unwrap(),
            Lookup::Complete(Ok(_))
        ));
        assert!(
            ledger
                .begin(
                    "owner",
                    "session_open",
                    Some("key"),
                    &json!({"connectionId":"b"})
                )
                .is_err()
        );
        ledger.entries.values_mut().next().unwrap().completed =
            Some(std::time::Instant::now() - std::time::Duration::from_secs(601));
        ledger.cleanup();
        assert!(
            ledger
                .begin("owner", "session_open", Some("key"), &args)
                .is_err()
        );
        assert!(matches!(
            ledger
                .begin("other", "session_open", Some("key"), &args)
                .unwrap(),
            Lookup::New(_)
        ));
    }
}
