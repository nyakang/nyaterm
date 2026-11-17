use nyaterm_core::PortableSnapshotKind;
use nyaterm_core::models::startup::{StartupMode, StartupTarget};
use nyaterm_core::test_support::TestTempDir;

use crate::storage::ConnectionStore;

#[test]
fn startup_preferences_migrate_legacy_flags_and_write_compatible_projection() {
    let dir = TestTempDir::new("nyaterm-startup-settings");
    let store = ConnectionStore::open(dir.path()).unwrap();
    for enabled in [false, true] {
        store
            .save_settings_value(&serde_json::json!({"general":{"startup_restore":enabled}}))
            .unwrap();
        let mut settings = store.load_app_settings_summary().unwrap();
        assert_eq!(settings.startup_mode.restores_last_session(), enabled);
        assert_eq!(
            store.load_workspace_startup_restore().unwrap().startup_mode,
            settings.startup_mode
        );
        for mode in [
            StartupMode::Specified,
            StartupMode::LastSession,
            StartupMode::None,
        ] {
            settings.startup_mode = mode.clone();
            settings.startup_targets =
                vec![StartupTarget::connection("c"), StartupTarget::group("g")];
            let saved = store.save_general_settings(&settings).unwrap();
            assert_eq!(saved.startup_mode, mode);
            assert_eq!(saved.startup_targets, settings.startup_targets);
            assert_eq!(
                store.load_settings_value().unwrap()["general"]["startup_restore"],
                mode.restores_last_session()
            );
        }
    }
}

#[test]
fn startup_unknown_fields_survive_general_save_and_backup_and_sync() {
    let dir = TestTempDir::new("nyaterm-startup-future");
    let store = ConnectionStore::open(dir.path()).unwrap();
    let general = serde_json::json!({
        "startup_restore": false,
        "startup_mode": "future_mode",
        "startup_targets": [{"kind":"connection","id":"c","future":42}, {"kind":"future","payload":[1,2]}],
        "future": {"retained":true}
    });
    store
        .save_settings_value(&serde_json::json!({"general":general}))
        .unwrap();
    let settings = store.load_app_settings_summary().unwrap();
    assert!(matches!(settings.startup_mode, StartupMode::Unknown(_)));
    store.save_general_settings(&settings).unwrap();
    for kind in [PortableSnapshotKind::Backup, PortableSnapshotKind::Sync] {
        let mut snapshot = store
            .build_raw_portable_snapshot(kind, "startup-fixture", "2.0.5")
            .unwrap();
        snapshot.recalculate_hash().unwrap();
        let encoded = crate::encode_raw_portable_snapshot(&snapshot).unwrap();
        let snapshot = crate::decode_raw_portable_snapshot(&encoded).unwrap();
        let target_dir = TestTempDir::new("nyaterm-startup-target");
        let target = ConnectionStore::open(target_dir.path()).unwrap();
        target.apply_raw_portable_snapshot(&snapshot).unwrap();
        let restored = target.load_app_settings_summary().unwrap();
        assert_eq!(restored.startup_mode, settings.startup_mode);
        assert_eq!(restored.startup_targets, settings.startup_targets);
        let raw = target.load_settings_value().unwrap();
        assert_eq!(raw["general"]["future"], general["future"]);
        assert_eq!(
            raw["general"]["startup_targets"],
            general["startup_targets"]
        );
    }
}

#[test]
fn startup_malformed_target_collection_is_rejected_without_overwriting() {
    let dir = TestTempDir::new("nyaterm-startup-malformed");
    let store = ConnectionStore::open(dir.path()).unwrap();
    let value = serde_json::json!({"general":{"startup_targets":{"future":42}}});
    store.save_settings_value(&value).unwrap();
    assert!(store.load_app_settings_summary().is_err());
    assert_eq!(store.load_settings_value().unwrap(), value);
}
