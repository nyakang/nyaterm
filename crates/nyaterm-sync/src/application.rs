//! Isolate invalid business groups without changing causal history or candidates.
use std::collections::BTreeMap;

use nyaterm_core::sync_schema::{SyncFieldId, SyncValues};

use crate::engine::SyncError;

pub struct SyncApplication {
    pub effective: SyncValues,
    pub deferred: Vec<SyncFieldId>,
}

/// The adapter supplies its pure business validator. Existing fields are atomic
/// groups; creation/deletion is atomic for the whole resource. The common valid
/// path performs one validation. Invalid batches are bisected so one bad group
/// does not require a validation for every unrelated edit.
pub fn isolate_changes(
    local: &SyncValues,
    desired: &SyncValues,
    mut valid: impl FnMut(&SyncValues) -> bool,
) -> Result<SyncApplication, SyncError> {
    if valid(desired) {
        return Ok(SyncApplication {
            effective: desired.clone(),
            deferred: Vec::new(),
        });
    }
    if !valid(local) {
        // Never attempt to repair an already unreadable local catalog by
        // guessing which saved data may be removed.
        return Err(SyncError::InvalidReplica);
    }
    let mut groups = BTreeMap::new();
    for key in local.keys().chain(desired.keys()) {
        if local.get(key) == desired.get(key) {
            continue;
        }
        let field = SyncFieldId::from_key(key).ok_or(SyncError::InvalidReplica)?;
        let presence = SyncFieldId::presence(field.resource.clone()).key();
        let whole_resource = local.contains_key(&presence) != desired.contains_key(&presence);
        let group = (field.resource, (!whole_resource).then_some(field.field));
        groups
            .entry(group)
            .or_insert_with(Vec::new)
            .push(key.clone());
    }
    let mut groups = groups.into_values().collect::<Vec<_>>();
    for group in &mut groups {
        group.sort();
        group.dedup();
    }
    let mut effective = local.clone();
    let mut pending = groups;
    loop {
        let previous = pending.len();
        let mut deferred = Vec::new();
        apply_batch(&pending, desired, &mut effective, &mut valid, &mut deferred);
        if deferred.len() == previous {
            let mut fields = deferred
                .iter()
                .flatten()
                .filter_map(|key| SyncFieldId::from_key(key))
                .collect::<Vec<_>>();
            fields.sort();
            fields.dedup();
            return Ok(SyncApplication {
                effective,
                deferred: fields,
            });
        }
        pending = deferred;
        if pending.is_empty() {
            return Ok(SyncApplication {
                effective,
                deferred: Vec::new(),
            });
        }
    }
}

fn apply_batch(
    groups: &[Vec<String>],
    desired: &SyncValues,
    effective: &mut SyncValues,
    valid: &mut impl FnMut(&SyncValues) -> bool,
    deferred: &mut Vec<Vec<String>>,
) {
    if groups.is_empty() {
        return;
    }
    let mut candidate = effective.clone();
    for key in groups.iter().flatten() {
        if let Some(value) = desired.get(key) {
            candidate.insert(key.clone(), value.clone());
        } else {
            candidate.remove(key);
        }
    }
    if valid(&candidate) {
        *effective = candidate;
    } else if groups.len() == 1 {
        deferred.push(groups[0].clone());
    } else {
        let middle = groups.len() / 2;
        apply_batch(&groups[..middle], desired, effective, valid, deferred);
        apply_batch(&groups[middle..], desired, effective, valid, deferred);
    }
}

#[cfg(test)]
mod tests;
