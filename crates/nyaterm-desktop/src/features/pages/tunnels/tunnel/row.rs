use std::borrow::Cow;

use gpui::prelude::*;
use gpui::{App, ClickEvent, FontWeight, Hsla, IntoElement, Window, div, px, rgb};

use super::super::common::{NetworkItemMenuConfig, network_item_overflow_menu};
use crate::features::{
    formatting::tunnel_endpoint, formatting::tunnel_mode, formatting::tunnel_name,
};
use crate::widgets::status_pill;
use nyaterm_core::{TunnelConfig, truncate_preview};
use nyaterm_transport::SshTunnelInfo;
use nyaterm_ui::NyaSwitch;

pub(in crate::features::pages::tunnels) struct TunnelNetworkRow<'a> {
    pub tunnel: &'a TunnelConfig,
    pub connection_label: String,
    pub open_info: Option<SshTunnelInfo>,
    pub pending: bool,
    pub open_status_label: Cow<'static, str>,
    pub closed_status_label: Cow<'static, str>,
    pub mode_label: Cow<'static, str>,
    pub menu: NetworkItemMenuConfig,
}

pub(in crate::features::pages::tunnels) fn tunnel_network_row(
    row: TunnelNetworkRow<'_>,
    on_open: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    on_close: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    on_edit: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    on_move: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    on_delete: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let TunnelNetworkRow {
        tunnel,
        connection_label,
        open_info,
        pending,
        open_status_label,
        closed_status_label,
        mode_label,
        menu,
    } = row;
    let palette = menu.palette;
    let supported = tunnel_mode(tunnel).is_some();
    let is_open = open_info.is_some();
    let status = if pending {
        Cow::Borrowed("pending")
    } else if is_open {
        open_status_label
    } else if supported {
        closed_status_label
    } else {
        Cow::Borrowed("porting")
    };
    let (status_color, status_bg) = tunnel_status_style(palette, pending, is_open, supported);
    let bind = if tunnel.bind_localhost {
        "127.0.0.1"
    } else {
        "0.0.0.0"
    };
    let listen = open_info
        .as_ref()
        .map(|info| format!("{}:{}", info.bind_host, info.listen_port))
        .unwrap_or_else(|| format!("{bind}:{}", tunnel.listen_port));
    // Tauri TunnelRow: 3-line left stack, StatusBadge, Switch, overflow actions.
    let toggle = if pending {
        status_pill("…", rgb(palette.warning), rgb(palette.hover)).into_any_element()
    } else if is_open {
        network_switch_button(
            palette,
            format!("network-tunnel-close-{}", tunnel.id),
            true,
            on_close,
        )
        .into_any_element()
    } else if supported {
        network_switch_button(
            palette,
            format!("network-tunnel-open-{}", tunnel.id),
            false,
            on_open,
        )
        .into_any_element()
    } else {
        status_pill("porting", rgb(palette.warning), rgb(palette.hover)).into_any_element()
    };

    // Tauri: px-3 py-2.5; side-panel density uses slightly tighter mono stack.
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
                .gap_0()
                .child(
                    div()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .text_size(px(14.))
                                .font_weight(FontWeight(600.))
                                .text_color(rgb(palette.text))
                                .truncate()
                                .child(truncate_preview(&tunnel_name(tunnel), 52)),
                        )
                        .child(div().flex_none().child(status_pill(
                            status,
                            status_color,
                            status_bg,
                        )))
                        .when(tunnel.auto_open, |this| {
                            this.child(
                                div()
                                    .flex_none()
                                    .text_size(px(10.))
                                    .text_color(rgb(palette.success))
                                    .child("auto"),
                            )
                        }),
                )
                .child(
                    div()
                        .mt(px(1.))
                        .text_size(px(12.))
                        .text_color(rgb(palette.text_muted))
                        .truncate()
                        .child(format!(
                            "{} · {}",
                            truncate_preview(&connection_label, 44),
                            mode_label
                        )),
                )
                .child(
                    div()
                        .mt(px(1.))
                        .font_family(crate::features::shell::gpui_code_font_family())
                        .text_size(px(11.))
                        .text_color(rgb(palette.text_dimmed))
                        .truncate()
                        .child(truncate_preview(&tunnel_endpoint(tunnel, &listen), 88)),
                ),
        )
        .child(
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap_1()
                .child(toggle)
                .child(network_item_overflow_menu(
                    menu, on_edit, on_move, on_delete,
                )),
        )
}

fn network_switch_button(
    _palette: crate::theme::ThemePalette,
    id: impl Into<String>,
    on: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    NyaSwitch::new(id.into())
        .checked(on)
        .on_click(move |_, window, cx| {
            on_click(&ClickEvent::default(), window, cx);
        })
}

pub(super) fn tunnel_status_style(
    palette: crate::theme::ThemePalette,
    pending: bool,
    is_open: bool,
    supported: bool,
) -> (Hsla, Hsla) {
    if pending {
        (rgb(palette.warning).into(), rgb(palette.hover).into())
    } else if is_open {
        (rgb(palette.success).into(), rgb(palette.hover).into())
    } else if supported {
        (rgb(palette.link).into(), rgb(palette.hover).into())
    } else {
        (rgb(palette.warning).into(), rgb(palette.hover).into())
    }
}
