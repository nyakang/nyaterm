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
        for (key, placeholders) in [
            ("saved", &["updated", "unchanged", "skippedFields"][..]),
            ("summary", &["connections", "fields"][..]),
            ("fieldApplicability", &["applicable", "total"][..]),
            ("skipProtocol", &["count"][..]),
            ("skipMstsc", &["count"][..]),
            ("skipNonFixed", &["count"][..]),
            ("skipGlobalRecording", &["count"][..]),
        ] {
            let value = catalog["assets"]["batch"][key].as_str().unwrap();
            for placeholder in placeholders {
                assert!(
                    value.contains(&format!("%{{{placeholder}}}")),
                    "{locale}: {key}: {placeholder}"
                );
            }
        }
    }
}

fn rdp_connection(id: &str, backend: &str, mode: &str, nla: bool) -> SavedConnection {
    serde_json::from_value(serde_json::json!({
        "id": id, "name": id, "type": "rdp", "host": "example.test", "username": "user",
        "backend": backend, "display": { "mode": mode }, "security": { "use_nla": nla }
    }))
    .unwrap()
}

#[test]
fn rdp_scope_compares_applicable_members_and_preserves_dormant_patches() {
    let connections = [
        rdp_connection("a", "builtin", "fixed", true),
        rdp_connection("b", "mstsc", "fixed", false),
        rdp_connection("c", "windows", "fixed", true),
    ];
    let mut draft = started(&connections);
    draft.set_kind(ConnectionBatchKind::Rdp);
    let state = draft.rdp_applicability(BatchFieldId::RdpUseNla).unwrap();
    assert_eq!((state.applicable, state.total), (2, 3));
    assert_eq!(draft.basis(BatchFieldId::RdpUseNla), FieldBasis::Uniform);
    assert!(draft.bool_value(BatchFieldId::RdpUseNla));
    draft.set_mode(BatchFieldId::RdpUseNla, BatchEditMode::Set);
    draft.set_bool_value(BatchFieldId::RdpUseNla, false);
    draft.set_mode(BatchFieldId::RdpBackend, BatchEditMode::Set);
    draft.set_value(BatchFieldId::RdpBackend, "mstsc".into());
    draft.refresh_rdp_basis(&connections);
    assert_eq!(
        draft
            .rdp_applicability(BatchFieldId::RdpUseNla)
            .unwrap()
            .applicable,
        0
    );
    assert!(!draft.field_available(BatchFieldId::RdpUseNla));
    draft.set_mode(BatchFieldId::RdpUseNla, BatchEditMode::Clear);
    draft.set_bool_value(BatchFieldId::RdpUseNla, true);
    assert_eq!(draft.mode(BatchFieldId::RdpUseNla), BatchEditMode::Set);
    assert!(!draft.bool_value(BatchFieldId::RdpUseNla));
    assert_eq!(
        draft.build_edits().unwrap()[&Some(ConnectionBatchKind::Rdp)].rdp_use_nla,
        BatchField::Set(false)
    );
    draft.set_scope(None);
    draft.set_kind(ConnectionBatchKind::Rdp);
    assert!(!draft.bool_value(BatchFieldId::RdpUseNla));
    draft.set_mode(BatchFieldId::RdpBackend, BatchEditMode::Clear);
    draft.refresh_rdp_basis(&connections);
    assert_eq!(
        draft
            .rdp_applicability(BatchFieldId::RdpUseNla)
            .unwrap()
            .applicable,
        3
    );
    assert!(draft.field_available(BatchFieldId::RdpUseNla));
    assert_eq!(draft.basis(BatchFieldId::RdpUseNla), FieldBasis::Mixed);
    assert!(!draft.bool_value(BatchFieldId::RdpUseNla));
    draft.set_mode(BatchFieldId::RdpUseNla, BatchEditMode::Keep);
    assert!(!draft.current_bool_is_set(BatchFieldId::RdpUseNla));
    assert_eq!(
        draft.build_edits().unwrap()[&Some(ConnectionBatchKind::Rdp)].rdp_use_nla,
        BatchField::Unchanged
    );
}

#[test]
fn rdp_dimensions_follow_native_fixed_and_cleared_display_dependencies() {
    let connections = [
        rdp_connection("a", "builtin", "native", true),
        rdp_connection("b", "windows", "fit-window", true),
        rdp_connection("c", "mstsc", "fixed", true),
    ];
    let mut draft = started(&connections);
    draft.set_kind(ConnectionBatchKind::Rdp);
    assert_eq!(
        draft
            .rdp_applicability(BatchFieldId::RdpWidth)
            .unwrap()
            .applicable,
        1
    );
    draft.set_mode(BatchFieldId::RdpWidth, BatchEditMode::Set);
    draft.set_value(BatchFieldId::RdpWidth, "1600".into());
    draft.set_mode(BatchFieldId::RdpDisplayMode, BatchEditMode::Clear);
    draft.refresh_rdp_basis(&connections);
    assert_eq!(
        draft
            .rdp_applicability(BatchFieldId::RdpWidth)
            .unwrap()
            .applicable,
        0
    );
    assert!(!draft.field_available(BatchFieldId::RdpWidth));
    assert_eq!(draft.value(BatchFieldId::RdpWidth), "1600");
    draft.set_mode(BatchFieldId::RdpDisplayMode, BatchEditMode::Set);
    draft.set_value(BatchFieldId::RdpDisplayMode, "fixed".into());
    draft.refresh_rdp_basis(&connections);
    assert_eq!(
        draft
            .rdp_applicability(BatchFieldId::RdpWidth)
            .unwrap()
            .applicable,
        2
    );
    assert_eq!(draft.value(BatchFieldId::RdpWidth), "1600");
    draft.set_mode(BatchFieldId::RdpBackend, BatchEditMode::Set);
    draft.set_value(BatchFieldId::RdpBackend, "builtin".into());
    draft.refresh_rdp_basis(&connections);
    assert_eq!(
        draft
            .rdp_applicability(BatchFieldId::RdpWidth)
            .unwrap()
            .applicable,
        3
    );
    let patch = draft
        .build_edits()
        .unwrap()
        .remove(&Some(ConnectionBatchKind::Rdp))
        .unwrap();
    assert_eq!(patch.rdp_display_width, BatchField::Set(1600));
}

#[test]
fn mstsc_only_fields_start_disabled_and_recover_after_backend_change() {
    let connections = [rdp_connection("a", "mstsc", "native", true)];
    let mut draft = started(&connections);
    draft.set_kind(ConnectionBatchKind::Rdp);
    assert!(!draft.field_available(BatchFieldId::RdpUseNla));
    assert!(!draft.field_available(BatchFieldId::RdpWidth));
    assert!(draft.field_available(BatchFieldId::RdpBackend));
    draft.set_mode(BatchFieldId::RdpUseNla, BatchEditMode::Set);
    assert_eq!(draft.mode(BatchFieldId::RdpUseNla), BatchEditMode::Keep);
    draft.set_mode(BatchFieldId::RdpBackend, BatchEditMode::Set);
    draft.set_value(BatchFieldId::RdpBackend, "builtin".into());
    draft.refresh_rdp_basis(&connections);
    assert!(draft.field_available(BatchFieldId::RdpUseNla));
    assert!(draft.field_available(BatchFieldId::RdpWidth));
    assert!(draft.bool_value(BatchFieldId::RdpUseNla));
    draft.set_mode(BatchFieldId::RdpUseNla, BatchEditMode::Set);
    assert!(draft.build_edits().is_ok());
}
