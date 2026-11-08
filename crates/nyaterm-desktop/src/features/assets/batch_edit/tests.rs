use std::collections::HashMap;

use nyaterm_core::{
    BatchField, ConnectionBatchKind, MAX_SFTP_SHELL_DETECTION_TIMEOUT_MS,
    MIN_SFTP_SHELL_DETECTION_TIMEOUT_MS, SavedConnection, SftpCwdFollowMode, SshAlgorithmMode,
    SshProfile, SshTerminalType,
};

use super::{AssetBatchEditDraft, BatchFieldId, FieldBasis};

fn started(
    fields: &[(BatchFieldId, FieldBasis, &str)],
    bools: &[(BatchFieldId, bool)],
) -> AssetBatchEditDraft {
    let mut basis = HashMap::new();
    let mut values = HashMap::new();
    let mut bool_values = HashMap::new();
    for (field, field_basis, value) in fields {
        basis.insert(*field, *field_basis);
        values.insert(*field, (*value).to_string());
    }
    for (field, value) in bools {
        basis.insert(*field, FieldBasis::Uniform);
        bool_values.insert(*field, *value);
    }
    let mut draft = AssetBatchEditDraft::default();
    draft.start(
        vec!["connection".to_string()],
        FieldBasis::Uniform,
        Some(ConnectionBatchKind::Ssh),
        basis,
        values,
        bool_values,
    );
    draft
}

fn draft_with(field: BatchFieldId, value: &str) -> AssetBatchEditDraft {
    let mut draft = started(&[(field, FieldBasis::Uniform, value)], &[]);
    draft.set_value(field, value.to_string());
    draft
}

#[test]
fn empty_draft_is_rejected() {
    let draft = AssetBatchEditDraft::default();
    assert!(draft.build_edit().is_err());
}

#[test]
fn uniform_values_are_not_written_until_edited() {
    let draft = started(
        &[
            (BatchFieldId::Description, FieldBasis::Uniform, "shared"),
            (BatchFieldId::Port, FieldBasis::Uniform, "2222"),
        ],
        &[(BatchFieldId::DynamicTabTitle, true)],
    );

    assert!(draft.build_edit().is_err());
    assert!(!draft.has_edits());
}

#[test]
fn edited_uniform_field_is_written_and_the_rest_stays_unchanged() {
    let mut draft = started(
        &[
            (BatchFieldId::Description, FieldBasis::Uniform, "shared"),
            (BatchFieldId::Port, FieldBasis::Uniform, "2222"),
        ],
        &[],
    );
    draft.set_value(BatchFieldId::Description, "  hello  ".to_string());
    let edit = draft.build_edit().expect("valid edit");

    assert_eq!(edit.description, BatchField::Set("hello".to_string()));
    assert_eq!(edit.port, BatchField::Unchanged);
}

#[test]
fn mixed_text_stays_unchanged_until_edited() {
    let mut draft = started(&[(BatchFieldId::Description, FieldBasis::Mixed, "")], &[]);
    draft.set_value(BatchFieldId::Port, "2222".to_string());
    let edit = draft.build_edit().expect("valid edit");

    assert_eq!(edit.description, BatchField::Unchanged);
    assert_eq!(edit.port, BatchField::Set(2222));
}

#[test]
fn mixed_text_edited_to_empty_clears() {
    let mut draft = started(&[(BatchFieldId::Description, FieldBasis::Mixed, "")], &[]);
    draft.set_value(BatchFieldId::Description, String::new());
    let edit = draft.build_edit().expect("valid edit");
    assert_eq!(edit.description, BatchField::Clear);
}

#[test]
fn group_set_to_root_becomes_clear() {
    let draft = draft_with(BatchFieldId::Group, "");
    let edit = draft.build_edit().expect("valid edit");
    assert_eq!(edit.group_id, BatchField::Clear);
}

#[test]
fn port_parses_and_rejects_zero_or_text() {
    let edit = draft_with(BatchFieldId::Port, "2222")
        .build_edit()
        .expect("valid port");
    assert_eq!(edit.port, BatchField::Set(2222));

    assert!(draft_with(BatchFieldId::Port, "0").build_edit().is_err());
    assert!(draft_with(BatchFieldId::Port, "abc").build_edit().is_err());
}

#[test]
fn shell_timeout_respects_core_bounds() {
    let edit = draft_with(
        BatchFieldId::SftpShellTimeout,
        &MIN_SFTP_SHELL_DETECTION_TIMEOUT_MS.to_string(),
    )
    .build_edit()
    .expect("minimum is accepted");
    assert_eq!(
        edit.sftp_shell_detection_timeout_ms,
        BatchField::Set(MIN_SFTP_SHELL_DETECTION_TIMEOUT_MS)
    );

    let too_large = MAX_SFTP_SHELL_DETECTION_TIMEOUT_MS + 1;
    assert!(
        draft_with(BatchFieldId::SftpShellTimeout, &too_large.to_string())
            .build_edit()
            .is_err()
    );
}

#[test]
fn pipeline_depth_requires_non_zero() {
    assert!(
        draft_with(BatchFieldId::SftpPipelineDepth, "0")
            .build_edit()
            .is_err()
    );
    let edit = draft_with(BatchFieldId::SftpPipelineDepth, "8")
        .build_edit()
        .expect("valid depth");
    assert_eq!(edit.sftp_pipeline_depth, BatchField::Set(8));
}

#[test]
fn mixed_bool_is_unchanged_until_the_switch_is_used() {
    let mut draft = started(&[], &[]);
    draft.start(
        vec!["connection".to_string()],
        FieldBasis::Uniform,
        Some(ConnectionBatchKind::Ssh),
        HashMap::from([(BatchFieldId::DynamicTabTitle, FieldBasis::Mixed)]),
        HashMap::new(),
        HashMap::new(),
    );
    draft.set_value(BatchFieldId::Description, "note".to_string());
    let edit = draft.build_edit().expect("valid edit");
    assert_eq!(edit.dynamic_tab_title, BatchField::Unchanged);

    draft.set_bool_value(BatchFieldId::DynamicTabTitle, true);
    let edit = draft.build_edit().expect("valid edit");
    assert_eq!(edit.dynamic_tab_title, BatchField::Set(true));
}

#[test]
fn edited_bool_writes_the_interacted_value() {
    let mut draft = started(&[], &[(BatchFieldId::DynamicTabTitle, true)]);
    draft.set_bool_value(BatchFieldId::DynamicTabTitle, false);
    let edit = draft.build_edit().expect("valid edit");
    assert_eq!(edit.dynamic_tab_title, BatchField::Set(false));
}

#[test]
fn inapplicable_fields_never_enter_the_patch() {
    let mut draft = started(
        &[(BatchFieldId::SftpEnabled, FieldBasis::Inapplicable, "")],
        &[],
    );
    draft.set_value(BatchFieldId::Description, "note".to_string());
    let edit = draft.build_edit().expect("valid edit");
    assert_eq!(edit.sftp_enabled, BatchField::Unchanged);
}

#[test]
fn select_values_map_to_typed_enums() {
    let edit = draft_with(BatchFieldId::SshProfile, "network_device")
        .build_edit()
        .expect("valid profile");
    assert_eq!(edit.ssh_profile, BatchField::Set(SshProfile::NetworkDevice));

    let edit = draft_with(BatchFieldId::TerminalType, "vt100")
        .build_edit()
        .expect("valid terminal");
    assert_eq!(edit.terminal_type, BatchField::Set(SshTerminalType::Vt100));

    let edit = draft_with(BatchFieldId::SftpCwdFollow, "rc_file")
        .build_edit()
        .expect("valid cwd follow");
    assert_eq!(
        edit.sftp_cwd_follow_mode,
        BatchField::Set(SftpCwdFollowMode::RcFile)
    );

    let edit = draft_with(BatchFieldId::SshAlgorithmMode, "custom")
        .build_edit()
        .expect("valid algorithm mode");
    assert_eq!(
        edit.ssh_algorithm_mode,
        BatchField::Set(SshAlgorithmMode::Custom)
    );
}

#[test]
fn algorithm_lists_split_on_commas_and_whitespace() {
    let edit = draft_with(
        BatchFieldId::SshCiphers,
        "aes256-gcm, chacha20-poly1305 aes128-ctr",
    )
    .build_edit()
    .expect("valid ciphers");
    assert_eq!(
        edit.ssh_ciphers,
        BatchField::Set(vec![
            "aes256-gcm".to_string(),
            "chacha20-poly1305".to_string(),
            "aes128-ctr".to_string(),
        ]),
    );
}

#[test]
fn empty_text_clears_and_empty_lists_are_rejected() {
    let edit = draft_with(BatchFieldId::Description, "   ")
        .build_edit()
        .expect("empty text clears");
    assert_eq!(edit.description, BatchField::Clear);

    assert!(
        draft_with(BatchFieldId::SshKex, " , ")
            .build_edit()
            .is_err()
    );
}

#[test]
fn mixed_protocols_await_a_pick_and_write_nothing() {
    let mut draft = AssetBatchEditDraft::default();
    draft.start(
        vec!["connection".to_string()],
        FieldBasis::Mixed,
        None,
        HashMap::new(),
        HashMap::new(),
        HashMap::new(),
    );

    assert!(draft.awaiting_kind_pick());
    assert_eq!(draft.display_kind(), None);
    assert!(!draft.has_edits());
}

#[test]
fn picking_a_protocol_becomes_the_only_kind_patch() {
    let mut draft = started(&[], &[]);
    assert!(!draft.has_edits());

    assert!(draft.set_kind(ConnectionBatchKind::Telnet));
    assert_eq!(draft.picked_kind(), Some(ConnectionBatchKind::Telnet));
    assert!(draft.has_edits());

    let edit = draft.build_edit().expect("valid edit");
    assert_eq!(edit.kind, BatchField::Set(ConnectionBatchKind::Telnet));
}

#[test]
fn display_fields_are_never_editable_and_never_patched() {
    assert!(!BatchFieldId::Name.is_editable());
    assert!(!BatchFieldId::Host.is_editable());
    assert!(!BatchFieldId::AgentEndpoints.is_editable());
    assert!(BatchFieldId::Password.is_editable());
    assert!(BatchFieldId::from_key("display.host").is_none());
    assert!(BatchFieldId::from_key("display.password").is_some());

    let draft = started(&[(BatchFieldId::Host, FieldBasis::Uniform, "host")], &[]);
    assert!(!draft.is_edited(BatchFieldId::Host));
}

#[test]
fn refresh_keeps_edits_for_fields_that_still_apply() {
    let mut draft = started(
        &[
            (BatchFieldId::Description, FieldBasis::Uniform, "old"),
            (BatchFieldId::SftpEnabled, FieldBasis::Uniform, "true"),
        ],
        &[],
    );
    draft.set_value(BatchFieldId::Description, "edited".to_string());

    draft.refresh_for_kind(
        HashMap::from([
            (BatchFieldId::Description, FieldBasis::Uniform),
            (BatchFieldId::SftpEnabled, FieldBasis::Inapplicable),
        ]),
        HashMap::from([
            (BatchFieldId::Description, "new".to_string()),
            (BatchFieldId::SftpEnabled, String::new()),
        ]),
        HashMap::new(),
    );

    assert!(draft.is_edited(BatchFieldId::Description));
    assert_eq!(draft.value(BatchFieldId::Description), "edited");
    assert!(!draft.is_edited(BatchFieldId::SftpEnabled));
}

fn record(id: &str, protocol: &str) -> SavedConnection {
    let value = match protocol {
        "ssh" => serde_json::json!({
            "id": id, "name": id, "type": "ssh", "host": "example.com", "username": "root",
        }),
        "local" => serde_json::json!({"id": id, "name": id, "type": "local_terminal"}),
        "rdp" => serde_json::json!({
            "id": id, "name": id, "type": "rdp", "host": "example.invalid",
        }),
        _ => unreachable!("unsupported test protocol"),
    };
    serde_json::from_value(value).unwrap()
}

#[test]
fn mixed_protocol_counts_omit_zero_protocols_and_missing_ids() {
    let connections = vec![
        record("ssh-a", "ssh"),
        record("ssh-b", "ssh"),
        record("ssh-c", "ssh"),
        record("local", "local"),
        record("rdp-a", "rdp"),
        record("rdp-b", "rdp"),
        record("ssh-unselected", "ssh"),
    ];
    let ids = [
        "ssh-a", "ssh-b", "ssh-c", "local", "rdp-a", "rdp-b", "missing",
    ]
    .map(String::from)
    .to_vec();

    assert_eq!(
        super::asset_batch_kind_counts(&connections, &ids),
        vec![
            (ConnectionBatchKind::Ssh, 3),
            (ConnectionBatchKind::LocalTerminal, 1),
            (ConnectionBatchKind::Rdp, 2),
        ]
    );
}

#[test]
fn mixed_protocol_counts_label_uses_the_editor_protocol_names() {
    let counts = vec![
        (ConnectionBatchKind::Ssh, 3),
        (ConnectionBatchKind::LocalTerminal, 1),
        (ConnectionBatchKind::Rdp, 2),
    ];
    let separator = rust_i18n::t!("assets.batch.mixedProtocolsSeparator").to_string();
    let local = rust_i18n::t!("dialog.localTerminal").to_string();
    let expected = [
        rust_i18n::t!("assets.batch.mixedProtocolsItem", kind = "SSH", count = 3).to_string(),
        rust_i18n::t!(
            "assets.batch.mixedProtocolsItem",
            kind = local.clone(),
            count = 1
        )
        .to_string(),
        rust_i18n::t!("assets.batch.mixedProtocolsItem", kind = "RDP", count = 2).to_string(),
    ]
    .join(&separator);

    assert_eq!(super::asset_batch_kind_counts_label(&counts), expected);
    assert!(
        expected.contains(&local),
        "local terminal name is localized"
    );
}
