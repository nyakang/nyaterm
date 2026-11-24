use std::collections::HashMap;

use serde::Deserialize;

use nyaterm_core::{KeywordHighlightImportResult, KeywordHighlightRule, uuid};

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum KeywordHighlightImportFile {
    Config {
        keyword_highlights: Vec<KeywordHighlightRule>,
    },
    Rules(Vec<KeywordHighlightRule>),
}

pub(super) fn parse_keyword_highlight_import(
    raw: &str,
) -> Result<Vec<KeywordHighlightRule>, serde_json::Error> {
    match serde_json::from_str(raw)? {
        KeywordHighlightImportFile::Config { keyword_highlights } => Ok(keyword_highlights),
        KeywordHighlightImportFile::Rules(rules) => Ok(rules),
    }
}

pub(super) fn normalize_keyword_highlight_rule(
    mut rule: KeywordHighlightRule,
) -> Option<KeywordHighlightRule> {
    rule.id = rule.id.trim().to_string();
    rule.name = rule.name.trim().to_string();
    // Keep blank pattern lines for editor drafts (Tauri joins patterns with newlines).
    // Drop completely empty pattern lists only when name is also empty.
    rule.patterns = rule
        .patterns
        .into_iter()
        .map(|pattern| pattern.trim_end().to_string())
        .collect();
    let has_pattern = rule.patterns.iter().any(|p| !p.trim().is_empty());
    if rule.name.is_empty() && !has_pattern {
        return None;
    }
    if rule.name.is_empty() {
        rule.name = "Untitled rule".to_string();
    }
    if rule.color_dark.trim().is_empty() {
        rule.color_dark = "#79c0ff".to_string();
    }
    if rule.color_light.trim().is_empty() {
        rule.color_light = "#0969da".to_string();
    }
    rule.bg_color_dark = rule.bg_color_dark.trim().to_string();
    rule.bg_color_light = rule.bg_color_light.trim().to_string();
    Some(rule)
}

pub(super) fn merge_keyword_highlight_rules(
    existing: &mut Vec<KeywordHighlightRule>,
    imported: Vec<KeywordHighlightRule>,
) -> KeywordHighlightImportResult {
    let mut imported_rules = 0;
    let mut updated_rules = 0;
    let mut indexes = existing
        .iter()
        .enumerate()
        .filter(|(_, rule)| !rule.id.trim().is_empty())
        .map(|(index, rule)| (rule.id.clone(), index))
        .collect::<HashMap<_, _>>();

    for mut rule in imported
        .into_iter()
        .filter_map(normalize_keyword_highlight_rule)
    {
        if rule.id.is_empty() {
            rule.id = format!("highlight-{}", uuid());
        }
        if let Some(index) = indexes.get(&rule.id).copied() {
            existing[index] = rule;
            updated_rules += 1;
        } else {
            let id = rule.id.clone();
            existing.push(rule);
            indexes.insert(id, existing.len() - 1);
            imported_rules += 1;
        }
    }

    KeywordHighlightImportResult {
        imported_rules,
        updated_rules,
        total_rules: existing.len(),
    }
}

#[cfg(test)]
mod tests {
    use nyaterm_core::KeywordHighlightRule;

    use super::{
        merge_keyword_highlight_rules, normalize_keyword_highlight_rule,
        parse_keyword_highlight_import,
    };

    #[test]
    fn parses_object_and_array_keyword_highlight_imports() {
        let object_rules = parse_keyword_highlight_import(
            r##"{"keyword_highlights":[{"id":"panic","name":"Panic","patterns":["panic"]}]}"##,
        )
        .expect("object import");
        assert_eq!(object_rules.len(), 1);
        assert_eq!(object_rules[0].id, "panic");

        let array_rules = parse_keyword_highlight_import(
            r##"[{"id":"warn","name":"Warn","patterns":["warn"]}]"##,
        )
        .expect("array import");
        assert_eq!(array_rules.len(), 1);
        assert_eq!(array_rules[0].id, "warn");
    }

    #[test]
    fn normalizes_blank_names_and_colors_without_dropping_named_drafts() {
        let normalized = normalize_keyword_highlight_rule(KeywordHighlightRule {
            id: "  id  ".to_string(),
            name: "   ".to_string(),
            patterns: vec!["warn  ".to_string()],
            color_dark: " ".to_string(),
            color_light: String::new(),
            bg_color_dark: String::new(),
            bg_color_light: String::new(),
            enabled: true,
        })
        .expect("rule");

        assert_eq!(normalized.id, "id");
        assert_eq!(normalized.name, "Untitled rule");
        assert_eq!(normalized.patterns, vec!["warn".to_string()]);
        assert_eq!(normalized.color_dark, "#79c0ff");
        assert_eq!(normalized.color_light, "#0969da");

        assert!(
            normalize_keyword_highlight_rule(KeywordHighlightRule {
                id: String::new(),
                name: String::new(),
                patterns: vec![" ".to_string()],
                color_dark: String::new(),
                color_light: String::new(),
                bg_color_dark: String::new(),
                bg_color_light: String::new(),
                enabled: true,
            })
            .is_none()
        );
    }

    #[test]
    fn merge_updates_existing_ids_and_imports_new_rules() {
        let mut existing = vec![KeywordHighlightRule {
            id: "panic".to_string(),
            name: "Panic".to_string(),
            patterns: vec!["panic".to_string()],
            ..KeywordHighlightRule::default()
        }];
        let result = merge_keyword_highlight_rules(
            &mut existing,
            vec![
                KeywordHighlightRule {
                    id: "panic".to_string(),
                    name: "Panic Updated".to_string(),
                    patterns: vec!["fatal".to_string()],
                    ..KeywordHighlightRule::default()
                },
                KeywordHighlightRule {
                    name: "Deploy".to_string(),
                    patterns: vec!["deploy".to_string()],
                    ..KeywordHighlightRule::default()
                },
            ],
        );

        assert_eq!(result.imported_rules, 1);
        assert_eq!(result.updated_rules, 1);
        assert_eq!(result.total_rules, 2);
        assert_eq!(existing[0].name, "Panic Updated");
        assert!(existing[1].id.starts_with("highlight-"));
    }
}
