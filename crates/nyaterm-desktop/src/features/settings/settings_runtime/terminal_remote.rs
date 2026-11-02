use gpui::Context;

use crate::features::NyaTermApp;

use super::general_interaction::SettingsSaveKind;

impl NyaTermApp {
    /// Apply an edit from the X11 display box.
    pub(in crate::features) fn apply_terminal_x11_display(
        &mut self,
        text: String,
        cx: &mut Context<Self>,
    ) {
        self.settings.set_terminal_x11_display(text);
        cx.notify();
    }

    pub(in crate::features) fn toggle_terminal_zebra_stripes(&mut self, cx: &mut Context<Self>) {
        self.settings.toggle_terminal_zebra_stripes();
        self.save_terminal_settings(cx);
        let session_ids = self
            .visible_terminal_session_ids()
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        for session_id in session_ids {
            self.notify_terminal_surface_only(Some(session_id.as_str()), cx);
        }
    }

    pub(in crate::features) fn set_terminal_scrollback_lines(
        &mut self,
        value: u32,
        cx: &mut Context<Self>,
    ) {
        self.settings.set_terminal_scrollback_lines(value);
        if !self.shell.has_settings_draft() {
            self.enforce_terminal_scrollback_limit();
        }
        self.save_terminal_settings(cx);
    }

    pub(in crate::features) fn set_terminal_keep_alive_interval(
        &mut self,
        value: u32,
        cx: &mut Context<Self>,
    ) {
        self.settings.set_terminal_keep_alive_interval(value);
        self.save_terminal_settings(cx);
    }

    pub(in crate::features) fn set_terminal_keep_alive_mode(
        &mut self,
        mode: &'static str,
        cx: &mut Context<Self>,
    ) {
        self.settings.set_terminal_keep_alive_mode(mode);
        self.save_terminal_settings(cx);
    }

    pub(in crate::features) fn apply_terminal_timestamp_format(
        &mut self,
        text: String,
        cx: &mut Context<Self>,
    ) {
        self.settings.set_terminal_timestamp_format(text);
        self.save_terminal_settings(cx);
    }

    pub(in crate::features) fn toggle_terminal_workspace_padding(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        self.settings.toggle_terminal_workspace_padding();
        self.save_terminal_settings(cx);
    }

    pub(in crate::features) fn toggle_terminal_line_numbers(&mut self, cx: &mut Context<Self>) {
        self.settings.toggle_terminal_line_numbers();
        self.save_terminal_settings(cx);
    }

    pub(in crate::features) fn toggle_terminal_timestamps(&mut self, cx: &mut Context<Self>) {
        self.settings.toggle_terminal_timestamps();
        self.save_terminal_settings(cx);
    }

    pub(in crate::features) fn toggle_terminal_reconnect_restore_cwd(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        self.settings.toggle_terminal_reconnect_restore_cwd();
        self.save_terminal_settings(cx);
    }

    pub(in crate::features) fn toggle_remote_stats_show_users(&mut self, cx: &mut Context<Self>) {
        self.settings.toggle_remote_stats_show_users();
        self.remote_stats_users_setting_changed(cx);
        self.save_terminal_settings(cx);
    }

    pub(in crate::features) fn toggle_multi_line_paste_dialog(&mut self, cx: &mut Context<Self>) {
        self.settings.toggle_multi_line_paste_dialog();
        self.save_terminal_settings(cx);
    }

    pub(in crate::features) fn toggle_paste_image_as_path(&mut self, cx: &mut Context<Self>) {
        self.settings.toggle_paste_image_as_path();
        self.save_terminal_settings(cx);
    }

    pub(in crate::features) fn set_remote_stats_interval(
        &mut self,
        value: u32,
        cx: &mut Context<Self>,
    ) {
        self.settings.set_remote_stats_interval(value);
        self.save_terminal_settings(cx);
    }

    pub(in crate::features) fn set_gpu_monitor_interval(
        &mut self,
        value: u32,
        cx: &mut Context<Self>,
    ) {
        self.settings.set_gpu_monitor_interval(value);
        self.save_terminal_settings(cx);
    }

    pub(in crate::features) fn set_ascend_npu_monitor_interval(
        &mut self,
        value: u32,
        cx: &mut Context<Self>,
    ) {
        self.settings.set_ascend_npu_monitor_interval(value);
        self.save_terminal_settings(cx);
    }

    pub(in crate::features) fn set_process_manager_interval(
        &mut self,
        value: u32,
        cx: &mut Context<Self>,
    ) {
        self.settings.set_process_manager_interval(value);
        self.save_terminal_settings(cx);
    }

    pub(in crate::features) fn set_docker_manager_interval(
        &mut self,
        value: u32,
        cx: &mut Context<Self>,
    ) {
        self.settings.set_docker_manager_interval(value);
        self.save_terminal_settings(cx);
    }

    pub(in crate::features) fn reset_remote_monitoring_intervals(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        let defaults = nyaterm_core::AppSettingsSummary::default();
        self.settings
            .set_remote_stats_interval(defaults.ui_remote_stats_interval);
        self.settings
            .set_gpu_monitor_interval(defaults.ui_gpu_monitor_interval);
        self.settings
            .set_ascend_npu_monitor_interval(defaults.ui_ascend_npu_monitor_interval);
        self.settings
            .set_process_manager_interval(defaults.ui_process_manager_interval);
        self.settings
            .set_docker_manager_interval(defaults.ui_docker_manager_interval);
        self.sync_remote_monitoring_inputs(cx);
        self.save_terminal_settings(cx);
    }

    pub(in crate::features) fn sync_remote_monitoring_inputs(&mut self, cx: &mut Context<Self>) {
        let settings = self.settings.summary();
        let inputs = self.number_input_fields_snapshot();
        for (id, value) in [
            (
                "settings.number.remote-stats-interval",
                settings.ui_remote_stats_interval,
            ),
            (
                "settings.number.gpu-monitor-interval",
                settings.ui_gpu_monitor_interval,
            ),
            (
                "settings.number.ascend-npu-monitor-interval",
                settings.ui_ascend_npu_monitor_interval,
            ),
            (
                "settings.number.process-manager-interval",
                settings.ui_process_manager_interval,
            ),
            (
                "settings.number.docker-manager-interval",
                settings.ui_docker_manager_interval,
            ),
        ] {
            if let Some(input) = inputs.get(id) {
                input.update(cx, |input, cx| {
                    input.set_content_silent(&value.to_string(), cx)
                });
            }
        }
    }

    pub(in crate::features) fn save_terminal_settings(&mut self, cx: &mut Context<Self>) {
        if self.defer_settings_domain_persistence(
            crate::features::settings::SettingsPersistenceDomain::Terminal,
            cx,
        ) {
            return;
        }
        self.enforce_terminal_scrollback_limit();
        self.queue_settings_save(SettingsSaveKind::Terminal, cx);
    }
}
