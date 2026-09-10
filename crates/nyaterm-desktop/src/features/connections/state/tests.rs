use std::collections::{HashMap, HashSet};

use super::{
    ConnectionEditorFeatureState, apply_connection_editor_shell_path,
    apply_connection_editor_working_dir, clear_connection_editor_runtime_state,
    clear_connection_list_runtime_state, clear_network_proxy_editor, clear_network_tunnel_editor,
    clear_selected_connection_ids, commit_connection_editor_new_group,
    connection_drop_position_for_target, connection_editor_inline_panel_draft,
    cycle_connection_sort_mode, insert_connection_editor_description_newline,
    move_connection_editor_ssh_algorithm, remove_connection_list_references,
    remove_group_list_references, remove_network_group_references, remove_network_item_references,
    retain_loaded_connection_references, retain_loaded_group_list_references,
    saved_connections_in_group_tree_for_list_state, select_connection_ids,
    select_saved_connection_after_editor_save, selected_connections_for_list_state,
    set_connection_drop_target_if_changed, set_connection_editor_advanced_tab,
    set_connection_editor_error, set_connection_editor_field_text, set_connection_editor_icon,
    set_connection_editor_kind, set_connection_editor_password_source,
    set_connection_editor_select_value, set_connection_editor_ssh_algorithm_enabled,
    set_connection_editor_ssh_algorithm_tab, set_connection_editor_telnet_tab,
    set_connection_group_editor_error, set_connection_group_hover, set_network_group_editor_error,
    set_network_group_editor_name, set_network_proxy_editor_error, set_network_proxy_editor_field,
    set_network_proxy_protocol, set_network_tunnel_bind_localhost, set_network_tunnel_connection,
    set_network_tunnel_editor_error, set_network_tunnel_editor_field, set_network_tunnel_group,
    set_network_tunnel_type, sync_connection_search_expansion, toggle_connection_editor_flag,
    toggle_network_move_picker_state, toggle_network_tunnel_auto_open,
    visible_connection_ids_for_list_state,
};
use crate::entities::{OverlayStore, StartupRestoreStore, UiStoreHandles};
use crate::features::NyaTermApp;
use crate::features::{
    connections::ConnectionDragKind, connections::ConnectionDropPosition,
    connections::ConnectionDropTarget, connections::ConnectionEditorToggle,
};
use crate::models::{
    ConnectionEditorAdvancedTab, ConnectionEditorField, ConnectionEditorPasswordSource,
    ConnectionEditorSelect, ConnectionEditorSshAlgorithmTab, ConnectionEditorState,
    ConnectionEditorTelnetTab, ConnectionGroupEditorMode, ConnectionGroupEditorState,
    ConnectionKindTab, ConnectionSortMode, NetworkGroupEditorState, NetworkMovePickerState,
    NetworkProxyEditorField, NetworkProxyEditorState, NetworkTab, NetworkTunnelEditorField,
    NetworkTunnelEditorState,
};
use gpui::{AppContext as _, TestAppContext};
use nyaterm_core::{
    AiExecutionProfile, AppRuntime, ConnectionRecordingSettings, ConnectionType, Group,
    RecordingMode, RecordingRotationPolicy, RuntimeMode, SavedConnection, SshAgentEndpoint,
    SshAgentForwardingPolicy, SshProfile, SshTerminalType, uuid,
};
use nyaterm_ui::ChildWindowSlot;
use std::path::PathBuf;

#[test]
fn search_expansion_opens_matches_and_restores_the_prior_tree() {
    let mut expanded = HashSet::from(["kept".to_string()]);
    let mut base = None;
    let mut applied = None;

    assert!(sync_connection_search_expansion(
        &mut expanded,
        &mut base,
        &mut applied,
        "web",
        ["hit".to_string()],
    ));
    assert_eq!(
        expanded,
        HashSet::from(["kept".to_string(), "hit".to_string()])
    );

    // Clearing the filter must not leave the auto-opened folder behind.
    assert!(sync_connection_search_expansion(
        &mut expanded,
        &mut base,
        &mut applied,
        "",
        Vec::new(),
    ));
    assert_eq!(expanded, HashSet::from(["kept".to_string()]));
    assert!(base.is_none());
}

#[test]
fn search_expansion_lets_a_folder_stay_collapsed_within_one_keyword() {
    let mut expanded = HashSet::new();
    let mut base = None;
    let mut applied = None;

    sync_connection_search_expansion(
        &mut expanded,
        &mut base,
        &mut applied,
        "web",
        ["hit".to_string()],
    );
    expanded.remove("hit");

    // Same keyword re-rendering must not re-open what the user just closed.
    assert!(!sync_connection_search_expansion(
        &mut expanded,
        &mut base,
        &mut applied,
        "web",
        ["hit".to_string()],
    ));
    assert!(expanded.is_empty());

    // A new keyword is a fresh search, so it expands again.
    assert!(sync_connection_search_expansion(
        &mut expanded,
        &mut base,
        &mut applied,
        "webs",
        ["hit".to_string()],
    ));
    assert!(expanded.contains("hit"));
}

#[test]
fn search_expansion_reopens_when_the_catalog_moves_under_a_fixed_query() {
    let mut expanded = HashSet::new();
    let mut base = None;
    let mut applied = None;

    assert!(sync_connection_search_expansion(
        &mut expanded,
        &mut base,
        &mut applied,
        "prod",
        ["one".to_string()],
    ));

    // Nothing typed, but the catalog moved: a store reload or a drag into a
    // folder made "two" match as well. Guarding on the query alone left this
    // folder collapsed forever and its match unreachable.
    assert!(sync_connection_search_expansion(
        &mut expanded,
        &mut base,
        &mut applied,
        "prod",
        ["one".to_string(), "two".to_string()],
    ));
    assert_eq!(
        expanded,
        HashSet::from(["one".to_string(), "two".to_string()])
    );

    // A catalog change that does not move the matching set is still a no-op, so
    // the fix does not cost the collapsed-folder behaviour above.
    assert!(!sync_connection_search_expansion(
        &mut expanded,
        &mut base,
        &mut applied,
        "prod",
        ["two".to_string(), "one".to_string()],
    ));
}

#[test]
fn clear_selected_connection_ids_clears_selection_and_anchor() {
    let mut selected_ids = HashSet::from(["one".to_string(), "two".to_string()]);
    let mut last_selected_id = Some("two".to_string());

    clear_selected_connection_ids(&mut selected_ids, &mut last_selected_id);

    assert!(selected_ids.is_empty());
    assert_eq!(last_selected_id, None);
}

#[test]
fn cycle_connection_sort_mode_updates_and_returns_next_mode() {
    let mut sort_mode = ConnectionSortMode::Default;

    let next = cycle_connection_sort_mode(&mut sort_mode);

    assert_eq!(sort_mode, next);
    assert_eq!(sort_mode, ConnectionSortMode::NameAsc);
}

#[test]
fn set_connection_group_hover_is_idempotent() {
    let mut hovered_group_id = None;

    assert!(set_connection_group_hover(
        &mut hovered_group_id,
        "group-a".to_string(),
        true,
    ));
    assert_eq!(hovered_group_id.as_deref(), Some("group-a"));
    assert!(!set_connection_group_hover(
        &mut hovered_group_id,
        "group-a".to_string(),
        true,
    ));
    assert!(!set_connection_group_hover(
        &mut hovered_group_id,
        "group-b".to_string(),
        false,
    ));
    assert!(set_connection_group_hover(
        &mut hovered_group_id,
        "group-a".to_string(),
        false,
    ));
    assert_eq!(hovered_group_id, None);
}

#[test]
fn set_connection_drop_target_if_changed_ignores_repeated_target() {
    let target = ConnectionDropTarget {
        id: Some("one".to_string()),
        kind: ConnectionDragKind::Connection,
        position: ConnectionDropPosition::After,
    };
    let mut drop_target = None;

    assert!(set_connection_drop_target_if_changed(
        &mut drop_target,
        target.clone()
    ));
    assert_eq!(drop_target, Some(target.clone()));
    assert!(!set_connection_drop_target_if_changed(
        &mut drop_target,
        target
    ));
}

#[test]
fn connection_drop_position_for_target_uses_matching_target_or_fallback() {
    let drop_target = Some(ConnectionDropTarget {
        id: Some("group-a".to_string()),
        kind: ConnectionDragKind::Group,
        position: ConnectionDropPosition::Inside,
    });

    assert_eq!(
        connection_drop_position_for_target(
            &drop_target,
            "group-a",
            ConnectionDropPosition::Before
        ),
        ConnectionDropPosition::Inside
    );
    assert_eq!(
        connection_drop_position_for_target(
            &drop_target,
            "group-b",
            ConnectionDropPosition::Before
        ),
        ConnectionDropPosition::Before
    );
}

#[test]
fn clear_connection_list_runtime_state_removes_transient_ui_references() {
    let mut selected_ids = HashSet::from(["one".to_string()]);
    let mut last_selected_id = Some("one".to_string());
    let mut expanded_group_ids = HashSet::from(["group-a".to_string()]);
    let mut drop_target = Some(ConnectionDropTarget {
        id: Some("group-a".to_string()),
        kind: ConnectionDragKind::Group,
        position: ConnectionDropPosition::Inside,
    });
    let mut hovered_group_id = Some("group-a".to_string());

    clear_connection_list_runtime_state(
        &mut selected_ids,
        &mut last_selected_id,
        &mut expanded_group_ids,
        &mut drop_target,
        &mut hovered_group_id,
    );

    assert!(selected_ids.is_empty());
    assert_eq!(last_selected_id, None);
    assert!(expanded_group_ids.is_empty());
    assert_eq!(drop_target, None);
    assert_eq!(hovered_group_id, None);
}

#[test]
fn select_connection_ids_replaces_toggles_and_tracks_anchor() {
    let visible_ids = vec!["one".to_string(), "two".to_string(), "three".to_string()];
    let mut selected_ids = HashSet::from(["old".to_string()]);
    let mut last_selected_id = Some("old".to_string());

    let count = select_connection_ids(
        &mut selected_ids,
        &mut last_selected_id,
        "two".to_string(),
        &visible_ids,
        false,
        false,
    );

    assert_eq!(count, 1);
    assert_eq!(selected_ids, HashSet::from(["two".to_string()]));
    assert_eq!(last_selected_id.as_deref(), Some("two"));

    let count = select_connection_ids(
        &mut selected_ids,
        &mut last_selected_id,
        "three".to_string(),
        &visible_ids,
        true,
        false,
    );

    assert_eq!(count, 2);
    assert_eq!(
        selected_ids,
        HashSet::from(["two".to_string(), "three".to_string()])
    );
    assert_eq!(last_selected_id.as_deref(), Some("three"));

    let count = select_connection_ids(
        &mut selected_ids,
        &mut last_selected_id,
        "three".to_string(),
        &visible_ids,
        true,
        false,
    );

    assert_eq!(count, 1);
    assert_eq!(selected_ids, HashSet::from(["two".to_string()]));
    assert_eq!(last_selected_id.as_deref(), Some("three"));
}

#[test]
fn select_connection_ids_ranges_from_anchor() {
    let visible_ids = vec![
        "one".to_string(),
        "two".to_string(),
        "three".to_string(),
        "four".to_string(),
    ];
    let mut selected_ids = HashSet::from(["one".to_string()]);
    let mut last_selected_id = Some("two".to_string());

    let count = select_connection_ids(
        &mut selected_ids,
        &mut last_selected_id,
        "four".to_string(),
        &visible_ids,
        false,
        true,
    );

    assert_eq!(count, 3);
    assert_eq!(
        selected_ids,
        HashSet::from(["two".to_string(), "three".to_string(), "four".to_string()])
    );
    assert_eq!(last_selected_id.as_deref(), Some("four"));
}

#[test]
fn selected_connections_for_list_state_follows_loaded_connection_order() {
    let connections = vec![
        saved_connection("first", "First", None, 0),
        saved_connection("second", "Second", None, 1),
        saved_connection("third", "Third", None, 2),
    ];
    let selected_ids = HashSet::from([
        "third".to_string(),
        "missing".to_string(),
        "first".to_string(),
    ]);

    let selected = selected_connections_for_list_state(&connections, &selected_ids);

    assert_eq!(
        selected
            .iter()
            .map(|connection| connection.id.as_str())
            .collect::<Vec<_>>(),
        vec!["first", "third"]
    );
}

#[test]
fn saved_connections_in_group_tree_for_list_state_includes_descendants_in_loaded_order() {
    let connections = vec![
        saved_connection("root", "Root", None, 0),
        saved_connection("direct", "Direct", Some("parent-group"), 1),
        saved_connection("nested", "Nested", Some("child-group"), 0),
        saved_connection("other", "Other", Some("other-group"), 0),
    ];
    let groups = vec![
        group("parent-group", "Parent Group", None, 0),
        group("child-group", "Child Group", Some("parent-group"), 0),
        group("other-group", "Other Group", None, 1),
    ];

    let grouped =
        saved_connections_in_group_tree_for_list_state(&connections, &groups, "parent-group");

    assert_eq!(
        grouped
            .iter()
            .map(|connection| connection.id.as_str())
            .collect::<Vec<_>>(),
        vec!["direct", "nested"]
    );
}

#[test]
fn visible_connection_ids_for_list_state_tracks_expanded_tree_order() {
    let connections = vec![
        saved_connection("root", "Root", None, 0),
        saved_connection("parent", "Parent", Some("parent-group"), 1),
        saved_connection("child", "Child", Some("child-group"), 0),
        saved_connection("closed", "Closed", Some("closed-group"), 0),
    ];
    let groups = vec![
        group("parent-group", "Parent Group", None, 0),
        group("child-group", "Child Group", Some("parent-group"), 0),
        group("closed-group", "Closed Group", None, 1),
    ];
    let expanded = HashSet::from(["parent-group".to_string(), "child-group".to_string()]);

    let visible = visible_connection_ids_for_list_state(
        &connections,
        &groups,
        "",
        ConnectionSortMode::Default,
        &expanded,
    );

    assert_eq!(visible, vec!["child", "parent", "root"]);
}

#[test]
fn connection_list_model_cache_hits_for_unchanged_revisions() {
    let mut cx = TestAppContext::single();
    let app = cache_test_app(&mut cx);
    seed_cached_connections(&mut cx, &app);

    let first = cx.update_entity(&app, |app, _| app.connection_state.connection_list_model());
    let second = cx.update_entity(&app, |app, _| app.connection_state.connection_list_model());

    assert!(!first.stats.cache_hit);
    assert!(second.stats.cache_hit);
    assert_eq!(first.rows.len(), second.rows.len());
    assert_eq!(second.stats.sections_ms, 0.0);
    assert_eq!(second.stats.flatten_ms, 0.0);
    assert_eq!(second.stats.widest_ms, 0.0);
}

#[test]
fn connection_list_model_cache_misses_when_search_changes() {
    let mut cx = TestAppContext::single();
    let app = cache_test_app(&mut cx);
    seed_cached_connections(&mut cx, &app);
    cx.update_entity(&app, |app, _| {
        let _ = app.connection_state.connection_list_model();
    });

    let filtered = cx.update_entity(&app, |app, _| {
        app.connection_state
            .set_list_search_text("child".to_string());
        app.connection_state.connection_list_model()
    });

    assert!(!filtered.stats.cache_hit);
    assert!(filtered.stats.flat_row_count > 0);
}

#[test]
fn connection_list_model_cache_misses_when_expansion_changes() {
    let mut cx = TestAppContext::single();
    let app = cache_test_app(&mut cx);
    seed_cached_connections(&mut cx, &app);
    cx.update_entity(&app, |app, _| {
        let _ = app.connection_state.connection_list_model();
    });

    let collapsed = cx.update_entity(&app, |app, _| {
        app.connection_state
            .toggle_list_group_expanded("parent-group".to_string());
        app.connection_state.connection_list_model()
    });

    assert!(!collapsed.stats.cache_hit);
}

#[test]
fn connection_list_model_cache_ignores_unrelated_shell_state() {
    let mut cx = TestAppContext::single();
    let app = cache_test_app(&mut cx);
    seed_cached_connections(&mut cx, &app);
    cx.update_entity(&app, |app, _| {
        let _ = app.connection_state.connection_list_model();
    });

    let after_shell_status_change = cx.update_entity(&app, |app, _| {
        app.shell.set_status("unrelated render state");
        app.connection_state.connection_list_model()
    });

    assert!(after_shell_status_change.stats.cache_hit);
}

#[test]
fn remove_connection_references_clears_invalid_list_state() {
    let mut selected_ids = HashSet::from(["one".to_string(), "two".to_string()]);
    let mut last_selected_id = Some("one".to_string());
    let mut drop_target = Some(ConnectionDropTarget {
        id: Some("one".to_string()),
        kind: ConnectionDragKind::Connection,
        position: ConnectionDropPosition::After,
    });

    remove_connection_list_references(
        &mut selected_ids,
        &mut last_selected_id,
        &mut drop_target,
        "one",
    );

    assert_eq!(selected_ids, HashSet::from(["two".to_string()]));
    assert_eq!(last_selected_id, None);
    assert_eq!(drop_target, None);
}

#[test]
fn remove_group_references_clears_invalid_list_state() {
    let mut expanded_group_ids = HashSet::from(["root".to_string(), "child".to_string()]);
    let mut hovered_group_id = Some("child".to_string());
    let mut drop_target = Some(ConnectionDropTarget {
        id: Some("child".to_string()),
        kind: ConnectionDragKind::Group,
        position: ConnectionDropPosition::Inside,
    });

    remove_group_list_references(
        &mut expanded_group_ids,
        &mut hovered_group_id,
        &mut drop_target,
        "child",
    );

    assert_eq!(expanded_group_ids, HashSet::from(["root".to_string()]));
    assert_eq!(hovered_group_id, None);
    assert_eq!(drop_target, None);
}

#[test]
fn retain_loaded_connection_list_references_prunes_stale_refresh_state() {
    let mut selected_ids = HashSet::from(["kept".to_string(), "stale".to_string()]);
    let mut last_selected_id = Some("stale".to_string());
    let mut expanded_group_ids =
        HashSet::from(["kept-group".to_string(), "stale-group".to_string()]);
    let mut hovered_group_id = Some("stale-group".to_string());
    let mut drop_target = Some(ConnectionDropTarget {
        id: Some("stale-group".to_string()),
        kind: ConnectionDragKind::Group,
        position: ConnectionDropPosition::Inside,
    });
    let connection_ids = HashSet::from(["kept".to_string()]);
    let group_ids = HashSet::from(["kept-group".to_string()]);

    retain_loaded_connection_references(
        &mut selected_ids,
        &mut last_selected_id,
        &mut drop_target,
        &connection_ids,
    );
    retain_loaded_group_list_references(
        &mut expanded_group_ids,
        &mut hovered_group_id,
        &mut drop_target,
        &group_ids,
    );

    assert_eq!(selected_ids, HashSet::from(["kept".to_string()]));
    assert_eq!(last_selected_id, None);
    assert_eq!(
        expanded_group_ids,
        HashSet::from(["kept-group".to_string()])
    );
    assert_eq!(hovered_group_id, None);
    assert_eq!(drop_target, None);
}

#[test]
fn clear_connection_editor_runtime_state_clears_secret_draft_and_icon_picker() {
    let mut draft = Some(connection_editor_state_with_secret_draft());
    let mut icon_picker_open = true;
    let mut group_select_open = true;
    let mut window = ChildWindowSlot::default();
    assert!(window.begin_open());

    clear_connection_editor_runtime_state(
        &mut draft,
        &mut icon_picker_open,
        &mut group_select_open,
        &mut window,
    );

    assert_eq!(draft, None);
    assert!(!icon_picker_open);
    assert!(!group_select_open);
    assert!(!window.is_open_or_pending());
}

#[test]
fn finish_connection_editor_save_state_clears_editor_and_selects_saved_connection() {
    let mut draft = Some(connection_editor_state_with_secret_draft());
    let mut icon_picker_open = true;
    let mut group_select_open = true;
    let mut window = ChildWindowSlot::default();
    assert!(window.begin_open());
    let mut selected_ids = HashSet::from(["old".to_string()]);
    let mut last_selected_id = Some("old".to_string());
    let mut expanded_group_ids = HashSet::from(["existing-group".to_string()]);

    clear_connection_editor_runtime_state(
        &mut draft,
        &mut icon_picker_open,
        &mut group_select_open,
        &mut window,
    );
    select_saved_connection_after_editor_save(
        &mut selected_ids,
        &mut last_selected_id,
        &mut expanded_group_ids,
        "conn-b".to_string(),
        Some("group-b".to_string()),
    );

    assert_eq!(draft, None);
    assert!(!icon_picker_open);
    assert!(!group_select_open);
    assert!(!window.is_open_or_pending());
    assert_eq!(selected_ids, HashSet::from(["conn-b".to_string()]));
    assert_eq!(last_selected_id.as_deref(), Some("conn-b"));
    assert!(expanded_group_ids.contains("existing-group"));
    assert!(expanded_group_ids.contains("group-b"));
}

#[test]
fn connection_editor_inline_panel_draft_requires_draft_without_window() {
    let draft = Some(connection_editor_state_with_secret_draft());

    assert_eq!(
        connection_editor_inline_panel_draft(&draft, false, false)
            .as_ref()
            .map(|editor| editor.id.as_deref()),
        Some(Some("conn"))
    );
    assert!(connection_editor_inline_panel_draft(&draft, true, false).is_none());
    assert!(connection_editor_inline_panel_draft(&draft, false, true).is_none());
    assert!(connection_editor_inline_panel_draft(&None, false, false).is_none());
}

fn connection_editor_owner(cx: &TestAppContext) -> ConnectionEditorFeatureState {
    let focus = cx.update(|cx| cx.focus_handle());
    let mut owner = ConnectionEditorFeatureState {
        draft: None,
        fields: HashMap::new(),
        number_fields: HashMap::new(),
        field_subscriptions: Vec::new(),
        number_field_subscriptions: Vec::new(),
        forwarding_endpoint_fields: HashMap::new(),
        forwarding_endpoint_field_subscriptions: Vec::new(),
        window: ChildWindowSlot::default(),
        focus,
        icon_picker_open: false,
        group_select_open: false,
        agent_identity_picker_open: false,
        agent_preview_generation: 0,
        group_select_trigger_bounds: None,
    };
    owner.begin_edit(connection_editor_state_with_secret_draft());
    owner
}

#[test]
fn connection_editor_group_select_is_mutually_exclusive_with_icon_picker() {
    let cx = TestAppContext::single();
    let mut owner = connection_editor_owner(&cx);

    assert!(owner.set_icon_picker_open(true));
    assert!(owner.icon_picker_is_open());
    assert!(!owner.group_select_is_open());

    owner.toggle_group_select();
    assert!(!owner.icon_picker_is_open());
    assert!(owner.group_select_is_open());
}

#[test]
fn connection_editor_group_selection_closes_select_and_clears_pending_group() {
    let cx = TestAppContext::single();
    let mut owner = connection_editor_owner(&cx);
    owner.toggle_group_select();
    owner.draft.as_mut().expect("draft").pending_group_name = Some("pending".to_string());
    owner.draft.as_mut().expect("draft").pending_group_parent_id = Some("parent".to_string());

    assert!(owner.set_select_value(ConnectionEditorSelect::Group, Some("group-a".to_string())));

    let editor = owner.draft.as_ref().expect("draft remains open");
    assert!(!owner.group_select_is_open());
    assert_eq!(editor.group_id.as_deref(), Some("group-a"));
    assert_eq!(editor.pending_group_name, None);
    assert_eq!(editor.pending_group_parent_id, None);
}

#[test]
fn connection_editor_new_group_commit_closes_select_and_keeps_pending_parent() {
    let cx = TestAppContext::single();
    let mut owner = connection_editor_owner(&cx);
    owner.toggle_group_select();
    owner.draft.as_mut().expect("draft").group_id = Some("parent".to_string());
    owner.draft.as_mut().expect("draft").new_group_name = "  staging  ".to_string();

    assert!(owner.commit_new_group("Group name is required".to_string()));

    let editor = owner.draft.as_ref().expect("draft remains open");
    assert!(!owner.group_select_is_open());
    assert_eq!(editor.pending_group_name.as_deref(), Some("staging"));
    assert_eq!(editor.pending_group_parent_id.as_deref(), Some("parent"));
}

#[test]
fn editing_a_field_clears_a_stale_validation_error() {
    let mut draft = ConnectionEditorState {
        error: Some("SSH host is required".to_string()),
        ..connection_editor_state_with_secret_draft()
    };

    set_connection_editor_field_text(
        &mut draft,
        ConnectionEditorField::Host,
        "10.0.0.5".to_string(),
    );

    assert_eq!(draft.host, "10.0.0.5");
    assert_eq!(draft.error, None);
}

#[test]
fn set_connection_editor_icon_trims_empty_values_and_clears_error() {
    let mut draft = Some(ConnectionEditorState {
        error: Some("stale validation".to_string()),
        ..connection_editor_state_with_secret_draft()
    });

    assert!(set_connection_editor_icon(&mut draft, Some("  server  ")));
    assert_eq!(
        draft.as_ref().and_then(|editor| editor.icon.as_deref()),
        Some("server")
    );
    assert_eq!(
        draft.as_ref().and_then(|editor| editor.error.as_deref()),
        None
    );

    assert!(set_connection_editor_icon(&mut draft, Some("  ")));
    assert_eq!(
        draft.as_ref().and_then(|editor| editor.icon.as_deref()),
        None
    );
}

#[test]
fn set_connection_editor_auth_none_clears_password_and_key_state() {
    let mut draft = Some(ConnectionEditorState {
        password_id: Some("saved-password".to_string()),
        key_id: Some("key-a".to_string()),
        error: Some("stale validation".to_string()),
        ..connection_editor_state_with_secret_draft()
    });

    assert!(set_connection_editor_select_value(
        &mut draft,
        ConnectionEditorSelect::Authentication,
        Some("none".to_string()),
    ));

    let editor = draft.expect("editor remains open");
    assert_eq!(editor.auth_mode, "none");
    assert_eq!(editor.password_source, ConnectionEditorPasswordSource::Ask);
    assert_eq!(editor.password_id, None);
    assert!(editor.password.is_empty());
    assert_eq!(editor.existing_password, None);
    assert_eq!(editor.key_id, None);
    assert_eq!(editor.error, None);
}

#[test]
fn set_connection_editor_agent_endpoint_updates_the_authentication_endpoint() {
    let mut draft = Some(connection_editor_state_with_secret_draft());

    assert!(set_connection_editor_select_value(
        &mut draft,
        ConnectionEditorSelect::SshAgentEndpoint,
        Some("environment".to_string()),
    ));

    assert_eq!(
        draft.expect("editor remains open").agent_endpoint,
        SshAgentEndpoint::Environment {
            variable: "SSH_AUTH_SOCK".to_string(),
        }
    );
}

#[test]
fn enabling_allow_all_agent_forwarding_requires_fresh_confirmation() {
    let mut draft = Some(ConnectionEditorState {
        agent_allow_all_confirmed: true,
        agent_forwarding_config: nyaterm_core::SshAgentForwardingConfig {
            enabled: false,
            policy: SshAgentForwardingPolicy::All,
            ..Default::default()
        },
        ..connection_editor_state_with_secret_draft()
    });

    assert!(toggle_connection_editor_flag(
        &mut draft,
        ConnectionEditorToggle::AgentForwarding,
    ));

    let editor = draft.expect("editor remains open");
    assert!(editor.agent_forwarding_config.enabled);
    assert!(!editor.agent_allow_all_confirmed);
}

#[test]
fn set_connection_editor_group_select_value_clears_group_draft() {
    let mut draft = Some(ConnectionEditorState {
        new_group_name: "scratch".to_string(),
        pending_group_name: Some("pending".to_string()),
        pending_group_parent_id: Some("parent".to_string()),
        focused_field: ConnectionEditorField::NewGroupName,
        error: Some("stale validation".to_string()),
        ..connection_editor_state_with_secret_draft()
    });

    assert!(set_connection_editor_select_value(
        &mut draft,
        ConnectionEditorSelect::Group,
        Some("group-a".to_string()),
    ));

    let editor = draft.expect("editor remains open");
    assert_eq!(editor.group_id.as_deref(), Some("group-a"));
    assert!(editor.new_group_name.is_empty());
    assert_eq!(editor.pending_group_name, None);
    assert_eq!(editor.pending_group_parent_id, None);
    assert_eq!(editor.focused_field, ConnectionEditorField::Name);
    assert_eq!(editor.error, None);
}

#[test]
fn ssh_profile_selection_preserves_explicit_terminal_type() {
    let mut draft = Some(ConnectionEditorState {
        terminal_type: Some(SshTerminalType::Ansi),
        ..connection_editor_state_with_secret_draft()
    });

    assert!(set_connection_editor_select_value(
        &mut draft,
        ConnectionEditorSelect::SshProfile,
        Some("network_device".to_string()),
    ));

    let editor = draft.as_ref().expect("editor remains open");
    assert_eq!(editor.ssh_profile, SshProfile::NetworkDevice);
    assert_eq!(editor.terminal_type, Some(SshTerminalType::Ansi));

    assert!(set_connection_editor_select_value(
        &mut draft,
        ConnectionEditorSelect::SshTerminalType,
        None,
    ));
    let editor = draft.expect("editor remains open");
    assert_eq!(editor.terminal_type, None);
    assert_eq!(
        nyaterm_core::resolve_ssh_terminal_type(editor.ssh_profile, editor.terminal_type),
        SshTerminalType::Vt100
    );
}

#[test]
fn ssh_algorithm_mode_fills_only_empty_custom_lists_and_clears_presets() {
    let mut draft = Some(ConnectionEditorState {
        ssh_algorithm_kex: vec!["future-kex".to_string()],
        ..connection_editor_state_with_secret_draft()
    });

    assert!(set_connection_editor_select_value(
        &mut draft,
        ConnectionEditorSelect::SshAlgorithmMode,
        Some("custom".to_string()),
    ));
    let editor = draft.as_ref().expect("editor remains open");
    let defaults = &nyaterm_transport::supported_ssh_algorithms().compatible;
    assert_eq!(editor.ssh_algorithm_kex, ["future-kex"]);
    assert_eq!(editor.ssh_algorithm_ciphers, defaults.ciphers);
    assert_eq!(editor.ssh_algorithm_macs, defaults.macs);
    assert_eq!(editor.ssh_algorithm_host_keys, defaults.host_keys);

    assert!(set_connection_editor_select_value(
        &mut draft,
        ConnectionEditorSelect::SshAlgorithmMode,
        Some("secure".to_string()),
    ));
    let editor = draft.expect("editor remains open");
    assert_eq!(editor.ssh_algorithm_mode, "secure");
    assert!(editor.ssh_algorithm_kex.is_empty());
    assert!(editor.ssh_algorithm_ciphers.is_empty());
    assert!(editor.ssh_algorithm_macs.is_empty());
    assert!(editor.ssh_algorithm_host_keys.is_empty());
}

#[test]
fn ssh_algorithm_custom_edits_preserve_unknown_values_and_order() {
    let defaults = &nyaterm_transport::supported_ssh_algorithms().compatible;
    let first = defaults.kex[0].clone();
    let second = defaults.kex[1].clone();
    let mut draft = Some(ConnectionEditorState {
        ssh_algorithm_mode: "custom".to_string(),
        ssh_algorithm_kex: vec!["future-kex".to_string(), first.clone(), second.clone()],
        ..connection_editor_state_with_secret_draft()
    });

    assert!(set_connection_editor_ssh_algorithm_tab(
        &mut draft,
        ConnectionEditorSshAlgorithmTab::Ciphers,
    ));
    assert!(move_connection_editor_ssh_algorithm(
        &mut draft,
        ConnectionEditorSshAlgorithmTab::KeyExchange,
        &first,
        1,
    ));
    assert_eq!(
        draft.as_ref().expect("editor").ssh_algorithm_kex,
        ["future-kex", second.as_str(), first.as_str()]
    );
    assert!(set_connection_editor_ssh_algorithm_enabled(
        &mut draft,
        ConnectionEditorSshAlgorithmTab::KeyExchange,
        "future-kex",
        false,
    ));
    assert_eq!(
        draft.as_ref().expect("editor").ssh_algorithm_kex,
        [second.as_str(), first.as_str()]
    );
    assert!(!move_connection_editor_ssh_algorithm(
        &mut draft,
        ConnectionEditorSshAlgorithmTab::KeyExchange,
        &second,
        -1,
    ));

    assert!(set_connection_editor_ssh_algorithm_enabled(
        &mut draft,
        ConnectionEditorSshAlgorithmTab::KeyExchange,
        &first,
        false,
    ));
    assert!(!set_connection_editor_ssh_algorithm_enabled(
        &mut draft,
        ConnectionEditorSshAlgorithmTab::KeyExchange,
        &second,
        false,
    ));
    assert!(!set_connection_editor_ssh_algorithm_enabled(
        &mut draft,
        ConnectionEditorSshAlgorithmTab::KeyExchange,
        "future-kex",
        true,
    ));
}

#[test]
fn recording_edits_preserve_advanced_compatibility_fields() {
    let expected_path = "logs/{session}.log".to_string();
    let expected_rotation = RecordingRotationPolicy::Size { max_bytes: 8192 };
    let mut draft = Some(ConnectionEditorState {
        recording: Some(ConnectionRecordingSettings {
            auto_start: Some(true),
            mode: Some(RecordingMode::Raw),
            path_template: Some(expected_path.clone()),
            include_timestamps: Some(false),
            rotation: Some(expected_rotation.clone()),
        }),
        ..connection_editor_state_with_secret_draft()
    });

    assert!(toggle_connection_editor_flag(
        &mut draft,
        ConnectionEditorToggle::RecordingAutoStart,
    ));
    assert!(set_connection_editor_select_value(
        &mut draft,
        ConnectionEditorSelect::RecordingMode,
        Some("transcript".to_string()),
    ));

    let recording = draft
        .as_ref()
        .and_then(|editor| editor.recording.as_ref())
        .expect("connection override remains enabled");
    assert_eq!(recording.auto_start, Some(false));
    assert_eq!(recording.mode, Some(RecordingMode::Transcript));
    assert_eq!(
        recording.path_template.as_deref(),
        Some(expected_path.as_str())
    );
    assert_eq!(recording.include_timestamps, Some(false));
    assert_eq!(recording.rotation, Some(expected_rotation));

    assert!(toggle_connection_editor_flag(
        &mut draft,
        ConnectionEditorToggle::RecordingUseGlobal,
    ));
    assert_eq!(draft.expect("editor remains open").recording, None);
}

#[test]
fn set_connection_editor_password_source_clears_secret_drafts() {
    let mut draft = Some(ConnectionEditorState {
        password_id: Some("saved-password".to_string()),
        ..connection_editor_state_with_secret_draft()
    });

    assert!(set_connection_editor_password_source(
        &mut draft,
        ConnectionEditorPasswordSource::Saved
    ));

    let editor = draft.as_ref().expect("editor remains open");
    assert_eq!(
        editor.password_source,
        ConnectionEditorPasswordSource::Saved
    );
    assert!(editor.password.is_empty());
    assert_eq!(editor.existing_password, None);

    assert!(set_connection_editor_password_source(
        &mut draft,
        ConnectionEditorPasswordSource::Ask
    ));

    let editor = draft.expect("editor remains open");
    assert_eq!(editor.password_source, ConnectionEditorPasswordSource::Ask);
    assert_eq!(editor.password_id, None);
    assert!(editor.password.is_empty());
    assert_eq!(editor.existing_password, None);
}

#[test]
fn set_connection_editor_advanced_tab_resets_hidden_post_login_focus() {
    let mut draft = Some(ConnectionEditorState {
        focused_field: ConnectionEditorField::PostLoginCommand,
        advanced_behavior_tab: ConnectionEditorAdvancedTab::PostLogin,
        ..connection_editor_state_with_secret_draft()
    });

    assert!(set_connection_editor_advanced_tab(
        &mut draft,
        ConnectionEditorAdvancedTab::X11
    ));

    let editor = draft.expect("editor remains open");
    assert_eq!(
        editor.advanced_behavior_tab,
        ConnectionEditorAdvancedTab::X11
    );
    assert_eq!(editor.focused_field, ConnectionEditorField::Name);

    let mut draft = Some(connection_editor_state_with_secret_draft());
    assert!(set_connection_editor_advanced_tab(
        &mut draft,
        ConnectionEditorAdvancedTab::AgentForwarding
    ));
    assert_eq!(
        draft.expect("editor remains open").advanced_network_tab,
        ConnectionEditorAdvancedTab::AgentForwarding
    );
}

#[test]
fn set_connection_editor_kind_updates_default_ports_and_clears_error() {
    let mut draft = Some(ConnectionEditorState {
        port: "22".to_string(),
        error: Some("stale validation".to_string()),
        ..connection_editor_state_with_secret_draft()
    });

    assert!(set_connection_editor_kind(
        &mut draft,
        ConnectionKindTab::Telnet
    ));

    let editor = draft.expect("editor remains open");
    assert_eq!(editor.kind, ConnectionKindTab::Telnet);
    assert_eq!(editor.port, "23");
    assert_eq!(editor.focused_field, ConnectionEditorField::Name);
    assert_eq!(editor.error, None);
}

#[test]
fn set_connection_editor_telnet_tab_clears_error() {
    let mut draft = Some(ConnectionEditorState {
        error: Some("stale validation".to_string()),
        ..connection_editor_state_with_secret_draft()
    });

    assert!(set_connection_editor_telnet_tab(
        &mut draft,
        ConnectionEditorTelnetTab::Compatibility
    ));

    let editor = draft.expect("editor remains open");
    assert_eq!(
        editor.telnet_advanced_tab,
        ConnectionEditorTelnetTab::Compatibility
    );
    assert_eq!(editor.error, None);
}

#[test]
fn commit_connection_editor_new_group_requires_non_empty_name() {
    let mut draft = Some(ConnectionEditorState {
        new_group_name: "  ".to_string(),
        error: None,
        ..connection_editor_state_with_secret_draft()
    });

    assert!(commit_connection_editor_new_group(
        &mut draft,
        "Group name is required".to_string()
    ));

    let editor = draft.expect("editor remains open");
    assert_eq!(editor.error.as_deref(), Some("Group name is required"));
    assert_eq!(editor.pending_group_name, None);
}

#[test]
fn commit_connection_editor_new_group_captures_parent_and_clears_draft() {
    let mut draft = Some(ConnectionEditorState {
        group_id: Some("parent".to_string()),
        new_group_name: "  staging  ".to_string(),
        focused_field: ConnectionEditorField::NewGroupName,
        error: Some("stale validation".to_string()),
        ..connection_editor_state_with_secret_draft()
    });

    assert!(commit_connection_editor_new_group(
        &mut draft,
        "Group name is required".to_string()
    ));

    let editor = draft.expect("editor remains open");
    assert_eq!(editor.pending_group_name.as_deref(), Some("staging"));
    assert_eq!(editor.pending_group_parent_id.as_deref(), Some("parent"));
    assert_eq!(editor.group_id, None);
    assert!(editor.new_group_name.is_empty());
    assert_eq!(editor.focused_field, ConnectionEditorField::Name);
    assert_eq!(editor.error, None);
}

#[test]
fn toggle_connection_editor_raw_tcp_forces_cr_enter_mode() {
    let mut draft = Some(ConnectionEditorState {
        raw_tcp_cli: false,
        telnet_enter_mode: "lf".to_string(),
        error: Some("stale validation".to_string()),
        ..connection_editor_state_with_secret_draft()
    });

    assert!(toggle_connection_editor_flag(
        &mut draft,
        ConnectionEditorToggle::RawTcp
    ));

    let editor = draft.expect("editor remains open");
    assert!(editor.raw_tcp_cli);
    assert_eq!(editor.telnet_enter_mode, "cr");
    assert_eq!(editor.error, None);
}

#[test]
fn toggle_connection_editor_advanced_closed_resets_hidden_focus() {
    let mut draft = Some(ConnectionEditorState {
        advanced_open: true,
        focused_field: ConnectionEditorField::PostLoginDelay,
        error: Some("stale validation".to_string()),
        ..connection_editor_state_with_secret_draft()
    });

    assert!(toggle_connection_editor_flag(
        &mut draft,
        ConnectionEditorToggle::Advanced
    ));

    let editor = draft.expect("editor remains open");
    assert!(!editor.advanced_open);
    assert_eq!(editor.focused_field, ConnectionEditorField::Name);
    assert_eq!(editor.error, None);
}

#[test]
fn insert_connection_editor_description_newline_only_when_description_focused() {
    let mut draft = Some(ConnectionEditorState {
        focused_field: ConnectionEditorField::Description,
        description: "first".to_string(),
        error: Some("stale validation".to_string()),
        ..connection_editor_state_with_secret_draft()
    });

    assert!(insert_connection_editor_description_newline(&mut draft));

    let editor = draft.as_ref().expect("editor remains open");
    assert_eq!(editor.description, "first\n");
    assert_eq!(editor.error, None);

    draft.as_mut().expect("editor remains open").focused_field = ConnectionEditorField::Name;
    assert!(!insert_connection_editor_description_newline(&mut draft));
}

#[test]
fn set_connection_editor_error_updates_active_draft() {
    let mut draft = Some(connection_editor_state_with_secret_draft());

    assert!(set_connection_editor_error(
        &mut draft,
        "SSH host is required".to_string()
    ));

    assert_eq!(
        draft.and_then(|editor| editor.error),
        Some("SSH host is required".to_string())
    );
}

#[test]
fn apply_connection_editor_paths_update_field_and_clear_error() {
    let mut draft = Some(ConnectionEditorState {
        error: Some("stale validation".to_string()),
        ..connection_editor_state_with_secret_draft()
    });

    assert!(apply_connection_editor_shell_path(
        &mut draft,
        "/bin/zsh".to_string()
    ));
    assert!(apply_connection_editor_working_dir(
        &mut draft,
        "/home/kang".to_string()
    ));

    let editor = draft.expect("editor remains open");
    assert_eq!(editor.shell_path, "/bin/zsh");
    assert_eq!(editor.working_dir, "/home/kang");
    assert_eq!(editor.error, None);
}

#[test]
fn set_connection_group_editor_error_updates_active_draft() {
    let mut draft = Some(ConnectionGroupEditorState {
        mode: ConnectionGroupEditorMode::Create,
        id: None,
        name: String::new(),
        parent_id: None,
        error: None,
    });

    assert!(set_connection_group_editor_error(
        &mut draft,
        "Folder name is required".to_string()
    ));

    assert_eq!(
        draft.and_then(|editor| editor.error),
        Some("Folder name is required".to_string())
    );
}

#[test]
fn network_group_editor_name_updates_name_and_clears_error() {
    let mut group_editor = Some(NetworkGroupEditorState {
        tab: NetworkTab::Tunnels,
        id: Some("group-a".to_string()),
        name: "prod".to_string(),
        error: Some("stale validation".to_string()),
    });

    assert!(set_network_group_editor_name(
        &mut group_editor,
        "staging".to_string()
    ));

    let editor = group_editor.expect("network group editor remains open");
    assert_eq!(editor.name, "staging");
    assert_eq!(editor.error, None);
}

#[test]
fn set_network_group_editor_error_updates_active_draft() {
    let mut group_editor = Some(NetworkGroupEditorState {
        tab: NetworkTab::Proxies,
        id: None,
        name: String::new(),
        error: None,
    });

    assert!(set_network_group_editor_error(
        &mut group_editor,
        "Group name is required".to_string()
    ));

    assert_eq!(
        group_editor.and_then(|editor| editor.error),
        Some("Group name is required".to_string())
    );
}

#[test]
fn network_tunnel_editor_field_filters_ports_and_clears_error() {
    let mut tunnel_editor = Some(NetworkTunnelEditorState {
        focused_field: NetworkTunnelEditorField::Name,
        listen_port: String::new(),
        error: Some("stale validation".to_string()),
        ..network_tunnel_editor("tunnel-a")
    });

    assert!(set_network_tunnel_editor_field(
        &mut tunnel_editor,
        NetworkTunnelEditorField::ListenPort,
        "8x0".to_string(),
    ));

    let editor = tunnel_editor.expect("tunnel editor remains open");
    assert_eq!(editor.listen_port, "80");
    assert_eq!(editor.error, None);
}

#[test]
fn network_tunnel_type_selection_resets_hidden_dynamic_focus() {
    let mut tunnel_editor = Some(NetworkTunnelEditorState {
        tunnel_type: "remote".to_string(),
        focused_field: NetworkTunnelEditorField::TargetPort,
        error: Some("stale validation".to_string()),
        ..network_tunnel_editor("tunnel-a")
    });

    assert_eq!(
        set_network_tunnel_type(&mut tunnel_editor, "dynamic").as_deref(),
        Some("dynamic")
    );

    let editor = tunnel_editor.expect("tunnel editor remains open");
    assert_eq!(editor.tunnel_type, "dynamic");
    assert_eq!(editor.focused_field, NetworkTunnelEditorField::ListenPort);
    assert_eq!(editor.error, None);
}

#[test]
fn network_tunnel_selects_connection_group_and_flags() {
    let mut tunnel_editor = Some(NetworkTunnelEditorState {
        connection_id: Some("conn-a".to_string()),
        group_id: None,
        auto_open: false,
        bind_localhost: false,
        error: Some("stale validation".to_string()),
        ..network_tunnel_editor("tunnel-a")
    });

    assert!(set_network_tunnel_connection(
        &mut tunnel_editor,
        Some("conn-b".to_string()),
    ));
    assert!(set_network_tunnel_group(
        &mut tunnel_editor,
        Some("group-a".to_string()),
    ));
    assert!(set_network_tunnel_bind_localhost(&mut tunnel_editor, true));
    assert_eq!(
        toggle_network_tunnel_auto_open(&mut tunnel_editor),
        Some(true)
    );

    let editor = tunnel_editor.expect("tunnel editor remains open");
    assert_eq!(editor.connection_id.as_deref(), Some("conn-b"));
    assert_eq!(editor.group_id.as_deref(), Some("group-a"));
    assert!(editor.bind_localhost);
    assert!(editor.auto_open);
    assert_eq!(editor.error, None);
}

#[test]
fn network_proxy_editor_field_filters_port_and_preserves_password_draft() {
    let mut proxy_editor = Some(NetworkProxyEditorState {
        focused_field: NetworkProxyEditorField::Port,
        port: String::new(),
        password: "draft-password".to_string().into(),
        existing_password: Some("existing-password".to_string().into()),
        error: Some("stale validation".to_string()),
        ..network_proxy_editor("proxy-a")
    });

    assert!(set_network_proxy_editor_field(
        &mut proxy_editor,
        NetworkProxyEditorField::Port,
        "1x2".to_string(),
    ));

    let editor = proxy_editor.expect("proxy editor remains open");
    assert_eq!(editor.port, "12");
    assert_eq!(editor.password.expose_secret(), "draft-password");
    assert_eq!(
        editor.existing_password.as_deref(),
        Some("existing-password")
    );
    assert_eq!(editor.error, None);
}

#[test]
fn network_proxy_protocol_selection_resets_hidden_focus() {
    let mut proxy_editor = Some(NetworkProxyEditorState {
        protocol: "http".to_string(),
        focused_field: NetworkProxyEditorField::Password,
        error: Some("stale validation".to_string()),
        ..network_proxy_editor("proxy-a")
    });

    assert_eq!(
        set_network_proxy_protocol(&mut proxy_editor, "proxycommand").as_deref(),
        Some("proxycommand")
    );

    let editor = proxy_editor.expect("proxy editor remains open");
    assert_eq!(editor.protocol, "proxycommand");
    assert_eq!(editor.focused_field, NetworkProxyEditorField::Command);
    assert_eq!(editor.error, None);
}

#[test]
fn toggle_network_move_picker_toggles_same_item() {
    let mut move_picker = None;

    assert!(toggle_network_move_picker_state(
        &mut move_picker,
        NetworkTab::Proxies,
        "proxy".to_string(),
    ));

    assert_eq!(
        move_picker,
        Some(NetworkMovePickerState {
            tab: NetworkTab::Proxies,
            id: "proxy".to_string(),
        })
    );

    assert!(!toggle_network_move_picker_state(
        &mut move_picker,
        NetworkTab::Proxies,
        "proxy".to_string(),
    ));

    assert_eq!(move_picker, None);
}

#[test]
fn remove_network_item_references_clears_only_matching_tab_and_id() {
    let mut move_picker = Some(NetworkMovePickerState {
        tab: NetworkTab::Tunnels,
        id: "one".to_string(),
    });
    let mut tunnel_editor = Some(network_tunnel_editor("one"));
    let mut proxy_editor = Some(network_proxy_editor("one"));

    remove_network_item_references(
        &mut move_picker,
        &mut tunnel_editor,
        &mut proxy_editor,
        NetworkTab::Tunnels,
        "one",
    );

    assert_eq!(move_picker, None);
    assert_eq!(tunnel_editor, None);
    assert_eq!(proxy_editor, Some(network_proxy_editor("one")));
}

#[test]
fn remove_network_group_references_clears_matching_group_state() {
    let mut group_editor = Some(NetworkGroupEditorState {
        tab: NetworkTab::Tunnels,
        id: Some("group-a".to_string()),
        name: "Group A".to_string(),
        error: Some("stale".to_string()),
    });
    let mut expanded_sections = HashSet::from([
        "tunnel:group-a".to_string(),
        "proxy:group-a".to_string(),
        "tunnel:group-b".to_string(),
    ]);

    remove_network_group_references(
        &mut group_editor,
        &mut expanded_sections,
        NetworkTab::Tunnels,
        "group-a",
    );

    assert_eq!(group_editor, None);
    assert_eq!(
        expanded_sections,
        HashSet::from(["proxy:group-a".to_string(), "tunnel:group-b".to_string()])
    );
}

#[test]
fn remove_network_group_and_item_references_clears_deleted_child_state() {
    let mut group_editor = Some(NetworkGroupEditorState {
        tab: NetworkTab::Proxies,
        id: Some("group-a".to_string()),
        name: "Group A".to_string(),
        error: None,
    });
    let mut expanded_sections = HashSet::from(["proxy:group-a".to_string()]);
    let mut move_picker = Some(NetworkMovePickerState {
        tab: NetworkTab::Proxies,
        id: "proxy-a".to_string(),
    });
    let mut tunnel_editor = Some(network_tunnel_editor("proxy-a"));
    let mut proxy_editor = Some(network_proxy_editor("proxy-a"));

    remove_network_group_references(
        &mut group_editor,
        &mut expanded_sections,
        NetworkTab::Proxies,
        "group-a",
    );
    remove_network_item_references(
        &mut move_picker,
        &mut tunnel_editor,
        &mut proxy_editor,
        NetworkTab::Proxies,
        "proxy-a",
    );

    assert_eq!(group_editor, None);
    assert!(expanded_sections.is_empty());
    assert_eq!(move_picker, None);
    assert_eq!(proxy_editor, None);
    assert_eq!(tunnel_editor, Some(network_tunnel_editor("proxy-a")));
}

#[test]
fn clear_network_tunnel_editor_closes_active_draft() {
    let mut tunnel_editor = Some(network_tunnel_editor("tunnel-a"));

    clear_network_tunnel_editor(&mut tunnel_editor);

    assert_eq!(tunnel_editor, None);
}

#[test]
fn set_network_tunnel_editor_error_updates_active_editor() {
    let mut tunnel_editor = Some(network_tunnel_editor("tunnel-a"));

    assert!(set_network_tunnel_editor_error(
        &mut tunnel_editor,
        "Tunnel name is required".to_string()
    ));

    assert_eq!(
        tunnel_editor.and_then(|editor| editor.error),
        Some("Tunnel name is required".to_string())
    );
}

#[test]
fn clear_network_proxy_editor_clears_secret_draft() {
    let mut proxy_editor = Some(network_proxy_editor("proxy-a"));

    clear_network_proxy_editor(&mut proxy_editor);

    assert_eq!(proxy_editor, None);
}

#[test]
fn set_network_proxy_editor_error_updates_active_editor() {
    let mut proxy_editor = Some(network_proxy_editor("proxy-a"));

    assert!(set_network_proxy_editor_error(
        &mut proxy_editor,
        "Proxy host is required".to_string()
    ));

    assert_eq!(
        proxy_editor.and_then(|editor| editor.error),
        Some("Proxy host is required".to_string())
    );
}

fn cache_test_app(cx: &mut TestAppContext) -> gpui::Entity<NyaTermApp> {
    let root = unique_test_dir("connection-list-cache");
    let runtime = AppRuntime::from_parts_for_test(
        RuntimeMode::Portable,
        root.clone(),
        root.join("config"),
        root.join("logs"),
        root.join("cache"),
        None,
    );
    let stores = UiStoreHandles {
        startup_restore: cx.new(|_| StartupRestoreStore::default()),
        overlays: cx.new(|_| OverlayStore::default()),
    };
    cx.new(|cx| NyaTermApp::new(runtime, stores, cx))
}

fn seed_cached_connections(cx: &mut TestAppContext, app: &gpui::Entity<NyaTermApp>) {
    let connections = vec![
        saved_connection("root", "Root", None, 0),
        saved_connection("parent", "Parent", Some("parent-group"), 1),
        saved_connection("child", "Child", Some("child-group"), 0),
    ];
    let groups = vec![
        group("parent-group", "Parent Group", None, 0),
        group("child-group", "Child Group", Some("parent-group"), 0),
    ];
    cx.update_entity(app, |app, _| {
        app.connection_state.replace_loaded(connections, groups);
        app.connection_state
            .expand_list_group("parent-group".to_string());
        app.connection_state
            .expand_list_group("child-group".to_string());
    });
}

fn unique_test_dir(label: &str) -> PathBuf {
    // A uuid rather than a clock reading: these tests run in parallel and
    // Windows' ~15ms clock granularity lets a nanosecond timestamp repeat, which
    // would share one config dir and so one settings database.
    std::env::temp_dir().join(format!("nyaterm-{label}-{}-{}", std::process::id(), uuid()))
}

fn saved_connection(
    id: &str,
    name: &str,
    group_id: Option<&str>,
    sort_order: i32,
) -> SavedConnection {
    SavedConnection {
        extensions: Default::default(),
        id: id.to_string(),
        name: name.to_string(),
        config: ConnectionType::LocalTerminal {
            shell_path: String::new(),
            shell_args: String::new(),
            working_dir: None,
            ai_execution_profile: AiExecutionProfile::Auto,
            encoding: String::new(),
            dynamic_tab_title: false,
        },
        group_id: group_id.map(ToOwned::to_owned),
        description: None,
        sort_order,
        icon: None,
        icon_auto_detect: None,
        auth: None,
        recording: None,
        ssh_algorithms: None,
        ssh_profile: Default::default(),
        terminal_type: None,
        sftp: Default::default(),
        network: None,
        post_login: None,
        asset: None,
        created_at_ms: None,
        updated_at_ms: None,
        last_used_at_ms: None,
    }
}

fn group(id: &str, name: &str, parent_id: Option<&str>, sort_order: i32) -> Group {
    Group {
        id: id.to_string(),
        name: name.to_string(),
        parent_id: parent_id.map(ToOwned::to_owned),
        sort_order,
        created_at_ms: None,
        updated_at_ms: None,
    }
}

fn connection_editor_state_with_secret_draft() -> ConnectionEditorState {
    ConnectionEditorState {
        id: Some("conn".to_string()),
        kind: ConnectionKindTab::Ssh,
        name: "prod".to_string(),
        description: String::new(),
        icon: None,
        icon_auto_detect: true,
        group_id: None,
        new_group_name: String::new(),
        pending_group_name: None,
        pending_group_parent_id: None,
        host: "example.test".to_string(),
        port: "22".to_string(),
        username: "root".to_string(),
        domain: String::new(),
        auth_mode: "password".to_string(),
        rdp_security: Default::default(),
        rdp_display: Default::default(),
        rdp_clipboard: Default::default(),
        rdp_reconnect: Default::default(),
        rdp_advanced_tab: crate::models::ConnectionEditorRdpTab::Security,
        vnc_security: Default::default(),
        vnc_display: Default::default(),
        vnc_clipboard: Default::default(),
        vnc_reconnect: Default::default(),
        vnc_shared: true,
        vnc_view_only: false,
        password_source: ConnectionEditorPasswordSource::Direct,
        password_id: None,
        password: "draft-secret".to_string().into(),
        existing_password: Some("existing-secret".to_string().into()),
        key_id: None,
        otp_id: None,
        auto_fill_otp: false,
        proxy_id: None,
        proxy_jump_id: None,
        x11_forwarding: false,
        dynamic_tab_title: false,
        agent_endpoint: Default::default(),
        agent_forwarding_config: Default::default(),
        agent_allow_all_confirmed: false,
        agent_forwarding_endpoint_index: 0,
        agent_preview: None,
        agent_preview_loading: false,
        backspace_mode: "del".to_string(),
        encoding: "global".to_string(),
        ssh_profile: Default::default(),
        terminal_type: None,
        sftp_enabled: true,
        sftp_cwd_follow_mode: "shell_integration".to_string(),
        sftp_shell_detection_timeout_ms: "3000".to_string(),
        sftp_pipeline_depth: None,
        sftp_extra: Default::default(),
        sftp_filename_encoding: "terminal".to_string(),
        ssh_algorithm_mode: "compatible".to_string(),
        ssh_algorithm_kex: Vec::new(),
        ssh_algorithm_ciphers: Vec::new(),
        ssh_algorithm_macs: Vec::new(),
        ssh_algorithm_host_keys: Vec::new(),
        ssh_algorithm_tab: ConnectionEditorSshAlgorithmTab::KeyExchange,
        shell_path: String::new(),
        shell_args: String::new(),
        working_dir: String::new(),
        serial_port: String::new(),
        baud_rate: "115200".to_string(),
        data_bits: "8".to_string(),
        parity: "none".to_string(),
        stop_bits: "1".to_string(),
        raw_tcp_cli: false,
        telnet_enter_mode: "cr".to_string(),
        local_echo: false,
        local_line_edit: false,
        force_character_at_a_time: false,
        send_naws: true,
        send_sga: true,
        telnet_auto_login_enabled: true,
        telnet_auto_login_send_wake_enter: true,
        telnet_auto_login_timeout_ms: "60000".to_string(),
        telnet_auto_login_username_prompt_regex: String::new(),
        telnet_auto_login_password_prompt_regex: String::new(),
        telnet_auto_login_success_prompt_regex: String::new(),
        telnet_auto_login_failure_prompt_regex: String::new(),
        telnet_auto_login_max_retries: "0".to_string(),
        post_login_enabled: false,
        post_login_command: String::new(),
        post_login_delay_ms: "1000".to_string(),
        recording: None,
        advanced_open: false,
        advanced_network_tab: ConnectionEditorAdvancedTab::Proxy,
        advanced_behavior_tab: ConnectionEditorAdvancedTab::PostLogin,
        telnet_advanced_tab: ConnectionEditorTelnetTab::Input,
        connect_after_save: false,
        focused_field: ConnectionEditorField::Name,
        error: None,
    }
}

fn network_tunnel_editor(id: &str) -> NetworkTunnelEditorState {
    NetworkTunnelEditorState {
        id: Some(id.to_string()),
        is_open: false,
        name: "Tunnel".to_string(),
        tunnel_type: "local".to_string(),
        connection_id: Some("conn".to_string()),
        listen_port: "8080".to_string(),
        target_host: "127.0.0.1".to_string(),
        target_port: "80".to_string(),
        auto_open: false,
        bind_localhost: true,
        group_id: None,
        focused_field: NetworkTunnelEditorField::Name,
        error: None,
    }
}

fn network_proxy_editor(id: &str) -> NetworkProxyEditorState {
    NetworkProxyEditorState {
        id: Some(id.to_string()),
        name: "Proxy".to_string(),
        protocol: "socks5".to_string(),
        host: "127.0.0.1".to_string(),
        port: "1080".to_string(),
        command: String::new(),
        username: String::new(),
        password: "draft-password".to_string().into(),
        existing_password: Some("existing-password".to_string().into()),
        password_id: None,
        group_id: None,
        focused_field: NetworkProxyEditorField::Name,
        error: None,
    }
}
