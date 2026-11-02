//! Read-only credential preparation and transaction-local re-encryption. Master
//! keys remain local; no key file is created or replaced during synchronization.
mod bound;
use std::collections::BTreeMap;

use nyaterm_core::sync_schema::secrets::{
    EncryptedSecretValue, SecretReference, SecretValueCodec, payload_fields, secret_fields,
};
use nyaterm_core::sync_schema::{
    SyncFieldId, SyncProjection, SyncResourceId, SyncValue, SyncValues,
};
use nyaterm_core::{CredentialCrypto, SecretString};
use redb::{ReadableTable, TableDefinition};
use serde_json::{Value, json};
use zeroize::Zeroize;

use crate::storage::cloud_sync_v4::invalid;
use crate::storage::{
    CONNECTION_PASSWORD_PREFIX, CREDENTIAL_PREFIX, CREDENTIALS_TABLE, ConnectionStore,
    LEGACY_TEXT_MASTER_KEY, META_MASTER_KEY, META_TABLE, OTP_ACCOUNTS_TABLE, OTP_PREFIX,
    PASSWORD_PREFIX, SSH_KEY_PREFIX, StorageError, TEXT_DOCS_TABLE, entity_key, write_json_in_txn,
};

const CATALOG: &str = "v4_secret_catalog";
const MATERIALIZATIONS: &str = "v4_secret_materializations";
const TOKEN: &str = "v4_secret_master_key_token";

type VaultTable = TableDefinition<'static, &'static str, &'static [u8]>;

const VAULTS: &[(&str, VaultTable, &str)] = &[
    ("key", CREDENTIALS_TABLE, SSH_KEY_PREFIX),
    ("password", CREDENTIALS_TABLE, PASSWORD_PREFIX),
    ("credential", CREDENTIALS_TABLE, CREDENTIAL_PREFIX),
    ("otp", OTP_ACCOUNTS_TABLE, OTP_PREFIX),
];

struct SecretJson(Value);

impl Drop for SecretJson {
    fn drop(&mut self) {
        fn wipe(value: &mut Value) {
            match value {
                Value::String(text) => text.zeroize(),
                Value::Array(values) => values.iter_mut().for_each(wipe),
                Value::Object(fields) => fields.values_mut().for_each(wipe),
                _ => {}
            }
        }
        wipe(&mut self.0);
    }
}

fn normalize(kind: &str, value: &Value) -> Result<Value, StorageError> {
    let mut normalized = match kind {
        "key" => serde_json::to_value(serde_json::from_value::<nyaterm_core::SshKey>(
            value.clone(),
        )?)?,
        "password" => serde_json::to_value(serde_json::from_value::<nyaterm_core::SavedPassword>(
            value.clone(),
        )?)?,
        "credential" => serde_json::to_value(serde_json::from_value::<
            nyaterm_core::SavedCredential,
        >(value.clone())?)?,
        "otp" => serde_json::to_value(serde_json::from_value::<nyaterm_core::OtpEntry>(
            value.clone(),
        )?)?,
        _ => return Err(invalid()),
    };
    // SavedPassword omits an empty username when serialized, but null is not
    // an empty username on deserialization. Preserve that default in the value
    // object so an ordinary password without a username stays syncable.
    if kind == "password" && normalized.get("username").is_none() {
        normalized["username"] = json!("");
    }
    Ok(normalized)
}

fn plaintext(
    kind: &str,
    source: &Value,
    crypto: &CredentialCrypto,
    token: Option<&str>,
) -> Result<SecretJson, StorageError> {
    let mut payload = SecretJson(json!({}));
    for field in payload_fields(kind) {
        payload.0[*field] = source.get(*field).cloned().unwrap_or(Value::Null);
    }
    for field in secret_fields(kind) {
        let value = source.get(*field).and_then(Value::as_str);
        payload.0[*field] = match value.filter(|value| !value.is_empty()) {
            Some(value) => Value::String(
                crypto.decrypt_secret(token.ok_or(StorageError::MissingMasterKey)?, value)?,
            ),
            None => Value::Null,
        };
    }
    Ok(payload)
}

pub(super) fn attach(
    store: &ConnectionStore,
    entities: &mut BTreeMap<String, String>,
    password: &str,
    objects: &mut BTreeMap<String, EncryptedSecretValue>,
) -> Result<SyncProjection, StorageError> {
    attach_source(store, entities, password, objects, None)
}

pub(super) fn legacy_projection(
    store: &ConnectionStore,
    snapshot: &nyaterm_core::RawPortableSnapshot,
    password: &str,
) -> Result<(SyncProjection, BTreeMap<String, EncryptedSecretValue>), StorageError> {
    let mut entities = snapshot.entities.clone();
    entities
        .entry("sessions".into())
        .or_insert_with(|| json!({"connections":[]}).to_string());
    entities
        .entry("proxies".into())
        .or_insert_with(|| json!([]).to_string());
    entities
        .entry("settings".into())
        .or_insert_with(|| json!({}).to_string());
    let mut objects = BTreeMap::new();
    let projection = attach_source(store, &mut entities, password, &mut objects, Some(snapshot))?;
    Ok((projection, objects))
}

fn attach_source(
    store: &ConnectionStore,
    entities: &mut BTreeMap<String, String>,
    password: &str,
    objects: &mut BTreeMap<String, EncryptedSecretValue>,
    snapshot: Option<&nyaterm_core::RawPortableSnapshot>,
) -> Result<SyncProjection, StorageError> {
    let codec = SecretValueCodec::new(password)?;
    let crypto = if snapshot.is_some() {
        CredentialCrypto::new(None, Some(password.into()))
    } else {
        store.credential_crypto()?
    };
    let original_token = if let Some(snapshot) = snapshot {
        snapshot
            .entities
            .get("master_key_token")
            .map(|raw| serde_json::from_str::<Option<String>>(raw))
            .transpose()?
            .flatten()
    } else {
        store.load_master_key_token()?
    };
    let token = original_token
        .clone()
        .map(Ok)
        .unwrap_or_else(|| crypto.generate_master_key_token())?;
    let legacy_plaintext_ai = snapshot.is_some_and(|snapshot| {
        semver::Version::parse(snapshot.meta.app_version.trim_start_matches('v'))
            .is_ok_and(|version| version.major < 2)
    });
    if legacy_plaintext_ai {
        let mut settings: Value = serde_json::from_str(&entities["settings"])?;
        if let Some(ai) = settings.get_mut("ai") {
            for collection in ["provider_credentials", "provider_profiles"] {
                if let Some(records) = ai.get_mut(collection).and_then(Value::as_array_mut) {
                    for record in records {
                        if let Some(key) = record
                            .get("api_key")
                            .and_then(Value::as_str)
                            .filter(|key| !key.is_empty())
                            .map(str::to_owned)
                        {
                            record["api_key"] = json!(crypto.encrypt_secret(&token, &key)?);
                        }
                        if let Some(headers) = record
                            .get_mut("extra_headers")
                            .and_then(Value::as_array_mut)
                        {
                            for header in headers {
                                if let Some(value) = header
                                    .get("value")
                                    .and_then(Value::as_str)
                                    .filter(|value| !value.is_empty())
                                {
                                    header["value"] = json!(crypto.encrypt_secret(&token, value)?);
                                }
                            }
                        }
                    }
                }
            }
        }
        entities.insert("settings".into(), settings.to_string());
    }
    let mut projection = SyncProjection::default();
    let mut catalog = json!({});
    let mut materializations = json!({});
    for (kind, table, prefix) in VAULTS {
        projection.complete.insert((*kind).into());
        catalog[*kind] = json!({});
        let source_records = if let Some(snapshot) = snapshot {
            let (entity, section) = match *kind {
                "key" => ("keys", "keys"),
                "password" => ("passwords", "passwords"),
                "credential" => ("credentials", "credentials"),
                "otp" => ("otp", "entries"),
                _ => return Err(invalid()),
            };
            if let Some(raw) = snapshot.entities.get(entity) {
                let value: Value = serde_json::from_str(raw)?;
                value
                    .get(section)
                    .unwrap_or(&value)
                    .as_array()
                    .cloned()
                    .ok_or_else(invalid)?
            } else {
                Vec::new()
            }
        } else {
            store.list_raw_json_values_by_prefix(*table, prefix)?
        };
        for raw in source_records {
            let source = normalize(kind, &raw)?;
            let id = source["id"].as_str().ok_or_else(invalid)?;
            let resource = SyncResourceId::new(*kind, id);
            let payload = plaintext(kind, &source, &crypto, original_token.as_deref())?;
            let value = SyncValue::new(&payload.0);
            let reference = codec.reference(&resource, &value)?;
            if !objects.contains_key(&reference.version) {
                objects.insert(reference.version.clone(), codec.seal(&resource, &value)?);
            }
            let mut fields = json!({"value": reference});
            for field in ["name", "sort_order"] {
                if let Some(value) = source.get(field) {
                    fields[field] = value.clone();
                }
            }
            projection
                .record(kind, id, &fields)
                .map_err(|_| invalid())?;
            let mut encrypted_payload = json!({});
            for field in payload_fields(kind) {
                encrypted_payload[*field] = source.get(*field).cloned().unwrap_or(Value::Null);
            }
            materializations[&reference.version] = encrypted_payload;
            catalog[*kind][id] = raw;
        }
    }
    bound::attach(
        store,
        snapshot.is_none(),
        entities,
        &crypto,
        if legacy_plaintext_ai {
            Some(token.as_str())
        } else {
            original_token.as_deref()
        },
        &token,
        &codec,
        &mut projection,
        objects,
        &mut materializations,
    )?;
    for (version, object) in objects.iter() {
        let kind = object.reference.resource.kind.as_str();
        if payload_fields(kind).is_empty() {
            continue;
        }
        if version != &object.reference.version {
            return Err(invalid());
        }
        let payload = SecretJson(codec.open(&object.reference, object)?.decode()?);
        // Keep existing local ciphertext byte for byte. A reference matches a
        // local value only after deriving its keyed version from decrypted data.
        if materializations.get(version).is_some() {
            continue;
        }
        let fields = payload.0.as_object().ok_or_else(invalid)?;
        if fields
            .keys()
            .any(|field| !payload_fields(kind).contains(&field.as_str()))
        {
            return Err(invalid());
        }
        let mut encrypted = json!({});
        for field in payload_fields(kind)
            .iter()
            .filter(|field| !secret_fields(kind).contains(field))
        {
            encrypted[*field] = payload.0.get(*field).cloned().unwrap_or(Value::Null);
        }
        for field in secret_fields(kind) {
            encrypted[*field] = match payload.0.get(*field) {
                Some(Value::String(value)) => Value::String(crypto.encrypt_secret(&token, value)?),
                None | Some(Value::Null) => Value::Null,
                _ => return Err(invalid()),
            };
        }
        if kind == "ai_secret" {
            encrypted["extra_headers"] =
                bound::encrypt_headers(&payload.0["extra_headers"], &crypto, &token)?;
        }
        materializations[version] = encrypted;
    }
    entities.insert(CATALOG.into(), catalog.to_string());
    entities.insert(MATERIALIZATIONS.into(), materializations.to_string());
    entities.insert(
        TOKEN.into(),
        serde_json::to_string(&SecretString::from(token))?,
    );
    Ok(projection)
}

pub(super) fn reconstruct(
    entities: &mut BTreeMap<String, Value>,
    values: &SyncValues,
) -> Result<(), StorageError> {
    let original = entities.get(CATALOG).cloned().ok_or_else(invalid)?;
    let materializations = entities
        .get(MATERIALIZATIONS)
        .cloned()
        .ok_or_else(invalid)?;
    let mut catalog = json!({});
    for (kind, _, _) in VAULTS {
        catalog[*kind] = json!({});
        for (id, record) in crate::storage::cloud_sync_v4::records(values, kind)? {
            let reference: SecretReference =
                serde_json::from_value(record.get("value").cloned().ok_or_else(invalid)?)?;
            if reference.resource != SyncResourceId::new(*kind, &id) {
                return Err(invalid());
            }
            let encrypted = materializations
                .get(&reference.version)
                .ok_or_else(invalid)?;
            let mut next = original[*kind]
                .get(&id)
                .cloned()
                .unwrap_or_else(|| json!({"id":id}));
            for field in payload_fields(kind) {
                next[*field] = encrypted.get(*field).cloned().unwrap_or(Value::Null);
            }
            for field in ["name", "sort_order"] {
                if let Some(value) = record.get(field) {
                    next[field] = value.clone();
                } else {
                    next.as_object_mut().ok_or_else(invalid)?.remove(field);
                }
            }
            normalize(kind, &next)?;
            catalog[*kind][id] = next;
        }
    }
    let identities = entities
        .get_mut("v4_local_auth_identities")
        .ok_or_else(invalid)?;
    let mut ids: std::collections::BTreeSet<String> = serde_json::from_value(identities.clone())?;
    for (kind, _, prefix) in VAULTS {
        ids.retain(|key| !key.starts_with(prefix));
        for id in catalog[*kind].as_object().ok_or_else(invalid)?.keys() {
            ids.insert(entity_key(prefix, id));
        }
    }
    *identities = serde_json::to_value(ids)?;
    entities.insert(CATALOG.into(), catalog);
    bound::reconstruct(entities, values, &materializations)?;
    Ok(())
}

pub(super) fn persist(
    txn: &redb::WriteTransaction,
    original: &BTreeMap<String, String>,
    entities: &BTreeMap<String, Value>,
) -> Result<(), StorageError> {
    let old: Value = serde_json::from_str(&original[CATALOG])?;
    let next = &entities[CATALOG];
    let token: String = serde_json::from_value(entities[TOKEN].clone())?;
    // The key is already wrapped for this device; installing it and all values
    // shares the business/replica commit. A crash cannot leave half a key migration.
    let existing = txn
        .open_table(META_TABLE)?
        .get(META_MASTER_KEY)?
        .map(|value| value.value().to_owned());
    if existing
        .as_deref()
        .is_some_and(|existing| existing != token)
    {
        return Err(StorageError::CloudSyncLocalChanged);
    }
    txn.open_table(META_TABLE)?
        .insert(META_MASTER_KEY, token.as_str())?;
    txn.open_table(TEXT_DOCS_TABLE)?
        .insert(LEGACY_TEXT_MASTER_KEY, token.as_str())?;
    for (kind, table, prefix) in VAULTS {
        let old = old[*kind].as_object().ok_or_else(invalid)?;
        let next = next[*kind].as_object().ok_or_else(invalid)?;
        for id in old.keys().filter(|id| !next.contains_key(*id)) {
            let key = entity_key(prefix, id);
            crate::storage::cloud_sync_v4::record_remove(txn, *table, &key)?;
            txn.open_table(*table)?.remove(key.as_str())?;
        }
        for (id, value) in next {
            if old.get(id) != Some(value) {
                write_json_in_txn(txn, *table, &entity_key(prefix, id), value)?;
            }
        }
    }
    bound::persist(txn, original, entities)?;
    Ok(())
}

pub(super) fn journal_field(key: &str) -> Option<SyncFieldId> {
    if let Some(id) = key.strip_prefix(CONNECTION_PASSWORD_PREFIX) {
        return Some(SyncFieldId::new(
            SyncResourceId::new("inline_secret", format!("connection/{id}")),
            "value",
        ));
    }
    VAULTS.iter().find_map(|(kind, _, prefix)| {
        key.strip_prefix(prefix)
            .map(|id| SyncFieldId::new(SyncResourceId::new(*kind, id), "value"))
    })
}

pub(super) fn bound_edits(
    key: &str,
    before: Option<&[u8]>,
    after: Option<&[u8]>,
) -> Vec<SyncFieldId> {
    if !key.starts_with(crate::storage::PROXY_PREFIX) && key != crate::storage::SETTINGS_DEFAULT {
        return Vec::new();
    }
    let read = |bytes: Option<&[u8]>| -> Value {
        bytes
            .and_then(|bytes| serde_json::from_slice(bytes).ok())
            .unwrap_or(Value::Null)
    };
    let before = SecretJson(read(before));
    let after = SecretJson(read(after));
    let mut edits = Vec::new();
    if let Some(id) = key.strip_prefix(crate::storage::PROXY_PREFIX) {
        if before.0.get("password") != after.0.get("password") {
            edits.push(SyncFieldId::new(
                SyncResourceId::new("inline_secret", format!("proxy/{id}")),
                "value",
            ));
        }
    } else if key == crate::storage::SETTINGS_DEFAULT {
        for collection in ["provider_credentials", "provider_profiles"] {
            let records = |settings: &Value| {
                settings
                    .pointer(&format!("/ai/{collection}"))
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
            };
            let old = records(&before.0);
            let next = records(&after.0);
            for record in old.iter().chain(&next) {
                let Some(id) = record.get("id").and_then(Value::as_str) else {
                    continue;
                };
                let find = |records: &Vec<Value>| {
                    records.iter().find(|record| record["id"] == id).map(|record| json!({"api_key":record.get("api_key"),"extra_headers":record.get("extra_headers")}))
                };
                if find(&old) != find(&next) {
                    edits.push(SyncFieldId::new(
                        SyncResourceId::new("ai_secret", format!("{collection}/{id}")),
                        "value",
                    ));
                }
            }
        }
    }
    edits
}

/// Excluding credential scope must also protect credentials embedded in core
/// records. Remote deletion or endpoint changes cannot destroy/redirect those
/// slots merely because the portable metadata is otherwise valid.
pub(super) fn validate_excluded_bindings(
    candidate: &BTreeMap<String, Value>,
    original: &BTreeMap<String, Value>,
) -> Result<(), StorageError> {
    let identities: std::collections::BTreeSet<String> = serde_json::from_value(
        original
            .get("v4_local_auth_identities")
            .cloned()
            .ok_or_else(invalid)?,
    )?;
    for (entity, section, kind) in [
        ("sessions", "connections", "connection"),
        ("proxies", "", "proxy"),
    ] {
        let collection = |entities: &BTreeMap<String, Value>| {
            let value = &entities[entity];
            if section.is_empty() {
                value.clone()
            } else {
                value[section].clone()
            }
        };
        let old = collection(original);
        let next = collection(candidate);
        let by_id = next
            .as_array()
            .ok_or_else(invalid)?
            .iter()
            .filter_map(|record| {
                record
                    .get("id")
                    .and_then(Value::as_str)
                    .map(|id| (id, record))
            })
            .collect::<BTreeMap<_, _>>();
        for record in old.as_array().ok_or_else(invalid)? {
            let id = record
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(invalid)?;
            let password = if kind == "connection" {
                record.pointer("/auth/password")
            } else {
                record.get("password")
            };
            let has_secret = password
                .is_some_and(|value| !value.is_null() && value.as_str() != Some(""))
                || (kind == "connection"
                    && identities.contains(&entity_key(CONNECTION_PASSWORD_PREFIX, id)));
            if !has_secret {
                continue;
            }
            let next = by_id.get(id).ok_or_else(invalid)?;
            let before =
                nyaterm_core::sync_schema::project_record(kind, record).map_err(|_| invalid())?;
            let after =
                nyaterm_core::sync_schema::project_record(kind, next).map_err(|_| invalid())?;
            for field in ["endpoint", "authentication"] {
                if before.get(field) != after.get(field) {
                    return Err(invalid());
                }
            }
        }
    }
    Ok(())
}
