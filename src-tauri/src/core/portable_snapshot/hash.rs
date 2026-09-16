fn validate_portable_snapshot(snapshot: &PortableSnapshot) -> AppResult<()> {
    if snapshot.schema_version != PORTABLE_SNAPSHOT_SCHEMA_VERSION {
        return Err(AppError::Config(format!(
            "Unsupported portable snapshot version {}",
            snapshot.schema_version
        )));
    }
    let actual = calculate_payload_hash(snapshot)?;
    if actual != snapshot.payload_hash {
        return Err(AppError::Crypto(
            "Portable snapshot payload hash mismatch".to_string(),
        ));
    }
    Ok(())
}

pub(crate) fn calculate_payload_hash(snapshot: &PortableSnapshot) -> AppResult<String> {
    let payload_bytes = serde_json::to_vec(&SnapshotHashInput {
        settings: &snapshot.settings,
        sessions: &snapshot.sessions,
        keys: &snapshot.keys,
        passwords: &snapshot.passwords,
        credentials: &snapshot.credentials,
        otp: &snapshot.otp,
        proxies: &snapshot.proxies,
        proxy_groups: &snapshot.proxy_groups,
        tunnels: &snapshot.tunnels,
        tunnel_groups: &snapshot.tunnel_groups,
        quick_commands: &snapshot.quick_commands,
        history: &snapshot.history,
        master_key_token: &snapshot.master_key_token,
        known_hosts: &snapshot.known_hosts,
        notes: &snapshot.notes,
    })?;
    Ok(hex::encode(Sha256::digest(&payload_bytes)))
}

fn decode_v3_snapshot(
    meta: PortableSnapshotMeta,
    entities: &BTreeMap<String, String>,
) -> AppResult<DecodedPortableSnapshot> {
    let source_payload_hash = meta.payload_hash.clone();
    let expected = calculate_v3_raw_payload_hash(entities)?;
    if expected != source_payload_hash {
        return Err(AppError::Crypto(
            "Portable snapshot payload hash mismatch".to_string(),
        ));
    }

    let mut snapshot = PortableSnapshot {
        schema_version: meta.schema_version,
        snapshot_kind: meta.snapshot_kind,
        revision_id: meta.revision_id,
        device_id: meta.device_id,
        created_at_ms: meta.created_at_ms,
        payload_hash: String::new(),
        app_version: meta.app_version,
        settings: read_entity(entities, "settings")?,
        sessions: read_entity_or_default(entities, "sessions")?,
        keys: read_entity_or_default(entities, "keys")?,
        passwords: read_entity_or_default(entities, "passwords")?,
        credentials: read_entity_or_default(entities, "credentials")?,
        otp: read_entity_or_default(entities, "otp")?,
        proxies: read_entity_or_default(entities, "proxies")?,
        proxy_groups: read_entity_or_default(entities, "proxy_groups")?,
        tunnels: read_entity_or_default(entities, "tunnels")?,
        tunnel_groups: read_entity_or_default(entities, "tunnel_groups")?,
        quick_commands: read_entity_or_default(entities, "quick_commands")?,
        history: read_entity_or_default(entities, "history")?,
        master_key_token: read_entity_or_default(entities, "master_key_token")?,
        known_hosts: read_entity_or_default(entities, "known_hosts")?,
        notes: read_entity_or_default(entities, "notes")?,
    };
    normalize_portable_session_tags(&mut snapshot.sessions);
    snapshot.payload_hash = calculate_payload_hash(&snapshot)?;
    log_snapshot_hash_normalized(&snapshot, &source_payload_hash);
    Ok(DecodedPortableSnapshot {
        snapshot,
        source_payload_hash,
    })
}

fn log_snapshot_hash_normalized(snapshot: &PortableSnapshot, source_payload_hash: &str) {
    if snapshot.payload_hash == source_payload_hash {
        return;
    }
    tracing::info!(
        revision = %snapshot.revision_id,
        app_version = %snapshot.app_version,
        source_payload_hash = %source_payload_hash,
        normalized_payload_hash = %snapshot.payload_hash,
        "Portable snapshot payload hash normalized after decoding legacy entity shape"
    );
}

pub(crate) fn calculate_v3_raw_payload_hash(entities: &BTreeMap<String, String>) -> AppResult<String> {
    let settings = read_raw_entity(entities, "settings")?;
    let sessions = read_raw_entity(entities, "sessions")?;
    let keys = read_raw_entity(entities, "keys")?;
    let passwords = read_raw_entity(entities, "passwords")?;
    let credentials = read_raw_entity(entities, "credentials")?;
    let otp = read_raw_entity(entities, "otp")?;
    let proxies = read_raw_entity(entities, "proxies")?;
    let tunnels = read_raw_entity(entities, "tunnels")?;
    let quick_commands = read_raw_entity(entities, "quick_commands")?;
    let history = read_raw_entity(entities, "history")?;
    let master_key_token = read_raw_entity(entities, "master_key_token")?;
    let known_hosts = read_raw_entity(entities, "known_hosts")?;

    let payload_bytes = serde_json::to_vec(&LegacySnapshotRawHashInput {
        settings: settings.as_ref(),
        sessions: sessions.as_ref(),
        keys: keys.as_ref(),
        passwords: passwords.as_ref(),
        credentials: credentials.as_ref(),
        otp: otp.as_ref(),
        proxies: proxies.as_ref(),
        tunnels: tunnels.as_ref(),
        quick_commands: quick_commands.as_ref(),
        history: history.as_ref(),
        master_key_token: master_key_token.as_ref(),
        known_hosts: known_hosts.as_ref(),
    })?;
    let hash = hex::encode(Sha256::digest(&payload_bytes));
    if entities.contains_key("proxy_groups") || entities.contains_key("tunnel_groups") {
        let proxy_groups = read_raw_entity(entities, "proxy_groups")?;
        let tunnel_groups = read_raw_entity(entities, "tunnel_groups")?;
        if entities.contains_key("notes") {
            let notes = read_raw_entity(entities, "notes")?;
            let payload_bytes = serde_json::to_vec(&SnapshotRawHashInputWithNotes {
                settings: settings.as_ref(),
                sessions: sessions.as_ref(),
                keys: keys.as_ref(),
                passwords: passwords.as_ref(),
                credentials: credentials.as_ref(),
                otp: otp.as_ref(),
                proxies: proxies.as_ref(),
                proxy_groups: proxy_groups.as_ref(),
                tunnels: tunnels.as_ref(),
                tunnel_groups: tunnel_groups.as_ref(),
                quick_commands: quick_commands.as_ref(),
                history: history.as_ref(),
                master_key_token: master_key_token.as_ref(),
                known_hosts: known_hosts.as_ref(),
                notes: notes.as_ref(),
            })?;
            return Ok(hex::encode(Sha256::digest(&payload_bytes)));
        }
        let payload_bytes = serde_json::to_vec(&SnapshotRawHashInput {
            settings: settings.as_ref(),
            sessions: sessions.as_ref(),
            keys: keys.as_ref(),
            passwords: passwords.as_ref(),
            credentials: credentials.as_ref(),
            otp: otp.as_ref(),
            proxies: proxies.as_ref(),
            proxy_groups: proxy_groups.as_ref(),
            tunnels: tunnels.as_ref(),
            tunnel_groups: tunnel_groups.as_ref(),
            quick_commands: quick_commands.as_ref(),
            history: history.as_ref(),
            master_key_token: master_key_token.as_ref(),
            known_hosts: known_hosts.as_ref(),
        })?;
        return Ok(hex::encode(Sha256::digest(&payload_bytes)));
    }
    Ok(hash)
}

