use rust_i18n::t;

use std::borrow::Cow;

use gpui::{
    App, ClickEvent, Context, FontWeight, IntoElement, SharedString, Window, div, prelude::*, px,
    rgb,
};
use nyaterm_ui::NyaSelectOption;

use crate::features::pages::settings::panel::SettingsPanel;
use crate::theme::ThemePalette;

use super::super::{
    settings_form_row, settings_form_section, settings_switch, settings_switch_with_enabled,
};

impl SettingsPanel {
    pub(in crate::features) fn terminal_general_settings_section(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let x11_display_input = self
            .existing_text_input_box("settings.terminal.x11-display", false)
            .into_any_element();
        let timestamp_format_input = self
            .existing_text_input_box("settings.terminal.timestamp-format", false)
            .into_any_element();
        let action_links_enabled = self.settings.summary().terminal_action_links_enabled;

        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(settings_form_section(
                palette,
                None,
                None,
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(settings_form_row(
                        palette,
                        t!("settings.scrollbackLines"),
                        Some(SharedString::from(t!("settings.scrollbackLinesDesc"))),
                        self.existing_number_input_box("settings.number.terminal-scrollback-lines"),
                    ))
                    .child(settings_form_row(
                        palette,
                        t!("settings.keepAliveMode"),
                        Some(SharedString::from(t!(
                            "settings.keepAliveModeCompatibleDescription"
                        ))),
                        self.settings_select_control(
                            "settings.terminal.keep-alive-mode",
                            vec![
                                NyaSelectOption::new(
                                    "compatible",
                                    t!("settings.keepAliveModeCompatible"),
                                ),
                                NyaSelectOption::new("strict", t!("settings.keepAliveModeStrict")),
                                NyaSelectOption::new(
                                    "disabled",
                                    t!("settings.keepAliveModeDisabled"),
                                ),
                            ],
                            self.settings.summary().terminal_keep_alive_mode.clone(),
                            false,
                            cx,
                        ),
                    ))
                    .child(settings_form_row(
                        palette,
                        t!("settings.keepAliveInterval"),
                        Some(SharedString::from(t!("settings.keepAliveIntervalDesc"))),
                        self.existing_number_input_box(
                            "settings.number.terminal-keep-alive-interval",
                        ),
                    ))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(terminal_settings_field_meta(
                                palette,
                                t!("settings.x11Display"),
                                t!("settings.x11DisplayDesc"),
                            ))
                            .child(div().w_full().max_w(px(520.)).child(x11_display_input)),
                    )
                    .child(settings_form_row(
                        palette,
                        t!("settings.hardwareAcceleration"),
                        Some(SharedString::from(t!("settings.hardwareAccelerationDesc"))),
                        div().text_xs().child(t!("settings.nativeGpuRenderer")),
                    ))
                    .child(settings_form_row(
                        palette,
                        t!("settings.lowLatencyMode"),
                        Some(SharedString::from(t!("settings.lowLatencyModeDesc"))),
                        settings_switch(
                            palette,
                            "terminal-low-latency-mode",
                            self.settings.summary().terminal_low_latency_mode,
                            cx.listener(|this, _, _, cx| {
                                this.toggle_terminal_low_latency_mode(cx);
                            }),
                        ),
                    ))
                    .child(settings_form_row(
                        palette,
                        t!("settings.zebraStripes"),
                        Some(SharedString::from(t!("settings.zebraStripesDesc"))),
                        settings_switch(
                            palette,
                            "terminal-zebra-stripes",
                            self.settings.summary().terminal_zebra_stripes_enabled,
                            cx.listener(|this, _, _, cx| {
                                this.toggle_terminal_zebra_stripes(cx);
                            }),
                        ),
                    ))
                    .child(settings_form_row(
                        palette,
                        t!("settings.showWorkspacePadding"),
                        Some(SharedString::from(t!("settings.showWorkspacePaddingDesc"))),
                        settings_switch(
                            palette,
                            "terminal-workspace-padding",
                            self.settings.summary().terminal_show_workspace_padding,
                            cx.listener(|this, _, _, cx| {
                                this.toggle_terminal_workspace_padding(cx);
                            }),
                        ),
                    ))
                    .child(settings_form_row(
                        palette,
                        t!("settings.showLineNumbers"),
                        Some(SharedString::from(t!("settings.showLineNumbersDesc"))),
                        settings_switch(
                            palette,
                            "terminal-line-numbers",
                            self.settings.summary().terminal_show_line_numbers,
                            cx.listener(|this, _, _, cx| {
                                this.toggle_terminal_line_numbers(cx);
                            }),
                        ),
                    ))
                    .child(settings_form_row(
                        palette,
                        t!("settings.showTimestamps"),
                        Some(SharedString::from(t!("settings.showTimestampsDesc"))),
                        settings_switch(
                            palette,
                            "terminal-timestamps",
                            self.settings.summary().terminal_show_timestamps,
                            cx.listener(|this, _, _, cx| {
                                this.toggle_terminal_timestamps(cx);
                            }),
                        ),
                    ))
                    .when(self.settings.summary().terminal_show_timestamps, |this| {
                        this.child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .child(terminal_settings_field_meta(
                                    palette,
                                    t!("settings.timestampFormat"),
                                    t!("settings.timestampFormatDesc"),
                                ))
                                .child(
                                    div().w_full().max_w(px(520.)).child(timestamp_format_input),
                                ),
                        )
                    })
                    .child(settings_form_row(
                        palette,
                        t!("terminal.showMultiLinePasteDialog"),
                        Some(SharedString::from(t!(
                            "terminal.showMultiLinePasteDialogDesc"
                        ))),
                        settings_switch(
                            palette,
                            "terminal-multi-line-paste",
                            self.settings
                                .summary()
                                .terminal_show_multi_line_paste_dialog,
                            cx.listener(|this, _, _, cx| {
                                this.toggle_multi_line_paste_dialog(cx);
                            }),
                        ),
                    ))
                    .child(settings_form_row(
                        palette,
                        t!("terminal.pasteImageAsPath"),
                        Some(SharedString::from(t!("terminal.pasteImageAsPathDesc"))),
                        settings_switch(
                            palette,
                            "terminal-paste-image-path",
                            self.settings.summary().terminal_paste_image_as_path,
                            cx.listener(|this, _, _, cx| {
                                this.toggle_paste_image_as_path(cx);
                            }),
                        ),
                    ))
                    .child(settings_form_row(
                        palette,
                        t!("settings.reconnectRestoreCwd"),
                        Some(SharedString::from(t!("settings.reconnectRestoreCwdDesc"))),
                        settings_switch(
                            palette,
                            "terminal-reconnect-restore-cwd",
                            self.settings.summary().terminal_reconnect_restore_cwd,
                            cx.listener(|this, _, _, cx| {
                                this.toggle_terminal_reconnect_restore_cwd(cx);
                            }),
                        ),
                    ))
                    .child(settings_form_row(
                        palette,
                        t!("settings.showNotesPanel"),
                        Some(SharedString::from(t!("settings.showNotesPanelDesc"))),
                        settings_switch(
                            palette,
                            "terminal-notes-panel",
                            self.settings.summary().ui_show_notes_panel,
                            cx.listener(|this, _, _, cx| {
                                this.toggle_notes_panel(cx);
                            }),
                        ),
                    ))
                    .child(settings_form_row(
                        palette,
                        t!("settings.showRemoteStats"),
                        Some(SharedString::from(t!("settings.showRemoteStatsDesc"))),
                        settings_switch(
                            palette,
                            "terminal-remote-stats",
                            self.settings.summary().ui_show_remote_stats,
                            cx.listener(|this, _, _, cx| {
                                this.toggle_remote_stats_panel(cx);
                            }),
                        ),
                    ))
                    .when(self.settings.summary().ui_show_remote_stats, |this| {
                        this.child(settings_form_row(
                            palette,
                            t!("settings.remoteStatsInterval"),
                            Some(SharedString::from(t!("settings.remoteStatsIntervalDesc"))),
                            self.existing_number_input_box("settings.number.remote-stats-interval"),
                        ))
                    })
                    .child(settings_form_row(
                        palette,
                        t!("settings.showGpuMonitor"),
                        Some(SharedString::from(t!("settings.showGpuMonitorDesc"))),
                        settings_switch(
                            palette,
                            "terminal-gpu-monitor",
                            self.settings.summary().ui_show_gpu_monitor,
                            cx.listener(|this, _, _, cx| {
                                this.toggle_gpu_monitor_panel(cx);
                            }),
                        ),
                    ))
                    .when(self.settings.summary().ui_show_gpu_monitor, |this| {
                        this.child(settings_form_row(
                            palette,
                            t!("settings.gpuMonitorInterval"),
                            Some(SharedString::from(t!("settings.gpuMonitorIntervalDesc"))),
                            self.existing_number_input_box("settings.number.gpu-monitor-interval"),
                        ))
                    })
                    .child(settings_form_row(
                        palette,
                        t!("settings.showAscendNpuMonitor"),
                        Some(SharedString::from(t!("settings.showAscendNpuMonitorDesc"))),
                        settings_switch(
                            palette,
                            "terminal-ascend-npu-monitor",
                            self.settings.summary().ui_show_ascend_npu_monitor,
                            cx.listener(|this, _, _, cx| {
                                this.toggle_ascend_npu_monitor_panel(cx);
                            }),
                        ),
                    ))
                    .when(self.settings.summary().ui_show_ascend_npu_monitor, |this| {
                        this.child(settings_form_row(
                            palette,
                            t!("settings.ascendNpuMonitorInterval"),
                            Some(SharedString::from(t!(
                                "settings.ascendNpuMonitorIntervalDesc"
                            ))),
                            self.existing_number_input_box(
                                "settings.number.ascend-npu-monitor-interval",
                            ),
                        ))
                    })
                    .child(settings_form_row(
                        palette,
                        t!("settings.showProcessManager"),
                        Some(SharedString::from(t!("settings.showProcessManagerDesc"))),
                        settings_switch(
                            palette,
                            "terminal-process-manager",
                            self.settings.summary().ui_show_process_manager,
                            cx.listener(|this, _, _, cx| {
                                this.toggle_process_manager_panel(cx);
                            }),
                        ),
                    ))
                    .when(self.settings.summary().ui_show_process_manager, |this| {
                        this.child(settings_form_row(
                            palette,
                            t!("settings.processManagerInterval"),
                            Some(SharedString::from(t!(
                                "settings.processManagerIntervalDesc"
                            ))),
                            self.existing_number_input_box(
                                "settings.number.process-manager-interval",
                            ),
                        ))
                    })
                    .child(settings_form_row(
                        palette,
                        t!("settings.showDockerManager"),
                        Some(SharedString::from(t!("settings.showDockerManagerDesc"))),
                        settings_switch(
                            palette,
                            "terminal-docker-manager",
                            self.settings.summary().ui_show_docker_manager,
                            cx.listener(|this, _, _, cx| {
                                this.toggle_docker_manager_panel(cx);
                            }),
                        ),
                    ))
                    .when(self.settings.summary().ui_show_docker_manager, |this| {
                        this.child(settings_form_row(
                            palette,
                            t!("settings.dockerManagerInterval"),
                            Some(SharedString::from(t!("settings.dockerManagerIntervalDesc"))),
                            self.existing_number_input_box(
                                "settings.number.docker-manager-interval",
                            ),
                        ))
                    }),
            ))
            .child(settings_form_section(
                palette,
                None,
                None,
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(settings_form_row(
                        palette,
                        t!("settings.actionLinks"),
                        Some(SharedString::from(t!("settings.actionLinksDesc"))),
                        settings_switch(
                            palette,
                            "terminal-action-links",
                            action_links_enabled,
                            cx.listener(|this, _, _, cx| {
                                this.toggle_terminal_action_links(cx);
                            }),
                        ),
                    ))
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight(500.))
                            .text_color(rgb(palette.text))
                            .child(t!("settings.actionLinksMatchers")),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(terminal_action_matcher_row(
                                palette,
                                TerminalActionMatcherPresentation {
                                    id: "terminal-action-links-ipv4",
                                    label: t!("settings.actionLinksMatcherIpv4"),
                                    example: Cow::Borrowed("192.168.1.1"),
                                    description: t!("settings.actionLinksMatcherIpv4Desc"),
                                    checked: self
                                        .settings
                                        .summary()
                                        .terminal_action_links_matchers
                                        .ipv4,
                                    enabled: action_links_enabled,
                                },
                                cx.listener(|this, _, _, cx| {
                                    this.toggle_terminal_action_links_matcher("ipv4", cx);
                                }),
                            ))
                            .child(terminal_action_matcher_row(
                                palette,
                                TerminalActionMatcherPresentation {
                                    id: "terminal-action-links-host-port",
                                    label: t!("settings.actionLinksMatcherHostPort"),
                                    example: Cow::Borrowed("localhost:8080"),
                                    description: t!("settings.actionLinksMatcherHostPortDesc"),
                                    checked: self
                                        .settings
                                        .summary()
                                        .terminal_action_links_matchers
                                        .host_port,
                                    enabled: action_links_enabled,
                                },
                                cx.listener(|this, _, _, cx| {
                                    this.toggle_terminal_action_links_matcher("host_port", cx);
                                }),
                            ))
                            .child(terminal_action_matcher_row(
                                palette,
                                TerminalActionMatcherPresentation {
                                    id: "terminal-action-links-archive",
                                    label: t!("settings.actionLinksMatcherArchive"),
                                    example: Cow::Borrowed("backup.tar.gz"),
                                    description: t!("settings.actionLinksMatcherArchiveDesc"),
                                    checked: self
                                        .settings
                                        .summary()
                                        .terminal_action_links_matchers
                                        .archive,
                                    enabled: action_links_enabled,
                                },
                                cx.listener(|this, _, _, cx| {
                                    this.toggle_terminal_action_links_matcher("archive", cx);
                                }),
                            )),
                    ),
            ))
            .child(self.keyword_highlights_settings_section(cx))
    }
}

fn terminal_settings_field_meta(
    palette: ThemePalette,
    label: impl Into<SharedString>,
    desc: impl Into<SharedString>,
) -> impl IntoElement {
    let label: SharedString = label.into();
    let desc: SharedString = desc.into();
    div()
        .min_w_0()
        .child(
            div()
                .text_size(px(13.))
                .font_weight(FontWeight(500.))
                .text_color(rgb(palette.text))
                .child(label),
        )
        .child(
            div()
                .mt_1()
                .text_size(px(11.))
                .text_color(rgb(palette.text_dimmed))
                .child(desc),
        )
}

fn terminal_action_matcher_row(
    palette: ThemePalette,
    presentation: TerminalActionMatcherPresentation,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let TerminalActionMatcherPresentation {
        id,
        label,
        example,
        description,
        checked,
        enabled,
    } = presentation;
    div()
        .rounded_md()
        .border_1()
        .border_color(rgb(palette.border))
        .bg(rgb(palette.input))
        .px_3()
        .py_2()
        .flex()
        .items_center()
        .justify_between()
        .gap_3()
        .child(
            div()
                .min_w_0()
                .flex_1()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .text_size(px(12.))
                                .font_weight(FontWeight(500.))
                                .text_color(rgb(palette.text))
                                .child(label),
                        )
                        .child(
                            div()
                                .px_2()
                                .py(px(1.))
                                .rounded_sm()
                                .bg(rgb(palette.surface_elevated))
                                .font_family(crate::features::shell::gpui_code_font_family())
                                .text_size(px(10.))
                                .text_color(rgb(palette.text_muted))
                                .child(example),
                        ),
                )
                .child(
                    div()
                        .mt_1()
                        .text_size(px(11.))
                        .text_color(rgb(palette.text_dimmed))
                        .child(description),
                ),
        )
        .child(settings_switch_with_enabled(
            palette, id, checked, enabled, on_click,
        ))
}

struct TerminalActionMatcherPresentation {
    id: &'static str,
    label: Cow<'static, str>,
    example: Cow<'static, str>,
    description: Cow<'static, str>,
    checked: bool,
    enabled: bool,
}
