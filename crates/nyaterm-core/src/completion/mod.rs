//! UI-independent, declaration-only command completion.
pub mod catalog;
pub mod history;
pub mod matching;
pub mod parser;
pub mod privacy;
pub mod types;

use crate::completion::catalog::Catalog;
use crate::completion::history::TokenHistoryIndex;
use crate::completion::matching::TextMatcher;
use crate::completion::parser::parse_context;
use crate::completion::types::{
    CandidateKind, CandidateSource, CompletionCandidate, CompletionScope, ShellDialect,
};
use crate::terminal::editing::supports_scalar_editing;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

pub struct CompletionEngine {
    catalog: Option<Arc<Catalog>>,
    matcher: TextMatcher,
}
impl CompletionEngine {
    pub fn embedded() -> Self {
        static CATALOG: OnceLock<Option<Arc<Catalog>>> = OnceLock::new();
        Self {
            catalog: CATALOG
                .get_or_init(|| Catalog::embedded().ok().map(Arc::new))
                .clone(),
            matcher: TextMatcher::default(),
        }
    }
    pub fn complete(&mut self, request: TokenCompletionRequest<'_>) -> Vec<CompletionCandidate> {
        let TokenCompletionRequest {
            input,
            cursor,
            dialect,
            scope,
            history,
            quick,
            limit,
        } = request;
        let Some(mut context) = parse_context(input, cursor, dialect, scope) else {
            return Vec::new();
        };
        // Catalog inference may adjust the value range (e.g. --format=json).
        let static_items = self
            .catalog
            .as_ref()
            .map(|catalog| catalog.candidates(&mut context))
            .unwrap_or_default();
        self.matcher.set_query(&context.prefix);
        let mut merged = HashMap::<String, RankedCandidate<'_>>::new();
        let native_case = dialect.case_insensitive_commands()
            && (context.preceding.is_empty()
                || (context.option_value.is_none()
                    && context
                        .preceding
                        .first()
                        .is_some_and(|root| dialect.native_options(root))
                    && (context.prefix.starts_with('-')
                        || dialect == ShellDialect::Cmd && context.prefix.starts_with('/'))));
        let learned_kind = if context.preceding.is_empty() {
            CandidateKind::Command
        } else {
            CandidateKind::Argument
        };
        let static_iter = static_items.iter().map(|(text, description, kind)| {
            (
                text.as_str(),
                description.as_str(),
                *kind,
                CandidateSource::Catalog,
                0,
                0,
            )
        });
        let history_iter = history
            .candidate_iter(&context.scope, &context.preceding, dialect)
            .map(|(text, count, time)| {
                (
                    text,
                    "",
                    learned_kind,
                    CandidateSource::History,
                    count,
                    time,
                )
            });
        let quick_iter = quick
            .candidate_iter(&context.scope, &context.preceding, dialect)
            .map(|(text, _, _)| (text, "", learned_kind, CandidateSource::QuickCommand, 0, 0));
        for (text, description, kind, source, count, time) in
            static_iter.chain(history_iter).chain(quick_iter)
        {
            // Score borrowed words first; only matching words need a dedup key.
            let Some(score) = self.matcher.score(text) else {
                continue;
            };
            if !crate::completion::privacy::safe_word(text, dialect) {
                continue;
            }
            let prefix = text.starts_with(&context.prefix);
            let key = if native_case {
                text.to_ascii_lowercase()
            } else {
                text.to_string()
            };
            let item = merged.entry(key).or_insert(RankedCandidate {
                text,
                description,
                kind,
                sources: 0,
                count: 0,
                time: 0,
                score,
                prefix,
                priority: if kind == CandidateKind::Command || context.option_value.is_some() {
                    2
                } else {
                    1
                },
            });
            // Case-insensitive native spellings still choose a stable display
            // when separately recorded commands differ only in letter case.
            if native_case && text < item.text && source != CandidateSource::QuickCommand {
                item.text = text;
                item.score = score;
                item.prefix = prefix;
            }
            item.sources |= 1 << source as u8;
            // The index already aggregates per-command contributions. A second
            // provider of the same word must not count them again.
            item.count = item.count.max(count);
            item.time = item.time.max(time);
            if item.description.is_empty() {
                item.description = description;
            }
            if source == CandidateSource::Catalog {
                item.kind = kind;
                item.priority = 2;
            }
        }
        let mut ranked: Vec<_> = merged.into_values().collect();
        ranked.sort_unstable_by(|a, b| {
            b.priority
                .cmp(&a.priority)
                .then(b.prefix.cmp(&a.prefix))
                .then(b.score.cmp(&a.score))
                .then(b.count.cmp(&a.count))
                .then(b.time.cmp(&a.time))
                .then(a.sources.cmp(&b.sources))
                .then(a.text.cmp(b.text))
        });
        ranked.truncate(limit);
        ranked
            .into_iter()
            .map(|item| {
                let mut insert_text = item.text.to_string();
                if native_case
                    && insert_text
                        .get(..context.prefix.len())
                        .is_some_and(|p| p.eq_ignore_ascii_case(&context.prefix))
                {
                    insert_text.replace_range(..context.prefix.len(), &context.prefix);
                }
                let mut candidate = CompletionCandidate {
                    display: item.text.into(),
                    insert_text,
                    description: item.description.into(),
                    range: context.token_range.clone(),
                    kind: item.kind,
                    sources: [
                        CandidateSource::Catalog,
                        CandidateSource::History,
                        CandidateSource::QuickCommand,
                    ]
                    .into_iter()
                    .filter(|source| item.sources & (1 << *source as u8) != 0)
                    .collect(),
                    use_count: item.count,
                    last_used_at_ms: item.time,
                    score: item.score,
                    indices: self.matcher.indices(item.text),
                    fillable: false,
                    context_priority: item.priority,
                };
                candidate.fillable = candidate.target_line(input).is_some_and(|target| {
                    target.starts_with(input) && supports_scalar_editing(&target)
                });
                candidate
            })
            .collect()
    }
}

pub struct TokenCompletionRequest<'a> {
    pub input: &'a str,
    pub cursor: usize,
    pub dialect: ShellDialect,
    pub scope: CompletionScope,
    pub history: &'a TokenHistoryIndex,
    pub quick: &'a TokenHistoryIndex,
    pub limit: usize,
}
struct RankedCandidate<'a> {
    text: &'a str,
    description: &'a str,
    kind: CandidateKind,
    sources: u8,
    count: u32,
    time: u64,
    score: u32,
    prefix: bool,
    priority: u8,
}

#[cfg(test)]
mod tests {
    use crate::completion::history::TokenHistoryIndex;
    use crate::completion::types::{
        CandidateSource, CompletionScope, ScopedHistoryRecord, ShellDialect,
    };
    use crate::completion::{CompletionEngine, TokenCompletionRequest};
    use std::collections::BTreeMap;
    fn record(scope: CompletionScope, command: &str) -> ScopedHistoryRecord {
        ScopedHistoryRecord {
            version: 1,
            scope,
            dialect: ShellDialect::Bash,
            command: command.into(),
            use_count: 3,
            last_used_at_ms: 1,
            extensions: BTreeMap::new(),
        }
    }
    #[test]
    fn static_and_learned_tokens_complete_without_whole_history_rows() {
        let mut engine = CompletionEngine::embedded();
        let scope = CompletionScope::new(&["host-a"]);
        let mut history = TokenHistoryIndex::default();
        for line in [
            "docker ps",
            "docker run nginx",
            "docker run -d redis",
            "internal-tool deploy",
        ] {
            history.upsert(record(scope.clone(), line));
        }
        let quick = TokenHistoryIndex::default();
        for (line, expected) in [
            ("dock", "docker"),
            ("docker r", "run"),
            ("docker run --na", "--name"),
            ("internal-t", "internal-tool"),
            ("docker run ng", "nginx"),
        ] {
            let result = engine.complete(TokenCompletionRequest {
                input: line,
                cursor: line.len(),
                dialect: ShellDialect::Bash,
                scope: scope.clone(),
                history: &history,
                quick: &quick,
                limit: 12,
            });
            let item = result
                .iter()
                .find(|r| r.insert_text == expected)
                .unwrap_or_else(|| panic!("missing candidate for fixture {line:?}: {expected}"));
            assert!(item.fillable);
            assert!(item.target_line(line).unwrap().starts_with(line));
        }
        let result = engine.complete(TokenCompletionRequest {
            input: "docker r",
            cursor: 8,
            dialect: ShellDialect::Bash,
            scope: scope.clone(),
            history: &history,
            quick: &quick,
            limit: 12,
        });
        assert_eq!(result.iter().filter(|r| r.insert_text == "run").count(), 1);
        assert!(
            result
                .iter()
                .find(|r| r.insert_text == "run")
                .unwrap()
                .sources
                .contains(&CandidateSource::History)
        );
        assert!(
            engine
                .complete(TokenCompletionRequest {
                    input: "internal-t",
                    cursor: 10,
                    dialect: ShellDialect::Bash,
                    scope: CompletionScope::new(&["host-b"]),
                    history: &history,
                    quick: &quick,
                    limit: 12
                })
                .is_empty()
        );
        history.remove_command("internal-tool deploy");
        assert!(
            engine
                .complete(TokenCompletionRequest {
                    input: "internal-t",
                    cursor: 10,
                    dialect: ShellDialect::Bash,
                    scope,
                    history: &history,
                    quick: &quick,
                    limit: 12
                })
                .is_empty()
        );
    }
    #[test]
    fn native_windows_names_keep_typed_case() {
        let mut engine = CompletionEngine::embedded();
        let scope = CompletionScope::new(&["windows"]);
        let mut history = TokenHistoryIndex::default();
        let mut r = record(scope.clone(), "Get-Process");
        r.dialect = ShellDialect::PowerShell;
        history.upsert(r);
        let items = engine.complete(TokenCompletionRequest {
            input: "get-pr",
            cursor: 6,
            dialect: ShellDialect::PowerShell,
            scope,
            history: &history,
            quick: &TokenHistoryIndex::default(),
            limit: 12,
        });
        assert!(
            items
                .iter()
                .any(|c| c.insert_text == "get-process" && c.fillable)
        );
    }
    #[test]
    fn native_options_paths_and_unknown_inline_values_keep_raw_suffixes() {
        let mut engine = CompletionEngine::embedded();
        for (dialect, command, input, expected) in [
            (
                ShellDialect::PowerShell,
                "Get-Process -Name worker",
                "get-process -na",
                "-name",
            ),
            (
                ShellDialect::PowerShell,
                "Get-Process -Name:worker",
                "get-process -name:wo",
                "worker",
            ),
            (ShellDialect::Cmd, "DIR /W", "dir /w", "/w"),
            (
                ShellDialect::Cmd,
                r"type C:\logs\sample.txt",
                r"type C:\logs\sa",
                r"C:\logs\sample.txt",
            ),
            (
                ShellDialect::Bash,
                "internal-cli --format=json",
                "internal-cli --format=js",
                "json",
            ),
            (
                ShellDialect::Bash,
                "internal-cli --format=json apply staging",
                "internal-cli --format=json apply st",
                "staging",
            ),
            (
                ShellDialect::Bash,
                "internal-cli -- --format=json",
                "internal-cli -- --format=js",
                "--format=json",
            ),
            (
                ShellDialect::PowerShell,
                "internal-cli -Mode:worker",
                "internal-cli -Mode:wo",
                "-Mode:worker",
            ),
        ] {
            let scope = CompletionScope::new(&["native-fixture"]);
            let mut history = TokenHistoryIndex::default();
            let mut fixture = record(scope.clone(), command);
            fixture.dialect = dialect;
            history.upsert(fixture);
            let candidates = engine.complete(TokenCompletionRequest {
                input,
                cursor: input.len(),
                dialect,
                scope,
                history: &history,
                quick: &TokenHistoryIndex::default(),
                limit: 12,
            });
            let candidate = candidates
                .iter()
                .find(|item| item.insert_text == expected)
                .unwrap_or_else(|| panic!("missing fixture candidate for {dialect:?}"));
            assert!(candidate.fillable);
            assert!(candidate.target_line(input).unwrap().starts_with(input));
        }
    }
    #[test]
    fn missing_catalog_keeps_history_and_quick_token_providers_available() {
        let mut engine = CompletionEngine::embedded();
        engine.catalog = None;
        let scope = CompletionScope::new(&["fallback-fixture"]);
        let mut history = TokenHistoryIndex::default();
        let mut quick = TokenHistoryIndex::default();
        history.upsert(record(scope.clone(), "internal-tool deploy"));
        quick.upsert(record(scope.clone(), "internal-tool deploy"));
        let input = "internal-tool de";
        let candidates = engine.complete(TokenCompletionRequest {
            input,
            cursor: input.len(),
            dialect: ShellDialect::Bash,
            scope,
            history: &history,
            quick: &quick,
            limit: 12,
        });
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].insert_text, "deploy");
        assert_eq!(
            candidates[0].sources,
            [CandidateSource::History, CandidateSource::QuickCommand]
        );
        assert!(candidates[0].fillable);
    }
    #[test]
    fn native_aliases_learn_their_options_without_importing_posix_definitions() {
        let mut engine = CompletionEngine::embedded();
        for (dialect, command, input, expected) in [
            (
                ShellDialect::PowerShell,
                "LS -Recurse",
                "ls -re",
                "-recurse",
            ),
            (ShellDialect::Cmd, "SORT /R", "sort /r", "/r"),
        ] {
            let scope = CompletionScope::new(&["native-alias-fixture"]);
            let mut history = TokenHistoryIndex::default();
            let mut fixture = record(scope.clone(), command);
            fixture.dialect = dialect;
            history.upsert(fixture);
            let candidates = engine.complete(TokenCompletionRequest {
                input,
                cursor: input.len(),
                dialect,
                scope,
                history: &history,
                quick: &TokenHistoryIndex::default(),
                limit: 12,
            });
            assert_eq!(candidates.len(), 1);
            assert_eq!(candidates[0].insert_text, expected);
            assert_eq!(candidates[0].sources, [CandidateSource::History]);
            assert!(candidates[0].fillable);
        }
    }
    #[test]
    fn exact_command_prefix_precedes_more_frequent_fuzzy_matches_and_merges_statistics_once() {
        let mut engine = CompletionEngine::embedded();
        let scope = CompletionScope::new(&["rank-fixture"]);
        let mut history = TokenHistoryIndex::default();
        let mut quick = TokenHistoryIndex::default();
        history.upsert(record(scope.clone(), "internal-tool run"));
        let mut frequent = record(scope.clone(), "intermediate-extra-tool run");
        frequent.use_count = 1000;
        history.upsert(frequent);
        quick.upsert(record(scope.clone(), "internal-tool run"));
        let input = "internal-t";
        let candidates = engine.complete(TokenCompletionRequest {
            input,
            cursor: input.len(),
            dialect: ShellDialect::Bash,
            scope,
            history: &history,
            quick: &quick,
            limit: 12,
        });
        assert_eq!(candidates[0].insert_text, "internal-tool");
        assert_eq!(candidates[0].use_count, 3);
        assert_eq!(
            candidates[0].sources,
            [CandidateSource::History, CandidateSource::QuickCommand]
        );
    }
}
