use rust_i18n::t;

use std::borrow::Cow;

use gpui::prelude::*;
use gpui::{Context, FontWeight, IntoElement, div, px, rgb};

use super::super::common::{
    NetworkItemMenuConfig, network_item_overflow_menu, network_move_menu_items,
};
use super::helpers::proxy_protocol_label;
use crate::features::NyaTermApp;
use crate::models::NetworkTab;
use nyaterm_core::{ProxyConfig, truncate_preview};

pub(in crate::features::pages::tunnels) fn proxy_network_row(
    proxy: &ProxyConfig,
    app: &NyaTermApp,
    cx: &mut Context<NyaTermApp>,
) -> impl IntoElement {
    let palette = app.theme_palette();
    let is_command = proxy.protocol == "proxycommand";
    let address = if is_command {
        proxy
            .command
            .as_deref()
            .filter(|command| !command.trim().is_empty())
            .map(Cow::Borrowed)
            .unwrap_or_else(|| t!("network.proxyCommand"))
            .to_string()
    } else if let Some(username) = proxy.username.as_deref().filter(|value| !value.is_empty()) {
        format!("{username}@{}:{}", proxy.host, proxy.port)
    } else {
        format!("{}:{}", proxy.host, proxy.port)
    };
    let proxy_id_for_edit = proxy.id.clone();
    let proxy_id_for_delete = proxy.id.clone();
    let proxy_label_for_delete = proxy.name.clone();
    // Tauri ProxyRow: name, protocol, address; overflow actions on the right.
    div()
        .px_3()
        .py(px(10.))
        .flex()
        .items_center()
        .gap(px(12.))
        .hover(|this| this.bg(rgb(palette.hover)))
        .child(
            div()
                .min_w_0()
                .flex_1()
                .flex()
                .flex_col()
                .child(
                    div()
                        .text_size(px(14.))
                        .font_weight(FontWeight(600.))
                        .text_color(rgb(palette.text))
                        .truncate()
                        .child(truncate_preview(&proxy.name, 52)),
                )
                .child(
                    div()
                        .mt(px(1.))
                        .text_size(px(12.))
                        .text_color(rgb(palette.text_muted))
                        .child(proxy_protocol_label(&proxy.protocol)),
                )
                .child(
                    div()
                        .mt(px(1.))
                        .font_family(crate::features::shell::gpui_code_font_family())
                        .text_size(px(11.))
                        .text_color(rgb(palette.text_dimmed))
                        .truncate()
                        .child(truncate_preview(&address, 92)),
                ),
        )
        .child(network_item_overflow_menu(
            NetworkItemMenuConfig {
                palette,
                id: format!("proxy-actions-{}", proxy.id),
                more_label: t!("common.more"),
                edit_label: t!("common.edit"),
                move_label: t!("network.moveToGroup"),
                delete_label: t!("common.delete"),
                move_items: network_move_menu_items(
                    NetworkTab::Proxies,
                    &proxy.id,
                    proxy.group_id.as_deref(),
                    app.tunnel_state
                        .proxy_groups()
                        .iter()
                        .map(|group| (group.id.clone(), group.name.clone())),
                    cx,
                ),
            },
            cx.listener(move |this, _, window, cx| {
                this.open_network_proxy_editor(Some(proxy_id_for_edit.clone()), window, cx);
            }),
            cx.listener(move |this, _, window, cx| {
                this.open_network_delete_confirm(
                    NetworkTab::Proxies,
                    proxy_id_for_delete.clone(),
                    proxy_label_for_delete.clone(),
                    window,
                    cx,
                );
            }),
        ))
}
