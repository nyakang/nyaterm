//! Credentials embedded in a connection or provider are bound to the portable
//! destination. A remotely changed destination cannot inherit a local secret.
use std::collections::{BTreeMap, BTreeSet};

use nyaterm_core::sync_schema::secrets::{EncryptedSecretValue, SecretValueCodec};
use nyaterm_core::sync_schema::{
    SyncProjection, SyncResourceId, SyncValue, SyncValues, project_record,
};
use nyaterm_core::{CredentialCrypto, SavedConnection};
use redb::ReadableTable;
use serde_json::{Value, json};

use crate::storage::cloud_sync_v4::invalid;
use crate::storage::cloud_sync_v4::secrets::SecretJson;
use crate::storage::{
    CONNECTION_PASSWORD_PREFIX, CONNECTION_PREFIX, CONNECTIONS_TABLE, CREDENTIALS_TABLE,
    ConnectionStore, PROXIES_TABLE, PROXY_PREFIX, PROXY_SECRET_PREFIX, StorageError,
    current_time_ms, entity_key, write_json_in_txn,
};

pub(super) const CATALOG: &str = "v4_bound_secret_catalog";

fn binding(kind: &str, source: &Value) -> Result<Value, StorageError> {
    let normalized = match kind {
        "connection" => {
            serde_json::to_value(serde_json::from_value::<SavedConnection>(source.clone())?)?
        }
        "proxy" => serde_json::to_value(serde_json::from_value::<nyaterm_core::ProxyConfig>(
            source.clone(),
        )?)?,
        "ai_provider" => serde_json::to_value(serde_json::from_value::<
            nyaterm_core::AiProviderCredential,
        >(source.clone())?)?,
        "ai_profile" => serde_json::to_value(serde_json::from_value::<
            nyaterm_core::AiProviderProfile,
        >(source.clone())?)?,
        _ => return Err(invalid()),
    };
    let projected = project_record(kind, &normalized).map_err(|_| invalid())?;
    Ok(if kind.starts_with("ai_") {
        projected["transport"].clone()
    } else {
        json!({"endpoint":projected["endpoint"],"authentication":projected.get("authentication")})
    })
}

fn decrypt(
    value: &Value,
    crypto: &CredentialCrypto,
    token: Option<&str>,
) -> Result<Value, StorageError> {
    match value {
        Value::Null => Ok(Value::Null),
        Value::String(value) if value.is_empty() => Ok(Value::Null),
        Value::String(value) => Ok(Value::String(
            crypto.decrypt_secret(token.ok_or(StorageError::MissingMasterKey)?, value)?,
        )),
        _ => Err(invalid()),
    }
}

#[allow(clippy::too_many_arguments)]
fn observe(
    resource: SyncResourceId,
    payload: SecretJson,
    encrypted: Value,
    codec: &SecretValueCodec<'_>,
    projection: &mut SyncProjection,
    objects: &mut BTreeMap<String, EncryptedSecretValue>,
    catalog: &mut Value,
    materializations: &mut Value,
) -> Result<(), StorageError> {
    let value = SyncValue::new(&payload.0);
    let reference = codec.reference(&resource, &value)?;
    if !objects.contains_key(&reference.version) {
        objects.insert(reference.version.clone(), codec.seal(&resource, &value)?);
    }
    projection
        .record(&resource.kind, &resource.id, &json!({"value":reference}))
        .map_err(|_| invalid())?;
    materializations[&reference.version] = encrypted;
    catalog[&resource.kind][&resource.id] = json!({"value":reference});
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn attach(
    store: &ConnectionStore,
    read_local_slots: bool,
    entities: &mut BTreeMap<String, String>,
    crypto: &CredentialCrypto,
    original_token: Option<&str>,
    token: &str,
    codec: &SecretValueCodec<'_>,
    projection: &mut SyncProjection,
    objects: &mut BTreeMap<String, EncryptedSecretValue>,
    materializations: &mut Value,
) -> Result<(), StorageError> {
    let mut catalog = json!({"inline_secret":{},"ai_secret":{}});
    projection
        .complete
        .extend(["inline_secret".into(), "ai_secret".into()]);
    let sessions: Value = serde_json::from_str(&entities["sessions"])?;
    for source in sessions["connections"].as_array().ok_or_else(invalid)? {
        let id = source["id"].as_str().ok_or_else(invalid)?;
        let slot: Option<Value> = if read_local_slots {
            store.read_json_table(
                CREDENTIALS_TABLE,
                &entity_key(CONNECTION_PASSWORD_PREFIX, id),
            )?
        } else {
            None
        };
        let inline = source.pointer("/auth/password").unwrap_or(&Value::Null);
        let stored = slot
            .as_ref()
            .and_then(|slot| slot.get("password"))
            .unwrap_or(inline);
        if stored.is_null() || stored.as_str() == Some("") {
            continue;
        }
        let encrypted = slot.is_some()
            || source
                .pointer("/auth/has_password")
                .and_then(Value::as_bool)
                == Some(true);
        let password = if encrypted {
            decrypt(stored, crypto, original_token)?
        } else {
            stored.clone()
        };
        let payload =
            SecretJson(json!({"binding":binding("connection", source)?,"password":password}));
        let encrypted_password = if encrypted {
            stored.clone()
        } else {
            Value::String(
                crypto
                    .encrypt_secret(token, payload.0["password"].as_str().ok_or_else(invalid)?)?,
            )
        };
        let encrypted = json!({"binding":payload.0["binding"],"password":encrypted_password});
        observe(
            SyncResourceId::new("inline_secret", format!("connection/{id}")),
            payload,
            encrypted,
            codec,
            projection,
            objects,
            &mut catalog,
            materializations,
        )?;
    }
    let proxies: Value = serde_json::from_str(&entities["proxies"])?;
    for source in proxies.as_array().ok_or_else(invalid)? {
        let stored = source.get("password").unwrap_or(&Value::Null);
        if stored.is_null() || stored.as_str() == Some("") {
            continue;
        }
        let id = source["id"].as_str().ok_or_else(invalid)?;
        let password = if let Some(ciphertext) = stored
            .as_str()
            .and_then(|value| value.strip_prefix(PROXY_SECRET_PREFIX))
        {
            decrypt(&json!(ciphertext), crypto, original_token)?
        } else {
            stored.clone()
        };
        let payload = SecretJson(json!({"binding":binding("proxy", source)?,"password":password}));
        let encrypted_password = if stored
            .as_str()
            .is_some_and(|value| value.starts_with(PROXY_SECRET_PREFIX))
        {
            stored.clone()
        } else {
            json!(format!(
                "{PROXY_SECRET_PREFIX}{}",
                crypto
                    .encrypt_secret(token, payload.0["password"].as_str().ok_or_else(invalid)?)?
            ))
        };
        let encrypted = json!({"binding":payload.0["binding"],"password":encrypted_password});
        observe(
            SyncResourceId::new("inline_secret", format!("proxy/{id}")),
            payload,
            encrypted,
            codec,
            projection,
            objects,
            &mut catalog,
            materializations,
        )?;
    }
    let settings: Value = serde_json::from_str(&entities["settings"])?;
    for (collection, kind) in [
        ("provider_credentials", "ai_provider"),
        ("provider_profiles", "ai_profile"),
    ] {
        let Some(sources) = settings.pointer(&format!("/ai/{collection}")) else {
            continue;
        };
        for source in sources.as_array().ok_or_else(invalid)? {
            let key = source.get("api_key").unwrap_or(&Value::Null);
            let headers = source
                .get("extra_headers")
                .cloned()
                .unwrap_or_else(|| json!([]));
            if (key.is_null() || key.as_str() == Some(""))
                && headers.as_array().is_some_and(Vec::is_empty)
            {
                continue;
            }
            let Ok(destination) = binding(kind, source) else {
                projection.complete.remove("ai_secret");
                continue;
            };
            let id = source["id"].as_str().ok_or_else(invalid)?;
            let mut payload = SecretJson(
                json!({"binding":destination,"api_key":decrypt(key, crypto, original_token)?,"extra_headers":[]}),
            );
            let mut encrypted_headers = Vec::new();
            for header in headers.as_array().ok_or_else(invalid)? {
                let value = header.get("value").unwrap_or(&Value::Null);
                payload.0["extra_headers"].as_array_mut().ok_or_else(invalid)?.push(json!({"id":header["id"],"name":header["name"],"value":decrypt(value, crypto, original_token)?}));
                encrypted_headers
                    .push(json!({"id":header["id"],"name":header["name"],"value":value}));
            }
            let encrypted = json!({"binding":payload.0["binding"],"api_key":key,"extra_headers":encrypted_headers});
            observe(
                SyncResourceId::new("ai_secret", format!("{collection}/{id}")),
                payload,
                encrypted,
                codec,
                projection,
                objects,
                &mut catalog,
                materializations,
            )?;
        }
    }
    entities.insert(CATALOG.into(), catalog.to_string());
    Ok(())
}

pub(super) fn encrypt_headers(
    payload: &Value,
    crypto: &CredentialCrypto,
    token: &str,
) -> Result<Value, StorageError> {
    let mut result = Vec::new();
    let mut seen = BTreeSet::new();
    for header in payload.as_array().ok_or_else(invalid)? {
        let id = header["id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .ok_or_else(invalid)?;
        let name = header["name"].as_str().ok_or_else(invalid)?;
        let value = header["value"].as_str().unwrap_or_default();
        if !seen.insert(id) || name.contains(['\r', '\n']) || value.contains(['\r', '\n']) {
            return Err(invalid());
        }
        result.push(json!({"id":id,"name":name,"value":crypto.encrypt_secret(token, value)?}));
    }
    Ok(Value::Array(result))
}

fn target_mut<'a>(
    entities: &'a mut BTreeMap<String, Value>,
    resource: &SyncResourceId,
) -> Result<Option<(&'static str, &'a mut Value)>, StorageError> {
    let (category, id) = resource.id.split_once('/').ok_or_else(invalid)?;
    let (records, kind) = match (resource.kind.as_str(), category) {
        ("inline_secret", "connection") => (
            &mut entities.get_mut("sessions").ok_or_else(invalid)?["connections"],
            "connection",
        ),
        ("inline_secret", "proxy") => (entities.get_mut("proxies").ok_or_else(invalid)?, "proxy"),
        ("ai_secret", "provider_credentials") => (
            &mut entities.get_mut("settings").ok_or_else(invalid)?["ai"]["provider_credentials"],
            "ai_provider",
        ),
        ("ai_secret", "provider_profiles") => (
            &mut entities.get_mut("settings").ok_or_else(invalid)?["ai"]["provider_profiles"],
            "ai_profile",
        ),
        _ => return Err(invalid()),
    };
    if records.is_null() {
        return Ok(None);
    }
    Ok(records
        .as_array_mut()
        .ok_or_else(invalid)?
        .iter_mut()
        .find(|record| record["id"] == id)
        .map(|record| (kind, record)))
}

pub(super) fn reconstruct(
    entities: &mut BTreeMap<String, Value>,
    values: &SyncValues,
    materializations: &Value,
) -> Result<(), StorageError> {
    let original = entities.get(CATALOG).cloned().ok_or_else(invalid)?;
    let mut catalog = json!({"inline_secret":{},"ai_secret":{}});
    for kind in ["inline_secret", "ai_secret"] {
        let desired = crate::storage::cloud_sync_v4::records(values, kind)?;
        for id in original[kind]
            .as_object()
            .ok_or_else(invalid)?
            .keys()
            .filter(|id| !desired.contains_key(*id))
        {
            let resource = SyncResourceId::new(kind, id);
            if let Some((target_kind, target)) = target_mut(entities, &resource)? {
                if target_kind == "connection" {
                    if target.get("auth").is_some_and(Value::is_object) {
                        target["auth"]["password"] = Value::Null;
                        target["auth"]["has_password"] = json!(false);
                    }
                } else if target_kind == "proxy" {
                    target["password"] = Value::Null;
                } else {
                    target["api_key"] = Value::Null;
                    target["extra_headers"] = json!([]);
                }
            }
        }
        for (id, record) in desired {
            let resource = SyncResourceId::new(kind, &id);
            let reference: nyaterm_core::sync_schema::secrets::SecretReference =
                serde_json::from_value(record["value"].clone())?;
            if reference.resource != resource {
                return Err(invalid());
            }
            let encrypted = materializations
                .get(&reference.version)
                .ok_or_else(invalid)?;
            let (target_kind, target) = target_mut(entities, &resource)?.ok_or_else(invalid)?;
            if binding(target_kind, target)? != encrypted["binding"] {
                return Err(invalid());
            }
            if target_kind == "connection" {
                if !target.get("auth").is_some_and(Value::is_object) {
                    target["auth"] = json!({});
                }
                target["auth"]["password"] = encrypted["password"].clone();
                target["auth"]["has_password"] = json!(true);
            } else if target_kind == "proxy" {
                let password = encrypted["password"].as_str().ok_or_else(invalid)?;
                target["password"] = if password.starts_with(PROXY_SECRET_PREFIX) {
                    json!(password)
                } else {
                    json!(format!("{PROXY_SECRET_PREFIX}{password}"))
                };
            } else {
                target["api_key"] = encrypted["api_key"].clone();
                let mut headers = encrypted["extra_headers"].clone();
                if let (Some(old), Some(next)) = (
                    target.get("extra_headers").and_then(Value::as_array),
                    headers.as_array_mut(),
                ) {
                    for header in next {
                        if let Some(original) = old.iter().find(|old| old["id"] == header["id"]) {
                            let mut merged = original.as_object().cloned().ok_or_else(invalid)?;
                            merged.extend(header.as_object().cloned().ok_or_else(invalid)?);
                            *header = Value::Object(merged);
                        }
                    }
                }
                target["extra_headers"] = headers;
            }
            catalog[kind][id] = record;
        }
    }
    entities.insert(CATALOG.into(), catalog);
    Ok(())
}

pub(super) fn persist(
    txn: &redb::WriteTransaction,
    original: &BTreeMap<String, String>,
    entities: &BTreeMap<String, Value>,
) -> Result<(), StorageError> {
    let old: Value = serde_json::from_str(&original[CATALOG])?;
    let next = &entities[CATALOG];
    let ids = old["inline_secret"]
        .as_object()
        .ok_or_else(invalid)?
        .keys()
        .chain(
            next["inline_secret"]
                .as_object()
                .ok_or_else(invalid)?
                .keys(),
        )
        .collect::<BTreeSet<_>>();
    for id in ids {
        if old["inline_secret"].get(id) == next["inline_secret"].get(id) {
            continue;
        }
        let (category, local_id) = id.split_once('/').ok_or_else(invalid)?;
        let (entity, section, table, prefix) = match category {
            "connection" => (
                "sessions",
                "connections",
                CONNECTIONS_TABLE,
                CONNECTION_PREFIX,
            ),
            "proxy" => ("proxies", "", PROXIES_TABLE, PROXY_PREFIX),
            _ => return Err(invalid()),
        };
        let records = if section.is_empty() {
            &entities[entity]
        } else {
            &entities[entity][section]
        };
        let desired = records
            .as_array()
            .ok_or_else(invalid)?
            .iter()
            .find(|record| record["id"] == local_id);
        let key = entity_key(prefix, local_id);
        let current = txn
            .open_table(table)?
            .get(key.as_str())?
            .map(|raw| serde_json::from_slice::<Value>(raw.value()))
            .transpose()?;
        if let (Some(mut current), Some(desired)) = (current, desired) {
            if category == "connection" {
                if current.get("auth").is_some_and(Value::is_object) {
                    current["auth"]["password"] = Value::Null;
                    current["auth"]["has_password"] = json!(false);
                }
                let password = desired
                    .pointer("/auth/password")
                    .cloned()
                    .unwrap_or(Value::Null);
                let slot_key = entity_key(CONNECTION_PASSWORD_PREFIX, local_id);
                if password.is_null() {
                    crate::storage::cloud_sync_v4::record_remove(
                        txn,
                        CREDENTIALS_TABLE,
                        &slot_key,
                    )?;
                    txn.open_table(CREDENTIALS_TABLE)?
                        .remove(slot_key.as_str())?;
                } else {
                    let mut slot = txn.open_table(CREDENTIALS_TABLE)?.get(slot_key.as_str())?.map(|raw| serde_json::from_slice::<Value>(raw.value())).transpose()?.unwrap_or_else(|| json!({"id":local_id,"connection_id":local_id,"created_at_ms":current_time_ms()}));
                    slot["password"] = password;
                    slot["updated_at_ms"] = json!(current_time_ms());
                    write_json_in_txn(txn, CREDENTIALS_TABLE, &slot_key, &slot)?;
                }
            } else {
                current["password"] = desired["password"].clone();
            }
            write_json_in_txn(txn, table, &key, &current)?;
        }
    }
    Ok(())
}
