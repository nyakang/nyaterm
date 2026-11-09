use crate::completion::matching::TextMatcher;
use crate::{CommandHistoryEntry, FuzzyResult, QuickCommand};
use std::collections::HashSet;

mod ranked;
pub use ranked::{RankedCommandSourceQuery, search_ranked_command_sources_using};

const EMPTY_MANUAL_HISTORY_LIMIT: usize = 8;
const EMPTY_MANUAL_QUICK_COMMAND_LIMIT: usize = 8;

pub fn search_command_sources(
    history: &[CommandHistoryEntry],
    quick_commands: &[QuickCommand],
    pattern: &str,
    limit: usize,
    min_history_command_length: Option<usize>,
    max_history_command_length: Option<usize>,
) -> Vec<FuzzyResult> {
    search_command_sources_using(
        &mut TextMatcher::default(),
        CommandSourceQuery {
            history,
            quick_commands,
            pattern,
            limit,
            min_history_command_length,
            max_history_command_length,
        },
    )
}

pub struct CommandSourceQuery<'a> {
    pub history: &'a [CommandHistoryEntry],
    pub quick_commands: &'a [QuickCommand],
    pub pattern: &'a str,
    pub limit: usize,
    pub min_history_command_length: Option<usize>,
    pub max_history_command_length: Option<usize>,
}

/// The background completion worker retains the matcher across history queries.
pub fn search_command_sources_using(
    matcher: &mut TextMatcher,
    query: CommandSourceQuery<'_>,
) -> Vec<FuzzyResult> {
    let CommandSourceQuery {
        history,
        quick_commands,
        pattern,
        limit,
        min_history_command_length,
        max_history_command_length,
    } = query;
    if limit == 0 {
        return Vec::new();
    }
    let pattern = pattern.trim();
    if pattern.is_empty() {
        return Vec::new();
    }
    matcher.set_query(
        &pattern
            .chars()
            .filter(|ch| !ch.is_whitespace())
            .collect::<String>(),
    );
    let mut scored = Vec::new();
    for entry in history {
        if command_within_length_limits(
            &entry.command,
            min_history_command_length,
            max_history_command_length,
        ) && let Some(score) = matcher.score(&entry.command)
        {
            scored.push((
                entry.command.as_str(),
                entry.command.as_str(),
                "history",
                score,
            ));
        }
    }
    for command in quick_commands {
        let display = if command.label.trim().is_empty() {
            command.command.as_str()
        } else {
            command.label.as_str()
        };
        if let Some(score) = matcher.score(display) {
            scored.push((display, command.command.as_str(), "quickCommand", score));
        }
    }
    scored.sort_by(|a, b| b.3.cmp(&a.3).then(a.2.cmp(b.2)).then(a.1.cmp(b.1)));
    let mut seen = HashSet::new();
    scored
        .into_iter()
        .filter(|(_, command, _, _)| seen.insert(*command))
        .take(limit)
        .map(|(display, command, source, score)| FuzzyResult {
            command: command.to_string(),
            display: display.to_string(),
            source: source.to_string(),
            score,
            indices: matcher.indices(display),
        })
        .collect()
}

pub fn manual_empty_command_suggestions(
    history: &[CommandHistoryEntry],
    quick_commands: &[QuickCommand],
    limit: usize,
    min_history_command_length: Option<usize>,
    max_history_command_length: Option<usize>,
) -> Vec<FuzzyResult> {
    if limit == 0 {
        return Vec::new();
    }
    let mut results = Vec::new();
    let mut seen = HashSet::new();
    results.extend(
        history
            .iter()
            .filter(|entry| {
                command_within_length_limits(
                    &entry.command,
                    min_history_command_length,
                    max_history_command_length,
                )
            })
            .filter(|entry| seen.insert(entry.command.clone()))
            .take(EMPTY_MANUAL_HISTORY_LIMIT)
            .enumerate()
            .map(|(index, entry)| FuzzyResult {
                command: entry.command.clone(),
                display: entry.command.clone(),
                indices: Vec::new(),
                score: 2_000u32.saturating_sub(index as u32),
                source: "history".to_string(),
            }),
    );

    let mut quick = quick_commands
        .iter()
        .filter(|command| !command.command.trim().is_empty())
        .collect::<Vec<_>>();
    quick.sort_by_key(|command| std::cmp::Reverse(quick_command_rank(command)));
    results.extend(
        quick
            .into_iter()
            .filter(|command| seen.insert(command.command.clone()))
            .take(EMPTY_MANUAL_QUICK_COMMAND_LIMIT)
            .enumerate()
            .map(|(index, command)| FuzzyResult {
                command: command.command.clone(),
                display: if command.label.trim().is_empty() {
                    command.command.clone()
                } else {
                    command.label.clone()
                },
                indices: Vec::new(),
                score: 1_000u32.saturating_sub(index as u32),
                source: "quickCommand".to_string(),
            }),
    );
    deduplicate_commands(&mut results);
    results.truncate(limit);
    results
}

fn deduplicate_commands(results: &mut Vec<FuzzyResult>) {
    let mut seen = HashSet::new();
    results.retain(|result| seen.insert(result.command.clone()));
}

pub fn fuzzy_search_items(
    items: &[(&str, &str)],
    pattern: &str,
    source: &str,
    limit: usize,
    min_command_length: Option<usize>,
    max_command_length: Option<usize>,
) -> Vec<FuzzyResult> {
    let pattern = pattern.trim();
    if pattern.is_empty() || limit == 0 {
        return Vec::new();
    }

    let mut matcher = TextMatcher::default();
    matcher.set_query(
        &pattern
            .chars()
            .filter(|ch| !ch.is_whitespace())
            .collect::<String>(),
    );
    let mut scored = Vec::new();
    for (index, (display, command)) in items.iter().enumerate() {
        let command_len = command.chars().count();
        if min_command_length.is_some_and(|min| command_len < min)
            || max_command_length.is_some_and(|max| command_len > max)
        {
            continue;
        }
        if let Some(score) = matcher.score(display) {
            scored.push((index, score));
        }
    }
    scored.sort_by(|a, b| b.1.cmp(&a.1).then(b.0.cmp(&a.0)));
    scored
        .into_iter()
        .take(limit)
        .map(|(index, score)| {
            let (display, command) = items[index];
            FuzzyResult {
                command: command.to_string(),
                display: display.to_string(),
                source: source.to_string(),
                score,
                indices: matcher.indices(display),
            }
        })
        .collect()
}

fn command_within_length_limits(
    command: &str,
    min_command_length: Option<usize>,
    max_command_length: Option<usize>,
) -> bool {
    let command_len = command.trim().chars().count();
    min_command_length.is_none_or(|min| command_len >= min)
        && max_command_length.is_none_or(|max| command_len <= max)
}

fn quick_command_rank(command: &QuickCommand) -> u64 {
    let pinned = u64::from(command.pinned.unwrap_or_default());
    let use_count = command.use_count.unwrap_or_default();
    let updated_at = command
        .updated_at
        .or(command.created_at)
        .unwrap_or_default();
    pinned
        .saturating_mul(1_000_000_000)
        .saturating_add(use_count.saturating_mul(1_000_000))
        .saturating_add(updated_at)
}

#[cfg(test)]
mod tests {
    use super::{fuzzy_search_items, manual_empty_command_suggestions, search_command_sources};
    use crate::{CommandHistoryEntry, QuickCommand};

    fn history_entry(command: &str) -> CommandHistoryEntry {
        CommandHistoryEntry {
            command: command.to_string(),
            last_used_at_ms: 10,
            use_count: 1,
        }
    }

    fn quick_command(id: &str, command: &str) -> QuickCommand {
        QuickCommand {
            id: id.to_string(),
            label: command.to_string(),
            command: command.to_string(),
            category_id: None,
            description: None,
            color_tag: None,
            icon_tag: None,
            pinned: None,
            execution_mode: None,
            raw_send: None,
            source: None,
            risk_level: None,
            updated_at: None,
            created_at: None,
            use_count: None,
            sort_order: None,
        }
    }

    #[test]
    fn fuzzy_search_items_scores_contiguous_and_filters_length() {
        let items = [
            ("docker compose ps", "docker compose ps"),
            ("git status", "git status"),
            ("long command", "0123456789"),
        ];
        let results = fuzzy_search_items(&items, "g s", "history", 5, Some(2), Some(10));
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].command, "git status");
        assert_eq!(results[0].source, "history");
        assert!(!results[0].indices.is_empty());
    }

    #[test]
    fn command_search_merges_history_and_quick_commands() {
        let history = vec![CommandHistoryEntry {
            command: "git status".to_string(),
            last_used_at_ms: 10,
            use_count: 2,
        }];
        let quick_commands = vec![QuickCommand {
            id: "qc-1".to_string(),
            label: "Docker PS".to_string(),
            command: "docker ps".to_string(),
            category_id: None,
            description: None,
            color_tag: None,
            icon_tag: None,
            pinned: None,
            execution_mode: None,
            raw_send: None,
            source: None,
            risk_level: None,
            updated_at: None,
            created_at: None,
            use_count: None,
            sort_order: None,
        }];

        let results = search_command_sources(&history, &quick_commands, "ps", 10, None, None);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].command, "docker ps");
        assert_eq!(results[0].display, "Docker PS");
        assert_eq!(results[0].source, "quickCommand");
    }

    #[test]
    fn command_search_shows_used_quick_command_only_once() {
        let history = vec![history_entry("ls -la")];
        let quick_commands = vec![
            quick_command("ls-all", "ls -la"),
            quick_command("ls-long", "ls -lh"),
        ];

        let results = search_command_sources(&history, &quick_commands, "ls", 2, None, None);

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].command, "ls -la");
        assert_eq!(results[0].source, "history");
        assert_eq!(results[1].command, "ls -lh");
    }

    #[test]
    fn manual_empty_suggestions_show_used_quick_command_only_once() {
        let history = vec![history_entry("ls -la")];
        let quick_commands = vec![
            quick_command("ls-all", "ls -la"),
            quick_command("ls-long", "ls -lh"),
        ];

        let results = manual_empty_command_suggestions(&history, &quick_commands, 2, None, None);

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].command, "ls -la");
        assert_eq!(results[1].command, "ls -lh");
    }

    #[test]
    fn command_search_deduplicates_before_applying_result_limit() {
        let mut history = vec![history_entry("ps -ef"); 12];
        history.push(history_entry("ps aux"));

        let results = search_command_sources(&history, &[], "ps", 2, None, None);

        assert_eq!(results.len(), 2);
        assert!(results.iter().any(|result| result.command == "ps -ef"));
        assert!(results.iter().any(|result| result.command == "ps aux"));
    }

    #[test]
    fn manual_suggestions_skip_duplicates_before_source_limits() {
        let mut history = vec![history_entry("ps -ef"); 8];
        history.push(history_entry("ps aux"));

        let results = manual_empty_command_suggestions(&history, &[], 2, None, None);

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].command, "ps -ef");
        assert_eq!(results[1].command, "ps aux");
    }

    #[test]
    fn manual_empty_suggestions_mix_recent_history_and_ranked_quick_commands() {
        let history = vec![
            CommandHistoryEntry {
                command: "git status".to_string(),
                last_used_at_ms: 30,
                use_count: 3,
            },
            CommandHistoryEntry {
                command: "x".to_string(),
                last_used_at_ms: 20,
                use_count: 1,
            },
        ];
        let quick_commands = vec![
            QuickCommand {
                id: "qc-low".to_string(),
                label: "Low".to_string(),
                command: "echo low".to_string(),
                category_id: None,
                description: None,
                color_tag: None,
                icon_tag: None,
                pinned: None,
                execution_mode: None,
                raw_send: None,
                source: None,
                risk_level: None,
                updated_at: Some(100),
                created_at: None,
                use_count: Some(1),
                sort_order: None,
            },
            QuickCommand {
                id: "qc-pinned".to_string(),
                label: "Pinned".to_string(),
                command: "echo pinned".to_string(),
                category_id: None,
                description: None,
                color_tag: None,
                icon_tag: None,
                pinned: Some(true),
                execution_mode: None,
                raw_send: None,
                source: None,
                risk_level: None,
                updated_at: Some(1),
                created_at: None,
                use_count: Some(0),
                sort_order: None,
            },
        ];

        let results =
            manual_empty_command_suggestions(&history, &quick_commands, 12, Some(2), None);

        assert_eq!(results[0].command, "git status");
        assert_eq!(results[0].source, "history");
        assert!(!results.iter().any(|result| result.command == "x"));
        assert_eq!(results[1].command, "echo pinned");
        assert_eq!(results[1].display, "Pinned");
    }
}
