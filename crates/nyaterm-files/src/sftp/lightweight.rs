use crate::file_browser::lightweight::{
    DirectoryEntry, DirectoryLimits, DirectoryListing, MAX_DIRECTORY_TARGETS,
};
use crate::sftp::{
    SftpPathCodec, SftpService, SftpTransferControl, attrs_to_sftp_file_type, close_sftp_session,
};
use std::time::{Duration, Instant};

impl SftpService {
    pub(crate) fn list_completion_directory(
        &self,
        path: &crate::sftp::RemoteFilePath,
        limits: crate::file_browser::completion::CompletionLimits,
        control: SftpTransferControl,
        deadline: Instant,
    ) -> anyhow::Result<crate::file_browser::completion::CompletionListing> {
        use crate::file_browser::completion::{CompletionListing, check};
        use crate::sftp::{RemoteFilePath, SftpFileEntry, remote_file_path_bytes};
        let codec = SftpPathCodec::from_ssh_config(&self.config)?;
        let raw = remote_file_path_bytes(&codec, path)?;
        let display = path.display_path.trim_end_matches('/').to_string();
        let factory = self.session_factory();
        let cancel = control.clone();
        let timer_control = control.clone();
        let timer = self.export_runtime()?.spawn(async move {
            tokio::time::sleep(deadline.saturating_duration_since(Instant::now())).await;
            timer_control.cancel();
        });
        let result = self.run_controlled_operation(control, false, async move {
            check(&cancel, deadline)?;
            let session = cancel.until_cancelled(factory.open()).await??;
            let result = cancel
                .until_cancelled(session.sftp.read_dir_bytes_limited(
                    raw,
                    limits.entries,
                    limits.name_bytes,
                ))
                .await;
            let _ = tokio::time::timeout(Duration::from_secs(1), close_sftp_session(session)).await;
            let listing = result??;
            let mut output = CompletionListing {
                entries: Vec::new(),
                complete: listing.complete,
            };
            let mut bytes = 0usize;
            for entry in listing.entries {
                let name = codec.decode_path_lossy(entry.file_name_bytes());
                if name == "." || name == ".." {
                    continue;
                }
                anyhow::ensure!(!name.contains(['/', '\0']), "invalid directory entry");
                bytes = bytes.saturating_add(name.len());
                if bytes > limits.name_bytes {
                    output.complete = false;
                    break;
                }
                let path =
                    RemoteFilePath::from_raw(format!("{display}/{name}"), &entry.path_bytes());
                output.entries.push(SftpFileEntry {
                    name,
                    path: path.display_path,
                    raw_path_token: path.raw_path_token,
                    file_type: attrs_to_sftp_file_type(&entry.metadata()),
                    size: None,
                    permissions: None,
                    owner: String::new(),
                    group: String::new(),
                    modified_at: None,
                    symlink_target_is_directory: false,
                });
            }
            Ok(output)
        });
        timer.abort();
        result
    }

    pub(crate) fn completion_target_is_directory(
        &self,
        path: &crate::sftp::RemoteFilePath,
        control: SftpTransferControl,
        deadline: Instant,
    ) -> anyhow::Result<bool> {
        let codec = SftpPathCodec::from_ssh_config(&self.config)?;
        let path = super::remote_file_path_bytes(&codec, path)?;
        let factory = self.session_factory();
        let cancel = control.clone();
        self.run_controlled_operation(control, false, async move {
            let operation = async {
                let session = cancel.until_cancelled(factory.open()).await??;
                let result = cancel
                    .until_cancelled(session.sftp.metadata_bytes(path))
                    .await;
                let _ =
                    tokio::time::timeout(Duration::from_secs(1), close_sftp_session(session)).await;
                Ok(attrs_to_sftp_file_type(&result??) == crate::sftp::SftpFileType::Directory)
            };
            tokio::time::timeout(
                deadline.saturating_duration_since(Instant::now()),
                operation,
            )
            .await?
        })
    }

    pub(crate) fn probe_directory_targets(
        &self,
        paths: &[String],
        control: SftpTransferControl,
        deadline: Instant,
    ) -> anyhow::Result<Vec<Option<bool>>> {
        anyhow::ensure!(
            paths.len() <= MAX_DIRECTORY_TARGETS,
            "too many directory targets"
        );
        if paths.is_empty() || control.is_cancelled() || Instant::now() >= deadline {
            return Ok(vec![None; paths.len()]);
        }
        let codec = SftpPathCodec::from_ssh_config(&self.config)?;
        let paths = paths
            .iter()
            .map(|path| codec.encode_path(path))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let factory = self.session_factory();
        let cancel = control.clone();
        let timeout = control.clone();
        let timer = self.export_runtime()?.spawn(async move {
            tokio::time::sleep(deadline.saturating_duration_since(Instant::now())).await;
            timeout.cancel();
        });
        let result = self.run_controlled_operation(control, false, async move {
            let session = cancel.until_cancelled(factory.open()).await??;
            let mut result = Vec::with_capacity(paths.len());
            let count = paths.len();
            for path in paths {
                if cancel.is_cancelled() || Instant::now() >= deadline {
                    break;
                }
                match cancel
                    .until_cancelled(session.sftp.metadata_bytes(path))
                    .await
                {
                    Ok(value) => result.push(value.ok().map(|attrs| {
                        attrs_to_sftp_file_type(&attrs) == crate::sftp::SftpFileType::Directory
                    })),
                    Err(_) => break,
                }
            }
            let _ = tokio::time::timeout(Duration::from_secs(1), close_sftp_session(session)).await;
            result.resize(count, None);
            Ok(result)
        });
        timer.abort();
        result
    }

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
    fn address_completion_preserves_raw_names_and_limits_wire_enumeration() {
        use crate::file_browser::completion::CompletionLimits;
        use crate::sftp::RemoteFilePath;
        let (service, files) = filename_tests::service("UTF-8");
        {
            let mut files = files.lock().unwrap();
            files.directories.insert(b"/folder".to_vec());
            files.directories.insert(b"/folder/\xff".to_vec());
            files.directories.insert(b"/folder/\xfe".to_vec());
        }
        let path = RemoteFilePath::new("/folder");
        let listing = service
            .list_completion_directory(
                &path,
                CompletionLimits::default(),
                SftpTransferControl::new(),
                Instant::now() + Duration::from_secs(5),
            )
            .unwrap();
        assert!(listing.complete);
        assert_eq!(listing.entries.len(), 2);
        assert_eq!(listing.entries[0].name, listing.entries[1].name);
        assert_ne!(
            listing.entries[0].identity_key(),
            listing.entries[1].identity_key()
        );
        for entry in listing.entries {
            assert!(entry.is_directory());
            assert!(
                service
                    .completion_target_is_directory(
                        &entry.remote_path(),
                        SftpTransferControl::new(),
                        Instant::now() + Duration::from_secs(5)
                    )
                    .unwrap()
            );
        }
        let listing = service
            .list_completion_directory(
                &path,
                CompletionLimits {
                    entries: 1,
                    name_bytes: 100,
                },
                SftpTransferControl::new(),
                Instant::now() + Duration::from_secs(5),
            )
            .unwrap();
        assert_eq!(listing.entries.len(), 1);
        assert!(!listing.complete);
    }

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
    #[test]
    fn selected_links_use_bounded_stat_only_and_the_configured_path_encoding() {
        let (service, files) = filename_tests::service("GBK");
        let target = crate::sftp::SftpPathCodec::from_ssh_config(&service.config)
            .unwrap()
            .encode_path("/目录")
            .unwrap();
        {
            let mut files = files.lock().unwrap();
            files.directories.insert(target.clone());
            files.contents.insert(b"/file".to_vec(), Vec::new());
            files.links.insert(b"/loop".to_vec(), b"/loop".to_vec());
            files.links.insert(b"/dir-link".to_vec(), target);
            files
                .links
                .insert(b"/file-link".to_vec(), b"/file".to_vec());
            files
                .links
                .insert(b"/broken".to_vec(), b"/missing".to_vec());
        }
        let paths: Vec<_> = ["/dir-link", "/file-link", "/broken", "/目录", "/loop"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let values = service
            .probe_directory_targets(
                &paths,
                SftpTransferControl::default(),
                Instant::now() + Duration::from_secs(2),
            )
            .unwrap();
        assert_eq!(
            values,
            vec![Some(true), Some(false), None, Some(true), None]
        );
        let files = files.lock().unwrap();
        assert_eq!(
            files
                .request_kinds
                .iter()
                .filter(|&&kind| kind == 17)
                .count(),
            5
        );
        assert!(files.request_kinds.iter().all(|&kind| kind == 17));
        drop(files);
        assert!(
            service
                .probe_directory_targets(
                    &vec!["/dir-link".into(); 13],
                    SftpTransferControl::default(),
                    Instant::now() + Duration::from_secs(2)
                )
                .is_err()
        );
    }

    #[test]
    fn link_probe_deadline_interrupts_a_server_that_never_returns_stat() {
        let (service, files) = filename_tests::service("UTF-8");
        files
            .lock()
            .unwrap()
            .request_delay
            .insert(17, Duration::from_secs(30));
        let start = Instant::now();
        let result = service.probe_directory_targets(
            &["/link".into()],
            SftpTransferControl::default(),
            start + Duration::from_millis(50),
        );
        assert!(result.is_err() || result.unwrap() == vec![None]);
        assert!(start.elapsed() < Duration::from_secs(2));
    }
    #[test]
    fn cancellation_interrupts_an_in_flight_link_probe() {
        let (service, files) = filename_tests::service("UTF-8");
        let (started, ready) = std::sync::mpsc::channel();
        {
            let mut files = files.lock().unwrap();
            files.request_delay.insert(17, Duration::from_secs(30));
            files.request_delay_started = Some(started);
        }
        let control = SftpTransferControl::default();
        let worker_control = control.clone();
        let worker = std::thread::spawn(move || {
            service.probe_directory_targets(
                &["/link".into()],
                worker_control,
                Instant::now() + Duration::from_secs(10),
            )
        });
        assert_eq!(ready.recv_timeout(Duration::from_secs(2)).unwrap(), 17);
        let start = Instant::now();
        control.cancel();
        let result = worker.join().unwrap();
        assert!(result.is_err() || result.unwrap() == vec![None]);
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}
