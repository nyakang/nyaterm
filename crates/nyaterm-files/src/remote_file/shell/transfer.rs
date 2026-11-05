use super::metadata::{shell_file_replace_command, shell_upload_properties};
use super::{REMOTE_FILE_COMMAND_TIMEOUT, ShellRemote, shell_quote, shell_quote_path};
use crate::download_path::{DownloadTargetAccess, TransferDownload};
use crate::sftp::SftpTransferControl;
use crate::sftp_transfer_types::SftpTransferOptions;
use futures::{Stream, StreamExt as _};
use nyaterm_transport::ssh_channel::SshChannelConnection;
use russh::ChannelMsg;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};
use tokio::time::Instant;

#[cfg(test)]
mod tests;

/// Uploads and replies share one inactivity clock: a server need not produce
/// stdout while it is still consuming a large stdin stream.
#[derive(Clone)]
struct TransferActivity {
    last: Arc<Mutex<Instant>>,
    timeout: Duration,
}

impl TransferActivity {
    fn new(timeout: Duration) -> Self {
        Self {
            last: Arc::new(Mutex::new(Instant::now())),
            timeout,
        }
    }

    fn touch(&self) {
        *self.last.lock().expect("transfer activity lock") = Instant::now();
    }

    async fn step<T>(
        &self,
        control: &SftpTransferControl,
        operation: impl Future<Output = T>,
    ) -> anyhow::Result<T> {
        tokio::pin!(operation);
        loop {
            control.check_cancelled()?;
            if control.is_paused() {
                self.touch();
            } else if self.last.lock().expect("transfer activity lock").elapsed() >= self.timeout {
                anyhow::bail!("remote file transfer timed out");
            }
            tokio::select! {
                result = &mut operation, if !control.is_paused() => {
                    self.touch();
                    return Ok(result);
                }
                _ = tokio::time::sleep(Duration::from_millis(25)) => {}
            }
        }
    }
}

enum StreamFile {
    Upload(PathBuf),
    Download {
        root: PathBuf,
        target: PathBuf,
        expected_size: Option<u64>,
        modified_at: Option<u32>,
    },
}

impl ShellRemote {
    pub(in crate::remote_file) fn upload_stream<F>(
        &self,
        local: &Path,
        remote: &str,
        control: SftpTransferControl,
        options: &SftpTransferOptions,
        progress: F,
    ) -> anyhow::Result<u64>
    where
        F: FnMut(u64) + Send + 'static,
    {
        let metadata = std::fs::metadata(local)?;
        anyhow::ensure!(metadata.is_file(), "upload source is not a regular file");
        let length = metadata.len();
        let temporary = format!("{remote}.nyaterm-upload-{}", uuid::Uuid::new_v4().simple());
        control.wait_if_paused_blocking()?;
        let original = shell_upload_properties(self, remote)?;
        let replace = if let Some(properties) = original {
            shell_file_replace_command(&properties, &temporary, remote)?
        } else {
            let mode = options
                .default_file_mode
                .map(|mode| format!("chmod {mode:o} {} && ", shell_quote_path(&temporary)))
                .unwrap_or_default();
            format!(
                "{mode}mv -f {} {}",
                shell_quote_path(&temporary),
                shell_quote_path(remote)
            )
        };
        let command = format!(
            "umask 077; cat > {temp} && test $(wc -c < {temp}) -eq {length} && {replace}",
            temp = shell_quote(&temporary),
        );
        let result = self.stream_file(
            command,
            StreamFile::Upload(local.to_path_buf()),
            control,
            options.buffer_size_bytes(),
            progress,
        );
        if result.is_err() {
            let _ = self.exec(format!("rm -f -- {}", shell_quote(&temporary)), None);
        }
        result
    }

    #[expect(clippy::too_many_arguments)]
    pub(in crate::remote_file) fn download_stream<F>(
        &self,
        remote: &str,
        local: &Path,
        root: &Path,
        expected_size: Option<u64>,
        modified_at: Option<u32>,
        control: SftpTransferControl,
        progress: F,
    ) -> anyhow::Result<u64>
    where
        F: FnMut(u64) + Send + 'static,
    {
        self.stream_file(
            format!("cat -- {}", shell_quote(remote)),
            StreamFile::Download {
                root: root.to_path_buf(),
                target: local.to_path_buf(),
                expected_size,
                modified_at,
            },
            control,
            crate::sftp_transfer_types::SFTP_TRANSFER_DEFAULT_BUFFER_SIZE,
            progress,
        )
    }

    fn stream_file<F>(
        &self,
        command: String,
        file: StreamFile,
        control: SftpTransferControl,
        buffer_size: usize,
        mut progress: F,
    ) -> anyhow::Result<u64>
    where
        F: FnMut(u64) + Send + 'static,
    {
        let config = self.config.clone();
        let multiplex = self.multiplex.clone();
        let operation = async move {
            let activity = TransferActivity::new(REMOTE_FILE_COMMAND_TIMEOUT);
            let owner = activity
                .step(
                    &control,
                    SshChannelConnection::connect(&config, multiplex.as_ref()),
                )
                .await??;
            let result = async {
                let channel = activity.step(&control, owner.open_session()).await??;
                let (reader, writer) = channel.split();
                let result = async {
                    activity
                        .step(&control, writer.exec(true, command.into_bytes()))
                        .await??;
                    let messages =
                        Box::pin(futures::stream::unfold(reader, |mut reader| async move {
                            reader.wait().await.map(|message| (message, reader))
                        }));
                    match file {
                        StreamFile::Upload(path) => {
                            let mut input = tokio::fs::File::open(path).await?;
                            let mut output = writer.make_writer();
                            let send = async {
                                let bytes = send_file(
                                    &mut input,
                                    &mut output,
                                    buffer_size,
                                    &control,
                                    &activity,
                                    &mut progress,
                                )
                                .await?;
                                activity.step(&control, writer.eof()).await??;
                                Ok::<_, anyhow::Error>(bytes)
                            };
                            let mut ignore_progress = |_| {};
                            let receive = receive_file(
                                messages,
                                tokio::io::sink(),
                                false,
                                &control,
                                &activity,
                                &mut ignore_progress,
                            );
                            let (bytes, _) = tokio::try_join!(send, receive)?;
                            Ok(bytes)
                        }
                        StreamFile::Download {
                            root,
                            target,
                            expected_size,
                            modified_at,
                        } => {
                            let access = DownloadTargetAccess::capture(&root, &target)?;
                            let (temporary, mut output) =
                                TransferDownload::prepare_with_access(&root, &target, 0, access)
                                    .await?;
                            let result = async {
                                activity.step(&control, writer.eof()).await??;
                                let bytes = receive_file(
                                    messages,
                                    &mut output,
                                    true,
                                    &control,
                                    &activity,
                                    &mut progress,
                                )
                                .await?;
                                activity.step(&control, output.flush()).await??;
                                Ok::<_, anyhow::Error>(bytes)
                            }
                            .await;
                            let output = output.into_std().await;
                            if result.is_ok()
                                && let Some(seconds) = modified_at
                            {
                                output.set_modified(
                                    std::time::UNIX_EPOCH + Duration::from_secs(u64::from(seconds)),
                                )?;
                            }
                            drop(output);
                            let bytes = result?;
                            anyhow::ensure!(
                                expected_size.is_none_or(|expected| expected == bytes),
                                "remote download size changed during transfer"
                            );
                            control.wait_if_paused().await?;
                            temporary.commit_async(&root, &target).await?;
                            Ok(bytes)
                        }
                    }
                }
                .await;
                let _ = tokio::time::timeout(Duration::from_secs(1), writer.close()).await;
                result
            }
            .await;
            let _ = owner.close("file transfer completed").await;
            result
        };
        if let Some(multiplex) = self.multiplex.as_ref() {
            multiplex.block_on(operation)
        } else {
            nyaterm_transport::ssh_execution::run_ssh_exec_operation(operation)
        }
    }
}

async fn send_file<R, W, F>(
    input: &mut R,
    output: &mut W,
    buffer_size: usize,
    control: &SftpTransferControl,
    activity: &TransferActivity,
    progress: &mut F,
) -> anyhow::Result<u64>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
    F: FnMut(u64),
{
    let mut buffer = vec![0_u8; buffer_size];
    let mut bytes = 0_u64;
    loop {
        let read = activity.step(control, input.read(&mut buffer)).await??;
        if read == 0 {
            break;
        }
        activity
            .step(control, output.write_all(&buffer[..read]))
            .await??;
        bytes = bytes.saturating_add(read as u64);
        progress(bytes);
    }
    activity.step(control, output.flush()).await??;
    Ok(bytes)
}

async fn receive_file<S, W, F>(
    mut messages: S,
    mut output: W,
    download: bool,
    control: &SftpTransferControl,
    activity: &TransferActivity,
    progress: &mut F,
) -> anyhow::Result<u64>
where
    S: Stream<Item = ChannelMsg> + Unpin,
    W: AsyncWrite + Unpin,
    F: FnMut(u64),
{
    let mut bytes = 0_u64;
    let mut exit_status = None;
    let mut eof = false;
    loop {
        match activity.step(control, messages.next()).await? {
            Some(ChannelMsg::Data { data }) if download => {
                activity.step(control, output.write_all(&data)).await??;
                bytes = bytes.saturating_add(data.len() as u64);
                progress(bytes);
            }
            Some(ChannelMsg::ExitStatus {
                exit_status: status,
            }) => {
                exit_status = Some(status);
                if eof {
                    break;
                }
            }
            Some(ChannelMsg::Eof) => {
                eof = true;
                if exit_status.is_some() {
                    break;
                }
            }
            Some(ChannelMsg::Close) | None => break,
            Some(ChannelMsg::Failure) => anyhow::bail!("remote file transfer request failed"),
            _ => {}
        }
    }
    anyhow::ensure!(
        exit_status == Some(0),
        "remote file transfer failed with status {}",
        exit_status.unwrap_or(255)
    );
    Ok(bytes)
}
