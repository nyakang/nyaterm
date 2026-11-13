//! Derived saved-connection tree shared by presentation and navigation.
use std::collections::{HashMap, HashSet};

use nyaterm_core::connection_search::{ConnectionSearch, ConnectionSearchRank};
use nyaterm_core::{Group, SavedConnection, natural_compare};

use crate::models::{ConnectionGroupEditorMode, ConnectionGroupEditorState, ConnectionSortMode};

#[derive(Clone)]
pub(in crate::features) enum ConnectionListRow {
    Separator,
    GroupHeader(ConnectionSectionHeader),
    InlineGroupEditor {
        parent_id: Option<String>,
        depth: usize,
    },
    EmptyGroup {
        depth: usize,
    },
    Connection {
        connection_id: String,
        depth: usize,
    },
}

pub(in crate::features) fn flatten_connection_rows(
    sections: &[ConnectionSection],
    expanded_groups: &HashSet<String>,
    group_editor: Option<&ConnectionGroupEditorState>,
) -> Vec<ConnectionListRow> {
    let has_groups = sections.iter().any(|section| !section.is_root);
    let mut rows = Vec::new();
    let mut children_by_parent: HashMap<Option<&str>, Vec<&ConnectionSection>> = HashMap::new();
    let mut root_connections = Vec::new();
    let create_parent_id = group_editor
        .filter(|editor| editor.mode == ConnectionGroupEditorMode::Create)
        .map(|editor| editor.parent_id.clone());
    for section in sections {
        if section.is_root {
            root_connections.extend(section.connections.iter());
            continue;
        }
        children_by_parent
            .entry(section.parent_id.as_deref())
            .or_default()
            .push(section);
    }

    for section in children_by_parent.get(&None).into_iter().flatten() {
        append_connection_section_rows(
            section,
            &children_by_parent,
            expanded_groups,
            create_parent_id.as_ref(),
            &mut rows,
        );
    }

    if create_parent_id.as_ref() == Some(&None) {
        rows.push(ConnectionListRow::InlineGroupEditor {
            parent_id: None,
            depth: 0,
        });
    }
    if has_groups && !root_connections.is_empty() {
        rows.push(ConnectionListRow::Separator);
    }
    for connection in root_connections {
        rows.push(ConnectionListRow::Connection {
            connection_id: connection.id.clone(),
            depth: 0,
        });
    }
    rows
}

fn append_connection_section_rows(
    section: &ConnectionSection,
    children_by_parent: &HashMap<Option<&str>, Vec<&ConnectionSection>>,
    expanded_groups: &HashSet<String>,
    create_parent_id: Option<&Option<String>>,
    rows: &mut Vec<ConnectionListRow>,
) {
    let children = children_by_parent
        .get(&section.group_id.as_deref())
        .map(Vec::as_slice)
        .unwrap_or_default();
    rows.push(ConnectionListRow::GroupHeader(section.header()));
    let expanded = section
        .group_id
        .as_ref()
        .map(|id| expanded_groups.contains(id))
        .unwrap_or(true);
    if !expanded {
        return;
    }

    let create_child = create_parent_id == Some(&section.group_id);
    if create_child {
        rows.push(ConnectionListRow::InlineGroupEditor {
            parent_id: section.group_id.clone(),
            depth: section.depth + 1,
        });
    }

    for child in children {
        append_connection_section_rows(
            child,
            children_by_parent,
            expanded_groups,
            create_parent_id,
            rows,
        );
    }
    if section.connections.is_empty() && children.is_empty() && !create_child {
        rows.push(ConnectionListRow::EmptyGroup {
            depth: section.depth + 1,
        });
        return;
    }
    for connection in &section.connections {
        rows.push(ConnectionListRow::Connection {
            connection_id: connection.id.clone(),
            depth: section.depth + 1,
        });
    }
}

/// What a flat group-header row needs, without the group's connections.
///
/// `ConnectionSection` carries every `SavedConnection` filed under it, and a row
/// used to embed a whole one. The flat list only ever draws the header, so those
/// vectors rode along into a row list that is cloned on every read -- a copy
/// proportional to the catalog for data no row reads. This carries the header
/// fields, plus the one thing the header needed the vector for.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::features) struct ConnectionSectionHeader {
    pub(in crate::features) group_id: Option<String>,
    pub(in crate::features) label: String,
    pub(in crate::features) is_root: bool,
    pub(in crate::features) depth: usize,
    pub(in crate::features) total_count: usize,
    pub(in crate::features) has_child_groups: bool,
    pub(in crate::features) is_empty: bool,
}

impl ConnectionSection {
    pub(in crate::features) fn header(&self) -> ConnectionSectionHeader {
        ConnectionSectionHeader {
            group_id: self.group_id.clone(),
            label: self.label.clone(),
            is_root: self.is_root,
            depth: self.depth,
            total_count: self.total_count,
            has_child_groups: self.has_child_groups,
            is_empty: self.connections.is_empty(),
        }
    }
}

#[derive(Clone)]
pub(in crate::features) struct ConnectionSection {
    pub(in crate::features) group_id: Option<String>,
    pub(in crate::features) parent_id: Option<String>,
    pub(in crate::features) label: String,
    pub(in crate::features) is_root: bool,
    pub(in crate::features) depth: usize,
    pub(in crate::features) total_count: usize,
    pub(in crate::features) has_child_groups: bool,
    pub(in crate::features) connections: Vec<SavedConnection>,
}

struct SearchRanks {
    connections: HashMap<String, ConnectionSearchRank>,
    groups: HashMap<String, ConnectionSearchRank>,
}

impl SearchRanks {
    fn new(search: &ConnectionSearch, connections: &[SavedConnection], groups: &[Group]) -> Self {
        let mut ranks = Self {
            connections: HashMap::new(),
            groups: HashMap::new(),
        };
        let mut pending = Vec::new();
        for connection in connections {
            if let Some(rank) = search.rank_connection(connection) {
                ranks.connections.insert(connection.id.clone(), rank);
                if let Some(id) = connection.group_id.as_deref() {
                    pending.push((id, rank));
                }
            }
        }
        let parents: HashMap<_, _> = groups
            .iter()
            .map(|group| (group.id.as_str(), group.parent_id.as_deref()))
            .collect();
        for group in groups {
            if let Some(rank) = search.group_rank(&group.id) {
                pending.push((group.id.as_str(), rank));
            }
        }
        // Propagate only improvements, including through cyclic legacy catalogs.
        // A larger subtree never gains an advantage merely by having more hits.
        while let Some((id, rank)) = pending.pop() {
            let Some(parent) = parents.get(id) else {
                continue;
            };
            if ranks
                .groups
                .get(id)
                .is_some_and(|previous| *previous >= rank)
            {
                continue;
            }
            ranks.groups.insert(id.to_owned(), rank);
            if let Some(parent) = parent {
                pending.push((parent, rank));
            }
        }
        ranks
    }

    fn sort_connections(&self, connections: &mut [SavedConnection], mode: ConnectionSortMode) {
        sort_connections(connections, mode);
        // Stable relevance sorting retains the chosen list ordering on ties.
        connections.sort_by_key(|connection| std::cmp::Reverse(self.connections[&connection.id]));
    }

    fn sort_groups(&self, groups: &mut [Group], mode: ConnectionSortMode) {
        sort_groups(groups, mode);
        groups.sort_by_key(|group| std::cmp::Reverse(self.groups.get(&group.id).copied()));
    }
}

pub(in crate::features) fn connection_sections(
    connections: &[SavedConnection],
    groups: &[Group],
    query: &str,
    sort_mode: ConnectionSortMode,
) -> Vec<ConnectionSection> {
    let search = ConnectionSearch::new(query, groups);
    let ranks = SearchRanks::new(&search, connections, groups);
    let mut by_group: HashMap<Option<String>, Vec<SavedConnection>> = HashMap::new();
    for connection in connections {
        if !ranks.connections.contains_key(&connection.id) {
            continue;
        }
        by_group
            .entry(connection.group_id.clone())
            .or_default()
            .push(connection.clone());
    }
    for list in by_group.values_mut() {
        ranks.sort_connections(list, sort_mode);
    }

    let mut sections = Vec::new();
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
    for children in children_by_parent.values_mut() {
        ranks.sort_groups(children, sort_mode);
    }

    let mut visited = HashSet::new();
    for group in children_by_parent.get(&None).cloned().unwrap_or_default() {
        let (mut group_sections, _) = build_connection_group_sections(
            group,
            0,
            &children_by_parent,
            &mut by_group,
            &search,
            &mut visited,
        );
        sections.append(&mut group_sections);
    }

    // Preserve catalog order for tied root/orphan rows instead of depending on
    // HashMap iteration order. Unreachable cyclic groups retain their connections.
    let mut root: Vec<_> = connections
        .iter()
        .filter(|connection| {
            by_group.contains_key(&connection.group_id)
                && ranks.connections.contains_key(&connection.id)
        })
        .cloned()
        .collect();
    ranks.sort_connections(&mut root, sort_mode);
    // Tauri: folders first, then ungrouped connections (no "Ungrouped" header).
    if !root.is_empty() || sections.is_empty() {
        let total_count = root.len();
        sections.push(ConnectionSection {
            group_id: None,
            parent_id: None,
            label: "Ungrouped".to_string(),
            is_root: true,
            depth: 0,
            total_count,
            has_child_groups: false,
            connections: root,
        });
    }
    sections
}

fn build_connection_group_sections(
    group: Group,
    depth: usize,
    children_by_parent: &HashMap<Option<String>, Vec<Group>>,
    by_group: &mut HashMap<Option<String>, Vec<SavedConnection>>,
    search: &ConnectionSearch,
    visited: &mut HashSet<String>,
) -> (Vec<ConnectionSection>, usize) {
    if !visited.insert(group.id.clone()) {
        return (Vec::new(), 0);
    }

    let direct_connections = by_group.remove(&Some(group.id.clone())).unwrap_or_default();
    let mut child_sections = Vec::new();
    let mut total_count = direct_connections.len();
    for child in children_by_parent
        .get(&Some(group.id.clone()))
        .cloned()
        .unwrap_or_default()
    {
        let (mut sections, child_count) = build_connection_group_sections(
            child,
            depth + 1,
            children_by_parent,
            by_group,
            search,
            visited,
        );
        total_count += child_count;
        child_sections.append(&mut sections);
    }

    if !search.includes_group(&group.id) && total_count == 0 && child_sections.is_empty() {
        return (Vec::new(), 0);
    }

    let has_child_groups = !child_sections.is_empty();
    let mut sections = vec![ConnectionSection {
        group_id: Some(group.id),
        parent_id: group.parent_id,
        label: group.name,
        is_root: false,
        depth,
        total_count,
        has_child_groups,
        connections: direct_connections,
    }];
    sections.append(&mut child_sections);
    (sections, total_count)
}

/// Folders obey the sort button too — sorting only the connections inside them
/// leaves the tree looking unsorted, which is what the old UI never did.
fn sort_groups(groups: &mut [Group], mode: ConnectionSortMode) {
    match mode {
        ConnectionSortMode::Default => groups.sort_by(|left, right| {
            left.sort_order
                .cmp(&right.sort_order)
                .then_with(|| natural_compare(&left.name, &right.name))
        }),
        ConnectionSortMode::NameAsc => {
            groups.sort_by(|left, right| natural_compare(&left.name, &right.name));
        }
        ConnectionSortMode::NameDesc => {
            groups.sort_by(|left, right| natural_compare(&right.name, &left.name));
        }
    }
}

fn sort_connections(connections: &mut [SavedConnection], mode: ConnectionSortMode) {
    match mode {
        ConnectionSortMode::Default => connections.sort_by(|left, right| {
            left.sort_order
                .cmp(&right.sort_order)
                .then_with(|| natural_compare(&left.name, &right.name))
        }),
        ConnectionSortMode::NameAsc => {
            connections.sort_by(|left, right| natural_compare(&left.name, &right.name));
        }
        ConnectionSortMode::NameDesc => {
            connections.sort_by(|left, right| natural_compare(&right.name, &left.name));
        }
    }
}

/// Index of the connection row that is most likely the widest.
///
/// `uniform_list` measures a single row to decide how far the list can scroll
/// sideways, so pointing it at row 0 would cap the scroll at whatever that row
/// happens to be. This picks the candidate by indent plus rendered name width -
/// an estimate, since the real width comes from the text system, but one that
/// only has to identify the right row rather than its exact size.
pub(in crate::features) fn widest_connection_row(
    rows: &[ConnectionListRow],
    connections: &[SavedConnection],
) -> Option<usize> {
    let names_by_id = connections
        .iter()
        .map(|connection| (connection.id.as_str(), connection.name.as_str()))
        .collect::<HashMap<_, _>>();
    rows.iter()
        .enumerate()
        .filter_map(|(index, row)| match row {
            ConnectionListRow::Connection {
                connection_id,
                depth,
            } => {
                let name = names_by_id.get(connection_id.as_str()).copied()?;
                let name_width: usize = name
                    .chars()
                    // CJK and other wide glyphs take about two Latin advances.
                    .map(|c| if c as u32 >= 0x1100 { 2 } else { 1 })
                    .sum();
                Some((index, *depth * 16 + name_width * 8))
            }
            ConnectionListRow::InlineGroupEditor { depth, .. } => Some((index, *depth * 16 + 128)),
            _ => None,
        })
        .max_by_key(|(_, width)| *width)
        .map(|(index, _)| index)
}

#[cfg(test)]
mod tests {
    use super::{
        ConnectionListRow, connection_sections, flatten_connection_rows, widest_connection_row,
    };
    use crate::models::{
        ConnectionGroupEditorMode, ConnectionGroupEditorState, ConnectionSortMode,
    };
    use nyaterm_core::{Group, SavedConnection};
    use std::collections::HashSet;

    #[test]
    fn group_search_includes_the_whole_subtree_and_unions_connection_hits() {
        let groups = vec![
            group("region", "Region", None, 0),
            group("prod", "Production", Some("region"), 0),
            group("child", "Services", Some("prod"), 0),
            group("empty", "Empty", Some("child"), 0),
            group("staging", "Staging", Some("region"), 1),
        ];
        let connections = vec![
            connection("direct", "Production host", Some("prod"), 0),
            connection("nested", "Database", Some("child"), 0),
            connection("other", "Staging host", Some("staging"), 0),
            connection("root-hit", "Production gateway", None, 0),
        ];
        let sections = connection_sections(
            &connections,
            &groups,
            "  PrOD  ",
            ConnectionSortMode::Default,
        );
        assert_eq!(
            sections
                .iter()
                .filter_map(|s| s.group_id.as_deref())
                .collect::<Vec<_>>(),
            vec!["region", "prod", "child", "empty"]
        );
        assert_eq!(sections[0].total_count, 2);
        assert_eq!(sections[1].total_count, 2);
        assert_eq!(sections[2].total_count, 1);
        assert_eq!(sections[3].total_count, 0);
        let expanded = groups.iter().map(|g| g.id.clone()).collect();
        let rows = flatten_connection_rows(&sections, &expanded, None);
        let ids = rows
            .iter()
            .filter_map(|row| match row {
                ConnectionListRow::Connection { connection_id, .. } => Some(connection_id.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["nested", "direct", "root-hit"]);
        assert!(
            rows.iter()
                .any(|row| matches!(row, ConnectionListRow::EmptyGroup { depth: 4 }))
        );
    }

    #[test]
    fn empty_group_search_keeps_ancestors_without_unrelated_branches() {
        let groups = vec![
            group("region", "Region", None, 0),
            group("parent", "Parent", Some("region"), 0),
            group("empty", "生产环境", Some("parent"), 0),
            group("sibling", "Sibling", Some("parent"), 1),
        ];
        let connections = vec![connection("other", "Host", Some("parent"), 0)];
        let sections =
            connection_sections(&connections, &groups, "生产", ConnectionSortMode::Default);
        assert_eq!(
            sections
                .iter()
                .filter_map(|s| s.group_id.as_deref())
                .collect::<Vec<_>>(),
            vec!["region", "parent", "empty"]
        );
        assert!(
            sections
                .iter()
                .all(|s| s.total_count == 0 && s.connections.is_empty())
        );
        assert!(sections[0].has_child_groups);
        assert!(sections[1].has_child_groups);
    }

    #[test]
    fn connection_only_search_keeps_its_path_without_including_other_connections() {
        let groups = vec![
            group("parent", "Parent", None, 0),
            group("child", "Child", Some("parent"), 0),
            group("empty", "Empty", Some("parent"), 1),
        ];
        let connections = vec![
            connection("hit", "Bastion", Some("child"), 0),
            connection("other", "Database", Some("child"), 1),
        ];
        let sections = connection_sections(
            &connections,
            &groups,
            "bastion",
            ConnectionSortMode::Default,
        );
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].group_id.as_deref(), Some("parent"));
        assert_eq!(sections[0].total_count, 1);
        assert_eq!(sections[1].connections.len(), 1);
        assert_eq!(sections[1].connections[0].id, "hit");
        let no_hits =
            connection_sections(&connections, &groups, "absent", ConnectionSortMode::Default);
        assert!(flatten_connection_rows(&no_hits, &HashSet::new(), None).is_empty());
    }

    #[test]
    fn nested_groups_render_children_before_connections_and_root_last() {
        let groups = vec![
            group("parent", "Parent", None, 0),
            group("child", "Child", Some("parent"), 0),
        ];
        let connections = vec![
            connection("parent-conn", "Parent Conn", Some("parent"), 0),
            connection("child-conn", "Child Conn", Some("child"), 0),
            connection("root-conn", "Root Conn", None, 0),
        ];
        let sections = connection_sections(
            &connections,
            &groups,
            "",
            crate::models::ConnectionSortMode::Default,
        );
        let expanded = ["parent".to_string(), "child".to_string()]
            .into_iter()
            .collect();
        let rows = flatten_connection_rows(&sections, &expanded, None);

        let labels = rows
            .iter()
            .map(|row| match row {
                ConnectionListRow::GroupHeader(section) => {
                    format!("group:{}:{}", section.label, section.depth)
                }
                ConnectionListRow::Connection {
                    connection_id,
                    depth,
                } => {
                    format!("conn:{connection_id}:{depth}")
                }
                ConnectionListRow::InlineGroupEditor { depth, .. } => {
                    format!("inline:{depth}")
                }
                ConnectionListRow::Separator => "separator".to_string(),
                ConnectionListRow::EmptyGroup { depth } => format!("empty:{depth}"),
            })
            .collect::<Vec<_>>();

        assert_eq!(
            labels,
            vec![
                "group:Parent:0",
                "group:Child:1",
                "conn:child-conn:2",
                "conn:parent-conn:1",
                "separator",
                "conn:root-conn:0",
            ]
        );
    }

    #[test]
    fn sort_mode_reorders_folders_not_only_their_connections() {
        let groups = vec![
            group("beta", "Beta", None, 1),
            group("alpha", "Alpha", None, 2),
        ];
        let connections = vec![
            connection("b", "Beta Host", Some("beta"), 0),
            connection("a", "Alpha Host", Some("alpha"), 0),
        ];

        let folder_order = |mode| {
            connection_sections(&connections, &groups, "", mode)
                .into_iter()
                .filter(|section| !section.is_root)
                .map(|section| section.label)
                .collect::<Vec<_>>()
        };

        // Default keeps the manual order the user dragged into place.
        assert_eq!(
            folder_order(crate::models::ConnectionSortMode::Default),
            vec!["Beta", "Alpha"]
        );
        assert_eq!(
            folder_order(crate::models::ConnectionSortMode::NameAsc),
            vec!["Alpha", "Beta"]
        );
        assert_eq!(
            folder_order(crate::models::ConnectionSortMode::NameDesc),
            vec!["Beta", "Alpha"]
        );
    }

    #[test]
    fn names_sort_by_number_value_so_host_lists_read_correctly() {
        let connections = vec![
            connection("c", "192.168.142.100", None, 0),
            connection("a", "192.168.142.13", None, 0),
            connection("b", "192.168.142.9", None, 0),
        ];
        let sections = connection_sections(
            &connections,
            &[],
            "",
            crate::models::ConnectionSortMode::NameAsc,
        );

        let names = sections[0]
            .connections
            .iter()
            .map(|connection| connection.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec!["192.168.142.9", "192.168.142.13", "192.168.142.100"]
        );
    }

    #[test]
    fn collapsed_groups_omit_every_descendant_from_flat_rows() {
        let groups = vec![
            group("parent", "Parent", None, 0),
            group("child", "Child", Some("parent"), 0),
        ];
        let connections = vec![
            connection("parent-conn", "Parent Conn", Some("parent"), 0),
            connection("child-conn", "Child Conn", Some("child"), 0),
            connection("root-conn", "Root Conn", None, 0),
        ];
        let sections = connection_sections(
            &connections,
            &groups,
            "",
            crate::models::ConnectionSortMode::Default,
        );
        let rows = flatten_connection_rows(&sections, &HashSet::new(), None);

        assert_eq!(rows.len(), 3);
        assert!(matches!(
            rows.first(),
            Some(ConnectionListRow::GroupHeader(section)) if section.label == "Parent"
        ));
        assert!(matches!(rows.get(1), Some(ConnectionListRow::Separator)));
        assert!(matches!(
            rows.get(2),
            Some(ConnectionListRow::Connection { connection_id, depth: 0 })
                if connection_id == "root-conn"
        ));
    }

    #[test]
    fn create_group_editor_inserts_root_inline_row_before_root_connections() {
        let connections = vec![connection("root-conn", "Root Conn", None, 0)];
        let sections = connection_sections(
            &connections,
            &[],
            "",
            crate::models::ConnectionSortMode::Default,
        );
        let editor = ConnectionGroupEditorState {
            mode: ConnectionGroupEditorMode::Create,
            id: None,
            name: String::new(),
            parent_id: None,
            error: None,
        };
        let rows = flatten_connection_rows(&sections, &HashSet::new(), Some(&editor));

        assert!(matches!(
            rows.first(),
            Some(ConnectionListRow::InlineGroupEditor {
                parent_id: None,
                depth: 0,
            })
        ));
        assert!(matches!(
            rows.get(1),
            Some(ConnectionListRow::Connection { connection_id, depth: 0 })
                if connection_id == "root-conn"
        ));
    }

    #[test]
    fn flat_connection_rows_store_ids_not_full_connections() {
        let connections = vec![connection("root-conn", "Root Conn", None, 0)];
        let sections = connection_sections(
            &connections,
            &[],
            "",
            crate::models::ConnectionSortMode::Default,
        );

        let rows = flatten_connection_rows(&sections, &HashSet::new(), None);

        assert!(matches!(
            rows.first(),
            Some(ConnectionListRow::Connection { connection_id, depth: 0 })
                if connection_id == "root-conn"
        ));
    }

    #[test]
    fn create_group_editor_inserts_child_inline_row_inside_expanded_parent() {
        let groups = vec![group("parent", "Parent", None, 0)];
        let sections =
            connection_sections(&[], &groups, "", crate::models::ConnectionSortMode::Default);
        let expanded = HashSet::from(["parent".to_string()]);
        let editor = ConnectionGroupEditorState {
            mode: ConnectionGroupEditorMode::Create,
            id: None,
            name: String::new(),
            parent_id: Some("parent".to_string()),
            error: None,
        };
        let rows = flatten_connection_rows(&sections, &expanded, Some(&editor));

        assert!(matches!(
            rows.first(),
            Some(ConnectionListRow::GroupHeader(section)) if section.label == "Parent"
        ));
        assert!(matches!(
            rows.get(1),
            Some(ConnectionListRow::InlineGroupEditor {
                parent_id: Some(parent_id),
                depth: 1,
            }) if parent_id == "parent"
        ));
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn widest_row_accounts_for_full_cjk_name_and_deep_tree_indent() {
        let connections = vec![
            connection("latin", "connection-name-0123", None, 0),
            connection("cjk", "生产环境数据库", None, 0),
        ];
        let rows = vec![
            ConnectionListRow::Connection {
                connection_id: "latin".to_string(),
                depth: 0,
            },
            ConnectionListRow::InlineGroupEditor {
                parent_id: None,
                depth: 0,
            },
            ConnectionListRow::Connection {
                connection_id: "cjk".to_string(),
                depth: 4,
            },
        ];

        assert_eq!(widest_connection_row(&rows, &connections), Some(2));
    }

    #[test]
    fn widest_row_ignores_group_headers_and_missing_connection_records() {
        let connections = vec![connection("real", "reachable full name", None, 0)];
        let rows = vec![
            ConnectionListRow::GroupHeader(super::ConnectionSectionHeader {
                group_id: Some("group".to_string()),
                label: "a folder label remains truncated".to_string(),
                is_root: false,
                depth: 0,
                total_count: 1,
                has_child_groups: false,
                is_empty: false,
            }),
            ConnectionListRow::Connection {
                connection_id: "missing".to_string(),
                depth: 9,
            },
            ConnectionListRow::Connection {
                connection_id: "real".to_string(),
                depth: 0,
            },
        ];

        assert_eq!(widest_connection_row(&rows, &connections), Some(2));
    }

    #[test]
    fn relevance_precedes_each_sort_mode_and_ties_keep_its_order() {
        let mut items = vec![
            connection("description", "Other", None, 0),
            connection("substring", "my-prod", None, 1),
            connection("prefix", "production", None, 2),
            connection("exact", "prod", None, 3),
        ];
        items[0].description = Some("prod".into());
        let original = items.clone();
        for mode in [
            ConnectionSortMode::Default,
            ConnectionSortMode::NameAsc,
            ConnectionSortMode::NameDesc,
        ] {
            let sections = connection_sections(&items, &[], "prod", mode);
            let ids: Vec<_> = sections[0]
                .connections
                .iter()
                .map(|c| c.id.as_str())
                .collect();
            assert_eq!(ids, ["exact", "prefix", "substring", "description"]);
        }
        assert_eq!(items, original, "ranking must never modify persisted order");
        let mut tied = vec![
            connection("twenty", "Server 20", None, 0),
            connection("two", "Server 2", None, 1),
        ];
        for item in &mut tied {
            item.description = Some("prod".into());
        }
        for (mode, expected) in [
            (ConnectionSortMode::Default, vec!["twenty", "two"]),
            (ConnectionSortMode::NameAsc, vec!["two", "twenty"]),
            (ConnectionSortMode::NameDesc, vec!["twenty", "two"]),
        ] {
            let sections = connection_sections(&tied, &[], "prod", mode);
            assert_eq!(
                sections[0]
                    .connections
                    .iter()
                    .map(|c| c.id.as_str())
                    .collect::<Vec<_>>(),
                expected
            );
        }
        tied[0].name = "Same".into();
        tied[1].name = "Same".into();
        tied[1].sort_order = 0;
        let sections = connection_sections(&tied, &[], "prod", ConnectionSortMode::Default);
        assert_eq!(sections[0].connections[0].id, "twenty");
        let cleared = connection_sections(&items, &[], "  ", ConnectionSortMode::Default);
        assert_eq!(cleared[0].connections, original);
    }

    #[test]
    fn sibling_groups_follow_best_descendant_and_empty_group_name_matches() {
        let groups = vec![
            group("many", "First", None, 0),
            group("best", "Last", None, 1),
            group("weak-child", "Child one", Some("best"), 0),
            group("best-child", "Child two", Some("best"), 1),
            group("empty", "prod", None, 2),
        ];
        let mut items: Vec<_> = (0..20)
            .map(|n| connection(&format!("prefix-{n}"), "production", Some("many"), n))
            .collect();
        items.push(connection("exact", "prod", Some("best-child"), 0));
        items.push(connection("substring", "my-prod", Some("weak-child"), 0));
        let sections = connection_sections(&items, &groups, "prod", ConnectionSortMode::Default);
        let order: Vec<_> = sections
            .iter()
            .filter_map(|s| s.group_id.as_deref())
            .collect();
        assert_eq!(order, ["best", "best-child", "weak-child", "many", "empty"]);
        assert!(sections.last().unwrap().connections.is_empty());
        let restored = connection_sections(&items, &groups, "", ConnectionSortMode::Default);
        assert_eq!(restored[0].group_id.as_deref(), Some("many"));
        assert_eq!(restored[2].group_id.as_deref(), Some("weak-child"));
        let expanded = groups.iter().map(|g| g.id.clone()).collect();
        let rows = flatten_connection_rows(&sections, &expanded, None);
        assert!(
            matches!(&rows[2], ConnectionListRow::Connection { connection_id, .. } if connection_id == "exact")
        );
    }

    #[test]
    fn malformed_group_graphs_keep_each_matching_connection_reachable_once() {
        let groups = vec![
            group("a", "A", Some("b"), 0),
            group("b", "B", Some("a"), 0),
            group("orphan", "Orphan", Some("missing"), 0),
            group("self", "Self", Some("self"), 0),
        ];
        let items = vec![
            connection("cycle", "prod", Some("a"), 0),
            connection("orphan", "production", Some("orphan"), 0),
            connection("missing", "my-prod", Some("missing"), 0),
            connection("self", "production", Some("self"), 0),
        ];
        let sections = connection_sections(&items, &groups, "prod", ConnectionSortMode::Default);
        let expanded = groups.iter().map(|g| g.id.clone()).collect();
        let ids: Vec<_> = flatten_connection_rows(&sections, &expanded, None)
            .into_iter()
            .filter_map(|row| match row {
                ConnectionListRow::Connection { connection_id, .. } => Some(connection_id),
                _ => None,
            })
            .collect();
        assert_eq!(ids, ["orphan", "cycle", "self", "missing"]);
        assert_eq!(ids.iter().collect::<HashSet<_>>().len(), items.len());
    }

    fn group(id: &str, name: &str, parent_id: Option<&str>, sort_order: i32) -> Group {
        Group {
            id: id.to_string(),
            name: name.to_string(),
            parent_id: parent_id.map(ToOwned::to_owned),
            sort_order,
            created_at_ms: None,
            updated_at_ms: None,
        }
    }

    fn connection(
        id: &str,
        name: &str,
        group_id: Option<&str>,
        sort_order: i32,
    ) -> SavedConnection {
        SavedConnection {
            extensions: Default::default(),
            tags: Vec::new(),
            id: id.to_string(),
            name: name.to_string(),
            config: nyaterm_core::ConnectionType::LocalTerminal {
                shell_path: String::new(),
                shell_args: String::new(),
                working_dir: None,
                ai_execution_profile: nyaterm_core::AiExecutionProfile::Auto,
                encoding: String::new(),
                dynamic_tab_title: false,
            },
            group_id: group_id.map(ToOwned::to_owned),
            description: None,
            sort_order,
            icon: None,
            icon_auto_detect: None,
            auth: None,
            recording: None,
            ssh_algorithms: None,
            ssh_compatibility_mode: false,
            ssh_profile: Default::default(),
            terminal_type: None,
            sftp: Default::default(),
            network: None,
            post_login: None,
            asset: None,
            created_at_ms: None,
            updated_at_ms: None,
            last_used_at_ms: None,
        }
    }
}
