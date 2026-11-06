//! Shared matching rules for the saved-connection tree and its navigation.

use std::collections::{HashMap, HashSet};

use crate::models::{Group, SavedConnection};

pub fn normalize_connection_search_query(query: &str) -> String {
    query.trim().to_ascii_lowercase()
}

/// A group-name hit includes the entire subtree, even groups without connections.
pub struct ConnectionSearch {
    query: String,
    included_group_ids: HashSet<String>,
}

impl ConnectionSearch {
    pub fn new(query: &str, groups: &[Group]) -> Self {
        let query = normalize_connection_search_query(query);
        let mut included_group_ids = HashSet::new();
        if !query.is_empty() {
            let mut children: HashMap<&str, Vec<&str>> = HashMap::new();
            let mut pending = Vec::new();
            for group in groups {
                if let Some(parent_id) = group.parent_id.as_deref() {
                    children.entry(parent_id).or_default().push(&group.id);
                }
                if group.name.to_ascii_lowercase().contains(&query) {
                    pending.push(group.id.as_str());
                }
            }
            while let Some(id) = pending.pop() {
                // Visit each group once, including overlapping hits and cyclic catalogs.
                if included_group_ids.insert(id.to_owned())
                    && let Some(descendants) = children.get(id)
                {
                    pending.extend(descendants.iter().copied());
                }
            }
        }
        Self {
            query,
            included_group_ids,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.query.is_empty()
    }

    pub fn includes_group(&self, group_id: &str) -> bool {
        self.is_empty() || self.included_group_ids.contains(group_id)
    }

    pub fn matches_connection(&self, connection: &SavedConnection) -> bool {
        if self.is_empty()
            || connection
                .group_id
                .as_deref()
                .is_some_and(|id| self.includes_group(id))
        {
            return true;
        }
        let haystack = format!(
            "{} {} {} {} {}",
            connection.name,
            connection.endpoint(),
            connection.kind_label(),
            connection.description.as_deref().unwrap_or_default(),
            connection.id
        )
        .to_ascii_lowercase();
        haystack.contains(&self.query)
    }
}

#[cfg(test)]
mod tests {
    use crate::connection_search::{ConnectionSearch, normalize_connection_search_query};
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
}
