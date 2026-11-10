//! Protocol writers, readers, and terminal transport adapters.
pub(crate) mod telnet_naws;

use crate::local_shell::LocalShellPathStyle;
use crate::session::{ManagedSession, local_pty_size};
use crate::session_event_queue::SessionEventQueue;
use crate::ssh_shell_integration::SshIntegrationOutput;
use crate::ssh_worker::{SshCommand, TmuxCommandSender, send_session_error};
use crate::telnet_codec::{
    DO, DONT, OPT_NAWS, TelnetCommandDecoder, negotiate_response, unescape_iac_iac,
};
use crate::telnet_input::{TelnetInputState, telnet_auto_login_line_bytes};
use crate::telnet_prompts::{
    compile_optional_regex, default_failure_regex, default_password_regex, default_success_regex,
    default_username_regex, default_wake_regex, last_chars, last_login_regex, last_non_empty_line,
    prompt_candidates, strip_telnet_auto_login_control_sequences,
};
use crate::transport_io::telnet_naws::TelnetNawsWriter;
use crate::writer_mailbox::WriterMailbox;
use crate::{
    CleanupOperation, SerialSessionConfig, SessionEvent, TelnetSessionConfig, TerminalTransport,
    connection_attempt, local_shell_integration, telnet_codec,
};
use portable_pty::{Child, MasterPty};
use regex::Regex;
use serialport::{DataBits, FlowControl, Parity, SerialPort, StopBits};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::{SessionEventConsumerId, SessionInfo, ssh_mailbox};

pub struct LocalPtyTransport {
    pub(crate) info: SessionInfo,
    pub(crate) cwd_path_style: LocalShellPathStyle,
    pub(crate) wsl_distro: Option<String>,
    pub(crate) startup_files: Option<tempfile::TempDir>,
    pub(crate) master: Option<Box<dyn MasterPty + Send>>,
    pub(crate) writer: QueuedTransportWriter,
    pub(crate) child: Box<dyn Child + Send + Sync>,
    pub(crate) reader_thread: Option<JoinHandle<()>>,
    pub(crate) event_queue: SessionEventQueue,
    pub(crate) cleanup_consumer: Option<SessionEventConsumerId>,
}

pub struct TelnetTransport {
    pub(crate) route: Option<Arc<crate::network_route::NetworkRoute>>,
    pub(crate) info: SessionInfo,
    pub(crate) writer: QueuedTransportWriter,
    pub(crate) reader_stream: TcpStream,
    pub(crate) config: TelnetSessionConfig,
    pub(crate) backspace_as_bs: bool,
    pub(crate) input: TelnetInputState,
    pub(crate) auto_login: Option<Arc<Mutex<TelnetAutoLoginState>>>,
    pub(crate) naws: TelnetNawsWriter,
    pub(crate) event_queue: SessionEventQueue,
    pub(crate) reader_thread: Option<JoinHandle<()>>,
}

pub struct SshChannelTransport {
    pub(crate) info: SessionInfo,
    pub(crate) shell_enabled: bool,
    pub(crate) attempt: connection_attempt::ConnectionAttempt,
    pub(crate) command_tx: ssh_mailbox::Sender,
    /// Ordered tmux control-channel commands; buffered until the channel
    /// enters control mode so early UI calls cannot reach the user's shell.
    pub(crate) tmux_tx: TmuxCommandSender,
    /// Set only while the worker runs the tmux control session; gates
    /// `tmux_send_command`/`tmux_detach` so commands cannot be written into
    /// a plain shell.
    pub(crate) tmux_active: Arc<AtomicBool>,
    pub(crate) backspace_as_bs: bool,
    pub(crate) worker_thread: Option<JoinHandle<()>>,
}

/// A tmux pane projected as an independent session. All operations are
/// forwarded onto the parent SSH channel's ordered tmux command channel.
pub struct TmuxPaneTransport {
    pub(crate) close_requested: bool,
    pub(crate) info: SessionInfo,
    /// tmux pane id this session projects (e.g. `%12`).
    pub(crate) pane: String,
    pub(crate) tmux_tx: TmuxCommandSender,
}

pub struct SerialTransport {
    pub(crate) info: SessionInfo,
    pub(crate) writer: QueuedTransportWriter,
    pub(crate) backspace_as_bs: bool,
    pub(crate) stop_reader: Arc<AtomicBool>,
    pub(crate) reader_thread: Option<JoinHandle<()>>,
    pub(crate) flow_control: nyaterm_core::models::connection::SerialFlowControl,
}

pub(crate) struct QueuedTransportWriter {
    pub(crate) command_tx: WriterMailbox,
    pub(crate) worker_thread: Option<JoinHandle<()>>,
    pub(crate) telnet_lifecycle: Option<Arc<TelnetWriterLifecycle>>,
    pub(crate) cancellation: Option<portable_pty::WriteCancellation>,
}

// Shared by a TCP session's reader and writer. Closing the socket also
// interrupts a blocked write before either thread is joined.
pub(crate) struct TelnetWriterLifecycle {
    pub(crate) stopped: AtomicBool,
    pub(crate) stream: TcpStream,
}

pub(crate) struct TelnetWriterPolicy {
    pub(crate) idle_interval: Option<Duration>,
    pub(crate) lifecycle: Arc<TelnetWriterLifecycle>,
}

#[derive(Clone, Copy)]
pub(crate) enum WriterFlushPolicy {
    Never,
    AfterWrite,
    EachByte,
}

pub(crate) enum WriteCapacity {
    Native(WriterMailbox),
    Ssh(ssh_mailbox::Sender),
}

pub(crate) enum TransportWriterCommand {
    Write(Vec<u8>),
    // Already encoded wire bytes: never apply application escaping or EachByte.
    TelnetControl(Vec<u8>),
}

pub(crate) const TELNET_AUTO_LOGIN_TAIL_CHARS: usize = 2048;

pub(crate) const TELNET_AUTO_LOGIN_PROMPT_WINDOW_CHARS: usize = 320;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TelnetAutoLoginAction {
    Send(Vec<u8>),
    Complete,
    Disable,
}

pub(crate) struct TelnetAutoLoginState {
    pub(crate) username: String,
    pub(crate) password: Option<nyaterm_core::SecretString>,
    pub(crate) started_at: Instant,
    pub(crate) tail: String,
    pub(crate) sent_wake: bool,
    pub(crate) sent_username: bool,
    pub(crate) sent_password: bool,
    pub(crate) disabled: bool,
    pub(crate) completed: bool,
    pub(crate) retries: u8,
    pub(crate) username_regex: Option<Regex>,
    pub(crate) password_regex: Option<Regex>,
    pub(crate) success_regex: Option<Regex>,
    pub(crate) failure_regex: Option<Regex>,
}

impl TelnetWriterLifecycle {
    pub(crate) fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
        let _ = self.stream.shutdown(Shutdown::Both);
    }

    pub(crate) fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::Acquire)
    }
}

impl WriterFlushPolicy {
    pub(crate) fn serial() -> Self {
        if cfg!(windows) {
            Self::Never
        } else {
            Self::AfterWrite
        }
    }
}

impl WriteCapacity {
    pub(crate) fn wait(&self, bytes: usize, cancelled: &AtomicBool) -> std::io::Result<()> {
        match self {
            Self::Native(mailbox) => mailbox.wait_capacity(bytes, cancelled),
            Self::Ssh(mailbox) => mailbox.wait_capacity(bytes, cancelled),
        }
    }
}

impl QueuedTransportWriter {
    pub(crate) fn spawn<W>(
        session_id: String,
        writer: W,
        flush_policy: WriterFlushPolicy,
        event_queue: SessionEventQueue,
    ) -> std::io::Result<Self>
    where
        W: Write + Send + 'static,
    {
        Self::spawn_with_telnet_policy(session_id, writer, flush_policy, event_queue, None)
    }

    pub(crate) fn spawn_with_telnet_policy<W>(
        session_id: String,
        writer: W,
        flush_policy: WriterFlushPolicy,
        event_queue: SessionEventQueue,
        telnet_policy: Option<TelnetWriterPolicy>,
    ) -> std::io::Result<Self>
    where
        W: Write + Send + 'static,
    {
        let command_tx = WriterMailbox::default();
        let command_rx = command_tx.clone();
        let telnet_lifecycle = telnet_policy
            .as_ref()
            .map(|policy| policy.lifecycle.clone());
        let worker_thread = nyaterm_core::runtime::worker::spawn(
            format!("transport-writer-{session_id}"),
            move || {
                run_transport_writer(
                    session_id,
                    writer,
                    flush_policy,
                    command_rx,
                    event_queue,
                    telnet_policy,
                )
            },
        )?;
        Ok(Self {
            command_tx,
            worker_thread: Some(worker_thread),
            telnet_lifecycle,
            cancellation: None,
        })
    }

    pub(crate) fn write(&self, data: Vec<u8>) -> anyhow::Result<()> {
        if data.is_empty() {
            return Ok(());
        }
        self.command_tx
            .send(TransportWriterCommand::Write(data))
            .map_err(anyhow::Error::from)
    }

    pub(crate) fn write_slice(&self, data: &[u8]) -> anyhow::Result<()> {
        if data.is_empty() {
            return Ok(());
        }
        self.command_tx.check_capacity(data.len())?;
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(data.len())?;
        bytes.extend_from_slice(data);
        self.write(bytes)
    }

    pub(crate) fn stop(&self) {
        self.command_tx.stop();
        if let Some(cancellation) = &self.cancellation {
            cancellation.cancel();
        }
    }

    pub(crate) fn close(&mut self) -> bool {
        self.stop();
        if let Some(lifecycle) = &self.telnet_lifecycle {
            lifecycle.stop();
        }

        if let Some(worker_thread) = self.worker_thread.take() {
            while !worker_thread.is_finished() && self.cancellation.is_some() {
                self.stop();
                std::thread::sleep(Duration::from_millis(5));
            }
            return worker_thread.join().is_ok();
        }
        true
    }
}

pub(crate) fn telnet_wire_size(data: &[u8], raw_tcp: bool) -> usize {
    data.len().saturating_add(if raw_tcp {
        0
    } else {
        data.iter().filter(|byte| **byte == 255).count()
    })
}

pub(crate) fn queue_telnet_control(
    command_tx: &WriterMailbox,
    data: Vec<u8>,
) -> anyhow::Result<()> {
    if data.is_empty() {
        return Ok(());
    }
    command_tx
        .send(TransportWriterCommand::TelnetControl(data))
        .map_err(anyhow::Error::from)
}

pub(crate) fn run_transport_writer<W>(
    session_id: String,
    mut writer: W,
    flush_policy: WriterFlushPolicy,
    command_rx: WriterMailbox,
    event_queue: SessionEventQueue,
    telnet_policy: Option<TelnetWriterPolicy>,
) where
    W: Write,
{
    let stopped = || {
        command_rx.is_stopped()
            || telnet_policy
                .as_ref()
                .is_some_and(|policy| policy.lifecycle.is_stopped())
    };
    let idle_interval = telnet_policy
        .as_ref()
        .and_then(|policy| policy.idle_interval);
    let mut last_sent = Instant::now();
    while !stopped() {
        let (command, _permit) = match command_rx
            .receive(idle_interval.map(|interval| interval.saturating_sub(last_sent.elapsed())))
        {
            Ok(Some(command)) => command,
            Ok(None) => {
                match command_rx.send(TransportWriterCommand::TelnetControl(
                    telnet_codec::NOP.to_vec(),
                )) {
                    Ok(()) => {}
                    // Input can race the idle deadline. Consume that accepted
                    // input before considering another keepalive.
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(error) => {
                        if !stopped() {
                            send_session_error(&event_queue, &session_id, error);
                        }
                        break;
                    }
                }
                continue;
            }
            Err(_) => break,
        };
        if stopped() {
            break;
        }
        let (data, policy) = match command {
            TransportWriterCommand::Write(data) => (data, flush_policy),
            TransportWriterCommand::TelnetControl(data) => (data, WriterFlushPolicy::AfterWrite),
        };
        if data.is_empty() {
            continue;
        }
        let write_result = if matches!(policy, WriterFlushPolicy::EachByte) {
            data.iter().try_for_each(|byte| {
                if stopped() {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::BrokenPipe,
                        "writer stopped",
                    ));
                }
                writer
                    .write_all(std::slice::from_ref(byte))
                    .and_then(|_| writer.flush())
            })
        } else {
            writer.write_all(&data).and_then(|_| {
                if matches!(policy, WriterFlushPolicy::Never) {
                    Ok(())
                } else {
                    writer.flush()
                }
            })
        };
        if let Err(error) = write_result {
            if !stopped() {
                send_session_error(&event_queue, &session_id, error);
            }
            break;
        }
        // Count completed sends, including negotiation and NOP; never catch up
        // missed deadlines after a slow write.
        last_sent = Instant::now();
    }
    command_rx.stop();
    if let Some(policy) = telnet_policy {
        policy.lifecycle.stop();
    }
}

impl LocalPtyTransport {
    pub(crate) fn report_cleanup_failure(&self, operation: CleanupOperation) {
        self.event_queue
            .push_cleanup_failure(SessionEvent::CleanupFailed {
                session_id: self.info.id.clone(),
                consumer_id: self.cleanup_consumer,
                operation,
            });
    }
}

impl TerminalTransport for LocalPtyTransport {
    fn write(&mut self, data: &[u8]) -> anyhow::Result<()> {
        self.writer.write_slice(data)
    }

    fn resize(
        &mut self,
        cols: u16,
        rows: u16,
        pixel_width: u16,
        pixel_height: u16,
    ) -> anyhow::Result<()> {
        self.master
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("local PTY is closed"))?
            .resize(local_pty_size(cols, rows, pixel_width, pixel_height))?;
        self.info.cols = cols;
        self.info.rows = rows;
        Ok(())
    }

    fn close(&mut self) -> anyhow::Result<()> {
        self.writer.stop();
        let kill_result = self.child.kill();
        if kill_result.is_err() {
            self.report_cleanup_failure(CleanupOperation::Terminate);
        }
        if !self.writer.close() {
            self.report_cleanup_failure(CleanupOperation::WriterJoin);
        }
        drop(self.master.take());
        if kill_result.is_ok() && self.child.wait().is_err() {
            self.report_cleanup_failure(CleanupOperation::Wait);
        }
        if let Some(reader) = self.reader_thread.take()
            && reader.join().is_err()
        {
            self.report_cleanup_failure(CleanupOperation::ReaderJoin);
        }
        // ConPTY teardown and joins must finish before deleting files that a
        // shell may still have open (notably when closing during rc startup).
        let startup_cleanup = self
            .startup_files
            .take()
            .map(tempfile::TempDir::close)
            .transpose();
        if startup_cleanup.is_err() {
            self.report_cleanup_failure(CleanupOperation::StartupFiles);
        }
        kill_result?;
        startup_cleanup
            .map(|_| ())
            .map_err(|_| anyhow::anyhow!("could not remove local shell startup files"))
    }
}

impl TerminalTransport for TelnetTransport {
    fn write(&mut self, data: &[u8]) -> anyhow::Result<()> {
        let user_input = !data.is_empty();
        self.writer.command_tx.check_capacity(data.len())?;
        let data = if self.backspace_as_bs {
            remap_del_to_bs(data)
        } else {
            data.to_vec()
        };
        // Commit editing and escape-filter state only after the complete wire
        // payload is accepted, so backpressure leaves the input retryable.
        let mut next_input = self.input.clone();
        let (data, visible_echo) = next_input.prepare(&data, &self.config);
        self.writer.write(data)?;
        self.input = next_input;
        if user_input
            && let Some(auto_login) = self.auto_login.as_ref()
            && let Ok(mut auto_login) = auto_login.lock()
        {
            let _ = auto_login.handle_user_input(false);
        }
        if !visible_echo.is_empty() {
            self.event_queue.push(SessionEvent::Output {
                session_id: self.info.id.clone(),
                data: visible_echo,
            });
        }
        Ok(())
    }

    fn write_raw(&mut self, data: &[u8]) -> anyhow::Result<()> {
        let bytes = telnet_wire_size(data, self.config.raw_tcp);
        self.writer.command_tx.check_capacity(bytes)?;
        self.writer
            .write(telnet_codec::escape_telnet_application_data(
                data,
                self.config.raw_tcp,
            ))
    }

    fn resize(
        &mut self,
        cols: u16,
        rows: u16,
        _pixel_width: u16,
        _pixel_height: u16,
    ) -> anyhow::Result<()> {
        self.naws.resize(cols, rows)?;
        self.info.cols = cols;
        self.info.rows = rows;
        Ok(())
    }

    fn close(&mut self) -> anyhow::Result<()> {
        let _ = self.reader_stream.shutdown(Shutdown::Both);
        if let Some(route) = self.route.take() {
            route.close();
        }
        self.writer.close();
        if let Some(reader_thread) = self.reader_thread.take() {
            let _ = reader_thread.join();
        }
        Ok(())
    }
}

impl TerminalTransport for SshChannelTransport {
    fn write(&mut self, data: &[u8]) -> anyhow::Result<()> {
        if !self.shell_enabled {
            return Err(connection_attempt::ShellUnavailable.into());
        }
        self.attempt.ensure_shell_available()?;
        let data = if self.backspace_as_bs {
            remap_del_to_bs(data)
        } else {
            data.to_vec()
        };
        self.command_tx.write(data)?;
        Ok(())
    }

    fn write_raw(&mut self, data: &[u8]) -> anyhow::Result<()> {
        if self.tmux_active.load(Ordering::Acquire) {
            return Err(tmux_modem_unsupported().into());
        }
        if !self.shell_enabled {
            return Err(connection_attempt::ShellUnavailable.into());
        }
        self.attempt.ensure_shell_available()?;
        self.command_tx
            .write(data.to_vec())
            .map_err(anyhow::Error::from)
    }

    fn resize(
        &mut self,
        cols: u16,
        rows: u16,
        pixel_width: u16,
        pixel_height: u16,
    ) -> anyhow::Result<()> {
        self.info.cols = cols;
        self.info.rows = rows;
        self.command_tx
            .resize(cols, rows, pixel_width, pixel_height);
        Ok(())
    }

    fn close(&mut self) -> anyhow::Result<()> {
        self.command_tx.close();
        if let Some(worker_thread) = self.worker_thread.take() {
            let _ = worker_thread.join();
        }
        Ok(())
    }
}

/// A host worker may be parked publishing a virtual pane's output. Cancel its
/// projections before joining it; cancelling only the host's queue cannot wake
/// that producer. Identify ownership by the shared mailbox, not an id prefix.
pub(crate) fn cancel_tmux_pane_output(
    queue: &SessionEventQueue,
    sessions: &HashMap<String, ManagedSession>,
    sender: &TmuxCommandSender,
) {
    for (id, session) in sessions {
        if let ManagedSession::TmuxPane(pane) = session
            && pane.tmux_tx.same_channel(sender)
        {
            queue.cancel_session(id);
        }
    }
}

pub(crate) fn tmux_modem_unsupported() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "file-transfer protocols are not supported in tmux control mode",
    )
}

impl TerminalTransport for TmuxPaneTransport {
    fn write(&mut self, data: &[u8]) -> anyhow::Result<()> {
        if data.is_empty() {
            return Ok(());
        }
        self.tmux_tx.write(&self.pane, data).map_err(Into::into)
    }

    /// Protocol bytes stay literal; the control session classifies them for
    /// `send-keys` so remote panes receive the right key encoding. Modem
    /// transfers stay gated by `supports_modem_transfer` and the cancellable
    /// transfer writer, not by this channel.
    fn write_raw(&mut self, data: &[u8]) -> anyhow::Result<()> {
        self.write(data)
    }

    fn resize(
        &mut self,
        cols: u16,
        rows: u16,
        _pixel_width: u16,
        _pixel_height: u16,
    ) -> anyhow::Result<()> {
        self.info.cols = cols;
        self.info.rows = rows;
        self.tmux_tx
            .send(SshCommand::TmuxPaneResize {
                pane: self.pane.clone(),
                cols,
                rows,
            })
            .map_err(|_| anyhow::anyhow!("tmux control channel closed for pane {}", self.pane))
    }

    /// A virtual transport owns only a local projection. Remote deletion must
    /// go through SessionManager::tmux_close_pane after an explicit user action.
    fn close(&mut self) -> anyhow::Result<()> {
        Ok(())
    }
}

impl TerminalTransport for SerialTransport {
    fn write(&mut self, data: &[u8]) -> anyhow::Result<()> {
        self.writer.command_tx.check_capacity(data.len())?;
        let data = if self.backspace_as_bs {
            remap_del_to_bs(data)
        } else {
            data.to_vec()
        };
        self.writer.write(data)
    }

    fn write_raw(&mut self, data: &[u8]) -> anyhow::Result<()> {
        self.writer.write_slice(data)
    }

    fn resize(
        &mut self,
        cols: u16,
        rows: u16,
        _pixel_width: u16,
        _pixel_height: u16,
    ) -> anyhow::Result<()> {
        self.info.cols = cols;
        self.info.rows = rows;
        Ok(())
    }

    fn close(&mut self) -> anyhow::Result<()> {
        self.stop_reader.store(true, Ordering::Relaxed);
        self.writer.close();
        if let Some(reader_thread) = self.reader_thread.take() {
            let _ = reader_thread.join();
        }
        Ok(())
    }
}

pub(crate) fn spawn_reader_thread(
    session_id: String,
    mut reader: Box<dyn Read + Send>,
    event_queue: SessionEventQueue,
    mut integration: Option<local_shell_integration::LocalShellIntegration>,
) -> std::io::Result<JoinHandle<()>> {
    nyaterm_core::runtime::worker::spawn(format!("transport-reader-{session_id}"), move || {
        let mut buffer = [0_u8; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => {
                    event_queue.push(SessionEvent::Exited {
                        session_id: session_id.clone(),
                        reason: "reader reached EOF".to_string(),
                    });
                    break;
                }
                Ok(read) => {
                    let output = match &mut integration {
                        Some(integration) => integration.filter(&buffer[..read]),
                        None => SshIntegrationOutput {
                            visible: buffer[..read].to_vec(),
                            ..SshIntegrationOutput::default()
                        },
                    };
                    if let Some(integration) = &integration {
                        for identity in output.shell_identities {
                            event_queue.push(SessionEvent::CommandContext {
                                session_id: session_id.clone(),
                                context: integration.completion_context(identity),
                            });
                        }
                    }
                    for cwd in output.cwd_paths {
                        event_queue.push(SessionEvent::CwdChanged {
                            session_id: session_id.clone(),
                            cwd,
                        });
                    }
                    for command in output.accepted_commands {
                        event_queue.push(SessionEvent::CommandAccepted {
                            session_id: session_id.clone(),
                            command,
                        });
                    }
                    if !output.visible.is_empty() {
                        event_queue.push(SessionEvent::Output {
                            session_id: session_id.clone(),
                            data: output.visible,
                        });
                    }
                }
                Err(error) => {
                    event_queue.push(SessionEvent::Error {
                        session_id: session_id.clone(),
                        message: error.to_string(),
                    });
                    break;
                }
            }
        }
    })
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_tcp_reader_thread(
    session_id: String,
    mut reader: TcpStream,
    command_tx: WriterMailbox,
    lifecycle: Arc<TelnetWriterLifecycle>,
    config: TelnetSessionConfig,
    naws: TelnetNawsWriter,
    auto_login: Option<Arc<Mutex<TelnetAutoLoginState>>>,
    event_queue: SessionEventQueue,
) -> std::io::Result<JoinHandle<()>> {
    nyaterm_core::runtime::worker::spawn(format!("transport-reader-{session_id}"), move || {
        let mut buffer = [0_u8; 8192];
        let mut decoder = TelnetCommandDecoder::default();
        while !lifecycle.is_stopped() {
            match reader.read(&mut buffer) {
                Ok(0) => {
                    if lifecycle.is_stopped() {
                        break;
                    }
                    event_queue.push(SessionEvent::Exited {
                        session_id: session_id.clone(),
                        reason: if config.raw_tcp {
                            "raw TCP peer closed connection".to_string()
                        } else {
                            "telnet peer closed connection".to_string()
                        },
                    });
                    break;
                }
                Ok(read) => {
                    let visible = if config.raw_tcp {
                        unescape_iac_iac(&buffer[..read])
                    } else {
                        decoder.decode(&buffer[..read], &mut |command, option| {
                            if lifecycle.is_stopped() {
                                return;
                            }
                            let queued = match (command, option) {
                                (DO | DONT, OPT_NAWS) => {
                                    naws.negotiate(command).map_err(anyhow::Error::from)
                                }
                                _ => queue_telnet_control(
                                    &command_tx,
                                    negotiate_response(command, option, config.send_sga),
                                ),
                            };
                            if let Err(error) = queued {
                                send_session_error(&event_queue, &session_id, error);
                                lifecycle.stop();
                            }
                        })
                    };
                    if !visible.is_empty() {
                        if let Some(auto_login) = auto_login.as_ref()
                            && let Ok(mut auto_login) = auto_login.lock()
                        {
                            for action in auto_login.handle_visible_output(&visible, &config) {
                                match action {
                                    TelnetAutoLoginAction::Send(payload) => {
                                        if let Err(error) =
                                            queue_telnet_control(&command_tx, payload)
                                        {
                                            send_session_error(&event_queue, &session_id, error);
                                            lifecycle.stop();
                                        }
                                    }
                                    TelnetAutoLoginAction::Complete
                                    | TelnetAutoLoginAction::Disable => {}
                                }
                            }
                        }
                        event_queue.push(SessionEvent::Output {
                            session_id: session_id.clone(),
                            data: visible,
                        });
                    }
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    continue;
                }
                Err(error) => {
                    if lifecycle.is_stopped() {
                        break;
                    }
                    event_queue.push(SessionEvent::Error {
                        session_id: session_id.clone(),
                        message: error.to_string(),
                    });
                    break;
                }
            }
        }
        lifecycle.stop();
        command_tx.stop();
    })
}

impl TelnetAutoLoginState {
    pub(crate) fn new(config: &TelnetSessionConfig) -> Option<Self> {
        if !config.auto_login.enabled {
            return None;
        }
        let username = config.username.trim().to_string();
        let password = config.password.clone().filter(|value| !value.is_empty());
        if username.is_empty() && password.is_none() {
            return None;
        }
        Some(Self {
            username,
            password,
            started_at: Instant::now(),
            tail: String::new(),
            sent_wake: false,
            sent_username: false,
            sent_password: false,
            disabled: false,
            completed: false,
            retries: 0,
            username_regex: compile_optional_regex(
                config.auto_login.username_prompt_regex.as_deref(),
            ),
            password_regex: compile_optional_regex(
                config.auto_login.password_prompt_regex.as_deref(),
            ),
            success_regex: compile_optional_regex(
                config.auto_login.success_prompt_regex.as_deref(),
            ),
            failure_regex: compile_optional_regex(
                config.auto_login.failure_prompt_regex.as_deref(),
            ),
        })
    }

    pub(crate) fn handle_visible_output(
        &mut self,
        visible: &[u8],
        config: &TelnetSessionConfig,
    ) -> Vec<TelnetAutoLoginAction> {
        if self.disabled || self.completed {
            return Vec::new();
        }
        if self.started_at.elapsed() > Duration::from_millis(config.auto_login.timeout_ms) {
            self.disabled = true;
            return vec![TelnetAutoLoginAction::Disable];
        }

        let text = String::from_utf8_lossy(visible);
        self.push_tail(&text);
        let clean = strip_telnet_auto_login_control_sequences(&self.tail);
        let clean_input = strip_telnet_auto_login_control_sequences(&text).replace('\r', "\n");
        let normalized = clean.replace('\r', "\n");
        let window = last_chars(&normalized, TELNET_AUTO_LOGIN_PROMPT_WINDOW_CHARS);
        let last_line = last_non_empty_line(&normalized);
        let prompts = prompt_candidates(&window, &clean_input);

        if self.matches_failure(&window, &last_line) {
            if self.retries < config.auto_login.max_retries {
                self.retries += 1;
                self.sent_username = false;
                self.sent_password = false;
                self.tail.clear();
                return Vec::new();
            }
            self.disabled = true;
            return vec![TelnetAutoLoginAction::Disable];
        }

        let mut actions = Vec::new();
        if config.auto_login.send_wake_enter
            && !self.sent_wake
            && default_wake_regex().is_match(&window)
        {
            self.sent_wake = true;
            actions.push(TelnetAutoLoginAction::Send(telnet_auto_login_line_bytes(
                "", config,
            )));
        }
        if !self.sent_username
            && !self.username.is_empty()
            && self.matches_username_prompt(&prompts, &last_line)
        {
            self.sent_username = true;
            actions.push(TelnetAutoLoginAction::Send(telnet_auto_login_line_bytes(
                &self.username,
                config,
            )));
        }
        if !self.sent_password
            && let Some(password) = self.password.as_deref()
            && self.matches_password_prompt(&prompts)
        {
            self.sent_password = true;
            actions.push(TelnetAutoLoginAction::Send(telnet_auto_login_line_bytes(
                password, config,
            )));
        }
        if (self.sent_username || self.sent_password) && self.matches_success(&last_line) {
            self.completed = true;
            actions.push(TelnetAutoLoginAction::Complete);
        }
        actions
    }

    pub(crate) fn handle_user_input(&mut self, automated: bool) -> Option<TelnetAutoLoginAction> {
        if automated || self.disabled || self.completed {
            return None;
        }
        self.disabled = true;
        Some(TelnetAutoLoginAction::Disable)
    }

    pub(crate) fn push_tail(&mut self, text: &str) {
        self.tail.push_str(text);
        self.tail = last_chars(&self.tail, TELNET_AUTO_LOGIN_TAIL_CHARS);
    }

    pub(crate) fn matches_username_prompt(&self, prompts: &[String], last_line: &str) -> bool {
        if last_login_regex().is_match(last_line) {
            return false;
        }
        prompts.iter().any(|prompt| {
            self.username_regex.as_ref().map_or_else(
                || default_username_regex().is_match(prompt),
                |regex| regex.is_match(prompt),
            )
        })
    }

    pub(crate) fn matches_password_prompt(&self, prompts: &[String]) -> bool {
        prompts.iter().any(|prompt| {
            self.password_regex.as_ref().map_or_else(
                || default_password_regex().is_match(prompt),
                |regex| regex.is_match(prompt),
            )
        })
    }

    pub(crate) fn matches_success(&self, last_line: &str) -> bool {
        self.success_regex.as_ref().map_or_else(
            || default_success_regex().is_match(last_line),
            |regex| regex.is_match(last_line),
        )
    }

    pub(crate) fn matches_failure(&self, text: &str, last_line: &str) -> bool {
        self.failure_regex.as_ref().map_or_else(
            || {
                default_failure_regex().is_match(text)
                    || default_failure_regex().is_match(last_line)
            },
            |regex| regex.is_match(text) || regex.is_match(last_line),
        )
    }
}

pub(crate) fn spawn_serial_reader_thread(
    session_id: String,
    mut reader: Box<dyn SerialPort>,
    stop_reader: Arc<AtomicBool>,
    event_queue: SessionEventQueue,
) -> std::io::Result<JoinHandle<()>> {
    nyaterm_core::runtime::worker::spawn(format!("transport-reader-{session_id}"), move || {
        let mut buffer = [0_u8; 4096];
        while !stop_reader.load(Ordering::Relaxed) {
            match reader.read(&mut buffer) {
                Ok(0) => continue,
                Ok(read) => {
                    event_queue.push(SessionEvent::Output {
                        session_id: session_id.clone(),
                        data: buffer[..read].to_vec(),
                    });
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    continue;
                }
                Err(error) => {
                    event_queue.push(SessionEvent::Error {
                        session_id: session_id.clone(),
                        message: error.to_string(),
                    });
                    break;
                }
            }
        }
    })
}

pub(crate) fn remap_del_to_bs(data: &[u8]) -> Vec<u8> {
    data.iter()
        .map(|byte| if *byte == 0x7f { 0x08 } else { *byte })
        .collect()
}

pub(crate) fn open_serial_port(
    config: &SerialSessionConfig,
) -> serialport::Result<Box<dyn SerialPort>> {
    serialport::new(&config.port_name, config.baud_rate)
        .data_bits(parse_data_bits(config.data_bits))
        .parity(parse_parity(&config.parity))
        .stop_bits(parse_stop_bits(&config.stop_bits))
        .flow_control(match config.flow_control {
            nyaterm_core::models::connection::SerialFlowControl::None => FlowControl::None,
            nyaterm_core::models::connection::SerialFlowControl::Software => FlowControl::Software,
            nyaterm_core::models::connection::SerialFlowControl::Hardware => FlowControl::Hardware,
        })
        .timeout(Duration::from_millis(10))
        .open()
}

pub(crate) fn parse_data_bits(value: u8) -> DataBits {
    match value {
        5 => DataBits::Five,
        6 => DataBits::Six,
        7 => DataBits::Seven,
        _ => DataBits::Eight,
    }
}

pub(crate) fn parse_parity(value: &str) -> Parity {
    match value {
        "odd" => Parity::Odd,
        "even" => Parity::Even,
        _ => Parity::None,
    }
}

pub(crate) fn parse_stop_bits(value: &str) -> StopBits {
    match value {
        "2" => StopBits::Two,
        _ => StopBits::One,
    }
}
