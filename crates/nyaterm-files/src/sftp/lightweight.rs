use crate::file_browser::lightweight::{DirectoryEntry, DirectoryLimits, DirectoryListing};
use crate::sftp::{
    SftpPathCodec, SftpService, SftpTransferControl, attrs_to_sftp_file_type, close_sftp_session,
};
use std::time::{Duration, Instant};

impl SftpService {
    pub(crate) fn list_directory_lightweight(
        &self,
        path: &str,
        limits: DirectoryLimits,
        control: SftpTransferControl,
        deadline: Instant,
    ) -> anyhow::Result<DirectoryListing> {
        let codec = SftpPathCodec::from_ssh_config(&self.config)?;
        let path = codec.encode_path(path)?;
        let factory = self.session_factory();
        let cancel = control.clone();
        let timeout_control = control.clone();
        // Includes compatibility-gate waiting and opening the subsystem. The
        // UI never waits on this timer or on remote cleanup.
        let timer = self.export_runtime()?.spawn(async move {
            tokio::time::sleep(deadline.saturating_duration_since(Instant::now())).await;
            timeout_control.cancel();
        });
        let result = self.run_controlled_operation(control, false, async move {
            let session = cancel.until_cancelled(factory.open()).await??;
            let result = cancel
                .until_cancelled(session.sftp.read_dir_bytes_limited(
                    path,
                    limits.entries,
                    limits.name_bytes,
                ))
                .await;
            let _ = tokio::time::timeout(Duration::from_secs(1), close_sftp_session(session)).await;
            let listing = result??;
            let mut bytes = 0usize;
            let mut complete = listing.complete;
            let mut entries = Vec::new();
            for entry in listing.entries {
                cancel.check_cancelled()?;
                let raw = entry.file_name_bytes();
                let name = codec.decode_path_lossy(raw);
                if codec.encode_path(&name).ok().as_deref() != Some(raw) {
                    continue;
                }
                if name.len() > limits.name_bytes - bytes {
                    complete = false;
                    break;
                }
                bytes += name.len();
                let attrs = entry.metadata();
                entries.push(DirectoryEntry {
                    name,
                    file_type: attrs_to_sftp_file_type(&attrs),
                    size: attrs.size,
                });
            }
            Ok(DirectoryListing { entries, complete })
        });
        timer.abort();
        result
    }
}

#[cfg(test)]
mod tests {
    use crate::file_browser::lightweight::DirectoryLimits;
    use crate::sftp::{SftpTransferControl, filename_tests};
    use std::time::{Duration, Instant};
    #[test]
    fn large_directories_stop_at_the_wire_budget_without_metadata_queries() {
        let (service, files) = filename_tests::service("UTF-8");
        {
            let mut files = files.lock().unwrap();
            files.directories.insert(b"/folder".to_vec());
            for index in 0..30_000 {
                files
                    .contents
                    .insert(format!("/folder/{index:05}").into_bytes(), Vec::new());
            }
        }
        let listing = service
            .list_directory_lightweight(
                "/folder",
                DirectoryLimits {
                    entries: 20_000,
                    name_bytes: 4 * 1024 * 1024,
                },
                SftpTransferControl::default(),
                Instant::now() + Duration::from_secs(10),
            )
            .unwrap();
        assert_eq!(listing.entries.len(), 20_000);
        assert!(!listing.complete);
        let files = files.lock().unwrap();
        assert_eq!(
            files
                .request_kinds
                .iter()
                .filter(|&&kind| kind == 12)
                .count(),
            40
        );
        assert!(
            files
                .request_kinds
                .iter()
                .all(|kind| [11, 12, 4].contains(kind))
        );
        assert_eq!(files.open_handles, 0);
    }
    #[test]
    fn deadline_includes_a_server_that_never_returns_directory_contents() {
        let (service, files) = filename_tests::service("UTF-8");
        {
            let mut files = files.lock().unwrap();
            files.directories.insert(b"/folder".to_vec());
            files.request_delay.insert(12, Duration::from_secs(30));
        }
        let start = Instant::now();
        assert!(
            service
                .list_directory_lightweight(
                    "/folder",
                    DirectoryLimits {
                        entries: 100,
                        name_bytes: 1024
                    },
                    SftpTransferControl::default(),
                    start + Duration::from_millis(50)
                )
                .is_err()
        );
        assert!(start.elapsed() < Duration::from_secs(2));
    }
    #[test]
    fn cancellation_interrupts_waiting_for_the_compatibility_gate() {
        let (service, _) = filename_tests::service("UTF-8");
        let state = service.compatibility.as_ref().unwrap().clone();
        let (ready, started) = std::sync::mpsc::channel();
        let (release, wait) = tokio::sync::oneshot::channel::<()>();
        let hold = crate::sftp::compatibility_runtime()
            .unwrap()
            .spawn(async move {
                let _guard = state.gate.lock().await;
                ready.send(()).unwrap();
                let _ = wait.await;
            });
        started.recv_timeout(Duration::from_secs(2)).unwrap();
        let start = Instant::now();
        let result = service.list_directory_lightweight(
            "/folder",
            DirectoryLimits {
                entries: 100,
                name_bytes: 1024,
            },
            SftpTransferControl::default(),
            start + Duration::from_millis(50),
        );
        let _ = release.send(());
        crate::sftp::compatibility_runtime()
            .unwrap()
            .block_on(hold)
            .unwrap();
        assert!(result.is_err());
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}
