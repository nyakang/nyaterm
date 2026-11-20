use gpui::{Context, IntoElement, div, prelude::*, px, rgb};
use nyaterm_core::themes::appearance::{SystemAppearance, ThemeMode};
use nyaterm_ui::{NyaSelectOption, NyaTabItem, NyaTabs, NyaTabsVariant, ThemePalette};
use rust_i18n::t;

use crate::features::pages::settings::panel::SettingsPanel;
use crate::features::pages::settings::settings_form_row;

impl SettingsPanel {
    pub(super) fn appearance_theme_settings(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let palette = self.theme_palette();
        let system = self.settings.summary().theme_mode == ThemeMode::System;
        let mode_tabs = div()
            .w(px(260.))
            .max_w_full()
            .debug_selector(|| "appearance-theme-mode".into())
            .child(
                NyaTabs::new("appearance-theme-mode")
                    .variant(NyaTabsVariant::Segmented)
                    .items([
                        NyaTabItem::new(t!("settings.themeModeManual")),
                        NyaTabItem::new(t!("settings.themeModeSystem")),
                    ])
                    .selected_index(usize::from(system))
                    .on_select(cx.listener(|panel, index: &usize, _, cx| {
                        let mode = if *index == 1 {
                            ThemeMode::System
                        } else {
                            ThemeMode::Manual
                        };
                        panel.with_app(cx, |app, cx| app.update_theme_mode(mode, cx));
                    })),
            );
        let mut content = div().flex().flex_col().gap_3().child(settings_form_row(
            palette,
            t!("settings.themeMode"),
            None,
            mode_tabs,
        ));
        if system {
            let light = self.system_theme_card(SystemAppearance::Light, cx);
            let dark = self.system_theme_card(SystemAppearance::Dark, cx);
            content = content.child(div().flex().flex_wrap().gap_3().child(light).child(dark));
        } else {
            content = content.child(settings_form_row(
                palette,
                t!("settings.theme"),
                None,
                div()
                    .w_full()
                    .child(self.appearance_theme_select(false, cx)),
            ));
        }
        content
    }

    fn system_theme_card(
        &mut self,
        appearance: SystemAppearance,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let palette = self.theme_palette();
        let summary = self.settings.summary();
        let (control_id, selected, label) = match appearance {
            SystemAppearance::Light => (
                "appearance-light-theme",
                summary.light_theme.clone(),
                t!("settings.systemLightTheme"),
            ),
            SystemAppearance::Dark => (
                "appearance-dark-theme",
                summary.dark_theme.clone(),
                t!("settings.systemDarkTheme"),
            ),
        };
        let fallback = match appearance {
            SystemAppearance::Light => nyaterm_core::themes::appearance::DEFAULT_LIGHT_THEME_ID,
            SystemAppearance::Dark => nyaterm_core::themes::DEFAULT_THEME_ID,
        };
        let preview_id = if nyaterm_core::themes::is_builtin(&selected)
            || summary.custom_themes.resolve(&selected).is_some()
        {
            selected.as_str()
        } else {
            fallback
        };
        let preview = nyaterm_ui::resolve_theme_palette(preview_id, &summary.custom_themes);
        let active = self.settings.system_appearance == appearance;
        let mut options = self
            .settings
            .theme_options
            .iter()
            .filter(|(id, _)| {
                *id == selected
                    || nyaterm_ui::resolve_theme_palette(id, &summary.custom_themes).is_dark()
                        == (appearance == SystemAppearance::Dark)
            })
            .map(|(id, name)| NyaSelectOption::new(id.clone(), name.clone()))
            .collect::<Vec<_>>();
        // Preserve missing or unsupported references until the user replaces them.
        if !self
            .settings
            .theme_options
            .iter()
            .any(|(id, _)| *id == selected)
        {
            options.push(NyaSelectOption::new(selected.clone(), selected.clone()));
        }
        let select = self.select_control(control_id, options, Some(selected), false, cx);
        div()
            .flex_1()
            .min_w(px(200.))
            .flex()
            .flex_col()
            .gap_3()
            .p_3()
            .rounded_md()
            .border_1()
            .border_color(rgb(if active {
                palette.accent
            } else {
                palette.border
            }))
            .bg(rgb(palette.bg))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(palette.text))
                            .child(label),
                    )
                    .when(active, |row| {
                        row.child(
                            div()
                                .text_size(px(11.))
                                .text_color(rgb(palette.accent))
                                .child(t!("settings.themeInUse")),
                        )
                    }),
            )
            .child(theme_swatch_preview(preview))
            .child(select)
    }
}

fn theme_swatch_preview(palette: ThemePalette) -> impl IntoElement {
    div()
        .h(px(70.))
        .w_full()
        .flex()
        .rounded_sm()
        .overflow_hidden()
        .border_1()
        .border_color(rgb(palette.border))
        .bg(rgb(palette.bg))
        .child(
            div()
                .w(px(42.))
                .h_full()
                .bg(rgb(palette.surface))
                .p_2()
                .flex()
                .flex_col()
                .gap_2()
                .children((0..3).map(|_| {
                    div()
                        .h(px(3.))
                        .w_full()
                        .bg(rgb(palette.border))
                        .rounded_sm()
                })),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .p_3()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .w(px(56.))
                        .h(px(4.))
                        .rounded_sm()
                        .bg(rgb(palette.text)),
                )
                .child(
                    div()
                        .w(px(80.))
                        .h(px(3.))
                        .rounded_sm()
                        .bg(rgb(palette.accent)),
                )
                .child(
                    div()
                        .w_full()
                        .h(px(3.))
                        .rounded_sm()
                        .bg(rgb(palette.border)),
                ),
        )
}
