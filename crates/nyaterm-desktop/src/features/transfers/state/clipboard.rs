use super::TransferFeatureState;
use crate::models::{TransferJobKind, TransferJobStatus};
use nyaterm_files::sftp::SftpFileEntry;
use std::path::PathBuf;

#[derive(Clone)]
pub(in crate::features) struct TransferFileClipboard {
    pub source_session_id: String,
    pub entries: Vec<SftpFileEntry>,
    pub cut: bool,
    pub generation: u64,
    pub os_paths_at_capture: Vec<PathBuf>,
}

impl TransferFeatureState {
    pub(in crate::features) fn file_clipboard(&self) -> Option<&TransferFileClipboard> {
        self.clipboard.as_ref()
    }

    pub(in crate::features) fn clear_file_clipboard_for_session(&mut self, session_id: &str) {
        for job in &mut self.queue.jobs {
            if !self.cut_jobs.contains_key(&job.id) {
                continue;
            }
            let target_closing = matches!(&job.kind, TransferJobKind::SendTo { target_session_id, .. }
                if target_session_id == session_id);
            if (job.session_id.as_deref() == Some(session_id) || target_closing)
                && let Some(control) = job.control.as_ref()
            {
                control.cancel();
                job.status = TransferJobStatus::Cancelling;
            }
        }
        if self
            .clipboard
            .as_ref()
            .is_some_and(|clipboard| clipboard.source_session_id == session_id)
        {
            self.clipboard = None;
        }
    }

    pub(in crate::features) fn set_file_clipboard(
        &mut self,
        source_session_id: String,
        entries: Vec<SftpFileEntry>,
        cut: bool,
        os_paths_at_capture: Vec<PathBuf>,
    ) {
        self.clipboard_generation = self.clipboard_generation.wrapping_add(1);
        self.clipboard = Some(TransferFileClipboard {
            source_session_id,
            entries,
            cut,
            generation: self.clipboard_generation,
            os_paths_at_capture,
        });
    }

    pub(in crate::features) fn track_cut_job(
        &mut self,
        job_id: String,
        generation: u64,
        path: String,
    ) {
        self.cut_jobs.insert(job_id, (generation, path));
    }

    pub(in crate::features) fn settle_cut_job(&mut self, job_id: &str, succeeded: bool) {
        let Some((generation, path)) = self.cut_jobs.remove(job_id) else {
            return;
        };
        if !succeeded {
            return;
        }
        let Some(clipboard) = self.clipboard.as_mut() else {
            return;
        };
        if clipboard.generation != generation {
            return;
        }
        clipboard.entries.retain(|entry| entry.path != path);
        if clipboard.entries.is_empty() {
            self.clipboard = None;
        }
    }
}
