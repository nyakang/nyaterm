use super::TransferFeatureState;
use crate::models::{TransferJobKind, TransferJobStatus};
use nyaterm_files::sftp::SftpFileEntry;

#[derive(Clone)]
pub(in crate::features) struct TransferFileClipboard {
    pub source: super::super::file_operations::FileOperationEndpoint,
    pub os_item: Option<gpui::ClipboardItem>,
    pub os_revision: Option<u64>,
    pub entries: Vec<SftpFileEntry>,
    pub cut: bool,
    pub generation: u64,
}

impl TransferFileClipboard {
    pub(in crate::features) fn matches_system_clipboard(&self, cx: &gpui::App) -> bool {
        if let Some(revision) = self.os_revision {
            return cx.clipboard_revision() == Some(revision);
        }
        self.os_item.is_some() && self.os_item == cx.read_from_clipboard()
    }
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
            .is_some_and(|clipboard| clipboard.source.session_id() == Some(session_id))
        {
            self.clipboard = None;
        }
    }

    pub(in crate::features) fn set_file_clipboard(
        &mut self,
        source: super::super::file_operations::FileOperationEndpoint,
        entries: Vec<SftpFileEntry>,
        cut: bool,
        item: Option<gpui::ClipboardItem>,
        revision: Option<u64>,
    ) {
        self.clipboard_generation = self.clipboard_generation.wrapping_add(1);
        self.clipboard = Some(TransferFileClipboard {
            source,
            os_item: item,
            os_revision: revision,
            entries,
            cut,
            generation: self.clipboard_generation,
        });
    }

    pub(in crate::features) fn cut_path_in_flight(&self, generation: u64, path: &str) -> bool {
        self.cut_jobs
            .values()
            .any(|(value, item)| *value == generation && item == path)
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
        clipboard
            .entries
            .retain(|entry| entry.identity_key() != path);
        if clipboard.entries.is_empty() {
            self.clipboard = None;
        }
    }
}
