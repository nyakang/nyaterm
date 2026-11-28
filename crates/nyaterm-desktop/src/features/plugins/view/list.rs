use crate::features::plugins::management::ManagementAction;
use crate::features::plugins::native::native_error_message;
use crate::features::plugins::projection::{InstalledGroup, PluginEntry};
use crate::features::plugins::view::{PluginPanel, empty_hint};
use crate::theme::ThemePalette;
use gpui::{
    Context, Div, FontWeight, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled, div, prelude::FluentBuilder as _, px, rgb,
};
use nyaterm_core::plugins::management::{OperationPhase, PluginKey};
use nyaterm_core::plugins::native::NativeComponentId;
use nyaterm_plugin_host::native::NativeAction;
use nyaterm_plugin_host::service::PluginOperation;
use nyaterm_ui::icons::{color_icon, mono_icon};
use nyaterm_ui::presentation::progress_bar;
use nyaterm_ui::{
    NyaButton, NyaButtonVariant, NyaDropdownMenu, NyaMenuItem, NyaScrollable, NyaSearchInput,
    NyaTabItem, NyaTabs, NyaTabsVariant,
};
use rust_i18n::t;

pub(super) fn phase_label(phase: OperationPhase) -> String {
    t!(match phase {
        OperationPhase::Queued => "plugins.queued",
        OperationPhase::Checking => "plugins.checking",
        OperationPhase::Downloading => "plugins.downloading",
        OperationPhase::Verifying => "plugins.verifying",
        OperationPhase::Installing => "plugins.installing",
        OperationPhase::Committing => "plugins.finishing",
    })
    .to_string()
}

pub(super) fn status(entry: &PluginEntry, discover: bool) -> String {
    if let Some(progress) = &entry.progress {
        return phase_label(progress.phase);
    }
    t!(if entry.pending_uninstall {
        "plugins.nativePendingUninstall"
    } else if entry.busy {
        "plugins.working"
    } else if !entry.compatible {
        "plugins.incompatible"
    } else if entry.error.is_some() {
        "plugins.operationFailed"
    } else if entry.damaged {
        "plugins.loadFailed"
    } else if !entry.installed {
        "plugins.nativeMissing"
    } else if discover {
        "plugins.installed"
    } else if entry.enabled {
        "plugins.enabled"
    } else {
        "plugins.nativeDisabled"
    })
    .to_string()
}

pub(super) fn icon(entry: &PluginEntry, size: f32, palette: ThemePalette) -> gpui::AnyElement {
    if entry.icon().starts_with("color/") {
        color_icon(entry.icon(), size).into_any_element()
    } else {
        mono_icon(entry.icon(), rgb(palette.primary).into(), size).into_any_element()
    }
}

impl PluginPanel {
    pub(super) fn open_details(&self, key: PluginKey, tab: usize, cx: &mut Context<Self>) {
        let app = self.app.clone();
        cx.defer(move |cx| {
            let _ = app.update(cx, |app, cx| {
                app.open_plugin_details(key, tab, cx);
            });
        });
    }

    pub(super) fn manage(&self, action: ManagementAction, cx: &mut Context<Self>) {
        self.process
            .update(cx, |process, cx| process.manage(action, cx));
    }

    pub(super) fn install_native(&self, id: NativeComponentId, cx: &mut Context<Self>) {
        self.manage(ManagementAction::Native(id, NativeAction::Install), cx);
    }

    pub(super) fn install_entry(&self, key: &PluginKey, cx: &mut Context<Self>) {
        match key {
            PluginKey::Native(id) => self.install_native(*id, cx),
            PluginKey::Wasm(id) => self.manage(
                ManagementAction::Wasm(PluginOperation::InstallFromStore { id: id.clone() }),
                cx,
            ),
        }
    }

    pub(super) fn toggle_entry(&self, key: &PluginKey, enabled: bool, cx: &mut Context<Self>) {
        self.manage(
            match key {
                PluginKey::Native(id) => {
                    ManagementAction::Native(*id, NativeAction::Enable(enabled))
                }
                PluginKey::Wasm(id) => ManagementAction::Wasm(PluginOperation::Enable {
                    id: id.clone(),
                    enabled,
                }),
            },
            cx,
        );
    }

    pub(in crate::features) fn header_actions(&self, cx: &mut Context<Self>) -> Div {
        let busy = self.process.read(cx).job_busy("\0global");
        div()
            .flex()
            .items_center()
            .gap_1()
            .child(
                NyaButton::new("extensions-refresh", "")
                    .icon("icons/fe/refresh.svg")
                    .small()
                    .compact()
                    .variant(NyaButtonVariant::Ghost)
                    .tooltip(t!("plugins.checkAllUpdates"))
                    .loading(busy)
                    .disabled(busy)
                    .on_click(
                        cx.listener(|this, _, _, cx| this.manage(ManagementAction::Refresh, cx)),
                    ),
            )
            .child(
                NyaDropdownMenu::new("extensions-more")
                    .icon("icons/session/more.svg")
                    .tooltip(t!("common.more"))
                    .items([
                        NyaMenuItem::action(t!("plugins.checkAllUpdates"))
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.manage(ManagementAction::Refresh, cx)
                            })),
                        NyaMenuItem::action(t!("plugins.updateAll"))
                            .disabled(
                                busy || !super::super::projection::entries(self.process.read(cx))
                                    .iter()
                                    .any(|e| e.update_available),
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.manage(ManagementAction::UpdateAll, cx)
                            })),
                        NyaMenuItem::separator(),
                        NyaMenuItem::action(t!("plugins.importFile"))
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| this.browse(false, cx))),
                        NyaMenuItem::action(t!("plugins.openRoot")).on_click(cx.listener(
                            |this, _, _, cx| {
                                let app = this.app.clone();
                                cx.defer(move |cx| {
                                    let _ = app.update(cx, |app, cx| {
                                        cx.reveal_path(&app.runtime.data_dir().join("plugins"));
                                    });
                                });
                            },
                        )),
                        NyaMenuItem::action(t!("plugins.developerTools"))
                            .checked(self.developer_tools)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.developer_tools = !this.developer_tools;
                                cx.notify();
                            })),
                    ]),
            )
    }

    pub(super) fn toolbar(
        &self,
        count: usize,
        updates: usize,
        palette: ThemePalette,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut label = format!("{} {count}", t!("plugins.installed"));
        if updates > 0 {
            label.push_str(&format!(" · ↑{updates}"));
        }
        div()
            .w_full()
            .min_w_0()
            .flex_none()
            .p_2()
            .border_b_1()
            .border_color(rgb(palette.border))
            .flex()
            .flex_col()
            .gap_2()
            .child(
                NyaTabs::new("extensions-mode-tabs")
                    .variant(NyaTabsVariant::Segmented)
                    .full_width(true)
                    .selected_index(usize::from(!self.discover))
                    .item(NyaTabItem::new(t!("plugins.discover")))
                    .item(NyaTabItem::new(label))
                    .on_select(cx.listener(|this, index: &usize, _, cx| {
                        this.discover = *index == 0;
                        if this.discover && this.process.read(cx).store.is_none() {
                            this.manage(ManagementAction::Refresh, cx);
                        }
                        this.search.update(cx, |state, cx| {
                            state.set_placeholder(
                                t!(if this.discover {
                                    "plugins.search"
                                } else {
                                    "plugins.searchInstalled"
                                }),
                                cx,
                            );
                        });
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .w_full()
                    .h(px(32.))
                    .child(NyaSearchInput::new("extensions-search", &self.search)),
            )
    }

    pub(super) fn entry_actions(
        &self,
        entry: &PluginEntry,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let key = entry.key.clone();
        let id = key.id();
        let process = self.process.read(cx);
        let blocked = process.job_busy("\0global");
        let retry = process.jobs.get(&id).is_some_and(|job| job.error.is_some());
        let mut actions = div()
            .id(format!("actions-{id}"))
            .on_click(|_, _, cx| cx.stop_propagation())
            .flex_none()
            .flex()
            .items_center()
            .gap_1()
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation());
        if let Some(progress) = &entry.progress {
            actions = actions.child(
                NyaButton::new(format!("busy-{id}"), "")
                    .small()
                    .compact()
                    .loading(true)
                    .disabled(true),
            );
            if progress.phase.cancellable() {
                actions = actions.child(
                    NyaButton::new(format!("cancel-{id}"), "")
                        .icon("icons/close.svg")
                        .tooltip(t!("common.cancel"))
                        .small()
                        .compact()
                        .variant(NyaButtonVariant::Ghost)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            if let Some(service) = this.process.read(cx).service() {
                                match &key {
                                    PluginKey::Native(id) => service.native.cancel(*id),
                                    PluginKey::Wasm(id) => service.cancel_install(id),
                                }
                            }
                        })),
                );
            }
        } else if !entry.busy && !entry.pending_uninstall && entry.compatible {
            let label = if retry || entry.error.is_some() {
                Some(t!("plugins.retry"))
            } else if entry.damaged {
                Some(t!("plugins.nativeRepair"))
            } else if !entry.installed {
                Some(t!("plugins.install"))
            } else if entry.update_available {
                Some(t!("plugins.update"))
            } else if !entry.enabled {
                Some(t!("plugins.enable"))
            } else {
                None
            };
            if let Some(label) = label {
                let entry = entry.clone();
                actions = actions.child(
                    NyaButton::new(format!("primary-{id}"), label)
                        .small()
                        .compact()
                        .disabled(blocked)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            if retry {
                                this.process
                                    .update(cx, |process, cx| process.retry(&key.id(), cx));
                            } else if entry.installed
                                && !entry.enabled
                                && !entry.update_available
                                && !entry.damaged
                            {
                                this.toggle_entry(&key, true, cx);
                            } else if entry.damaged && matches!(key, PluginKey::Wasm(_)) {
                                this.manage(
                                    ManagementAction::Wasm(PluginOperation::Reload {
                                        id: key.id(),
                                    }),
                                    cx,
                                );
                            } else {
                                this.install_entry(&key, cx);
                            }
                        })),
                );
            }
        }
        actions.child(
            NyaDropdownMenu::new(format!("manage-{id}"))
                .icon("icons/session/more.svg")
                .tooltip(t!("plugins.manage"))
                .items(self.entry_menu(entry, cx)),
        )
    }

    pub(super) fn entry_menu(
        &self,
        entry: &PluginEntry,
        cx: &mut Context<Self>,
    ) -> Vec<NyaMenuItem> {
        let key = entry.key.clone();
        let mut items =
            vec![NyaMenuItem::action(t!("plugins.viewDetails")).on_click(
                cx.listener(move |this, _, _, cx| this.open_details(key.clone(), 0, cx)),
            )];
        let busy =
            entry.busy || entry.pending_uninstall || self.process.read(cx).job_busy("\0global");
        let key = entry.key.clone();
        items.push(
            NyaMenuItem::action(t!("plugins.checkUpdates"))
                .disabled(busy || !entry.compatible)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.manage(
                        match &key {
                            PluginKey::Native(id) => {
                                ManagementAction::Native(*id, NativeAction::Probe)
                            }
                            PluginKey::Wasm(_) => {
                                ManagementAction::Wasm(PluginOperation::RefreshStore)
                            }
                        },
                        cx,
                    )
                })),
        );
        if let PluginKey::Native(id) = entry.key {
            items.push(
                NyaMenuItem::action(t!("plugins.nativeImport"))
                    .disabled(busy || !entry.compatible)
                    .on_click(cx.listener(move |this, _, _, cx| this.browse_native(id, cx))),
            );
            if entry.rollback {
                items.push(
                    NyaMenuItem::action(t!("plugins.nativeRollback"))
                        .disabled(busy)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.native_operation(id, NativeAction::Rollback, cx)
                        })),
                );
            }
        } else if entry.installed {
            let id = entry.key.id();
            items.push(
                NyaMenuItem::action(t!("plugins.reload"))
                    .disabled(busy)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.operation(PluginOperation::Reload { id: id.clone() }, cx)
                    })),
            );
            let id = entry.key.id();
            items.push(
                NyaMenuItem::action(t!("plugins.importUpdate"))
                    .disabled(busy)
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.browse_update(id.clone(), cx)),
                    ),
            );
        }
        if entry.installed {
            let key = entry.key.clone();
            let enabled = entry.enabled;
            items.push(
                NyaMenuItem::action(t!(if enabled {
                    "plugins.disable"
                } else {
                    "plugins.enable"
                }))
                .disabled(busy)
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_entry(&key, !enabled, cx))),
            );
            if let Some(path) = entry.install_path.clone() {
                items.push(
                    NyaMenuItem::action(t!("plugins.openDirectory"))
                        .on_click(move |_, _, cx| cx.reveal_path(&path)),
                );
            }
            items.push(NyaMenuItem::separator());
            let key = entry.key.clone();
            items.push(
                NyaMenuItem::action(t!("plugins.uninstall"))
                    .danger()
                    .disabled(busy)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.manage(
                            match &key {
                                PluginKey::Native(id) => {
                                    ManagementAction::Native(*id, NativeAction::Uninstall)
                                }
                                PluginKey::Wasm(id) => {
                                    ManagementAction::Wasm(PluginOperation::Uninstall {
                                        id: id.clone(),
                                    })
                                }
                            },
                            cx,
                        )
                    })),
            );
        }
        items
    }

    fn entry_row(
        &self,
        entry: &PluginEntry,
        palette: ThemePalette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id = entry.key.id();
        let key = entry.key.clone();
        let open_key = key.clone();
        let tone = if entry.error.is_some() || entry.damaged {
            palette.danger
        } else if entry.busy || entry.pending_uninstall {
            palette.warning
        } else if entry.installed && entry.enabled {
            palette.success
        } else {
            palette.text_dimmed
        };
        let mut metadata = div()
            .flex()
            .items_center()
            .gap_1()
            .min_w_0()
            .text_size(px(11.))
            .text_color(rgb(palette.text_muted))
            .child(div().size(px(6.)).flex_none().rounded_full().bg(rgb(tone)))
            .child(div().truncate().child(status(entry, self.discover)));
        if !self.discover {
            if let Some(version) = &entry.installed_version {
                let version = if entry.update_available {
                    format!(
                        "· {version} → {}",
                        entry.available_version.as_deref().unwrap_or_default()
                    )
                } else {
                    format!("· {version}")
                };
                metadata = metadata.child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(rgb(palette.text_dimmed))
                        .child(version),
                );
            }
            if entry.in_use {
                metadata = metadata.child(
                    div()
                        .text_color(rgb(palette.text_muted))
                        .child(t!("plugins.inUse")),
                );
            }
        }
        let body = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(
                div()
                    .text_size(px(14.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(
                        NyaButton::new(format!("details-{id}"), entry.name.clone())
                            .text_size(px(14.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .small()
                            .compact()
                            .full_width()
                            .label_left()
                            .variant(NyaButtonVariant::Ghost)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.open_details(open_key.clone(), 0, cx);
                            })),
                    ),
            )
            .when(self.discover, |body| {
                body.child(
                    div()
                        .truncate()
                        .text_size(px(12.))
                        .text_color(rgb(palette.text_muted))
                        .child(entry.description.clone()),
                )
            })
            .child(metadata);
        let mut row = div()
            .id(format!("plugin-row-{id}"))
            .debug_selector(move || format!("plugin-row-{id}"))
            .w_full()
            .min_w_0()
            .min_h(px(if self.discover { 84. } else { 60. }))
            .flex_none()
            .px_2()
            .py(px(6.))
            .rounded_md()
            .hover(move |style| style.bg(rgb(palette.hover)))
            .cursor_pointer()
            .flex()
            .flex_col()
            .gap_1()
            .on_click(cx.listener(move |this, _, _, cx| this.open_details(key.clone(), 0, cx)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .w_full()
                    .min_w_0()
                    .child(
                        div()
                            .size(px(36.))
                            .flex_none()
                            .rounded_md()
                            .bg(rgb(palette.surface_elevated))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(icon(entry, 22., palette)),
                    )
                    .child(body)
                    .child(self.entry_actions(entry, cx)),
            );
        if let Some(progress) = &entry.progress
            && progress.phase == OperationPhase::Downloading
        {
            row = row.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().flex_1().when_some(progress.ratio(), |view, ratio| {
                        view.child(progress_bar(palette, ratio))
                    }))
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(palette.text_muted))
                            .child(
                                progress
                                    .ratio()
                                    .map(|ratio| format!("{:.0}%", ratio * 100.))
                                    .unwrap_or_else(|| {
                                        format!("{:.1} MiB", progress.downloaded as f64 / 1048576.)
                                    }),
                            ),
                    ),
            );
        }
        if let Some(error) = &entry.error {
            row = row.child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(palette.danger))
                    .child(native_error_message(error)),
            );
        }
        row
    }

    pub(super) fn render_list(
        &self,
        entries: &[PluginEntry],
        query: &str,
        palette: ThemePalette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mut list = div()
            .id("plugins-scroll")
            .flex_1()
            .min_h_0()
            .min_w_0()
            .p_2()
            .flex()
            .flex_col()
            .gap_1();
        if self.developer_tools {
            list = list.child(
                NyaButton::new("development-load", t!("plugins.development"))
                    .small()
                    .full_width()
                    .on_click(cx.listener(|this, _, _, cx| this.browse(true, cx))),
            );
        }
        let process = self.process.read(cx);
        if let Some(error) = process
            .snapshot
            .startup_error
            .as_ref()
            .or_else(|| process.jobs.get("\0global").and_then(|j| j.error.as_ref()))
        {
            list = list.child(
                div()
                    .text_xs()
                    .text_color(rgb(palette.danger))
                    .child(native_error_message(error)),
            );
        }
        if process
            .jobs
            .get("\0global")
            .is_some_and(|job| job.error.is_some())
        {
            list = list.child(
                NyaButton::new("retry-management", t!("plugins.retry"))
                    .small()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.process
                            .update(cx, |process, cx| process.retry("\0global", cx));
                    })),
            );
        }
        if let Some((succeeded, failed)) = process.batch_summary {
            list = list.child(
                div()
                    .text_xs()
                    .text_color(rgb(palette.text_muted))
                    .child(t!(
                        "plugins.batchSummary",
                        succeeded = succeeded.to_string(),
                        failed = failed.to_string()
                    )),
            );
        }
        let matching: Vec<_> = entries
            .iter()
            .filter(|entry| {
                entry.matches(query)
                    && (self.discover || entry.installed || entry.busy || entry.error.is_some())
            })
            .collect();
        if matching.is_empty() {
            list = list.child(empty_hint(
                "icons/extensions.svg",
                t!(if query.is_empty() {
                    "plugins.empty"
                } else {
                    "plugins.noMatches"
                })
                .to_string(),
                palette,
            ));
        }
        let groups: Vec<_> = if self.discover {
            vec![
                (
                    t!("plugins.nativeTitle").to_string(),
                    matching
                        .iter()
                        .copied()
                        .filter(|e| matches!(e.key, PluginKey::Native(_)))
                        .collect::<Vec<_>>(),
                ),
                (
                    t!("plugins.extensions").to_string(),
                    matching
                        .iter()
                        .copied()
                        .filter(|e| matches!(e.key, PluginKey::Wasm(_)))
                        .collect(),
                ),
            ]
        } else {
            InstalledGroup::ALL
                .into_iter()
                .map(|group| {
                    (
                        group.label(),
                        matching
                            .iter()
                            .copied()
                            .filter(|e| e.group() == group)
                            .collect(),
                    )
                })
                .collect()
        };
        for (label, rows) in groups {
            if rows.is_empty() {
                continue;
            }
            let update_group = !self.discover && rows[0].group() == InstalledGroup::Updates;
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .pt_2()
                    .pb_1()
                    .text_size(px(12.))
                    .text_color(rgb(palette.text_muted))
                    .child(div().flex_1().child(label))
                    .child(rows.len().to_string())
                    .when(update_group, |header| {
                        header.child(
                            NyaButton::new("plugins-update-all", t!("plugins.updateAll"))
                                .small()
                                .compact()
                                .disabled(self.process.read(cx).job_busy("\0global"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.manage(ManagementAction::UpdateAll, cx)
                                })),
                        )
                    }),
            );
            for row in rows {
                list = list.child(self.entry_row(row, palette, cx));
            }
        }
        list.overflow_y_scrollbar()
    }
}
