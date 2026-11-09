use std::collections::HashMap;

use gpui::TestAppContext;
use nyaterm_core::{
    AppSettingsSummary, KeywordHighlightConfig, KeywordHighlightRule, SearchEngineConfig,
};

use super::persistence::SettingsPersistenceDomain;
use super::search_engines::SearchEngineMenu;
use super::{
    SettingsFeatureFocus, SettingsFeatureInit, SettingsFeatureState, UiLayoutSettingsUpdate,
};
use crate::features::{
    FontAvailability, FontAvailabilityReason, FontCatalogEntry, FontCatalogSnapshot,
    FontCatalogState,
};
use crate::models::{
    ConfigPathPromptKind, KeywordHighlightEditorField, SnapshotPasswordPromptKind,
};

fn settings_state() -> SettingsFeatureState {
    let cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let focus = || cx.update(|cx| cx.focus_handle());
    SettingsFeatureState::new(
        SettingsFeatureInit {
            summary: AppSettingsSummary::default(),
            keyword_config: KeywordHighlightConfig::default(),
            store_path: String::new(),
            store_message: String::new(),
            store_ready: true,
            ui_font_options: vec!["Inter".to_string()],
            terminal_font_options: vec!["JetBrains Mono".to_string()],
        },
        SettingsFeatureFocus {
            search_engine: focus(),
            keyword_highlight: focus(),
            keybindings: focus(),
        },
    )
}

#[test]
fn minimum_contrast_updates_render_policy_without_recoloring_the_palette() {
    let mut state = settings_state();
    let original = state.terminal_palette();
    for (setting, expected) in [
        ("3", 3.0),
        ("4.5", 4.5),
        ("7", 7.0),
        ("21", 21.0),
        ("1", 1.0),
    ] {
        state.set_minimum_contrast_ratio(setting.into());
        assert_eq!(state.terminal_minimum_contrast_ratio(), expected);
        let palette = state.terminal_palette();
        assert_eq!(palette.terminal_bg, original.terminal_bg);
        assert_eq!(palette.terminal_fg, original.terminal_fg);
        assert_eq!(palette.terminal_fg_intense, original.terminal_fg_intense);
        assert_eq!(palette.terminal_ansi, original.terminal_ansi);
    }
}

#[test]
fn settings_owner_normalizes_general_and_interaction_transitions() {
    let mut state = settings_state();
    state.summary.diagnostics_level = "info".to_string();
    state.summary.diagnostics_retention_days = 7;
    state.summary.interaction_command_suggestion_min_chars = 5;
    state.summary.interaction_command_suggestion_max_chars = 10;
    state.summary.interaction_duplicate_session_command_delay_ms = 100;

    assert!(state.set_diagnostics_level("debug"));
    assert!(!state.set_diagnostics_level("debug"));
    assert!(state.set_diagnostics_level("trace"));
    assert_eq!(state.summary().diagnostics_level, "info");
    assert!(state.set_diagnostics_retention_days(30));
    assert!(state.set_diagnostics_retention_days(99));
    assert_eq!(state.summary().diagnostics_retention_days, 7);

    state.summary.diagnostics_log_shard_bytes = nyaterm_core::diagnostics::DEFAULT_LOG_SHARD_BYTES;
    assert!(state.set_diagnostics_log_shard_bytes(20 * 1024 * 1024));
    assert!(!state.set_diagnostics_log_shard_bytes(20 * 1024 * 1024));
    assert!(state.set_diagnostics_log_shard_bytes(0));
    assert_eq!(
        state.summary().diagnostics_log_shard_bytes,
        nyaterm_core::diagnostics::DEFAULT_LOG_SHARD_BYTES,
        "zero bytes must fall back to the default instead of disabling sharding"
    );
    assert!(state.set_diagnostics_log_shard_bytes(1024));
    assert_eq!(
        state.summary().diagnostics_log_shard_bytes,
        nyaterm_core::diagnostics::MIN_LOG_SHARD_BYTES
    );

    state.set_command_suggestion_min_chars(50);
    assert_eq!(state.summary().interaction_command_suggestion_min_chars, 10);
    state.set_command_suggestion_max_chars(1);
    assert_eq!(state.summary().interaction_command_suggestion_max_chars, 10);
    state.set_duplicate_session_command_delay(75_000);
    assert_eq!(
        state
            .summary()
            .interaction_duplicate_session_command_delay_ms,
        60_000
    );

    let restore_layout = state.summary().startup_restore_window_layout;
    assert_eq!(
        state.toggle_startup_restore_window_layout(),
        !restore_layout
    );
    let suggestions_enabled = state.summary().interaction_command_suggestions_enabled;
    assert_eq!(state.toggle_command_suggestions(), !suggestions_enabled);
    let autofill_enabled = state.summary().interaction_credential_autofill_enabled;
    assert_eq!(state.toggle_credential_autofill(), !autofill_enabled);
}

#[test]
fn appearance_font_options_load_once_and_replace_the_catalog() {
    let mut state = settings_state();
    state.appearance.font_catalog = FontCatalogState::new(Vec::new(), Vec::new());

    let generation = state.begin_font_options_load().expect("font catalog load");
    assert!(state.begin_font_options_load().is_none());
    state.finish_font_options_load(
        generation,
        FontCatalogSnapshot::from_entries(
            generation,
            [
                FontCatalogEntry::new(
                    "Inter".to_string(),
                    FontAvailability::Available {
                        resolved_family: "Inter".into(),
                    },
                    FontAvailability::Unavailable {
                        reason: FontAvailabilityReason::NotMonospaced,
                    },
                ),
                FontCatalogEntry::new(
                    "Noto Sans".to_string(),
                    FontAvailability::Available {
                        resolved_family: "Noto Sans".into(),
                    },
                    FontAvailability::Unavailable {
                        reason: FontAvailabilityReason::NotMonospaced,
                    },
                ),
                FontCatalogEntry::new(
                    "JetBrains Mono".to_string(),
                    FontAvailability::Available {
                        resolved_family: "JetBrains Mono".into(),
                    },
                    FontAvailability::Available {
                        resolved_family: "JetBrains Mono".into(),
                    },
                ),
            ],
        ),
    );

    assert_eq!(
        state.ui_font_options(),
        ["Inter", "JetBrains Mono", "Noto Sans"]
    );
    assert_eq!(state.terminal_font_options(), ["JetBrains Mono"]);
    assert!(state.begin_font_options_load().is_none());
}

#[test]
fn appearance_font_settings_report_only_real_changes() {
    let mut state = settings_state();
    let defaults = state.summary().clone();

    assert!(!state.set_ui_font_family(defaults.ui_font_family.clone()));
    assert!(state.set_ui_font_family("Noto Sans".to_string()));
    assert!(!state.set_ui_font_family("Noto Sans".to_string()));

    assert!(!state.set_ui_font_size(defaults.ui_font_size));
    assert!(state.set_ui_font_size(defaults.ui_font_size + 1));
    assert!(!state.set_ui_font_size(defaults.ui_font_size + 1));

    assert!(!state.set_terminal_font_size(defaults.terminal_font_size));
    assert!(state.set_terminal_font_size(defaults.terminal_font_size + 1));
    assert!(!state.set_terminal_font_size(defaults.terminal_font_size + 1));
}

#[test]
fn settings_owner_clamps_terminal_and_remote_transitions() {
    let mut state = settings_state();
    state.summary.terminal_scrollback_lines = 100;
    state.summary.terminal_keep_alive_interval = 0;
    state.summary.ui_remote_stats_interval = 1;
    state.summary.ui_process_manager_interval = 3;
    state.summary.ui_docker_manager_interval = 120;

    state.set_terminal_scrollback_lines(0);
    state.set_terminal_keep_alive_interval(700);
    state.set_remote_stats_interval(0);
    state.set_process_manager_interval(0);
    state.set_docker_manager_interval(200);

    let summary = state.summary();
    assert_eq!(summary.terminal_scrollback_lines, 100);
    assert_eq!(summary.terminal_keep_alive_interval, 600);
    assert_eq!(summary.ui_remote_stats_interval, 1);
    assert_eq!(summary.ui_process_manager_interval, 3);
    assert_eq!(summary.ui_docker_manager_interval, 120);

    let zebra = state.summary().terminal_zebra_stripes_enabled;
    state.toggle_terminal_zebra_stripes();
    assert_eq!(state.summary().terminal_zebra_stripes_enabled, !zebra);
    assert!(!state.summary().terminal_zebra_stripes_by_row);
    state.toggle_terminal_zebra_stripes_by_row();
    state.toggle_terminal_zebra_stripes();
    assert!(state.summary().terminal_zebra_stripes_by_row);
    state.toggle_terminal_zebra_stripes();
    assert!(state.summary().terminal_zebra_stripes_by_row);
    assert!(state.toggle_terminal_action_link_matcher("ipv4"));
    assert!(state.summary().terminal_action_links_matchers.mac);
    assert!(state.toggle_terminal_action_link_matcher("mac"));
    assert!(!state.summary().terminal_action_links_matchers.mac);
    assert!(!state.toggle_terminal_action_link_matcher("unknown"));
}

#[test]
fn settings_owner_clamps_recording_and_transfer_transitions() {
    let mut state = settings_state();
    state.summary.recording_memory_limit_bytes = 1024 * 1024;
    state.summary.transfer_download_threads = 1;
    state.summary.transfer_upload_threads = 10;
    state.summary.transfer_max_retries = 0;
    state.summary.transfer_buffer_size = 8;

    state.set_recording_memory_limit_mib(0);
    assert_eq!(state.summary().recording_memory_limit_bytes, 1024 * 1024);
    state.set_recording_memory_limit_mib(1_000);
    assert_eq!(
        state.summary().recording_memory_limit_bytes,
        512 * 1024 * 1024
    );

    state.set_transfer_download_threads(0);
    state.set_transfer_upload_threads(11);
    state.set_transfer_max_retries(11);
    state.set_transfer_buffer_size(0);
    let summary = state.summary();
    assert_eq!(summary.transfer_download_threads, 1);
    assert_eq!(summary.transfer_upload_threads, 10);
    assert_eq!(summary.transfer_max_retries, 10);
    assert_eq!(summary.transfer_buffer_size, 8);

    state.set_transfer_duplicate_strategy("rename".to_string());
    state.set_transfer_editor_type("internal");
    state.set_transfer_file_permissions("640");
    let summary = state.summary();
    assert_eq!(summary.transfer_duplicate_strategy, "rename");
    assert_eq!(summary.transfer_editor_type, "internal");
    assert_eq!(summary.transfer_default_file_permissions, "640");

    state.summary.transfer_internal_editor_font_size = 13;
    assert!(state.adjust_transfer_internal_editor_font_size(1));
    assert_eq!(state.summary().transfer_internal_editor_font_size, 14);
    assert!(state.adjust_transfer_internal_editor_font_size(100));
    assert_eq!(state.summary().transfer_internal_editor_font_size, 72);
    assert!(!state.adjust_transfer_internal_editor_font_size(1));
    assert!(state.adjust_transfer_internal_editor_font_size(-100));
    assert_eq!(state.summary().transfer_internal_editor_font_size, 8);
    assert!(!state.set_transfer_internal_editor_font_size(0));
    assert!(state.set_transfer_internal_editor_font_size(u16::MAX));
    assert_eq!(state.summary().transfer_internal_editor_font_size, 72);
    assert!(!state.set_transfer_internal_editor_font_size(72));
    assert!(state.set_transfer_internal_editor_font_size(
        nyaterm_core::models::settings::default_internal_editor_font_size()
    ));
    assert_eq!(state.summary().transfer_internal_editor_font_size, 13);
}

#[test]
fn settings_owner_keeps_search_engine_rows_and_menus_consistent() {
    let mut state = settings_state();
    for name in ["one", "two", "three"] {
        state
            .summary
            .search_custom_engines
            .push(SearchEngineConfig {
                name: name.to_string(),
                url_template: format!("https://{name}.example/?q=%s"),
                icon: None,
                show_in_menu: true,
            });
    }

    assert_eq!(state.toggle_search_engine_expanded(2), Some(false));
    assert!(state.toggle_search_engine_menu(SearchEngineMenu::Icon, 2));
    let interaction = state.search_engine_presentation();
    assert_eq!(interaction.expanded_index, Some(2));
    assert_eq!(interaction.icon_picker_index, Some(2));

    assert!(state.remove_search_engine(0));
    assert_eq!(state.search_engine_presentation().expanded_index, Some(1));
    assert_eq!(state.search_engine_presentation().icon_picker_index, None);
    assert!(state.remove_search_engine(1));
    assert_eq!(state.search_engine_presentation().expanded_index, None);
}

#[test]
fn settings_owner_adds_a_search_engine_open_for_editing() {
    let mut state = settings_state();
    assert_eq!(state.toggle_search_engine_expanded(0), Some(false));
    let before = state.summary().search_custom_engines.len();

    state.add_search_engine(SearchEngineConfig {
        name: "baidu".to_string(),
        url_template: "https://www.baidu.com/s?wd=%s".to_string(),
        icon: Some("baidu".to_string()),
        show_in_menu: true,
    });

    assert_eq!(state.summary().search_custom_engines.len(), before + 1);
    assert_eq!(state.summary().search_custom_engines[0].name, "baidu");
    assert_eq!(state.search_engine_presentation().expanded_index, Some(0));
    assert_eq!(state.search_engine_presentation().icon_picker_index, None);
}

#[test]
fn settings_owner_reconciles_keyword_highlight_edit_lifecycle() {
    let mut state = settings_state();
    state.add_keyword_highlight_rule(KeywordHighlightRule {
        id: "first".to_string(),
        name: "first".to_string(),
        patterns: Vec::new(),
        color_dark: "#ffffff".to_string(),
        color_light: "#000000".to_string(),
        enabled: true,
    });
    state.begin_keyword_highlight_edit("first".to_string(), KeywordHighlightEditorField::Patterns);
    let interaction = state.keyword_highlight_presentation();
    assert_eq!(interaction.expanded_id.as_deref(), Some("first"));
    assert_eq!(interaction.edit_id.as_deref(), Some("first"));
    assert_eq!(
        interaction.edit_field,
        KeywordHighlightEditorField::Patterns
    );

    assert_eq!(
        state.toggle_keyword_highlight_expanded("second".to_string()),
        vec!["first".to_string()]
    );
    let interaction = state.keyword_highlight_presentation();
    assert_eq!(interaction.expanded_id.as_deref(), Some("second"));
    assert_eq!(interaction.edit_id, None);
}

#[test]
fn settings_owner_admits_and_finishes_prompts_by_identity() {
    let mut state = settings_state();
    assert!(state.begin_config_path_prompt(ConfigPathPromptKind::EncryptedPortableExport));
    assert!(!state.begin_config_path_prompt(ConfigPathPromptKind::EncryptedPortableImport));
    assert!(!state.finish_config_path_prompt(ConfigPathPromptKind::EncryptedPortableImport));
    assert!(state.config_path_prompt_active());
    assert!(state.finish_config_path_prompt(ConfigPathPromptKind::EncryptedPortableExport));

    assert!(state.begin_snapshot_password_prompt(SnapshotPasswordPromptKind::Import));
    assert!(!state.begin_snapshot_password_prompt(SnapshotPasswordPromptKind::Export));
    assert!(state.apply_snapshot_password_input("secret".to_string()));
    let prompt = state.take_snapshot_password_prompt().expect("prompt");
    assert_eq!(prompt.kind, SnapshotPasswordPromptKind::Import);
    assert_eq!(prompt.value.expose_secret(), "secret");
    assert!(!state.snapshot_password_prompt_active());
}

#[test]
fn settings_owner_restores_snapshot_prompt_after_empty_password() {
    let mut state = settings_state();
    assert!(state.begin_snapshot_password_prompt(SnapshotPasswordPromptKind::Export));

    let prompt = state.take_snapshot_password_prompt().expect("prompt");
    assert_eq!(prompt.kind, SnapshotPasswordPromptKind::Export);
    assert!(prompt.value.is_empty());
    assert!(!state.snapshot_password_prompt_active());

    state.restore_snapshot_password_prompt(prompt.kind);
    assert!(state.snapshot_password_prompt_active());
    let prompt = state.snapshot_password_prompt().expect("restored prompt");
    assert_eq!(prompt.kind, SnapshotPasswordPromptKind::Export);
    assert!(prompt.value.is_empty());
}

#[test]
fn settings_owner_keeps_keybinding_recording_and_search_atomic() {
    let mut state = settings_state();
    state.begin_keybinding_recording(crate::shortcuts::ShortcutId::TerminalCopy);
    state.set_pending_keybinding(Some(
        crate::shortcuts::ShortcutBinding::parse("ctrl+shift+c").unwrap(),
    ));
    state.set_keybinding_search("copy".to_string());
    let interaction = state.keybinding_presentation();
    assert_eq!(interaction.recording_id.as_deref(), Some("terminal.copy"));
    assert_eq!(interaction.pending_keys.as_deref(), Some("ctrl+shift+c"));
    assert_eq!(interaction.search_draft, "copy");

    state.cancel_keybinding_recording();
    state.clear_keybinding_search();
    let interaction = state.keybinding_presentation();
    assert_eq!(interaction.recording_id, None);
    assert_eq!(interaction.pending_keys, None);
    assert!(interaction.search_draft.is_empty());
}

#[test]
fn settings_owner_controls_store_status_updates_and_replacement() {
    let mut state = settings_state();

    state.update_store_status("saving settings", false);
    let status = state.store_status();
    assert_eq!(status.path, "");
    assert_eq!(status.message, "saving settings");
    assert!(!status.ready);

    state.replace_store_status(
        "/tmp/nyaterm.redb".to_string(),
        "store reopened".to_string(),
        true,
    );
    let status = state.store_status();
    assert_eq!(status.path, "/tmp/nyaterm.redb");
    assert_eq!(status.message, "store reopened");
    assert!(status.ready);
}

#[test]
fn settings_owner_applies_ui_layout_as_one_transition() {
    let mut state = settings_state();

    state.apply_ui_layout(UiLayoutSettingsUpdate {
        left_panel_width: 320,
        right_panel_width: 360,
        transfer_height: 240,
        quick_command_height: 180,
        quick_command_visible: false,
        serial_send_height: 160,
        serial_send_visible: true,
        active_left_panel: Some("connections".to_string()),
        active_right_panel: Some("sftp".to_string()),
        left_panel_collapsed: false,
        right_panel_collapsed: true,
        saved_connections_sort_mode: "recent".to_string(),
        saved_connections_expanded_group_ids: vec!["group-a".to_string()],
        activity_bar_left_top: vec!["connections".to_string()],
        activity_bar_left_bottom: vec!["settings".to_string()],
        activity_bar_right_top: vec!["sftp".to_string()],
        activity_bar_right_bottom: vec!["ai".to_string()],
        activity_bar_show_labels: true,
        activity_bar_hidden_items: vec!["aiAssistant".to_string()],
        panel_multi_open: true,
        panel_open_mode: "floating".to_string(),
        start_workspace_mode: "assets".to_string(),
        asset_sort_key: Some("memory".to_string()),
        asset_sort_direction: Some("desc".to_string()),
        left_open_panels: vec!["connections".to_string()],
        right_open_panels: vec!["sftp".to_string()],
        panel_stack_sizes: HashMap::from([("left:connections".to_string(), 600)]),
    });

    let summary = state.summary();
    assert_eq!(summary.ui_left_panel_width, 320);
    assert_eq!(summary.ui_right_panel_width, 360);
    assert_eq!(
        summary.ui_saved_connections_expanded_group_ids,
        vec!["group-a".to_string()]
    );
    assert!(!summary.ui_quick_cmd_visible);
    assert!(summary.ui_serial_send_visible);
    assert_eq!(summary.ui_active_left_panel.as_deref(), Some("connections"));
    assert!(summary.ui_right_panel_collapsed);
    assert_eq!(summary.ui_activity_bar_right_bottom, ["ai"]);
    assert_eq!(summary.ui_activity_bar_hidden_items, ["aiAssistant"]);
    assert_eq!(summary.ui_panel_open_mode, "floating");
    assert_eq!(summary.ui_start_workspace_mode, "assets");
    assert_eq!(summary.ui_asset_sort_key.as_deref(), Some("memory"));
    assert_eq!(summary.ui_asset_sort_direction.as_deref(), Some("desc"));
    assert!(summary.ui_panel_multi_open);
    assert_eq!(summary.ui_panel_stack_sizes["left:connections"], 600);
}

#[test]
fn settings_owner_controls_keyword_catalog_transitions() {
    let mut state = settings_state();
    state.add_keyword_highlight_rule(KeywordHighlightRule {
        id: "warning".to_string(),
        name: "warning".to_string(),
        patterns: vec!["WARN".to_string()],
        color_dark: "#ffffff".to_string(),
        color_light: "#000000".to_string(),
        enabled: true,
    });

    assert!(state.toggle_keyword_highlights());
    state.toggle_keyword_highlight_builtin("builtin-error".to_string());
    assert!(state.toggle_keyword_highlight_rule("warning"));
    assert!(state.set_keyword_highlight_rule_color("warning", true, "#123456".to_string()));
    assert!(state.apply_keyword_highlight_rule_input(
        "warning",
        KeywordHighlightEditorField::Patterns,
        "WARN\nERROR".to_string(),
    ));

    let config = state.keyword_config();
    assert!(config.enabled);
    assert!(!config.across_wrapped_lines); // Preserve the legacy value while editing rules.
    assert!(!config.builtin_rules["builtin-error"]);
    assert!(!config.rules[0].enabled);
    assert_eq!(config.rules[0].color_dark, "#123456");
    assert_eq!(config.rules[0].patterns, ["WARN", "ERROR"]);
}

#[test]
fn keyword_master_switch_retains_output_and_rule_preferences_while_disabled() {
    for line_roles in [false, true] {
        let mut state = settings_state();
        state.summary.ui_highlight_line_roles = line_roles;
        state.keyword_config.enabled = true;
        state.toggle_keyword_highlight_builtin("builtin-variable".to_string());
        state.toggle_keyword_highlight_builtin("builtin-net-address".to_string());
        let original = state.keyword_config().clone();

        assert!(!state.toggle_keyword_highlights());
        assert_eq!(state.toggle_line_role_highlights(), None);
        assert_eq!(state.summary().ui_highlight_line_roles, line_roles);
        assert_eq!(state.keyword_config().builtin_rules, original.builtin_rules);

        assert!(state.toggle_keyword_highlights());
        assert_eq!(state.keyword_config(), &original);
        assert_eq!(state.summary().ui_highlight_line_roles, line_roles);
        assert_eq!(state.toggle_line_role_highlights(), Some(!line_roles));
    }
}

#[test]
fn settings_owner_exposes_master_password_as_a_borrowed_view() {
    let mut state = settings_state();
    assert_eq!(state.toggle_master_password(false), Ok(true));
    assert!(state.edit_master_password_draft("replacement secret".to_string()));

    let view = state.master_password();
    assert!(view.enabled);
    assert_eq!(view.draft, "replacement secret");

    state.summary.has_master_password = false;
    state.rebase_master_password();
    let view = state.master_password();
    assert!(!view.enabled);
    assert!(view.draft.is_empty());
}

#[test]
fn settings_persistence_coalesces_to_the_latest_snapshot() {
    let mut state = settings_state();
    let (first_generation, first) = state
        .queue_persistence(SettingsPersistenceDomain::General)
        .expect("first request");
    assert_eq!(first_generation, 1);
    assert_eq!(first.language, state.summary().language);

    state.set_language("zh-CN");
    assert!(
        state
            .queue_persistence(SettingsPersistenceDomain::General)
            .is_none()
    );
    state.set_language("en-US");
    assert!(
        state
            .queue_persistence(SettingsPersistenceDomain::General)
            .is_none()
    );

    let completion =
        state.finish_persistence(SettingsPersistenceDomain::General, first_generation, true);
    assert!(!completion.apply_result);
    assert!(!completion.report_result);
    let (latest_generation, latest) = completion.next.expect("latest request");
    assert_eq!(latest_generation, 3);
    assert_eq!(latest.language, "en-US");
}

#[test]
fn failed_latest_settings_persistence_stays_dirty() {
    let mut state = settings_state();
    let (generation, _) = state
        .queue_persistence(SettingsPersistenceDomain::Interaction)
        .expect("request");
    let completion =
        state.finish_persistence(SettingsPersistenceDomain::Interaction, generation, false);
    assert!(completion.report_result);
    assert!(!completion.apply_result);
    assert!(
        state
            .dirty_persistence_domains()
            .contains(&SettingsPersistenceDomain::Interaction)
    );
}

#[test]
fn shutdown_snapshot_lists_only_dirty_settings_domains() {
    let mut state = settings_state();
    state.queue_persistence(SettingsPersistenceDomain::Transfer);
    state.queue_persistence(SettingsPersistenceDomain::General);

    assert_eq!(
        state.dirty_persistence_domains(),
        [
            SettingsPersistenceDomain::General,
            SettingsPersistenceDomain::Transfer,
        ]
    );
}
#[test]
fn quit_prompt_retains_one_request_and_can_be_retried_after_cancel() {
    let mut state = settings_state();
    assert!(state.begin_quit_confirmation(3));
    assert!(!state.begin_quit_confirmation(9));
    assert_eq!(state.take_quit_confirmation(), Some(3));
    assert_eq!(state.take_quit_confirmation(), None);
    assert!(state.begin_quit_confirmation(0));
    assert_eq!(state.take_quit_confirmation(), Some(0));
}
