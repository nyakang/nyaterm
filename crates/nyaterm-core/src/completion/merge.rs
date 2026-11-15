//! Bounded merging of ordinary and path completion candidates.
use crate::completion::types::{CandidateKind, CompletionCandidate, CompletionResult};

pub const PATH_CANDIDATE_LIMIT: usize = 12;

/// Path rows lead once the user typed a directory part (`cd /va`); a bare
/// name (`cd pr`) keeps history and catalog rows first.
pub fn merge_path_candidates(
    mut result: CompletionResult,
    paths: Vec<CompletionCandidate>,
    explicit_directory: bool,
    input: &str,
) -> CompletionResult {
    if paths.is_empty() {
        return result;
    }
    let mut seen = std::collections::HashSet::new();
    let mut existing = std::mem::take(&mut result.candidates);
    let reserved: Vec<_> = existing
        .iter()
        .filter(|item| matches!(item.kind, CandidateKind::Option | CandidateKind::Subcommand))
        .take(3)
        .cloned()
        .collect();
    let mut merged = Vec::new();
    let mut add = |item: CompletionCandidate| {
        if let Some(target) = item.target_line(input)
            && seen.insert(target)
        {
            merged.push(item);
        }
    };
    for item in reserved {
        add(item);
    }
    existing.truncate(6);
    let paths = paths.into_iter().take(6);
    if !explicit_directory {
        for item in existing {
            add(item);
        }
        for item in paths {
            add(item);
        }
    } else {
        for item in paths {
            add(item);
        }
        for item in existing {
            add(item);
        }
    }
    merged.truncate(PATH_CANDIDATE_LIMIT);
    result.candidates = merged;
    result
}

#[cfg(test)]
mod tests {

    use super::merge_path_candidates;
    use crate::completion::types::{
        CandidateKind, CandidateSource, CompletionCandidate, CompletionResult,
    };

    fn candidate(line: &str, input: &str, kind: CandidateKind) -> CompletionCandidate {
        CompletionCandidate {
            display: line.into(),
            insert_text: line.into(),
            description: String::new(),
            range: 0..input.len(),
            kind,
            sources: vec![CandidateSource::History],
            use_count: 1,
            last_used_at_ms: 1,
            score: 1,
            indices: vec![],
            fillable: true,
            context_priority: 1,
        }
    }
    #[test]
    fn paths_and_options_both_keep_rows_when_history_is_full() {
        let input = "tool ";
        let candidate = |line: &str, kind| candidate(line, input, kind);
        for explicit in [false, true] {
            let mut ordinary = CompletionResult {
                revision: 7,
                candidates: (0..12)
                    .map(|i| candidate(&format!("tool history{i}"), CandidateKind::History))
                    .collect(),
            };
            ordinary.candidates.extend(
                ["tool --all", "tool --help", "tool child"]
                    .into_iter()
                    .map(|line| candidate(line, CandidateKind::Option)),
            );
            let paths = (0..12)
                .map(|i| candidate(&format!("tool path{i}/"), CandidateKind::Argument))
                .collect();
            let result = merge_path_candidates(ordinary, paths, explicit, input);
            assert_eq!(result.revision, 7);
            assert_eq!(result.candidates.len(), 12);
            assert_eq!(
                result
                    .candidates
                    .iter()
                    .filter(|item| item.kind == CandidateKind::Option)
                    .count(),
                3
            );
            assert!(
                result
                    .candidates
                    .iter()
                    .filter(|item| item.kind == CandidateKind::Argument)
                    .count()
                    >= 3
            );
            let targets: std::collections::HashSet<_> = result
                .candidates
                .iter()
                .filter_map(|item| item.target_line(input))
                .collect();
            assert_eq!(targets.len(), result.candidates.len());
        }
    }

    #[test]
    fn empty_paths_preserve_all_ordinary_rows_and_revision() {
        let ordinary = CompletionResult {
            revision: 42,
            candidates: (0..20)
                .map(|i| candidate(&format!("tool {i}"), "tool ", CandidateKind::History))
                .collect(),
        };
        let result = merge_path_candidates(ordinary, Vec::new(), true, "tool ");
        assert_eq!(result.revision, 42);
        assert_eq!(result.candidates.len(), 20);
        assert_eq!(result.candidates[19].insert_text, "tool 19");
    }

    #[test]
    fn directory_context_changes_order_after_reserved_options_and_subcommands() {
        for explicit in [false, true] {
            let ordinary = CompletionResult {
                revision: 3,
                candidates: vec![
                    candidate("tool history", "tool ", CandidateKind::History),
                    candidate("tool --all", "tool ", CandidateKind::Option),
                    candidate("tool child", "tool ", CandidateKind::Subcommand),
                ],
            };
            let result = merge_path_candidates(
                ordinary,
                vec![candidate("tool dir/", "tool ", CandidateKind::Argument)],
                explicit,
                "tool ",
            );
            let lines: Vec<_> = result
                .candidates
                .iter()
                .map(|c| c.insert_text.as_str())
                .collect();
            assert_eq!(
                lines,
                if explicit {
                    vec!["tool --all", "tool child", "tool dir/", "tool history"]
                } else {
                    vec!["tool --all", "tool child", "tool history", "tool dir/"]
                }
            );
        }
    }

    #[test]
    fn merge_deduplicates_full_targets_and_ignores_invalid_ranges() {
        let input = "tool ";
        let mut same_target = candidate("tool dir/", input, CandidateKind::Argument);
        same_target.range = 5..5;
        same_target.insert_text = "dir/".into();
        let mut invalid = candidate("bad", input, CandidateKind::Argument);
        invalid.range = 99..100;
        for explicit in [false, true] {
            let result = merge_path_candidates(
                CompletionResult {
                    revision: 17,
                    candidates: vec![candidate("tool dir/", input, CandidateKind::History)],
                },
                vec![same_target.clone(), invalid.clone()],
                explicit,
                input,
            );
            assert_eq!(result.revision, 17);
            assert_eq!(result.candidates.len(), 1);
            assert_eq!(
                result.candidates[0].target_line(input).as_deref(),
                Some("tool dir/")
            );
            assert_eq!(
                result.candidates[0].kind,
                if explicit {
                    CandidateKind::Argument
                } else {
                    CandidateKind::History
                }
            );
        }
    }

    #[test]
    fn each_provider_is_limited_before_final_merge() {
        let result = merge_path_candidates(
            CompletionResult {
                revision: 9,
                candidates: (0..20)
                    .map(|i| candidate(&format!("ordinary{i}"), "", CandidateKind::History))
                    .collect(),
            },
            (0..20)
                .map(|i| candidate(&format!("path{i}"), "", CandidateKind::Argument))
                .collect(),
            false,
            "",
        );
        assert_eq!(result.candidates.len(), 12);
        assert_eq!(result.candidates[5].insert_text, "ordinary5");
        assert_eq!(result.candidates[6].insert_text, "path0");
        assert_eq!(result.candidates[11].insert_text, "path5");
    }
}
