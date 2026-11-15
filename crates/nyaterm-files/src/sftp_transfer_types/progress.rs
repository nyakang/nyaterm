//! Throttling policy for transfer progress. The caller supplies time and owns delivery.
use super::{FileTransferPhase, SftpTransferProgress};
use std::time::{Duration, Instant};
const PROGRESS_INTERVAL: Duration = Duration::from_millis(50);
#[derive(Default)]
pub struct TransferProgressGate {
    last_sent_at: Option<Instant>,
    final_sent: bool,
    phase: Option<FileTransferPhase>,
}
impl TransferProgressGate {
    pub fn should_emit(&mut self, progress: &SftpTransferProgress, now: Instant) -> bool {
        let completed = progress
            .total_bytes
            .is_some_and(|total| progress.bytes_transferred >= total)
            && progress.item_count_total.is_none_or(|total| {
                progress
                    .item_count_completed
                    .is_some_and(|completed| completed >= total)
            });
        let due = self
            .last_sent_at
            .is_none_or(|last_sent_at| now.duration_since(last_sent_at) >= PROGRESS_INTERVAL);
        if completed && self.final_sent {
            return false;
        }
        let phase_changed = self.phase != progress.phase;
        if !completed && !due && !phase_changed {
            return false;
        }

        self.phase = progress.phase;
        self.final_sent |= completed;
        self.last_sent_at = Some(now);
        true
    }
}
#[cfg(test)]
mod tests {
    use super::TransferProgressGate;
    use crate::sftp_transfer_types::{FileTransferPhase, SftpTransferProgress};
    use std::time::{Duration, Instant};
    #[test]
    fn time_phase_and_completion_control_delivery_without_sleeping() {
        let mut gate = TransferProgressGate::default();
        let start = Instant::now();
        let mut progress = SftpTransferProgress {
            phase: Some(FileTransferPhase::Scanning),
            remote_path: String::new(),
            local_path: Default::default(),
            bytes_transferred: 0,
            total_bytes: Some(10),
            item_count_completed: Some(0),
            item_count_total: Some(2),
        };
        assert!(gate.should_emit(&progress, start));
        assert!(!gate.should_emit(&progress, start + Duration::from_millis(49)));
        assert!(gate.should_emit(&progress, start + Duration::from_millis(50)));
        progress.phase = Some(FileTransferPhase::Transferring);
        assert!(gate.should_emit(&progress, start + Duration::from_millis(51)));
        progress.bytes_transferred = 10;
        assert!(!gate.should_emit(&progress, start + Duration::from_millis(52)));
        progress.item_count_completed = Some(2);
        assert!(gate.should_emit(&progress, start + Duration::from_millis(53)));
        assert!(!gate.should_emit(&progress, start + Duration::from_secs(1)));
    }
}
