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
        "builtin-error" | "builtin-warn" | "builtin-success" | "builtin-info" | "builtin-debug"
        | "builtin-constant" => BuiltinKeywordMatcher::LiteralWords,
        "builtin-command"
        | "builtin-path"
        | "builtin-permissions"
        | "builtin-prompt"
        // Network-device configuration vocabulary. These exist only as colour
        // sources for the line-role parser; they never match text on their own.
        | "builtin-net-keyword"
        | "builtin-net-port"
        | "builtin-net-address"
        | "builtin-net-vlan"
        | "builtin-net-secondary"
        // Semantic classes used only by the line-role parser.
        | "builtin-keyword"
        | "builtin-comment"
        | "builtin-link"
        | "builtin-weekday"
        | "builtin-month"
        | "builtin-perm-read"
        | "builtin-perm-write"
        | "builtin-perm-execute"
        | "builtin-perm-special" => BuiltinKeywordMatcher::Structured,
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
        "builtin-net-keyword",
        "builtin-net-port",
        "builtin-net-address",
        "builtin-net-vlan",
        "builtin-net-secondary",
        "builtin-keyword",
        "builtin-variable",
        "builtin-comment",
        "builtin-link",
        "builtin-weekday",
        "builtin-month",
        "builtin-perm-read",
        "builtin-perm-write",
        "builtin-perm-execute",
        "builtin-perm-special",
    ]
}

/// Rules that exist only as colour sources for the line-role parser.
///
/// Unlike these rules, `builtin-variable` also colours shell parameter
/// expansion and remains active when command-output highlighting is disabled.
pub fn builtin_keyword_rule_is_specialized(id: &str) -> bool {
    matches!(
        id,
        "builtin-net-keyword"
            | "builtin-net-port"
            | "builtin-net-address"
            | "builtin-net-vlan"
            | "builtin-net-secondary"
            | "builtin-keyword"
            | "builtin-comment"
            | "builtin-link"
            | "builtin-weekday"
            | "builtin-month"
            | "builtin-perm-read"
            | "builtin-perm-write"
            | "builtin-perm-execute"
            | "builtin-perm-special"
    )
}

/// ANSI palette index (0..=15) used by a built-in rule.
///
/// Used by palette-based categories and as a fallback for invalid colours.
/// `builtin_keyword_rule_swatch_ansi` preserves v2.0.2 light/dark swatches for
/// the original 21 categories; newer categories follow the terminal palette.
pub fn builtin_keyword_rule_ansi(id: &str) -> u8 {
    match id {
        "builtin-command" => 2,
        "builtin-path" => 4,
        "builtin-permissions" => 14,
        "builtin-url" => 14,
        "builtin-version" => 6,
        "builtin-address" => 10,
        "builtin-size" => 6,
        "builtin-string" => 3,
        "builtin-option" => 6,
        "builtin-uuid" => 13,
        "builtin-datetime" => 2,
        "builtin-error" => 9,
        "builtin-warn" => 11,
        "builtin-success" => 10,
        "builtin-info" => 12,
        "builtin-debug" => 8,
        "builtin-duration" => 3,
        "builtin-constant" => 13,
        "builtin-number" => 5,
        "builtin-prompt" => 2,
        "builtin-operator" => 11,
        "builtin-net-keyword" => 2,
        "builtin-net-port" => 4,
        "builtin-net-address" => 10,
        "builtin-net-vlan" => 10,
        "builtin-net-secondary" => 8,
        "builtin-keyword" => 13,
        "builtin-variable" => 12,
        "builtin-comment" => 8,
        "builtin-link" => 14,
        "builtin-weekday" => 6,
        "builtin-month" => 3,
        "builtin-perm-read" => 6,
        "builtin-perm-write" => 3,
        "builtin-perm-execute" => 2,
        "builtin-perm-special" => 5,
        _ => 7,
    }
}

/// Resolve a built-in swatch, preserving all v2.0.2 light/dark colours.
pub fn builtin_keyword_rule_swatch_ansi(id: &str, ansi: [u32; 16], is_dark: bool) -> String {
    if matches!(
        id,
        "builtin-command"
            | "builtin-path"
            | "builtin-permissions"
            | "builtin-url"
            | "builtin-version"
            | "builtin-address"
            | "builtin-size"
            | "builtin-string"
            | "builtin-option"
            | "builtin-uuid"
            | "builtin-datetime"
            | "builtin-error"
            | "builtin-warn"
            | "builtin-success"
            | "builtin-info"
            | "builtin-debug"
            | "builtin-duration"
            | "builtin-constant"
            | "builtin-number"
            | "builtin-prompt"
            | "builtin-operator"
    ) {
        return builtin_keyword_rule_swatch(id, is_dark).to_string();
    }
    let index = usize::from(builtin_keyword_rule_ansi(id).min(15));
    format!("#{:06x}", ansi[index])
}

/// Built-in rules with legacy light/dark swatches and terminal ANSI colours.
pub fn get_builtin_keyword_rules_with_ansi(
    ansi: [u32; 16],
    is_dark: bool,
) -> Vec<ResolvedKeywordHighlightRule> {
    builtin_keyword_rule_ids()
        .iter()
        .map(|id| ResolvedKeywordHighlightRule {
            id: (*id).to_string(),
            name: builtin_keyword_rule_label(id).to_string(),
            patterns: builtin_patterns(id),
            color: builtin_keyword_rule_swatch_ansi(id, ansi, is_dark),
            enabled: true,
        })
        .collect()
}

/// Merge user rules then built-ins, preserving each category's colour policy.
pub fn merge_keyword_highlight_rules_for_paint_with_ansi(
    user_rules: &[KeywordHighlightRule],
    builtin_settings: &HashMap<String, bool>,
    is_dark: bool,
    ansi: [u32; 16],
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
    for builtin in get_builtin_keyword_rules_with_ansi(ansi, is_dark) {
        let enabled = builtin_settings.get(&builtin.id).copied().unwrap_or(true);
        out.push(ResolvedKeywordHighlightRule { enabled, ..builtin });
    }
    out
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
        "builtin-keyword" => "Keyword",
        "builtin-variable" => "Variable",
        "builtin-comment" => "Comment",
        "builtin-link" => "Link",
        "builtin-weekday" => "Weekday",
        "builtin-month" => "Month",
        "builtin-perm-read" => "Permission read",
        "builtin-perm-write" => "Permission write",
        "builtin-perm-execute" => "Permission execute",
        "builtin-perm-special" => "Permission special",
        "builtin-net-keyword" => "Network keyword",
        "builtin-net-port" => "Network port",
        "builtin-net-address" => "Network address",
        "builtin-net-vlan" => "Network VLAN",
        "builtin-net-secondary" => "Network secondary",
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
        "builtin-keyword" => {
            if is_dark {
                "#d2a8ff"
            } else {
                "#8250df"
            }
        }
        "builtin-variable" => {
            if is_dark {
                "#56d4dd"
            } else {
                "#0a7d8c"
            }
        }
        "builtin-comment" => {
            if is_dark {
                "#8b949e"
            } else {
                "#57606a"
            }
        }
        "builtin-link" => {
            if is_dark {
                "#79c0ff"
            } else {
                "#0969da"
            }
        }
        "builtin-weekday" => {
            if is_dark {
                "#e3b341"
            } else {
                "#a58900"
            }
        }
        "builtin-month" => {
            if is_dark {
                "#e3b341"
            } else {
                "#a58900"
            }
        }
        "builtin-perm-read" => {
            if is_dark {
                "#79c0ff"
            } else {
                "#0969da"
            }
        }
        "builtin-perm-write" => {
            if is_dark {
                "#ffa657"
            } else {
                "#b04a00"
            }
        }
        "builtin-perm-execute" => {
            if is_dark {
                "#7ee787"
            } else {
                "#1a7f37"
            }
        }
        "builtin-perm-special" => {
            if is_dark {
                "#ff7b72"
            } else {
                "#cf222e"
            }
        }
        // Network configuration palette: one meaning per colour, so an address
        // never borrows the "info" hue or a VLAN id the "success" hue.
        "builtin-net-keyword" => {
            if is_dark {
                "#79c0ff"
            } else {
                "#0969da"
            }
        }
        "builtin-net-port" => {
            if is_dark {
                "#a5d6ff"
            } else {
                "#0550ae"
            }
        }
        "builtin-net-address" => {
            if is_dark {
                "#56d4dd"
            } else {
                "#0a7d8c"
            }
        }
        "builtin-net-vlan" => {
            if is_dark {
                "#7ee787"
            } else {
                "#1a7f37"
            }
        }
        "builtin-net-secondary" => {
            if is_dark {
                "#8b949e"
            } else {
                "#57606a"
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
        // `$VAR` / `${VAR}` / `$1` — also matched outside shell integration so
        // the command line is coloured even without OSC 133.
        "builtin-variable" => vec![
            r"\$\{[^}]*\}".into(),
            r"\$[A-Za-z_][A-Za-z0-9_]*".into(),
            r"\$[0-9]+".into(),
        ],
        "builtin-prompt" => Vec::new(),
        "builtin-operator" => vec![r"[-:;=+&*()$\[\]<>?|{}]+".into()],
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
        KeywordHighlightRule, builtin_keyword_rule_ids, builtin_keyword_rule_is_specialized,
        get_builtin_keyword_rules, merge_keyword_highlight_rules_for_paint,
    };
    use std::collections::{HashMap, HashSet};

    #[test]
    fn builtin_catalog_is_stable() {
        let rules = get_builtin_keyword_rules(true);
        // 22 general + 14 command-output-only colour classes.
        assert_eq!(rules.len(), 36);
        assert!(rules.iter().all(|rule| rule.id.starts_with("builtin-")));
        assert!(
            rules
                .iter()
                .find(|r| r.id == "builtin-url")
                .is_some_and(|r| !r.patterns.is_empty())
        );
    }

    #[test]
    fn builtin_groups_cover_the_catalog_and_keep_shell_variables_general() {
        let ids = builtin_keyword_rule_ids();
        let unique: HashSet<_> = ids.iter().copied().collect();
        assert_eq!(unique.len(), 36);
        let (specialized, general): (Vec<_>, Vec<_>) = ids
            .iter()
            .copied()
            .partition(|id| builtin_keyword_rule_is_specialized(id));
        assert_eq!(general.len(), 22);
        assert_eq!(specialized.len(), 14);
        assert!(general.contains(&"builtin-variable"));
        assert!(specialized.contains(&"builtin-perm-read"));
        assert!(specialized.contains(&"builtin-net-address"));
        assert!(general.iter().all(|id| !specialized.contains(id)));
        assert_eq!(general.len() + specialized.len(), ids.len());
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

#[cfg(test)]
mod palette_tests {
    use super::{
        builtin_keyword_rule_ansi, get_builtin_keyword_rules_with_ansi,
        merge_keyword_highlight_rules_for_paint_with_ansi,
    };
    use crate::models::settings::KeywordHighlightRule;
    use std::collections::HashMap;

    #[test]
    fn all_v2_0_2_categories_keep_their_colours_while_new_builtins_follow_ansi() {
        let legacy_swatches = [
            ("builtin-command", "#66D9EF", "#007197"),
            ("builtin-path", "#E6DB74", "#8a6700"),
            ("builtin-permissions", "#b267e6", "#8250df"),
            ("builtin-url", "#8be9fd", "#2aa198"),
            ("builtin-version", "#ff9e64", "#b04a00"),
            ("builtin-address", "#A6E22E", "#1a7f37"),
            ("builtin-size", "#2ac3de", "#007197"),
            ("builtin-string", "#E6DB74", "#1a8c8c"),
            ("builtin-option", "#FD971F", "#b04a00"),
            ("builtin-uuid", "#ffb86c", "#bc4c00"),
            ("builtin-datetime", "#A6E22E", "#a58900"),
            ("builtin-error", "#f44747", "#cf222e"),
            ("builtin-warn", "#cd9731", "#9a6700"),
            ("builtin-success", "#32cd32", "#116329"),
            ("builtin-info", "#6796e6", "#0969da"),
            ("builtin-debug", "#b267e6", "#8250df"),
            ("builtin-duration", "#E6DB74", "#859900"),
            ("builtin-constant", "#ffb86c", "#cb4b16"),
            ("builtin-number", "#AE81FF", "#6f42c1"),
            ("builtin-prompt", "#f92672", "#e00862"),
            ("builtin-operator", "#A6E22E", "#008573"),
        ];
        for base in [0x123400, 0xabcdef] {
            let ansi = std::array::from_fn(|index| base + index as u32);
            for dark in [true, false] {
                for rule in get_builtin_keyword_rules_with_ansi(ansi, dark) {
                    let expected = legacy_swatches
                        .iter()
                        .find(|(id, _, _)| *id == rule.id)
                        .map_or_else(
                            || {
                                format!(
                                    "#{:06x}",
                                    ansi[usize::from(builtin_keyword_rule_ansi(&rule.id))]
                                )
                            },
                            |(_, color_dark, color_light)| {
                                if dark { color_dark } else { color_light }.to_string()
                            },
                        );
                    assert_eq!(rule.color, expected, "{} (dark={dark})", rule.id);
                }
            }
        }
    }

    #[test]
    fn changing_palette_keeps_user_colours_and_disabled_builtin_choices() {
        let user = KeywordHighlightRule {
            id: "custom".into(),
            name: "Custom".into(),
            patterns: vec!["hello".into()],
            color_dark: "#123456".into(),
            color_light: "#654321".into(),
            enabled: true,
        };
        let disabled = HashMap::from([
            ("builtin-info".to_string(), false),
            ("builtin-prompt".to_string(), false),
        ]);
        for (dark, expected) in [(true, "#123456"), (false, "#654321")] {
            let rules = merge_keyword_highlight_rules_for_paint_with_ansi(
                std::slice::from_ref(&user),
                &disabled,
                dark,
                [0xabcdef; 16],
            );
            assert_eq!(rules[0].color, expected);
            for id in disabled.keys() {
                assert!(!rules.iter().find(|rule| &rule.id == id).unwrap().enabled);
            }
        }
    }
}
