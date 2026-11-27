//! Native component preferences have their own contract; V1 plugin preferences stay unchanged.
use nyaterm_core::plugins::native::catalog::NativeSelection;
use nyaterm_core::plugins::native::{
    NativeComponentId, NativePreference, NativePreferences, SCHEMA,
};
use nyaterm_core::plugins::{ErrorCode, PluginError, PluginResult};
use redb::{Database, ReadableDatabase, TableDefinition};
use std::path::Path;

const TABLE: TableDefinition<&str, &str> = TableDefinition::new("native_components_v1");
const SELECTIONS: TableDefinition<&str, &str> =
    TableDefinition::new("native_component_selections_v1");
pub struct NativePreferenceStore {
    database: Database,
}
fn error() -> PluginError {
    PluginError::new(
        ErrorCode::Storage,
        "Cannot read or save native component preferences; preserve the existing database",
    )
}
impl NativePreferenceStore {
    pub fn selection(&self, identity: &str) -> PluginResult<NativeSelection> {
        let transaction = self.database.begin_read().map_err(|_| error())?;
        let table = match transaction.open_table(SELECTIONS) {
            Ok(table) => table,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(NativeSelection::default()),
            Err(_) => return Err(error()),
        };
        let Some(raw) = table.get(identity).map_err(|_| error())? else {
            return Ok(NativeSelection::default());
        };
        if raw.value().len() > 4096 {
            return Err(error());
        }
        let selection: NativeSelection = serde_json::from_str(raw.value()).map_err(|_| error())?;
        selection.validate()?;
        Ok(selection)
    }

    pub fn save_selection(&self, identity: &str, selection: &NativeSelection) -> PluginResult<()> {
        selection.validate()?;
        let raw = serde_json::to_string(selection).map_err(|_| error())?;
        let transaction = self.database.begin_write().map_err(|_| error())?;
        transaction
            .open_table(SELECTIONS)
            .map_err(|_| error())?
            .insert(identity, raw.as_str())
            .map_err(|_| error())?;
        transaction.commit().map_err(|_| error())
    }
    pub fn open(path: &Path) -> PluginResult<Self> {
        let store = Self {
            database: Database::create(path).map_err(|_| error())?,
        };
        store.load()?;
        Ok(store)
    }
    pub fn load(&self) -> PluginResult<NativePreferences> {
        let transaction = self.database.begin_read().map_err(|_| error())?;
        let table = match transaction.open_table(TABLE) {
            Ok(table) => table,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(NativePreferences::default()),
            Err(_) => return Err(error()),
        };
        let Some(raw) = table.get("preferences").map_err(|_| error())? else {
            return Ok(NativePreferences::default());
        };
        if raw.value().len() > 16384 {
            return Err(error());
        }
        let mut value: NativePreferences =
            serde_json::from_str(raw.value()).map_err(|_| error())?;
        if value.schema_version != SCHEMA {
            return Err(error());
        }
        if let Some(raw) = table.get("preferences.x11.v1").map_err(|_| error())? {
            if raw.value().len() > 4096 {
                return Err(error());
            }
            let preference: NativePreference =
                serde_json::from_str(raw.value()).map_err(|_| error())?;
            value.components.insert(NativeComponentId::X11, preference);
        }
        Ok(value)
    }
    pub fn save(&self, value: &NativePreferences) -> PluginResult<()> {
        if value.schema_version != SCHEMA {
            return Err(error());
        }
        // Keep the legacy document readable by clients with a closed four-ID enum.
        let mut legacy = value.clone();
        let x11 = legacy.components.remove(&NativeComponentId::X11);
        let raw = serde_json::to_string(&legacy).map_err(|_| error())?;
        let transaction = self.database.begin_write().map_err(|_| error())?;
        transaction
            .open_table(TABLE)
            .map_err(|_| error())?
            .insert("preferences", raw.as_str())
            .map_err(|_| error())?;
        if let Some(x11) = x11 {
            transaction
                .open_table(TABLE)
                .map_err(|_| error())?
                .insert(
                    "preferences.x11.v1",
                    serde_json::to_string(&x11).map_err(|_| error())?.as_str(),
                )
                .map_err(|_| error())?;
        } else {
            transaction
                .open_table(TABLE)
                .map_err(|_| error())?
                .remove("preferences.x11.v1")
                .map_err(|_| error())?;
        }
        transaction.commit().map_err(|_| error())
    }
}

#[cfg(test)]
mod tests {
    use crate::native_component_preferences::NativePreferenceStore;
    use nyaterm_core::plugins::native::{NativeComponentId, NativePreference};
    use nyaterm_core::test_support::TestTempDir;
    #[test]
    fn x11_preferences_do_not_break_old_client_documents() {
        use crate::native_component_preferences::TABLE;
        use redb::ReadableDatabase;
        let root = TestTempDir::new("nyaterm-native-x11-preferences");
        std::fs::create_dir_all(root.path()).unwrap();
        let store = NativePreferenceStore::open(&root.join("preferences.redb")).unwrap();
        let mut preferences = store.load().unwrap();
        preferences.components.insert(
            NativeComponentId::X11,
            NativePreference {
                disabled: true,
                uninstall_pending: false,
            },
        );
        preferences.components.insert(
            NativeComponentId::Vnc,
            NativePreference {
                disabled: false,
                uninstall_pending: false,
            },
        );
        store.save(&preferences).unwrap();
        let transaction = store.database.begin_read().unwrap();
        let table = transaction.open_table(TABLE).unwrap();
        let raw = table.get("preferences").unwrap().unwrap();
        let old: serde_json::Value = serde_json::from_str(raw.value()).unwrap();
        assert_eq!(
            old["components"]
                .as_object()
                .unwrap()
                .keys()
                .collect::<Vec<_>>(),
            vec!["vnc"]
        );
        assert!(table.get("preferences.x11.v1").unwrap().is_some());
        assert_eq!(store.load().unwrap(), preferences);
        drop(raw);
        drop(table);
        drop(transaction);
        // An older client updates only its known document; X11 state survives.
        let write = store.database.begin_write().unwrap();
        write
            .open_table(TABLE)
            .unwrap()
            .insert("preferences", "{\"schema_version\":1,\"components\":{}}")
            .unwrap();
        write.commit().unwrap();
        assert!(store.load().unwrap().components[&NativeComponentId::X11].disabled);
    }
    #[test]
    fn version_selection_is_atomic_and_does_not_replace_existing_preferences() {
        use nyaterm_core::plugins::native::catalog::{InstalledComponent, NativeSelection};
        let root = TestTempDir::new("nyaterm-native-selection");
        std::fs::create_dir_all(root.path()).unwrap();
        let path = root.join("native.redb");
        let store = NativePreferenceStore::open(&path).unwrap();
        let mut preferences = store.load().unwrap();
        preferences.components.insert(
            NativeComponentId::Vnc,
            NativePreference {
                disabled: true,
                uninstall_pending: false,
            },
        );
        store.save(&preferences).unwrap();
        let identity = "vnc/x86_64-pc-windows-msvc/windows-msvc";
        let selection = NativeSelection {
            active: Some(InstalledComponent {
                version: "1.1.0".into(),
                digest: "a".repeat(64),
            }),
            previous: None,
        };
        store.save_selection(identity, &selection).unwrap();
        let invalid = NativeSelection {
            active: Some(InstalledComponent {
                version: "../bad".into(),
                digest: "b".repeat(64),
            }),
            previous: None,
        };
        assert!(store.save_selection(identity, &invalid).is_err());
        drop(store);
        let reopened = NativePreferenceStore::open(&path).unwrap();
        assert_eq!(reopened.load().unwrap(), preferences);
        assert_eq!(reopened.selection(identity).unwrap(), selection);
        assert!(
            reopened
                .selection("vnc/another-target/runtime")
                .unwrap()
                .active
                .is_none()
        );
    }
    #[test]
    fn native_preferences_round_trip_without_changing_plugin_preferences() {
        let root = TestTempDir::new("nyaterm-native-preferences");
        std::fs::create_dir_all(root.path()).unwrap();
        let path = root.join("native.redb");
        let store = NativePreferenceStore::open(&path).unwrap();
        let mut prefs = store.load().unwrap();
        prefs.components.insert(
            NativeComponentId::Spice,
            NativePreference {
                disabled: true,
                uninstall_pending: false,
            },
        );
        store.save(&prefs).unwrap();
        drop(store);
        assert_eq!(
            NativePreferenceStore::open(&path).unwrap().load().unwrap(),
            prefs
        );
    }
    #[test]
    fn unknown_preference_schema_is_preserved_and_not_reinitialized() {
        use redb::TableDefinition;
        let root = TestTempDir::new("nyaterm-native-unknown-preferences");
        std::fs::create_dir_all(root.path()).unwrap();
        let path = root.join("preferences.redb");
        let database = redb::Database::create(&path).unwrap();
        let transaction = database.begin_write().unwrap();
        let raw = r#"{"schema_version":2,"components":{},"future":true}"#;
        transaction
            .open_table(TableDefinition::<&str, &str>::new("native_components_v1"))
            .unwrap()
            .insert("preferences", raw)
            .unwrap();
        transaction.commit().unwrap();
        drop(database);
        assert!(NativePreferenceStore::open(&path).is_err());
        use redb::ReadableDatabase;
        let database = redb::Database::open(&path).unwrap();
        let transaction = database.begin_read().unwrap();
        let table = transaction
            .open_table(TableDefinition::<&str, &str>::new("native_components_v1"))
            .unwrap();
        assert_eq!(table.get("preferences").unwrap().unwrap().value(), raw);
    }
}
