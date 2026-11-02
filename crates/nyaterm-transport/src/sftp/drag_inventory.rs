use super::{
    SftpPathCodec, SftpService, attrs_to_sftp_file_type, await_sftp_request, close_sftp_session,
    open_sftp_session, remote_file_path_bytes, remote_join,
};
use crate::drag_export::{RemoteDragFile, tree::RemoteDragEntry};
use crate::{RemoteFilePath, SftpFileEntry, SftpFileType, SftpTransferControl};
use std::{
    collections::HashSet,
    path::PathBuf,
    time::{Duration, UNIX_EPOCH},
};

impl SftpService {
    pub(crate) fn enumerate_drag(
        &self,
        roots: Vec<SftpFileEntry>,
        control: &SftpTransferControl,
    ) -> anyhow::Result<Vec<RemoteDragEntry>> {
        self.enumerate_drag_with_root_controls(roots, control, Vec::new())
    }

    pub(crate) fn enumerate_drag_with_root_controls(
        &self,
        roots: Vec<SftpFileEntry>,
        control: &SftpTransferControl,
        root_controls: Vec<SftpTransferControl>,
    ) -> anyhow::Result<Vec<RemoteDragEntry>> {
        anyhow::ensure!(roots.len() <= 1024, "too many drag roots");
        anyhow::ensure!(
            root_controls.is_empty() || root_controls.len() == roots.len(),
            "invalid drag root controls"
        );
        let config = self.config.clone();
        let multiplex = self.multiplex.clone();
        let control = control.clone();
        // All roots being cancelled must also wake an inventory waiting for its
        // source's permit/gate. Individual cancellation skips only that root.
        let monitor = if root_controls.is_empty() {
            None
        } else {
            let controls = root_controls.clone();
            let control = control.clone();
            Some(self.export_runtime()?.spawn(async move {
                loop {
                    if controls.iter().all(SftpTransferControl::is_cancelled) {
                        control.cancel();
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
            }))
        };
        let result = self.run_controlled_operation(control.clone(), true, async move {
            let codec = SftpPathCodec::from_ssh_config(&config)?;
            let session =
                await_sftp_request(&control, open_sftp_session(&config, multiplex.as_ref()))
                    .await?;
            let result = async {
                let mut pending: Vec<_> = roots
                    .into_iter()
                    .enumerate()
                    .rev()
                    .map(|(root_index, entry)| {
                        let relative = PathBuf::from(&entry.name);
                        (entry, relative, 0_usize, root_index)
                    })
                    .collect();
                let mut entries = Vec::new();
                let mut visited = HashSet::new();
                while let Some((mut entry, relative_path, depth, root_index)) = pending.pop() {
                    control.wait_if_paused().await?;
                    let item_control = root_controls.get(root_index).unwrap_or(&control);
                    if item_control.wait_if_paused().await.is_err() {
                        continue;
                    }
                    anyhow::ensure!(
                        depth <= 128 && entries.len() < 65536,
                        "drag directory tree is too large"
                    );
                    crate::download_path::validate_name(&entry.name)?;
                    let remote_path = entry.remote_path();
                    let raw = remote_file_path_bytes(&codec, &remote_path)?;
                    anyhow::ensure!(visited.insert(raw.clone()), "duplicate remote drag source");
                    if depth == 0 {
                        let attrs = match await_sftp_request(
                            item_control,
                            session.sftp.symlink_metadata_bytes(raw.clone()),
                        )
                        .await
                        {
                            Ok(attrs) => attrs,
                            Err(_) if item_control.is_cancelled() => continue,
                            Err(error) => return Err(error),
                        };
                        anyhow::ensure!(
                            attrs_to_sftp_file_type(&attrs) == entry.file_type,
                            "remote drag source changed type"
                        );
                        entry.size = attrs.size;
                        entry.modified_at = attrs.mtime;
                    }
                    anyhow::ensure!(
                        matches!(
                            entry.file_type,
                            SftpFileType::File | SftpFileType::Directory
                        ),
                        "links and special files cannot be dragged out"
                    );
                    let is_directory = entry.file_type == SftpFileType::Directory;
                    if is_directory {
                        let children: Vec<_> = match await_sftp_request(
                            item_control,
                            session.sftp.read_dir_bytes(raw),
                        )
                        .await
                        {
                            Ok(children) => children.collect(),
                            Err(_) if item_control.is_cancelled() => continue,
                            Err(error) => return Err(error),
                        };
                        anyhow::ensure!(
                            children.len() + pending.len() + entries.len() <= 65536,
                            "drag directory tree is too large"
                        );
                        for child in children.into_iter().rev() {
                            let name = codec.decode_path_lossy(child.file_name_bytes());
                            let attrs = child.metadata();
                            let file_type = attrs_to_sftp_file_type(&attrs);
                            if !crate::download_path::validate_directory_entry(&name, file_type)? {
                                continue;
                            }
                            let path = remote_join(&remote_path.display_path, &name);
                            let raw_path_token =
                                RemoteFilePath::from_raw(&path, &child.path_bytes()).raw_path_token;
                            let child_relative = relative_path.join(&name);
                            pending.push((
                                SftpFileEntry {
                                    name,
                                    path,
                                    file_type,
                                    size: attrs.size,
                                    permissions: attrs.permissions,
                                    owner: String::new(),
                                    group: String::new(),
                                    modified_at: attrs.mtime,
                                    raw_path_token,
                                    symlink_target_is_directory: false,
                                },
                                child_relative,
                                depth + 1,
                                root_index,
                            ));
                        }
                    }
                    entries.push(RemoteDragEntry {
                        relative_path,
                        root_index,
                        is_directory,
                        file: RemoteDragFile {
                            display_name: entry.name.into(),
                            remote_path,
                            size: if is_directory { None } else { entry.size },
                            modified_at: entry
                                .modified_at
                                .map(|time| UNIX_EPOCH + Duration::from_secs(u64::from(time))),
                        },
                    });
                }
                anyhow::ensure!(!entries.is_empty(), "no files selected");
                Ok(entries)
            }
            .await;
            let _ = tokio::time::timeout(Duration::from_secs(5), close_sftp_session(session)).await;
            result
        });
        if let Some(monitor) = monitor {
            monitor.abort();
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::SftpService;
    use crate::sftp::{compatibility_runtime, filename_tests};
    use crate::{SftpFileEntry, SftpFileType, SftpSettings, SftpTransferControl, SshSessionConfig};
    use std::{
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
            mpsc,
        },
        time::{Duration, Instant},
    };

    fn root() -> SftpFileEntry {
        SftpFileEntry {
            name: "folder".into(),
            path: "/folder".into(),
            file_type: SftpFileType::Directory,
            size: None,
            permissions: None,
            owner: String::new(),
            group: String::new(),
            modified_at: None,
            raw_path_token: None,
            symlink_target_is_directory: false,
        }
    }
    #[test]
    fn large_inventory_uses_listing_attributes_without_content_or_per_child_stat_requests() {
        let (service, files) = filename_tests::service("UTF-8");
        {
            let mut files = files.lock().unwrap();
            files.directories.insert(b"/folder".to_vec());
            for index in 0..10_000 {
                files
                    .contents
                    .insert(format!("/folder/{index}").into_bytes(), vec![1]);
            }
        }
        let entries = service
            .enumerate_drag(vec![root()], &SftpTransferControl::new())
            .unwrap();
        assert_eq!(entries.len(), 10_001);
        assert!(entries.iter().all(|entry| entry.root_index == 0));
        let files = files.lock().unwrap();
        assert!(files.reads.is_empty());
        assert_eq!(
            files
                .request_kinds
                .iter()
                .filter(|&&kind| kind == 7)
                .count(),
            1
        );
        assert!(
            files
                .request_kinds
                .iter()
                .all(|kind| [7, 11, 12, 4].contains(kind))
        );
    }
    #[test]
    fn blocked_metadata_and_directory_requests_observe_cancel_before_the_request_timeout() {
        for kind in [7, 12] {
            let (service, files) = filename_tests::service("UTF-8");
            let (started, delayed) = mpsc::channel();
            {
                let mut files = files.lock().unwrap();
                files.directories.insert(b"/folder".to_vec());
                files.request_delay.insert(kind, Duration::from_secs(60));
                files.request_delay_started = Some(started);
            }
            let control = SftpTransferControl::new();
            let worker_control = control.clone();
            let worker =
                std::thread::spawn(move || service.enumerate_drag(vec![root()], &worker_control));
            assert_eq!(delayed.recv_timeout(Duration::from_secs(5)).unwrap(), kind);
            let started = Instant::now();
            control.cancel();
            assert!(worker.join().unwrap().is_err());
            // The bound only has to separate a cancel wake-up from the 60 s stall
            // the peer holds; 250 ms was below what the shared two-thread runtime
            // delivers while the rest of this binary's tests run in parallel.
            assert!(started.elapsed() < Duration::from_secs(5));
        }
    }
    #[test]
    fn gate_and_permit_waits_are_cancellable_and_release_for_next_operation() {
        for block_gate in [false, true] {
            let (service, _) = filename_tests::service("UTF-8");
            let (ready, arrived) = mpsc::channel();
            let (release, released) = tokio::sync::oneshot::channel::<()>();
            let state = service.compatibility.clone().unwrap();
            let budget = service.export_budget.clone();
            let holder = compatibility_runtime().unwrap().spawn(async move {
                if block_gate {
                    let _guard = state.gate.lock().await;
                    ready.send(()).unwrap();
                    released.await.unwrap();
                } else {
                    let _permit = budget.acquire_owned().await.unwrap();
                    ready.send(()).unwrap();
                    released.await.unwrap();
                }
            });
            arrived.recv_timeout(Duration::from_secs(5)).unwrap();
            let control = SftpTransferControl::new();
            let cancel = control.clone();
            let worker_service = service.clone();
            let worker = std::thread::spawn(move || {
                worker_service.run_controlled_operation(control, true, async { Ok(()) })
            });
            let started = Instant::now();
            cancel.cancel();
            assert!(worker.join().unwrap().is_err());
            assert!(started.elapsed() < Duration::from_secs(5));
            release.send(()).unwrap();
            compatibility_runtime().unwrap().block_on(holder).unwrap();
            service
                .run_controlled_operation(SftpTransferControl::new(), true, async { Ok(()) })
                .unwrap();
        }
    }

    #[test]
    fn queue_root_cancel_interrupts_blocked_inventory_and_preserves_other_roots() {
        let (service, files) = filename_tests::service("UTF-8");
        let (started, delayed) = mpsc::channel();
        {
            let mut files = files.lock().unwrap();
            files.directories.insert(b"/folder".to_vec());
            files.directories.insert(b"/other".to_vec());
            files.request_delay.insert(7, Duration::from_secs(60));
            files.request_delay_started = Some(started);
        }
        let cancelled = SftpTransferControl::new();
        let controls = vec![cancelled.clone(), SftpTransferControl::new()];
        let mut other = root();
        other.name = "other".into();
        other.path = "/other".into();
        let worker = std::thread::spawn(move || {
            service.enumerate_drag_with_root_controls(
                vec![root(), other],
                &SftpTransferControl::new(),
                controls,
            )
        });
        assert_eq!(delayed.recv_timeout(Duration::from_secs(5)).unwrap(), 7);
        files.lock().unwrap().request_delay.remove(&7);
        let started = Instant::now();
        cancelled.cancel();
        let entries = worker.join().unwrap().unwrap();
        // Includes the remaining root and worker shutdown on the shared runtime.
        // It must finish well before the cancelled response's 60 s delay.
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].root_index, 1);
        assert_eq!(entries[0].relative_path, std::path::PathBuf::from("other"));
    }
    #[test]
    fn per_source_export_budget_bounds_parallel_operations_to_three_or_one() {
        for compatibility in [false, true] {
            let service = SftpService::new(SshSessionConfig {
                sftp: SftpSettings {
                    compatibility_mode: compatibility,
                    ..Default::default()
                },
                ..Default::default()
            });
            let active = Arc::new(AtomicUsize::new(0));
            let maximum = Arc::new(AtomicUsize::new(0));
            let threads: Vec<_> = (0..8)
                .map(|_| {
                    let service = service.clone();
                    let active = active.clone();
                    let maximum = maximum.clone();
                    std::thread::spawn(move || {
                        service
                            .run_controlled_operation(
                                SftpTransferControl::new(),
                                true,
                                async move {
                                    let count = active.fetch_add(1, Ordering::SeqCst) + 1;
                                    maximum.fetch_max(count, Ordering::SeqCst);
                                    tokio::time::sleep(Duration::from_millis(20)).await;
                                    active.fetch_sub(1, Ordering::SeqCst);
                                    Ok(())
                                },
                            )
                            .unwrap()
                    })
                })
                .collect();
            for thread in threads {
                thread.join().unwrap();
            }
            assert_eq!(
                maximum.load(Ordering::SeqCst),
                if compatibility { 1 } else { 3 }
            );
            assert_eq!(
                service.export_budget.available_permits(),
                if compatibility { 1 } else { 3 }
            );
        }
    }
}
