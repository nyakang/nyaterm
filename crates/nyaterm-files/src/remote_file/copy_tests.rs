use super::copy_progress::CopyProgress;
use super::{FileCopyRequest, FileTransferEndpoint};
use crate::sftp::{SFTP_TRANSFER_CANCELLED, SftpTransferControl};
use crate::sftp_transfer_types::{SftpPathTransferOptions, SftpTransferProgress};
use nyaterm_core::test_support::TestTempDir;
use std::fs;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

fn request(source: std::path::PathBuf, target: std::path::PathBuf) -> FileCopyRequest {
    FileCopyRequest {
        source: FileTransferEndpoint::Local(source),
        destination: FileTransferEndpoint::Local(target),
        control: SftpTransferControl::new(),
        options: SftpPathTransferOptions::default(),
    }
}

#[test]
fn local_large_file_reports_partial_writes_and_cumulative_directory_progress() {
    let root = TestTempDir::new("nyaterm-copy-progress");
    fs::create_dir_all(root.path()).unwrap();
    let source = root.join("source");
    let target = root.join("target");
    fs::create_dir_all(source.join("empty-dir")).unwrap();
    fs::write(source.join("empty-file"), []).unwrap();
    let content = vec![42_u8; 512 * 1024];
    fs::write(source.join("large-file"), &content).unwrap();
    let values = Arc::new(Mutex::new(Vec::new()));
    let record = values.clone();
    let summary = request(source.clone(), target.clone())
        .execute_with_progress(move |value| record.lock().unwrap().push(value))
        .unwrap();
    assert_eq!(summary.bytes, content.len() as u64);
    assert_eq!(fs::read(target.join("large-file")).unwrap(), content);
    assert!(target.join("empty-dir").is_dir());
    let values = values.lock().unwrap();
    assert!(
        values
            .iter()
            .any(|p| p.bytes_transferred > 0 && p.bytes_transferred < summary.bytes)
    );
    assert!(
        values
            .windows(2)
            .all(|pair| pair[0].bytes_transferred <= pair[1].bytes_transferred)
    );
    let final_value = values.last().unwrap();
    assert_eq!(final_value.bytes_transferred, summary.bytes);
    assert_eq!(final_value.total_bytes, Some(summary.bytes));
    assert_eq!(final_value.item_count_total, Some(4));
    assert_eq!(final_value.item_count_completed, Some(4));
    assert!(
        values[..values.len() - 1]
            .iter()
            .all(|p| p.item_count_total.is_none() || p.item_count_completed != p.item_count_total)
    );
}

#[test]
fn cancelling_a_partial_copy_keeps_source_and_never_reports_completion() {
    let root = TestTempDir::new("nyaterm-copy-cancel");
    fs::create_dir_all(root.path()).unwrap();
    let source = root.join("source");
    fs::write(&source, vec![7; 512 * 1024]).unwrap();
    let target = root.join("target");
    fs::write(&target, b"original").unwrap();
    let request = request(source.clone(), target.clone());
    let cancel = request.control.clone();
    let values = Arc::new(Mutex::new(Vec::new()));
    let record = values.clone();
    let error = request
        .execute_with_progress(move |value| {
            if value.bytes_transferred > 0 {
                cancel.cancel();
            }
            record.lock().unwrap().push(value);
        })
        .unwrap_err();
    assert_eq!(error.to_string(), SFTP_TRANSFER_CANCELLED);
    assert_eq!(fs::metadata(source).unwrap().len(), 512 * 1024);
    assert_eq!(fs::read(target).unwrap(), b"original");
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 2);
    assert!(
        values
            .lock()
            .unwrap()
            .iter()
            .all(|p| p.item_count_total.is_none() || p.item_count_completed != p.item_count_total)
    );
}

#[test]
fn paused_copy_can_be_resumed_without_resetting_its_byte_counter() {
    let root = TestTempDir::new("nyaterm-copy-pause");
    fs::create_dir_all(root.path()).unwrap();
    let source = root.join("source");
    fs::write(&source, vec![7; 512 * 1024]).unwrap();
    let request = request(source, root.join("target"));
    let control = request.control.clone();
    let paused = control.clone();
    let (sender, receiver) = mpsc::sync_channel(1);
    let worker = std::thread::spawn(move || {
        let mut stopped = false;
        request.execute_with_progress(move |p| {
            if !stopped && p.bytes_transferred > 0 {
                stopped = true;
                paused.pause();
                sender.send(p.bytes_transferred).unwrap();
            }
        })
    });
    let bytes = receiver.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(bytes < 512 * 1024);
    assert!(control.is_paused());
    control.resume();
    assert_eq!(worker.join().unwrap().unwrap().bytes, 512 * 1024);
}

#[test]
fn empty_directory_copy_finishes_using_items_without_dividing_by_zero() {
    let root = TestTempDir::new("nyaterm-copy-empty");
    fs::create_dir_all(root.path()).unwrap();
    let source = root.join("source");
    fs::create_dir(&source).unwrap();
    let values = Arc::new(Mutex::new(Vec::new()));
    let record = values.clone();
    let summary = request(source, root.join("target"))
        .execute_with_progress(move |p| record.lock().unwrap().push(p))
        .unwrap();
    assert_eq!(summary.bytes, 0);
    let values = values.lock().unwrap();
    assert_eq!(values.last().unwrap().item_count_completed, Some(1));
    assert_eq!(values.last().unwrap().total_bytes, Some(0));
}

#[test]
fn staged_copy_combines_both_legs_and_does_not_finish_at_download_completion() {
    let root = TestTempDir::new("nyaterm-copy-stages");
    fs::create_dir_all(root.path()).unwrap();
    let source = root.join("source");
    fs::write(&source, [0_u8; 100]).unwrap();
    let source = FileTransferEndpoint::Local(source);
    let target = FileTransferEndpoint::Local(root.join("target"));
    let values = Arc::new(Mutex::new(Vec::new()));
    let record = values.clone();
    let reporter = CopyProgress::new(&source, &target, 2, move |p| record.lock().unwrap().push(p));
    reporter.scan(&source, &SftpTransferControl::new()).unwrap();
    let stage = |bytes| SftpTransferProgress {
        phase: None,
        remote_path: String::new(),
        local_path: Default::default(),
        bytes_transferred: bytes,
        total_bytes: Some(100),
        item_count_completed: Some(1),
        item_count_total: Some(1),
    };
    reporter.update(stage(100));
    reporter.next_stage(100);
    reporter.update(stage(25));
    // Delayed or coalesced smaller counters must not move the overall bar backwards.
    reporter.update(stage(20));
    reporter.update(stage(100));
    reporter.finish(100);
    let values = values.lock().unwrap();
    assert!(
        values
            .iter()
            .any(|p| p.bytes_transferred == 100 && p.total_bytes == Some(200))
    );
    assert!(values.iter().any(|p| p.bytes_transferred == 125));
    assert!(
        values
            .windows(2)
            .all(|p| p[0].bytes_transferred <= p[1].bytes_transferred)
    );
    assert_eq!(values.last().unwrap().bytes_transferred, 200);
    assert_eq!(values.last().unwrap().item_count_completed, Some(2));
    assert!(
        values[..values.len() - 1]
            .iter()
            .all(|p| p.item_count_completed != Some(2))
    );
}

#[test]
fn local_copy_rejects_hard_link_to_source_without_truncating_it() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    let target = root.path().join("target");
    fs::write(&source, b"original").unwrap();
    fs::hard_link(&source, &target).unwrap();
    assert!(request(source.clone(), target).execute().is_err());
    assert_eq!(fs::read(source).unwrap(), b"original");
}
