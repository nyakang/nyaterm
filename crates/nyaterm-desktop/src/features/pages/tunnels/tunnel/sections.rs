use rust_i18n::t;

use std::collections::{HashMap, HashSet};

use gpui::prelude::*;
use gpui::{Context, FontWeight, IntoElement, MouseButton, SharedString, div, px, rgb, rgba, svg};

use super::super::common::{NetworkItemMenuConfig, network_item_overflow_menu};
use super::row::{TunnelNetworkRow, tunnel_network_row};
use crate::features::tunnels::{TunnelBatchAction, TunnelBatchProgress};
use crate::features::{NyaTermApp, formatting::tunnel_name};
use crate::models::NetworkTab;
use crate::widgets::{small_button, status_pill};
use nyaterm_core::{TunnelConfig, TunnelGroup, truncate_preview};
use nyaterm_transport::SshTunnelInfo;
use nyaterm_ui::{NyaButton, NyaButtonVariant};

#[derive(Debug, Clone)]
pub(in crate::features::pages::tunnels) struct TunnelSection {
    id: String,
    label: String,
    group: Option<TunnelGroup>,
    tunnels: Vec<TunnelConfig>,
}

pub(in crate::features::pages::tunnels) fn tunnel_sections(
    tunnels: &[TunnelConfig],
    groups: &[TunnelGroup],
    ungrouped_label: impl Into<SharedString>,
) -> Vec<TunnelSection> {
    let ungrouped_label: SharedString = ungrouped_label.into();
    let valid_group_ids = groups
        .iter()
        .map(|group| group.id.as_str())
        .collect::<HashSet<_>>();
    let mut by_group = HashMap::<String, Vec<TunnelConfig>>::new();
    let mut ungrouped = Vec::<TunnelConfig>::new();

    for tunnel in tunnels {
        match tunnel.group_id.as_deref() {
            Some(group_id) if valid_group_ids.contains(group_id) => {
                by_group
                    .entry(group_id.to_string())
                    .or_default()
                    .push(tunnel.clone());
            }
            _ => ungrouped.push(tunnel.clone()),
        }
    }

    let mut sections = groups
        .iter()
        .cloned()
        .map(|group| TunnelSection {
            id: group.id.clone(),
            label: group.name.clone(),
            tunnels: by_group.remove(&group.id).unwrap_or_default(),
            group: Some(group),
        })
        .collect::<Vec<_>>();

    if !ungrouped.is_empty() || sections.is_empty() {
        sections.push(TunnelSection {
            id: "__ungrouped__".to_string(),
            label: ungrouped_label.to_string(),
            group: None,
            tunnels: ungrouped,
        });
    }

    sections
}

pub(in crate::features::pages::tunnels) fn tunnel_section(
    palette: crate::theme::ThemePalette,
    section: TunnelSection,
    open_tunnels: &HashMap<String, SshTunnelInfo>,
    app: &NyaTermApp,
    cx: &mut Context<NyaTermApp>,
) -> impl IntoElement {
    let item_count = section.tunnels.len();
    let section_key = format!("tunnel:{}", section.id);
    let collapsed = !app
        .connection_state
        .network_section_is_expanded(&section_key);
    let section_id_for_toggle = section.id.clone();
    let mut rows = div().flex().flex_col();
    if section.tunnels.is_empty() {
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
        for (index, tunnel) in section.tunnels.into_iter().enumerate() {
            let open_info = open_tunnels.get(&tunnel.id).cloned();
            let pending = app.tunnel_state.is_pending(&tunnel.id);
            let connection_label = tunnel
                .connection_id
                .as_deref()
                .and_then(|id| {
                    app.connection_state
                        .connections()
                        .iter()
                        .find(|connection| connection.id == id)
                        .map(|connection| connection.name.clone())
                })
                .unwrap_or_else(|| t!("network.connectionMissing").to_string());
            let mode_label = match tunnel.tunnel_type.as_str() {
                "remote" => t!("network.remoteTunnel"),
                "dynamic" => t!("network.dynamicTunnel"),
                _ => t!("network.localTunnel"),
            };
            let tunnel_for_open = tunnel.clone();
            let tunnel_id_for_close = tunnel.id.clone();
            let tunnel_id_for_edit = tunnel.id.clone();
            let tunnel_id_for_move = tunnel.id.clone();
            let tunnel_id_for_delete = tunnel.id.clone();
            let tunnel_label_for_delete = tunnel_name(&tunnel);
            let move_picker_open = app
                .connection_state
                .network_move_picker_is_open(NetworkTab::Tunnels, &tunnel.id);
            let current_group_id = tunnel.group_id.clone();
            rows = rows.child(
                div()
                    .flex()
                    .flex_col()
                    .when(index + 1 < item_count, |this| {
                        this.border_b_1().border_color(rgb(palette.border))
                    })
                    .child(tunnel_network_row(
                        TunnelNetworkRow {
                            tunnel: &tunnel,
                            connection_label,
                            open_info,
                            pending,
                            open_status_label: t!("network.tunnelOpen"),
                            closed_status_label: t!("network.tunnelClosed"),
                            mode_label,
                            menu: NetworkItemMenuConfig {
                                palette,
                                id: format!("network-tunnel-actions-{}", tunnel.id),
                                more_label: t!("common.more"),
                                edit_label: t!("common.edit"),
                                move_label: t!("network.moveToGroup"),
                                delete_label: t!("common.delete"),
                                can_move: !app.tunnel_state.tunnel_groups().is_empty(),
                            },
                        },
                        cx.listener(move |this, _, window, cx| {
                            this.start_tunnel_job(tunnel_for_open.clone(), window, cx);
                        }),
                        cx.listener(move |this, _, _, cx| {
                            this.close_tunnel_job(tunnel_id_for_close.clone(), cx);
                        }),
                        cx.listener(move |this, _, window, cx| {
                            this.open_network_tunnel_editor(
                                Some(tunnel_id_for_edit.clone()),
                                window,
                                cx,
                            );
                        }),
                        cx.listener(move |this, _, _, cx| {
                            this.open_network_move_picker(
                                NetworkTab::Tunnels,
                                tunnel_id_for_move.clone(),
                                cx,
                            );
                        }),
                        cx.listener(move |this, _, window, cx| {
                            this.open_network_delete_confirm(
                                NetworkTab::Tunnels,
                                tunnel_id_for_delete.clone(),
                                tunnel_label_for_delete.clone(),
                                window,
                                cx,
                            );
                        }),
                    ))
                    .when(move_picker_open, |this| {
                        this.child(tunnel_move_picker(
                            palette,
                            tunnel.id.clone(),
                            current_group_id,
                            app.tunnel_state.tunnel_groups(),
                            cx,
                        ))
                    }),
            );
        }
    }

    div()
        .id(gpui::SharedString::from(format!(
            "tunnel-section-{}",
            section.id
        )))
        .rounded_md()
        .border_1()
        .border_color(rgb(palette.border))
        .child(
            div()
                .id(gpui::SharedString::from(format!(
                    "tunnel-section-header-{}",
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
                            NetworkTab::Tunnels,
                            section_id_for_toggle.clone(),
                            cx,
                        );
                    })
                })
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
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
                    if item_count == 0 {
                        return this;
                    }
                    this.child(tunnel_group_batch_actions(
                        palette,
                        group.id.clone(),
                        app.tunnel_state.batch_progress(&group.id),
                        cx,
                    ))
                })
                .when_some(section.group.clone(), |this, group| {
                    let rename_id = group.id.clone();
                    let delete_id = group.id.clone();
                    let delete_label = group.name.clone();
                    this.child(network_item_overflow_menu(
                        NetworkItemMenuConfig {
                            palette,
                            id: format!("tunnel-group-actions-{}", group.id),
                            more_label: t!("common.more"),
                            edit_label: t!("network.renameGroup"),
                            move_label: t!("network.moveToGroup"),
                            delete_label: t!("network.deleteGroup"),
                            can_move: false,
                        },
                        cx.listener(move |this, _, window, cx| {
                            this.open_network_group_editor(
                                NetworkTab::Tunnels,
                                Some(rename_id.clone()),
                                window,
                                cx,
                            );
                        }),
                        cx.listener(|_, _, _, _| {}),
                        cx.listener(move |this, _, window, cx| {
                            this.open_network_group_delete_confirm(
                                NetworkTab::Tunnels,
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

/// Group-wide start/stop controls in a tunnel group header.
///
/// The header toggles collapse on click, so the strip swallows mouse-down
/// before the header sees it. A running batch shows its progress; the
/// opposite action stays available and supersedes it.
fn tunnel_group_batch_actions(
    palette: crate::theme::ThemePalette,
    group_id: String,
    progress: Option<TunnelBatchProgress>,
    cx: &mut Context<NyaTermApp>,
) -> impl IntoElement {
    let running = progress.map(|progress| progress.action);
    let start_group_id = group_id.clone();
    let stop_group_id = group_id.clone();
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap_1()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .when_some(progress, |this, progress| {
            this.child(
                div()
                    .text_size(px(10.))
                    .text_color(rgb(palette.text_muted))
                    .child(t!(
                        "network.batchProgress",
                        done = progress.done,
                        total = progress.total
                    )),
            )
        })
        .child(
            NyaButton::new(
                format!("tunnel-group-start-all-{group_id}"),
                t!("network.startAll"),
            )
            .variant(NyaButtonVariant::Ghost)
            .icon("icons/transfer/play.svg")
            .small()
            .compact()
            .loading(running == Some(TunnelBatchAction::Start))
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.start_tunnel_group(start_group_id.clone(), cx);
            })),
        )
        .child(
            NyaButton::new(
                format!("tunnel-group-stop-all-{group_id}"),
                t!("network.stopAll"),
            )
            .variant(NyaButtonVariant::Ghost)
            .icon("icons/transfer/stop.svg")
            .small()
            .compact()
            .loading(running == Some(TunnelBatchAction::Stop))
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.stop_tunnel_group(stop_group_id.clone(), cx);
            })),
        )
}

fn tunnel_move_picker(
    palette: crate::theme::ThemePalette,
    tunnel_id: String,
    current_group_id: Option<String>,
    groups: &[TunnelGroup],
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
        let target_id = tunnel_id.clone();
        targets = targets.child(small_button(
            palette,
            format!("network-tunnel-move-{tunnel_id}-ungrouped"),
            t!("network.ungrouped"),
            cx.listener(move |this, _, _, cx| {
                this.move_tunnel_to_group(target_id.clone(), None, cx);
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
            let target_id = tunnel_id.clone();
            let group_id = group.id.clone();
            targets = targets.child(small_button(
                palette,
                format!("network-tunnel-move-{tunnel_id}-{}", group.id),
                t!("network.moveHere"),
                cx.listener(move |this, _, _, cx| {
                    this.move_tunnel_to_group(target_id.clone(), Some(group_id.clone()), cx);
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
