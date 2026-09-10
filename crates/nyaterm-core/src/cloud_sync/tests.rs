use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, UNIX_EPOCH};

use super::{
    CLOUD_SYNC_HISTORY_DOMAIN, CLOUD_SYNC_HISTORY_EVENT, CLOUD_SYNC_HISTORY_LIMIT,
    CloudConflictKind, CloudRemoteCheckDecision, CloudSyncError, CloudSyncHistoryEntry,
    CloudSyncRemote, CloudSyncSettings, CloudSyncState, GiteeSnippetHttpBackend,
    GiteeSnippetSyncSettings, GithubGistHttpBackend, GithubGistSyncSettings, LocalCloudSyncOptions,
    MASKED_SECRET_VALUE, RemoteSyncPointer, S3HttpMethod, S3SyncSettings, SnippetBlobBackend,
    SnippetHttpClient, SnippetHttpMethod, SnippetHttpRequest, SnippetHttpResponse, SnippetRemote,
    append_cloud_sync_history, build_s3_signed_request, build_s3_signed_request_with_query,
    decide_cloud_remote_check, decode_snippet_blob, drive_remote_segments, encode_snippet_blob,
    gitee_snippet_patch_body, github_gist_patch_body, google_drive_query_literal,
    merge_masked_cloud_sync_settings, read_cloud_sync_history, remote_path, s3_payload_sha256,
    snippet_remote_filename, snippet_remote_path,
};
use crate::{
    AiExecutionProfile, CloudLocalStore, CloudSyncBackupInfo, CloudSyncResult, ConnectionType,
    PortableSnapshotKind, RawPortableSnapshot, SavedConnection, SessionsConfig,
};

#[derive(Default, Clone)]
struct TestLocalData {
    sessions: SessionsConfig,
    cloud_sync_state: CloudSyncState,
}

fn test_local_data() -> &'static Mutex<HashMap<PathBuf, TestLocalData>> {
    static DATA: OnceLock<Mutex<HashMap<PathBuf, TestLocalData>>> = OnceLock::new();
    DATA.get_or_init(|| Mutex::new(HashMap::new()))
}

struct ConnectionStore {
    config_dir: PathBuf,
}

#[test]
fn cloud_sync_debug_output_redacts_all_secret_values() {
    let secret = "nya-cloud-secret-never-log";
    let mut settings = CloudSyncSettings::default();
    settings.webdav.password = Some(secret.to_string().into());
    settings.s3.access_key_id = Some(secret.to_string().into());
    settings.s3.secret_access_key = Some(secret.to_string().into());
    settings.s3.session_token = Some(secret.to_string().into());
    settings.gitee_snippet.access_token = Some(secret.to_string().into());
    settings.google_drive.access_token = Some(secret.to_string().into());
    settings.google_drive.refresh_token = Some(secret.to_string().into());
    settings.google_drive.client_secret = Some(secret.to_string().into());
    settings.aliyun_drive.access_token = Some(secret.to_string().into());
    settings.github_gist.access_token = Some(secret.to_string().into());
    let settings_output = format!("{settings:?}");

    let options = LocalCloudSyncOptions {
        config_dir: PathBuf::from("config"),
        portable_key_path: None,
        remote_dir: PathBuf::from("remote"),
        remote_root: "nyaterm".to_string(),
        device_id: "device".to_string(),
        app_version: "test".to_string(),
        master_password: secret.to_string().into(),
        enabled: true,
    };
    let options_output = format!("{options:?}");

    assert!(!settings_output.contains(secret));
    assert!(!options_output.contains(secret));
    assert!(settings_output.contains("<redacted>"));
    assert!(options_output.contains("<redacted>"));
}

impl ConnectionStore {
    fn open(config_dir: impl AsRef<Path>) -> Result<Self, std::io::Error> {
        Self::open_with_portable_key_path(config_dir, None)
    }

    fn open_with_portable_key_path(
        config_dir: impl AsRef<Path>,
        _portable_key_path: Option<PathBuf>,
    ) -> Result<Self, std::io::Error> {
        let config_dir = config_dir.as_ref().to_path_buf();
        test_local_data()
            .lock()
            .expect("test local store lock")
            .entry(config_dir.clone())
            .or_default();
        Ok(Self { config_dir })
    }

    fn replace_sessions(&self, sessions: &SessionsConfig) -> Result<(), std::io::Error> {
        test_local_data()
            .lock()
            .expect("test local store lock")
            .entry(self.config_dir.clone())
            .or_default()
            .sessions = sessions.clone();
        Ok(())
    }

    fn load_sessions(&self) -> Result<SessionsConfig, std::io::Error> {
        Ok(test_local_data()
            .lock()
            .expect("test local store lock")
            .get(&self.config_dir)
            .cloned()
            .unwrap_or_default()
            .sessions)
    }

    fn save_cloud_sync_state(&self, state: &CloudSyncState) -> Result<(), std::io::Error> {
        test_local_data()
            .lock()
            .expect("test local store lock")
            .entry(self.config_dir.clone())
            .or_default()
            .cloud_sync_state = state.clone();
        Ok(())
    }

    fn load_cloud_sync_state(&self) -> Result<CloudSyncState, std::io::Error> {
        Ok(test_local_data()
            .lock()
            .expect("test local store lock")
            .get(&self.config_dir)
            .cloned()
            .unwrap_or_default()
            .cloud_sync_state)
    }

    fn build_raw_portable_snapshot(
        &self,
        kind: PortableSnapshotKind,
        device_id: String,
        app_version: String,
    ) -> Result<RawPortableSnapshot, std::io::Error> {
        let mut snapshot = match kind {
            PortableSnapshotKind::Sync => RawPortableSnapshot::sync(device_id, app_version),
            PortableSnapshotKind::Backup => RawPortableSnapshot::backup(device_id, app_version),
        };
        snapshot.entities.insert(
            "sessions".to_string(),
            serde_json::to_string(&self.load_sessions()?).map_err(std::io::Error::other)?,
        );
        Ok(snapshot)
    }

    fn apply_raw_portable_snapshot(
        &self,
        snapshot: &RawPortableSnapshot,
    ) -> Result<(), std::io::Error> {
        let sessions = snapshot
            .entities
            .get("sessions")
            .ok_or_else(|| std::io::Error::other("missing sessions"))?;
        self.replace_sessions(&serde_json::from_str(sessions).map_err(std::io::Error::other)?)
    }

    fn db_path(&self) -> PathBuf {
        self.config_dir.join("nyaterm.redb")
    }
}

struct TestLocalStore;

impl CloudLocalStore for TestLocalStore {
    fn encode_sync_snapshot(
        &self,
        snapshot: &RawPortableSnapshot,
        master_password: &str,
    ) -> Result<Vec<u8>, CloudSyncError> {
        let bytes = serde_json::to_vec(snapshot)?;
        crate::encrypt_snapshot_bytes(master_password, &bytes)
            .map_err(CloudSyncError::PortableSnapshot)
    }

    fn decode_sync_snapshot(
        &self,
        bytes: &[u8],
        master_password: &str,
    ) -> Result<RawPortableSnapshot, CloudSyncError> {
        let bytes = crate::decrypt_snapshot_bytes(master_password, bytes)?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    fn encode_sync_pointer(&self, pointer: &RemoteSyncPointer) -> Result<Vec<u8>, CloudSyncError> {
        Ok(serde_json::to_vec(pointer)?)
    }

    fn decode_sync_pointer(&self, bytes: &[u8]) -> Result<RemoteSyncPointer, CloudSyncError> {
        Ok(serde_json::from_slice(bytes)?)
    }

    fn build_sync_snapshot(
        &self,
        options: &LocalCloudSyncOptions,
    ) -> Result<RawPortableSnapshot, CloudSyncError> {
        let store = ConnectionStore::open_with_portable_key_path(
            &options.config_dir,
            options.portable_key_path.clone(),
        )
        .map_err(|error| CloudSyncError::LocalStore(error.to_string()))?;
        store
            .build_raw_portable_snapshot(
                PortableSnapshotKind::Sync,
                options.device_id.clone(),
                options.app_version.clone(),
            )
            .map_err(|error| CloudSyncError::LocalStore(error.to_string()))
    }

    fn apply_sync_snapshot(
        &self,
        options: &LocalCloudSyncOptions,
        snapshot: &RawPortableSnapshot,
    ) -> Result<CloudSyncBackupInfo, CloudSyncError> {
        let store = ConnectionStore::open_with_portable_key_path(
            &options.config_dir,
            options.portable_key_path.clone(),
        )
        .map_err(|error| CloudSyncError::LocalStore(error.to_string()))?;
        store
            .apply_raw_portable_snapshot(snapshot)
            .map_err(|error| CloudSyncError::LocalStore(error.to_string()))?;
        Ok(CloudSyncBackupInfo {
            database_path: store.db_path().to_path_buf(),
            safety_backup_path: None,
        })
    }

    fn persist_cloud_sync_state(&self, state: &CloudSyncState) -> Result<(), CloudSyncError> {
        Err(CloudSyncError::LocalStore(format!(
            "test store requires options to persist state for {}",
            state.device_id
        )))
    }
}

struct OptionsTestLocalStore<'a>(&'a LocalCloudSyncOptions);

impl CloudLocalStore for OptionsTestLocalStore<'_> {
    fn encode_sync_snapshot(
        &self,
        snapshot: &RawPortableSnapshot,
        master_password: &str,
    ) -> Result<Vec<u8>, CloudSyncError> {
        TestLocalStore.encode_sync_snapshot(snapshot, master_password)
    }

    fn decode_sync_snapshot(
        &self,
        bytes: &[u8],
        master_password: &str,
    ) -> Result<RawPortableSnapshot, CloudSyncError> {
        TestLocalStore.decode_sync_snapshot(bytes, master_password)
    }

    fn encode_sync_pointer(&self, pointer: &RemoteSyncPointer) -> Result<Vec<u8>, CloudSyncError> {
        TestLocalStore.encode_sync_pointer(pointer)
    }

    fn decode_sync_pointer(&self, bytes: &[u8]) -> Result<RemoteSyncPointer, CloudSyncError> {
        TestLocalStore.decode_sync_pointer(bytes)
    }

    fn build_sync_snapshot(
        &self,
        options: &LocalCloudSyncOptions,
    ) -> Result<RawPortableSnapshot, CloudSyncError> {
        TestLocalStore.build_sync_snapshot(options)
    }

    fn apply_sync_snapshot(
        &self,
        options: &LocalCloudSyncOptions,
        snapshot: &RawPortableSnapshot,
    ) -> Result<CloudSyncBackupInfo, CloudSyncError> {
        TestLocalStore.apply_sync_snapshot(options, snapshot)
    }

    fn persist_cloud_sync_state(&self, state: &CloudSyncState) -> Result<(), CloudSyncError> {
        let store = ConnectionStore::open_with_portable_key_path(
            &self.0.config_dir,
            self.0.portable_key_path.clone(),
        )
        .map_err(|error| CloudSyncError::LocalStore(error.to_string()))?;
        store
            .save_cloud_sync_state(state)
            .map_err(|error| CloudSyncError::LocalStore(error.to_string()))
    }
}

fn push_local_snapshot(
    options: &LocalCloudSyncOptions,
    state: &CloudSyncState,
    force: bool,
) -> Result<CloudSyncResult, CloudSyncError> {
    super::push_local_snapshot(&OptionsTestLocalStore(options), options, state, force)
}

fn pull_local_snapshot(
    options: &LocalCloudSyncOptions,
    state: &CloudSyncState,
    force: bool,
) -> Result<CloudSyncResult, CloudSyncError> {
    super::pull_local_snapshot(&OptionsTestLocalStore(options), options, state, force)
}

fn push_snapshot_with_remote(
    options: &LocalCloudSyncOptions,
    remote: &dyn CloudSyncRemote,
    state: &CloudSyncState,
    force: bool,
) -> Result<CloudSyncResult, CloudSyncError> {
    super::push_snapshot_with_remote(
        &OptionsTestLocalStore(options),
        options,
        remote,
        state,
        force,
    )
}

fn pull_snapshot_with_remote(
    options: &LocalCloudSyncOptions,
    remote: &dyn CloudSyncRemote,
    state: &CloudSyncState,
    force: bool,
) -> Result<CloudSyncResult, CloudSyncError> {
    super::pull_snapshot_with_remote(
        &OptionsTestLocalStore(options),
        options,
        remote,
        state,
        force,
    )
}

fn recover_current_snapshot_with_remote(
    options: &LocalCloudSyncOptions,
    remote: &dyn CloudSyncRemote,
) -> Result<CloudSyncResult, CloudSyncError> {
    super::recover_current_snapshot_with_remote(&OptionsTestLocalStore(options), options, remote)
}

#[derive(Default)]
struct MemoryRemote {
    files: Mutex<HashMap<String, Vec<u8>>>,
    fail_writes_containing: Mutex<VecDeque<String>>,
}

impl MemoryRemote {
    fn fail_next_write_containing(&self, path_fragment: &str) {
        self.fail_writes_containing
            .lock()
            .expect("failure queue lock")
            .push_back(path_fragment.to_string());
    }
}

#[derive(Default)]
struct MemorySnippetBackend {
    blobs: Mutex<std::collections::BTreeMap<String, String>>,
}

#[derive(Clone)]
struct RecordingSnippetHttpClient {
    inner: Arc<RecordingSnippetHttpClientInner>,
}

struct RecordingSnippetHttpClientInner {
    requests: Mutex<Vec<SnippetHttpRequest>>,
    responses: Mutex<VecDeque<Result<SnippetHttpResponse, CloudSyncError>>>,
}

impl RecordingSnippetHttpClient {
    fn new(responses: Vec<SnippetHttpResponse>) -> Self {
        Self {
            inner: Arc::new(RecordingSnippetHttpClientInner {
                requests: Mutex::new(Vec::new()),
                responses: Mutex::new(responses.into_iter().map(Ok).collect()),
            }),
        }
    }

    fn requests(&self) -> Vec<SnippetHttpRequest> {
        self.inner
            .requests
            .lock()
            .expect("http requests lock")
            .clone()
    }
}

impl SnippetHttpClient for RecordingSnippetHttpClient {
    fn send(&self, request: SnippetHttpRequest) -> Result<SnippetHttpResponse, CloudSyncError> {
        self.inner
            .requests
            .lock()
            .expect("http requests lock")
            .push(request);
        self.inner
            .responses
            .lock()
            .expect("http responses lock")
            .pop_front()
            .expect("queued response")
    }
}

impl SnippetBlobBackend for MemorySnippetBackend {
    fn fetch_blob(&self, filename: &str) -> Result<Option<String>, CloudSyncError> {
        Ok(self
            .blobs
            .lock()
            .expect("snippet lock")
            .get(filename)
            .cloned())
    }

    fn patch_blobs(
        &self,
        files: std::collections::BTreeMap<String, Option<String>>,
    ) -> Result<(), CloudSyncError> {
        let mut blobs = self.blobs.lock().expect("snippet lock");
        for (filename, content) in files {
            match content {
                Some(content) => {
                    blobs.insert(filename, content);
                }
                None => {
                    blobs.remove(&filename);
                }
            }
        }
        Ok(())
    }

    fn list_blob_names(&self) -> Result<Vec<String>, CloudSyncError> {
        Ok(self
            .blobs
            .lock()
            .expect("snippet lock")
            .keys()
            .cloned()
            .collect())
    }
}

impl CloudSyncRemote for MemoryRemote {
    fn provider(&self) -> &'static str {
        "memory"
    }

    fn create_dir(&self, _path: &str) -> Result<(), CloudSyncError> {
        Ok(())
    }

    fn read_if_exists(&self, path: &str) -> Result<Option<Vec<u8>>, CloudSyncError> {
        Ok(self.files.lock().expect("memory lock").get(path).cloned())
    }

    fn write(&self, path: &str, bytes: &[u8]) -> Result<(), CloudSyncError> {
        let mut failures = self
            .fail_writes_containing
            .lock()
            .expect("failure queue lock");
        if failures
            .front()
            .is_some_and(|fragment| path.contains(fragment))
        {
            failures.pop_front();
            return Err(CloudSyncError::Remote(format!(
                "injected write failure for {path}"
            )));
        }
        drop(failures);
        self.files
            .lock()
            .expect("memory lock")
            .insert(path.to_string(), bytes.to_vec());
        Ok(())
    }

    fn delete(&self, path: &str) -> Result<(), CloudSyncError> {
        self.files.lock().expect("memory lock").remove(path);
        Ok(())
    }

    fn list_files(&self, path: &str) -> Result<Vec<String>, CloudSyncError> {
        Ok(self
            .files
            .lock()
            .expect("memory lock")
            .keys()
            .filter(|key| key.starts_with(path))
            .cloned()
            .collect())
    }
}

#[derive(Default)]
struct ConcurrentUpdateRemote {
    inner: MemoryRemote,
    latest_reads_until_replace: Mutex<Option<usize>>,
    replacement_latest: Mutex<Option<Vec<u8>>>,
}

impl ConcurrentUpdateRemote {
    fn replace_latest_on_second_read(&self, bytes: Vec<u8>) {
        *self
            .latest_reads_until_replace
            .lock()
            .expect("read counter lock") = Some(2);
        *self.replacement_latest.lock().expect("replacement lock") = Some(bytes);
    }
}

impl CloudSyncRemote for ConcurrentUpdateRemote {
    fn provider(&self) -> &'static str {
        "concurrent-memory"
    }

    fn create_dir(&self, path: &str) -> Result<(), CloudSyncError> {
        self.inner.create_dir(path)
    }

    fn read_if_exists(&self, path: &str) -> Result<Option<Vec<u8>>, CloudSyncError> {
        if path.ends_with(super::SYNC_LATEST_FILE) {
            let mut counter = self
                .latest_reads_until_replace
                .lock()
                .expect("read counter lock");
            if let Some(remaining) = counter.as_mut() {
                *remaining = remaining.saturating_sub(1);
                if *remaining == 0 {
                    if let Some(bytes) = self
                        .replacement_latest
                        .lock()
                        .expect("replacement lock")
                        .take()
                    {
                        self.inner
                            .files
                            .lock()
                            .expect("memory lock")
                            .insert(path.to_string(), bytes);
                    }
                    *counter = None;
                }
            }
        }
        self.inner.read_if_exists(path)
    }

    fn write(&self, path: &str, bytes: &[u8]) -> Result<(), CloudSyncError> {
        self.inner.write(path, bytes)
    }

    fn delete(&self, path: &str) -> Result<(), CloudSyncError> {
        self.inner.delete(path)
    }

    fn list_files(&self, path: &str) -> Result<Vec<String>, CloudSyncError> {
        self.inner.list_files(path)
    }
}

#[test]
fn remote_path_joins_without_duplicate_slashes() {
    assert_eq!(
        remote_path("nyaterm", "sync/latest.redb"),
        "nyaterm/sync/latest.redb"
    );
    assert_eq!(
        remote_path("/nyaterm/", "/sync/latest.redb"),
        "nyaterm/sync/latest.redb"
    );
    assert_eq!(remote_path("", "sync/latest.redb"), "sync/latest.redb");
}

#[test]
fn remote_pointer_defaults_legacy_documents_to_schema_v1() {
    let pointer: RemoteSyncPointer = serde_json::from_value(serde_json::json!({
        "revision_id": "r1",
        "created_at_ms": 1,
        "payload_hash": "hash",
        "device_id": "device",
        "app_version": "1"
    }))
    .expect("legacy pointer");
    assert_eq!(pointer.schema_version, 1);
}

#[test]
fn drive_remote_segments_trim_root_and_child_paths() {
    assert_eq!(
        drive_remote_segments("/root/", "/sync/latest.redb"),
        vec!["root", "sync", "latest.redb"]
    );
    assert_eq!(
        drive_remote_segments("", "nyaterm//sync/latest.redb"),
        vec!["nyaterm", "sync", "latest.redb"]
    );
}

#[test]
fn google_drive_query_literal_escapes_quotes_and_backslashes() {
    assert_eq!(google_drive_query_literal("a'b\\c"), "'a\\'b\\\\c'");
}

#[test]
fn s3_signed_request_uses_path_style_url_and_headers() {
    let settings = S3SyncSettings {
        endpoint: "https://s3.example.com/".to_string(),
        bucket: "nyaterm-sync".to_string(),
        region: "ap-east-1".to_string(),
        root: "/profiles/default/".to_string(),
        access_key_id: Some("AKIDEXAMPLE".to_string().into()),
        secret_access_key: Some("SECRET".to_string().into()),
        session_token: Some("SESSION".to_string().into()),
        virtual_host_style: false,
    };
    let request = build_s3_signed_request(
        &settings,
        S3HttpMethod::Put,
        "/nyaterm/sync/latest redb",
        &s3_payload_sha256(b"payload"),
        UNIX_EPOCH + Duration::from_secs(1_704_067_200),
    )
    .expect("signed request");

    assert_eq!(
        request.url,
        "https://s3.example.com/nyaterm-sync/profiles/default/nyaterm/sync/latest%20redb"
    );
    assert_eq!(
        request.headers.get("x-amz-date").map(String::as_str),
        Some("20240101T000000Z")
    );
    assert_eq!(
        request
            .headers
            .get("x-amz-security-token")
            .map(String::as_str),
        Some("SESSION")
    );
    let authorization = request.headers.get("authorization").expect("authorization");
    assert!(authorization.contains("Credential=AKIDEXAMPLE/20240101/ap-east-1/s3/aws4_request"));
    assert!(
        authorization
            .contains("SignedHeaders=host;x-amz-content-sha256;x-amz-date;x-amz-security-token")
    );
}

#[test]
fn s3_signed_request_supports_virtual_host_style() {
    let settings = S3SyncSettings {
        endpoint: "https://objects.example.com/base".to_string(),
        bucket: "nyaterm".to_string(),
        region: String::new(),
        root: String::new(),
        access_key_id: Some("key".to_string().into()),
        secret_access_key: Some("secret".to_string().into()),
        session_token: None,
        virtual_host_style: true,
    };
    let request = build_s3_signed_request(
        &settings,
        S3HttpMethod::Get,
        "sync/current.redb.enc",
        &s3_payload_sha256(&[]),
        UNIX_EPOCH,
    )
    .expect("signed request");

    assert_eq!(
        request.url,
        "https://nyaterm.objects.example.com/base/sync/current.redb.enc"
    );
    assert_eq!(
        request.headers.get("host").map(String::as_str),
        Some("nyaterm.objects.example.com")
    );
    assert!(request.headers["authorization"].contains("/19700101/us-east-1/s3/aws4_request"));
}

#[test]
fn s3_signed_list_request_sorts_and_encodes_query_parameters() {
    let settings = S3SyncSettings {
        endpoint: "https://s3.example.com".to_string(),
        bucket: "bucket".to_string(),
        region: "us-east-1".to_string(),
        access_key_id: Some("key".to_string().into()),
        secret_access_key: Some("secret".to_string().into()),
        ..S3SyncSettings::default()
    };
    let query = BTreeMap::from([
        ("prefix".to_string(), "nyaterm/sync snapshots/".to_string()),
        ("list-type".to_string(), "2".to_string()),
        ("continuation-token".to_string(), "next+/=".to_string()),
    ]);

    let request = build_s3_signed_request_with_query(
        &settings,
        S3HttpMethod::Get,
        "",
        &query,
        &s3_payload_sha256(&[]),
        UNIX_EPOCH + Duration::from_secs(1_704_067_200),
    )
    .expect("signed list request");

    assert_eq!(
        request.url,
        "https://s3.example.com/bucket?continuation-token=next%2B%2F%3D&list-type=2&prefix=nyaterm%2Fsync%20snapshots%2F"
    );
    assert!(
        request.headers["authorization"]
            .contains("SignedHeaders=host;x-amz-content-sha256;x-amz-date")
    );
}

#[test]
fn s3_signed_request_requires_bucket_and_credentials() {
    let settings = S3SyncSettings {
        endpoint: "https://s3.example.com".to_string(),
        access_key_id: Some("key".to_string().into()),
        secret_access_key: Some("secret".to_string().into()),
        ..S3SyncSettings::default()
    };
    let error = build_s3_signed_request(
        &settings,
        S3HttpMethod::Head,
        "sync/latest.redb",
        &s3_payload_sha256(&[]),
        UNIX_EPOCH,
    )
    .expect_err("missing bucket");
    assert!(error.to_string().contains("S3 bucket is required"));

    let settings = S3SyncSettings {
        endpoint: "https://s3.example.com".to_string(),
        bucket: "bucket".to_string(),
        ..S3SyncSettings::default()
    };
    let error = build_s3_signed_request(
        &settings,
        S3HttpMethod::Head,
        "sync/latest.redb",
        &s3_payload_sha256(&[]),
        UNIX_EPOCH,
    )
    .expect_err("missing access key");
    assert!(error.to_string().contains("S3 access key ID is required"));
}

#[test]
fn cloud_sync_history_append_and_read_matches_legacy_log_shape() {
    let dir = unique_temp_dir("cloud-history-append");
    let entry = CloudSyncHistoryEntry {
        id: "history-1".to_string(),
        timestamp_ms: 300,
        kind: "sync".to_string(),
        status: "success".to_string(),
        trigger: "manual_push".to_string(),
        provider: Some("local_directory".to_string()),
        revision: Some("rev-1".to_string()),
        duration_ms: Some(42),
        message: "uploaded".to_string(),
    };

    append_cloud_sync_history(&dir, &entry).expect("append history");
    let entries =
        read_cloud_sync_history(&dir, 7, CLOUD_SYNC_HISTORY_LIMIT).expect("read appended history");

    assert_eq!(entries, vec![entry]);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn cloud_sync_history_reads_only_recent_cloud_entries_with_limit() {
    let dir = unique_temp_dir("cloud-history-limit");
    let path = dir.join(format!(
        "{}-legacy.{}",
        crate::diagnostics::LOG_FILE_PREFIX,
        crate::diagnostics::LOG_FILE_SUFFIX
    ));
    let lines = [
        serde_json::json!({
            "domain": "session.lifecycle",
            "event": "entry",
            "message": "ignored",
            "data": {
                "id": "ignored",
                "timestamp_ms": 999,
                "kind": "sync",
                "status": "success",
                "trigger": "manual_push"
            }
        })
        .to_string(),
        history_line("old", 100),
        history_line("new", 200),
    ];
    std::fs::write(&path, lines.join("\n")).expect("write legacy history log");

    let entries = read_cloud_sync_history(&dir, 7, 1).expect("read history");

    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<Vec<_>>(),
        vec!["new"]
    );
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn local_cloud_sync_push_and_forced_pull_round_trip() {
    let source_dir = unique_temp_dir("cloud-source");
    let target_dir = unique_temp_dir("cloud-target");
    let remote_dir = unique_temp_dir("cloud-remote");
    let source_options = options(&source_dir, &remote_dir, "source-device");
    let target_options = options(&target_dir, &remote_dir, "target-device");
    let source_store = ConnectionStore::open(&source_dir).expect("source store");
    source_store
        .replace_sessions(&SessionsConfig {
            custom_icons: Vec::new(),
            groups: Vec::new(),
            connections: vec![local_connection("conn-1", "Synced Shell", "bash")],
        })
        .expect("seed source");
    drop(source_store);

    let push = push_local_snapshot(&source_options, &CloudSyncState::default(), false)
        .expect("push snapshot");
    assert_eq!(push.status.message, "Cloud sync snapshot uploaded");
    assert!(remote_dir.join("nyaterm/sync/current.redb.enc").exists());
    assert!(remote_dir.join("nyaterm/sync/latest.redb").exists());
    let saved_source_state = ConnectionStore::open(&source_dir)
        .expect("source reopen")
        .load_cloud_sync_state()
        .expect("source cloud state");
    assert_eq!(
        saved_source_state.last_synced_payload_hash,
        push.state.last_synced_payload_hash
    );

    let pull = pull_local_snapshot(&target_options, &CloudSyncState::default(), true)
        .expect("pull snapshot");
    assert_eq!(pull.status.message, "Cloud sync snapshot downloaded");
    assert!(pull.backup.is_some());
    let saved_target_state = ConnectionStore::open(&target_dir)
        .expect("target reopen")
        .load_cloud_sync_state()
        .expect("target cloud state");
    assert_eq!(
        saved_target_state.last_applied_remote_revision,
        pull.state.last_applied_remote_revision
    );

    let loaded = ConnectionStore::open(&target_dir)
        .expect("target store")
        .load_sessions()
        .expect("load target");
    assert_eq!(loaded.connections[0].name, "Synced Shell");
    assert_eq!(
        pull.state.last_synced_payload_hash,
        push.state.last_synced_payload_hash
    );

    std::fs::remove_dir_all(source_dir).ok();
    std::fs::remove_dir_all(target_dir).ok();
    std::fs::remove_dir_all(remote_dir).ok();
}

#[test]
fn cloud_sync_algorithm_uses_remote_backend_abstraction() {
    let source_dir = unique_temp_dir("cloud-remote-source");
    let target_dir = unique_temp_dir("cloud-remote-target");
    let remote_dir = unique_temp_dir("cloud-remote-unused");
    let source_options = options(&source_dir, &remote_dir, "source-device");
    let target_options = options(&target_dir, &remote_dir, "target-device");
    let remote = MemoryRemote::default();

    ConnectionStore::open(&source_dir)
        .expect("source store")
        .replace_sessions(&SessionsConfig {
            custom_icons: Vec::new(),
            groups: Vec::new(),
            connections: vec![local_connection("conn-1", "Remote Trait Shell", "bash")],
        })
        .expect("seed source");

    let push =
        push_snapshot_with_remote(&source_options, &remote, &CloudSyncState::default(), false)
            .expect("push through memory remote");
    assert_eq!(push.status.provider, "memory");
    assert!(
        remote
            .read_if_exists("nyaterm/sync/latest.redb")
            .expect("read pointer")
            .is_some()
    );

    let pull =
        pull_snapshot_with_remote(&target_options, &remote, &CloudSyncState::default(), true)
            .expect("pull through memory remote");
    assert_eq!(pull.status.provider, "memory");

    let loaded = ConnectionStore::open(&target_dir)
        .expect("target store")
        .load_sessions()
        .expect("load target");
    assert_eq!(loaded.connections[0].name, "Remote Trait Shell");

    std::fs::remove_dir_all(source_dir).ok();
    std::fs::remove_dir_all(target_dir).ok();
    std::fs::remove_dir_all(remote_dir).ok();
}

#[test]
fn snapshot_upload_failure_does_not_commit_latest_pointer() {
    let source_dir = unique_temp_dir("cloud-upload-failure-source");
    let unused_remote = unique_temp_dir("cloud-upload-failure-unused");
    let source_options = options(&source_dir, &unused_remote, "source-device");
    let remote = MemoryRemote::default();
    remote.fail_next_write_containing("sync/snapshots/");

    let error =
        push_snapshot_with_remote(&source_options, &remote, &CloudSyncState::default(), false)
            .expect_err("snapshot upload must fail");

    assert!(matches!(error, CloudSyncError::Remote(_)));
    assert!(
        super::load_sync_pointer_from_remote(
            &OptionsTestLocalStore(&source_options),
            &remote,
            &source_options.remote_root,
        )
        .expect("load latest")
        .is_none()
    );
    std::fs::remove_dir_all(source_dir).ok();
    std::fs::remove_dir_all(unused_remote).ok();
}

#[test]
fn pointer_commit_failure_keeps_previous_head_readable() {
    let source_dir = unique_temp_dir("cloud-pointer-failure-source");
    let unused_remote = unique_temp_dir("cloud-pointer-failure-unused");
    let source_options = options(&source_dir, &unused_remote, "source-device");
    let remote = MemoryRemote::default();
    let store = ConnectionStore::open(&source_dir).expect("source store");
    store
        .replace_sessions(&SessionsConfig {
            custom_icons: Vec::new(),
            groups: Vec::new(),
            connections: vec![local_connection("first", "First", "bash")],
        })
        .expect("seed first");
    drop(store);
    let first =
        push_snapshot_with_remote(&source_options, &remote, &CloudSyncState::default(), false)
            .expect("first push");
    ConnectionStore::open(&source_dir)
        .expect("source store reopen")
        .replace_sessions(&SessionsConfig {
            custom_icons: Vec::new(),
            groups: Vec::new(),
            connections: vec![local_connection("second", "Second", "zsh")],
        })
        .expect("seed second");
    remote.fail_next_write_containing(super::SYNC_LATEST_FILE);

    let error = push_snapshot_with_remote(&source_options, &remote, &first.state, false)
        .expect_err("pointer commit must fail");

    assert!(matches!(error, CloudSyncError::Remote(_)));
    let latest = super::load_sync_pointer_from_remote(
        &OptionsTestLocalStore(&source_options),
        &remote,
        &source_options.remote_root,
    )
    .expect("load latest")
    .expect("previous latest");
    assert_eq!(
        latest.revision_id,
        first.pointer.expect("first pointer").revision_id
    );
    super::protocol::read_snapshot_for_pointer(
        &OptionsTestLocalStore(&source_options),
        &remote,
        &source_options,
        &latest,
    )
    .expect("previous snapshot remains readable");
    std::fs::remove_dir_all(source_dir).ok();
    std::fs::remove_dir_all(unused_remote).ok();
}

#[test]
fn compatibility_snapshot_failure_does_not_reverse_committed_push() {
    let source_dir = unique_temp_dir("cloud-current-failure-source");
    let unused_remote = unique_temp_dir("cloud-current-failure-unused");
    let source_options = options(&source_dir, &unused_remote, "source-device");
    let remote = MemoryRemote::default();
    remote.fail_next_write_containing(super::SYNC_CURRENT_FILE);

    let pushed =
        push_snapshot_with_remote(&source_options, &remote, &CloudSyncState::default(), false)
            .expect("committed push");

    let pointer = pushed.pointer.expect("pointer");
    let latest = super::load_sync_pointer_from_remote(
        &OptionsTestLocalStore(&source_options),
        &remote,
        &source_options.remote_root,
    )
    .expect("load latest")
    .expect("latest");
    assert_eq!(latest.revision_id, pointer.revision_id);
    super::protocol::read_snapshot_for_pointer(
        &OptionsTestLocalStore(&source_options),
        &remote,
        &source_options,
        &pointer,
    )
    .expect("immutable snapshot");
    assert!(
        remote
            .read_if_exists(&remote_path(
                &source_options.remote_root,
                super::SYNC_CURRENT_FILE
            ))
            .expect("read current")
            .is_none()
    );
    std::fs::remove_dir_all(source_dir).ok();
    std::fs::remove_dir_all(unused_remote).ok();
}

#[test]
fn concurrent_pointer_update_is_rejected_before_commit() {
    let source_dir = unique_temp_dir("cloud-concurrent-source");
    let unused_remote = unique_temp_dir("cloud-concurrent-unused");
    let source_options = options(&source_dir, &unused_remote, "source-device");
    let remote = ConcurrentUpdateRemote::default();
    let store = ConnectionStore::open(&source_dir).expect("source store");
    store
        .replace_sessions(&SessionsConfig {
            custom_icons: Vec::new(),
            groups: Vec::new(),
            connections: vec![local_connection("first", "First", "bash")],
        })
        .expect("seed first");
    drop(store);
    let first =
        push_snapshot_with_remote(&source_options, &remote, &CloudSyncState::default(), false)
            .expect("first push");
    ConnectionStore::open(&source_dir)
        .expect("source store reopen")
        .replace_sessions(&SessionsConfig {
            custom_icons: Vec::new(),
            groups: Vec::new(),
            connections: vec![local_connection("second", "Second", "zsh")],
        })
        .expect("seed second");
    let competitor = remote_pointer("competitor", "competing-hash");
    remote.replace_latest_on_second_read(
        TestLocalStore
            .encode_sync_pointer(&competitor)
            .expect("encode competing pointer"),
    );

    let error = push_snapshot_with_remote(&source_options, &remote, &first.state, false)
        .expect_err("concurrent update");

    assert!(matches!(error, CloudSyncError::ConcurrentUpdate { .. }));
    let latest = super::load_sync_pointer_from_remote(
        &OptionsTestLocalStore(&source_options),
        &remote,
        &source_options.remote_root,
    )
    .expect("load latest")
    .expect("latest");
    assert_eq!(latest.revision_id, competitor.revision_id);
    std::fs::remove_dir_all(source_dir).ok();
    std::fs::remove_dir_all(unused_remote).ok();
}

#[test]
fn pull_rejects_hash_revision_and_corrupted_snapshot_data() {
    let source_dir = unique_temp_dir("cloud-validation-source");
    let target_dir = unique_temp_dir("cloud-validation-target");
    let unused_remote = unique_temp_dir("cloud-validation-unused");
    let source_options = options(&source_dir, &unused_remote, "source-device");
    let target_options = options(&target_dir, &unused_remote, "target-device");
    let remote = MemoryRemote::default();
    let pushed =
        push_snapshot_with_remote(&source_options, &remote, &CloudSyncState::default(), false)
            .expect("push");
    let pointer = pushed.pointer.expect("pointer");
    let snapshot_path = remote_path(
        &source_options.remote_root,
        &super::legacy_sync_snapshot_file(&pointer.revision_id),
    );
    let encrypted = remote
        .read_if_exists(&snapshot_path)
        .expect("read snapshot")
        .expect("snapshot");

    let mut wrong_hash = pointer.clone();
    wrong_hash.payload_hash = "wrong-hash".to_string();
    remote
        .write(
            &remote_path(&source_options.remote_root, super::SYNC_LATEST_FILE),
            &TestLocalStore
                .encode_sync_pointer(&wrong_hash)
                .expect("encode wrong hash pointer"),
        )
        .expect("write wrong hash pointer");
    assert!(matches!(
        pull_snapshot_with_remote(&target_options, &remote, &CloudSyncState::default(), true),
        Err(CloudSyncError::HashMismatch { .. })
    ));

    let mut wrong_revision = pointer.clone();
    wrong_revision.revision_id = "wrong-revision".to_string();
    remote
        .write(
            &remote_path(
                &source_options.remote_root,
                &super::legacy_sync_snapshot_file(&wrong_revision.revision_id),
            ),
            &encrypted,
        )
        .expect("write aliased snapshot");
    remote
        .write(
            &remote_path(&source_options.remote_root, super::SYNC_LATEST_FILE),
            &TestLocalStore
                .encode_sync_pointer(&wrong_revision)
                .expect("encode wrong revision pointer"),
        )
        .expect("write wrong revision pointer");
    assert!(matches!(
        pull_snapshot_with_remote(&target_options, &remote, &CloudSyncState::default(), true),
        Err(CloudSyncError::RevisionMismatch { .. })
    ));

    remote
        .write(
            &remote_path(&source_options.remote_root, super::SYNC_LATEST_FILE),
            &TestLocalStore
                .encode_sync_pointer(&pointer)
                .expect("encode pointer"),
        )
        .expect("restore pointer");
    remote
        .write(&snapshot_path, b"broken")
        .expect("write corrupt snapshot");
    assert!(matches!(
        pull_snapshot_with_remote(&target_options, &remote, &CloudSyncState::default(), true),
        Err(CloudSyncError::CorruptedSnapshot { .. })
    ));

    std::fs::remove_dir_all(source_dir).ok();
    std::fs::remove_dir_all(target_dir).ok();
    std::fs::remove_dir_all(unused_remote).ok();
}

#[test]
fn missing_immutable_snapshot_is_migrated_from_matching_current() {
    let source_dir = unique_temp_dir("cloud-migrate-source");
    let target_dir = unique_temp_dir("cloud-migrate-target");
    let unused_remote = unique_temp_dir("cloud-migrate-unused");
    let source_options = options(&source_dir, &unused_remote, "source-device");
    let target_options = options(&target_dir, &unused_remote, "target-device");
    let remote = MemoryRemote::default();
    ConnectionStore::open(&source_dir)
        .expect("source")
        .replace_sessions(&SessionsConfig {
            custom_icons: Vec::new(),
            groups: Vec::new(),
            connections: vec![local_connection("conn", "Migrated", "bash")],
        })
        .expect("seed");

    let pushed =
        push_snapshot_with_remote(&source_options, &remote, &CloudSyncState::default(), false)
            .expect("push");
    let pointer = pushed.pointer.expect("pointer");
    let snapshot_path = remote_path(
        &source_options.remote_root,
        &super::legacy_sync_snapshot_file(&pointer.revision_id),
    );
    remote.files.lock().expect("memory").remove(&snapshot_path);

    pull_snapshot_with_remote(&target_options, &remote, &CloudSyncState::default(), true)
        .expect("legacy migration pull");

    assert!(
        remote
            .files
            .lock()
            .expect("memory")
            .contains_key(&snapshot_path)
    );
    std::fs::remove_dir_all(source_dir).ok();
    std::fs::remove_dir_all(target_dir).ok();
    std::fs::remove_dir_all(unused_remote).ok();
}

#[test]
fn inconsistent_remote_requires_explicit_current_recovery() {
    let first_dir = unique_temp_dir("cloud-recover-first");
    let second_dir = unique_temp_dir("cloud-recover-second");
    let target_dir = unique_temp_dir("cloud-recover-target");
    let unused_remote = unique_temp_dir("cloud-recover-unused");
    let first_options = options(&first_dir, &unused_remote, "first-device");
    let second_options = options(&second_dir, &unused_remote, "second-device");
    let target_options = options(&target_dir, &unused_remote, "target-device");
    let remote = MemoryRemote::default();

    ConnectionStore::open(&first_dir)
        .expect("first")
        .replace_sessions(&SessionsConfig {
            custom_icons: Vec::new(),
            groups: Vec::new(),
            connections: vec![local_connection("first", "Recover Me", "bash")],
        })
        .expect("seed first");
    let first =
        push_snapshot_with_remote(&first_options, &remote, &CloudSyncState::default(), false)
            .expect("first push");
    let current_path = remote_path(&first_options.remote_root, super::SYNC_CURRENT_FILE);
    let recoverable_current = remote
        .files
        .lock()
        .expect("memory")
        .get(&current_path)
        .cloned()
        .expect("current");

    ConnectionStore::open(&second_dir)
        .expect("second")
        .replace_sessions(&SessionsConfig {
            custom_icons: Vec::new(),
            groups: Vec::new(),
            connections: vec![local_connection("second", "New Head", "zsh")],
        })
        .expect("seed second");
    let second =
        push_snapshot_with_remote(&second_options, &remote, &CloudSyncState::default(), true)
            .expect("second push");
    let second_pointer = second.pointer.expect("second pointer");
    let second_snapshot_path = remote_path(
        &second_options.remote_root,
        &super::legacy_sync_snapshot_file(&second_pointer.revision_id),
    );
    {
        let mut files = remote.files.lock().expect("memory");
        files.remove(&second_snapshot_path);
        files.insert(current_path, recoverable_current);
    }

    let error =
        pull_snapshot_with_remote(&target_options, &remote, &CloudSyncState::default(), true)
            .expect_err("remote must be inconsistent");
    let CloudSyncError::Conflict(preview) = error else {
        panic!("expected structured conflict");
    };
    assert_eq!(preview.kind, CloudConflictKind::RemoteInconsistent);
    assert_eq!(
        preview.recovery_revision.as_deref(),
        first
            .pointer
            .as_ref()
            .map(|pointer| pointer.revision_id.as_str())
    );

    let recovered =
        recover_current_snapshot_with_remote(&target_options, &remote).expect("recover current");
    assert_eq!(
        recovered
            .pointer
            .as_ref()
            .map(|pointer| pointer.revision_id.as_str()),
        first
            .pointer
            .as_ref()
            .map(|pointer| pointer.revision_id.as_str())
    );
    let sessions = ConnectionStore::open(&target_dir)
        .expect("target")
        .load_sessions()
        .expect("sessions");
    assert_eq!(sessions.connections[0].name, "Recover Me");

    std::fs::remove_dir_all(first_dir).ok();
    std::fs::remove_dir_all(second_dir).ok();
    std::fs::remove_dir_all(target_dir).ok();
    std::fs::remove_dir_all(unused_remote).ok();
}

#[test]
fn snippet_remote_codec_matches_legacy_blob_layout_and_syncs() {
    let source_dir = unique_temp_dir("cloud-snippet-source");
    let target_dir = unique_temp_dir("cloud-snippet-target");
    let remote_dir = unique_temp_dir("cloud-snippet-unused");
    let source_options = options(&source_dir, &remote_dir, "source-device");
    let target_options = options(&target_dir, &remote_dir, "target-device");
    let backend = MemorySnippetBackend::default();
    let remote = SnippetRemote::new("gitee_snippet", backend);

    assert_eq!(
        snippet_remote_path(&snippet_remote_filename("nyaterm/sync/latest.redb")).as_deref(),
        Some("nyaterm/sync/latest.redb")
    );
    assert_eq!(
        decode_snippet_blob(&encode_snippet_blob(b"hello")).expect("decode"),
        b"hello"
    );

    ConnectionStore::open(&source_dir)
        .expect("source store")
        .replace_sessions(&SessionsConfig {
            custom_icons: Vec::new(),
            groups: Vec::new(),
            connections: vec![local_connection("conn-1", "Snippet Shell", "bash")],
        })
        .expect("seed source");

    let push =
        push_snapshot_with_remote(&source_options, &remote, &CloudSyncState::default(), false)
            .expect("push snippet");
    assert_eq!(push.status.provider, "gitee_snippet");
    assert!(
        remote
            .read_if_exists("nyaterm/sync/latest.redb")
            .expect("snippet pointer")
            .is_some()
    );

    let pull =
        pull_snapshot_with_remote(&target_options, &remote, &CloudSyncState::default(), true)
            .expect("pull snippet");
    assert_eq!(pull.status.provider, "gitee_snippet");

    let loaded = ConnectionStore::open(&target_dir)
        .expect("target store")
        .load_sessions()
        .expect("load target");
    assert_eq!(loaded.connections[0].name, "Snippet Shell");

    std::fs::remove_dir_all(source_dir).ok();
    std::fs::remove_dir_all(target_dir).ok();
    std::fs::remove_dir_all(remote_dir).ok();
}

#[test]
fn gitee_http_backend_fetches_raw_filename_with_access_token() {
    let settings = GiteeSnippetSyncSettings {
        api_endpoint: "https://gitee.example/api/v5/".to_string(),
        gist_id: "gist-1".to_string(),
        access_token: Some("token-1".to_string().into()),
    };
    let client = RecordingSnippetHttpClient::new(vec![SnippetHttpResponse {
        status: 200,
        body: encode_snippet_blob(b"hello"),
    }]);
    let backend = GiteeSnippetHttpBackend::new(&settings, client.clone()).expect("backend");

    let content = backend
        .fetch_blob(&snippet_remote_filename("nyaterm/sync/latest.redb"))
        .expect("fetch blob")
        .expect("blob");

    assert_eq!(decode_snippet_blob(&content).expect("decode"), b"hello");
    let requests = client.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, SnippetHttpMethod::Get);
    assert_eq!(
        requests[0].query.get("access_token").map(String::as_str),
        Some("token-1")
    );
    assert!(requests[0].url.contains("/gists/gist-1/raw/nyaterm-"));
}

#[test]
fn github_gist_http_backend_fetches_raw_url_for_truncated_file() {
    let filename = snippet_remote_filename("nyaterm/sync/current.redb.enc");
    let settings = GithubGistSyncSettings {
        gist_id: "gist-2".to_string(),
        access_token: Some("gh-token".to_string().into()),
    };
    let document = serde_json::json!({
        "files": {
            filename.clone(): {
                "content": "partial",
                "raw_url": "https://gist.example/raw/file",
                "truncated": true
            }
        }
    });
    let client = RecordingSnippetHttpClient::new(vec![
        SnippetHttpResponse {
            status: 200,
            body: document.to_string(),
        },
        SnippetHttpResponse {
            status: 200,
            body: encode_snippet_blob(b"full"),
        },
    ]);
    let backend = GithubGistHttpBackend::new(&settings, client.clone()).expect("backend");

    let content = backend
        .fetch_blob(&filename)
        .expect("fetch blob")
        .expect("blob");

    assert_eq!(decode_snippet_blob(&content).expect("decode"), b"full");
    let requests = client.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].url, "https://api.github.com/gists/gist-2");
    assert_eq!(requests[1].url, "https://gist.example/raw/file");
    assert_eq!(
        requests[0].headers.get("Authorization").map(String::as_str),
        Some("Bearer gh-token")
    );
}

#[test]
fn github_gist_http_backend_retries_retryable_update_conflict() {
    let settings = GithubGistSyncSettings {
        gist_id: "gist-3".to_string(),
        access_token: Some("gh-token".to_string().into()),
    };
    let client = RecordingSnippetHttpClient::new(vec![
        SnippetHttpResponse {
            status: 409,
            body: r#"{"message":"Gist cannot be updated."}"#.to_string(),
        },
        SnippetHttpResponse {
            status: 200,
            body: "{}".to_string(),
        },
    ]);
    let backend = GithubGistHttpBackend::new(&settings, client.clone()).expect("backend");
    let mut files = BTreeMap::new();
    files.insert("nyaterm-rev.blob".to_string(), Some("payload".to_string()));

    backend.patch_blobs(files).expect("patch retry");

    let requests = client.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0], requests[1]);
    assert_eq!(requests[0].method, SnippetHttpMethod::Patch);
}

#[test]
fn snippet_patch_bodies_match_gitee_and_github_shapes() {
    let mut files = BTreeMap::new();
    files.insert("nyaterm-a.blob".to_string(), Some("payload".to_string()));
    files.insert("nyaterm-b.blob".to_string(), None);

    let gitee = gitee_snippet_patch_body("token", files.clone());
    assert_eq!(gitee["access_token"], "token");
    assert_eq!(gitee["files"]["nyaterm-a.blob"]["content"], "payload");
    assert!(gitee["files"]["nyaterm-b.blob"].is_null());

    let github = github_gist_patch_body(files);
    assert!(github.get("access_token").is_none());
    assert_eq!(github["files"]["nyaterm-a.blob"]["content"], "payload");
    assert!(github["files"]["nyaterm-b.blob"].is_null());
}

#[test]
fn local_cloud_sync_detects_push_conflict() {
    let source_dir = unique_temp_dir("cloud-conflict-source");
    let other_dir = unique_temp_dir("cloud-conflict-other");
    let remote_dir = unique_temp_dir("cloud-conflict-remote");
    let source_options = options(&source_dir, &remote_dir, "source-device");
    let other_options = options(&other_dir, &remote_dir, "other-device");

    ConnectionStore::open(&source_dir)
        .expect("source")
        .replace_sessions(&SessionsConfig {
            custom_icons: Vec::new(),
            groups: Vec::new(),
            connections: vec![local_connection("conn-1", "Local A", "bash")],
        })
        .expect("seed source");
    let source_state = push_local_snapshot(&source_options, &CloudSyncState::default(), false)
        .expect("initial push")
        .state;

    ConnectionStore::open(&other_dir)
        .expect("other")
        .replace_sessions(&SessionsConfig {
            custom_icons: Vec::new(),
            groups: Vec::new(),
            connections: vec![local_connection("conn-2", "Remote B", "zsh")],
        })
        .expect("seed other");
    push_local_snapshot(&other_options, &CloudSyncState::default(), true)
        .expect("remote force push");

    ConnectionStore::open(&source_dir)
        .expect("source reopen")
        .replace_sessions(&SessionsConfig {
            custom_icons: Vec::new(),
            groups: Vec::new(),
            connections: vec![local_connection("conn-1", "Local Changed", "fish")],
        })
        .expect("change source");
    let error =
        push_local_snapshot(&source_options, &source_state, false).expect_err("conflict expected");
    assert!(matches!(error, CloudSyncError::Conflict(_)));

    std::fs::remove_dir_all(source_dir).ok();
    std::fs::remove_dir_all(other_dir).ok();
    std::fs::remove_dir_all(remote_dir).ok();
}

#[test]
fn local_cloud_sync_detects_pull_conflict_until_forced() {
    let source_dir = unique_temp_dir("cloud-pull-conflict-source");
    let target_dir = unique_temp_dir("cloud-pull-conflict-target");
    let other_dir = unique_temp_dir("cloud-pull-conflict-other");
    let remote_dir = unique_temp_dir("cloud-pull-conflict-remote");
    let source_options = options(&source_dir, &remote_dir, "source-device");
    let target_options = options(&target_dir, &remote_dir, "target-device");
    let other_options = options(&other_dir, &remote_dir, "other-device");

    ConnectionStore::open(&source_dir)
        .expect("source")
        .replace_sessions(&SessionsConfig {
            custom_icons: Vec::new(),
            groups: Vec::new(),
            connections: vec![local_connection("conn-1", "Initial", "bash")],
        })
        .expect("seed source");
    push_local_snapshot(&source_options, &CloudSyncState::default(), false).expect("initial push");
    let target_state = pull_local_snapshot(&target_options, &CloudSyncState::default(), true)
        .expect("initial pull")
        .state;

    ConnectionStore::open(&other_dir)
        .expect("other")
        .replace_sessions(&SessionsConfig {
            custom_icons: Vec::new(),
            groups: Vec::new(),
            connections: vec![local_connection("conn-2", "Remote Changed", "zsh")],
        })
        .expect("seed other");
    push_local_snapshot(&other_options, &CloudSyncState::default(), true)
        .expect("remote force push");

    ConnectionStore::open(&target_dir)
        .expect("target reopen")
        .replace_sessions(&SessionsConfig {
            custom_icons: Vec::new(),
            groups: Vec::new(),
            connections: vec![local_connection("conn-1", "Local Changed", "fish")],
        })
        .expect("change target");

    let error = pull_local_snapshot(&target_options, &target_state, false)
        .expect_err("pull conflict expected");
    assert!(matches!(error, CloudSyncError::Conflict(_)));

    pull_local_snapshot(&target_options, &target_state, true).expect("forced pull");
    let loaded = ConnectionStore::open(&target_dir)
        .expect("target final")
        .load_sessions()
        .expect("load target");
    assert_eq!(loaded.connections[0].name, "Remote Changed");

    std::fs::remove_dir_all(source_dir).ok();
    std::fs::remove_dir_all(target_dir).ok();
    std::fs::remove_dir_all(other_dir).ok();
    std::fs::remove_dir_all(remote_dir).ok();
}

#[test]
fn local_cloud_sync_wrong_password_does_not_replace_target() {
    let source_dir = unique_temp_dir("cloud-password-source");
    let target_dir = unique_temp_dir("cloud-password-target");
    let remote_dir = unique_temp_dir("cloud-password-remote");
    let source_options = options(&source_dir, &remote_dir, "source-device");
    let mut wrong_options = options(&target_dir, &remote_dir, "target-device");
    wrong_options.master_password = "wrong".to_string().into();

    ConnectionStore::open(&source_dir)
        .expect("source")
        .replace_sessions(&SessionsConfig {
            custom_icons: Vec::new(),
            groups: Vec::new(),
            connections: vec![local_connection("conn-1", "Remote State", "bash")],
        })
        .expect("seed source");
    push_local_snapshot(&source_options, &CloudSyncState::default(), false).expect("push");

    ConnectionStore::open(&target_dir)
        .expect("target")
        .replace_sessions(&SessionsConfig {
            custom_icons: Vec::new(),
            groups: Vec::new(),
            connections: vec![local_connection("keep", "Keep Local", "zsh")],
        })
        .expect("seed target");

    let error = pull_local_snapshot(&wrong_options, &CloudSyncState::default(), true)
        .expect_err("wrong password");
    assert!(
        error
            .to_string()
            .contains("cloud snapshot decryption failed")
    );
    let loaded = ConnectionStore::open(&target_dir)
        .expect("target reopen")
        .load_sessions()
        .expect("load target");
    assert_eq!(loaded.connections[0].name, "Keep Local");

    std::fs::remove_dir_all(source_dir).ok();
    std::fs::remove_dir_all(target_dir).ok();
    std::fs::remove_dir_all(remote_dir).ok();
}

#[test]
fn masked_cloud_sync_merge_preserves_provider_secrets() {
    let mut current = CloudSyncSettings::default();
    current.webdav.password = Some("webdav-password".to_string().into());
    current.s3.secret_access_key = Some("s3-secret".to_string().into());
    current.google_drive.access_token = Some("google-access".to_string().into());
    current.google_drive.refresh_token = Some("google-refresh".to_string().into());
    current.google_drive.client_secret = Some("google-secret".to_string().into());
    current.onedrive.access_token = Some("onedrive-access".to_string().into());
    current.aliyun_drive.refresh_token = Some("aliyun-refresh".to_string().into());
    current.github_gist.access_token = Some("github-token".to_string().into());

    let mut next = CloudSyncSettings::default();
    next.webdav.password = Some(MASKED_SECRET_VALUE.to_string().into());
    next.s3.secret_access_key = Some(String::new().into());
    next.google_drive.access_token = Some(MASKED_SECRET_VALUE.to_string().into());
    next.google_drive.refresh_token = Some(MASKED_SECRET_VALUE.to_string().into());
    next.google_drive.client_secret = Some(MASKED_SECRET_VALUE.to_string().into());
    next.onedrive.access_token = Some(MASKED_SECRET_VALUE.to_string().into());
    next.aliyun_drive.refresh_token = Some(MASKED_SECRET_VALUE.to_string().into());
    next.github_gist.access_token = Some("replacement".to_string().into());

    let merged = merge_masked_cloud_sync_settings(&current, next);

    assert_eq!(merged.webdav.password.as_deref(), Some("webdav-password"));
    assert_eq!(merged.s3.secret_access_key, None);
    assert_eq!(
        merged.google_drive.access_token.as_deref(),
        Some("google-access")
    );
    assert_eq!(
        merged.google_drive.refresh_token.as_deref(),
        Some("google-refresh")
    );
    assert_eq!(
        merged.google_drive.client_secret.as_deref(),
        Some("google-secret")
    );
    assert_eq!(
        merged.onedrive.access_token.as_deref(),
        Some("onedrive-access")
    );
    assert_eq!(
        merged.aliyun_drive.refresh_token.as_deref(),
        Some("aliyun-refresh")
    );
    assert_eq!(
        merged.github_gist.access_token.as_deref(),
        Some("replacement")
    );
}

fn remote_pointer(revision_id: &str, payload_hash: &str) -> RemoteSyncPointer {
    RemoteSyncPointer {
        schema_version: super::REMOTE_SYNC_POINTER_SCHEMA_VERSION,
        revision_id: revision_id.to_string(),
        created_at_ms: 2,
        payload_hash: payload_hash.to_string(),
        device_id: "remote-device".to_string(),
        app_version: "test".to_string(),
    }
}

fn synced_state(revision_id: &str, payload_hash: &str) -> CloudSyncState {
    CloudSyncState {
        device_id: "local-device".to_string(),
        last_synced_payload_hash: Some(payload_hash.to_string()),
        last_applied_remote_revision: Some(revision_id.to_string()),
        last_checked_at_ms: None,
        last_synced_at_ms: None,
    }
}

#[test]
fn cloud_sync_settings_default_auto_pull_remote_changes_to_enabled() {
    let settings: CloudSyncSettings =
        serde_json::from_str(r#"{"enabled":true}"#).expect("legacy settings deserialize");

    assert!(settings.auto_pull_remote_changes);
    assert!(CloudSyncSettings::default().auto_pull_remote_changes);
}

#[test]
fn remote_check_decides_up_to_date_when_local_and_remote_match() {
    let state = synced_state("r1", "hash-1");
    let remote = remote_pointer("r2", "hash-1");

    assert_eq!(
        decide_cloud_remote_check(&state, "hash-1", &remote, true),
        CloudRemoteCheckDecision::UpToDate
    );
}

#[test]
fn remote_check_decides_auto_pull_when_remote_changed_and_local_clean() {
    let state = synced_state("r1", "hash-1");
    let remote = remote_pointer("r2", "hash-2");

    assert_eq!(
        decide_cloud_remote_check(&state, "hash-1", &remote, true),
        CloudRemoteCheckDecision::AutoPull
    );
}

#[test]
fn remote_check_decides_remote_available_when_auto_pull_disabled() {
    let state = synced_state("r1", "hash-1");
    let remote = remote_pointer("r2", "hash-2");

    assert_eq!(
        decide_cloud_remote_check(&state, "hash-1", &remote, false),
        CloudRemoteCheckDecision::RemoteAvailable
    );
}

#[test]
fn remote_check_decides_local_changed_when_only_local_changed() {
    let state = synced_state("r1", "hash-1");
    let remote = remote_pointer("r1", "hash-remote");

    assert_eq!(
        decide_cloud_remote_check(&state, "hash-local", &remote, true),
        CloudRemoteCheckDecision::LocalChanged
    );
}

#[test]
fn remote_check_decides_conflict_when_local_and_remote_changed() {
    let state = synced_state("r1", "hash-1");
    let remote = remote_pointer("r2", "hash-2");

    assert_eq!(
        decide_cloud_remote_check(&state, "hash-local", &remote, true),
        CloudRemoteCheckDecision::Conflict
    );
}

fn options(config_dir: &Path, remote_dir: &Path, device_id: &str) -> LocalCloudSyncOptions {
    LocalCloudSyncOptions {
        config_dir: config_dir.to_path_buf(),
        portable_key_path: None,
        remote_dir: remote_dir.to_path_buf(),
        remote_root: "nyaterm".to_string(),
        device_id: device_id.to_string(),
        app_version: "test".to_string(),
        master_password: "secret".to_string().into(),
        enabled: true,
    }
}

fn local_connection(id: &str, name: &str, shell: &str) -> SavedConnection {
    SavedConnection {
        extensions: Default::default(),
        id: id.to_string(),
        name: name.to_string(),
        config: ConnectionType::LocalTerminal {
            shell_path: shell.to_string(),
            shell_args: String::new(),
            working_dir: None,
            ai_execution_profile: AiExecutionProfile::Auto,
            encoding: String::new(),
            dynamic_tab_title: false,
        },
        group_id: None,
        description: None,
        sort_order: 0,
        icon: None,
        icon_auto_detect: None,
        auth: None,
        ssh_algorithms: None,
        ssh_profile: Default::default(),
        terminal_type: None,
        sftp: Default::default(),
        network: None,
        post_login: None,
        recording: None,
        asset: None,
        created_at_ms: None,
        updated_at_ms: None,
        last_used_at_ms: None,
    }
}

fn unique_temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "nyaterm-cloud-sync-{name}-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn history_line(id: &str, timestamp_ms: u64) -> String {
    serde_json::json!({
        "domain": CLOUD_SYNC_HISTORY_DOMAIN,
        "event": CLOUD_SYNC_HISTORY_EVENT,
        "message": format!("history {id}"),
        "data": {
            "id": id,
            "timestamp_ms": timestamp_ms,
            "kind": "sync",
            "status": "success",
            "trigger": "manual_pull",
            "provider": "webdav",
            "revision": null,
            "duration_ms": 1,
        }
    })
    .to_string()
}
