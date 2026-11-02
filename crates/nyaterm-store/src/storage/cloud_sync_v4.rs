//! V4 configuration application uses domain transactions, never backup restore.
mod rotation;
mod secrets;
mod trust;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use nyaterm_core::sync_schema::{
    SyncFieldId, SyncProjection, SyncScope, SyncValues, apply_record, encryption, project_entities,
    project_record,
};
use nyaterm_core::{Group, NotesSnapshot, SessionsConfig};
use nyaterm_sync::application::{SyncApplication, isolate_changes};
use nyaterm_sync::engine::{SyncReplica, digest};
use redb::{ReadableDatabase, ReadableTable, TableDefinition};
use serde_json::{Value, json};
use uuid::Uuid;
use zeroize::Zeroizing;

use super::notes::{replace_notes_snapshot_in_txn, validate_notes_snapshot};
use super::{
    CONNECTION_PASSWORD_PREFIX, CONNECTION_PREFIX, CONNECTIONS_TABLE, CREDENTIALS_TABLE,
    ConnectionStore, GROUP_PREFIX, GROUPS_TABLE, META_TABLE, PROXIES_TABLE, PROXY_PREFIX,
    PreparedConnectionForStorage, SETTINGS_DEFAULT, SETTINGS_PROXY_GROUPS, SETTINGS_QUICK_COMMANDS,
    SETTINGS_TABLE, SETTINGS_TUNNEL_GROUPS, StorageError, TUNNEL_PREFIX, TUNNELS_TABLE,
    current_time_ms, entity_key, remove_connection_indexes_for_ids,
    save_connection_with_clean_indexes_in_txn, save_group_in_txn, write_json_in_txn,
};
use crate::sync_v4::{LocalReplica, LocalView, PreparedApply};

pub(super) const REPLICAS: TableDefinition<&str, &[u8]> = TableDefinition::new("sync_v4_replicas");
const EDITS: TableDefinition<&str, u64> = TableDefinition::new("sync_v4_field_edits");
const GENERATION: &str = "sync_v4/generation";
static PROCESS_ACTOR: OnceLock<Uuid> = OnceLock::new();

#[derive(Clone, Copy, PartialEq, Eq)]
enum ApplyPoint {
    Locked,
    Connections,
    Groups,
    Collections,
    Notes,
    Settings,
    Replica,
    Committed,
}

fn invalid() -> StorageError {
    StorageError::InvalidData("Cloud Sync V3 data is invalid or incompatible".into())
}

fn generation(txn: &redb::WriteTransaction) -> Result<u64, StorageError> {
    Ok(txn
        .open_table(META_TABLE)?
        .get(GENERATION)?
        .and_then(|value| value.value().parse().ok())
        .unwrap_or(0))
}

/// This durable marker shares the business transaction. It contains no user
/// values; the encrypted replica captures its final domain projection later.
pub(super) fn record_write(
    txn: &redb::WriteTransaction,
    table: TableDefinition<&str, &[u8]>,
    key: &str,
    bytes: &[u8],
) -> Result<(), StorageError> {
    let previous = txn
        .open_table(table)?
        .get(key)?
        .map(|value| value.value().to_vec());
    if previous.as_deref() == Some(bytes) {
        return Ok(());
    }
    if key.starts_with("credentials/") || key.starts_with("otp_accounts/") {
        // These values stay outside the current sync scope, but a concurrent
        // local password edit must still invalidate a business Apply snapshot.
        return record_secret_edit(txn, key);
    }
    track_edit(txn, key, previous.as_deref(), Some(bytes))
}

pub(super) fn record_remove(
    txn: &redb::WriteTransaction,
    table: TableDefinition<&str, &[u8]>,
    key: &str,
) -> Result<(), StorageError> {
    let previous = txn
        .open_table(table)?
        .get(key)?
        .map(|value| value.value().to_vec());
    if previous.is_some() {
        if key.starts_with("credentials/") || key.starts_with("otp_accounts/") {
            return record_secret_edit(txn, key);
        }
        track_edit(txn, key, previous.as_deref(), None)?;
    }
    Ok(())
}

fn project_entry(key: &str, bytes: Option<&[u8]>) -> Result<SyncProjection, StorageError> {
    let Some(bytes) = bytes else {
        return Ok(SyncProjection::default());
    };
    let source: Value = serde_json::from_slice(bytes)?;
    for (prefix, kind) in [
        (CONNECTION_PREFIX, "connection"),
        ("connection_custom_icons/", "connection_icon"),
        (GROUP_PREFIX, "connection_group"),
        ("notes/", "note"),
        ("note_folders/", "note_folder"),
        (PROXY_PREFIX, "proxy"),
        (TUNNEL_PREFIX, "tunnel"),
        (super::KNOWN_HOST_PREFIX, "known_host"),
        ("vnc_known_hosts/", "vnc_known_host"),
    ] {
        if let Some(id) = key.strip_prefix(prefix) {
            if kind == "known_host" && key.starts_with(super::KNOWN_HOST_RAW_PREFIX) {
                return Ok(SyncProjection::default());
            }
            let id = if nyaterm_core::sync_schema::trust::KINDS.contains(&kind) {
                key
            } else {
                id
            };
            let mut projection = SyncProjection::default();
            projection
                .record(
                    kind,
                    id,
                    &project_record(kind, &source).map_err(|_| invalid())?,
                )
                .map_err(|_| invalid())?;
            return Ok(projection);
        }
    }
    let (entity, source) = match key {
        SETTINGS_DEFAULT => ("settings", source),
        SETTINGS_PROXY_GROUPS => (
            "proxy_groups",
            source.get("groups").cloned().unwrap_or_else(|| json!([])),
        ),
        SETTINGS_TUNNEL_GROUPS => (
            "tunnel_groups",
            source.get("groups").cloned().unwrap_or_else(|| json!([])),
        ),
        SETTINGS_QUICK_COMMANDS => ("quick_commands", source),
        _ => return Ok(SyncProjection::default()),
    };
    project_entities(
        &BTreeMap::from([(entity.into(), source.to_string())]),
        &SyncScope {
            preferences: true,
            keybindings: true,
            ai: true,
            ..Default::default()
        },
    )
    .map_err(|_| invalid())
}

fn track_edit(
    txn: &redb::WriteTransaction,
    key: &str,
    before: Option<&[u8]>,
    after: Option<&[u8]>,
) -> Result<(), StorageError> {
    // Domain records remain authoritative. The journal only remembers field
    // identities and a transaction generation, never unencrypted field values.
    let bound_edits = secrets::bound_edits(key, before, after);
    let (Ok(before), Ok(after)) = (project_entry(key, before), project_entry(key, after)) else {
        // A future business format must remain writable by existing storage
        // paths even when this sync adapter cannot classify it.
        return record_change(txn);
    };
    let mut changed = before
        .values
        .keys()
        .chain(after.values.keys())
        .filter(|key| before.values.get(*key) != after.values.get(*key))
        .cloned()
        .collect::<BTreeSet<_>>();
    changed.extend(bound_edits.into_iter().map(|field| field.key()));
    if changed.is_empty() {
        if key == SETTINGS_DEFAULT {
            record_change(txn)?;
        }
        return Ok(());
    }
    record_change(txn)?;
    let generation = generation(txn)?;
    let mut edits = txn.open_table(EDITS)?;
    for key in changed {
        edits.insert(key.as_str(), generation)?;
    }
    Ok(())
}

pub(super) fn record_change(txn: &redb::WriteTransaction) -> Result<(), StorageError> {
    let next = generation(txn)?.checked_add(1).ok_or_else(invalid)?;
    txn.open_table(META_TABLE)?
        .insert(GENERATION, next.to_string().as_str())?;
    Ok(())
}

fn record_secret_edit(txn: &redb::WriteTransaction, key: &str) -> Result<(), StorageError> {
    record_change(txn)?;
    if let Some(field) = secrets::journal_field(key) {
        txn.open_table(EDITS)?
            .insert(field.key().as_str(), generation(txn)?)?;
    }
    Ok(())
}

impl ConnectionStore {
    pub fn project_v4_legacy_trust(
        &self,
        snapshot: &nyaterm_core::RawPortableSnapshot,
    ) -> Result<SyncProjection, StorageError> {
        trust::legacy_projection(snapshot)
    }
    pub fn project_v4_legacy_secrets(
        &self,
        snapshot: &nyaterm_core::RawPortableSnapshot,
        password: &str,
    ) -> Result<
        (
            SyncProjection,
            BTreeMap<String, nyaterm_core::sync_schema::secrets::EncryptedSecretValue>,
        ),
        StorageError,
    > {
        secrets::legacy_projection(self, snapshot, password)
    }

    pub fn inspect_v4_application(
        &self,
        plan: &nyaterm_sync::engine::SyncPlan,
    ) -> Result<SyncApplication, StorageError> {
        let entities = self.v4_entities()?;
        constrain_application(
            &entities,
            &plan.expected_local.values,
            &plan.effective,
            &plan.scope,
            &BTreeMap::new(),
        )
    }

    pub fn inspect_v4_application_with_secrets(
        &self,
        plan: &nyaterm_sync::engine::SyncPlan,
        objects: &BTreeMap<String, nyaterm_core::sync_schema::secrets::EncryptedSecretValue>,
        password: &str,
        trust_approvals: &BTreeMap<String, String>,
    ) -> Result<SyncApplication, StorageError> {
        let mut entities = self.v4_entities()?;
        if plan.scope.secrets {
            secrets::attach(self, &mut entities, password, &mut objects.clone())?;
        }
        if plan.scope.trusted_hosts {
            trust::attach(self, &mut entities)?;
        }
        constrain_application(
            &entities,
            &plan.expected_local.values,
            &plan.effective,
            &plan.scope,
            trust_approvals,
        )
    }

    pub(super) fn v4_backup_sessions(&self) -> Result<Value, StorageError> {
        let entities = self.v4_entities()?;
        let mut sessions: Value = serde_json::from_str(&entities["sessions"])?;
        for connection in sessions["connections"].as_array_mut().ok_or_else(invalid)? {
            let id = connection
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(invalid)?
                .to_owned();
            if let Some(auth) = connection.get_mut("auth").and_then(Value::as_object_mut)
                && let Some(password) = self.read_json_table::<super::ConnectionPasswordRecord>(
                    CREDENTIALS_TABLE,
                    &entity_key(CONNECTION_PASSWORD_PREFIX, &id),
                )?
            {
                auth.insert("password".into(), serde_json::to_value(password.password)?);
                auth.insert("has_password".into(), Value::Bool(true));
            }
        }
        Ok(sessions)
    }

    fn v4_entities(&self) -> Result<BTreeMap<String, String>, StorageError> {
        // The V4 input boundary deliberately does not build a backup: command
        // history, trust stores, cloud authentication and vault tables are not
        // read just to check whether a portable user field changed.
        let mut entities = BTreeMap::new();
        entities.insert(
            "settings".into(),
            serde_json::to_string(&self.load_settings_value()?)?,
        );
        // load_sessions() performs legacy icon and credential migrations. A
        // preview must not invoke those writes or decrypt an excluded vault.
        // Raw records also retain properties unknown to the typed catalogs.
        let sessions = json!({
            "connections": self.list_raw_json_values_by_prefix(CONNECTIONS_TABLE, CONNECTION_PREFIX)?,
            "groups": self.list_raw_json_values_by_prefix(GROUPS_TABLE, GROUP_PREFIX)?,
            "custom_icons": self.list_raw_json_values_by_prefix(CONNECTIONS_TABLE, "connection_custom_icons/")?
        });
        entities.insert("sessions".into(), sessions.to_string());
        entities.insert(
            "proxies".into(),
            serde_json::to_string(
                &self.list_raw_json_values_by_prefix(PROXIES_TABLE, PROXY_PREFIX)?,
            )?,
        );
        entities.insert(
            "tunnels".into(),
            serde_json::to_string(
                &self.list_raw_json_values_by_prefix(TUNNELS_TABLE, TUNNEL_PREFIX)?,
            )?,
        );
        for (entity, key) in [
            ("proxy_groups", SETTINGS_PROXY_GROUPS),
            ("tunnel_groups", SETTINGS_TUNNEL_GROUPS),
        ] {
            let value = self.load_settings_doc_value(key, json!({"groups":[]}))?;
            entities.insert(
                entity.into(),
                value
                    .get("groups")
                    .cloned()
                    .unwrap_or_else(|| json!([]))
                    .to_string(),
            );
        }
        entities.insert(
            "quick_commands".into(),
            self.load_settings_doc_value(
                SETTINGS_QUICK_COMMANDS,
                json!({"commands":[],"categories":[]}),
            )?
            .to_string(),
        );
        entities.insert(
            "notes".into(),
            serde_json::to_string(&self.load_notes_snapshot()?)?,
        );
        // Only identities are needed for authentication-reference validation.
        // Do not decode/decrypt excluded credential or OTP records.
        let txn = self.db.begin_read()?;
        let mut identities = BTreeSet::new();
        for table in [CREDENTIALS_TABLE, super::OTP_ACCOUNTS_TABLE] {
            for row in txn.open_table(table)?.iter()? {
                let (key, _) = row?;
                identities.insert(key.value().to_owned());
            }
        }
        entities.insert(
            "v4_local_auth_identities".into(),
            serde_json::to_string(&identities)?,
        );
        Ok(entities)
    }
    pub fn load_v4_view(
        &self,
        destination: &str,
        password: &str,
        scope: &SyncScope,
    ) -> Result<LocalView, StorageError> {
        let txn = self.db.begin_write()?;
        let bytes = txn
            .open_table(REPLICAS)?
            .get(destination)?
            .map(|value| value.value().to_vec());
        let generation = generation(&txn)?;
        drop(txn);
        let record_hash = bytes.as_deref().map(digest);
        let mut record: LocalReplica = if let Some(bytes) = bytes {
            let plaintext = encryption::open(password, "local-replica", destination, &bytes)?;
            serde_json::from_slice(&plaintext).map_err(|_| invalid())?
        } else {
            LocalReplica::default()
        };
        if record.schema != 4 {
            return Err(invalid());
        }
        let owner = digest(
            format!(
                "{}:{}",
                PROCESS_ACTOR.get_or_init(Uuid::new_v4),
                self.db_path().display()
            )
            .as_bytes(),
        );
        if record.owner != owner {
            // A restored/copied DB may retain exact pending bytes, but never
            // author another operation with a previous process's ActorId.
            record.writer = Uuid::new_v4();
            record.next_sequence = 1;
            record.owner = owner;
        }
        let mut entities = self.v4_entities()?;
        let mut projection = project_entities(&entities, scope).map_err(|_| invalid())?;
        if scope.secrets {
            let secret_projection =
                secrets::attach(self, &mut entities, password, &mut record.secret_objects)?;
            projection.values.extend(secret_projection.values);
            projection.complete.extend(secret_projection.complete);
        }
        if scope.trusted_hosts {
            let trust_projection = trust::attach(self, &mut entities)?;
            projection.values.extend(trust_projection.values);
            projection.complete.extend(trust_projection.complete);
        }
        // SessionsConfig omits an empty icon catalog when serialized. This
        // local typed reader still proves that the category was read fully.
        projection.complete.insert("connection_icon".into());
        let check = self.db.begin_write()?;
        if generation != self::generation(&check)? {
            return Err(StorageError::CloudSyncLocalChanged);
        }
        for row in check.open_table(EDITS)?.iter()? {
            let (field, edited_at) = row?;
            if edited_at.value() > record.applied_generation {
                let field = SyncFieldId::from_key(field.value()).ok_or_else(invalid)?;
                if scope.includes(&field.resource.kind) {
                    projection.edited.insert(field);
                }
            }
        }
        Ok(LocalView {
            record,
            record_hash,
            generation,
            projection,
        })
    }

    /// Publication acknowledgements compare the actual durable record. Unlike
    /// Prepare, this read does not bind a writer or observe new business/secret
    /// values, which must remain separate from the publication being retried.
    pub fn load_v4_replica(
        &self,
        destination: &str,
        password: &str,
    ) -> Result<Option<(LocalReplica, String)>, StorageError> {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(REPLICAS)?;
        let Some(bytes) = table.get(destination)? else {
            return Ok(None);
        };
        let hash = digest(bytes.value());
        let plaintext = encryption::open(password, "local-replica", destination, bytes.value())?;
        let record: LocalReplica = serde_json::from_slice(&plaintext).map_err(|_| invalid())?;
        if record.schema != 4 {
            return Err(invalid());
        }
        Ok(Some((record, hash)))
    }

    pub fn persist_v4_record(
        &self,
        destination: &str,
        password: &str,
        expected: Option<&str>,
        record: &LocalReplica,
    ) -> Result<String, StorageError> {
        let plaintext = Zeroizing::new(serde_json::to_vec(record)?);
        let bytes = encryption::seal(password, "local-replica", destination, &plaintext)?;
        let txn = self.db.begin_write()?;
        let current = txn
            .open_table(REPLICAS)?
            .get(destination)?
            .map(|value| digest(value.value()));
        if current.as_deref() != expected {
            return Err(StorageError::CloudSyncLocalChanged);
        }
        txn.open_table(REPLICAS)?
            .insert(destination, bytes.as_slice())?;
        txn.commit()?;
        Ok(digest(&bytes))
    }

    pub fn apply_v4_plan(
        &self,
        prepared: PreparedApply,
        password: &str,
    ) -> Result<LocalReplica, StorageError> {
        self.apply_v4_plan_observed(prepared, password, |_| {})
    }

    fn apply_v4_plan_observed(
        &self,
        mut prepared: PreparedApply,
        password: &str,
        mut observe: impl FnMut(ApplyPoint),
    ) -> Result<LocalReplica, StorageError> {
        let current = self.load_v4_view(&prepared.destination, password, &prepared.plan.scope)?;
        if current.record_hash != prepared.expected_record
            || current.generation != prepared.generation
            || current.projection != prepared.plan.expected_local
        {
            return Err(StorageError::CloudSyncLocalChanged);
        }
        let live_settings = self.load_cloud_sync_settings()?;
        if live_settings.v4_scope != prepared.plan.scope || live_settings.protocol_version != 4 {
            return Err(StorageError::CloudSyncLocalChanged);
        }
        let replica = SyncReplica::load(&prepared.plan.replica, prepared.local.writer)
            .map_err(|_| invalid())?;
        if replica
            .materialize(&current.projection.values, &prepared.plan.scope)
            .map_err(|_| invalid())?
            != prepared.plan.effective
        {
            return Err(invalid());
        }
        let mut snapshot = self.v4_entities()?;
        if prepared.plan.scope.secrets {
            secrets::attach(
                self,
                &mut snapshot,
                password,
                &mut prepared.local.secret_objects,
            )?;
        }
        if prepared.plan.scope.trusted_hosts {
            trust::attach(self, &mut snapshot)?;
        }
        let mut entities = snapshot
            .iter()
            .map(|(key, value)| {
                serde_json::from_str::<Value>(value).map(|value| (key.clone(), value))
            })
            .collect::<Result<BTreeMap<_, _>, _>>()?;
        // Preserve the complete live document, including the encrypted local
        // verifier. A redacted portable-backup projection is never applied here.
        let original_settings = entities["settings"].clone();
        let application = constrain_application(
            &snapshot,
            &current.projection.values,
            &prepared.plan.effective,
            &prepared.plan.scope,
            &prepared.trust_approvals,
        )?;
        reconstruct(&mut entities, &application.effective, &prepared.plan.scope)?;
        let sessions: SessionsConfig = serde_json::from_value(entities["sessions"].clone())?;
        validate_relationships(&entities)?;
        let notes: NotesSnapshot = serde_json::from_value(entities["notes"].clone())?;
        validate_notes_snapshot(&notes)?;
        // Core V4 fields never carry credential material. Keep existing local
        // slots verbatim instead of decrypting or re-encrypting them here.
        let connections = sessions
            .connections
            .iter()
            .cloned()
            .map(PreparedConnectionForStorage)
            .collect::<Vec<_>>();
        prepared.local.replica = Some(prepared.plan.replica);
        entities.insert("sessions".into(), serde_json::to_value(&sessions)?);
        entities.insert("notes".into(), serde_json::to_value(&notes)?);
        prepared.local.conflicts = prepared.plan.conflicts;
        prepared.local.deferred = application.deferred;
        prepared.local.scope = prepared.plan.scope;
        let txn = self.db.begin_write()?;
        let current_hash = txn
            .open_table(REPLICAS)?
            .get(prepared.destination.as_str())?
            .map(|value| digest(value.value()));
        if generation(&txn)? != prepared.generation || current_hash != prepared.expected_record {
            return Err(StorageError::CloudSyncLocalChanged);
        }
        observe(ApplyPoint::Locked);
        let original_sessions: SessionsConfig = serde_json::from_str(&snapshot["sessions"])?;
        let original_connections = original_sessions
            .connections
            .iter()
            .map(|connection| (connection.id.as_str(), connection))
            .collect::<BTreeMap<_, _>>();
        let next_connections = sessions
            .connections
            .iter()
            .map(|connection| connection.id.as_str())
            .collect::<BTreeSet<_>>();
        let original_groups = original_sessions
            .groups
            .iter()
            .map(|group| (group.id.as_str(), group))
            .collect::<BTreeMap<_, _>>();
        let next_groups = sessions
            .groups
            .iter()
            .map(|group| group.id.as_str())
            .collect::<BTreeSet<_>>();
        let original_sessions_value: Value = serde_json::from_str(&snapshot["sessions"])?;
        write_collection(
            &txn,
            CONNECTIONS_TABLE,
            "connection_custom_icons/",
            original_sessions_value
                .get("custom_icons")
                .unwrap_or(&json!([])),
            entities["sessions"]
                .get("custom_icons")
                .unwrap_or(&json!([])),
        )?;
        let affected_connections = original_sessions
            .connections
            .iter()
            .filter(|connection| !next_connections.contains(connection.id.as_str()))
            .map(|connection| connection.id.as_str())
            .chain(sessions.connections.iter().filter_map(|next| {
                (original_connections.get(next.id.as_str()).copied() != Some(next))
                    .then_some(next.id.as_str())
            }))
            .collect::<BTreeSet<_>>();
        remove_connection_indexes_for_ids(&txn, &affected_connections)?;
        for original in &original_sessions.connections {
            if !next_connections.contains(original.id.as_str()) {
                txn.open_table(CONNECTIONS_TABLE)?
                    .remove(entity_key(CONNECTION_PREFIX, &original.id).as_str())?;
                txn.open_table(CREDENTIALS_TABLE)?
                    .remove(entity_key(CONNECTION_PASSWORD_PREFIX, &original.id).as_str())?;
            }
        }
        for (next, connection) in sessions.connections.iter().zip(connections.iter()) {
            if original_connections.get(next.id.as_str()).copied() != Some(next) {
                let connection = rebase_connection_in_txn(&txn, connection)?;
                save_v4_connection_in_txn(&txn, connection)?;
            }
        }
        observe(ApplyPoint::Connections);
        for old in &original_sessions.groups {
            if !next_groups.contains(old.id.as_str()) {
                txn.open_table(GROUPS_TABLE)?
                    .remove(entity_key(GROUP_PREFIX, &old.id).as_str())?;
            }
        }
        for group in &sessions.groups {
            if original_groups.get(group.id.as_str()).copied() != Some(group) {
                save_group_in_txn(&txn, group)?;
            }
        }
        observe(ApplyPoint::Groups);
        for (entity, table, prefix) in [
            ("proxies", PROXIES_TABLE, PROXY_PREFIX),
            ("tunnels", TUNNELS_TABLE, TUNNEL_PREFIX),
        ] {
            write_collection(
                &txn,
                table,
                prefix,
                &serde_json::from_str::<Value>(&snapshot[entity])?,
                &entities[entity],
            )?;
        }
        for (entity, key, wrapped) in [
            ("proxy_groups", SETTINGS_PROXY_GROUPS, true),
            ("tunnel_groups", SETTINGS_TUNNEL_GROUPS, true),
            ("quick_commands", SETTINGS_QUICK_COMMANDS, false),
        ] {
            let old: Value = serde_json::from_str(&snapshot[entity])?;
            if old != entities[entity] {
                let next = if wrapped {
                    json!({"groups":entities[entity]})
                } else {
                    entities[entity].clone()
                };
                write_json_in_txn(&txn, SETTINGS_TABLE, key, &next)?;
            }
        }
        observe(ApplyPoint::Collections);
        let old_notes: Value = serde_json::from_str(&snapshot["notes"])?;
        if old_notes != entities["notes"] {
            replace_notes_snapshot_in_txn(&txn, &notes)?;
        }
        observe(ApplyPoint::Notes);
        let old_settings = original_settings;
        if old_settings != entities["settings"] {
            write_json_in_txn(
                &txn,
                SETTINGS_TABLE,
                SETTINGS_DEFAULT,
                &entities["settings"],
            )?;
        }
        observe(ApplyPoint::Settings);
        if prepared.local.scope.secrets {
            secrets::persist(&txn, &snapshot, &entities)?;
        }
        if prepared.local.scope.trusted_hosts {
            trust::persist(&txn, &snapshot, &entities)?;
        }
        // Derive the baseline from actual transaction-local records. Untouched
        // legacy records may omit serde defaults; newly written records may
        // introduce them. Neither normalization is a later user edit.
        entities.insert(
            "sessions".into(),
            json!({
                "connections": records_in_txn(&txn, CONNECTIONS_TABLE, CONNECTION_PREFIX)?,
                "groups": records_in_txn(&txn, GROUPS_TABLE, GROUP_PREFIX)?,
                "custom_icons": records_in_txn(&txn, CONNECTIONS_TABLE, "connection_custom_icons/")?
            }),
        );
        let applied_entities = entities
            .iter()
            .map(|(key, value)| (key.clone(), value.to_string()))
            .collect();
        prepared.local.effective = project_entities(&applied_entities, &prepared.local.scope)
            .map_err(|_| invalid())?
            .values;
        if prepared.local.scope.secrets || prepared.local.scope.trusted_hosts {
            prepared
                .local
                .effective
                .extend(application.effective.into_iter().filter(|(key, _)| {
                    SyncFieldId::from_key(key).is_some_and(|field| {
                        (prepared.local.scope.secrets
                            && nyaterm_core::sync_schema::SECRET_KINDS
                                .contains(&field.resource.kind.as_str()))
                            || (prepared.local.scope.trusted_hosts
                                && nyaterm_core::sync_schema::trust::KINDS
                                    .contains(&field.resource.kind.as_str()))
                    })
                }));
        }
        prepared.local.applied_generation = generation(&txn)?;
        let plaintext = Zeroizing::new(serde_json::to_vec(&prepared.local)?);
        let bytes = encryption::seal(password, "local-replica", &prepared.destination, &plaintext)?;
        txn.open_table(REPLICAS)?
            .insert(prepared.destination.as_str(), bytes.as_slice())?;
        observe(ApplyPoint::Replica);
        // Every mutation here belongs to this one Redb transaction. A process
        // death therefore exposes either the entire apply or none of it.
        txn.commit()?;
        observe(ApplyPoint::Committed);
        if prepared.local.scope.secrets {
            super::vault::bump_ssh_key_revision();
        }
        Ok(prepared.local)
    }
}

fn constrain_application(
    snapshot: &BTreeMap<String, String>,
    local: &SyncValues,
    desired: &SyncValues,
    scope: &SyncScope,
    trust_approvals: &BTreeMap<String, String>,
) -> Result<SyncApplication, StorageError> {
    let original = snapshot
        .iter()
        .map(|(key, value)| serde_json::from_str::<Value>(value).map(|value| (key.clone(), value)))
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    isolate_changes(local, desired, |values| {
        let mut candidate = original.clone();
        let validate = || -> Result<(), StorageError> {
            if scope.trusted_hosts && !nyaterm_sync::trust::approved(local, values, trust_approvals)
            {
                return Err(invalid());
            }
            reconstruct(&mut candidate, values, scope)?;
            if !scope.secrets {
                secrets::validate_excluded_bindings(&candidate, &original)?;
            }
            validate_relationships(&candidate)?;
            validate_authentication_references(&candidate, &original)?;
            if scope.ai {
                validate_ai_application(&candidate["settings"], &original["settings"])?;
            }
            let notes: NotesSnapshot = serde_json::from_value(candidate["notes"].clone())?;
            validate_notes_snapshot(&notes)?;
            Ok(())
        };
        let mut validate = validate;
        validate().is_ok()
    })
    .map_err(|_| invalid())
}

fn validate_authentication_references(
    candidate: &BTreeMap<String, Value>,
    original: &BTreeMap<String, Value>,
) -> Result<(), StorageError> {
    let identities: BTreeSet<String> = serde_json::from_value(
        candidate
            .get("v4_local_auth_identities")
            .cloned()
            .ok_or_else(invalid)?,
    )?;
    let previous_identities: BTreeSet<String> = serde_json::from_value(
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
        let old_by_id = old
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
        let next = collection(candidate);
        for record in next.as_array().ok_or_else(invalid)? {
            let id = record
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(invalid)?;
            let previous = old_by_id.get(id).copied();
            let field = if kind == "connection" {
                "authentication"
            } else {
                "endpoint"
            };
            let projected = project_record(kind, record).map_err(|_| invalid())?;
            let previous = previous
                .map(|record| project_record(kind, record))
                .transpose()
                .map_err(|_| invalid())?;
            let unchanged =
                previous.as_ref().and_then(|record| record.get(field)) == projected.get(field);
            let references: &[(&str, &str)] = if kind == "connection" {
                &[
                    ("key_id", super::SSH_KEY_PREFIX),
                    ("account_id", super::PASSWORD_PREFIX),
                    ("password_id", super::PASSWORD_PREFIX),
                    ("otp_id", super::OTP_PREFIX),
                ]
            } else {
                &[("password_id", super::PASSWORD_PREFIX)]
            };
            for (name, prefix) in references {
                if let Some(reference) = projected
                    .get(field)
                    .and_then(|group| group.get(*name))
                    .and_then(Value::as_str)
                    && !reference.is_empty()
                    && !identities.contains(&entity_key(prefix, reference))
                    && (!unchanged || previous_identities.contains(&entity_key(prefix, reference)))
                {
                    return Err(invalid());
                }
            }
        }
    }
    Ok(())
}

fn records_in_txn(
    txn: &redb::WriteTransaction,
    table: TableDefinition<&str, &[u8]>,
    prefix: &str,
) -> Result<Vec<Value>, StorageError> {
    let table = txn.open_table(table)?;
    let mut records = Vec::new();
    for row in table.iter()? {
        let (key, value) = row?;
        if key.value().starts_with(prefix) {
            records.push(serde_json::from_slice(value.value())?);
        }
    }
    Ok(records)
}

fn rebase_connection_in_txn(
    txn: &redb::WriteTransaction,
    next: &PreparedConnectionForStorage,
) -> Result<PreparedConnectionForStorage, StorageError> {
    let key = entity_key(CONNECTION_PREFIX, &next.0.id);
    let current = txn
        .open_table(CONNECTIONS_TABLE)?
        .get(key.as_str())?
        .map(|raw| serde_json::from_slice::<Value>(raw.value()))
        .transpose()?;
    let Some(current) = current else {
        return Ok(PreparedConnectionForStorage(next.0.clone()));
    };
    // Runtime writes deliberately do not dirty the causal journal. Overlay on
    // the transaction's current record so monitoring/usage between Prepare and
    // the write lock survives. User edits are separately guarded by generation.
    let projected =
        project_record("connection", &serde_json::to_value(&next.0)?).map_err(|_| invalid())?;
    let merged = apply_record("connection", &current, &projected).map_err(|_| invalid())?;
    let merged: nyaterm_core::SavedConnection = serde_json::from_value(merged)?;
    Ok(PreparedConnectionForStorage(merged))
}

fn save_v4_connection_in_txn(
    txn: &redb::WriteTransaction,
    mut connection: PreparedConnectionForStorage,
) -> Result<(), StorageError> {
    let credential_key = entity_key(CONNECTION_PASSWORD_PREFIX, &connection.0.id);
    let existing = txn
        .open_table(CREDENTIALS_TABLE)?
        .get(credential_key.as_str())?
        .map(|raw| raw.value().to_vec());
    let inline = connection.0.auth.as_mut().map(|auth| {
        let inline = (auth.password.take(), auth.has_password);
        auth.has_password = false;
        inline
    });
    save_connection_with_clean_indexes_in_txn(txn, &connection)?;
    // The ordinary writer owns indexes, timestamps and agent validation. Its
    // credential replacement is outside this scope; restore exact bytes,
    // including unknown slot metadata and supported legacy inline layouts.
    if let Some(existing) = existing {
        txn.open_table(CREDENTIALS_TABLE)?
            .insert(credential_key.as_str(), existing.as_slice())?;
    }
    if let Some((Some(password), has_password)) = inline {
        let key = entity_key(CONNECTION_PREFIX, &connection.0.id);
        let mut saved: Value = serde_json::from_slice(
            txn.open_table(CONNECTIONS_TABLE)?
                .get(key.as_str())?
                .ok_or_else(invalid)?
                .value(),
        )?;
        saved["auth"]["password"] = serde_json::to_value(password)?;
        saved["auth"]["has_password"] = json!(has_password);
        write_json_in_txn(txn, CONNECTIONS_TABLE, &key, &saved)?;
    }
    Ok(())
}

fn records(values: &SyncValues, kind: &str) -> Result<BTreeMap<String, Value>, StorageError> {
    let mut result = BTreeMap::<String, Value>::new();
    for (key, value) in values {
        let field = SyncFieldId::from_key(key).ok_or_else(invalid)?;
        if field.resource.kind != kind
            || field.field == "$present"
            || !nyaterm_core::sync_schema::supports_field(kind, &field.field)
        {
            continue;
        }
        result.entry(field.resource.id).or_insert_with(|| json!({}))[&field.field] =
            value.decode().map_err(|_| invalid())?;
    }
    Ok(result)
}

fn merge_collection(
    original: &Value,
    values: &SyncValues,
    kind: &str,
) -> Result<Value, StorageError> {
    let original = original.as_array().ok_or_else(invalid)?;
    let by_id = original
        .iter()
        .filter_map(|record| {
            record
                .get("id")
                .and_then(Value::as_str)
                .map(|id| (id, record))
        })
        .collect::<BTreeMap<_, _>>();
    let positions = original
        .iter()
        .enumerate()
        .filter_map(|(index, record)| {
            record
                .get("id")
                .and_then(Value::as_str)
                .map(|id| (id, index))
        })
        .collect::<BTreeMap<_, _>>();
    let projected = records(values, kind)?;
    let projected_ids = projected.keys().cloned().collect::<BTreeSet<_>>();
    let mut result = Vec::new();
    for (id, fields) in projected {
        let old = by_id.get(id.as_str()).copied();
        let mut merged =
            apply_record(kind, old.unwrap_or(&Value::Null), &fields).map_err(|_| invalid())?;
        merged["id"] = json!(id);
        if old.is_none() && matches!(kind, "note" | "note_folder") {
            merged["created_at_ms"] = json!(current_time_ms());
            merged["updated_at_ms"] = json!(current_time_ms());
            if kind == "note" {
                merged["revision"] = json!(1);
            }
        }
        if let Some(old) = old.filter(|old| *old != &merged)
            && matches!(kind, "note" | "note_folder")
        {
            merged["updated_at_ms"] = json!(current_time_ms());
            if kind == "note" {
                merged["revision"] = json!(
                    old.get("revision")
                        .and_then(Value::as_u64)
                        .unwrap_or(0)
                        .checked_add(1)
                        .ok_or_else(invalid)?
                );
            }
        }
        if kind == "connection_icon" && old.is_none() {
            merged["created_at_ms"] = json!(current_time_ms());
            merged["updated_at_ms"] = json!(current_time_ms());
        }
        if kind == "connection" && old.is_none() {
            match merged.get("type").and_then(Value::as_str) {
                Some("serial") => merged["port_name"] = json!(""),
                Some("local_terminal") => {
                    merged["shell_path"] = json!("");
                    merged["working_dir"] = Value::Null;
                }
                _ => {}
            }
        }
        result.push(merged);
    }
    if kind == "theme" {
        result.extend(
            original
                .iter()
                .filter(|record| {
                    serde_json::from_value::<nyaterm_core::themes::ThemeConfig>((*record).clone())
                        .is_err()
                })
                .cloned(),
        );
    }
    if nyaterm_core::sync_schema::ai::KINDS.contains(&kind) {
        result.extend(
            original
                .iter()
                .filter(|record| {
                    record
                        .get("id")
                        .and_then(Value::as_str)
                        .is_some_and(|id| !projected_ids.contains(id))
                        && nyaterm_core::sync_schema::ai::project_record(kind, record).is_err()
                })
                .cloned(),
        );
    }
    // Preserve order for unchanged resources and append new IDs deterministically.
    result.sort_by_key(|record| {
        record
            .get("id")
            .and_then(Value::as_str)
            .and_then(|id| positions.get(id))
            .copied()
            .unwrap_or(usize::MAX)
    });
    Ok(Value::Array(result))
}

fn reconstruct(
    entities: &mut BTreeMap<String, Value>,
    effective: &SyncValues,
    scope: &SyncScope,
) -> Result<(), StorageError> {
    for (entity, section, kind) in [
        ("sessions", "connections", "connection"),
        ("sessions", "groups", "connection_group"),
        ("sessions", "custom_icons", "connection_icon"),
        ("proxies", "", "proxy"),
        ("proxy_groups", "", "proxy_group"),
        ("tunnels", "", "tunnel"),
        ("tunnel_groups", "", "tunnel_group"),
        ("quick_commands", "commands", "command"),
        ("quick_commands", "categories", "command_category"),
        ("notes", "notes", "note"),
        ("notes", "folders", "note_folder"),
    ] {
        let original = if section.is_empty() {
            entities.get(entity)
        } else {
            entities.get(entity).and_then(|value| value.get(section))
        };
        let empty = json!([]);
        let merged = merge_collection(original.unwrap_or(&empty), effective, kind)?;
        if section.is_empty() {
            entities.insert(entity.into(), merged);
        } else {
            entities.entry(entity.into()).or_insert_with(|| json!({}))[section] = merged;
        }
    }
    let settings = entities.get_mut("settings").ok_or_else(invalid)?;
    if let Some(themes) = settings
        .pointer("/appearance/custom_themes")
        .filter(|value| value.is_array())
        .cloned()
    {
        settings["appearance"]["custom_themes"] = merge_collection(&themes, effective, "theme")?;
    }
    if scope.preferences {
        for (id, value) in records(effective, "preference")? {
            let (section, field) = id.split_once('/').ok_or_else(invalid)?;
            if !nyaterm_core::sync_schema::PREFERENCE_FIELDS.contains(&(section, field)) {
                return Err(invalid());
            }
            let value = value.get("value").ok_or_else(invalid)?;
            if !nyaterm_core::sync_schema::valid_preference(&id, value) {
                return Err(invalid());
            }
            settings[section][field] = value.clone();
        }
    }
    if scope.keybindings {
        let bindings = settings
            .as_object_mut()
            .ok_or_else(invalid)?
            .entry("keybindings")
            .or_insert_with(|| json!({}));
        let bindings = bindings.as_object_mut().ok_or_else(invalid)?;
        // Only readable local bindings can be removed. Opaque entries survive
        // and are excluded from the category's deletion observations.
        bindings.retain(|_, value| !value.is_string());
        for (action, record) in records(effective, "keybinding")? {
            let value = record
                .get("value")
                .filter(|value| value.is_string())
                .ok_or_else(invalid)?;
            bindings.entry(action).or_insert_with(|| value.clone());
        }
    }
    if scope.ai {
        reconstruct_ai(settings, effective)?;
    }
    if scope.secrets {
        secrets::reconstruct(entities, effective)?;
    }
    if scope.trusted_hosts {
        trust::reconstruct(entities, effective)?;
    }
    Ok(())
}

fn reconstruct_ai(settings: &mut Value, effective: &SyncValues) -> Result<(), StorageError> {
    use nyaterm_core::sync_schema::ai::{COLLECTIONS, PREFERENCES};
    if !settings.get("ai").is_some_and(Value::is_object) {
        settings["ai"] = json!({});
    }
    for (collection, kind) in COLLECTIONS {
        let original = settings["ai"]
            .get(*collection)
            .cloned()
            .unwrap_or_else(|| json!([]));
        let mut original = original.as_array().cloned().ok_or_else(invalid)?;
        let mut filtered = effective.clone();
        let prefix = format!("{collection}/");
        if *kind == "ai_action" {
            for record in &mut original {
                if let Some(id) = record.get("id").and_then(Value::as_str) {
                    record["id"] = json!(format!("{prefix}{id}"));
                }
            }
            filtered.retain(|key, _| {
                SyncFieldId::from_key(key).is_some_and(|field| {
                    field.resource.kind != "ai_action" || field.resource.id.starts_with(&prefix)
                })
            });
        }
        let mut merged = merge_collection(&Value::Array(original), &filtered, kind)?;
        if *kind == "ai_action" {
            for record in merged.as_array_mut().ok_or_else(invalid)? {
                let id = record
                    .get("id")
                    .and_then(Value::as_str)
                    .and_then(|id| id.strip_prefix(&prefix))
                    .ok_or_else(invalid)?;
                record["id"] = json!(id);
            }
        }
        settings["ai"][*collection] = merged;
    }
    for field in PREFERENCES {
        settings["ai"]
            .as_object_mut()
            .ok_or_else(invalid)?
            .remove(*field);
    }
    for (id, record) in records(effective, "ai_preference")? {
        if !PREFERENCES.contains(&id.as_str()) {
            return Err(invalid());
        }
        settings["ai"][&id] = record.get("value").cloned().ok_or_else(invalid)?;
    }
    Ok(())
}

fn validate_ai_application(candidate: &Value, original: &Value) -> Result<(), StorageError> {
    let Some(next) = candidate.get("ai") else {
        return Ok(());
    };
    let parsed: nyaterm_core::ai::AiSettings = serde_json::from_value(next.clone())?;
    let previous = original.get("ai").unwrap_or(&Value::Null);
    for (collection, kind) in nyaterm_core::sync_schema::ai::COLLECTIONS {
        let empty = Vec::new();
        let old = previous
            .get(*collection)
            .and_then(Value::as_array)
            .unwrap_or(&empty);
        let next = next
            .get(*collection)
            .and_then(Value::as_array)
            .unwrap_or(&empty);
        for record in old {
            let id = record
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(invalid)?;
            let has_secret = record
                .get("api_key")
                .is_some_and(|value| !value.is_null() && value.as_str() != Some(""))
                || record
                    .get("extra_headers")
                    .and_then(Value::as_array)
                    .is_some_and(|headers| !headers.is_empty());
            if !has_secret {
                continue;
            }
            // Optional nonsecret synchronization cannot remove local secrets
            // or silently redirect them to a remotely changed endpoint.
            let next = next
                .iter()
                .find(|record| record["id"] == id)
                .ok_or_else(invalid)?;
            if matches!(*kind, "ai_provider" | "ai_profile")
                && ["provider_kind", "api_format", "api_protocol", "base_url"]
                    .iter()
                    .any(|field| record.get(field) != next.get(field))
            {
                return Err(invalid());
            }
        }
    }
    for model in &parsed.models {
        let old = previous
            .get("models")
            .and_then(Value::as_array)
            .and_then(|models| models.iter().find(|record| record["id"] == model.id));
        let unchanged = old.is_some_and(|old| {
            old.get("credential_id").and_then(Value::as_str) == model.credential_id.as_deref()
        });
        if !unchanged
            && model.credential_id.as_deref().is_some_and(|id| {
                !parsed
                    .provider_credentials
                    .iter()
                    .any(|provider| provider.id == id)
            })
        {
            return Err(invalid());
        }
    }
    if next.get("default_model_id") != previous.get("default_model_id")
        && parsed
            .default_model_id
            .as_deref()
            .is_some_and(|id| !parsed.models.iter().any(|model| model.id == id))
    {
        return Err(invalid());
    }
    Ok(())
}

fn write_collection(
    txn: &redb::WriteTransaction,
    table: TableDefinition<&str, &[u8]>,
    prefix: &str,
    old: &Value,
    next: &Value,
) -> Result<(), StorageError> {
    let old = old.as_array().ok_or_else(invalid)?;
    let next = next.as_array().ok_or_else(invalid)?;
    let next_ids = next
        .iter()
        .filter_map(|record| record.get("id").and_then(Value::as_str))
        .collect::<BTreeSet<_>>();
    let old_by_id = old
        .iter()
        .filter_map(|record| {
            record
                .get("id")
                .and_then(Value::as_str)
                .map(|id| (id, record))
        })
        .collect::<BTreeMap<_, _>>();
    for record in old {
        let id = record
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(invalid)?;
        if !next_ids.contains(id) {
            txn.open_table(table)?
                .remove(entity_key(prefix, id).as_str())?;
        }
    }
    for record in next {
        let id = record
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(invalid)?;
        if old_by_id.get(id).copied() != Some(record) {
            let key = entity_key(prefix, id);
            let kind = match prefix {
                PROXY_PREFIX => Some("proxy"),
                TUNNEL_PREFIX => Some("tunnel"),
                "connection_custom_icons/" => Some("connection_icon"),
                _ => None,
            };
            let rebased = if let Some(kind) = kind {
                let current = txn
                    .open_table(table)?
                    .get(key.as_str())?
                    .map(|raw| serde_json::from_slice::<Value>(raw.value()))
                    .transpose()?;
                current
                    .map(|current| {
                        let projected = project_record(kind, record).map_err(|_| invalid())?;
                        apply_record(kind, &current, &projected).map_err(|_| invalid())
                    })
                    .transpose()?
            } else {
                None
            };
            write_json_in_txn(txn, table, &key, rebased.as_ref().unwrap_or(record))?;
        }
    }
    Ok(())
}

fn validate_relationships(entities: &BTreeMap<String, Value>) -> Result<(), StorageError> {
    let sessions: SessionsConfig = serde_json::from_value(entities["sessions"].clone())?;
    let proxies: Vec<nyaterm_core::ProxyConfig> =
        serde_json::from_value(entities["proxies"].clone())?;
    let proxy_groups: Vec<nyaterm_core::ProxyGroup> =
        serde_json::from_value(entities["proxy_groups"].clone())?;
    let tunnels: Vec<nyaterm_core::TunnelConfig> =
        serde_json::from_value(entities["tunnels"].clone())?;
    let tunnel_groups: Vec<nyaterm_core::TunnelGroup> =
        serde_json::from_value(entities["tunnel_groups"].clone())?;
    let commands: nyaterm_core::QuickCommandsConfig =
        serde_json::from_value(entities["quick_commands"].clone())?;
    let connections = sessions
        .connections
        .iter()
        .map(|connection| (connection.id.as_str(), connection))
        .collect::<BTreeMap<_, _>>();
    let proxy_ids = proxies
        .iter()
        .map(|proxy| proxy.id.as_str())
        .collect::<BTreeSet<_>>();
    let proxy_group_ids = proxy_groups
        .iter()
        .map(|group| group.id.as_str())
        .collect::<BTreeSet<_>>();
    let tunnel_group_ids = tunnel_groups
        .iter()
        .map(|group| group.id.as_str())
        .collect::<BTreeSet<_>>();
    let groups = sessions
        .groups
        .iter()
        .map(|group| (&group.id, group))
        .collect::<BTreeMap<_, _>>();
    for group in &sessions.groups {
        let mut seen = BTreeSet::new();
        let mut current: Option<&Group> = Some(group);
        while let Some(group) = current {
            if !seen.insert(&group.id) {
                return Err(invalid());
            }
            current = group
                .parent_id
                .as_ref()
                .map(|parent| groups.get(parent).copied().ok_or_else(invalid))
                .transpose()?;
        }
    }
    for connection in &sessions.connections {
        if connection
            .group_id
            .as_ref()
            .is_some_and(|id| !groups.contains_key(id))
        {
            return Err(invalid());
        }
        if connection
            .network
            .as_ref()
            .and_then(|network| network.proxy_id.as_deref())
            .is_some_and(|id| !proxy_ids.contains(id))
        {
            return Err(invalid());
        }
        let mut seen = BTreeSet::new();
        let mut current = Some(connection);
        while let Some(connection) = current {
            if !seen.insert(connection.id.as_str()) {
                return Err(invalid());
            }
            current = connection
                .network
                .as_ref()
                .and_then(|network| network.proxy_jump_id.as_deref())
                .map(|id| connections.get(id).copied().ok_or_else(invalid))
                .transpose()?;
            if current.is_some_and(|jump| {
                !matches!(jump.config, nyaterm_core::ConnectionType::Ssh { .. })
            }) {
                return Err(invalid());
            }
        }
    }
    for proxy in &proxies {
        if proxy
            .group_id
            .as_deref()
            .is_some_and(|id| !proxy_group_ids.contains(id))
        {
            return Err(invalid());
        }
    }
    for tunnel in &tunnels {
        if tunnel
            .group_id
            .as_deref()
            .is_some_and(|id| !tunnel_group_ids.contains(id))
            || tunnel.connection_id.as_deref().is_some_and(|id| {
                connections.get(id).is_none_or(|connection| {
                    !matches!(connection.config, nyaterm_core::ConnectionType::Ssh { .. })
                })
            })
        {
            return Err(invalid());
        }
    }
    let categories = commands
        .categories
        .iter()
        .map(|category| (category.id.as_str(), category))
        .collect::<BTreeMap<_, _>>();
    for category in &commands.categories {
        let mut seen = BTreeSet::new();
        let mut current = Some(category);
        while let Some(category) = current {
            if !seen.insert(category.id.as_str()) {
                return Err(invalid());
            }
            current = category
                .parent_id
                .as_deref()
                .map(|id| categories.get(id).copied().ok_or_else(invalid))
                .transpose()?;
        }
    }
    if commands.commands.iter().any(|command| {
        command
            .category_id
            .as_deref()
            .is_some_and(|id| !categories.contains_key(id))
    }) {
        return Err(invalid());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
