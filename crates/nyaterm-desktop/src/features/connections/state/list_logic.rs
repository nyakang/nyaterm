use std::collections::{BTreeSet, HashMap, HashSet};

use nyaterm_core::connection_search::ConnectionSearch;
use nyaterm_core::{Group, SavedConnection, natural_compare};

use crate::features::{
    connections::ConnectionDragKind, connections::ConnectionDropPosition,
    connections::ConnectionDropTarget,
};
use crate::models::ConnectionSortMode;

pub(super) fn selected_connections_for_list_state(
    connections: &[SavedConnection],
    selected_ids: &HashSet<String>,
) -> Vec<SavedConnection> {
    let loaded_ids = connections
        .iter()
        .map(|connection| connection.id.as_str())
        .collect::<HashSet<_>>();
    connections
        .iter()
        .filter(|connection| selected_ids.contains(&connection.id))
        .cloned()
        .chain(
            selected_ids
                .iter()
                .map(String::as_str)
                .filter(|id| !loaded_ids.contains(*id))
                .filter_map(|id| {
                    connections
                        .iter()
                        .find(|connection| connection.id == id)
                        .cloned()
                }),
        )
        .collect()
}

pub(super) fn saved_connections_in_group_tree_for_list_state(
    connections: &[SavedConnection],
    groups: &[Group],
    group_id: &str,
) -> Vec<SavedConnection> {
    let group_ids = group_tree_ids(groups, group_id);
    connections
        .iter()
        .filter(|connection| {
            connection
                .group_id
                .as_ref()
                .is_some_and(|id| group_ids.contains(id))
        })
        .cloned()
        .collect()
}

pub(super) fn visible_connection_ids_for_list_state(
    connections: &[SavedConnection],
    groups: &[Group],
    query: &str,
    sort_mode: ConnectionSortMode,
    expanded_group_ids: &HashSet<String>,
) -> Vec<String> {
    // Must order exactly like the rendered tree, or Shift-range selection and
    // keyboard navigation walk a different list than the one on screen.
    let search = ConnectionSearch::new(query, groups);
    let mut by_group: HashMap<Option<String>, Vec<&SavedConnection>> = HashMap::new();
    for connection in connections {
        if !search.matches_connection(connection) {
            continue;
        }
        by_group
            .entry(connection.group_id.clone())
            .or_default()
            .push(connection);
    }
    for connections in by_group.values_mut() {
        sort_connection_refs(connections, sort_mode);
    }

    let group_ids = groups
        .iter()
        .map(|group| group.id.clone())
        .collect::<HashSet<_>>();
    let mut children_by_parent: HashMap<Option<String>, Vec<Group>> = HashMap::new();
    for group in groups {
        let parent_id = group
            .parent_id
            .clone()
            .filter(|parent_id| group_ids.contains(parent_id));
        let mut group = group.clone();
        group.parent_id = parent_id.clone();
        children_by_parent.entry(parent_id).or_default().push(group);
    }
    for groups in children_by_parent.values_mut() {
        sort_groups(groups, sort_mode);
    }

    let mut ids = Vec::new();
    let mut visited = HashSet::new();
    // Groups first, ungrouped last (matches connection_sections / Tauri).
    for group in children_by_parent.get(&None).cloned().unwrap_or_default() {
        append_visible_connection_ids(
            group,
            &children_by_parent,
            &mut by_group,
            &mut ids,
            &mut visited,
            expanded_group_ids,
        );
    }
    if let Some(root) = by_group.remove(&None) {
        ids.extend(root.into_iter().map(|connection| connection.id.clone()));
    }
    ids
}

fn group_tree_ids(groups: &[Group], group_id: &str) -> HashSet<String> {
    let mut group_ids = HashSet::from([group_id.to_string()]);
    let mut changed = true;
    while changed {
        changed = false;
        for group in groups {
            if let Some(parent) = group.parent_id.as_ref()
                && group_ids.contains(parent)
                && group_ids.insert(group.id.clone())
            {
                changed = true;
            }
        }
    }
    group_ids
}

fn sort_connection_refs(connections: &mut [&SavedConnection], mode: ConnectionSortMode) {
    connections.sort_by(|left, right| match mode {
        ConnectionSortMode::Default => left
            .sort_order
            .cmp(&right.sort_order)
            .then_with(|| natural_compare(&left.name, &right.name)),
        ConnectionSortMode::NameAsc => natural_compare(&left.name, &right.name),
        ConnectionSortMode::NameDesc => natural_compare(&right.name, &left.name),
    });
}

fn sort_groups(groups: &mut [Group], mode: ConnectionSortMode) {
    groups.sort_by(|left, right| match mode {
        ConnectionSortMode::Default => left
            .sort_order
            .cmp(&right.sort_order)
            .then_with(|| natural_compare(&left.name, &right.name)),
        ConnectionSortMode::NameAsc => natural_compare(&left.name, &right.name),
        ConnectionSortMode::NameDesc => natural_compare(&right.name, &left.name),
    });
}

fn append_visible_connection_ids(
    group: Group,
    children_by_parent: &HashMap<Option<String>, Vec<Group>>,
    by_group: &mut HashMap<Option<String>, Vec<&SavedConnection>>,
    ids: &mut Vec<String>,
    visited: &mut HashSet<String>,
    expanded_group_ids: &HashSet<String>,
) {
    if !visited.insert(group.id.clone()) {
        return;
    }
    // A collapsed folder's rows are not on screen, so they are not reachable by
    // Shift-range or by the arrow keys either.
    if !expanded_group_ids.contains(&group.id) {
        by_group.remove(&Some(group.id));
        return;
    }
    for child in children_by_parent
        .get(&Some(group.id.clone()))
        .cloned()
        .unwrap_or_default()
    {
        append_visible_connection_ids(
            child,
            children_by_parent,
            by_group,
            ids,
            visited,
            expanded_group_ids,
        );
    }
    if let Some(connections) = by_group.remove(&Some(group.id)) {
        ids.extend(
            connections
                .into_iter()
                .map(|connection| connection.id.clone()),
        );
    }
}

pub(super) fn remove_connection_list_references(
    selected_ids: &mut HashSet<String>,
    last_selected_id: &mut Option<String>,
    drop_target: &mut Option<ConnectionDropTarget>,
    connection_id: &str,
) {
    selected_ids.remove(connection_id);
    if last_selected_id.as_deref() == Some(connection_id) {
        *last_selected_id = None;
    }
    if drop_target.as_ref().is_some_and(|target| {
        target.kind == ConnectionDragKind::Connection && target.id.as_deref() == Some(connection_id)
    }) {
        *drop_target = None;
    }
}

pub(super) fn remove_group_list_references(
    expanded_group_ids: &mut HashSet<String>,
    hovered_group_id: &mut Option<String>,
    drop_target: &mut Option<ConnectionDropTarget>,
    group_id: &str,
) {
    expanded_group_ids.remove(group_id);
    if hovered_group_id.as_deref() == Some(group_id) {
        *hovered_group_id = None;
    }
    if drop_target.as_ref().is_some_and(|target| {
        target.kind == ConnectionDragKind::Group && target.id.as_deref() == Some(group_id)
    }) {
        *drop_target = None;
    }
}

pub(super) fn retain_loaded_connection_references(
    selected_ids: &mut HashSet<String>,
    last_selected_id: &mut Option<String>,
    drop_target: &mut Option<ConnectionDropTarget>,
    connection_ids: &HashSet<String>,
) {
    selected_ids.retain(|id| connection_ids.contains(id));
    if last_selected_id
        .as_ref()
        .is_some_and(|id| !connection_ids.contains(id))
    {
        *last_selected_id = None;
    }
    if drop_target.as_ref().is_some_and(|target| {
        target.kind == ConnectionDragKind::Connection
            && target
                .id
                .as_ref()
                .is_some_and(|id| !connection_ids.contains(id))
    }) {
        *drop_target = None;
    }
}

pub(super) fn retain_loaded_group_list_references(
    expanded_group_ids: &mut HashSet<String>,
    hovered_group_id: &mut Option<String>,
    drop_target: &mut Option<ConnectionDropTarget>,
    group_ids: &HashSet<String>,
) {
    expanded_group_ids.retain(|id| group_ids.contains(id));
    if hovered_group_id
        .as_ref()
        .is_some_and(|id| !group_ids.contains(id))
    {
        *hovered_group_id = None;
    }
    if drop_target.as_ref().is_some_and(|target| {
        target.kind == ConnectionDragKind::Group
            && target.id.as_ref().is_some_and(|id| !group_ids.contains(id))
    }) {
        *drop_target = None;
    }
}

pub(super) fn clear_selected_connection_ids(
    selected_ids: &mut HashSet<String>,
    last_selected_id: &mut Option<String>,
) {
    selected_ids.clear();
    *last_selected_id = None;
}

pub(super) fn cycle_connection_sort_mode(sort_mode: &mut ConnectionSortMode) -> ConnectionSortMode {
    *sort_mode = sort_mode.next();
    *sort_mode
}

pub(super) fn set_connection_group_hover(
    hovered_group_id: &mut Option<String>,
    group_id: String,
    hovered: bool,
) -> bool {
    if hovered {
        if hovered_group_id.as_deref() == Some(group_id.as_str()) {
            return false;
        }
        *hovered_group_id = Some(group_id);
        return true;
    }
    if hovered_group_id.as_deref() == Some(group_id.as_str()) {
        *hovered_group_id = None;
        return true;
    }
    false
}

pub(super) fn set_connection_drop_target_if_changed(
    drop_target: &mut Option<ConnectionDropTarget>,
    target: ConnectionDropTarget,
) -> bool {
    if drop_target.as_ref() == Some(&target) {
        return false;
    }
    *drop_target = Some(target);
    true
}

pub(super) fn connection_drop_position_for_target(
    drop_target: &Option<ConnectionDropTarget>,
    target_id: &str,
    fallback: ConnectionDropPosition,
) -> ConnectionDropPosition {
    drop_target
        .as_ref()
        .filter(|target| target.id.as_deref() == Some(target_id))
        .map(|target| target.position)
        .unwrap_or(fallback)
}

pub(super) fn clear_connection_list_runtime_state(
    selected_ids: &mut HashSet<String>,
    last_selected_id: &mut Option<String>,
    expanded_group_ids: &mut HashSet<String>,
    drop_target: &mut Option<ConnectionDropTarget>,
    hovered_group_id: &mut Option<String>,
) {
    clear_selected_connection_ids(selected_ids, last_selected_id);
    expanded_group_ids.clear();
    *drop_target = None;
    *hovered_group_id = None;
}

pub(super) fn select_connection_ids(
    selected_ids: &mut HashSet<String>,
    last_selected_id: &mut Option<String>,
    connection_id: String,
    visible_ids: &[String],
    additive: bool,
    range: bool,
) -> usize {
    if range {
        let anchor = last_selected_id
            .clone()
            .unwrap_or_else(|| connection_id.clone());
        let mut next = if additive {
            selected_ids.clone()
        } else {
            HashSet::new()
        };
        if let (Some(start), Some(end)) = (
            visible_ids.iter().position(|id| id == &anchor),
            visible_ids.iter().position(|id| id == &connection_id),
        ) {
            let (lo, hi) = if start <= end {
                (start, end)
            } else {
                (end, start)
            };
            for id in &visible_ids[lo..=hi] {
                next.insert(id.clone());
            }
        } else {
            next.insert(connection_id.clone());
        }
        *selected_ids = next;
    } else if additive {
        if selected_ids.contains(&connection_id) {
            selected_ids.remove(&connection_id);
        } else {
            selected_ids.insert(connection_id.clone());
        }
    } else {
        selected_ids.clear();
        selected_ids.insert(connection_id.clone());
    }
    *last_selected_id = Some(connection_id);
    selected_ids.len()
}

/// Keep the expanded set in step with the filter box.
///
/// Groups start collapsed, so an unexpanded tree would hide every hit. While a
/// filter is active the groups that still have matches are opened; clearing the
/// filter puts the tree back the way the user left it. `applied_query` makes the
/// auto-expand one-shot per keyword, so collapsing an auto-opened group during a
/// search sticks instead of springing back on the next keystroke.
/// What the auto-expand has already been applied for.
///
/// The query alone is not enough. Which groups match a *fixed* query changes
/// whenever the catalog does -- a store reload, a drag into a folder, a rename --
/// so keying the guard on the query meant those newly matching groups never
/// expanded and their matches stayed hidden behind a collapsed folder.
///
/// One consequence worth naming: a folder the user collapsed by hand while the
/// query stood still re-expands once the matching set moves. That is the same
/// thing an edit to the query has always done, and it beats hiding a match.
#[derive(Clone, PartialEq, Eq)]
pub(super) struct AppliedSearchExpansion {
    query: String,
    matching: BTreeSet<String>,
}

pub(super) fn sync_connection_search_expansion(
    expanded_group_ids: &mut HashSet<String>,
    search_expanded_base: &mut Option<HashSet<String>>,
    applied: &mut Option<AppliedSearchExpansion>,
    query: &str,
    matching_group_ids: impl IntoIterator<Item = String>,
) -> bool {
    if query.is_empty() {
        *applied = None;
        let Some(base) = search_expanded_base.take() else {
            return false;
        };
        if *expanded_group_ids == base {
            return false;
        }
        *expanded_group_ids = base;
        return true;
    }

    if search_expanded_base.is_none() {
        *search_expanded_base = Some(expanded_group_ids.clone());
    }
    let next = AppliedSearchExpansion {
        query: query.to_string(),
        matching: matching_group_ids.into_iter().collect(),
    };
    if applied.as_ref() == Some(&next) {
        return false;
    }

    let mut changed = false;
    for group_id in &next.matching {
        changed |= expanded_group_ids.insert(group_id.clone());
    }
    *applied = Some(next);
    changed
}
