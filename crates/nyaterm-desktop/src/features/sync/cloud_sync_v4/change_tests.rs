use nyaterm_core::cloud_sync::control::CloudSyncOperationControl;
use nyaterm_core::sync_schema::{SyncFieldId, SyncResourceId, SyncScope};
use nyaterm_core::{
    CloudSyncSettings, LocalCloudSyncOptions, LocalDirectoryRemote, SavedConnection,
};
use nyaterm_store::{
    ConnectionStore, StorageError, StoreBlockingClient, StoreClientError, StoreDomain,
};
use nyaterm_sync::protocol::PendingPublication;
use serde_json::json;
use uuid::Uuid;

use super::{local, prepare_remote, preview_on_change, publish};
use crate::features::sync::io_runtime::IoScope;
use crate::test_support::{TestConfigDir, blocking_test_store};

struct Fixture {
    _dir: TestConfigDir,
    _remote: TestConfigDir,
    store: StoreBlockingClient,
    settings: CloudSyncSettings,
    options: LocalCloudSyncOptions,
}

impl Fixture {
    fn new(scope: SyncScope) -> Self {
        let dir = TestConfigDir::new("nyaterm-v4-local-change");
        let remote = TestConfigDir::new("nyaterm-v4-local-change-remote");
        let store = blocking_test_store(dir.path());
        // Intentionally invalid S3 configuration. Creating the production
        // adapter must fail, even if it never sends an HTTP request.
        let settings = CloudSyncSettings {
            enabled: true,
            protocol_version: 4,
            provider: "s3".into(),
            v4_scope: scope,
            ..Default::default()
        };
        let saved = settings.clone();
        store.request_fn(StoreDomain::CloudSync, move |store| {
            store.save_master_password(Some("password"))?;
            store.save_cloud_sync_settings(saved)?;
            // Match the typed appearance form's persisted representation,
            // including the integral font size accepted by V4 preferences.
            store.save_appearance_settings(&store.load_app_settings_summary()?)?;
            let connection: SavedConnection = serde_json::from_value(json!({
                "id":"server", "type":"ssh", "name":"Server", "host":"example.test", "username":"user"
            }))?;
            store.save_connection(&connection)
        }).unwrap();
        let options = LocalCloudSyncOptions {
            config_dir: dir.path().into(),
            portable_key_path: None,
            remote_dir: remote.path().into(),
            remote_root: settings.remote_root.clone(),
            device_id: Uuid::new_v4().to_string(),
            device_name: None,
            app_version: "test".into(),
            master_password: "password".into(),
            enabled: true,
        };
        let fixture = Self {
            _dir: dir,
            _remote: remote,
            store,
            settings,
            options,
        };
        fixture.establish_baseline();
        fixture
    }

    fn establish_baseline(&self) {
        // Seed a verified replica using a disposable local backend, without
        // constructing the deliberately invalid production S3 adapter.
        let remote = LocalDirectoryRemote::new(self.options.remote_dir.clone());
        let preview = prepare_remote(&self.store, &self.settings, &self.options, &remote, false)
            .unwrap_or_else(|error| {
                panic!(
                    "baseline prepare for {:?}: {error:?}",
                    self.settings.v4_scope
                )
            });
        let destination = preview.apply.destination.clone();
        let record = self
            .store
            .request_fn(StoreDomain::CloudSync, move |store| {
                store.apply_v4_plan(preview.apply, "password")
            })
            .unwrap();
        assert!(
            publish(&self.store, &self.options, &destination, record, &remote)
                .unwrap()
                .verified
        );
    }

    fn assert_skipped(&self) {
        let io = IoScope::new("local-change-test");
        assert!(
            preview_on_change(
                &self.store,
                &self.settings,
                &self.options,
                false,
                &CloudSyncOperationControl::default(),
            )
            .unwrap()
            .is_none()
        );
        assert_eq!(io.stats.lock().unwrap().requests, 0);
    }

    fn needs_remote(&self) -> bool {
        let view = local::load(&self.store, &self.settings, &self.options).unwrap();
        local::needs_remote(&view, &nyaterm_core::sync_schema::FIXED_CLOUD_SYNC_SCOPE).unwrap()
    }
}

#[test]
fn v4_local_changes_and_excluded_scopes_never_construct_the_remote_adapter() {
    let fixture = Fixture::new(SyncScope::default());
    type Update = fn(&ConnectionStore) -> Result<(), StorageError>;
    let updates: &[Update] = &[
        |store| store.save_connection(&store.get_connection("server")?.unwrap()),
        |store| store.mark_connection_used("server"),
        |store| {
            store
                .update_auto_detected_connection_icon("server", "linux")
                .map(|_| ())
        },
        |store| {
            store
                .merge_connection_asset_from_monitoring(
                    "server",
                    nyaterm_core::AssetMetadata {
                        os_name: Some("Linux".into()),
                        ..Default::default()
                    },
                )
                .map(|_| ())
        },
        |store| {
            let mut settings = store.load_app_settings_summary()?;
            settings.terminal_font_size = 19;
            store.save_appearance_settings(&settings).map(|_| ())
        },
        |store| {
            store
                .save_keybindings(&std::collections::HashMap::from([(
                    "copy".into(),
                    "ctrl-shift-c".into(),
                )]))
                .map(|_| ())
        },
        |store| {
            let mut settings = store.load_ai_settings()?;
            settings.provider_credentials.push(serde_json::from_value(json!({
                "id":"fixture", "name":"Fixture", "provider_kind":"openai", "base_url":"https://example.test/v1/"
            }))?);
            store.save_ai_settings(settings).map(|_| ())
        },
        |store| {
            store
                .save_password(serde_json::from_value(json!({
                    "id":"fixture", "name":"Fixture", "password":"fixture-secret"
                }))?)
                .map(|_| ())
        },
        |store| {
            store
                .remember_vnc_known_host(
                    "example.test",
                    5900,
                    &format!("SHA256:{}", "ab".repeat(32)),
                    None,
                )
                .map(|_| ())
        },
        |store| {
            let mut settings = store.load_translation_settings()?;
            settings.target_language = "en".into();
            store.save_translation_settings(settings).map(|_| ())
        },
    ];
    fixture.assert_skipped();
    for &update in updates {
        fixture
            .store
            .request_mutation_fn(StoreDomain::Settings, update)
            .unwrap();
        fixture.assert_skipped();
    }
}

#[test]
fn v4_change_preflight_preserves_edits_deletions_and_edits_reverted_to_the_baseline() {
    let fixture = Fixture::new(SyncScope::default());
    fixture
        .store
        .request_mutation_fn(StoreDomain::Connections, |store| {
            let mut connection = store.get_connection("server")?.unwrap();
            connection.name = "Edited".into();
            store.save_connection(&connection)
        })
        .unwrap();
    assert!(fixture.needs_remote());
    fixture
        .store
        .request_mutation_fn(StoreDomain::Connections, |store| {
            let mut connection = store.get_connection("server")?.unwrap();
            connection.name = "Server".into();
            store.save_connection(&connection)
        })
        .unwrap();
    assert!(
        fixture.needs_remote(),
        "the edit journal preserves reverted writes"
    );
    fixture.establish_baseline();
    fixture.assert_skipped();
    fixture
        .store
        .request_mutation_fn(StoreDomain::Connections, |store| {
            store.delete_connection("server")
        })
        .unwrap();
    assert!(fixture.needs_remote());
}

#[test]
fn fixed_preflight_detects_core_changes_and_ignores_old_optional_switches() {
    type Update = fn(&ConnectionStore) -> Result<(), StorageError>;
    let cases: &[(SyncScope, Update)] = &[
        (SyncScope::default(), |store| {
            store
                .upsert_quick_command(
                    serde_json::from_value(json!({
                        "id":"quick", "label":"Quick", "command":"pwd"
                    }))?,
                    None,
                )
                .map(|_| ())
        }),
        (SyncScope::default(), |store| {
            store
                .create_note(None, Some("Fixture".into()), Some("Text".into()))
                .map(|_| ())
        }),
        (SyncScope::default(), |store| {
            store.replace_proxies(&[serde_json::from_value(json!({
                "id":"proxy", "name":"Proxy", "host":"proxy.example", "port":1080
            }))?])
        }),
        (SyncScope::default(), |store| {
            store.replace_tunnels(&[serde_json::from_value(json!({
            "id":"tunnel", "name":"Tunnel", "connection_id":"server", "listen_port":1234, "target_port":22
        }))?])
        }),
        (
            SyncScope {
                preferences: true,
                ..Default::default()
            },
            |store| {
                let mut settings = store.load_app_settings_summary()?;
                settings.terminal_font_size = 19;
                store.save_appearance_settings(&settings).map(|_| ())
            },
        ),
        (
            SyncScope {
                keybindings: true,
                ..Default::default()
            },
            |store| {
                store
                    .save_keybindings(&std::collections::HashMap::from([(
                        "copy".into(),
                        "ctrl-shift-c".into(),
                    )]))
                    .map(|_| ())
            },
        ),
        (
            SyncScope {
                ai: true,
                ..Default::default()
            },
            |store| {
                let mut settings = store.load_ai_settings()?;
                settings.provider_credentials.push(serde_json::from_value(json!({
                "id":"fixture", "name":"Fixture", "provider_kind":"openai", "base_url":"https://example.test/v1/"
            }))?);
                store.save_ai_settings(settings).map(|_| ())
            },
        ),
        (
            SyncScope {
                secrets: true,
                ..Default::default()
            },
            |store| {
                store
                    .save_password(serde_json::from_value(json!({
                        "id":"fixture", "name":"Fixture", "password":"fixture-secret"
                    }))?)
                    .map(|_| ())
            },
        ),
        (
            SyncScope {
                trusted_hosts: true,
                ..Default::default()
            },
            |store| {
                store
                    .remember_vnc_known_host(
                        "example.test",
                        5900,
                        &format!("SHA256:{}", "ab".repeat(32)),
                        None,
                    )
                    .map(|_| ())
            },
        ),
    ];
    for (scope, update) in cases {
        let fixture = Fixture::new(scope.clone());
        fixture.assert_skipped();
        fixture
            .store
            .request_mutation_fn(StoreDomain::Settings, *update)
            .unwrap();
        if *scope != SyncScope::default() {
            assert!(!fixture.needs_remote());
            fixture.assert_skipped();
            continue;
        }
        assert!(fixture.needs_remote());
        assert!(
            preview_on_change(
                &fixture.store,
                &fixture.settings,
                &fixture.options,
                false,
                &CloudSyncOperationControl::default(),
            )
            .is_err(),
            "an in-scope change must reach the production adapter"
        );
    }
}

#[test]
fn v4_change_preflight_keeps_initial_scope_publication_and_deferred_work() {
    let fixture = Fixture::new(SyncScope::default());
    let view = local::load(&fixture.store, &fixture.settings, &fixture.options).unwrap();
    let variants: &[fn(&mut nyaterm_store::sync_v4::LocalView)] = &[
        |view| view.record.replica = None,
        |view| view.record.migration_approved = false,
        |view| view.record.scope.preferences = true,
        |view| view.record.published_document = None,
        |view| {
            view.record.pending = Some(
                PendingPublication::new(view.record.writer, 1, vec![1, 2, 3], "document".into())
                    .unwrap(),
            )
        },
        |view| {
            view.record.deferred.push(SyncFieldId::new(
                SyncResourceId::new("connection", "server"),
                "endpoint",
            ))
        },
    ];
    for update in variants {
        let mut variant = local::load(&fixture.store, &fixture.settings, &fixture.options).unwrap();
        update(&mut variant);
        assert!(
            local::needs_remote(&variant, &nyaterm_core::sync_schema::FIXED_CLOUD_SYNC_SCOPE)
                .unwrap()
        );
    }
    assert!(
        !local::needs_remote(&view, &nyaterm_core::sync_schema::FIXED_CLOUD_SYNC_SCOPE).unwrap()
    );
    assert!(
        preview_on_change(
            &fixture.store,
            &fixture.settings,
            &fixture.options,
            true,
            &CloudSyncOperationControl::default(),
        )
        .is_err(),
        "known remote work must reach the production adapter"
    );
}

#[test]
fn v4_preflight_view_still_rejects_a_concurrent_local_write_at_apply() {
    let fixture = Fixture::new(SyncScope::default());
    let view = local::load(&fixture.store, &fixture.settings, &fixture.options).unwrap();
    fixture
        .store
        .request_fn(StoreDomain::Connections, |store| {
            let mut connection = store.get_connection("server")?.unwrap();
            connection.name = "Saved during preflight".into();
            store.save_connection(&connection)
        })
        .unwrap();
    let remote = LocalDirectoryRemote::new(fixture.options.remote_dir.clone());
    let preview = super::prepare_remote_with_view(
        &fixture.store,
        &fixture.settings,
        &fixture.options,
        &remote,
        false,
        view,
    )
    .unwrap();
    assert!(matches!(
        fixture
            .store
            .request_fn(StoreDomain::CloudSync, move |store| {
                store.apply_v4_plan(preview.apply, "password")
            }),
        Err(StoreClientError::Operation(error)) if error.category() == "sync_local_changed"
    ));
    let saved = fixture
        .store
        .request_fn(StoreDomain::Connections, |store| {
            store.get_connection("server")
        })
        .unwrap()
        .unwrap();
    assert_eq!(saved.name, "Saved during preflight");
}
