use nyaterm_core::PortableSnapshotKind;
use nyaterm_core::test_support::TestTempDir;

use crate::storage::ConnectionStore;

#[test]
fn terminal_bell_loads_legacy_settings_and_round_trips_both_values() {
    let root = TestTempDir::new("nyaterm-terminal-bell-settings");
    let store = ConnectionStore::open(root.path()).unwrap();
    store
        .save_settings_value(&serde_json::json!({
            "terminal": {"scrollback_lines": 1000, "future": {"keep": true}},
            "future_section": [1, 2]
        }))
        .unwrap();
    let mut settings = store.load_app_settings_summary().unwrap();
    assert!(settings.terminal_bell_enabled);
    for enabled in [false, true] {
        settings.terminal_bell_enabled = enabled;
        store.save_terminal_settings(&settings).unwrap();
        assert_eq!(
            store
                .load_app_settings_summary()
                .unwrap()
                .terminal_bell_enabled,
            enabled
        );
        let raw = store.load_settings_value().unwrap();
        assert_eq!(raw["terminal"]["bell_enabled"], enabled);
        assert_eq!(raw["terminal"]["future"], serde_json::json!({"keep": true}));
        assert_eq!(raw["future_section"], serde_json::json!([1, 2]));
    }
    drop(store);
    assert!(
        ConnectionStore::open(root.path())
            .unwrap()
            .load_app_settings_summary()
            .unwrap()
            .terminal_bell_enabled
    );
}

#[test]
fn terminal_bell_preserves_unsupported_values_during_unrelated_saves() {
    let root = TestTempDir::new("nyaterm-terminal-bell-future");
    let store = ConnectionStore::open(root.path()).unwrap();
    store
        .save_settings_value(&serde_json::json!({
            "terminal": {"bell_enabled": {"mode": "future"}}
        }))
        .unwrap();
    let mut settings = store.load_app_settings_summary().unwrap();
    settings.terminal_scrollback_lines = 3000;
    store.save_terminal_settings(&settings).unwrap();
    assert_eq!(
        store.load_settings_value().unwrap()["terminal"]["bell_enabled"],
        serde_json::json!({"mode": "future"})
    );
    settings.terminal_bell_enabled = false;
    store.save_terminal_settings(&settings).unwrap();
    assert!(
        !store
            .load_app_settings_summary()
            .unwrap()
            .terminal_bell_enabled
    );
}

#[test]
fn terminal_bell_preference_survives_portable_and_native_backups() {
    let source_dir = TestTempDir::new("nyaterm-terminal-bell-source");
    let portable_dir = TestTempDir::new("nyaterm-terminal-bell-portable");
    let native_dir = TestTempDir::new("nyaterm-terminal-bell-native");
    let backup_dir = TestTempDir::new("nyaterm-terminal-bell-backup");
    let backup_path = backup_dir.path().join("settings.redb");
    let source = ConnectionStore::open(source_dir.path()).unwrap();
    let mut settings = source.load_app_settings_summary().unwrap();
    settings.terminal_bell_enabled = false;
    source.save_terminal_settings(&settings).unwrap();
    let mut snapshot = source
        .build_raw_portable_snapshot(PortableSnapshotKind::Backup, "device", "test")
        .unwrap();
    snapshot.recalculate_hash().unwrap();
    let portable = ConnectionStore::open(portable_dir.path()).unwrap();
    portable.apply_raw_portable_snapshot(&snapshot).unwrap();
    assert!(
        !portable
            .load_app_settings_summary()
            .unwrap()
            .terminal_bell_enabled
    );
    drop(source);
    ConnectionStore::export_config_database(source_dir.path(), None, &backup_path).unwrap();
    ConnectionStore::import_config_database(native_dir.path(), None, &backup_path).unwrap();
    assert!(
        !ConnectionStore::open(native_dir.path())
            .unwrap()
            .load_app_settings_summary()
            .unwrap()
            .terminal_bell_enabled
    );
}
