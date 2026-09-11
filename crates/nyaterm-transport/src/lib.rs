pub mod connection_attempt;
pub mod network_route;
use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream};
use std::path::Path;
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
mod file_browser;
mod gpu;
mod local_fs;
mod reconnect_cwd;
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
mod telnet_codec;
mod telnet_prompts;
#[cfg(test)]
use ssh_algorithms::defaults_from_preferred;
use ssh_algorithms::resolve_preferred_algorithms;
use ssh_auth::{authenticate_ssh, is_agent_retry};
#[cfg(test)]
use telnet_codec::{DO, IAC, OPT_SUPPRESS_GO_AHEAD, WILL};
use telnet_codec::{
    edit_telnet_line_input, maybe_build_naws, negotiate_response, normalize_telnet_input,
    strip_telnet_commands, telnet_auto_login_line_bytes, unescape_iac_iac,
};
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
    EnvironmentSnapshot, EnvironmentValue, ShellEnvironmentCache, ShellEnvironmentError,
    normalize_environment_variable_name,
};
pub use file_browser::{
    FileBrowserBackendKind, FileBrowserCapabilities, FileBrowserService, file_browser_join,
    file_browser_name, file_browser_parent, valid_file_browser_child_name,
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
pub use reconnect_cwd::build_ssh_reconnect_cwd_command;
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
    RemoteTextDocument, RemoteTextGeneration, RemoteTextMetadata, RemoteTextRevision,
    RemoteTextWriteResult,
};
pub use remote_process::{
    PROCESS_LIST_SCRIPT, PROCESS_LIST_UNSUPPORTED_ERROR, PROCESS_LIST_UNSUPPORTED_MARKER,
    RemoteCommandOutput, RemoteProcess, SshProcessService, is_process_list_unsupported,
    normalize_process_signal, parse_process_output, run_local_command,
};
pub(crate) use remote_process::{
    PROCESS_TIMEOUT, ensure_remote_command_success, run_ssh_command, run_ssh_exec_operation,
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
    CpuCoreUsage, CpuInfo, CpuUsageSource, DiskInfo, LoadInfo, MemoryInfo, NetworkInfo,
    NetworkSummaryInfo, ParsedRemoteStats, RemoteStats, RemoteStatsSampler, RemoteStatsService,
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
type X11Registry = Arc<tokio::sync::Mutex<Option<X11Registration>>>;

struct SshMultiplexInner {
    docker_elevation: Arc<Mutex<Option<docker::DockerElevation>>>,
    runtime: Arc<tokio::runtime::Runtime>,
    target: SharedSshHandle,
    jumps: Vec<SharedSshHandle>,
    info: SshMultiplexInfo,
    /// The Agent handler is fixed when the transport is created, so multiplex
    /// handles with different forwarding policies must never be shared.
    agent_forwarding_config: Option<SshAgentForwardingConfig>,
    agent_stored_key_revision: Option<u64>,
    forwarded_tcpip: ForwardedTcpIpRegistry,
    x11: X11Registry,
    primary_session: Arc<PrimarySessionGate>,
    closed: AtomicBool,
}

#[derive(Default)]
struct ForwardedTcpIpDispatch {
    fallback: Option<tokio_mpsc::UnboundedSender<ForwardedTcpIpChannel>>,
    by_listener: HashMap<(String, u32), tokio_mpsc::UnboundedSender<ForwardedTcpIpChannel>>,
}

struct X11Registration {
    session_id: String,
    tx: tokio_mpsc::UnboundedSender<X11ChannelOpen>,
}

/// How long an auxiliary channel waits for the interactive shell to take the
/// connection's first session channel before opening anyway.
///
/// `sshd` hands the PAM login message (`pam_motd`, i.e. the whole
/// `/etc/update-motd.d` banner) to the first session channel that runs `do_exec`
/// and clears its buffer afterwards, so only one channel per connection can ever
/// print it. Before multiplexing the terminal owned its connection and always
/// won that slot; now Stats/Docker/GPU/SFTP share the connection and would
/// silently eat the banner if one of them opened a channel first.
const PRIMARY_SESSION_WAIT_TIMEOUT: Duration = Duration::from_secs(10);

/// Holds auxiliary channels back until the interactive shell has requested its
/// channel on a freshly authenticated connection.
struct PrimarySessionGate {
    claimed: AtomicBool,
    /// A zero-permit semaphore, used only for its closed state: closing releases
    /// every waiter, including the ones that start waiting afterwards.
    opened: tokio::sync::Semaphore,
}

impl PrimarySessionGate {
    fn new() -> Self {
        Self {
            claimed: AtomicBool::new(false),
            opened: tokio::sync::Semaphore::new(0),
        }
    }

    /// Take the first-channel slot, or `None` if it is already spoken for.
    ///
    /// Only the connection's first interactive session can claim it; a second
    /// terminal on the same multiplex handle gets `None` and proceeds at once,
    /// which matches OpenSSH's own `ControlMaster` behaviour of printing the
    /// banner once per connection.
    fn claim(gate: &Arc<Self>) -> Option<PrimarySessionClaim> {
        gate.claimed
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
            .then(|| PrimarySessionClaim { gate: gate.clone() })
    }

    async fn wait(&self) {
        if !self.claimed.load(Ordering::SeqCst) {
            return;
        }
        // Permits are never added, so this only resolves once the claim is
        // dropped and closes the semaphore. The timeout keeps a wedged shell
        // from stalling every other feature on the connection forever.
        let _ = tokio::time::timeout(PRIMARY_SESSION_WAIT_TIMEOUT, self.opened.acquire()).await;
    }
}

/// Released once the interactive shell holds its channel -- or has given up --
/// which lets the queued auxiliary channels through.
struct PrimarySessionClaim {
    gate: Arc<PrimarySessionGate>,
}

impl Drop for PrimarySessionClaim {
    fn drop(&mut self) {
        self.gate.opened.close();
    }
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
        if self.inner.closed.swap(true, Ordering::Relaxed) {
            return Ok(());
        }
        let target = self.inner.target.clone();
        let jumps = self.inner.jumps.clone();
        self.inner.runtime.block_on(async move {
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
        })
    }

    /// The shared connection, for channels whose ordering does not matter.
    ///
    /// Only two callers qualify: the interactive terminal, which is the channel
    /// everything else is ordered behind, and `direct-tcpip` forwards, which
    /// never make the server run `do_exec`. Anything that will `exec` a command
    /// or start a subsystem must use [`Self::exec_target_handle`] instead.
    fn target_handle(&self) -> SharedSshHandle {
        self.inner.target.clone()
    }

    /// The shared connection, for a channel that will `exec` or start a
    /// subsystem, once the interactive shell has taken its own channel.
    ///
    /// `sshd` hands the PAM login banner to whichever session channel runs
    /// `do_exec` first and clears its buffer afterwards, so a Stats, Docker or
    /// SFTP channel that opens ahead of the terminal costs the user their whole
    /// MOTD. Waiting here is what keeps the terminal first.
    async fn exec_target_handle(&self) -> SharedSshHandle {
        self.inner.primary_session.wait().await;
        self.inner.target.clone()
    }

    fn forwarded_tcpip_registry(&self) -> ForwardedTcpIpRegistry {
        self.inner.forwarded_tcpip.clone()
    }

    /// Reserve the connection's first session channel for an interactive shell.
    fn claim_primary_session(&self) -> Option<PrimarySessionClaim> {
        PrimarySessionGate::claim(&self.inner.primary_session)
    }

    async fn register_x11_sender(
        &self,
        session_id: &str,
        tx: tokio_mpsc::UnboundedSender<X11ChannelOpen>,
    ) -> anyhow::Result<()> {
        if self.is_closed() {
            anyhow::bail!("SSH multiplex handle is closed");
        }
        register_x11_sender(&self.inner.x11, session_id, tx).await
    }

    async fn unregister_x11_sender(&self, session_id: &str) {
        unregister_x11_sender(&self.inner.x11, session_id).await;
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
}

async fn register_x11_sender(
    registry: &X11Registry,
    session_id: &str,
    tx: tokio_mpsc::UnboundedSender<X11ChannelOpen>,
) -> anyhow::Result<()> {
    let mut registration = registry.lock().await;
    if registration
        .as_ref()
        .is_some_and(|registration| registration.tx.is_closed())
    {
        *registration = None;
    }
    if let Some(registration) = registration.as_ref() {
        if registration.session_id == session_id {
            anyhow::bail!("X11 forwarding is already active for this multiplexed SSH session");
        }
        anyhow::bail!("X11 forwarding is already active for another multiplexed SSH session");
    }
    *registration = Some(X11Registration {
        session_id: session_id.to_string(),
        tx,
    });
    Ok(())
}

async fn unregister_x11_sender(registry: &X11Registry, session_id: &str) {
    let mut registration = registry.lock().await;
    if registration
        .as_ref()
        .is_some_and(|registration| registration.session_id == session_id)
    {
        *registration = None;
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
    let agent_forwarding_config = effective_agent_forwarding_config(&config);
    let agent_stored_key_revision = current_agent_stored_key_revision(&config);
    let runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("nyaterm-ssh-multiplex")
            .build()
            .map_err(|error| anyhow::anyhow!("failed to start SSH multiplex runtime: {error}"))?,
    );
    let forwarded_tcpip = Arc::new(tokio::sync::Mutex::new(ForwardedTcpIpDispatch::default()));
    let x11 = Arc::new(tokio::sync::Mutex::new(None));
    let shell_environment = ShellEnvironmentCache::global();
    let (target, jumps) = runtime.block_on(open_authenticated_ssh_handle_with_sender_registry(
        &config,
        Some(forwarded_tcpip.clone()),
        Some(x11.clone()),
        shell_environment,
    ))?;
    let info = SshMultiplexInfo {
        name: config.name,
        host: config.host,
        port: config.port,
        username: config.username,
        proxy: config.proxy,
        jump_count: jumps.len(),
    };
    Ok(SshMultiplexHandle {
        inner: Arc::new(SshMultiplexInner {
            docker_elevation: Default::default(),
            runtime,
            target: Arc::new(tokio::sync::Mutex::new(target)),
            jumps: jumps
                .into_iter()
                .map(|jump| Arc::new(tokio::sync::Mutex::new(jump)))
                .collect(),
            info,
            agent_forwarding_config,
            agent_stored_key_revision,
            forwarded_tcpip,
            x11,
            primary_session: Arc::new(PrimarySessionGate::new()),
            closed: AtomicBool::new(false),
        }),
    })
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

pub struct SessionManager {
    sessions: Mutex<HashMap<String, ManagedSession>>,
    event_queue: SessionEventQueue,
    shell_environment: Arc<ShellEnvironmentCache>,
}

enum ManagedSession {
    Local(LocalPtyTransport),
    Ssh(SshChannelTransport),
    Tcp(Box<TelnetTransport>),
    Serial(SerialTransport),
}

pub struct LocalPtyTransport {
    info: SessionInfo,
    master: Option<Box<dyn MasterPty + Send>>,
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
    handle: SshShellHandle,
    attempt: connection_attempt::ConnectionAttempt,
    post_login: Option<nyaterm_core::models::sessions::ConnectionPostLogin>,
    channel: russh::Channel<client::Msg>,
    jump_handles: Vec<client::Handle<SshClientHandler>>,
    disconnect_on_close: bool,
    x11_forwarder: Option<X11Forwarder>,
    x11_multiplex_registration: Option<SshMultiplexHandle>,
    local_notice: Option<Vec<u8>>,
    injection_script: Option<Vec<u8>>,
    ready_marker: String,
    legacy_ready_marker: Option<String>,
    shell_kind: Option<ShellKind>,
}

enum SshShellHandle {
    Dedicated(client::Handle<SshClientHandler>),
    Multiplexed(SharedSshHandle),
}

struct PendingOpenSshShellSession {
    handle: SshShellHandle,
    jump_handles: Vec<client::Handle<SshClientHandler>>,
    disconnect_on_close: bool,
    multiplex: Option<SshMultiplexHandle>,
    /// Held from before the deferred-PTY wait until the shell request lands, so
    /// no shared-connection feature can open a channel ahead of the terminal.
    primary_claim: Option<PrimarySessionClaim>,
    x11_config: Option<X11ForwardingConfig>,
    x11_tx: Option<tokio_mpsc::UnboundedSender<X11ChannelOpen>>,
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

    /// Return the runtime-only shell environment cache shared by SSH tasks.
    pub fn shell_environment(&self) -> Arc<ShellEnvironmentCache> {
        Arc::clone(&self.shell_environment)
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
        // Startup asynchronously warms the complete shell environment. Copy
        // values from the shared snapshot here without starting another shell.
        // If a terminal is created before warm-up finishes, retain portable-pty's
        // inherited environment as a non-blocking fallback.
        let requested_shell = config
            .shell_path
            .as_deref()
            .filter(|path| !path.trim().is_empty())
            .map(str::to_string)
            .unwrap_or_else(default_local_shell_path);
        let shell_snapshot = self
            .shell_environment
            .snapshot()
            .ok()
            .flatten()
            .filter(|snapshot| snapshot.matches_shell_path(Some(Path::new(&requested_shell))));
        configure_environment(&mut command, shell_snapshot.as_deref());
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
            master: Some(pair.master),
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
        self.create_telnet_session_with_attempt(config, Default::default())
    }

    pub fn create_telnet_session_with_attempt(
        &self,
        config: TelnetSessionConfig,
        attempt: connection_attempt::ConnectionAttempt,
    ) -> Result<SessionInfo, SessionError> {
        let session_id = uuid::Uuid::new_v4().to_string();
        let addr = format!("{}:{}", config.host, config.port);
        let connect = || -> Result<TcpStream, std::io::Error> {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            let stream = runtime
                .block_on(attempt.until_cancelled(tokio::net::TcpStream::connect((
                    config.host.as_str(),
                    config.port,
                ))))
                .map_err(std::io::Error::other)??;
            let stream = stream.into_std()?;
            stream.set_nonblocking(false)?;
            Ok(stream)
        };
        let stream = connect().map_err(|source| SessionError::ConnectTcp {
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
            backspace_as_bs: nyaterm_core::terminal::connection_input::BackspaceMode::parse(
                &config.backspace_mode,
            )
                == nyaterm_core::terminal::connection_input::BackspaceMode::Backspace,
            local_line_buffer: Vec::new(),
            auto_login,
            event_queue: self.event_queue.clone(),
            config,
            reader_thread: Some(reader_thread),
        };

        self.sessions
            .lock()
            .map_err(|_| SessionError::LockPoisoned)?
            .insert(session_id, ManagedSession::Tcp(Box::new(session)));

        Ok(info)
    }

    pub fn create_ssh_session(
        &self,
        config: SshSessionConfig,
    ) -> Result<SessionInfo, SessionError> {
        self.create_ssh_session_inner(config, None)
    }

    pub fn create_ssh_session_with_multiplex(
        &self,
        config: SshSessionConfig,
        multiplex: SshMultiplexHandle,
    ) -> Result<SessionInfo, SessionError> {
        multiplex
            .ensure_matches_config(&config)
            .map_err(|source| SessionError::CreateSsh {
                addr: format!("{}:{}", config.host, config.port),
                source,
            })?;
        self.create_ssh_session_inner(config, Some(multiplex))
    }

    fn create_ssh_session_inner(
        &self,
        config: SshSessionConfig,
        multiplex: Option<SshMultiplexHandle>,
    ) -> Result<SessionInfo, SessionError> {
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
        let event_queue = self.event_queue.clone();
        let worker_config = config.clone();
        let worker_session_id = session_id.clone();
        let shell_environment = self.shell_environment();
        let worker_thread = std::thread::spawn(move || {
            run_ssh_worker(
                worker_session_id,
                worker_config,
                command_rx,
                ready_tx,
                event_queue,
                multiplex,
                shell_environment,
            );
        });

        match ready_rx.recv() {
            Ok(Ok(())) => {}
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
        }

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
            backspace_as_bs: nyaterm_core::terminal::connection_input::BackspaceMode::parse(
                &config.backspace_mode,
            )
                == nyaterm_core::terminal::connection_input::BackspaceMode::Backspace,
            worker_thread: Some(worker_thread),
        };

        self.sessions
            .lock()
            .map_err(|_| SessionError::LockPoisoned)?
            .insert(session_id, ManagedSession::Ssh(session));

        Ok(info)
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
            backspace_as_bs: nyaterm_core::terminal::connection_input::BackspaceMode::parse(
                &config.backspace_mode,
            )
                == nyaterm_core::terminal::connection_input::BackspaceMode::Backspace,
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
        self.event_queue.cancel_session(session_id);
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
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("local PTY is closed"))?
            .resize(local_pty_size(cols, rows, pixel_width, pixel_height))?;
        self.info.cols = cols;
        self.info.rows = rows;
        Ok(())
    }

    fn close(&mut self) -> anyhow::Result<()> {
        let _ = self.child.kill();
        self.writer.close();
        // On Windows, portable-pty's ConPTY reader is a clone of a pipe owned by
        // the master. Killing the child does not guarantee EOF while that master
        // still owns the pseudo console, so release it before joining the reader.
        drop(self.master.take());
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
            let visible_echo = if self.config.local_echo {
                data.clone()
            } else {
                Vec::new()
            };
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

impl Drop for SessionManager {
    fn drop(&mut self) {
        self.event_queue.close();
        let sessions = match self.sessions.get_mut() {
            Ok(sessions) => std::mem::take(sessions),
            Err(poisoned) => std::mem::take(poisoned.into_inner()),
        };
        for (_, mut session) in sessions {
            session.close();
        }
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
    password: Option<nyaterm_core::SecretString>,
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
    ready_tx: mpsc::Sender<Result<(), String>>,
    event_queue: SessionEventQueue,
    multiplex: Option<SshMultiplexHandle>,
    shell_environment: Arc<ShellEnvironmentCache>,
) {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("nyaterm-ssh")
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            let _ = ready_tx.send(Err(format!("failed to start SSH runtime: {error}")));
            return;
        }
    };

    runtime.block_on(async move {
        if config.deferred_pty {
            run_deferred_ssh_worker(
                session_id,
                config,
                command_rx,
                ready_tx,
                event_queue,
                multiplex,
                shell_environment,
            )
            .await;
            return;
        }

        let open_session =
            match open_ssh_shell(&session_id, &config, multiplex.as_ref(), shell_environment).await
            {
                Ok(session) => {
                    let _ = ready_tx.send(Ok(()));
                    session
                }
                Err(error) => {
                    let _ = ready_tx.send(Err(error.to_string()));
                    return;
                }
            };
        run_open_ssh_shell_session(
            session_id,
            open_session,
            command_rx,
            event_queue,
            VecDeque::new(),
        )
        .await;
    });
}

async fn run_deferred_ssh_worker(
    session_id: String,
    config: SshSessionConfig,
    mut command_rx: tokio_mpsc::UnboundedReceiver<SshCommand>,
    ready_tx: mpsc::Sender<Result<(), String>>,
    event_queue: SessionEventQueue,
    multiplex: Option<SshMultiplexHandle>,
    shell_environment: Arc<ShellEnvironmentCache>,
) {
    let pending_session =
        match open_pending_ssh_shell(&config, multiplex.as_ref(), shell_environment).await {
            Ok(session) => {
                let _ = ready_tx.send(Ok(()));
                session
            }
            Err(error) => {
                let _ = ready_tx.send(Err(error.to_string()));
                return;
            }
        };
    let mut pending_session = Some(pending_session);
    let mut dimensions = SshPtyDimensions::from_config(&config);
    let mut pending_writes = VecDeque::new();
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
                            disconnect_pending_ssh_shell(session).await;
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

    let Some(pending_session) = pending_session.take() else {
        return;
    };
    if drain_deferred_ssh_open_commands(&mut command_rx, &mut dimensions, &mut pending_writes) {
        disconnect_pending_ssh_shell(pending_session).await;
        return;
    }
    match open_ssh_shell_from_pending(&session_id, &config, pending_session, dimensions).await {
        Ok(open_session) => {
            run_open_ssh_shell_session(
                session_id,
                open_session,
                command_rx,
                event_queue,
                pending_writes,
            )
            .await;
        }
        Err(error) => {
            send_session_error(&event_queue, &session_id, error);
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

struct PendingSshIntegrationUpload {
    script: Vec<u8>,
    cancel: Option<tokio::sync::oneshot::Sender<()>>,
    join: tokio::task::JoinHandle<ssh_shell_integration::ScriptUploadOutcome>,
}

async fn cancel_pending_upload(pending_upload: &mut Option<PendingSshIntegrationUpload>) {
    let Some(mut pending) = pending_upload.take() else {
        return;
    };

    if let Some(cancel) = pending.cancel.take() {
        let _ = cancel.send(());
    }

    let _ = tokio::time::timeout(Duration::from_secs(1), &mut pending.join).await;
}

async fn run_open_ssh_shell_session(
    session_id: String,
    open_session: OpenSshShellSession,
    mut command_rx: tokio_mpsc::UnboundedReceiver<SshCommand>,
    event_queue: SessionEventQueue,
    mut pending_writes: VecDeque<Vec<u8>>,
) {
    let OpenSshShellSession {
        attempt,
        mut post_login,
        handle,
        mut channel,
        jump_handles,
        disconnect_on_close,
        x11_forwarder,
        x11_multiplex_registration,
        local_notice,
        injection_script,
        ready_marker,
        legacy_ready_marker,
        shell_kind: _shell_kind,
    } = open_session;
    let handle = Arc::new(handle);
    let mut shell_integration =
        SshShellIntegrationState::new(injection_script, ready_marker, legacy_ready_marker);
    if let Some(notice) = local_notice {
        event_queue.push(SessionEvent::Output {
            session_id: session_id.clone(),
            data: notice,
        });
    }
    if let Some(forwarder) = x11_forwarder {
        spawn_x11_forwarder(event_queue.clone(), session_id.clone(), forwarder);
    }

    if shell_integration.is_normal() {
        while let Some(data) = pending_writes.pop_front() {
            if let Err(error) = channel.data_bytes(data).await {
                send_session_error(&event_queue, &session_id, error);
                disconnect_open_ssh_shell(
                    &session_id,
                    &handle,
                    jump_handles,
                    disconnect_on_close,
                    x11_multiplex_registration,
                )
                .await;
                return;
            }
        }
    }

    let initial_inject_delay = tokio::time::sleep(Duration::from_millis(500));
    tokio::pin!(initial_inject_delay);
    let inject_timeout = tokio::time::sleep(ssh_shell_integration::SSH_INTEGRATION_TIMEOUT);
    tokio::pin!(inject_timeout);

    let mut pending_upload: Option<PendingSshIntegrationUpload> = None;

    post_login = post_login.filter(|command| command.enabled && !command.command.trim().is_empty());
    let post_login_timer = tokio::time::sleep(Duration::ZERO);
    tokio::pin!(post_login_timer);
    let mut post_login_armed = false;

    loop {
        if !post_login_armed
            && shell_integration.is_normal()
            && let Some(command) = post_login.as_ref()
        {
            post_login_timer.as_mut().reset(
                tokio::time::Instant::now() + Duration::from_millis(command.delay_ms.min(60_000)),
            );
            post_login_armed = true;
        }
        tokio::select! {
            _ = async { while !attempt.is_cancelled() { tokio::time::sleep(Duration::from_millis(25)).await; } } => break,
            _ = &mut post_login_timer, if post_login_armed && post_login.is_some() => {
                if let Some(command) = post_login.take() {
                    let mut text = command.command.replace("\r\n", "\r").replace('\n', "\r");
                    if !text.ends_with('\r') { text.push('\r'); }
                    if let Err(error) = channel.data_bytes(text.into_bytes()).await {
                        send_session_error(&event_queue, &session_id, error);
                    }
                }
            }
            _ = &mut initial_inject_delay, if shell_integration.should_inject_on_initial_delay() => {
                if let Some(script) = shell_integration.take_pending_script() {
                    shell_integration.begin_suppression();
                    inject_timeout
                        .as_mut()
                        .reset(
                            tokio::time::Instant::now()
                                + ssh_shell_integration::SSH_INTEGRATION_TIMEOUT,
                        );
                    let task_handle = Arc::clone(&handle);
                    let task_script = script.clone();
                    let (cancel, cancel_rx) = tokio::sync::oneshot::channel();
                    let join = tokio::spawn(ssh_shell_integration::upload_integration_script(
                        task_handle,
                        task_script,
                        cancel_rx,
                    ));
                    pending_upload = Some(PendingSshIntegrationUpload {
                        script,
                        cancel: Some(cancel),
                        join,
                    });
                }
            }
            outcome = async {
                match pending_upload.as_mut() {
                    Some(pending) => (&mut pending.join).await,
                    None => std::future::pending().await,
                }
            }, if pending_upload.is_some() => {
                let pending = pending_upload
                    .take()
                    .expect("pending_upload checked Some by the select guard");
                let outcome = outcome.unwrap_or_else(|_join_error| {
                    // The upload task panicked or was cancelled; treat it as
                    // a failed upload so we fall back to the direct-write
                    // path below instead of hanging the injection forever.
                    ssh_shell_integration::ScriptUploadOutcome::failed()
                });
                let script = pending.script;
                shell_integration
                    .apply_upload_outcome(outcome, script, &mut channel)
                    .await;
                inject_timeout
                    .as_mut()
                    .reset(
                        tokio::time::Instant::now()
                            + ssh_shell_integration::SSH_INTEGRATION_TIMEOUT,
                    );
                if shell_integration.is_normal() {
                    while let Some(data) = pending_writes.pop_front() {
                        if let Err(error) = channel.data_bytes(data).await {
                            send_session_error(&event_queue, &session_id, error);
                            break;
                        }
                    }
                }
            }
            _ = &mut inject_timeout, if shell_integration.is_suppressing() => {
                let output = shell_integration.force_normal_after_timeout();
                push_ssh_integration_output(&event_queue, &session_id, output);
                while let Some(data) = pending_writes.pop_front() {
                    if let Err(error) = channel.data_bytes(data).await {
                        send_session_error(&event_queue, &session_id, error);
                        break;
                    }
                }
            }
            command = command_rx.recv() => {
                match command {
                    Some(SshCommand::Write(data)) => {
                        if !shell_integration.is_normal() {
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
            message = channel.wait() => {
                match message {
                    Some(ChannelMsg::Data { data }) | Some(ChannelMsg::ExtendedData { data, .. }) => {
                        let was_waiting_initial = shell_integration.is_waiting_initial();
                        let output = shell_integration.filter_output(&data);
                        push_ssh_integration_output(&event_queue, &session_id, output);
                        if was_waiting_initial
                            && shell_integration.is_waiting_initial()
                            && let Some(script) = shell_integration.take_pending_script() {
                            shell_integration.begin_suppression();
                            inject_timeout
                                .as_mut()
                                .reset(
                                    tokio::time::Instant::now()
                                        + ssh_shell_integration::SSH_INTEGRATION_TIMEOUT,
                                );
                            let task_handle = Arc::clone(&handle);
                            let task_script = script.clone();
                            let (cancel, cancel_rx) = tokio::sync::oneshot::channel();
                            let join = tokio::spawn(ssh_shell_integration::upload_integration_script(
                                task_handle,
                                task_script,
                                cancel_rx,
                            ));
                            pending_upload = Some(PendingSshIntegrationUpload {
                                script,
                                cancel: Some(cancel),
                                join,
                            });
                        }
                        if shell_integration.is_normal() {
                            while let Some(data) = pending_writes.pop_front() {
                                if let Err(error) = channel.data_bytes(data).await {
                                    send_session_error(&event_queue, &session_id, error);
                                    break;
                                }
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
        }
    }
    cancel_pending_upload(&mut pending_upload).await;

    disconnect_open_ssh_shell(
        &session_id,
        &handle,
        jump_handles,
        disconnect_on_close,
        x11_multiplex_registration,
    )
    .await;
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

async fn open_ssh_shell(
    session_id: &str,
    config: &SshSessionConfig,
    multiplex: Option<&SshMultiplexHandle>,
    shell_environment: Arc<ShellEnvironmentCache>,
) -> anyhow::Result<OpenSshShellSession> {
    let pending = open_pending_ssh_shell(config, multiplex, shell_environment).await?;
    open_ssh_shell_from_pending(
        session_id,
        config,
        pending,
        SshPtyDimensions::from_config(config),
    )
    .await
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
    let mut pending_multiplex = None;
    let mut primary_claim = None;
    let (handle, jump_handles, disconnect_on_close) = if let Some(multiplex) = multiplex {
        multiplex.ensure_matches_config(config)?;
        pending_multiplex = Some(multiplex.clone());
        // Claimed here rather than next to the channel open: the desktop reports
        // the session ready as soon as this returns, and with a deferred PTY the
        // channel is not opened until the first resize arrives.
        primary_claim = multiplex.claim_primary_session();
        let handle = multiplex.target_handle();
        (SshShellHandle::Multiplexed(handle), Vec::new(), false)
    } else {
        let (handle, jump_handles) = open_authenticated_ssh_handle_with_channel_senders(
            config,
            None,
            x11_tx.clone(),
            shell_environment,
        )
        .await?;
        (SshShellHandle::Dedicated(handle), jump_handles, true)
    };

    Ok(PendingOpenSshShellSession {
        handle,
        jump_handles,
        disconnect_on_close,
        multiplex: pending_multiplex,
        primary_claim,
        x11_config,
        x11_tx,
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
        multiplex,
        primary_claim,
        x11_config,
        x11_tx,
        x11_rx,
    } = pending;
    let ready_marker = build_ssh_ready_marker(session_id);
    let legacy_ready_marker = build_legacy_ssh_ready_marker(&ready_marker);
    let channel = match &mut handle {
        SshShellHandle::Dedicated(handle) => handle.channel_open_session().await?,
        SshShellHandle::Multiplexed(handle) => handle.lock().await.channel_open_session().await?,
    };
    if effective_agent_forwarding_config(config).is_some_and(|forwarding| forwarding.enabled)
        && let Err(error) = channel.agent_forward(false).await
    {
        let _ = channel.close().await;
        return Err(error.into());
    }
    tracing::debug!(
        stage = "interactive-channel",
        host = %config.host,
        port = config.port,
        profile = ?config.profile,
        "opened SSH session channel"
    );
    let (x11_forwarder, x11_multiplex_registration, local_notice) =
        if let (Some(config), Some(rx)) = (x11_config, x11_rx) {
            if let Some(multiplex) = multiplex.as_ref() {
                let Some(tx) = x11_tx.clone() else {
                    let _ = channel.close().await;
                    anyhow::bail!("X11 forwarding sender is unavailable");
                };
                if let Err(error) = multiplex.register_x11_sender(session_id, tx).await {
                    let _ = channel.close().await;
                    return Err(error);
                }
            }
            match channel
                .request_x11(true, false, MIT_MAGIC_COOKIE, &config.fake_cookie_hex, 0)
                .await
            {
                Ok(()) => (Some(X11Forwarder { rx, config }), multiplex.clone(), None),
                Err(_) => {
                    if let Some(multiplex) = multiplex.as_ref() {
                        multiplex.unregister_x11_sender(session_id).await;
                    }
                    (None, None, Some(enable_x11_failed_message().into_bytes()))
                }
            }
        } else {
            (None, None, None)
        };
    if let Err(error) = channel
        .request_pty(
            false,
            &config.term,
            dimensions.cols.into(),
            dimensions.rows.into(),
            dimensions.pixel_width.into(),
            dimensions.pixel_height.into(),
            &[],
        )
        .await
    {
        if let Some(multiplex) = x11_multiplex_registration.as_ref() {
            multiplex.unregister_x11_sender(session_id).await;
        }
        let _ = channel.close().await;
        return Err(error.into());
    }
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
    if let Err(error) = channel.request_shell(true).await {
        if let Some(multiplex) = x11_multiplex_registration.as_ref() {
            multiplex.unregister_x11_sender(session_id).await;
        }
        let _ = channel.close().await;
        return Err(error.into());
    }
    // `request_shell` waited for the server's reply, so `sshd` has already run
    // `do_exec` for this channel and flushed the login banner into it. Anything
    // else on this connection is free to open its own channel now.
    drop(primary_claim);
    tracing::debug!(
        stage = "shell",
        host = %config.host,
        port = config.port,
        profile = ?config.profile,
        "SSH shell accepted"
    );
    let cwd_follow_mode = config.effective_cwd_follow_mode();
    let terminal_shell_integration = config.effective_terminal_shell_integration();
    let shell_kind =
        if terminal_shell_integration || !matches!(cwd_follow_mode, SftpCwdFollowMode::Off) {
            detect_ssh_shell_type(
                &handle,
                config.shell_integration_detection_timeout_ms(cwd_follow_mode),
            )
            .await
        } else {
            None
        };
    let injection_script = match shell_kind {
        Some(kind) => {
            build_ssh_shell_integration_script(
                &handle,
                kind,
                &ready_marker,
                terminal_shell_integration,
                cwd_follow_mode,
                config.sftp.shell_detection_timeout_ms,
            )
            .await
        }
        None => None,
    };
    tracing::debug!(
        stage = "integration",
        host = %config.host,
        port = config.port,
        profile = ?config.profile,
        cwd_follow_mode = ?cwd_follow_mode,
        shell_detected = shell_kind.is_some(),
        integration_enabled = injection_script.is_some(),
        "resolved SSH shell integration"
    );
    Ok(OpenSshShellSession {
        attempt: config.attempt.clone(),
        post_login: config.post_login.clone(),
        handle,
        channel,
        jump_handles,
        disconnect_on_close,
        x11_forwarder,
        x11_multiplex_registration,
        local_notice,
        injection_script,
        ready_marker,
        legacy_ready_marker,
        shell_kind,
    })
}

async fn disconnect_pending_ssh_shell(session: PendingOpenSshShellSession) {
    if session.disconnect_on_close {
        if let SshShellHandle::Dedicated(handle) = session.handle {
            let _ = handle
                .disconnect(Disconnect::ByApplication, "session closed", "en")
                .await;
        }
        for jump_handle in session.jump_handles {
            let _ = jump_handle
                .disconnect(Disconnect::ByApplication, "session closed", "en")
                .await;
        }
    }
}

async fn disconnect_open_ssh_shell(
    session_id: &str,
    handle: &SshShellHandle,
    jump_handles: Vec<client::Handle<SshClientHandler>>,
    disconnect_on_close: bool,
    x11_multiplex_registration: Option<SshMultiplexHandle>,
) {
    if let Some(multiplex) = x11_multiplex_registration {
        multiplex.unregister_x11_sender(session_id).await;
    }
    if disconnect_on_close {
        if let SshShellHandle::Dedicated(handle) = handle {
            let _ = handle
                .disconnect(Disconnect::ByApplication, "session closed", "en")
                .await;
        }
        for jump_handle in jump_handles {
            let _ = jump_handle
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
    let keepalive_interval = if config.keep_alive_interval_secs == 0
        || config.keep_alive_mode
            == nyaterm_core::terminal::connection_input::KeepaliveMode::Disabled
    {
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
        keepalive_mode: match config.keep_alive_mode {
            nyaterm_core::terminal::connection_input::KeepaliveMode::Strict => {
                russh::client::KeepaliveMode::Strict
            }
            _ => russh::client::KeepaliveMode::Compatible,
        },
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
    open_authenticated_ssh_handle_with_channel_senders(
        config,
        None,
        None,
        ShellEnvironmentCache::global(),
    )
}

fn open_authenticated_ssh_handle_with_forwarded_tx(
    config: &SshSessionConfig,
    forwarded_tcpip_tx: Option<tokio_mpsc::UnboundedSender<ForwardedTcpIpChannel>>,
) -> Pin<Box<dyn Future<Output = anyhow::Result<SshHandleChain>> + Send + '_>> {
    open_authenticated_ssh_handle_with_channel_senders(
        config,
        forwarded_tcpip_tx,
        None,
        ShellEnvironmentCache::global(),
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
    let x11 = x11_tx.map(|tx| {
        Arc::new(tokio::sync::Mutex::new(Some(X11Registration {
            session_id: String::new(),
            tx,
        })))
    });
    open_authenticated_ssh_handle_with_sender_registry(
        config,
        forwarded_tcpip,
        x11,
        shell_environment,
    )
}

fn open_authenticated_ssh_handle_with_sender_registry(
    config: &SshSessionConfig,
    forwarded_tcpip: Option<ForwardedTcpIpRegistry>,
    x11: Option<X11Registry>,
    shell_environment: Arc<ShellEnvironmentCache>,
) -> Pin<Box<dyn Future<Output = anyhow::Result<SshHandleChain>> + Send + '_>> {
    Box::pin(async move {
        config
            .attempt
            .until_cancelled(async {
                const MAX_AGENT_ATTEMPTS: u32 = 3;
                let mut agent_attempt = 1;
                loop {
                    match open_authenticated_ssh_handle_once(
                        config,
                        forwarded_tcpip.clone(),
                        x11.clone(),
                        Arc::clone(&shell_environment),
                        agent_attempt,
                    )
                    .await
                    {
                        Err(error)
                            if is_agent_retry(&error) && agent_attempt < MAX_AGENT_ATTEMPTS =>
                        {
                            agent_attempt += 1;
                        }
                        Err(error) if is_agent_retry(&error) => {
                            return Err(anyhow::anyhow!(
                                "SSH Agent authentication failed after {agent_attempt} attempts"
                            ));
                        }
                        result => return result,
                    }
                }
            })
            .await
            .map_err(anyhow::Error::msg)?
    })
}

async fn open_authenticated_ssh_handle_once(
    config: &SshSessionConfig,
    forwarded_tcpip: Option<ForwardedTcpIpRegistry>,
    x11: Option<X11Registry>,
    shell_environment: Arc<ShellEnvironmentCache>,
    agent_attempt: u32,
) -> anyhow::Result<SshHandleChain> {
    if let Some(jump_config) = config.proxy_jump.as_deref() {
        let (jump_handle, mut jump_handles) = Box::pin(open_authenticated_ssh_handle_once(
            jump_config,
            None,
            None,
            Arc::clone(&shell_environment),
            agent_attempt,
        ))
        .await?;
        let direct_channel = tokio::time::timeout(
            Duration::from_secs(30),
            jump_handle.channel_open_direct_tcpip(&config.host, config.port.into(), "127.0.0.1", 0),
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
                    x11: x11.clone(),
                    agent_forwarding_config: effective_agent_forwarding_config(config),
                    agent_stored_key_provider: config.agent_stored_key_provider.clone(),
                    shell_environment: Arc::clone(&shell_environment),
                },
            ),
        )
        .await
        .map_err(|_| anyhow::anyhow!("SSH ProxyJump target connection timed out"))??;
        authenticate_ssh(
            &mut handle,
            config,
            agent_attempt,
            Arc::clone(&shell_environment),
        )
        .await?;
        tracing::debug!(
            stage = "authentication",
            host = %config.host,
            port = config.port,
            profile = ?config.profile,
            via_jump = true,
            "SSH authentication completed"
        );
        jump_handles.push(jump_handle);
        return Ok((handle, jump_handles));
    }

    let mut handle = tokio::time::timeout(
        Duration::from_secs(30),
        connect_ssh_transport(
            config,
            forwarded_tcpip.clone(),
            x11.clone(),
            Arc::clone(&shell_environment),
        ),
    )
    .await
    .map_err(|_| anyhow::anyhow!("SSH connection timed out"))??;

    authenticate_ssh(&mut handle, config, agent_attempt, shell_environment).await?;
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

async fn connect_ssh_transport(
    config: &SshSessionConfig,
    forwarded_tcpip: Option<ForwardedTcpIpRegistry>,
    x11: Option<X11Registry>,
    shell_environment: Arc<ShellEnvironmentCache>,
) -> anyhow::Result<client::Handle<SshClientHandler>> {
    let handler = SshClientHandler {
        host: config.host.clone(),
        port: config.port,
        verifier: config.host_key_verifier.clone(),
        forwarded_tcpip,
        x11,
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
    x11: Option<X11Registry>,
    agent_forwarding_config: Option<SshAgentForwardingConfig>,
    agent_stored_key_provider: Option<Arc<dyn SshAgentStoredKeyProvider>>,
    shell_environment: Arc<ShellEnvironmentCache>,
}

impl client::Handler for SshClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &russh::keys::PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        // A host certificate replaces the host-key check rather than adding to it,
        // and known hosts records plain host keys only, so there is nothing to
        // compare a certificate against. Reject instead of falling back to the key
        // inside it: that key is not what the user was asked to trust. Unreachable
        // in practice, because the client never offers certificate host-key
        // algorithms (`Preferred::host_key_certificates` is left empty).
        let russh::keys::PublicKeyOrCertificate::PublicKey {
            key: server_public_key,
            ..
        } = server_public_key
        else {
            return Ok(false);
        };
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
        let Some(registry) = self.x11.as_ref() else {
            reply
                .reject(russh::ChannelOpenFailure::AdministrativelyProhibited)
                .await;
            return Ok(());
        };
        let mut registration = registry.lock().await;
        if registration
            .as_ref()
            .is_some_and(|registration| registration.tx.is_closed())
        {
            *registration = None;
        }
        let Some(tx) = registration
            .as_ref()
            .map(|registration| registration.tx.clone())
        else {
            reply
                .reject(russh::ChannelOpenFailure::AdministrativelyProhibited)
                .await;
            return Ok(());
        };
        drop(registration);
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
            let shell_environment = Arc::clone(&self.shell_environment);
            tokio::spawn(async move {
                let Ok(agent_stream) = ssh_agent::connect_agent_stream_with_environment(
                    &endpoint,
                    Some(shell_environment.clone()),
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
        let shell_environment = Arc::clone(&self.shell_environment);
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

fn build_command(config: &LocalSessionConfig) -> CommandBuilder {
    let shell = config
        .shell_path
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(default_local_shell_path);
    let mut command = CommandBuilder::new(&shell);
    if config.shell_args.is_empty() {
        #[cfg(not(windows))]
        if should_use_interactive_login_args(&shell) {
            command.args(["--login", "-i"]);
        }
    } else {
        command.args(config.shell_args.iter().map(String::as_str));
    }
    command
}

fn configure_environment(
    command: &mut CommandBuilder,
    shell_snapshot: Option<&EnvironmentSnapshot>,
) {
    if let Some(shell_snapshot) = shell_snapshot {
        // Clear portable-pty's inherited process environment only after a shell
        // query succeeds and can be represented completely. This prevents
        // variables removed by the shell (especially stale SSH agent paths)
        // from leaking through, while preserving non-UTF-8 values that may be
        // omitted by the inherited fallback snapshot.
        if shell_snapshot.replaces_inherited_environment() {
            command.env_clear();
        }
        for (variable, value) in shell_snapshot.iter() {
            command.env(variable, value.as_str());
        }
    }
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

#[cfg(windows)]
fn default_local_shell_path() -> String {
    std::env::var("COMSPEC")
        .ok()
        .filter(|path| !path.trim().is_empty())
        .unwrap_or_else(|| "cmd.exe".to_string())
}

#[cfg(not(windows))]
fn default_local_shell_path() -> String {
    std::env::var("SHELL")
        .ok()
        .filter(|path| !path.trim().is_empty())
        .unwrap_or_else(|| "/bin/sh".to_string())
}

#[cfg(not(windows))]
fn should_use_interactive_login_args(program: &str) -> bool {
    let name = program
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(program)
        .to_ascii_lowercase();
    matches!(name.as_str(), "bash" | "zsh" | "fish")
}

pub type SharedSessionManager = Arc<SessionManager>;

#[cfg(test)]
mod tests;
