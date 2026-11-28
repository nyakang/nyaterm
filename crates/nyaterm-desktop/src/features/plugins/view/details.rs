use gpui::{
    Context, Div, FontWeight, InteractiveElement, IntoElement, ParentElement, Styled, div, px, rgb,
};
use nyaterm_core::plugins::management::PluginKey;
use nyaterm_ui::{NyaButton, NyaButtonVariant, NyaScrollable, NyaTabItem, NyaTabs, NyaTabsVariant};
use rust_i18n::t;

use crate::features::plugins::projection::entries;
use crate::features::plugins::view::{PluginPanel, list};
use crate::theme::ThemePalette;

impl PluginPanel {
    pub(super) fn render_details(
        &self,
        palette: ThemePalette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let key = self.detail_key.as_ref().expect("detail identity");
        let entry = entries(self.process.read(cx))
            .into_iter()
            .find(|entry| &entry.key == key);
        let Some(entry) = entry else {
            return div()
                .size_full()
                .p_4()
                .child(t!("plugins.nativeMissing"))
                .into_any_element();
        };
        let snapshot = self.process.read(cx).snapshot.clone();
        let plugin = snapshot.plugins.iter().find(|plugin| plugin.id == key.id());
        let has_tools = matches!(key, PluginKey::Wasm(_)) && entry.installed;
        let detail_tab = self.detail_tab.min(if has_tools { 3 } else { 1 });
        let mut tabs = NyaTabs::new("plugin-detail-tabs")
            .variant(NyaTabsVariant::Underline)
            .selected_index(detail_tab)
            .item(NyaTabItem::new(t!("plugins.overview")))
            .item(NyaTabItem::new(t!("plugins.information")));
        if has_tools {
            tabs = tabs
                .item(NyaTabItem::new(t!("plugins.permissionsSection")))
                .item(NyaTabItem::new(t!("plugins.tools")));
        }
        tabs = tabs.on_select(cx.listener(|this, index: &usize, _, cx| {
            this.detail_tab = *index;
            cx.notify();
        }));
        let mut actions = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .child(self.entry_actions(&entry, cx));
        if entry.installed && !entry.busy && !entry.pending_uninstall {
            if entry.enabled {
                let key = key.clone();
                actions = actions.child(
                    NyaButton::new("detail-disable", t!("plugins.disable"))
                        .small()
                        .variant(NyaButtonVariant::Secondary)
                        .disabled(self.process.read(cx).job_busy("\0global"))
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.toggle_entry(&key, false, cx)),
                        ),
                );
            }
            let key = key.clone();
            actions = actions.child(
                NyaButton::new("detail-uninstall", t!("plugins.uninstall"))
                    .small()
                    .variant(NyaButtonVariant::Ghost)
                    .disabled(self.process.read(cx).job_busy("\0global"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.manage(
                            match &key {
                                PluginKey::Native(id) => {
                                    crate::features::plugins::management::ManagementAction::Native(
                                        *id,
                                        nyaterm_plugin_host::native::NativeAction::Uninstall,
                                    )
                                }
                                PluginKey::Wasm(id) => {
                                    crate::features::plugins::management::ManagementAction::Wasm(
                                        nyaterm_plugin_host::service::PluginOperation::Uninstall {
                                            id: id.clone(),
                                        },
                                    )
                                }
                            },
                            cx,
                        )
                    })),
            );
        }
        let mut body = div().flex().flex_col().gap_4().w_full().min_w_0();
        if detail_tab == 0 {
            body = body
                .child(
                    div()
                        .text_size(px(16.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(t!("plugins.about")),
                )
                .child(entry.description.clone());
        } else if detail_tab == 1 {
            for (label, value) in [
                (
                    t!("plugins.installedVersion").to_string(),
                    entry.installed_version.clone(),
                ),
                (
                    t!("plugins.availableVersion").to_string(),
                    entry.available_version.clone(),
                ),
                (
                    t!("plugins.currentState").to_string(),
                    Some(list::status(&entry, false)),
                ),
                (
                    t!("plugins.componentType").to_string(),
                    Some(
                        t!(if matches!(key, PluginKey::Native(_)) {
                            "plugins.nativeType"
                        } else {
                            "plugins.wasmType"
                        })
                        .to_string(),
                    ),
                ),
                (
                    t!("plugins.publisher").to_string(),
                    Some(entry.publisher.clone()),
                ),
                (
                    t!("plugins.installDirectory").to_string(),
                    entry
                        .install_path
                        .as_ref()
                        .map(|path| path.display().to_string()),
                ),
                (
                    t!("plugins.downloadSize").to_string(),
                    entry
                        .download_size
                        .map(|size| format!("{:.1} MiB", size as f64 / 1048576.)),
                ),
                (
                    t!("plugins.compatibility").to_string(),
                    Some(if entry.compatible {
                        t!("plugins.compatible").to_string()
                    } else {
                        t!("plugins.incompatible").to_string()
                    }),
                ),
                (
                    t!("plugins.sourceLabel").to_string(),
                    Some(if entry.development {
                        t!("plugins.development").to_string()
                    } else {
                        t!("plugins.managed").to_string()
                    }),
                ),
            ] {
                if let Some(value) = value.filter(|value| !value.is_empty()) {
                    body = body.child(detail_field(label, value, palette));
                }
            }
            if let Some(manifest) = plugin.and_then(|plugin| plugin.manifest.as_ref()) {
                body = body
                    .child(detail_field(
                        "API".into(),
                        manifest.api_version.clone(),
                        palette,
                    ))
                    .child(detail_field(
                        "NyaTerm".into(),
                        manifest.host_version.clone(),
                        palette,
                    ));
            }
        } else if let Some(plugin) = plugin {
            body = body.child(self.render_plugin_card(plugin, &snapshot, palette, cx));
            if detail_tab == 3 {
                if self.selected.is_some() || self.preview.is_some() {
                    body = body.child(self.render_workbench(palette, cx));
                }
                body = body.child(self.render_tool_panels(&snapshot, palette, cx));
            }
        }
        if let Some(error) = &entry.error {
            body = body.child(
                div()
                    .text_color(rgb(palette.danger))
                    .child(super::super::native::native_error_message(error)),
            );
        }
        div()
            .id("plugin-details")
            .track_focus(&self.focus)
            .size_full()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(rgb(palette.surface))
            .text_color(rgb(palette.text))
            .text_sm()
            .child(
                div()
                    .p_4()
                    .flex()
                    .items_start()
                    .gap_4()
                    .child(
                        div()
                            .size(px(72.))
                            .flex_none()
                            .rounded_lg()
                            .bg(rgb(palette.surface_elevated))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(list::icon(&entry, 40., palette)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(px(24.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(entry.name.clone()),
                            )
                            .child(div().text_color(rgb(palette.text_muted)).child(format!(
                                    "{} · {}{}",
                                    entry.publisher,
                                    list::status(&entry, false),
                                    entry
                                        .installed_version
                                        .as_ref()
                                        .map(|version| format!(" · v{version}"))
                                        .unwrap_or_default()
                                )))
                            .child(actions),
                    ),
            )
            .child(tabs)
            .child(
                div()
                    .id("plugin-details-scroll")
                    .flex_1()
                    .min_h_0()
                    .p_4()
                    .child(body)
                    .overflow_y_scrollbar(),
            )
            .into_any_element()
    }
}

fn detail_field(label: String, value: String, palette: ThemePalette) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .text_size(px(12.))
                .text_color(rgb(palette.text_muted))
                .child(label),
        )
        .child(div().text_size(px(14.)).child(value))
}
