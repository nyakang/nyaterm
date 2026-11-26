use crate::features::plugins::native::{
    native_description, native_display_name, native_error_message,
};
use crate::features::plugins::view::{PendingKind, PluginPanel};
use crate::theme::ThemePalette;
use gpui::{
    Context, Div, FontWeight, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled, div, prelude::FluentBuilder as _, px, rgb, svg,
};
use nyaterm_core::plugins::native::{NativeComponentId, NativeStatus};
use nyaterm_plugin_host::manager::PluginSnapshot;
use nyaterm_plugin_host::native::NativeAction;
use nyaterm_plugin_host::service::PluginOperation;
use nyaterm_ui::icons::{color_icon, mono_icon};
use nyaterm_ui::presentation::progress_bar;
use nyaterm_ui::{
    NyaButton, NyaButtonVariant, NyaDropdownMenu, NyaMenuItem, NyaSearchInput, NyaTabItem, NyaTabs,
    NyaTabsVariant,
};
use rust_i18n::t;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum RowTone {
    Neutral,
    Active,
    Warning,
    Danger,
}

impl RowTone {
    fn dot_color(self, palette: ThemePalette) -> u32 {
        match self {
            Self::Neutral => palette.text_dimmed,
            Self::Active => palette.success,
            Self::Warning => palette.warning,
            Self::Danger => palette.danger,
        }
    }

    fn text_color(self, palette: ThemePalette) -> u32 {
        match self {
            Self::Neutral | Self::Active => palette.text_muted,
            Self::Warning => palette.warning,
            Self::Danger => palette.danger,
        }
    }
}

pub(super) struct ExtensionRowSpec {
    pub id: String,
    pub icon: &'static str,
    pub name: String,
    pub description: String,
    pub metadata: String,
    pub tone: Option<RowTone>,
    pub progress: Option<f64>,
    pub selected: bool,
}

pub(super) fn matches_query(query: &str, fields: &[&str]) -> bool {
    fields
        .iter()
        .any(|field| field.to_lowercase().contains(query))
}

pub(super) fn matches_plugin(plugin: &PluginSnapshot, query: &str) -> bool {
    matches_query(query, &[&plugin.id])
        || plugin.manifest.as_ref().is_some_and(|manifest| {
            matches_query(
                query,
                &[
                    &manifest.name,
                    &manifest.description,
                    &manifest.authors.join(" "),
                ],
            )
        })
}

fn count_badge(count: usize, palette: ThemePalette) -> Div {
    div()
        .px(px(6.))
        .py(px(1.))
        .rounded_full()
        .border_1()
        .border_color(rgb(palette.border))
        .bg(rgb(palette.surface_elevated))
        .text_size(px(11.))
        .text_color(rgb(palette.text_muted))
        .child(count.to_string())
}

pub(super) fn section(title: String, count: usize, palette: ThemePalette) -> Div {
    div()
        .w_full()
        .flex_none()
        .px_2()
        .pt_2()
        .pb_1()
        .flex()
        .items_center()
        .gap_2()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(palette.text_muted))
        .child(div().flex_1().min_w_0().truncate().child(title))
        .child(count_badge(count, palette))
}

fn collapsible_section(
    id: &'static str,
    title: String,
    count: usize,
    collapsed: bool,
    palette: ThemePalette,
    on_toggle: impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> gpui::Stateful<Div> {
    let chevron = if collapsed {
        "icons/chevron-down.svg"
    } else {
        "icons/chevron-up.svg"
    };
    div()
        .id(id)
        .w_full()
        .flex_none()
        .px_2()
        .py_1()
        .mt_1()
        .rounded_sm()
        .cursor_pointer()
        .hover(move |style| style.bg(rgb(palette.hover)))
        .flex()
        .items_center()
        .gap_1()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(palette.text_muted))
        .on_click(on_toggle)
        .child(
            svg()
                .size(px(14.))
                .flex_none()
                .path(chevron)
                .text_color(rgb(palette.text_dimmed)),
        )
        .child(div().flex_1().min_w_0().truncate().child(title))
        .child(count_badge(count, palette))
}

pub(super) fn extension_row(
    spec: ExtensionRowSpec,
    palette: ThemePalette,
    action: impl IntoElement,
) -> impl IntoElement {
    let row_id = spec.id.clone();
    let selector_id = spec.id.clone();
    let action_id = format!("{}-actions", spec.id);
    let image = if spec.icon.starts_with("color/") {
        color_icon(spec.icon, 20.).into_any_element()
    } else {
        mono_icon(spec.icon, rgb(palette.primary).into(), 20.).into_any_element()
    };
    let meta_color = spec
        .tone
        .map_or(palette.text_muted, |tone| tone.text_color(palette));

    let mut status_cell = div().flex_1().min_w_0().flex().items_center().gap(px(6.));
    if let Some(tone) = spec.tone {
        status_cell = status_cell.child(
            div()
                .w(px(6.))
                .h(px(6.))
                .flex_none()
                .rounded_full()
                .bg(rgb(tone.dot_color(palette))),
        );
    }
    status_cell = status_cell.child(
        div()
            .flex_1()
            .min_w_0()
            .truncate()
            .text_xs()
            .text_color(rgb(meta_color))
            .child(spec.metadata),
    );

    let mut body = div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .truncate()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(palette.text))
                .child(spec.name),
        )
        .when(!spec.description.is_empty(), |col| {
            col.child(
                div()
                    .truncate()
                    .text_xs()
                    .text_color(rgb(palette.text_muted))
                    .child(spec.description),
            )
        })
        .child(
            div()
                .min_w_0()
                .flex()
                .items_center()
                .gap_1()
                .child(status_cell)
                .child(
                    div()
                        .debug_selector(move || action_id.clone())
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(action),
                ),
        );

    if let Some(ratio) = spec.progress {
        body = body.child(progress_bar(palette, ratio));
    }

    div()
        .id(row_id)
        .debug_selector(move || selector_id.clone())
        .w_full()
        .min_w_0()
        .flex_none()
        .px_2()
        .py_2()
        .rounded_md()
        .border_1()
        .border_color(if spec.selected {
            rgb(palette.border).into()
        } else {
            gpui::transparent_black()
        })
        .when(spec.selected, |row| row.bg(rgb(palette.surface_elevated)))
        .hover(move |style| style.bg(rgb(palette.hover)))
        .flex()
        .items_start()
        .gap(px(10.))
        .child(
            div()
                .mt(px(2.))
                .w(px(36.))
                .h(px(36.))
                .flex_none()
                .rounded_md()
                .border_1()
                .border_color(rgb(palette.border))
                .bg(rgb(palette.surface_elevated))
                .flex()
                .items_center()
                .justify_center()
                .child(image),
        )
        .child(body)
}

impl PluginPanel {
    pub(super) fn install_native(&self, id: NativeComponentId, cx: &mut Context<Self>) {
        let app = self.app.clone();
        // Acquiring the service reads this panel through NyaTermApp. Release the
        // click listener's panel update before entering that application flow.
        cx.defer(move |cx| {
            let _ = app.update(cx, |app, cx| {
                app.ensure_native_component(
                    id,
                    |_| true,
                    |app, result, cx| {
                        if let Err(error) = result {
                            app.shell.set_status(error.message);
                        }
                        cx.notify();
                    },
                    cx,
                );
            });
        });
    }

    pub(super) fn toolbar(
        &self,
        installed_count: usize,
        palette: ThemePalette,
        cx: &mut Context<Self>,
    ) -> Div {
        let installed_label = if installed_count > 0 {
            format!("{} ({installed_count})", t!("plugins.installed"))
        } else {
            t!("plugins.installed").to_string()
        };
        let selected_tab = if self.discover { 0 } else { 1 };

        div()
            .w_full()
            .min_w_0()
            .flex_none()
            .px(px(10.))
            .pt_2()
            .pb_2()
            .border_b_1()
            .border_color(rgb(palette.border))
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        div().flex_1().min_w_0().child(
                            NyaTabs::new("extensions-mode-tabs")
                                .variant(NyaTabsVariant::Segmented)
                                .selected_index(selected_tab)
                                .item(
                                    NyaTabItem::new(t!("plugins.discover"))
                                        .accessible_label(t!("plugins.discover")),
                                )
                                .item(
                                    NyaTabItem::new(installed_label)
                                        .accessible_label(t!("plugins.installed")),
                                )
                                .on_select(cx.listener(|this, index: &usize, _, cx| {
                                    this.close_workbench_if_idle(cx);
                                    if *index == 0 {
                                        this.discover = true;
                                        if !this.busy() {
                                            this.operation(PluginOperation::RefreshStore, cx);
                                        }
                                    } else {
                                        this.discover = false;
                                    }
                                    cx.notify();
                                })),
                        ),
                    )
                    .child(
                        NyaButton::new("extensions-refresh", "")
                            .icon("icons/fe/refresh.svg")
                            .small()
                            .compact()
                            .variant(NyaButtonVariant::Ghost)
                            .tooltip(t!("common.refresh"))
                            .loading(
                                self.pending
                                    .as_ref()
                                    .is_some_and(|pending| pending.kind == PendingKind::Management),
                            )
                            .disabled(self.busy())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.operation(PluginOperation::RefreshStore, cx)
                            })),
                    )
                    .child(
                        NyaDropdownMenu::new("extensions-more")
                            .icon("icons/session/more.svg")
                            .tooltip(t!("common.more"))
                            .items([
                                NyaMenuItem::action(t!("plugins.importFile"))
                                    .disabled(self.busy())
                                    .on_click(cx.listener(|this, _, _, cx| this.browse(false, cx))),
                                NyaMenuItem::action(t!("plugins.developerTools"))
                                    .checked(self.developer_tools)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.developer_tools = !this.developer_tools;
                                        cx.notify();
                                    })),
                            ]),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .h(px(30.))
                    .flex_none()
                    .child(NyaSearchInput::new("extensions-search", &self.search).compact()),
            )
    }

    pub(super) fn native_list(
        &self,
        query: &str,
        palette: ThemePalette,
        cx: &mut Context<Self>,
    ) -> (usize, Div) {
        let mut list = div()
            .w_full()
            .min_w_0()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(2.));
        let Some(service) = self.process.read(cx).service() else {
            return (0, list);
        };
        let native_title = t!("plugins.nativeTitle").to_string();
        let components: Vec<_> = service
            .native
            .snapshot()
            .into_iter()
            .filter(|component| {
                let display_name = native_display_name(component.id);
                let description = native_description(component.id);
                matches_query(
                    query,
                    &[
                        component.id.title(),
                        &display_name,
                        &description,
                        "NyaTerm",
                        &native_title,
                    ],
                )
            })
            .collect();
        let count = components.len();
        if count == 0 && !query.is_empty() {
            return (0, list);
        }

        let collapsed = self.native_collapsed && query.is_empty();
        list = list.child(collapsible_section(
            "native-components-header",
            native_title,
            count,
            collapsed,
            palette,
            cx.listener(|this, _, _, cx| {
                this.native_collapsed = !this.native_collapsed;
                cx.notify();
            }),
        ));
        if collapsed {
            return (count, list);
        }

        for component in components {
            let id = component.id;
            let installing = component.status == NativeStatus::Installing;
            let installed = !matches!(
                component.status,
                NativeStatus::Missing | NativeStatus::Installing
            );
            let enabled = !matches!(
                component.status,
                NativeStatus::Disabled | NativeStatus::PendingUninstall
            );
            let downloadable = matches!(
                component.status,
                NativeStatus::Missing
                    | NativeStatus::NeedsUpdate
                    | NativeStatus::UpdateAvailable
                    | NativeStatus::Damaged
            );
            let label = t!(match component.status {
                NativeStatus::Missing => "plugins.nativeMissing",
                NativeStatus::Installing => "plugins.working",
                NativeStatus::Ready => "plugins.enabled",
                NativeStatus::UpdateAvailable => "plugins.update",
                NativeStatus::Disabled => "plugins.nativeDisabled",
                NativeStatus::NeedsUpdate => "plugins.nativeNeedsUpdate",
                NativeStatus::Damaged => "plugins.loadFailed",
                NativeStatus::PendingUninstall => "plugins.nativePendingUninstall",
            })
            .to_string();
            let tone = match component.status {
                NativeStatus::Ready => RowTone::Active,
                NativeStatus::Installing
                | NativeStatus::NeedsUpdate
                | NativeStatus::UpdateAvailable
                | NativeStatus::PendingUninstall => RowTone::Warning,
                NativeStatus::Damaged => RowTone::Danger,
                NativeStatus::Missing | NativeStatus::Disabled => RowTone::Neutral,
            };

            let mut actions = div().flex_none().flex().items_center().gap_1();
            if downloadable {
                actions = actions.child(
                    NyaButton::new(
                        format!("native-install-{}", id.slug()),
                        t!(match component.status {
                            NativeStatus::Missing => "plugins.install",
                            NativeStatus::Damaged => "plugins.nativeRepair",
                            _ => "plugins.update",
                        }),
                    )
                    .small()
                    .compact()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if component.status == NativeStatus::UpdateAvailable {
                            this.native_operation(id, NativeAction::Install, cx);
                        } else {
                            this.install_native(id, cx);
                        }
                    })),
                );
            }
            if installing {
                actions = actions.child(
                    NyaButton::new(format!("native-cancel-{}", id.slug()), t!("common.cancel"))
                        .small()
                        .compact()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(service) = this.process.read(cx).service() {
                                service.native.cancel(id);
                            }
                        })),
                );
            }
            let mut items = vec![
                NyaMenuItem::action(t!("plugins.nativeImport"))
                    .disabled(installing)
                    .on_click(cx.listener(move |this, _, _, cx| this.browse_native(id, cx))),
            ];
            items.push(
                NyaMenuItem::action(t!("tray.update"))
                    .disabled(installing)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.native_operation(id, NativeAction::Probe, cx)
                    })),
            );
            if component.rollback_available {
                items.push(
                    NyaMenuItem::action(t!("plugins.nativeRollback"))
                        .disabled(installing)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.native_operation(id, NativeAction::Rollback, cx)
                        })),
                );
            }
            if installed {
                items.push(
                    NyaMenuItem::action(t!(if enabled {
                        "plugins.disable"
                    } else {
                        "plugins.enable"
                    }))
                    .disabled(component.status == NativeStatus::PendingUninstall)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.native_operation(id, NativeAction::Enable(!enabled), cx)
                    })),
                );
                items.push(NyaMenuItem::separator());
                items.push(
                    NyaMenuItem::action(t!("plugins.uninstall"))
                        .danger()
                        .disabled(component.status == NativeStatus::PendingUninstall)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.native_operation(id, NativeAction::Uninstall, cx)
                        })),
                );
            }
            actions = actions.child(
                NyaDropdownMenu::new(format!("native-manage-{}", id.slug()))
                    .icon("icons/session/more.svg")
                    .tooltip(t!("plugins.manage"))
                    .disabled(installing)
                    .items(items),
            );
            let icon = match id {
                NativeComponentId::Spice => "icons/conn/spice.svg",
                NativeComponentId::Rdp => "icons/conn/rdp.svg",
                NativeComponentId::Vnc => "icons/conn/vnc.svg",
                NativeComponentId::Android => "color/os/android.svg",
            };
            let metadata = if installing {
                format!(
                    "{:.1} / {} MiB",
                    component.downloaded as f64 / 1048576.,
                    component
                        .download_size
                        .map(|size| format!("{:.1}", size as f64 / 1048576.))
                        .unwrap_or_else(|| "?".into())
                )
            } else if let Some(size) = component
                .download_size
                .filter(|_| component.status == NativeStatus::Missing)
            {
                format!("{label} · {:.1} MiB", size as f64 / 1048576.)
            } else {
                label
            };
            let metadata = match (&component.installed_version, &component.available_version) {
                (Some(installed), Some(available))
                    if component.status == NativeStatus::UpdateAvailable =>
                {
                    format!("{metadata} · {installed} → {available}")
                }
                (Some(installed), _) => format!("{metadata} · {installed}"),
                _ => metadata,
            };
            let progress = if installing {
                component
                    .download_size
                    .filter(|total| *total > 0)
                    .map(|total| component.downloaded as f64 / total as f64)
            } else {
                None
            };
            list = list.child(extension_row(
                ExtensionRowSpec {
                    id: format!("native-row-{}", id.slug()),
                    icon,
                    name: native_display_name(id),
                    description: native_description(id),
                    metadata,
                    tone: Some(tone),
                    progress,
                    selected: false,
                },
                palette,
                actions,
            ));
            if let Some(error) = component.error {
                list = list.child(
                    div()
                        .mx_2()
                        .mb_1()
                        .px_2()
                        .py_1()
                        .rounded_sm()
                        .border_1()
                        .border_color(rgb(palette.danger))
                        .text_xs()
                        .text_color(rgb(palette.danger))
                        .child(native_error_message(&error)),
                );
            }
        }
        (count, list)
    }
}
