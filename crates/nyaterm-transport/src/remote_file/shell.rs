use std::time::Duration;

use russh::{ChannelMsg, Disconnect};

use crate::{
    SshMultiplexHandle, SshSessionConfig, open_authenticated_ssh_handle,
    remote_file::RemoteFileBackendKind,
};

const REMOTE_FILE_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Default)]
pub(super) struct ShellOutput {
    pub(super) stdout: Vec<u8>,
    pub(super) stderr: Vec<u8>,
    pub(super) exit_status: Option<u32>,
}

#[derive(Clone)]
pub(super) struct ShellRemote {
    config: SshSessionConfig,
    multiplex: Option<SshMultiplexHandle>,
    kind: RemoteFileBackendKind,
}

impl ShellRemote {
    pub(super) fn new(
        config: SshSessionConfig,
        multiplex: Option<SshMultiplexHandle>,
        kind: RemoteFileBackendKind,
    ) -> Self {
        Self {
            config,
            multiplex,
            kind,
        }
    }

    pub(super) fn probe(&self) -> anyhow::Result<()> {
        let command = match self.kind {
            RemoteFileBackendKind::ScpEnhanced => concat!(
                "command -v sh && command -v find && command -v stat && ",
                "command -v tar && command -v cat && command -v mkdir && ",
                "command -v rm && command -v mv"
            ),
            RemoteFileBackendKind::ScpNormal => {
                "command -v ls && command -v cat && command -v mkdir && command -v rm && command -v mv"
            }
            RemoteFileBackendKind::Sftp => anyhow::bail!("invalid shell backend probe"),
        };
        self.exec_ok(command, None)?;
        if self.kind == RemoteFileBackendKind::ScpEnhanced {
            let find = self.exec_ok("LC_ALL=C find . -maxdepth 0 -printf 'x\\0y'", None)?;
            if !find.starts_with(b"x\0y") {
                anyhow::bail!("enhanced SCP requires GNU find -printf with NUL output");
            }
            let stat = self.exec_ok("LC_ALL=C stat -c 'x\\0y' .", None)?;
            if !stat.starts_with(b"x\0y") {
                anyhow::bail!("enhanced SCP requires GNU stat -c with NUL output");
            }
        }
        Ok(())
    }

    pub(super) fn kind(&self) -> RemoteFileBackendKind {
        self.kind
    }

    pub(super) fn exec_ok(
        &self,
        command: impl Into<String>,
        stdin: Option<Vec<u8>>,
    ) -> anyhow::Result<Vec<u8>> {
        let output = self.exec(command, stdin)?;
        if output.exit_status != Some(0) {
            let reason = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!(
                "remote file operation failed with status {}: {}",
                output.exit_status.unwrap_or(255),
                reason.trim()
            );
        }
        Ok(output.stdout)
    }

    pub(super) fn exec(
        &self,
        command: impl Into<String>,
        stdin: Option<Vec<u8>>,
    ) -> anyhow::Result<ShellOutput> {
        let command = command.into().into_bytes();
        let multiplex = self.multiplex.clone();
        let config = self.config.clone();
        let operation = async move {
            tokio::time::timeout(REMOTE_FILE_COMMAND_TIMEOUT, async move {
                if let Some(multiplex) = multiplex {
                    let handle = multiplex.target_handle();
                    let mut channel = {
                        let handle = handle.lock().await;
                        handle.channel_open_session().await?
                    };
                    channel.exec(true, command).await?;
                    send_stdin_and_collect(&mut channel, stdin.as_deref()).await
                } else {
                    let (handle, jump_handles) = open_authenticated_ssh_handle(&config).await?;
                    let mut channel = handle.channel_open_session().await?;
                    channel.exec(true, command).await?;
                    let output = send_stdin_and_collect(&mut channel, stdin.as_deref()).await;
                    let _ = handle
                        .disconnect(
                            Disconnect::ByApplication,
                            "remote file operation completed",
                            "en",
                        )
                        .await;
                    for jump_handle in jump_handles {
                        let _ = jump_handle
                            .disconnect(
                                Disconnect::ByApplication,
                                "remote file operation completed",
                                "en",
                            )
                            .await;
                    }
                    output
                }
            })
            .await
            .map_err(|_| anyhow::anyhow!("remote file operation timed out"))?
        };
        if let Some(multiplex) = self.multiplex.as_ref() {
            multiplex.block_on_after_interactive_ready(operation)
        } else {
            crate::run_ssh_exec_operation(operation)
        }
    }
}

async fn send_stdin_and_collect(
    channel: &mut russh::Channel<russh::client::Msg>,
    stdin: Option<&[u8]>,
) -> anyhow::Result<ShellOutput> {
    if let Some(stdin) = stdin {
        channel.data(stdin).await?;
    }
    channel.eof().await?;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut exit_status = None;
    let mut eof_received = false;
    loop {
        let message = channel.wait().await;
        let mut output = ShellOutput {
            stdout,
            stderr,
            exit_status,
        };
        let keep_collecting = apply_channel_message(&mut output, &mut eof_received, message);
        stdout = output.stdout;
        stderr = output.stderr;
        exit_status = output.exit_status;
        if !keep_collecting {
            break;
        }
    }
    let _ = channel.close().await;
    Ok(ShellOutput {
        stdout,
        stderr,
        exit_status,
    })
}

fn apply_channel_message(
    output: &mut ShellOutput,
    eof_received: &mut bool,
    message: Option<ChannelMsg>,
) -> bool {
    match message {
        Some(ChannelMsg::Data { data }) => output.stdout.extend_from_slice(&data),
        Some(ChannelMsg::ExtendedData { data, ext: 1 }) => output.stderr.extend_from_slice(&data),
        Some(ChannelMsg::ExitStatus { exit_status }) => {
            output.exit_status = Some(exit_status);
            if *eof_received {
                return false;
            }
        }
        // RFC 4254 permits exit-status after EOF. Some servers use exactly
        // that order, so EOF only closes the data stream, not the channel.
        Some(ChannelMsg::Eof) => {
            *eof_received = true;
            if output.exit_status.is_some() {
                return false;
            }
        }
        Some(ChannelMsg::Close) | None => return false,
        Some(_) => {}
    }
    true
}

pub(super) fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        "''".to_string()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::{ShellOutput, apply_channel_message, shell_quote};
    use russh::ChannelMsg;

    #[test]
    fn quotes_posix_shell_paths() {
        assert_eq!(shell_quote(""), "''");
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote("a'b"), "'a'\\''b'");
    }

    #[test]
    fn keeps_collecting_exit_status_after_eof() {
        let mut output = ShellOutput::default();
        let mut eof_received = false;

        assert!(apply_channel_message(
            &mut output,
            &mut eof_received,
            Some(ChannelMsg::Eof)
        ));
        assert!(!apply_channel_message(
            &mut output,
            &mut eof_received,
            Some(ChannelMsg::ExitStatus { exit_status: 0 })
        ));
        assert_eq!(output.exit_status, Some(0));
    }
}
