use crate::drag_export::{ExportObserver, ExportReadEvent, RemoteDragFile, SftpReadSource};
use crate::remote_file::RemoteFileService;
use crate::sftp::{
    RemoteFilePath, SFTP_COMPATIBILITY_CACHE, SftpTransferControl, compatibility_runtime,
    filename_tests,
};
use std::{
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};

fn fixture(
    bytes: &[u8],
) -> (
    Arc<RemoteFileService>,
    RemoteDragFile,
    Arc<Mutex<filename_tests::Files>>,
) {
    fixture_mode(bytes, true)
}

fn fixture_mode(
    bytes: &[u8],
    compatibility: bool,
) -> (
    Arc<RemoteFileService>,
    RemoteDragFile,
    Arc<Mutex<filename_tests::Files>>,
) {
    let (mut service, files) = filename_tests::service("UTF-8");
    // Keep the injected cached peer, while testing the production mode's
    // effective concurrency policy independently of authentication.
    service.config.sftp.compatibility_mode = compatibility;
    files
        .lock()
        .unwrap()
        .contents
        .insert(b"/raw-\xff".to_vec(), bytes.to_vec());
    let source = Arc::new(RemoteFileService::with_test_export_service(service));
    let file = RemoteDragFile {
        display_name: "export.txt".into(),
        size: Some(bytes.len() as u64),
        remote_path: RemoteFilePath::from_raw("/display-only", b"/raw-\xff"),
        modified_at: None,
    };
    (source, file, files)
}

fn observer() -> (
    Arc<dyn Fn() -> ExportObserver + Send + Sync>,
    mpsc::Receiver<ExportReadEvent>,
) {
    let (tx, rx) = mpsc::channel();
    (
        Arc::new(move || {
            let tx = tx.clone();
            Arc::new(move |event| {
                let _ = tx.send(event);
            })
        }),
        rx,
    )
}

fn finished(events: &mpsc::Receiver<ExportReadEvent>) -> Result<u64, String> {
    loop {
        if let ExportReadEvent::Finished(result) =
            events.recv_timeout(Duration::from_secs(2)).unwrap()
        {
            return result;
        }
    }
}

#[test]
fn export_reads_raw_path_in_chunks_seeks_and_releases_task() {
    let content: Vec<u8> = (0..140_000).map(|i| (i % 251) as u8).collect();
    let (service, file, _) = fixture(&content);
    let (observe, events) = observer();
    let source = SftpReadSource::new(Arc::downgrade(&service), file, observe);
    let mut stream = source.open().unwrap();
    let mut buffer = vec![0; 64 * 1024];
    assert_eq!(stream.read_at(100_000, &mut buffer).unwrap(), 40_000);
    assert_eq!(&buffer[..40_000], &content[100_000..]);
    let mut offset = 0;
    while offset < content.len() {
        let count = stream.read_at(offset as u64, &mut buffer).unwrap();
        assert_eq!(&buffer[..count], &content[offset..offset + count]);
        offset += count;
    }
    assert_eq!(stream.read_at(offset as u64, &mut buffer).unwrap(), 0);
    drop(stream);
    assert_eq!(finished(&events).unwrap(), content.len() as u64);
}

#[test]
fn cancelled_export_before_content_never_touches_sftp() {
    let (service, file, files) = fixture(b"test");
    let before = files.lock().unwrap().requests;
    let (observe, events) = observer();
    let source = SftpReadSource::new(Arc::downgrade(&service), file, observe);
    let stream = source.open().unwrap();
    source.cancel();
    drop(stream);
    assert!(
        finished(&events)
            .unwrap_err()
            .contains(crate::sftp::SFTP_TRANSFER_CANCELLED)
    );
    assert_eq!(files.lock().unwrap().requests, before);
}

#[test]
fn consumer_closing_early_and_connection_owner_drop_cancel_reads() {
    for disconnect in [false, true] {
        let (service, file, _) = fixture(b"content");
        let (observe, events) = observer();
        let source = SftpReadSource::new(Arc::downgrade(&service), file, observe);
        let mut stream = source.open().unwrap();
        assert_eq!(stream.read_at(0, &mut [0; 2]).unwrap(), 2);
        if disconnect {
            drop(service);
            assert!(finished(&events).is_err());
            assert!(stream.read_at(2, &mut [0; 2]).is_err());
        } else {
            drop(stream);
            assert!(finished(&events).is_err());
        }
    }
}

#[test]
fn zero_byte_file_completes_and_changed_size_or_missing_path_fails() {
    for (bytes, change) in [
        (b"".as_slice(), 0),
        (b"content".as_slice(), 1),
        (b"content".as_slice(), 2),
    ] {
        let (service, mut file, files) = fixture(bytes);
        if change == 1 {
            file.size = Some(100);
        }
        if change == 2 {
            files.lock().unwrap().contents.clear();
        }
        let (observe, events) = observer();
        let source = SftpReadSource::new(Arc::downgrade(&service), file, observe);
        let mut stream = source.open().unwrap();
        let read = stream.read_at(0, &mut [0; 1]);
        drop(stream);
        if change == 0 {
            assert_eq!(read.unwrap(), 0);
            assert_eq!(finished(&events).unwrap(), 0);
        } else {
            assert!(read.is_err());
            assert!(finished(&events).is_err());
        }
    }
}

#[test]
fn cancellation_releases_compatibility_gate_for_following_operations() {
    let (service, file, _) = fixture(b"content");
    let (observe, events) = observer();
    let source = SftpReadSource::new(Arc::downgrade(&service), file, observe);
    let mut stream = source.open().unwrap();
    assert_eq!(stream.read_at(0, &mut [0; 1]).unwrap(), 1);
    drop(stream);
    assert!(finished(&events).is_err());
    let sftp = service.export_sftp_service();
    let cache = sftp.compatibility.as_ref().unwrap().cache.clone();
    compatibility_runtime()
        .unwrap()
        .block_on(SFTP_COMPATIBILITY_CACHE.scope(Some(cache), async {
            let control = SftpTransferControl::new();
            let _guard = control
                .until_cancelled(sftp.compatibility.as_ref().unwrap().gate.lock())
                .await
                .unwrap();
        }));
}

#[test]
fn closed_export_session_is_evicted_before_releasing_gate_and_permit() {
    let (service, file, _) = fixture(b"content");
    let sftp = service.export_sftp_service();
    let state = sftp.compatibility.as_ref().unwrap();
    let session = state.cache.lock().unwrap().as_ref().unwrap().clone();
    compatibility_runtime()
        .unwrap()
        .block_on(async { session.sftp.close().await.unwrap() });

    let (observe, events) = observer();
    let source = SftpReadSource::new(Arc::downgrade(&service), file, observe);
    let mut stream = source.open().unwrap();
    assert!(stream.read_at(0, &mut [0; 1]).is_err());
    drop(stream);
    assert!(finished(&events).is_err());
    assert!(state.cache.lock().unwrap().is_none());
    assert!(state.gate.try_lock().is_ok());
    assert!(sftp.export_budget.clone().try_acquire_owned().is_ok());
}

#[test]
fn idle_incomplete_native_streams_do_not_starve_later_reads() {
    // A target can retain the original IStream while reading its Clone. More
    // than the operation budget may stay open without occupying network slots.
    let (service, file, _) = fixture(&vec![7; 1024 * 1024]);
    let (observe, events) = observer();
    let source = Arc::new(SftpReadSource::new(Arc::downgrade(&service), file, observe));
    let (done, result) = mpsc::channel();
    let worker = {
        let source = source.clone();
        std::thread::spawn(move || {
            let mut streams = Vec::new();
            for _ in 0..5 {
                let mut stream = source.open().unwrap();
                let read = stream.read_at(0, &mut [0; 1]);
                if read.is_err() {
                    return;
                }
                streams.push(stream);
            }
            // The first stream reacquires a slot after the others became idle.
            assert_eq!(streams[0].read_at(800_000, &mut [0; 1]).unwrap(), 1);
            done.send(()).unwrap();
        })
    };
    let completed = result.recv_timeout(Duration::from_secs(2));
    source.cancel();
    worker.join().unwrap();
    assert!(
        completed.is_ok(),
        "idle streams starved the next native read"
    );
    for _ in 0..5 {
        assert!(finished(&events).is_err());
    }
    assert!(
        service
            .export_sftp_service()
            .compatibility
            .as_ref()
            .unwrap()
            .gate
            .try_lock()
            .is_ok()
    );
}

#[test]
fn concurrent_export_honors_options_and_compatibility_uses_one_read() {
    for compatibility in [false, true] {
        let content: Vec<u8> = (0..700_000).map(|i| (i % 251) as u8).collect();
        let (service, file, files) = fixture_mode(&content, compatibility);
        files.lock().unwrap().read_delay = Duration::from_millis(5);
        let (observe, events) = observer();
        let source = SftpReadSource::new(Arc::downgrade(&service), file, observe)
            .with_transfer_options(
                crate::sftp_transfer_types::SftpTransferOptions::default()
                    .with_buffer_size_bytes(128 * 1024),
            );
        let mut stream = source.open().unwrap();
        let mut buffer = [0; 64 * 1024];
        let mut offset = 0;
        while offset < content.len() {
            let count = stream.read_at(offset as u64, &mut buffer).unwrap();
            assert_eq!(&buffer[..count], &content[offset..offset + count]);
            offset += count;
        }
        drop(stream);
        assert_eq!(finished(&events).unwrap(), content.len() as u64);
        let peer = files.lock().unwrap();
        assert_eq!(peer.max_active_reads, if compatibility { 1 } else { 6 });
        assert!(peer.reads.iter().any(|(_, length)| *length > 64 * 1024));
    }
}

#[test]
fn small_reads_share_cache_and_prefetch_does_not_advance_delivery_progress() {
    let (service, file, files) = fixture_mode(&vec![7; 1024 * 1024], false);
    let (observe, events) = observer();
    let source = SftpReadSource::new(Arc::downgrade(&service), file, observe);
    let mut stream = source.open().unwrap();
    assert_eq!(stream.read_at(0, &mut [0; 2]).unwrap(), 2);
    for offset in 1..30 {
        assert_eq!(stream.read_at(offset, &mut [0; 2]).unwrap(), 2);
    }
    // Let the idle worker fill its bounded window, then close before consuming it.
    std::thread::sleep(Duration::from_millis(50));
    let reads = files.lock().unwrap().reads.clone();
    assert_eq!(reads.iter().filter(|(offset, _)| *offset == 0).count(), 1);
    assert!(reads.len() <= 32);
    drop(stream);
    let mut completed = false;
    loop {
        match events.recv_timeout(Duration::from_secs(2)).unwrap() {
            ExportReadEvent::Progress { bytes, .. } => assert!(bytes <= 31),
            ExportReadEvent::Finished(result) => {
                assert!(result.is_err());
                completed = true;
            }
            _ => {}
        }
        if completed {
            break;
        }
    }
}

#[test]
fn short_packets_are_filled_and_unknown_metadata_size_uses_demand_reads() {
    for unknown in [false, true] {
        let content: Vec<u8> = (0..90_003).map(|i| (i % 251) as u8).collect();
        let (service, mut file, files) = fixture_mode(&content, false);
        {
            let mut peer = files.lock().unwrap();
            peer.read_limit = Some(4096);
            peer.omit_size = unknown;
        }
        if unknown {
            file.size = None;
        }
        let (observe, events) = observer();
        let source = SftpReadSource::new(Arc::downgrade(&service), file, observe);
        let mut stream = source.open().unwrap();
        let mut buffer = [0; 64 * 1024];
        let mut offset = 0;
        loop {
            let count = stream.read_at(offset as u64, &mut buffer).unwrap();
            assert_eq!(&buffer[..count], &content[offset..offset + count]);
            offset += count;
            if count == 0 {
                break;
            }
        }
        drop(stream);
        assert_eq!(finished(&events).unwrap(), content.len() as u64);
        assert_eq!(offset, content.len());
    }
}

#[test]
fn speculative_sftp_failure_is_reported_only_when_consumed() {
    let (service, file, files) = fixture_mode(&vec![3; 300_000], false);
    files.lock().unwrap().fail_read_at = Some(2 * 63 * 1024);
    let (observe, events) = observer();
    let source = SftpReadSource::new(Arc::downgrade(&service), file, observe);
    let mut stream = source.open().unwrap();
    let mut buffer = [0; 64 * 1024];
    assert_eq!(stream.read_at(0, &mut buffer).unwrap(), buffer.len());
    assert!(stream.read_at(64 * 1024, &mut buffer).is_err());
    drop(stream);
    assert!(finished(&events).is_err());
}

/// A manual measurement, not a timing-sensitive CI assertion. Both paths use
/// the actual SFTP request/reply peer with independently delayed READ responses.
#[test]
#[ignore = "50 MiB / 50 ms latency throughput measurement"]
fn drag_prefetch_latency_benchmark() {
    use sha2::{Digest as _, Sha256};
    use std::time::Instant;

    let content: Vec<u8> = (0..50 * 1024 * 1024).map(|i| (i % 251) as u8).collect();
    let expected = Sha256::digest(&content);
    let mut elapsed = Vec::new();
    for concurrent in [false, true] {
        let (service, file, files) = fixture_mode(&content, false);
        files.lock().unwrap().read_delay = Duration::from_millis(50);
        let start = Instant::now();
        let mut hash = Sha256::new();
        if concurrent {
            let (observe, events) = observer();
            let source = SftpReadSource::new(Arc::downgrade(&service), file, observe);
            let mut stream = source.open().unwrap();
            let mut buffer = [0; 64 * 1024];
            let mut offset = 0;
            while offset < content.len() {
                let count = stream.read_at(offset as u64, &mut buffer).unwrap();
                hash.update(&buffer[..count]);
                offset += count;
            }
            drop(stream);
            assert_eq!(finished(&events).unwrap(), content.len() as u64);
        } else {
            let sftp = service.export_sftp_service();
            let cache = sftp.compatibility.as_ref().unwrap().cache.clone();
            compatibility_runtime()
                .unwrap()
                .block_on(SFTP_COMPATIBILITY_CACHE.scope(Some(cache), async {
                    let session = super::super::open_sftp_session(&sftp.config, None)
                        .await
                        .unwrap();
                    let remote = session
                        .sftp
                        .open_bytes(b"/raw-\xff".to_vec())
                        .await
                        .unwrap();
                    let control = SftpTransferControl::new();
                    for offset in (0..content.len()).step_by(64 * 1024) {
                        let bytes = super::super::read_transfer_range(
                            &remote,
                            &control,
                            offset as u64,
                            64 * 1024,
                        )
                        .await
                        .unwrap();
                        assert_eq!(bytes.len(), 64 * 1024);
                        hash.update(bytes);
                    }
                }));
        }
        let duration = start.elapsed();
        assert_eq!(hash.finalize(), expected);
        println!(
            "{}: {:.3}s, {:.3} MiB/s, max concurrent reads {}",
            if concurrent {
                "prefetch"
            } else {
                "serial baseline"
            },
            duration.as_secs_f64(),
            50.0 / duration.as_secs_f64(),
            files.lock().unwrap().max_active_reads
        );
        elapsed.push(duration.as_secs_f64());
    }
    println!("speedup: {:.2}x", elapsed[0] / elapsed[1]);
}

#[test]
fn directory_export_enumerates_raw_paths_and_empty_folders_without_reading_contents() {
    use crate::sftp::{SftpFileEntry, SftpFileType};
    let (service, _, files) = fixture(b"unrelated");
    {
        let mut files = files.lock().unwrap();
        files.directories.insert(b"/tree-\xff".to_vec());
        files.directories.insert(b"/tree-\xff/empty".to_vec());
        files
            .contents
            .insert(b"/tree-\xff/raw-\xfe".to_vec(), b"nested".to_vec());
    }
    let root = SftpFileEntry {
        name: "folder".into(),
        path: "/display-only".into(),
        file_type: SftpFileType::Directory,
        size: None,
        permissions: None,
        owner: String::new(),
        group: String::new(),
        modified_at: None,
        raw_path_token: RemoteFilePath::from_raw("/display-only", b"/tree-\xff").raw_path_token,
        symlink_target_is_directory: false,
    };
    let entries = crate::drag_export::tree::enumerate_remote_drag(
        &service,
        vec![root.clone()],
        &SftpTransferControl::new(),
    )
    .unwrap();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].relative_path, std::path::PathBuf::from("folder"));
    assert!(entries[0].is_directory);
    assert!(
        entries
            .iter()
            .any(|entry| entry.is_directory && entry.relative_path.ends_with("empty"))
    );
    let file = entries.iter().find(|entry| !entry.is_directory).unwrap();
    assert_eq!(
        file.file.remote_path.raw_path().unwrap().unwrap(),
        b"/tree-\xff/raw-\xfe"
    );
    assert_eq!(file.file.size, Some(6));
    assert!(files.lock().unwrap().reads.is_empty());
    let cancelled = SftpTransferControl::new();
    cancelled.cancel();
    let before = files.lock().unwrap().requests;
    assert!(
        crate::drag_export::tree::enumerate_remote_drag(&service, vec![root.clone()], &cancelled)
            .is_err()
    );
    assert_eq!(files.lock().unwrap().requests, before);
    files
        .lock()
        .unwrap()
        .directories
        .remove(b"/tree-\xff".as_slice());
    files
        .lock()
        .unwrap()
        .contents
        .insert(b"/tree-\xff".to_vec(), b"changed type".to_vec());
    assert!(
        crate::drag_export::tree::enumerate_remote_drag(
            &service,
            vec![root],
            &SftpTransferControl::new()
        )
        .is_err()
    );
}
