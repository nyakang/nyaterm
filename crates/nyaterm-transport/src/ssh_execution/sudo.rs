//! Lossless, non-PTY sudo channels. Authentication never shares stdin with a protocol.
use crate::ssh_channel::SshChannelConnection;
use crate::{SshMultiplexHandle, SshSessionConfig};
use nyaterm_core::SecretString;
use russh::{Channel, ChannelMsg, client};
use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream, ReadBuf};
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

const AUTH_TIMEOUT: Duration = Duration::from_secs(20);
const STARTUP_LIMIT: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SudoError {
    #[error("sudo requires a password")]
    PasswordRequired,
    #[error("sudo rejected the password")]
    AuthenticationRejected,
    #[error("sudo is unavailable for this SSH session")]
    ExecUnavailable,
    #[error("sudo policy requires a terminal")]
    TerminalRequired,
    #[error("sudo policy denied the command")]
    PolicyDenied,
    #[error("sudo startup timed out")]
    TimedOut,
    #[error("sudo authorization was cancelled or revoked")]
    Cancelled,
    #[error("sudo startup output exceeded its limit")]
    OutputLimit,
    #[error("sudo password contains a line break or NUL")]
    InvalidPassword,
    #[error("sudo command could not start (exit status {0:?})")]
    CommandFailed(Option<u32>),
}

/// Dropping an in-progress open must close its channel, including timeout paths.
struct OpeningChannel(Option<Channel<client::Msg>>);
impl Drop for OpeningChannel {
    fn drop(&mut self) {
        if let Some(channel) = self.0.take() {
            drop(channel.into_stream());
        }
    }
}

/// Owns its stream driver. Cancellation closes only this auxiliary SSH channel.
pub struct SudoExecChannel {
    stream: DuplexStream,
    cancellation: CancellationToken,
    status: Arc<Mutex<Option<SudoError>>>,
    exit_status: Arc<Mutex<Option<u32>>>,
    closed: CancellationToken,
}

impl SudoExecChannel {
    pub fn exit_status(&self) -> Option<u32> {
        *self
            .exit_status
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    /// Completes after this channel's bounded close has run, without closing SSH.
    pub fn completion(&self) -> CancellationToken {
        self.closed.clone()
    }

    pub async fn close(&mut self) {
        self.cancellation.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(3), self.closed.cancelled()).await;
    }

    pub fn status(&self) -> Arc<Mutex<Option<SudoError>>> {
        Arc::clone(&self.status)
    }
}

impl Drop for SudoExecChannel {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

impl AsyncRead for SudoExecChannel {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.cancellation.is_cancelled() {
            return Poll::Ready(Err(io::Error::other(SudoError::Cancelled)));
        }
        Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}
impl AsyncWrite for SudoExecChannel {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        if self.cancellation.is_cancelled() {
            return Poll::Ready(Err(io::Error::other(SudoError::Cancelled)));
        }
        Pin::new(&mut self.stream).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn command(script: &str, prompt: &str, ready: &str) -> String {
    let startup = format!(
        "printf '%s' {} >&2; exec sh -c {}",
        quote(ready),
        quote(script)
    );
    format!(
        "LC_ALL=C sudo -S -p {} -u root -- sh -c {}",
        quote(prompt),
        quote(&startup)
    )
}

fn startup_error(stderr: &[u8], code: Option<u32>) -> SudoError {
    let message = Zeroizing::new(String::from_utf8_lossy(stderr).to_ascii_lowercase());
    if message.contains("must have a tty") || message.contains("a terminal is required") {
        SudoError::TerminalRequired
    } else if message.contains("not in the sudoers") || message.contains("not allowed to execute") {
        SudoError::PolicyDenied
    } else if message.contains("incorrect password attempt") || message.contains("sorry, try again")
    {
        SudoError::AuthenticationRejected
    } else {
        SudoError::CommandFailed(code)
    }
}

pub async fn open_sudo_exec(
    config: &SshSessionConfig,
    multiplex: &SshMultiplexHandle,
    script: &str,
    password: Option<SecretString>,
    cancellation: CancellationToken,
) -> Result<SudoExecChannel, SudoError> {
    config
        .ensure_shell_available()
        .map_err(|_| SudoError::ExecUnavailable)?;
    if multiplex.is_closed()
        || multiplex.shell_availability()
            == crate::connection_attempt::ShellAvailability::Unavailable
    {
        return Err(SudoError::ExecUnavailable);
    }
    if password
        .as_ref()
        .is_some_and(|password| password.expose_secret().contains(['\0', '\r', '\n']))
    {
        return Err(SudoError::InvalidPassword);
    }
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let prompt = format!("nyaterm-sudo-password-{nonce}:");
    let ready = format!("nyaterm-sudo-ready-{nonce}:");
    let open = async {
        let owner = SshChannelConnection::connect(config, Some(multiplex))
            .await
            .map_err(|_| SudoError::ExecUnavailable)?;
        let channel = owner
            .open_session()
            .await
            .map_err(|_| SudoError::ExecUnavailable)?;
        let mut guard = OpeningChannel(Some(channel));
        let channel = guard.0.as_mut().expect("opening channel");
        channel
            .exec(true, command(script, &prompt, &ready))
            .await
            .map_err(|_| SudoError::ExecUnavailable)?;
        let mut stderr = Zeroizing::new(Vec::new());
        let mut stdout = Vec::new();
        let mut sent_password = false;
        let mut observed = 0usize;
        loop {
            match channel.wait().await {
                Some(ChannelMsg::Success) => {}
                Some(ChannelMsg::Failure) => return Err(SudoError::ExecUnavailable),
                Some(ChannelMsg::Data { data }) => {
                    observed = observed.saturating_add(data.len());
                    stdout.extend_from_slice(&data);
                }
                Some(ChannelMsg::ExtendedData { data, ext: 1 }) => {
                    observed = observed.saturating_add(data.len());
                    stderr.extend_from_slice(&data);
                    if observed > STARTUP_LIMIT {
                        return Err(SudoError::OutputLimit);
                    }
                    if stderr
                        .windows(ready.len())
                        .any(|part| part == ready.as_bytes())
                    {
                        return Ok((guard.0.take().expect("authorized channel"), stdout));
                    }
                    if stderr
                        .windows(prompt.len())
                        .any(|part| part == prompt.as_bytes())
                    {
                        if sent_password {
                            return Err(SudoError::AuthenticationRejected);
                        }
                        let password = password
                            .as_ref()
                            .filter(|value| !value.is_empty())
                            .ok_or(SudoError::PasswordRequired)?;
                        let input =
                            Zeroizing::new(format!("{}\n", password.expose_secret()).into_bytes());
                        channel
                            .data(&input[..])
                            .await
                            .map_err(|_| SudoError::CommandFailed(None))?;
                        sent_password = true;
                        stderr.clear();
                    }
                }
                Some(ChannelMsg::ExitStatus { exit_status }) => {
                    return Err(startup_error(&stderr, Some(exit_status)));
                }
                Some(ChannelMsg::Eof | ChannelMsg::Close | ChannelMsg::ExitSignal { .. })
                | None => return Err(startup_error(&stderr, None)),
                _ => {}
            }
            if observed > STARTUP_LIMIT {
                return Err(SudoError::OutputLimit);
            }
        }
    };
    let result = tokio::select! {
        biased;
        _ = cancellation.cancelled() => return Err(SudoError::Cancelled),
        result = config.attempt.until_cancelled(tokio::time::timeout(AUTH_TIMEOUT, open)) => result,
    };
    let (channel, prefix) = result
        .map_err(|_| SudoError::Cancelled)?
        .map_err(|_| SudoError::TimedOut)??;
    drop(password);
    let cancellation = cancellation.child_token();
    let status = Arc::new(Mutex::new(None));
    let exit_status = Arc::new(Mutex::new(None));
    let driver_exit_status = exit_status.clone();
    let closed = CancellationToken::new();
    let completion = closed.clone();
    let (stream, peer) = tokio::io::duplex(256 * 1024);
    let stop = cancellation.clone();
    let driver_status = Arc::clone(&status);
    let attempt = config.attempt.clone();
    tokio::spawn(async move {
        let (mut reader, writer) = channel.split();
        let (mut input, mut output) = tokio::io::split(peer);
        let receive = async {
            output.write_all(&prefix).await?;
            loop {
                match reader.wait().await {
                    Some(ChannelMsg::Data { data }) => output.write_all(&data).await?,
                    Some(ChannelMsg::ExitStatus { exit_status }) => {
                        *driver_exit_status
                            .lock()
                            .unwrap_or_else(|error| error.into_inner()) = Some(exit_status);
                        if exit_status != 0 {
                            *driver_status
                                .lock()
                                .unwrap_or_else(|error| error.into_inner()) =
                                Some(SudoError::CommandFailed(Some(exit_status)));
                        }
                    }
                    Some(ChannelMsg::Eof) => {
                        output.shutdown().await?;
                    }
                    Some(ChannelMsg::Close) | None => {
                        return Ok::<(), io::Error>(());
                    }
                    _ => {}
                }
            }
        };
        let send = async {
            let mut buffer = vec![0u8; 64 * 1024];
            loop {
                let count = input.read(&mut buffer).await?;
                if count == 0 {
                    return Ok::<(), io::Error>(());
                }
                writer
                    .data(&buffer[..count])
                    .await
                    .map_err(io::Error::other)?;
            }
        };
        tokio::select! {
            biased;
            _ = stop.cancelled() => {},
            _ = attempt.until_cancelled(std::future::pending::<()>()) => {},
            _ = receive => {},
            _ = send => {},
        }
        let _ = tokio::time::timeout(Duration::from_secs(2), writer.close()).await;
        completion.cancel();
    });
    Ok(SudoExecChannel {
        stream,
        cancellation,
        status,
        exit_status,
        closed,
    })
}
