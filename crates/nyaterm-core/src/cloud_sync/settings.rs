use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::SecretString;

pub const MASKED_SECRET_VALUE: &str = "__SET__";

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct WebdavSyncSettings {
    #[serde(default)]
    pub endpoint: String,
    #[serde(default)]
    pub root: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: Option<SecretString>,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct S3SyncSettings {
    #[serde(default)]
    pub endpoint: String,
    #[serde(default)]
    pub bucket: String,
    #[serde(default)]
    pub region: String,
    #[serde(default)]
    pub root: String,
    #[serde(default)]
    pub access_key_id: Option<SecretString>,
    #[serde(default)]
    pub secret_access_key: Option<SecretString>,
    #[serde(default)]
    pub session_token: Option<SecretString>,
    #[serde(default)]
    pub virtual_host_style: bool,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GiteeSnippetSyncSettings {
    #[serde(default = "default_gitee_api_endpoint")]
    pub api_endpoint: String,
    #[serde(default)]
    pub gist_id: String,
    #[serde(default)]
    pub access_token: Option<SecretString>,
}

impl Default for GiteeSnippetSyncSettings {
    fn default() -> Self {
        Self {
            api_endpoint: default_gitee_api_endpoint(),
            gist_id: String::new(),
            access_token: None,
        }
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct OAuthDriveSyncSettings {
    #[serde(default)]
    pub root: String,
    #[serde(default)]
    pub access_token: Option<SecretString>,
    #[serde(default)]
    pub refresh_token: Option<SecretString>,
    #[serde(default)]
    pub client_id: Option<String>,
    #[serde(default)]
    pub client_secret: Option<SecretString>,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AliyunDriveSyncSettings {
    #[serde(default)]
    pub root: String,
    #[serde(default)]
    pub access_token: Option<SecretString>,
    #[serde(default)]
    pub refresh_token: Option<SecretString>,
    #[serde(default)]
    pub client_id: Option<String>,
    #[serde(default)]
    pub client_secret: Option<SecretString>,
    #[serde(default = "default_aliyun_drive_type")]
    pub drive_type: String,
}

impl Default for AliyunDriveSyncSettings {
    fn default() -> Self {
        Self {
            root: String::new(),
            access_token: None,
            refresh_token: None,
            client_id: None,
            client_secret: None,
            drive_type: default_aliyun_drive_type(),
        }
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct GithubGistSyncSettings {
    #[serde(default)]
    pub gist_id: String,
    #[serde(default)]
    pub access_token: Option<SecretString>,
}

macro_rules! impl_redacted_sync_debug {
    ($type:ident, safe { $($safe:ident),* $(,)? }, secret { $($secret:ident),* $(,)? } $(, hidden { $($hidden:ident),* $(,)? })?) => {
        impl std::fmt::Debug for $type {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                let mut debug = formatter.debug_struct(stringify!($type));
                $(debug.field(stringify!($safe), &self.$safe);)*
                $(debug.field(
                    stringify!($secret),
                    &self.$secret.as_ref().map(|_| "<redacted>"),
                );)*
                $($(debug.field(stringify!($hidden), &"<redacted>");)*)?
                debug.finish()
            }
        }
    };
}

impl_redacted_sync_debug!(
    WebdavSyncSettings,
    safe { root, username },
    secret { password },
    hidden { endpoint }
);
impl_redacted_sync_debug!(
    S3SyncSettings,
    safe {
        bucket,
        region,
        root,
        virtual_host_style
    },
    secret {
        access_key_id,
        secret_access_key,
        session_token
    },
    hidden { endpoint }
);
impl_redacted_sync_debug!(
    GiteeSnippetSyncSettings,
    safe { gist_id },
    secret { access_token },
    hidden { api_endpoint }
);
impl_redacted_sync_debug!(
    OAuthDriveSyncSettings,
    safe { root, client_id },
    secret {
        access_token,
        refresh_token,
        client_secret
    }
);
impl_redacted_sync_debug!(
    AliyunDriveSyncSettings,
    safe {
        root,
        client_id,
        drive_type
    },
    secret {
        access_token,
        refresh_token,
        client_secret
    }
);
impl_redacted_sync_debug!(
    GithubGistSyncSettings,
    safe { gist_id },
    secret { access_token }
);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CloudSyncSettings {
    #[serde(default = "default_protocol_version")]
    pub protocol_version: u32,
    #[serde(default)]
    pub v4_scope: crate::sync_schema::SyncScope,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_provider")]
    pub provider: String,
    #[serde(default = "default_remote_root")]
    pub remote_root: String,
    #[serde(default = "default_device_name")]
    pub device_name: String,
    #[serde(default = "default_true")]
    pub auto_check_on_startup: bool,
    #[serde(default = "default_true")]
    pub auto_push_on_change: bool,
    #[serde(default = "default_true")]
    pub auto_pull_remote_changes: bool,
    #[serde(default = "default_sync_debounce_seconds")]
    pub sync_debounce_seconds: u64,
    #[serde(default)]
    pub webdav: WebdavSyncSettings,
    #[serde(default)]
    pub s3: S3SyncSettings,
    #[serde(default)]
    pub gitee_snippet: GiteeSnippetSyncSettings,
    #[serde(default)]
    pub google_drive: OAuthDriveSyncSettings,
    #[serde(default)]
    pub onedrive: OAuthDriveSyncSettings,
    #[serde(default)]
    pub aliyun_drive: AliyunDriveSyncSettings,
    #[serde(default)]
    pub github_gist: GithubGistSyncSettings,
}

impl Default for CloudSyncSettings {
    fn default() -> Self {
        Self {
            protocol_version: 3,
            v4_scope: crate::sync_schema::SyncScope::default(),
            enabled: false,
            provider: default_provider(),
            remote_root: default_remote_root(),
            device_name: default_device_name(),
            auto_check_on_startup: true,
            auto_push_on_change: true,
            auto_pull_remote_changes: true,
            sync_debounce_seconds: default_sync_debounce_seconds(),
            webdav: WebdavSyncSettings::default(),
            s3: S3SyncSettings::default(),
            gitee_snippet: GiteeSnippetSyncSettings::default(),
            google_drive: OAuthDriveSyncSettings::default(),
            onedrive: OAuthDriveSyncSettings::default(),
            aliyun_drive: AliyunDriveSyncSettings::default(),
            github_gist: GithubGistSyncSettings::default(),
        }
    }
}

fn default_protocol_version() -> u32 {
    3
}

pub fn mask_cloud_sync_settings(mut settings: CloudSyncSettings) -> CloudSyncSettings {
    settings.webdav.password = mask_secret(settings.webdav.password);
    settings.s3.access_key_id = mask_secret(settings.s3.access_key_id);
    settings.s3.secret_access_key = mask_secret(settings.s3.secret_access_key);
    settings.s3.session_token = mask_secret(settings.s3.session_token);
    settings.gitee_snippet.access_token = mask_secret(settings.gitee_snippet.access_token);
    mask_oauth_drive_settings(&mut settings.google_drive);
    mask_oauth_drive_settings(&mut settings.onedrive);
    mask_aliyun_drive_settings(&mut settings.aliyun_drive);
    settings.github_gist.access_token = mask_secret(settings.github_gist.access_token);
    settings
}

pub fn merge_masked_cloud_sync_settings(
    current: &CloudSyncSettings,
    mut next: CloudSyncSettings,
) -> CloudSyncSettings {
    next.webdav.password = merge_secret(
        current.webdav.password.as_ref(),
        next.webdav.password.as_ref(),
    );
    next.s3.access_key_id = merge_secret(
        current.s3.access_key_id.as_ref(),
        next.s3.access_key_id.as_ref(),
    );
    next.s3.secret_access_key = merge_secret(
        current.s3.secret_access_key.as_ref(),
        next.s3.secret_access_key.as_ref(),
    );
    next.s3.session_token = merge_secret(
        current.s3.session_token.as_ref(),
        next.s3.session_token.as_ref(),
    );
    next.gitee_snippet.access_token = merge_secret(
        current.gitee_snippet.access_token.as_ref(),
        next.gitee_snippet.access_token.as_ref(),
    );
    merge_oauth_drive_settings(&current.google_drive, &mut next.google_drive);
    merge_oauth_drive_settings(&current.onedrive, &mut next.onedrive);
    merge_aliyun_drive_settings(&current.aliyun_drive, &mut next.aliyun_drive);
    next.github_gist.access_token = merge_secret(
        current.github_gist.access_token.as_ref(),
        next.github_gist.access_token.as_ref(),
    );
    next
}

#[derive(Clone)]
pub struct LocalCloudSyncOptions {
    pub config_dir: PathBuf,
    pub portable_key_path: Option<PathBuf>,
    pub remote_dir: PathBuf,
    pub remote_root: String,
    pub device_id: String,
    pub device_name: Option<String>,
    pub app_version: String,
    pub master_password: SecretString,
    pub enabled: bool,
}

impl std::fmt::Debug for LocalCloudSyncOptions {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LocalCloudSyncOptions")
            .field("config_dir", &self.config_dir)
            .field("portable_key_path", &self.portable_key_path)
            .field("remote_dir", &self.remote_dir)
            .field("remote_root", &self.remote_root)
            .field("device_id", &self.device_id)
            .field("app_version", &self.app_version)
            .field("master_password", &"<redacted>")
            .field("enabled", &self.enabled)
            .finish()
    }
}

fn default_provider() -> String {
    "webdav".to_string()
}

fn default_remote_root() -> String {
    "nyaterm".to_string()
}

fn default_gitee_api_endpoint() -> String {
    "https://gitee.com/api/v5".to_string()
}

fn default_aliyun_drive_type() -> String {
    "resource".to_string()
}

fn default_device_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "This Device".to_string())
}

fn default_sync_debounce_seconds() -> u64 {
    60
}

fn default_true() -> bool {
    true
}

fn mask_oauth_drive_settings(settings: &mut OAuthDriveSyncSettings) {
    settings.access_token = mask_secret(settings.access_token.take());
    settings.refresh_token = mask_secret(settings.refresh_token.take());
    settings.client_secret = mask_secret(settings.client_secret.take());
}

fn mask_aliyun_drive_settings(settings: &mut AliyunDriveSyncSettings) {
    settings.access_token = mask_secret(settings.access_token.take());
    settings.refresh_token = mask_secret(settings.refresh_token.take());
    settings.client_secret = mask_secret(settings.client_secret.take());
}

fn merge_oauth_drive_settings(current: &OAuthDriveSyncSettings, next: &mut OAuthDriveSyncSettings) {
    next.access_token = merge_secret(current.access_token.as_ref(), next.access_token.as_ref());
    next.refresh_token = merge_secret(current.refresh_token.as_ref(), next.refresh_token.as_ref());
    next.client_secret = merge_secret(current.client_secret.as_ref(), next.client_secret.as_ref());
}

fn merge_aliyun_drive_settings(
    current: &AliyunDriveSyncSettings,
    next: &mut AliyunDriveSyncSettings,
) {
    next.access_token = merge_secret(current.access_token.as_ref(), next.access_token.as_ref());
    next.refresh_token = merge_secret(current.refresh_token.as_ref(), next.refresh_token.as_ref());
    next.client_secret = merge_secret(current.client_secret.as_ref(), next.client_secret.as_ref());
}

fn mask_secret(value: Option<SecretString>) -> Option<SecretString> {
    value.and_then(|secret| {
        if secret.is_empty() {
            None
        } else {
            Some(MASKED_SECRET_VALUE.into())
        }
    })
}

fn merge_secret(
    current: Option<&SecretString>,
    incoming: Option<&SecretString>,
) -> Option<SecretString> {
    match incoming.map(SecretString::expose_secret) {
        Some(MASKED_SECRET_VALUE) | None => current.cloned(),
        Some("") => None,
        Some(value) => Some(value.into()),
    }
}
