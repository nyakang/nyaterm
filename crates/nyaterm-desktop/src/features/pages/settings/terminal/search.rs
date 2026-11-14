use nyaterm_ui::icons::{SEARCH_ENGINE_ICON_IDS, search_engine_icon};
use rust_i18n::t;

use gpui::{
    App, ClickEvent, Context, FontWeight, IntoElement, SharedString, Window, div, prelude::*, px,
    rgb, rgba, svg,
};
use nyaterm_core::truncate_preview;

use crate::features::pages::settings::panel::SettingsPanel;
use crate::features::settings::SearchEngineMenu;
use crate::features::view_widgets::mono_icon;
use crate::theme::ThemePalette;
use nyaterm_ui::{NyaDropdownMenu, NyaMenuAnchor, NyaMenuItem, NyaTooltip};

use super::super::settings_switch;

impl SettingsPanel {
    pub(in crate::features) fn terminal_search_settings_section(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.online_search_engines_settings_section(cx)
    }

    pub(in crate::features) fn online_search_engines_settings_section(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let engines = self.settings.summary().search_custom_engines.clone();
        let interaction = self.settings.search_engine_presentation();
        let expanded_index = interaction.expanded_index;
        let icon_picker_index = interaction.icon_picker_index;
        // Built before the row closure, which only has `&self`: the inputs are
        // entities the app has to create, and only the expanded row shows them.
        let mut editor_inputs = expanded_index.and_then(|index| {
            engines.get(index)?;
            let name = self
                .existing_text_input_box(format!("settings.search-engine.{index}.name"), false)
                .into_any_element();
            let url = self
                .existing_text_input_box(format!("settings.search-engine.{index}.url"), false)
                .into_any_element();
            Some((name, url))
        });
        let add_action = search_engine_text_button(
            palette,
            "settings-search-engine-add",
            "icons/conn/add.svg",
            t!("common.add"),
            false,
            true,
            cx.listener(|this, _, window, cx| {
                this.add_search_engine(cx);
                window.focus(this.settings.search_engine_focus(), cx);
            }),
        )
        .into_any_element();

        search_engine_settings_section(
            palette,
            t!("settings.customEngines"),
            t!("settings.engineUrlHint"),
            add_action,
            div()
                .flex()
                .flex_col()
                .gap_3()
                .when(engines.is_empty(), |this| {
                    this.child(
                        div()
                            .rounded_md()
                            .border_1()
                            .border_color(rgb(palette.border))
                            .bg(rgb(palette.input))
                            .px_4()
                            .py_6()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(12.))
                            .text_color(rgb(palette.text_dimmed))
                            .child(t!("settings.noCustomEngines")),
                    )
                })
                .when(!engines.is_empty(), |this| {
                    this.child(
                        div()
                            .rounded_md()
                            .border_1()
                            .border_color(rgb(palette.border))
                            .bg(rgb(palette.input))
                            .overflow_hidden()
                            .children(engines.into_iter().enumerate().map(|(index, engine)| {
                                let is_open = expanded_index == Some(index);
                                let (name_input, url_input) = if is_open {
                                    match editor_inputs.take() {
                                        Some((name, url)) => (Some(name), Some(url)),
                                        None => (None, None),
                                    }
                                } else {
                                    (None, None)
                                };
                                let icon_picker_open = icon_picker_index == Some(index);
                                let has_placeholder = engine.url_template.contains("%s");
                                let icon_def = engine
                                    .icon
                                    .as_deref()
                                    .map(|icon| search_engine_icon(Some(icon), palette));
                                let select_icon_label = t!("settings.selectIcon");
                                let show_menu_label = t!("settings.showInSearchMenu");
                                let actions_label = t!("settings.searchEngineActions");
                                let actions_panel = cx.entity().downgrade();

                                div()
                                    .id(SharedString::from(format!(
                                        "settings-search-engine-{index}"
                                    )))
                                    .when(index > 0, |this| {
                                        this.border_t_1().border_color(rgb(palette.border))
                                    })
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .px_3()
                                            .py_3()
                                            .flex()
                                            .items_start()
                                            .gap_3()
                                            .child(
                                                div()
                                                    .id(SharedString::from(format!(
                                                        "settings-search-engine-icon-{index}"
                                                    )))
                                                    .mt(px(2.))
                                                    .size(px(36.))
                                                    .rounded_md()
                                                    .bg(rgb(palette.bg))
                                                    .border_1()
                                                    .border_color(if icon_picker_open {
                                                        rgb(palette.link)
                                                    } else {
                                                        rgb(palette.border)
                                                    })
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .cursor_pointer()
                                                    .hover(|this| this.bg(rgb(palette.hover)))
                                                    .tooltip(move |window, cx| {
                                                        NyaTooltip::new(select_icon_label.clone())
                                                            .build(window, cx)
                                                    })
                                                    .child(match icon_def {
                                                        Some(def) => mono_icon(
                                                            def.path,
                                                            rgb(def
                                                                .tint(palette)
                                                                .unwrap_or(palette.text))
                                                            .into(),
                                                            18.,
                                                        )
                                                        .into_any_element(),
                                                        // No engine icon yet: an
                                                        // add affordance, not a
                                                        // brand.
                                                        None => mono_icon(
                                                            "icons/conn/add.svg",
                                                            rgb(palette.text_muted).into(),
                                                            18.,
                                                        )
                                                        .into_any_element(),
                                                    })
                                                    .on_click(cx.listener(move |this, _, _, cx| {
                                                        if this.toggle_search_engine_menu(
                                                            SearchEngineMenu::Icon,
                                                            index,
                                                            cx,
                                                        ) {
                                                            cx.notify();
                                                        }
                                                    })),
                                            )
                                            .child(
                                                div()
                                                    .min_w_0()
                                                    .flex_1()
                                                    .py(px(2.))
                                                    .child(
                                                        div()
                                                            .text_size(px(12.))
                                                            .font_weight(FontWeight(600.))
                                                            .text_color(rgb(palette.text))
                                                            .overflow_hidden()
                                                            .child(if engine.name.trim().is_empty() {
                                                                t!("settings.engineName")
                                                                    .to_string()
                                                            } else {
                                                                engine.name.clone()
                                                            }),
                                                    )
                                                    .child(
                                                        div()
                                                            .mt_1()
                                                            .text_size(px(10.))
                                                            .text_color(rgb(if has_placeholder {
                                                                palette.text_muted
                                                            } else {
                                                                palette.danger
                                                            }))
                                                            .overflow_hidden()
                                                            .child(
                                                                if engine
                                                                    .url_template
                                                                    .trim()
                                                                    .is_empty()
                                                                {
                                                                    t!("settings.engineUrl")
                                                                        .to_string()
                                                                } else {
                                                                    truncate_preview(
                                                                        &engine.url_template,
                                                                        56,
                                                                    )
                                                                },
                                                            ),
                                                    ),
                                            )
                                            .child(
                                                div()
                                                    .flex_none()
                                                    .flex()
                                                    .items_center()
                                                    .gap_2()
                                                    .child(
                                                        div()
                                                            .id(SharedString::from(format!(
                                                                "settings-search-engine-menu-tip-{index}"
                                                            )))
                                                            .tooltip(move |window, cx| {
                                                                NyaTooltip::new(show_menu_label.clone())
                                                                    .build(window, cx)
                                                            })
                                                            .child(settings_switch(
                                                                palette,
                                                                format!(
                                                                    "settings-search-engine-menu-{index}"
                                                                ),
                                                                engine.show_in_menu,
                                                                cx.listener(
                                                                    move |this, _, _, cx| {
                                                                        this.toggle_search_engine_in_menu(
                                                                            index, cx,
                                                                        );
                                                                    },
                                                                ),
                                                            )),
                                                    )
                                                    .child(
                                                        NyaDropdownMenu::new(format!(
                                                            "settings-search-engine-actions-{index}"
                                                        ))
                                                        .icon("icons/session/more.svg")
                                                        .icon_size(px(15.))
                                                        .tooltip(actions_label)
                                                        .anchor(NyaMenuAnchor::TopRight)
                                                        .min_width(px(144.))
                                                        .max_width(px(144.))
                                                        .items_dynamic(move |_, _| {
                                                            let edit_panel = actions_panel.clone();
                                                            let test_panel = actions_panel.clone();
                                                            let delete_panel = actions_panel.clone();
                                                            vec![
                                                                NyaMenuItem::action(t!(
                                                                    "common.edit"
                                                                ))
                                                                .icon("icons/net/edit.svg")
                                                                .on_click(move |_, _, cx| {
                                                                    let _ = edit_panel.update(
                                                                        cx,
                                                                        |panel, cx| {
                                                                            panel
                                                                                .expand_search_engine(
                                                                                    index, cx,
                                                                                );
                                                                        },
                                                                    );
                                                                }),
                                                                NyaMenuItem::action(t!(
                                                                    "settings.testSearchEngine"
                                                                ))
                                                                .icon("icons/fe/forward.svg")
                                                                .disabled(!has_placeholder)
                                                                .on_click(move |_, _, cx| {
                                                                    let _ = test_panel.update(
                                                                        cx,
                                                                        |panel, cx| {
                                                                            panel
                                                                                .close_search_engine_menus(
                                                                                    cx,
                                                                                );
                                                                            panel.test_search_engine(
                                                                                index, cx,
                                                                            );
                                                                        },
                                                                    );
                                                                }),
                                                                NyaMenuItem::separator(),
                                                                NyaMenuItem::action(t!(
                                                                    "common.delete"
                                                                ))
                                                                .icon("icons/fe/delete.svg")
                                                                .danger()
                                                                .on_click(move |_, _, cx| {
                                                                    let _ = delete_panel.update(
                                                                        cx,
                                                                        |panel, cx| {
                                                                            panel
                                                                                .remove_search_engine(
                                                                                    index, cx,
                                                                                );
                                                                        },
                                                                    );
                                                                }),
                                                            ]
                                                        }),
                                                    ),
                                            ),
                                    )
                                    .when(icon_picker_open, |this| {
                                        this.child(search_engine_icon_picker(
                                            palette,
                                            index,
                                            engine.icon.as_deref(),
                                            t!("common.remove"),
                                            cx,
                                        ))
                                    })
                                    .when(is_open, |this| {
                                        this.child(
                                            div()
                                                .border_t_1()
                                                .border_color(rgb(palette.border))
                                                .bg(rgb(palette.bg))
                                                .px_3()
                                                .py_3()
                                                .flex()
                                                .flex_col()
                                                .gap_3()
                                                .child(
                                                    div()
                                                        .flex()
                                                        .flex_col()
                                                        .gap_2()
                                                        .child(
                                                            div()
                                                                .text_size(px(11.))
                                                                .font_weight(FontWeight(500.))
                                                                .text_color(rgb(
                                                                    palette.text_muted,
                                                                ))
                                                                .child(t!(
                                                                    "settings.engineName",
                                                                )),
                                                        )
                                                        .children(name_input),
                                                )
                                                .child(
                                                    div()
                                                        .flex()
                                                        .flex_col()
                                                        .gap_2()
                                                        .child(
                                                            div()
                                                                .text_size(px(11.))
                                                                .font_weight(FontWeight(500.))
                                                                .text_color(rgb(
                                                                    palette.text_muted,
                                                                ))
                                                                .child(t!(
                                                                    "settings.engineUrl",
                                                                )),
                                                        )
                                                        .children(url_input)
                                                        .when(!has_placeholder, |this| {
                                                            this.child(
                                                                div()
                                                                    .text_size(px(10.))
                                                                    .text_color(rgb(
                                                                        palette.danger,
                                                                    ))
                                                                    .child(t!(
                                                                        "settings.engineUrlInvalid",
                                                                    )),
                                                            )
                                                        }),
                                                ),
                                        )
                                    })
                            })),
                    )
                }),
        )
    }
}

fn search_engine_icon_picker(
    palette: ThemePalette,
    index: usize,
    selected_icon: Option<&str>,
    clear_label: impl Into<SharedString>,
    cx: &mut Context<SettingsPanel>,
) -> impl IntoElement {
    let clear_label: SharedString = clear_label.into();
    div()
        .border_t_1()
        .border_color(rgb(palette.border))
        .bg(rgb(palette.bg))
        .px_3()
        .py_2()
        .grid()
        .grid_cols(6)
        .gap_1()
        .child(search_engine_icon_choice(
            palette,
            format!("settings-search-engine-icon-clear-{index}"),
            "icons/close.svg",
            palette.text_muted,
            clear_label.to_string(),
            selected_icon.is_none(),
            cx.listener(move |this, _, _, cx| {
                this.set_search_engine_icon(index, None, cx);
            }),
        ))
        .children(SEARCH_ENGINE_ICON_IDS.iter().map(|icon| {
            let icon_id = (*icon).to_string();
            let def = search_engine_icon(Some(icon), palette);
            search_engine_icon_choice(
                palette,
                format!("settings-search-engine-icon-{index}-{icon}"),
                def.path,
                def.tint(palette).unwrap_or(palette.text),
                (*icon).to_string(),
                selected_icon == Some(*icon),
                cx.listener(move |this, _, _, cx| {
                    this.set_search_engine_icon(index, Some(&icon_id), cx);
                }),
            )
        }))
}

fn search_engine_icon_choice(
    palette: ThemePalette,
    id: String,
    icon_path: &'static str,
    color: u32,
    tooltip: String,
    selected: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(SharedString::from(id))
        .h(px(28.))
        .rounded_sm()
        .border_1()
        .border_color(if selected {
            rgb(palette.primary)
        } else {
            rgb(palette.border)
        })
        .bg(if selected {
            rgb(palette.hover)
        } else {
            rgb(palette.input)
        })
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(|this| this.bg(rgb(palette.hover)))
        .tooltip(move |window, cx| NyaTooltip::new(tooltip.clone()).build(window, cx))
        .child(mono_icon(icon_path, rgb(color).into(), 15.))
        .on_click(on_click)
}

fn search_engine_settings_section(
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
                .items_start()
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

fn search_engine_text_button(
    palette: ThemePalette,
    id: impl Into<String>,
    icon_path: &'static str,
    label: impl Into<SharedString>,
    destructive: bool,
    enabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let label: SharedString = label.into();
    let hover = if destructive {
        rgba((palette.danger << 8) | 0x18)
    } else {
        rgb(palette.hover)
    };
    div()
        .id(SharedString::from(id.into()))
        .h(px(28.))
        .px_2()
        .rounded_sm()
        .flex()
        .items_center()
        .gap_1()
        .text_size(px(11.))
        .text_color(rgb(if destructive {
            palette.danger
        } else {
            palette.primary
        }))
        .opacity(if enabled { 1.0 } else { 0.45 })
        .child(
            svg()
                .size(px(14.))
                .path(icon_path)
                .text_color(rgb(if destructive {
                    palette.danger
                } else {
                    palette.primary
                })),
        )
        .child(label)
        .when(enabled, |this| {
            this.cursor_pointer()
                .hover(move |this| this.bg(hover))
                .on_click(on_click)
        })
}
