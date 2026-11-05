use super::TransferFeatureState;
use nyaterm_files::file_browser::{
    FileBrowserBackendKind, file_browser_identity, file_browser_parent,
};
use nyaterm_files::sftp::RemoteFilePath;
use std::collections::{HashMap, HashSet};

pub(super) struct TransferDeleteBatch {
    backend: FileBrowserBackendKind,
    pending: HashMap<String, RemoteFilePath>,
    attempted: Vec<RemoteFilePath>,
    deleted: Vec<RemoteFilePath>,
    errors: Vec<String>,
}

pub(in crate::features::transfers) struct TransferDeleteOutcome {
    pub backend: FileBrowserBackendKind,
    pub attempted: Vec<RemoteFilePath>,
    pub deleted: Vec<RemoteFilePath>,
    pub errors: Vec<String>,
}

fn parent(backend: FileBrowserBackendKind, path: &RemoteFilePath) -> RemoteFilePath {
    let display = file_browser_parent(backend, &path.display_path);
    if backend == FileBrowserBackendKind::Remote
        && let Ok(Some(raw)) = path.raw_path()
        && let Some(index) = raw.iter().rposition(|byte| *byte == b'/')
    {
        return RemoteFilePath::from_raw(display, &raw[..index.max(1)]);
    }
    RemoteFilePath::new(display)
}

fn within(
    backend: FileBrowserBackendKind,
    current: &RemoteFilePath,
    ancestor: &RemoteFilePath,
) -> bool {
    if backend == FileBrowserBackendKind::Remote
        && let (Ok(Some(current)), Ok(Some(ancestor))) = (current.raw_path(), ancestor.raw_path())
    {
        let ancestor = ancestor.strip_suffix(b"/").unwrap_or(&ancestor);
        return current == ancestor
            || current
                .strip_prefix(ancestor)
                .is_some_and(|rest| rest.starts_with(b"/"));
    }
    let current = file_browser_identity(backend, &current.display_path);
    let ancestor = file_browser_identity(backend, &ancestor.display_path);
    let ancestor = ancestor.trim_end_matches(['/', '\\']);
    current == ancestor
        || current.strip_prefix(ancestor).is_some_and(|rest| {
            rest.starts_with('/')
                || (backend == FileBrowserBackendKind::Local && rest.starts_with('\\'))
        })
}

impl TransferDeleteOutcome {
    pub fn parents(&self) -> Vec<RemoteFilePath> {
        let mut seen = HashSet::new();
        self.attempted
            .iter()
            .map(|path| parent(self.backend, path))
            .filter(|path| {
                let identity = if path.raw_path_token.is_some() {
                    path.identity_key()
                } else {
                    file_browser_identity(self.backend, &path.display_path)
                };
                seen.insert(identity)
            })
            .collect()
    }

    pub fn refresh_target(&self, current: &RemoteFilePath) -> Option<RemoteFilePath> {
        // Pick the outermost deleted ancestor if both a parent and child were selected.
        let ancestor = self
            .deleted
            .iter()
            .filter(|path| within(self.backend, current, path))
            .min_by_key(|path| path.display_path.len());
        if let Some(ancestor) = ancestor {
            return Some(parent(self.backend, ancestor));
        }
        // A failed directory deletion may still have removed some of its children.
        if self
            .attempted
            .iter()
            .any(|path| within(self.backend, current, path))
        {
            return Some(current.clone());
        }
        self.parents()
            .iter()
            .any(|path| within(self.backend, current, path) && within(self.backend, path, current))
            .then(|| current.clone())
    }
}

impl TransferDeleteBatch {
    fn new(backend: FileBrowserBackendKind, jobs: Vec<(String, RemoteFilePath)>) -> Self {
        Self {
            backend,
            attempted: jobs.iter().map(|(_, path)| path.clone()).collect(),
            pending: jobs.into_iter().collect(),
            deleted: Vec::new(),
            errors: Vec::new(),
        }
    }

    fn settle(&mut self, job_id: &str, result: Result<(), String>) -> bool {
        let Some(path) = self.pending.remove(job_id) else {
            return false;
        };
        match result {
            Ok(()) => self.deleted.push(path),
            Err(error) => self.errors.push(error),
        }
        self.pending.is_empty()
    }

    fn outcome(self) -> TransferDeleteOutcome {
        TransferDeleteOutcome {
            backend: self.backend,
            attempted: self.attempted,
            deleted: self.deleted,
            errors: self.errors,
        }
    }
}

impl TransferFeatureState {
    pub(in crate::features) fn begin_delete_batch(
        &mut self,
        batch_id: String,
        backend: FileBrowserBackendKind,
        jobs: Vec<(String, RemoteFilePath)>,
    ) {
        self.file_ops
            .delete_batches
            .insert(batch_id, TransferDeleteBatch::new(backend, jobs));
    }

    pub(in crate::features::transfers) fn settle_delete_job(
        &mut self,
        batch_id: &str,
        job_id: &str,
        result: Result<(), String>,
    ) -> Option<TransferDeleteOutcome> {
        let batch = self.file_ops.delete_batches.get_mut(batch_id)?;
        if !batch.settle(job_id, result) {
            return None;
        }
        self.file_ops
            .delete_batches
            .remove(batch_id)
            .map(TransferDeleteBatch::outcome)
    }

    pub(in crate::features::transfers) fn invalidate_delete_batch(
        &mut self,
        session: &str,
        outcome: &TransferDeleteOutcome,
    ) {
        for path in &outcome.attempted {
            self.invalidate_tree_path(session, path, true);
        }
        for path in outcome.parents() {
            self.invalidate_tree_path(session, &path, false);
        }
        if let Some(cache) = self.browser.session_cache.get_mut(session) {
            let current = RemoteFilePath {
                display_path: cache.current_path.clone(),
                raw_path_token: cache.current_raw_path_token.clone(),
            };
            if let Some(target) = outcome.refresh_target(&current) {
                cache.current_path = target.display_path;
                cache.current_raw_path_token = target.raw_path_token;
                self.file_ops
                    .delete_refresh_pending
                    .insert(session.to_owned());
            }
        }
    }

    pub(in crate::features) fn take_delete_refresh_pending(&mut self, session: &str) -> bool {
        self.file_ops.delete_refresh_pending.remove(session)
    }
}

#[cfg(test)]
mod tests {
    use super::TransferDeleteBatch;
    use nyaterm_files::file_browser::FileBrowserBackendKind;
    use nyaterm_files::sftp::RemoteFilePath;

    #[cfg(windows)]
    #[test]
    fn deleting_a_wsl_directory_does_not_refresh_its_case_distinct_sibling() {
        let root = r"\\wsl.localhost\Ubuntu\home\user";
        let mut batch = TransferDeleteBatch::new(
            FileBrowserBackendKind::Local,
            vec![(
                "upper".into(),
                RemoteFilePath::new(format!(r"{root}\Project")),
            )],
        );
        assert!(batch.settle("upper", Ok(())));
        let outcome = batch.outcome();
        assert_eq!(
            outcome.refresh_target(&RemoteFilePath::new(format!(r"{root}\project\child"))),
            None
        );
        assert_eq!(
            outcome.refresh_target(&RemoteFilePath::new(
                r"\\wsl$\UBUNTU\home\user\Project\child"
            )),
            Some(RemoteFilePath::new(root))
        );

        let mut batch = TransferDeleteBatch::new(
            FileBrowserBackendKind::Local,
            vec![
                (
                    "upper".into(),
                    RemoteFilePath::new(format!(r"{root}\Project\a")),
                ),
                (
                    "lower".into(),
                    RemoteFilePath::new(format!(r"{root}\project\b")),
                ),
            ],
        );
        batch.settle("upper", Ok(()));
        batch.settle("lower", Ok(()));
        assert_eq!(batch.outcome().parents().len(), 2);
    }

    #[test]
    fn batch_waits_for_all_results_and_refreshes_each_parent_once() {
        let mut batch = TransferDeleteBatch::new(
            FileBrowserBackendKind::Remote,
            vec![
                ("a".into(), RemoteFilePath::new("/dir/a")),
                ("b".into(), RemoteFilePath::new("/dir/b")),
            ],
        );
        assert!(!batch.settle("b", Err("permission denied".into())));
        assert!(!batch.settle("b", Ok(())));
        assert!(batch.settle("a", Ok(())));
        let outcome = batch.outcome();
        assert_eq!(outcome.deleted.len(), 1);
        assert_eq!(outcome.errors.len(), 1);
        assert_eq!(outcome.parents(), vec![RemoteFilePath::new("/dir")]);
        assert_eq!(
            outcome.refresh_target(&RemoteFilePath::new("/elsewhere")),
            None
        );
        assert_eq!(
            outcome.refresh_target(&RemoteFilePath::new("/dir")),
            Some(RemoteFilePath::new("/dir"))
        );
    }

    #[test]
    fn deleting_ancestor_returns_to_surviving_parent_without_prefix_collision() {
        let mut batch = TransferDeleteBatch::new(
            FileBrowserBackendKind::Remote,
            vec![
                ("child".into(), RemoteFilePath::new("/dir/a/sub")),
                ("parent".into(), RemoteFilePath::new("/dir/a")),
            ],
        );
        batch.settle("child", Ok(()));
        batch.settle("parent", Ok(()));
        let outcome = batch.outcome();
        assert_eq!(
            outcome.refresh_target(&RemoteFilePath::new("/dir/a/sub/deep")),
            Some(RemoteFilePath::new("/dir"))
        );
        assert_eq!(
            outcome.refresh_target(&RemoteFilePath::new("/dir/abc")),
            None
        );
    }

    #[test]
    fn refresh_preserves_non_utf8_parent_identity_and_partial_failure() {
        let mut batch = TransferDeleteBatch::new(
            FileBrowserBackendKind::Remote,
            vec![(
                "a".into(),
                RemoteFilePath::from_raw("/dir/�/file", b"/dir/\xff/file"),
            )],
        );
        assert!(batch.settle("a", Err("partial deletion".into())));
        let outcome = batch.outcome();
        let current = RemoteFilePath::from_raw("/dir/�", b"/dir/\xff");
        assert_eq!(outcome.parents(), vec![current.clone()]);
        assert_eq!(outcome.refresh_target(&current), Some(current));
        assert_eq!(
            outcome.refresh_target(&RemoteFilePath::from_raw("/dir/�", b"/dir/\xfe")),
            None
        );
    }

    #[test]
    fn partial_directory_failure_refreshes_the_current_descendant_without_navigating() {
        let mut batch = TransferDeleteBatch::new(
            FileBrowserBackendKind::Remote,
            vec![("delete".into(), RemoteFilePath::new("/dir"))],
        );
        batch.settle("delete", Err("partial deletion".into()));
        let outcome = batch.outcome();
        for path in ["/dir", "/dir/sub"] {
            let current = RemoteFilePath::new(path);
            assert_eq!(outcome.refresh_target(&current), Some(current));
        }
        assert_eq!(
            outcome.refresh_target(&RemoteFilePath::new("/directory")),
            None
        );
    }
}
