//! Local-only extension grants, isolated from legacy preferences and sync documents.
use nyaterm_core::plugins::extension::Authorization;
use nyaterm_core::plugins::{ErrorCode, PluginError, PluginResult};
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
use std::path::Path;

const GRANTS: TableDefinition<&str, &str> = TableDefinition::new("extension_authorizations_v2");

pub struct AuthorizationStore(Database);

fn failure() -> PluginError {
    PluginError::new(ErrorCode::Storage, "Cannot access extension authorizations")
}

impl AuthorizationStore {
    pub fn open(path: &Path) -> PluginResult<Self> {
        Database::create(path).map(Self).map_err(|_| failure())
    }

    pub fn load(&self, identity: &str) -> PluginResult<Option<Authorization>> {
        let transaction = self.0.begin_read().map_err(|_| failure())?;
        let table = match transaction.open_table(GRANTS) {
            Ok(table) => table,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(_) => return Err(failure()),
        };
        table
            .get(identity)
            .map_err(|_| failure())?
            .map(|value| {
                if value.value().len() > 128 * 1024 {
                    return Err(failure());
                }
                serde_json::from_str(value.value()).map_err(|_| failure())
            })
            .transpose()
    }

    pub fn save(&self, identity: &str, authorization: &Authorization) -> PluginResult<()> {
        let value = serde_json::to_string(authorization).map_err(|_| failure())?;
        if value.len() > 128 * 1024 {
            return Err(failure());
        }
        let transaction = self.0.begin_write().map_err(|_| failure())?;
        transaction
            .open_table(GRANTS)
            .map_err(|_| failure())?
            .insert(identity, value.as_str())
            .map_err(|_| failure())?;
        transaction.commit().map_err(|_| failure())
    }

    /// Revoke every reviewed build, so reinstalling an older build cannot restore grants.
    pub fn revoke_plugin(&self, plugin_id: &str) -> PluginResult<()> {
        let transaction = self.0.begin_write().map_err(|_| failure())?;
        {
            let mut table = transaction.open_table(GRANTS).map_err(|_| failure())?;
            let mut identities = Vec::new();
            for entry in table.iter().map_err(|_| failure())? {
                let (key, value) = entry.map_err(|_| failure())?;
                let authorization: Authorization =
                    serde_json::from_str(value.value()).map_err(|_| failure())?;
                if authorization.plugin_id == plugin_id {
                    identities.push(key.value().to_owned());
                }
            }
            for identity in identities {
                table.remove(identity.as_str()).map_err(|_| failure())?;
            }
        }
        transaction.commit().map_err(|_| failure())
    }
}

#[cfg(test)]
mod tests {
    use super::{AuthorizationStore, GRANTS};
    use nyaterm_core::plugins::extension::{Authorization, Capability, Grant, Scope};
    use nyaterm_core::test_support::TestTempDir;

    #[test]
    fn grants_survive_reopen_and_revocation_covers_all_builds_only_for_that_plugin() {
        let root = TestTempDir::new("nyaterm-extension-grants");
        std::fs::create_dir_all(root.path()).unwrap();
        let path = root.join("grants.redb");
        let auth = Authorization {
            plugin_id: "one".into(),
            source: "digest".into(),
            grants: vec![Grant {
                capability: Capability::Network,
                scope: Scope::Origin {
                    origin: "https://example.com".into(),
                },
            }],
        };
        {
            let store = AuthorizationStore::open(&path).unwrap();
            store.save("build-one", &auth).unwrap();
            store.save("build-two", &auth).unwrap();
            store
                .save(
                    "other",
                    &Authorization {
                        plugin_id: "two".into(),
                        ..auth.clone()
                    },
                )
                .unwrap();
        }
        let store = AuthorizationStore::open(&path).unwrap();
        assert_eq!(store.load("build-one").unwrap(), Some(auth));
        store.revoke_plugin("one").unwrap();
        assert!(store.load("build-one").unwrap().is_none());
        assert!(store.load("build-two").unwrap().is_none());
        assert!(store.load("other").unwrap().is_some());
    }

    #[test]
    fn unsupported_grant_fields_fail_closed_without_overwriting_data() {
        let root = TestTempDir::new("nyaterm-extension-invalid-grants");
        std::fs::create_dir_all(root.path()).unwrap();
        let store = AuthorizationStore::open(&root.join("grants.redb")).unwrap();
        let transaction = store.0.begin_write().unwrap();
        transaction
            .open_table(GRANTS)
            .unwrap()
            .insert(
                "future",
                r#"{"plugin_id":"one","source":"digest","grants":[],"future":true}"#,
            )
            .unwrap();
        transaction.commit().unwrap();
        assert!(store.load("future").is_err());
        assert!(store.revoke_plugin("one").is_err());
        assert!(store.load("future").is_err());
    }
}
