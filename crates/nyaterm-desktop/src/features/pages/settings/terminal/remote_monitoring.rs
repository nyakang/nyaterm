use gpui::{Context, IntoElement, SharedString, div, prelude::*};
use nyaterm_core::AppSettingsSummary;
use nyaterm_ui::{NyaButton, NyaButtonVariant};
use rust_i18n::t;

use super::super::{settings_form_row, settings_form_section_with_action, settings_switch};
use crate::features::pages::settings::panel::SettingsPanel;

fn remote_monitoring_is_default(summary: &AppSettingsSummary) -> bool {
    let defaults = AppSettingsSummary::default();
    summary.ui_remote_stats_interval == defaults.ui_remote_stats_interval
        && summary.ui_gpu_monitor_interval == defaults.ui_gpu_monitor_interval
        && summary.ui_ascend_npu_monitor_interval == defaults.ui_ascend_npu_monitor_interval
        && summary.ui_process_manager_interval == defaults.ui_process_manager_interval
        && summary.ui_docker_manager_interval == defaults.ui_docker_manager_interval
        && summary.ui_remote_stats_show_users == defaults.ui_remote_stats_show_users
}

impl SettingsPanel {
    pub(in crate::features) fn remote_monitoring_settings_section(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let is_default = remote_monitoring_is_default(self.settings.summary());
        let reset_action = div()
            .debug_selector(|| "remote-monitoring-reset".to_string())
            .child(
                NyaButton::new(
                    "remote-monitoring-reset",
                    t!("settings.remoteMonitoringReset"),
                )
                .small()
                .variant(NyaButtonVariant::Ghost)
                .disabled(is_default)
                .on_click(cx.listener(|this, _, _, cx| this.reset_remote_monitoring(cx))),
            )
            .into_any_element();

        let mut rows = div().flex().flex_col().gap_3();
        for (label, description, input) in [
            (
                "settings.remoteStatsInterval",
                "settings.remoteStatsIntervalDesc",
                "settings.number.remote-stats-interval",
            ),
            (
                "settings.gpuMonitorInterval",
                "settings.gpuMonitorIntervalDesc",
                "settings.number.gpu-monitor-interval",
            ),
            (
                "settings.ascendNpuMonitorInterval",
                "settings.ascendNpuMonitorIntervalDesc",
                "settings.number.ascend-npu-monitor-interval",
            ),
            (
                "settings.processManagerInterval",
                "settings.processManagerIntervalDesc",
                "settings.number.process-manager-interval",
            ),
            (
                "settings.dockerManagerInterval",
                "settings.dockerManagerIntervalDesc",
                "settings.number.docker-manager-interval",
            ),
        ] {
            rows = rows.child(settings_form_row(
                palette,
                t!(label),
                Some(SharedString::from(t!(description))),
                self.existing_number_input_box(input),
            ));
        }
        rows = rows.child(settings_form_row(
            palette,
            t!("settings.remoteStatsShowUsers"),
            Some(SharedString::from(t!("settings.remoteStatsShowUsersDesc"))),
            settings_switch(
                palette,
                "remote-stats-show-users",
                self.settings.summary().ui_remote_stats_show_users,
                cx.listener(|this, _, _, cx| this.toggle_remote_stats_show_users(cx)),
            ),
        ));
        settings_form_section_with_action(
            palette,
            Some(t!("settings.remoteMonitoring")),
            Some(t!("settings.remoteMonitoringDesc")),
            Some(reset_action),
            rows,
        )
    }
}

#[cfg(test)]
mod tests {
    use nyaterm_core::AppSettingsSummary;

    use super::remote_monitoring_is_default;

    #[test]
    fn remote_monitoring_default_check_tracks_intervals_and_user_visibility() {
        let mut summary = AppSettingsSummary::default();
        assert!(remote_monitoring_is_default(&summary));

        summary.ui_remote_stats_show_users = true;
        assert!(!remote_monitoring_is_default(&summary));

        summary.ui_remote_stats_show_users = false;
        summary.ui_docker_manager_interval = 15;
        assert!(!remote_monitoring_is_default(&summary));
    }
}
