use nyaterm_core::completion::types::{CompletionCandidate, CompletionResult};

/// Ordinary completion has no dependency on directory I/O. The caller starts
/// both providers and revalidates each publication against current UI state.
pub(super) async fn publish_progressively<B, P>(
    ordinary: B,
    paths: P,
    mut publish: impl FnMut(CompletionResult, Option<Vec<CompletionCandidate>>),
) where
    B: std::future::Future<Output = CompletionResult>,
    P: std::future::Future<Output = Vec<CompletionCandidate>>,
{
    let result = ordinary.await;
    publish(result.clone(), None);
    let paths = paths.await;
    if !paths.is_empty() {
        publish(result, Some(paths));
    }
}

#[cfg(test)]
mod tests {
    use super::super::{merge_path_candidates, tests::history_search_fixture};
    use super::publish_progressively;
    use nyaterm_core::completion::types::{
        CandidateKind, CandidateSource, CompletionCandidate, CompletionResult,
    };
    use std::time::Duration;

    #[test]
    fn paths_and_options_both_keep_rows_when_history_is_full() {
        let input = "tool ";
        let candidate = |line: &str, kind| {
            let mut item = history_search_fixture(line, input, 7).candidates.remove(0);
            item.kind = kind;
            item
        };
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
    fn ordinary_results_publish_before_a_blocked_path_provider_and_paths_publish_without_typing() {
        let (paths, waiting) = futures::channel::oneshot::channel();
        let (events, received) = std::sync::mpsc::channel();
        let worker =
            nyaterm_core::runtime::worker::spawn("progressive-completion-test".into(), move || {
                futures::executor::block_on(publish_progressively(
                    std::future::ready(CompletionResult::default()),
                    async { waiting.await.unwrap_or_default() },
                    move |_, paths| {
                        events.send(paths.is_some()).unwrap();
                    },
                ))
            })
            .unwrap();
        let first = received.recv_timeout(Duration::from_secs(2));
        // Always release the provider before asserting, including a regression.
        let _ = paths.send(vec![CompletionCandidate {
            display: "dir/".into(),
            insert_text: "dir/".into(),
            description: String::new(),
            range: 0..0,
            kind: CandidateKind::Argument,
            sources: vec![CandidateSource::Catalog],
            score: 0,
            context_priority: 0,
            indices: Vec::new(),
            fillable: true,
            use_count: 0,
            last_used_at_ms: 0,
        }]);
        assert_eq!(first, Ok(false));
        assert_eq!(received.recv_timeout(Duration::from_secs(2)), Ok(true));
        worker.join().unwrap();
    }
}
