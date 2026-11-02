use rust_i18n::t;

use std::borrow::Cow;

use gpui::{
    App, ClickEvent, Context, FontWeight, IntoElement, SharedString, Window, div, prelude::*, px,
    rgb, svg,
};
use nyaterm_ui::{NyaSelectOption, NyaTooltip};

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
        let raw_encoding = self.settings.summary().interaction_default_encoding.clone();
        let encoding = nyaterm_core::character_encoding::CharacterEncoding::parse(&raw_encoding)
            .map(|encoding| encoding.label().to_string())
            .unwrap_or(raw_encoding);
        let x11_display_input = self
            .existing_text_input_box("settings.terminal.x11-display", false)
            .into_any_element();
        let timestamp_format_input = self
            .existing_text_input_box("settings.terminal.timestamp-format", false)
            .into_any_element();
        let action_links_enabled = self.settings.summary().terminal_action_links_enabled;
        let keep_alive_description = match self.settings.summary().terminal_keep_alive_mode.as_str()
        {
            "strict" => t!("settings.keepAliveModeStrictDescription"),
            "disabled" => t!("settings.keepAliveModeDisabledDescription"),
            _ => t!("settings.keepAliveModeCompatibleDescription"),
        };

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
                        Some(SharedString::from(keep_alive_description)),
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
                                "settings-x11-display-help",
                                t!("settings.x11Display"),
                                t!("settings.x11DisplayDesc"),
                                t!("settings.x11DisplayHelp"),
                            ))
                            .child(div().w_full().max_w(px(520.)).child(x11_display_input)),
                    )
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
                        None,
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
                        None,
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
                                    "settings-timestamp-format-help",
                                    t!("settings.timestampFormat"),
                                    t!("settings.timestampFormatDesc"),
                                    t!("settings.timestampFormatHelp"),
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
                    )),
            ))
            .child(settings_form_section(
                palette,
                Some(t!("settings.interactionEncoding")),
                None,
                self.settings_select_field(
                    "settings.interaction.default-encoding",
                    t!("settings.defaultEncoding"),
                    Some(SharedString::from(t!("settings.defaultEncodingDesc"))),
                    (
                        nyaterm_core::character_encoding::CharacterEncoding::ALL
                            .into_iter()
                            .map(|encoding| {
                                NyaSelectOption::new(encoding.label(), encoding.label())
                            })
                            .collect(),
                        encoding,
                        false,
                    ),
                    cx,
                ),
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
                                    id: "terminal-action-links-mac",
                                    label: t!("settings.actionLinksMatcherMac"),
                                    example: Cow::Borrowed("AA:BB:CC:DD:EE:FF"),
                                    description: t!("settings.actionLinksMatcherMacDesc"),
                                    checked: self
                                        .settings
                                        .summary()
                                        .terminal_action_links_matchers
                                        .mac,
                                    enabled: action_links_enabled,
                                },
                                cx.listener(|this, _, _, cx| {
                                    this.toggle_terminal_action_links_matcher("mac", cx);
                                }),
                            ))
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
            .child(self.recording_settings_section(cx))
    }
}

fn terminal_settings_field_meta(
    palette: ThemePalette,
    id: &'static str,
    label: impl Into<SharedString>,
    desc: impl Into<SharedString>,
    help: impl Into<SharedString>,
) -> impl IntoElement {
    let label: SharedString = label.into();
    let desc: SharedString = desc.into();
    let help: SharedString = help.into();
    div()
        .min_w_0()
        .child(
            div()
                .id(id)
                .flex()
                .items_center()
                .gap_1()
                .text_size(px(13.))
                .font_weight(FontWeight(500.))
                .text_color(rgb(palette.text))
                .child(label)
                .child(
                    svg()
                        .size(px(13.))
                        .path("icons/menu/info.svg")
                        .text_color(rgb(palette.text_dimmed)),
                )
                .tooltip(move |window, cx| NyaTooltip::new(help.clone()).build(window, cx)),
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
