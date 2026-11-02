//! Persistent records used by the cloud coordinator. No HTTP or GPUI ownership.
use nyaterm_core::sync_schema::secrets::EncryptedSecretValue;
use nyaterm_core::sync_schema::{SyncFieldId, SyncProjection, SyncScope, SyncValues};
use nyaterm_sync::engine::{ReplicaSnapshot, SyncConflict, SyncPlan};
use nyaterm_sync::protocol::{PendingPublication, PublicationId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use uuid::Uuid;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalReplica {
    pub schema: u32,
    pub writer: Uuid,
    pub owner: String,
    pub next_sequence: u64,
    pub replica: Option<ReplicaSnapshot>,
    pub effective: SyncValues,
    #[serde(default)]
    pub secret_objects: BTreeMap<String, EncryptedSecretValue>,
    #[serde(default)]
    pub applied_generation: u64,
    pub scope: SyncScope,
    pub pending: Option<PendingPublication>,
    pub published_document: Option<String>,
    pub observed: BTreeMap<String, PublicationId>,
    pub conflicts: Vec<SyncConflict>,
    #[serde(default)]
    pub deferred: Vec<SyncFieldId>,
    pub legacy_fingerprint: Option<String>,
    pub migration_approved: bool,
}

impl Default for LocalReplica {
    fn default() -> Self {
        Self {
            schema: 4,
            writer: Uuid::new_v4(),
            owner: String::new(),
            next_sequence: 1,
            replica: None,
            effective: SyncValues::new(),
            secret_objects: BTreeMap::new(),
            applied_generation: 0,
            scope: SyncScope::default(),
            pending: None,
            published_document: None,
            observed: BTreeMap::new(),
            conflicts: Vec::new(),
            deferred: Vec::new(),
            legacy_fingerprint: None,
            migration_approved: false,
        }
    }
}

pub struct PreparedApply {
    pub destination: String,
    pub expected_record: Option<String>,
    pub generation: u64,
    pub local: LocalReplica,
    pub plan: SyncPlan,
    pub trust_approvals: BTreeMap<String, String>,
}

pub struct LocalView {
    pub record: LocalReplica,
    pub record_hash: Option<String>,
    pub generation: u64,
    pub projection: SyncProjection,
}

/// A durable, exact-byte rotation publication. No password leaves the Store in
/// this ticket; the settings switch is a separate compare-and-swap transaction.
pub struct PasswordRotationPublication {
    pub ticket: String,
    pub remote_root: String,
    pub pending: PendingPublication,
}
