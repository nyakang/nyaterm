use crate::error::{AppError, AppResult};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use hmac::Mac;
use redb::ReadableTable;
use serde::Serialize;
use ssh_key::{HashAlg, PublicKey};

use super::KnownHostCheck;
use super::Storage;
use super::history::history_id;
use super::tables::*;
use super::util::*;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct KnownHostEntry {
    pub id: String,
    pub marker: Option<String>,
    pub host_identifier: String,
    pub host_patterns: Vec<String>,
    pub key_type: String,
    pub fingerprint: Option<String>,
}

impl Storage {
    pub fn check_known_host(
        &self,
        host_identifier: &str,
        key_type: &str,
        key_base64: &str,
    ) -> AppResult<KnownHostCheck> {
        let mut host_seen = false;
        let records: Vec<(String, Vec<u8>)> =
            self.list_raw_by_prefix(KNOWN_HOSTS_TABLE, KNOWN_HOST_PREFIX)?;
        for (key, value) in records {
            if key.starts_with(KNOWN_HOST_RAW_PREFIX) {
                continue;
            }
            let host: KnownHostRecord = deserialize_json(&value)?;
            if known_host_record_matches(&host, host_identifier) {
                host_seen = true;
                if host.key_type == key_type && host.key_base64 == key_base64 {
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
    pub fn upsert_known_host(&self, line: &str) -> AppResult<()> {
        let txn = self.db.begin_write().map_err(storage_error)?;
        save_known_hosts_line_in_txn(&txn, line)?;
        txn.commit().map_err(storage_error)?;
        Ok(())
    }
    pub fn replace_known_host_for_host(&self, host_identifier: &str, line: &str) -> AppResult<()> {
        let txn = self.db.begin_write().map_err(storage_error)?;
        remove_known_hosts_for_host_in_txn(&txn, host_identifier)?;
        save_known_hosts_line_in_txn(&txn, line)?;
        txn.commit().map_err(storage_error)?;
        Ok(())
    }
    pub fn render_known_hosts_export(&self) -> AppResult<String> {
        self.render_known_hosts_text()
    }
    pub fn replace_known_hosts_export(&self, content: &str) -> AppResult<()> {
        let txn = self.db.begin_write().map_err(storage_error)?;
        replace_known_hosts_text_in_txn(&txn, content)?;
        txn.commit().map_err(storage_error)?;
        Ok(())
    }
    pub fn list_known_hosts(&self) -> AppResult<Vec<KnownHostEntry>> {
        let mut records: Vec<(String, Vec<u8>)> =
            self.list_raw_by_prefix(KNOWN_HOSTS_TABLE, KNOWN_HOST_PREFIX)?;
        records.sort_by(|left, right| left.0.cmp(&right.0));

        let mut entries = Vec::new();
        for (id, value) in records {
            if id.starts_with(KNOWN_HOST_RAW_PREFIX) {
                continue;
            }
            let record: KnownHostRecord = deserialize_json(&value)?;
            entries.push(KnownHostEntry {
                id,
                marker: record.marker.clone(),
                host_identifier: record.host_identifier.clone(),
                host_patterns: record.host_patterns.clone(),
                key_type: record.key_type.clone(),
                fingerprint: known_host_fingerprint(&record),
            });
        }
        Ok(entries)
    }
    pub fn delete_known_host(&self, id: &str) -> AppResult<()> {
        if !id.starts_with(KNOWN_HOST_PREFIX) || id.starts_with(KNOWN_HOST_RAW_PREFIX) {
            return Err(AppError::Storage("Invalid SSH known host id".to_string()));
        }
        self.remove_key(KNOWN_HOSTS_TABLE, id)
    }
    pub fn clear_known_hosts(&self) -> AppResult<()> {
        let txn = self.db.begin_write().map_err(storage_error)?;
        clear_prefix_in_txn(&txn, KNOWN_HOSTS_TABLE, KNOWN_HOST_PREFIX)?;
        txn.commit().map_err(storage_error)?;
        Ok(())
    }
    pub(super) fn render_known_hosts_text(&self) -> AppResult<String> {
        let mut records: Vec<(String, Vec<u8>)> =
            self.list_raw_by_prefix(KNOWN_HOSTS_TABLE, KNOWN_HOST_PREFIX)?;
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
}

fn known_host_fingerprint(record: &KnownHostRecord) -> Option<String> {
    let key =
        PublicKey::from_openssh(&format!("{} {}", record.key_type, record.key_base64)).ok()?;
    Some(key.fingerprint(HashAlg::Sha256).to_string())
}

pub(super) fn replace_known_hosts_text_in_txn(
    txn: &redb::WriteTransaction,
    content: &str,
) -> AppResult<()> {
    clear_prefix_in_txn(txn, KNOWN_HOSTS_TABLE, KNOWN_HOST_PREFIX)?;
    for line in content.lines() {
        save_known_hosts_line_in_txn(txn, line)?;
    }
    Ok(())
}
pub(super) fn save_known_hosts_line_in_txn(
    txn: &redb::WriteTransaction,
    line: &str,
) -> AppResult<()> {
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
            &format!("{}{}", KNOWN_HOST_RAW_PREFIX, history_id(trimmed)),
            &record,
        )?;
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
pub(super) fn remove_known_hosts_for_host_in_txn(
    txn: &redb::WriteTransaction,
    host_identifier: &str,
) -> AppResult<()> {
    let table = txn.open_table(KNOWN_HOSTS_TABLE).map_err(storage_error)?;
    let mut keys = Vec::new();
    for entry in table.iter().map_err(storage_error)? {
        let (key, value) = entry.map_err(storage_error)?;
        if key.value().starts_with(KNOWN_HOST_RAW_PREFIX) {
            continue;
        }
        let record: KnownHostRecord = deserialize_json(value.value())?;
        if known_host_record_matches(&record, host_identifier) {
            keys.push(key.value().to_string());
        }
    }
    drop(table);
    let mut table = txn.open_table(KNOWN_HOSTS_TABLE).map_err(storage_error)?;
    for key in keys {
        table.remove(key.as_str()).map_err(storage_error)?;
    }
    Ok(())
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
    format!("{KNOWN_HOST_PREFIX}{}", history_id(&digest_input))
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
    if let Some(comment) = &record.comment {
        if !comment.is_empty() {
            line.push(' ');
            line.push_str(comment);
        }
    }
    line
}
