/** Type of terminal session. */
export type SessionType = "SSH" | "Local" | "Telnet" | "Serial";

/** Backend-authoritative, runtime-only cwd values safe for presentation/copy. */
export type SessionCwdPresentation = {
  title: string;
  displayPath: string;
  copyValue: string;
  /** Host-native path for filesystem consumers; absent for presentation-only locations. */
  operationalPath?: string | null;
  copyAsUri: boolean;
};

export type SessionTitleSnapshot = {
  /** Sanitized application-provided OSC 0/2 title. */
  applicationTitle: string | null;
  /** Validated Local cwd presentation; SSH cwd is never promoted. */
  cwd: SessionCwdPresentation | null;
  /** Final policy-resolved title, or null when static fallback should win. */
  effectiveTitle: string | null;
  /** Saved-connection policy copied into this runtime session. */
  enabled: boolean;
};

export type WorkspaceSessionType = SessionType | "RDP" | "VNC";
export type WorkspacePaneKind = "terminal" | "remote-desktop" | "file";
export type PersistedWorkspacePaneKind = WorkspacePaneKind | "rdp";
export type { TemporaryLinkConfig } from "@/types/temporaryConnection";

export interface AppRuntimeInfo {
  portable: boolean;
  mode: "installed" | "portable";
  executableDir: string;
  dataDir: string;
  configDir: string;
  logDir: string;
  webviewDataDir: string;
  portableMarkerPath?: string | null;
}

export interface AppSupportInfo {
  os: string;
  architecture: string;
  runtime: "portable" | "installed";
}

/** AI Agent command execution wrapper profile. */
export type AIExecutionProfile =
  | "auto"
  | "posix"
  | "powershell"
  | "cmd"
  | "send_only"
  | "disabled";
export type SshProfile = "standard" | "network_device";
export type SshRuntimeMode = "standard" | "terminal";
export type SshTerminalType =
  | "xterm-256color"
  | "xterm"
  | "vt100"
  | "vt220"
  | "ansi"
  | "linux";

/** A group of sessions whose terminal input is broadcast to all members. */
export interface SyncGroup {
  id: string;
  name: string;
  color: string;
  sessionIds: string[];
  /** Session ids that are temporarily paused (still members, but not broadcasting). */
  pausedSessionIds: string[];
  enabled: boolean;
}

/** Split orientation inside a workspace tab. */
export type PaneSplitDirection = "horizontal" | "vertical";

/** Connection type discriminator matching Rust ConnectionType. */
export type ConnectionTypeTag =
  | "ssh"
  | "local_terminal"
  | "telnet"
  | "serial"
  | "rdp"
  | "vnc";

/** Metadata for a connected or disconnected session. */
export interface SessionInfo {
  id: string;
  name: string;
  session_type: WorkspaceSessionType;
  started_at: string;
  connection_id?: string | null;
  connected: boolean;
  owner_window_label?: string | null;
  ai_execution_profile: AIExecutionProfile;
  /** True when backend shell command-confirmation integration is active. */
  injection_active: boolean;
  /** Whether application/remote dynamic titles may be promoted. */
  dynamic_title_enabled: boolean;
  /** Whether the selected shell received NyaTerm dynamic-title/cwd hooks. */
  dynamic_title_integration_active: boolean;
  /** Trusted resolved Windows executable identity for initial ConPTY filtering. */
  trusted_initial_title?: string | null;
  /** True when the remote file browser is enabled for this session. */
  remote_file_browser_enabled: boolean;
  /** True when Linux-style remote resource stats are enabled for this session. */
  remote_stats_enabled: boolean;
  /** SSH runtime profile used for capability gating. */
  ssh_profile?: SshProfile | null;
}

/** Shared fields for one session-like leaf inside a workspace tab. */
export interface WorkspacePaneBase {
  id: string;
  kind: "leaf";
  paneKind: WorkspacePaneKind;
  sessionId: string;
  name: string;
  type: WorkspaceSessionType;
  connectionId?: string;
  /** Config for ad-hoc (temporary) sessions that have no saved connection. */
  temporaryConfig?: import("@/types/temporaryConnection").TemporaryLinkConfig;
  /** True while the backend session is being established. XTerminal is not rendered yet. */
  connecting?: boolean;
  /** Backend creation request id used to cancel an in-flight session creation. */
  createRequestId?: string;
  /** Populated when session creation failed and the pane should stay visible as an error state. */
  connectError?: string;
}

/** Leaf node representing one terminal session inside a workspace tab. */
export interface TerminalSessionPane extends WorkspacePaneBase {
  paneKind: "terminal";
  type: SessionType;
}

export type RemoteDesktopScaleMode = "fit" | "actual" | "stretch";

export interface RemoteDesktopDisplayMetadata {
  remoteWidth?: number;
  remoteHeight?: number;
  scaleMode?: RemoteDesktopScaleMode;
  viewOnly?: boolean;
  clipboardEnabled?: boolean;
}

/** Leaf node representing one graphical remote desktop session inside a workspace tab. */
export interface RemoteDesktopSessionPane extends WorkspacePaneBase {
  paneKind: "remote-desktop";
  type: "RDP" | "VNC";
  display?: RemoteDesktopDisplayMetadata;
}

/** Leaf node representing one graphical RDP session inside a workspace tab. */
export interface RdpSessionPane extends RemoteDesktopSessionPane {
  type: "RDP";
}

export interface VncSessionPane extends RemoteDesktopSessionPane {
  type: "VNC";
}

export type FileDocumentBackend = "local" | "remote";

export interface FileDocumentSnapshot {
  content: string;
  size: number;
  mtime: number;
  mtimeNanos?: string;
  contentHash: string;
}

/** Runtime-only editable document backed by an existing terminal session. */
export interface FileDocumentPane extends WorkspacePaneBase {
  paneKind: "file";
  type: SessionType;
  file: {
    backend: FileDocumentBackend;
    path: string;
    initial: FileDocumentSnapshot;
  };
}

export type SessionPane =
  | TerminalSessionPane
  | RdpSessionPane
  | VncSessionPane
  | FileDocumentPane;

/** Split node containing two child panes. */
export interface SplitPane {
  id: string;
  kind: "split";
  direction: PaneSplitDirection;
  /** Ratio of the first child between 0 and 1. */
  ratio: number;
  first: PaneNode;
  second: PaneNode;
}

/** Recursive pane tree for a workspace tab. */
export type PaneNode = SessionPane | SplitPane;

/** Top-level workspace tab shown in the terminal tab bar. */
export interface Tab {
  id: string;
  /** Stable restore ordering, independent from runtime drag-reorder. */
  persistOrder: number;
  activePaneId: string;
  root: PaneNode;
  /** User-set display name shown instead of `name` when present. */
  customName?: string;
  /** Hex color string for the tab accent line and background tint. */
  tabColor?: string;
  /** True when the tab is protected from accidental close actions. */
  locked?: boolean;
}

/** SSH connection config for creating a session. */
export interface SshConfig {
  connection_id?: string | null;
  owner_window_label?: string | null;
  name: string;
  host: string;
  port: number;
  username: string;
  auth: SshAuth;
  backspace_mode?: string;
  x11_forwarding?: boolean;
  x11_display?: string;
  auth_agent_endpoint?: SshAgentEndpoint;
  agent_forwarding_config?: SshAgentForwardingConfig;
  proxy?: ProxySettings | null;
  proxy_jump?: SshConfig | null;
  post_login?: { command: string; delay_ms: number } | null;
  ssh_algorithms?: SshAlgorithmPreferences | null;
  ssh_profile?: SshProfile;
  runtime_mode?: SshRuntimeMode;
  terminal_type?: SshTerminalType;
  sftp?: SftpSettings;
  encoding?: string;
  dynamic_tab_title?: boolean;
}

/** SSH authentication: none, password, private key (PEM content), or SSH Agent. */
export type SshAgentEndpoint =
  | { type: "auto" }
  | { type: "environment"; variable: string }
  | { type: "unix_socket"; path: string }
  | { type: "pageant" }
  | { type: "windows_open_ssh" };

export interface SshAgentForwardingSources {
  external_agent: boolean;
  external_agent_endpoints: SshAgentEndpoint[];
  stored_keys: boolean;
}

export type SshAgentForwardingPolicy =
  | { mode: "allowlist"; fingerprints: string[] }
  | { mode: "all" };

export interface SshAgentForwardingConfig {
  enabled: boolean;
  sources: SshAgentForwardingSources;
  policy: SshAgentForwardingPolicy;
}

export interface SshAgentForwardingIdentity {
  fingerprint: string;
  comment: string;
  source: "external_agent" | "stored_key";
  custom_endpoint_index?: number;
}

export type SshAgentForwardingEndpointErrorCode =
  | "connect_failed"
  | "identity_enumeration_failed";

export interface SshAgentForwardingEndpointError {
  custom_endpoint_index: number;
  endpoint_type: SshAgentEndpoint["type"];
  code: SshAgentForwardingEndpointErrorCode;
}

export interface SshAgentForwardingIdentityResponse {
  identities: SshAgentForwardingIdentity[];
  endpoint_errors: SshAgentForwardingEndpointError[];
  truncated: boolean;
}

export type SshAuth =
  | { type: "none" }
  | { type: "password"; password?: string | null }
  | { type: "agent" }
  | {
      type: "key";
      key_data: string;
      cert_data?: string | null;
      passphrase?: string;
    };

/** Group for organizing saved connections. Groups form a tree via parent_id. */
export interface Group {
  id: string;
  name: string;
  parent_id?: string;
  sort_order: number;
}

/** Managed SSH private key stored in local app storage. */
export interface SshKey {
  id: string;
  name: string;
  /** Transient: plaintext private key content pasted from the UI. */
  key_data?: string;
  /** Transient: plaintext certificate content pasted from the UI. */
  cert_data?: string;
  /** Encrypted certificate content is never returned to the UI. */
  cert?: string;
  /** True when encrypted key data exists in local storage. */
  has_key_data?: boolean;
  /** True when encrypted certificate data exists in local storage. */
  has_cert_data?: boolean;
  /** Transient: file path from the UI file picker. */
  key_file_path?: string;
  /** Transient: certificate file path from the UI file picker. */
  cert_file_path?: string;
  /** Passphrase for this key (only sent when creating/updating). */
  passphrase?: string;
}

/** Managed password entry stored in local app storage. */
export interface SavedPassword {
  id: string;
  name: string;
  /** True when encrypted password data exists in local storage. */
  has_password?: boolean;
  /** Plaintext password (only sent when creating/updating). */
  password?: string;
}

/** Terminal credential entry used for prompt-based autofill. */
export interface SavedCredential {
  id: string;
  sort_order: number;
  name: string;
  username: string;
  /** Plaintext password (only sent when creating/updating). */
  password?: string;
  /** Optional JavaScript regex source for username prompts. */
  username_prompt_regex?: string | null;
  /** Optional JavaScript regex source for password prompts. */
  password_prompt_regex?: string | null;
  enabled: boolean;
  /** True when encrypted password data exists in local storage. */
  has_password?: boolean;
}

/** Auth block for SSH connections. */
export interface ConnectionAuth {
  mode: string;
  password_id?: string;
  /** Inline password (plaintext when saving, absent when loading). */
  password?: string;
  /** True when an inline password is stored locally (set by backend on load). */
  has_password?: boolean;
  key_id?: string;
  otp_id?: string;
  auto_fill_otp?: boolean;
}

/** Network block for connections. */
export interface ConnectionNetwork {
  proxy_id?: string;
  proxy_jump_id?: string;
}

/** SSH post-login command automation. */
export interface ConnectionPostLogin {
  enabled: boolean;
  command: string;
  delay_ms: number;
}

export type AssetDeviceType =
  | "physical"
  | "virtual"
  | "cloud"
  | "network"
  | "storage"
  | "embedded"
  | "other";

export type AssetAcceleratorType = "gpu" | "npu" | "other";

export interface AssetAccelerator {
  type: AssetAcceleratorType;
  vendor?: string;
  model?: string;
  count?: number;
  memory_bytes?: number;
}

export interface AssetDisk {
  kind?: "hdd" | "ssd" | "nvme" | "other";
  model?: string;
  capacity_bytes?: number;
  count?: number;
  purpose?: "system" | "data" | "cache" | "other";
}

export interface AssetMetadata {
  device_type?: AssetDeviceType;
  os_name?: string;
  os_version?: string;
  architecture?: string;
  kernel_version?: string;
  hostname?: string;
  cpu_model?: string;
  cpu_sockets?: number;
  cpu_cores?: number;
  cpu_threads?: number;
  memory_bytes?: number;
  accelerators?: AssetAccelerator[];
  disks?: AssetDisk[];
  tags?: string[];
  notes?: string;
  updated_at?: string;
}

export interface TelnetAutoLoginConfig {
  enabled?: boolean;
  send_wake_enter?: boolean;
  timeout_ms?: number;
  username_prompt_regex?: string | null;
  password_prompt_regex?: string | null;
  success_prompt_regex?: string | null;
  failure_prompt_regex?: string | null;
  max_retries?: number;
}

export type SshAlgorithmMode = "compatible" | "secure" | "custom";

export interface SshAlgorithmPreferences {
  mode: SshAlgorithmMode;
  kex: string[];
  ciphers: string[];
  macs: string[];
  host_keys: string[];
}

export type SftpCwdFollowMode = "off" | "shell_integration" | "rc_file";

export interface SftpSettings {
  enabled: boolean;
  cwd_follow_mode: SftpCwdFollowMode;
  shell_detection_timeout_ms: number;
  filename_encoding?: string;
  /** Override SFTP single-file pipeline depth. Undefined means automatic. */
  pipeline_depth?: number;
}

export type AlgorithmRisk = "modern" | "legacy" | "insecure";

export interface AlgorithmOption {
  id: string;
  label: string;
  risk: AlgorithmRisk;
}

export interface SshAlgorithmDefaults {
  kex: string[];
  ciphers: string[];
  macs: string[];
  host_keys: string[];
}

export interface SupportedSshAlgorithms {
  kex: AlgorithmOption[];
  ciphers: AlgorithmOption[];
  macs: AlgorithmOption[];
  host_keys: AlgorithmOption[];
  compatible: SshAlgorithmDefaults;
  secure: SshAlgorithmDefaults;
}

export interface ConnectionCustomIcon {
  id: string;
  name: string;
  data_url: string;
  created_at_ms: number;
  updated_at_ms: number;
}

/** Unified saved connection with type-discriminated config. */
export interface SavedConnection {
  id: string;
  name: string;
  /** Connection type discriminator. */
  type: ConnectionTypeTag;
  group_id?: string;
  description?: string;
  sort_order?: number;
  icon?: string;
  icon_auto_detect?: boolean;
  created_at_ms?: number;
  updated_at_ms?: number;
  last_used_at_ms?: number;
  auth?: ConnectionAuth;
  network?: ConnectionNetwork;
  post_login?: ConnectionPostLogin;
  recording?: ConnectionRecordingSettings;
  ssh_algorithms?: SshAlgorithmPreferences;
  /** SSH-only: runtime profile. Network devices skip Linux-only probes and integrations. */
  ssh_profile?: SshProfile;
  /** SSH-only: PTY terminal type. Omitted means profile default. */
  terminal_type?: SshTerminalType;
  sftp?: SftpSettings;
  asset?: AssetMetadata;
  /** SSH-specific fields (present when type === "ssh"). */
  host?: string;
  port?: number;
  username?: string;
  /** Local terminal fields (present when type === "local_terminal"). */
  shell_path?: string;
  shell_args?: string;
  working_dir?: string;
  /** Show the session's live window title / cwd as the tab label. */
  dynamic_tab_title?: boolean;
  /** Legacy saved value; runtime sessions now resolve the effective AI execution profile automatically. */
  ai_execution_profile?: AIExecutionProfile;
  /** Serial fields (present when type === "serial"). */
  port_name?: string;
  baud_rate?: number;
  data_bits?: number;
  parity?: string;
  stop_bits?: string;
  /** Backspace key mode for SSH/Telnet/Serial connections ("ctrl_h" or "del"). */
  backspace_mode?: string;
  /** Telnet-only: bypass Telnet option negotiation for embedded/raw TCP CLIs. */
  raw_tcp_cli?: boolean;
  /** Telnet-only: Enter send mode ("crlf", "cr", or "lf"). */
  enter_mode?: "crlf" | "cr" | "lf";
  /** Telnet-only: locally echo typed input when the remote does not echo. */
  local_echo?: boolean;
  /** Telnet-only: locally edit a line and send it when Enter is pressed. */
  local_line_edit?: boolean;
  /** Telnet-only: write each input character to the socket immediately. */
  force_character_at_a_time?: boolean;
  /** Telnet-only: send NAWS resize subnegotiation in standard Telnet mode. */
  send_naws?: boolean;
  /** Telnet-only: accept/respond to SGA negotiation in standard Telnet mode. */
  send_sga?: boolean;
  /** Telnet-only: prompt-driven automatic login. */
  auto_login?: TelnetAutoLoginConfig;
  /** SSH-only: enables X11 forwarding for remote graphical applications. */
  x11_forwarding?: boolean;
  /** SSH-only: local Agent endpoint used for authentication. Forwarding endpoints are configured separately. */
  auth_agent_endpoint?: SshAgentEndpoint;
  /** SSH-only: forwarding sources and fingerprint policy. */
  agent_forwarding_config?: SshAgentForwardingConfig;
  /** Per-connection encoding override. Empty string means follow global setting. */
  encoding?: string;
  /** RDP-only: optional Windows/domain part for authentication. */
  domain?: string;
  /** RDP-only: connect using the administrative (console) session. */
  admin?: boolean;
  /** RDP/VNC security options. */
  security?: Partial<RdpSecuritySettings & VncSecuritySettings>;
  /** RDP/VNC display options. */
  display?: Partial<RdpDisplaySettings & VncDisplaySettings>;
  /** RDP/VNC clipboard options. */
  clipboard?: Partial<RdpClipboardSettings & VncClipboardSettings>;
  /** RDP/VNC reconnect options. */
  reconnect?: Partial<RdpReconnectSettings & VncReconnectSettings>;
  /** VNC-only shared-session flag. */
  shared?: boolean;
  /** VNC-only local input policy. */
  view_only?: boolean;
}

export type RdpCertificatePolicy = "strict" | "prompt" | "accept-temporarily";
export type RdpDisplayMode = "fit-window" | "fixed" | "native";
export type RdpClipboardMode = "disabled" | "text-only";

export interface RdpSecuritySettings {
  use_nla: boolean;
  certificate_policy: RdpCertificatePolicy;
}

export interface RdpDisplaySettings {
  mode: RdpDisplayMode;
  width: number;
  height: number;
  color_depth: 16 | 24 | 32;
}

export interface RdpClipboardSettings {
  mode: RdpClipboardMode;
}

export interface RdpReconnectSettings {
  enabled: boolean;
  max_attempts: number;
}

export interface VncSecuritySettings {
  mode: "auto" | "vnc-auth" | "none";
}

export interface VncDisplaySettings {
  scale_mode: RemoteDesktopScaleMode;
}

export interface VncClipboardSettings {
  enabled: boolean;
}

export interface VncReconnectSettings {
  enabled: boolean;
  max_attempts: number;
}

export type RecordingMode = "transcript" | "raw";
export type RecordingState =
  | "starting"
  | "recording"
  | "degraded"
  | "failed"
  | "stopping";
export type ExistingFileBehavior = "unique" | "append" | "overwrite";
export type RotationPolicy =
  | { type: "session" }
  | { type: "daily" }
  | { type: "size"; max_bytes: number };

export interface RecordingSettings {
  auto_start: boolean;
  default_mode: RecordingMode;
  base_path: string;
  path_template: string;
  include_timestamps: boolean;
  include_io_labels: boolean;
  include_session_metadata: boolean;
  rotation: RotationPolicy;
  existing_file_behavior: ExistingFileBehavior;
  memory_limit_bytes: number;
  include_binary_transfer_payloads: boolean;
}

export interface ConnectionRecordingSettings {
  auto_start?: boolean | null;
  mode?: RecordingMode | null;
  path_template?: string | null;
  include_timestamps?: boolean | null;
  rotation?: RotationPolicy | null;
}

export interface RecordingStatus {
  sessionId: string;
  state: RecordingState;
  mode: RecordingMode;
  filePath: string;
  startedAt: string;
  writtenBytes: number;
  queuedBytes: number;
  droppedBytes: number;
  lastError?: string | null;
}

/** Stored OTP entry for two-factor authentication. */
export interface OtpEntry {
  id: string;
  /** "totp" or "hotp". */
  otp_type: string;
  issuer: string;
  username: string;
  /** Base32-encoded secret (only sent when creating/updating). */
  secret?: string;
  algorithm: string;
  digits: number;
  /** Time step in seconds (TOTP only). */
  period: number;
  /** Counter value (HOTP only). */
  counter: number;
  /** True when encrypted secret data exists in local storage. */
  has_secret?: boolean;
}

/** Result of generating an OTP code. */
export interface OtpCodeResult {
  code: string;
  remainingSeconds: number;
}

/** Saved leaf pane for startup restoration. */
export interface RestorableSessionPane {
  id?: string;
  kind: "leaf";
  pane_kind?: PersistedWorkspacePaneKind;
  title: string;
  session_type: WorkspaceSessionType | "local";
  connection_id?: string;
  display?: RemoteDesktopDisplayMetadata;
}

/** Saved split pane for startup restoration. */
export interface RestorableSplitPane {
  id?: string;
  kind: "split";
  direction: PaneSplitDirection;
  ratio: number;
  first: RestorablePaneNode;
  second: RestorablePaneNode;
}

/** Saved pane tree node for startup restoration. */
export type RestorablePaneNode = RestorableSessionPane | RestorableSplitPane;

/** Saved workspace tab state for startup restoration. */
export interface RestorableTab {
  active_pane_id?: string;
  root?: RestorablePaneNode;
  /** Legacy fields kept optional so older frontend payloads still type-check during migration. */
  title: string;
  session_type: string;
  connection_id?: string;
  custom_name?: string;
  tab_color?: string;
  locked?: boolean;
}

export type LeftPanelId =
  | "fileExplorer"
  | "notes"
  | "network"
  | "securityAuth"
  | "syncBackupHistory";

export type RightPanelId =
  | "savedConnections"
  | "aiAssistant"
  | "activeSessions"
  | "commandHistory"
  | "resourceMonitor"
  | "gpuMonitor"
  | "ascendNpuMonitor"
  | "processManager"
  | "dockerManager"
  | "recording"
  | "syncBackupHistory";

export type ActivityBarZone =
  | "left_top"
  | "left_bottom"
  | "right_top"
  | "right_bottom";

export interface ActivityBarLayout {
  left_top: string[];
  left_bottom: string[];
  right_top: string[];
  right_bottom: string[];
  /** When true every activity bar icon shows its name below the icon. */
  show_labels: boolean;
  /** Activity bar item ids hidden by the user without changing their layout position. */
  hidden_items: string[];
}

/** Layout preferences: panel widths, active panels, theme. */
export type QuickCommandViewMode = "list" | "compact" | "tile";
export type QuickCommandSortMode = "created" | "name" | "useCount" | "custom";
export type HeaderStatusMode =
  | "session"
  | "resources"
  | "host"
  | "datetime"
  | "gpu"
  | "npu";

export type RestorableTerminalWindowNode =
  | {
      kind: "leaf";
      tab_indexes: number[];
      active_tab_index: number | null;
    }
  | {
      kind: "split";
      direction: PaneSplitDirection;
      ratio: number;
      first: RestorableTerminalWindowNode;
      second: RestorableTerminalWindowNode;
    };

export interface UiConfig {
  open_tabs: RestorableTab[];
  terminal_window_layout: RestorableTerminalWindowNode | null;
  start_workspace_mode?: "workbench" | "assets";
  panel_open_mode: "docked" | "floating";
  left_width: number;
  right_width: number;
  quick_cmd_height: number;
  quick_cmd_category_width?: number;
  quick_cmd_view_mode: QuickCommandViewMode;
  quick_cmd_sort_mode?: QuickCommandSortMode;
  quick_cmd_selected_category?: string;
  /** ID of whichever panel is currently open on the left side. */
  active_left_panel: string | null;
  /** ID of whichever panel is currently open on the right side. */
  active_right_panel: string | null;
  /** Panels currently open on the left side when multi-open panels are enabled. */
  left_open_panels: string[];
  /** Panels currently open on the right side when multi-open panels are enabled. */
  right_open_panels: string[];
  /** Relative height weight per panel id for stacked multi-open panels. */
  panel_stack_sizes: Record<string, number>;
  network_panel_active_tab?: "tunnel" | "proxy";
  security_auth_panel_active_tab?: "keys" | "passwords" | "otp" | "credentials";
  show_quick_cmd_bar: boolean;
  show_serial_send_panel: boolean;
  serial_send_height: number;
  serial_send_clear_after_send: boolean;
  zoom_level: number;
  language?: string;
  header_status_mode?: HeaderStatusMode;
  header_status_visible?: boolean;
  show_notes_panel: boolean;
  show_remote_stats: boolean;
  remote_stats_interval: number;
  show_gpu_monitor: boolean;
  gpu_monitor_interval: number;
  show_ascend_npu_monitor: boolean;
  ascend_npu_monitor_interval: number;
  show_process_manager: boolean;
  process_manager_interval: number;
  show_docker_manager: boolean;
  docker_manager_interval: number;
  saved_connections_sort_mode?: string;
  saved_connections_expanded_group_ids?: string[];
  asset_sort_key?: string | null;
  asset_sort_direction?: "asc" | "desc" | null;
  recent_connection_ids: string[];
  transfer_height: number;
  file_explorer_show_hidden_files: boolean;
  file_explorer_auto_sync_cwd_connection_ids: string[];
  file_explorer_favorite_dirs_by_connection_id: Record<string, string[]>;
  notes_expanded_folder_ids: string[];
  notes_last_selected_node_id: string | null;
  activity_bar_layout: ActivityBarLayout;
}

/** Resource usage stats fetched from the active remote SSH host. */
export interface RemoteStatsSystem {
  hostname: string;
  uptime_sec: number;
  os: string;
  arch: string;
}

export interface RemoteStatsLoad {
  load1: number;
  load5: number;
  load15: number;
}

export interface RemoteStatsCpu {
  model: string;
  cores: number;
  usage: number | null;
  per_core: { id: number; usage: number }[];
  sample_window_ms: number | null;
  usage_source: "warming_up" | "aggregate" | "core_weighted_fallback";
}

export interface RemoteStatsMemory {
  used: number;
  available: number;
  cached: number;
}

export interface RemoteStatsNetwork {
  nic: string;
  state: string;
  rx_bytes_per_sec: number;
  tx_bytes_per_sec: number;
}

export interface RemoteStatsNetworkSummary {
  rx_bytes_per_sec: number;
  tx_bytes_per_sec: number;
}

export interface RemoteStatsDisk {
  device: string;
  mount: string;
  total: number;
  available: number;
  use_percent: number;
}

export interface RemoteStats {
  system: RemoteStatsSystem;
  load: RemoteStatsLoad;
  cpu: RemoteStatsCpu;
  memory: RemoteStatsMemory;
  networks: RemoteStatsNetwork[];
  network_summary: RemoteStatsNetworkSummary;
  disks: RemoteStatsDisk[];
}

export interface RemoteProcess {
  pid: number;
  ppid: number;
  user: string;
  state: string;
  cpu_percent: number;
  memory_percent: number;
  rss_kb: number;
  vsz_kb: number;
  elapsed: string;
  command: string;
  command_line: string;
}

export interface RemoteCommandOutput {
  stdout: string;
  stderr: string;
  exit_status?: number | null;
}

export interface DockerContainerStats {
  cpu_percent: number;
  memory_percent: number;
  memory_usage: string;
  net_io: string;
  block_io: string;
  pids: string;
}

export interface DockerContainer {
  id: string;
  name: string;
  image: string;
  status: string;
  state: string;
  ports: string;
  created_at: string;
  size: string;
  stats?: DockerContainerStats | null;
}

export interface DockerContainerMount {
  kind: string;
  source: string;
  destination: string;
  mode: string;
  rw: boolean;
}

export interface DockerContainerNetwork {
  name: string;
  ip_address: string;
}

export interface DockerContainerDetails {
  stats?: DockerContainerStats | null;
  started_at: string;
  finished_at: string;
  restart_count: number;
  entrypoint: string;
  command: string;
  mounts: DockerContainerMount[];
  networks: DockerContainerNetwork[];
}

export interface DockerImage {
  id: string;
  repository: string;
  tag: string;
  size: string;
  created_since: string;
}

export interface DockerVolume {
  driver: string;
  name: string;
}

export interface DockerNetwork {
  id: string;
  name: string;
  driver: string;
  scope: string;
}

export interface DockerComposeProject {
  name: string;
  status: string;
  config_files: string;
}

export interface DockerComposeServiceContainer {
  id: string;
  name: string;
  state: string;
  status: string;
}

export interface DockerComposeService {
  name: string;
  status: string;
  containers: DockerComposeServiceContainer[];
}

export interface RemoteDockerOverview {
  available: boolean;
  version: string;
  compose_available: boolean;
  containers: DockerContainer[];
  images: DockerImage[];
  volumes: DockerVolume[];
  networks: DockerNetwork[];
  compose_projects: DockerComposeProject[];
}

export interface RemoteGpu {
  index: number;
  uuid: string;
  name: string;
  temperature_c?: number | null;
  utilization_gpu_percent?: number | null;
  utilization_memory_percent?: number | null;
  memory_total_mb: number;
  memory_used_mb: number;
  memory_free_mb: number;
  power_draw_w?: number | null;
  power_limit_w?: number | null;
  fan_speed_percent?: number | null;
  pstate: string;
}

export interface RemoteGpuProcess {
  gpu_uuid: string;
  gpu_index?: number | null;
  pid: number;
  process_name: string;
  used_memory_mb: number;
}

export interface RemoteGpuOverview {
  available: boolean;
  driver_version: string;
  cuda_version: string;
  gpus: RemoteGpu[];
  processes: RemoteGpuProcess[];
}

export interface RemoteNpu {
  index: number;
  chip_id: number;
  physical_id?: number | null;
  device_key: string;
  name: string;
  health: string;
  bus_id: string;
  temperature_c?: number | null;
  utilization_aicore_percent?: number | null;
  utilization_memory_percent?: number | null;
  memory_total_mb: number;
  memory_used_mb: number;
  memory_free_mb: number;
  memory_kind: string;
  hbm_total_mb?: number | null;
  hbm_used_mb?: number | null;
  power_draw_w?: number | null;
}

export interface RemoteNpuProcess {
  npu_index: number;
  chip_id: number;
  device_key: string;
  pid: number;
  process_name: string;
  used_memory_mb: number;
}

export interface RemoteNpuOverview {
  available: boolean;
  driver_version: string;
  cann_version: string;
  npus: RemoteNpu[];
  processes: RemoteNpuProcess[];
}

/** Labeled command shortcut for quick execution. */
export interface QuickCommandCategory {
  id: string;
  name: string;
  parent_id?: string;
  sort_order?: number;
}

export interface QuickCommand {
  id: string;
  label: string;
  command: string;
  category_id?: string;
  description?: string;
  color_tag?: string;
  icon_tag?: string;
  pinned?: boolean;
  execution_mode?: string;
  source?: "manual" | "ai";
  risk_level?: RiskLevel;
  updated_at?: number;
  created_at?: number;
  use_count?: number;
  sort_order?: number;
}

export interface QuickCommandsConfig {
  commands: QuickCommand[];
  categories: QuickCommandCategory[];
}

export type QuickCommandImportSource =
  | "windterm_quickbar"
  | "xshell_xts"
  | "nyaterm_json";

export interface QuickCommandImportResult {
  imported_commands: number;
  imported_categories: number;
  updated_commands: number;
  total_commands: number;
  total_categories: number;
}

/** Fuzzy search result with matched command and highlight indices. */
export interface FuzzyResult {
  command: string;
  score: number;
  indices: number[];
  /** Provider tag: "history" | "quickCommand" | future sources. */
  source: string;
  /** Text shown in the suggestion panel (may differ from command). */
  display: string;
}

export interface GeneralSettings {
  startup_restore: boolean;
  startup_restore_window_layout: boolean;
  minimize_to_tray: boolean;
  boss_key: string | null;
  confirm_on_close: boolean;
}

export type BackgroundImageFit = "cover" | "contain" | "stretch" | "tile";

/** Internal native transparency marker. Windows 11 only; other platforms no-op. */
export type WindowTransparency = "none" | "transparent";

export interface TerminalThemeColors {
  background: string;
  foreground: string;
  cursor: string;
  selectionBackground: string;
  lineHighlight: string;
  findMatchBackground: string;
  findMatchBorder: string;
  black: string;
  red: string;
  green: string;
  yellow: string;
  blue: string;
  magenta: string;
  cyan: string;
  white: string;
  brightBlack: string;
  brightRed: string;
  brightGreen: string;
  brightYellow: string;
  brightBlue: string;
  brightMagenta: string;
  brightCyan: string;
  brightWhite: string;
}

export interface ThemeSettingsColors {
  bg: string;
  bgPanel: string;
  bgTerminal: string;
  bgHover: string;
  bgInput: string;
  bgSectionHeader: string;
  border: string;
  text: string;
  textMuted: string;
  textDimmed: string;
  primary: string;
  primaryHover: string;
  onPrimary: string;
  focusRing: string;
  danger: string;
  dangerHover: string;
  success: string;
  warning: string;
  link: string;
  shadow: string;
  scrollThumb: string;
  accent: string;
  terminal: TerminalThemeColors;
}

export interface CustomThemeSettings {
  id: string;
  name: string;
  label: string;
  swatch: string;
  colors: ThemeSettingsColors;
}

export interface AppearanceSettings {
  theme: string;
  custom_themes: CustomThemeSettings[];
  font_family: string;
  ui_font_family: string;
  font_size: number;
  font_weight: number;
  font_weight_bold: number;
  background_opacity: number;
  background_image_path: string | null;
  background_image_fit: BackgroundImageFit;
  background_image_opacity: number;
  cursor_style: string;
  cursor_blink: boolean;
  ui_font_size: number;
  terminal_theme: string | null;
  minimum_contrast_ratio: number;
  /** Allow opening multiple side panels at once, stacked vertically. */
  panel_multi_open: boolean;
  /** Internal native window transparency marker. */
  window_transparency: WindowTransparency;
  /** Surface opacity for transparent windows, 0.0 to 1.0. Low values may reveal windows behind the app. */
  window_transparency_tint: number;
  /** Whether native Acrylic material applies blur behind transparent windows. */
  window_transparency_blur: boolean;
}

export interface ProxySettings {
  enabled: boolean;
  protocol: string;
  host: string;
  port: number;
  command?: string;
}

export interface ProxyConfig {
  id: string;
  name: string;
  protocol: string;
  host: string;
  port: number;
  command?: string;
  username?: string;
  password?: string;
  password_id?: string;
  group_id?: string;
}

export interface NetworkGroup {
  id: string;
  name: string;
  sort_order: number;
}

export interface SearchEngine {
  name: string;
  url_template: string;
  icon?: string;
  show_in_menu?: boolean;
}

export interface SearchSettings {
  custom_engines: SearchEngine[];
}

export interface TranslationSettings {
  target_language: string;
  deepl_api_key: string;
  baidu_app_id: string;
  baidu_app_key: string;
  ali_app_id: string;
  ali_app_key: string;
  youdao_app_id: string;
  youdao_app_key: string;
}

export interface TranslateResult {
  original: string;
  translated: string;
  detected_language: string;
  provider: string;
}

export interface SecuritySettings {
  use_os_keyring: boolean;
  enable_startup_lock: boolean;
  enable_idle_lock: boolean;
  idle_lock_minutes: number;
  master_password?: string;
  host_key_policy: string;
}

export interface KeywordHighlightRule {
  id: string;
  name: string;
  /** Regex patterns (one per entry, compiled with gi flags). */
  patterns: string[];
  /** Color used when the terminal background is dark. */
  color_dark: string;
  /** Color used when the terminal background is light. */
  color_light: string;
  enabled: boolean;
}

export interface KeywordHighlightImportResult {
  imported_rules: number;
  updated_rules: number;
  total_rules: number;
}

export interface ActionLinksMatcherSettings {
  ipv4: boolean;
  archive: boolean;
  host_port: boolean;
}

export type KeywordHighlightBuiltinRuleSettings = Record<string, boolean>;
export type SshKeepAliveMode = "compatible" | "strict" | "disabled";

export interface TerminalSettings {
  scrollback_lines: number;
  keep_alive_mode: SshKeepAliveMode;
  keep_alive_interval: number;
  font_size_delta: number;
  x11_display?: string;
  hardware_acceleration: boolean;
  keyword_highlights_enabled: boolean;
  keyword_highlights_across_wrapped_lines: boolean;
  keyword_highlight_builtin_rules: KeywordHighlightBuiltinRuleSettings;
  keyword_highlights: KeywordHighlightRule[];
  action_links_enabled: boolean;
  action_links_matchers: ActionLinksMatcherSettings;
  show_workspace_padding: boolean;
  show_line_numbers: boolean;
  show_timestamps: boolean;
  timestamp_format: string;
  show_multi_line_paste_dialog: boolean;
  paste_image_as_path: boolean;
  reconnect_restore_cwd: boolean;
}

export interface TransferSettings {
  editor_type: "external" | "internal";
  internal_editor_display: "workspace" | "window";
  internal_editor_font_size: number;
  download_threads: number;
  upload_threads: number;
  duplicate_strategy: string;
  preserve_timestamps: boolean;
  resume_broken_transfer: boolean;
  default_file_permissions: string;
  max_transfer_retries: number;
  transfer_buffer_size: number;
  download_path: string;
  ask_save_location: boolean;
  default_editor: string;
  recording_path: string;
  recording_include_io_labels: boolean;
  recording_include_timestamps: boolean;
  recording_auto_start: boolean;
  recording_memory_limit_bytes: number;
}

export type DiagnosticsLogLevel = "warn" | "info" | "debug";

export interface DiagnosticsSettings {
  level: DiagnosticsLogLevel;
  retention_days: number;
}

export type RiskLevel = "low" | "medium" | "high" | "critical";
export type AIMode = "ask" | "agent";
export type AIAgentCommandExecutionMode = "confirm_each" | "smart" | "auto";
export type AIAgentKind = "nyaterm" | "codex" | "claude_code";
export type AIPermissionMode = "observer" | "confirm" | "auto" | "full_access";
export type ExternalMcpSessionScope = "current_window" | "all_sessions";
export interface ExternalMcpSettings {
  enabled: boolean;
  permission_mode: AIPermissionMode;
  session_scope: ExternalMcpSessionScope;
}
export type AIReasoningEffort =
  | "auto"
  | "none"
  | "low"
  | "medium"
  | "high"
  | "xhigh";
export type AIApiFormat = "chat_completions" | "responses";
export type AIModelSource = "rust-genai" | "manual";
export type AIBackendKind = "genai" | "codex";
export type CodexThreadMode = "persistent" | "ephemeral";

export type AIProviderKind =
  | "openai"
  | "anthropic"
  | "gemini"
  | "deepseek"
  | "groq"
  | "ollama"
  | "xai"
  | "cohere"
  | "mimo"
  | "zai"
  | "openai_compatible";

export interface AIModelConfigItem {
  id: string;
  name: string;
  backend?: AIBackendKind;
  provider_kind?: AIProviderKind | null;
  credential_id?: string | null;
  enabled: boolean;
  source: AIModelSource;
  last_seen_at?: string | null;
}

export interface CodexIntegrationSettings {
  enabled: boolean;
  executable_path?: string | null;
  runtime?: string | null;
  default_model?: string | null;
  config_directory?: string | null;
  permission_mode?: AIPermissionMode;
  tool_integration_mode?: string | null;
  thread_mode: CodexThreadMode;
  remote_terminal_agent_enabled: boolean;
}

export interface ClaudeCodeIntegrationSettings {
  enabled: boolean;
  executable_path?: string | null;
  runtime?: string | null;
  default_model?: string | null;
  config_directory?: string | null;
  permission_mode?: AIPermissionMode;
  tool_integration_mode?: string | null;
}

export interface AIProviderProfile {
  id: string;
  name: string;
  provider_kind: AIProviderKind;
  model: string;
  base_url?: string | null;
  api_key?: string | null;
  enabled: boolean;
}

export interface AIProviderCredential {
  id: string;
  name: string;
  provider_kind: AIProviderKind;
  api_format: AIApiFormat;
  base_url?: string | null;
  api_key?: string | null;
  enabled: boolean;
}

export interface AICustomActionConfig {
  id: string;
  name: string;
  prompt: string;
  enabled: boolean;
}

export interface AISettings {
  schema_version: number;
  enabled: boolean;
  context_line_limit: number;
  redaction_enabled: boolean;
  allow_save_command: boolean;
  record_history: boolean;
  timeout_ms: number;
  request_user_agent: string;
  active_profile_id: string;
  provider_profiles: AIProviderProfile[];
  default_mode: AIMode;
  default_agent_kind?: AIAgentKind;
  external_agent_permission_mode?: AIPermissionMode;
  default_reasoning_effort?: AIReasoningEffort;
  default_model_id?: string | null;
  models: AIModelConfigItem[];
  provider_credentials: AIProviderCredential[];
  terminal_ai_actions: AICustomActionConfig[];
  file_ai_actions: AICustomActionConfig[];
  max_ai_file_size_bytes: number;
  max_agent_steps?: number | null;
  agent_step_timeout_ms?: number | null;
  terminal_output_lines: number;
  agent_background_execution_enabled: boolean;
  agent_command_execution_mode: AIAgentCommandExecutionMode;
  agent_smart_auto_execute_max_risk: RiskLevel;
  codex: CodexIntegrationSettings;
  claude_code: ClaudeCodeIntegrationSettings;
  external_mcp: ExternalMcpSettings;
}

export interface McpRuntimeStatus {
  enabled: boolean;
  running: boolean;
  error?: string | null;
  ownerWindowLabel?: string | null;
  scopedSessionCount: number;
  connectionCount: number;
  port?: number | null;
  generation?: string | null;
}

export interface McpApprovalRequest {
  requestId: string;
  client: string;
  capability: string;
  sessionId?: string | null;
  sessionName?: string | null;
  connectionId?: string | null;
  connectionName?: string | null;
  parameterSummary: string;
  risk: RiskLevel;
}

export interface McpSessionOpenRequest {
  requestId: string;
  connectionId: string;
  targetWindowLabel: string;
}

export interface McpSessionOpenCancel {
  requestId: string;
  targetWindowLabel: string;
}

export interface AIContext {
  connectionName?: string | null;
  host?: string | null;
  port?: number | null;
  username?: string | null;
  cwd?: string | null;
  os?: string | null;
  arch?: string | null;
  recentOutput: string;
  selectedText: string;
  inputBuffer: string;
}

export type AIAction =
  | "generate_command"
  | "explain_output"
  | "explain_selected"
  | "analyze_error"
  | "repair_from_selection"
  | "custom_terminal_action"
  | "custom_file_action";

export interface AIModelDiscovery {
  id: string;
  name: string;
  backend?: AIBackendKind;
  providerKind?: AIProviderKind | null;
  credentialId?: string | null;
  source: AIModelSource;
}

export interface AICommandCard {
  id: string;
  title: string;
  command: string;
  explanation: string;
  riskLevel?: RiskLevel | null;
  riskReason?: string | null;
  expectedEffect: string;
  rollback?: string | null;
  category?: string | null;
  references?: string[];
  targetTerminalSessionId?: string | null;
  target?: AITerminalTarget | null;
}

export type AIScopeType = "terminal" | "workspace" | "global" | "unbound";

export interface AISessionScope {
  type: AIScopeType;
  targetId?: string | null;
  connectionIds?: string[];
  label?: string | null;
}

export interface AITerminalTarget {
  terminalSessionId: string;
  connectionId?: string | null;
  label: string;
  host?: string | null;
  username?: string | null;
  sessionType: string;
}

export interface AITargetContext {
  target?: AITerminalTarget | null;
  context: AIContext;
}

export interface AIMessage {
  id: string;
  sessionId: string;
  role: "user" | "assistant" | "system";
  content: string;
  createdAt: string;
  reasoningContent?: string | null;
  commandCards?: AICommandCard[];
}

export interface AISession {
  id: string;
  agentKind?: AIAgentKind;
  scope?: AISessionScope;
  connectionId?: string | null;
  title: string;
  createdAt: string;
  updatedAt: string;
  externalSessionId?: string | null;
  backendMetadata?: {
    backend: AIBackendKind;
    externalThreadId?: string | null;
  } | null;
}

export interface AIStreamStart {
  streamId: string;
  sessionId: string;
}

export interface AIStreamEventPayload {
  type: "start" | "delta" | "reasoning_delta" | "warning" | "done" | "error";
  streamId: string;
  sessionId?: string;
  textDelta?: string;
  reasoningDelta?: string;
  message?: AIMessage;
  commandCards?: AICommandCard[];
  usage?: unknown;
  error?: string;
}

export type AgentActionKind = "execute_command" | "final_answer";
export type AgentStepStatus =
  | "running"
  | "completed"
  | "needs_approval"
  | "rejected"
  | "failed";

export interface AgentStepAction {
  kind: AgentActionKind;
  command?: string | null;
  target?: AITerminalTarget | null;
  riskLevel?: RiskLevel | null;
  modelRiskLevel?: RiskLevel | null;
  localRiskLevel?: RiskLevel | null;
  riskReason?: string | null;
  approvalReason?: string | null;
  answer?: string | null;
}

export interface CommandObservation {
  output: string;
  exitCode?: number | null;
  durationMs: number;
}

export interface AgentStepPayload {
  streamId: string;
  sessionId?: string;
  stepIndex: number;
  thought: string;
  action: AgentStepAction;
  observation?: CommandObservation | null;
  status: AgentStepStatus;
  error?: string | null;
}

export type AiCaptureEvent =
  | { type: "commandStart"; command: string; stepIndex: number }
  | {
      type: "commandEnd";
      output: string;
      exitCode: number | null;
      durationMs: number;
      truncated: boolean;
    };

export interface TunnelConfig {
  id: string;
  name: string;
  tunnel_type: string;
  connection_id?: string;
  listen_port: number;
  target_host: string;
  target_port: number;
  is_open: boolean;
  auto_open: boolean;
  bind_localhost: boolean;
  group_id?: string;
}

export type TunnelRuntimeStatus =
  | "stopped"
  | "starting"
  | "running"
  | "reconnecting"
  | "disconnected"
  | "error";

export interface TunnelRuntimeState {
  tunnelId: string;
  status: TunnelRuntimeStatus;
  error?: string | null;
  updatedAt?: number | null;
}

export interface InteractionSettings {
  copy_on_select: boolean;
  allow_osc52_clipboard_write: boolean;
  terminal_right_click_action: "none" | "menu" | "paste";
  terminal_zoom_enabled: boolean;
  command_suggestions_enabled: boolean;
  command_suggestion_min_chars: number;
  command_suggestion_max_chars: number;
  duplicate_session_command_delay_ms: number;
  word_separators: string;
  alt_as_meta: boolean;
  ime_compatibility: boolean;
  default_encoding: string;
  tab_double_click_action: import("@/lib/interactionSettings").TabMouseAction;
  tab_middle_click_action: import("@/lib/interactionSettings").TabMouseAction;
  tab_right_click_action: import("@/lib/interactionSettings").TabMouseAction;
}

export interface AppSettings {
  general: GeneralSettings;
  appearance: AppearanceSettings;
  proxy: ProxySettings;
  search: SearchSettings;
  translation: TranslationSettings;
  security: SecuritySettings;
  terminal: TerminalSettings;
  interaction: InteractionSettings;
  recording: RecordingSettings;
  transfer: TransferSettings;
  diagnostics: DiagnosticsSettings;
  ai: AISettings;
  cloud_sync: CloudSyncSettings;
  ui: UiConfig;
  keybindings: Record<string, string>;
}

export interface FileEntry {
  name: string;
  is_dir: boolean;
  is_symlink: boolean;
  size: number;
  permissions: string;
  owner: string;
  group: string;
  mtime: number;
  raw_path_token?: string;
}

export interface FileProperties {
  name: string;
  is_dir: boolean;
  is_symlink: boolean;
  symlink_target?: string | null;
  size: number;
  permissions: string;
  owner: string;
  group: string;
  uid: string;
  gid: string;
  mtime: number;
  atime: number;
}

export interface FileExplorerProps {
  activeSessionId: string | null;
  activeSessionType: SessionType | null;
  activeConnectionId?: string | null;
  activeSessionName?: string | null;
}

export interface WebdavSyncSettings {
  endpoint: string;
  root: string;
  username: string;
  password?: string | null;
}

export interface S3SyncSettings {
  endpoint: string;
  bucket: string;
  region: string;
  root: string;
  access_key_id?: string | null;
  secret_access_key?: string | null;
  session_token?: string | null;
  virtual_host_style: boolean;
}

export interface GiteeSnippetSyncSettings {
  api_endpoint: string;
  gist_id: string;
  access_token?: string | null;
}

export interface OAuthDriveSyncSettings {
  root: string;
  access_token?: string | null;
  refresh_token?: string | null;
  client_id?: string | null;
  client_secret?: string | null;
}

export interface AliyunDriveSyncSettings {
  root: string;
  access_token?: string | null;
  refresh_token?: string | null;
  client_id?: string | null;
  client_secret?: string | null;
  drive_type: string;
}

export interface GithubGistSyncSettings {
  gist_id: string;
  access_token?: string | null;
}

export interface CloudSyncSettings {
  enabled: boolean;
  provider: string;
  remote_root: string;
  device_name: string;
  auto_check_on_startup: boolean;
  auto_push_on_change: boolean;
  auto_pull_remote_changes: boolean;
  sync_debounce_seconds: number;
  webdav: WebdavSyncSettings;
  s3: S3SyncSettings;
  gitee_snippet: GiteeSnippetSyncSettings;
  google_drive: OAuthDriveSyncSettings;
  onedrive: OAuthDriveSyncSettings;
  aliyun_drive: AliyunDriveSyncSettings;
  github_gist: GithubGistSyncSettings;
}

export interface GithubGistDeviceFlowStart {
  flow_id: string;
  user_code: string;
  verification_uri: string;
  expires_in: number;
  interval: number;
}

export interface GithubGistDeviceFlowPoll {
  state: "pending" | "slow_down" | "success" | "expired" | "denied" | "error";
  access_token?: string | null;
  scope?: string | null;
  login?: string | null;
  gist_id?: string | null;
  interval?: number | null;
  message?: string | null;
}

export interface CloudConflictPreview {
  detected_at_ms: number;
  provider: string;
  kind?: "content_conflict" | "remote_inconsistent";
  local_payload_hash: string;
  remote_payload_hash: string;
  remote_revision: string;
  remote_created_at_ms: number;
  remote_device_id: string;
  recovery_revision?: string | null;
  recovery_payload_hash?: string | null;
  recovery_created_at_ms?: number | null;
  message: string;
}

export interface CloudSyncStatus {
  enabled: boolean;
  provider: string;
  state: string;
  message: string;
  current_operation?: string | null;
  last_checked_at_ms?: number | null;
  last_synced_at_ms?: number | null;
  conflict?: CloudConflictPreview | null;
}

export interface CloudSyncHistoryEntry {
  id: string;
  timestamp_ms: number;
  kind: string;
  status: string;
  trigger: string;
  provider?: string | null;
  revision?: string | null;
  duration_ms?: number | null;
  message: string;
}

// ── SSH Config Import ─────────────────────────────────────────────────────────

export interface SshConfigHop {
  host: string;
  port: number;
  user: string;
  isTarget: boolean;
}

export interface SshConfigEntry {
  alias: string;
  host: string;
  port: number;
  user: string;
  identityFile?: string | null;
  proxyJump?: string | null;
  hops: SshConfigHop[];
  hostKeyAlias?: string | null;
}
