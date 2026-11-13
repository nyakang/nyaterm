use super::TransferFeatureState;
use nyaterm_files::remote_file::RemoteTextGeneration;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

/// A watcher can start overlapping uploads sharing a queue row. Count the actual
/// work lifetime so an older completion cannot permit an identity switch early.
pub(in crate::features) struct ExternalFileTask(Arc<AtomicUsize>);
impl Drop for ExternalFileTask {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}
impl TransferFeatureState {
    pub(in crate::features) fn begin_external_file_task(
        &mut self,
        session: &str,
    ) -> ExternalFileTask {
        self.external_file_tasks
            .retain(|_, count| count.load(Ordering::Acquire) != 0);
        let count = self
            .external_file_tasks
            .entry(session.to_owned())
            .or_default()
            .clone();
        count.fetch_add(1, Ordering::AcqRel);
        ExternalFileTask(count)
    }
    pub(in crate::features) fn has_external_file_tasks(&self, sessions: &[String]) -> bool {
        sessions.iter().any(|session| {
            self.external_file_tasks
                .get(session)
                .is_some_and(|count| count.load(Ordering::Acquire) != 0)
        })
    }
    pub(in crate::features) fn invalidate_file_readers(&mut self, sessions: &[String]) {
        if let Some(workspace) = self.editor.workspace.as_mut() {
            for tab in &mut workspace.tabs {
                if tab.loading
                    && tab
                        .session_id
                        .as_ref()
                        .is_some_and(|id| sessions.contains(id))
                {
                    tab.generation = RemoteTextGeneration::next();
                    tab.loading = false;
                    tab.error = Some(rust_i18n::t!("fileSudo.reload").to_string());
                }
            }
        }
        if let Some(workspace) = self.preview.workspace.as_mut() {
            for tab in &mut workspace.tabs {
                if tab
                    .session_id
                    .as_ref()
                    .is_some_and(|id| sessions.contains(id))
                {
                    tab.generation = RemoteTextGeneration::next();
                    tab.content = crate::models::PreviewContent::Error(
                        rust_i18n::t!("fileSudo.reload").to_string(),
                    );
                }
            }
        }
    }
}
