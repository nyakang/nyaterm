use super::basis::compute_batch_basis;
use super::draft::{AssetBatchEditDraft, BatchEditMode};
use super::fields::{BatchFieldKey, FieldBasis};
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
    draft.start(ids, basis, values, bools, None);
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
    assert_eq!(draft.display_kind(), Some(ConnectionBatchKind::Ssh));
    assert_eq!(draft.counts().len(), 2);
    draft.set_mode(BatchFieldKey::Description, BatchEditMode::Set);
    draft.set_value(BatchFieldKey::Description, "common".into());
    draft.set_kind(ConnectionBatchKind::Ssh);
    draft.set_mode(BatchFieldKey::Port, BatchEditMode::Set);
    draft.set_value(BatchFieldKey::Port, "2222".into());
    draft.set_kind(ConnectionBatchKind::Rdp);
    draft.set_mode(BatchFieldKey::Username, BatchEditMode::Set);
    draft.set_value(BatchFieldKey::Username, "rdp-user".into());
    draft.set_kind(ConnectionBatchKind::Ssh);
    assert_eq!(draft.value(BatchFieldKey::Port), "2222");
    assert_eq!(draft.summary(), (3, 3));
    let edits = draft.build_edits().unwrap();
    for original in connections {
        let mut value = original.clone();
        let kind = ConnectionBatchKind::from_config(&value.config);
        apply_connection_batch_edit(&mut value, &edits[&Some(kind)]);
        assert_eq!(ConnectionBatchKind::from_config(&value.config), kind);
        match value.config {
            ConnectionType::Ssh { port, username, .. } => {
                assert_eq!(port, 2222);
                assert_eq!(username, "user");
                assert_eq!(value.description.as_deref(), Some("common"));
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
    draft.set_mode(BatchFieldKey::Description, BatchEditMode::Set);
    assert!(draft.build_edits().is_err());
    draft.set_value(BatchFieldKey::Description, "value".into());
    assert!(draft.build_edits().is_ok());
    draft.set_value(BatchFieldKey::Description, "  ".into());
    assert!(draft.build_edits().is_err());
    draft.set_mode(BatchFieldKey::Description, BatchEditMode::Clear);
    assert_eq!(
        draft.build_edits().unwrap()[&Some(ConnectionBatchKind::Ssh)].description,
        BatchField::Clear
    );
    draft.set_mode(BatchFieldKey::Description, BatchEditMode::Keep);
    assert!(!draft.has_edits());
    assert!(draft.build_edits().is_err());
}
#[test]
fn mixed_booleans_require_choice_and_secret_drafts_are_discarded() {
    let mut draft = AssetBatchEditDraft::default();
    draft.start(
        vec!["a".into()],
        HashMap::from([(BatchFieldKey::IconAutoDetect, FieldBasis::Mixed)]),
        HashMap::new(),
        HashMap::new(),
        None,
    );
    draft.set_mode(BatchFieldKey::IconAutoDetect, BatchEditMode::Set);
    assert!(draft.build_edits().is_err());
    draft.set_bool_value(BatchFieldKey::IconAutoDetect, false);
    assert!(draft.build_edits().is_ok());
    draft.set_mode(BatchFieldKey::Password, BatchEditMode::Set);
    draft.set_value(BatchFieldKey::Password, "  secret  ".into());
    assert_eq!(draft.replacement_password(), "  secret  ");
    assert_eq!(draft.value(BatchFieldKey::Password), "");
    draft.set_mode(BatchFieldKey::Password, BatchEditMode::Keep);
    assert_eq!(draft.replacement_password(), "");
    let generation = draft.generation();
    draft.clear();
    assert!(!draft.matches_generation(generation));
    draft.start(vec![], HashMap::new(), HashMap::new(), HashMap::new(), None);
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
            BatchFieldKey::SshProfile,
            options::ssh_profile_select_options(),
        ),
        (
            BatchFieldKey::TerminalType,
            options::terminal_type_select_options(SshProfile::Standard),
        ),
        (
            BatchFieldKey::SftpCwdFollow,
            options::sftp_cwd_follow_select_options(),
        ),
        (
            BatchFieldKey::SshAlgorithmMode,
            options::ssh_algorithm_mode_select_options(),
        ),
        (
            BatchFieldKey::FlowControl,
            options::flow_control_select_options(""),
        ),
        (
            BatchFieldKey::RecordingMode,
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
                None,
            );
            draft.set_mode(field, BatchEditMode::Set);
            draft.set_value(field, option.value().to_string());
            assert!(draft.build_edits().is_ok(), "{field:?}: {}", option.value());
        }
    }
}

#[test]
fn ssh_compatibility_mode_builds_a_boolean_patch() {
    let connections = [connection("a", "ssh")];
    let mut draft = started(&connections);
    draft.set_bool_value(BatchFieldKey::SshCompatibility, true);

    let edits = draft.build_edits().unwrap();

    assert_eq!(
        edits[&Some(ConnectionBatchKind::Ssh)].ssh_compatibility_mode,
        BatchField::Set(true)
    );
}

#[test]
fn allow_short_rsa_host_keys_builds_a_boolean_patch() {
    let connections = [connection("a", "ssh")];
    let mut draft = started(&connections);
    draft.set_bool_value(BatchFieldKey::AllowShortRsaHostKeys, true);

    let edits = draft.build_edits().unwrap();

    assert_eq!(
        edits[&Some(ConnectionBatchKind::Ssh)].allow_short_rsa_host_keys,
        BatchField::Set(true)
    );
}

#[test]
fn batch_field_table_covers_every_editable_field_once() {
    use super::basis::batch_field_in_kind;
    use super::fields::{BatchFieldKey, DISPLAY_FIELDS, EDITABLE_FIELDS, batch_rows};
    use crate::models::{ConnectionEditorSection, ConnectionKindTab};

    let mut covered: Vec<BatchFieldKey> = Vec::new();
    for tab in ConnectionKindTab::ALL {
        for section in ConnectionEditorSection::for_kind(tab, false) {
            let rows = batch_rows(tab, *section);
            assert!(!rows.is_empty(), "{tab:?} {section:?} has no batch rows");
            let mut seen: Vec<BatchFieldKey> = Vec::new();
            for row in rows {
                for spec in row.fields {
                    assert_eq!(
                        spec.action,
                        spec.key.action(),
                        "action mismatch for {:?} in {tab:?} {section:?}",
                        spec.key
                    );
                    assert!(
                        !seen.contains(&spec.key),
                        "{:?} appears twice in {tab:?} {section:?}",
                        spec.key
                    );
                    assert!(
                        batch_field_in_kind(tab.batch_kind(), spec.key),
                        "{:?} is not applicable to {tab:?}",
                        spec.key
                    );
                    seen.push(spec.key);
                    if !covered.contains(&spec.key) {
                        covered.push(spec.key);
                    }
                }
            }
        }
    }

    let mut expected = EDITABLE_FIELDS.to_vec();
    expected.extend(DISPLAY_FIELDS);
    for field in &expected {
        assert!(
            covered.contains(field),
            "{field:?} is missing from every row"
        );
    }
    for field in &covered {
        assert!(
            expected.contains(field),
            "{field:?} is not an exposed field"
        );
    }
}

#[test]
fn every_batch_field_section_is_reachable_for_its_kind() {
    use super::basis::batch_field_in_kind;
    use super::fields::{DISPLAY_FIELDS, EDITABLE_FIELDS};
    use crate::models::{ConnectionEditorSection, ConnectionKindTab};

    for field in DISPLAY_FIELDS.into_iter().chain(EDITABLE_FIELDS) {
        let mut reachable = false;
        for tab in ConnectionKindTab::ALL {
            if !batch_field_in_kind(tab.batch_kind(), field) {
                continue;
            }
            let sections: Vec<ConnectionEditorSection> =
                ConnectionEditorSection::for_kind(tab, false).to_vec();
            assert!(
                sections.contains(&field.section()),
                "{field:?} sits in {:?}, which {tab:?} does not render",
                field.section()
            );
            reachable = true;
        }
        assert!(
            reachable,
            "{field:?} is not reachable from any protocol tab"
        );
    }
}

#[test]
fn editor_surface_parity() {
    batch_field_table_covers_every_editable_field_once();
    every_batch_field_section_is_reachable_for_its_kind();
    connection_tab_and_batch_kind_round_trip();
    reset_actions_expose_the_same_default_that_clear_writes();
}

#[test]
fn connection_tab_and_batch_kind_round_trip() {
    use crate::models::ConnectionKindTab;

    for tab in ConnectionKindTab::ALL {
        assert_eq!(ConnectionKindTab::from_batch_kind(tab.batch_kind()), tab);
    }
}

#[test]
fn reset_actions_expose_the_same_default_that_clear_writes() {
    use super::basis::field_display_value;
    use super::fields::{BatchFieldAction, BatchFieldKey};
    use crate::models::ConnectionKindTab;

    let cases = [
        (ConnectionKindTab::Ssh, BatchFieldKey::Port),
        (ConnectionKindTab::Ssh, BatchFieldKey::Backspace),
        (ConnectionKindTab::Rdp, BatchFieldKey::RdpWidth),
        (ConnectionKindTab::Rdp, BatchFieldKey::RdpDisplayMode),
        (ConnectionKindTab::Ftp, BatchFieldKey::FtpPassiveMode),
        (ConnectionKindTab::Ftp, BatchFieldKey::FtpConnectTimeout),
        (ConnectionKindTab::Adb, BatchFieldKey::AdbScreenMaxSize),
        (ConnectionKindTab::Adb, BatchFieldKey::AdbScreenQuality),
    ];
    for (tab, field) in cases {
        assert_eq!(field.action(), BatchFieldAction::ResetDefault);
        let connection = fixture(tab);
        let default = field.default_value(tab).expect("default");
        let edit = clear_for(field);
        let mut value = connection.clone();
        apply_connection_batch_edit(&mut value, &edit);
        assert_eq!(
            field_display_value(&value, field).as_deref(),
            Some(default.as_str()),
            "clear/default mismatch for {field:?}"
        );
    }
}

fn clear_for(field: BatchFieldKey) -> nyaterm_core::models::connection_batch::ConnectionBatchEdit {
    use nyaterm_core::models::connection_batch::ConnectionBatchEdit;

    let mut edit = ConnectionBatchEdit::default();
    match field {
        BatchFieldKey::Port => edit.port = BatchField::Clear,
        BatchFieldKey::Backspace => edit.backspace_mode = BatchField::Clear,
        BatchFieldKey::RdpWidth => edit.rdp_display_width = BatchField::Clear,
        BatchFieldKey::RdpDisplayMode => edit.rdp_display_mode = BatchField::Clear,
        BatchFieldKey::FtpPassiveMode => edit.ftp_passive_mode = BatchField::Clear,
        BatchFieldKey::FtpConnectTimeout => edit.ftp_connect_timeout_ms = BatchField::Clear,
        BatchFieldKey::AdbScreenMaxSize => edit.adb_screen_max_size = BatchField::Clear,
        BatchFieldKey::AdbScreenQuality => edit.adb_screen_quality = BatchField::Clear,
        _ => unreachable!("no clear fixture for {field:?}"),
    }
    edit
}

fn fixture(tab: crate::models::ConnectionKindTab) -> SavedConnection {
    let mut value = serde_json::json!({
        "id": "fixture",
        "name": "fixture",
        "type": kind_id(tab),
        "host": "host",
        "username": "user",
    });
    if tab == crate::models::ConnectionKindTab::Adb {
        value["device"] = serde_json::json!({"kind": "usb", "serial": "device"});
    }
    serde_json::from_value(value).unwrap()
}

fn kind_id(tab: crate::models::ConnectionKindTab) -> &'static str {
    match tab {
        crate::models::ConnectionKindTab::Ssh => "ssh",
        crate::models::ConnectionKindTab::Rdp => "rdp",
        crate::models::ConnectionKindTab::Adb => "adb",
        crate::models::ConnectionKindTab::Ftp => "ftp",
        _ => "ssh",
    }
}

#[test]
fn every_batch_field_label_exists_in_all_supported_locales() {
    use super::fields::{DISPLAY_FIELDS, EDITABLE_FIELDS};
    use rust_i18n::Backend as _;
    crate::preload_i18n().unwrap();
    for locale in nyaterm_i18n::available_locales() {
        for field in DISPLAY_FIELDS.into_iter().chain(EDITABLE_FIELDS) {
            let key = field.label_key();
            assert!(
                nyaterm_i18n::SharedCatalogBackend
                    .translate(&locale, key)
                    .is_some(),
                "{locale}: {key}"
            );
        }
        for (key, placeholders) in [
            ("saved", &["updated", "unchanged", "skippedFields"][..]),
            ("fieldApplicability", &["applicable", "total"][..]),
            ("skipProtocol", &["count"][..]),
            ("skipMstsc", &["count"][..]),
            ("skipNonFixed", &["count"][..]),
            ("skipGlobalRecording", &["count"][..]),
        ] {
            let value = nyaterm_i18n::SharedCatalogBackend
                .translate(&locale, &format!("assets.batch.{key}"))
                .unwrap();
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
    let state = draft.rdp_applicability(BatchFieldKey::RdpUseNla).unwrap();
    assert_eq!((state.applicable, state.total), (2, 3));
    assert_eq!(draft.basis(BatchFieldKey::RdpUseNla), FieldBasis::Uniform);
    assert!(draft.bool_value(BatchFieldKey::RdpUseNla));
    draft.set_mode(BatchFieldKey::RdpUseNla, BatchEditMode::Set);
    draft.set_bool_value(BatchFieldKey::RdpUseNla, false);
    draft.set_mode(BatchFieldKey::RdpBackend, BatchEditMode::Set);
    draft.set_value(BatchFieldKey::RdpBackend, "mstsc".into());
    draft.refresh_rdp_basis(&connections);
    assert_eq!(
        draft
            .rdp_applicability(BatchFieldKey::RdpUseNla)
            .unwrap()
            .applicable,
        0
    );
    assert!(!draft.field_available(BatchFieldKey::RdpUseNla));
    draft.set_mode(BatchFieldKey::RdpUseNla, BatchEditMode::Clear);
    draft.set_bool_value(BatchFieldKey::RdpUseNla, true);
    assert_eq!(draft.mode(BatchFieldKey::RdpUseNla), BatchEditMode::Set);
    assert!(!draft.bool_value(BatchFieldKey::RdpUseNla));
    assert_eq!(
        draft.build_edits().unwrap()[&Some(ConnectionBatchKind::Rdp)].rdp_use_nla,
        BatchField::Set(false)
    );
    assert!(!draft.bool_value(BatchFieldKey::RdpUseNla));
    draft.set_mode(BatchFieldKey::RdpBackend, BatchEditMode::Clear);
    draft.refresh_rdp_basis(&connections);
    assert_eq!(
        draft
            .rdp_applicability(BatchFieldKey::RdpUseNla)
            .unwrap()
            .applicable,
        3
    );
    assert!(draft.field_available(BatchFieldKey::RdpUseNla));
    assert_eq!(draft.basis(BatchFieldKey::RdpUseNla), FieldBasis::Mixed);
    assert!(!draft.bool_value(BatchFieldKey::RdpUseNla));
    draft.set_mode(BatchFieldKey::RdpUseNla, BatchEditMode::Keep);
    assert!(!draft.current_bool_is_set(BatchFieldKey::RdpUseNla));
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
            .rdp_applicability(BatchFieldKey::RdpWidth)
            .unwrap()
            .applicable,
        1
    );
    draft.set_mode(BatchFieldKey::RdpWidth, BatchEditMode::Set);
    draft.set_value(BatchFieldKey::RdpWidth, "1600".into());
    draft.set_mode(BatchFieldKey::RdpDisplayMode, BatchEditMode::Clear);
    draft.refresh_rdp_basis(&connections);
    assert_eq!(
        draft
            .rdp_applicability(BatchFieldKey::RdpWidth)
            .unwrap()
            .applicable,
        0
    );
    assert!(!draft.field_available(BatchFieldKey::RdpWidth));
    assert_eq!(draft.value(BatchFieldKey::RdpWidth), "1600");
    draft.set_mode(BatchFieldKey::RdpDisplayMode, BatchEditMode::Set);
    draft.set_value(BatchFieldKey::RdpDisplayMode, "fixed".into());
    draft.refresh_rdp_basis(&connections);
    assert_eq!(
        draft
            .rdp_applicability(BatchFieldKey::RdpWidth)
            .unwrap()
            .applicable,
        2
    );
    assert_eq!(draft.value(BatchFieldKey::RdpWidth), "1600");
    draft.set_mode(BatchFieldKey::RdpBackend, BatchEditMode::Set);
    draft.set_value(BatchFieldKey::RdpBackend, "builtin".into());
    draft.refresh_rdp_basis(&connections);
    assert_eq!(
        draft
            .rdp_applicability(BatchFieldKey::RdpWidth)
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
    assert!(!draft.field_available(BatchFieldKey::RdpUseNla));
    assert!(!draft.field_available(BatchFieldKey::RdpWidth));
    assert!(draft.field_available(BatchFieldKey::RdpBackend));
    draft.set_mode(BatchFieldKey::RdpUseNla, BatchEditMode::Set);
    assert_eq!(draft.mode(BatchFieldKey::RdpUseNla), BatchEditMode::Keep);
    draft.set_mode(BatchFieldKey::RdpBackend, BatchEditMode::Set);
    draft.set_value(BatchFieldKey::RdpBackend, "builtin".into());
    draft.refresh_rdp_basis(&connections);
    assert!(draft.field_available(BatchFieldKey::RdpUseNla));
    assert!(draft.field_available(BatchFieldKey::RdpWidth));
    assert!(draft.bool_value(BatchFieldKey::RdpUseNla));
    draft.set_mode(BatchFieldKey::RdpUseNla, BatchEditMode::Set);
    assert!(draft.build_edits().is_ok());
}
