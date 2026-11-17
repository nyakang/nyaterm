//! Startup preferences and deterministic, credential-free target expansion.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::connection_tree::{Node, NodeKind, children};
use crate::models::connection::ConnectionType;

pub fn supports_automatic_start(config: &ConnectionType) -> bool {
    match config {
        ConnectionType::Rdp { backend, .. } => backend != "mstsc",
        ConnectionType::Serial { connect_prompt, .. } => !connect_prompt.asks_anything(),
        _ => true,
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "Value", into = "Value")]
pub enum StartupMode {
    #[default]
    None,
    LastSession,
    Specified,
    Unknown(Value),
}

impl StartupMode {
    pub fn restores_last_session(&self) -> bool {
        matches!(self, Self::LastSession)
    }
}

impl From<Value> for StartupMode {
    fn from(value: Value) -> Self {
        match value.as_str() {
            Some("none") => Self::None,
            Some("last_session") => Self::LastSession,
            Some("specified") => Self::Specified,
            _ => match value {
                Value::Bool(true) => Self::LastSession,
                Value::Bool(false) => Self::None,
                other => Self::Unknown(other),
            },
        }
    }
}

impl From<StartupMode> for Value {
    fn from(mode: StartupMode) -> Self {
        match mode {
            StartupMode::None => "none".into(),
            StartupMode::LastSession => "last_session".into(),
            StartupMode::Specified => "specified".into(),
            StartupMode::Unknown(value) => value,
        }
    }
}

/// Retains unknown kinds and additional fields through settings/backup round trips.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StartupTarget(Value);

impl StartupTarget {
    pub fn connection(id: impl Into<String>) -> Self {
        Self(serde_json::json!({"kind": "connection", "id": id.into()}))
    }

    pub fn group(id: impl Into<String>) -> Self {
        Self(serde_json::json!({"kind": "group", "id": id.into()}))
    }

    pub fn reference(&self) -> Option<(NodeKind, &str)> {
        let kind = match self.0.get("kind")?.as_str()? {
            "connection" => NodeKind::Connection,
            "group" => NodeKind::Group,
            _ => return None,
        };
        let id = self.0.get("id")?.as_str()?;
        (!id.is_empty()).then_some((kind, id))
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct StartupPlan {
    pub connection_ids: Vec<String>,
    /// Target indexes, not names or user-provided payloads.
    pub unavailable_targets: Vec<usize>,
}

/// Expand each group depth-first in persisted tree order, independent of UI filters.
/// The visited sets also bound malformed cyclic catalogs and overlapping groups.
pub fn plan_startup(targets: &[StartupTarget], nodes: &[Node]) -> StartupPlan {
    let mut plan = StartupPlan::default();
    let mut seen_connections = HashSet::new();
    for (index, target) in targets.iter().enumerate() {
        let Some((kind, id)) = target.reference() else {
            plan.unavailable_targets.push(index);
            continue;
        };
        let Some(node) = nodes
            .iter()
            .find(|node| node.key.kind == kind && node.key.id == id)
        else {
            plan.unavailable_targets.push(index);
            continue;
        };
        let mut stack = vec![node];
        let mut visited_groups = HashSet::new();
        let mut found_connection = false;
        while let Some(node) = stack.pop() {
            match node.key.kind {
                NodeKind::Connection => {
                    found_connection = true;
                    if seen_connections.insert(node.key.id.clone()) {
                        plan.connection_ids.push(node.key.id.clone());
                    }
                }
                NodeKind::Group if visited_groups.insert(node.key.id.as_str()) => {
                    stack.extend(children(nodes, Some(&node.key.id)).into_iter().rev());
                }
                NodeKind::Group => {}
            }
        }
        if !found_connection {
            plan.unavailable_targets.push(index);
        }
    }
    plan
}

#[cfg(test)]
mod tests {
    use super::{StartupMode, StartupTarget, plan_startup};
    use crate::connection_tree::{Node, NodeId, NodeKind};

    fn node(kind: NodeKind, id: &str, parent: Option<&str>, order: i64) -> Node {
        Node {
            key: NodeId {
                kind,
                id: id.into(),
            },
            parent: parent.map(str::to_owned),
            name: id.into(),
            sort_order: 0,
            tree_order: Some(order),
        }
    }

    #[test]
    fn groups_follow_tree_order_and_overlaps_keep_first_occurrence() {
        let nodes = vec![
            node(NodeKind::Group, "g", None, 0),
            node(NodeKind::Connection, "b", Some("g"), 2),
            node(NodeKind::Group, "nested", Some("g"), 1),
            node(NodeKind::Connection, "a", Some("nested"), 0),
        ];
        let plan = plan_startup(
            &[
                StartupTarget::connection("b"),
                StartupTarget::group("g"),
                StartupTarget::group("nested"),
            ],
            &nodes,
        );
        assert_eq!(plan.connection_ids, ["b", "a"]);
        assert!(plan.unavailable_targets.is_empty());
    }

    #[test]
    fn missing_empty_unknown_and_cyclic_groups_terminate() {
        let nodes = vec![
            node(NodeKind::Group, "a", Some("b"), 0),
            node(NodeKind::Group, "b", Some("a"), 0),
        ];
        let unknown =
            serde_json::from_value(serde_json::json!({"kind":"future", "data":42})).unwrap();
        let plan = plan_startup(
            &[
                StartupTarget::group("a"),
                StartupTarget::connection("gone"),
                unknown,
            ],
            &nodes,
        );
        assert!(plan.connection_ids.is_empty());
        assert_eq!(plan.unavailable_targets, [0, 1, 2]);
    }

    #[test]
    fn future_fields_and_legacy_flags_roundtrip() {
        for value in [
            serde_json::json!({"kind":"connection","id":"a","future":true}),
            serde_json::json!({"kind":"future","payload":[1,2]}),
        ] {
            let target: StartupTarget = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(serde_json::to_value(target).unwrap(), value);
        }
        assert_eq!(
            StartupMode::from(serde_json::json!(true)),
            StartupMode::LastSession
        );
        assert_eq!(
            StartupMode::from(serde_json::json!(false)),
            StartupMode::None
        );
        let mode = StartupMode::from(serde_json::json!("future"));
        assert!(!mode.restores_last_session());
        assert_eq!(serde_json::to_value(mode).unwrap(), "future");
    }
}
