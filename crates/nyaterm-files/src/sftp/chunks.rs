//! Bounded binary access for callers that own their own transfer/task lifecycle.
use crate::sftp::{
    RemoteFilePath, SftpPathCodec, SftpService, close_sftp_session, remote_file_path_bytes,
};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

impl SftpService {
    pub fn read_chunk(
        &self,
        path: &str,
        offset: u64,
        length: u32,
        cancelled: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> anyhow::Result<Vec<u8>> {
        if length > 1024 * 1024 {
            anyhow::bail!("Read chunk exceeds limit");
        }
        self.binary_chunk(path, offset, Some(length), Vec::new(), cancelled)
    }

    /// Offset zero replaces the file; subsequent chunks append at explicit offsets.
    pub fn write_chunk(
        &self,
        path: &str,
        offset: u64,
        bytes: Vec<u8>,
        cancelled: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> anyhow::Result<()> {
        if bytes.len() > 1024 * 1024 {
            anyhow::bail!("Write chunk exceeds limit");
        }
        self.binary_chunk(path, offset, None, bytes, cancelled)
            .map(|_| ())
    }

    fn binary_chunk(
        &self,
        path: &str,
        offset: u64,
        length: Option<u32>,
        bytes: Vec<u8>,
        cancelled: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> anyhow::Result<Vec<u8>> {
        let path = RemoteFilePath::new(path);
        let config = self.config.clone();
        let factory = self.session_factory();
        self.run_operation("binary_chunk", async move {
            if cancelled() { anyhow::bail!("File operation cancelled"); }
            let operation = async {
                let codec = SftpPathCodec::from_ssh_config(&config)?;
                let path = remote_file_path_bytes(&codec, &path)?;
                let session = factory.open().await?;
                let result = async {
                    if cancelled() { anyhow::bail!("File operation cancelled"); }
                    let mut file = if length.is_some() {
                        session.sftp.open_bytes(path).await?
                    } else {
                        use russh_sftp::protocol::OpenFlags;
                        let mut flags = OpenFlags::WRITE | OpenFlags::CREATE;
                        if offset == 0 { flags |= OpenFlags::TRUNCATE; }
                        session.sftp.open_with_flags_bytes(path, flags).await?
                    };
                    file.seek(std::io::SeekFrom::Start(offset)).await?;
                    let mut result = Vec::new();
                    if let Some(length) = length {
                        (&mut file).take(u64::from(length)).read_to_end(&mut result).await?;
                    } else {
                        if cancelled() { anyhow::bail!("File operation cancelled"); }
                        file.write_all(&bytes).await?;
                    }
                    file.shutdown().await?;
                    Ok(result)
                }.await;
                close_sftp_session(session).await;
                result
            };
            tokio::select! {
                result = operation => result,
                _ = async { while !cancelled() { tokio::time::sleep(std::time::Duration::from_millis(25)).await; } } => anyhow::bail!("File operation cancelled"),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::sftp::editor_save_tests::seeded_service;
    use std::sync::Arc;

    #[test]
    fn bounded_chunks_replace_then_continue_at_offsets_and_read_exact_ranges() {
        let (service, files) = seeded_service(false);
        files
            .lock()
            .unwrap()
            .files
            .insert("/file.txt".into(), b"old long contents".to_vec());
        service
            .write_chunk("/file.txt", 0, b"new".to_vec(), Arc::new(|| false))
            .unwrap();
        assert_eq!(files.lock().unwrap().files["/file.txt"], b"new");
        service
            .write_chunk("/file.txt", 3, b" bytes".to_vec(), Arc::new(|| false))
            .unwrap();
        assert_eq!(files.lock().unwrap().files["/file.txt"], b"new bytes");
        assert_eq!(
            service
                .read_chunk("/file.txt", 2, 4, Arc::new(|| false))
                .unwrap(),
            b"w by"
        );
        assert!(
            service
                .read_chunk("/file.txt", 9, 10, Arc::new(|| false))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn rejected_or_cancelled_chunks_leave_the_remote_file_unchanged() {
        let (service, files) = seeded_service(false);
        assert!(
            service
                .write_chunk("/file.txt", 0, b"changed".to_vec(), Arc::new(|| true))
                .is_err()
        );
        assert!(
            service
                .write_chunk("/file.txt", 0, vec![0; 1024 * 1024 + 1], Arc::new(|| false))
                .is_err()
        );
        assert!(
            service
                .read_chunk("/file.txt", 0, 1024 * 1024 + 1, Arc::new(|| false))
                .is_err()
        );
        assert_eq!(files.lock().unwrap().files["/file.txt"], b"old");
        assert_eq!(
            service
                .read_chunk("/file.txt", 0, 3, Arc::new(|| false))
                .unwrap(),
            b"old"
        );
    }
}
