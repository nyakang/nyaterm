use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream};
use std::pin::Pin;
use std::process::Stdio;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use regex::Regex;
use russh::keys::PublicKeyBase64;
use russh::{ChannelMsg, Disconnect, client};
#[cfg(test)]
use russh::{cipher, kex, mac};
use serialport::{DataBits, FlowControl, Parity, SerialPort, StopBits};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};
use tokio::sync::mpsc as tokio_mpsc;

mod ascend_npu;
mod environment;
mod gpu;
mod local_fs;
mod rdp;
mod recording;
mod remote_file;
mod remote_process;
mod session_config;
mod session_event_queue;
mod session_types;
mod sftp;
mod ssh_agent;
mod ssh_agent_broker;
mod ssh_algorithms;
mod ssh_auth;
mod telnet_prompts;
#[cfg(test)]
use ssh_algorithms::defaults_from_preferred;
use ssh_algorithms::resolve_preferred_algorithms;
use ssh_auth::{authenticate_ssh, is_agent_retry};
use telnet_prompts::{
    compile_optional_regex, default_failure_regex, default_password_regex, default_success_regex,
    default_username_regex, default_wake_regex, last_chars, last_login_regex, last_non_empty_line,
    prompt_candidates, strip_telnet_auto_login_control_sequences,
};
#[cfg(test)]
use telnet_prompts::{has_password_prompt, has_username_prompt};
mod ssh_shell_integration;
mod tunnel;
mod x11;

use environment::{default_shell, should_use_interactive_login_args};

pub use tunnel::{SshTunnelConfig, SshTunnelInfo, SshTunnelManager, SshTunnelMode};
pub use x11::{
    X11AuthRewriter, X11DisplayTarget, X11ForwardingConfig, effective_x11_display,
    prepare_x11_forwarding, resolve_x11_display_spec, resolve_x11_display_targets,
    rewrite_x11_auth_setup_packet,
};
use x11::{X11ChannelOpen, X11Forwarder, enable_x11_failed_message, spawn_x11_forwarder};
mod sftp_transfer_types;
mod trzsz;
mod zmodem;

pub use environment::{
    EnvironmentValue, ShellEnvironmentCache, ShellEnvironmentError,
    normalize_environment_variable_name,
};
pub use session_config::{
    LocalSessionConfig, SerialSessionConfig, SftpCwdFollowMode, SftpSettings, SshAgentEndpoint,
    SshAgentForwardingConfig, SshAgentForwardingPolicy, SshAgentForwardingSources, SshAgentPrompt,
    SshAgentPromptAction, SshAgentPromptPhase, SshAgentPromptProvider, SshAgentPromptRequest,
    SshAgentStoredKey, SshAgentStoredKeyProvider, SshAgentStoredKeySnapshot, SshAlgorithmMode,
    SshAlgorithmPreferences, SshCredentialPrompt, SshCredentialPromptKind,
    SshCredentialPromptReason, SshCredentialProvider, SshHostKey, SshHostKeyDecision,
    SshHostKeyVerifier, SshKeyAuthConfig, SshKeyboardInteractivePrompt,
    SshKeyboardInteractiveRequest, SshOtpProvider, SshProxyConfig, SshSessionConfig,
    SshSessionProfile, TelnetAutoLoginConfig, TelnetEnterMode, TelnetSessionConfig,
};
use session_event_queue::SessionEventQueue;
#[cfg(test)]
use session_event_queue::{
    SESSION_EVENT_QUEUE_OUTPUT_EVENT_LIMIT, SESSION_EVENT_QUEUE_OUTPUT_LIMIT,
};
pub use session_types::{
    SessionDrain, SessionDrainStats, SessionError, SessionEvent, SessionInfo, SessionKind,
    TerminalTransport,
};
pub use sftp::{
    RemoteBinaryFile, RemoteFilePath, SFTP_TRANSFER_CANCELLED, SftpAttributeUpdate, SftpFileEntry,
    SftpFileProperties, SftpFileType, SftpRemoteTextFile, SftpService, SftpTransferControl,
    SftpWriteTextResult,
};
pub use sftp_transfer_types::{
    SFTP_TRANSFER_DEFAULT_BUFFER_SIZE, SFTP_TRANSFER_DEFAULT_DIRECTORY_UPLOAD_THREADS,
    SFTP_TRANSFER_MAX_BUFFER_SIZE, SFTP_TRANSFER_MAX_DIRECTORY_UPLOAD_THREADS,
    SFTP_TRANSFER_MAX_RETRIES, SFTP_TRANSFER_MIN_BUFFER_SIZE,
    SFTP_TRANSFER_MIN_DIRECTORY_UPLOAD_THREADS, SftpDuplicateDecision, SftpDuplicatePolicy,
    SftpDuplicateRequest, SftpDuplicateResolver, SftpPathTransferOptions, SftpTransferDirection,
    SftpTransferOptions, SftpTransferProgress, SftpTransferSummary,
};
pub use ssh_agent_broker::{
    SshAgentEndpointPreviewError, SshAgentEndpointPreviewErrorCode, SshAgentIdentityPreview,
    SshAgentIdentityPreviewResponse, preview_identities, preview_identities_blocking,
    preview_identities_blocking_with_environment, preview_identities_with_environment,
};
pub use ssh_algorithms::{
    SshAlgorithmDefaults, SshAlgorithmListKind, SshAlgorithmOption, SshAlgorithmRisk,
    SshAlgorithmValidationError, SupportedSshAlgorithms, supported_ssh_algorithms,
    validate_ssh_algorithm_preferences,
};
pub use trzsz::{
    TrzszAction, TrzszConfig, TrzszDetectResult, TrzszDetector, TrzszDownloadEngine,
    TrzszDownloadError, TrzszDownloadEvent, TrzszDownloadStep, TrzszFilteredOutput, TrzszMode,
    TrzszOutputEvent, TrzszOutputScan, TrzszProtocolFilteredOutput, TrzszProtocolFrame,
    TrzszProtocolPayload, TrzszProtocolStream, TrzszTransferEvent, TrzszTransferPhase,
    TrzszTransferState, TrzszTrigger, TrzszUploadEngine, TrzszUploadEntry, TrzszUploadError,
    TrzszUploadEvent, TrzszUploadPayload, TrzszUploadSource, TrzszUploadStep,
    build_trzsz_action_frame, build_trzsz_config_frame, build_trzsz_integer_frame,
    build_trzsz_string_frame, parse_trzsz_action_frame, parse_trzsz_config_frame,
    parse_trzsz_json_frame, parse_trzsz_protocol_frame, trzsz_fail_response,
};
pub use zmodem::{
    ZmodemAction, ZmodemDetectResult, ZmodemDetector, ZmodemDirection, ZmodemEvent, ZmodemTransfer,
    start_zmodem_transfer,
};
mod stats;

mod docker;

pub use ascend_npu::{
    ASCEND_NPU_OVERVIEW_SCRIPT, RemoteNpu, RemoteNpuOverview, RemoteNpuProcess, RemoteNpuService,
    parse_npu_overview_output,
};
pub use docker::{
    DOCKER_COMPOSE_PROJECTS_SCRIPT, DOCKER_IMAGES_SCRIPT, DOCKER_NETWORKS_SCRIPT,
    DOCKER_OVERVIEW_SCRIPT, DOCKER_VOLUMES_SCRIPT, DockerComposeProject, DockerComposeService,
    DockerComposeServiceContainer, DockerContainer, DockerContainerDetails, DockerContainerMount,
    DockerContainerNetwork, DockerContainerStats, DockerImage, DockerNetwork, DockerService,
    DockerVolume, RemoteDockerOverview, docker_container_details_script, parse_compose_projects,
    parse_compose_services_output, parse_docker_container_details_output,
    parse_docker_images_output, parse_docker_networks_output, parse_docker_overview_output,
    parse_docker_stats_output, parse_docker_volumes_output,
};
pub use gpu::{
    GPU_OVERVIEW_SCRIPT, RemoteGpu, RemoteGpuOverview, RemoteGpuProcess, RemoteGpuService,
    parse_gpu_overview_output,
};
pub use local_fs::{LocalDirectoryChild, LocalFileService};
pub use rdp::{
    RdpCapability, RdpCertificatePolicy, RdpCertificateRequest, RdpCertificateResponse,
    RdpClipboardConfig, RdpClipboardMode, RdpDisplayConfig, RdpDisplayMode, RdpError, RdpErrorKind,
    RdpFrameEvent, RdpInputEvent, RdpPointerButton, RdpReconnectConfig, RdpRuntimeEvent,
    RdpSessionConfig, RdpSessionDrain, RdpSessionManager, RdpSessionState, VncClipboardConfig,
    VncDisplayConfig, VncError, VncErrorKind, VncInputEvent, VncReconnectConfig, VncRuntimeEvent,
    VncScaleMode, VncSecurityConfig, VncSecurityMode, VncSessionConfig, VncSessionDrain,
    VncSessionManager, VncSessionState, parse_rdp_certificate_policy, parse_rdp_clipboard_mode,
    parse_rdp_display_mode, parse_vnc_scale_mode, parse_vnc_security_mode, validate_vnc_config,
};
pub use recording::{
    DEFAULT_HISTORY_SEARCH_LIMIT, DEFAULT_HISTORY_SEARCH_LINES, DEFAULT_MEMORY_LIMIT_BYTES,
    ExistingFileBehavior, MAX_HISTORY_SEARCH_LINES, RecordingContext, RecordingError,
    RecordingManager, RecordingMode, RecordingProfile, RecordingRotationPolicy, RecordingStatus,
    RecordingStatusState, TerminalHistorySearchRequest, TerminalHistorySearchResponse,
    TerminalHistorySearchResult, safe_recording_name,
};
pub use remote_file::{
    FileCopyRequest, FileCopySummary, FileTransferEndpoint, RemoteFileBackendKind,
    RemoteFileBackendPreference, RemoteFileBackendPreferenceStore, RemoteFileService,
};
pub use remote_process::{
    PROCESS_LIST_SCRIPT, PROCESS_LIST_UNSUPPORTED_ERROR, PROCESS_LIST_UNSUPPORTED_MARKER,
    RemoteCommandOutput, RemoteProcess, SshProcessService, is_process_list_unsupported,
    normalize_process_signal, parse_process_output, run_local_command,
};
pub(crate) use remote_process::{
    PROCESS_TIMEOUT, ensure_remote_command_success, exec_ssh_command, run_ssh_exec_operation,
};
#[cfg(test)]
use ssh_shell_integration::{
    OscStripper, ShellIntegrationMode, activation_script, bytes_after_ssh_ready_marker,
    persistent_script, rc_managed_block, ssh_shell_injection_script, strip_ssh_ready_markers,
};
use ssh_shell_integration::{
    ShellKind, SshIntegrationOutput, SshShellIntegrationState, build_legacy_ssh_ready_marker,
    build_ssh_ready_marker, build_ssh_shell_integration_script, detect_ssh_shell_type,
};
pub use stats::{
    CpuInfo, DiskInfo, LoadInfo, MemoryInfo, NetworkInfo, RemoteStats, RemoteStatsService,
    SYSINFO_SCRIPT, SystemInfo, parse_stats_output,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshMultiplexInfo {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub proxy: Option<SshProxyConfig>,
    pub jump_count: usize,
}

#[derive(Clone)]
pub struct SshMultiplexHandle {
    inner: Arc<SshMultiplexInner>,
}

type SharedSshHandle = Arc<tokio::sync::Mutex<client::Handle<SshClientHandler>>>;
type ForwardedTcpIpRegistry = Arc<tokio::sync::Mutex<ForwardedTcpIpDispatch>>;

/// Starts an SSH channel after its desktop session has been registered.
///
/// The transport manager must insert the channel before it can return the
/// session id, but the PTY reader must not run before GPUI has created the
/// corresponding terminal frame state. Keeping this sender in the start
/// result gives the desktop layer the same registration-before-IO ordering as
/// the Tauri implementation.
pub struct SshSessionStartHandle {
    start_tx: Option<tokio::sync::oneshot::Sender<()>>,
}

impl SshSessionStartHandle {
    fn new(start_tx: tokio::sync::oneshot::Sender<()>) -> Self {
        Self {
            start_tx: Some(start_tx),
        }
    }

    /// Releases the worker to open/read the interactive PTY channel.
    pub fn start(mut self) {
        if let Some(start_tx) = self.start_tx.take() {
            let _ = start_tx.send(());
        }
    }
}

struct SshMultiplexInner {
    runtime: Arc<tokio::runtime::Runtime>,
    target: SharedSshHandle,
    jumps: Vec<SharedSshHandle>,
    info: SshMultiplexInfo,
    /// The Agent handler is fixed when the transport is created, so multiplex
    /// handles with different forwarding policies must never be shared.
    agent_forwarding_config: Option<SshAgentForwardingConfig>,
    agent_stored_key_revision: Option<u64>,
    shell_environment: Arc<ShellEnvironmentCache>,
    forwarded_tcpip: ForwardedTcpIpRegistry,
    /// Remote operations wait until the owning interactive PTY reader has
    /// started.  Before that point a second channel could race the desktop
    /// registration barrier and make the first login burst invisible.
    interactive_ready: Arc<AtomicBool>,
    interactive_ready_notify: Arc<tokio::sync::Notify>,
    closed: AtomicBool,
}

#[derive(Default)]
struct ForwardedTcpIpDispatch {
    fallback: Option<tokio_mpsc::UnboundedSender<ForwardedTcpIpChannel>>,
    by_listener: HashMap<(String, u32), tokio_mpsc::UnboundedSender<ForwardedTcpIpChannel>>,
}

impl std::fmt::Debug for SshMultiplexHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SshMultiplexHandle")
            .field("info", &self.inner.info)
            .field("closed", &self.is_closed())
            .finish()
    }
}

impl SshMultiplexHandle {
    pub fn info(&self) -> SshMultiplexInfo {
        self.inner.info.clone()
    }

    pub fn jump_count(&self) -> usize {
        self.inner.info.jump_count
    }

    pub fn is_closed(&self) -> bool {
        self.inner.closed.load(Ordering::Relaxed)
    }

    pub fn matches_config(&self, config: &SshSessionConfig) -> bool {
        self.inner.info.host == config.host
            && self.inner.info.port == config.port
            && self.inner.info.username == config.username
            && self.inner.info.proxy == config.proxy
            && self.inner.agent_forwarding_config == effective_agent_forwarding_config(config)
            && self.inner.agent_stored_key_revision == current_agent_stored_key_revision(config)
    }

    pub fn ensure_matches_config(&self, config: &SshSessionConfig) -> anyhow::Result<()> {
        if self.matches_config(config) {
            return Ok(());
        }
        let info = &self.inner.info;
        anyhow::bail!(
            "SSH multiplex handle targets {}@{}:{}, but operation targets {}@{}:{}",
            info.username,
            info.host,
            info.port,
            config.username,
            config.host,
            config.port
        )
    }

    pub fn disconnect(&self) -> anyhow::Result<()> {
        self.inner.runtime.block_on(self.disconnect_async())
    }

    async fn disconnect_async(&self) -> anyhow::Result<()> {
        if self.inner.closed.swap(true, Ordering::Relaxed) {
            return Ok(());
        }
        self.inner.interactive_ready_notify.notify_waiters();
        let target = self.inner.target.clone();
        let jumps = self.inner.jumps.clone();
        let _ = target
            .lock()
            .await
            .disconnect(Disconnect::ByApplication, "ssh multiplex closed", "en")
            .await;
        for jump in jumps {
            let _ = jump
                .lock()
                .await
                .disconnect(Disconnect::ByApplication, "ssh multiplex closed", "en")
                .await;
        }
        Ok(())
    }

    fn target_handle(&self) -> SharedSshHandle {
        self.inner.target.clone()
    }

    fn forwarded_tcpip_registry(&self) -> ForwardedTcpIpRegistry {
        self.inner.forwarded_tcpip.clone()
    }

    fn shell_environment(&self) -> Arc<ShellEnvironmentCache> {
        self.inner.shell_environment.clone()
    }

    fn mark_interactive_ready(&self) {
        self.inner.interactive_ready.store(true, Ordering::Release);
        self.inner.interactive_ready_notify.notify_waiters();
    }

    pub(crate) async fn wait_for_interactive_ready(&self) -> anyhow::Result<()> {
        tokio::time::timeout(INTERACTIVE_READY_TIMEOUT, async {
            loop {
                if self.is_closed() {
                    anyhow::bail!("SSH multiplex handle is closed");
                }
                if self.inner.interactive_ready.load(Ordering::Acquire) {
                    return Ok(());
                }
                let notified = self.inner.interactive_ready_notify.notified();
                if self.inner.interactive_ready.load(Ordering::Acquire) {
                    return Ok(());
                }
                notified.await;
            }
        })
        .await
        .map_err(|_| anyhow::anyhow!("timed out waiting for SSH interactive shell readiness"))?
    }

    fn block_on<T, F>(&self, operation: F) -> anyhow::Result<T>
    where
        F: Future<Output = anyhow::Result<T>> + Send + 'static,
        T: Send + 'static,
    {
        if self.is_closed() {
            anyhow::bail!("SSH multiplex handle is closed");
        }
        self.inner.runtime.block_on(operation)
    }

    pub(crate) fn block_on_after_interactive_ready<T, F>(&self, operation: F) -> anyhow::Result<T>
    where
        F: Future<Output = anyhow::Result<T>> + Send + 'static,
        T: Send + 'static,
    {
        let ready_handle = self.clone();
        self.block_on(async move {
            ready_handle.wait_for_interactive_ready().await?;
            operation.await
        })
    }
}

fn connected_ssh_multiplex_handle(
    config: &SshSessionConfig,
    runtime: Arc<tokio::runtime::Runtime>,
    target: SharedSshHandle,
    jumps: Vec<SharedSshHandle>,
    forwarded_tcpip: ForwardedTcpIpRegistry,
    interactive_ready: bool,
    shell_environment: Arc<ShellEnvironmentCache>,
) -> SshMultiplexHandle {
    let info = SshMultiplexInfo {
        name: config.name.clone(),
        host: config.host.clone(),
        port: config.port,
        username: config.username.clone(),
        proxy: config.proxy.clone(),
        jump_count: jumps.len(),
    };
    SshMultiplexHandle {
        inner: Arc::new(SshMultiplexInner {
            runtime,
            target,
            jumps,
            info,
            agent_forwarding_config: effective_agent_forwarding_config(config),
            agent_stored_key_revision: current_agent_stored_key_revision(config),
            shell_environment,
            forwarded_tcpip,
            interactive_ready: Arc::new(AtomicBool::new(interactive_ready)),
            interactive_ready_notify: Arc::new(tokio::sync::Notify::new()),
            closed: AtomicBool::new(false),
        }),
    }
}

fn forwarded_tcpip_sender_for(
    dispatch: &ForwardedTcpIpDispatch,
    connected_address: &str,
    connected_port: u32,
) -> Option<tokio_mpsc::UnboundedSender<ForwardedTcpIpChannel>> {
    dispatch
        .by_listener
        .get(&(connected_address.to_string(), connected_port))
        .or(dispatch.fallback.as_ref())
        .cloned()
}

pub fn open_ssh_multiplex_handle(config: SshSessionConfig) -> anyhow::Result<SshMultiplexHandle> {
    open_ssh_multiplex_handle_with_environment(config, ShellEnvironmentCache::global())
}

/// Opens an SSH multiplex handle using a shared shell environment cache.
pub fn open_ssh_multiplex_handle_with_environment(
    config: SshSessionConfig,
    shell_environment: Arc<ShellEnvironmentCache>,
) -> anyhow::Result<SshMultiplexHandle> {
    let runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("nyaterm-ssh-multiplex")
            .build()
            .map_err(|error| anyhow::anyhow!("failed to start SSH multiplex runtime: {error}"))?,
    );
    let forwarded_tcpip = Arc::new(tokio::sync::Mutex::new(ForwardedTcpIpDispatch::default()));
    let (target, jumps) = runtime.block_on(open_authenticated_ssh_handle_with_sender_registry(
        &config,
        Some(forwarded_tcpip.clone()),
        None,
        shell_environment.clone(),
    ))?;
    let jumps = jumps
        .into_iter()
        .map(|jump| Arc::new(tokio::sync::Mutex::new(jump)))
        .collect();
    Ok(connected_ssh_multiplex_handle(
        &config,
        runtime,
        Arc::new(tokio::sync::Mutex::new(target)),
        jumps,
        forwarded_tcpip,
        // This handle is created specifically for independent remote
        // operations; no interactive PTY owns a readiness barrier for it.
        true,
        shell_environment,
    ))
}

impl std::fmt::Debug for SshTunnelConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SshTunnelConfig")
            .field("id", &self.id)
            .field("ssh_config", &self.ssh_config)
            .field("mode", &self.mode)
            .field("bind_host", &self.bind_host)
            .field("listen_port", &self.listen_port)
            .field("target_host", &self.target_host)
            .field("target_port", &self.target_port)
            .finish()
    }
}

struct ForwardedTcpIpChannel {
    channel: russh::Channel<client::Msg>,
    connected_address: String,
    connected_port: u32,
    originator_address: String,
    originator_port: u32,
}

const MIT_MAGIC_COOKIE: &str = "MIT-MAGIC-COOKIE-1";
const XAUTH_TIMEOUT: Duration = Duration::from_secs(2);
const INTERACTIVE_READY_TIMEOUT: Duration = Duration::from_secs(30);

pub struct SessionManager {
    sessions: Mutex<HashMap<String, ManagedSession>>,
    event_queue: SessionEventQueue,
    shell_environment: Arc<ShellEnvironmentCache>,
}

enum ManagedSession {
    Local(LocalPtyTransport),
    Ssh(SshChannelTransport),
    Tcp(TelnetTransport),
    Serial(SerialTransport),
}

pub struct LocalPtyTransport {
    info: SessionInfo,
    master: Box<dyn MasterPty + Send>,
    writer: QueuedTransportWriter,
    child: Box<dyn Child + Send + Sync>,
    reader_thread: Option<JoinHandle<()>>,
}

pub struct TelnetTransport {
    info: SessionInfo,
    writer: QueuedTransportWriter,
    reader_stream: TcpStream,
    config: TelnetSessionConfig,
    backspace_as_bs: bool,
    local_line_buffer: Vec<u8>,
    auto_login: Option<Arc<Mutex<TelnetAutoLoginState>>>,
    event_queue: SessionEventQueue,
    reader_thread: Option<JoinHandle<()>>,
}

pub struct SshChannelTransport {
    info: SessionInfo,
    command_tx: tokio_mpsc::UnboundedSender<SshCommand>,
    backspace_as_bs: bool,
    worker_thread: Option<JoinHandle<()>>,
}

pub struct SerialTransport {
    info: SessionInfo,
    writer: QueuedTransportWriter,
    backspace_as_bs: bool,
    stop_reader: Arc<AtomicBool>,
    reader_thread: Option<JoinHandle<()>>,
}

struct QueuedTransportWriter {
    command_tx: mpsc::Sender<TransportWriterCommand>,
    worker_thread: Option<JoinHandle<()>>,
}

enum TransportWriterCommand {
    Write(Vec<u8>),
    Close,
}

impl QueuedTransportWriter {
    fn spawn<W>(
        session_id: String,
        writer: W,
        flush_each_byte: bool,
        event_queue: SessionEventQueue,
    ) -> Self
    where
        W: Write + Send + 'static,
    {
        let (command_tx, command_rx) = mpsc::channel();
        let worker_thread = std::thread::spawn(move || {
            run_transport_writer(session_id, writer, flush_each_byte, command_rx, event_queue)
        });
        Self {
            command_tx,
            worker_thread: Some(worker_thread),
        }
    }

    fn write(&self, data: Vec<u8>) -> anyhow::Result<()> {
        if data.is_empty() {
            return Ok(());
        }
        self.command_tx
            .send(TransportWriterCommand::Write(data))
            .map_err(|_| anyhow::anyhow!("transport writer stopped"))
    }

    fn close(&mut self) {
        let _ = self.command_tx.send(TransportWriterCommand::Close);
        if let Some(worker_thread) = self.worker_thread.take() {
            let _ = worker_thread.join();
        }
    }
}

fn run_transport_writer<W>(
    session_id: String,
    mut writer: W,
    flush_each_byte: bool,
    command_rx: mpsc::Receiver<TransportWriterCommand>,
    event_queue: SessionEventQueue,
) where
    W: Write,
{
    while let Ok(command) = command_rx.recv() {
        match command {
            TransportWriterCommand::Write(data) => {
                let write_result = if flush_each_byte {
                    data.iter().try_for_each(|byte| {
                        writer
                            .write_all(std::slice::from_ref(byte))
                            .and_then(|_| writer.flush())
                    })
                } else {
                    writer.write_all(&data).and_then(|_| writer.flush())
                };
                if let Err(error) = write_result {
                    send_session_error(&event_queue, &session_id, error);
                    break;
                }
            }
            TransportWriterCommand::Close => break,
        }
    }
}

enum SshCommand {
    Write(Vec<u8>),
    Resize {
        cols: u16,
        rows: u16,
        pixel_width: u16,
        pixel_height: u16,
    },
    Close,
}

struct OpenSshShellSession {
    handle: Option<SharedSshHandle>,
    channel: russh::Channel<client::Msg>,
    jump_handles: Vec<SharedSshHandle>,
    disconnect_on_close: bool,
    x11_forwarder: Option<X11Forwarder>,
    local_notice: Option<Vec<u8>>,
    injection_script: Option<Vec<u8>>,
    integration_future: Option<SshIntegrationPreparation>,
    ready_marker: String,
    legacy_ready_marker: Option<String>,
    shell_kind: Option<ShellKind>,
}

type SshIntegrationPreparation = Pin<Box<dyn Future<Output = Option<Vec<u8>>> + Send>>;

#[derive(Clone)]
enum SshShellHandle {
    Dedicated(SharedSshHandle),
    Multiplexed(SharedSshHandle),
}

struct PendingOpenSshShellSession {
    handle: SshShellHandle,
    forwarded_tcpip: ForwardedTcpIpRegistry,
    jump_handles: Vec<SharedSshHandle>,
    disconnect_on_close: bool,
    x11_config: Option<X11ForwardingConfig>,
    x11_rx: Option<tokio_mpsc::UnboundedReceiver<X11ChannelOpen>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SshPtyDimensions {
    cols: u16,
    rows: u16,
    pixel_width: u16,
    pixel_height: u16,
}

fn local_pty_size(cols: u16, rows: u16, pixel_width: u16, pixel_height: u16) -> PtySize {
    PtySize {
        rows,
        cols,
        pixel_width,
        pixel_height,
    }
}

impl SessionManager {
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            event_queue: SessionEventQueue::new(),
            shell_environment: ShellEnvironmentCache::global(),
        }
    }

    /// Returns the shared shell environment cache used by this manager's SSH
    /// sessions and forwarding channels.
    pub fn shell_environment(&self) -> Arc<ShellEnvironmentCache> {
        self.shell_environment.clone()
    }

    pub fn create_local_session(
        &self,
        config: LocalSessionConfig,
    ) -> Result<SessionInfo, SessionError> {
        let session_id = uuid::Uuid::new_v4().to_string();
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(local_pty_size(
                config.cols,
                config.rows,
                config.pixel_width,
                config.pixel_height,
            ))
            .map_err(SessionError::OpenPty)?;

        let mut command = build_command(&config);
        configure_environment(&mut command);
        if let Some(working_dir) = &config.working_dir {
            command.cwd(working_dir);
        }

        let reader = pair
            .master
            .try_clone_reader()
            .map_err(SessionError::CloneReader)?;
        let writer = pair
            .master
            .take_writer()
            .map_err(SessionError::TakeWriter)?;
        let child = pair
            .slave
            .spawn_command(command)
            .map_err(SessionError::Spawn)?;
        drop(pair.slave);

        let info = SessionInfo {
            id: session_id.clone(),
            name: config.name,
            kind: SessionKind::LocalPty,
            working_dir: config.working_dir.clone(),
            cols: config.cols,
            rows: config.rows,
        };
        let reader_thread =
            spawn_reader_thread(session_id.clone(), reader, self.event_queue.clone());
        let writer = QueuedTransportWriter::spawn(
            session_id.clone(),
            writer,
            false,
            self.event_queue.clone(),
        );
        let session = LocalPtyTransport {
            info: info.clone(),
            master: pair.master,
            writer,
            child,
            reader_thread: Some(reader_thread),
        };

        self.sessions
            .lock()
            .map_err(|_| SessionError::LockPoisoned)?
            .insert(session_id, ManagedSession::Local(session));

        Ok(info)
    }

    pub fn create_telnet_session(
        &self,
        config: TelnetSessionConfig,
    ) -> Result<SessionInfo, SessionError> {
        let session_id = uuid::Uuid::new_v4().to_string();
        let addr = format!("{}:{}", config.host, config.port);
        let stream = TcpStream::connect(&addr).map_err(|source| SessionError::ConnectTcp {
            addr: addr.clone(),
            source,
        })?;
        stream.set_nodelay(true).ok();
        stream
            .set_read_timeout(Some(Duration::from_millis(250)))
            .ok();

        let mut writer = stream
            .try_clone()
            .map_err(|source| SessionError::CloneTcp {
                session_id: session_id.clone(),
                source,
            })?;
        let response_writer = stream
            .try_clone()
            .map_err(|source| SessionError::CloneTcp {
                session_id: session_id.clone(),
                source,
            })?;

        if let Some(naws) = maybe_build_naws(config.cols, config.rows, &config) {
            writer.write_all(&naws).ok();
            writer.flush().ok();
        }

        let info = SessionInfo {
            id: session_id.clone(),
            name: config.name.clone(),
            kind: if config.raw_tcp {
                SessionKind::RawTcp
            } else {
                SessionKind::Telnet
            },
            working_dir: None,
            cols: config.cols,
            rows: config.rows,
        };

        let auto_login =
            TelnetAutoLoginState::new(&config).map(|state| Arc::new(Mutex::new(state)));
        let reader_thread = spawn_tcp_reader_thread(
            session_id.clone(),
            stream
                .try_clone()
                .map_err(|source| SessionError::CloneTcp {
                    session_id: session_id.clone(),
                    source,
                })?,
            response_writer,
            config.clone(),
            auto_login.clone(),
            self.event_queue.clone(),
        );
        let writer = QueuedTransportWriter::spawn(
            session_id.clone(),
            writer,
            config.force_character_at_a_time,
            self.event_queue.clone(),
        );

        let session = TelnetTransport {
            info: info.clone(),
            writer,
            reader_stream: stream,
            backspace_as_bs: config.backspace_mode == "ctrl_h",
            local_line_buffer: Vec::new(),
            auto_login,
            event_queue: self.event_queue.clone(),
            config,
            reader_thread: Some(reader_thread),
        };

        self.sessions
            .lock()
            .map_err(|_| SessionError::LockPoisoned)?
            .insert(session_id, ManagedSession::Tcp(session));

        Ok(info)
    }

    pub fn create_ssh_session(
        &self,
        config: SshSessionConfig,
    ) -> Result<SessionInfo, SessionError> {
        let (info, _, start_handle) = self.create_ssh_session_inner(config, None)?;
        start_handle.start();
        Ok(info)
    }

    pub fn create_ssh_session_with_shared_handle(
        &self,
        config: SshSessionConfig,
    ) -> Result<(SessionInfo, SshMultiplexHandle), SessionError> {
        let (info, handle, start_handle) =
            self.create_ssh_session_with_shared_handle_deferred(config)?;
        start_handle.start();
        Ok((info, handle))
    }

    /// Creates an SSH session whose PTY remains paused until the desktop has
    /// registered the session and terminal frame state.
    pub fn create_ssh_session_with_shared_handle_deferred(
        &self,
        config: SshSessionConfig,
    ) -> Result<(SessionInfo, SshMultiplexHandle, SshSessionStartHandle), SessionError> {
        let addr = format!("{}:{}", config.host, config.port);
        let (info, handle, start_handle) = self.create_ssh_session_inner(config, None)?;
        let handle = handle.ok_or_else(|| SessionError::CreateSsh {
            addr,
            source: anyhow::anyhow!("SSH session did not expose a reusable handle"),
        })?;
        Ok((info, handle, start_handle))
    }

    pub fn create_ssh_session_with_multiplex(
        &self,
        config: SshSessionConfig,
        multiplex: SshMultiplexHandle,
    ) -> Result<SessionInfo, SessionError> {
        let (info, start_handle) =
            self.create_ssh_session_with_multiplex_deferred(config, multiplex)?;
        start_handle.start();
        Ok(info)
    }

    /// Opens an interactive channel on an existing authenticated connection,
    /// keeping its PTY paused until the desktop registration is complete.
    pub fn create_ssh_session_with_multiplex_deferred(
        &self,
        config: SshSessionConfig,
        multiplex: SshMultiplexHandle,
    ) -> Result<(SessionInfo, SshSessionStartHandle), SessionError> {
        multiplex
            .ensure_matches_config(&config)
            .map_err(|source| SessionError::CreateSsh {
                addr: format!("{}:{}", config.host, config.port),
                source,
            })?;
        let (info, _, start_handle) = self.create_ssh_session_inner(config, Some(multiplex))?;
        Ok((info, start_handle))
    }

    fn create_ssh_session_inner(
        &self,
        config: SshSessionConfig,
        multiplex: Option<SshMultiplexHandle>,
    ) -> Result<
        (
            SessionInfo,
            Option<SshMultiplexHandle>,
            SshSessionStartHandle,
        ),
        SessionError,
    > {
        let session_id = uuid::Uuid::new_v4().to_string();
        let addr = format!("{}:{}", config.host, config.port);
        validate_ssh_algorithm_preferences(config.ssh_algorithms.as_ref()).map_err(|source| {
            SessionError::CreateSsh {
                addr: addr.clone(),
                source: source.into(),
            }
        })?;
        let (command_tx, command_rx) = tokio_mpsc::unbounded_channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        // Keep the IO loop behind a registration barrier. Tauri registers the
        // session before spawning its reader; without the equivalent handshake
        // here, the first MOTD bytes can arrive before the GPUI frame session
        // and reconnect seed are installed.
        let (start_tx, start_rx) = tokio::sync::oneshot::channel();
        let event_queue = self.event_queue.clone();
        let worker_config = config.clone();
        let worker_session_id = session_id.clone();
        let worker_thread = std::thread::spawn(move || {
            run_ssh_worker(
                worker_session_id,
                worker_config,
                command_rx,
                ready_tx,
                event_queue,
                multiplex,
                start_rx,
            );
        });

        let shared_handle = match ready_rx.recv() {
            Ok(Ok(handle)) => handle,
            Ok(Err(message)) => {
                let _ = worker_thread.join();
                return Err(SessionError::CreateSsh {
                    addr,
                    source: anyhow::anyhow!(message),
                });
            }
            Err(error) => {
                let _ = worker_thread.join();
                return Err(SessionError::CreateSsh {
                    addr,
                    source: anyhow::anyhow!("SSH worker exited before readiness: {error}"),
                });
            }
        };

        let info = SessionInfo {
            id: session_id.clone(),
            name: config.name,
            kind: SessionKind::Ssh,
            working_dir: None,
            cols: config.cols,
            rows: config.rows,
        };
        let session = SshChannelTransport {
            info: info.clone(),
            command_tx,
            backspace_as_bs: config.backspace_mode == "ctrl_h",
            worker_thread: Some(worker_thread),
        };

        self.sessions
            .lock()
            .map_err(|_| SessionError::LockPoisoned)?
            .insert(session_id, ManagedSession::Ssh(session));

        // The manager now owns the channel. Keep the worker paused until the
        // desktop registers the matching session/frame state; dropping this
        // handle on a failed start closes the worker's start receiver and
        // releases its pending SSH resources.
        Ok((info, shared_handle, SshSessionStartHandle::new(start_tx)))
    }

    pub fn create_serial_session(
        &self,
        config: SerialSessionConfig,
    ) -> Result<SessionInfo, SessionError> {
        let session_id = uuid::Uuid::new_v4().to_string();
        let port = open_serial_port(&config).map_err(|source| SessionError::OpenSerial {
            port_name: config.port_name.clone(),
            source,
        })?;
        let reader = port
            .try_clone()
            .map_err(|source| SessionError::CloneSerial {
                session_id: session_id.clone(),
                source,
            })?;

        let info = SessionInfo {
            id: session_id.clone(),
            name: config.name,
            kind: SessionKind::Serial,
            working_dir: None,
            cols: 80,
            rows: 24,
        };
        let stop_reader = Arc::new(AtomicBool::new(false));
        let reader_thread = spawn_serial_reader_thread(
            session_id.clone(),
            reader,
            stop_reader.clone(),
            self.event_queue.clone(),
        );
        let writer =
            QueuedTransportWriter::spawn(session_id.clone(), port, false, self.event_queue.clone());
        let session = SerialTransport {
            info: info.clone(),
            writer,
            backspace_as_bs: config.backspace_mode == "ctrl_h",
            stop_reader,
            reader_thread: Some(reader_thread),
        };

        self.sessions
            .lock()
            .map_err(|_| SessionError::LockPoisoned)?
            .insert(session_id, ManagedSession::Serial(session));

        Ok(info)
    }

    pub fn list_serial_ports(&self) -> Result<Vec<String>, SessionError> {
        let mut ports = serialport::available_ports()
            .map_err(|source| SessionError::OpenSerial {
                port_name: "<list>".to_string(),
                source,
            })?
            .into_iter()
            .map(|port| port.port_name)
            .collect::<Vec<_>>();
        ports.sort_unstable();
        Ok(ports)
    }

    pub fn list_sessions(&self) -> Result<Vec<SessionInfo>, SessionError> {
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| SessionError::LockPoisoned)?
            .values()
            .map(ManagedSession::info)
            .collect::<Vec<_>>();
        sessions.sort_by(|left, right| left.name.cmp(&right.name).then(left.id.cmp(&right.id)));
        Ok(sessions)
    }

    pub fn write(&self, session_id: &str, data: &[u8]) -> Result<(), SessionError> {
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| SessionError::LockPoisoned)?;
        let session = sessions
            .get_mut(session_id)
            .ok_or_else(|| SessionError::NotFound(session_id.to_string()))?;
        session.write(data).map_err(|source| SessionError::Write {
            session_id: session_id.to_string(),
            source,
        })
    }

    pub fn resize(&self, session_id: &str, cols: u16, rows: u16) -> Result<(), SessionError> {
        self.resize_with_pixels(session_id, cols, rows, 0, 0)
    }

    /// Resize the live session, including total pixel dimensions when known.
    /// Pixel size is used by local PTY masters and SSH `window-change` / `request_pty`.
    pub fn resize_with_pixels(
        &self,
        session_id: &str,
        cols: u16,
        rows: u16,
        pixel_width: u16,
        pixel_height: u16,
    ) -> Result<(), SessionError> {
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| SessionError::LockPoisoned)?;
        let session = sessions
            .get_mut(session_id)
            .ok_or_else(|| SessionError::NotFound(session_id.to_string()))?;
        session
            .resize(cols, rows, pixel_width, pixel_height)
            .map_err(|source| SessionError::Resize {
                session_id: session_id.to_string(),
                source,
            })
    }

    pub fn close(&self, session_id: &str) -> Result<(), SessionError> {
        let mut session = self
            .sessions
            .lock()
            .map_err(|_| SessionError::LockPoisoned)?
            .remove(session_id)
            .ok_or_else(|| SessionError::NotFound(session_id.to_string()))?;
        session.close();
        Ok(())
    }

    pub fn try_recv_event(&self) -> Result<Option<SessionEvent>, SessionError> {
        Ok(self.event_queue.drain(1).events.into_iter().next())
    }

    pub fn drain_events(&self, max_events: usize) -> Result<SessionDrain, SessionError> {
        Ok(self.event_queue.drain(max_events))
    }

    pub fn drain_events_with_output_budget(
        &self,
        max_events: usize,
        max_output_bytes: usize,
    ) -> Result<SessionDrain, SessionError> {
        Ok(self
            .event_queue
            .drain_with_output_budget(max_events, Some(max_output_bytes)))
    }

    /// Like [`Self::drain_events_with_output_budget`], but parks up to `timeout`
    /// waiting for the first event instead of returning an empty drain.
    ///
    /// Only for a dedicated consumer thread — the UI tick path must keep using
    /// the non-blocking variants.
    pub fn drain_events_blocking_with_output_budget(
        &self,
        max_events: usize,
        max_output_bytes: usize,
        timeout: Duration,
    ) -> Result<SessionDrain, SessionError> {
        Ok(self.event_queue.drain_blocking_with_output_budget(
            max_events,
            Some(max_output_bytes),
            timeout,
        ))
    }
}

impl ManagedSession {
    fn info(&self) -> SessionInfo {
        match self {
            Self::Local(session) => session.info.clone(),
            Self::Ssh(session) => session.info.clone(),
            Self::Tcp(session) => session.info.clone(),
            Self::Serial(session) => session.info.clone(),
        }
    }

    fn write(&mut self, data: &[u8]) -> anyhow::Result<()> {
        match self {
            Self::Local(session) => session.write(data),
            Self::Tcp(session) => session.write(data),
            Self::Ssh(session) => session.write(data),
            Self::Serial(session) => session.write(data),
        }
    }

    fn resize(
        &mut self,
        cols: u16,
        rows: u16,
        pixel_width: u16,
        pixel_height: u16,
    ) -> anyhow::Result<()> {
        match self {
            Self::Local(session) => session.resize(cols, rows, pixel_width, pixel_height),
            Self::Tcp(session) => session.resize(cols, rows, pixel_width, pixel_height),
            Self::Ssh(session) => session.resize(cols, rows, pixel_width, pixel_height),
            Self::Serial(session) => session.resize(cols, rows, pixel_width, pixel_height),
        }
    }

    fn close(&mut self) {
        match self {
            Self::Local(session) => {
                let _ = session.close();
            }
            Self::Tcp(session) => {
                let _ = session.close();
            }
            Self::Ssh(session) => {
                let _ = session.close();
            }
            Self::Serial(session) => {
                let _ = session.close();
            }
        }
    }
}

impl TerminalTransport for LocalPtyTransport {
    fn write(&mut self, data: &[u8]) -> anyhow::Result<()> {
        self.writer.write(data.to_vec())
    }

    fn resize(
        &mut self,
        cols: u16,
        rows: u16,
        pixel_width: u16,
        pixel_height: u16,
    ) -> anyhow::Result<()> {
        self.master
            .resize(local_pty_size(cols, rows, pixel_width, pixel_height))?;
        self.info.cols = cols;
        self.info.rows = rows;
        Ok(())
    }

    fn close(&mut self) -> anyhow::Result<()> {
        let _ = self.child.kill();
        self.writer.close();
        if let Some(reader_thread) = self.reader_thread.take() {
            let _ = reader_thread.join();
        }
        Ok(())
    }
}

impl TerminalTransport for TelnetTransport {
    fn write(&mut self, data: &[u8]) -> anyhow::Result<()> {
        if !data.is_empty()
            && let Some(auto_login) = self.auto_login.as_ref()
            && let Ok(mut auto_login) = auto_login.lock()
        {
            let _ = auto_login.handle_user_input(false);
        }
        let data = if self.backspace_as_bs {
            remap_del_to_bs(data)
        } else {
            data.to_vec()
        };
        let (data, visible_echo) = if self.config.local_line_edit {
            edit_telnet_line_input(&data, &mut self.local_line_buffer, &self.config)
        } else {
            let visible_echo = self
                .config
                .local_echo
                .then(|| data.clone())
                .unwrap_or_default();
            (data, visible_echo)
        };
        let data = normalize_telnet_input(&data, &self.config);
        self.writer.write(data)?;
        if !visible_echo.is_empty() {
            self.event_queue.push(SessionEvent::Output {
                session_id: self.info.id.clone(),
                data: visible_echo,
            });
        }
        Ok(())
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
        if let Some(naws) = maybe_build_naws(cols, rows, &self.config) {
            self.writer.write(naws)?;
        }
        Ok(())
    }

    fn close(&mut self) -> anyhow::Result<()> {
        let _ = self.reader_stream.shutdown(Shutdown::Both);
        self.writer.close();
        if let Some(reader_thread) = self.reader_thread.take() {
            let _ = reader_thread.join();
        }
        Ok(())
    }
}

impl TerminalTransport for SshChannelTransport {
    fn write(&mut self, data: &[u8]) -> anyhow::Result<()> {
        let data = if self.backspace_as_bs {
            remap_del_to_bs(data)
        } else {
            data.to_vec()
        };
        self.command_tx
            .send(SshCommand::Write(data))
            .map_err(|_| anyhow::anyhow!("SSH worker stopped"))?;
        Ok(())
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
            .send(SshCommand::Resize {
                cols,
                rows,
                pixel_width,
                pixel_height,
            })
            .map_err(|_| anyhow::anyhow!("SSH worker stopped"))?;
        Ok(())
    }

    fn close(&mut self) -> anyhow::Result<()> {
        let _ = self.command_tx.send(SshCommand::Close);
        if let Some(worker_thread) = self.worker_thread.take() {
            let _ = worker_thread.join();
        }
        Ok(())
    }
}

impl TerminalTransport for SerialTransport {
    fn write(&mut self, data: &[u8]) -> anyhow::Result<()> {
        let data = if self.backspace_as_bs {
            remap_del_to_bs(data)
        } else {
            data.to_vec()
        };
        self.writer.write(data)
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

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}

fn spawn_reader_thread(
    session_id: String,
    mut reader: Box<dyn Read + Send>,
    event_queue: SessionEventQueue,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
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
                    event_queue.push(SessionEvent::Output {
                        session_id: session_id.clone(),
                        data: buffer[..read].to_vec(),
                    });
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

fn spawn_tcp_reader_thread(
    session_id: String,
    mut reader: TcpStream,
    mut response_writer: TcpStream,
    config: TelnetSessionConfig,
    auto_login: Option<Arc<Mutex<TelnetAutoLoginState>>>,
    event_queue: SessionEventQueue,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let mut buffer = [0_u8; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => {
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
                        strip_telnet_commands(&buffer[..read], &mut |command, option| {
                            let response = negotiate_response(
                                command,
                                option,
                                config.send_naws,
                                config.send_sga,
                            );
                            if !response.is_empty() {
                                let _ = response_writer.write_all(&response);
                                let _ = response_writer.flush();
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
                                        let _ = response_writer.write_all(&payload);
                                        let _ = response_writer.flush();
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

const TELNET_AUTO_LOGIN_TAIL_CHARS: usize = 2048;
const TELNET_AUTO_LOGIN_PROMPT_WINDOW_CHARS: usize = 320;

#[derive(Debug, Clone, PartialEq, Eq)]
enum TelnetAutoLoginAction {
    Send(Vec<u8>),
    Complete,
    Disable,
}

struct TelnetAutoLoginState {
    username: String,
    password: Option<String>,
    started_at: Instant,
    tail: String,
    sent_wake: bool,
    sent_username: bool,
    sent_password: bool,
    disabled: bool,
    completed: bool,
    retries: u8,
    username_regex: Option<Regex>,
    password_regex: Option<Regex>,
    success_regex: Option<Regex>,
    failure_regex: Option<Regex>,
}

impl TelnetAutoLoginState {
    fn new(config: &TelnetSessionConfig) -> Option<Self> {
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

    fn handle_visible_output(
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

    fn handle_user_input(&mut self, automated: bool) -> Option<TelnetAutoLoginAction> {
        if automated || self.disabled || self.completed {
            return None;
        }
        self.disabled = true;
        Some(TelnetAutoLoginAction::Disable)
    }

    fn push_tail(&mut self, text: &str) {
        self.tail.push_str(text);
        self.tail = last_chars(&self.tail, TELNET_AUTO_LOGIN_TAIL_CHARS);
    }

    fn matches_username_prompt(&self, prompts: &[String], last_line: &str) -> bool {
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

    fn matches_password_prompt(&self, prompts: &[String]) -> bool {
        prompts.iter().any(|prompt| {
            self.password_regex.as_ref().map_or_else(
                || default_password_regex().is_match(prompt),
                |regex| regex.is_match(prompt),
            )
        })
    }

    fn matches_success(&self, last_line: &str) -> bool {
        self.success_regex.as_ref().map_or_else(
            || default_success_regex().is_match(last_line),
            |regex| regex.is_match(last_line),
        )
    }

    fn matches_failure(&self, text: &str, last_line: &str) -> bool {
        self.failure_regex.as_ref().map_or_else(
            || {
                default_failure_regex().is_match(text)
                    || default_failure_regex().is_match(last_line)
            },
            |regex| regex.is_match(text) || regex.is_match(last_line),
        )
    }
}

fn run_ssh_worker(
    session_id: String,
    config: SshSessionConfig,
    command_rx: tokio_mpsc::UnboundedReceiver<SshCommand>,
    ready_tx: mpsc::Sender<Result<Option<SshMultiplexHandle>, String>>,
    event_queue: SessionEventQueue,
    multiplex: Option<SshMultiplexHandle>,
    start_rx: tokio::sync::oneshot::Receiver<()>,
) {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("nyaterm-ssh")
        .build()
    {
        Ok(runtime) => Arc::new(runtime),
        Err(error) => {
            let _ = ready_tx.send(Err(format!("failed to start SSH runtime: {error}")));
            return;
        }
    };

    let runtime_for_worker = runtime.clone();
    let wait_for_open_commands = config.deferred_pty;
    runtime.block_on(async move {
        run_deferred_ssh_worker(
            session_id,
            config,
            command_rx,
            ready_tx,
            event_queue,
            multiplex,
            runtime_for_worker,
            start_rx,
            wait_for_open_commands,
        )
        .await;
    });
}

async fn run_deferred_ssh_worker(
    session_id: String,
    config: SshSessionConfig,
    mut command_rx: tokio_mpsc::UnboundedReceiver<SshCommand>,
    ready_tx: mpsc::Sender<Result<Option<SshMultiplexHandle>, String>>,
    event_queue: SessionEventQueue,
    multiplex: Option<SshMultiplexHandle>,
    runtime: Arc<tokio::runtime::Runtime>,
    start_rx: tokio::sync::oneshot::Receiver<()>,
    wait_for_open_commands: bool,
) {
    // Keep SFTP and other multiplexed operations behind the interactive shell
    // setup.  Opening another channel during this window can change the
    // ordering of the PTY startup burst that contains the login banner.
    let multiplex_for_ready = multiplex.clone();
    let shell_environment = multiplex
        .as_ref()
        .map(SshMultiplexHandle::shell_environment)
        .unwrap_or_else(ShellEnvironmentCache::global);
    let pending_session = match open_pending_ssh_shell(
        &config,
        multiplex.as_ref(),
        shell_environment.clone(),
    )
    .await
    {
        Ok(session) => session,
        Err(error) => {
            // A reused authenticated connection remains usable even when this
            // session cannot open its interactive channel. Wake other remote
            // operations instead of leaving them parked behind this failed
            // startup attempt.
            if let Some(handle) = multiplex_for_ready.as_ref() {
                handle.mark_interactive_ready();
            }
            let _ = ready_tx.send(Err(error.to_string()));
            return;
        }
    };
    let shared_handle = match &pending_session.handle {
        SshShellHandle::Dedicated(target) => Some(connected_ssh_multiplex_handle(
            &config,
            runtime,
            target.clone(),
            pending_session.jump_handles.clone(),
            pending_session.forwarded_tcpip.clone(),
            false,
            shell_environment,
        )),
        SshShellHandle::Multiplexed(_) => None,
    };
    let worker_handle = shared_handle.clone();
    let _ = ready_tx.send(Ok(shared_handle));
    if start_rx.await.is_err() {
        disconnect_pending_ssh_shell(pending_session, worker_handle.clone()).await;
        return;
    }
    let mut pending_session = Some(pending_session);
    let mut dimensions = SshPtyDimensions::from_config(&config);
    let mut pending_writes = VecDeque::new();
    if wait_for_open_commands {
        let mut fallback = Box::pin(tokio::time::sleep(Duration::from_millis(750)));

        loop {
            tokio::select! {
                command = command_rx.recv() => {
                    match command {
                        Some(SshCommand::Write(data)) => {
                            pending_writes.push_back(data);
                        }
                        Some(SshCommand::Resize {
                            cols,
                            rows,
                            pixel_width,
                            pixel_height,
                        }) => {
                            dimensions = SshPtyDimensions::new(cols, rows, pixel_width, pixel_height);
                            break;
                        }
                        Some(SshCommand::Close) | None => {
                            if let Some(session) = pending_session.take() {
                                disconnect_pending_ssh_shell(session, worker_handle.clone()).await;
                            }
                            return;
                        }
                    }
                }
                _ = &mut fallback => {
                    break;
                }
            }
        }
    }

    let Some(pending_session) = pending_session.take() else {
        return;
    };
    if drain_deferred_ssh_open_commands(&mut command_rx, &mut dimensions, &mut pending_writes) {
        disconnect_pending_ssh_shell(pending_session, worker_handle.clone()).await;
        return;
    }
    match open_ssh_shell_from_pending(&session_id, &config, pending_session, dimensions).await {
        Ok(open_session) => {
            let interactive_ready_handle = worker_handle
                .clone()
                .or_else(|| multiplex_for_ready.clone());
            run_open_ssh_shell_session(
                session_id,
                open_session,
                command_rx,
                event_queue,
                pending_writes,
                worker_handle,
                interactive_ready_handle,
            )
            .await;
        }
        Err(error) => {
            send_session_error(&event_queue, &session_id, error);
            if let Some(handle) = worker_handle {
                let _ = handle.disconnect_async().await;
            } else if let Some(handle) = multiplex_for_ready {
                // The shared authenticated connection remains usable for other
                // sessions even when this interactive channel failed to open.
                handle.mark_interactive_ready();
            }
        }
    }
}

fn drain_deferred_ssh_open_commands(
    command_rx: &mut tokio_mpsc::UnboundedReceiver<SshCommand>,
    dimensions: &mut SshPtyDimensions,
    pending_writes: &mut VecDeque<Vec<u8>>,
) -> bool {
    loop {
        match command_rx.try_recv() {
            Ok(SshCommand::Write(data)) => {
                pending_writes.push_back(data);
            }
            Ok(SshCommand::Resize {
                cols,
                rows,
                pixel_width,
                pixel_height,
            }) => {
                *dimensions = SshPtyDimensions::new(cols, rows, pixel_width, pixel_height);
            }
            Ok(SshCommand::Close) => return true,
            Err(tokio_mpsc::error::TryRecvError::Empty) => return false,
            Err(tokio_mpsc::error::TryRecvError::Disconnected) => return true,
        }
    }
}

async fn run_open_ssh_shell_session(
    session_id: String,
    open_session: OpenSshShellSession,
    mut command_rx: tokio_mpsc::UnboundedReceiver<SshCommand>,
    event_queue: SessionEventQueue,
    mut pending_writes: VecDeque<Vec<u8>>,
    shared_handle: Option<SshMultiplexHandle>,
    interactive_ready_handle: Option<SshMultiplexHandle>,
) {
    let OpenSshShellSession {
        handle,
        mut channel,
        jump_handles,
        disconnect_on_close,
        x11_forwarder,
        local_notice,
        injection_script,
        integration_future,
        ready_marker,
        legacy_ready_marker,
        shell_kind: _shell_kind,
    } = open_session;
    let mut integration_future = integration_future;
    let mut shell_integration = if integration_future.is_some() {
        SshShellIntegrationState::waiting_for_integration(ready_marker, legacy_ready_marker)
    } else {
        SshShellIntegrationState::new(injection_script, ready_marker, legacy_ready_marker)
    };
    // The PTY reader is now running before this handle is exposed to SFTP and
    // other remote operations.  Once the reader owns the channel, opening a
    // second channel cannot hide the login banner; keeping the old
    // post-injection gate here only adds an avoidable startup round trip.
    if let Some(handle) = interactive_ready_handle.as_ref() {
        handle.mark_interactive_ready();
    }
    let mut interactive_ready_marked = false;
    if let Some(notice) = local_notice {
        event_queue.push(SessionEvent::Output {
            session_id: session_id.clone(),
            data: notice,
        });
    }
    if let Some(forwarder) = x11_forwarder {
        spawn_x11_forwarder(event_queue.clone(), session_id.clone(), forwarder);
    }

    // Keep the initial phase open long enough for a delayed PTY/login banner to arrive. A reused
    // SSH transport may split the banner across several channel messages; injecting immediately
    // after the first message would put the shell in suppression mode and discard the remaining
    // banner. The quiet deadline is refreshed for each initial output chunk, while the fallback
    // deadline still guarantees that a silent shell cannot block initialization forever.
    const INITIAL_INJECT_FALLBACK: Duration = Duration::from_secs(5);
    // A PTY can deliver the first prompt before a delayed PAM/MOTD chunk. Only
    // inject after the prompt is observed and the stream has been quiet for a
    // short settling window; otherwise the integration write would suppress
    // a late login banner. The deadline is refreshed for every chunk, and the
    // 500 ms window matches the Tauri transport's initial injection delay.
    const INITIAL_OUTPUT_QUIET: Duration = Duration::from_millis(500);
    let initial_inject_delay = tokio::time::sleep(INITIAL_INJECT_FALLBACK);
    tokio::pin!(initial_inject_delay);
    let initial_output_quiet_delay = tokio::time::sleep(INITIAL_INJECT_FALLBACK);
    tokio::pin!(initial_output_quiet_delay);
    let mut initial_output_seen = false;
    let mut initial_output_log_count = 0_u8;
    let inject_timeout = tokio::time::sleep(Duration::from_secs(30));
    tokio::pin!(inject_timeout);

    loop {
        tokio::select! {
            // Keep command/channel handling ahead of the injection deadlines. Initial output is
            // forwarded first so the login banner remains visible before integration starts.
            biased;
            command = command_rx.recv() => {
                match command {
                    Some(SshCommand::Write(data)) => {
                        if !shell_integration.is_normal() || !interactive_ready_marked {
                            pending_writes.push_back(data);
                        } else if let Err(error) = channel.data_bytes(data).await {
                                send_session_error(&event_queue, &session_id, error);
                                break;
                        }
                    }
                    Some(SshCommand::Resize {
                        cols,
                        rows,
                        pixel_width,
                        pixel_height,
                    }) => {
                        if let Err(error) = channel
                            .window_change(
                                cols.into(),
                                rows.into(),
                                pixel_width.into(),
                                pixel_height.into(),
                            )
                            .await
                        {
                            send_session_error(&event_queue, &session_id, error);
                            break;
                        }
                    }
                    Some(SshCommand::Close) | None => {
                        let _ = channel.eof().await;
                        let _ = channel.close().await;
                        break;
                    }
                }
            }
            integration_script = async {
                integration_future
                    .as_mut()
                    .expect("integration future guarded by select")
                    .as_mut()
                    .await
            }, if integration_future.is_some() => {
                integration_future = None;
                let integration_enabled = integration_script.is_some();
                shell_integration.set_integration_script(integration_script);
                tracing::info!(
                    diagnostic = "ssh_integration_prepared",
                    session_id = %session_id,
                    integration_enabled,
                    "SSH shell integration preparation completed while PTY was readable"
                );
                if !integration_enabled && !initial_output_seen {
                    // A shell that produced no initial bytes does not need the five-second
                    // silent-shell fallback. Give the no-integration path the same short
                    // readiness window as an ordinary prompt burst.
                    initial_output_seen = true;
                    initial_output_quiet_delay
                        .as_mut()
                        .reset(tokio::time::Instant::now() + INITIAL_OUTPUT_QUIET);
                } else if integration_enabled && !initial_output_seen {
                    // Do not retain the old five-second fallback after detection has completed.
                    // If the shell stays silent, sending the integration script after the short
                    // window is sufficient and keeps SFTP startup responsive.
                    initial_output_seen = true;
                    initial_inject_delay
                        .as_mut()
                        .reset(tokio::time::Instant::now() + INITIAL_OUTPUT_QUIET);
                }
            }
            message = channel.wait() => {
                match message {
                    Some(ChannelMsg::Data { data }) => {
                        let was_suppressing = shell_integration.is_suppressing();
                        let output = shell_integration.filter_output(&data);
                        let visible_bytes = output.visible.len();
                        push_ssh_integration_output(&event_queue, &session_id, output);
                        if was_suppressing && shell_integration.is_normal() {
                            mark_ssh_interactive_ready_once(
                                interactive_ready_handle.as_ref(),
                                &mut interactive_ready_marked,
                            );
                            tracing::info!(
                                diagnostic = "ssh_integration_ready",
                                session_id = %session_id,
                                channel_kind = "data",
                                raw_bytes = data.len(),
                                visible_bytes,
                                "SSH shell integration ready marker reached the terminal"
                            );
                        }
                        let is_initial_output = shell_integration.is_waiting_initial()
                            || (shell_integration.is_normal() && !interactive_ready_marked);
                        if is_initial_output {
                            initial_output_seen = true;
                            // Treat every initial chunk as evidence that the remote login burst
                            // is still active. The fallback is a quiet-period deadline, not an
                            // absolute deadline from channel creation; slow PAM/MOTD output must
                            // not be hidden merely because shell detection took a few seconds.
                            initial_inject_delay.as_mut().reset(
                                tokio::time::Instant::now() + INITIAL_INJECT_FALLBACK,
                            );
                            initial_output_quiet_delay.as_mut().reset(
                                tokio::time::Instant::now() + INITIAL_OUTPUT_QUIET,
                            );
                            if initial_output_log_count < 8
                                || shell_integration.initial_prompt_seen()
                            {
                                initial_output_log_count = initial_output_log_count.saturating_add(1);
                                tracing::info!(
                                    diagnostic = "ssh_initial_output",
                                    session_id = %session_id,
                                    channel_kind = "data",
                                    raw_bytes = data.len(),
                                    visible_bytes,
                                    prompt_seen = shell_integration.initial_prompt_seen(),
                                    "SSH initial terminal output reached integration"
                                );
                            }
                        }
                        if shell_integration.is_normal()
                            && interactive_ready_marked
                            && !flush_ssh_pending_writes(
                                &mut channel,
                                &mut pending_writes,
                                &event_queue,
                                &session_id,
                            )
                            .await
                        {
                            break;
                        }
                    }
                    // PTY stderr is delivered as extended data by some servers. Keep it
                    // visible verbatim, matching Tauri, and never let it trigger or corrupt
                    // the shell-integration state machine.
                    Some(ChannelMsg::ExtendedData { data, .. }) => {
                        let was_suppressing = shell_integration.is_suppressing();
                        event_queue.push(SessionEvent::Output {
                            session_id: session_id.clone(),
                            data: data.to_vec(),
                        });
                        if was_suppressing {
                            tracing::debug!(
                                diagnostic = "ssh_integration_extended_data",
                                session_id = %session_id,
                                raw_bytes = data.len(),
                                "SSH extended data arrived while shell integration was suppressing output"
                            );
                        }
                        let is_initial_output = shell_integration.is_waiting_initial()
                            || (shell_integration.is_normal() && !interactive_ready_marked);
                        if is_initial_output {
                            initial_output_seen = true;
                            initial_inject_delay.as_mut().reset(
                                tokio::time::Instant::now() + INITIAL_INJECT_FALLBACK,
                            );
                            initial_output_quiet_delay.as_mut().reset(
                                tokio::time::Instant::now() + INITIAL_OUTPUT_QUIET,
                            );
                            if initial_output_log_count < 8
                                || shell_integration.initial_prompt_seen()
                            {
                                initial_output_log_count = initial_output_log_count.saturating_add(1);
                                tracing::info!(
                                    diagnostic = "ssh_initial_output",
                                    session_id = %session_id,
                                    channel_kind = "extended_data",
                                    raw_bytes = data.len(),
                                    visible_bytes = data.len(),
                                    prompt_seen = shell_integration.initial_prompt_seen(),
                                    "SSH initial terminal output reached integration"
                                );
                            }
                        }
                    }
                    Some(ChannelMsg::ExitStatus { exit_status }) => {
                        event_queue.push(SessionEvent::Exited {
                            session_id: session_id.clone(),
                            reason: format!("SSH channel exit status {exit_status}"),
                        });
                        break;
                    }
                    Some(ChannelMsg::Eof) => {
                        event_queue.push(SessionEvent::Exited {
                            session_id: session_id.clone(),
                            reason: "SSH channel EOF".to_string(),
                        });
                        break;
                    }
                    Some(ChannelMsg::Close) => {
                        event_queue.push(SessionEvent::Exited {
                            session_id: session_id.clone(),
                            reason: "SSH channel closed by remote".to_string(),
                        });
                        break;
                    }
                    None => {
                        event_queue.push(SessionEvent::Exited {
                            session_id: session_id.clone(),
                            reason: "SSH connection task ended".to_string(),
                        });
                        break;
                    }
                    Some(_) => {}
                }
            }
            _ = &mut initial_output_quiet_delay,
                if initial_output_seen
                    && ((shell_integration.initial_prompt_seen()
                        && shell_integration.should_inject_on_initial_delay())
                        || (shell_integration.is_normal() && !interactive_ready_marked)) =>
            {
                if shell_integration.should_inject_on_initial_delay() {
                    // Initial output has been quiet long enough to be considered a complete login
                    // banner/prompt burst. Inject only after forwarding that burst in full.
                    tracing::info!(
                        diagnostic = "ssh_integration_inject",
                        session_id = %session_id,
                        reason = "initial_output_quiet",
                        "sending SSH shell integration script"
                    );
                    shell_integration.inject(&mut channel).await;
                    // The banner has already been forwarded and the integration write is now
                    // in flight. Other channels can safely reuse the authenticated SSH
                    // connection; waiting for the ready marker would add another network RTT.
                    mark_ssh_interactive_ready_once(
                        interactive_ready_handle.as_ref(),
                        &mut interactive_ready_marked,
                    );
                    inject_timeout
                        .as_mut()
                        .reset(tokio::time::Instant::now() + Duration::from_secs(30));
                } else {
                    // Without shell integration there is no marker to wait for. A quiet initial
                    // stream is the only safe point at which SFTP may reuse this connection.
                    mark_ssh_interactive_ready_once(
                        interactive_ready_handle.as_ref(),
                        &mut interactive_ready_marked,
                    );
                }
                if shell_integration.is_normal() {
                    // A failed integration write transitions directly back to normal.
                    mark_ssh_interactive_ready_once(
                        interactive_ready_handle.as_ref(),
                        &mut interactive_ready_marked,
                    );
                }
                if shell_integration.is_normal() && interactive_ready_marked
                    && !flush_ssh_pending_writes(
                        &mut channel,
                        &mut pending_writes,
                        &event_queue,
                        &session_id,
                    )
                    .await
                {
                    break;
                }
            }
            _ = &mut initial_inject_delay,
                if shell_integration.should_inject_on_initial_delay()
                    || (shell_integration.is_normal()
                        && !interactive_ready_marked
                        && !initial_output_seen) =>
            {
                if shell_integration.should_inject_on_initial_delay() {
                    // No output arrived during the fallback window. Inject now so a quiet shell
                    // can still finish setup without waiting for a prompt that may never be emitted.
                    tracing::info!(
                        diagnostic = "ssh_integration_inject",
                        session_id = %session_id,
                        reason = "initial_output_fallback",
                        "sending SSH shell integration script"
                    );
                    shell_integration.inject(&mut channel).await;
                    inject_timeout
                        .as_mut()
                        .reset(tokio::time::Instant::now() + Duration::from_secs(30));
                } else {
                    // A silent shell has no banner to wait for, so use the fallback deadline to
                    // release operations that share the authenticated connection.
                    mark_ssh_interactive_ready_once(
                        interactive_ready_handle.as_ref(),
                        &mut interactive_ready_marked,
                    );
                }
                if shell_integration.is_normal() {
                    // A failed integration write transitions directly back to normal.
                    mark_ssh_interactive_ready_once(
                        interactive_ready_handle.as_ref(),
                        &mut interactive_ready_marked,
                    );
                }
                if shell_integration.is_normal() && interactive_ready_marked
                    && !flush_ssh_pending_writes(
                        &mut channel,
                        &mut pending_writes,
                        &event_queue,
                        &session_id,
                    )
                    .await
                {
                    break;
                }
            }
            _ = &mut inject_timeout, if shell_integration.is_suppressing() => {
                let output = shell_integration.force_normal_after_timeout();
                push_ssh_integration_output(&event_queue, &session_id, output);
                mark_ssh_interactive_ready_once(
                    interactive_ready_handle.as_ref(),
                    &mut interactive_ready_marked,
                );
                if !flush_ssh_pending_writes(
                    &mut channel,
                    &mut pending_writes,
                    &event_queue,
                    &session_id,
                )
                .await
                {
                    break;
                }
            }
        }
    }

    // Release waiters even if the remote closes before emitting a ready marker.
    // A failed shell must not leave SFTP blocked forever.
    mark_ssh_interactive_ready_once(
        interactive_ready_handle.as_ref(),
        &mut interactive_ready_marked,
    );
    disconnect_open_ssh_shell(shared_handle, handle, jump_handles, disconnect_on_close).await;
}

fn mark_ssh_interactive_ready_once(handle: Option<&SshMultiplexHandle>, marked: &mut bool) {
    if *marked {
        return;
    }
    if let Some(handle) = handle {
        handle.mark_interactive_ready();
    }
    *marked = true;
}

async fn flush_ssh_pending_writes(
    channel: &mut russh::Channel<client::Msg>,
    pending_writes: &mut VecDeque<Vec<u8>>,
    event_queue: &SessionEventQueue,
    session_id: &str,
) -> bool {
    while let Some(data) = pending_writes.pop_front() {
        if let Err(error) = channel.data_bytes(data).await {
            send_session_error(event_queue, session_id, error);
            return false;
        }
    }
    true
}

fn push_ssh_integration_output(
    event_queue: &SessionEventQueue,
    session_id: &str,
    output: SshIntegrationOutput,
) {
    for cwd in output.cwd_paths {
        event_queue.push(SessionEvent::CwdChanged {
            session_id: session_id.to_string(),
            cwd,
        });
    }
    for command in output.accepted_commands {
        event_queue.push(SessionEvent::CommandAccepted {
            session_id: session_id.to_string(),
            command,
        });
    }
    if !output.visible.is_empty() {
        event_queue.push(SessionEvent::Output {
            session_id: session_id.to_string(),
            data: output.visible,
        });
    }
}

async fn open_pending_ssh_shell(
    config: &SshSessionConfig,
    multiplex: Option<&SshMultiplexHandle>,
    shell_environment: Arc<ShellEnvironmentCache>,
) -> anyhow::Result<PendingOpenSshShellSession> {
    tracing::debug!(
        stage = "connection",
        host = %config.host,
        port = config.port,
        profile = ?config.profile,
        multiplexed = multiplex.is_some(),
        "opening SSH transport"
    );
    let x11_config = if config.x11_forwarding {
        Some(prepare_x11_forwarding(&config.x11_display).await)
    } else {
        None
    };
    let (x11_tx, x11_rx) = if x11_config.is_some() {
        let (tx, rx) = tokio_mpsc::unbounded_channel();
        (Some(tx), Some(rx))
    } else {
        (None, None)
    };
    let (handle, forwarded_tcpip, jump_handles, disconnect_on_close) = if let Some(multiplex) =
        multiplex
    {
        multiplex.ensure_matches_config(config)?;
        if x11_tx.is_some() {
            anyhow::bail!("X11 forwarding is not supported for multiplexed SSH shell sessions");
        }
        let handle = multiplex.target_handle();
        (
            SshShellHandle::Multiplexed(handle),
            multiplex.forwarded_tcpip_registry(),
            Vec::new(),
            false,
        )
    } else {
        // Keep the forwarding registry attached to the authenticated client's
        // handler and expose the same registry through the reusable handle.
        // Otherwise tunnels opened after the terminal would share SSH auth but
        // could not receive forwarded TCP/IP channels on that connection.
        let forwarded_tcpip = Arc::new(tokio::sync::Mutex::new(ForwardedTcpIpDispatch::default()));
        let (handle, jump_handles) = open_authenticated_ssh_handle_with_sender_registry(
            config,
            Some(forwarded_tcpip.clone()),
            x11_tx,
            shell_environment,
        )
        .await?;
        let handle = Arc::new(tokio::sync::Mutex::new(handle));
        let jump_handles = jump_handles
            .into_iter()
            .map(|jump| Arc::new(tokio::sync::Mutex::new(jump)))
            .collect();
        (
            SshShellHandle::Dedicated(handle),
            forwarded_tcpip,
            jump_handles,
            true,
        )
    };

    Ok(PendingOpenSshShellSession {
        handle,
        forwarded_tcpip,
        jump_handles,
        disconnect_on_close,
        x11_config,
        x11_rx,
    })
}

async fn open_ssh_shell_from_pending(
    session_id: &str,
    config: &SshSessionConfig,
    pending: PendingOpenSshShellSession,
    dimensions: SshPtyDimensions,
) -> anyhow::Result<OpenSshShellSession> {
    let PendingOpenSshShellSession {
        mut handle,
        jump_handles,
        disconnect_on_close,
        x11_config,
        x11_rx,
        ..
    } = pending;
    let ready_marker = build_ssh_ready_marker(session_id);
    let legacy_ready_marker = build_legacy_ssh_ready_marker(&ready_marker);
    let channel = match &mut handle {
        SshShellHandle::Dedicated(handle) | SshShellHandle::Multiplexed(handle) => {
            handle.lock().await.channel_open_session().await?
        }
    };
    if effective_agent_forwarding_config(config).is_some_and(|forwarding| forwarding.enabled) {
        channel.agent_forward(false).await?;
    }
    tracing::debug!(
        stage = "interactive-channel",
        host = %config.host,
        port = config.port,
        profile = ?config.profile,
        "opened SSH session channel"
    );
    let (x11_forwarder, local_notice) = if let (Some(config), Some(rx)) = (x11_config, x11_rx) {
        match channel
            .request_x11(true, false, MIT_MAGIC_COOKIE, &config.fake_cookie_hex, 0)
            .await
        {
            Ok(()) => (Some(X11Forwarder { rx, config }), None),
            Err(_) => (None, Some(enable_x11_failed_message().into_bytes())),
        }
    } else {
        (None, None)
    };
    channel
        .request_pty(
            false,
            &config.term,
            dimensions.cols.into(),
            dimensions.rows.into(),
            dimensions.pixel_width.into(),
            dimensions.pixel_height.into(),
            &[],
        )
        .await?;
    tracing::debug!(
        stage = "pty",
        host = %config.host,
        port = config.port,
        profile = ?config.profile,
        term = %config.term,
        cols = dimensions.cols,
        rows = dimensions.rows,
        "SSH PTY accepted"
    );
    channel.request_shell(false).await?;
    tracing::debug!(
        stage = "shell",
        host = %config.host,
        port = config.port,
        profile = ?config.profile,
        "SSH shell accepted"
    );
    let cwd_follow_mode = config.effective_cwd_follow_mode();
    let terminal_shell_integration = config.effective_terminal_shell_integration();
    let integration_future = if terminal_shell_integration
        || !matches!(cwd_follow_mode, SftpCwdFollowMode::Off)
    {
        let integration_handle = handle.clone();
        let integration_ready_marker = ready_marker.clone();
        let detection_timeout_ms = config.shell_integration_detection_timeout_ms(cwd_follow_mode);
        let install_timeout_ms = config.sftp.shell_detection_timeout_ms;
        Some(Box::pin(async move {
            let started_at = Instant::now();
            let shell_kind = detect_ssh_shell_type(&integration_handle, detection_timeout_ms).await;
            tracing::info!(
                diagnostic = "ssh_shell_detection",
                elapsed_ms = started_at.elapsed().as_millis() as u64,
                detected = shell_kind.is_some(),
                timeout_ms = detection_timeout_ms,
                "SSH shell detection completed without blocking the PTY reader"
            );
            let Some(shell_kind) = shell_kind else {
                return None;
            };
            let script = build_ssh_shell_integration_script(
                &integration_handle,
                shell_kind,
                &integration_ready_marker,
                terminal_shell_integration,
                cwd_follow_mode,
                install_timeout_ms,
            )
            .await;
            tracing::info!(
                diagnostic = "ssh_integration_prepare",
                elapsed_ms = started_at.elapsed().as_millis() as u64,
                shell = ?shell_kind,
                integration_enabled = script.is_some(),
                "SSH shell integration script prepared asynchronously"
            );
            script
        }) as SshIntegrationPreparation)
    } else {
        None
    };
    tracing::debug!(
        stage = "integration",
        host = %config.host,
        port = config.port,
        profile = ?config.profile,
        cwd_follow_mode = ?cwd_follow_mode,
        integration_pending = integration_future.is_some(),
        integration_enabled = terminal_shell_integration
            || !matches!(cwd_follow_mode, SftpCwdFollowMode::Off),
        "started SSH shell integration preparation"
    );
    let handle = match handle {
        SshShellHandle::Dedicated(handle) => Some(handle),
        SshShellHandle::Multiplexed(_) => None,
    };
    Ok(OpenSshShellSession {
        handle,
        channel,
        jump_handles,
        disconnect_on_close,
        x11_forwarder,
        local_notice,
        injection_script: None,
        integration_future,
        ready_marker,
        legacy_ready_marker,
        shell_kind: None,
    })
}

async fn disconnect_pending_ssh_shell(
    session: PendingOpenSshShellSession,
    shared_handle: Option<SshMultiplexHandle>,
) {
    if session.disconnect_on_close {
        if let Some(handle) = shared_handle {
            let _ = handle.disconnect_async().await;
            return;
        }
        if let SshShellHandle::Dedicated(handle) = session.handle {
            let _ = handle
                .lock()
                .await
                .disconnect(Disconnect::ByApplication, "session closed", "en")
                .await;
        }
        for jump_handle in session.jump_handles {
            let _ = jump_handle
                .lock()
                .await
                .disconnect(Disconnect::ByApplication, "session closed", "en")
                .await;
        }
    }
}

async fn disconnect_open_ssh_shell(
    shared_handle: Option<SshMultiplexHandle>,
    handle: Option<SharedSshHandle>,
    jump_handles: Vec<SharedSshHandle>,
    disconnect_on_close: bool,
) {
    if disconnect_on_close {
        if let Some(handle) = shared_handle {
            let _ = handle.disconnect_async().await;
            return;
        }
        if let Some(handle) = handle {
            let _ = handle
                .lock()
                .await
                .disconnect(Disconnect::ByApplication, "session closed", "en")
                .await;
        }
        for jump_handle in jump_handles {
            let _ = jump_handle
                .lock()
                .await
                .disconnect(Disconnect::ByApplication, "session closed", "en")
                .await;
        }
    }
}

impl SshPtyDimensions {
    fn new(cols: u16, rows: u16, pixel_width: u16, pixel_height: u16) -> Self {
        Self {
            cols: cols.max(1),
            rows: rows.max(1),
            pixel_width,
            pixel_height,
        }
    }

    fn from_config(config: &SshSessionConfig) -> Self {
        Self::new(
            config.cols,
            config.rows,
            config.pixel_width,
            config.pixel_height,
        )
    }
}

fn ssh_client_config(config: &SshSessionConfig) -> anyhow::Result<Arc<russh::client::Config>> {
    let keepalive_interval = if config.keep_alive_interval_secs == 0 {
        None
    } else {
        Some(Duration::from_secs(u64::from(
            config.keep_alive_interval_secs,
        )))
    };
    let preferred = resolve_preferred_algorithms(config.ssh_algorithms.as_ref())?;
    Ok(Arc::new(russh::client::Config {
        inactivity_timeout: None,
        keepalive_interval,
        keepalive_max: 3,
        preferred,
        ..Default::default()
    }))
}

type SshHandleChain = (
    client::Handle<SshClientHandler>,
    Vec<client::Handle<SshClientHandler>>,
);

fn open_authenticated_ssh_handle(
    config: &SshSessionConfig,
) -> Pin<Box<dyn Future<Output = anyhow::Result<SshHandleChain>> + Send + '_>> {
    open_authenticated_ssh_handle_with_environment(config, ShellEnvironmentCache::global())
}

fn open_authenticated_ssh_handle_with_environment(
    config: &SshSessionConfig,
    shell_environment: Arc<ShellEnvironmentCache>,
) -> Pin<Box<dyn Future<Output = anyhow::Result<SshHandleChain>> + Send + '_>> {
    open_authenticated_ssh_handle_with_channel_senders(config, None, None, shell_environment)
}

fn open_authenticated_ssh_handle_with_forwarded_tx(
    config: &SshSessionConfig,
    forwarded_tcpip_tx: Option<tokio_mpsc::UnboundedSender<ForwardedTcpIpChannel>>,
) -> Pin<Box<dyn Future<Output = anyhow::Result<SshHandleChain>> + Send + '_>> {
    open_authenticated_ssh_handle_with_forwarded_tx_and_environment(
        config,
        forwarded_tcpip_tx,
        ShellEnvironmentCache::global(),
    )
}

fn open_authenticated_ssh_handle_with_forwarded_tx_and_environment(
    config: &SshSessionConfig,
    forwarded_tcpip_tx: Option<tokio_mpsc::UnboundedSender<ForwardedTcpIpChannel>>,
    shell_environment: Arc<ShellEnvironmentCache>,
) -> Pin<Box<dyn Future<Output = anyhow::Result<SshHandleChain>> + Send + '_>> {
    open_authenticated_ssh_handle_with_channel_senders(
        config,
        forwarded_tcpip_tx,
        None,
        shell_environment,
    )
}

fn open_authenticated_ssh_handle_with_channel_senders(
    config: &SshSessionConfig,
    forwarded_tcpip_tx: Option<tokio_mpsc::UnboundedSender<ForwardedTcpIpChannel>>,
    x11_tx: Option<tokio_mpsc::UnboundedSender<X11ChannelOpen>>,
    shell_environment: Arc<ShellEnvironmentCache>,
) -> Pin<Box<dyn Future<Output = anyhow::Result<SshHandleChain>> + Send + '_>> {
    let forwarded_tcpip = forwarded_tcpip_tx.map(|tx| {
        Arc::new(tokio::sync::Mutex::new(ForwardedTcpIpDispatch {
            fallback: Some(tx),
            by_listener: HashMap::new(),
        }))
    });
    open_authenticated_ssh_handle_with_sender_registry(
        config,
        forwarded_tcpip,
        x11_tx,
        shell_environment,
    )
}

fn open_authenticated_ssh_handle_with_sender_registry(
    config: &SshSessionConfig,
    forwarded_tcpip: Option<ForwardedTcpIpRegistry>,
    x11_tx: Option<tokio_mpsc::UnboundedSender<X11ChannelOpen>>,
    shell_environment: Arc<ShellEnvironmentCache>,
) -> Pin<Box<dyn Future<Output = anyhow::Result<SshHandleChain>> + Send + '_>> {
    Box::pin(async move {
        // Retry is an explicit user action from the prompt. Keep creating a
        // fresh authenticated transport until the user cancels or a
        // non-retryable transport error is returned.
        let mut agent_attempt = 1_u32;
        loop {
            let result = async {
                if let Some(jump_config) = config.proxy_jump.as_deref() {
                    let (jump_handle, mut jump_handles) =
                        open_authenticated_ssh_handle_with_environment(
                            jump_config,
                            shell_environment.clone(),
                        )
                        .await?;
                    let direct_channel = tokio::time::timeout(
                        Duration::from_secs(30),
                        jump_handle.channel_open_direct_tcpip(
                            &config.host,
                            config.port.into(),
                            "127.0.0.1",
                            0,
                        ),
                    )
                    .await
                    .map_err(|_| anyhow::anyhow!("SSH ProxyJump direct-tcpip open timed out"))??;
                    let mut handle = tokio::time::timeout(
                        Duration::from_secs(30),
                        client::connect_stream(
                            ssh_client_config(config)?,
                            direct_channel.into_stream(),
                            SshClientHandler {
                                host: config.host.clone(),
                                port: config.port,
                                verifier: config.host_key_verifier.clone(),
                                forwarded_tcpip: forwarded_tcpip.clone(),
                                x11_tx: x11_tx.clone(),
                                agent_forwarding_config: effective_agent_forwarding_config(config),
                                agent_stored_key_provider: config.agent_stored_key_provider.clone(),
                                shell_environment: shell_environment.clone(),
                            },
                        ),
                    )
                    .await
                    .map_err(|_| anyhow::anyhow!("SSH ProxyJump target connection timed out"))??;
                    let authentication = authenticate_ssh(
                        &mut handle,
                        config,
                        agent_attempt,
                        shell_environment.clone(),
                    )
                    .await;
                    if let Err(error) = &authentication
                        && is_agent_retry(error)
                    {
                        let _ = handle
                            .disconnect(
                                Disconnect::ByApplication,
                                "SSH Agent authentication retry",
                                "en",
                            )
                            .await;
                        let _ = jump_handle
                            .disconnect(
                                Disconnect::ByApplication,
                                "SSH Agent authentication retry",
                                "en",
                            )
                            .await;
                        for jump in &jump_handles {
                            let _ = jump
                                .disconnect(
                                    Disconnect::ByApplication,
                                    "SSH Agent authentication retry",
                                    "en",
                                )
                                .await;
                        }
                    }
                    authentication?;
                    tracing::debug!(
                        stage = "authentication",
                        host = %config.host,
                        port = config.port,
                        profile = ?config.profile,
                        via_jump = true,
                        "SSH authentication completed"
                    );
                    jump_handles.push(jump_handle);
                    Ok((handle, jump_handles))
                } else {
                    let mut handle = tokio::time::timeout(
                        Duration::from_secs(30),
                        connect_ssh_transport(
                            config,
                            forwarded_tcpip.clone(),
                            x11_tx.clone(),
                            shell_environment.clone(),
                        ),
                    )
                    .await
                    .map_err(|_| anyhow::anyhow!("SSH connection timed out"))??;

                    let authentication = authenticate_ssh(
                        &mut handle,
                        config,
                        agent_attempt,
                        shell_environment.clone(),
                    )
                    .await;
                    if let Err(error) = &authentication
                        && is_agent_retry(error)
                    {
                        let _ = handle
                            .disconnect(
                                Disconnect::ByApplication,
                                "SSH Agent authentication retry",
                                "en",
                            )
                            .await;
                    }
                    authentication?;
                    tracing::debug!(
                        stage = "authentication",
                        host = %config.host,
                        port = config.port,
                        profile = ?config.profile,
                        via_jump = false,
                        "SSH authentication completed"
                    );
                    Ok((handle, Vec::new()))
                }
            }
            .await;

            match result {
                Err(error) if is_agent_retry(&error) => {
                    agent_attempt = agent_attempt.saturating_add(1);
                    continue;
                }
                result => return result,
            }
        }
    })
}

async fn connect_ssh_transport(
    config: &SshSessionConfig,
    forwarded_tcpip: Option<ForwardedTcpIpRegistry>,
    x11_tx: Option<tokio_mpsc::UnboundedSender<X11ChannelOpen>>,
    shell_environment: Arc<ShellEnvironmentCache>,
) -> anyhow::Result<client::Handle<SshClientHandler>> {
    let handler = SshClientHandler {
        host: config.host.clone(),
        port: config.port,
        verifier: config.host_key_verifier.clone(),
        forwarded_tcpip,
        x11_tx,
        agent_forwarding_config: effective_agent_forwarding_config(config),
        agent_stored_key_provider: config.agent_stored_key_provider.clone(),
        shell_environment,
    };
    let Some(proxy) = config.proxy.as_ref() else {
        return client::connect(
            ssh_client_config(config)?,
            (config.host.as_str(), config.port),
            handler,
        )
        .await
        .map_err(|error| anyhow::anyhow!("SSH connection failed: {error}"));
    };

    match proxy.protocol.as_str() {
        "socks5" => {
            let proxy_addr = format!("{}:{}", proxy.host, proxy.port);
            let target = (config.host.as_str(), config.port);
            let stream = match (
                proxy.username.as_deref().filter(|value| !value.is_empty()),
                proxy.password.as_deref().filter(|value| !value.is_empty()),
            ) {
                (Some(username), Some(password)) => {
                    tokio_socks::tcp::Socks5Stream::connect_with_password(
                        proxy_addr.as_str(),
                        target,
                        username,
                        password,
                    )
                    .await
                }
                _ => tokio_socks::tcp::Socks5Stream::connect(proxy_addr.as_str(), target).await,
            }
            .map_err(|error| anyhow::anyhow!("SOCKS5 proxy connection failed: {error}"))?;
            client::connect_stream(ssh_client_config(config)?, stream.into_inner(), handler)
                .await
                .map_err(|error| anyhow::anyhow!("SSH connection via SOCKS5 proxy failed: {error}"))
        }
        "http" => {
            let proxy_addr = format!("{}:{}", proxy.host, proxy.port);
            let mut stream = tokio::net::TcpStream::connect(&proxy_addr)
                .await
                .map_err(|error| anyhow::anyhow!("HTTP proxy connection failed: {error}"))?;
            match (
                proxy.username.as_deref().filter(|value| !value.is_empty()),
                proxy.password.as_deref().filter(|value| !value.is_empty()),
            ) {
                (Some(username), Some(password)) => {
                    async_http_proxy::http_connect_tokio_with_basic_auth(
                        &mut stream,
                        &config.host,
                        config.port,
                        username,
                        password,
                    )
                    .await
                }
                _ => {
                    async_http_proxy::http_connect_tokio(&mut stream, &config.host, config.port)
                        .await
                }
            }
            .map_err(|error| anyhow::anyhow!("HTTP proxy tunnel failed: {error}"))?;
            client::connect_stream(ssh_client_config(config)?, stream, handler)
                .await
                .map_err(|error| anyhow::anyhow!("SSH connection via HTTP proxy failed: {error}"))
        }
        "proxycommand" => {
            let stream = open_proxy_command_stream(
                proxy.command.as_deref(),
                &config.host,
                config.port,
                &config.username,
            )
            .await?;
            client::connect_stream(ssh_client_config(config)?, stream, handler)
                .await
                .map_err(|error| anyhow::anyhow!("SSH connection via ProxyCommand failed: {error}"))
        }
        other => anyhow::bail!("unsupported SSH proxy protocol '{other}'"),
    }
}

async fn open_proxy_command_stream(
    template: Option<&str>,
    host: &str,
    port: u16,
    username: &str,
) -> anyhow::Result<ProxyCommandStream> {
    let command = expand_proxy_command(template, host, port, username)?;
    let mut process = system_shell_command(&command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| anyhow::anyhow!("ProxyCommand failed to start: {error}"))?;

    let stdin = process
        .stdin
        .take()
        .ok_or_else(|| anyhow::anyhow!("ProxyCommand stdin unavailable"))?;
    let stdout = process
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("ProxyCommand stdout unavailable"))?;

    if let Some(mut stderr) = process.stderr.take() {
        tokio::spawn(async move {
            let mut buffer = [0_u8; 1024];
            loop {
                match stderr.read(&mut buffer).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
            }
        });
    }

    tokio::spawn(async move {
        let _ = process.wait().await;
    });

    Ok(ProxyCommandStream { stdout, stdin })
}

struct ProxyCommandStream {
    stdout: tokio::process::ChildStdout,
    stdin: tokio::process::ChildStdin,
}

impl AsyncRead for ProxyCommandStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        Pin::new(&mut self.stdout).poll_read(cx, buf)
    }
}

impl AsyncWrite for ProxyCommandStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        Pin::new(&mut self.stdin).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        Pin::new(&mut self.stdin).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        Pin::new(&mut self.stdin).poll_shutdown(cx)
    }
}

fn expand_proxy_command(
    template: Option<&str>,
    host: &str,
    port: u16,
    username: &str,
) -> anyhow::Result<String> {
    let template = template.unwrap_or_default().trim();
    if template.is_empty() {
        anyhow::bail!("ProxyCommand is empty");
    }

    let quoted_host = local_shell_quote(host);
    let port = port.to_string();
    let quoted_port = local_shell_quote(&port);
    let quoted_username = local_shell_quote(username);

    let mut output = String::with_capacity(template.len());
    let mut chars = template.chars();
    while let Some(ch) = chars.next() {
        if ch != '%' {
            output.push(ch);
            continue;
        }
        match chars.next() {
            Some('%') => output.push('%'),
            Some('h') => output.push_str(&quoted_host),
            Some('p') => output.push_str(&quoted_port),
            Some('r') => output.push_str(&quoted_username),
            Some(other) => {
                output.push('%');
                output.push(other);
            }
            None => output.push('%'),
        }
    }

    Ok(output)
}

#[cfg(windows)]
fn system_shell_command(command: &str) -> tokio::process::Command {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let mut cmd = tokio::process::Command::new("cmd");
    cmd.arg("/C").arg(command);
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

#[cfg(not(windows))]
fn system_shell_command(command: &str) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg("-c").arg(command);
    cmd
}

#[cfg(windows)]
fn local_shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "\"\"".to_string();
    }

    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | ':' | '@' | '%'))
    {
        return value.to_string();
    }

    let escaped = value.replace('"', "\"\"");
    format!("\"{escaped}\"")
}

#[cfg(not(windows))]
fn local_shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }

    format!("'{}'", value.replace('\'', "'\\''"))
}

struct SshClientHandler {
    host: String,
    port: u16,
    verifier: Option<Arc<dyn SshHostKeyVerifier>>,
    forwarded_tcpip: Option<ForwardedTcpIpRegistry>,
    x11_tx: Option<tokio_mpsc::UnboundedSender<X11ChannelOpen>>,
    agent_forwarding_config: Option<SshAgentForwardingConfig>,
    agent_stored_key_provider: Option<Arc<dyn SshAgentStoredKeyProvider>>,
    shell_environment: Arc<ShellEnvironmentCache>,
}

impl client::Handler for SshClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &russh::keys::PublicKey,
    ) -> Result<bool, Self::Error> {
        let Some(verifier) = &self.verifier else {
            return Ok(false);
        };
        let host_identifier = ssh_host_identifier(&self.host, self.port);
        let host_key = SshHostKey {
            host: self.host.clone(),
            port: self.port,
            host_identifier,
            key_type: server_public_key.algorithm().to_string(),
            key_base64: server_public_key.public_key_base64(),
            fingerprint: server_public_key
                .fingerprint(Default::default())
                .to_string(),
        };
        match verifier.verify(&host_key) {
            Ok(SshHostKeyDecision::Accept) => Ok(true),
            Ok(SshHostKeyDecision::Reject(_)) | Err(_) => Ok(false),
        }
    }

    async fn server_channel_open_forwarded_tcpip(
        &mut self,
        channel: russh::Channel<client::Msg>,
        connected_address: &str,
        connected_port: u32,
        originator_address: &str,
        originator_port: u32,
        reply: russh::client::ChannelOpenHandle,
        _session: &mut russh::client::Session,
    ) -> Result<(), Self::Error> {
        let Some(registry) = self.forwarded_tcpip.as_ref() else {
            reply
                .reject(russh::ChannelOpenFailure::AdministrativelyProhibited)
                .await;
            return Ok(());
        };
        let dispatch = registry.lock().await;
        let tx = forwarded_tcpip_sender_for(&dispatch, connected_address, connected_port);
        drop(dispatch);
        let Some(tx) = tx else {
            reply
                .reject(russh::ChannelOpenFailure::AdministrativelyProhibited)
                .await;
            return Ok(());
        };
        if tx
            .send(ForwardedTcpIpChannel {
                channel,
                connected_address: connected_address.to_string(),
                connected_port,
                originator_address: originator_address.to_string(),
                originator_port,
            })
            .is_ok()
        {
            reply.accept().await;
        } else {
            reply
                .reject(russh::ChannelOpenFailure::AdministrativelyProhibited)
                .await;
        }
        Ok(())
    }

    async fn server_channel_open_x11(
        &mut self,
        channel: russh::Channel<client::Msg>,
        originator_address: &str,
        originator_port: u32,
        reply: russh::client::ChannelOpenHandle,
        _session: &mut russh::client::Session,
    ) -> Result<(), Self::Error> {
        let Some(tx) = self.x11_tx.as_ref() else {
            reply
                .reject(russh::ChannelOpenFailure::AdministrativelyProhibited)
                .await;
            return Ok(());
        };
        if tx
            .send(X11ChannelOpen {
                channel,
                originator_address: originator_address.to_string(),
                originator_port,
            })
            .is_ok()
        {
            reply.accept().await;
        } else {
            reply
                .reject(russh::ChannelOpenFailure::AdministrativelyProhibited)
                .await;
        }
        Ok(())
    }

    async fn server_channel_open_agent_forward(
        &mut self,
        channel: russh::Channel<client::Msg>,
        reply: russh::client::ChannelOpenHandle,
        _session: &mut russh::client::Session,
    ) -> Result<(), Self::Error> {
        let Some(config) = self.agent_forwarding_config.clone() else {
            reply
                .reject(russh::ChannelOpenFailure::AdministrativelyProhibited)
                .await;
            return Ok(());
        };
        if !config.enabled {
            reply
                .reject(russh::ChannelOpenFailure::AdministrativelyProhibited)
                .await;
            return Ok(());
        }
        let Some(permit) = ssh_agent_broker::try_acquire_agent_channel_permit() else {
            reply
                .reject(russh::ChannelOpenFailure::ResourceShortage)
                .await;
            let _ = channel.close().await;
            return Ok(());
        };
        if is_raw_relay_compatible(&config) {
            let endpoint = config.sources.external_agent_endpoints[0].clone();
            let shell_environment = self.shell_environment.clone();
            tokio::spawn(async move {
                let Ok(agent_stream) = ssh_agent::connect_agent_stream_with_environment(
                    &endpoint,
                    Some(shell_environment),
                )
                .await
                else {
                    reply.reject(russh::ChannelOpenFailure::ConnectFailed).await;
                    let _ = channel.close().await;
                    return;
                };
                reply.accept().await;
                ssh_agent_broker::serve_raw_channel(channel.into_stream(), agent_stream, permit)
                    .await;
            });
            return Ok(());
        }
        let provider = self.agent_stored_key_provider.clone();
        let shell_environment = self.shell_environment.clone();
        tokio::spawn(async move {
            reply.accept().await;
            ssh_agent_broker::serve_channel(
                channel.into_stream(),
                config,
                provider,
                shell_environment,
                permit,
            )
            .await;
        });
        Ok(())
    }
}

fn effective_agent_forwarding_config(
    config: &SshSessionConfig,
) -> Option<SshAgentForwardingConfig> {
    config.agent_forwarding_config.clone().or_else(|| {
        config.agent_forwarding.then(|| SshAgentForwardingConfig {
            enabled: true,
            sources: SshAgentForwardingSources {
                external_agent: true,
                external_agent_endpoints: vec![config.agent_endpoint.clone()],
                stored_keys: false,
            },
            policy: SshAgentForwardingPolicy::All,
        })
    })
}

fn current_agent_stored_key_revision(config: &SshSessionConfig) -> Option<u64> {
    let forwarding = effective_agent_forwarding_config(config)?;
    if !forwarding.enabled || !forwarding.sources.stored_keys {
        return None;
    }
    config
        .agent_stored_key_provider
        .as_ref()
        .and_then(|provider| provider.revision().ok())
}

fn is_raw_relay_compatible(config: &SshAgentForwardingConfig) -> bool {
    config.sources.external_agent
        && config.sources.external_agent_endpoints.len() == 1
        && !config.sources.stored_keys
        && matches!(config.policy, SshAgentForwardingPolicy::All)
}

fn ssh_host_identifier(host: &str, port: u16) -> String {
    if port == 22 {
        host.to_string()
    } else {
        format!("[{host}]:{port}")
    }
}

fn send_session_error(
    event_queue: &SessionEventQueue,
    session_id: &str,
    error: impl std::fmt::Display,
) {
    event_queue.push(SessionEvent::Error {
        session_id: session_id.to_string(),
        message: error.to_string(),
    });
}

fn spawn_serial_reader_thread(
    session_id: String,
    mut reader: Box<dyn SerialPort>,
    stop_reader: Arc<AtomicBool>,
    event_queue: SessionEventQueue,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
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

fn remap_del_to_bs(data: &[u8]) -> Vec<u8> {
    data.iter()
        .map(|byte| if *byte == 0x7f { 0x08 } else { *byte })
        .collect()
}

fn open_serial_port(config: &SerialSessionConfig) -> serialport::Result<Box<dyn SerialPort>> {
    serialport::new(&config.port_name, config.baud_rate)
        .data_bits(parse_data_bits(config.data_bits))
        .parity(parse_parity(&config.parity))
        .stop_bits(parse_stop_bits(&config.stop_bits))
        .flow_control(FlowControl::None)
        .timeout(Duration::from_millis(10))
        .open()
}

fn parse_data_bits(value: u8) -> DataBits {
    match value {
        5 => DataBits::Five,
        6 => DataBits::Six,
        7 => DataBits::Seven,
        _ => DataBits::Eight,
    }
}

fn parse_parity(value: &str) -> Parity {
    match value {
        "odd" => Parity::Odd,
        "even" => Parity::Even,
        _ => Parity::None,
    }
}

fn parse_stop_bits(value: &str) -> StopBits {
    match value {
        "2" => StopBits::Two,
        _ => StopBits::One,
    }
}

const IAC: u8 = 255;
const WILL: u8 = 251;
const WONT: u8 = 252;
const DO: u8 = 253;
const DONT: u8 = 254;
const SB: u8 = 250;
const SE: u8 = 240;
const OPT_ECHO: u8 = 1;
const OPT_SUPPRESS_GO_AHEAD: u8 = 3;
const OPT_NAWS: u8 = 31;

fn negotiate_response(command: u8, option: u8, send_naws: bool, send_sga: bool) -> Vec<u8> {
    match command {
        WILL => {
            if option == OPT_ECHO || (send_sga && option == OPT_SUPPRESS_GO_AHEAD) {
                vec![IAC, DO, option]
            } else {
                vec![IAC, DONT, option]
            }
        }
        DO => {
            if send_naws && option == OPT_NAWS {
                vec![IAC, WILL, option]
            } else {
                vec![IAC, WONT, option]
            }
        }
        WONT => vec![IAC, DONT, option],
        DONT => vec![IAC, WONT, option],
        _ => vec![],
    }
}

fn maybe_build_naws(cols: u16, rows: u16, config: &TelnetSessionConfig) -> Option<Vec<u8>> {
    if config.raw_tcp || !config.send_naws {
        return None;
    }
    Some(vec![
        IAC,
        SB,
        OPT_NAWS,
        (cols >> 8) as u8,
        (cols & 0xff) as u8,
        (rows >> 8) as u8,
        (rows & 0xff) as u8,
        IAC,
        SE,
    ])
}

fn unescape_iac_iac(data: &[u8]) -> Vec<u8> {
    let mut visible = Vec::with_capacity(data.len());
    let mut index = 0;
    while index < data.len() {
        if data[index] == IAC && index + 1 < data.len() && data[index + 1] == IAC {
            visible.push(IAC);
            index += 2;
        } else {
            visible.push(data[index]);
            index += 1;
        }
    }
    visible
}

fn strip_telnet_commands(data: &[u8], on_negotiate: &mut impl FnMut(u8, u8)) -> Vec<u8> {
    let mut visible = Vec::with_capacity(data.len());
    let mut index = 0;
    while index < data.len() {
        if data[index] == IAC && index + 1 < data.len() {
            let command = data[index + 1];
            match command {
                IAC => {
                    visible.push(IAC);
                    index += 2;
                }
                WILL | WONT | DO | DONT => {
                    if index + 2 < data.len() {
                        on_negotiate(command, data[index + 2]);
                        index += 3;
                    } else {
                        index += 2;
                    }
                }
                SB => {
                    index += 2;
                    while index < data.len() {
                        if data[index] == IAC && index + 1 < data.len() && data[index + 1] == SE {
                            index += 2;
                            break;
                        }
                        index += 1;
                    }
                }
                _ => index += 2,
            }
        } else {
            visible.push(data[index]);
            index += 1;
        }
    }
    visible
}

fn normalize_telnet_input(data: &[u8], config: &TelnetSessionConfig) -> Vec<u8> {
    if config.raw_tcp {
        return data.to_vec();
    }
    let newline = match config.enter_mode {
        TelnetEnterMode::Crlf => b"\r\n".as_slice(),
        TelnetEnterMode::Cr => b"\r".as_slice(),
        TelnetEnterMode::Lf => b"\n".as_slice(),
    };
    let mut normalized = Vec::with_capacity(data.len());
    for byte in data {
        match *byte {
            b'\n' | b'\r' => normalized.extend_from_slice(newline),
            IAC => normalized.extend_from_slice(&[IAC, IAC]),
            _ => normalized.push(*byte),
        }
    }
    normalized
}

fn edit_telnet_line_input(
    data: &[u8],
    line_buffer: &mut Vec<u8>,
    config: &TelnetSessionConfig,
) -> (Vec<u8>, Vec<u8>) {
    let mut send = Vec::new();
    let mut echo = Vec::new();
    let mut index = 0;
    while index < data.len() {
        let byte = data[index];
        match byte {
            b'\r' | b'\n' => {
                send.extend_from_slice(line_buffer);
                send.push(byte);
                line_buffer.clear();
                if config.local_echo {
                    echo.extend_from_slice(b"\r\n");
                }
                if byte == b'\r' && index + 1 < data.len() && data[index + 1] == b'\n' {
                    index += 1;
                }
            }
            b'\x08' | b'\x7f' => {
                if line_buffer.pop().is_some() && config.local_echo {
                    echo.extend_from_slice(b"\x08 \x08");
                }
            }
            _ => {
                line_buffer.push(byte);
                if config.local_echo {
                    echo.push(byte);
                }
            }
        }
        index += 1;
    }
    (send, echo)
}

fn telnet_auto_login_line_bytes(value: &str, config: &TelnetSessionConfig) -> Vec<u8> {
    telnet_prompts::telnet_auto_login_line_bytes(value, config, normalize_telnet_input)
}

fn build_command(config: &LocalSessionConfig) -> CommandBuilder {
    let shell = config
        .shell_path
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(default_shell);
    let mut command = CommandBuilder::new(&shell);
    if config.shell_args.is_empty() && cfg!(not(target_os = "windows")) {
        if should_use_interactive_login_args(&shell) {
            command.args(["--login", "-i"]);
        }
    } else {
        command.args(config.shell_args.iter().map(String::as_str));
    }
    command
}

fn configure_environment(command: &mut CommandBuilder) {
    command.env("TERM", "xterm-256color");
    if cfg!(target_os = "macos") {
        command.env("LANG", utf8_env_or("LANG", "en_US.UTF-8"));
        command.env("LC_CTYPE", utf8_env_or("LC_CTYPE", "UTF-8"));
    }
}

fn utf8_env_or(name: &str, fallback: &str) -> String {
    std::env::var(name)
        .ok()
        .filter(|value| {
            let normalized = value.to_ascii_lowercase().replace('_', "-");
            normalized.contains("utf-8") || normalized.contains("utf8")
        })
        .unwrap_or_else(|| fallback.to_string())
}

pub type SharedSessionManager = Arc<SessionManager>;

#[cfg(test)]
mod tests;
