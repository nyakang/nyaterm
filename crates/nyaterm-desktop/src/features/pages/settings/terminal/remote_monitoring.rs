use gpui::{Context, IntoElement, SharedString, div, prelude::*};
use nyaterm_ui::NyaButton;
use rust_i18n::t;

use super::super::{settings_form_row, settings_form_section, settings_switch};
use crate::features::pages::settings::panel::SettingsPanel;

impl SettingsPanel {
    pub(in crate::features) fn remote_monitoring_settings_section(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
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
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(settings_form_section(
                palette,
                Some(t!("settings.remoteMonitoring")),
                Some(t!("settings.remoteMonitoringDesc")),
                rows,
            ))
            .child(settings_form_section(
                palette,
                None,
                Some(t!("settings.remoteMonitoringIconDesc")),
                NyaButton::new(
                    "remote-monitoring-reset",
                    t!("settings.remoteMonitoringReset"),
                )
                .on_click(cx.listener(|this, _, _, cx| this.reset_remote_monitoring_intervals(cx))),
            ))
    }
}
