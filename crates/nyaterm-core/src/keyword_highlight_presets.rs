//! Theme-aware terminal semantic presets; persisted user rules retain their legacy format.
//! Patterns are adapted for the Rust `regex` crate (no look-around).

use crate::models::KeywordHighlightRule;
use std::collections::HashMap;

/// Resolved rule with a single theme-selected color (runtime paint).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedKeywordHighlightRule {
    pub id: String,
    pub name: String,
    pub patterns: Vec<String>,
    pub color: String,
    pub enabled: bool,
}

/// Built-ins declare their matcher instead of guessing from regex punctuation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinKeywordMatcher {
    LiteralWords,
    Regex,
    Structured,
}

pub fn builtin_keyword_matcher(id: &str) -> Option<BuiltinKeywordMatcher> {
    if !builtin_keyword_rule_ids().contains(&id) {
        return None;
    }
    Some(match id {
        "builtin-error"
        | "builtin-warn"
        | "builtin-success"
        | "builtin-info"
        | "builtin-debug"
        | "builtin-constant"
        | "builtin-net-interface-up"
        | "builtin-net-interface-down"
        | "builtin-net-admin-down"
        | "builtin-net-standby"
        | "builtin-net-acl-permit"
        | "builtin-net-acl-deny" => BuiltinKeywordMatcher::LiteralWords,
        "builtin-command" | "builtin-path" | "builtin-permissions" | "builtin-prompt" => {
            BuiltinKeywordMatcher::Structured
        }
        _ => BuiltinKeywordMatcher::Regex,
    })
}

/// Stable settings catalog. The semantic matcher assigns explicit overlap priorities.
pub fn builtin_keyword_rule_ids() -> &'static [&'static str] {
    &[
        "builtin-command",
        "builtin-path",
        "builtin-permissions",
        "builtin-url",
        "builtin-version",
        "builtin-address",
        "builtin-size",
        "builtin-string",
        "builtin-option",
        "builtin-uuid",
        "builtin-datetime",
        "builtin-error",
        "builtin-warn",
        "builtin-success",
        "builtin-info",
        "builtin-debug",
        "builtin-duration",
        "builtin-constant",
        "builtin-number",
        "builtin-prompt",
        "builtin-operator",
        // Network-device CLI vocabulary (H3C / Huawei / Cisco / Ruijie / Aruba).
        "builtin-net-interface",
        "builtin-net-vlan",
        "builtin-net-interface-up",
        "builtin-net-interface-down",
        "builtin-net-admin-down",
        "builtin-net-standby",
        "builtin-net-acl-permit",
        "builtin-net-acl-deny",
        "builtin-net-wildcard-mask",
    ]
}

pub fn builtin_keyword_rule_label(id: &str) -> &'static str {
    match id {
        "builtin-command" => "Command",
        "builtin-path" => "Path",
        "builtin-permissions" => "Permissions",
        "builtin-url" => "URL",
        "builtin-version" => "Version",
        "builtin-address" => "Address",
        "builtin-size" => "Size",
        "builtin-string" => "String",
        "builtin-option" => "Option",
        "builtin-uuid" => "UUID",
        "builtin-datetime" => "DateTime",
        "builtin-error" => "Error",
        "builtin-warn" => "Warning",
        "builtin-success" => "Success",
        "builtin-info" => "Info",
        "builtin-debug" => "Debug",
        "builtin-duration" => "Duration",
        "builtin-constant" => "Constant",
        "builtin-number" => "Number",
        "builtin-prompt" => "Prompt",
        "builtin-operator" => "Operator",
        "builtin-net-interface" => "Interface",
        "builtin-net-vlan" => "VLAN",
        "builtin-net-interface-up" => "Interface UP",
        "builtin-net-interface-down" => "Interface DOWN",
        "builtin-net-admin-down" => "Administratively down",
        "builtin-net-standby" => "Standby",
        "builtin-net-acl-permit" => "ACL permit",
        "builtin-net-acl-deny" => "ACL deny",
        "builtin-net-wildcard-mask" => "Wildcard mask",
        _ => "Builtin",
    }
}

pub fn builtin_keyword_rule_swatch(id: &str, is_dark: bool) -> &'static str {
    let dark = is_dark;
    match id {
        "builtin-error" => {
            if dark {
                "#f44747"
            } else {
                "#cf222e"
            }
        }
        "builtin-warn" => {
            if dark {
                "#cd9731"
            } else {
                "#9a6700"
            }
        }
        "builtin-success" => {
            if dark {
                "#32cd32"
            } else {
                "#116329"
            }
        }
        "builtin-info" => {
            if dark {
                "#6796e6"
            } else {
                "#0969da"
            }
        }
        "builtin-debug" => {
            if dark {
                "#b267e6"
            } else {
                "#8250df"
            }
        }
        "builtin-option" => {
            if dark {
                "#FD971F"
            } else {
                "#b04a00"
            }
        }
        "builtin-command" => {
            if dark {
                "#66D9EF"
            } else {
                "#007197"
            }
        }
        "builtin-path" => {
            if dark {
                "#E6DB74"
            } else {
                "#8a6700"
            }
        }
        "builtin-permissions" => {
            if dark {
                "#b267e6"
            } else {
                "#8250df"
            }
        }
        "builtin-datetime" => {
            if dark {
                "#A6E22E"
            } else {
                "#a58900"
            }
        }
        "builtin-number" => {
            if dark {
                "#AE81FF"
            } else {
                "#6f42c1"
            }
        }
        "builtin-constant" => {
            if dark {
                "#ffb86c"
            } else {
                "#cb4b16"
            }
        }
        "builtin-address" => {
            if dark {
                "#A6E22E"
            } else {
                "#1a7f37"
            }
        }
        "builtin-url" => {
            if dark {
                "#8be9fd"
            } else {
                "#2aa198"
            }
        }
        "builtin-uuid" => {
            if dark {
                "#ffb86c"
            } else {
                "#bc4c00"
            }
        }
        "builtin-string" => {
            if dark {
                "#E6DB74"
            } else {
                "#1a8c8c"
            }
        }
        "builtin-operator" => {
            if dark {
                "#A6E22E"
            } else {
                "#008573"
            }
        }
        "builtin-version" => {
            if dark {
                "#ff9e64"
            } else {
                "#b04a00"
            }
        }
        "builtin-size" => {
            if dark {
                "#2ac3de"
            } else {
                "#007197"
            }
        }
        "builtin-duration" => {
            if dark {
                "#E6DB74"
            } else {
                "#859900"
            }
        }
        "builtin-prompt" => {
            if dark {
                "#f92672"
            } else {
                "#e00862"
            }
        }
        "builtin-net-interface" => {
            if dark {
                "#7dd3fc"
            } else {
                "#1b7c83"
            }
        }
        "builtin-net-vlan" => {
            if dark {
                "#c084fc"
            } else {
                "#8250df"
            }
        }
        "builtin-net-interface-up" => {
            if dark {
                "#4ade80"
            } else {
                "#1a7f37"
            }
        }
        "builtin-net-interface-down" => {
            if dark {
                "#f87171"
            } else {
                "#cf222e"
            }
        }
        "builtin-net-admin-down" => {
            if dark {
                "#fbbf24"
            } else {
                "#9a6700"
            }
        }
        "builtin-net-standby" => {
            if dark {
                "#38bdf8"
            } else {
                "#0969da"
            }
        }
        "builtin-net-acl-permit" => {
            if dark {
                "#4ade80"
            } else {
                "#1a7f37"
            }
        }
        "builtin-net-acl-deny" => {
            if dark {
                "#f87171"
            } else {
                "#cf222e"
            }
        }
        "builtin-net-wildcard-mask" => {
            if dark {
                "#94a3b8"
            } else {
                "#6e7781"
            }
        }
        _ => {
            if dark {
                "#6796e6"
            } else {
                "#0969da"
            }
        }
    }
}

fn word_patterns(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| (*word).to_string()).collect()
}

const DECIMAL_NUMBER: &str = r"(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][-+]?\d+)?";
const HORIZONTAL_SPACE: &str = r"[\t\p{Zs}]";

fn builtin_patterns(id: &str) -> Vec<String> {
    match id {
        "builtin-url" => vec![r"(?i)\b(?:https?|ftp|wss?)://[-\w+&@#/%?=~_|!:,.;]*[-\w+&@#/%=~_|]".into()],
        "builtin-version" => vec![
            r"(?i)\bv\d+(?:\.\d+){1,2}(?:-[a-z0-9.-]+)?\b".into(),
            r"(?i)\b\d+(?:\.\d+){2,}(?:-[a-z0-9.-]+)?\b".into(),
            r"(?i)\b(?:latest|release|stable|beta|alpha|revision)\b".into(),
        ],
        "builtin-address" => vec![
            r"\b(?:25[0-5]|2[0-4]\d|[01]?\d\d?)\.(?:25[0-5]|2[0-4]\d|[01]?\d\d?)\.(?:25[0-5]|2[0-4]\d|[01]?\d\d?)\.(?:25[0-5]|2[0-4]\d|[01]?\d\d?)\b".into(),
            r"\b(?:[0-9A-Fa-f]{2}[:-]){5}[0-9A-Fa-f]{2}\b".into(),
        ],
        "builtin-size" => {
            vec![r"(?i)\b\d+(?:\.\d+)?\s*(?:[kmgtep]i?b|b|bytes?|[kmgtep]bps)\b".into()]
        }
        "builtin-string" => vec![
            r#""(?:[^"\\]|\\.)*""#.into(),
            r"'(?:[^'\\]|\\.)*'".into(),
        ],
        "builtin-option" => vec![r"(?i)--?[a-zA-Z][\w-]*".into()],
        "builtin-uuid" => {
            vec![r"(?i)\b[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}\b".into()]
        }
        "builtin-datetime" => vec![
            r"\b\d{4}[-/](?:0[1-9]|1[0-2])[-/](?:0[1-9]|[12]\d|3[01])(?:T(?:[01]\d|2[0-3])[-:][0-5]\d[-:][0-5]\d(?:\.\d{1,9})?(?:Z|[+-](?:[01]\d|2[0-3]):?[0-5]\d)?)?\b".into(),
            r"\b(?:[01]\d|2[0-3]):[0-5]\d(?::[0-5]\d)?(?:\.\d{1,9})?\b".into(),
            r"(?i)\b(?:Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sept?|Oct|Nov|Dec|Mon|Tues?|Wed|Thu|Thur|Fri|Sat|Sun)\b".into(),
            r"\b(?:0?[1-9]|1[0-2])[-/](?:0?[1-9]|[12]\d|3[01])[-/](?:\d{4}|\d{2})\b".into(),
            r"\b\d{4}(?:0[1-9]|1[0-2])(?:0[1-9]|[12]\d|3[01])T(?:[01]\d|2[0-3])[0-5]\d[0-5]\dZ?\b".into(),
        ],
        "builtin-error" => word_patterns(&[
            "access denied",
            "address already in use",
            "authentication failed",
            "broken pipe",
            "cannot allocate memory",
            "command not found",
            "connection refused",
            "connection reset by peer",
            "connection timed out",
            "error",
            "fail", "failed", "failure",
            "fatal",
            "exception",
            "host key verification failed",
            "module not found",
            "network is unreachable",
            "no route to host",
            "no space left on device",
            "operation timed out",
            "out of memory",
            "panic",
            "permission denied",
            "port already in use",
            "segmentation fault",
            "critical",
            "traceback",
            "unable to connect",
            "bad", "denied", "disabled", "incorrect", "invalid",
            "refused", "unknown", "unsupported", "wrong",
        ]),
        "builtin-warn" => word_patterns(&["warn", "warning", "deprecated", "caution"]),
        "builtin-success" => word_patterns(&[
            "accepted",
            "active",
            "already up to date",
            "authenticated",
            "authorized",
            "available",
            "backup completed",
            "completed successfully",
            "deployed",
            "enabled",
            "installed",
            "listening",
            "login successful",
            "online",
            "operation completed",
            "reachable",
            "restored",
            "synchronized",
            "synced",
            "updated",
            "upgraded",
            "validated",
            "verified",
            "success", "successful", "successfully",
            "ok",
            "done",
            "pass", "passed",
            "complete", "completed",
            "ready",
            "healthy",
            "running",
            "started",
            "connected",
            "uploaded",
            "downloaded",
            "created",
            "saved",
            "finished",
        ]),
        "builtin-info" => word_patterns(&["info", "information", "notice"]),
        "builtin-debug" => word_patterns(&["debug", "trace", "verbose"]),
        "builtin-duration" => vec![format!(
            r"(?i)[-+]?{DECIMAL_NUMBER}{HORIZONTAL_SPACE}*(?:ns|us|µs|ms|sec|mins?|minutes|hrs?|hours|days|weeks|months|years)\b"
        )],
        "builtin-constant" => word_patterns(&[
            "true",
            "false",
            "null",
            "nil",
            "none",
            "undefined",
            "NaN",
            "Infinity",
            "nullptr",
            "EOF",
            "stop", "stopped",
            "exit", "exited", "exiting",
            "quit", "quited", "quiting",
            "abort", "aborted", "aborting",
            "cancel", "canceled", "canceling",
            "interrupt", "interrupted", "interrupting",
            "pause", "pauseed", "pauseing",
            "resume", "resumeed", "resumeing",
        ]),
        "builtin-number" => vec![
            r"(?i)[-+]?0x[0-9a-f]+".into(),
            // Percentages may contain one digit; bare integers still need two.
            // Context boundaries are validated by the semantic matcher so signs,
            // leading/trailing decimal points and '%' stay inside the token.
            format!(
                r"[-+]?(?:{DECIMAL_NUMBER}{HORIZONTAL_SPACE}*%|(?:\d+\.\d*|\.\d+)(?:[eE][-+]?\d+)?|\d+[eE][-+]?\d+|\d{{2,}})"
            ),
        ],
        "builtin-prompt" => Vec::new(),
        "builtin-operator" => vec![r"[-:;=+&*()$\[\]<>?|{}]+".into()],
        // Network-device CLI vocabulary. Interface names and VLAN tags match as
        // whole tokens so colour does not depend on how many digits a port
        // number happens to have; status words and ACL verbs stay literal.
        "builtin-net-interface" => vec![
            r"(?i)\b(?:GE|XGE|M-GE|BAGG|GigabitEthernet|XGigabitEthernet|Ten-GigabitEthernet|Bridge-Aggregation|Vlan-interface|Vlanif)\d+(?:/\d+)*\b".into(),
        ],
        "builtin-net-vlan" => vec![r"(?i)\bVLAN\s+\d+".into()],
        "builtin-net-interface-up" => word_patterns(&["UP"]),
        "builtin-net-interface-down" => word_patterns(&["DOWN"]),
        "builtin-net-admin-down" => word_patterns(&["ADM"]),
        "builtin-net-standby" => word_patterns(&["Stby"]),
        "builtin-net-acl-permit" => word_patterns(&["permit"]),
        "builtin-net-acl-deny" => word_patterns(&["deny"]),
        "builtin-net-wildcard-mask" => vec![r"\b0\.0\.0\.\d{1,3}\b".into()],
        _ => Vec::new(),
    }
}

/// Built-in rules coloured for the current terminal theme family.
pub fn get_builtin_keyword_rules(is_dark: bool) -> Vec<ResolvedKeywordHighlightRule> {
    builtin_keyword_rule_ids()
        .iter()
        .map(|id| ResolvedKeywordHighlightRule {
            id: (*id).to_string(),
            name: builtin_keyword_rule_label(id).to_string(),
            patterns: builtin_patterns(id),
            color: builtin_keyword_rule_swatch(id, is_dark).to_string(),
            enabled: true,
        })
        .collect()
}

/// Merge user rules (higher priority) then enabled built-ins for paint.
pub fn merge_keyword_highlight_rules_for_paint(
    user_rules: &[KeywordHighlightRule],
    builtin_settings: &HashMap<String, bool>,
    is_dark: bool,
) -> Vec<ResolvedKeywordHighlightRule> {
    let mut out = Vec::new();
    for rule in user_rules {
        out.push(ResolvedKeywordHighlightRule {
            id: rule.id.clone(),
            name: rule.name.clone(),
            patterns: rule.patterns.clone(),
            color: if is_dark {
                rule.color_dark.clone()
            } else {
                rule.color_light.clone()
            },
            enabled: rule.enabled,
        });
    }
    for builtin in get_builtin_keyword_rules(is_dark) {
        let enabled = builtin_settings.get(&builtin.id).copied().unwrap_or(true);
        out.push(ResolvedKeywordHighlightRule { enabled, ..builtin });
    }
    out
}

/// Curated swatches for rule color pickers (Tauri DARK_PALETTE / LIGHT_PALETTE subset).
pub fn keyword_highlight_color_palette(is_dark: bool) -> &'static [&'static str] {
    if is_dark {
        &[
            "#f44747", "#cd9731", "#32cd32", "#6796e6", "#b267e6", "#FD971F", "#E6DB74", "#AE81FF",
            "#ffb86c", "#A6E22E", "#8be9fd", "#A6E22E",
        ]
    } else {
        &[
            "#cf222e", "#9a6700", "#116329", "#0969da", "#8250df", "#b04a00", "#a58900", "#6f42c1",
            "#cb4b16", "#1a7f37", "#2aa198", "#008573",
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::{
        KeywordHighlightRule, get_builtin_keyword_rules, merge_keyword_highlight_rules_for_paint,
    };
    use std::collections::HashMap;

    #[test]
    fn builtin_catalog_is_stable() {
        let rules = get_builtin_keyword_rules(true);
        // 21 general rules + 9 network-device CLI rules.
        assert_eq!(rules.len(), 30);
        assert!(rules.iter().all(|rule| rule.id.starts_with("builtin-")));
        assert!(
            rules
                .iter()
                .find(|r| r.id == "builtin-url")
                .is_some_and(|r| !r.patterns.is_empty())
        );
    }

    #[test]
    fn numeric_presets_cover_complete_literals_without_bare_single_digits() {
        let rules = get_builtin_keyword_rules(true);
        for (id, tokens) in [
            (
                "builtin-number",
                &[
                    "0%", "5 %", "-5%", "12.5 %", "-12", "+12", ".5", "-.5", "12.", "1e-3",
                    "-1.2E+3", "-0xFF", "5\t%", "5\u{a0}%",
                ][..],
            ),
            (
                "builtin-duration",
                &[
                    "-12ms",
                    ".5 ms",
                    "-.5ms",
                    "74.1 ms",
                    "17018ms",
                    "1e-3 ms",
                    "12 minutes",
                    "12\tµs",
                    "12\u{a0}ms",
                ][..],
            ),
        ] {
            let rule = rules.iter().find(|rule| rule.id == id).unwrap();
            let patterns: Vec<_> = rule
                .patterns
                .iter()
                .map(|pattern| regex::Regex::new(pattern).unwrap())
                .collect();
            for token in tokens {
                assert!(
                    patterns.iter().any(|pattern| pattern
                        .find(token)
                        .is_some_and(|found| found.range() == (0..token.len()))),
                    "{id}: {token:?}"
                );
            }
            for token in ["0", "7", "-5", "+3", "5\n%", "5\r%", "5\u{2028}%", "5\nms"] {
                assert!(
                    patterns.iter().all(|pattern| !pattern.is_match(token)),
                    "{id}: {token:?}"
                );
            }
        }
    }

    #[test]
    fn merge_puts_user_before_builtin() {
        let user = vec![KeywordHighlightRule {
            id: "kh-1".into(),
            name: "Panic".into(),
            patterns: vec!["panic".into()],
            color_dark: "#ff0000".into(),
            color_light: "#aa0000".into(),
            enabled: true,
        }];
        let mut settings = HashMap::new();
        settings.insert("builtin-error".into(), false);
        let merged = merge_keyword_highlight_rules_for_paint(&user, &settings, true);
        assert_eq!(merged[0].id, "kh-1");
        assert_eq!(merged[0].color, "#ff0000");
        let error = merged.iter().find(|r| r.id == "builtin-error").unwrap();
        assert!(!error.enabled);
    }

    #[test]
    fn legacy_keyword_configuration_round_trips_without_schema_changes() {
        use crate::models::settings::KeywordHighlightConfig;
        let old = serde_json::json!({
            "enabled": true,
            "across_wrapped_lines": false,
            "builtin_rules": {"builtin-error": false, "future-category": false},
            "rules": [{"id": "kh-old", "name": "Legacy", "patterns": ["panic", "err.*"],
                "color_dark": "#123456", "color_light": "#654321", "enabled": true}]
        });
        let config: KeywordHighlightConfig = serde_json::from_value(old.clone()).unwrap();
        assert_eq!(serde_json::to_value(&config).unwrap(), old);
        let merged =
            merge_keyword_highlight_rules_for_paint(&config.rules, &config.builtin_rules, true);
        assert_eq!(merged[0].patterns, vec!["panic", "err.*"]);
        assert!(
            !merged
                .iter()
                .find(|r| r.id == "builtin-error")
                .unwrap()
                .enabled
        );
        for id in ["builtin-command", "builtin-path", "builtin-permissions"] {
            assert!(merged.iter().find(|r| r.id == id).unwrap().enabled);
        }
    }

    #[test]
    fn status_presets_use_explicit_literals_without_consuming_boundaries() {
        use super::{BuiltinKeywordMatcher, builtin_keyword_matcher};
        let rules = get_builtin_keyword_rules(true);
        for id in [
            "builtin-error",
            "builtin-warn",
            "builtin-info",
            "builtin-debug",
            "builtin-success",
        ] {
            assert_eq!(
                builtin_keyword_matcher(id),
                Some(BuiltinKeywordMatcher::LiteralWords)
            );
            let rule = rules.iter().find(|r| r.id == id).unwrap();
            assert!(rule.patterns.iter().all(|p| !p.contains(['(', '[', '\\'])));
        }
    }
}
