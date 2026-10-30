use std::collections::{BTreeMap, BTreeSet};

use automerge::{ActorId, AutoCommit, ROOT, ReadDoc, ScalarValue, transaction::Transactable};
use nyaterm_core::sync_schema::{
    SyncFieldId, SyncProjection, SyncResourceId, SyncScope, SyncValue, SyncValues, supports_field,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum SyncError {
    #[error("invalid or unsupported sync replica")]
    InvalidReplica,
    #[error("sync conflict changed; refresh the preview")]
    StaleConflict,
    #[error("local sync configuration changed; refresh the plan")]
    StalePlan,
    #[error("sync resource is excluded or unsupported")]
    Excluded,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplicaSnapshot {
    pub schema: u32,
    pub document: Vec<u8>,
    /// Only references are retained by Automerge. Adapters encrypt this value
    /// inventory at rest, including all historical conflict candidates.
    pub values: BTreeMap<String, SyncValue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncCandidate {
    pub operation: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncConflict {
    pub field: SyncFieldId,
    pub candidates: Vec<SyncCandidate>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct SyncPlan {
    pub expected_local: SyncProjection,
    pub scope: SyncScope,
    pub replica: ReplicaSnapshot,
    pub effective: SyncValues,
    pub conflicts: Vec<SyncConflict>,
}

pub struct SyncReplica {
    document: AutoCommit,
    values: BTreeMap<String, SyncValue>,
}

type Registers = BTreeMap<SyncResourceId, BTreeMap<String, Vec<SyncCandidate>>>;

impl SyncReplica {
    pub fn new(actor: Uuid) -> Self {
        let mut document = AutoCommit::new();
        document.set_actor(ActorId::from(actor.as_bytes().as_slice()));
        Self {
            document,
            values: BTreeMap::new(),
        }
    }

    pub fn load(snapshot: &ReplicaSnapshot, actor: Uuid) -> Result<Self, SyncError> {
        if snapshot.schema != 4 {
            return Err(SyncError::InvalidReplica);
        }
        let mut document =
            AutoCommit::load(&snapshot.document).map_err(|_| SyncError::InvalidReplica)?;
        document.set_actor(ActorId::from(actor.as_bytes().as_slice()));
        let replica = Self {
            document,
            values: snapshot.values.clone(),
        };
        replica.validate()?;
        Ok(replica)
    }

    pub fn snapshot(&mut self) -> ReplicaSnapshot {
        ReplicaSnapshot {
            schema: 4,
            document: self.document.save(),
            values: self.values.clone(),
        }
    }

    pub fn merge(&mut self, snapshot: &ReplicaSnapshot) -> Result<(), SyncError> {
        let mut remote = Self::load(snapshot, Uuid::nil())?;
        // A value ID is a digest of its canonical bytes, never a last-writer map.
        for (id, value) in &remote.values {
            if self.values.get(id).is_some_and(|current| current != value) {
                return Err(SyncError::InvalidReplica);
            }
        }
        self.document
            .merge(&mut remote.document)
            .map_err(|_| SyncError::InvalidReplica)?;
        self.values.extend(remote.values);
        self.validate()
    }

    fn write(&mut self, field: &SyncFieldId, value: SyncValue) -> Result<(), SyncError> {
        let digest = digest(value.bytes());
        self.values.insert(digest.clone(), value);
        // A fresh write identity is necessary even for an unchanged presence
        // value: AutoCommit intentionally elides ordinary equal-value puts.
        let reference = format!("{digest}/{}", Uuid::new_v4());
        self.document
            .put(ROOT, field.key(), ScalarValue::Str(reference.into()))
            .map_err(|_| SyncError::InvalidReplica)?;
        Ok(())
    }

    fn delete_field(&mut self, field: &SyncFieldId) -> Result<(), SyncError> {
        self.document
            .put(
                ROOT,
                field.key(),
                ScalarValue::Str(format!("deleted/{}", Uuid::new_v4()).into()),
            )
            .map_err(|_| SyncError::InvalidReplica)?;
        Ok(())
    }

    /// Migration-only preservation of validated, encrypted opaque data. This
    /// does not authorize business materialization of an unknown resource.
    pub fn preserve(&mut self, projection: &SyncProjection) -> Result<(), SyncError> {
        for (key, value) in &projection.values {
            self.write(
                &SyncFieldId::from_key(key).ok_or(SyncError::InvalidReplica)?,
                value.clone(),
            )?;
        }
        self.document.commit();
        Ok(())
    }

    pub fn candidates(&self, field: &SyncFieldId) -> Result<Vec<SyncCandidate>, SyncError> {
        let mut candidates = self
            .document
            .get_all(ROOT, field.key())
            .map_err(|_| SyncError::InvalidReplica)?
            .into_iter()
            .map(|(value, operation)| {
                let reference = value.as_str().ok_or(SyncError::InvalidReplica)?;
                let (digest, identity) =
                    reference.split_once('/').ok_or(SyncError::InvalidReplica)?;
                Uuid::parse_str(identity).map_err(|_| SyncError::InvalidReplica)?;
                if digest != "deleted" && !self.values.contains_key(digest) {
                    return Err(SyncError::InvalidReplica);
                }
                Ok(SyncCandidate {
                    operation: operation.to_string(),
                    value: digest.into(),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        candidates.sort_by(|a, b| a.operation.cmp(&b.operation));
        Ok(candidates)
    }

    pub fn candidate_value(&self, candidate: &SyncCandidate) -> Result<&SyncValue, SyncError> {
        self.values
            .get(&candidate.value)
            .ok_or(SyncError::InvalidReplica)
    }

    fn fields(&self) -> Result<Vec<SyncFieldId>, SyncError> {
        self.document
            .keys(ROOT)
            .map(|key| SyncFieldId::from_key(&key).ok_or(SyncError::InvalidReplica))
            .collect()
    }

    fn registers(&self) -> Result<Registers, SyncError> {
        let mut registers = Registers::new();
        for field in self.fields()? {
            let candidates = self.candidates(&field)?;
            registers
                .entry(field.resource)
                .or_default()
                .insert(field.field, candidates);
        }
        Ok(registers)
    }

    fn validate(&self) -> Result<(), SyncError> {
        for (id, value) in &self.values {
            if digest(value.bytes()) != *id || value.decode().is_err() {
                return Err(SyncError::InvalidReplica);
            }
        }
        for fields in self.registers()?.values() {
            let candidates = fields.get("$present").ok_or(SyncError::InvalidReplica)?;
            if candidates.is_empty() {
                return Err(SyncError::InvalidReplica);
            }
            for candidate in candidates {
                if !self
                    .candidate_value(candidate)?
                    .decode()
                    .map_err(|_| SyncError::InvalidReplica)?
                    .is_boolean()
                {
                    return Err(SyncError::InvalidReplica);
                }
            }
        }
        Ok(())
    }

    pub fn conflicts(&self) -> Result<Vec<SyncConflict>, SyncError> {
        self.conflicts_from(&self.registers()?)
    }

    fn conflicts_from(&self, registers: &Registers) -> Result<Vec<SyncConflict>, SyncError> {
        let mut conflicts = Vec::new();
        for (resource, fields) in registers {
            let presence = fields.get("$present").ok_or(SyncError::InvalidReplica)?;
            let hidden = distinct(presence) > 1 || self.boolean(presence)? == Some(false);
            for (name, candidates) in fields {
                if name != "$present" && hidden {
                    continue;
                }
                if distinct(candidates) > 1 {
                    conflicts.push(SyncConflict {
                        field: SyncFieldId::new(resource.clone(), name),
                        candidates: candidates.clone(),
                    });
                }
            }
        }
        conflicts.sort_by(|a, b| a.field.cmp(&b.field));
        Ok(conflicts)
    }

    fn boolean(&self, candidates: &[SyncCandidate]) -> Result<Option<bool>, SyncError> {
        candidates
            .first()
            .map(|candidate| {
                self.candidate_value(candidate)?
                    .decode()
                    .map_err(|_| SyncError::InvalidReplica)?
                    .as_bool()
                    .ok_or(SyncError::InvalidReplica)
            })
            .transpose()
    }

    /// Capture against effective business data, never against Automerge's
    /// selected winner. Incomplete observations cannot prove deletion.
    pub fn capture(
        &mut self,
        baseline: &SyncValues,
        current: &SyncProjection,
        scope: &SyncScope,
    ) -> Result<bool, SyncError> {
        if current.edited.is_empty() && baseline == &current.values {
            return Ok(false);
        }
        let keys = baseline
            .keys()
            .chain(current.values.keys())
            .collect::<BTreeSet<_>>();
        let mut changed = BTreeSet::new();
        let mut removed = BTreeSet::new();
        for key in &keys {
            let field = SyncFieldId::from_key(key).ok_or(SyncError::InvalidReplica)?;
            if field.field != "$present"
                || !scope.includes(&field.resource.kind)
                || !current.complete.contains(&field.resource.kind)
            {
                continue;
            }
            if baseline.contains_key(*key) && !current.values.contains_key(*key) {
                self.write(&field, SyncValue::new(&serde_json::Value::Bool(false)))?;
                removed.insert(field.resource);
            }
        }
        for key in keys {
            let field = SyncFieldId::from_key(key).ok_or(SyncError::InvalidReplica)?;
            if !scope.includes(&field.resource.kind)
                || !supports_field(&field.resource.kind, &field.field)
                || (!current.complete.contains(&field.resource.kind)
                    && !current.values.contains_key(key))
                || removed.contains(&field.resource)
                || (baseline.get(key) == current.values.get(key)
                    && !current.edited.contains(&field))
            {
                continue;
            }
            if let Some(value) = current.values.get(key) {
                self.write(&field, value.clone())?;
            } else {
                // A field tombstone is a value distinct from JSON null.
                self.delete_field(&field)?;
            }
            changed.insert(field.resource);
        }
        for resource in &changed {
            self.write(
                &SyncFieldId::presence(resource.clone()),
                SyncValue::new(&serde_json::Value::Bool(true)),
            )?;
        }
        self.document.commit();
        Ok(!changed.is_empty() || !removed.is_empty())
    }

    pub fn materialize(
        &self,
        local: &SyncValues,
        scope: &SyncScope,
    ) -> Result<SyncValues, SyncError> {
        self.materialize_from(local, scope, &self.registers()?)
    }

    fn materialize_from(
        &self,
        local: &SyncValues,
        scope: &SyncScope,
        resources: &Registers,
    ) -> Result<SyncValues, SyncError> {
        let mut result = local.clone();
        let conflicts = self.conflicts_from(resources)?;
        let blocked_new = conflicts
            .iter()
            .filter(|conflict| supports_field(&conflict.field.resource.kind, &conflict.field.field))
            .map(|conflict| conflict.field.resource.clone())
            .collect::<BTreeSet<_>>();
        let mut deleted = BTreeSet::new();
        for (resource, fields) in resources {
            if !scope.includes(&resource.kind) {
                continue;
            }
            let presence_key = SyncFieldId::presence(resource.clone());
            let candidates = fields.get("$present").ok_or(SyncError::InvalidReplica)?;
            if distinct(candidates) > 1 {
                continue;
            }
            if self.boolean(candidates)? == Some(false) {
                deleted.insert(resource.clone());
                continue;
            }
            if !local.contains_key(&presence_key.key()) && blocked_new.contains(resource) {
                continue;
            }
            for (name, candidates) in fields {
                if !supports_field(&resource.kind, name) {
                    continue;
                }
                let field = SyncFieldId::new(resource.clone(), name);
                if distinct(candidates) == 1 {
                    if candidates[0].value == "deleted" {
                        result.remove(&field.key());
                    } else {
                        result.insert(field.key(), self.candidate_value(&candidates[0])?.clone());
                    }
                }
            }
        }
        if !deleted.is_empty() {
            result.retain(|key, _| {
                SyncFieldId::from_key(key).is_none_or(|field| !deleted.contains(&field.resource))
            });
        }
        Ok(result)
    }

    pub fn resolve(
        &mut self,
        conflict: &SyncConflict,
        candidate: &SyncCandidate,
        scope: &SyncScope,
    ) -> Result<(), SyncError> {
        if !scope.includes(&conflict.field.resource.kind)
            || !supports_field(&conflict.field.resource.kind, &conflict.field.field)
        {
            return Err(SyncError::Excluded);
        }
        if self.candidates(&conflict.field)? != conflict.candidates
            || !conflict.candidates.contains(candidate)
        {
            return Err(SyncError::StaleConflict);
        }
        if candidate.value == "deleted" {
            self.delete_field(&conflict.field)?;
        } else {
            self.write(&conflict.field, self.candidate_value(candidate)?.clone())?;
        }
        self.document.commit();
        Ok(())
    }
}

pub fn prepare(
    actor: Uuid,
    local: Option<&ReplicaSnapshot>,
    baseline: &SyncValues,
    current: SyncProjection,
    remotes: &[ReplicaSnapshot],
    scope: SyncScope,
) -> Result<SyncPlan, SyncError> {
    let mut replica = if let Some(snapshot) = local {
        SyncReplica::load(snapshot, actor)?
    } else {
        SyncReplica::new(actor)
    };
    // A category re-enabled after being excluded may already exist in the
    // document without ever having been confirmed by this device. Seed those
    // resources on an independent branch, so observation is not mistaken for
    // user confirmation and cannot causally overwrite their remote candidates.
    let unbased = current
        .values
        .keys()
        .filter_map(|key| SyncFieldId::from_key(key))
        .filter(|field| field.field == "$present" && !baseline.contains_key(&field.key()))
        .map(|field| field.resource)
        .collect::<BTreeSet<_>>();
    let mut ordinary = current.clone();
    let mut initial_remote_resources = BTreeSet::new();
    if local.is_none() {
        for remote in remotes {
            for field in SyncReplica::load(remote, Uuid::nil())?.fields()? {
                if field.field == "$present" {
                    initial_remote_resources.insert(field.resource);
                }
            }
        }
    }
    let mut independent = SyncProjection {
        complete: current.complete.clone(),
        ..Default::default()
    };
    let default_ai = if scope.ai && local.is_none() {
        nyaterm_core::sync_schema::ai::default_resources(&current.values)
    } else {
        BTreeSet::new()
    };
    ordinary.values.retain(|key, value| {
        if SyncFieldId::from_key(key).is_some_and(|field| unbased.contains(&field.resource)) {
            let field = SyncFieldId::from_key(key).expect("validated identity");
            let value_key = SyncFieldId::new(field.resource.clone(), "value").key();
            let default_on_join = local.is_none()
                && (default_ai.contains(&field.resource)
                    || current.values.get(&value_key).is_some_and(|value| {
                        nyaterm_core::sync_schema::is_default_preference(&field.resource, value)
                    }))
                && initial_remote_resources.contains(&field.resource);
            if !default_on_join {
                independent.values.insert(key.clone(), value.clone());
            }
            false
        } else {
            true
        }
    });
    replica.capture(baseline, &ordinary, &scope)?;
    if !independent.values.is_empty() {
        let mut seed = SyncReplica::new(Uuid::new_v4());
        seed.capture(&SyncValues::new(), &independent, &scope)?;
        replica.merge(&seed.snapshot())?;
    }
    // Local writes must branch before observing remote history.
    for remote in remotes {
        replica.merge(remote)?;
    }
    let registers = replica.registers()?;
    let effective = replica.materialize_from(&current.values, &scope, &registers)?;
    let conflicts = replica.conflicts_from(&registers)?;
    Ok(SyncPlan {
        expected_local: current,
        scope,
        effective,
        conflicts,
        replica: replica.snapshot(),
    })
}

fn distinct(candidates: &[SyncCandidate]) -> usize {
    candidates
        .iter()
        .map(|candidate| &candidate.value)
        .collect::<BTreeSet<_>>()
        .len()
}
pub fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[cfg(test)]
mod tests;
