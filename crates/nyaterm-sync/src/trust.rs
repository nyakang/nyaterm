//! Approval tokens bind the exact local and proposed host identity. They are
//! transient local decisions, never part of a replica or portable backup.
use std::collections::{BTreeMap, BTreeSet};

use nyaterm_core::sync_schema::{SyncFieldId, SyncResourceId, SyncValues, trust::KINDS};
use serde_json::{Value, json};

use crate::engine::digest;

#[derive(Clone, PartialEq)]
pub struct TrustReview {
    pub resource: SyncResourceId,
    pub key: String,
    pub token: String,
    pub previous: Value,
    pub proposed: Value,
}

fn resource_values(values: &SyncValues) -> BTreeMap<SyncResourceId, Value> {
    let mut records = BTreeMap::new();
    for (key, value) in values {
        if let Some(field) = SyncFieldId::from_key(key)
            && KINDS.contains(&field.resource.kind.as_str())
            && field.field == "identity"
            && let Ok(value) = value.decode()
        {
            records.insert(field.resource, json!({"identity":value}));
        }
    }
    records
}

pub fn reviews(local: &SyncValues, desired: &SyncValues) -> Vec<TrustReview> {
    let local = resource_values(local);
    let desired = resource_values(desired);
    let resources = local
        .keys()
        .chain(desired.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    resources
        .into_iter()
        .filter_map(|resource| {
            let previous = local.get(&resource).cloned().unwrap_or(Value::Null);
            let proposed = desired.get(&resource).cloned().unwrap_or(Value::Null);
            if previous == proposed {
                return None;
            }
            let key = SyncFieldId::presence(resource.clone()).key();
            let token = digest(
                json!([&resource, &previous, &proposed])
                    .to_string()
                    .as_bytes(),
            );
            Some(TrustReview {
                resource,
                key,
                token,
                previous,
                proposed,
            })
        })
        .collect()
}

pub fn approved(
    local: &SyncValues,
    desired: &SyncValues,
    approvals: &BTreeMap<String, String>,
) -> bool {
    reviews(local, desired)
        .iter()
        .all(|review| approvals.get(&review.key) == Some(&review.token))
}

#[cfg(test)]
mod tests {
    use crate::trust::{approved, reviews};
    use nyaterm_core::sync_schema::SyncProjection;
    use serde_json::json;
    use std::collections::BTreeMap;

    #[test]
    fn approval_observes_both_identities_and_cannot_authorize_a_later_change() {
        let local = SyncProjection::default();
        let mut next = SyncProjection::default();
        next.record(
            "vnc_known_host",
            "host",
            &json!({"identity":{"host":"example","port":5900,"sha256_fingerprint":"first"}}),
        )
        .unwrap();
        let review = reviews(&local.values, &next.values).remove(0);
        let approvals = BTreeMap::from([(review.key, review.token)]);
        assert!(approved(&local.values, &next.values, &approvals));
        let before = next.clone();
        next.values.clear();
        next.record(
            "vnc_known_host",
            "host",
            &json!({"identity":{"host":"example","port":5900,"sha256_fingerprint":"second"}}),
        )
        .unwrap();
        assert!(!approved(&local.values, &next.values, &approvals));
        assert!(!approved(&before.values, &next.values, &approvals));
    }
}
