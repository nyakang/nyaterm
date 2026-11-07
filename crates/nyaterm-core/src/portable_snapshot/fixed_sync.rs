use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::portable_snapshot::{
    FIXED_SYNC_SNAPSHOT_SCHEMA_VERSION, PortableSnapshotError, PortableSnapshotKind,
    RawPortableSnapshot,
};
use crate::sync_schema::{
    CORE_KINDS, FIXED_CLOUD_SYNC_SCOPE, SyncFieldId, SyncProjection, apply_record, project_entities,
};

const COMPLETE_KINDS_ENTITY: &str = "fixed_sync_complete_kinds";

pub fn project_fixed_sync_entities(
    source: &RawPortableSnapshot,
) -> Result<SyncProjection, PortableSnapshotError> {
    let mut projection = project_entities(&source.entities, &FIXED_CLOUD_SYNC_SCOPE)
        .map_err(PortableSnapshotError::Codec)?;
    for (key, value) in &projection.values {
        let field = SyncFieldId::from_key(key).ok_or(PortableSnapshotError::CorruptPayload)?;
        if !crate::sync_schema::valid_fixed_cloud_sync_value(&field, &value.decode()?) {
            return Err(PortableSnapshotError::CorruptPayload);
        }
    }
    if source.meta.schema_version == FIXED_SYNC_SNAPSHOT_SCHEMA_VERSION {
        let raw = source
            .entities
            .get(COMPLETE_KINDS_ENTITY)
            .ok_or(PortableSnapshotError::CorruptPayload)?;
        let complete: std::collections::BTreeSet<String> = serde_json::from_str(raw)?;
        if !complete
            .iter()
            .all(|kind| CORE_KINDS.contains(&kind.as_str()))
            || !complete.is_subset(&projection.complete)
        {
            return Err(PortableSnapshotError::CorruptPayload);
        }
        projection.complete = complete;
    }
    Ok(projection)
}

/// Projection for cloud transport only. Full backups retain their public data
/// contract, and unsupported content stays in the original snapshot.
pub fn project_fixed_sync_snapshot(
    source: &RawPortableSnapshot,
) -> Result<RawPortableSnapshot, PortableSnapshotError> {
    let projection = project_fixed_sync_entities(source)?;
    let complete = projection.complete.clone();
    let mut records: BTreeMap<(String, String), Value> = BTreeMap::new();
    for (key, value) in projection.values {
        let field = SyncFieldId::from_key(&key).ok_or(PortableSnapshotError::CorruptPayload)?;
        let record = records
            .entry((field.resource.kind, field.resource.id))
            .or_insert_with(|| json!({}));
        if field.field != "$present" {
            record[field.field] = value.decode()?;
        }
    }
    let mut entities: BTreeMap<String, Value> = BTreeMap::from([
        (
            "sessions".into(),
            json!({"connections":[],"groups":[],"custom_icons":[]}),
        ),
        (
            "settings".into(),
            json!({"appearance":{"custom_themes":[]}}),
        ),
        ("proxies".into(), json!([])),
        ("proxy_groups".into(), json!([])),
        ("tunnels".into(), json!([])),
        ("tunnel_groups".into(), json!([])),
        (
            "quick_commands".into(),
            json!({"commands":[],"categories":[]}),
        ),
        ("notes".into(), json!({"notes":[],"folders":[]})),
    ]);
    for ((kind, id), fields) in records {
        let mut record =
            apply_record(&kind, &Value::Null, &fields).map_err(PortableSnapshotError::Codec)?;
        record["id"] = Value::String(id);
        let (entity, section) = match kind.as_str() {
            "connection" => ("sessions", "connections"),
            "connection_group" => ("sessions", "groups"),
            "connection_icon" => ("sessions", "custom_icons"),
            "proxy" => ("proxies", ""),
            "proxy_group" => ("proxy_groups", ""),
            "tunnel" => ("tunnels", ""),
            "tunnel_group" => ("tunnel_groups", ""),
            "command" => ("quick_commands", "commands"),
            "command_category" => ("quick_commands", "categories"),
            "note" => ("notes", "notes"),
            "note_folder" => ("notes", "folders"),
            "theme" => ("settings", ""),
            _ => return Err(PortableSnapshotError::CorruptPayload),
        };
        let collection = if kind == "theme" {
            &mut entities.get_mut(entity).expect("known entity")["appearance"]["custom_themes"]
        } else if section.is_empty() {
            entities.get_mut(entity).expect("known entity")
        } else {
            &mut entities.get_mut(entity).expect("known entity")[section]
        };
        collection.as_array_mut().expect("collection").push(record);
    }
    let mut result = RawPortableSnapshot::sync(&source.meta.device_id, &source.meta.app_version);
    result.meta = source.meta.clone();
    result.meta.snapshot_kind = PortableSnapshotKind::Sync;
    result.meta.schema_version = FIXED_SYNC_SNAPSHOT_SCHEMA_VERSION;
    result.entities.extend(
        entities
            .into_iter()
            .map(|(key, value)| (key, value.to_string())),
    );
    // Empty compatibility slots never prove deletion of unsupported content.
    result.entities.insert(
        COMPLETE_KINDS_ENTITY.into(),
        serde_json::to_string(&complete)?,
    );
    result.recalculate_hash()?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::portable_snapshot::{
        PortableSnapshotError, RawPortableSnapshot, project_fixed_sync_snapshot,
        validate_raw_snapshot,
    };

    #[test]
    fn fixed_snapshot_excludes_secrets_preferences_and_opaque_future_data() {
        let mut source = RawPortableSnapshot::backup("device", "supported-version");
        source.entities.insert("sessions".into(), json!({
            "connections":[{"id":"server","name":"Server","type":"ssh","host":"example.test",
                "password":"fixture-inline-secret", "auth":{"mode":"password","password":"fixture-auth-secret"},
                "future":{"token":"fixture-future-secret"}}],
            "groups":[{"id":"group","name":"Group"}],
            "custom_icons":[{"id":"icon","name":"Icon","data_url":"data:image/png;base64,AA=="}]
        }).to_string());
        source.entities.insert("proxies".into(), json!([{"id":"proxy","name":"Proxy","host":"proxy.test","protocol":"socks5","password":"fixture-proxy-secret"}]).to_string());
        source.entities.insert(
            "keys".into(),
            json!({"keys":[{"id":"key","private_key":"fixture-private-secret"}]}).to_string(),
        );
        source.entities.insert("settings".into(), json!({"appearance":{"theme":"local-theme","font_family":"local-font","custom_themes":[]},"ai":{"api_key":"fixture-ai-secret"},"keybindings":{"copy":"ctrl-x"},"cloud_sync":{"future":true}}).to_string());
        source.entities.insert(
            "future_entity".into(),
            json!({"value":"fixture-future-secret"}).to_string(),
        );
        source.recalculate_hash().unwrap();
        let before = source.clone();
        let fixed = project_fixed_sync_snapshot(&source).unwrap();
        validate_raw_snapshot(&fixed).unwrap();
        assert_eq!(source, before);
        assert_eq!(source.meta.schema_version, 3);
        assert_eq!(fixed.meta.schema_version, 4);
        let serialized = serde_json::to_string(&fixed).unwrap();
        for excluded in [
            "fixture-inline-secret",
            "fixture-auth-secret",
            "fixture-proxy-secret",
            "fixture-private-secret",
            "fixture-ai-secret",
            "fixture-future-secret",
            "local-theme",
            "local-font",
            "ctrl-x",
        ] {
            assert!(!serialized.contains(excluded));
        }
        let sessions: serde_json::Value =
            serde_json::from_str(&fixed.entities["sessions"]).unwrap();
        assert_eq!(sessions["connections"][0]["host"], "example.test");
        assert_eq!(sessions["groups"][0]["name"], "Group");
        assert_eq!(sessions["custom_icons"][0]["name"], "Icon");
        assert!(!fixed.entities.contains_key("future_entity"));
    }

    #[test]
    fn missing_legacy_collections_remain_incomplete_after_projection() {
        let mut source = RawPortableSnapshot::sync("device", "version");
        source.entities.remove("notes");
        source
            .entities
            .insert("settings".into(), json!({}).to_string());
        source.recalculate_hash().unwrap();
        let fixed = project_fixed_sync_snapshot(&source).unwrap();
        let projection = crate::portable_snapshot::project_fixed_sync_entities(&fixed).unwrap();
        assert!(!projection.complete.contains("note"));
        assert!(!projection.complete.contains("theme"));
        assert_eq!(project_fixed_sync_snapshot(&fixed).unwrap(), fixed);
        let mut invalid = fixed.clone();
        invalid.entities.insert(
            "fixed_sync_complete_kinds".into(),
            json!(["password"]).to_string(),
        );
        assert!(crate::portable_snapshot::project_fixed_sync_entities(&invalid).is_err());
        invalid.entities.remove("fixed_sync_complete_kinds");
        assert!(crate::portable_snapshot::project_fixed_sync_entities(&invalid).is_err());
    }

    #[test]
    fn sync_schema_four_does_not_authorize_a_schema_four_full_backup() {
        let mut snapshot = RawPortableSnapshot::backup("device", "version");
        snapshot.meta.schema_version = 4;
        snapshot.recalculate_hash().unwrap();
        assert!(matches!(
            validate_raw_snapshot(&snapshot),
            Err(PortableSnapshotError::UnsupportedVersion(4))
        ));
    }

    #[test]
    fn notes_and_themes_count_as_user_content_but_a_vault_only_sync_does_not() {
        let mut source = RawPortableSnapshot::sync("device", "version");
        source.entities.insert(
            "keys".into(),
            json!({"keys":[{"id":"local-key"}]}).to_string(),
        );
        assert!(!source.has_user_content());
        source.entities.insert(
            "notes".into(),
            json!({"notes":[{"id":"note","title":"Note","markdown":"Text"}],"folders":[]})
                .to_string(),
        );
        assert!(source.has_user_content());
    }
}
