//! Shared matching rules for the saved-connection tree and its navigation.

use std::collections::HashMap;

use crate::models::{ConnectionType, Group, SavedConnection};

pub fn normalize_connection_search_query(query: &str) -> String {
    query.trim().to_ascii_lowercase()
}

/// A group-name hit includes the entire subtree, even groups without connections.
pub struct ConnectionSearch {
    query: String,
    group_ranks: HashMap<String, ConnectionSearchRank>,
}

/// Text relevance only. Zero is reserved for an unfiltered catalog.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct ConnectionSearchRank(u16);

impl ConnectionSearch {
    pub fn new(query: &str, groups: &[Group]) -> Self {
        let query = normalize_connection_search_query(query);
        let mut group_ranks = HashMap::new();
        if !query.is_empty() {
            let mut children: HashMap<&str, Vec<&str>> = HashMap::new();
            let mut pending = Vec::new();
            for group in groups {
                if let Some(parent_id) = group.parent_id.as_deref() {
                    children.entry(parent_id).or_default().push(&group.id);
                }
                let rank = text_rank(&group.name, &query, [350, 325, 300]);
                if rank > ConnectionSearchRank::default() {
                    pending.push((group.id.as_str(), rank));
                }
            }
            while let Some((id, rank)) = pending.pop() {
                // Only improvements propagate. There are three possible group
                // ranks, so overlapping hits and cyclic catalogs stay bounded.
                if group_ranks
                    .get(id)
                    .is_some_and(|previous| *previous >= rank)
                {
                    continue;
                }
                group_ranks.insert(id.to_owned(), rank);
                if let Some(descendants) = children.get(id) {
                    pending.extend(descendants.iter().map(|id| (*id, rank)));
                }
            }
        }
        Self { query, group_ranks }
    }

    pub fn is_empty(&self) -> bool {
        self.query.is_empty()
    }

    pub fn includes_group(&self, group_id: &str) -> bool {
        self.group_rank(group_id).is_some()
    }

    /// Includes the strongest matching ancestor, preserving subtree recall.
    pub fn group_rank(&self, group_id: &str) -> Option<ConnectionSearchRank> {
        if self.is_empty() {
            Some(ConnectionSearchRank::default())
        } else {
            self.group_ranks.get(group_id).copied()
        }
    }

    pub fn matches_connection(&self, connection: &SavedConnection) -> bool {
        self.rank_connection(connection).is_some()
    }

    pub fn rank_connection(&self, connection: &SavedConnection) -> Option<ConnectionSearchRank> {
        if self.is_empty() {
            return Some(ConnectionSearchRank::default());
        }
        let inherited = connection
            .group_id
            .as_deref()
            .and_then(|id| self.group_rank(id));
        let endpoint = connection.endpoint();
        let haystack = format!(
            "{} {} {} {} {}",
            connection.name,
            endpoint,
            connection.kind_label(),
            connection.description.as_deref().unwrap_or_default(),
            connection.id
        )
        .to_ascii_lowercase();
        // Keep eligibility identical to the original concatenated-field filter.
        // Field scoring must neither drop cross-field hits nor add new matches.
        if inherited.is_none() && !haystack.contains(&self.query) {
            return None;
        }
        let host = match &connection.config {
            ConnectionType::Ssh { host, .. }
            | ConnectionType::Telnet { host, .. }
            | ConnectionType::Rdp { host, .. }
            | ConnectionType::Vnc { host, .. } => host.as_str(),
            ConnectionType::Adb {
                device: crate::models::android::AdbDeviceSelector::Tcp { host, .. },
                ..
            } => host.as_str(),
            _ => "",
        };
        let mut rank = inherited.unwrap_or(ConnectionSearchRank(1));
        for (text, weights) in [
            (connection.name.as_str(), [1000, 900, 800]),
            (host, [1000, 900, 800]),
            (endpoint.as_str(), [1000, 900, 800]),
            (connection.kind_label(), [250, 225, 200]),
            (
                connection.description.as_deref().unwrap_or_default(),
                [150, 125, 100],
            ),
            (connection.id.as_str(), [75, 50, 25]),
        ] {
            rank = rank.max(text_rank(text, &self.query, weights));
        }
        Some(rank)
    }
}

fn text_rank(text: &str, query: &str, weights: [u16; 3]) -> ConnectionSearchRank {
    let text = text.to_ascii_lowercase();
    ConnectionSearchRank(if text == query {
        weights[0]
    } else if text.starts_with(query) {
        weights[1]
    } else if text.contains(query) {
        weights[2]
    } else {
        0
    })
}

#[cfg(test)]
mod tests {
    use crate::connection_search::{
        ConnectionSearch, ConnectionSearchRank, normalize_connection_search_query,
    };
    use crate::models::{Group, SavedConnection};

    fn group(id: &str, name: &str, parent: Option<&str>) -> Group {
        Group {
            id: id.to_owned(),
            name: name.to_owned(),
            parent_id: parent.map(str::to_owned),
            sort_order: 0,
            created_at_ms: None,
            updated_at_ms: None,
        }
    }

    fn connection(group_id: Option<&str>) -> SavedConnection {
        serde_json::from_value(serde_json::json!({
            "id": "unique-identifier",
            "name": "Gateway",
            "type": "ssh",
            "host": "host.example.com",
            "description": "Bastion for testing",
            "group_id": group_id,
        }))
        .expect("valid saved connection")
    }

    #[test]
    fn group_name_hits_include_descendants_and_same_named_groups() {
        let groups = vec![
            group("region", "Region", None),
            group("prod", "Production", Some("region")),
            group("nested", "Services", Some("prod")),
            group("empty", "Empty", Some("nested")),
            group("other-prod", "Production", None),
            group("unrelated", "Staging", Some("region")),
        ];
        let search = ConnectionSearch::new("  PrOd  ", &groups);
        for id in ["prod", "nested", "empty", "other-prod"] {
            assert!(search.includes_group(id), "missing group {id}");
            assert!(search.matches_connection(&connection(Some(id))));
        }
        for id in ["region", "unrelated", "missing"] {
            assert!(!search.includes_group(id), "unexpected group {id}");
            assert!(!search.matches_connection(&connection(Some(id))));
        }
        assert!(!search.matches_connection(&connection(None)));
    }

    #[test]
    fn chinese_group_names_support_substring_matches() {
        let groups = vec![group("cn", "上海生产环境", None)];
        assert!(ConnectionSearch::new("生产", &groups).includes_group("cn"));
        assert!(!ConnectionSearch::new("北京", &groups).includes_group("cn"));
    }

    #[test]
    fn original_connection_fields_remain_searchable_without_group_hits() {
        let connection = connection(Some("unmatched"));
        for query in ["GATE", "HOST.example", "SSH", "bastion", "unique-ident"] {
            assert!(
                ConnectionSearch::new(query, &[]).matches_connection(&connection),
                "field query did not match: {query}"
            );
        }
        assert!(!ConnectionSearch::new("absent", &[]).matches_connection(&connection));
    }

    #[test]
    fn whitespace_query_preserves_the_unfiltered_catalog() {
        assert_eq!(normalize_connection_search_query(" \tPrOD\n"), "prod");
        let search = ConnectionSearch::new(" \t\n", &[]);
        assert!(search.is_empty());
        assert!(search.includes_group("any"));
        assert!(search.matches_connection(&connection(None)));
    }

    #[test]
    fn overlapping_hits_cycles_and_missing_parents_are_bounded() {
        let groups = vec![
            group("a", "Match A", Some("b")),
            group("b", "Match B", Some("a")),
            group("c", "Child", Some("a")),
            group("orphan", "Match orphan", Some("missing")),
            group("self", "Match self", Some("self")),
        ];
        let search = ConnectionSearch::new("match", &groups);
        for id in ["a", "b", "c", "orphan", "self"] {
            assert!(search.includes_group(id));
        }
        assert!(!search.includes_group("missing"));
    }

    #[test]
    fn ranking_keeps_the_original_recall_for_fields_cross_field_text_and_group_trees() {
        use std::collections::HashSet;

        // Deliberately independent of the ranked implementation: the supported
        // concatenated-text/subtree filter is the reference for eligibility.
        fn legacy_matches(connection: &SavedConnection, groups: &[Group], query: &str) -> bool {
            let query = query.trim().to_ascii_lowercase();
            let mut included: HashSet<_> = groups
                .iter()
                .filter(|group| group.name.to_ascii_lowercase().contains(&query))
                .map(|group| group.id.as_str())
                .collect();
            loop {
                let children: Vec<_> = groups
                    .iter()
                    .filter(|group| {
                        group
                            .parent_id
                            .as_deref()
                            .is_some_and(|id| included.contains(id))
                    })
                    .map(|group| group.id.as_str())
                    .collect();
                let before = included.len();
                included.extend(children);
                if before == included.len() {
                    break;
                }
            }
            query.is_empty()
                || connection
                    .group_id
                    .as_deref()
                    .is_some_and(|id| included.contains(id))
                || format!(
                    "{} {} {} {} {}",
                    connection.name,
                    connection.endpoint(),
                    connection.kind_label(),
                    connection.description.as_deref().unwrap_or_default(),
                    connection.id
                )
                .to_ascii_lowercase()
                .contains(&query)
        }

        let groups = vec![
            group("a", "Production", None),
            group("b", "上海", Some("a")),
            group("cycle1", "Cycle", Some("cycle2")),
            group("cycle2", "Child", Some("cycle1")),
            group("orphan", "Lost", Some("missing")),
        ];
        let mut connections = Vec::new();
        for group_id in [
            None,
            Some("a"),
            Some("b"),
            Some("cycle1"),
            Some("cycle2"),
            Some("missing"),
        ] {
            for config in [
                serde_json::json!({"type":"ssh", "host":"host.example.com", "username":"alice", "port":2222}),
                serde_json::json!({"type":"rdp", "host":"10.2.1.28", "port":3389}),
                serde_json::json!({"type":"telnet", "host":"switch", "port":23}),
                serde_json::json!({"type":"vnc", "host":"screen", "port":5900}),
                serde_json::json!({"type":"local_terminal", "shell_path":"/bin/bash", "working_dir":"/srv/app"}),
                serde_json::json!({"type":"serial", "port_name":"COM3", "baud_rate":115200}),
            ] {
                let mut entry = connection(group_id);
                entry.config = serde_json::from_value(config).unwrap();
                connections.push(entry);
            }
        }
        let mut queries: Vec<String> = [
            "",
            " \t\n",
            "GATE",
            "alice",
            "host.example",
            "SSH",
            "bastion",
            "unique-ident",
            "production",
            "上海",
            "cycle",
            "missing",
            "absent",
            "prod 2222",
            "2128",
            ":2222",
        ]
        .into_iter()
        .map(str::to_string)
        .collect();
        for entry in &connections {
            let text = format!(
                "{} {} {} {} {}",
                entry.name,
                entry.endpoint(),
                entry.kind_label(),
                entry.description.as_deref().unwrap_or_default(),
                entry.id
            );
            let chars: Vec<_> = text.chars().collect();
            for start in 0..chars.len() {
                queries.push(chars[start..(start + 14).min(chars.len())].iter().collect());
            }
        }
        for query in queries {
            let search = ConnectionSearch::new(&query, &groups);
            for entry in &connections {
                assert_eq!(
                    search.rank_connection(entry).is_some(),
                    legacy_matches(entry, &groups, &query),
                    "query {query:?}, kind {}, group {:?}",
                    entry.kind_label(),
                    entry.group_id
                );
            }
        }
    }

    #[test]
    fn direct_fields_outrank_auxiliary_fields_without_adding_duplicate_hits() {
        let mut entry = connection(None);
        entry.name = "prod".into();
        entry.description = Some("prod".into());
        entry.id = "prod".into();
        let search = ConnectionSearch::new("prod", &[]);
        assert_eq!(
            search.rank_connection(&entry),
            Some(ConnectionSearchRank(1000))
        );
        entry.name = "production".into();
        assert_eq!(
            search.rank_connection(&entry),
            Some(ConnectionSearchRank(900))
        );
        entry.name = "my production".into();
        assert_eq!(
            search.rank_connection(&entry),
            Some(ConnectionSearchRank(800))
        );
        entry.name = "Gateway".into();
        assert_eq!(
            search.rank_connection(&entry),
            Some(ConnectionSearchRank(150))
        );
        entry.description = None;
        assert_eq!(
            search.rank_connection(&entry),
            Some(ConnectionSearchRank(75))
        );
        assert_eq!(
            ConnectionSearch::new("host.example.com", &[]).rank_connection(&entry),
            Some(ConnectionSearchRank(1000))
        );
        assert_eq!(
            ConnectionSearch::new("ssh", &[]).rank_connection(&entry),
            Some(ConnectionSearchRank(250))
        );
        let cross_field = format!("{} {}", entry.name, entry.endpoint());
        assert_eq!(
            ConnectionSearch::new(&cross_field, &[]).rank_connection(&entry),
            Some(ConnectionSearchRank(1))
        );
    }

    #[test]
    fn strongest_group_hit_reaches_all_descendants_even_with_cycles() {
        let groups = vec![
            group("a", "prod", Some("b")),
            group("b", "my prod", Some("a")),
            group("c", "production", Some("b")),
            group("d", "child", Some("c")),
        ];
        let search = ConnectionSearch::new("prod", &groups);
        for id in ["a", "b", "c", "d"] {
            assert_eq!(search.group_rank(id), Some(ConnectionSearchRank(350)));
            assert_eq!(
                search.rank_connection(&connection(Some(id))),
                Some(ConnectionSearchRank(350))
            );
        }
    }
}
