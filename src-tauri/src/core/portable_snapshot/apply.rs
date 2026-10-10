pub async fn apply_portable_snapshot(
    app: &AppHandle,
    snapshot: &PortableSnapshot,
) -> AppResult<()> {
    validate_portable_snapshot(snapshot)?;
    install_snapshot_master_key_token(snapshot)?;

    let mut sessions = snapshot.sessions.clone();
    if snapshot.snapshot_kind == PortableSnapshotKind::Sync {
        let current_sessions = config::load_sessions(app).unwrap_or_default();
        preserve_device_local_sessions(&mut sessions, &current_sessions);
    } else {
        normalize_backup_sessions_for_platform(
            &mut sessions,
            AgentEndpointTargetPlatform::current(),
        )?;
    }
    config::save_sessions(app, &sessions)?;
    config::save_keys(app, &snapshot.keys)?;
    config::save_passwords(app, &snapshot.passwords)?;
    config::save_credentials(app, &snapshot.credentials)?;
    config::save_otp_entries(app, &snapshot.otp)?;
    config::save_proxies(app, &snapshot.proxies)?;
    config::save_proxy_groups(app, &snapshot.proxy_groups)?;
    config::save_tunnels(app, &snapshot.tunnels)?;
    config::save_tunnel_groups(app, &snapshot.tunnel_groups)?;
    config::save_quick_commands(app, &snapshot.quick_commands)?;
    crate::storage::replace_command_history_entries(&snapshot.history)?;
    crate::storage::replace_notes_snapshot(&snapshot.notes)?;

    let merged = snapshot.settings.clone().apply_to(
        config::load_app_settings(app).unwrap_or_default(),
        &snapshot.snapshot_kind,
    );
    let mut persisted = merged.clone();
    persisted.cloud_sync = config::encrypt_cloud_sync_settings(merged.cloud_sync.clone())?;
    persisted.ai = config::encrypt_ai_settings(merged.ai.clone())?;
    config::save_app_settings(app, &persisted)?;

    if !snapshot.known_hosts.is_empty() {
        crate::storage::replace_known_hosts_export(&snapshot.known_hosts)?;
    }

    let quick_commands_store = app.state::<Arc<QuickCommandsStore>>();
    quick_commands_store.load_from_disk(app)?;

    let session_manager = app.state::<Arc<SessionManager>>();
    session_manager
        .inner()
        .as_ref()
        .reload_history_from_storage()
        .await?;

    let _ = app.emit("connections-changed", ());
    let _ = app.emit("quick-commands-changed", ());
    let _ = app.emit("settings-changed", ());
    let _ = app.emit("command-history-changed", ());
    let _ = app.emit(
        "notes-changed",
        crate::config::NotesChangedEvent {
            kind: "replaced".to_string(),
            node_kind: None,
            ids: Vec::new(),
            folders: Vec::new(),
            notes: Vec::new(),
            tree_changed: Some(true),
        },
    );

    Ok(())
}

/// Only update merged connection data after a verified remote publish. Do not
/// invoke apply_portable_snapshot: ordinary Sync restores overwrite unrelated
/// data including local command history.
pub fn apply_merged_connection_entities(
    app: &AppHandle,
    snapshot: &PortableSnapshot,
) -> AppResult<()> {
    validate_portable_snapshot(snapshot)?;
    let mut sessions = snapshot.sessions.clone();
    let current_sessions = config::load_sessions(app)?;
    preserve_device_local_sessions(&mut sessions, &current_sessions);
    config::apply_merged_sync_entities(snapshot, &sessions)?;
    let _ = app.emit("connections-changed", ());
    Ok(())
}

fn install_snapshot_master_key_token(snapshot: &PortableSnapshot) -> AppResult<()> {
    let Some(master_key) = &snapshot.master_key_token else {
        return Ok(());
    };
    crate::storage::save_master_key_token(master_key)?;
    crate::utils::crypto::verify_master_key_token()?;
    Ok(())
}
