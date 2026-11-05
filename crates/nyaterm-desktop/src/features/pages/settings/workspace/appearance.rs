use std::collections::HashMap;
use std::sync::Arc;

use rust_i18n::t;

use gpui::{
    AnyElement, App, ClickEvent, Context, FontWeight, IntoElement, SharedString, Window, div,
    prelude::*, px, rgb, rgba,
};
use nyaterm_ui::{NYA_FORM_CONTROL_HEIGHT_PX, NyaSelectOption};

use crate::features::{
    FontAvailability, FontCatalogLoadState, FontResolutionSource, FontResolutionStatus,
    normalize_font_family,
    pages::settings::panel::SettingsPanel,
    selects::FOLLOW_UI_THEME_VALUE,
    shell::{
        appearance_font_stack, configured_appearance_font_stack, gpui_terminal_font_fallback,
        gpui_ui_font_fallback,
    },
};
use crate::theme::ThemePalette;

use super::super::{settings_form_row, settings_form_section, settings_switch};
use super::window_background::AppearanceOpacity;

struct AppearanceFontStackPresentation {
    terminal: bool,
    kind: &'static str,
    normalized_options: Arc<HashMap<String, String>>,
    select_options: Arc<[NyaSelectOption]>,
    primary_label: String,
    fallback_label: String,
    remove_label: String,
}

impl SettingsPanel {
    pub(in crate::features) fn appearance_settings_section(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let manage_themes = self.theme_designer_entry_button(cx);
        div()
            .flex()
            .flex_col()
            .gap_5()
            .child(appearance_form_section_with_action(
                palette,
                t!("themeDesigner.sectionTitle"),
                t!("themeDesigner.sectionDesc"),
                manage_themes,
                div()
                    .flex()
                    .flex_col()
                    .gap_5()
                    .child(appearance_settings_field(
                        palette,
                        t!("settings.theme"),
                        None,
                        self.appearance_theme_select(false, cx),
                    ))
                    .child(appearance_settings_field(
                        palette,
                        t!("settings.terminalTheme"),
                        Some(SharedString::from(t!("settings.terminalThemeDesc"))),
                        self.appearance_theme_select(true, cx),
                    ))
                    .child(appearance_settings_field(
                        palette,
                        t!("settings.minimumContrastRatio"),
                        Some(SharedString::from(
                            t!("settings.minimumContrastRatioDesc"),
                        )),
                        self.appearance_contrast_select(cx),
                    ))
                    .child(settings_form_row(
                        palette,
                        t!("settings.boldDefaultForeground"),
                        Some(SharedString::from(t!(
                            "settings.boldDefaultForegroundDesc"
                        ))),
                        settings_switch(
                            palette,
                            "appearance-bold-default-foreground",
                            self.settings.summary().bold_default_foreground,
                            cx.listener(|this, _, _, cx| {
                                this.toggle_bold_default_foreground(cx);
                            }),
                        ),
                    )),
            ))
            .child(self.window_background_section(cx))
            .map(|element| {
                #[cfg(target_os = "linux")]
                let element = element.child(self.window_decorations_section(cx));
                element
            })
            .child(settings_form_section(
                palette,
                Some(t!("settings.backgroundImage")),
                Some(t!("settings.backgroundImageDesc")),
                {
                    let path_label = self
                        .settings
                        .summary()
                        .background_image_path
                        .as_deref()
                        .map(|p| {
                            if p.chars().count() > 56 {
                                format!(
                                    "…{}",
                                    p.chars()
                                        .rev()
                                        .take(52)
                                        .collect::<String>()
                                        .chars()
                                        .rev()
                                        .collect::<String>()
                                )
                            } else {
                                p.to_string()
                            }
                        })
                        .unwrap_or_else(|| t!("settings.backgroundImageEmpty").to_string());
                    let has_image = self.settings.summary().background_image_path.is_some();
                    div()
                        .flex()
                        .flex_col()
                        .gap_5()
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .h(px(NYA_FORM_CONTROL_HEIGHT_PX))
                                        .flex()
                                        .items_center()
                                        .min_w_0()
                                        .flex_1()
                                        .px_3()
                                        .rounded_sm()
                                        .border_1()
                                        .border_color(rgb(palette.border))
                                        .bg(rgb(palette.input))
                                        .font_family(crate::features::shell::gpui_code_font_family())
                                        .text_size(px(11.))
                                        .text_color(rgb(if has_image {
                                            palette.text
                                        } else {
                                            palette.text_muted
                                        }))
                                        .child(div().min_w_0().truncate().child(path_label)),
                                )
                                .child(appearance_icon_text_button(
                                    palette,
                                    "appearance-wallpaper-browse",
                                    "icons/conn/folder.svg",
                                    t!("settings.selectBackgroundImage"),
                                    cx.listener(|this, _, _, cx| {
                                        this.prompt_background_image(cx);
                                    }),
                                ))
                                .when(has_image, |this| {
                                    this.child(appearance_icon_text_button(
                                        palette,
                                        "appearance-wallpaper-clear",
                                        "icons/fe/delete.svg",
                                        t!("settings.removeBackgroundImage"),
                                        cx.listener(|this, _, _, cx| {
                                            this.clear_background_image(cx);
                                        }),
                                    ))
                                }),
                        )
                        .child(appearance_settings_field(
                            palette,
                            t!("settings.backgroundImageFit"),
                            None,
                            self.appearance_background_fit_select(has_image, cx),
                        ))
                        .child(self.appearance_opacity_slider(AppearanceOpacity::Image, has_image, cx))
                },
            ))
            .child(self.appearance_font_stack_settings_section(false, cx))
            .child(self.appearance_font_stack_settings_section(true, cx))
            .child(settings_form_section(
                palette,
                None,
                None,
                div()
                    .flex()
                    .flex_col()
                    .gap_5()
                    .child(appearance_settings_field(
                        palette,
                        t!("settings.cursorStyle"),
                        None,
                        self.appearance_cursor_style_select(cx),
                    ))
                    .child(settings_form_row(
                        palette,
                        t!("settings.cursorBlink"),
                        None,
                        settings_switch(
                            palette,
                            "appearance-cursor-blink",
                            self.settings.summary().cursor_blink,
                            cx.listener(|this, _, _, cx| {
                                this.toggle_cursor_blink(cx);
                            }),
                        ),
                    )),
            ))
    }

    fn appearance_font_stack_settings_section(
        &mut self,
        terminal: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let (title, desc, raw, fallback) = if terminal {
            (
                t!("settings.terminalFontFamily"),
                t!("settings.terminalFontFamilyDesc"),
                self.settings.summary().terminal_font_family.clone(),
                "JetBrains Mono",
            )
        } else {
            (
                t!("settings.uiFontFamily"),
                t!("settings.uiFontFamilyDesc"),
                self.settings.summary().ui_font_family.clone(),
                "Inter",
            )
        };
        let fonts = appearance_font_stack(&raw, fallback);
        let (select_options, normalized_options) = self.font_select_option_catalog(terminal);
        let kind = if terminal { "terminal" } else { "ui" };
        let add_id = if terminal {
            "appearance-terminal-font-add"
        } else {
            "appearance-ui-font-add"
        };
        let add_handler: AppearanceClickHandler = Box::new(cx.listener(move |this, _, _, cx| {
            this.add_appearance_fallback_font(terminal, cx);
        }));
        let add_action = appearance_icon_text_button(
            palette,
            add_id,
            "icons/conn/add.svg",
            t!("settings.addFallbackFont"),
            add_handler,
        );
        let primary_label = t!("settings.fontPrimary");
        let fallback_label = t!("settings.fontFallback");
        let remove_label = t!("common.remove");
        let effective_summary = self.appearance_font_resolution_summary(terminal);
        let font_rows = self.appearance_font_stack_rows(
            &fonts,
            AppearanceFontStackPresentation {
                terminal,
                kind,
                normalized_options,
                select_options,
                primary_label: primary_label.to_string(),
                fallback_label: fallback_label.to_string(),
                remove_label: remove_label.to_string(),
            },
            cx,
        );

        let mut content = div()
            .flex()
            .flex_col()
            .gap_3()
            .when_some(effective_summary, |this, summary| {
                this.child(
                    div()
                        .text_size(px(11.))
                        .text_color(rgb(palette.text_muted))
                        .child(summary),
                )
            })
            .child(font_rows);

        if terminal {
            let _font_size_label = self.settings.summary().terminal_font_size.to_string();
            content = content
                .child(appearance_settings_field(
                    palette,
                    t!("settings.fontSize"),
                    None,
                    self.existing_number_input_box("appearance.number.terminal-font-size"),
                ))
                .child(appearance_settings_field(
                    palette,
                    t!("settings.terminalFontWeight"),
                    None,
                    self.appearance_font_weight_select(false, cx),
                ))
                .child(appearance_settings_field(
                    palette,
                    t!("settings.terminalFontWeightBold"),
                    None,
                    self.appearance_font_weight_select(true, cx),
                ));
        } else {
            let _font_size_label = self.settings.summary().ui_font_size.to_string();
            content = content.child(appearance_settings_field(
                palette,
                t!("settings.uiFontSize"),
                None,
                self.existing_number_input_box("appearance.number.ui-font-size"),
            ));
        }

        appearance_form_section_with_action(palette, title, desc, add_action, content)
    }

    fn appearance_font_stack_rows(
        &mut self,
        fonts: &[String],
        presentation: AppearanceFontStackPresentation,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let rows = fonts
            .iter()
            .cloned()
            .enumerate()
            .map(|(index, family)| self.appearance_font_stack_row(&presentation, index, family, cx))
            .collect::<Vec<_>>();
        div()
            .flex()
            .flex_col()
            .gap_3()
            .children(rows)
            .into_any_element()
    }

    fn appearance_font_stack_row(
        &mut self,
        presentation: &AppearanceFontStackPresentation,
        index: usize,
        family: String,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = self.theme_palette();
        let menu_id = format!("appearance-{}-font-{index}", presentation.kind);
        let select_options = Arc::clone(&presentation.select_options);
        let selected_option = presentation
            .normalized_options
            .get(&normalize_font_family(&family))
            .cloned();
        let selected_value = selected_option.unwrap_or_else(|| family.clone());
        let availability = self
            .settings
            .font_availability(&family, presentation.terminal);
        let placeholder_content = matches!(availability, FontAvailability::Unavailable { .. })
            .then(|| {
                div()
                    .w_full()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .truncate()
                            .text_color(rgb(palette.text_muted))
                            .child(family.clone()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_color(rgb(palette.danger))
                            .child(t!("settings.fontUnavailableSuffix")),
                    )
                    .into_any_element()
            });
        let delete_id = format!("appearance-{}-font-delete-{index}", presentation.kind);
        let terminal = presentation.terminal;
        let delete: AppearanceClickHandler = Box::new(cx.listener(move |this, _, _, cx| {
            this.remove_appearance_font_stack_entry(terminal, index, cx);
        }));
        let row_label = if index == 0 {
            presentation.primary_label.clone()
        } else {
            format!("{} {index}", presentation.fallback_label)
        };
        let select = self.font_select_control(
            menu_id,
            select_options,
            Some(selected_value),
            placeholder_content,
            false,
            cx,
        );

        div()
            .rounded_md()
            .border_1()
            .border_color(rgb(palette.border))
            .bg(rgb(palette.input))
            .p_3()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_start()
                    .gap_3()
                    .child(
                        div()
                            .w(px(96.))
                            .h(px(NYA_FORM_CONTROL_HEIGHT_PX))
                            .flex_none()
                            .flex()
                            .items_center()
                            .text_size(px(11.))
                            .font_weight(FontWeight(500.))
                            .text_color(rgb(palette.text_muted))
                            .child(row_label),
                    )
                    .child(div().flex_1().min_w(px(220.)).child(select))
                    .child(appearance_icon_button(
                        palette,
                        delete_id,
                        "icons/fe/delete.svg",
                        presentation.remove_label.clone(),
                        delete,
                    )),
            )
            .into_any_element()
    }

    fn appearance_font_resolution_summary(&self, terminal: bool) -> Option<String> {
        match self.settings.font_catalog_state() {
            FontCatalogLoadState::Unloaded | FontCatalogLoadState::Loading => {
                return Some(t!("settings.fontAvailabilityChecking").to_string());
            }
            FontCatalogLoadState::Failed => {
                return Some(t!("settings.fontCatalogUnavailable").to_string());
            }
            FontCatalogLoadState::Loaded => {}
        }
        let status = self.appearance_font_resolution(terminal)?;
        let configured_families = if terminal {
            configured_appearance_font_stack(
                &self.settings.summary().terminal_font_family,
                "JetBrains Mono",
            )
        } else {
            let raw = if self.settings.summary().ui_font_family.trim().is_empty() {
                self.settings.summary().terminal_font_family.as_str()
            } else {
                self.settings.summary().ui_font_family.as_str()
            };
            configured_appearance_font_stack(raw, "Inter")
        };
        let mut effective_families = configured_families
            .into_iter()
            .filter_map(
                |family| match self.settings.font_availability(&family, terminal) {
                    FontAvailability::Available { resolved_family } => {
                        Some(resolved_family.to_string())
                    }
                    FontAvailability::Internal => Some(family),
                    FontAvailability::Unavailable { .. } | FontAvailability::Checking => None,
                },
            )
            .fold(Vec::<String>::new(), |mut families, family| {
                if !families.iter().any(|existing| {
                    normalize_font_family(existing) == normalize_font_family(&family)
                }) {
                    families.push(family);
                }
                families
            });
        if effective_families.is_empty()
            || matches!(
                status.source,
                FontResolutionSource::PlatformDefault
                    | FontResolutionSource::EmergencyMetricsFallback
            )
        {
            effective_families = vec![status.effective_family];
        }
        Some(
            t!(
                "settings.effectiveFont",
                families = effective_families.join(", ")
            )
            .to_string(),
        )
    }

    fn appearance_font_resolution(&self, terminal: bool) -> Option<FontResolutionStatus> {
        let (raw, fallback, platform_default) = if terminal {
            (
                self.settings.summary().terminal_font_family.as_str(),
                "JetBrains Mono",
                gpui_terminal_font_fallback(),
            )
        } else {
            (
                if self.settings.summary().ui_font_family.trim().is_empty() {
                    self.settings.summary().terminal_font_family.as_str()
                } else {
                    self.settings.summary().ui_font_family.as_str()
                },
                "Inter",
                gpui_ui_font_fallback(),
            )
        };
        let families = configured_appearance_font_stack(raw, fallback);
        self.settings
            .resolve_font_stack(&families, terminal, platform_default)
    }

    fn appearance_theme_select(
        &mut self,
        terminal: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id = if terminal {
            "appearance-terminal-theme"
        } else {
            "appearance-ui-theme"
        };
        let current = if terminal {
            self.settings
                .summary()
                .terminal_theme
                .as_deref()
                .filter(|theme| !theme.trim().is_empty())
        } else {
            Some(self.settings.summary().theme.as_str())
        };
        let mut options = Vec::new();
        if terminal {
            options.push(NyaSelectOption::new(
                FOLLOW_UI_THEME_VALUE,
                t!("settings.followUiTheme"),
            ));
        }
        for (theme_id, name) in self.settings.theme_options.iter().cloned() {
            options.push(NyaSelectOption::new(theme_id, name));
        }
        let selected = current
            .map(|theme| {
                if theme == "catppuccin" {
                    "catppuccin-mocha"
                } else {
                    theme
                }
            })
            .unwrap_or(FOLLOW_UI_THEME_VALUE)
            .to_string();
        self.select_control(id, options, Some(selected), false, cx)
    }

    fn appearance_contrast_select(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let id = "appearance-minimum-contrast";
        let current = match self.settings.summary().minimum_contrast_ratio.as_str() {
            "3" => "3",
            "4.5" => "4.5",
            "7" => "7",
            "21" => "21",
            _ => "1",
        };
        let label_for = |ratio: &str| match ratio {
            "3" => t!("settings.minimumContrastRatio_3"),
            "4.5" => t!("settings.minimumContrastRatio_4_5"),
            "7" => t!("settings.minimumContrastRatio_7"),
            "21" => t!("settings.minimumContrastRatio_21"),
            _ => t!("settings.minimumContrastRatio_1"),
        };
        let options = ["1", "3", "4.5", "7", "21"]
            .into_iter()
            .map(|ratio| NyaSelectOption::new(ratio, label_for(ratio)))
            .collect();
        self.select_control(id, options, Some(current.to_string()), false, cx)
    }

    fn appearance_background_fit_select(
        &mut self,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id = "appearance-background-fit";
        let current = match self.settings.summary().background_image_fit.as_str() {
            "contain" => "contain",
            "stretch" | "fill" => "stretch",
            "tile" => "tile",
            _ => "cover",
        };
        let label_for = |fit: &str| match fit {
            "contain" => t!("settings.backgroundImageFit_contain"),
            "stretch" => t!("settings.backgroundImageFit_stretch"),
            "tile" => t!("settings.backgroundImageFit_tile"),
            _ => t!("settings.backgroundImageFit_cover"),
        };
        let options = ["cover", "contain", "stretch", "tile"]
            .into_iter()
            .map(|fit| NyaSelectOption::new(fit, label_for(fit)))
            .collect();
        self.select_control(id, options, Some(current.to_string()), !enabled, cx)
    }

    fn appearance_font_weight_select(
        &mut self,
        bold: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id = if bold {
            "appearance-terminal-font-weight-bold"
        } else {
            "appearance-terminal-font-weight"
        };
        let current = if bold {
            self.settings.summary().terminal_font_weight_bold
        } else {
            self.settings.summary().terminal_font_weight
        };
        let label_for = |weight| match weight {
            300 => t!("settings.fontWeight_300"),
            500 => t!("settings.fontWeight_500"),
            600 => t!("settings.fontWeight_600"),
            700 => t!("settings.fontWeight_700"),
            800 => t!("settings.fontWeight_800"),
            _ => t!("settings.fontWeight_400"),
        };
        let options = [300_u16, 400, 500, 600, 700, 800]
            .into_iter()
            .map(|weight| NyaSelectOption::new(weight.to_string(), label_for(weight)))
            .collect();
        let selected = match current {
            300 => 300,
            500 => 500,
            600 => 600,
            700 => 700,
            800 => 800,
            _ => 400,
        };
        self.select_control(id, options, Some(selected.to_string()), false, cx)
    }

    fn appearance_cursor_style_select(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let id = "appearance-cursor-style";
        let current = match self.settings.summary().cursor_style.as_str() {
            "underline" => "underline",
            "bar" => "bar",
            _ => "block",
        };
        let label_for = |style: &str| match style {
            "underline" => t!("settings.cursorUnderline"),
            "bar" => t!("settings.cursorBar"),
            _ => t!("settings.cursorBlock"),
        };
        let options = ["block", "underline", "bar"]
            .into_iter()
            .map(|style| NyaSelectOption::new(style, label_for(style)))
            .collect();
        self.select_control(id, options, Some(current.to_string()), false, cx)
    }
}

type AppearanceClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

fn appearance_form_section_with_action(
    palette: ThemePalette,
    title: impl Into<SharedString>,
    desc: impl Into<SharedString>,
    action: impl IntoElement,
    content: impl IntoElement,
) -> impl IntoElement {
    let title: SharedString = title.into();
    let desc: SharedString = desc.into();
    div()
        .rounded_lg()
        .border_1()
        .border_color(rgba((palette.border << 8) | 0xb3))
        .bg(rgba((palette.surface << 8) | 0x99))
        .overflow_hidden()
        .child(
            div()
                .px_4()
                .py_4()
                .border_b_1()
                .border_color(rgba((palette.surface_elevated << 8) | 0x99))
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
                                .text_size(px(14.))
                                .font_weight(FontWeight(600.))
                                .text_color(rgb(palette.text))
                                .child(title),
                        )
                        .child(
                            div()
                                .mt_1()
                                .text_size(px(12.))
                                .text_color(rgb(palette.text_dimmed))
                                .child(desc),
                        ),
                )
                .child(action),
        )
        .child(div().px_4().py_4().child(content))
}

fn appearance_settings_field(
    palette: ThemePalette,
    label: impl Into<SharedString>,
    desc: Option<SharedString>,
    control: impl IntoElement,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .text_size(px(13.))
                .font_weight(FontWeight(500.))
                .text_color(rgb(palette.text))
                .child(label.into()),
        )
        .when_some(desc, |this, desc| {
            this.child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(palette.text_dimmed))
                    .child(desc),
            )
        })
        .child(div().w_full().max_w(px(576.)).child(control))
}

fn appearance_icon_text_button(
    palette: ThemePalette,
    id: &'static str,
    icon_path: &'static str,
    label: impl Into<SharedString>,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    nyaterm_ui::NyaButton::new(id, label)
        .form_control()
        .icon(icon_path)
        .text_color(rgb(if icon_path == "icons/fe/delete.svg" {
            palette.danger
        } else {
            palette.text
        }))
        .on_click(on_click)
}

fn appearance_icon_button(
    palette: ThemePalette,
    id: String,
    icon_path: &'static str,
    tooltip: impl Into<SharedString>,
    on_click: AppearanceClickHandler,
) -> impl IntoElement {
    nyaterm_ui::NyaButton::new(id, "")
        .form_control()
        .icon(icon_path)
        .text_color(rgb(palette.danger))
        .tooltip(tooltip)
        .on_click(on_click)
}
