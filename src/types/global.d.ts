/** Type of terminal session. */
export type SessionType = "SSH" | "Local" | "Telnet" | "Serial";

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

/** AI Agent command execution wrapper profile. */
export type AIExecutionProfile = "auto" | "posix" | "powershell" | "cmd" | "send_only" | "disabled";

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
export type ConnectionTypeTag = "ssh" | "local_terminal" | "telnet" | "serial";

/** Metadata for a connected or disconnected session. */
export interface SessionInfo {
  id: string;
  name: string;
  session_type: SessionType;
  connected: boolean;
  owner_window_label?: string | null;
  ai_execution_profile: AIExecutionProfile;
  /** True when backend terminal-path tracking is available for this session. */
  injection_active: boolean;
}

/** Leaf node representing one terminal session inside a workspace tab. */
export interface SessionPane {
  id: string;
  kind: "leaf";
  sessionId: string;
  name: string;
  type: SessionType;
  connectionId?: string;
  /** True while the backend session is being established. XTerminal is not rendered yet. */
  connecting?: boolean;
  /** Populated when session creation failed and the pane should stay visible as an error state. */
  connectError?: string;
}

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
}

/** SSH connection config for creating a session. */
export interface SshConfig {
  name: string;
  host: string;
  port: number;
  username: string;
  auth: SshAuth;
}

/** SSH authentication: none, password, or private key (PEM content). */
export type SshAuth =
  | { type: "none" }
  | { type: "password"; password: string }
  | { type: "key"; key_data: string; passphrase?: string };

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
  /** True when encrypted key data exists in local storage. */
  has_key_data?: boolean;
  /** Transient: file path from the UI file picker. */
  key_file_path?: string;
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
  auth?: ConnectionAuth;
  network?: ConnectionNetwork;
  post_login?: ConnectionPostLogin;
  /** SSH-specific fields (present when type === "ssh"). */
  host?: string;
  port?: number;
  username?: string;
  /** Local terminal fields (present when type === "local_terminal"). */
  shell_path?: string;
  shell_args?: string;
  working_dir?: string;
  /** Legacy saved value; runtime sessions now resolve the effective AI execution profile automatically. */
  ai_execution_profile?: AIExecutionProfile;
  /** Serial fields (present when type === "serial"). */
  port_name?: string;
  baud_rate?: number;
  data_bits?: number;
  parity?: string;
  stop_bits?: string;
  /** Backspace key mode for serial/telnet connections ("ctrl_h" or "del"). */
  backspace_mode?: string;
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
  title: string;
  session_type: SessionType | "local";
  connection_id?: string;
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
}

export type LeftPanelId = "fileExplorer" | "network" | "securityAuth" | "syncBackupHistory";

export type RightPanelId =
  | "savedConnections"
  | "aiAssistant"
  | "activeSessions"
  | "commandHistory"
  | "resourceMonitor"
  | "recording"
  | "syncBackupHistory";

export type ActivityBarZone = "left_top" | "left_bottom" | "right_top" | "right_bottom";

export interface ActivityBarLayout {
  left_top: string[];
  left_bottom: string[];
  right_top: string[];
  right_bottom: string[];
  /** When true every activity bar icon shows its name below the icon. */
  show_labels: boolean;
}

/** Layout preferences: panel widths, active panels, theme. */
export type QuickCommandViewMode = "list" | "tile";

export interface UiConfig {
  open_tabs: RestorableTab[];
  left_width: number;
  right_width: number;
  quick_cmd_height: number;
  quick_cmd_view_mode: QuickCommandViewMode;
  /** ID of whichever panel is currently open on the left side. */
  active_left_panel: string | null;
  /** ID of whichever panel is currently open on the right side. */
  active_right_panel: string | null;
  show_quick_cmd_bar: boolean;
  show_serial_send_panel: boolean;
  serial_send_height: number;
  zoom_level: number;
  language?: string;
  show_remote_stats: boolean;
  remote_stats_interval: number;
  saved_connections_sort_mode?: string;
  recent_connection_ids: string[];
  transfer_height: number;
  file_explorer_auto_sync_cwd_connection_ids: string[];
  file_explorer_favorite_dirs_by_connection_id: Record<string, string[]>;
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
  usage: number;
  per_core: number[];
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
  disks: RemoteStatsDisk[];
}

/** Labeled command shortcut for quick execution. */
export interface QuickCommandCategory {
  id: string;
  name: string;
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
}

export interface QuickCommandsConfig {
  commands: QuickCommand[];
  categories: QuickCommandCategory[];
}

export type QuickCommandImportSource = "windterm_quickbar" | "xshell_xts" | "nyaterm_json";

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
  minimize_to_tray: boolean;
  boss_key: string | null;
  confirm_on_close: boolean;
}

export type BackgroundImageFit = "cover" | "contain" | "stretch" | "tile";

export interface AppearanceSettings {
  theme: string;
  font_family: string;
  ui_font_family: string;
  font_size: number;
  ligatures: boolean;
  background_opacity: number;
  background_image_path: string | null;
  background_image_fit: BackgroundImageFit;
  background_image_opacity: number;
  cursor_style: string;
  cursor_blink: boolean;
  ui_font_size: number;
  terminal_theme: string | null;
}

export interface ProxySettings {
  enabled: boolean;
  protocol: string;
  host: string;
  port: number;
}

export interface ProxyConfig {
  id: string;
  name: string;
  protocol: string;
  host: string;
  port: number;
  username?: string;
  password?: string;
  password_id?: string;
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
  enable_screen_lock: boolean;
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

export interface ActionLinksMatcherSettings {
  ipv4: boolean;
  archive: boolean;
  host_port: boolean;
}

export type KeywordHighlightBuiltinRuleSettings = Record<string, boolean>;

export interface TerminalSettings {
  scrollback_lines: number;
  keep_alive_interval: number;
  hardware_acceleration: boolean;
  keyword_highlights_enabled: boolean;
  keyword_highlights_across_wrapped_lines: boolean;
  keyword_highlight_builtin_rules: KeywordHighlightBuiltinRuleSettings;
  keyword_highlights: KeywordHighlightRule[];
  action_links_enabled: boolean;
  action_links_matchers: ActionLinksMatcherSettings;
  show_line_numbers: boolean;
  show_timestamps: boolean;
  show_multi_line_paste_dialog: boolean;
}

export interface TransferSettings {
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
  recording_memory_limit_bytes: number;
}

export type DiagnosticsLogLevel = "warn" | "info" | "debug";

export interface DiagnosticsSettings {
  level: DiagnosticsLogLevel;
  retention_days: number;
}

export type RiskLevel = "low" | "medium" | "high" | "critical";
export type AIMode = "ask" | "agent";
export type AIModelSource = "rust-genai" | "manual";

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
  provider_kind?: AIProviderKind | null;
  credential_id?: string | null;
  enabled: boolean;
  source: AIModelSource;
  last_seen_at?: string | null;
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
  active_profile_id: string;
  provider_profiles: AIProviderProfile[];
  default_mode: AIMode;
  default_model_id?: string | null;
  models: AIModelConfigItem[];
  provider_credentials: AIProviderCredential[];
  terminal_ai_actions: AICustomActionConfig[];
  file_ai_actions: AICustomActionConfig[];
  max_ai_file_size_bytes: number;
  max_agent_steps?: number | null;
  agent_step_timeout_ms?: number | null;
  terminal_output_lines: number;
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
  connectionId?: string | null;
  title: string;
  createdAt: string;
  updatedAt: string;
}

export interface AIStreamStart {
  streamId: string;
  sessionId: string;
}

export interface AIStreamEventPayload {
  type: "start" | "delta" | "reasoning_delta" | "done" | "error";
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
export type AgentStepStatus = "running" | "completed" | "needs_approval" | "rejected" | "failed";

export interface AgentStepAction {
  kind: AgentActionKind;
  command?: string | null;
  riskLevel?: RiskLevel | null;
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
}

export interface InteractionSettings {
  copy_on_select: boolean;
  right_click_paste: boolean;
  command_suggestions_enabled: boolean;
  command_suggestion_min_chars: number;
  command_suggestion_max_chars: number;
  word_separators: string;
  default_encoding: string;
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
}

export interface FileExplorerProps {
  activeSessionId: string | null;
  activeSessionType: SessionType | null;
  activeConnectionId?: string | null;
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

export interface CloudSyncSettings {
  enabled: boolean;
  provider: string;
  remote_root: string;
  device_name: string;
  auto_check_on_startup: boolean;
  auto_push_on_change: boolean;
  sync_debounce_seconds: number;
  scheduled_backup_enabled: boolean;
  backup_interval_hours: number;
  backup_retention_count: number;
  webdav: WebdavSyncSettings;
  s3: S3SyncSettings;
}

export interface CloudConflictPreview {
  detected_at_ms: number;
  provider: string;
  local_payload_hash: string;
  remote_payload_hash: string;
  remote_revision: string;
  remote_created_at_ms: number;
  remote_device_id: string;
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
  last_backup_at_ms?: number | null;
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

export interface RemoteBackupEntry {
  revision: string;
  created_at_ms: number;
  payload_hash: string;
  device_id: string;
  app_version: string;
  message: string;
}
