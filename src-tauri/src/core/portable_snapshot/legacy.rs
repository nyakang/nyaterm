#[derive(Deserialize, Default)]
struct V2ProxiesConfig {
    #[serde(default)]
    proxies: Vec<config::ProxyConfig>,
}

#[derive(Deserialize, Default)]
struct V2HistoryStore {
    #[serde(default)]
    entries: Vec<crate::core::history::HistoryEntry>,
}

#[derive(Serialize)]
struct V2SnapshotHashInput<'a> {
    json_docs: &'a BTreeMap<String, String>,
    text_docs: &'a BTreeMap<String, String>,
}

fn decode_v2_snapshot(
    read: &redb::ReadTransaction,
    meta: PortableSnapshotMeta,
) -> AppResult<DecodedPortableSnapshot> {
    let json_docs = read_string_table(read, SNAPSHOT_V2_JSON_DOCS_TABLE)?;
    let text_docs = read_string_table(read, SNAPSHOT_V2_TEXT_DOCS_TABLE)?;
    let source_payload_hash = meta.payload_hash.clone();
    let expected = calculate_v2_payload_hash(&json_docs, &text_docs)?;
    if expected != source_payload_hash {
        return Err(AppError::Crypto(
            "Portable snapshot payload hash mismatch".to_string(),
        ));
    }

    let settings = if let Some(raw) = json_docs.get(SNAPSHOT_JSON_PORTABLE_SETTINGS) {
        serde_json::from_str(raw)?
    } else if let Some(raw) = json_docs.get("settings") {
        PortableAppSettings::from_app_settings(
            &serde_json::from_str::<AppSettings>(raw)?,
            &meta.snapshot_kind,
        )
    } else {
        PortableAppSettings::from_app_settings(&AppSettings::default(), &meta.snapshot_kind)
    };

    let history = json_docs
        .get("history")
        .map(|raw| serde_json::from_str::<V2HistoryStore>(raw).map(|store| store.entries))
        .transpose()?
        .unwrap_or_default();
    let proxies = json_docs
        .get("proxies")
        .map(|raw| serde_json::from_str::<V2ProxiesConfig>(raw).map(|cfg| cfg.proxies))
        .transpose()?
        .unwrap_or_default();
    let tunnels = json_docs
        .get("tunnels")
        .map(|raw| serde_json::from_str::<config::TunnelsConfig>(raw).map(|cfg| cfg.tunnels))
        .transpose()?
        .unwrap_or_default();
    let proxy_groups = parse_v2_json_doc(&json_docs, "proxy-groups")?;
    let tunnel_groups = parse_v2_json_doc(&json_docs, "tunnel-groups")?;

    let mut snapshot = PortableSnapshot {
        schema_version: PORTABLE_SNAPSHOT_SCHEMA_VERSION,
        snapshot_kind: meta.snapshot_kind,
        revision_id: meta.revision_id,
        device_id: meta.device_id,
        created_at_ms: meta.created_at_ms,
        payload_hash: String::new(),
        app_version: meta.app_version,
        settings,
        sessions: parse_v2_json_doc(&json_docs, "sessions")?,
        keys: parse_v2_json_doc(&json_docs, "keys")?,
        passwords: parse_v2_json_doc(&json_docs, "passwords")?,
        credentials: parse_v2_json_doc(&json_docs, "credentials")?,
        otp: parse_v2_json_doc(&json_docs, "otp")?,
        proxies,
        proxy_groups,
        tunnels,
        tunnel_groups,
        quick_commands: parse_v2_json_doc(&json_docs, "quick-command")?,
        history,
        master_key_token: text_docs.get("master.key").cloned(),
        known_hosts: text_docs.get("known_hosts").cloned().unwrap_or_default(),
        notes: config::NotesSnapshot::default(),
    };
    normalize_portable_session_tags(&mut snapshot.sessions);
    snapshot.payload_hash = calculate_payload_hash(&snapshot)?;
    log_snapshot_hash_normalized(&snapshot, &source_payload_hash);
    Ok(DecodedPortableSnapshot {
        snapshot,
        source_payload_hash,
    })
}

fn parse_v2_json_doc<T>(docs: &BTreeMap<String, String>, key: &str) -> AppResult<T>
where
    T: serde::de::DeserializeOwned + Default,
{
    docs.get(key)
        .map(|raw| serde_json::from_str(raw).map_err(Into::into))
        .transpose()
        .map(|value| value.unwrap_or_default())
}

fn calculate_v2_payload_hash(
    json_docs: &BTreeMap<String, String>,
    text_docs: &BTreeMap<String, String>,
) -> AppResult<String> {
    let payload_bytes = serde_json::to_vec(&V2SnapshotHashInput {
        json_docs,
        text_docs,
    })?;
    Ok(hex::encode(Sha256::digest(&payload_bytes)))
}
