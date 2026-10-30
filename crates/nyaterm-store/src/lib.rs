//! Persistent-store boundary and single-owner runtime.

mod portable_codec;
mod portable_decode_helper;
pub use portable_decode_helper::run_cloud_snapshot_decode_helper_if_requested;
pub mod plugin_preferences;
mod runtime;
mod storage;
pub use storage::runtime_auth;

pub use runtime::{
    BootstrapSnapshot, FlushBarrier, LoadBootstrap, LoadDeviceWindowManifest, LoadMainWindowState,
    LoadWorkspaceRestoreManifest, RequestId, SaveDeviceWindowManifest, SaveMainWindowState,
    SaveWorkspaceRestoreManifest, StoreBlockingClient, StoreClientError, StoreConfig, StoreDomain,
    StoreEvent, StoreFnRequest, StoreMutationEvent, StoreOperationError, StoreRequest,
    StoreRuntime, StoreSubmitError, StoreTask, StoreUiClient, store_mutation, store_request,
};

pub use storage::{
    ConfigBackupInfo, ConnectionStore, KnownHostCheck, KnownHostEntry, RdpCertificateMetadata,
    RdpKnownHostCheck, RemoteFileBackendCache, RemoteFileBackendCacheEntry, StorageError,
    ThemeCommitOutcome,
};

pub use nyaterm_core::{
    DiagnosticsError, DiagnosticsExportInfo, DiagnosticsExportOptions, DiagnosticsRuntimeSnapshot,
    PortableSnapshotError, PortableSnapshotKind, PortableSnapshotMeta, RawPortableSnapshot,
    export_diagnostics_archive,
};
pub use portable_codec::{
    decode_encrypted_raw_portable_snapshot, decode_raw_portable_snapshot,
    encode_encrypted_raw_portable_snapshot, encode_raw_portable_snapshot,
};

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use redb::{Database, TableDefinition};
    use sha2::{Digest, Sha256};

    #[test]
    fn tauri_v2_redb_snapshot_converts_without_losing_source_hash() {
        const META: TableDefinition<&str, &str> = TableDefinition::new("snapshot_meta");
        const JSON: TableDefinition<&str, &str> = TableDefinition::new("json_docs");
        const TEXT: TableDefinition<&str, &str> = TableDefinition::new("text_docs");

        let mut json_docs = BTreeMap::new();
        json_docs.insert(
            "portable-settings".to_string(),
            r#"{"general":{},"security":{},"ui":{"language":"en-US"}}"#.to_string(),
        );
        json_docs.insert("sessions".to_string(), r#"{"connections":[]}"#.to_string());
        json_docs.insert("future-doc".to_string(), r#"{"future":true}"#.to_string());
        let mut text_docs = BTreeMap::new();
        text_docs.insert("known_hosts".to_string(), "example host key".to_string());
        let hash_input = serde_json::json!({"json_docs":json_docs,"text_docs":text_docs});
        let source_hash = Sha256::digest(serde_json::to_vec(&hash_input).unwrap())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let meta = serde_json::json!({
            "schema_version":2,"snapshot_kind":"sync","revision_id":"v2-revision",
            "device_id":"tauri-device","created_at_ms":42,"payload_hash":source_hash,
            "app_version":"1.9.0"
        });
        let path =
            std::env::temp_dir().join(format!("nyaterm-v2-test-{}.redb", uuid::Uuid::new_v4()));
        {
            let db = Database::create(&path).unwrap();
            let txn = db.begin_write().unwrap();
            txn.open_table(META)
                .unwrap()
                .insert("meta", meta.to_string().as_str())
                .unwrap();
            {
                let mut table = txn.open_table(JSON).unwrap();
                for (key, value) in &json_docs {
                    table.insert(key.as_str(), value.as_str()).unwrap();
                }
            }
            {
                let mut table = txn.open_table(TEXT).unwrap();
                for (key, value) in &text_docs {
                    table.insert(key.as_str(), value.as_str()).unwrap();
                }
            }
            txn.commit().unwrap();
        }
        let bytes = std::fs::read(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        let converted = crate::decode_raw_portable_snapshot(&bytes).unwrap();
        assert_eq!(converted.source_payload_hash(), source_hash);
        assert_ne!(converted.meta.payload_hash, source_hash);
        assert_eq!(converted.meta.schema_version, 3);
        assert!(converted.entities["v2_unknown_json_docs"].contains("future-doc"));
        assert_eq!(converted.entities["history"], "[]");
    }
    use super::{
        PortableSnapshotKind, RawPortableSnapshot, decode_encrypted_raw_portable_snapshot,
        decode_raw_portable_snapshot, encode_encrypted_raw_portable_snapshot,
        encode_raw_portable_snapshot,
    };

    #[test]
    fn raw_portable_snapshot_round_trips_through_store_boundary() {
        let mut snapshot = RawPortableSnapshot::backup("test-device", "test-version");
        snapshot
            .entities
            .insert("settings/default".into(), r#"{"theme":"dark"}"#.into());
        snapshot.recalculate_hash().expect("hash snapshot");

        let encoded = encode_raw_portable_snapshot(&snapshot).expect("encode snapshot");
        assert!(encoded.starts_with(b"PK\x03\x04"));
        let decoded = decode_raw_portable_snapshot(&encoded).expect("decode snapshot");

        assert_eq!(decoded.meta.snapshot_kind, PortableSnapshotKind::Backup);
        assert_eq!(decoded.meta.device_id, "test-device");
        assert_eq!(decoded.meta.entities_hash, snapshot.meta.entities_hash);
        assert_eq!(decoded.entities, snapshot.entities);
    }

    #[test]
    fn encrypted_portable_snapshot_preserves_entity_map_hash() {
        let mut snapshot = RawPortableSnapshot::backup("test-device", "test-version");
        snapshot.entities.insert(
            "future_entity".into(),
            r#"{"schema":7,"future":true}"#.into(),
        );
        snapshot.recalculate_hash().expect("hash snapshot");

        let encoded = encode_encrypted_raw_portable_snapshot(&snapshot, "test-password")
            .expect("encrypt snapshot");
        let decoded = decode_encrypted_raw_portable_snapshot(&encoded, "test-password")
            .expect("decrypt snapshot");

        assert_eq!(decoded.meta.entities_hash, snapshot.meta.entities_hash);
        assert_eq!(decoded.entities, snapshot.entities);
    }

    #[test]
    fn sync_pointer_round_trips_through_legacy_redb_document() {
        use redb::ReadableDatabase;
        let pointer = nyaterm_core::RemoteSyncPointer {
            schema_version: 2,
            revision_id: "revision-1".to_string(),
            created_at_ms: 123,
            payload_hash: "hash".to_string(),
            device_id: "device-1".to_string(),
            app_version: "2.0.0".to_string(),
            sync_clock: None,
        };

        let encoded = crate::portable_codec::encode_sync_pointer(&pointer).expect("encode pointer");
        let decoded = crate::portable_codec::decode_sync_pointer(&encoded).expect("decode pointer");

        assert_eq!(decoded, pointer);

        // Both directions must remain interoperable with the historical
        // file-backed codec, not merely roundtrip through the new backend.
        const POINTER: TableDefinition<&str, &str> = TableDefinition::new("sync_pointer");
        let dir = nyaterm_core::test_support::TestTempDir::new("nyaterm-pointer-memory-compat");
        std::fs::create_dir_all(dir.path()).unwrap();
        let path = dir.join("pointer.redb");
        std::fs::write(&path, &encoded).unwrap();
        {
            let database = Database::open(&path).unwrap();
            let transaction = database.begin_read().unwrap();
            let table = transaction.open_table(POINTER).unwrap();
            let value = table.get("latest").unwrap().unwrap();
            assert_eq!(
                serde_json::from_str::<nyaterm_core::RemoteSyncPointer>(value.value()).unwrap(),
                pointer
            );
        }
        let legacy_path = dir.join("legacy.redb");
        {
            let database = Database::create(&legacy_path).unwrap();
            let transaction = database.begin_write().unwrap();
            transaction
                .open_table(POINTER)
                .unwrap()
                .insert("latest", serde_json::to_string(&pointer).unwrap().as_str())
                .unwrap();
            transaction.commit().unwrap();
        }
        assert_eq!(
            crate::portable_codec::decode_sync_pointer(&std::fs::read(legacy_path).unwrap())
                .unwrap(),
            pointer
        );
    }
}
pub mod sync_v4;
