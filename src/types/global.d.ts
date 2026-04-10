/** Type of terminal session: SSH remote or local shell. */
export type SessionType = "SSH" | "Local";

/** Metadata for a connected or disconnected session. */
export interface SessionInfo {
  id: string;
  name: string;
  session_type: SessionType;
  connected: boolean;
  /** True when OSC 7 shell integration was injected. False for non-standard shells (JumpServer etc.). */
  injection_active: boolean;
}

/** UI tab representing a terminal session. */
export interface Tab {
  id: string;
  sessionId: string;
  name: string;
  type: SessionType;
  connectionId?: string;
  /** True while the backend session is being established. XTerminal is not rendered yet. */
  connecting?: boolean;
}

/** SSH connection config for creating a session. */
export interface SshConfig {
  name: string;
  host: string;
  port: number;
  username: string;
  auth: SshAuth;
}

/** SSH authentication: password or private key (PEM content). */
export type SshAuth =
  | { type: "password"; password: string }
  | { type: "key"; key_data: string; passphrase?: string };

/** Group for organizing saved connections. Groups form a tree via parent_id. */
export interface Group {
  id: string;
  name: string;
  parent_id?: string;
  sort_order: number;
}

/** Managed SSH private key stored in keys.json. */
export interface SshKey {
  id: string;
  name: string;
  /** True when encrypted key data exists on disk. */
  has_key_data?: boolean;
  /** Transient: file path from the UI file picker. */
  key_file_path?: string;
  /** Passphrase for this key (only sent when creating/updating). */
  passphrase?: string;
}

/** Managed password entry stored in passwords.json. */
export interface SavedPassword {
  id: string;
  name: string;
  /** True when encrypted password data exists on disk. */
  has_password?: boolean;
  /** Plaintext password (only sent when creating/updating). */
  password?: string;
}

/** Stored SSH connection with host, auth, and optional group. */
export interface SavedConnection {
  id: string;
  name: string;
  group_id?: string;
  description?: string;
  host: string;
  port: number;
  username: string;
  auth_type: string;
  /** References a managed password by id. */
  password_id?: string;
  /** References a managed SSH key by id. */
  key_id?: string;
  sort_order?: number;
  /** Icon key referencing a named icon from QUICK_ICONS (e.g. "docker", "ubuntu"). */
  icon?: string;
  /** References a standalone proxy config by id. */
  proxy_id?: string;
  /** References an OTP entry for two-factor authentication. */
  otp_id?: string;
  /** When true, auto-fill the OTP code during keyboard-interactive auth. */
  auto_fill_otp?: boolean;
  network?: ConnectionNetworkSettings;
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
  /** True when encrypted secret data exists on disk. */
  has_secret?: boolean;
}

/** Result of generating an OTP code. */
export interface OtpCodeResult {
  code: string;
  remainingSeconds: number;
}

/** Saved tab state for startup restoration. */
export interface RestorableTab {
  title: string;
  session_type: string;
  connection_id?: string;
}

export type LeftPanelId = "fileExplorer" | "fileTransfer" | "securityAuth";

export type RightPanelId =
  | "savedConnections"
  | "activeSessions"
  | "commandHistory"
  | "resourceMonitor";

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
export interface UiConfig {
  open_tabs: RestorableTab[];
  left_width: number;
  right_width: number;
  quick_cmd_height: number;
  /** ID of whichever panel is currently open on the left side. */
  active_left_panel: string | null;
  /** ID of whichever panel is currently open on the right side. */
  active_right_panel: string | null;
  show_quick_cmd_bar: boolean;
  zoom_level: number;
  language?: string;
  show_remote_stats: boolean;
  remote_stats_interval: number;
  saved_connections_sort_mode?: string;
  transfer_height: number;
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
}

export interface QuickCommandsConfig {
  commands: QuickCommand[];
  categories: QuickCommandCategory[];
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
  default_local_shell: string;
  minimize_to_tray: boolean;
  boss_key: string | null;
  confirm_on_close: boolean;
}

export interface AppearanceSettings {
  theme: string;
  font_family: string;
  font_size: number;
  ligatures: boolean;
  background_opacity: number;
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

export interface ConnectionNetworkSettings {
  proxy?: ProxySettings;
}

export interface SearchEngine {
  name: string;
  url_template: string;
  icon?: string;
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
  require_master_password: boolean;
  enable_screen_lock: boolean;
  idle_lock_minutes: number;
  lock_password?: string;
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

export interface TerminalSettings {
  scrollback_lines: number;
  keep_alive_interval: number;
  hardware_acceleration: boolean;
  keyword_highlights_enabled: boolean;
  keyword_highlights_across_wrapped_lines: boolean;
  keyword_highlights: KeywordHighlightRule[];
  action_links_enabled: boolean;
  action_links_matchers: ActionLinksMatcherSettings;
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
}

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
  ui: UiConfig;
}

export interface FileEntry {
  name: string;
  is_dir: boolean;
  is_symlink: boolean;
  size: number;
  permissions: string;
}

export interface FileExplorerProps {
  activeSessionId: string | null;
}
