//! Portable backup wire format shared by Desktop and Web.
use crate::config::{
    self, ActivityBarLayout, AppSettings, DiagnosticsSettings, InteractionSettings, SearchSettings,
    TerminalSettings, TransferSettings, TranslationSettings,
};
use crate::error::{AppError, AppResult};
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::{Cursor, Read, Write},
    panic::{AssertUnwindSafe, catch_unwind},
    path::{Path, PathBuf},
};
use zip::write::SimpleFileOptions;
pub const PORTABLE_SNAPSHOT_SCHEMA_VERSION: u32 = 3;
const SNAPSHOT_META_KEY: &str = "meta";
const SNAPSHOT_JSON_PORTABLE_SETTINGS: &str = "portable-settings";
const SNAPSHOT_ZIP_MANIFEST_NAME: &str = "manifest.json";
const SNAPSHOT_ZIP_PAYLOAD_NAME: &str = "snapshot.redb";
const MAX_COMPRESSED_SNAPSHOT_PAYLOAD_BYTES: u64 = 50 * 1024 * 1024;
const SNAPSHOT_META_TABLE: TableDefinition<&str, &str> = TableDefinition::new("snapshot_meta");
const SNAPSHOT_ENTITIES_TABLE: TableDefinition<&str, &str> = TableDefinition::new("entity_docs");
const SNAPSHOT_V2_JSON_DOCS_TABLE: TableDefinition<&str, &str> = TableDefinition::new("json_docs");
const SNAPSHOT_V2_TEXT_DOCS_TABLE: TableDefinition<&str, &str> = TableDefinition::new("text_docs");
include!("types.rs");
include!("merge.rs");
include!("codec.rs");
include!("hash.rs");
include!("legacy.rs");
include!("redb_codec.rs");
include!("temp_redb.rs");
include!("tests.rs");
pub fn sync_settings_payload_changed(
    previous: &AppSettings,
    next: &AppSettings,
) -> AppResult<bool> {
    let previous = PortableAppSettings::from_app_settings(previous, &PortableSnapshotKind::Sync);
    let next = PortableAppSettings::from_app_settings(next, &PortableSnapshotKind::Sync);
    Ok(serde_json::to_vec(&previous)? != serde_json::to_vec(&next)?)
}
