use gpui::{Context, IntoElement, div, prelude::*, px, rgb};
use nyaterm_core::connection_tree::NodeKind;
use nyaterm_core::models::sessions::{Group, SavedConnection};
use nyaterm_core::models::startup::{StartupMode, supports_automatic_start};
use nyaterm_ui::{
    NYA_FORM_CONTROL_HEIGHT_PX, NyaRadioGroup, NyaScrollable, NyaSelect, NyaSelectOption,
};
use rust_i18n::t;
use std::collections::HashMap;

use crate::features::pages::settings::panel::SettingsPanel;
use crate::features::pages::settings::settings_button;

use super::super::{settings_form_row, settings_form_section, settings_switch};

pub(in crate::features::pages::settings) fn startup_options(
    groups: &[Group],
    connections: &[SavedConnection],
) -> Vec<NyaSelectOption> {
    let mut paths = HashMap::new();
    groups
        .iter()
        .map(|group| {
            (
                "group",
                group.id.as_str(),
                group.name.as_str(),
                group.parent_id.as_deref(),
                false,
            )
        })
        .chain(connections.iter().map(|connection| {
            (
                "connection",
                connection.id.as_str(),
                connection.name.as_str(),
                connection.group_id.as_deref(),
                !supports_automatic_start(&connection.config),
            )
        }))
        .map(|(kind, id, name, parent, disabled)| {
            let label = if kind == "group" {
                t!("settings.startupGroup")
            } else {
                t!("settings.startupConnection")
            };
            let path = paths.entry(parent).or_insert_with(|| {
                nyaterm_core::assets::build_group_path(groups, parent)
                    .into_iter()
                    .filter(|segment| segment.id.is_some())
                    .map(|segment| segment.name)
                    .collect::<Vec<_>>()
                    .join(" / ")
            });
            let title = if path.is_empty() {
                format!("{label} · {name}")
            } else {
                format!("{label} · {path} / {name}")
            };
            let subtitle = if disabled {
                t!("settings.startupUnsupported").to_string()
            } else {
                path.clone()
            };
            NyaSelectOption::new(format!("{kind}:{id}"), title.clone())
                .search_text(format!("{title} {path}"))
                .subtitle(subtitle)
                .disabled(disabled)
        })
        .collect()
}

impl SettingsPanel {
    fn startup_select_control(
        &mut self,
        id: String,
        options: Vec<NyaSelectOption>,
        selected: Option<String>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let select = self.select_entity(id.into(), options, selected, false, cx);
        select.update(cx, |select, cx| select.set_searchable(true, cx));
        div()
            .w_full()
            .h(px(NYA_FORM_CONTROL_HEIGHT_PX))
            .child(NyaSelect::new(&select))
    }

    pub(in crate::features::pages::settings) fn startup_settings_section(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let mode = self.settings.summary().startup_mode.clone();
        let selected = match mode {
            StartupMode::None => Some(0),
            StartupMode::LastSession => Some(1),
            StartupMode::Specified => Some(2),
            StartupMode::Unknown(_) => None,
        };
        let radio = NyaRadioGroup::new("settings.startup.mode")
            .items([
                t!("settings.startupNone"),
                t!("settings.startupLast"),
                t!("settings.startupSpecified"),
            ])
            .selected_index(selected)
            .on_select(cx.listener(|this, index, _, cx| {
                let mode = match index {
                    1 => StartupMode::LastSession,
                    2 => StartupMode::Specified,
                    _ => StartupMode::None,
                };
                this.with_app(cx, |app, cx| app.set_startup_mode(mode, cx));
            }));
        let mut content = div().flex().flex_col().gap_3().child(radio);
        if mode == StartupMode::LastSession {
            content = content.child(settings_form_row(
                palette,
                t!("settings.startupRestoreWindowLayout"),
                None,
                settings_switch(
                    palette,
                    "general-startup-restore-window-layout",
                    self.settings.summary().startup_restore_window_layout,
                    cx.listener(|this, _, _, cx| this.toggle_startup_restore_window_layout(cx)),
                ),
            ));
        } else if mode == StartupMode::Specified {
            let targets = self.settings.summary().startup_targets.clone();
            let options = self.settings.startup_options.to_vec();
            let mut rows = div().flex().flex_col().gap_2().max_h(px(280.));
            for (index, target) in targets.iter().enumerate() {
                let value = target
                    .reference()
                    .map(|(kind, id)| {
                        format!(
                            "{}:{id}",
                            if kind == NodeKind::Group {
                                "group"
                            } else {
                                "connection"
                            }
                        )
                    })
                    .unwrap_or_else(|| format!("unavailable:{index}"));
                let mut row_options = options.clone();
                if !row_options.iter().any(|option| option.value() == value) {
                    row_options.push(
                        NyaSelectOption::new(value.clone(), t!("settings.startupMissing"))
                            .disabled(true),
                    );
                }
                let selector = self.startup_select_control(
                    format!("settings.startup.target.{index}"),
                    row_options,
                    Some(value),
                    cx,
                );
                let mut row = div()
                    .flex()
                    .items_center()
                    .flex_shrink_0()
                    .gap_2()
                    .child(div().flex_1().min_w_0().child(selector));
                if index > 0 {
                    row = row.child(settings_button(
                        format!("startup-up-{index}"),
                        t!("settings.startupMoveUp"),
                        cx.listener(move |this, _, _, cx| {
                            this.with_app(cx, |app, cx| {
                                app.move_startup_target(index, index - 1, cx)
                            });
                        }),
                    ));
                }
                if index + 1 < targets.len() {
                    row = row.child(settings_button(
                        format!("startup-down-{index}"),
                        t!("settings.startupMoveDown"),
                        cx.listener(move |this, _, _, cx| {
                            this.with_app(cx, |app, cx| {
                                app.move_startup_target(index, index + 1, cx)
                            });
                        }),
                    ));
                }
                rows = rows.child(row.child(settings_button(
                    format!("startup-remove-{index}"),
                    t!("common.delete"),
                    cx.listener(move |this, _, _, cx| {
                        this.with_app(cx, |app, cx| app.remove_startup_target(index, cx));
                    }),
                )));
            }
            if targets.is_empty() {
                content = content.child(t!("settings.startupEmpty").to_string());
            }
            content = content
                .child(rows.overflow_y_scrollbar())
                .child(settings_form_row(
                    palette,
                    t!("settings.startupAdd"),
                    None,
                    self.startup_select_control(
                        format!("settings.startup.target.{}", targets.len()),
                        options,
                        None,
                        cx,
                    ),
                ))
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(palette.text_dimmed))
                        .child(t!("settings.startupTargetsHint").to_string()),
                );
        } else if matches!(mode, StartupMode::Unknown(_)) {
            content = content.child(t!("settings.startupUnknown").to_string());
        }
        settings_form_section(
            palette,
            Some(t!("settings.startupTitle")),
            Some(t!("settings.startupHint")),
            content,
        )
    }
}
