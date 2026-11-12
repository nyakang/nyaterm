//! settings values state and transitions.

use super::persistence::SettingsPersistenceDomain;
use super::{SettingsFeatureState, UiLayoutSettingsUpdate};
use nyaterm_core::{
    ExistingFileBehavior, RecordingMode, RecordingRotationPolicy, TransferBrowserViewMode,
};

impl SettingsFeatureState {
    pub(in crate::features) fn mark_draft_domain_dirty(
        &mut self,
        domain: SettingsPersistenceDomain,
    ) {
        self.draft_dirty_domains.insert(domain);
    }
    pub(in crate::features) fn set_language(&mut self, language: &str) {
        self.summary.language = language.to_string();
        crate::i18n::apply_locale(&self.summary.language);
    }
    pub(in crate::features) fn toggle_startup_restore(&mut self) {
        self.summary.startup_restore = !self.summary.startup_restore;
    }
    pub(in crate::features) fn toggle_startup_restore_window_layout(&mut self) -> bool {
        self.summary.startup_restore_window_layout = !self.summary.startup_restore_window_layout;
        self.summary.startup_restore_window_layout
    }
    pub(in crate::features) fn toggle_confirm_on_close(&mut self) {
        self.summary.confirm_on_close = !self.summary.confirm_on_close;
    }
    pub(in crate::features) fn toggle_minimize_to_tray(&mut self) {
        self.summary.minimize_to_tray = !self.summary.minimize_to_tray;
    }
    pub(in crate::features) fn set_diagnostics_level(&mut self, level: &str) -> bool {
        let level = match level {
            "warn" | "debug" => level,
            _ => "info",
        };
        if self.summary.diagnostics_level == level {
            return false;
        }
        self.summary.diagnostics_level = level.to_string();
        true
    }
    pub(in crate::features) fn set_diagnostics_retention_days(&mut self, days: u32) -> bool {
        let days = match days {
            3 | 7 | 14 | 30 => days,
            _ => 7,
        };
        if self.summary.diagnostics_retention_days == days {
            return false;
        }
        self.summary.diagnostics_retention_days = days;
        true
    }
    pub(in crate::features) fn set_diagnostics_log_shard_bytes(&mut self, bytes: u64) -> bool {
        let bytes = nyaterm_core::diagnostics::normalize_log_shard_bytes(bytes);
        if self.summary.diagnostics_log_shard_bytes == bytes {
            return false;
        }
        self.summary.diagnostics_log_shard_bytes = bytes;
        true
    }
    pub(in crate::features) fn toggle_interaction_copy_on_select(&mut self) {
        self.summary.interaction_copy_on_select = !self.summary.interaction_copy_on_select;
    }
    pub(in crate::features) fn toggle_osc52_clipboard_write(&mut self) {
        self.summary.interaction_allow_osc52_clipboard_write =
            !self.summary.interaction_allow_osc52_clipboard_write;
    }
    pub(in crate::features) fn set_terminal_right_click_action(
        &mut self,
        action: nyaterm_core::TerminalRightClickAction,
    ) {
        self.summary.interaction_terminal_right_click_action = action;
        self.summary.interaction_right_click_paste =
            action == nyaterm_core::TerminalRightClickAction::Paste;
    }
    pub(in crate::features) fn toggle_mouse_events_require_alt(&mut self) {
        self.summary.interaction_mouse_events_require_alt =
            !self.summary.interaction_mouse_events_require_alt;
    }
    pub(in crate::features) fn toggle_terminal_zoom_enabled(&mut self) {
        self.summary.interaction_terminal_zoom_enabled =
            !self.summary.interaction_terminal_zoom_enabled;
    }
    pub(in crate::features) fn toggle_command_suggestions(&mut self) -> bool {
        self.summary.interaction_command_suggestions_enabled =
            !self.summary.interaction_command_suggestions_enabled;
        self.summary.interaction_command_suggestions_enabled
    }
    pub(in crate::features) fn toggle_credential_autofill(&mut self) -> bool {
        self.summary.interaction_credential_autofill_enabled =
            !self.summary.interaction_credential_autofill_enabled;
        self.summary.interaction_credential_autofill_enabled
    }
    pub(in crate::features) fn set_command_suggestion_min_chars(&mut self, value: u32) {
        let max_chars = self.summary.interaction_command_suggestion_max_chars;
        self.summary.interaction_command_suggestion_min_chars = value.clamp(1, max_chars);
    }
    pub(in crate::features) fn set_command_suggestion_max_chars(&mut self, value: u32) {
        let min_chars = self.summary.interaction_command_suggestion_min_chars;
        self.summary.interaction_command_suggestion_max_chars = value.clamp(min_chars, 500);
    }
    pub(in crate::features) fn set_duplicate_session_command_delay(&mut self, value_ms: u32) {
        self.summary.interaction_duplicate_session_command_delay_ms = value_ms.clamp(0, 60_000);
    }
    pub(in crate::features) fn toggle_alt_as_meta(&mut self) {
        self.summary.interaction_alt_as_meta = !self.summary.interaction_alt_as_meta;
    }
    pub(in crate::features) fn set_interaction_encoding(&mut self, encoding: &str) {
        self.summary.interaction_default_encoding = encoding.to_string();
    }
    pub(in crate::features) fn set_interaction_word_separators(&mut self, text: String) {
        self.summary.interaction_word_separators = text;
    }
    pub(in crate::features) fn toggle_startup_lock_enabled(&mut self) {
        self.summary.enable_startup_lock = !self.summary.enable_startup_lock;
        self.summary.enable_screen_lock =
            self.summary.enable_startup_lock || self.summary.enable_idle_lock;
    }
    pub(in crate::features) fn toggle_idle_lock_enabled(&mut self) {
        self.summary.enable_idle_lock = !self.summary.enable_idle_lock;
        self.summary.enable_screen_lock =
            self.summary.enable_startup_lock || self.summary.enable_idle_lock;
    }
    pub(in crate::features) fn set_idle_lock_minutes(&mut self, value: u32) {
        self.summary.idle_lock_minutes = value.clamp(0, 1440);
    }
    pub(in crate::features) fn set_terminal_x11_display(&mut self, text: String) {
        self.summary.x11_display = text;
    }
    pub(in crate::features) fn set_android_adb_path(&mut self, text: String) {
        self.summary.android_adb_path = text;
    }
    pub(in crate::features) fn toggle_terminal_zebra_stripes(&mut self) {
        self.summary.terminal_zebra_stripes_enabled = !self.summary.terminal_zebra_stripes_enabled;
    }
    pub(in crate::features) fn toggle_terminal_zebra_stripes_by_row(&mut self) {
        self.summary.terminal_zebra_stripes_by_row = !self.summary.terminal_zebra_stripes_by_row;
    }
    pub(in crate::features) fn set_terminal_scrollback_lines(&mut self, value: u32) {
        self.summary.terminal_scrollback_lines = value.clamp(100, 100_000);
    }
    pub(in crate::features) fn set_terminal_keep_alive_interval(&mut self, value: u32) {
        self.summary.terminal_keep_alive_interval = value.clamp(0, 600);
    }
    pub(in crate::features) fn set_terminal_keep_alive_mode(&mut self, mode: &str) {
        self.summary.terminal_keep_alive_mode = match mode {
            "strict" | "disabled" => mode.to_string(),
            _ => "compatible".to_string(),
        };
    }
    pub(in crate::features) fn set_terminal_timestamp_format(&mut self, text: String) {
        let trimmed = text.trim();
        self.summary.terminal_timestamp_format = if trimmed.is_empty() {
            nyaterm_core::DEFAULT_TERMINAL_TIMESTAMP_FORMAT.to_string()
        } else {
            trimmed.chars().take(64).collect()
        };
    }
    pub(in crate::features) fn toggle_terminal_workspace_padding(&mut self) {
        self.summary.terminal_show_workspace_padding =
            !self.summary.terminal_show_workspace_padding;
    }
    pub(in crate::features) fn toggle_terminal_line_numbers(&mut self) {
        self.summary.terminal_show_line_numbers = !self.summary.terminal_show_line_numbers;
    }
    pub(in crate::features) fn set_terminal_wrap_mode(
        &mut self,
        mode: nyaterm_core::models::settings::TerminalWrapMode,
    ) {
        self.summary.terminal_wrap_mode = mode;
    }
    pub(in crate::features) fn toggle_terminal_wrap_markers(&mut self) {
        self.summary.terminal_show_wrap_markers = !self.summary.terminal_show_wrap_markers;
    }
    pub(in crate::features) fn toggle_terminal_timestamps(&mut self) {
        self.summary.terminal_show_timestamps = !self.summary.terminal_show_timestamps;
    }
    pub(in crate::features) fn toggle_terminal_reconnect_restore_cwd(&mut self) {
        self.summary.terminal_reconnect_restore_cwd = !self.summary.terminal_reconnect_restore_cwd;
    }
    pub(in crate::features) fn toggle_multi_line_paste_dialog(&mut self) {
        self.summary.terminal_show_multi_line_paste_dialog =
            !self.summary.terminal_show_multi_line_paste_dialog;
    }
    pub(in crate::features) fn toggle_paste_image_as_path(&mut self) {
        self.summary.terminal_paste_image_as_path = !self.summary.terminal_paste_image_as_path;
    }
    pub(in crate::features) fn toggle_remote_stats_show_users(&mut self) {
        self.summary.ui_remote_stats_show_users = !self.summary.ui_remote_stats_show_users;
    }
    pub(in crate::features) fn set_remote_stats_interval(&mut self, value: u32) {
        self.summary.ui_remote_stats_interval = value.clamp(1, 60);
    }
    pub(in crate::features) fn set_gpu_monitor_interval(&mut self, value: u32) {
        self.summary.ui_gpu_monitor_interval = value.clamp(3, 120);
    }
    pub(in crate::features) fn set_ascend_npu_monitor_interval(&mut self, value: u32) {
        self.summary.ui_ascend_npu_monitor_interval = value.clamp(3, 120);
    }
    pub(in crate::features) fn set_process_manager_interval(&mut self, value: u32) {
        self.summary.ui_process_manager_interval = value.clamp(3, 120);
    }
    pub(in crate::features) fn set_docker_manager_interval(&mut self, value: u32) {
        self.summary.ui_docker_manager_interval = value.clamp(3, 120);
    }
    pub(in crate::features) fn toggle_terminal_action_links(&mut self) {
        self.summary.terminal_action_links_enabled = !self.summary.terminal_action_links_enabled;
    }
    pub(in crate::features) fn toggle_terminal_action_link_matcher(&mut self, which: &str) -> bool {
        let matcher = &mut self.summary.terminal_action_links_matchers;
        match which {
            "mac" => matcher.mac = !matcher.mac,
            "ipv4" => matcher.ipv4 = !matcher.ipv4,
            "archive" => matcher.archive = !matcher.archive,
            "host_port" => matcher.host_port = !matcher.host_port,
            _ => return false,
        }
        true
    }
    pub(in crate::features) fn set_host_key_policy(&mut self, policy: &str) {
        self.summary.host_key_policy = policy.to_string();
    }
    pub(in crate::features) fn toggle_recording_auto_start(&mut self) {
        self.summary.recording_auto_start = !self.summary.recording_auto_start;
    }
    pub(in crate::features) fn set_recording_default_mode(&mut self, mode: RecordingMode) {
        self.summary.recording_default_mode = mode;
    }
    pub(in crate::features) fn set_recording_path_template(&mut self, template: String) {
        let trimmed = template.trim();
        self.summary.recording_path_template = if trimmed.is_empty() {
            nyaterm_core::DEFAULT_RECORDING_PATH_TEMPLATE.to_string()
        } else {
            trimmed.to_string()
        };
    }
    pub(in crate::features) fn toggle_recording_input(&mut self) {
        self.summary.recording_include_input = !self.summary.recording_include_input;
    }
    pub(in crate::features) fn toggle_recording_io_labels(&mut self) {
        self.summary.recording_include_io_labels = !self.summary.recording_include_io_labels;
    }
    pub(in crate::features) fn toggle_recording_timestamps(&mut self) {
        self.summary.recording_include_timestamps = !self.summary.recording_include_timestamps;
    }
    pub(in crate::features) fn toggle_recording_session_metadata(&mut self) {
        self.summary.recording_include_session_metadata =
            !self.summary.recording_include_session_metadata;
    }
    pub(in crate::features) fn set_recording_rotation(
        &mut self,
        rotation: RecordingRotationPolicy,
    ) {
        self.summary.recording_rotation = rotation;
    }
    pub(in crate::features) fn set_recording_rotation_size_mib(&mut self, value_mib: u64) {
        self.summary.recording_rotation = RecordingRotationPolicy::Size {
            max_bytes: value_mib.clamp(1, 102_400) * 1024 * 1024,
        };
    }
    pub(in crate::features) fn set_recording_existing_file_behavior(
        &mut self,
        behavior: ExistingFileBehavior,
    ) {
        self.summary.recording_existing_file_behavior = behavior;
    }
    pub(in crate::features) fn toggle_recording_binary_transfer_payloads(&mut self) {
        self.summary.recording_include_binary_transfer_payloads =
            !self.summary.recording_include_binary_transfer_payloads;
    }
    pub(in crate::features) fn set_recording_memory_limit_mib(&mut self, value_mib: u64) {
        self.summary.recording_memory_limit_bytes = value_mib.clamp(1, 512) * 1024 * 1024;
    }
    pub(in crate::features) fn set_transfer_duplicate_strategy(&mut self, strategy: String) {
        self.summary.transfer_duplicate_strategy = strategy;
    }
    pub(in crate::features) fn set_transfer_editor_type(&mut self, editor_type: &str) {
        self.summary.transfer_editor_type = editor_type.to_string();
    }
    pub(in crate::features) fn set_internal_editor_display(
        &mut self,
        display: nyaterm_core::InternalEditorDisplay,
    ) {
        self.summary.transfer_internal_editor_display = display;
    }
    pub(in crate::features) fn toggle_transfer_ask_save_location(&mut self) {
        self.summary.transfer_ask_save_location = !self.summary.transfer_ask_save_location;
    }
    pub(in crate::features) fn toggle_transfer_preserve_timestamps(&mut self) {
        self.summary.transfer_preserve_timestamps = !self.summary.transfer_preserve_timestamps;
    }
    pub(in crate::features) fn toggle_transfer_resume_broken(&mut self) {
        self.summary.transfer_resume_broken_transfer =
            !self.summary.transfer_resume_broken_transfer;
    }
    pub(in crate::features) fn set_transfer_download_threads(&mut self, value: u32) {
        self.summary.transfer_download_threads = value.clamp(1, 10);
    }
    pub(in crate::features) fn set_transfer_upload_threads(&mut self, value: u32) {
        self.summary.transfer_upload_threads = value.clamp(1, 10);
    }
    pub(in crate::features) fn set_transfer_max_retries(&mut self, value: u32) {
        self.summary.transfer_max_retries = value.clamp(0, 10);
    }
    pub(in crate::features) fn set_transfer_buffer_size(&mut self, value: u32) {
        self.summary.transfer_buffer_size = value.clamp(8, 256);
    }
    pub(in crate::features) fn set_transfer_file_permissions(&mut self, permissions: &str) {
        self.summary.transfer_default_file_permissions = permissions.to_string();
    }
    pub(in crate::features) fn set_quick_command_view_mode(&mut self, mode: String) {
        self.summary.ui_quick_cmd_view_mode = mode;
    }
    pub(in crate::features) fn set_quick_command_sort_mode(&mut self, mode: String) {
        self.summary.ui_quick_cmd_sort_mode = mode;
    }
    pub(in crate::features) fn set_quick_command_selected_category(&mut self, category: String) {
        self.summary.ui_quick_cmd_selected_category = category;
    }
    pub(in crate::features) fn set_saved_connections_sort_mode(&mut self, mode: String) {
        self.summary.ui_saved_connections_sort_mode = mode;
    }
    pub(in crate::features) fn set_header_status_mode(&mut self, mode: String) {
        self.summary.ui_header_status_mode = mode;
        self.summary.ui_header_status_visible = true;
    }
    pub(in crate::features) fn set_header_status_visible(&mut self, visible: bool) {
        self.summary.ui_header_status_visible = visible;
    }
    pub(in crate::features) fn set_transfer_download_path(&mut self, path: String) {
        self.summary.transfer_download_path = path;
    }
    pub(in crate::features) fn set_file_manager_ui(
        &mut self,
        settings: nyaterm_core::FileManagerUiSettings,
    ) {
        self.summary.ui_file_manager = settings;
    }
    pub(in crate::features) fn toggle_file_manager_auto_collapse(&mut self) {
        self.summary.ui_file_manager.auto_collapse_sidebar =
            !self.summary.ui_file_manager.auto_collapse_sidebar;
    }
    pub(in crate::features) fn set_recording_path(&mut self, path: String) {
        self.summary.recording_path = path;
    }
    pub(in crate::features) fn set_transfer_default_editor(&mut self, path: String) {
        self.summary.transfer_default_editor = path;
    }
    pub(in crate::features) fn set_tab_double_click_action(&mut self, action: String) {
        self.summary.interaction_tab_double_click_action = action;
    }
    pub(in crate::features) fn set_tab_middle_click_action(&mut self, action: String) {
        self.summary.interaction_tab_middle_click_action = action;
    }
    pub(in crate::features) fn set_tab_right_click_action(&mut self, action: String) {
        self.summary.interaction_tab_right_click_action = action;
    }
    pub(in crate::features) fn set_send_command_clear_after_send(&mut self, enabled: bool) {
        self.summary.ui_serial_send_clear_after_send = enabled;
    }
    pub(in crate::features) fn apply_ui_layout(&mut self, update: UiLayoutSettingsUpdate) {
        self.summary.ui_left_panel_width = update.left_panel_width;
        self.summary.ui_right_panel_width = update.right_panel_width;
        self.summary.ui_transfer_height = update.transfer_height;
        self.summary.ui_quick_cmd_height = update.quick_command_height;
        self.summary.ui_quick_cmd_visible = update.quick_command_visible;
        self.summary.ui_serial_send_height = update.serial_send_height;
        self.summary.ui_serial_send_visible = update.serial_send_visible;
        self.summary.ui_active_left_panel = update.active_left_panel;
        self.summary.ui_active_right_panel = update.active_right_panel;
        self.summary.ui_left_panel_collapsed = update.left_panel_collapsed;
        self.summary.ui_right_panel_collapsed = update.right_panel_collapsed;
        self.summary.ui_saved_connections_sort_mode = update.saved_connections_sort_mode;
        self.summary.ui_saved_connections_expanded_group_ids =
            update.saved_connections_expanded_group_ids;
        self.summary.ui_start_workspace_mode = update.start_workspace_mode;
        self.summary.ui_asset_sort_key = update.asset_sort_key;
        self.summary.ui_asset_sort_direction = update.asset_sort_direction;
        self.summary.ui_activity_bar_left_top = update.activity_bar_left_top;
        self.summary.ui_activity_bar_left_bottom = update.activity_bar_left_bottom;
        self.summary.ui_activity_bar_right_top = update.activity_bar_right_top;
        self.summary.ui_activity_bar_right_bottom = update.activity_bar_right_bottom;
        self.summary.ui_activity_bar_show_labels = update.activity_bar_show_labels;
        self.summary.ui_activity_bar_hidden_items = update.activity_bar_hidden_items;
        self.summary.ui_panel_multi_open = update.panel_multi_open;
        self.summary.ui_panel_open_mode =
            nyaterm_core::normalize_panel_open_mode(&update.panel_open_mode);
        self.summary.ui_left_open_panels = update.left_open_panels;
        self.summary.ui_right_open_panels = update.right_open_panels;
        self.summary.ui_panel_stack_sizes = update.panel_stack_sizes;
    }
    pub(in crate::features) fn toggle_file_explorer_auto_sync_cwd(
        &mut self,
        connection_id: String,
    ) -> bool {
        let ids = &mut self.summary.ui_file_explorer_auto_sync_cwd_connection_ids;
        let was_enabled = ids.iter().any(|id| id == &connection_id);
        ids.retain(|id| id != &connection_id);
        if !was_enabled {
            ids.push(connection_id);
        }
        !was_enabled
    }
    pub(in crate::features) fn toggle_file_explorer_hidden_files(&mut self) -> bool {
        self.summary.ui_file_explorer_show_hidden_files =
            !self.summary.ui_file_explorer_show_hidden_files;
        self.summary.ui_file_explorer_show_hidden_files
    }
    pub(in crate::features) fn toggle_file_explorer_view_mode(
        &mut self,
    ) -> TransferBrowserViewMode {
        self.summary.ui_file_explorer_view_mode = match self.summary.ui_file_explorer_view_mode {
            TransferBrowserViewMode::List => TransferBrowserViewMode::Tree,
            TransferBrowserViewMode::Tree => TransferBrowserViewMode::List,
        };
        self.summary.ui_file_explorer_view_mode
    }
    pub(in crate::features) fn set_file_explorer_favorites(
        &mut self,
        connection_id: String,
        favorites: Vec<String>,
    ) {
        if favorites.is_empty() {
            self.summary
                .ui_file_explorer_favorite_dirs_by_connection_id
                .remove(&connection_id);
        } else {
            self.summary
                .ui_file_explorer_favorite_dirs_by_connection_id
                .insert(connection_id, favorites);
        }
    }
}
