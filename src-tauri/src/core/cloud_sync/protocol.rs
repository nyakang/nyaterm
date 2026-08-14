use crate::core::portable_snapshot::{
    DecodedPortableSnapshot, PortableSnapshot, encode_portable_snapshot,
};
use crate::error::{AppResult, CloudSyncError};

use super::crypto::encrypt_snapshot_bytes;
use super::operator::CloudRemote;
use super::remote::{
    REMOTE_SYNC_POINTER_SCHEMA_VERSION, RemoteSyncPointer, SYNC_CURRENT_FILE,
    legacy_sync_snapshot_file, load_sync_pointer, remote_path, write_sync_pointer,
};
use super::snapshot_decode_helper::decode_remote_snapshot_with_source_hash_isolated;

const SNAPSHOT_HASH_LOG_PREFIX_LEN: usize = 12;
const MIN_ENCRYPTED_SNAPSHOT_BYTES: usize = 13;
const MAX_ENCRYPTED_SNAPSHOT_BYTES: usize = 64 * 1024 * 1024;

pub(super) fn sync_snapshot_file(revision: &str) -> String {
    legacy_sync_snapshot_file(revision)
}

pub(super) fn sync_snapshot_path(remote_root: &str, revision: &str) -> String {
    remote_path(remote_root, &sync_snapshot_file(revision))
}

pub(super) fn pointer_from_snapshot(snapshot: &PortableSnapshot) -> RemoteSyncPointer {
    RemoteSyncPointer {
        schema_version: REMOTE_SYNC_POINTER_SCHEMA_VERSION,
        revision_id: snapshot.revision_id.clone(),
        created_at_ms: snapshot.created_at_ms,
        payload_hash: snapshot.payload_hash.clone(),
        device_id: snapshot.device_id.clone(),
        app_version: snapshot.app_version.clone(),
    }
}

pub(super) async fn upload_sync_snapshot(
    remote: &CloudRemote,
    remote_root: &str,
    snapshot: &PortableSnapshot,
) -> AppResult<()> {
    let encoded = encode_portable_snapshot(snapshot)?;
    let encrypted = encrypt_snapshot_bytes(&encoded)?;
    remote
        .write(
            &sync_snapshot_path(remote_root, &snapshot.revision_id),
            encrypted,
        )
        .await
}

pub(super) async fn verify_uploaded_sync_snapshot(
    remote: &CloudRemote,
    remote_root: &str,
    pointer: &RemoteSyncPointer,
) -> AppResult<PortableSnapshot> {
    read_snapshot_for_pointer(remote, remote_root, pointer).await
}

pub(super) async fn read_snapshot_for_pointer(
    remote: &CloudRemote,
    remote_root: &str,
    pointer: &RemoteSyncPointer,
) -> AppResult<PortableSnapshot> {
    let path = sync_snapshot_path(remote_root, &pointer.revision_id);
    let Some(raw) = remote.read_if_exists(&path).await? else {
        tracing::warn!(
            remote_path = %path,
            revision = %pointer.revision_id,
            expected_hash = %short_hash(&pointer.payload_hash),
            "Remote sync snapshot file is missing"
        );
        return Err(CloudSyncError::SnapshotMissing {
            revision: pointer.revision_id.clone(),
        }
        .into());
    };
    log_remote_snapshot_read("snapshot", &path, Some(pointer), &raw);
    validate_remote_snapshot_size(&raw, &pointer.revision_id)?;
    let decoded = decode_remote_sync_snapshot(&raw, &pointer.revision_id).await?;
    tracing::info!(
        revision = %decoded.snapshot.revision_id,
        source_hash = %short_hash(&decoded.source_payload_hash),
        normalized_hash = %short_hash(&decoded.snapshot.payload_hash),
        "Remote sync snapshot decoded"
    );
    validate_decoded_snapshot_against_pointer(pointer, &decoded)?;
    tracing::info!(
        revision = %pointer.revision_id,
        expected_hash = %short_hash(&pointer.payload_hash),
        "Remote sync snapshot validated against pointer"
    );
    Ok(decoded.snapshot)
}

pub(super) async fn write_current_sync_snapshot_compat(
    remote: &CloudRemote,
    remote_root: &str,
    snapshot: &PortableSnapshot,
) -> AppResult<()> {
    let encoded = encode_portable_snapshot(snapshot)?;
    let encrypted = encrypt_snapshot_bytes(&encoded)?;
    remote
        .write(&remote_path(remote_root, SYNC_CURRENT_FILE), encrypted)
        .await
}

pub(super) async fn read_current_sync_snapshot_compat(
    remote: &CloudRemote,
    remote_root: &str,
) -> AppResult<Option<PortableSnapshot>> {
    read_current_sync_snapshot_compat_decoded(remote, remote_root)
        .await
        .map(|decoded| decoded.map(|decoded| decoded.snapshot))
}

pub(super) async fn read_current_sync_snapshot_compat_decoded(
    remote: &CloudRemote,
    remote_root: &str,
) -> AppResult<Option<DecodedPortableSnapshot>> {
    let path = remote_path(remote_root, SYNC_CURRENT_FILE);
    let Some(raw) = remote.read_if_exists(&path).await? else {
        tracing::info!(
            remote_path = %path,
            "Compatible current cloud sync snapshot is missing"
        );
        return Ok(None);
    };
    log_remote_snapshot_read("current", &path, None, &raw);
    validate_remote_snapshot_size(&raw, "current")?;
    decode_remote_sync_snapshot(&raw, "current").await.map(Some)
}

pub(super) async fn commit_sync_pointer(
    remote: &CloudRemote,
    remote_root: &str,
    pointer: &RemoteSyncPointer,
) -> AppResult<()> {
    write_sync_pointer(remote, remote_root, pointer).await
}

pub(super) async fn ensure_remote_head_unchanged(
    remote: &CloudRemote,
    remote_root: &str,
    expected: Option<&RemoteSyncPointer>,
) -> AppResult<()> {
    let actual = load_sync_pointer(remote, remote_root).await?;
    let expected_revision = expected.map(|pointer| pointer.revision_id.clone());
    let actual_revision = actual.as_ref().map(|pointer| pointer.revision_id.clone());
    if expected_revision != actual_revision {
        return Err(CloudSyncError::ConcurrentUpdate {
            expected_revision,
            actual_revision,
        }
        .into());
    }
    Ok(())
}

pub(super) fn validate_decoded_snapshot_against_pointer(
    pointer: &RemoteSyncPointer,
    decoded: &DecodedPortableSnapshot,
) -> AppResult<()> {
    validate_snapshot_source_hash_against_pointer(
        pointer,
        &decoded.snapshot,
        &decoded.source_payload_hash,
    )
}

fn validate_snapshot_source_hash_against_pointer(
    pointer: &RemoteSyncPointer,
    snapshot: &PortableSnapshot,
    source_payload_hash: &str,
) -> AppResult<()> {
    if snapshot.revision_id != pointer.revision_id {
        return Err(CloudSyncError::RevisionMismatch {
            pointer_revision: pointer.revision_id.clone(),
            snapshot_revision: snapshot.revision_id.clone(),
        }
        .into());
    }

    if source_payload_hash != pointer.payload_hash {
        return Err(CloudSyncError::HashMismatch {
            expected: pointer.payload_hash.clone(),
            actual: source_payload_hash.to_string(),
        }
        .into());
    }

    Ok(())
}

async fn decode_remote_sync_snapshot(
    raw: &[u8],
    revision: &str,
) -> AppResult<DecodedPortableSnapshot> {
    tracing::info!(
        revision,
        encrypted_bytes = raw.len(),
        "Decoding remote sync snapshot in isolated helper"
    );
    decode_remote_snapshot_with_source_hash_isolated(raw, revision).await
}

fn validate_remote_snapshot_size(raw: &[u8], revision: &str) -> AppResult<()> {
    if raw.len() < MIN_ENCRYPTED_SNAPSHOT_BYTES || raw.len() > MAX_ENCRYPTED_SNAPSHOT_BYTES {
        tracing::warn!(
            revision,
            encrypted_bytes = raw.len(),
            min_bytes = MIN_ENCRYPTED_SNAPSHOT_BYTES,
            max_bytes = MAX_ENCRYPTED_SNAPSHOT_BYTES,
            "Remote sync snapshot size is outside supported bounds"
        );
        return Err(CloudSyncError::CorruptedSnapshot {
            revision: revision.to_string(),
        }
        .into());
    }
    Ok(())
}

fn log_remote_snapshot_read(
    kind: &str,
    path: &str,
    pointer: Option<&RemoteSyncPointer>,
    raw: &[u8],
) {
    tracing::info!(
        kind,
        remote_path = %path,
        encrypted_bytes = raw.len(),
        pointer_revision = pointer.map(|pointer| pointer.revision_id.as_str()).unwrap_or(""),
        pointer_hash = %pointer.map(|pointer| short_hash(&pointer.payload_hash)).unwrap_or_default(),
        "Remote sync snapshot file read"
    );
}

fn short_hash(hash: &str) -> String {
    hash.chars().take(SNAPSHOT_HASH_LOG_PREFIX_LEN).collect()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use crate::config::{self, AppSettings};
    use crate::core::portable_snapshot::{
        PortableAppSettings, PortableSnapshotKind, calculate_payload_hash,
        calculate_v3_raw_payload_hash, encode_v3_raw_snapshot_redb_for_test,
    };
    use crate::error::AppError;
    use crate::utils::crypto::set_master_password;

    use super::super::migration::{RemoteSnapshotResolution, resolve_remote_snapshot};
    use super::super::operator::MemoryRemote;
    use super::super::remote::{load_sync_pointer, remote_path};
    use super::*;

    static MASTER_PASSWORD_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn memory_remote() -> (MemoryRemote, CloudRemote) {
        let memory = MemoryRemote::with_files(HashMap::new());
        let remote = CloudRemote::Memory(memory.clone());
        (memory, remote)
    }

    fn sample_snapshot(revision_id: &str, created_at_ms: u64) -> PortableSnapshot {
        let settings = PortableAppSettings::from_app_settings(
            &AppSettings::default(),
            &PortableSnapshotKind::Sync,
        );
        let mut snapshot = PortableSnapshot {
            schema_version: 3,
            snapshot_kind: PortableSnapshotKind::Sync,
            revision_id: revision_id.to_string(),
            device_id: "device".to_string(),
            created_at_ms,
            payload_hash: String::new(),
            app_version: "test".to_string(),
            settings,
            sessions: Default::default(),
            keys: Default::default(),
            passwords: Default::default(),
            credentials: Default::default(),
            otp: Default::default(),
            proxies: Default::default(),
            proxy_groups: Default::default(),
            tunnels: Default::default(),
            tunnel_groups: Default::default(),
            quick_commands: Default::default(),
            history: Default::default(),
            master_key_token: None,
            known_hosts: String::new(),
            notes: Default::default(),
        };
        snapshot.payload_hash = calculate_payload_hash(&snapshot).expect("hash snapshot");
        snapshot
    }

    fn sample_snapshot_with_known_hosts(
        revision_id: &str,
        created_at_ms: u64,
        known_hosts: &str,
    ) -> PortableSnapshot {
        let mut snapshot = sample_snapshot(revision_id, created_at_ms);
        snapshot.known_hosts = known_hosts.to_string();
        snapshot.payload_hash = calculate_payload_hash(&snapshot).expect("hash snapshot");
        snapshot
    }

    fn sample_snapshot_with_quick_command(
        revision_id: &str,
        created_at_ms: u64,
    ) -> PortableSnapshot {
        let mut snapshot = sample_snapshot(revision_id, created_at_ms);
        snapshot.quick_commands.commands.push(config::QuickCommand {
            id: "cmd-1".to_string(),
            label: "List".to_string(),
            command: "ls".to_string(),
            category_id: None,
            description: None,
            color_tag: None,
            icon_tag: None,
            pinned: false,
            execution_mode: "execute".to_string(),
            source: None,
            risk_level: None,
            updated_at: None,
            created_at: None,
            use_count: None,
            sort_order: None,
            target_connection_ids: None,
        });
        snapshot.payload_hash = calculate_payload_hash(&snapshot).expect("hash snapshot");
        snapshot
    }

    fn snapshot_entities(snapshot: &PortableSnapshot) -> HashMap<String, String> {
        HashMap::from([
            (
                "settings".to_string(),
                serde_json::to_string(&snapshot.settings).expect("settings raw"),
            ),
            (
                "sessions".to_string(),
                serde_json::to_string(&snapshot.sessions).expect("sessions raw"),
            ),
            (
                "keys".to_string(),
                serde_json::to_string(&snapshot.keys).expect("keys raw"),
            ),
            (
                "passwords".to_string(),
                serde_json::to_string(&snapshot.passwords).expect("passwords raw"),
            ),
            (
                "credentials".to_string(),
                serde_json::to_string(&snapshot.credentials).expect("credentials raw"),
            ),
            (
                "otp".to_string(),
                serde_json::to_string(&snapshot.otp).expect("otp raw"),
            ),
            (
                "proxies".to_string(),
                serde_json::to_string(&snapshot.proxies).expect("proxies raw"),
            ),
            (
                "proxy_groups".to_string(),
                serde_json::to_string(&snapshot.proxy_groups).expect("proxy groups raw"),
            ),
            (
                "tunnels".to_string(),
                serde_json::to_string(&snapshot.tunnels).expect("tunnels raw"),
            ),
            (
                "tunnel_groups".to_string(),
                serde_json::to_string(&snapshot.tunnel_groups).expect("tunnel groups raw"),
            ),
            (
                "quick_commands".to_string(),
                serde_json::to_string(&snapshot.quick_commands).expect("quick commands raw"),
            ),
            (
                "history".to_string(),
                serde_json::to_string(&snapshot.history).expect("history raw"),
            ),
            (
                "master_key_token".to_string(),
                serde_json::to_string(&snapshot.master_key_token).expect("master key raw"),
            ),
            (
                "known_hosts".to_string(),
                serde_json::to_string(&snapshot.known_hosts).expect("known hosts raw"),
            ),
            (
                "notes".to_string(),
                serde_json::to_string(&snapshot.notes).expect("notes raw"),
            ),
        ])
    }

    fn v121_quick_command_entities(
        snapshot: &PortableSnapshot,
    ) -> std::collections::BTreeMap<String, String> {
        let mut quick_commands =
            serde_json::to_value(&snapshot.quick_commands).expect("quick commands json");
        quick_commands["commands"][0]
            .as_object_mut()
            .expect("quick command object")
            .remove("sort_order");

        let mut entities: std::collections::BTreeMap<_, _> =
            snapshot_entities(snapshot).into_iter().collect();
        entities.insert(
            "quick_commands".to_string(),
            serde_json::to_string(&quick_commands).expect("quick commands raw"),
        );
        entities
    }

    async fn write_committed_snapshot(
        remote: &CloudRemote,
        revision_id: &str,
    ) -> RemoteSyncPointer {
        let snapshot = sample_snapshot(revision_id, 1);
        let pointer = pointer_from_snapshot(&snapshot);
        upload_sync_snapshot(remote, "nyaterm", &snapshot)
            .await
            .expect("upload snapshot");
        commit_sync_pointer(remote, "nyaterm", &pointer)
            .await
            .expect("commit pointer");
        pointer
    }

    async fn write_raw_snapshot_file(
        remote: &CloudRemote,
        remote_root: &str,
        revision_id: &str,
        raw: Vec<u8>,
    ) {
        remote
            .write(&sync_snapshot_path(remote_root, revision_id), raw)
            .await
            .expect("write raw snapshot");
    }

    async fn write_encrypted_snapshot_bytes(
        remote: &CloudRemote,
        remote_root: &str,
        revision_id: &str,
        encoded_snapshot: Vec<u8>,
    ) {
        remote
            .write(
                &sync_snapshot_path(remote_root, revision_id),
                encrypt_snapshot_bytes(&encoded_snapshot).expect("encrypt snapshot"),
            )
            .await
            .expect("write encrypted snapshot");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn snapshot_upload_failure_leaves_latest_unchanged() {
        let _guard = MASTER_PASSWORD_TEST_LOCK.lock().expect("lock password");
        set_master_password(Some("secret".to_string()));
        let (memory, remote) = memory_remote();
        let old_pointer = write_committed_snapshot(&remote, "r1").await;
        memory.fail_next_write_containing("snapshots/r2");

        let new_snapshot = sample_snapshot("r2", 2);
        let result = upload_sync_snapshot(&remote, "nyaterm", &new_snapshot).await;

        assert!(result.is_err());
        let latest = load_sync_pointer(&remote, "nyaterm")
            .await
            .expect("load latest")
            .expect("latest");
        assert_eq!(latest.revision_id, old_pointer.revision_id);
        set_master_password(None);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn pointer_write_failure_keeps_old_revision_readable() {
        let _guard = MASTER_PASSWORD_TEST_LOCK.lock().expect("lock password");
        set_master_password(Some("secret".to_string()));
        let (memory, remote) = memory_remote();
        let old_pointer = write_committed_snapshot(&remote, "r1").await;
        let new_snapshot = sample_snapshot("r2", 2);
        let new_pointer = pointer_from_snapshot(&new_snapshot);

        upload_sync_snapshot(&remote, "nyaterm", &new_snapshot)
            .await
            .expect("upload new snapshot");
        memory.fail_next_write_containing("latest.redb");
        let result = commit_sync_pointer(&remote, "nyaterm", &new_pointer).await;

        assert!(result.is_err());
        let old_snapshot = read_snapshot_for_pointer(&remote, "nyaterm", &old_pointer)
            .await
            .expect("old snapshot readable");
        assert_eq!(old_snapshot.revision_id, "r1");
        set_master_password(None);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn current_pointer_mismatch_returns_inconsistent_resolution() {
        let _guard = MASTER_PASSWORD_TEST_LOCK.lock().expect("lock password");
        set_master_password(Some("secret".to_string()));
        let (_memory, remote) = memory_remote();
        let pointer = pointer_from_snapshot(&sample_snapshot("r1", 1));
        commit_sync_pointer(&remote, "nyaterm", &pointer)
            .await
            .expect("commit pointer");
        write_current_sync_snapshot_compat(&remote, "nyaterm", &sample_snapshot("r2", 2))
            .await
            .expect("write current");

        let resolution = resolve_remote_snapshot(&remote, "nyaterm", &pointer)
            .await
            .expect("resolve remote");

        match resolution {
            RemoteSnapshotResolution::Inconsistent {
                pointer,
                recovery_candidate,
            } => {
                assert_eq!(pointer.revision_id, "r1");
                assert_eq!(recovery_candidate.revision_id, "r2");
            }
            _ => panic!("expected inconsistent remote"),
        }
        set_master_password(None);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn corrupt_pointer_snapshot_without_current_returns_corrupted_snapshot() {
        let _guard = MASTER_PASSWORD_TEST_LOCK.lock().expect("lock password");
        set_master_password(Some("secret".to_string()));
        let (_memory, remote) = memory_remote();
        let pointer = pointer_from_snapshot(&sample_snapshot("r1", 1));
        commit_sync_pointer(&remote, "nyaterm", &pointer)
            .await
            .expect("commit pointer");
        write_raw_snapshot_file(&remote, "nyaterm", "r1", b"not a snapshot".to_vec()).await;

        let result = resolve_remote_snapshot(&remote, "nyaterm", &pointer).await;

        assert!(matches!(
            result,
            Err(AppError::CloudSync(CloudSyncError::CorruptedSnapshot { revision }))
                if revision == "r1"
        ));
        set_master_password(None);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn corrupt_pointer_snapshot_with_matching_current_is_migrated() {
        let _guard = MASTER_PASSWORD_TEST_LOCK.lock().expect("lock password");
        set_master_password(Some("secret".to_string()));
        let (memory, remote) = memory_remote();
        let snapshot = sample_snapshot("r1", 1);
        let pointer = pointer_from_snapshot(&snapshot);
        commit_sync_pointer(&remote, "nyaterm", &pointer)
            .await
            .expect("commit pointer");
        write_raw_snapshot_file(&remote, "nyaterm", "r1", b"not a snapshot".to_vec()).await;
        write_current_sync_snapshot_compat(&remote, "nyaterm", &snapshot)
            .await
            .expect("write current");

        let resolution = resolve_remote_snapshot(&remote, "nyaterm", &pointer)
            .await
            .expect("resolve remote");

        assert!(matches!(
            resolution,
            RemoteSnapshotResolution::LegacyMigrated(_)
        ));
        let repaired = read_snapshot_for_pointer(&remote, "nyaterm", &pointer)
            .await
            .expect("read repaired snapshot");
        assert_eq!(repaired.payload_hash, pointer.payload_hash);
        assert!(
            memory
                .file(&remote_path("nyaterm", &sync_snapshot_file("r1")))
                .is_some()
        );
        set_master_password(None);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn hash_mismatch_pointer_snapshot_with_different_current_is_inconsistent() {
        let _guard = MASTER_PASSWORD_TEST_LOCK.lock().expect("lock password");
        set_master_password(Some("secret".to_string()));
        let (_memory, remote) = memory_remote();
        let pointer = pointer_from_snapshot(&sample_snapshot("r1", 1));
        commit_sync_pointer(&remote, "nyaterm", &pointer)
            .await
            .expect("commit pointer");
        upload_sync_snapshot(
            &remote,
            "nyaterm",
            &sample_snapshot_with_known_hosts("r1", 2, "changed"),
        )
        .await
        .expect("upload mismatched snapshot");
        write_current_sync_snapshot_compat(
            &remote,
            "nyaterm",
            &sample_snapshot_with_known_hosts("r2", 3, "recovery"),
        )
        .await
        .expect("write current");

        let resolution = resolve_remote_snapshot(&remote, "nyaterm", &pointer)
            .await
            .expect("resolve remote");

        match resolution {
            RemoteSnapshotResolution::Inconsistent {
                pointer,
                recovery_candidate,
            } => {
                assert_eq!(pointer.revision_id, "r1");
                assert_eq!(recovery_candidate.revision_id, "r2");
            }
            _ => panic!("expected inconsistent remote"),
        }
        set_master_password(None);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn pointer_accepts_v121_quick_command_source_payload_hash() {
        let _guard = MASTER_PASSWORD_TEST_LOCK.lock().expect("lock password");
        set_master_password(Some("secret".to_string()));
        let (_memory, remote) = memory_remote();
        let snapshot = sample_snapshot_with_quick_command("r1", 1);
        let entities = v121_quick_command_entities(&snapshot);
        let source_hash = calculate_v3_raw_payload_hash(&entities).expect("source hash");
        assert_ne!(source_hash, snapshot.payload_hash);
        let encoded =
            encode_v3_raw_snapshot_redb_for_test(&snapshot, &entities, source_hash.clone());
        let mut pointer = pointer_from_snapshot(&snapshot);
        pointer.payload_hash = source_hash.clone();
        commit_sync_pointer(&remote, "nyaterm", &pointer)
            .await
            .expect("commit pointer");
        write_encrypted_snapshot_bytes(&remote, "nyaterm", "r1", encoded).await;

        let decoded = read_snapshot_for_pointer(&remote, "nyaterm", &pointer)
            .await
            .expect("read v1.2.1 snapshot");

        assert_eq!(decoded.revision_id, "r1");
        assert_eq!(decoded.payload_hash, snapshot.payload_hash);
        assert_eq!(decoded.quick_commands.commands[0].sort_order, None);
        set_master_password(None);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn pointer_rejects_wrong_hash_for_v121_quick_command_snapshot() {
        let _guard = MASTER_PASSWORD_TEST_LOCK.lock().expect("lock password");
        set_master_password(Some("secret".to_string()));
        let (_memory, remote) = memory_remote();
        let snapshot = sample_snapshot_with_quick_command("r1", 1);
        let entities = v121_quick_command_entities(&snapshot);
        let source_hash = calculate_v3_raw_payload_hash(&entities).expect("source hash");
        let encoded = encode_v3_raw_snapshot_redb_for_test(&snapshot, &entities, source_hash);
        let mut pointer = pointer_from_snapshot(&snapshot);
        pointer.payload_hash = "wrong".to_string();
        commit_sync_pointer(&remote, "nyaterm", &pointer)
            .await
            .expect("commit pointer");
        write_encrypted_snapshot_bytes(&remote, "nyaterm", "r1", encoded).await;

        let result = read_snapshot_for_pointer(&remote, "nyaterm", &pointer).await;

        assert!(matches!(
            result,
            Err(AppError::CloudSync(CloudSyncError::HashMismatch { .. }))
        ));
        set_master_password(None);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn new_protocol_reads_latest_snapshot() {
        let _guard = MASTER_PASSWORD_TEST_LOCK.lock().expect("lock password");
        set_master_password(Some("secret".to_string()));
        let (_memory, remote) = memory_remote();
        let pointer = write_committed_snapshot(&remote, "r2").await;

        let snapshot = read_snapshot_for_pointer(&remote, "nyaterm", &pointer)
            .await
            .expect("read snapshot");

        assert_eq!(snapshot.revision_id, "r2");
        set_master_password(None);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn legacy_current_is_migrated_to_snapshot_path() {
        let _guard = MASTER_PASSWORD_TEST_LOCK.lock().expect("lock password");
        set_master_password(Some("secret".to_string()));
        let (memory, remote) = memory_remote();
        let snapshot = sample_snapshot("r1", 1);
        let pointer = pointer_from_snapshot(&snapshot);
        commit_sync_pointer(&remote, "nyaterm", &pointer)
            .await
            .expect("commit pointer");
        write_current_sync_snapshot_compat(&remote, "nyaterm", &snapshot)
            .await
            .expect("write current");

        let resolution = resolve_remote_snapshot(&remote, "nyaterm", &pointer)
            .await
            .expect("resolve legacy");

        assert!(matches!(
            resolution,
            RemoteSnapshotResolution::LegacyMigrated(_)
        ));
        assert!(
            memory
                .file(&remote_path("nyaterm", &sync_snapshot_file("r1")))
                .is_some()
        );
        set_master_password(None);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn concurrent_update_is_detected_before_pointer_commit() {
        let _guard = MASTER_PASSWORD_TEST_LOCK.lock().expect("lock password");
        set_master_password(Some("secret".to_string()));
        let (_memory, remote) = memory_remote();
        let base_pointer = write_committed_snapshot(&remote, "r1").await;
        let next_pointer = pointer_from_snapshot(&sample_snapshot("r2", 2));
        commit_sync_pointer(&remote, "nyaterm", &next_pointer)
            .await
            .expect("commit competing pointer");

        let result = ensure_remote_head_unchanged(&remote, "nyaterm", Some(&base_pointer)).await;

        assert!(matches!(
            result,
            Err(AppError::CloudSync(CloudSyncError::ConcurrentUpdate { .. }))
        ));
        let latest = load_sync_pointer(&remote, "nyaterm")
            .await
            .expect("load latest")
            .expect("latest");
        assert_eq!(latest.revision_id, "r2");
        set_master_password(None);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn pointer_snapshot_hash_mismatch_is_rejected() {
        let _guard = MASTER_PASSWORD_TEST_LOCK.lock().expect("lock password");
        set_master_password(Some("secret".to_string()));
        let (_memory, remote) = memory_remote();
        let snapshot = sample_snapshot("r1", 1);
        upload_sync_snapshot(&remote, "nyaterm", &snapshot)
            .await
            .expect("upload snapshot");
        let mut pointer = pointer_from_snapshot(&snapshot);
        pointer.payload_hash = "wrong".to_string();

        let result = read_snapshot_for_pointer(&remote, "nyaterm", &pointer).await;

        assert!(matches!(
            result,
            Err(AppError::CloudSync(CloudSyncError::HashMismatch { .. }))
        ));
        set_master_password(None);
    }
}
