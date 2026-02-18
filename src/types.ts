/** Type of terminal session: SSH remote or local shell. */
export type SessionType = "SSH" | "Local";

/** Metadata for a connected or disconnected session. */
export interface SessionInfo {
  id: string;
  name: string;
  session_type: SessionType;
  connected: boolean;
}

/** UI tab representing a terminal session. */
export interface Tab {
  id: string;
  sessionId: string;
  name: string;
  type: SessionType;
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

/** Group for organizing saved connections. */
export interface Group {
  id: string;
  name: string;
  sort_order: number;
}

/** Stored SSH connection with host, auth, and optional group. */
export interface SavedConnection {
  id: string;
  name: string;
  group?: string;
  description?: string;
  host: string;
  port: number;
  username: string;
  auth_type: string;
  password?: string;
  passphrase?: string;
  /** File path selected via the file picker — backend reads and encrypts. */
  key_file_path?: string;
  /** True when an encrypted private key is already stored on disk. */
  has_key_data?: boolean;
}

/** Layout preferences: panel widths, visibility flags, theme. */
export interface UiConfig {
  left_width: number;
  right_width: number;
  saved_conn_height: number;
  history_height: number;
  quick_cmd_height: number;
  show_file_explorer: boolean;
  show_saved_connections: boolean;
  show_active_sessions: boolean;
  show_command_history: boolean;
  show_quick_commands: boolean;
  zoom_level: number;
  theme?: string;
}

/** Labeled command shortcut for quick execution. */
export interface QuickCommand {
  id: string;
  label: string;
  command: string;
}

/** Fuzzy search result with matched command and highlight indices. */
export interface FuzzyResult {
  command: string;
  score: number;
  indices: number[];
}
