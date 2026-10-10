use std::collections::HashSet;

/// An intentionally additive two-way merge: without a common ancestor, a
/// changed/deleted entity cannot be safely distinguished from an addition.
pub fn merge_additive_sync_connections(
    local: &PortableSnapshot,
    remote: &PortableSnapshot,
) -> AppResult<PortableSnapshot> {
    if local.snapshot_kind != PortableSnapshotKind::Sync
        || remote.snapshot_kind != PortableSnapshotKind::Sync
    {
        return Err(AppError::Config(
            "Cloud sync merge requires two Sync snapshots".into(),
        ));
    }
    validate_portable_snapshot(local)?;
    validate_portable_snapshot(remote)?;
    crate::storage::backup::validate_backup_data(local)?;
    crate::storage::backup::validate_backup_data(remote)?;

    macro_rules! unchanged {
        ($field:ident) => {
            if serde_json::to_value(&local.$field)? != serde_json::to_value(&remote.$field)? {
                return Err(AppError::Config(format!(
                    "Cloud sync merge refused: {} differs; use upload or download instead",
                    stringify!($field)
                )));
            }
        };
    }
    unchanged!(settings);
    unchanged!(credentials);
    unchanged!(tunnels);
    unchanged!(tunnel_groups);
    unchanged!(quick_commands);
    unchanged!(history);
    unchanged!(master_key_token);
    unchanged!(known_hosts);
    unchanged!(notes);

    let mut merged = local.clone();
    merge_by_id(
        &mut merged.sessions.connections,
        &remote.sessions.connections,
        |entry| &entry.id,
        "connection",
    )?;
    merge_by_id(
        &mut merged.sessions.groups,
        &remote.sessions.groups,
        |entry| &entry.id,
        "connection group",
    )?;
    merge_by_id(
        &mut merged.sessions.custom_icons,
        &remote.sessions.custom_icons,
        |entry| &entry.id,
        "custom icon",
    )?;
    merge_by_id(&mut merged.keys.keys, &remote.keys.keys, |entry| &entry.id, "SSH key")?;
    merge_by_id(
        &mut merged.passwords.passwords,
        &remote.passwords.passwords,
        |entry| &entry.id,
        "saved account",
    )?;
    merge_by_id(&mut merged.otp.entries, &remote.otp.entries, |entry| &entry.id, "OTP")?;
    merge_by_id(&mut merged.proxies, &remote.proxies, |entry| &entry.id, "proxy")?;
    merge_by_id(
        &mut merged.proxy_groups,
        &remote.proxy_groups,
        |entry| &entry.id,
        "proxy group",
    )?;

    // Match the ordering produced by redb-backed load_* when rebuilding the
    // local Sync snapshot after applying the merged entities.
    merged.sessions.groups.sort_by(|a, b| {
        a.sort_order.cmp(&b.sort_order).then(a.name.cmp(&b.name)).then(a.id.cmp(&b.id))
    });
    merged.sessions.connections.sort_by(|a, b| {
        a.group_id
            .cmp(&b.group_id)
            .then(a.sort_order.cmp(&b.sort_order))
            .then(a.name.cmp(&b.name))
            .then(a.id.cmp(&b.id))
    });
    merged.sessions.custom_icons.sort_by(|a, b| {
        a.created_at_ms
            .cmp(&b.created_at_ms)
            .then(a.name.cmp(&b.name))
            .then(a.id.cmp(&b.id))
    });
    merged.keys.keys.sort_by(|a, b| a.sort_order.cmp(&b.sort_order).then(a.id.cmp(&b.id)));
    merged.passwords.passwords.sort_by(|a, b| a.sort_order.cmp(&b.sort_order).then(a.id.cmp(&b.id)));
    merged.otp.entries.sort_by(|a, b| a.id.cmp(&b.id));
    merged.proxies.sort_by(|a, b| a.id.cmp(&b.id));

    validate_merged_connection_references(&merged)?;
    merged.payload_hash = calculate_payload_hash(&merged)?;
    validate_portable_snapshot(&merged)?;
    crate::storage::backup::validate_backup_data(&merged)?;
    Ok(merged)
}

fn merge_by_id<T: Clone + Serialize>(
    local: &mut Vec<T>,
    remote: &[T],
    id: impl Fn(&T) -> &str,
    kind: &str,
) -> AppResult<()> {
    for entry in remote {
        if let Some(existing) = local.iter().find(|item| id(item) == id(entry)) {
            if serde_json::to_value(existing)? != serde_json::to_value(entry)? {
                return Err(AppError::Config(format!(
                    "Cloud sync merge refused: conflicting {kind} ID '{}'",
                    id(entry)
                )));
            }
        } else {
            local.push(entry.clone());
        }
    }
    Ok(())
}

fn validate_merged_connection_references(snapshot: &PortableSnapshot) -> AppResult<()> {
    let sessions = &snapshot.sessions;
    let groups: HashSet<_> = sessions.groups.iter().map(|group| group.id.as_str()).collect();
    let connections: HashSet<_> = sessions.connections.iter().map(|entry| entry.id.as_str()).collect();
    let icons: HashSet<_> = sessions.custom_icons.iter().map(|entry| entry.id.as_str()).collect();
    let accounts: HashSet<_> = snapshot.passwords.passwords.iter().map(|entry| entry.id.as_str()).collect();
    let keys: HashSet<_> = snapshot.keys.keys.iter().map(|entry| entry.id.as_str()).collect();
    let otp: HashSet<_> = snapshot.otp.entries.iter().map(|entry| entry.id.as_str()).collect();
    let proxies: HashSet<_> = snapshot.proxies.iter().map(|entry| entry.id.as_str()).collect();
    let proxy_groups: HashSet<_> = snapshot.proxy_groups.iter().map(|entry| entry.id.as_str()).collect();

    fn require(id: Option<&str>, ids: &HashSet<&str>, kind: &str) -> AppResult<()> {
        if let Some(id) = id {
            if id.is_empty() || !ids.contains(id) {
                return Err(AppError::Config(format!(
                    "Cloud sync merge refused: missing {kind} reference '{id}'"
                )));
            }
        }
        Ok(())
    }

    for group in &sessions.groups {
        require(group.parent_id.as_deref(), &groups, "parent group")?;
    }
    for proxy in &snapshot.proxies {
        require(proxy.group_id.as_deref(), &proxy_groups, "proxy group")?;
    }
    for connection in &sessions.connections {
        require(connection.group_id.as_deref(), &groups, "connection group")?;
        if let Some(icon) = connection.icon.as_deref() {
            if icon.starts_with("custom-icon-") {
                require(Some(icon), &icons, "custom icon")?;
            }
        }
        if let Some(auth) = &connection.auth {
            require(auth.account_id.as_deref(), &accounts, "account")?;
            require(auth.password_id.as_deref(), &accounts, "saved password")?;
            require(auth.key_id.as_deref(), &keys, "SSH key")?;
            require(auth.otp_id.as_deref(), &otp, "OTP")?;
        }
        if let Some(network) = &connection.network {
            require(network.proxy_id.as_deref(), &proxies, "proxy")?;
            require(network.proxy_jump_id.as_deref(), &connections, "jump connection")?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod merge_tests {
    use super::*;

    fn snapshot() -> PortableSnapshot {
        let mut snapshot = PortableSnapshot {
            schema_version: PORTABLE_SNAPSHOT_SCHEMA_VERSION,
            snapshot_kind: PortableSnapshotKind::Sync,
            revision_id: "revision".into(),
            device_id: "device".into(),
            created_at_ms: 1,
            payload_hash: String::new(),
            app_version: "test".into(),
            settings: PortableAppSettings::from_app_settings(
                &config::AppSettings::default(),
                &PortableSnapshotKind::Sync,
            ),
            sessions: Default::default(),
            keys: Default::default(),
            passwords: Default::default(),
            credentials: Default::default(),
            otp: Default::default(),
            proxies: Default::default(),
            proxy_groups: Default::default(),
            tunnels: Default::default(),
            tunnel_groups: Default::default(),
            quick_commands: Default::default(),
            history: Default::default(),
            master_key_token: None,
            known_hosts: String::new(),
            notes: Default::default(),
        };
        rehash(&mut snapshot);
        snapshot
    }

    fn rehash(snapshot: &mut PortableSnapshot) {
        snapshot.payload_hash = calculate_payload_hash(snapshot).expect("hash");
    }

    fn connection(id: &str, name: &str) -> config::SavedConnection {
        serde_json::from_value(serde_json::json!({
            "id":id, "name":name, "type":"ssh", "host":"example.net", "port":22,
            "username":"root"
        }))
        .expect("saved connection")
    }

    #[test]
    fn merge_additive_sync_connections_preserves_both_sides_and_shared_ids() {
        let mut local = snapshot();
        let mut remote = snapshot();
        local.sessions.connections.push(connection("local", "Local"));
        local.sessions.connections.push(connection("shared", "Shared"));
        remote.sessions.connections.push(connection("shared", "Shared"));
        remote.sessions.connections.push(connection("remote", "Remote"));
        rehash(&mut local);
        rehash(&mut remote);

        let merged = merge_additive_sync_connections(&local, &remote).expect("merge");
        assert_eq!(
            merged.sessions.connections.iter().map(|connection| connection.id.as_str()).collect::<Vec<_>>(),
            vec!["local", "remote", "shared"]
        );
        assert_eq!(merged.device_id, local.device_id);
        validate_portable_snapshot(&merged).expect("valid merged hash");
    }

    #[test]
    fn merge_additive_sync_connections_preserves_hierarchy_and_referenced_secrets() {
        let local = snapshot();
        let mut remote = snapshot();
        remote.sessions.groups = serde_json::from_value(serde_json::json!([
            {"id":"group-child","name":"Child","parent_id":"group-root"},
            {"id":"group-root","name":"Root"}
        ])).unwrap();
        remote.sessions.custom_icons = serde_json::from_value(serde_json::json!([{
            "id":"custom-icon-a","name":"Icon","data_url":"data:image/png;base64,YQ==",
            "created_at_ms":1,"updated_at_ms":1
        }])).unwrap();
        remote.passwords.passwords = serde_json::from_value(serde_json::json!([
            {"id":"account","name":"Account","username":"root","password":"encrypted"}
        ])).unwrap();
        remote.keys.keys = serde_json::from_value(serde_json::json!([
            {"id":"key","name":"Key","key":"encrypted-key"}
        ])).unwrap();
        remote.otp.entries = serde_json::from_value(serde_json::json!([
            {"id":"otp","otp_type":"totp","issuer":"Issuer","username":"root"}
        ])).unwrap();
        remote.proxy_groups = serde_json::from_value(serde_json::json!([
            {"id":"proxy-group","name":"Proxies"}
        ])).unwrap();
        remote.proxies = serde_json::from_value(serde_json::json!([
            {"id":"proxy","name":"Proxy","group_id":"proxy-group"}
        ])).unwrap();
        remote.sessions.connections.push(connection("jump", "Jump"));
        let mut client = connection("client", "Client");
        client.group_id = Some("group-child".into());
        client.icon = Some("custom-icon-a".into());
        client.auth = serde_json::from_value(serde_json::json!({
            "account_id":"account","key_id":"key","otp_id":"otp"
        })).unwrap();
        client.network = serde_json::from_value(serde_json::json!({
            "proxy_id":"proxy","proxy_jump_id":"jump"
        })).unwrap();
        remote.sessions.connections.push(client);
        rehash(&mut remote);

        let merged = merge_additive_sync_connections(&local, &remote).expect("linked merge");
        assert_eq!(merged.sessions.groups.len(), 2);
        assert_eq!(merged.sessions.custom_icons.len(), 1);
        assert_eq!(merged.passwords.passwords.len(), 1);
        assert_eq!(merged.keys.keys.len(), 1);
        assert_eq!(merged.otp.entries.len(), 1);
        assert_eq!(merged.proxies.len(), 1);
        assert_eq!(merged.sessions.connections.len(), 2);
        validate_portable_snapshot(&merged).expect("valid payload");
    }

    #[test]
    fn merge_additive_sync_connections_rejects_conflicting_and_duplicate_ids() {
        let mut local = snapshot();
        let mut remote = snapshot();
        local.sessions.connections.push(connection("same", "Local"));
        remote.sessions.connections.push(connection("same", "Remote"));
        rehash(&mut local);
        rehash(&mut remote);
        assert!(merge_additive_sync_connections(&local, &remote)
            .unwrap_err().to_string().contains("conflicting connection ID"));
        remote.sessions.connections.push(connection("same", "Duplicate"));
        rehash(&mut remote);
        assert!(merge_additive_sync_connections(&local, &remote).is_err());
        remote.sessions.connections[1].id.clear();
        rehash(&mut remote);
        assert!(merge_additive_sync_connections(&local, &remote).is_err());
    }

    #[test]
    fn merge_additive_sync_connections_rejects_unrelated_changes_and_missing_refs() {
        let local = snapshot();
        let mut remote = snapshot();
        remote.known_hosts = "changed".into();
        rehash(&mut remote);
        assert!(merge_additive_sync_connections(&local, &remote)
            .unwrap_err().to_string().contains("known_hosts differs"));
        remote.known_hosts.clear();
        remote.sessions.connections.push(connection("client", "Client"));
        remote.sessions.connections[0].group_id = Some("missing".into());
        rehash(&mut remote);
        assert!(merge_additive_sync_connections(&local, &remote)
            .unwrap_err().to_string().contains("missing connection group"));
    }

    #[test]
    fn merge_additive_sync_connections_rejects_corruption_and_backup_kind() {
        let local = snapshot();
        let mut remote = snapshot();
        remote.payload_hash = "corrupted".into();
        assert!(merge_additive_sync_connections(&local, &remote).is_err());
        remote.snapshot_kind = PortableSnapshotKind::Backup;
        rehash(&mut remote);
        assert!(merge_additive_sync_connections(&local, &remote)
            .unwrap_err().to_string().contains("two Sync snapshots"));
    }
}
