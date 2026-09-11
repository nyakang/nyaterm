use rust_i18n::t;

use gpui::{
    AnyElement, App, ClickEvent, Context, FontWeight, IntoElement, Window, div, prelude::*, px,
    rgb, svg,
};

use std::collections::HashMap;

use crate::models::NetworkTab;
use nyaterm_ui::{NyaScrollable, NyaTabItem, NyaTabs};

use super::super::NyaTermApp;

mod common;
mod proxy;
mod tunnel;

use common::network_group_editor_content;
use proxy::{network_proxy_editor_content, proxy_section, proxy_sections};
use tunnel::{network_tunnel_editor_content, tunnel_section, tunnel_sections};

impl NyaTermApp {
    pub(in crate::features) fn tunnels_view(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = self.theme_palette();
        let open_tunnels = self
            .tunnel_state
            .open_tunnels()
            .into_iter()
            .map(|info| (info.id.clone(), info))
            .collect::<HashMap<_, _>>();
        let sections = tunnel_sections(
            self.tunnel_state.tunnels(),
            self.tunnel_state.tunnel_groups(),
            t!("network.ungrouped"),
        );
        let proxy_sections = proxy_sections(
            self.tunnel_state.proxies(),
            self.tunnel_state.proxy_groups(),
            t!("network.ungrouped"),
        );
        let mut tunnel_list = div().flex().flex_col().gap_2();
        if self.tunnel_state.tunnels().is_empty() && self.tunnel_state.tunnel_groups().is_empty() {
            let (title, description) = if self.connection_state.connections().is_empty() {
                (
                    t!("network.noConnections").to_string(),
                    t!("network.noConnectionsHint").to_string(),
                )
            } else {
                (
                    t!("network.noTunnels").to_string(),
                    t!("network.tunnelEmptyHint").to_string(),
                )
            };
            tunnel_list = tunnel_list.child(network_empty_state(
                palette,
                "icons/network.svg",
                title,
                description,
            ));
        } else {
            for section in sections {
                tunnel_list =
                    tunnel_list.child(tunnel_section(palette, section, &open_tunnels, self, cx));
            }
        }

        let mut proxy_list = div().flex().flex_col().gap_2();
        if self.tunnel_state.proxies().is_empty() && self.tunnel_state.proxy_groups().is_empty() {
            proxy_list = proxy_list.child(network_empty_state(
                palette,
                "icons/network.svg",
                t!("network.noProxyConfigs").to_string(),
                t!("network.proxyEmptyHint").to_string(),
            ));
        } else {
            for section in proxy_sections {
                proxy_list = proxy_list.child(proxy_section(palette, section, self, cx));
            }
        }
        let active_tab = self.connection_state.network_active_tab();
        // Tauri NetworkPanel body (PanelHeader is shared):
        // scroll(p-3) > Tabs(grid-cols-2) > config row (label + New Group/New item) > grouped list.
        // Network create/edit/delete use modal dialogs (Tauri Dialog) over the panel.
        let config_label = match active_tab {
            NetworkTab::Tunnels => t!("network.tunnelConfig").to_string(),
            NetworkTab::Proxies => t!("network.proxyConfig").to_string(),
        };
        let has_connections = !self.connection_state.connections().is_empty();

        div()
            .flex()
            .flex_col()
            .size_full()
            .overflow_hidden()
            .bg(self.shell_transparent_color(palette.surface))
            .child(
                div().flex_1().min_h_0().overflow_hidden().child(
                    div()
                        .id("network-list-scroll")
                        .size_full()
                        // Expanded records must stay within the sidebar width.
                        .overflow_y_scrollbar()
                        .p_3()
                        .flex()
                        .flex_col()
                        // TabsList grid-cols-2 h-8
                        .child(
                            NyaTabs::new("network-tabs")
                                .items([
                                    NyaTabItem::new(t!("network.tunnels").to_string()),
                                    NyaTabItem::new(t!("network.proxy").to_string()),
                                ])
                                .selected_index(
                                    if self.connection_state.network_tab_is(NetworkTab::Tunnels) {
                                        0
                                    } else {
                                        1
                                    },
                                )
                                .on_select(cx.listener(|this, index, _, cx| {
                                    let tab = match *index {
                                        0 => NetworkTab::Tunnels,
                                        _ => NetworkTab::Proxies,
                                    };
                                    this.set_network_tab(tab, cx);
                                })),
                        )
                        // Config row: label left, group + new right (Tauri)
                        .child(
                            div()
                                .mt_3()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap_2()
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .font_weight(FontWeight(600.))
                                        .text_color(rgb(palette.text))
                                        .child(config_label),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_1()
                                        .child(icon_network_action(
                                            palette,
                                            "network-group-new",
                                            "icons/fe/new-folder.svg",
                                            cx.listener(|this, _, window, cx| {
                                                this.open_network_group_editor(
                                                    this.connection_state.network_active_tab(),
                                                    None,
                                                    window,
                                                    cx,
                                                );
                                            }),
                                        ))
                                        .when(
                                            self.connection_state
                                                .network_tab_is(NetworkTab::Tunnels),
                                            |this| {
                                                this.child(network_create_button(
                                                    palette,
                                                    "network-tunnel-new",
                                                    t!("network.newTunnel").to_string(),
                                                    has_connections,
                                                    cx.listener(|this, _, window, cx| {
                                                        this.open_network_tunnel_editor(
                                                            None, window, cx,
                                                        );
                                                    }),
                                                ))
                                            },
                                        )
                                        .when(
                                            self.connection_state
                                                .network_tab_is(NetworkTab::Proxies),
                                            |this| {
                                                this.child(network_create_button(
                                                    palette,
                                                    "network-proxy-new",
                                                    t!("network.newProxy").to_string(),
                                                    true,
                                                    cx.listener(|this, _, window, cx| {
                                                        this.open_network_proxy_editor(
                                                            None, window, cx,
                                                        );
                                                    }),
                                                ))
                                            },
                                        ),
                                ),
                        )
                        .child(div().mt_2().child(match active_tab {
                            NetworkTab::Tunnels => tunnel_list.into_any_element(),
                            NetworkTab::Proxies => proxy_list.into_any_element(),
                        })),
                ),
            )
    }

    pub(in crate::features) fn network_group_editor_dialog_content(
        &mut self,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(editor) = self.connection_state.active_network_group_editor() else {
            return div().into_any_element();
        };
        network_group_editor_content(self, editor, cx).into_any_element()
    }

    pub(in crate::features) fn network_tunnel_editor_dialog_content(
        &mut self,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = self.theme_palette();
        let Some(editor) = self.connection_state.active_network_tunnel_editor() else {
            return div().into_any_element();
        };
        network_tunnel_editor_content(palette, editor, self, cx).into_any_element()
    }

    pub(in crate::features) fn network_proxy_editor_dialog_content(
        &mut self,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = self.theme_palette();
        let Some(editor) = self.connection_state.active_network_proxy_editor() else {
            return div().into_any_element();
        };
        network_proxy_editor_content(palette, editor, self, cx).into_any_element()
    }
}

fn icon_network_action(
    palette: crate::theme::ThemePalette,
    id: impl Into<String>,
    icon_path: &'static str,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(gpui::SharedString::from(id.into()))
        .size(px(28.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .text_color(rgb(palette.text_muted))
        .cursor_pointer()
        .hover(|this| {
            this.bg(rgb(palette.surface_elevated))
                .text_color(rgb(palette.text))
        })
        .child(
            svg()
                .size(px(16.))
                .flex_none()
                .path(icon_path)
                .text_color(rgb(palette.text_muted)),
        )
        .on_click(on_click)
}

fn network_create_button(
    palette: crate::theme::ThemePalette,
    id: impl Into<String>,
    label: String,
    enabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(gpui::SharedString::from(id.into()))
        .h(px(28.))
        .px_2()
        .flex()
        .items_center()
        .gap_1()
        .rounded_md()
        .text_size(px(12.))
        .text_color(rgb(palette.link))
        .when(enabled, |this| {
            this.cursor_pointer()
                .hover(|this| this.bg(rgb(palette.hover)).text_color(rgb(palette.text)))
                .on_click(on_click)
        })
        .when(!enabled, |this| this.opacity(0.4))
        .child(
            svg()
                .size(px(16.))
                .flex_none()
                .path("icons/conn/add.svg")
                .text_color(rgb(palette.link)),
        )
        .child(label)
}

fn network_empty_state(
    palette: crate::theme::ThemePalette,
    icon_path: &'static str,
    title: String,
    description: String,
) -> impl IntoElement {
    div()
        .min_h(px(132.))
        .rounded_md()
        .border_1()
        .border_color(rgb(palette.border))
        .px_4()
        .py_5()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_2()
        .text_center()
        .child(
            svg()
                .size(px(24.))
                .path(icon_path)
                .text_color(rgb(palette.text_dimmed)),
        )
        .child(
            div()
                .text_size(px(12.))
                .font_weight(FontWeight(600.))
                .text_color(rgb(palette.text))
                .child(title),
        )
        .child(
            div()
                .text_size(px(11.))
                .text_color(rgb(palette.text_dimmed))
                .child(description),
        )
}
