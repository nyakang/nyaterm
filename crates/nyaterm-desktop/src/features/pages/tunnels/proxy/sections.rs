use rust_i18n::t;

use std::collections::{HashMap, HashSet};

use gpui::prelude::*;
use gpui::{Context, FontWeight, IntoElement, SharedString, div, px, rgb, rgba, svg};

use super::super::common::{NetworkItemMenuConfig, network_item_overflow_menu};
use super::rows::proxy_network_row;
use crate::features::NyaTermApp;
use crate::models::NetworkTab;
use nyaterm_core::{ProxyConfig, ProxyGroup, truncate_preview};

#[derive(Debug, Clone)]
pub(in crate::features::pages::tunnels) struct ProxySection {
    id: String,
    label: String,
    group: Option<ProxyGroup>,
    proxies: Vec<ProxyConfig>,
}

pub(in crate::features::pages::tunnels) fn proxy_sections(
    proxies: &[ProxyConfig],
    groups: &[ProxyGroup],
    ungrouped_label: impl Into<SharedString>,
) -> Vec<ProxySection> {
    let ungrouped_label: SharedString = ungrouped_label.into();
    let valid_group_ids = groups
        .iter()
        .map(|group| group.id.as_str())
        .collect::<HashSet<_>>();
    let mut by_group = HashMap::<String, Vec<ProxyConfig>>::new();
    let mut ungrouped = Vec::<ProxyConfig>::new();

    for proxy in proxies {
        match proxy.group_id.as_deref() {
            Some(group_id) if valid_group_ids.contains(group_id) => {
                by_group
                    .entry(group_id.to_string())
                    .or_default()
                    .push(proxy.clone());
            }
            _ => ungrouped.push(proxy.clone()),
        }
    }

    let mut sections = groups
        .iter()
        .cloned()
        .map(|group| ProxySection {
            id: group.id.clone(),
            label: group.name.clone(),
            proxies: by_group.remove(&group.id).unwrap_or_default(),
            group: Some(group),
        })
        .collect::<Vec<_>>();

    if !ungrouped.is_empty() || sections.is_empty() {
        sections.push(ProxySection {
            id: "__ungrouped__".to_string(),
            label: ungrouped_label.to_string(),
            group: None,
            proxies: ungrouped,
        });
    }

    sections
}

pub(in crate::features::pages::tunnels) fn proxy_section(
    palette: crate::theme::ThemePalette,
    section: ProxySection,
    app: &NyaTermApp,
    cx: &mut Context<NyaTermApp>,
) -> impl IntoElement {
    let item_count = section.proxies.len();
    let section_key = format!("proxy:{}", section.id);
    let collapsed = !app
        .connection_state
        .network_section_is_expanded(&section_key);
    let section_id_for_toggle = section.id.clone();
    let mut rows = div().flex().flex_col();
    if section.proxies.is_empty() {
        rows = rows.child(
            div()
                .border_t_1()
                .border_color(rgb(palette.border))
                .px_2()
                .py_2()
                .text_size(px(11.))
                .text_color(rgb(palette.text_muted))
                .child(t!("network.groupEmpty")),
        );
    } else {
        for (index, proxy) in section.proxies.into_iter().enumerate() {
            rows = rows.child(
                div()
                    .flex()
                    .flex_col()
                    .when(index + 1 < item_count, |this| {
                        this.border_b_1().border_color(rgb(palette.border))
                    })
                    .child(proxy_network_row(&proxy, app, cx)),
            );
        }
    }

    div()
        .id(gpui::SharedString::from(format!(
            "proxy-section-{}",
            section.id
        )))
        .rounded_md()
        .border_1()
        .border_color(rgb(palette.border))
        .child(
            div()
                .id(gpui::SharedString::from(format!(
                    "proxy-section-header-{}",
                    section.id
                )))
                .h(px(32.))
                .px_3()
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .bg(rgba((palette.hover << 8) | 0x8c))
                .cursor_pointer()
                .hover(|this| this.bg(rgb(palette.hover)))
                .on_click({
                    let section_id_for_toggle = section_id_for_toggle.clone();
                    cx.listener(move |this, _, _, cx| {
                        this.toggle_network_section(
                            NetworkTab::Proxies,
                            section_id_for_toggle.clone(),
                            cx,
                        );
                    })
                })
                .child(
                    div()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            svg()
                                .size(px(16.))
                                .flex_none()
                                .path(if collapsed {
                                    "icons/fe/forward.svg"
                                } else {
                                    "icons/chevron-down.svg"
                                })
                                .text_color(rgb(palette.text_muted)),
                        )
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight(600.))
                                .text_color(rgb(palette.text))
                                .child(truncate_preview(&section.label, 48)),
                        )
                        .child(
                            div()
                                .rounded_full()
                                .px_1()
                                .text_size(px(10.))
                                .text_color(rgb(palette.text_muted))
                                .bg(rgba((palette.text_muted << 8) | 0x1f))
                                .child(item_count.to_string()),
                        ),
                )
                .when_some(section.group.clone(), |this, group| {
                    let rename_id = group.id.clone();
                    let delete_id = group.id.clone();
                    let delete_label = group.name.clone();
                    this.child(network_item_overflow_menu(
                        NetworkItemMenuConfig {
                            palette,
                            id: format!("proxy-group-actions-{}", group.id),
                            more_label: t!("common.more"),
                            edit_label: t!("network.renameGroup"),
                            move_label: t!("network.moveToGroup"),
                            delete_label: t!("network.deleteGroup"),
                            move_items: Vec::new(),
                        },
                        cx.listener(move |this, _, window, cx| {
                            this.open_network_group_editor(
                                NetworkTab::Proxies,
                                Some(rename_id.clone()),
                                window,
                                cx,
                            );
                        }),
                        cx.listener(move |this, _, window, cx| {
                            this.open_network_group_delete_confirm(
                                NetworkTab::Proxies,
                                delete_id.clone(),
                                delete_label.clone(),
                                item_count,
                                window,
                                cx,
                            );
                        }),
                    ))
                }),
        )
        .when(!collapsed, |this| this.child(rows))
}
