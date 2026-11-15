use nyaterm_core::completion::types::{CompletionCandidate, CompletionResult};

/// Ordinary completion has no dependency on directory I/O. The caller starts
/// both providers and revalidates each publication against current UI state.
pub async fn publish_progressively<B, P>(
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
    use super::publish_progressively;
    use nyaterm_core::completion::types::{
        CandidateKind, CandidateSource, CompletionCandidate, CompletionResult,
    };
    use std::time::Duration;

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

    #[test]
    fn empty_paths_publish_ordinary_results_once_with_the_original_revision() {
        let mut publications = Vec::new();
        futures::executor::block_on(publish_progressively(
            std::future::ready(CompletionResult {
                revision: 23,
                candidates: Vec::new(),
            }),
            std::future::ready(Vec::new()),
            |result, paths| publications.push((result.revision, paths.is_some())),
        ));
        assert_eq!(publications, vec![(23, false)]);
    }
}
