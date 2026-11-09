//! Terminal transports and session lifecycle, independent of desktop presentation.
use session::ManagedSession;
pub use session::SessionManager;
pub use session::SharedSessionManager;
#[cfg(not(windows))]
use session::should_use_interactive_login_args;
use ssh_connection::ForwardedTcpIpChannel;
#[cfg(test)]
use ssh_connection::ForwardedTcpIpDispatch;
#[cfg(test)]
use ssh_connection::PrimarySessionGate;
use ssh_connection::SshClientHandler;
use ssh_connection::SshHandleChain;
pub use ssh_connection::SshMultiplexHandle;
pub use ssh_connection::SshMultiplexInfo;
#[cfg(test)]
use ssh_connection::open_authenticated_ssh_handle_with_sender_registry;
use ssh_connection::{
    open_authenticated_ssh_handle, open_authenticated_ssh_handle_with_forwarded_tx,
    open_proxy_command_stream,
};
use ssh_worker::SshCommand;
#[cfg(test)]
use ssh_worker::SshPtyDimensions;
use ssh_worker::SshShellHandle;
use ssh_worker::TmuxCommandReceiver;
use ssh_worker::TmuxCommandSender;
pub use transport_io::LocalPtyTransport;
pub use transport_io::SerialTransport;
pub use transport_io::SshChannelTransport;
#[cfg(test)]
use transport_io::TelnetAutoLoginAction;
pub use transport_io::TelnetTransport;
pub use transport_io::TmuxPaneTransport;
use transport_io::TransportWriterCommand;
#[cfg(test)]
use transport_io::spawn_reader_thread;
mod proxy_command;
mod ssh_connection;
#[cfg(test)]
use ssh_connection::expand_proxy_command;
#[cfg(test)]
use ssh_connection::forwarded_tcpip_sender_for;
pub use ssh_connection::open_ssh_multiplex_handle;
#[cfg(test)]
use ssh_connection::register_x11_sender;
#[cfg(test)]
use ssh_connection::ssh_client_config;
#[cfg(test)]
use ssh_connection::ssh_host_identifier;
#[cfg(test)]
use ssh_connection::unregister_x11_sender;
mod transport_io;
#[cfg(test)]
use transport_io::remap_del_to_bs;
mod session;
use session::default_local_shell_path;
mod ssh_worker;
#[cfg(test)]
use ssh_worker::drain_deferred_ssh_open_commands;
pub mod connection_attempt;
#[cfg(windows)]
pub mod local_conpty;
pub mod network_route;
pub mod protocol_transfer;
pub mod ssh_channel;
pub mod ssh_services;
#[cfg(test)]
use russh::{cipher, kex, mac};

mod directory_command;
mod environment;
mod http_proxy;
pub mod local_shell;
mod local_shell_integration;
mod reconnect_cwd;
mod session_config;
mod session_event_queue;
pub mod session_notice;
mod session_types;
mod ssh_agent;
mod ssh_agent_broker;
mod ssh_algorithms;
mod ssh_auth;
#[cfg(test)]
mod ssh_auth_tests;
pub mod ssh_authentication;
mod ssh_mailbox;
mod telnet_codec;
mod telnet_input;
mod telnet_prompts;
pub mod tmux;
mod writer_mailbox;
#[cfg(test)]
use ssh_algorithms::defaults_from_preferred;
#[cfg(test)]
use telnet_codec::{DO, IAC, OPT_SUPPRESS_GO_AHEAD, WILL};
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
pub mod trzsz;
pub mod xymodem;
pub mod zmodem;

pub use directory_command::{
    DirectoryShell, build_directory_change_command, local_directory_shell,
    valid_terminal_directory_path,
};
pub use environment::{
    EnvironmentSnapshot, EnvironmentValue, ShellEnvironmentCache, ShellEnvironmentError,
    normalize_environment_variable_name,
};
pub use local_shell_integration::wsl_distro_from_launcher;
pub use session_config::{
    LocalSessionConfig, SerialSessionConfig, SftpCwdFollowMode, SftpSettings, SshAgentEndpoint,
    SshAgentForwardingConfig, SshAgentForwardingPolicy, SshAgentForwardingSources, SshAgentPrompt,
    SshAgentPromptAction, SshAgentPromptPhase, SshAgentPromptProvider, SshAgentPromptRequest,
    SshAgentStoredKey, SshAgentStoredKeyProvider, SshAgentStoredKeySnapshot, SshAlgorithmMode,
    SshAlgorithmPreferences, SshCredentialPrompt, SshCredentialPromptKind,
    SshCredentialPromptReason, SshCredentialProvider, SshHostKey, SshHostKeyDecision,
    SshHostKeyVerifier, SshKeyAuthConfig, SshKeyboardInteractivePrompt,
    SshKeyboardInteractiveRequest, SshOtpProvider, SshProxyConfig, SshResourceProbesDisabled,
    SshSessionConfig, SshSessionProfile, TelnetAutoLoginConfig, TelnetEnterMode,
    TelnetSessionConfig,
};
use session_event_queue::SessionEventQueue;
#[cfg(test)]
use session_event_queue::{
    SESSION_EVENT_QUEUE_OUTPUT_EVENT_LIMIT, SESSION_EVENT_QUEUE_OUTPUT_LIMIT,
};
pub use session_types::{
    CleanupOperation, SessionDrain, SessionDrainStats, SessionError, SessionEvent,
    SessionEventConsumerId, SessionInfo, SessionKind, TerminalTransport,
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

pub use reconnect_cwd::build_ssh_reconnect_cwd_command;
pub mod ssh_execution;
#[cfg(all(test, unix))]
use ssh_shell_integration::prepare_shell_integration;
#[cfg(test)]
use ssh_shell_integration::{
    OscStripper, ShellIntegrationMode, activation_script, bytes_after_ssh_ready_marker,
    persistent_script, rc_managed_block, ssh_shell_injection_script, strip_ssh_ready_markers,
};

#[cfg(test)]
mod tests;
