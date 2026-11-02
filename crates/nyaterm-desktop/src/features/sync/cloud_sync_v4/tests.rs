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
    apply_and_publish, destination, prepare_remote, preview, preview_manual, publish, publish_bytes,
};
use crate::test_support::{TestConfigDir, blocking_test_store};

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
        assert_eq!(
            store
                .load_decrypted_password_by_id("account")?
                .unwrap()
                .password
                .unwrap()
                .expose_secret(),
            "synthetic-rotation-secret"
        );
        Ok(())
    })
    .unwrap();
}

#[test]
fn received_trust_waits_for_local_preview_confirmation_while_other_fields_apply() {
    let a = TestConfigDir::new("nyaterm-v4-trust-provider-a");
    let b = TestConfigDir::new("nyaterm-v4-trust-provider-b");
    let dir = TestConfigDir::new("nyaterm-v4-trust-provider-remote");
    let (sa, mut settings, oa) = setup(&a, &dir);
    let (sb, _, ob) = setup(&b, &dir);
    settings.v4_scope.trusted_hosts = true;
    for store in [&sa, &sb] {
        let saved = settings.clone();
        store
            .request_fn(StoreDomain::CloudSync, move |store| {
                store.save_cloud_sync_settings(saved)
            })
            .unwrap();
    }
    sa.request_fn(StoreDomain::CloudSync, |store| {
        store.remember_vnc_known_host(
            "example.com",
            5900,
            &format!("SHA256:{}", "ab".repeat(32)),
            None,
        )
    })
    .unwrap();
    save(&sa, "server", "Portable config");
    let control = CloudSyncOperationControl::default();
    let initial = preview(&sa, &settings, &oa, &control).unwrap();
    apply_and_publish(&sa, &settings, &oa, initial, &control).unwrap();
    let control = CloudSyncOperationControl::default();
    let incoming = preview(&sb, &settings, &ob, &control).unwrap();
    assert_eq!(incoming.summary().trust.len(), 1);
    let applied = apply_and_publish(&sb, &settings, &ob, incoming, &control).unwrap();
    assert!(applied.deferred > 0);
    sb.request_fn(StoreDomain::CloudSync, |store| {
        assert!(store.load_vnc_known_host("example.com", 5900)?.is_none());
        assert_eq!(store.list_connections()?[0].name, "Portable config");
        Ok(())
    })
    .unwrap();
    let control = CloudSyncOperationControl::default();
    let mut pending = preview(&sb, &settings, &ob, &control).unwrap();
    let review = pending.summary().trust[0].clone();
    pending
        .confirm_trust(&sb, "password", &review.key, &review.token)
        .unwrap();
    assert!(pending.summary().trust.is_empty());
    assert!(
        apply_and_publish(&sb, &settings, &ob, pending, &control)
            .unwrap()
            .verified
    );
    assert!(
        sb.request_fn(StoreDomain::CloudSync, |store| store
            .load_vnc_known_host("example.com", 5900))
            .unwrap()
            .is_some()
    );
}

#[test]
fn local_provider_transfers_encrypted_secret_values_and_scope_off_preserves_the_vault() {
    let a = TestConfigDir::new("nyaterm-v4-secret-provider-a");
    let b = TestConfigDir::new("nyaterm-v4-secret-provider-b");
    let remote = TestConfigDir::new("nyaterm-v4-secret-provider-remote");
    let (sa, mut settings, oa) = setup(&a, &remote);
    let (sb, _, ob) = setup(&b, &remote);
    settings.v4_scope.secrets = true;
    for store in [&sa, &sb] {
        let settings = settings.clone();
        store
            .request_fn(StoreDomain::CloudSync, move |store| {
                store.save_cloud_sync_settings(settings)
            })
            .unwrap();
    }
    sa.request_fn(StoreDomain::CloudSync, |store| {
        store.save_master_password(Some("password"))?;
        store.save_password(serde_json::from_value(json!({"id":"account","name":"Account","username":"user","password":"synthetic-provider-secret"})).unwrap())
    }).unwrap();
    sb.request_fn(StoreDomain::CloudSync, |store| {
        store.save_master_password(Some("different-device-password"))
    })
    .unwrap();
    let control = CloudSyncOperationControl::default();
    let pa = preview(&sa, &settings, &oa, &control).unwrap();
    assert!(
        apply_and_publish(&sa, &settings, &oa, pa, &control)
            .unwrap()
            .verified
    );
    let control = CloudSyncOperationControl::default();
    let pb = preview(&sb, &settings, &ob, &control).unwrap();
    assert!(pb.summary().deferred.is_empty());
    assert!(
        apply_and_publish(&sb, &settings, &ob, pb, &control)
            .unwrap()
            .verified
    );
    sb.request_fn(StoreDomain::CloudSync, |store| {
        assert_eq!(
            store
                .load_decrypted_password_by_id("account")?
                .unwrap()
                .password
                .unwrap()
                .expose_secret(),
            "synthetic-provider-secret"
        );
        Ok(())
    })
    .unwrap();
    settings.v4_scope.secrets = false;
    let saved = settings.clone();
    sb.request_fn(StoreDomain::CloudSync, move |store| {
        store.save_cloud_sync_settings(saved)
    })
    .unwrap();
    let control = CloudSyncOperationControl::default();
    let pb = preview(&sb, &settings, &ob, &control).unwrap();
    assert!(
        apply_and_publish(&sb, &settings, &ob, pb, &control)
            .unwrap()
            .verified
    );
    sb.request_fn(StoreDomain::CloudSync, |store| {
        assert!(store.load_decrypted_password_by_id("account")?.is_some());
        Ok(())
    })
    .unwrap();
}

#[test]
fn optional_ai_configuration_reaches_a_second_device_without_secrets_or_permissions() {
    let a = TestConfigDir::new("nyaterm-v4-ai-a");
    let b = TestConfigDir::new("nyaterm-v4-ai-b");
    let remote = TestConfigDir::new("nyaterm-v4-ai-remote");
    let (sa, mut settings, oa) = setup(&a, &remote);
    let (sb, _, ob) = setup(&b, &remote);
    settings.v4_scope.ai = true;
    for store in [&sa, &sb] {
        let settings = settings.clone();
        store
            .request_fn(StoreDomain::CloudSync, move |store| {
                store.save_cloud_sync_settings(settings)
            })
            .unwrap();
    }
    sa.request_fn(StoreDomain::CloudSync, |store| {
        let mut ai = store.load_ai_settings()?;
        ai.provider_credentials.push(serde_json::from_value(json!({"id":"custom","name":"Custom","provider_kind":"openai","base_url":"https://example.com/v1/","api_key":"fixture-key"}))?);
        ai.models.push(serde_json::from_value(json!({"id":"custom-model","name":"a-model","credential_id":"custom","source":"manual"}))?);
        ai.default_model_id = Some("custom-model".into());
        ai.agent_command_execution_mode = nyaterm_core::ai::AgentCommandExecutionMode::Auto;
        store.save_ai_settings(ai)
    }).unwrap();
    let control = CloudSyncOperationControl::default();
    let initial = preview(&sa, &settings, &oa, &control).unwrap();
    apply_and_publish(&sa, &settings, &oa, initial, &control).unwrap();
    let joined = preview(&sb, &settings, &ob, &control).unwrap();
    assert!(joined.summary().deferred.is_empty());
    assert!(joined.apply.plan.conflicts.is_empty());
    let report = apply_and_publish(&sb, &settings, &ob, joined, &control).unwrap();
    assert!(report.verified);
    let ai = sb
        .request_fn(StoreDomain::CloudSync, |store| store.load_ai_settings())
        .unwrap();
    let provider = ai
        .provider_credentials
        .iter()
        .find(|provider| provider.id == "custom")
        .unwrap();
    assert!(provider.api_key.is_none());
    assert_eq!(
        provider.base_url.as_deref(),
        Some("https://example.com/v1/")
    );
    assert_eq!(ai.default_model_id.as_deref(), Some("custom-model"));
    assert_eq!(
        ai.agent_command_execution_mode,
        nyaterm_core::ai::AgentCommandExecutionMode::ConfirmEach
    );
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
            std::fs::read_dir(remote.path().join("space/sync/v4/writers"))
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
    let pending =
        PendingPublication::new(Uuid::new_v4(), 1, vec![1, 2, 3], "document".into()).unwrap();
    assert!(publish_bytes(&remote, &options, &pending).is_err());
    assert!(
        remote
            .list_files("space/sync/v4/registry")
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
        remote.list_files("space/sync/v4/registry").unwrap().len(),
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
            .list_sync_files("space/sync/v4/registry")
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
