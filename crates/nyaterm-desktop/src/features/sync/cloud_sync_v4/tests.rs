use std::cell::Cell;

use nyaterm_core::cloud_sync::control::CloudSyncOperationControl;
use nyaterm_core::{
    CloudSyncError, CloudSyncRemote, CloudSyncSettings, LocalCloudSyncOptions,
    LocalDirectoryRemote, SavedConnection,
};
use nyaterm_store::{StoreBlockingClient, StoreDomain};
use nyaterm_sync::protocol::PendingPublication;
use serde_json::json;
use uuid::Uuid;

use crate::features::sync::cloud_sync_v4::{
    apply_and_publish, destination, prepare_remote, preview, preview_manual, publish,
    publish_bytes, store_error,
};
use crate::test_support::{TestConfigDir, blocking_test_store};

#[test]
fn local_replica_size_errors_keep_their_typed_limit_instead_of_becoming_stale_previews() {
    let operation =
        nyaterm_store::StoreOperationError::from(nyaterm_store::StorageError::PortableSnapshot(
            nyaterm_core::PortableSnapshotError::LocalReplicaPayloadTooLarge { limit_bytes: 42 },
        ));
    assert!(matches!(
        store_error(nyaterm_store::StoreClientError::Operation(operation)),
        CloudSyncError::PortableSnapshot(
            nyaterm_core::PortableSnapshotError::LocalReplicaPayloadTooLarge { limit_bytes: 42 }
        )
    ));
    let operation = nyaterm_store::StoreOperationError::from(
        nyaterm_store::StorageError::CloudSyncLocalChanged,
    );
    assert!(matches!(
        store_error(nyaterm_store::StoreClientError::Operation(operation)),
        CloudSyncError::LocalChangedDuringSync
    ));
}

fn setup(
    dir: &TestConfigDir,
    remote: &TestConfigDir,
) -> (
    StoreBlockingClient,
    CloudSyncSettings,
    LocalCloudSyncOptions,
) {
    let store = blocking_test_store(dir.path());
    let settings = CloudSyncSettings {
        provider: "local_directory".into(),
        protocol_version: 4,
        enabled: true,
        ..Default::default()
    };
    let saved = settings.clone();
    store
        .request_fn(StoreDomain::CloudSync, move |store| {
            store.save_cloud_sync_settings(saved)
        })
        .unwrap();
    let options = LocalCloudSyncOptions {
        config_dir: dir.path().into(),
        portable_key_path: None,
        remote_dir: remote.path().into(),
        remote_root: "space".into(),
        device_id: Uuid::new_v4().to_string(),
        device_name: None,
        app_version: "test".into(),
        master_password: "password".into(),
        enabled: true,
    };
    (store, settings, options)
}

fn save(store: &StoreBlockingClient, id: &str, name: &str) {
    let connection: SavedConnection = serde_json::from_value(
        json!({"id":id,"type":"ssh","name":name,"host":"example","port":22,"username":"u"}),
    )
    .unwrap();
    store
        .request_fn(StoreDomain::CloudSync, move |store| {
            store.save_connection(&connection)
        })
        .unwrap();
}

#[test]
fn provider_edits_and_loaded_changes_invalidate_old_v4_previews() {
    use crate::features::sync::CloudSyncFeatureState;
    let dir = TestConfigDir::new("nyaterm-provider-preview-state");
    let remote = TestConfigDir::new("nyaterm-provider-preview-remote");
    let (store, settings, options) = setup(&dir, &remote);
    let mut state = CloudSyncFeatureState::new(settings.clone(), Default::default(), Vec::new());
    for loaded in [false, true] {
        state.replace_loaded(settings.clone(), Default::default());
        state.set_v4_preview(preview(&store, &settings, &options, &Default::default()).unwrap());
        assert!(state.v4_summary().is_some());
        let conflict = serde_json::from_value(json!({
            "detected_at_ms":1, "provider":"local_directory",
            "local_payload_hash":"local", "remote_payload_hash":"remote",
            "local_content_hash":"content", "remote_revision":"previous-provider",
            "remote_created_at_ms":1, "remote_device_id":"other", "message":"conflict"
        }))
        .unwrap();
        state.capture_conflict(
            &CloudSyncError::Conflict(Box::new(conflict)),
            settings.provider.clone(),
            true,
        );
        assert!(state.prepare_resolution(false));
        assert!(state.conflict().is_some());
        if loaded {
            let mut next = settings.clone();
            next.provider = "github_gist".into();
            state.replace_loaded(next, Default::default());
        } else {
            state.select_provider("github_gist");
        }
        assert!(state.v4_summary().is_none());
        assert!(state.conflict().is_none());
        assert!(state.take_resolution().is_none());
        assert_eq!(state.settings().protocol_version, 4);
    }
}

#[test]
fn webdav_gist_webdav_round_trip_restores_manual_and_automatic_v4_without_old_v3_baselines() {
    use crate::features::sync::operation::AutoSyncOperationControls;
    use crate::features::sync::{AutoSyncRequest, AutoSyncResult, AutoSyncTrigger, run_auto_sync};
    use nyaterm_core::{AppRuntime, CloudSyncState, RuntimeMode};
    use std::time::{Duration, Instant};
    let dir = TestConfigDir::new("nyaterm-provider-roundtrip");
    let remote_dir = TestConfigDir::new("nyaterm-provider-roundtrip-unused");
    let server = crate::http::cloud_sync::CloudStorageFixture::storage();
    let (store, mut settings, mut options) = setup(&dir, &remote_dir);
    settings.provider = "webdav".into();
    settings.remote_root = options.remote_root.clone();
    settings.webdav = server.webdav();
    settings.auto_pull_remote_changes = true;
    settings.auto_push_on_change = true;
    let saved = settings.clone();
    let device = store
        .request_fn(StoreDomain::CloudSync, move |s| {
            s.save_master_password(Some("password"))?;
            s.save_cloud_sync_settings(saved)?;
            Ok(s.load_cloud_sync_state()?.device_id)
        })
        .unwrap();
    options.device_id = device.clone();
    let control = CloudSyncOperationControl::default();
    let initial = preview(&store, &settings, &options, &control).unwrap();
    apply_and_publish(&store, &settings, &options, initial, &control).unwrap();
    let destination = destination(&settings, &options);
    let record_key = destination.clone();
    let before = store
        .request_fn(StoreDomain::CloudSync, move |s| {
            Ok(
                s.load_v4_view(&record_key, "password", &Default::default())?
                    .record_hash,
            )
        })
        .unwrap();
    let runtime = AppRuntime::from_parts_for_test(
        RuntimeMode::Portable,
        dir.path().into(),
        dir.path().into(),
        dir.join("logs"),
        dir.join("cache"),
        None,
    );
    let automatic = || {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let result = run_auto_sync(
                &store,
                &runtime,
                AutoSyncRequest {
                    trigger: AutoSyncTrigger::Settings,
                    change_elapsed: Duration::from_secs(61),
                    pull_safe: true,
                    remote_available: false,
                },
                &|| Ok(()),
                &AutoSyncOperationControls::default(),
            );
            if !matches!(result, Ok(AutoSyncResult::Busy)) {
                break result;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
    };
    settings.provider = "github_gist".into();
    let saved = settings.clone();
    store
        .request_fn(StoreDomain::CloudSync, move |s| {
            s.save_cloud_sync_state(&CloudSyncState {
                device_id: device,
                last_synced_payload_hash: Some("previous".into()),
                ..Default::default()
            })?;
            s.save_cloud_sync_settings(saved)?;
            assert!(
                s.load_cloud_sync_state()?
                    .last_synced_payload_hash
                    .is_none()
            );
            assert_eq!(s.load_cloud_sync_settings()?.protocol_version, 4);
            Ok(())
        })
        .unwrap();
    let manual = crate::features::sync::cloud_sync_provider::push_provider_snapshot(
        &store,
        &settings,
        &options,
        &Default::default(),
        false,
        &control,
    );
    for result in [manual.map(|_| ()), automatic().map(|_| ())] {
        assert!(
            matches!(result, Err(CloudSyncError::Remote(message)) if message == "GitHub Gist access token is required")
        );
    }
    settings.provider = "webdav".into();
    let saved = settings.clone();
    let record_key = destination;
    store
        .request_fn(StoreDomain::CloudSync, move |s| {
            s.save_cloud_sync_settings(saved)?;
            assert_eq!(
                s.load_v4_view(&record_key, "password", &Default::default())?
                    .record_hash,
                before
            );
            Ok(())
        })
        .unwrap();
    let resumed = preview(&store, &settings, &options, &control).unwrap();
    assert!(!resumed.migration);
    apply_and_publish(&store, &settings, &options, resumed, &control).unwrap();
    assert!(matches!(automatic().unwrap(), AutoSyncResult::Skipped));
    save(&store, "post-switch", "changed after returning to WebDAV");
    assert!(matches!(automatic().unwrap(), AutoSyncResult::V4(_)));
}

#[test]
fn old_optional_switches_cannot_upload_or_apply_extra_data() {
    let a = TestConfigDir::new("nyaterm-fixed-provider-a");
    let b = TestConfigDir::new("nyaterm-fixed-provider-b");
    let dir = TestConfigDir::new("nyaterm-fixed-provider-remote");
    let (sa, mut settings, oa) = setup(&a, &dir);
    let (sb, _, ob) = setup(&b, &dir);
    settings.v4_scope = nyaterm_core::sync_schema::SyncScope {
        secrets: true,
        preferences: true,
        ai: true,
        keybindings: true,
        trusted_hosts: true,
    };
    for store in [&sa, &sb] {
        let saved = settings.clone();
        store
            .request_fn(StoreDomain::CloudSync, move |store| {
                store.save_master_password(Some("password"))?;
                store.save_cloud_sync_settings(saved)
            })
            .unwrap();
    }
    sa.request_fn(StoreDomain::CloudSync, |store| {
        store.save_password(serde_json::from_value(json!({"id":"source-account","name":"Source account","password":"fixture-excluded-password"}))?)?;
        let mut ai = store.load_ai_settings()?;
        ai.provider_credentials.push(serde_json::from_value(json!({"id":"source-ai","name":"Source AI","provider_kind":"openai","base_url":"https://source.test/v1/","api_key":"fixture-excluded-ai-key"}))?);
        store.save_ai_settings(ai)?;
        store.remember_vnc_known_host("source.test", 5900, &format!("SHA256:{}", "ab".repeat(32)), None)?;
        Ok(())
    }).unwrap();
    save(&sa, "server", "Portable connection");
    let control = CloudSyncOperationControl::default();
    let outgoing = preview(&sa, &settings, &oa, &control).unwrap();
    apply_and_publish(&sa, &settings, &oa, outgoing, &control).unwrap();
    let remote = LocalDirectoryRemote::new(dir.path());
    let entries = remote
        .list_sync_files("space/sync/v5/registry")
        .unwrap()
        .into_iter()
        .flat_map(|registration| {
            let writer = registration
                .rsplit('/')
                .next()
                .unwrap()
                .strip_suffix(".bin")
                .unwrap();
            remote
                .list_sync_files(&format!("space/sync/v5/writers/{writer}"))
                .unwrap()
        })
        .collect::<Vec<_>>();
    assert!(!entries.is_empty());
    for entry in entries {
        let relative = entry.strip_prefix("space/").unwrap();
        let id = nyaterm_sync::protocol::PublicationId::parse(relative).unwrap();
        let bytes = remote.read_if_exists(&entry).unwrap().unwrap();
        let plaintext = nyaterm_core::sync_schema::encryption::open(
            "password",
            "publication",
            &format!("{}/{}", id.writer, id.sequence),
            &bytes,
        )
        .unwrap();
        let publication: nyaterm_sync::protocol::PublishedReplica =
            serde_json::from_slice(&plaintext).unwrap();
        publication.validate_fixed().unwrap();
        for marker in [
            "fixture-excluded-password",
            "fixture-excluded-ai-key",
            "Source AI",
        ] {
            assert!(!String::from_utf8_lossy(&plaintext).contains(marker));
            assert!(
                publication.replica.values.values().all(|value| !value
                    .decode()
                    .unwrap()
                    .to_string()
                    .contains(marker))
            );
        }
    }
    let incoming = preview(&sb, &settings, &ob, &control).unwrap();
    apply_and_publish(&sb, &settings, &ob, incoming, &control).unwrap();
    sb.request_fn(StoreDomain::CloudSync, |store| {
        assert_eq!(store.list_connections()?[0].name, "Portable connection");
        assert!(
            store
                .load_decrypted_password_by_id("source-account")?
                .is_none()
        );
        assert!(
            !store
                .load_ai_settings()?
                .provider_credentials
                .iter()
                .any(|provider| provider.id == "source-ai")
        );
        assert!(store.load_vnc_known_host("source.test", 5900)?.is_none());
        Ok(())
    })
    .unwrap();
}

#[test]
fn v4_history_migrates_core_conflicts_without_forwarding_extra_values_or_pending_bytes() {
    use nyaterm_core::sync_schema::{SyncProjection, SyncScope, SyncValues};
    use nyaterm_sync::engine::SyncReplica;
    use nyaterm_sync::protocol::{PendingPublication, PublishedReplica};
    let dir = TestConfigDir::new("nyaterm-fixed-legacy-local");
    let remote_dir = TestConfigDir::new("nyaterm-fixed-legacy-remote");
    let (store, settings, options) = setup(&dir, &remote_dir);
    let scope = SyncScope {
        secrets: true,
        preferences: true,
        ..Default::default()
    };
    let mut projection = SyncProjection::default();
    projection
        .complete
        .extend(["connection".into(), "password".into(), "preference".into()]);
    projection.record("connection", "server", &json!({"name":"Legacy connection","endpoint":{"type":"ssh","host":"server.test","port":22,"username":"user"}})).unwrap();
    projection
        .record(
            "password",
            "legacy-account",
            &json!({"value":"fixture-legacy-password"}),
        )
        .unwrap();
    projection
        .record(
            "preference",
            "appearance/font_family",
            &json!({"value":"fixture-legacy-font"}),
        )
        .unwrap();
    let actor = Uuid::new_v4();
    let mut legacy = SyncReplica::new(actor);
    legacy
        .capture(&SyncValues::new(), &projection, &scope)
        .unwrap();
    let snapshot = legacy.snapshot();
    let publication = PublishedReplica {
        schema: 4,
        writer: actor,
        sequence: 1,
        replica: snapshot.clone(),
        secret_objects: Default::default(),
    };
    let bytes = nyaterm_core::sync_schema::encryption::seal(
        "password",
        "publication",
        &format!("{actor}/1"),
        &serde_json::to_vec(&publication).unwrap(),
    )
    .unwrap();
    let id = PendingPublication::new(
        actor,
        1,
        bytes.clone(),
        nyaterm_sync::engine::digest(&snapshot.document),
    )
    .unwrap()
    .id;
    let remote = LocalDirectoryRemote::new(remote_dir.path());
    remote.create_dir("space/sync/v4/registry").unwrap();
    remote
        .create_dir(&format!("space/sync/v4/writers/{actor}"))
        .unwrap();
    let object = format!("space/sync/v4/writers/{actor}/1-{}.bin", id.hash);
    remote.write_sync_file(&object, &bytes).unwrap();
    remote
        .write_sync_file(
            &format!("space/sync/v4/registry/{actor}.bin"),
            actor.as_bytes(),
        )
        .unwrap();
    // A retained local baseline represents records that were actually applied
    // on this device; an absent record would correctly create a local deletion.
    store.request_fn(StoreDomain::CloudSync, |store| {
        let connection: SavedConnection = serde_json::from_value(json!({
            "id":"server", "name":"Legacy connection", "type":"ssh", "host":"server.test", "port":22, "username":"user"
        }))?;
        store.save_connection(&connection)
    }).unwrap();
    let archived_destination = super::legacy_destination(&settings, &options);
    let archived = nyaterm_store::sync_v4::LocalReplica {
        replica: Some(snapshot),
        effective: projection.values,
        scope,
        pending: Some(
            PendingPublication::new(
                actor,
                2,
                b"retained-old-pending-bytes".to_vec(),
                "retained-document".into(),
            )
            .unwrap(),
        ),
        ..Default::default()
    };
    let old_destination = archived_destination.clone();
    store
        .request_fn(StoreDomain::CloudSync, move |store| {
            store.persist_v4_record(&old_destination, "password", None, &archived)
        })
        .unwrap();
    let control = CloudSyncOperationControl::default();
    let prepared = preview_manual(&store, &settings, &options, &control).unwrap();
    assert!(prepared.migration);
    assert!(prepared.apply.local.pending.is_none());
    apply_and_publish(&store, &settings, &options, prepared, &control).unwrap();
    assert_eq!(remote.read_if_exists(&object).unwrap().unwrap(), bytes);
    assert_eq!(
        remote
            .list_sync_files("space/sync/v5/registry")
            .unwrap()
            .len(),
        1
    );
    for entry in remote.list_sync_files("space/sync/v5/registry").unwrap() {
        let writer = entry
            .rsplit('/')
            .next()
            .unwrap()
            .strip_suffix(".bin")
            .unwrap();
        for object in remote
            .list_sync_files(&format!("space/sync/v5/writers/{writer}"))
            .unwrap()
        {
            let id = nyaterm_sync::protocol::PublicationId::parse(
                object.strip_prefix("space/").unwrap(),
            )
            .unwrap();
            let bytes = remote.read_if_exists(&object).unwrap().unwrap();
            let plaintext = nyaterm_core::sync_schema::encryption::open(
                "password",
                "publication",
                &format!("{}/{}", id.writer, id.sequence),
                &bytes,
            )
            .unwrap();
            let publication: nyaterm_sync::protocol::PublishedReplica =
                serde_json::from_slice(&plaintext).unwrap();
            publication.validate_fixed().unwrap();
            for marker in ["fixture-legacy-password", "fixture-legacy-font"] {
                assert!(
                    publication.replica.values.values().all(|value| !value
                        .decode()
                        .unwrap()
                        .to_string()
                        .contains(marker))
                );
            }
        }
    }
    store
        .request_fn(StoreDomain::CloudSync, move |store| {
            let archived = store
                .load_v4_replica(&archived_destination, "password")?
                .unwrap()
                .0;
            assert_eq!(
                archived.pending.unwrap().bytes,
                b"retained-old-pending-bytes"
            );
            assert!(
                store
                    .load_decrypted_password_by_id("legacy-account")?
                    .is_none()
            );
            assert_eq!(store.list_connections()?[0].name, "Legacy connection");
            Ok(())
        })
        .unwrap();
}

#[test]
fn opendal_services_migrate_native_v3_and_preserve_v4_multi_device_conflicts_and_cancellation() {
    use crate::http::cloud_sync::{
        CloudStorageFixture, NativeS3Remote, NativeWebdavRemote, OpenDalRemote,
    };
    for webdav in [true, false] {
        let server = CloudStorageFixture::storage();
        let a = TestConfigDir::new("nyaterm-opendal-v4-a");
        let b = TestConfigDir::new("nyaterm-opendal-v4-b");
        let remote_dir = TestConfigDir::new("nyaterm-opendal-v4-unused");
        let (sa, mut settings, oa) = setup(&a, &remote_dir);
        let (sb, _, ob) = setup(&b, &remote_dir);
        settings.provider = if webdav { "webdav" } else { "s3" }.into();
        settings.remote_root = "space".into();
        settings.webdav = server.webdav();
        settings.s3 = server.s3();
        for store in [&sa, &sb] {
            let saved = settings.clone();
            store
                .request_fn(StoreDomain::CloudSync, move |store| {
                    store.save_cloud_sync_settings(saved)
                })
                .unwrap();
        }
        save(&sa, "server", "Legacy saved");
        let native: Box<dyn CloudSyncRemote> = if webdav {
            Box::new(NativeWebdavRemote::new(&settings.webdav).unwrap())
        } else {
            Box::new(NativeS3Remote::new(&settings.s3).unwrap())
        };
        let legacy = nyaterm_core::cloud_sync::push_snapshot_with_remote(
            &sa,
            &oa,
            native.as_ref(),
            &Default::default(),
            true,
        )
        .unwrap();
        let legacy_path = format!(
            "space/{}",
            nyaterm_core::cloud_sync::legacy_sync_snapshot_file(
                &legacy.pointer.unwrap().revision_id
            )
        );
        let old_bytes = native.read_if_exists(&legacy_path).unwrap().unwrap();
        let initial = preview(&sa, &settings, &oa, &CloudSyncOperationControl::default()).unwrap();
        assert!(initial.migration);
        assert!(
            apply_and_publish(
                &sa,
                &settings,
                &oa,
                initial,
                &CloudSyncOperationControl::default()
            )
            .unwrap()
            .verified
        );
        let joined = preview(&sb, &settings, &ob, &CloudSyncOperationControl::default()).unwrap();
        assert!(
            apply_and_publish(
                &sb,
                &settings,
                &ob,
                joined,
                &CloudSyncOperationControl::default()
            )
            .unwrap()
            .verified
        );
        save(&sa, "server", "A offline edit");
        save(&sb, "server", "B offline edit");
        let pa = preview(&sa, &settings, &oa, &CloudSyncOperationControl::default()).unwrap();
        let pb = preview(&sb, &settings, &ob, &CloudSyncOperationControl::default()).unwrap();
        apply_and_publish(
            &sa,
            &settings,
            &oa,
            pa,
            &CloudSyncOperationControl::default(),
        )
        .unwrap();
        assert!(matches!(
            apply_and_publish(
                &sb,
                &settings,
                &ob,
                pb,
                &CloudSyncOperationControl::default()
            ),
            Err(CloudSyncError::LocalChangedDuringSync)
        ));
        let conflicted =
            preview(&sb, &settings, &ob, &CloudSyncOperationControl::default()).unwrap();
        assert!(!conflicted.summary().conflicts.is_empty());
        let cancelled = CloudSyncOperationControl::default();
        cancelled.request_cancel();
        assert!(matches!(
            preview(&sb, &settings, &ob, &cancelled),
            Err(CloudSyncError::Cancelled)
        ));
        assert_eq!(
            native.read_if_exists(&legacy_path).unwrap().unwrap(),
            old_bytes
        );
        let adapter = if webdav {
            OpenDalRemote::webdav(&settings.webdav, Default::default())
        } else {
            OpenDalRemote::s3(&settings.s3, Default::default())
        }
        .unwrap();
        assert!(
            !native
                .list_sync_files("space/sync/v5/registry")
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            native.list_sync_files("space/sync/v5/registry").unwrap(),
            adapter.list_sync_files("space/sync/v5/registry").unwrap()
        );
    }
}

#[test]
fn rotated_password_publishes_a_new_space_preserves_credentials_and_leaves_old_objects_readable() {
    let a = TestConfigDir::new("nyaterm-v4-rotation-a");
    let b = TestConfigDir::new("nyaterm-v4-rotation-b");
    let dir = TestConfigDir::new("nyaterm-v4-rotation-remote");
    let (sa, mut settings, oa) = setup(&a, &dir);
    let (sb, _, mut ob) = setup(&b, &dir);
    settings.v4_scope.secrets = true;
    let saved = settings.clone();
    sa.request_fn(StoreDomain::CloudSync, move |store| {
        store.save_master_password(Some("password"))?;
        store.save_cloud_sync_settings(saved)?;
        store.save_password(
            serde_json::from_value(
                json!({"id":"account","name":"Account","password":"synthetic-rotation-secret"}),
            )
            .unwrap(),
        )
    })
    .unwrap();
    save(&sa, "server", "Saved server");
    let control = CloudSyncOperationControl::default();
    let initial = preview(&sa, &settings, &oa, &control).unwrap();
    apply_and_publish(&sa, &settings, &oa, initial, &control).unwrap();
    let remote = LocalDirectoryRemote::new(dir.path());
    let before = prepare_remote(&sa, &settings, &oa, &remote, false).unwrap();
    let old_object = before.apply.local.observed.values().next().unwrap().clone();
    let old_bytes = remote
        .read_if_exists(&format!("space/{}", old_object.path()))
        .unwrap()
        .unwrap();
    let mut target_options = oa.clone();
    target_options.remote_root = "space/rotation-fixture".into();
    target_options.master_password = "new-password".into();
    let mut target_settings = settings.clone();
    target_settings.remote_root = target_options.remote_root.clone();
    let target = destination(&target_settings, &target_options);
    let staged = sa
        .request_fn(StoreDomain::CloudSync, move |store| {
            store.stage_v4_password_rotation(
                before.apply,
                "new-password",
                &target,
                "space/rotation-fixture",
            )
        })
        .unwrap();
    let plaintext = nyaterm_core::sync_schema::encryption::open(
        "new-password",
        "publication",
        &format!(
            "{}/{}",
            staged.pending.id.writer, staged.pending.id.sequence
        ),
        &staged.pending.bytes,
    )
    .unwrap();
    let publication: nyaterm_sync::protocol::PublishedReplica =
        serde_json::from_slice(&plaintext).unwrap();
    publication.validate_fixed().unwrap();
    assert!(publication.replica.values.values().all(|value| {
        !value
            .decode()
            .unwrap()
            .to_string()
            .contains("synthetic-rotation-secret")
    }));
    let failing = LostResponse {
        remote: LocalDirectoryRemote::new(dir.path()),
        fail: Cell::new(true),
    };
    assert!(publish_bytes(&failing, &target_options, &staged.pending).is_err());
    assert_eq!(
        sa.request_fn(StoreDomain::CloudSync, |store| store
            .master_password_matches("password"))
            .unwrap(),
        Some(true)
    );
    assert_eq!(
        remote
            .read_if_exists(&format!(
                "{}/{}",
                target_options.remote_root,
                staged.pending.id.path()
            ))
            .unwrap()
            .unwrap(),
        staged.pending.bytes
    );
    let rotated = super::rotation::rotate_with_options(
        &sa,
        &settings,
        &oa,
        "new-password".into(),
        &CloudSyncOperationControl::default(),
    )
    .unwrap();
    assert_eq!(rotated.remote_root, "space/rotation-fixture");
    assert_ne!(rotated.remote_root, settings.remote_root);
    assert_eq!(
        remote
            .read_if_exists(&format!("space/{}", old_object.path()))
            .unwrap()
            .unwrap(),
        old_bytes
    );
    assert!(
        nyaterm_core::sync_schema::encryption::open(
            "password",
            "publication",
            &format!("{}/{}", old_object.writer, old_object.sequence),
            &old_bytes
        )
        .is_ok()
    );
    ob.remote_root = rotated.remote_root.clone();
    ob.master_password = "new-password".into();
    let saved = rotated.clone();
    sb.request_fn(StoreDomain::CloudSync, move |store| {
        store.save_master_password(Some("new-password"))?;
        store.save_cloud_sync_settings(saved)
    })
    .unwrap();
    let control = CloudSyncOperationControl::default();
    let joined = preview(&sb, &rotated, &ob, &control).unwrap();
    assert!(joined.summary().deferred.is_empty());
    assert!(
        apply_and_publish(&sb, &rotated, &ob, joined, &control)
            .unwrap()
            .verified
    );
    sb.request_fn(StoreDomain::CloudSync, |store| {
        assert_eq!(store.list_connections()?[0].name, "Saved server");
        assert!(store.load_decrypted_password_by_id("account")?.is_none());
        Ok(())
    })
    .unwrap();
}

#[test]
#[ignore = "manual persisted two-device Local Provider scale workload"]
fn persisted_local_provider_scale_and_unchanged_checks() {
    use std::time::Instant;
    for count in [1_000, 5_000, 10_000] {
        let a = TestConfigDir::new("nyaterm-v4-scale-a");
        let b = TestConfigDir::new("nyaterm-v4-scale-b");
        let remote = TestConfigDir::new("nyaterm-v4-scale-remote");
        let (sa, settings, oa) = setup(&a, &remote);
        let (sb, _, ob) = setup(&b, &remote);
        sa.request_fn(StoreDomain::CloudSync, move |store| {
            let mut sessions = nyaterm_core::SessionsConfig::default();
            for index in 0..count {
                sessions.connections.push(serde_json::from_value(json!({"id":format!("server-{index}"),"name":format!("Server {index}"),"type":"ssh","host":"example.invalid","port":22,"username":"user"}))?);
            }
            store.replace_sessions(&sessions)
        }).unwrap();
        let control = CloudSyncOperationControl::default();
        let start = Instant::now();
        let initial = preview(&sa, &settings, &oa, &control).unwrap();
        assert!(
            apply_and_publish(&sa, &settings, &oa, initial, &control)
                .unwrap()
                .verified
        );
        let initial_ms = start.elapsed().as_millis();
        let publications = || -> usize {
            std::fs::read_dir(remote.path().join("space/sync/v5/writers"))
                .unwrap()
                .map(|writer| std::fs::read_dir(writer.unwrap().path()).unwrap().count())
                .sum()
        };
        let before = publications();
        let start = Instant::now();
        let unchanged = preview(&sa, &settings, &oa, &control).unwrap();
        assert_eq!(
            unchanged.apply.local.replica.as_ref().unwrap().document,
            unchanged.apply.plan.replica.document
        );
        assert!(
            apply_and_publish(&sa, &settings, &oa, unchanged, &control)
                .unwrap()
                .verified
        );
        let unchanged_ms = start.elapsed().as_millis();
        assert_eq!(publications(), before);
        let start = Instant::now();
        let joined = preview(&sb, &settings, &ob, &control).unwrap();
        assert!(joined.apply.plan.conflicts.is_empty());
        assert!(
            apply_and_publish(&sb, &settings, &ob, joined, &control)
                .unwrap()
                .verified
        );
        assert_eq!(
            sb.request_fn(StoreDomain::CloudSync, |store| store.list_connections())
                .unwrap()
                .len(),
            count
        );
        println!(
            "persisted_local resources={count} initial_ms={initial_ms} unchanged_ms={unchanged_ms} second_device_ms={} publications_before_join={before}",
            start.elapsed().as_millis()
        );
    }
}

#[test]
fn two_devices_join_then_merge_offline_edits_without_overwriting_other_resources() {
    let a = TestConfigDir::new("nyaterm-v4-device-a");
    let b = TestConfigDir::new("nyaterm-v4-device-b");
    let remote = TestConfigDir::new("nyaterm-v4-remote");
    let (sa, settings, oa) = setup(&a, &remote);
    let (sb, _, ob) = setup(&b, &remote);
    save(&sa, "a", "A");
    save(&sb, "b", "B");
    let control = CloudSyncOperationControl::default();
    let first = preview(&sa, &settings, &oa, &control).unwrap();
    assert!(first.migration);
    assert!(
        apply_and_publish(&sa, &settings, &oa, first, &control)
            .unwrap()
            .verified
    );
    let joined = preview(&sb, &settings, &ob, &control).unwrap();
    assert_eq!(joined.resource_count, 2);
    assert!(
        apply_and_publish(&sb, &settings, &ob, joined, &control)
            .unwrap()
            .verified
    );
    let caught_up = preview(&sa, &settings, &oa, &control).unwrap();
    apply_and_publish(&sa, &settings, &oa, caught_up, &control).unwrap();
    save(&sa, "a", "A edited offline");
    save(&sb, "b", "B edited offline");
    let pa = preview(&sa, &settings, &oa, &control).unwrap();
    let pb = preview(&sb, &settings, &ob, &control).unwrap();
    apply_and_publish(&sa, &settings, &oa, pa, &control).unwrap();
    // The preview must be refreshed if another writer became visible before
    // confirmation. No saved offline edit is discarded by that rejection.
    assert!(matches!(
        apply_and_publish(&sb, &settings, &ob, pb, &control),
        Err(CloudSyncError::LocalChangedDuringSync)
    ));
    let pb = preview(&sb, &settings, &ob, &control).unwrap();
    apply_and_publish(&sb, &settings, &ob, pb, &control).unwrap();
    let merged = preview(&sa, &settings, &oa, &control).unwrap();
    assert!(merged.apply.plan.conflicts.is_empty());
    apply_and_publish(&sa, &settings, &oa, merged, &control).unwrap();
    let records = sa
        .request_fn(StoreDomain::CloudSync, |store| store.list_connections())
        .unwrap();
    assert!(
        records
            .iter()
            .any(|record| record.id == "a" && record.name == "A edited offline")
    );
    assert!(
        records
            .iter()
            .any(|record| record.id == "b" && record.name == "B edited offline")
    );
    let unchanged = preview(&sa, &settings, &oa, &control).unwrap();
    assert_eq!(
        unchanged.apply.local.replica.as_ref().unwrap().document,
        unchanged.apply.plan.replica.document
    );
    let original_record = unchanged.apply.expected_record.clone();
    apply_and_publish(&sa, &settings, &oa, unchanged, &control).unwrap();
    let next = preview(&sa, &settings, &oa, &control).unwrap();
    assert_eq!(next.apply.expected_record, original_record);
}

#[test]
#[ignore = "requires an explicitly configured disposable WebDAV test server"]
fn webdav_server_merges_two_devices_and_keeps_unchanged_publications() {
    let endpoint =
        std::env::var("NYATERM_V4_TEST_WEBDAV").expect("set the disposable test server URL");
    let a = TestConfigDir::new("nyaterm-v4-webdav-a");
    let b = TestConfigDir::new("nyaterm-v4-webdav-b");
    let unused = TestConfigDir::new("nyaterm-v4-webdav-unused");
    let (sa, mut settings, oa) = setup(&a, &unused);
    let (sb, _, ob) = setup(&b, &unused);
    settings.provider = "webdav".into();
    settings.webdav.endpoint = endpoint;
    settings.webdav.root = format!("v4-test-{}", Uuid::new_v4());
    for store in [&sa, &sb] {
        let saved = settings.clone();
        store
            .request_fn(StoreDomain::CloudSync, move |store| {
                store.save_cloud_sync_settings(saved)
            })
            .unwrap();
    }
    save(&sa, "shared", "Original");
    let control = CloudSyncOperationControl::default();
    let initial = preview(&sa, &settings, &oa, &control).unwrap();
    assert!(
        apply_and_publish(&sa, &settings, &oa, initial, &control)
            .unwrap()
            .verified
    );
    let control = CloudSyncOperationControl::default();
    let joined = preview(&sb, &settings, &ob, &control).unwrap();
    assert!(
        apply_and_publish(&sb, &settings, &ob, joined, &control)
            .unwrap()
            .verified
    );
    save(&sa, "shared", "Name edited offline");
    sb.request_fn(StoreDomain::CloudSync, |store| {
        let mut connection = store.get_connection("shared")?.unwrap();
        connection.description = Some("Description edited offline".into());
        store.save_connection(&connection)
    })
    .unwrap();
    let control = CloudSyncOperationControl::default();
    let from_a = preview(&sa, &settings, &oa, &control).unwrap();
    let stale_b = preview(&sb, &settings, &ob, &control).unwrap();
    assert!(
        apply_and_publish(&sa, &settings, &oa, from_a, &control)
            .unwrap()
            .verified
    );
    assert!(matches!(
        apply_and_publish(&sb, &settings, &ob, stale_b, &control),
        Err(CloudSyncError::LocalChangedDuringSync)
    ));
    let control = CloudSyncOperationControl::default();
    let merged = preview(&sb, &settings, &ob, &control).unwrap();
    assert!(merged.apply.plan.conflicts.is_empty());
    assert!(
        apply_and_publish(&sb, &settings, &ob, merged, &control)
            .unwrap()
            .verified
    );
    sb.request_fn(StoreDomain::CloudSync, |store| {
        let saved = store.get_connection("shared")?.unwrap();
        assert_eq!(saved.name, "Name edited offline");
        assert_eq!(
            saved.description.as_deref(),
            Some("Description edited offline")
        );
        Ok(())
    })
    .unwrap();
    let control = CloudSyncOperationControl::default();
    let unchanged = preview(&sb, &settings, &ob, &control).unwrap();
    let observed = unchanged.apply.local.observed.clone();
    let receipt = unchanged.apply.expected_record.clone();
    assert!(
        apply_and_publish(&sb, &settings, &ob, unchanged, &control)
            .unwrap()
            .verified
    );
    let checked = preview(&sb, &settings, &ob, &control).unwrap();
    assert_eq!(checked.apply.local.observed, observed);
    assert_eq!(checked.apply.expected_record, receipt);
}

#[test]
fn resolving_a_persisted_conflict_is_not_skipped_as_an_unchanged_check() {
    let a = TestConfigDir::new("nyaterm-v4-resolution-a");
    let b = TestConfigDir::new("nyaterm-v4-resolution-b");
    let remote = TestConfigDir::new("nyaterm-v4-resolution-remote");
    let (sa, settings, oa) = setup(&a, &remote);
    let (sb, _, ob) = setup(&b, &remote);
    let control = CloudSyncOperationControl::default();
    save(&sa, "shared", "Initial");
    let initial = preview(&sa, &settings, &oa, &control).unwrap();
    apply_and_publish(&sa, &settings, &oa, initial, &control).unwrap();
    let joined = preview(&sb, &settings, &ob, &control).unwrap();
    apply_and_publish(&sb, &settings, &ob, joined, &control).unwrap();
    save(&sa, "shared", "From A");
    save(&sb, "shared", "From B");
    let changed = preview(&sa, &settings, &oa, &control).unwrap();
    apply_and_publish(&sa, &settings, &oa, changed, &control).unwrap();
    let concurrent = preview(&sb, &settings, &ob, &control).unwrap();
    assert_eq!(concurrent.apply.plan.conflicts.len(), 1);
    apply_and_publish(&sb, &settings, &ob, concurrent, &control).unwrap();
    let mut resolution = preview(&sb, &settings, &ob, &control).unwrap();
    let choice = resolution.summary().conflicts[0]
        .choices
        .iter()
        .position(|choice| choice.contains("From A"))
        .unwrap();
    resolution.resolve(&sb, "password", 0, choice).unwrap();
    let applied = apply_and_publish(&sb, &settings, &ob, resolution, &control).unwrap();
    assert_eq!(applied.conflicts, 0);
    assert_eq!(
        sb.request_fn(StoreDomain::CloudSync, |store| store.list_connections())
            .unwrap()[0]
            .name,
        "From A"
    );
}

struct LostResponse {
    remote: LocalDirectoryRemote,
    fail: Cell<bool>,
}
impl CloudSyncRemote for LostResponse {
    fn provider(&self) -> &'static str {
        "local_directory"
    }
    fn create_dir(&self, path: &str) -> Result<(), CloudSyncError> {
        self.remote.create_dir(path)
    }
    fn read_if_exists(&self, path: &str) -> Result<Option<Vec<u8>>, CloudSyncError> {
        self.remote.read_if_exists(path)
    }
    fn write(&self, path: &str, bytes: &[u8]) -> Result<(), CloudSyncError> {
        self.remote.write(path, bytes)?;
        if self.fail.replace(false) {
            Err(CloudSyncError::Remote("response lost".into()))
        } else {
            Ok(())
        }
    }
    fn delete(&self, path: &str) -> Result<(), CloudSyncError> {
        self.remote.delete(path)
    }
    fn list_files(&self, path: &str) -> Result<Vec<String>, CloudSyncError> {
        self.remote.list_sync_files(path)
    }
}

#[test]
fn response_loss_retries_exact_bytes_and_verifies_before_registering_writer() {
    let local = TestConfigDir::new("nyaterm-v4-retry-local");
    let dir = TestConfigDir::new("nyaterm-v4-retry-remote");
    let (_, _, options) = setup(&local, &dir);
    let remote = LostResponse {
        remote: LocalDirectoryRemote::new(dir.path()),
        fail: Cell::new(true),
    };
    let writer = Uuid::new_v4();
    let mut replica = nyaterm_sync::engine::SyncReplica::new(writer);
    let snapshot = replica.snapshot();
    let publication =
        nyaterm_sync::protocol::PublishedReplica::fixed(writer, 1, &snapshot).unwrap();
    let bytes = nyaterm_core::sync_schema::encryption::seal(
        "password",
        "publication",
        &format!("{writer}/1"),
        &serde_json::to_vec(&publication).unwrap(),
    )
    .unwrap();
    let pending = PendingPublication::new(
        writer,
        1,
        bytes,
        nyaterm_sync::engine::digest(&snapshot.document),
    )
    .unwrap();
    assert!(publish_bytes(&remote, &options, &pending).is_err());
    assert!(
        remote
            .list_files("space/sync/v5/registry")
            .unwrap()
            .is_empty()
    );
    publish_bytes(&remote, &options, &pending).unwrap();
    assert_eq!(
        remote
            .read_if_exists(&format!("space/{}", pending.id.path()))
            .unwrap()
            .unwrap(),
        pending.bytes
    );
    assert_eq!(
        remote.list_files("space/sync/v5/registry").unwrap().len(),
        1
    );
}

#[test]
fn failed_publication_keeps_reserved_bytes_and_does_not_acknowledge_a_new_business_edit() {
    let local = TestConfigDir::new("nyaterm-v4-durable-retry-local");
    let dir = TestConfigDir::new("nyaterm-v4-durable-retry-remote");
    let (store, settings, options) = setup(&local, &dir);
    save(&store, "a", "before upload");
    let prepared = preview(
        &store,
        &settings,
        &options,
        &CloudSyncOperationControl::default(),
    )
    .unwrap();
    let destination = prepared.apply.destination.clone();
    let record = store
        .request_fn(StoreDomain::CloudSync, move |store| {
            store.apply_v4_plan(prepared.apply, "password")
        })
        .unwrap();
    let remote = LostResponse {
        remote: LocalDirectoryRemote::new(dir.path()),
        fail: Cell::new(true),
    };
    assert!(publish(&store, &options, &destination, record, &remote).is_err());
    save(&store, "a", "saved after apply");
    let dest = destination.clone();
    let view = store
        .request_fn(StoreDomain::CloudSync, move |store| {
            store.load_v4_view(&dest, "password", &Default::default())
        })
        .unwrap();
    let reserved = view.record.pending.as_ref().unwrap().clone();
    assert!(!view.projection.edited.is_empty());
    remote
        .remote
        .write(
            &format!("space/{}", reserved.id.path()),
            b"interrupted upload",
        )
        .unwrap();
    let retry_preview = prepare_remote(&store, &settings, &options, &remote, false).unwrap();
    assert_eq!(
        retry_preview.apply.local.pending.as_ref().unwrap().bytes,
        reserved.bytes
    );
    assert!(
        !retry_preview
            .apply
            .local
            .observed
            .contains_key(&reserved.id.path())
    );
    assert!(
        publish(&store, &options, &destination, view.record, &remote)
            .unwrap()
            .verified
    );
    assert_eq!(
        remote
            .read_if_exists(&format!("space/{}", reserved.id.path()))
            .unwrap()
            .unwrap(),
        reserved.bytes
    );
    let next = preview(
        &store,
        &settings,
        &options,
        &CloudSyncOperationControl::default(),
    )
    .unwrap();
    assert_ne!(
        next.apply.local.replica.as_ref().unwrap().document,
        next.apply.plan.replica.document
    );
    assert!(next.apply.plan.conflicts.is_empty());
}

#[test]
fn a_late_publisher_cannot_acknowledge_or_replace_a_newer_applied_replica() {
    let local = TestConfigDir::new("nyaterm-v4-stale-publisher-local");
    let dir = TestConfigDir::new("nyaterm-v4-stale-publisher-remote");
    let (store, settings, options) = setup(&local, &dir);
    save(&store, "a", "First apply");
    let initial = preview(&store, &settings, &options, &Default::default()).unwrap();
    let destination = initial.apply.destination.clone();
    let first = store
        .request_fn(StoreDomain::CloudSync, move |store| {
            store.apply_v4_plan(initial.apply, "password")
        })
        .unwrap();
    save(&store, "a", "Second apply");
    let next = preview(&store, &settings, &options, &Default::default()).unwrap();
    let latest = store
        .request_fn(StoreDomain::CloudSync, move |store| {
            store.apply_v4_plan(next.apply, "password")
        })
        .unwrap();
    let remote = LocalDirectoryRemote::new(dir.path());
    assert!(matches!(
        publish(&store, &options, &destination, first, &remote),
        Err(CloudSyncError::LocalChangedDuringSync)
    ));
    let dest = destination.clone();
    let saved = store
        .request_fn(StoreDomain::CloudSync, move |store| {
            store.load_v4_view(&dest, "password", &Default::default())
        })
        .unwrap();
    assert!(saved.record == latest);
    assert!(
        remote
            .list_sync_files("space/sync/v5/registry")
            .unwrap()
            .is_empty()
    );
    assert!(
        publish(&store, &options, &destination, latest, &remote)
            .unwrap()
            .verified
    );
    store
        .request_fn(StoreDomain::CloudSync, |store| {
            assert_eq!(store.get_connection("a")?.unwrap().name, "Second apply");
            Ok(())
        })
        .unwrap();
}

#[test]
fn moving_a_webdav_configuration_does_not_orphan_its_pending_publication() {
    let local = TestConfigDir::new("nyaterm-v4-destination-local");
    let remote = TestConfigDir::new("nyaterm-v4-destination-remote");
    let (_, mut settings, mut options) = setup(&local, &remote);
    settings.provider = "webdav".into();
    settings.webdav.endpoint = "https://example.invalid/dav".into();
    let original = destination(&settings, &options);
    options.config_dir = options.config_dir.join("moved");
    options.remote_dir = options.config_dir.join("cloud-sync-local");
    assert_eq!(original, destination(&settings, &options));
    settings.webdav.username = "different-account".into();
    assert_ne!(original, destination(&settings, &options));
    settings.provider = "s3".into();
    settings.s3.bucket = "one".into();
    let first = destination(&settings, &options);
    settings.s3.bucket = "two".into();
    assert_ne!(first, destination(&settings, &options));
}

#[test]
fn later_legacy_writes_pause_automatic_sync_and_require_a_confirmed_import_preview() {
    let local = TestConfigDir::new("nyaterm-v4-legacy-import-local");
    let old = TestConfigDir::new("nyaterm-v4-legacy-import-old");
    let dir = TestConfigDir::new("nyaterm-v4-legacy-import-remote");
    let (store, settings, options) = setup(&local, &dir);
    save(&store, "v4", "V4 saved");
    let control = CloudSyncOperationControl::default();
    let initial = preview(&store, &settings, &options, &control).unwrap();
    apply_and_publish(&store, &settings, &options, initial, &control).unwrap();

    let legacy = blocking_test_store(old.path());
    save(&legacy, "legacy", "Legacy saved");
    let mut legacy_options = options.clone();
    legacy_options.config_dir = old.path().into();
    legacy_options.device_id = Uuid::new_v4().to_string();
    nyaterm_core::cloud_sync::push_snapshot_with_remote(
        &legacy,
        &legacy_options,
        &LocalDirectoryRemote::new(dir.path()),
        &nyaterm_core::CloudSyncState::default(),
        true,
    )
    .unwrap();
    assert!(preview(&store, &settings, &options, &control).is_err());
    let import = preview_manual(&store, &settings, &options, &control).unwrap();
    assert!(import.migration);
    assert_eq!(import.resource_count, 2);
    // Prepare still has not changed the business configuration.
    assert_eq!(
        store
            .request_fn(StoreDomain::CloudSync, |store| store.list_connections())
            .unwrap()
            .len(),
        1
    );
    assert!(
        apply_and_publish(&store, &settings, &options, import, &control)
            .unwrap()
            .verified
    );
    assert!(
        !preview(&store, &settings, &options, &control)
            .unwrap()
            .migration
    );
    assert_eq!(
        store
            .request_fn(StoreDomain::CloudSync, |store| store.list_connections())
            .unwrap()
            .len(),
        2
    );
}

struct DelayedListing(LocalDirectoryRemote);
impl CloudSyncRemote for DelayedListing {
    fn provider(&self) -> &'static str {
        "local_directory"
    }
    fn create_dir(&self, path: &str) -> Result<(), CloudSyncError> {
        self.0.create_dir(path)
    }
    fn read_if_exists(&self, path: &str) -> Result<Option<Vec<u8>>, CloudSyncError> {
        self.0.read_if_exists(path)
    }
    fn write(&self, path: &str, bytes: &[u8]) -> Result<(), CloudSyncError> {
        self.0.write(path, bytes)
    }
    fn delete(&self, path: &str) -> Result<(), CloudSyncError> {
        self.0.delete(path)
    }
    fn list_files(&self, _: &str) -> Result<Vec<String>, CloudSyncError> {
        Ok(Vec::new())
    }
}

#[test]
fn delayed_listing_retains_receipts_and_identity_forks_stop_before_business_apply() {
    let local = TestConfigDir::new("nyaterm-v4-stale-list-local");
    let dir = TestConfigDir::new("nyaterm-v4-stale-list-remote");
    let (store, settings, options) = setup(&local, &dir);
    save(&store, "saved", "Preserved");
    let control = CloudSyncOperationControl::default();
    let initial = preview(&store, &settings, &options, &control).unwrap();
    apply_and_publish(&store, &settings, &options, initial, &control).unwrap();
    let before = preview(&store, &settings, &options, &control).unwrap();
    let delayed = prepare_remote(
        &store,
        &settings,
        &options,
        &DelayedListing(LocalDirectoryRemote::new(dir.path())),
        false,
    )
    .unwrap();
    assert_eq!(before.apply.local.observed, delayed.apply.local.observed);
    assert_eq!(before.apply.plan.effective, delayed.apply.plan.effective);
    let observed = before.apply.local.observed.values().next().unwrap();
    let fork = PendingPublication::new(
        observed.writer,
        observed.sequence,
        vec![9, 8, 7],
        "fork".into(),
    )
    .unwrap();
    let remote = LocalDirectoryRemote::new(dir.path());
    remote
        .write_sync_file(&format!("space/{}", fork.id.path()), &fork.bytes)
        .unwrap();
    let error = preview(&store, &settings, &options, &control)
        .err()
        .unwrap();
    assert!(error.to_string().contains("fork"));
    assert_eq!(
        store
            .request_fn(StoreDomain::CloudSync, |store| store.list_connections())
            .unwrap()[0]
            .name,
        "Preserved"
    );
    assert!(
        remote
            .read_if_exists(&format!("space/{}", observed.path()))
            .unwrap()
            .is_some()
    );
    assert!(
        remote
            .read_if_exists(&format!("space/{}", fork.id.path()))
            .unwrap()
            .is_some()
    );
}

#[test]
fn opendal_drives_import_native_v3_and_preserve_multi_device_conflicts_and_pending_recovery() {
    use crate::http::cloud_sync::metrics::with_test_drive_http;
    use crate::http::cloud_sync::{
        DriveFixture, NativeAliyunDriveRemote, NativeGoogleDriveRemote, NativeOneDriveRemote,
    };
    use nyaterm_core::OAuthDriveSyncSettings;
    use nyaterm_core::cloud_sync::credentials::DriveProvider;
    for provider in [
        DriveProvider::Google,
        DriveProvider::OneDrive,
        DriveProvider::Aliyun,
    ] {
        let fixture = DriveFixture::new();
        with_test_drive_http(&fixture.server.endpoint, || {
            let a = TestConfigDir::new("nyaterm-drive-v4-a");
            let b = TestConfigDir::new("nyaterm-drive-v4-b");
            let unused = TestConfigDir::new("nyaterm-drive-v4-unused");
            let (sa, mut settings, oa) = setup(&a, &unused);
            let (sb, _, ob) = setup(&b, &unused);
            let oauth = OAuthDriveSyncSettings {
                access_token: Some("access".into()),
                ..Default::default()
            };
            settings.provider = match provider {
                DriveProvider::Google => "google_drive",
                DriveProvider::OneDrive => "onedrive",
                DriveProvider::Aliyun => "aliyun_drive",
            }
            .into();
            settings.remote_root = "space".into();
            settings.google_drive = oauth.clone();
            settings.onedrive = oauth.clone();
            settings.aliyun_drive.access_token = oauth.access_token.clone();
            for store in [&sa, &sb] {
                let saved = settings.clone();
                store
                    .request_fn(StoreDomain::CloudSync, move |store| {
                        store.save_cloud_sync_settings(saved)
                    })
                    .unwrap();
            }
            save(&sa, "server", "Native legacy");
            let native: Box<dyn CloudSyncRemote> = match provider {
                DriveProvider::Google => {
                    Box::new(NativeGoogleDriveRemote::new(&settings.google_drive).unwrap())
                }
                DriveProvider::OneDrive => {
                    Box::new(NativeOneDriveRemote::new(&settings.onedrive).unwrap())
                }
                DriveProvider::Aliyun => {
                    Box::new(NativeAliyunDriveRemote::new(&settings.aliyun_drive).unwrap())
                }
            };
            let legacy = nyaterm_core::cloud_sync::push_snapshot_with_remote(
                &sa,
                &oa,
                native.as_ref(),
                &Default::default(),
                true,
            )
            .unwrap();
            let legacy_path = format!(
                "space/{}",
                nyaterm_core::cloud_sync::legacy_sync_snapshot_file(
                    &legacy.pointer.unwrap().revision_id
                )
            );
            let old_bytes = native.read_if_exists(&legacy_path).unwrap().unwrap();
            let initial =
                preview(&sa, &settings, &oa, &CloudSyncOperationControl::default()).unwrap();
            assert!(initial.migration);
            // Lost publication reply after the local commit must leave durable pending.
            fixture.state.lock().unwrap().lose_move_reply = true;
            let pending = apply_and_publish(
                &sa,
                &settings,
                &oa,
                initial,
                &CloudSyncOperationControl::default(),
            )
            .unwrap();
            assert!(!pending.verified);
            assert!(pending.publication_error.is_some());
            let recovery =
                preview(&sa, &settings, &oa, &CloudSyncOperationControl::default()).unwrap();
            let recovered = apply_and_publish(
                &sa,
                &settings,
                &oa,
                recovery,
                &CloudSyncOperationControl::default(),
            )
            .unwrap();
            assert!(
                recovered.verified,
                "{provider:?}: {:?}",
                recovered.publication_error
            );
            let joined =
                preview(&sb, &settings, &ob, &CloudSyncOperationControl::default()).unwrap();
            assert!(
                apply_and_publish(
                    &sb,
                    &settings,
                    &ob,
                    joined,
                    &CloudSyncOperationControl::default()
                )
                .unwrap()
                .verified
            );
            save(&sa, "server", "A offline edit");
            save(&sb, "server", "B offline edit");
            let pa = preview(&sa, &settings, &oa, &CloudSyncOperationControl::default()).unwrap();
            let pb = preview(&sb, &settings, &ob, &CloudSyncOperationControl::default()).unwrap();
            assert!(
                apply_and_publish(
                    &sa,
                    &settings,
                    &oa,
                    pa,
                    &CloudSyncOperationControl::default()
                )
                .unwrap()
                .verified
            );
            assert!(matches!(
                apply_and_publish(
                    &sb,
                    &settings,
                    &ob,
                    pb,
                    &CloudSyncOperationControl::default()
                ),
                Err(CloudSyncError::LocalChangedDuringSync)
            ));
            let conflicted =
                preview(&sb, &settings, &ob, &CloudSyncOperationControl::default()).unwrap();
            assert!(!conflicted.summary().conflicts.is_empty());
            let cancelled = CloudSyncOperationControl::default();
            cancelled.request_cancel();
            assert!(matches!(
                preview(&sb, &settings, &ob, &cancelled),
                Err(CloudSyncError::Cancelled)
            ));
            assert_eq!(
                native.read_if_exists(&legacy_path).unwrap().unwrap(),
                old_bytes
            );
            assert!(
                !native
                    .list_files("space/sync/v5/registry")
                    .unwrap()
                    .is_empty()
            );
        });
    }
}
