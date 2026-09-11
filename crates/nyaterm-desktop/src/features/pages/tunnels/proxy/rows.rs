use rust_i18n::t;

use std::borrow::Cow;

use gpui::prelude::*;
use gpui::{Context, FontWeight, IntoElement, div, px, rgb};

use super::super::common::{NetworkItemMenuConfig, network_item_overflow_menu};
use super::helpers::proxy_protocol_label;
use crate::features::NyaTermApp;
use crate::models::NetworkTab;
use crate::widgets::{small_button, status_pill};
use nyaterm_core::{ProxyConfig, ProxyGroup, truncate_preview};

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
    let proxy_id_for_move = proxy.id.clone();
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
                can_move: !app.tunnel_state.proxy_groups().is_empty(),
            },
            cx.listener(move |this, _, window, cx| {
                this.open_network_proxy_editor(Some(proxy_id_for_edit.clone()), window, cx);
            }),
            cx.listener(move |this, _, _, cx| {
                this.open_network_move_picker(NetworkTab::Proxies, proxy_id_for_move.clone(), cx);
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

pub(in crate::features::pages::tunnels) fn proxy_move_picker(
    palette: crate::theme::ThemePalette,
    proxy_id: String,
    current_group_id: Option<String>,
    groups: &[ProxyGroup],
    cx: &mut Context<NyaTermApp>,
) -> gpui::Div {
    let mut targets = div().flex().flex_wrap().items_center().gap_2();
    if current_group_id.is_none() {
        targets = targets.child(
            div()
                .rounded_md()
                .px_2()
                .py_1()
                .text_size(px(11.))
                .text_color(rgb(palette.link))
                .bg(rgb(palette.hover))
                .child(format!(
                    "{} · {}",
                    t!("network.ungrouped"),
                    t!("network.current")
                )),
        );
    } else {
        let target_id = proxy_id.clone();
        targets = targets.child(small_button(
            palette,
            format!("network-proxy-move-{proxy_id}-ungrouped"),
            t!("network.ungrouped"),
            cx.listener(move |this, _, _, cx| {
                this.move_proxy_to_group(target_id.clone(), None, cx);
            }),
        ));
    }

    for group in groups {
        if current_group_id.as_deref() == Some(group.id.as_str()) {
            targets = targets.child(status_pill(
                t!("network.current"),
                rgb(palette.success),
                rgb(palette.hover),
            ));
            targets = targets.child(
                div()
                    .text_xs()
                    .text_color(rgb(palette.text))
                    .child(truncate_preview(&group.name, 36)),
            );
        } else {
            let target_id = proxy_id.clone();
            let group_id = group.id.clone();
            targets = targets.child(small_button(
                palette,
                format!("network-proxy-move-{proxy_id}-{}", group.id),
                t!("network.moveHere"),
                cx.listener(move |this, _, _, cx| {
                    this.move_proxy_to_group(target_id.clone(), Some(group_id.clone()), cx);
                }),
            ));
            targets = targets.child(
                div()
                    .text_xs()
                    .text_color(rgb(palette.text_muted))
                    .child(truncate_preview(&group.name, 36)),
            );
        }
    }

    div()
        .border_t_1()
        .border_color(rgb(palette.border))
        .bg(rgb(palette.input))
        .px_3()
        .py_2()
        .flex()
        .items_center()
        .gap_3()
        .child(
            div()
                .flex_none()
                .text_xs()
                .font_weight(FontWeight(700.))
                .text_color(rgb(palette.text))
                .child(t!("network.moveToGroup")),
        )
        .child(targets)
}
