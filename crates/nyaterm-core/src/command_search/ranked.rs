use crate::command_search::{
    EMPTY_MANUAL_HISTORY_LIMIT, EMPTY_MANUAL_QUICK_COMMAND_LIMIT, quick_command_rank,
};
use crate::completion::matching::TextMatcher;
use crate::completion::types::{
    CandidateKind, CandidateSource, CompletionCandidate, CompletionFillPolicy, CompletionScope,
    supports_full_command_fill,
};
use crate::completion::whole_history::{
    HistorySearchEntry, HistoryStatistics, WholeCommandHistoryIndex, normalized_text,
};
use crate::models::quick_commands::QuickCommand;
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet};

const LOCAL_BONUS: u32 = 8;
const MAX_FREQUENCY_BONUS: u32 = 4;
const DAY_MS: u64 = 86_400_000;

pub struct RankedCommandSourceQuery<'a> {
    pub history: &'a WholeCommandHistoryIndex,
    pub scope: &'a CompletionScope,
    pub quick_commands: &'a [QuickCommand],
    pub pattern: &'a str,
    pub limit: usize,
    pub min_history_command_length: Option<usize>,
    pub max_history_command_length: Option<usize>,
    pub now_ms: u64,
    /// None preserves unrestricted catalog search, including unfillable entries.
    pub fill_policy: Option<CompletionFillPolicy>,
}

fn bonus(statistics: HistoryStatistics, current_scope: bool, now_ms: u64) -> u32 {
    let frequency = statistics.use_count.max(1).ilog2().min(MAX_FREQUENCY_BONUS);
    let age = now_ms.saturating_sub(statistics.last_used_at_ms);
    let recency = if statistics.last_used_at_ms == 0 {
        0
    } else if age <= DAY_MS {
        3
    } else if age <= 7 * DAY_MS {
        2
    } else if age <= 30 * DAY_MS {
        1
    } else {
        0
    };
    u32::from(current_scope) * LOCAL_BONUS + frequency + recency
}

fn match_class(normalized: &str, needle: &str) -> u8 {
    if normalized == needle {
        2
    } else if normalized.starts_with(needle) {
        1
    } else {
        0
    }
}

struct Item<'a> {
    command: &'a str,
    display: &'a str,
    kind: CandidateKind,
    statistics: HistoryStatistics,
    current_scope: bool,
    sources: u8,
    class: u8,
    raw_score: u32,
    score: u64,
    last_used: u64,
}

impl<'a> Item<'a> {
    fn new(
        command: &'a str,
        display: &'a str,
        kind: CandidateKind,
        history: Option<HistorySearchEntry<'a>>,
        raw_score: u32,
        class: u8,
        now_ms: u64,
    ) -> Self {
        let statistics = history.map_or(HistoryStatistics::default(), |entry| entry.statistics);
        let current_scope = history.is_some_and(|entry| entry.current_scope);
        let mut sources = history.map_or(0, |entry| entry.sources);
        if kind == CandidateKind::QuickCommand {
            sources |= 1 << CandidateSource::QuickCommand as u8;
        }
        Self {
            command,
            display,
            kind,
            statistics,
            current_scope,
            sources,
            class,
            raw_score,
            score: u64::from(raw_score) * u64::from(100 + bonus(statistics, current_scope, now_ms)),
            last_used: statistics.last_used_at_ms.min(now_ms),
        }
    }

    fn compare(&self, other: &Self) -> Ordering {
        other
            .class
            .cmp(&self.class)
            .then(other.score.cmp(&self.score))
            .then(other.raw_score.cmp(&self.raw_score))
            .then(other.current_scope.cmp(&self.current_scope))
            .then(other.last_used.cmp(&self.last_used))
            .then(other.statistics.use_count.cmp(&self.statistics.use_count))
            .then(self.command.cmp(other.command))
    }

    fn finish(self, matcher: &mut TextMatcher, input: &str) -> CompletionCandidate {
        CompletionCandidate {
            display: self.display.into(),
            insert_text: self.command.into(),
            description: String::new(),
            range: 0..input.len(),
            kind: self.kind,
            sources: [
                CandidateSource::History,
                CandidateSource::QuickCommand,
                CandidateSource::LegacyHistory,
            ]
            .into_iter()
            .filter(|source| self.sources & (1 << *source as u8) != 0)
            .collect(),
            use_count: self.statistics.use_count,
            last_used_at_ms: self.statistics.last_used_at_ms,
            score: u32::try_from(self.score).unwrap_or(u32::MAX),
            indices: if input.trim().is_empty() {
                Vec::new()
            } else {
                matcher.indices(self.display)
            },
            fillable: supports_full_command_fill(self.command),
            context_priority: 0,
        }
    }
}

fn within_limits(entry: HistorySearchEntry<'_>, query: &RankedCommandSourceQuery<'_>) -> bool {
    query
        .min_history_command_length
        .is_none_or(|min| entry.char_count >= min)
        && query
            .max_history_command_length
            .is_none_or(|max| entry.char_count <= max)
}

fn eligible_target(command: &str, query: &RankedCommandSourceQuery<'_>) -> bool {
    query
        .fill_policy
        .is_none_or(|policy| policy.allows(query.pattern, command))
}

impl PartialEq for Item<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.compare(other) == Ordering::Equal
    }
}
impl Eq for Item<'_> {}
impl PartialOrd for Item<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Item<'_> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.compare(other)
    }
}

fn retain_best<'a>(items: &mut BinaryHeap<Item<'a>>, item: Item<'a>, limit: usize) {
    if items.len() < limit {
        items.push(item);
    } else if let Some(mut worst) = items.peek_mut()
        && item.compare(&worst) == Ordering::Less
    {
        *worst = item;
    }
}

/// Score borrowed catalog text; allocate owned command text only for the selected rows.
pub fn search_ranked_command_sources_using(
    matcher: &mut TextMatcher,
    query: RankedCommandSourceQuery<'_>,
) -> Vec<CompletionCandidate> {
    search_ranked_command_sources_with_cancel(matcher, query, &mut || false).unwrap_or_default()
}

pub fn search_ranked_command_sources_with_cancel(
    matcher: &mut TextMatcher,
    query: RankedCommandSourceQuery<'_>,
    cancelled: &mut dyn FnMut() -> bool,
) -> Option<Vec<CompletionCandidate>> {
    if cancelled() {
        return None;
    }
    if query.limit == 0 {
        return Some(Vec::new());
    }
    let quick_targets: HashSet<_> = query
        .quick_commands
        .iter()
        .map(|quick| quick.command.as_str())
        .collect();
    if query.pattern.trim().is_empty() {
        return empty_suggestions(matcher, &query, &quick_targets, cancelled);
    }
    let needle = normalized_text(query.pattern);
    matcher.set_query(
        &query
            .pattern
            .trim()
            .chars()
            .filter(|ch| !ch.is_whitespace())
            .collect::<String>(),
    );
    // Only quick targets need merging storage; the history stream occupies K slots.
    let mut quick_rows = HashMap::<&str, Item<'_>>::new();
    for (i, quick) in query.quick_commands.iter().enumerate() {
        if i % 128 == 0 && cancelled() {
            return None;
        }
        if !eligible_target(&quick.command, &query) {
            continue;
        }
        let display = if quick.label.trim().is_empty() {
            quick.command.as_str()
        } else {
            quick.label.as_str()
        };
        let Some(score) = matcher.score(display) else {
            continue;
        };
        let item = Item::new(
            &quick.command,
            display,
            CandidateKind::QuickCommand,
            query.history.get(&quick.command, query.scope),
            score,
            match_class(&normalized_text(display), &needle),
            query.now_ms,
        );
        match quick_rows.get_mut(quick.command.as_str()) {
            Some(previous)
                if (item.class, item.raw_score) > (previous.class, previous.raw_score) =>
            {
                *previous = item
            }
            Some(_) => {}
            None => {
                quick_rows.insert(&quick.command, item);
            }
        }
    }
    let mut items = BinaryHeap::with_capacity(query.limit.min(64));
    for (i, entry) in query.history.iter(query.scope).enumerate() {
        if i % 128 == 0 && cancelled() {
            return None;
        }
        if !within_limits(entry, &query) || !eligible_target(entry.command, &query) {
            continue;
        }
        let score = match entry.prepared {
            Some(text) => matcher.score_prepared(text),
            None => matcher.score(entry.command),
        };
        let Some(score) = score else {
            continue;
        };
        let mut item = Item::new(
            entry.command,
            entry.command,
            CandidateKind::History,
            Some(entry),
            score,
            match_class(entry.normalized, &needle),
            query.now_ms,
        );
        if quick_targets.contains(entry.command) {
            item.sources |= 1 << CandidateSource::QuickCommand as u8;
            if let Some(mut quick) = quick_rows.remove(entry.command) {
                quick.sources |= item.sources;
                if (quick.class, quick.raw_score) > (item.class, item.raw_score) {
                    item = quick;
                }
            }
        }
        retain_best(&mut items, item, query.limit);
    }
    for item in quick_rows.into_values() {
        retain_best(&mut items, item, query.limit);
    }
    if cancelled() {
        return None;
    }
    Some(
        items
            .into_sorted_vec()
            .into_iter()
            .map(|item| item.finish(matcher, query.pattern))
            .collect(),
    )
}

fn empty_suggestions(
    matcher: &mut TextMatcher,
    query: &RankedCommandSourceQuery<'_>,
    quick_targets: &HashSet<&str>,
    cancelled: &mut dyn FnMut() -> bool,
) -> Option<Vec<CompletionCandidate>> {
    let mut best = BinaryHeap::new();
    let limit = EMPTY_MANUAL_HISTORY_LIMIT.min(query.limit);
    for (i, entry) in query.history.iter(query.scope).enumerate() {
        if i % 128 == 0 && cancelled() {
            return None;
        }
        if !within_limits(entry, query) || !eligible_target(entry.command, query) {
            continue;
        }
        let mut item = Item::new(
            entry.command,
            entry.command,
            CandidateKind::History,
            Some(entry),
            1,
            0,
            query.now_ms,
        );
        if quick_targets.contains(entry.command) {
            item.sources |= 1 << CandidateSource::QuickCommand as u8;
        }
        retain_best(&mut best, item, limit);
    }
    let mut items = best.into_sorted_vec();
    let mut seen: HashSet<_> = items.iter().map(|item| item.command).collect();
    let mut quick: Vec<_> = query
        .quick_commands
        .iter()
        .filter(|quick| !quick.command.trim().is_empty())
        .filter(|quick| eligible_target(&quick.command, query))
        .collect();
    quick.sort_by_key(|quick| std::cmp::Reverse(quick_command_rank(quick)));
    for quick in quick
        .into_iter()
        .filter(|quick| seen.insert(quick.command.as_str()))
        .take(EMPTY_MANUAL_QUICK_COMMAND_LIMIT)
    {
        let display = if quick.label.trim().is_empty() {
            quick.command.as_str()
        } else {
            quick.label.as_str()
        };
        items.push(Item::new(
            &quick.command,
            display,
            CandidateKind::QuickCommand,
            query.history.get(&quick.command, query.scope),
            1,
            0,
            query.now_ms,
        ));
    }
    items.truncate(query.limit);
    if cancelled() {
        return None;
    }
    Some(
        items
            .into_iter()
            .map(|item| item.finish(matcher, query.pattern))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use crate::command_search::ranked::{
        DAY_MS, RankedCommandSourceQuery, bonus, search_ranked_command_sources_using,
    };
    use crate::completion::matching::TextMatcher;
    use crate::completion::types::{
        CandidateKind, CandidateSource, CompletionFillPolicy, CompletionScope, ScopedHistoryRecord,
        ShellDialect,
    };
    use crate::completion::whole_history::{HistoryStatistics, WholeCommandHistoryIndex};
    use crate::models::quick_commands::{CommandHistoryEntry, QuickCommand};
    use std::collections::BTreeMap;

    fn record(scope: &CompletionScope, command: &str) -> ScopedHistoryRecord {
        ScopedHistoryRecord {
            version: 1,
            scope: scope.clone(),
            dialect: ShellDialect::Bash,
            command: command.into(),
            use_count: 1,
            last_used_at_ms: 0,
            extensions: BTreeMap::new(),
        }
    }

    fn query<'a>(
        index: &'a WholeCommandHistoryIndex,
        scope: &'a CompletionScope,
        pattern: &'a str,
        quick: &'a [QuickCommand],
    ) -> RankedCommandSourceQuery<'a> {
        RankedCommandSourceQuery {
            fill_policy: None,
            history: index,
            scope,
            quick_commands: quick,
            pattern,
            limit: 12,
            min_history_command_length: Some(2),
            max_history_command_length: Some(64),
            now_ms: 100 * DAY_MS,
        }
    }

    #[test]
    fn fill_policy_filters_real_targets_before_limits_and_keeps_multiline_review() {
        let scope = CompletionScope::new(&["fill-policy"]);
        let mut index = WholeCommandHistoryIndex::default();
        index.reconcile(
            &[],
            &[
                record(&scope, "mv source destination"),
                record(&scope, "vim file"),
            ],
        );
        let mut quick: Vec<QuickCommand> = (0..20)
            .map(|n| {
                serde_json::from_value(serde_json::json!({
                    "id": format!("bad-{n}"), "label": "mv", "command": format!("mv\x1b[{n}m")
                }))
                .unwrap()
            })
            .collect();
        for (id, label, command) in [
            ("alias", "mv", "cat file"),
            ("script", "mv", "echo first\necho second"),
            ("valid", "mv alias", "mv actual destination"),
        ] {
            quick.push(
                serde_json::from_value(serde_json::json!({
                    "id": id, "label": label, "command": command
                }))
                .unwrap(),
            );
        }
        let mut search = query(&index, &scope, "mv", &quick);
        search.limit = 3;
        search.fill_policy = Some(CompletionFillPolicy::AppendOnly);
        let results = search_ranked_command_sources_using(&mut TextMatcher::default(), search);
        assert_eq!(results.len(), 3);
        assert!(results.iter().all(|item| item.fillable));
        for target in [
            "mv source destination",
            "mv actual destination",
            "echo first\necho second",
        ] {
            assert!(
                results.iter().any(|item| item.insert_text == target),
                "{target}"
            );
        }
        let mut search = query(&index, &scope, "mv", &quick);
        search.fill_policy = Some(CompletionFillPolicy::ReplaceAllowed);
        let results = search_ranked_command_sources_using(&mut TextMatcher::default(), search);
        assert!(results.iter().any(|item| item.insert_text == "cat file"));
        assert!(results.iter().all(|item| item.fillable));
        let results = search_ranked_command_sources_using(
            &mut TextMatcher::default(),
            query(&index, &scope, "mv", &quick),
        );
        assert!(results.iter().any(|item| !item.fillable));
    }

    #[test]
    fn empty_history_search_applies_fill_policy_before_history_and_quick_quotas() {
        let scope = CompletionScope::new(&["empty-fill-policy"]);
        let mut index = WholeCommandHistoryIndex::default();
        let mut records: Vec<_> = (0..20)
            .map(|n| record(&scope, &format!("bad-{n}\x1b")))
            .collect();
        records.push(record(&scope, "echo valid"));
        index.reconcile(&[], &records);
        let mut search = query(&index, &scope, "", &[]);
        search.fill_policy = Some(CompletionFillPolicy::AppendOnly);
        let results = search_ranked_command_sources_using(&mut TextMatcher::default(), search);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].insert_text, "echo valid");
    }

    #[test]
    fn current_environment_wins_close_matches_after_ranking_all_candidates() {
        let a = CompletionScope::new(&["a"]);
        let b = CompletionScope::new(&["b"]);
        let mut records: Vec<_> = (0..20)
            .map(|n| record(&a, &format!("curl https://fixture-{n:02}.test")))
            .collect();
        records.push(record(&b, "curl https://www.baidu.com"));
        let mut index = WholeCommandHistoryIndex::default();
        index.reconcile(&[], &records);
        let results = search_ranked_command_sources_using(
            &mut TextMatcher::default(),
            query(&index, &b, "curl h", &[]),
        );
        assert_eq!(results.len(), 12);
        assert_eq!(results[0].insert_text, "curl https://www.baidu.com");
        let foreign = CompletionScope::new(&["foreign"]);
        let results = search_ranked_command_sources_using(
            &mut TextMatcher::default(),
            query(&index, &foreign, "curl https://www.bai", &[]),
        );
        assert_eq!(results[0].insert_text, "curl https://www.baidu.com");
    }

    #[test]
    fn better_match_levels_and_stronger_text_scores_win_over_environment_and_frequency() {
        let a = CompletionScope::new(&["a"]);
        let b = CompletionScope::new(&["b"]);
        let mut local = record(&b, "curl --header abc https://fixture.test");
        local.use_count = u32::MAX;
        local.last_used_at_ms = u64::MAX;
        let mut index = WholeCommandHistoryIndex::default();
        index.reconcile(&[], &[record(&a, "curl https://www.baidu.com"), local]);
        let results = search_ranked_command_sources_using(
            &mut TextMatcher::default(),
            query(&index, &b, "curl h", &[]),
        );
        assert_eq!(results[0].insert_text, "curl https://www.baidu.com");
        index.reconcile(
            &[],
            &[
                record(&a, "git status"),
                record(
                    &b,
                    "g            i            t            s            t a t u s",
                ),
            ],
        );
        let results = search_ranked_command_sources_using(
            &mut TextMatcher::default(),
            query(&index, &b, "gitst", &[]),
        );
        assert_eq!(results[0].insert_text, "git status");
        index.reconcile(
            &[],
            &[record(&a, "git status"), record(&b, "git status --short")],
        );
        let results = search_ranked_command_sources_using(
            &mut TextMatcher::default(),
            query(&index, &b, "git status", &[]),
        );
        assert_eq!(results[0].insert_text, "git status");
    }

    #[test]
    fn frequency_is_bounded_and_recency_handles_boundaries_unknown_and_future_times() {
        let now = 100 * DAY_MS;
        for (count, expected) in [
            (0, 0),
            (1, 0),
            (2, 1),
            (4, 2),
            (8, 3),
            (16, 4),
            (u32::MAX, 4),
        ] {
            assert_eq!(
                bonus(
                    HistoryStatistics {
                        use_count: count,
                        last_used_at_ms: 0
                    },
                    false,
                    now
                ),
                expected
            );
        }
        for (time, expected) in [
            (0, 0),
            (now, 3),
            (now - DAY_MS, 3),
            (now - DAY_MS - 1, 2),
            (now - 7 * DAY_MS, 2),
            (now - 7 * DAY_MS - 1, 1),
            (now - 30 * DAY_MS, 1),
            (now - 30 * DAY_MS - 1, 0),
            (u64::MAX, 3),
        ] {
            assert_eq!(
                bonus(
                    HistoryStatistics {
                        use_count: 1,
                        last_used_at_ms: time
                    },
                    false,
                    now
                ),
                expected
            );
        }
        assert_eq!(
            bonus(
                HistoryStatistics {
                    use_count: u32::MAX,
                    last_used_at_ms: u64::MAX
                },
                true,
                now
            ),
            15
        );
    }

    #[test]
    fn quick_alias_uses_better_display_but_history_statistics_are_counted_once() {
        let scope = CompletionScope::new(&["a"]);
        let mut history = record(&scope, "curl https://www.baidu.com");
        history.use_count = 3;
        let quick: QuickCommand = serde_json::from_value(serde_json::json!({
            "id":"curl", "label":"curl", "command":history.command, "use_count":999
        }))
        .unwrap();
        let mut index = WholeCommandHistoryIndex::default();
        index.reconcile(&[], &[history]);
        let results = search_ranked_command_sources_using(
            &mut TextMatcher::default(),
            query(&index, &scope, "curl", &[quick]),
        );
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].display, "curl");
        assert_eq!(results[0].kind, CandidateKind::QuickCommand);
        assert_eq!(results[0].use_count, 3);
        assert_eq!(
            results[0].sources,
            [CandidateSource::History, CandidateSource::QuickCommand]
        );
    }

    #[test]
    fn preserves_length_filters_cross_word_matching_and_unicode_highlights() {
        let scope = CompletionScope::new(&["a"]);
        let mut index = WholeCommandHistoryIndex::default();
        index.reconcile(
            &[],
            &[
                record(&scope, "systemctl restart mihomo.service"),
                record(&scope, "echo e\u{301}工具"),
            ],
        );
        let results = search_ranked_command_sources_using(
            &mut TextMatcher::default(),
            query(&index, &scope, "systmi", &[]),
        );
        assert_eq!(results[0].insert_text, "systemctl restart mihomo.service");
        let mut short = query(&index, &scope, "systmi", &[]);
        short.max_history_command_length = Some(3);
        assert!(search_ranked_command_sources_using(&mut TextMatcher::default(), short).is_empty());
        let results = search_ranked_command_sources_using(
            &mut TextMatcher::default(),
            query(&index, &scope, "工具", &[]),
        );
        let chars: Vec<_> = results[0].display.chars().collect();
        let highlighted: String = results[0]
            .indices
            .iter()
            .map(|index| chars[*index as usize])
            .collect();
        assert_eq!(highlighted, "工具");
    }

    #[test]
    fn empty_search_keeps_history_quota_pinned_quick_commands_and_no_duplicate_targets() {
        let scope = CompletionScope::new(&["a"]);
        let mut index = WholeCommandHistoryIndex::default();
        let history: Vec<_> = (0..10)
            .map(|n| CommandHistoryEntry {
                command: format!("fixture history-{n:02}"),
                use_count: 1,
                last_used_at_ms: n,
            })
            .collect();
        index.reconcile(&history, &[record(&scope, "fixture local")]);
        let quick: Vec<QuickCommand> = (0..10).map(|n| serde_json::from_value(serde_json::json!({
            "id":format!("q-{n}"), "label":format!("quick {n}"), "command":format!("fixture quick-{n}"), "pinned":n == 9
        })).unwrap()).collect();
        let results = search_ranked_command_sources_using(
            &mut TextMatcher::default(),
            query(&index, &scope, "", &quick),
        );
        assert_eq!(results.len(), 12);
        assert_eq!(results[0].insert_text, "fixture local");
        assert_eq!(
            results
                .iter()
                .filter(|item| item.kind == CandidateKind::History)
                .count(),
            8
        );
        assert_eq!(results[8].insert_text, "fixture quick-9");
        assert!(results.iter().all(|item| item.indices.is_empty()));
    }
    #[test]
    fn bounded_selection_matches_full_sort_for_all_ranking_ties() {
        use crate::command_search::ranked::{Item, retain_best};
        use std::collections::BinaryHeap;
        let scope = CompletionScope::new(&["rank-equivalence"]);
        let mut index = WholeCommandHistoryIndex::default();
        let records: Vec<_> = (0..1000)
            .map(|i| CommandHistoryEntry {
                command: format!("fixture {i:04}"),
                use_count: (i % 31) + 1,
                last_used_at_ms: u64::from(i % 23),
            })
            .collect();
        index.reconcile(&records, &[]);
        fn item(entry: crate::completion::whole_history::HistorySearchEntry<'_>) -> Item<'_> {
            Item::new(
                entry.command,
                entry.command,
                CandidateKind::History,
                Some(entry),
                entry.statistics.use_count % 7,
                (entry.statistics.use_count % 3) as u8,
                DAY_MS,
            )
        }
        let mut sorted: Vec<_> = index.iter(&scope).map(item).collect();
        sorted.sort_by(Item::compare);
        for limit in [1, 12, 128, 2000] {
            let mut heap = BinaryHeap::new();
            for entry in index.iter(&scope) {
                retain_best(&mut heap, item(entry), limit);
            }
            let actual: Vec<_> = heap
                .into_sorted_vec()
                .into_iter()
                .map(|i| i.command)
                .collect();
            let expected: Vec<_> = sorted.iter().take(limit).map(|i| i.command).collect();
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn cancellation_interrupts_matching_and_empty_history_scans() {
        use crate::command_search::ranked::search_ranked_command_sources_with_cancel;
        let scope = CompletionScope::new(&["cancel"]);
        let mut index = WholeCommandHistoryIndex::default();
        let records: Vec<_> = (0..1000)
            .map(|i| record(&scope, &format!("fixture {i:04}")))
            .collect();
        index.reconcile(&[], &records);
        for pattern in ["fixture", "", "no-match"] {
            let mut checks = 0;
            let result = search_ranked_command_sources_with_cancel(
                &mut TextMatcher::default(),
                query(&index, &scope, pattern, &[]),
                &mut || {
                    checks += 1;
                    checks >= 3
                },
            );
            assert!(result.is_none());
            assert_eq!(checks, 3);
        }
    }
}
