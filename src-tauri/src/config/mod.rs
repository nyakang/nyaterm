//! Config persistence for sessions, UI, and quick commands.
//!
//! Stores typed entities and small singleton documents in `~/.nyaterm/nyaterm.redb`.
//! Credentials are AES-256-GCM encrypted in-place.

mod cloud_sync;
mod connection;
mod credential;
mod key;
mod note;
mod otp;
mod password;
mod proxy;
mod quick_command;
mod settings;
mod tunnel;
mod ui;

#[allow(unused_imports)]
pub use cloud_sync::{
    CloudConflictPreview, CloudSyncHistoryEntry, CloudSyncSettings, CloudSyncState,
    CloudSyncStatus, GiteeSnippetSyncSettings, MASKED_SECRET_VALUE, S3SyncSettings,
    WebdavSyncSettings, decrypt_cloud_sync_settings, encrypt_cloud_sync_settings,
    load_cloud_sync_settings, load_cloud_sync_state, mask_cloud_sync_settings,
    merge_masked_cloud_sync_settings, save_cloud_sync_state,
};
#[allow(unused_imports)]
pub use connection::{
    AiExecutionProfile, AppConfig, AssetAccelerator, AssetAcceleratorType, AssetDeviceType,
    AssetDisk, AssetDiskKind, AssetDiskPurpose, AssetMetadata, ConnectionAuth, ConnectionNetwork,
    ConnectionType, Group, MAX_SFTP_SHELL_DETECTION_TIMEOUT_MS,
    MIN_SFTP_SHELL_DETECTION_TIMEOUT_MS, SavedConnection, SessionsConfig, SftpCwdFollowMode,
    SftpSettings, SshAlgorithmMode, SshAlgorithmPreferences, TelnetAutoLoginConfig, load_config,
    load_connection_by_id, load_sessions, resolve_connection_encoding, save_config, save_sessions,
};
#[allow(unused_imports)]
pub use credential::{
    CredentialsConfig, SavedCredential, load_credential_by_id, load_credentials,
    reorder_credentials, save_credentials, upsert_credential,
};
#[allow(unused_imports)]
pub use key::{
    KeysConfig, SshKey, decrypt_key_cert, decrypt_key_pem, load_key_by_id, load_keys, save_keys,
};
#[allow(unused_imports)]
pub use note::{
    DeleteNoteNodeResult, NoteDocument, NoteFolder, NoteNodeChange, NoteSummary, NoteTreePayload,
    NoteUpdateResult, NotesChangedEvent, NotesSnapshot,
};
#[allow(unused_imports)]
pub use otp::{OtpConfig, OtpEntry, load_otp_entries, load_otp_entry_by_id, save_otp_entries};
#[allow(unused_imports)]
pub use password::{
    PasswordsConfig, SavedPassword, load_password_by_id, load_passwords, save_passwords,
};
#[allow(unused_imports)]
pub use proxy::{
    ProxyConfig, ProxyGroup, ProxyGroupsConfig, load_proxies, load_proxy_by_id, load_proxy_groups,
    save_proxies, save_proxy_groups,
};
#[allow(unused_imports)]
pub use quick_command::{
    QuickCommand, QuickCommandCategory, QuickCommandsConfig, load_quick_commands,
    save_quick_commands,
};
#[allow(unused_imports)]
pub use settings::{
    AI_REQUEST_USER_AGENT_DEFAULT, ActionLinksMatcherSettings, AgentCommandExecutionMode,
    AiAgentKind, AiBackendKind, AiCustomActionConfig, AiMode, AiModelConfigItem, AiModelSource,
    AiPermissionMode, AiProviderCredential, AiProviderKind, AiProviderProfile, AiReasoningEffort,
    AiSettings, AppSettings, AppearanceSettings, ClaudeCodeIntegrationSettings,
    CodexIntegrationSettings, CodexThreadMode, DiagnosticsLogLevel, DiagnosticsSettings,
    GeneralSettings, InteractionSettings, KeywordHighlightRule, ProxySettings, RiskLevel,
    SearchEngine, SearchSettings, SecuritySettings, TerminalColorsConfig, TerminalSettings,
    ThemeColorsConfig, ThemeConfig, TransferSettings, TranslationSettings,
    ai_model_id_for_credential, ai_model_id_for_provider, decrypt_ai_settings, encrypt_ai_settings,
    load_app_settings, mask_ai_settings, merge_masked_ai_settings, normalize_ai_settings,
    save_app_settings,
};
#[allow(unused_imports)]
pub use tunnel::{
    TunnelConfig, TunnelGroup, TunnelGroupsConfig, TunnelsConfig, load_tunnel_groups, load_tunnels,
    save_tunnel_groups, save_tunnels,
};
#[allow(unused_imports)]
pub use ui::{ActivityBarLayout, RestorablePaneNode, RestorableTab, UiConfig};

pub(crate) fn uuid_v4() -> String {
    uuid::Uuid::new_v4().to_string()
}

pub(crate) fn default_true() -> bool {
    true
}

pub(crate) fn default_false() -> bool {
    false
}
