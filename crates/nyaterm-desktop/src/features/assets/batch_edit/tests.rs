use super::basis::compute_batch_basis;
use super::draft::{AssetBatchEditDraft, BatchEditMode};
use super::fields::{BatchFieldId, FieldBasis};
use nyaterm_core::models::connection_batch::{
    BatchField, ConnectionBatchKind, apply_connection_batch_edit,
};
use nyaterm_core::models::{ConnectionType, SavedConnection};
use std::collections::HashMap;

fn connection(id: &str, kind: &str) -> SavedConnection {
    serde_json::from_value(
        serde_json::json!({"id": id, "name": id, "type": kind, "host": "host", "username": "user"}),
    )
    .unwrap()
}
fn started(connections: &[SavedConnection]) -> AssetBatchEditDraft {
    let ids = connections.iter().map(|c| c.id.clone()).collect::<Vec<_>>();
    let (basis, values, bools) = compute_batch_basis(connections, &ids, None);
    let mut draft = AssetBatchEditDraft::default();
    draft.start(ids, basis, values, bools);
    draft.initialize_scopes(connections);
    draft
}
#[test]
fn scopes_preserve_drafts_and_only_apply_to_original_protocols() {
    let connections = [
        connection("a", "ssh"),
        connection("b", "rdp"),
        connection("c", "ssh"),
    ];
    let mut draft = started(&connections);
    assert_eq!(draft.display_kind(), None);
    assert_eq!(draft.counts().len(), 2);
    draft.set_mode(BatchFieldId::Description, BatchEditMode::Set);
    draft.set_value(BatchFieldId::Description, "common".into());
    draft.set_kind(ConnectionBatchKind::Ssh);
    draft.set_mode(BatchFieldId::Port, BatchEditMode::Set);
    draft.set_value(BatchFieldId::Port, "2222".into());
    draft.set_kind(ConnectionBatchKind::Rdp);
    draft.set_mode(BatchFieldId::Username, BatchEditMode::Set);
    draft.set_value(BatchFieldId::Username, "rdp-user".into());
    draft.set_kind(ConnectionBatchKind::Ssh);
    assert_eq!(draft.value(BatchFieldId::Port), "2222");
    assert_eq!(draft.summary(), (3, 3));
    let edits = draft.build_edits().unwrap();
    for original in connections {
        let mut value = original.clone();
        let kind = ConnectionBatchKind::from_config(&value.config);
        apply_connection_batch_edit(&mut value, &edits[&None]);
        apply_connection_batch_edit(&mut value, &edits[&Some(kind)]);
        assert_eq!(ConnectionBatchKind::from_config(&value.config), kind);
        assert_eq!(value.description.as_deref(), Some("common"));
        match value.config {
            ConnectionType::Ssh { port, username, .. } => {
                assert_eq!(port, 2222);
                assert_eq!(username, "user");
            }
            ConnectionType::Rdp { port, username, .. } => {
                assert_eq!(port, 3389);
                assert_eq!(username, "rdp-user");
            }
            _ => unreachable!(),
        }
    }
}
#[test]
fn three_states_require_explicit_clear_and_keep_removes_patch() {
    let mut draft = started(&[connection("a", "ssh")]);
    assert!(draft.build_edits().is_err());
    draft.set_mode(BatchFieldId::Description, BatchEditMode::Set);
    assert!(draft.build_edits().is_err());
    draft.set_value(BatchFieldId::Description, "value".into());
    assert!(draft.build_edits().is_ok());
    draft.set_value(BatchFieldId::Description, "  ".into());
    assert!(draft.build_edits().is_err());
    draft.set_mode(BatchFieldId::Description, BatchEditMode::Clear);
    assert_eq!(
        draft.build_edits().unwrap()[&None].description,
        BatchField::Clear
    );
    draft.set_mode(BatchFieldId::Description, BatchEditMode::Keep);
    assert!(!draft.has_edits());
    assert!(draft.build_edits().is_err());
}
#[test]
fn mixed_booleans_require_choice_and_secret_drafts_are_discarded() {
    let mut draft = AssetBatchEditDraft::default();
    draft.start(
        vec!["a".into()],
        HashMap::from([(BatchFieldId::IconAutoDetect, FieldBasis::Mixed)]),
        HashMap::new(),
        HashMap::new(),
    );
    draft.set_mode(BatchFieldId::IconAutoDetect, BatchEditMode::Set);
    assert!(draft.build_edits().is_err());
    draft.set_bool_value(BatchFieldId::IconAutoDetect, false);
    assert!(draft.build_edits().is_ok());
    draft.set_mode(BatchFieldId::Password, BatchEditMode::Set);
    draft.set_value(BatchFieldId::Password, "  secret  ".into());
    assert_eq!(draft.replacement_password(), "  secret  ");
    assert_eq!(draft.value(BatchFieldId::Password), "");
    draft.set_mode(BatchFieldId::Password, BatchEditMode::Keep);
    assert_eq!(draft.replacement_password(), "");
    let generation = draft.generation();
    draft.clear();
    assert!(!draft.matches_generation(generation));
    draft.start(vec![], HashMap::new(), HashMap::new(), HashMap::new());
    assert!(!draft.matches_generation(generation));
}
#[test]
fn protocol_counts_deduplicate_nonadjacent_protocols() {
    let draft = started(&[
        connection("a", "ssh"),
        connection("b", "rdp"),
        connection("c", "ssh"),
    ]);
    assert_eq!(
        draft.counts(),
        &[(ConnectionBatchKind::Ssh, 2), (ConnectionBatchKind::Rdp, 1)]
    );
}

#[test]
fn selectable_enum_values_are_accepted_by_the_patch_builder() {
    use super::options;
    use nyaterm_core::models::SshProfile;
    for (field, options) in [
        (
            BatchFieldId::SshProfile,
            options::ssh_profile_select_options(),
        ),
        (
            BatchFieldId::TerminalType,
            options::terminal_type_select_options(SshProfile::Standard),
        ),
        (
            BatchFieldId::SftpCwdFollow,
            options::sftp_cwd_follow_select_options(),
        ),
        (
            BatchFieldId::SshAlgorithmMode,
            options::ssh_algorithm_mode_select_options(),
        ),
        (
            BatchFieldId::FlowControl,
            options::flow_control_select_options(""),
        ),
        (
            BatchFieldId::RecordingMode,
            options::recording_mode_select_options(""),
        ),
    ] {
        for option in options {
            let mut draft = AssetBatchEditDraft::default();
            draft.start(
                vec!["selected".into()],
                HashMap::from([(field, FieldBasis::Uniform)]),
                HashMap::new(),
                HashMap::new(),
            );
            draft.set_mode(field, BatchEditMode::Set);
            draft.set_value(field, option.value().to_string());
            assert!(draft.build_edits().is_ok(), "{field:?}: {}", option.value());
        }
    }
}

#[test]
fn every_batch_field_label_exists_in_all_supported_locales() {
    use super::fields::{DISPLAY_FIELDS, EDITABLE_FIELDS};
    for (locale, source) in [
        ("en", include_str!("../../../../locales/en.json")),
        ("fr", include_str!("../../../../locales/fr.json")),
        ("ja", include_str!("../../../../locales/ja.json")),
        ("ko", include_str!("../../../../locales/ko.json")),
        ("zh-CN", include_str!("../../../../locales/zh-CN.json")),
        ("zh-TW", include_str!("../../../../locales/zh-TW.json")),
    ] {
        let catalog: serde_json::Value = serde_json::from_str(source).unwrap();
        for field in DISPLAY_FIELDS.into_iter().chain(EDITABLE_FIELDS) {
            let key = field.label_key();
            let value = key
                .split('.')
                .try_fold(&catalog, |value, part| value.get(part));
            assert!(
                value.and_then(serde_json::Value::as_str).is_some(),
                "{locale}: {key}"
            );
        }
    }
}
