//! Authoritative state for cloud-sync configuration, history and background jobs.

use std::collections::HashSet;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};

use rust_i18n::t;

use nyaterm_core::{CloudSyncError, CloudSyncHistoryEntry, CloudSyncSettings, CloudSyncState};

use crate::models::{
    CloudSyncConflictState, CloudSyncInputField, CloudSyncSecretDraft, GithubGistAuthJobEvent,
    GithubGistAuthState,
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(in crate::features) enum CloudSyncLiveState {
    #[default]
    Idle,
    Running,
    Success,
    Failed,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(in crate::features) enum SyncHistoryFilter {
    #[default]
    All,
    Success,
    Failed,
    Conflict,
}

impl SyncHistoryFilter {
    pub(in crate::features) fn matches(self, entry: &CloudSyncHistoryEntry) -> bool {
        match self {
            Self::All => true,
            Self::Success => entry.status == "success",
            Self::Failed => entry.status == "failed",
            Self::Conflict => entry.status == "conflict",
        }
    }
}

const HISTORY_PAGE_SIZE: usize = 20;

pub(in crate::features) struct CloudSyncFeatureState {
    pub(crate) operations: super::operation::CloudSyncOperationRegistry,
    settings: CloudSyncSettings,
    state: CloudSyncState,
    history: Vec<CloudSyncHistoryEntry>,
    history_expanded: HashSet<String>,
    history_filter: SyncHistoryFilter,
    history_visible_limit: usize,
    conflict: Option<CloudSyncConflictState>,
    pending_resolution: Option<nyaterm_core::cloud_sync::conflict::CloudResolutionRequest>,
    secret_draft: CloudSyncSecretDraft,
    status: String,
    /// Prevent overlapping network jobs from applying cloud state out of order.
    live_state: CloudSyncLiveState,
    focused_field: CloudSyncInputField,
    github: GithubGistAuthFeatureState,
    v4_preview: Option<super::cloud_sync_v4::Preview>,
}

struct GithubGistAuthFeatureState {
    auth: GithubGistAuthState,
    tx: UnboundedSender<GithubGistAuthJobEvent>,
    /// Taken once by `NyaTermApp::start_github_gist_auth_event_drain`, which
    /// owns delivery from then on. `None` afterwards, so a second start is a
    /// no-op.
    rx: Option<UnboundedReceiver<GithubGistAuthJobEvent>>,
    job_id: u64,
    cancel: Option<Arc<AtomicBool>>,
}

pub(in crate::features) struct GithubGistAuthJobStart {
    job_id: u64,
    existing_gist_id: Option<String>,
    cancel: Arc<AtomicBool>,
    tx: UnboundedSender<GithubGistAuthJobEvent>,
}

impl GithubGistAuthJobStart {
    pub(in crate::features) fn job_id(&self) -> u64 {
        self.job_id
    }

    pub(in crate::features) fn existing_gist_id(&self) -> Option<String> {
        self.existing_gist_id.clone()
    }

    pub(in crate::features) fn cancel(&self) -> Arc<AtomicBool> {
        self.cancel.clone()
    }

    pub(in crate::features) fn sender(&self) -> UnboundedSender<GithubGistAuthJobEvent> {
        self.tx.clone()
    }
}

impl CloudSyncFeatureState {
    pub(in crate::features) fn new(
        settings: CloudSyncSettings,
        state: CloudSyncState,
        history: Vec<CloudSyncHistoryEntry>,
    ) -> Self {
        let (tx, rx) = unbounded();
        Self {
            operations: Default::default(),
            settings,
            state,
            history,
            history_expanded: HashSet::new(),
            history_filter: SyncHistoryFilter::All,
            history_visible_limit: HISTORY_PAGE_SIZE,
            conflict: None,
            pending_resolution: None,
            secret_draft: CloudSyncSecretDraft::default(),
            status: String::new(),
            live_state: CloudSyncLiveState::Idle,
            focused_field: CloudSyncInputField::RemoteRoot,
            v4_preview: None,
            github: GithubGistAuthFeatureState {
                auth: GithubGistAuthState::default(),
                tx,
                rx: Some(rx),
                job_id: 0,
                cancel: None,
            },
        }
    }

    pub(super) fn apply_input(&mut self, field: CloudSyncInputField, text: String) -> bool {
        // A Gist id being fetched is not the user's to edit yet.
        if self.github.auth.pending && field == CloudSyncInputField::GithubGistId {
            return false;
        }
        self.focused_field = field;
        *self.input_value_mut() = text;
        self.status = t!("settings.syncSettingsEdited").to_string();
        true
    }

    pub(in crate::features) fn settings(&self) -> &CloudSyncSettings {
        &self.settings
    }

    pub(in crate::features) fn state(&self) -> &CloudSyncState {
        &self.state
    }

    pub(in crate::features) fn history(&self) -> &[CloudSyncHistoryEntry] {
        &self.history
    }

    pub(in crate::features) fn history_expanded(&self) -> &HashSet<String> {
        &self.history_expanded
    }

    pub(in crate::features) fn history_filter(&self) -> SyncHistoryFilter {
        self.history_filter
    }

    pub(in crate::features) fn filter_history(&mut self, filter: SyncHistoryFilter) {
        self.history_filter = filter;
        self.history_visible_limit = HISTORY_PAGE_SIZE;
    }

    pub(in crate::features) fn navigate_history_filter(&mut self, key: &str) -> bool {
        let filters = [
            SyncHistoryFilter::All,
            SyncHistoryFilter::Success,
            SyncHistoryFilter::Failed,
            SyncHistoryFilter::Conflict,
        ];
        let index = filters
            .iter()
            .position(|filter| *filter == self.history_filter)
            .unwrap_or(0);
        let index = match key {
            "left" => (index + filters.len() - 1) % filters.len(),
            "right" => (index + 1) % filters.len(),
            "home" => 0,
            "end" => filters.len() - 1,
            _ => return false,
        };
        self.filter_history(filters[index]);
        true
    }

    pub(in crate::features) fn history_visible_limit(&self) -> usize {
        self.history_visible_limit
    }

    pub(in crate::features) fn show_more_history(&mut self) {
        self.history_visible_limit = self.history_visible_limit.saturating_add(HISTORY_PAGE_SIZE);
    }

    /// Historical success is evidence of an earlier operation, never of current
    /// equality. Only the live conflict owner authorizes resolution actions.
    pub(in crate::features) fn activity_status(&self) -> &'static str {
        if self.job_running() {
            "running"
        } else if self.conflict.is_some() {
            "conflict"
        } else if !self.settings.enabled {
            "disabled"
        } else {
            "idle"
        }
    }

    pub(in crate::features) fn conflict(&self) -> Option<&CloudSyncConflictState> {
        self.conflict.as_ref()
    }

    pub(in crate::features) fn secret_draft(&self) -> &CloudSyncSecretDraft {
        &self.secret_draft
    }

    pub(in crate::features) fn status(&self) -> &str {
        &self.status
    }

    pub(in crate::features) fn job_running(&self) -> bool {
        self.live_state == CloudSyncLiveState::Running
    }

    pub(in crate::features) fn live_state(&self) -> CloudSyncLiveState {
        self.live_state
    }

    pub(in crate::features) fn github_auth(&self) -> &GithubGistAuthState {
        &self.github.auth
    }

    /// Arm the device flow through the real `begin_github_auth` path without the
    /// worker thread `NyaTermApp::start_github_gist_auth` spawns alongside it.
    #[cfg(test)]
    pub(in crate::features) fn begin_github_auth_for_test(
        &mut self,
    ) -> Option<GithubGistAuthJobStart> {
        self.begin_github_auth("waiting for github".to_string())
    }

    pub(in crate::features) fn settings_draft_snapshot(
        &self,
    ) -> (CloudSyncSettings, CloudSyncSecretDraft) {
        (self.settings.clone(), self.secret_draft.clone())
    }

    pub(in crate::features) fn settings_draft_matches(
        &self,
        settings: &CloudSyncSettings,
        secret_draft: &CloudSyncSecretDraft,
    ) -> bool {
        settings == &self.settings && secret_draft == &self.secret_draft
    }

    pub(in crate::features) fn rebase_credential_rotation(
        &mut self,
        base: &mut CloudSyncSettings,
        current: &CloudSyncSettings,
    ) {
        let mut effective = self.pending_settings();
        nyaterm_core::cloud_sync::credentials::rebase_rotated_tokens(base, &mut effective, current);
        let mut previous = self.settings.clone();
        nyaterm_core::cloud_sync::credentials::rebase_rotated_tokens(
            &mut previous,
            &mut self.settings,
            current,
        );
    }

    pub(in crate::features) fn replace_settings(
        &mut self,
        settings: CloudSyncSettings,
        secret_draft: CloudSyncSecretDraft,
    ) {
        self.settings = settings;
        self.secret_draft = secret_draft;
    }

    pub(in crate::features) fn replace_loaded(
        &mut self,
        settings: CloudSyncSettings,
        state: CloudSyncState,
    ) {
        self.settings = settings;
        self.state = state;
        self.secret_draft = CloudSyncSecretDraft::default();
    }

    pub(in crate::features) fn pending_settings(&self) -> CloudSyncSettings {
        let mut next = self.settings.clone();
        let draft = &self.secret_draft;
        if !draft.webdav_password.is_empty() {
            next.webdav.password = Some(draft.webdav_password.clone());
        }
        if !draft.s3_access_key_id.is_empty() {
            next.s3.access_key_id = Some(draft.s3_access_key_id.clone());
        }
        if !draft.s3_secret_access_key.is_empty() {
            next.s3.secret_access_key = Some(draft.s3_secret_access_key.clone());
        }
        if !draft.s3_session_token.is_empty() {
            next.s3.session_token = Some(draft.s3_session_token.clone());
        }
        if !draft.google_drive_access_token.is_empty() {
            next.google_drive.access_token = Some(draft.google_drive_access_token.clone());
        }
        if !draft.google_drive_refresh_token.is_empty() {
            next.google_drive.refresh_token = Some(draft.google_drive_refresh_token.clone());
        }
        if !draft.google_drive_client_secret.is_empty() {
            next.google_drive.client_secret = Some(draft.google_drive_client_secret.clone());
        }
        if !draft.onedrive_access_token.is_empty() {
            next.onedrive.access_token = Some(draft.onedrive_access_token.clone());
        }
        if !draft.onedrive_refresh_token.is_empty() {
            next.onedrive.refresh_token = Some(draft.onedrive_refresh_token.clone());
        }
        if !draft.onedrive_client_secret.is_empty() {
            next.onedrive.client_secret = Some(draft.onedrive_client_secret.clone());
        }
        if !draft.aliyun_drive_access_token.is_empty() {
            next.aliyun_drive.access_token = Some(draft.aliyun_drive_access_token.clone());
        }
        if !draft.aliyun_drive_refresh_token.is_empty() {
            next.aliyun_drive.refresh_token = Some(draft.aliyun_drive_refresh_token.clone());
        }
        if !draft.aliyun_drive_client_secret.is_empty() {
            next.aliyun_drive.client_secret = Some(draft.aliyun_drive_client_secret.clone());
        }
        if !draft.gitee_token.is_empty() {
            next.gitee_snippet.access_token = Some(draft.gitee_token.clone());
        }
        if !draft.github_token.is_empty() {
            next.github_gist.access_token = Some(draft.github_token.clone());
        }
        next
    }

    pub(in crate::features) fn set_status(&mut self, status: impl Into<String>) {
        self.status = status.into();
    }

    pub(in crate::features) fn v4_summary(&self) -> Option<super::cloud_sync_v4::PreviewSummary> {
        self.v4_preview
            .as_ref()
            .map(super::cloud_sync_v4::Preview::summary)
    }

    pub(super) fn set_v4_preview(&mut self, preview: super::cloud_sync_v4::Preview) {
        self.v4_preview = Some(preview);
    }
    pub(super) fn take_v4_preview(&mut self) -> Option<super::cloud_sync_v4::Preview> {
        self.v4_preview.take()
    }

    pub(in crate::features) fn select_v4(&mut self) {
        self.settings.protocol_version = 4;
        self.v4_preview = None;
    }
    pub(in crate::features) fn toggle_v4_preferences(&mut self) {
        self.settings.v4_scope.preferences = !self.settings.v4_scope.preferences;
        self.v4_preview = None;
    }
    pub(in crate::features) fn toggle_v4_keybindings(&mut self) {
        self.settings.v4_scope.keybindings = !self.settings.v4_scope.keybindings;
        self.v4_preview = None;
    }
    pub(in crate::features) fn toggle_v4_ai(&mut self) {
        self.settings.v4_scope.ai = !self.settings.v4_scope.ai;
        self.v4_preview = None;
    }
    pub(in crate::features) fn toggle_v4_secrets(&mut self) {
        self.settings.v4_scope.secrets = !self.settings.v4_scope.secrets;
        self.v4_preview = None;
    }
    pub(in crate::features) fn toggle_v4_trust(&mut self) {
        self.settings.v4_scope.trusted_hosts = !self.settings.v4_scope.trusted_hosts;
        self.v4_preview = None;
    }

    pub(in crate::features) fn select_provider(&mut self, provider: &str) {
        self.settings.provider = provider.to_string();
        self.status = t!("settings.syncProviderSetTo", provider = provider).to_string();
    }

    pub(in crate::features) fn toggle_enabled(&mut self) {
        self.settings.enabled = !self.settings.enabled;
        self.status = if self.settings.enabled {
            t!("settings.syncEnabledSaveToPersist").to_string()
        } else {
            t!("settings.syncDisabledSaveToPersist").to_string()
        };
    }

    pub(in crate::features) fn toggle_s3_virtual_host_style(&mut self) {
        self.settings.s3.virtual_host_style = !self.settings.s3.virtual_host_style;
        self.status = if self.settings.s3.virtual_host_style {
            t!("settings.s3VirtualHostStyleEnabled").to_string()
        } else {
            t!("settings.s3PathStyleUrlsEnabled").to_string()
        };
    }

    pub(in crate::features) fn toggle_auto_check(&mut self) {
        self.settings.auto_check_on_startup = !self.settings.auto_check_on_startup;
        self.status = t!("settings.syncAutoCheckEdited").to_string();
    }

    pub(in crate::features) fn toggle_auto_push(&mut self) {
        self.settings.auto_push_on_change = !self.settings.auto_push_on_change;
        self.status = t!("settings.syncAutoPushEdited").to_string();
    }

    pub(in crate::features) fn toggle_auto_pull_remote_changes(&mut self) {
        self.settings.auto_pull_remote_changes = !self.settings.auto_pull_remote_changes;
        self.status = t!("settings.syncAutoPullEdited").to_string();
    }

    pub(in crate::features) fn set_debounce(&mut self, value: u64) {
        self.settings.sync_debounce_seconds = value.clamp(1, 3_600);
        self.status = t!("settings.syncDebounceEdited").to_string();
    }

    pub(super) fn begin_job(&mut self) -> bool {
        if self.job_running() {
            return false;
        }
        self.live_state = CloudSyncLiveState::Running;
        true
    }

    pub(super) fn cancel_job_for_shutdown(&mut self) {
        self.live_state = CloudSyncLiveState::Idle;
        self.status.clear();
    }

    pub(super) fn complete_job(&mut self, state: CloudSyncState, status: String) {
        self.live_state = CloudSyncLiveState::Success;
        self.conflict = None;
        self.state = state;
        self.status = status;
    }

    pub(super) fn fail_job_with_status(&mut self, status: String) {
        self.live_state = CloudSyncLiveState::Failed;
        self.status = status;
    }

    pub(super) fn fail_job(
        &mut self,
        error: &CloudSyncError,
        status: String,
        provider: String,
        provider_action: bool,
    ) {
        self.live_state = CloudSyncLiveState::Failed;
        self.status = status;
        self.capture_conflict(error, provider, provider_action);
    }

    pub(super) fn replace_history(&mut self, history: Vec<CloudSyncHistoryEntry>) {
        self.history_expanded
            .retain(|id| history.iter().any(|entry| &entry.id == id));
        self.history = history;
    }

    pub(super) fn toggle_history_details(&mut self, entry_id: &str) {
        if self.history_expanded.contains(entry_id) {
            self.history_expanded.remove(entry_id);
        } else {
            self.history_expanded.insert(entry_id.to_string());
        }
    }

    pub(super) fn capture_conflict(
        &mut self,
        error: &CloudSyncError,
        _provider: String,
        provider_action: bool,
    ) {
        if let CloudSyncError::Conflict(preview) = error {
            self.conflict = Some(CloudSyncConflictState {
                selected_revision: {
                    let readable: Vec<_> = preview.branches.iter().filter(|b| b.readable).collect();
                    (readable.len() == 1).then(|| readable[0].revision.clone())
                },
                preview: preview.as_ref().clone(),
                provider_action,
            });
        }
    }

    pub(in crate::features) fn select_conflict_branch(&mut self, revision: &str) {
        if let Some(conflict) = &mut self.conflict
            && conflict
                .preview
                .branches
                .iter()
                .any(|branch| branch.readable && branch.revision == revision)
        {
            conflict.selected_revision = Some(revision.to_string());
        }
    }

    pub(super) fn prepare_resolution(&mut self, use_cloud: bool) -> bool {
        let Some(conflict) = &self.conflict else {
            return false;
        };
        let Some(local_content_hash) = conflict.preview.local_content_hash.clone() else {
            return false;
        };
        if (!use_cloud && conflict.preview.overwrite_blocked)
            || (use_cloud && conflict.selected_revision.is_none())
        {
            return false;
        }
        self.pending_resolution =
            Some(nyaterm_core::cloud_sync::conflict::CloudResolutionRequest {
                observed_revisions: conflict.preview.observed_revisions.clone(),
                selected_revision: if use_cloud {
                    conflict.selected_revision.clone()
                } else {
                    None
                },
                local_content_hash,
            });
        true
    }

    pub(super) fn take_resolution(
        &mut self,
    ) -> Option<nyaterm_core::cloud_sync::conflict::CloudResolutionRequest> {
        self.pending_resolution.take()
    }

    pub(super) fn begin_github_auth(
        &mut self,
        waiting_message: String,
    ) -> Option<GithubGistAuthJobStart> {
        if self.github.auth.pending {
            return None;
        }
        if let Some(cancel) = self.github.cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.github.job_id = self.github.job_id.wrapping_add(1);
        let existing_gist_id = self.settings.github_gist.gist_id.trim().to_string();
        let existing_gist_id = (!existing_gist_id.is_empty()).then_some(existing_gist_id);
        let cancel = Arc::new(AtomicBool::new(false));
        self.github.auth = GithubGistAuthState {
            pending: true,
            message: Some(waiting_message.clone()),
            ..Default::default()
        };
        self.github.cancel = Some(cancel.clone());
        self.status = waiting_message;
        Some(GithubGistAuthJobStart {
            job_id: self.github.job_id,
            existing_gist_id,
            cancel,
            tx: self.github.tx.clone(),
        })
    }

    pub(super) fn cancel_github_auth(&mut self) {
        if let Some(cancel) = self.github.cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.github.job_id = self.github.job_id.wrapping_add(1);
        self.github.auth = GithubGistAuthState::default();
    }

    pub(super) fn take_github_auth_event_receiver(
        &mut self,
    ) -> Option<UnboundedReceiver<GithubGistAuthJobEvent>> {
        self.github.rx.take()
    }

    /// Accept one device-flow event if it belongs to the current job.
    ///
    /// `cancel_github_auth` and `begin_github_auth` both bump `job_id`, so an
    /// event from a superseded flow is dropped rather than applied.
    pub(super) fn accept_github_auth_event(
        &self,
        job: GithubGistAuthJobEvent,
    ) -> Option<crate::models::GithubGistAuthEvent> {
        (job.job_id == self.github.job_id).then_some(job.event)
    }

    pub(super) fn apply_github_auth_started(
        &mut self,
        user_code: String,
        verification_uri: String,
        message: String,
    ) {
        self.github.auth.pending = true;
        self.github.auth.user_code = Some(user_code);
        self.github.auth.verification_uri = Some(verification_uri);
        self.github.auth.message = Some(message);
    }

    pub(super) fn apply_github_auth_polling(&mut self, message: String) {
        self.github.auth.message = Some(message);
    }

    pub(super) fn apply_github_auth_succeeded(
        &mut self,
        access_token: nyaterm_core::SecretString,
        gist_id: String,
        login: String,
        message: String,
    ) {
        self.github.cancel = None;
        self.secret_draft.github_token = access_token;
        self.settings.github_gist.gist_id = gist_id;
        self.github.auth = GithubGistAuthState {
            pending: false,
            login: Some(login),
            message: Some(message.clone()),
            ..Default::default()
        };
        self.status = message;
    }

    pub(super) fn apply_github_auth_failed(&mut self, message: String) {
        self.github.cancel = None;
        self.github.auth.pending = false;
        self.github.auth.user_code = None;
        self.github.auth.verification_uri = None;
        self.github.auth.message = Some(message.clone());
        self.status = message;
    }

    pub(super) fn apply_github_auth_cancelled(&mut self) {
        self.github.cancel = None;
        self.github.auth = GithubGistAuthState::default();
    }

    /// The value a given input holds, read-only.
    ///
    /// The mutable twin below drives editing; this one seeds an input when the tab
    /// that draws it is activated, so the two cannot disagree about where a field
    /// lives.
    pub(in crate::features) fn input_value(&self, field: CloudSyncInputField) -> String {
        match field {
            CloudSyncInputField::RemoteRoot => self.settings.remote_root.clone(),
            CloudSyncInputField::DeviceName => self.settings.device_name.clone(),
            CloudSyncInputField::WebdavEndpoint => self.settings.webdav.endpoint.clone(),
            CloudSyncInputField::WebdavRoot => self.settings.webdav.root.clone(),
            CloudSyncInputField::WebdavUsername => self.settings.webdav.username.clone(),
            CloudSyncInputField::WebdavPassword => {
                self.secret_draft.webdav_password.expose_secret().to_owned()
            }
            CloudSyncInputField::S3Endpoint => self.settings.s3.endpoint.clone(),
            CloudSyncInputField::S3Bucket => self.settings.s3.bucket.clone(),
            CloudSyncInputField::S3Region => self.settings.s3.region.clone(),
            CloudSyncInputField::S3Root => self.settings.s3.root.clone(),
            CloudSyncInputField::S3AccessKeyId => self
                .secret_draft
                .s3_access_key_id
                .expose_secret()
                .to_owned(),
            CloudSyncInputField::S3SecretAccessKey => self
                .secret_draft
                .s3_secret_access_key
                .expose_secret()
                .to_owned(),
            CloudSyncInputField::S3SessionToken => self
                .secret_draft
                .s3_session_token
                .expose_secret()
                .to_owned(),
            CloudSyncInputField::GoogleDriveRoot => self.settings.google_drive.root.clone(),
            CloudSyncInputField::GoogleDriveAccessToken => self
                .secret_draft
                .google_drive_access_token
                .expose_secret()
                .to_owned(),
            CloudSyncInputField::GoogleDriveRefreshToken => self
                .secret_draft
                .google_drive_refresh_token
                .expose_secret()
                .to_owned(),
            CloudSyncInputField::GoogleDriveClientId => self
                .settings
                .google_drive
                .client_id
                .clone()
                .unwrap_or_default(),
            CloudSyncInputField::GoogleDriveClientSecret => self
                .secret_draft
                .google_drive_client_secret
                .expose_secret()
                .to_owned(),
            CloudSyncInputField::OneDriveRoot => self.settings.onedrive.root.clone(),
            CloudSyncInputField::OneDriveAccessToken => self
                .secret_draft
                .onedrive_access_token
                .expose_secret()
                .to_owned(),
            CloudSyncInputField::OneDriveRefreshToken => self
                .secret_draft
                .onedrive_refresh_token
                .expose_secret()
                .to_owned(),
            CloudSyncInputField::OneDriveClientId => {
                self.settings.onedrive.client_id.clone().unwrap_or_default()
            }
            CloudSyncInputField::OneDriveClientSecret => self
                .secret_draft
                .onedrive_client_secret
                .expose_secret()
                .to_owned(),
            CloudSyncInputField::AliyunDriveRoot => self.settings.aliyun_drive.root.clone(),
            CloudSyncInputField::AliyunDriveType => self.settings.aliyun_drive.drive_type.clone(),
            CloudSyncInputField::AliyunDriveAccessToken => self
                .secret_draft
                .aliyun_drive_access_token
                .expose_secret()
                .to_owned(),
            CloudSyncInputField::AliyunDriveRefreshToken => self
                .secret_draft
                .aliyun_drive_refresh_token
                .expose_secret()
                .to_owned(),
            CloudSyncInputField::AliyunDriveClientId => self
                .settings
                .aliyun_drive
                .client_id
                .clone()
                .unwrap_or_default(),
            CloudSyncInputField::AliyunDriveClientSecret => self
                .secret_draft
                .aliyun_drive_client_secret
                .expose_secret()
                .to_owned(),
            CloudSyncInputField::GiteeEndpoint => self.settings.gitee_snippet.api_endpoint.clone(),
            CloudSyncInputField::GiteeGistId => self.settings.gitee_snippet.gist_id.clone(),
            CloudSyncInputField::GiteeToken => {
                self.secret_draft.gitee_token.expose_secret().to_owned()
            }
            CloudSyncInputField::GithubGistId => self.settings.github_gist.gist_id.clone(),
        }
    }

    fn input_value_mut(&mut self) -> &mut String {
        match self.focused_field {
            CloudSyncInputField::RemoteRoot => &mut self.settings.remote_root,
            CloudSyncInputField::DeviceName => &mut self.settings.device_name,
            CloudSyncInputField::WebdavEndpoint => &mut self.settings.webdav.endpoint,
            CloudSyncInputField::WebdavRoot => &mut self.settings.webdav.root,
            CloudSyncInputField::WebdavUsername => &mut self.settings.webdav.username,
            CloudSyncInputField::WebdavPassword => {
                self.secret_draft.webdav_password.expose_secret_mut()
            }
            CloudSyncInputField::S3Endpoint => &mut self.settings.s3.endpoint,
            CloudSyncInputField::S3Bucket => &mut self.settings.s3.bucket,
            CloudSyncInputField::S3Region => &mut self.settings.s3.region,
            CloudSyncInputField::S3Root => &mut self.settings.s3.root,
            CloudSyncInputField::S3AccessKeyId => {
                self.secret_draft.s3_access_key_id.expose_secret_mut()
            }
            CloudSyncInputField::S3SecretAccessKey => {
                self.secret_draft.s3_secret_access_key.expose_secret_mut()
            }
            CloudSyncInputField::S3SessionToken => {
                self.secret_draft.s3_session_token.expose_secret_mut()
            }
            CloudSyncInputField::GoogleDriveRoot => &mut self.settings.google_drive.root,
            CloudSyncInputField::GoogleDriveAccessToken => self
                .secret_draft
                .google_drive_access_token
                .expose_secret_mut(),
            CloudSyncInputField::GoogleDriveRefreshToken => self
                .secret_draft
                .google_drive_refresh_token
                .expose_secret_mut(),
            CloudSyncInputField::GoogleDriveClientId => self
                .settings
                .google_drive
                .client_id
                .get_or_insert_with(String::new),
            CloudSyncInputField::GoogleDriveClientSecret => self
                .secret_draft
                .google_drive_client_secret
                .expose_secret_mut(),
            CloudSyncInputField::OneDriveRoot => &mut self.settings.onedrive.root,
            CloudSyncInputField::OneDriveAccessToken => {
                self.secret_draft.onedrive_access_token.expose_secret_mut()
            }
            CloudSyncInputField::OneDriveRefreshToken => {
                self.secret_draft.onedrive_refresh_token.expose_secret_mut()
            }
            CloudSyncInputField::OneDriveClientId => self
                .settings
                .onedrive
                .client_id
                .get_or_insert_with(String::new),
            CloudSyncInputField::OneDriveClientSecret => {
                self.secret_draft.onedrive_client_secret.expose_secret_mut()
            }
            CloudSyncInputField::AliyunDriveRoot => &mut self.settings.aliyun_drive.root,
            CloudSyncInputField::AliyunDriveType => &mut self.settings.aliyun_drive.drive_type,
            CloudSyncInputField::AliyunDriveAccessToken => self
                .secret_draft
                .aliyun_drive_access_token
                .expose_secret_mut(),
            CloudSyncInputField::AliyunDriveRefreshToken => self
                .secret_draft
                .aliyun_drive_refresh_token
                .expose_secret_mut(),
            CloudSyncInputField::AliyunDriveClientId => self
                .settings
                .aliyun_drive
                .client_id
                .get_or_insert_with(String::new),
            CloudSyncInputField::AliyunDriveClientSecret => self
                .secret_draft
                .aliyun_drive_client_secret
                .expose_secret_mut(),
            CloudSyncInputField::GiteeEndpoint => &mut self.settings.gitee_snippet.api_endpoint,
            CloudSyncInputField::GiteeGistId => &mut self.settings.gitee_snippet.gist_id,
            CloudSyncInputField::GiteeToken => self.secret_draft.gitee_token.expose_secret_mut(),
            CloudSyncInputField::GithubGistId => &mut self.settings.github_gist.gist_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use nyaterm_core::{
        CloudConflictKind, CloudConflictPreview, CloudSyncError, CloudSyncHistoryEntry,
        CloudSyncSettings, CloudSyncState,
    };

    use crate::models::{
        CloudSyncInputField, CloudSyncSecretDraft, GithubGistAuthEvent, GithubGistAuthJobEvent,
    };

    use super::{CloudSyncFeatureState, CloudSyncLiveState, SyncHistoryFilter};

    #[test]
    fn historical_success_and_cancelled_prompts_do_not_claim_current_sync_health() {
        let mut cloud_sync = CloudSyncFeatureState::new(
            CloudSyncSettings {
                enabled: true,
                ..Default::default()
            },
            CloudSyncState::default(),
            vec![CloudSyncHistoryEntry::sync(
                "success",
                "manual_provider_push",
                None,
                None,
                "uploaded",
            )],
        );
        assert_eq!(cloud_sync.activity_status(), "idle");
        cloud_sync.complete_job(CloudSyncState::default(), "uploaded".to_string());
        cloud_sync.set_status("cancelled");
        assert_eq!(cloud_sync.activity_status(), "idle");
        assert_eq!(cloud_sync.status(), "cancelled");
        assert!(cloud_sync.begin_job());
        assert_eq!(cloud_sync.activity_status(), "running");
    }

    #[test]
    fn history_filter_resets_paging_and_refresh_prunes_only_missing_expansions() {
        let mut cloud_sync = CloudSyncFeatureState::new(
            CloudSyncSettings::default(),
            CloudSyncState::default(),
            Vec::new(),
        );
        let conflict =
            CloudSyncHistoryEntry::sync("conflict", "manual_provider_push", None, None, "conflict");
        let success =
            CloudSyncHistoryEntry::sync("success", "manual_provider_push", None, None, "uploaded");
        cloud_sync.replace_history(vec![success.clone(), conflict.clone()]);
        cloud_sync.toggle_history_details(&conflict.id);
        cloud_sync.show_more_history();
        assert_eq!(cloud_sync.history_visible_limit(), 40);
        cloud_sync.filter_history(SyncHistoryFilter::Conflict);
        assert_eq!(cloud_sync.history_visible_limit(), 20);
        assert!(cloud_sync.history_filter().matches(&conflict));
        assert!(!cloud_sync.history_filter().matches(&success));
        assert!(cloud_sync.history_expanded().contains(&conflict.id));
        assert!(
            cloud_sync.conflict().is_none(),
            "historical conflicts cannot authorize resolution"
        );
        cloud_sync.replace_history(vec![success]);
        assert!(cloud_sync.history_expanded().is_empty());
    }

    #[test]
    fn cloud_sync_state_owns_loaded_data_and_github_job_channel() {
        let settings = CloudSyncSettings {
            provider: "webdav".to_string(),
            remote_root: "team".to_string(),
            ..CloudSyncSettings::default()
        };
        let state = CloudSyncState::default();
        let history = vec![CloudSyncHistoryEntry::sync(
            "success",
            "manual_push",
            Some("webdav".to_string()),
            None,
            "done".to_string(),
        )];

        let mut cloud_sync = CloudSyncFeatureState::new(settings, state, history);

        assert_eq!(cloud_sync.settings().provider, "webdav");
        assert_eq!(cloud_sync.settings().remote_root, "team");
        assert_eq!(cloud_sync.history().len(), 1);
        assert!(cloud_sync.status().is_empty());
        assert!(
            cloud_sync
                .take_github_auth_event_receiver()
                .expect("a fresh state still holds its receiver")
                .try_recv()
                .is_err(),
            "the device-flow channel starts empty"
        );
        assert!(!cloud_sync.job_running());
        assert_eq!(cloud_sync.live_state(), CloudSyncLiveState::Idle);
        assert!(cloud_sync.conflict().is_none());

        let mut settings = cloud_sync.settings().clone();
        settings.webdav.password = Some("stored".to_string().into());
        cloud_sync.replace_settings(settings, CloudSyncSecretDraft::default());
        assert!(cloud_sync.apply_input(CloudSyncInputField::WebdavPassword, "draft".to_string(),));
        assert_eq!(
            cloud_sync.settings().webdav.password.as_deref(),
            Some("stored")
        );
        assert_eq!(
            cloud_sync.secret_draft().webdav_password.expose_secret(),
            "draft"
        );
    }

    #[test]
    fn pending_settings_merge_only_non_empty_secret_drafts() {
        let mut settings = CloudSyncSettings::default();
        settings.webdav.password = Some("stored-webdav".to_string().into());
        settings.s3.session_token = Some("stored-session".to_string().into());
        let mut cloud_sync =
            CloudSyncFeatureState::new(settings, CloudSyncState::default(), Vec::new());
        cloud_sync.secret_draft.webdav_password = "edited-webdav".to_string().into();
        cloud_sync.secret_draft.github_token = "new-github-token".to_string().into();

        let pending = cloud_sync.pending_settings();

        assert_eq!(pending.webdav.password.as_deref(), Some("edited-webdav"));
        assert_eq!(pending.s3.session_token.as_deref(), Some("stored-session"));
        assert_eq!(
            pending.github_gist.access_token.as_deref(),
            Some("new-github-token")
        );
    }

    #[test]
    fn job_transitions_keep_running_state_status_and_conflict_consistent() {
        let mut cloud_sync = CloudSyncFeatureState::new(
            CloudSyncSettings::default(),
            CloudSyncState::default(),
            Vec::new(),
        );

        assert!(cloud_sync.begin_job());
        assert!(!cloud_sync.begin_job());
        assert_eq!(cloud_sync.activity_status(), "running");
        cloud_sync.fail_job(
            &CloudSyncError::Conflict(Box::new(CloudConflictPreview {
                branches: Vec::new(),
                observed_revisions: Vec::new(),
                local_content_hash: None,
                detected_at_ms: 1,
                provider: "webdav".to_string(),
                kind: CloudConflictKind::ContentConflict,
                local_payload_hash: "local".to_string(),
                remote_payload_hash: "remote".to_string(),
                remote_revision: "revision".to_string(),
                remote_created_at_ms: 2,
                remote_device_id: "remote-device".to_string(),
                recovery_revision: None,
                recovery_payload_hash: None,
                recovery_created_at_ms: None,
                overwrite_blocked: false,
                message: "remote changed".to_string(),
            })),
            "push failed".to_string(),
            "webdav".to_string(),
            true,
        );
        assert!(!cloud_sync.job_running());
        assert_eq!(cloud_sync.live_state(), CloudSyncLiveState::Failed);
        assert_eq!(cloud_sync.status(), "push failed");
        assert_eq!(cloud_sync.conflict().unwrap().preview.provider, "webdav");
        assert_eq!(cloud_sync.activity_status(), "conflict");
        // A later failed attempt or a historical success cannot clear a live conflict.
        cloud_sync.fail_job_with_status("cancelled attempt".to_string());
        cloud_sync.replace_history(vec![CloudSyncHistoryEntry::sync(
            "success",
            "manual_push",
            None,
            None,
            "old upload",
        )]);
        assert_eq!(cloud_sync.activity_status(), "conflict");

        assert!(cloud_sync.begin_job());
        assert_eq!(cloud_sync.activity_status(), "running");
        let completed_state = CloudSyncState {
            device_id: "device-2".to_string(),
            ..CloudSyncState::default()
        };
        cloud_sync.complete_job(completed_state, "push complete".to_string());
        assert!(!cloud_sync.job_running());
        assert_eq!(cloud_sync.live_state(), CloudSyncLiveState::Success);
        assert!(cloud_sync.conflict().is_none());
        assert_eq!(cloud_sync.state().device_id, "device-2");
        assert_eq!(cloud_sync.status(), "push complete");
    }

    #[test]
    fn remote_inconsistent_conflict_preserves_typed_recovery_candidate() {
        let mut cloud_sync = CloudSyncFeatureState::new(
            CloudSyncSettings::default(),
            CloudSyncState::default(),
            Vec::new(),
        );
        let preview = CloudConflictPreview {
            branches: Vec::new(),
            observed_revisions: Vec::new(),
            local_content_hash: None,
            detected_at_ms: 1,
            provider: "s3".to_string(),
            kind: CloudConflictKind::RemoteInconsistent,
            local_payload_hash: "local".to_string(),
            remote_payload_hash: "missing-hash".to_string(),
            remote_revision: "missing-revision".to_string(),
            remote_created_at_ms: 2,
            remote_device_id: "remote-device".to_string(),
            recovery_revision: Some("recoverable-revision".to_string()),
            recovery_payload_hash: Some("recoverable-hash".to_string()),
            recovery_created_at_ms: Some(3),
            overwrite_blocked: false,
            message: "remote incomplete".to_string(),
        };

        assert!(cloud_sync.begin_job());
        cloud_sync.fail_job(
            &CloudSyncError::Conflict(Box::new(preview.clone())),
            "recovery required".to_string(),
            "s3".to_string(),
            true,
        );

        let conflict = cloud_sync.conflict().expect("typed conflict");
        assert_eq!(conflict.preview, preview);
        assert!(conflict.provider_action);
        assert!(!cloud_sync.job_running());
        assert_eq!(cloud_sync.live_state(), CloudSyncLiveState::Failed);

        assert!(cloud_sync.begin_job());
        cloud_sync.complete_job(CloudSyncState::default(), "recovered".to_string());
        assert!(cloud_sync.conflict().is_none());
        assert!(!cloud_sync.job_running());
        assert_eq!(cloud_sync.live_state(), CloudSyncLiveState::Success);
        assert_eq!(cloud_sync.status(), "recovered");
    }

    #[test]
    fn github_auth_filters_stale_events_and_cancels_the_active_job() {
        let mut cloud_sync = CloudSyncFeatureState::new(
            CloudSyncSettings::default(),
            CloudSyncState::default(),
            Vec::new(),
        );
        let job = cloud_sync
            .begin_github_auth("waiting".to_string())
            .expect("auth job should start");
        let job_id = job.job_id();
        let cancel = job.cancel();
        // A superseded job's event must be dropped rather than applied.
        assert!(
            cloud_sync
                .accept_github_auth_event(GithubGistAuthJobEvent {
                    job_id: job_id.wrapping_sub(1),
                    event: GithubGistAuthEvent::Cancelled,
                })
                .is_none()
        );
        assert!(matches!(
            cloud_sync.accept_github_auth_event(GithubGistAuthJobEvent {
                job_id,
                event: GithubGistAuthEvent::Polling { slow_down: true },
            }),
            Some(GithubGistAuthEvent::Polling { slow_down: true })
        ));

        cloud_sync.cancel_github_auth();
        assert!(cancel.load(Ordering::Relaxed));
        assert!(!cloud_sync.github_auth().pending);
    }
    #[test]
    fn conflict_resolution_requires_an_explicit_readable_branch_and_pins_the_preview() {
        use nyaterm_core::cloud_sync::conflict::CloudConflictBranch;
        let mut state = CloudSyncFeatureState::new(
            CloudSyncSettings::default(),
            CloudSyncState::default(),
            Vec::new(),
        );
        let mut preview: CloudConflictPreview = serde_json::from_value(serde_json::json!({
            "detected_at_ms": 1, "provider": "test", "local_payload_hash": "local",
            "remote_payload_hash": "remote", "remote_revision": "a", "remote_created_at_ms": 1,
            "remote_device_id": "a", "message": "conflict", "local_content_hash": "local-content",
            "observed_revisions": ["a", "b", "c"]
        }))
        .unwrap();
        preview.branches = ["a", "b", "c"]
            .into_iter()
            .map(|id| CloudConflictBranch {
                revision: id.into(),
                device_id: id.into(),
                device_name: None,
                created_at_ms: 1,
                readable: id != "c",
                differences: Vec::new(),
            })
            .collect();
        state.capture_conflict(
            &CloudSyncError::Conflict(Box::new(preview.clone())),
            "test".into(),
            true,
        );
        assert!(state.conflict().unwrap().selected_revision.is_none());
        assert!(!state.prepare_resolution(true));
        state.select_conflict_branch("c");
        assert!(!state.prepare_resolution(true));
        state.select_conflict_branch("b");
        assert!(state.prepare_resolution(true));
        let request = state.take_resolution().unwrap();
        assert_eq!(request.selected_revision.as_deref(), Some("b"));
        assert_eq!(request.observed_revisions, preview.observed_revisions);
        assert_eq!(request.local_content_hash, "local-content");
        assert!(state.take_resolution().is_none());
        assert!(state.prepare_resolution(false));
        assert!(state.take_resolution().unwrap().selected_revision.is_none());
        preview.branches[1].readable = false;
        state.capture_conflict(
            &CloudSyncError::Conflict(Box::new(preview)),
            "test".into(),
            true,
        );
        assert_eq!(
            state.conflict().unwrap().selected_revision.as_deref(),
            Some("a")
        );
    }
    #[test]
    fn cloud_sync_rotated_tokens_keep_clean_drafts_clean_and_preserve_explicit_secret_edits() {
        let mut base = CloudSyncSettings::default();
        base.google_drive.access_token = Some("old-access".into());
        base.google_drive.refresh_token = Some("old-refresh".into());
        let original = base.clone();
        let mut state =
            CloudSyncFeatureState::new(base.clone(), CloudSyncState::default(), Vec::new());
        let mut current = base.clone();
        current.google_drive.access_token = Some("new-access".into());
        current.google_drive.refresh_token = Some("new-refresh".into());
        state.rebase_credential_rotation(&mut base, &current);
        assert!(state.settings_draft_matches(&base, &CloudSyncSecretDraft::default()));
        assert_eq!(state.settings().google_drive, current.google_drive);

        let mut state =
            CloudSyncFeatureState::new(original.clone(), CloudSyncState::default(), Vec::new());
        state.secret_draft.google_drive_refresh_token = "user-refresh".into();
        let mut base = original;
        state.rebase_credential_rotation(&mut base, &current);
        assert_eq!(
            state
                .pending_settings()
                .google_drive
                .refresh_token
                .as_deref(),
            Some("user-refresh")
        );
        assert_eq!(
            state.settings().google_drive.access_token.as_deref(),
            Some("new-access")
        );
        assert_ne!(
            base, current,
            "an explicit edit retains the save-time conflict baseline"
        );
    }
}
