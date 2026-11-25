//! Native component preferences have their own contract; V1 plugin preferences stay unchanged.
use nyaterm_core::plugins::native::{NativePreferences, SCHEMA};
use nyaterm_core::plugins::{ErrorCode, PluginError, PluginResult};
use redb::{Database, ReadableDatabase, TableDefinition};
use std::path::Path;

const TABLE: TableDefinition<&str, &str> = TableDefinition::new("native_components_v1");
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
        let value: NativePreferences = serde_json::from_str(raw.value()).map_err(|_| error())?;
        if value.schema_version != SCHEMA {
            return Err(error());
        }
        Ok(value)
    }
    pub fn save(&self, value: &NativePreferences) -> PluginResult<()> {
        if value.schema_version != SCHEMA {
            return Err(error());
        }
        let raw = serde_json::to_string(value).map_err(|_| error())?;
        let transaction = self.database.begin_write().map_err(|_| error())?;
        transaction
            .open_table(TABLE)
            .map_err(|_| error())?
            .insert("preferences", raw.as_str())
            .map_err(|_| error())?;
        transaction.commit().map_err(|_| error())
    }
}

#[cfg(test)]
mod tests {
    use crate::native_component_preferences::NativePreferenceStore;
    use nyaterm_core::plugins::native::{NativeComponentId, NativePreference};
    use nyaterm_core::test_support::TestTempDir;
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
