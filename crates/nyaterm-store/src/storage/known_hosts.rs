//! `known_hosts` persistence and host-key verification.
//!
//! Split out of `storage.rs` by domain. Table name, key layout, record shape
//! and the hashed-host matching rules are unchanged; this only moves the code.

use hmac::{Hmac, Mac, digest::KeyInit as HmacKeyInit};
use redb::ReadableTable;
use serde::{Deserialize, Serialize};
use sha1::Sha1;
use ssh_key::{HashAlg, PublicKey};

use super::{
    ConnectionStore, KNOWN_HOST_PREFIX, KNOWN_HOST_RAW_PREFIX, KNOWN_HOSTS_TABLE,
    LEGACY_TEXT_KNOWN_HOSTS, RDP_KNOWN_HOST_PREFIX, RDP_KNOWN_HOSTS_TABLE, StorageError,
    TEXT_DOCS_TABLE, clear_prefix_in_txn, current_time_ms, deserialize_json, stable_id,
    write_json_in_txn,
};
use base64::{Engine, engine::general_purpose::STANDARD as B64};

type HmacSha1 = Hmac<Sha1>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KnownHostCheck {
    Match,
    HostSeen,
    UnknownHost,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct KnownHostEntry {
    pub id: String,
    pub marker: Option<String>,
    pub host_identifier: String,
    pub host_patterns: Vec<String>,
    pub key_type: String,
    pub fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RdpKnownHostCheck {
    Match,
    Changed { remembered_fingerprint: String },
    UnknownHost,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct KnownHostRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    marker: Option<String>,
    host_identifier: String,
    #[serde(default)]
    host_patterns: Vec<String>,
    key_type: String,
    key_base64: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    comment: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    raw_line: Option<String>,
    created_at_ms: u64,
    updated_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct KnownHostRawRecord {
    line: String,
    created_at_ms: u64,
    updated_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct RdpCertificateMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issuer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_to: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct RdpKnownHostMigration {
    pub migrated: usize,
    pub already_present: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct RdpKnownHostRecord {
    host: String,
    port: u16,
    sha256_fingerprint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    subject: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    issuer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    valid_from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    valid_to: Option<String>,
    created_at_ms: u64,
    updated_at_ms: u64,
}

impl ConnectionStore {
    pub fn check_known_host(
        &self,
        host_identifier: &str,
        key_type: &str,
        key_base64: &str,
    ) -> Result<KnownHostCheck, StorageError> {
        let mut host_seen = false;
        let records = self.list_raw_by_prefix(KNOWN_HOSTS_TABLE, KNOWN_HOST_PREFIX)?;
        for (key, value) in records {
            if key.starts_with(KNOWN_HOST_RAW_PREFIX) {
                continue;
            }
            let record: KnownHostRecord = deserialize_json(&value)?;
            if known_host_record_matches(&record, host_identifier) {
                host_seen = true;
                if record.key_type == key_type && record.key_base64 == key_base64 {
                    return Ok(KnownHostCheck::Match);
                }
            }
        }
        if host_seen {
            Ok(KnownHostCheck::HostSeen)
        } else {
            Ok(KnownHostCheck::UnknownHost)
        }
    }
    pub fn upsert_known_host(&self, line: &str) -> Result<(), StorageError> {
        let txn = self.db.begin_write()?;
        save_known_hosts_line_in_txn(&txn, line)?;
        txn.commit()?;
        Ok(())
    }
    pub fn replace_known_host_for_host(
        &self,
        host_identifier: &str,
        line: &str,
    ) -> Result<(), StorageError> {
        let txn = self.db.begin_write()?;
        remove_known_hosts_for_host_in_txn(&txn, host_identifier)?;
        save_known_hosts_line_in_txn(&txn, line)?;
        txn.commit()?;
        Ok(())
    }
    pub fn render_known_hosts_export(&self) -> Result<String, StorageError> {
        let mut records = self.list_raw_by_prefix(KNOWN_HOSTS_TABLE, KNOWN_HOST_PREFIX)?;
        records.sort_by(|left, right| left.0.cmp(&right.0));
        let mut lines = Vec::new();
        for (key, value) in records {
            if key.starts_with(KNOWN_HOST_RAW_PREFIX) {
                let raw: KnownHostRawRecord = deserialize_json(&value)?;
                lines.push(raw.line);
            } else {
                let host: KnownHostRecord = deserialize_json(&value)?;
                lines.push(
                    host.raw_line
                        .clone()
                        .unwrap_or_else(|| render_known_host_record(&host)),
                );
            }
        }
        if lines.is_empty() {
            Ok(String::new())
        } else {
            Ok(format!("{}\n", lines.join("\n")))
        }
    }
    pub fn replace_known_hosts_export(&self, content: &str) -> Result<(), StorageError> {
        let txn = self.db.begin_write()?;
        replace_known_hosts_text_in_txn(&txn, content)?;
        txn.commit()?;
        Ok(())
    }

    pub fn list_known_hosts(&self) -> Result<Vec<KnownHostEntry>, StorageError> {
        let records = self.list_raw_by_prefix(KNOWN_HOSTS_TABLE, KNOWN_HOST_PREFIX)?;
        let mut entries = Vec::new();
        for (id, value) in records {
            if id.starts_with(KNOWN_HOST_RAW_PREFIX) {
                continue;
            }
            let record: KnownHostRecord = deserialize_json(&value)?;
            let Some(fingerprint) = known_host_fingerprint(&record) else {
                // Invalid legacy records remain exportable but are not manageable entries.
                continue;
            };
            entries.push(KnownHostEntry {
                id,
                marker: record.marker.clone(),
                host_identifier: record.host_identifier.clone(),
                host_patterns: record.host_patterns.clone(),
                key_type: record.key_type.clone(),
                fingerprint,
            });
        }
        entries.sort_by(|left, right| {
            left.host_identifier
                .to_lowercase()
                .cmp(&right.host_identifier.to_lowercase())
                .then_with(|| left.host_identifier.cmp(&right.host_identifier))
                .then_with(|| left.key_type.cmp(&right.key_type))
                .then_with(|| left.marker.cmp(&right.marker))
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(entries)
    }

    pub fn delete_known_host(&self, id: &str) -> Result<(), StorageError> {
        if !id.starts_with(KNOWN_HOST_PREFIX) || id.starts_with(KNOWN_HOST_RAW_PREFIX) {
            return Err(StorageError::InvalidData(
                "invalid SSH known host id".to_string(),
            ));
        }
        let txn = self.db.begin_write()?;
        super::cloud_sync_v4::record_remove(&txn, KNOWN_HOSTS_TABLE, id)?;
        {
            let mut table = txn.open_table(KNOWN_HOSTS_TABLE)?;
            let manageable = table
                .get(id)?
                .map(|value| {
                    let record: KnownHostRecord = deserialize_json(value.value())?;
                    Ok::<_, StorageError>(known_host_fingerprint(&record).is_some())
                })
                .transpose()?
                .unwrap_or(true);
            if !manageable {
                return Err(StorageError::InvalidData(
                    "unparsed SSH host records require clear-all".into(),
                ));
            }
            table.remove(id)?;
        }
        txn.commit()?;
        Ok(())
    }

    pub fn clear_known_hosts(&self) -> Result<(), StorageError> {
        let txn = self.db.begin_write()?;
        clear_prefix_in_txn(&txn, KNOWN_HOSTS_TABLE, KNOWN_HOST_PREFIX)?;
        txn.commit()?;
        Ok(())
    }

    pub fn check_rdp_known_host(
        &self,
        host: &str,
        port: u16,
        sha256_fingerprint: &str,
    ) -> Result<RdpKnownHostCheck, StorageError> {
        let key = rdp_known_host_key(host, port);
        let value = match self.read_json_table::<RdpKnownHostRecord>(RDP_KNOWN_HOSTS_TABLE, &key)? {
            Some(record) => Some(record),
            None => self.read_json_table::<RdpKnownHostRecord>(KNOWN_HOSTS_TABLE, &key)?,
        };
        let Some(value) = value else {
            return Ok(RdpKnownHostCheck::UnknownHost);
        };
        if value
            .sha256_fingerprint
            .eq_ignore_ascii_case(sha256_fingerprint)
        {
            Ok(RdpKnownHostCheck::Match)
        } else {
            Ok(RdpKnownHostCheck::Changed {
                remembered_fingerprint: value.sha256_fingerprint,
            })
        }
    }

    pub fn upsert_rdp_known_host(
        &self,
        host: &str,
        port: u16,
        sha256_fingerprint: &str,
        certificate: RdpCertificateMetadata,
    ) -> Result<(), StorageError> {
        let key = rdp_known_host_key(host, port);
        let now = current_time_ms();
        let existing =
            match self.read_json_table::<RdpKnownHostRecord>(RDP_KNOWN_HOSTS_TABLE, &key)? {
                Some(record) => Some(record),
                None => self.read_json_table::<RdpKnownHostRecord>(KNOWN_HOSTS_TABLE, &key)?,
            };
        let record = RdpKnownHostRecord {
            host: host.trim().to_ascii_lowercase(),
            port,
            sha256_fingerprint: sha256_fingerprint.to_string(),
            subject: certificate.subject,
            issuer: certificate.issuer,
            valid_from: certificate.valid_from,
            valid_to: certificate.valid_to,
            created_at_ms: existing.map_or(now, |record| record.created_at_ms),
            updated_at_ms: now,
        };
        let txn = self.db.begin_write()?;
        write_json_in_txn(&txn, RDP_KNOWN_HOSTS_TABLE, &key, &record)?;
        txn.commit()?;
        Ok(())
    }

    pub fn replace_rdp_known_host_if_matches(
        &self,
        host: &str,
        port: u16,
        expected_previous_fingerprint: Option<&str>,
        sha256_fingerprint: &str,
        certificate: RdpCertificateMetadata,
    ) -> Result<bool, StorageError> {
        let key = rdp_known_host_key(host, port);
        let txn = self.db.begin_write()?;
        let current = {
            let table = txn.open_table(RDP_KNOWN_HOSTS_TABLE)?;
            table
                .get(key.as_str())?
                .map(|raw| deserialize_json::<RdpKnownHostRecord>(raw.value()))
                .transpose()?
        };
        let current = match current {
            Some(record) => Some(record),
            None => {
                let table = txn.open_table(KNOWN_HOSTS_TABLE)?;
                table
                    .get(key.as_str())?
                    .map(|raw| deserialize_json::<RdpKnownHostRecord>(raw.value()))
                    .transpose()?
            }
        };
        let current_matches = match (&current, expected_previous_fingerprint) {
            (None, None) => true,
            (Some(record), Some(expected)) => {
                record.sha256_fingerprint.eq_ignore_ascii_case(expected)
            }
            _ => false,
        };
        if !current_matches {
            return Ok(false);
        }

        let now = current_time_ms();
        let record = RdpKnownHostRecord {
            host: host.trim().to_ascii_lowercase(),
            port,
            sha256_fingerprint: sha256_fingerprint.to_string(),
            subject: certificate.subject,
            issuer: certificate.issuer,
            valid_from: certificate.valid_from,
            valid_to: certificate.valid_to,
            created_at_ms: current.map_or(now, |record| record.created_at_ms),
            updated_at_ms: now,
        };
        write_json_in_txn(&txn, RDP_KNOWN_HOSTS_TABLE, &key, &record)?;
        txn.commit()?;
        Ok(true)
    }

    pub(crate) fn migrate_legacy_rdp_known_hosts(
        &self,
    ) -> Result<RdpKnownHostMigration, StorageError> {
        let legacy = self.list_raw_by_prefix(KNOWN_HOSTS_TABLE, RDP_KNOWN_HOST_PREFIX)?;
        let mut validated = Vec::with_capacity(legacy.len());
        for (key, raw) in legacy {
            let _: RdpKnownHostRecord = deserialize_json(&raw)?;
            validated.push((key, raw));
        }

        let txn = self.db.begin_write()?;
        let mut migration = RdpKnownHostMigration::default();
        {
            let mut table = txn.open_table(RDP_KNOWN_HOSTS_TABLE)?;
            for (key, raw) in validated {
                if table.get(key.as_str())?.is_some() {
                    migration.already_present += 1;
                } else {
                    table.insert(key.as_str(), raw.as_slice())?;
                    migration.migrated += 1;
                }
            }
        }
        txn.commit()?;
        Ok(migration)
    }

    pub(super) fn import_legacy_known_hosts_if_needed(&self) -> Result<(), StorageError> {
        let has_native = !self
            .list_raw_by_prefix(KNOWN_HOSTS_TABLE, KNOWN_HOST_PREFIX)?
            .is_empty();
        if has_native {
            return Ok(());
        }
        let Some(content) = self.read_string_table(TEXT_DOCS_TABLE, LEGACY_TEXT_KNOWN_HOSTS)?
        else {
            return Ok(());
        };
        let txn = self.db.begin_write()?;
        replace_known_hosts_text_in_txn(&txn, &content)?;
        txn.commit()?;
        Ok(())
    }
}

fn known_host_fingerprint(record: &KnownHostRecord) -> Option<String> {
    let key =
        PublicKey::from_openssh(&format!("{} {}", record.key_type, record.key_base64)).ok()?;
    Some(key.fingerprint(HashAlg::Sha256).to_string())
}

pub(super) fn replace_known_hosts_text_in_txn(
    txn: &redb::WriteTransaction,
    content: &str,
) -> Result<(), StorageError> {
    clear_prefix_in_txn(txn, KNOWN_HOSTS_TABLE, KNOWN_HOST_PREFIX)?;
    for line in content.lines() {
        save_known_hosts_line_in_txn(txn, line)?;
    }
    Ok(())
}

fn save_known_hosts_line_in_txn(
    txn: &redb::WriteTransaction,
    line: &str,
) -> Result<(), StorageError> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    let now = current_time_ms();
    if let Some(record) = parse_known_host_line(trimmed, now) {
        write_json_in_txn(txn, KNOWN_HOSTS_TABLE, &known_host_key(&record), &record)?;
    } else {
        let record = KnownHostRawRecord {
            line: trimmed.to_string(),
            created_at_ms: now,
            updated_at_ms: now,
        };
        write_json_in_txn(
            txn,
            KNOWN_HOSTS_TABLE,
            &format!("{}{}", KNOWN_HOST_RAW_PREFIX, stable_id(trimmed)),
            &record,
        )?;
    }
    Ok(())
}

fn remove_known_hosts_for_host_in_txn(
    txn: &redb::WriteTransaction,
    host_identifier: &str,
) -> Result<(), StorageError> {
    let table = txn.open_table(KNOWN_HOSTS_TABLE)?;
    let mut keys = Vec::new();
    for entry in table.iter()? {
        let (key, value) = entry?;
        let key_value = key.value();
        if !key_value.starts_with(KNOWN_HOST_PREFIX) || key_value.starts_with(KNOWN_HOST_RAW_PREFIX)
        {
            continue;
        }
        let record: KnownHostRecord = deserialize_json(value.value())?;
        if known_host_record_matches(&record, host_identifier) {
            keys.push(key.value().to_string());
        }
    }
    drop(table);

    for key in &keys {
        super::cloud_sync_v4::record_remove(txn, KNOWN_HOSTS_TABLE, key)?;
    }
    let mut table = txn.open_table(KNOWN_HOSTS_TABLE)?;
    for key in keys {
        table.remove(key.as_str())?;
    }
    Ok(())
}

fn parse_known_host_line(line: &str, now: u64) -> Option<KnownHostRecord> {
    if line.starts_with('#') {
        return None;
    }
    let mut parts = line.split_whitespace();
    let first = parts.next()?;
    let (marker, host_list) = if first.starts_with('@') {
        (Some(first.to_string()), parts.next()?)
    } else {
        (None, first)
    };
    let key_type = parts.next()?;
    let key_base64 = parts.next()?;
    let comment = {
        let rest = parts.collect::<Vec<_>>().join(" ");
        if rest.is_empty() { None } else { Some(rest) }
    };
    let host_patterns = host_list
        .split(',')
        .filter(|pattern| !pattern.is_empty())
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if host_patterns.is_empty() {
        return None;
    }

    Some(KnownHostRecord {
        marker,
        host_identifier: host_patterns[0].clone(),
        host_patterns,
        key_type: key_type.to_string(),
        key_base64: key_base64.to_string(),
        comment,
        raw_line: Some(line.to_string()),
        created_at_ms: now,
        updated_at_ms: now,
    })
}

fn known_host_record_matches(record: &KnownHostRecord, host_identifier: &str) -> bool {
    let patterns = if record.host_patterns.is_empty() {
        std::slice::from_ref(&record.host_identifier)
    } else {
        record.host_patterns.as_slice()
    };
    let mut matched = false;
    for pattern in patterns {
        let (negated, pattern) = pattern
            .strip_prefix('!')
            .map_or((false, pattern.as_str()), |pattern| (true, pattern));
        if known_host_pattern_matches(pattern, host_identifier) {
            if negated {
                return false;
            }
            matched = true;
        }
    }
    matched
}

fn known_host_pattern_matches(pattern: &str, host_identifier: &str) -> bool {
    if pattern == host_identifier {
        return true;
    }
    if pattern.starts_with("|1|") {
        return hashed_known_host_matches(pattern, host_identifier);
    }
    false
}

fn hashed_known_host_matches(pattern: &str, host_identifier: &str) -> bool {
    let mut parts = pattern.split('|');
    if parts.next() != Some("") || parts.next() != Some("1") {
        return false;
    }
    let Some(salt_b64) = parts.next() else {
        return false;
    };
    let Some(hash_b64) = parts.next() else {
        return false;
    };
    if parts.next().is_some() {
        return false;
    }
    let Ok(salt) = B64.decode(salt_b64) else {
        return false;
    };
    let Ok(expected) = B64.decode(hash_b64) else {
        return false;
    };
    let Ok(mut mac) = HmacSha1::new_from_slice(&salt) else {
        return false;
    };
    mac.update(host_identifier.as_bytes());
    let actual = mac.finalize().into_bytes();
    expected.as_slice() == actual.as_slice()
}

fn known_host_key(record: &KnownHostRecord) -> String {
    let digest_input = format!(
        "{}|{}|{}",
        record.marker.as_deref().unwrap_or_default(),
        record.host_patterns.join(","),
        record.key_type
    );
    format!("{KNOWN_HOST_PREFIX}{}", stable_id(&digest_input))
}

pub(super) fn validate_v4_record(
    id: &str,
    value: &serde_json::Value,
) -> Result<serde_json::Value, StorageError> {
    let mut value = value.clone();
    if value.get("created_at_ms").is_none() {
        value["created_at_ms"] = serde_json::json!(current_time_ms());
    }
    value["updated_at_ms"] = serde_json::json!(current_time_ms());
    value["raw_line"] = serde_json::Value::Null;
    let record: KnownHostRecord = serde_json::from_value(value.clone())?;
    if known_host_key(&record) != id
        || known_host_fingerprint(&record).is_none()
        || record
            .marker
            .as_deref()
            .is_some_and(|marker| !matches!(marker, "@revoked" | "@cert-authority"))
        || record.host_patterns.is_empty()
        || record.host_identifier != record.host_patterns[0]
        || record
            .host_patterns
            .iter()
            .any(|pattern| pattern.is_empty() || pattern.contains(char::is_whitespace))
    {
        return Err(StorageError::InvalidData(
            "Invalid synchronized SSH host identity".into(),
        ));
    }
    Ok(value)
}

pub(super) fn parse_v4_export(content: &str) -> Vec<(String, serde_json::Value)> {
    content
        .lines()
        .filter_map(|line| {
            let record = parse_known_host_line(line.trim(), current_time_ms())?;
            let id = known_host_key(&record);
            let value = serde_json::to_value(record).ok()?;
            validate_v4_record(&id, &value)
                .ok()
                .map(|value| (id, value))
        })
        .collect()
}

fn rdp_known_host_key(host: &str, port: u16) -> String {
    let host_identifier = format!("{}:{}", host.trim().to_ascii_lowercase(), port);
    format!("{RDP_KNOWN_HOST_PREFIX}{}", stable_id(&host_identifier))
}

fn render_known_host_record(record: &KnownHostRecord) -> String {
    let host_list = if record.host_patterns.is_empty() {
        record.host_identifier.clone()
    } else {
        record.host_patterns.join(",")
    };
    let mut line = String::new();
    if let Some(marker) = &record.marker {
        line.push_str(marker);
        line.push(' ');
    }
    line.push_str(&host_list);
    line.push(' ');
    line.push_str(&record.key_type);
    line.push(' ');
    line.push_str(&record.key_base64);
    if let Some(comment) = &record.comment
        && !comment.is_empty()
    {
        line.push(' ');
        line.push_str(comment);
    }
    line
}
