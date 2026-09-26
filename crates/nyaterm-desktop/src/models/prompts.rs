use nyaterm_core::DiagnosticsExportInfo;
use nyaterm_store::ConfigBackupInfo;
use std::path::PathBuf;

#[derive(Clone, Default, PartialEq, Eq)]
pub(crate) struct GithubGistAuthState {
    pub(crate) pending: bool,
    pub(crate) user_code: Option<String>,
    pub(crate) verification_uri: Option<String>,
    pub(crate) login: Option<String>,
    pub(crate) message: Option<String>,
}

pub(crate) enum GithubGistAuthEvent {
    Started {
        user_code: String,
        verification_uri: String,
    },
    Polling {
        slow_down: bool,
    },
    Succeeded {
        access_token: nyaterm_core::SecretString,
        gist_id: String,
        login: String,
    },
    Failed(String),
    Cancelled,
}

#[derive(Debug)]
pub(crate) struct GithubGistAuthJobEvent {
    pub(crate) job_id: u64,
    pub(crate) event: GithubGistAuthEvent,
}

impl std::fmt::Debug for GithubGistAuthState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GithubGistAuthState")
            .field("pending", &self.pending)
            .field("user_code", &self.user_code.as_ref().map(|_| "<redacted>"))
            .field("verification_uri", &self.verification_uri)
            .field("login", &self.login)
            .field("message", &self.message)
            .finish()
    }
}

impl std::fmt::Debug for GithubGistAuthEvent {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Started {
                user_code: _,
                verification_uri,
            } => formatter
                .debug_struct("Started")
                .field("user_code", &"<redacted>")
                .field("verification_uri", verification_uri)
                .finish(),
            Self::Polling { slow_down } => formatter
                .debug_struct("Polling")
                .field("slow_down", slow_down)
                .finish(),
            Self::Succeeded {
                access_token: _,
                gist_id,
                login,
            } => formatter
                .debug_struct("Succeeded")
                .field("access_token", &"<redacted>")
                .field("gist_id", gist_id)
                .field("login", login)
                .finish(),
            Self::Failed(_) => formatter.write_str("Failed(<redacted>)"),
            Self::Cancelled => formatter.write_str("Cancelled"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CloudSyncInputField {
    RemoteRoot,
    DeviceName,
    WebdavEndpoint,
    WebdavRoot,
    WebdavUsername,
    WebdavPassword,
    S3Endpoint,
    S3Bucket,
    S3Region,
    S3Root,
    S3AccessKeyId,
    S3SecretAccessKey,
    S3SessionToken,
    GoogleDriveRoot,
    GoogleDriveAccessToken,
    GoogleDriveRefreshToken,
    GoogleDriveClientId,
    GoogleDriveClientSecret,
    OneDriveRoot,
    OneDriveAccessToken,
    OneDriveRefreshToken,
    OneDriveClientId,
    OneDriveClientSecret,
    AliyunDriveRoot,
    AliyunDriveType,
    AliyunDriveAccessToken,
    AliyunDriveRefreshToken,
    AliyunDriveClientId,
    AliyunDriveClientSecret,
    GiteeEndpoint,
    GiteeGistId,
    GiteeToken,
    GithubGistId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AiInputField {
    Model,
    BaseUrl,
    ApiKey,
    RequestUserAgent,
    CodexExecutable,
    CodexDefaultModel,
    CodexConfigDirectory,
    ClaudeExecutable,
    ClaudeDefaultModel,
    ClaudeConfigDirectory,
}

impl CloudSyncInputField {
    /// Every variant, so a text-input id can be mapped back to its field.
    pub(crate) const ALL: [Self; 33] = [
        Self::RemoteRoot,
        Self::DeviceName,
        Self::WebdavEndpoint,
        Self::WebdavRoot,
        Self::WebdavUsername,
        Self::WebdavPassword,
        Self::S3Endpoint,
        Self::S3Bucket,
        Self::S3Region,
        Self::S3Root,
        Self::S3AccessKeyId,
        Self::S3SecretAccessKey,
        Self::S3SessionToken,
        Self::GoogleDriveRoot,
        Self::GoogleDriveAccessToken,
        Self::GoogleDriveRefreshToken,
        Self::GoogleDriveClientId,
        Self::GoogleDriveClientSecret,
        Self::OneDriveRoot,
        Self::OneDriveAccessToken,
        Self::OneDriveRefreshToken,
        Self::OneDriveClientId,
        Self::OneDriveClientSecret,
        Self::AliyunDriveRoot,
        Self::AliyunDriveType,
        Self::AliyunDriveAccessToken,
        Self::AliyunDriveRefreshToken,
        Self::AliyunDriveClientId,
        Self::AliyunDriveClientSecret,
        Self::GiteeEndpoint,
        Self::GiteeGistId,
        Self::GiteeToken,
        Self::GithubGistId,
    ];

    /// The stable part of this field's text-input id.
    pub(crate) fn input_key(self) -> &'static str {
        match self {
            Self::RemoteRoot => "remote-root",
            Self::DeviceName => "device-name",
            Self::WebdavEndpoint => "webdav-endpoint",
            Self::WebdavRoot => "webdav-root",
            Self::WebdavUsername => "webdav-username",
            Self::WebdavPassword => "webdav-password",
            Self::S3Endpoint => "s3-endpoint",
            Self::S3Bucket => "s3-bucket",
            Self::S3Region => "s3-region",
            Self::S3Root => "s3-root",
            Self::S3AccessKeyId => "s3-access-key-id",
            Self::S3SecretAccessKey => "s3-secret-access-key",
            Self::S3SessionToken => "s3-session-token",
            Self::GoogleDriveRoot => "google-drive-root",
            Self::GoogleDriveAccessToken => "google-drive-access-token",
            Self::GoogleDriveRefreshToken => "google-drive-refresh-token",
            Self::GoogleDriveClientId => "google-drive-client-id",
            Self::GoogleDriveClientSecret => "google-drive-client-secret",
            Self::OneDriveRoot => "one-drive-root",
            Self::OneDriveAccessToken => "one-drive-access-token",
            Self::OneDriveRefreshToken => "one-drive-refresh-token",
            Self::OneDriveClientId => "one-drive-client-id",
            Self::OneDriveClientSecret => "one-drive-client-secret",
            Self::AliyunDriveRoot => "aliyun-drive-root",
            Self::AliyunDriveType => "aliyun-drive-type",
            Self::AliyunDriveAccessToken => "aliyun-drive-access-token",
            Self::AliyunDriveRefreshToken => "aliyun-drive-refresh-token",
            Self::AliyunDriveClientId => "aliyun-drive-client-id",
            Self::AliyunDriveClientSecret => "aliyun-drive-client-secret",
            Self::GiteeEndpoint => "gitee-endpoint",
            Self::GiteeGistId => "gitee-gist-id",
            Self::GiteeToken => "gitee-token",
            Self::GithubGistId => "github-gist-id",
        }
    }

    /// The field an input id names, or `None` if it names no field here.
    pub(crate) fn from_input_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|field| field.input_key() == key)
    }

    /// Whether this field holds a secret, and so is masked and never shown back.
    pub(crate) fn is_secret(self) -> bool {
        let key = self.input_key();
        key.contains("password")
            || key.contains("token")
            || key.contains("secret")
            || key.contains("access-key-id")
    }
}

impl AiInputField {
    /// Every variant, so a text-input id can be mapped back to its field.
    pub(crate) const ALL: [Self; 10] = [
        Self::Model,
        Self::BaseUrl,
        Self::ApiKey,
        Self::RequestUserAgent,
        Self::CodexExecutable,
        Self::CodexDefaultModel,
        Self::CodexConfigDirectory,
        Self::ClaudeExecutable,
        Self::ClaudeDefaultModel,
        Self::ClaudeConfigDirectory,
    ];

    /// The stable part of this field's text-input id.
    pub(crate) fn input_key(self) -> &'static str {
        match self {
            Self::Model => "model",
            Self::BaseUrl => "base-url",
            Self::ApiKey => "api-key",
            Self::RequestUserAgent => "request-user-agent",
            Self::CodexExecutable => "codex-executable",
            Self::CodexDefaultModel => "codex-default-model",
            Self::CodexConfigDirectory => "codex-config-directory",
            Self::ClaudeExecutable => "claude-executable",
            Self::ClaudeDefaultModel => "claude-default-model",
            Self::ClaudeConfigDirectory => "claude-config-directory",
        }
    }

    /// The field an input id names, or `None` if it names no field here.
    pub(crate) fn from_input_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|field| field.input_key() == key)
    }
}

impl TranslateInputField {
    /// Every variant, so a text-input id can be mapped back to its field.
    pub(crate) const ALL: [Self; 9] = [
        Self::TargetLanguage,
        Self::Text,
        Self::DeeplApiKey,
        Self::BaiduAppId,
        Self::BaiduAppKey,
        Self::AliAppId,
        Self::AliAppKey,
        Self::YoudaoAppId,
        Self::YoudaoAppKey,
    ];

    /// The stable part of this field's text-input id.
    pub(crate) fn input_key(self) -> &'static str {
        match self {
            Self::TargetLanguage => "target-language",
            Self::Text => "text",
            Self::DeeplApiKey => "deepl-api-key",
            Self::BaiduAppId => "baidu-app-id",
            Self::BaiduAppKey => "baidu-app-key",
            Self::AliAppId => "ali-app-id",
            Self::AliAppKey => "ali-app-key",
            Self::YoudaoAppId => "youdao-app-id",
            Self::YoudaoAppKey => "youdao-app-key",
        }
    }

    /// The field an input id names, or `None` if it names no field here.
    pub(crate) fn from_input_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|field| field.input_key() == key)
    }

    /// Whether this field holds a secret, and so is masked and never shown back.
    pub(crate) fn is_secret(self) -> bool {
        let key = self.input_key();
        key.contains("api-key") || key.contains("app-key")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AiActionListKind {
    Terminal,
    File,
}

impl AiActionListKind {
    pub(crate) fn input_key(self) -> &'static str {
        match self {
            Self::Terminal => "terminal",
            Self::File => "file",
        }
    }

    pub(crate) fn from_input_key(value: &str) -> Option<Self> {
        match value {
            "terminal" => Some(Self::Terminal),
            "file" => Some(Self::File),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AiActionEditorField {
    Name,
    Prompt,
}

impl AiActionEditorField {
    pub(crate) fn next(self) -> Self {
        match self {
            Self::Name => Self::Prompt,
            Self::Prompt => Self::Name,
        }
    }

    pub(crate) fn input_key(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Prompt => "prompt",
        }
    }

    pub(crate) fn from_input_key(value: &str) -> Option<Self> {
        match value {
            "name" => Some(Self::Name),
            "prompt" => Some(Self::Prompt),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TranslateInputField {
    TargetLanguage,
    Text,
    DeeplApiKey,
    BaiduAppId,
    BaiduAppKey,
    AliAppId,
    AliAppKey,
    YoudaoAppId,
    YoudaoAppKey,
}

impl TranslateInputField {
    pub(crate) fn is_settings_field(self) -> bool {
        matches!(
            self,
            Self::DeeplApiKey
                | Self::BaiduAppId
                | Self::BaiduAppKey
                | Self::AliAppId
                | Self::AliAppKey
                | Self::YoudaoAppId
                | Self::YoudaoAppKey
        )
    }
}

#[derive(Clone, Default, PartialEq, Eq)]
pub(crate) struct CloudSyncSecretDraft {
    pub(crate) webdav_password: nyaterm_core::SecretString,
    pub(crate) s3_access_key_id: nyaterm_core::SecretString,
    pub(crate) s3_secret_access_key: nyaterm_core::SecretString,
    pub(crate) s3_session_token: nyaterm_core::SecretString,
    pub(crate) google_drive_access_token: nyaterm_core::SecretString,
    pub(crate) google_drive_refresh_token: nyaterm_core::SecretString,
    pub(crate) google_drive_client_secret: nyaterm_core::SecretString,
    pub(crate) onedrive_access_token: nyaterm_core::SecretString,
    pub(crate) onedrive_refresh_token: nyaterm_core::SecretString,
    pub(crate) onedrive_client_secret: nyaterm_core::SecretString,
    pub(crate) aliyun_drive_access_token: nyaterm_core::SecretString,
    pub(crate) aliyun_drive_refresh_token: nyaterm_core::SecretString,
    pub(crate) aliyun_drive_client_secret: nyaterm_core::SecretString,
    pub(crate) gitee_token: nyaterm_core::SecretString,
    pub(crate) github_token: nyaterm_core::SecretString,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub(crate) struct TranslationSecretDraft {
    pub(crate) deepl_api_key: nyaterm_core::SecretString,
    pub(crate) baidu_app_key: nyaterm_core::SecretString,
    pub(crate) ali_app_key: nyaterm_core::SecretString,
    pub(crate) youdao_app_key: nyaterm_core::SecretString,
}

impl std::fmt::Debug for CloudSyncSecretDraft {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("CloudSyncSecretDraft(<redacted>)")
    }
}

impl std::fmt::Debug for TranslationSecretDraft {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("TranslationSecretDraft(<redacted>)")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransferPathPromptKind {
    UploadFile,
    UploadDirectory,
    UploadDirectoryContents,
    DownloadDirectory,
}

#[derive(Debug)]
pub(crate) enum TransferPathPromptResult {
    Selected(Vec<PathBuf>),
    Cancelled,
    Failed(String),
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecordingPathPromptKind {
    Start,
    SaveTranscript,
}

#[derive(Debug)]
pub(crate) enum RecordingPathPromptResult {
    Selected(PathBuf),
    Cancelled,
    Failed(String),
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConfigPathPromptKind {
    EncryptedPortableExport,
    EncryptedPortableImport,
}

#[derive(Debug)]
pub(crate) enum ConfigPathPromptResult {
    Exported(ConfigBackupInfo),
    Imported(ConfigBackupInfo),
    Cancelled,
    Failed(String),
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SnapshotPasswordPromptKind {
    Export,
    Import,
    CloudForcePush,
    CloudForcePull,
    CloudProviderPush,
    CloudProviderPull,
    CloudProviderForcePush,
    CloudProviderForcePull,
    CloudRecoverCurrent,
    CloudProviderRecoverCurrent,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct SnapshotPasswordPromptState {
    pub(crate) kind: SnapshotPasswordPromptKind,
    pub(crate) value: nyaterm_core::SecretString,
}

impl std::fmt::Debug for SnapshotPasswordPromptState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SnapshotPasswordPromptState")
            .field("kind", &self.kind)
            .field("value", &"[REDACTED]")
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CloudSyncConflictState {
    pub(crate) preview: nyaterm_core::CloudConflictPreview,
    pub(crate) provider_action: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DiagnosticsPathPromptKind {
    Export,
}

#[derive(Debug)]
pub(crate) enum DiagnosticsPathPromptResult {
    Exported(DiagnosticsExportInfo),
    Cancelled,
    Failed(String),
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeywordHighlightPathPromptKind {
    Import,
}

#[derive(Debug)]
pub(crate) enum KeywordHighlightPathPromptResult {
    Imported {
        imported_rules: usize,
        updated_rules: usize,
        total_rules: usize,
    },
    Cancelled,
    Failed(String),
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConnectionImportSource {
    NyatermBackup,
    Xshell,
    MobaXterm,
    WindTerm,
    SecureCrt,
    FinalShell,
    Termius,
    SshConfig,
    Electerm,
    NyatermJson,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QuickCommandImportPathPromptKind {
    NyatermJson,
    WindTermQuickbar,
    XshellXts,
}

#[derive(Debug)]
pub(crate) enum QuickCommandImportPathPromptResult {
    Imported {
        imported_commands: usize,
        imported_categories: usize,
        updated_commands: usize,
        total_commands: usize,
        total_categories: usize,
    },
    Cancelled,
    Failed(String),
    Closed,
}

#[cfg(test)]
mod snapshot_password_prompt_tests {
    use super::{
        CloudSyncSecretDraft, GithubGistAuthEvent, SnapshotPasswordPromptKind,
        SnapshotPasswordPromptState, TranslationSecretDraft,
    };

    #[test]
    fn snapshot_password_prompt_debug_redacts_the_password() {
        let state = SnapshotPasswordPromptState {
            kind: SnapshotPasswordPromptKind::CloudProviderPush,
            value: "snapshot-secret".to_string().into(),
        };

        let debug = format!("{state:?}");
        assert!(!debug.contains("snapshot-secret"));
        assert!(debug.contains("[REDACTED]"));
    }

    #[test]
    fn cloud_and_oauth_debug_output_redacts_secrets() {
        let secret = "nya-desktop-secret-never-log";
        let cloud = CloudSyncSecretDraft {
            github_token: secret.to_string().into(),
            ..CloudSyncSecretDraft::default()
        };
        let translation = TranslationSecretDraft {
            deepl_api_key: secret.to_string().into(),
            ..TranslationSecretDraft::default()
        };
        let oauth = GithubGistAuthEvent::Succeeded {
            access_token: secret.to_string().into(),
            gist_id: "gist".to_string(),
            login: "user".to_string(),
        };

        for output in [
            format!("{cloud:?}"),
            format!("{translation:?}"),
            format!("{oauth:?}"),
        ] {
            assert!(!output.contains(secret));
            assert!(output.contains("redacted"));
        }
    }
}
