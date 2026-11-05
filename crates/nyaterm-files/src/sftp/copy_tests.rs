use super::copy_remote_path_between_sftp;
use super::editor_save_tests::seeded_service;
use crate::remote_file::FileTransferEndpoint;
use crate::remote_file::copy_progress::CopyProgress;
use crate::sftp::{SFTP_TRANSFER_CANCELLED, SftpTransferControl};
use crate::sftp_transfer_types::SftpTransferOptions;
use std::sync::{Arc, Mutex};

#[test]
fn sftp_copy_reports_partial_writes_and_cleans_temporary_file_on_cancellation() {
    for cancel_midway in [false, true] {
        let (service, files) = seeded_service(false);
        let payload = vec![7_u8; 512 * 1024];
        files
            .lock()
            .unwrap()
            .files
            .insert("/file.txt".into(), payload.clone());
        let root = tempfile::tempdir().unwrap();
        let local = root.path().join("source");
        std::fs::write(&local, &payload).unwrap();
        let source = FileTransferEndpoint::Local(local);
        let target = FileTransferEndpoint::Local(root.path().join("target"));
        let values = Arc::new(Mutex::new(Vec::new()));
        let record = values.clone();
        let control = SftpTransferControl::new();
        let cancel = control.clone();
        let reporter = CopyProgress::new(&source, &target, 1, move |p| {
            if cancel_midway && p.bytes_transferred > 0 {
                cancel.cancel();
            }
            record.lock().unwrap().push(p);
        });
        reporter.scan(&source, &control).unwrap();
        let compatibility = service.compatibility.as_ref().unwrap();
        let sftp = compatibility
            .cache
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .sftp
            .clone();
        let finished = reporter.clone();
        let result = compatibility.block_on(async move {
            let bytes = copy_remote_path_between_sftp(
                &sftp,
                b"/file.txt".to_vec(),
                &sftp,
                b"/copy.txt".to_vec(),
                &control,
                &SftpTransferOptions::default(),
                &reporter,
            )
            .await?;
            finished.finish(bytes);
            Ok(bytes)
        });
        let files = files.lock().unwrap();
        assert_eq!(files.files["/file.txt"], payload);
        assert!(
            !files
                .files
                .keys()
                .any(|path| path.contains(".nyaterm-copy-"))
        );
        let values = values.lock().unwrap();
        assert!(
            values
                .iter()
                .any(|p| p.bytes_transferred > 0 && p.bytes_transferred < payload.len() as u64)
        );
        if cancel_midway {
            assert_eq!(result.unwrap_err().to_string(), SFTP_TRANSFER_CANCELLED);
            assert!(!files.files.contains_key("/copy.txt"));
            assert!(values.iter().all(|p| p.item_count_completed != Some(1)));
        } else {
            assert_eq!(result.unwrap(), payload.len() as u64);
            assert_eq!(files.files["/copy.txt"], payload);
            assert_eq!(
                values.last().unwrap().bytes_transferred,
                payload.len() as u64
            );
            assert_eq!(values.last().unwrap().item_count_completed, Some(1));
        }
    }
}
