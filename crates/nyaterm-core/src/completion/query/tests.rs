use super::{full_history as search_whole, search_candidates as search_with_indexes};
use crate::completion::types::{
    CandidateKind, CandidateSource, CompletionCandidate, CompletionFillPolicy, CompletionMode,
    CompletionScope, ScopedHistoryRecord, ShellCommandContext, ShellDialect,
};
use crate::completion::{
    CompletionEngine, history::TokenHistoryIndex, matching::TextMatcher,
    whole_history::WholeCommandHistoryIndex,
};
use crate::models::quick_commands::QuickCommand;
use std::sync::Arc;
struct CompletionQuery {
    context: ShellCommandContext,
    input: String,
    mode: CompletionMode,
    quick: Arc<[QuickCommand]>,
    min: usize,
    max: usize,
    fill_policy: Option<CompletionFillPolicy>,
}
impl CompletionQuery {
    fn borrowed(&self) -> crate::completion::query::CompletionQuery<'_> {
        crate::completion::query::CompletionQuery {
            context: &self.context,
            input: &self.input,
            mode: self.mode,
            quick: &self.quick,
            min: self.min,
            max: self.max,
            fill_policy: self.fill_policy,
            now_ms: 1_000_000,
        }
    }
}
#[derive(Default)]
struct HistoryFixture {
    tokens: TokenHistoryIndex,
    whole: WholeCommandHistoryIndex,
    records: Vec<ScopedHistoryRecord>,
}

impl HistoryFixture {
    fn upsert(&mut self, record: ScopedHistoryRecord) {
        self.tokens.upsert(record.clone());
        self.records.push(record);
        self.whole.reconcile(&[], &self.records);
    }
}

fn search_candidates(
    engine: &mut CompletionEngine,
    index: &HistoryFixture,
    query: &CompletionQuery,
    matcher: &mut TextMatcher,
) -> Vec<CompletionCandidate> {
    search_with_indexes(
        engine,
        &index.tokens,
        &index.whole,
        &query.borrowed(),
        matcher,
    )
}

fn full_history(
    index: &HistoryFixture,
    query: &CompletionQuery,
    matcher: &mut TextMatcher,
) -> Vec<CompletionCandidate> {
    search_whole(&index.whole, &query.borrowed(), matcher)
}

#[test]
fn both_modes_offer_multiline_quick_commands_for_paste_review() {
    use crate::completion::CompletionEngine;
    use crate::completion::matching::TextMatcher;
    use crate::completion::types::CandidateKind;

    let command = "echo first\\\r\n  second\n\necho third";
    let quick = serde_json::from_value(serde_json::json!({
        "id": "multiline", "label": "下载驱动", "command": command
    }))
    .unwrap();
    let mut query = CompletionQuery {
        fill_policy: None,
        context: ShellCommandContext {
            scope: CompletionScope::new(&["multiline"]),
            dialect: ShellDialect::Bash,
            command_events: true,
        },
        input: "下载驱动".into(),
        mode: CompletionMode::History,
        quick: Arc::from([quick]),
        min: 2,
        max: 64,
    };
    let mut engine = CompletionEngine::embedded();
    let mut matcher = TextMatcher::default();
    for mode in [CompletionMode::History, CompletionMode::Token] {
        query.mode = mode;
        for input in ["下载驱动", ""] {
            query.input = input.into();
            let candidates = search_candidates(
                &mut engine,
                &HistoryFixture::default(),
                &query,
                &mut matcher,
            );
            let candidate = &candidates[0];
            assert_eq!(candidate.kind, CandidateKind::QuickCommand);
            assert_eq!(candidate.display, "下载驱动");
            assert_eq!(candidate.target_line(input).as_deref(), Some(command));
            assert!(candidate.fillable);
        }
    }
}

#[test]
fn both_modes_find_cross_word_history_and_named_quick_commands() {
    use crate::completion::CompletionEngine;
    use crate::completion::matching::TextMatcher;
    use crate::models::quick_commands::QuickCommand;
    use std::collections::BTreeMap;

    let scope = CompletionScope::new(&["whole-command-search"]);
    let mut index = HistoryFixture::default();
    for origin in [scope.clone(), CompletionScope::legacy()] {
        index.upsert(ScopedHistoryRecord {
            version: 1,
            scope: origin,
            dialect: ShellDialect::Bash,
            command: "systemctl restart mihomo.service".into(),
            use_count: 3,
            last_used_at_ms: 10,
            extensions: BTreeMap::new(),
        });
    }
    let quick: QuickCommand = serde_json::from_value(serde_json::json!({
        "id": "restart-service", "label": "重启代理", "command": "systemctl restart example.service"
    }))
    .unwrap();
    let unnamed: QuickCommand = serde_json::from_value(serde_json::json!({
        "id": "unnamed", "label": "", "command": "internal-tool deploy"
    }))
    .unwrap();
    let mut query = CompletionQuery {
        fill_policy: None,
        context: ShellCommandContext {
            scope,
            dialect: ShellDialect::Bash,
            command_events: true,
        },
        input: "systmi".into(),
        mode: CompletionMode::History,
        quick: Arc::from([quick, unnamed]),
        min: 2,
        max: 64,
    };
    let mut engine = CompletionEngine::embedded();
    let mut matcher = TextMatcher::default();
    for mode in [CompletionMode::History, CompletionMode::Token] {
        query.mode = mode;
        for (input, command, display) in [
            (
                "systmi",
                "systemctl restart mihomo.service",
                "systemctl restart mihomo.service",
            ),
            ("重启代理", "systemctl restart example.service", "重启代理"),
            ("internal-t", "internal-tool deploy", "internal-tool deploy"),
        ] {
            query.input = input.into();
            let candidates = search_candidates(&mut engine, &index, &query, &mut matcher);
            assert_eq!(candidates[0].insert_text, command);
            assert_eq!(candidates[0].display, display);
            assert_eq!(candidates[0].range, 0..input.len());
            assert!(candidates[0].fillable);
            assert_eq!(
                candidates
                    .iter()
                    .filter(|c| c.insert_text == command)
                    .count(),
                1
            );
        }
    }
    query.context.scope = CompletionScope::new(&["other-host"]);
    query.input = "systmi".into();
    let candidates = search_candidates(&mut engine, &index, &query, &mut matcher);
    assert_eq!(candidates.len(), 1);
    assert_eq!(
        candidates[0].sources,
        [CandidateSource::History, CandidateSource::LegacyHistory]
    );
}

#[test]
fn automatic_search_supplements_whole_commands_and_deduplicates_target_lines() {
    use crate::completion::CompletionEngine;
    use crate::completion::matching::TextMatcher;
    use crate::models::quick_commands::QuickCommand;
    use std::collections::HashSet;

    let quick: QuickCommand = serde_json::from_value(serde_json::json!({
        "id": "docker", "label": "dock", "command": "docker"
    }))
    .unwrap();
    let mut query = CompletionQuery {
        fill_policy: None,
        context: ShellCommandContext {
            scope: CompletionScope::legacy(),
            dialect: ShellDialect::Bash,
            command_events: false,
        },
        input: "dock".into(),
        mode: CompletionMode::History,
        quick: Arc::from([quick]),
        min: 2,
        max: 64,
    };
    let index = HistoryFixture::default();
    let mut engine = CompletionEngine::embedded();
    let mut matcher = TextMatcher::default();
    let manual = search_candidates(&mut engine, &index, &query, &mut matcher);
    assert_eq!(manual.len(), 1);
    assert_eq!(
        manual[0].kind,
        crate::completion::types::CandidateKind::QuickCommand
    );
    query.mode = CompletionMode::Token;
    let automatic = search_candidates(&mut engine, &index, &query, &mut matcher);
    assert_eq!(automatic[0].display, "dock");
    assert_eq!(automatic[0].kind, manual[0].kind);
    assert_eq!(
        automatic[0].sources,
        [CandidateSource::Catalog, CandidateSource::QuickCommand]
    );
    let targets: HashSet<_> = automatic
        .iter()
        .map(|c| c.target_line(&query.input).unwrap())
        .collect();
    assert_eq!(automatic.len(), targets.len());

    query.input = "docker run --na".into();
    query.quick = Arc::from([]);
    assert!(
        search_candidates(&mut engine, &index, &query, &mut matcher)
            .iter()
            .any(|c| c.insert_text == "--name")
    );
    query.mode = CompletionMode::History;
    assert!(search_candidates(&mut engine, &index, &query, &mut matcher).is_empty());
}

#[test]
fn whole_commands_keep_their_order_and_limit_before_catalog_supplements() {
    use crate::completion::CompletionEngine;
    use crate::completion::matching::TextMatcher;
    use std::collections::BTreeMap;

    let scope = CompletionScope::new(&["ranking-fixture"]);
    let mut index = HistoryFixture::default();
    for n in 0..16 {
        index.upsert(ScopedHistoryRecord {
            version: 1,
            scope: scope.clone(),
            dialect: ShellDialect::Bash,
            command: format!("docker run fixture-{n}"),
            use_count: 1,
            last_used_at_ms: n,
            extensions: BTreeMap::new(),
        });
    }
    let mut query = CompletionQuery {
        fill_policy: None,
        context: ShellCommandContext {
            scope,
            dialect: ShellDialect::Bash,
            command_events: true,
        },
        input: "dock".into(),
        mode: CompletionMode::History,
        quick: Arc::from([]),
        min: 2,
        max: 64,
    };
    let mut engine = CompletionEngine::embedded();
    let mut matcher = TextMatcher::default();
    let manual = search_candidates(&mut engine, &index, &query, &mut matcher);
    query.mode = CompletionMode::Token;
    let automatic = search_candidates(&mut engine, &index, &query, &mut matcher);
    assert_eq!(automatic.len(), 12);
    assert_eq!(
        automatic.iter().map(|c| &c.insert_text).collect::<Vec<_>>(),
        manual.iter().map(|c| &c.insert_text).collect::<Vec<_>>()
    );
    query.max = 3;
    query.mode = CompletionMode::History;
    assert!(search_candidates(&mut engine, &index, &query, &mut matcher).is_empty());
}
#[test]
fn full_history_merges_origins_and_uses_current_scope_statistics_once() {
    use crate::completion::matching::TextMatcher;
    use crate::models::quick_commands::QuickCommand;
    use std::collections::BTreeMap;
    let scope = CompletionScope::new(&["history-origin-fixture"]);
    let mut index = HistoryFixture::default();
    for (scope, count, time) in [(scope.clone(), 8, 2), (CompletionScope::legacy(), 10, 1)] {
        index.upsert(ScopedHistoryRecord {
            version: 1,
            scope,
            dialect: ShellDialect::Bash,
            command: "tool run".into(),
            use_count: count,
            last_used_at_ms: time,
            extensions: BTreeMap::new(),
        });
    }
    let quick: QuickCommand = serde_json::from_value(serde_json::json!({
        "id":"origin-fixture", "label":"tool run", "command":"tool run", "use_count":99
    }))
    .unwrap();
    let query = CompletionQuery {
        fill_policy: None,
        context: ShellCommandContext {
            scope,
            dialect: ShellDialect::Bash,
            command_events: true,
        },
        input: "tool".into(),
        mode: CompletionMode::History,
        quick: Arc::from([quick]),
        min: 1,
        max: 100,
    };
    let candidates = full_history(&index, &query, &mut TextMatcher::default());
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].use_count, 8);
    assert_eq!(candidates[0].last_used_at_ms, 2);
    assert_eq!(
        candidates[0].sources,
        [
            CandidateSource::History,
            CandidateSource::QuickCommand,
            CandidateSource::LegacyHistory
        ]
    );
}

#[test]
fn both_modes_find_other_host_whole_commands_without_learning_its_argument_tokens() {
    let a = CompletionScope::new(&["host-a"]);
    let b = CompletionScope::new(&["host-b"]);
    let mut index = HistoryFixture::default();
    index.upsert(ScopedHistoryRecord {
        version: 1,
        scope: a.clone(),
        dialect: ShellDialect::Bash,
        command: "curl https://www.baidu.com".into(),
        use_count: 1,
        last_used_at_ms: 1,
        extensions: std::collections::BTreeMap::new(),
    });
    let mut engine = CompletionEngine::embedded();
    let mut matcher = TextMatcher::default();
    for scope in [a, b] {
        for mode in [CompletionMode::Token, CompletionMode::History] {
            let query = CompletionQuery {
                fill_policy: None,
                context: ShellCommandContext {
                    scope: scope.clone(),
                    dialect: ShellDialect::Bash,
                    command_events: true,
                },
                input: "curl h".into(),
                mode,
                quick: Arc::from([]),
                min: 2,
                max: 64,
            };
            let results = search_candidates(&mut engine, &index, &query, &mut matcher);
            let history = results
                .iter()
                .find(|item| item.insert_text == "curl https://www.baidu.com")
                .unwrap();
            assert_eq!(history.kind, CandidateKind::History);
            assert_eq!(
                history.target_line("curl h").as_deref(),
                Some("curl https://www.baidu.com")
            );
            assert!(history.fillable);
            assert!(
                !results
                    .iter()
                    .any(|item| item.kind == CandidateKind::Argument
                        && item.insert_text == "https://www.baidu.com")
            );
        }
    }
}
