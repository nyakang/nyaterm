//! Terminal action-link matchers (Tauri actionLinksMatcher parity).

use nyaterm_core::ActionLinksMatcherSettings;
use nyaterm_core::mac_address::{MacAddress, MacAddressFormat};
use regex::Regex;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ActionLinkKind {
    Url,
    Ip,
    HostPort,
    Archive,
    Mac,
}

impl ActionLinkKind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Url => "URL",
            Self::Ip => "IPv4",
            Self::HostPort => "Host:Port",
            Self::Archive => "Archive",
            Self::Mac => "MAC",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActionLinkMatch {
    pub(crate) kind: ActionLinkKind,
    pub(crate) text: String,
    pub(crate) value: String,
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) host: Option<String>,
    pub(crate) port: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActionLinkAction {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) target: ActionLinkTarget,
    pub(crate) mac_format: Option<MacAddressFormat>,
    pub(crate) is_default: bool,
}

/// Exactly one execution target, shared by menu and modifier-click dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ActionLinkTarget {
    ExecuteCommand(String),
    OpenUrl(String),
    CopyText(String),
    PasteText(String),
}

impl ActionLinkTarget {
    pub(crate) fn text(&self) -> &str {
        match self {
            Self::ExecuteCommand(text)
            | Self::OpenUrl(text)
            | Self::CopyText(text)
            | Self::PasteText(text) => text,
        }
    }
}

fn mac_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
        r"(?i)(?:[0-9a-f]{2}(?:[:-][0-9a-f]{2}){5}|[0-9a-f]{4}(?:[.-][0-9a-f]{4}){2}|[0-9a-f]{12})"
    ).expect("mac regex")
    })
}

fn mac_token_continuation(ch: char) -> bool {
    ch.is_alphanumeric() || matches!(ch, '_' | ':' | '-' | '.')
}

fn ipv4_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"\b(?:(?:25[0-5]|2[0-4]\d|1?\d?\d)\.){3}(?:25[0-5]|2[0-4]\d|1?\d?\d)\b")
            .expect("ipv4 regex")
    })
}

fn host_port_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)\b((?:[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z]{2,}|(?:(?:25[0-5]|2[0-4]\d|1?\d?\d)\.){3}(?:25[0-5]|2[0-4]\d|1?\d?\d)|localhost):(\d{1,5})\b",
        )
        .expect("host:port regex")
    })
}

fn archive_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r#"(?i)\b(?:[^\s"'`<>|]+?\.(?:zip|rar|7z|tar\.gz|tgz|tar\.bz2|tbz2|tar\.xz|txz))\b"#,
        )
        .expect("archive regex")
    })
}

fn url_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"(?i)\bhttps?://[^\s"'<>()]+"#).expect("url regex"))
}

const SOURCE_EXTS: &[&str] = &[
    "py", "js", "jsx", "ts", "tsx", "mjs", "cjs", "java", "kt", "kts", "go", "rs", "rb", "php",
    "c", "cc", "cpp", "cxx", "h", "hpp", "cs", "sh", "bash", "zsh", "fish", "ps1", "bat", "cmd",
    "log", "txt", "md", "json", "yaml", "yml", "xml", "toml", "ini",
];

fn looks_like_file_host(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    let ext = host.rsplit('.').next().unwrap_or("");
    SOURCE_EXTS.contains(&ext)
}

fn is_valid_ipv4(text: &str) -> bool {
    let parts: Vec<_> = text.split('.').collect();
    if parts.len() != 4 {
        return false;
    }
    parts
        .iter()
        .all(|part| part.parse::<u16>().ok().is_some_and(|value| value <= 255))
}

fn is_valid_archive_name(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    [
        ".zip", ".rar", ".7z", ".tar.gz", ".tgz", ".tar.bz2", ".tbz2", ".tar.xz", ".txz",
    ]
    .iter()
    .any(|suffix| lower.ends_with(suffix))
}

fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '/' | ':' | '@'))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn overlaps(a: (usize, usize), b: (usize, usize)) -> bool {
    a.0 < b.1 && b.0 < a.1
}

/// Scan plain text for action-link entities (priority: url > host:port > mac > ipv4 > archive).
pub(crate) fn find_action_links(
    text: &str,
    matchers: &ActionLinksMatcherSettings,
    include_url: bool,
) -> Vec<ActionLinkMatch> {
    let mut matches = Vec::new();

    if matchers.host_port {
        for cap in host_port_re().captures_iter(text) {
            let full = cap.get(0).expect("full");
            let host = cap.get(1).map(|m| m.as_str()).unwrap_or("");
            let port = cap.get(2).map(|m| m.as_str()).unwrap_or("");
            if looks_like_file_host(host) {
                continue;
            }
            if let Ok(port_num) = port.parse::<u32>() {
                if port_num == 0 || port_num > 65535 {
                    continue;
                }
            } else {
                continue;
            }
            matches.push(ActionLinkMatch {
                kind: ActionLinkKind::HostPort,
                text: full.as_str().to_string(),
                value: full.as_str().to_string(),
                start: full.start(),
                end: full.end(),
                host: Some(host.to_string()),
                port: Some(port.to_string()),
            });
        }
    }

    if include_url {
        for m in url_re().find_iter(text) {
            let mut end = m.end();
            // Trim trailing punctuation common in terminal logs.
            while end > m.start() {
                let last = text[m.start()..end].chars().last().unwrap_or(' ');
                if matches!(last, '.' | ',' | ';' | ')' | ']' | '}' | '"' | '\'') {
                    end -= last.len_utf8();
                } else {
                    break;
                }
            }
            if end <= m.start() {
                continue;
            }
            let slice = &text[m.start()..end];
            matches.push(ActionLinkMatch {
                kind: ActionLinkKind::Url,
                text: slice.to_string(),
                value: slice.to_string(),
                start: m.start(),
                end,
                host: None,
                port: None,
            });
        }
    }

    if matchers.mac {
        for m in mac_re().find_iter(text) {
            // Reject whole malformed tokens rather than extracting a valid suffix.
            if text[..m.start()]
                .chars()
                .next_back()
                .is_some_and(mac_token_continuation)
                || text[m.end()..]
                    .chars()
                    .next()
                    .is_some_and(mac_token_continuation)
                || m.as_str().parse::<MacAddress>().is_err()
            {
                continue;
            }
            matches.push(ActionLinkMatch {
                kind: ActionLinkKind::Mac,
                text: m.as_str().to_owned(),
                value: m.as_str().to_owned(),
                start: m.start(),
                end: m.end(),
                host: None,
                port: None,
            });
        }
    }

    if matchers.ipv4 {
        for m in ipv4_re().find_iter(text) {
            if !is_valid_ipv4(m.as_str()) {
                continue;
            }
            matches.push(ActionLinkMatch {
                kind: ActionLinkKind::Ip,
                text: m.as_str().to_string(),
                value: m.as_str().to_string(),
                start: m.start(),
                end: m.end(),
                host: None,
                port: None,
            });
        }
    }

    if matchers.archive {
        for m in archive_re().find_iter(text) {
            if !is_valid_archive_name(m.as_str()) {
                continue;
            }
            matches.push(ActionLinkMatch {
                kind: ActionLinkKind::Archive,
                text: m.as_str().to_string(),
                value: m.as_str().to_string(),
                start: m.start(),
                end: m.end(),
                host: None,
                port: None,
            });
        }
    }

    // Keep complete URLs instead of the host:port or IPv4 embedded in them.
    matches.sort_by(|a, b| {
        let rank = |kind: ActionLinkKind| match kind {
            ActionLinkKind::Url => 0,
            ActionLinkKind::HostPort => 1,
            ActionLinkKind::Mac => 2,
            ActionLinkKind::Ip => 3,
            ActionLinkKind::Archive => 4,
        };
        rank(a.kind)
            .cmp(&rank(b.kind))
            .then(a.start.cmp(&b.start))
            .then(b.end.cmp(&a.end))
    });

    let mut accepted = Vec::new();
    for item in matches {
        if accepted
            .iter()
            .any(|prev: &ActionLinkMatch| overlaps((prev.start, prev.end), (item.start, item.end)))
        {
            continue;
        }
        accepted.push(item);
    }
    accepted.sort_by_key(|item| item.start);
    accepted
}

pub(crate) fn match_at_offset(
    text: &str,
    offset: usize,
    matchers: &ActionLinksMatcherSettings,
) -> Option<ActionLinkMatch> {
    find_action_links(text, matchers, true)
        .into_iter()
        .find(|item| offset >= item.start && offset < item.end)
}

pub(crate) fn actions_for_match(item: &ActionLinkMatch) -> Vec<ActionLinkAction> {
    match item.kind {
        ActionLinkKind::Mac => {
            let Ok(mac) = item.value.parse::<MacAddress>() else {
                return Vec::new();
            };
            MacAddressFormat::ALL
                .into_iter()
                .enumerate()
                .flat_map(|(index, format)| {
                    let text = mac.format(format);
                    [
                        ActionLinkAction {
                            id: format!("mac-copy-{index}"),
                            label: rust_i18n::t!("terminal.macCopy").to_string(),
                            target: ActionLinkTarget::CopyText(text.clone()),
                            mac_format: Some(format),
                            is_default: format == MacAddressFormat::OctetColon,
                        },
                        ActionLinkAction {
                            id: format!("mac-paste-{index}"),
                            label: rust_i18n::t!("terminal.macInsert").to_string(),
                            target: ActionLinkTarget::PasteText(text),
                            mac_format: Some(format),
                            is_default: false,
                        },
                    ]
                })
                .collect()
        }
        ActionLinkKind::Url => vec![
            ActionLinkAction {
                id: "open".into(),
                label: "Open Link".into(),
                mac_format: None,
                target: ActionLinkTarget::OpenUrl(item.value.clone()),
                is_default: true,
            },
            ActionLinkAction {
                id: "curl".into(),
                label: "curl".into(),
                target: ActionLinkTarget::ExecuteCommand(format!(
                    "curl {}",
                    shell_quote(&item.value)
                )),
                mac_format: None,
                is_default: false,
            },
        ],
        ActionLinkKind::Ip => vec![
            ActionLinkAction {
                id: "ping".into(),
                label: "Ping".into(),
                target: ActionLinkTarget::ExecuteCommand(format!("ping {}", item.value)),
                mac_format: None,
                is_default: true,
            },
            ActionLinkAction {
                id: "traceroute".into(),
                label: "Traceroute".into(),
                target: ActionLinkTarget::ExecuteCommand(format!("traceroute {}", item.value)),
                mac_format: None,
                is_default: false,
            },
            ActionLinkAction {
                id: "ssh".into(),
                label: "SSH".into(),
                target: ActionLinkTarget::ExecuteCommand(format!("ssh {}", item.value)),
                mac_format: None,
                is_default: false,
            },
            ActionLinkAction {
                id: "curl-http".into(),
                label: "curl http://".into(),
                target: ActionLinkTarget::ExecuteCommand(format!("curl http://{}", item.value)),
                mac_format: None,
                is_default: false,
            },
        ],
        ActionLinkKind::HostPort => {
            let host = item
                .host
                .clone()
                .unwrap_or_else(|| item.value.split(':').next().unwrap_or("").to_string());
            let port = item
                .port
                .clone()
                .unwrap_or_else(|| item.value.split(':').nth(1).unwrap_or("").to_string());
            vec![
                ActionLinkAction {
                    id: "curl-http".into(),
                    label: "curl http://".into(),
                    target: ActionLinkTarget::ExecuteCommand(format!("curl http://{}", item.value)),
                    mac_format: None,
                    is_default: true,
                },
                ActionLinkAction {
                    id: "curl-https".into(),
                    label: "curl https://".into(),
                    target: ActionLinkTarget::ExecuteCommand(format!(
                        "curl https://{}",
                        item.value
                    )),
                    mac_format: None,
                    is_default: false,
                },
                ActionLinkAction {
                    id: "nc".into(),
                    label: "nc -vz".into(),
                    target: ActionLinkTarget::ExecuteCommand(format!("nc -vz {host} {port}")),
                    mac_format: None,
                    is_default: false,
                },
                ActionLinkAction {
                    id: "telnet".into(),
                    label: "Telnet".into(),
                    target: ActionLinkTarget::ExecuteCommand(format!("telnet {host} {port}")),
                    mac_format: None,
                    is_default: false,
                },
            ]
        }
        ActionLinkKind::Archive => {
            let f = shell_quote(&item.value);
            let lower = item.value.to_ascii_lowercase();
            let extract = if lower.ends_with(".zip") {
                Some(format!("unzip {f}"))
            } else if lower.ends_with(".rar") {
                Some(format!("unrar x {f}"))
            } else if lower.ends_with(".7z") {
                Some(format!("7z x {f}"))
            } else if lower.ends_with(".tar.gz") || lower.ends_with(".tgz") {
                Some(format!("tar -xzvf {f}"))
            } else if lower.ends_with(".tar.bz2") || lower.ends_with(".tbz2") {
                Some(format!("tar -xjf {f}"))
            } else if lower.ends_with(".tar.xz") || lower.ends_with(".txz") {
                Some(format!("tar -xJf {f}"))
            } else {
                None
            };
            let list = if lower.ends_with(".zip") {
                Some(format!("unzip -l {f}"))
            } else if lower.ends_with(".rar") {
                Some(format!("unrar l {f}"))
            } else if lower.ends_with(".7z") {
                Some(format!("7z l {f}"))
            } else if lower.ends_with(".tar.gz")
                || lower.ends_with(".tgz")
                || lower.ends_with(".tar.bz2")
                || lower.ends_with(".tbz2")
                || lower.ends_with(".tar.xz")
                || lower.ends_with(".txz")
            {
                Some(format!("tar -tf {f}"))
            } else {
                None
            };
            let mut actions = Vec::new();
            if let Some(command) = extract {
                actions.push(ActionLinkAction {
                    id: "extract".into(),
                    label: "Extract".into(),
                    target: ActionLinkTarget::ExecuteCommand(command),
                    mac_format: None,
                    is_default: true,
                });
            }
            if let Some(command) = list {
                actions.push(ActionLinkAction {
                    id: "list".into(),
                    label: "List contents".into(),
                    target: ActionLinkTarget::ExecuteCommand(command),
                    mac_format: None,
                    is_default: actions.is_empty(),
                });
            }
            actions
        }
    }
}

#[cfg(test)]
mod tests {
    use nyaterm_core::ActionLinksMatcherSettings;
    use nyaterm_core::mac_address::MacAddressFormat;

    use super::{
        ActionLinkKind, ActionLinkTarget, actions_for_match, find_action_links, match_at_offset,
    };

    #[test]
    fn mac_formats_share_five_copy_and_paste_candidates_and_one_safe_default() {
        for input in [
            "aa-bb-cc-dd-ee-ff",
            "aabb-ccdd-eeff",
            "aabb.ccdd.eeff",
            "aa:bb:cc:dd:ee:ff",
            "aabbccddeeff",
        ] {
            let found = find_action_links(input, &ActionLinksMatcherSettings::default(), true);
            assert_eq!(found.len(), 1, "{input}");
            assert_eq!(found[0].kind, ActionLinkKind::Mac);
            assert_eq!(found[0].text, input);
            let actions = actions_for_match(&found[0]);
            assert_eq!(actions.len(), 10);
            assert_eq!(actions.iter().filter(|action| action.is_default).count(), 1);
            assert_eq!(
                actions
                    .iter()
                    .find(|action| action.is_default)
                    .unwrap()
                    .target,
                ActionLinkTarget::CopyText("AA:BB:CC:DD:EE:FF".into())
            );
            for (format, expected) in MacAddressFormat::ALL.into_iter().zip([
                "AA-BB-CC-DD-EE-FF",
                "AABB-CCDD-EEFF",
                "AABB.CCDD.EEFF",
                "AA:BB:CC:DD:EE:FF",
                "AABBCCDDEEFF",
            ]) {
                let candidates = actions
                    .iter()
                    .filter(|action| action.mac_format == Some(format))
                    .collect::<Vec<_>>();
                assert_eq!(candidates.len(), 2);
                assert_eq!(
                    candidates[0].target,
                    ActionLinkTarget::CopyText(expected.into())
                );
                assert_eq!(
                    candidates[1].target,
                    ActionLinkTarget::PasteText(expected.into())
                );
            }
        }
    }

    #[test]
    fn mac_matching_rejects_malformed_tokens_and_identifier_substrings() {
        for text in [
            "AA-BB:CC-DD:EE-FF",
            "AABB.CCDD-EEFF",
            "AA:BB:CC:DD:EE:FF:00:11",
            "AABBCCDDEEFF00",
            "0AABBCCDDEEFF",
            "id_AABBCCDDEEFF",
            "AABBCCDDEEFF_id",
            "gAABBCCDDEEFF",
            "AABBCCDDEEFFg",
            "AA-BB-CC-DD-EE-FF-00",
            "AABB-CCDD-EEFF-0011",
            "éAABBCCDDEEFF",
            "AABBCCDDEEFF界",
            "AA:BB:CC:DD:EE:GG",
            "AABB.CCDD.EEF",
        ] {
            assert!(
                !find_action_links(text, &ActionLinksMatcherSettings::default(), true)
                    .iter()
                    .any(|item| item.kind == ActionLinkKind::Mac),
                "{text}"
            );
        }
    }

    #[test]
    fn mac_matches_keep_utf8_offsets_and_url_priority_and_respect_the_switch() {
        let text = "设备 [905d.7c16.b6d9], (90:5d:7c:16:b6:d9) https://example.com/AABBCCDDEEFF";
        let mut matchers = ActionLinksMatcherSettings::default();
        let matches = find_action_links(text, &matchers, true);
        assert_eq!(
            matches.iter().map(|item| item.kind).collect::<Vec<_>>(),
            [
                ActionLinkKind::Mac,
                ActionLinkKind::Mac,
                ActionLinkKind::Url
            ]
        );
        for item in &matches {
            assert_eq!(&text[item.start..item.end], item.text);
            assert_eq!(
                match_at_offset(text, item.start, &matchers).as_ref(),
                Some(item)
            );
            assert_eq!(
                match_at_offset(text, item.end - 1, &matchers).as_ref(),
                Some(item)
            );
        }
        matchers.mac = false;
        assert_eq!(find_action_links(text, &matchers, true).len(), 1);
    }

    #[test]
    fn urls_take_priority_over_embedded_hosts_ips_and_archives() {
        let matchers = ActionLinksMatcherSettings::default();
        for url in [
            "http://123.123.123.123:123/sdfsdf",
            "https://123.123.123.123:8443/path",
            "http://example.com:8080/path",
            "https://localhost:8443/archive.tar.gz?download=1#contents",
        ] {
            let line = format!("curl {url}");
            let found = find_action_links(&line, &matchers, true);
            assert_eq!(found.len(), 1, "{line}");
            let item = &found[0];
            assert_eq!(item.kind, ActionLinkKind::Url, "{line}");
            assert_eq!(item.text, url);
            assert_eq!(item.value, url);
            assert_eq!((item.start, item.end), (5, line.len()));
            // Hovering the scheme, host, port, or path must select the same URL.
            for offset in item.start..item.end {
                assert_eq!(
                    match_at_offset(&line, offset, &matchers).as_ref(),
                    Some(item)
                );
            }
            let actions = actions_for_match(item);
            let default = actions.iter().find(|action| action.is_default).unwrap();
            assert_eq!(default.target, ActionLinkTarget::OpenUrl(url.into()));
            let curl = actions.iter().find(|action| action.id == "curl").unwrap();
            let expected_command = if url.contains('?') {
                format!("curl '{url}'")
            } else {
                format!("curl {url}")
            };
            assert_eq!(
                curl.target,
                ActionLinkTarget::ExecuteCommand(expected_command)
            );
        }
    }

    #[test]
    fn bare_host_ports_keep_their_port_actions() {
        let matchers = ActionLinksMatcherSettings::default();
        for host in ["123.123.123.123", "example.com", "localhost"] {
            let value = format!("{host}:123");
            let found = find_action_links(&value, &matchers, true);
            assert_eq!(found.len(), 1);
            let item = &found[0];
            assert_eq!(item.kind, ActionLinkKind::HostPort);
            assert_eq!(item.value, value);
            assert_eq!(item.host.as_deref(), Some(host));
            assert_eq!(item.port.as_deref(), Some("123"));
            let actions = actions_for_match(item);
            let default = actions.iter().find(|action| action.is_default).unwrap();
            assert_eq!(default.id, "curl-http");
            assert_eq!(
                default.target,
                ActionLinkTarget::ExecuteCommand(format!("curl http://{value}"))
            );
            assert!(actions.iter().any(|action| action.id == "nc"));
            assert!(actions.iter().any(|action| action.id == "telnet"));
        }
    }

    #[test]
    fn url_priority_preserves_separate_action_links_in_text_order() {
        let matchers = ActionLinksMatcherSettings::default();
        let found = find_action_links(
            "192.168.1.1:22 https://example.com:8443/app.zip 10.0.0.1 backup.tar.gz",
            &matchers,
            true,
        );
        let entities = found
            .iter()
            .map(|item| (item.kind, item.value.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(
            entities,
            vec![
                (ActionLinkKind::HostPort, "192.168.1.1:22"),
                (ActionLinkKind::Url, "https://example.com:8443/app.zip"),
                (ActionLinkKind::Ip, "10.0.0.1"),
                (ActionLinkKind::Archive, "backup.tar.gz"),
            ]
        );
    }

    #[test]
    fn detects_ip_and_actions() {
        let matchers = ActionLinksMatcherSettings::default();
        let found = find_action_links("gateway 192.168.1.1 up", &matchers, true);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, ActionLinkKind::Ip);
        let actions = actions_for_match(&found[0]);
        assert!(actions.iter().any(|a| a.id == "ping"));
    }

    #[test]
    fn detects_host_port() {
        let matchers = ActionLinksMatcherSettings::default();
        let found = find_action_links("listening on api.example.com:8080", &matchers, true);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, ActionLinkKind::HostPort);
    }

    #[test]
    fn rejects_source_location_as_host_port() {
        let matchers = ActionLinksMatcherSettings::default();
        let found = find_action_links("error main.rs:42", &matchers, true);
        assert!(found.is_empty());
    }

    #[test]
    fn detects_archive() {
        let matchers = ActionLinksMatcherSettings::default();
        let found = find_action_links("download app.tar.gz now", &matchers, true);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, ActionLinkKind::Archive);
    }
}
