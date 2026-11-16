//! Mixed saved-connection ordering and validated, schema-neutral tree moves.
use std::collections::{HashMap, HashSet};

use crate::models::{Group, SavedConnection};
use crate::natural_compare;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum NodeKind {
    Group,
    Connection,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId {
    pub kind: NodeKind,
    pub id: String,
}

/// Only placement metadata crosses the drag/store boundary, never credentials.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    pub key: NodeId,
    pub parent: Option<String>,
    pub name: String,
    pub sort_order: i32,
    pub tree_order: Option<i64>,
}

impl Node {
    pub fn group(group: &Group) -> Self {
        Self {
            key: NodeId {
                kind: NodeKind::Group,
                id: group.id.clone(),
            },
            parent: group.parent_id.clone(),
            name: group.name.clone(),
            sort_order: group.sort_order,
            tree_order: group.tree_order,
        }
    }

    pub fn connection(connection: &SavedConnection) -> Self {
        Self {
            key: NodeId {
                kind: NodeKind::Connection,
                id: connection.id.clone(),
            },
            parent: connection.group_id.clone(),
            name: connection.name.clone(),
            sort_order: connection.sort_order,
            tree_order: connection.tree_order,
        }
    }
}

pub fn catalog(groups: &[Group], connections: &[SavedConnection]) -> Vec<Node> {
    groups
        .iter()
        .map(Node::group)
        .chain(connections.iter().map(Node::connection))
        .collect()
}

/// Missing ranks preserve released folder-first ordering, including partial imports.
pub fn compare(left: &Node, right: &Node) -> std::cmp::Ordering {
    match (left.tree_order, right.tree_order) {
        (Some(a), Some(b)) => a.cmp(&b).then_with(|| left.key.cmp(&right.key)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => left
            .key
            .kind
            .cmp(&right.key.kind)
            .then_with(|| left.sort_order.cmp(&right.sort_order))
            .then_with(|| natural_compare(&left.name, &right.name)),
    }
}

pub fn children<'a>(nodes: &'a [Node], parent: Option<&str>) -> Vec<&'a Node> {
    let mut result: Vec<_> = nodes
        .iter()
        .filter(|n| n.parent.as_deref() == parent)
        .collect();
    result.sort_by(|a, b| compare(a, b));
    result
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Destination {
    pub parent: Option<String>,
    /// A sibling of either type; None appends to the destination's children.
    pub before: Option<NodeId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MoveRequest {
    pub source: NodeId,
    pub destination: Destination,
    /// Optimistic placement precondition; unrelated secret edits do not conflict.
    pub expected: Vec<Node>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveError {
    MissingSource,
    InvalidParent,
    Cycle,
    InvalidAnchor,
    Stale,
    Duplicate,
}

#[derive(Clone, Debug)]
pub struct VisibleNode {
    pub key: NodeId,
    pub depth: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DropProjection {
    pub destination: Destination,
    pub index: usize,
    pub depth: usize,
    pub inside: bool,
}

/// Coordinates are in logical row units, including the current preview slot.
/// Animation never participates in hit testing.
pub fn project_drop(
    nodes: &[Node],
    rows: &[VisibleNode],
    y: f32,
    desired_depth: usize,
    previous: Option<&DropProjection>,
    reorder: bool,
) -> Option<DropProjection> {
    if !y.is_finite() || y < 0.0 {
        return None;
    }
    let top = |index: usize| {
        (index + usize::from(reorder && previous.is_some_and(|p| index >= p.index))) as f32
    };
    for (index, row) in rows.iter().enumerate() {
        if row.key.kind == NodeKind::Group && y >= top(index) + 0.25 && y < top(index) + 0.75 {
            let end = rows[index + 1..]
                .iter()
                .position(|r| r.depth <= row.depth)
                .map_or(rows.len(), |n| index + 1 + n);
            return Some(DropProjection {
                destination: Destination {
                    parent: Some(row.key.id.clone()),
                    before: None,
                },
                index: end,
                depth: row.depth + 1,
                inside: true,
            });
        }
    }
    if !reorder {
        return None;
    }
    if let Some(previous) = previous
        && y >= previous.index as f32
        && y < previous.index as f32 + 1.0
        && desired_depth == previous.depth
    {
        return Some(previous.clone());
    }
    let mut index = rows
        .iter()
        .enumerate()
        .filter(|(i, _)| y >= top(*i) + 0.5)
        .count();
    let mut edge_depth = None;
    for (i, row) in rows.iter().enumerate() {
        if row.key.kind == NodeKind::Group && y >= top(i) + 0.75 && y < top(i) + 1.0 {
            index = rows[i + 1..]
                .iter()
                .position(|r| r.depth <= row.depth)
                .map_or(rows.len(), |n| i + 1 + n);
            edge_depth = Some(row.depth);
        } else if row.key.kind == NodeKind::Group && y >= top(i) && y < top(i) + 0.25 {
            edge_depth = Some(row.depth);
        }
    }
    let prev = index.checked_sub(1).and_then(|i| rows.get(i));
    let max = prev.map_or(0, |r| r.depth + usize::from(r.key.kind == NodeKind::Group));
    let min = rows.get(index).map_or(0, |r| r.depth).min(max);
    let depth = edge_depth.unwrap_or(desired_depth).clamp(min, max);
    let parent = if depth == 0 {
        None
    } else {
        Some(
            rows[..index]
                .iter()
                .rev()
                .find(|r| r.depth + 1 == depth && r.key.kind == NodeKind::Group)?
                .key
                .id
                .clone(),
        )
    };
    let before = rows
        .get(index)
        .and_then(|r| nodes.iter().find(|n| n.key == r.key))
        .filter(|n| n.parent == parent)
        .map(|n| n.key.clone());
    Some(DropProjection {
        destination: Destination { parent, before },
        index,
        depth,
        inside: false,
    })
}

/// Validate first, then return only changed placement records. No-op moves do not migrate.
pub fn plan_move(nodes: &[Node], request: &MoveRequest) -> Result<Vec<Node>, MoveError> {
    let mut keys = HashSet::new();
    if nodes.iter().any(|n| !keys.insert(n.key.clone())) {
        return Err(MoveError::Duplicate);
    }
    let by_key: HashMap<_, _> = nodes.iter().map(|node| (&node.key, node)).collect();
    if nodes.len() != request.expected.len()
        || request
            .expected
            .iter()
            .any(|n| by_key.get(&n.key).copied() != Some(n))
    {
        return Err(MoveError::Stale);
    }
    let source = nodes
        .iter()
        .find(|n| n.key == request.source)
        .ok_or(MoveError::MissingSource)?;
    let mut ancestor = request.destination.parent.as_deref();
    let mut visited = HashSet::new();
    while let Some(id) = ancestor {
        if !visited.insert(id) || (source.key.kind == NodeKind::Group && source.key.id == id) {
            return Err(MoveError::Cycle);
        }
        let group = nodes
            .iter()
            .find(|n| n.key.kind == NodeKind::Group && n.key.id == id)
            .ok_or(MoveError::InvalidParent)?;
        ancestor = group.parent.as_deref();
    }
    let old = children(nodes, source.parent.as_deref());
    let mut target: Vec<Node> = children(nodes, request.destination.parent.as_deref())
        .into_iter()
        .filter(|n| n.key != source.key)
        .cloned()
        .collect();
    let index = match &request.destination.before {
        Some(key) => target
            .iter()
            .position(|n| &n.key == key)
            .ok_or(MoveError::InvalidAnchor)?,
        None => target.len(),
    };
    let mut moved = source.clone();
    moved.parent = request.destination.parent.clone();
    target.insert(index, moved);
    if source.parent == request.destination.parent
        && old.iter().map(|n| &n.key).eq(target.iter().map(|n| &n.key))
    {
        return Ok(Vec::new());
    }
    let mut batches = vec![target];
    if source.parent != request.destination.parent {
        batches.push(
            old.into_iter()
                .filter(|n| n.key != source.key)
                .cloned()
                .collect(),
        );
    }
    let mut changes = Vec::new();
    for batch in batches {
        let mut groups = 0;
        let mut connections = 0;
        for (index, mut node) in batch.into_iter().enumerate() {
            node.tree_order = Some(index as i64);
            let order = if node.key.kind == NodeKind::Group {
                &mut groups
            } else {
                &mut connections
            };
            node.sort_order = *order;
            *order += 1;
            if by_key.get(&node.key).copied() != Some(&node) {
                changes.push(node);
            }
        }
    }
    Ok(changes)
}

#[cfg(test)]
mod tests {
    use super::{
        Destination, MoveError, MoveRequest, Node, NodeId, NodeKind, VisibleNode, children,
        plan_move, project_drop,
    };

    fn node(id: &str, kind: NodeKind, parent: Option<&str>, order: i32) -> Node {
        Node {
            key: NodeId {
                kind,
                id: id.into(),
            },
            parent: parent.map(str::to_owned),
            name: id.into(),
            sort_order: order,
            tree_order: None,
        }
    }

    #[test]
    fn legacy_order_is_preserved_until_a_real_mixed_move() {
        let nodes = vec![
            node("c", NodeKind::Connection, None, 0),
            node("g", NodeKind::Group, None, 8),
        ];
        assert_eq!(children(&nodes, None)[0].key.id, "g");
        let mut request = MoveRequest {
            source: nodes[0].key.clone(),
            destination: Destination {
                parent: None,
                before: None,
            },
            expected: nodes.clone(),
        };
        assert!(plan_move(&nodes, &request).unwrap().is_empty());
        request.destination.before = Some(nodes[1].key.clone());
        let result = plan_move(&nodes, &request).unwrap();
        assert_eq!(result[0].key.id, "c");
        assert_eq!(result[0].tree_order, Some(0));
        assert_eq!(result[1].tree_order, Some(1));
    }

    #[test]
    fn descendants_and_stale_catalogs_are_rejected_before_changes() {
        let nodes = vec![
            node("a", NodeKind::Group, None, 0),
            node("b", NodeKind::Group, Some("a"), 0),
        ];
        let mut request = MoveRequest {
            source: nodes[0].key.clone(),
            destination: Destination {
                parent: Some("b".into()),
                before: None,
            },
            expected: nodes.clone(),
        };
        assert_eq!(plan_move(&nodes, &request), Err(MoveError::Cycle));
        request.expected.pop();
        assert_eq!(plan_move(&nodes, &request), Err(MoveError::Stale));
    }

    #[test]
    fn cross_parent_move_renumbers_both_parents_and_preserves_legacy_type_order() {
        let nodes = vec![
            node("g", NodeKind::Group, None, 0),
            node("a", NodeKind::Connection, None, 2),
            node("b", NodeKind::Connection, Some("g"), 4),
        ];
        let request = MoveRequest {
            source: nodes[1].key.clone(),
            destination: Destination {
                parent: Some("g".into()),
                before: Some(nodes[2].key.clone()),
            },
            expected: nodes.clone(),
        };
        let result = plan_move(&nodes, &request).unwrap();
        assert_eq!(result[0].parent.as_deref(), Some("g"));
        assert_eq!(result[1].sort_order, 1);
        assert_eq!(result[2].key.id, "g");
    }

    #[test]
    fn group_after_skips_its_entire_visible_subtree() {
        let nodes = vec![
            node("g", NodeKind::Group, None, 0),
            node("a", NodeKind::Connection, Some("g"), 0),
            node("b", NodeKind::Connection, None, 1),
        ];
        let rows: Vec<_> = nodes
            .iter()
            .map(|n| VisibleNode {
                key: n.key.clone(),
                depth: usize::from(n.parent.is_some()),
            })
            .collect();
        let p = project_drop(&nodes, &rows, 0.9, 0, None, true).unwrap();
        assert_eq!(p.index, 2);
        assert_eq!(p.destination.parent, None);
        assert_eq!(p.destination.before, Some(nodes[2].key.clone()));
        let p = project_drop(&nodes, &rows, 0.5, 0, None, true).unwrap();
        assert_eq!(p.destination.parent.as_deref(), Some("g"));
        assert_eq!(p.destination.before, None);
        assert!(p.inside);
    }

    #[test]
    fn connection_halves_and_horizontal_depth_are_independent() {
        let nodes = vec![
            node("g", NodeKind::Group, None, 0),
            node("a", NodeKind::Connection, Some("g"), 0),
            node("b", NodeKind::Connection, None, 1),
        ];
        let rows: Vec<_> = nodes
            .iter()
            .map(|n| VisibleNode {
                key: n.key.clone(),
                depth: usize::from(n.parent.is_some()),
            })
            .collect();
        assert_eq!(
            project_drop(&nodes, &rows, 1.49, 1, None, true)
                .unwrap()
                .destination
                .before,
            Some(nodes[1].key.clone())
        );
        let inner = project_drop(&nodes, &rows, 1.51, 1, None, true).unwrap();
        assert_eq!(inner.destination.parent.as_deref(), Some("g"));
        assert_eq!(inner.destination.before, None);
        let outer = project_drop(&nodes, &rows, 1.51, 0, None, true).unwrap();
        assert_eq!(outer.destination.parent, None);
        assert_eq!(outer.destination.before, Some(nodes[2].key.clone()));
        assert!(project_drop(&nodes, &rows, 1.51, 0, None, false).is_none());
        assert!(
            project_drop(&nodes, &rows, 0.5, 0, None, false)
                .unwrap()
                .inside
        );
    }

    #[test]
    fn partial_and_duplicate_ranks_have_deterministic_fallbacks() {
        let mut nodes = vec![
            node("g", NodeKind::Group, None, 0),
            node("b", NodeKind::Connection, None, 1),
            node("a", NodeKind::Connection, None, 2),
        ];
        nodes[1].tree_order = Some(0);
        nodes[2].tree_order = Some(0);
        assert_eq!(
            children(&nodes, None)
                .iter()
                .map(|n| n.key.id.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "b", "g"]
        );
    }

    #[test]
    fn legacy_and_ranked_records_round_trip_through_sync_without_losing_extensions() {
        use crate::models::{Group, SavedConnection};
        use crate::sync_schema::{apply_record, project_record};
        let group = serde_json::json!({"id":"g","name":"Group","sort_order":4});
        let parsed: Group = serde_json::from_value(group.clone()).unwrap();
        assert_eq!(parsed.tree_order, None);
        assert!(
            serde_json::to_value(parsed)
                .unwrap()
                .get("tree_order")
                .is_none()
        );
        let connection = serde_json::json!({"id":"c","name":"Shell","type":"local_terminal","shell_path":"pwsh.exe","tree_order":3,"future":{"keep":true}});
        let parsed: SavedConnection = serde_json::from_value(connection.clone()).unwrap();
        assert_eq!(parsed.tree_order, Some(3));
        assert_eq!(
            serde_json::to_value(parsed).unwrap()["future"],
            connection["future"]
        );
        let projected = project_record("connection", &connection).unwrap();
        assert_eq!(projected["placement"]["tree_order"], 3);
        assert_eq!(
            apply_record("connection", &connection, &projected).unwrap()["tree_order"],
            3
        );
        let group = serde_json::json!({"id":"g","name":"Group","tree_order":1});
        let projected = project_record("connection_group", &group).unwrap();
        assert_eq!(
            apply_record("connection_group", &group, &projected).unwrap()["tree_order"],
            1
        );
    }
}
