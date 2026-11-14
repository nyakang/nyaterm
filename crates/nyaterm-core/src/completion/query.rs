//! Combine whole-command history and token completion without I/O or a clock.
use crate::command_search::{RankedCommandSourceQuery, search_ranked_command_sources_using};
use crate::completion::history::TokenHistoryIndex;
use crate::completion::matching::TextMatcher;
use crate::completion::types::{
    CompletionCandidate, CompletionFillPolicy, CompletionMode, ShellCommandContext,
};
use crate::completion::whole_history::WholeCommandHistoryIndex;
use crate::completion::{CompletionEngine, TokenCompletionRequest};
use crate::models::quick_commands::QuickCommand;
use std::collections::HashMap;

pub struct CompletionQuery<'a> {
    pub context: &'a ShellCommandContext,
    pub input: &'a str,
    pub mode: CompletionMode,
    pub quick: &'a [QuickCommand],
    pub min: usize,
    pub max: usize,
    pub fill_policy: Option<CompletionFillPolicy>,
    pub now_ms: u64,
}

pub fn search_candidates(
    engine: &mut CompletionEngine,
    history: &TokenHistoryIndex,
    whole: &WholeCommandHistoryIndex,
    query: &CompletionQuery<'_>,
    matcher: &mut TextMatcher,
) -> Vec<CompletionCandidate> {
    let mut candidates = full_history(whole, query, matcher);
    if query.mode == CompletionMode::History {
        return candidates;
    }
    // Whole commands retain their display, kind and statistics when a token
    // provider reaches the same target. Match scores from the two queries are
    // not comparable, so complete commands precede the contextual token group.
    let mut targets: HashMap<_, _> = candidates
        .iter()
        .enumerate()
        .filter_map(|(index, item)| item.target_line(query.input).map(|line| (line, index)))
        .collect();
    let tokens = engine.complete(TokenCompletionRequest {
        fill_policy: query.fill_policy,
        input: query.input,
        cursor: query.input.len(),
        dialect: query.context.dialect,
        scope: query.context.scope.clone(),
        history,
        quick: &TokenHistoryIndex::default(),
        limit: 12,
    });
    for token in tokens {
        let Some(target) = token.target_line(query.input) else {
            continue;
        };
        if let Some(&index) = targets.get(&target) {
            let sources = &mut candidates[index].sources;
            sources.extend(token.sources);
            sources.sort_unstable();
            sources.dedup();
        } else {
            targets.insert(target, candidates.len());
            candidates.push(token);
        }
    }
    candidates.truncate(12);
    candidates
}

fn full_history(
    index: &WholeCommandHistoryIndex,
    query: &CompletionQuery<'_>,
    matcher: &mut TextMatcher,
) -> Vec<CompletionCandidate> {
    search_ranked_command_sources_using(
        matcher,
        RankedCommandSourceQuery {
            fill_policy: query.fill_policy,
            history: index,
            scope: &query.context.scope,
            quick_commands: query.quick,
            pattern: query.input,
            limit: 12,
            min_history_command_length: Some(query.min),
            max_history_command_length: Some(query.max),
            now_ms: query.now_ms,
        },
    )
}

#[cfg(test)]
mod tests;
