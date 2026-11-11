//! Field-aware quick switch matching. Names remain fuzzy, while network addresses
//! and ports are matched within their own fields rather than across display text.
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};

use nyaterm_core::{ConnectionType, fuzzy_search_items};

use crate::models::QuickSwitchItem;

const RESULT_LIMIT: usize = 50;

#[derive(Debug)]
enum SearchTerm {
    Text(String),
    Numeric(String),
    DottedAddress(String),
    Port(u16),
}

pub(super) fn filter_quick_switch_items(
    items: Vec<QuickSwitchItem>,
    query: &str,
) -> Vec<QuickSwitchItem> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return items.into_iter().take(RESULT_LIMIT).collect();
    }

    // Whitespace-separated terms are ANDed. Split "host:port" into separate
    // address and port constraints, but do not mistake bare IPv6 for a port.
    let terms = parse_terms(&query);
    let titles = items
        .iter()
        .enumerate()
        .map(|(index, item)| (item.title().to_string(), index.to_string()))
        .collect::<Vec<_>>();
    let title_refs = titles
        .iter()
        .map(|(title, id)| (title.as_str(), id.as_str()))
        .collect::<Vec<_>>();

    // Keep the existing fuzzy implementation for names only. Each token is
    // scored independently so a match cannot jump across unrelated fields.
    let fuzzy_scores = terms
        .iter()
        .map(|term| match term {
            SearchTerm::Text(text) => fuzzy_search_items(
                &title_refs,
                text,
                "sessionQuickSwitcher",
                items.len(),
                None,
                None,
            )
            .into_iter()
            .filter_map(|result| {
                result
                    .command
                    .parse::<usize>()
                    .ok()
                    .map(|index| (index, result.score))
            })
            .collect::<HashMap<_, _>>(),
            _ => HashMap::new(),
        })
        .collect::<Vec<_>>();

    let mut matched = items
        .into_iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let mut score = 0_u32;
            for (term, fuzzy) in terms.iter().zip(&fuzzy_scores) {
                let term_score = score_term(&item, term, fuzzy.get(&index).copied());
                if term_score == 0 {
                    return None;
                }
                score = score.saturating_add(term_score);
            }
            let active = matches!(
                &item,
                QuickSwitchItem::Session { active: true, .. }
                    | QuickSwitchItem::Pending { active: true, .. }
            );
            Some((score, active, index, item))
        })
        .collect::<Vec<_>>();

    // Relevance precedes active status; original catalog order breaks ties.
    matched.sort_by(|left, right| {
        right
            .0
            .cmp(&left.0)
            .then(right.1.cmp(&left.1))
            .then(left.2.cmp(&right.2))
    });
    matched
        .into_iter()
        .take(RESULT_LIMIT)
        .map(|(_, _, _, item)| item)
        .collect()
}

fn parse_terms(query: &str) -> Vec<SearchTerm> {
    let mut terms = Vec::new();
    for word in query.split_whitespace() {
        if let Some((host, port_text)) = word.rsplit_once(':') {
            let host = host.trim_start_matches('[').trim_end_matches(']');
            if !host.is_empty()
                && (host.contains('.') || word.starts_with('['))
                && let Ok(port) = port_text.parse::<u16>()
            {
                terms.push(classify_term(host));
                terms.push(SearchTerm::Port(port));
                continue;
            }
        }
        terms.push(classify_term(word));
    }
    terms
}

fn classify_term(word: &str) -> SearchTerm {
    if let Some(port) = word
        .strip_prefix(':')
        .and_then(|text| text.parse::<u16>().ok())
    {
        return SearchTerm::Port(port);
    }
    if word.bytes().all(|ch| ch.is_ascii_digit()) {
        return SearchTerm::Numeric(word.to_owned());
    }
    if word.contains('.') && word.bytes().all(|ch| ch.is_ascii_digit() || ch == b'.') {
        return SearchTerm::DottedAddress(word.to_owned());
    }
    SearchTerm::Text(word.to_owned())
}

fn score_term(item: &QuickSwitchItem, term: &SearchTerm, fuzzy: Option<u32>) -> u32 {
    let title = item.title().to_lowercase();
    let endpoint = item_endpoint(item);
    match term {
        SearchTerm::Port(port) => u32::from(endpoint.is_some_and(|(_, p)| p == *port)) * 900,
        SearchTerm::DottedAddress(query) => {
            let address_score = endpoint
                .map(|(host, _)| match_string(&host.to_lowercase(), query, 950, 800, 650))
                .unwrap_or(0);
            address_score.max(match_string(&title, query, 600, 550, 500))
        }
        SearchTerm::Numeric(query) => {
            let name_score = match_string(&title, query, 1000, 850, 700);
            let endpoint_score = endpoint
                .map(|(host, port)| {
                    let host_score = match host.parse::<Ipv4Addr>() {
                        Ok(_) => {
                            let digits = host
                                .chars()
                                .filter(char::is_ascii_digit)
                                .collect::<String>();
                            u32::from(digits.contains(query)) * 650
                        }
                        Err(_) => u32::from(host.contains(query)) * 550,
                    };
                    host_score.max(u32::from(query.parse::<u16>().ok() == Some(port)) * 720)
                })
                .unwrap_or(0);
            name_score.max(endpoint_score)
        }
        SearchTerm::Text(query) => {
            let name_score = match_string(&title, query, 1000, 850, 700);
            let host_score = endpoint
                .map(|(host, _)| {
                    let parsed_query = query.parse::<IpAddr>().ok();
                    let canonical_score = if parsed_query.is_some()
                        && parsed_query
                            == host
                                .trim_start_matches('[')
                                .trim_end_matches(']')
                                .parse::<IpAddr>()
                                .ok()
                    {
                        950
                    } else {
                        0
                    };
                    canonical_score.max(match_string(&host.to_lowercase(), query, 950, 800, 650))
                })
                .unwrap_or(0);
            let subtitle_score = u32::from(item.subtitle().to_lowercase().contains(query)) * 180;
            let detail_score = match item {
                QuickSwitchItem::Connection { connection, .. } => {
                    u32::from(
                        connection
                            .description
                            .as_deref()
                            .is_some_and(|detail| detail.to_lowercase().contains(query)),
                    ) * 160
                }
                QuickSwitchItem::Pending { search_detail, .. } => {
                    u32::from(
                        search_detail
                            .as_deref()
                            .is_some_and(|detail| detail.to_lowercase().contains(query)),
                    ) * 160
                }
                QuickSwitchItem::Session { .. } => 0,
            };
            // A mixed name/number query must not fuzzy-match scattered digits.
            let fuzzy_score = if query.bytes().any(|ch| ch.is_ascii_digit()) {
                0
            } else {
                fuzzy.map(|score| 200 + score.min(180)).unwrap_or(0)
            };
            name_score
                .max(host_score)
                .max(subtitle_score)
                .max(detail_score)
                .max(fuzzy_score)
        }
    }
}

fn match_string(text: &str, query: &str, exact: u32, prefix: u32, substring: u32) -> u32 {
    if text == query {
        exact
    } else if text.starts_with(query) {
        prefix
    } else if text.contains(query) {
        substring
    } else {
        0
    }
}

fn item_endpoint(item: &QuickSwitchItem) -> Option<(&str, u16)> {
    match item {
        QuickSwitchItem::Session { endpoint, .. } => {
            endpoint.as_ref().map(|(host, port)| (host.as_str(), *port))
        }
        QuickSwitchItem::Connection { connection, .. } => match &connection.config {
            ConnectionType::Ssh { host, port, .. }
            | ConnectionType::Telnet { host, port, .. }
            | ConnectionType::Rdp { host, port, .. }
            | ConnectionType::Vnc { host, port, .. } => Some((host, *port)),
            ConnectionType::Adb {
                device: nyaterm_core::models::android::AdbDeviceSelector::Tcp { host, port },
                ..
            } => Some((host, *port)),
            ConnectionType::Adb { .. }
            | ConnectionType::LocalTerminal { .. }
            | ConnectionType::Serial { .. } => None,
        },
        QuickSwitchItem::Pending { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::filter_quick_switch_items;
    use crate::models::QuickSwitchItem;

    fn session(title: &str, host: Option<&str>, port: u16) -> QuickSwitchItem {
        QuickSwitchItem::Session {
            id: title.into(),
            title: title.into(),
            subtitle: "SSH - abc123".into(),
            active: false,
            endpoint: host.map(|host| (host.into(), port)),
        }
    }

    fn titles(items: Vec<QuickSwitchItem>, query: &str) -> Vec<String> {
        filter_quick_switch_items(items, query)
            .iter()
            .map(|item| item.title().to_owned())
            .collect()
    }

    #[test]
    fn numeric_ip_matching_is_contiguous_and_does_not_fallback_to_fuzzy() {
        let items = vec![
            session("target", Some("10.26.21.28"), 22),
            session("other", Some("10.26.113.28"), 22),
        ];
        assert_eq!(titles(items, "2128"), ["target"]);
        assert!(titles(vec![session("other", Some("10.26.113.28"), 22)], "2128").is_empty());
        assert_eq!(
            titles(vec![session("second", Some("192.168.2.128"), 22)], "2128"),
            ["second"]
        );
    }

    #[test]
    fn dotted_address_preserves_segment_boundaries() {
        let items = vec![
            session("target", Some("10.26.21.28"), 22),
            session("false-positive", Some("10.2.62.1"), 22),
        ];
        assert_eq!(titles(items, "10.26.21"), ["target"]);
    }

    #[test]
    fn digits_do_not_match_across_unrelated_fields_or_session_ids() {
        let mut split = session("node2", None, 22);
        if let QuickSwitchItem::Session { subtitle, .. } = &mut split {
            *subtitle = "128".into();
        }
        let items = vec![
            split,
            session("node2128", None, 22),
            session("regular", Some("10.26.113.28"), 22),
        ];
        assert_eq!(titles(items, "2128"), ["node2128"]);
    }

    #[test]
    fn port_matches_exactly_and_supports_explicit_syntax() {
        let items = vec![
            session("port22", Some("10.1.1.1"), 22),
            session("port2222", Some("10.1.1.2"), 2222),
        ];
        assert_eq!(titles(items.clone(), ":2222"), ["port2222"]);
        assert_eq!(titles(items, "10.1.1.2:2222"), ["port2222"]);
    }

    #[test]
    fn name_fuzzy_and_multiple_and_terms_are_supported() {
        let items = vec![
            session("production", Some("10.26.21.28"), 22),
            session("proxy", Some("10.26.113.28"), 22),
        ];
        assert_eq!(titles(items.clone(), "prd"), ["production"]);
        assert_eq!(titles(items, "prod 2128"), ["production"]);
    }

    #[test]
    fn mixed_name_and_digits_never_fuzzy_match_scattered_numbers() {
        let items = vec![
            session("node2x1x2x8", None, 22),
            session("node2128", None, 22),
        ];
        assert_eq!(titles(items, "node2128"), ["node2128"]);
    }

    #[test]
    fn relevance_then_active_then_catalog_order() {
        let mut active = session("prod", None, 22);
        if let QuickSwitchItem::Session { active: flag, .. } = &mut active {
            *flag = true;
        }
        let items = vec![
            session("production", None, 22),
            active,
            session("product", None, 22),
        ];
        assert_eq!(titles(items, "prod"), ["prod", "production", "product"]);
    }

    #[test]
    fn empty_query_preserves_catalog_order_and_nonempty_limits_results() {
        let items = (0..60)
            .map(|i| session(&format!("node-{i}"), None, 22))
            .collect::<Vec<_>>();
        assert_eq!(filter_quick_switch_items(items.clone(), " ").len(), 50);
        assert_eq!(filter_quick_switch_items(items, "node").len(), 50);
    }

    #[test]
    fn ipv6_full_address_matches_equivalent_canonical_form() {
        assert_eq!(
            titles(
                vec![session("ipv6", Some("2001:0db8::1"), 22)],
                "2001:db8::1"
            ),
            ["ipv6"]
        );
    }
}
