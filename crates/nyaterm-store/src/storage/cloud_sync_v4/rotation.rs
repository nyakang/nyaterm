//! Password rotation stages immutable bytes before I/O. Only the verified
//! publication permits an atomic password/root/replica switch.
use std::collections::BTreeMap;

use nyaterm_core::CredentialCrypto;
use nyaterm_core::sync_schema::{encryption, secrets::SecretValueCodec};
use nyaterm_sync::engine::digest;
use nyaterm_sync::protocol::{MAX_PUBLICATION_BYTES, PendingPublication, PublishedReplica};
use redb::{ReadableTable, ReadableTableMetadata, TableDefinition};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;
use zeroize::Zeroizing;

use super::{REPLICAS, generation, invalid};
use crate::storage::{
    ConnectionStore, LEGACY_TEXT_MASTER_KEY, META_MASTER_KEY, META_TABLE, SETTINGS_DEFAULT,
    SETTINGS_TABLE, StorageError, TEXT_DOCS_TABLE, set_nested_json_value, write_json_in_txn,
};
use crate::sync_v4::{LocalReplica, PasswordRotationPublication, PreparedApply};

const ROTATIONS: TableDefinition<&str, &[u8]> = TableDefinition::new("sync_v4_password_rotations");
const STAGED: &str = "pending";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StagedRotation {
    source: String,
    generation: u64,
    settings_hash: String,
    originals: BTreeMap<String, String>,
    replacements: BTreeMap<String, Vec<u8>>,
    settings: Value,
    master_key: Option<String>,
    proof: Vec<u8>,
    remote_root: String,
    pending: PendingPublication,
}

fn settings_hash(txn: &redb::WriteTransaction) -> Result<String, StorageError> {
    Ok(txn
        .open_table(SETTINGS_TABLE)?
        .get(SETTINGS_DEFAULT)?
        .map(|value| digest(value.value()))
        .unwrap_or_default())
}

fn replica_hashes(txn: &redb::WriteTransaction) -> Result<BTreeMap<String, String>, StorageError> {
    txn.open_table(REPLICAS)?
        .iter()?
        .map(|row| {
            let (key, value) = row?;
            Ok((key.value().into(), digest(value.value())))
        })
        .collect()
}

fn matches(txn: &redb::WriteTransaction, stage: &StagedRotation) -> Result<bool, StorageError> {
    Ok(generation(txn)? == stage.generation
        && settings_hash(txn)? == stage.settings_hash
        && replica_hashes(txn)? == stage.originals)
}

impl ConnectionStore {
    pub fn has_v4_replicas(&self) -> Result<bool, StorageError> {
        let txn = self.db.begin_write()?;
        Ok(txn.open_table(REPLICAS)?.len()? > 0)
    }
    /// An interrupted publication leaves ordinary settings untouched. A retry
    /// after restart uses precisely the same target, sequence and ciphertext.
    pub fn load_v4_password_rotation(
        &self,
        source: &str,
        next_password: &str,
    ) -> Result<Option<PasswordRotationPublication>, StorageError> {
        let password = self
            .load_cloud_sync_master_password()?
            .ok_or_else(invalid)?;
        let txn = self.db.begin_write()?;
        let bytes = txn
            .open_table(ROTATIONS)?
            .get(STAGED)?
            .map(|value| value.value().to_vec());
        let Some(bytes) = bytes else {
            return Ok(None);
        };
        let plaintext = encryption::open(
            password.expose_secret(),
            "password-rotation",
            STAGED,
            &bytes,
        )?;
        let stage: StagedRotation = serde_json::from_slice(&plaintext)?;
        if stage.source != source
            || !matches(&txn, &stage)?
            || encryption::open(next_password, "rotation-proof", source, &stage.proof).is_err()
        {
            return Ok(None);
        }
        Ok(Some(PasswordRotationPublication {
            ticket: digest(&bytes),
            remote_root: stage.remote_root,
            pending: stage.pending,
        }))
    }

    pub fn stage_v4_password_rotation(
        &self,
        source: PreparedApply,
        next_password: &str,
        target: &str,
        remote_root: &str,
    ) -> Result<PasswordRotationPublication, StorageError> {
        let password = self
            .load_cloud_sync_master_password()?
            .ok_or_else(invalid)?;
        if next_password.trim().is_empty()
            || next_password == password.expose_secret()
            || target == source.destination
            || !source.local.migration_approved
        {
            return Err(invalid());
        }
        let view = self.load_v4_view(
            &source.destination,
            password.expose_secret(),
            &source.plan.scope,
        )?;
        if view.generation != source.generation
            || view.record_hash != source.expected_record
            || view.projection != source.plan.expected_local
        {
            return Err(StorageError::CloudSyncLocalChanged);
        }
        let original_settings = self.load_settings_value()?;
        let mut settings = original_settings.clone();
        // Keep encrypted provider credentials and future properties verbatim.
        set_nested_json_value(
            &mut settings,
            &["cloud_sync", "remote_root"],
            Value::String(remote_root.into()),
        );
        let bootstrap = CredentialCrypto::new(self.portable_key_path.clone(), None);
        set_nested_json_value(
            &mut settings,
            &["security", "master_password"],
            Value::String(bootstrap.encrypt_settings_secret(next_password)?),
        );
        let old_master_key = self.load_master_key_token()?;
        let master_key = old_master_key
            .as_ref()
            .map(|token| {
                CredentialCrypto::new(self.portable_key_path.clone(), Some(password.clone()))
                    .rewrap_master_key_token(token, Some(next_password))
            })
            .transpose()?;
        let codec = SecretValueCodec::new(password.expose_secret())?;
        let rewrap_record = |record: &mut LocalReplica| -> Result<(), StorageError> {
            for object in record.secret_objects.values_mut() {
                *object = codec.rewrap(object, next_password)?;
            }
            Ok(())
        };
        let mut record = source.local;
        record.replica = Some(source.plan.replica);
        record.effective = source.plan.expected_local.values;
        record.applied_generation = source.generation;
        record.conflicts = source.plan.conflicts;
        record.scope = source.plan.scope;
        record.writer = Uuid::new_v4();
        record.next_sequence = 2;
        record.pending = None;
        record.observed.clear();
        record.legacy_fingerprint = Some(digest(b"[]"));
        rewrap_record(&mut record)?;
        let replica = record.replica.clone().ok_or_else(invalid)?;
        let document_hash = digest(&replica.document);
        let publication = PublishedReplica {
            schema: 4,
            writer: record.writer,
            sequence: 1,
            replica,
            secret_objects: if record.scope.secrets {
                record.secret_objects.clone()
            } else {
                BTreeMap::new()
            },
        };
        let plaintext = Zeroizing::new(serde_json::to_vec(&publication)?);
        if plaintext.len() > MAX_PUBLICATION_BYTES / 2 {
            return Err(invalid());
        }
        let bytes = encryption::seal(
            next_password,
            "publication",
            &format!("{}/1", record.writer),
            &plaintext,
        )?;
        let pending = PendingPublication::new(record.writer, 1, bytes, document_hash.clone())
            .ok_or_else(invalid)?;
        // This receipt becomes live only after coordinator readback and CAS.
        record
            .observed
            .insert(pending.id.path(), pending.id.clone());
        record.published_document = Some(document_hash);

        let txn = self.db.begin_write()?;
        if generation(&txn)? != source.generation
            || txn
                .open_table(REPLICAS)?
                .get(source.destination.as_str())?
                .map(|v| digest(v.value()))
                != source.expected_record
            || txn.open_table(REPLICAS)?.get(target)?.is_some()
            || self.load_settings_value()? != original_settings
            || self.load_master_key_token()? != old_master_key
        {
            return Err(StorageError::CloudSyncLocalChanged);
        }
        let originals = replica_hashes(&txn)?;
        let mut replacements = BTreeMap::new();
        for row in txn.open_table(REPLICAS)?.iter()? {
            let (key, bytes) = row?;
            let plaintext = encryption::open(
                password.expose_secret(),
                "local-replica",
                key.value(),
                bytes.value(),
            )?;
            let mut archived: LocalReplica = serde_json::from_slice(&plaintext)?;
            rewrap_record(&mut archived)?;
            let plaintext = Zeroizing::new(serde_json::to_vec(&archived)?);
            replacements.insert(
                key.value().to_owned(),
                encryption::seal(next_password, "local-replica", key.value(), &plaintext)?,
            );
        }
        let plaintext = Zeroizing::new(serde_json::to_vec(&record)?);
        replacements.insert(
            target.into(),
            encryption::seal(next_password, "local-replica", target, &plaintext)?,
        );
        let stage = StagedRotation {
            source: source.destination.clone(),
            generation: source.generation,
            settings_hash: settings_hash(&txn)?,
            originals,
            replacements,
            settings,
            master_key,
            proof: encryption::seal(
                next_password,
                "rotation-proof",
                &source.destination,
                b"verified-space-switch",
            )?,
            remote_root: remote_root.into(),
            pending: pending.clone(),
        };
        let plaintext = Zeroizing::new(serde_json::to_vec(&stage)?);
        let bytes = encryption::seal(
            password.expose_secret(),
            "password-rotation",
            STAGED,
            &plaintext,
        )?;
        txn.open_table(ROTATIONS)?
            .insert(STAGED, bytes.as_slice())?;
        txn.commit()?;
        Ok(PasswordRotationPublication {
            ticket: digest(&bytes),
            remote_root: remote_root.into(),
            pending,
        })
    }

    pub fn commit_verified_v4_password_rotation(&self, ticket: &str) -> Result<(), StorageError> {
        let password = self
            .load_cloud_sync_master_password()?
            .ok_or_else(invalid)?;
        let txn = self.db.begin_write()?;
        let bytes = txn
            .open_table(ROTATIONS)?
            .get(STAGED)?
            .ok_or_else(invalid)?
            .value()
            .to_vec();
        if digest(&bytes) != ticket {
            return Err(StorageError::CloudSyncLocalChanged);
        }
        let plaintext = encryption::open(
            password.expose_secret(),
            "password-rotation",
            STAGED,
            &bytes,
        )?;
        let stage: StagedRotation = serde_json::from_slice(&plaintext)?;
        if !matches(&txn, &stage)? {
            return Err(StorageError::CloudSyncLocalChanged);
        }
        write_json_in_txn(&txn, SETTINGS_TABLE, SETTINGS_DEFAULT, &stage.settings)?;
        if let Some(token) = stage.master_key {
            txn.open_table(META_TABLE)?
                .insert(META_MASTER_KEY, token.as_str())?;
            txn.open_table(TEXT_DOCS_TABLE)?
                .insert(LEGACY_TEXT_MASTER_KEY, token.as_str())?;
        }
        for (destination, bytes) in stage.replacements {
            txn.open_table(REPLICAS)?
                .insert(destination.as_str(), bytes.as_slice())?;
        }
        txn.open_table(ROTATIONS)?.remove(STAGED)?;
        txn.commit()?;
        Ok(())
    }
}
