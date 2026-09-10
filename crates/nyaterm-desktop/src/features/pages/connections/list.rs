use std::borrow::Cow;
use std::collections::HashMap;

use gpui::{
    App, Context, Entity, FontWeight, IntoElement, SharedString, div,
    prelude::{
        FluentBuilder, InteractiveElement, ParentElement, StatefulInteractiveElement, Styled,
    },
    px, rgb, svg,
};
use nyaterm_core::{Group, SavedConnection, natural_compare};
use rust_i18n::t;

use crate::features::{
    NyaTermApp, text_inputs::ORDINARY_INPUT_SHELL_PADDING_X_PX,
    text_inputs::ordinary_input_focus_ring, text_inputs::ordinary_input_shell_border_color,
};
use crate::models::{
    ConnectionEditorField, ConnectionEditorSelect, ConnectionGroupEditorMode,
    ConnectionGroupEditorState, ConnectionSortMode,
};
use nyaterm_ui::{
    NYA_FORM_CONTROL_HEIGHT_PX, NyaInput, NyaInputState, NyaNumberInput, NyaNumberInputState,
    NyaSelect, NyaSelectState,
};

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
    expanded_groups: &std::collections::HashSet<String>,
    group_editor: Option<&ConnectionGroupEditorState>,
) -> Vec<ConnectionListRow> {
    let has_groups = sections.iter().any(|section| !section.is_root);
    let mut rows = Vec::new();
    let mut children_by_parent: HashMap<Option<String>, Vec<ConnectionSection>> = HashMap::new();
    let mut root_connections = Vec::new();
    let create_parent_id = group_editor
        .filter(|editor| editor.mode == ConnectionGroupEditorMode::Create)
        .map(|editor| editor.parent_id.clone());
    for section in sections {
        if section.is_root {
            root_connections.extend(section.connections.clone());
            continue;
        }
        children_by_parent
            .entry(section.parent_id.clone())
            .or_default()
            .push(section.clone());
    }

    for section in children_by_parent.get(&None).cloned().unwrap_or_default() {
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
            connection_id: connection.id,
            depth: 0,
        });
    }
    rows
}

fn append_connection_section_rows(
    section: ConnectionSection,
    children_by_parent: &HashMap<Option<String>, Vec<ConnectionSection>>,
    expanded_groups: &std::collections::HashSet<String>,
    create_parent_id: Option<&Option<String>>,
    rows: &mut Vec<ConnectionListRow>,
) {
    let children = children_by_parent
        .get(&section.group_id)
        .cloned()
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

    for child in &children {
        append_connection_section_rows(
            child.clone(),
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
    for connection in section.connections {
        rows.push(ConnectionListRow::Connection {
            connection_id: connection.id,
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

pub(in crate::features) fn connection_sections(
    connections: &[SavedConnection],
    groups: &[Group],
    query: &str,
    sort_mode: ConnectionSortMode,
) -> Vec<ConnectionSection> {
    let mut by_group: HashMap<Option<String>, Vec<SavedConnection>> = HashMap::new();
    for connection in connections {
        if !connection_matches(connection, query) {
            continue;
        }
        by_group
            .entry(connection.group_id.clone())
            .or_default()
            .push(connection.clone());
    }
    for list in by_group.values_mut() {
        sort_connections(list, sort_mode);
    }

    let mut sections = Vec::new();
    let group_ids = groups
        .iter()
        .map(|group| group.id.clone())
        .collect::<std::collections::HashSet<_>>();
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
        sort_groups(children, sort_mode);
    }

    let mut visited = std::collections::HashSet::new();
    for group in children_by_parent.get(&None).cloned().unwrap_or_default() {
        let (mut group_sections, _) = build_connection_group_sections(
            group,
            0,
            &children_by_parent,
            &mut by_group,
            query,
            &mut visited,
        );
        sections.append(&mut group_sections);
    }

    let mut root = by_group.remove(&None).unwrap_or_default();
    for mut list in by_group.into_values() {
        root.append(&mut list);
    }
    sort_connections(&mut root, sort_mode);
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
    query: &str,
    visited: &mut std::collections::HashSet<String>,
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
            query,
            visited,
        );
        total_count += child_count;
        child_sections.append(&mut sections);
    }

    if !query.is_empty() && total_count == 0 {
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

pub(super) fn connection_matches(connection: &SavedConnection, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let haystack = format!(
        "{} {} {} {} {}",
        connection.name,
        connection.endpoint(),
        connection.kind_label(),
        connection.description.clone().unwrap_or_default(),
        connection.id
    )
    .to_ascii_lowercase();
    haystack.contains(query)
}

pub(super) fn sort_connections(connections: &mut [SavedConnection], mode: ConnectionSortMode) {
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

pub(super) fn connection_tree_indent_px(depth: usize) -> f32 {
    if depth == 0 {
        8.
    } else {
        8. + depth as f32 * 16. + 16.
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

#[derive(Clone)]
pub(super) struct ConnectionEditorChoice {
    pub value: Option<String>,
    pub label: String,
    pub search_text: Option<String>,
    pub subtitle: Option<String>,
    pub selected: bool,
}

impl ConnectionEditorChoice {
    pub(super) fn new(value: Option<String>, label: impl Into<String>, selected: bool) -> Self {
        Self {
            value,
            label: label.into(),
            search_text: None,
            subtitle: None,
            selected,
        }
    }

    pub(super) fn search_text(mut self, search_text: impl Into<String>) -> Self {
        self.search_text = Some(search_text.into());
        self
    }

    pub(super) fn subtitle(mut self, subtitle: impl Into<String>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }
}

pub(super) struct ConnectionEditorRenderContext<'a, 'cx> {
    pub palette: crate::theme::ThemePalette,
    pub fields: &'a ConnectionEditorFields,
    pub cx: &'a mut Context<'cx, NyaTermApp>,
}

pub(super) fn connection_editor_select(
    render: ConnectionEditorRenderContext<'_, '_>,
    id: &'static str,
    label: impl Into<FieldLabel>,
    select: ConnectionEditorSelect,
) -> impl IntoElement {
    let ConnectionEditorRenderContext {
        palette,
        fields,
        cx,
    } = render;
    let _ = cx;
    let label = label.into();
    let show_label = !label.is_empty();
    div()
        .id(SharedString::from(id))
        .min_w_0()
        .flex_1()
        .flex()
        .flex_col()
        .gap_1()
        .when(show_label, |this| {
            this.child(field_caption(palette, &label))
        })
        .child(
            div()
                .id(SharedString::from(format!("{id}-container")))
                .h(px(EDITOR_CONTROL_HEIGHT_PX))
                .min_w_0()
                .child(NyaSelect::new(&fields.select(select))),
        )
}

pub(super) fn toggle_chip(
    palette: crate::theme::ThemePalette,
    label: impl Into<SharedString>,
    selected: bool,
    on_click: impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    let label: SharedString = label.into();
    div()
        .id(SharedString::from(format!("connection-toggle-{label}")))
        .h(px(28.))
        .px_2()
        .flex()
        .items_center()
        .gap_2()
        .rounded_md()
        .border_1()
        .border_color(rgb(palette.border))
        .text_size(px(10.))
        .font_weight(FontWeight(500.))
        .cursor_pointer()
        .text_color(if selected {
            rgb(palette.text)
        } else {
            rgb(palette.text_muted)
        })
        .bg(rgb(palette.input))
        .hover(|this| this.bg(rgb(palette.hover)))
        .child(div().min_w_0().child(label))
        .child(
            div()
                .w(px(28.))
                .h(px(16.))
                .flex()
                .items_center()
                .justify_start()
                .when(selected, |this| this.justify_end())
                .px(px(2.))
                .rounded_full()
                .bg(if selected {
                    rgb(palette.primary)
                } else {
                    rgb(palette.border)
                })
                .child(div().size(px(12.)).rounded_full().bg(if selected {
                    rgb(palette.on_primary)
                } else {
                    rgb(palette.text_dimmed)
                })),
        )
        .on_click(on_click)
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use nyaterm_core::{Group, SavedConnection};

    use crate::models::{ConnectionGroupEditorMode, ConnectionGroupEditorState};

    use super::{
        ConnectionListRow, connection_sections, flatten_connection_rows, widest_connection_row,
    };

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
    fn saved_connection_tooltip_matches_tauri_fields_for_every_session_type() {
        let cases = [
            (
                serde_json::json!({
                    "type": "ssh",
                    "host": " ssh.example.com ",
                    "port": 2222,
                    "username": " root "
                }),
                vec![
                    "savedConnections.host",
                    "savedConnections.port",
                    "savedConnections.user",
                    "savedConnections.description",
                ],
            ),
            (
                serde_json::json!({
                    "type": "local_terminal",
                    "shell_path": " pwsh.exe ",
                    "shell_args": " -NoLogo ",
                    "working_dir": " C:\\\\Work "
                }),
                vec![
                    "savedConnections.terminalPath",
                    "savedConnections.shellArgs",
                    "savedConnections.workingDir",
                    "savedConnections.description",
                ],
            ),
            (
                serde_json::json!({
                    "type": "telnet",
                    "host": "switch.example.com",
                    "port": 2323,
                    "username": "operator"
                }),
                vec![
                    "savedConnections.host",
                    "savedConnections.port",
                    "savedConnections.user",
                    "savedConnections.description",
                ],
            ),
            (
                serde_json::json!({
                    "type": "serial",
                    "port_name": "COM3",
                    "baud_rate": 115200,
                    "data_bits": 8,
                    "parity": "none",
                    "stop_bits": "1"
                }),
                vec![
                    "savedConnections.serialPort",
                    "savedConnections.baudRate",
                    "savedConnections.dataBits",
                    "savedConnections.parity",
                    "savedConnections.stopBits",
                    "savedConnections.description",
                ],
            ),
            (
                serde_json::json!({
                    "type": "rdp",
                    "host": "desktop.example.com",
                    "port": 3390,
                    "username": "alice"
                }),
                vec![
                    "savedConnections.host",
                    "savedConnections.port",
                    "savedConnections.user",
                    "savedConnections.description",
                ],
            ),
            (
                serde_json::json!({
                    "type": "vnc",
                    "host": "screen.example.com",
                    "port": 5901
                }),
                vec![
                    "savedConnections.host",
                    "savedConnections.port",
                    "savedConnections.user",
                    "savedConnections.description",
                ],
            ),
        ];

        for (config, expected_keys) in cases {
            let mut connection = connection("test", "Test", None, 0);
            connection.config = serde_json::from_value(config).expect("valid connection type");
            connection.description = Some(" production host ".to_string());

            let rows = super::connection_detail_rows(&connection, &HashMap::new());
            let labels = rows
                .iter()
                .map(|row| row.label.as_str())
                .collect::<Vec<_>>();
            let expected = expected_keys
                .into_iter()
                .map(|key| rust_i18n::t!(key).into_owned())
                .collect::<Vec<_>>();

            assert_eq!(
                labels,
                expected,
                "unexpected fields for {}",
                connection.kind_label()
            );
            assert_eq!(
                rows.last().map(|row| row.value.as_str()),
                Some("production host")
            );
            let expected_copy_count = match connection.kind_label() {
                "SSH" | "Telnet" | "RDP" => 3,
                "VNC" => 2,
                _ => 0,
            };
            assert_eq!(
                rows.iter().filter(|row| row.copy_value.is_some()).count(),
                expected_copy_count,
                "unexpected copy buttons for {}",
                connection.kind_label()
            );
        }
    }

    #[test]
    fn saved_connection_tooltip_uses_tauri_missing_value_rules() {
        let mut local = connection("local", "Local", None, 0);
        local.config = serde_json::from_value(serde_json::json!({
            "type": "local_terminal",
            "shell_path": " ",
            "shell_args": " ",
            "working_dir": null
        }))
        .expect("valid local terminal");

        let local_rows = super::connection_detail_rows(&local, &HashMap::new());
        assert_eq!(local_rows.len(), 3, "blank shell args must be omitted");
        assert_eq!(
            local_rows[0].value,
            rust_i18n::t!("savedConnections.notSet").to_string()
        );
        assert_eq!(
            local_rows[1].value,
            rust_i18n::t!("savedConnections.notSet").to_string()
        );
        assert_eq!(
            local_rows[2].value,
            rust_i18n::t!("savedConnections.noDescription").to_string()
        );

        assert!(
            local_rows.iter().all(|row| row.copy_value.is_none()),
            "Tauri does not offer copy buttons for local terminal fields"
        );

        let mut vnc = connection("vnc", "VNC", None, 0);
        vnc.config = serde_json::from_value(serde_json::json!({
            "type": "vnc",
            "host": "screen.example.com"
        }))
        .expect("valid VNC connection");
        let vnc_rows = super::connection_detail_rows(&vnc, &HashMap::new());
        assert_eq!(
            vnc_rows[2].value,
            rust_i18n::t!("savedConnections.notSet").to_string(),
            "Tauri renders the unavailable VNC username as not set"
        );
        assert_eq!(
            vnc_rows[0].copy_value.as_deref(),
            Some("screen.example.com")
        );
        assert_eq!(vnc_rows[1].copy_value.as_deref(), Some("5900"));
        assert_eq!(vnc_rows[2].copy_value, None);

        let mut ssh = connection("ssh", "SSH", None, 0);
        ssh.config = serde_json::from_value(serde_json::json!({
            "type": "ssh",
            "host": " ",
            "port": 22,
            "username": " "
        }))
        .expect("valid SSH connection");
        let ssh_rows = super::connection_detail_rows(&ssh, &HashMap::new());
        assert_eq!(ssh_rows[0].copy_value, None);
        assert_eq!(ssh_rows[1].copy_value.as_deref(), Some("22"));
        assert_eq!(ssh_rows[2].copy_value, None);
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

/// What the editor's inputs and selects need from the app, resolved once per
/// render.
///
/// Sections receive this rather than the app, because they are free functions
/// that already hold a `Context` — reading the app entity again from inside its
/// own update panics.
pub(super) struct ConnectionEditorFields {
    fields: HashMap<ConnectionEditorField, Entity<NyaInputState>>,
    number_fields: HashMap<ConnectionEditorField, Entity<NyaNumberInputState>>,
    selects: HashMap<ConnectionEditorSelect, Entity<NyaSelectState>>,
    forwarding_endpoint_selects: HashMap<usize, Entity<NyaSelectState>>,
    forwarding_endpoint_fields: HashMap<(usize, ConnectionEditorField), Entity<NyaInputState>>,
}

impl ConnectionEditorFields {
    pub(super) fn new(
        fields: HashMap<ConnectionEditorField, Entity<NyaInputState>>,
        number_fields: HashMap<ConnectionEditorField, Entity<NyaNumberInputState>>,
        selects: HashMap<ConnectionEditorSelect, Entity<NyaSelectState>>,
        forwarding_endpoint_selects: HashMap<usize, Entity<NyaSelectState>>,
        forwarding_endpoint_fields: HashMap<(usize, ConnectionEditorField), Entity<NyaInputState>>,
    ) -> Self {
        Self {
            fields,
            number_fields,
            selects,
            forwarding_endpoint_selects,
            forwarding_endpoint_fields,
        }
    }

    pub(super) fn get(&self, field: &ConnectionEditorField) -> Option<&Entity<NyaInputState>> {
        self.fields.get(field)
    }

    pub(super) fn get_number(
        &self,
        field: &ConnectionEditorField,
    ) -> Option<&Entity<NyaNumberInputState>> {
        self.number_fields.get(field)
    }

    pub(super) fn select(&self, select: ConnectionEditorSelect) -> Entity<NyaSelectState> {
        self.selects
            .get(&select)
            .cloned()
            .expect("connection editor select registered before rendering")
    }

    pub(super) fn forwarding_endpoint_select(&self, index: usize) -> Entity<NyaSelectState> {
        self.forwarding_endpoint_selects
            .get(&index)
            .cloned()
            .expect("forwarding endpoint select registered before rendering")
    }

    pub(super) fn forwarding_endpoint_field(
        &self,
        index: usize,
        field: ConnectionEditorField,
    ) -> Entity<NyaInputState> {
        self.forwarding_endpoint_fields
            .get(&(index, field))
            .cloned()
            .expect("forwarding endpoint field registered before rendering")
    }
}

/// A field's caption, and whether it carries the required marker.
#[derive(Clone, Default)]
pub(super) struct FieldLabel {
    text: SharedString,
    required: bool,
}

impl FieldLabel {
    fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
}

impl From<&'static str> for FieldLabel {
    fn from(text: &'static str) -> Self {
        Self {
            text: SharedString::from(text),
            required: false,
        }
    }
}

impl From<String> for FieldLabel {
    fn from(text: String) -> Self {
        Self {
            text: SharedString::from(text),
            required: false,
        }
    }
}

impl<'a> From<Cow<'a, str>> for FieldLabel {
    fn from(text: Cow<'a, str>) -> Self {
        Self {
            text: SharedString::from(text),
            required: false,
        }
    }
}

impl From<SharedString> for FieldLabel {
    fn from(text: SharedString) -> Self {
        Self {
            text,
            required: false,
        }
    }
}

/// Mark a caption as required, so it renders with the red asterisk.
pub(super) fn required(label: impl Into<FieldLabel>) -> FieldLabel {
    let mut label = label.into();
    label.required = true;
    label
}

/// The caption above an input or a select.
pub(super) fn field_caption(
    palette: crate::theme::ThemePalette,
    label: &FieldLabel,
) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap_0p5()
        .text_xs()
        .text_color(rgb(palette.text_muted))
        .child(label.text.clone())
        .when(label.required, |this| {
            this.child(div().text_color(rgb(palette.danger)).child("*"))
        })
}

/// The height every input, select and stepper in the editor shares.
pub(super) const EDITOR_CONTROL_HEIGHT_PX: f32 = NYA_FORM_CONTROL_HEIGHT_PX;

/// A caption above one editable field.
///
/// Tauri puts the caption above the box rather than inside it; a box that also
/// holds its label has half the room for what is typed into it, which is what
/// made long hosts and paths unreadable here.
pub(super) fn editor_field(
    palette: crate::theme::ThemePalette,
    label: impl Into<FieldLabel>,
    field: ConnectionEditorField,
    fields: &ConnectionEditorFields,
    cx: &App,
) -> impl IntoElement {
    let label = label.into();
    div()
        .flex()
        .flex_col()
        .gap_1()
        .when(!label.is_empty(), |this| {
            this.child(field_caption(palette, &label))
        })
        .child(editor_field_box(palette, field, fields, cx))
}

pub(super) fn forwarding_endpoint_editor_field(
    palette: crate::theme::ThemePalette,
    label: impl Into<FieldLabel>,
    index: usize,
    field: ConnectionEditorField,
    fields: &ConnectionEditorFields,
    cx: &App,
) -> impl IntoElement {
    let label = label.into();
    let entity = fields.forwarding_endpoint_field(index, field);
    let handle = entity.read(cx).focus_handle();
    let focused = entity.read(cx).has_focus();
    div()
        .flex()
        .flex_col()
        .gap_1()
        .when(!label.is_empty(), |this| {
            this.child(field_caption(palette, &label))
        })
        .child(
            div()
                .h(px(EDITOR_CONTROL_HEIGHT_PX))
                .id(SharedString::from(format!(
                    "connection-agent-endpoint-{index}-field"
                )))
                .min_w_0()
                .px(px(ORDINARY_INPUT_SHELL_PADDING_X_PX))
                .flex()
                .items_center()
                .rounded_sm()
                .border_1()
                .border_color(ordinary_input_shell_border_color(palette, focused))
                .when(focused, |this| {
                    this.shadow(ordinary_input_focus_ring(palette))
                })
                .bg(rgb(palette.input))
                .cursor_text()
                .on_click(move |_, window, cx| {
                    window.focus(&handle, cx);
                })
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .text_xs()
                        .text_color(rgb(palette.text))
                        .child(NyaInput::new(&entity)),
                ),
        )
}

/// The bordered box holding one field, without a caption.
pub(super) fn editor_field_box(
    palette: crate::theme::ThemePalette,
    field: ConnectionEditorField,
    fields: &ConnectionEditorFields,
    cx: &App,
) -> impl IntoElement {
    let entity = fields.get(&field);
    let handle = entity.map(|field| field.read(cx).focus_handle());
    let focused = entity.is_some_and(|field| field.read(cx).has_focus());
    div()
        .h(px(
            if matches!(
                field,
                ConnectionEditorField::Description | ConnectionEditorField::PostLoginCommand
            ) {
                112.
            } else {
                EDITOR_CONTROL_HEIGHT_PX
            },
        ))
        .id("connection-list-search-input-shell")
        .min_w_0()
        .px(px(ORDINARY_INPUT_SHELL_PADDING_X_PX))
        .flex()
        .items_center()
        .rounded_sm()
        .border_1()
        .border_color(ordinary_input_shell_border_color(palette, focused))
        .when(focused, |this| {
            this.shadow(ordinary_input_focus_ring(palette))
        })
        .bg(rgb(palette.input))
        .cursor_text()
        .when_some(handle, |row, handle| {
            row.on_click(move |_, window, cx| {
                window.focus(&handle, cx);
            })
        })
        .children(entity.map(|field| {
            div()
                .min_w_0()
                .flex_1()
                .text_xs()
                .text_color(rgb(palette.text))
                .child(NyaInput::new(field))
        }))
}

/// A numeric field backed by gpui-component's spinner control.
pub(super) fn editor_stepper_field(
    palette: crate::theme::ThemePalette,
    label: impl Into<FieldLabel>,
    field: ConnectionEditorField,
    fields: &ConnectionEditorFields,
    _cx: &App,
) -> impl IntoElement {
    let label = label.into();
    let entity = fields.get_number(&field);
    div()
        .flex()
        .flex_col()
        .gap_1()
        .when(!label.is_empty(), |this| {
            this.child(field_caption(palette, &label))
        })
        .child(
            div()
                .h(px(EDITOR_CONTROL_HEIGHT_PX))
                .min_w_0()
                .flex()
                .items_center()
                .children(entity.map(|field| {
                    div()
                        .min_w_0()
                        .flex_1()
                        .text_xs()
                        .text_color(rgb(palette.text))
                        .child(NyaNumberInput::new(field))
                })),
        )
}

pub(super) fn icon_action_button(
    palette: crate::theme::ThemePalette,
    id: impl Into<String>,
    label: &'static str,
    tooltip: impl Into<String>,
    on_click: impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    icon_action_button_styled(palette, id, label, tooltip, None, false, on_click)
}

/// [`icon_action_button`] with the two knobs the sort control needs: a tint that
/// marks the button as active, and a vertical flip that turns an ascending glyph
/// into a descending one.
pub(super) fn icon_action_button_styled(
    palette: crate::theme::ThemePalette,
    id: impl Into<String>,
    label: &'static str,
    tooltip: impl Into<String>,
    tint: Option<u32>,
    flip_vertical: bool,
    on_click: impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    // label may be a glyph fallback or an icons/*.svg path.
    let is_svg = label.starts_with("icons/") && label.ends_with(".svg");
    let tooltip = tooltip.into();
    let id = SharedString::from(id.into());
    let base_color = tint.unwrap_or(palette.text_muted);
    let hover_color = tint.unwrap_or(palette.text);
    // `svg()` takes its tint from its *own* computed style — GPUI starts every
    // element from `Style::default()`, so a glyph with no `text_color` of its own
    // resolves to `None` and is skipped entirely at paint time. The parent's
    // `text_color` only reaches real text. Hence the explicit color here, and the
    // group so hover still brightens the icon rather than only the background.
    div()
        .id(id.clone())
        .group(id.clone())
        .size(px(24.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .text_size(px(11.))
        .text_color(rgb(base_color))
        .cursor_pointer()
        .hover(move |this| {
            this.bg(rgb(palette.surface_elevated))
                .text_color(rgb(hover_color))
        })
        .on_click(on_click)
        .tooltip(move |window, cx| nyaterm_ui::NyaTooltip::new(tooltip.clone()).build(window, cx))
        .when(is_svg, |this| {
            let mut icon = svg()
                .size(px(14.))
                .flex_none()
                .path(label)
                .text_color(rgb(base_color))
                .group_hover(id.clone(), move |this| this.text_color(rgb(hover_color)));
            if flip_vertical {
                icon = icon.with_transformation(gpui::Transformation::scale(gpui::size(1., -1.)));
            }
            this.child(icon)
        })
        .when(!is_svg, |this| this.child(label))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ConnectionDetailRow {
    pub(super) label: String,
    pub(super) value: String,
    pub(super) copy_value: Option<String>,
    pub(super) multiline: bool,
}

impl ConnectionDetailRow {
    fn new(label: String, value: String) -> Self {
        Self {
            label,
            value,
            copy_value: None,
            multiline: false,
        }
    }

    fn copyable(mut self, copy_value: Option<String>) -> Self {
        self.copy_value = copy_value;
        self
    }

    fn multiline(mut self) -> Self {
        self.multiline = true;
        self
    }
}

pub(super) fn connection_detail_rows(
    connection: &SavedConnection,
    all_connections: &HashMap<String, SavedConnection>,
) -> Vec<ConnectionDetailRow> {
    let description = connection
        .description
        .as_deref()
        .and_then(non_empty_detail_value)
        .unwrap_or_else(|| t!("savedConnections.noDescription").to_string());

    let mut rows = match &connection.config {
        nyaterm_core::ConnectionType::Ssh {
            host,
            port,
            username,
            ..
        } => vec![
            copyable_text_detail_row(t!("savedConnections.host").to_string(), host),
            copyable_value_detail_row(t!("savedConnections.port").to_string(), port.to_string()),
            copyable_text_detail_row(t!("savedConnections.user").to_string(), username),
        ],
        nyaterm_core::ConnectionType::LocalTerminal {
            shell_path,
            shell_args,
            working_dir,
            ..
        } => {
            let mut rows = vec![ConnectionDetailRow::new(
                t!("savedConnections.terminalPath").to_string(),
                required_detail_value(shell_path),
            )];
            if let Some(shell_args) = non_empty_detail_value(shell_args) {
                rows.push(ConnectionDetailRow::new(
                    t!("savedConnections.shellArgs").to_string(),
                    shell_args,
                ));
            }
            rows.push(ConnectionDetailRow::new(
                t!("savedConnections.workingDir").to_string(),
                working_dir
                    .as_deref()
                    .map(required_detail_value)
                    .unwrap_or_else(|| t!("savedConnections.notSet").to_string()),
            ));
            rows
        }
        nyaterm_core::ConnectionType::Telnet {
            host,
            port,
            username,
            ..
        } => vec![
            copyable_text_detail_row(t!("savedConnections.host").to_string(), host),
            copyable_value_detail_row(t!("savedConnections.port").to_string(), port.to_string()),
            copyable_text_detail_row(t!("savedConnections.user").to_string(), username),
        ],
        nyaterm_core::ConnectionType::Serial {
            port_name,
            baud_rate,
            data_bits,
            parity,
            stop_bits,
            ..
        } => {
            let mut rows = vec![
                ConnectionDetailRow::new(
                    t!("savedConnections.serialPort").to_string(),
                    required_detail_value(port_name),
                ),
                ConnectionDetailRow::new(
                    t!("savedConnections.baudRate").to_string(),
                    baud_rate.to_string(),
                ),
                ConnectionDetailRow::new(
                    t!("savedConnections.dataBits").to_string(),
                    data_bits.to_string(),
                ),
            ];
            if let Some(parity) = non_empty_detail_value(parity) {
                rows.push(ConnectionDetailRow::new(
                    t!("savedConnections.parity").to_string(),
                    parity,
                ));
            }
            if let Some(stop_bits) = non_empty_detail_value(stop_bits) {
                rows.push(ConnectionDetailRow::new(
                    t!("savedConnections.stopBits").to_string(),
                    stop_bits,
                ));
            }
            rows
        }
        nyaterm_core::ConnectionType::Rdp {
            host,
            port,
            username,
            ..
        } => vec![
            copyable_text_detail_row(t!("savedConnections.host").to_string(), host),
            copyable_value_detail_row(t!("savedConnections.port").to_string(), port.to_string()),
            copyable_text_detail_row(t!("savedConnections.user").to_string(), username),
        ],
        nyaterm_core::ConnectionType::Vnc { host, port, .. } => vec![
            copyable_text_detail_row(t!("savedConnections.host").to_string(), host),
            copyable_value_detail_row(t!("savedConnections.port").to_string(), port.to_string()),
            ConnectionDetailRow::new(
                t!("savedConnections.user").to_string(),
                t!("savedConnections.notSet").to_string(),
            ),
        ],
    };

    if matches!(&connection.config, nyaterm_core::ConnectionType::Ssh { .. })
        && connection
            .network
            .as_ref()
            .and_then(|network| network.proxy_jump_id.as_deref())
            .is_some()
    {
        rows.push(
            ConnectionDetailRow::new(
                t!("savedConnections.jumpHostChain").to_string(),
                format_jump_host_chain(connection, all_connections),
            )
            .multiline(),
        );
    }
    rows.push(
        ConnectionDetailRow::new(t!("savedConnections.description").to_string(), description)
            .multiline(),
    );
    rows
}

fn required_detail_value(value: &str) -> String {
    non_empty_detail_value(value).unwrap_or_else(|| t!("savedConnections.notSet").to_string())
}

fn copyable_text_detail_row(label: String, value: &str) -> ConnectionDetailRow {
    let copy_value = non_empty_detail_value(value);
    ConnectionDetailRow::new(label, required_detail_value(value)).copyable(copy_value)
}

fn copyable_value_detail_row(label: String, value: String) -> ConnectionDetailRow {
    ConnectionDetailRow::new(label, value.clone()).copyable(Some(value))
}

fn non_empty_detail_value(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

pub(super) fn format_jump_host_chain(
    connection: &SavedConnection,
    by_id: &HashMap<String, SavedConnection>,
) -> String {
    let Some(mut jump_id) = connection
        .network
        .as_ref()
        .and_then(|network| network.proxy_jump_id.clone())
    else {
        return t!("savedConnections.notSet").to_string();
    };
    let mut seen = std::collections::HashSet::new();
    seen.insert(connection.id.clone());
    let mut labels = Vec::new();
    loop {
        if !seen.insert(jump_id.clone()) {
            labels.push(t!("savedConnections.jumpHostCycle").to_string());
            break;
        }
        let Some(jump) = by_id.get(jump_id.as_str()) else {
            labels.push(t!("savedConnections.jumpHostMissing").to_string());
            break;
        };
        labels.push(jump.name.clone());
        match jump
            .network
            .as_ref()
            .and_then(|network| network.proxy_jump_id.clone())
        {
            Some(next) => jump_id = next,
            None => break,
        }
    }
    if labels.is_empty() {
        t!("savedConnections.notSet").to_string()
    } else {
        labels.join(" -> ")
    }
}
