pub fn decode_portable_snapshot(bytes: &[u8]) -> AppResult<PortableSnapshot> {
    Ok(decode_portable_snapshot_with_source_hash(bytes)?.snapshot)
}

pub fn decode_portable_snapshot_with_source_hash(
    bytes: &[u8],
) -> AppResult<DecodedPortableSnapshot> {
    let payload = catch_unwind(AssertUnwindSafe(|| -> AppResult<Vec<u8>> {
        if is_zip_snapshot_payload(bytes) {
            decode_compressed_snapshot_payload(bytes)
        } else {
            Ok(bytes.to_vec())
        }
    }))
    .unwrap_or_else(|_| {
        Err(AppError::Storage(
            "Portable snapshot compressed payload is corrupt or incomplete".to_string(),
        ))
    })?;
    decode_portable_snapshot_redb_with_source_hash(&payload)
}

fn decode_portable_snapshot_redb_with_source_hash(
    bytes: &[u8],
) -> AppResult<DecodedPortableSnapshot> {
    let decoded = read_portable_snapshot_redb(bytes)?;
    validate_portable_snapshot(&decoded.snapshot)?;
    Ok(decoded)
}

fn read_portable_snapshot_redb(bytes: &[u8]) -> AppResult<DecodedPortableSnapshot> {
    catch_unwind(AssertUnwindSafe(
        || -> AppResult<DecodedPortableSnapshot> {
            let temp = TempRedbFile::new("portable-snapshot-decode");
            fs::write(temp.path(), bytes)?;
            let db = Database::open(temp.path()).map_err(storage_error)?;
            let read = db.begin_read().map_err(storage_error)?;
            let meta_table = read
                .open_table(SNAPSHOT_META_TABLE)
                .map_err(storage_error)?;
            let meta_raw = meta_table
                .get(SNAPSHOT_META_KEY)
                .map_err(storage_error)?
                .ok_or_else(|| {
                    AppError::Config("portable snapshot is missing metadata".to_string())
                })?
                .value()
                .to_string();
            let meta: PortableSnapshotMeta = serde_json::from_str(&meta_raw)?;

            if meta.schema_version == 2 {
                decode_v2_snapshot(&read, meta)
            } else if meta.schema_version == PORTABLE_SNAPSHOT_SCHEMA_VERSION {
                let entities = read_string_table(&read, SNAPSHOT_ENTITIES_TABLE)?;
                decode_v3_snapshot(meta, &entities)
            } else {
                Err(AppError::Config(format!(
                    "Unsupported portable snapshot version {}",
                    meta.schema_version
                )))
            }
        },
    ))
    .unwrap_or_else(|_| {
        Err(AppError::Storage(
            "Portable snapshot redb payload is corrupt or incomplete".to_string(),
        ))
    })
}

pub fn encode_portable_snapshot(snapshot: &PortableSnapshot) -> AppResult<Vec<u8>> {
    validate_portable_snapshot(snapshot)?;

    let redb_payload = encode_portable_snapshot_redb(snapshot)?;
    let compressed_payload = encode_compressed_snapshot_payload(&redb_payload)?;
    log_snapshot_compression(snapshot, redb_payload.len(), compressed_payload.len());
    Ok(compressed_payload)
}

fn encode_portable_snapshot_redb(snapshot: &PortableSnapshot) -> AppResult<Vec<u8>> {
    let temp = TempRedbFile::new("portable-snapshot-encode");
    {
        let db = Database::create(temp.path()).map_err(storage_error)?;
        let txn = db.begin_write().map_err(storage_error)?;
        {
            let mut meta = txn.open_table(SNAPSHOT_META_TABLE).map_err(storage_error)?;
            let meta_content = serde_json::to_string(&PortableSnapshotMeta::from(snapshot))?;
            meta.insert(SNAPSHOT_META_KEY, meta_content.as_str())
                .map_err(storage_error)?;
        }
        let mut entities = txn
            .open_table(SNAPSHOT_ENTITIES_TABLE)
            .map_err(storage_error)?;
        insert_entity(&mut entities, "settings", &snapshot.settings)?;
        insert_entity(&mut entities, "sessions", &snapshot.sessions)?;
        insert_entity(&mut entities, "keys", &snapshot.keys)?;
        insert_entity(&mut entities, "passwords", &snapshot.passwords)?;
        insert_entity(&mut entities, "credentials", &snapshot.credentials)?;
        insert_entity(&mut entities, "otp", &snapshot.otp)?;
        insert_entity(&mut entities, "proxies", &snapshot.proxies)?;
        insert_entity(&mut entities, "proxy_groups", &snapshot.proxy_groups)?;
        insert_entity(&mut entities, "tunnels", &snapshot.tunnels)?;
        insert_entity(&mut entities, "tunnel_groups", &snapshot.tunnel_groups)?;
        insert_entity(&mut entities, "quick_commands", &snapshot.quick_commands)?;
        insert_entity(&mut entities, "history", &snapshot.history)?;
        insert_entity(
            &mut entities,
            "master_key_token",
            &snapshot.master_key_token,
        )?;
        insert_entity(&mut entities, "known_hosts", &snapshot.known_hosts)?;
        insert_entity(&mut entities, "notes", &snapshot.notes)?;
        drop(entities);
        txn.commit().map_err(storage_error)?;
    }

    fs::read(temp.path()).map_err(Into::into)
}

#[cfg(any(test, feature = "test-support"))]
pub fn encode_v3_raw_snapshot_redb_for_test(
    snapshot: &PortableSnapshot,
    entities: &BTreeMap<String, String>,
    payload_hash: String,
) -> Vec<u8> {
    let temp = TempRedbFile::new("portable-snapshot-raw-test");
    {
        let db = Database::create(temp.path()).expect("create db");
        let txn = db.begin_write().expect("begin write");
        {
            let mut meta = txn.open_table(SNAPSHOT_META_TABLE).expect("open meta");
            let mut meta_value = PortableSnapshotMeta::from(snapshot);
            meta_value.payload_hash = payload_hash;
            let meta_content = serde_json::to_string(&meta_value).expect("meta json");
            meta.insert(SNAPSHOT_META_KEY, meta_content.as_str())
                .expect("insert meta");
        }
        let mut table = txn
            .open_table(SNAPSHOT_ENTITIES_TABLE)
            .expect("open entities");
        for (key, value) in entities {
            table
                .insert(key.as_str(), value.as_str())
                .expect("insert entity");
        }
        drop(table);
        txn.commit().expect("commit");
    }
    fs::read(temp.path()).expect("read redb")
}

fn encode_compressed_snapshot_payload(redb_payload: &[u8]) -> AppResult<Vec<u8>> {
    // Do not produce an archive the shared decoder cannot restore.
    if redb_payload.len() as u64 > MAX_COMPRESSED_SNAPSHOT_PAYLOAD_BYTES {
        return Err(AppError::Config(
            "Snapshot exceeds the 50 MiB restore limit".into(),
        ));
    }
    let cursor = Cursor::new(Vec::new());
    let mut zip = zip::ZipWriter::new(cursor);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    zip.start_file(SNAPSHOT_ZIP_MANIFEST_NAME, options)
        .map_err(zip_error)?;
    zip.write_all(
        br#"{"format":"nyaterm-portable-snapshot-zip","version":1,"payload":"snapshot.redb"}"#,
    )?;
    zip.start_file(SNAPSHOT_ZIP_PAYLOAD_NAME, options)
        .map_err(zip_error)?;
    zip.write_all(redb_payload)?;

    let cursor = zip.finish().map_err(zip_error)?;
    Ok(cursor.into_inner())
}

fn decode_compressed_snapshot_payload(bytes: &[u8]) -> AppResult<Vec<u8>> {
    let cursor = Cursor::new(bytes);
    let mut archive = zip::ZipArchive::new(cursor).map_err(zip_error)?;
    let mut entry = archive
        .by_name(SNAPSHOT_ZIP_PAYLOAD_NAME)
        .map_err(zip_error)?;
    if entry.size() > MAX_COMPRESSED_SNAPSHOT_PAYLOAD_BYTES {
        return Err(zip_error(format!(
            "decompressed snapshot payload exceeds maximum allowed size of {} bytes",
            MAX_COMPRESSED_SNAPSHOT_PAYLOAD_BYTES
        )));
    }
    let mut payload = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = entry.read(&mut buf).map_err(zip_error)?;
        if n == 0 {
            break;
        }
        payload.extend_from_slice(&buf[..n]);
        if u64::try_from(payload.len()).unwrap_or(u64::MAX) > MAX_COMPRESSED_SNAPSHOT_PAYLOAD_BYTES
        {
            return Err(zip_error(format!(
                "decompressed snapshot payload exceeds maximum allowed size of {} bytes",
                MAX_COMPRESSED_SNAPSHOT_PAYLOAD_BYTES
            )));
        }
    }
    Ok(payload)
}

fn is_zip_snapshot_payload(bytes: &[u8]) -> bool {
    bytes.starts_with(b"PK\x03\x04")
}

fn log_snapshot_compression(
    snapshot: &PortableSnapshot,
    original_bytes: usize,
    compressed_bytes: usize,
) {
    let saved_bytes = original_bytes as i128 - compressed_bytes as i128;
    let reduction_percent = if original_bytes == 0 {
        0.0
    } else {
        (saved_bytes as f64 / original_bytes as f64) * 100.0
    };
    tracing::info!(
        snapshot_kind = ?snapshot.snapshot_kind,
        original_bytes,
        compressed_bytes,
        saved_bytes,
        reduction_percent,
        "Portable snapshot compressed before encryption"
    );
}
