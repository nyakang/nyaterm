use std::time::{Duration, Instant};

use gpui::{
    Context, Entity, FontWeight, Hsla, IntoElement, ScrollHandle, div, prelude::*, px, rgb,
};
use nyaterm_remote_ops::stats::{NetworkInfo, NetworkRateStatus};
use nyaterm_ui::plot::{NyaLinePlot, NyaLineSeries};
use nyaterm_ui::{
    NYA_FORM_CONTROL_HEIGHT_PX, NyaButton, NyaButtonVariant, NyaCollapsible, NyaScrollable,
    NyaSelect, NyaSelectState, NyaTooltip,
};
use rust_i18n::t;

use super::{format_resource_rate, resource_section_card_with_action};
use crate::features::pages::remote::panels::RemoteMonitorPanel;
use crate::features::remote::StatsPresentationState;
use crate::features::shell::gpui_code_font_family;
use crate::features::view_widgets::mono_icon;
use crate::theme::ThemePalette;

const NETWORK_AXIS_WIDTH_PX: f32 = 28.;
const NETWORK_INTERFACE_ROW_HEIGHT_PX: f32 = 24.;
const NETWORK_INTERFACE_VISIBLE_ROWS: usize = 5;

pub(in crate::features::pages::remote) struct NetworkCardPresentation {
    pub now: Instant,
    pub interval: Duration,
    pub scroll: ScrollHandle,
}

pub(super) fn resource_network_card(
    palette: ThemePalette,
    state: &StatsPresentationState,
    select: &Entity<NyaSelectState>,
    presentation: &NetworkCardPresentation,
    cx: &mut Context<RemoteMonitorPanel>,
) -> gpui::Div {
    let has_interfaces = state
        .data
        .as_ref()
        .is_some_and(|stats| !stats.networks.is_empty());
    let action = has_interfaces.then(|| {
        div()
            .w(px(120.))
            .min_w_0()
            .h(px(NYA_FORM_CONTROL_HEIGHT_PX))
            .child(NyaSelect::new(select).appearance(false))
            .into_any_element()
    });
    let rx_color: Hsla = rgb(0x3b82f6).into();
    let tx_color: Hsla = rgb(0x22c55e).into();
    let traffic = state.network_traffic();
    let body = div()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .debug_selector(|| "stats-network-metrics".to_string())
                .grid()
                .grid_cols(2)
                .gap_2()
                .child(network_metric(
                    palette,
                    t!("resourceMonitor.receiveRx").to_string(),
                    traffic.as_ref().map(|rate| rate.rx_bytes_per_sec),
                    "icons/arrow-down.svg",
                    rx_color,
                ))
                .child(network_metric(
                    palette,
                    t!("resourceMonitor.transmitTx").to_string(),
                    traffic.as_ref().map(|rate| rate.tx_bytes_per_sec),
                    "icons/fe/up.svg",
                    tx_color,
                )),
        )
        .child(network_history_chart(
            palette,
            state,
            presentation.now,
            rx_color,
            tx_color,
        ))
        .child(network_interfaces(
            palette,
            state,
            &presentation.scroll,
            rx_color,
            tx_color,
            cx,
        ));
    resource_section_card_with_action(
        palette,
        "icons/network.svg",
        t!("resourceMonitor.network").to_string(),
        action,
        body,
    )
}

fn rate_text(value: Option<f64>) -> String {
    value
        .map(format_resource_rate)
        .unwrap_or_else(|| "—".to_string())
}

fn network_metric(
    palette: ThemePalette,
    label: String,
    value: Option<f64>,
    arrow: &'static str,
    color: Hsla,
) -> gpui::Div {
    div()
        .min_w_0()
        .rounded_md()
        .bg(rgb(palette.surface))
        .p_2()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .flex()
                .items_center()
                .gap_1()
                .text_size(px(10.))
                .text_color(rgb(palette.text_muted))
                .child(mono_icon(arrow, color, 11.))
                .child(label),
        )
        .child(
            div()
                .font_family(gpui_code_font_family())
                .text_size(px(13.))
                .font_weight(FontWeight(700.))
                .text_color(color)
                .child(rate_text(value)),
        )
}

fn network_history_chart(
    palette: ThemePalette,
    state: &StatsPresentationState,
    now: Instant,
    rx_color: Hsla,
    tx_color: Hsla,
) -> gpui::Div {
    let scale = state.network_chart.scale(now);
    let plot = NyaLinePlot::new(
        "stats-network-lines",
        vec![
            NyaLineSeries {
                points: state.network_chart.rx.clone(),
                color: rx_color,
            },
            NyaLineSeries {
                points: state.network_chart.tx.clone(),
                color: tx_color,
            },
        ],
        rgb(palette.border).into(),
    )
    .x_range(scale.x_range)
    .y_max(scale.maximum);
    let y_axis = div()
        .w(px(NETWORK_AXIS_WIDTH_PX))
        .flex_none()
        .h_full()
        .py(px(4.))
        .flex()
        .flex_col()
        .justify_between()
        .font_family(gpui_code_font_family())
        .text_right()
        .text_size(px(9.))
        .text_color(rgb(palette.text_dimmed))
        .children(
            [scale.maximum, scale.maximum / 2., 0.]
                .into_iter()
                .map(|value| div().child(axis_number(value / scale.divisor))),
        );
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_1()
                .text_size(px(10.))
                .text_color(rgb(palette.text_dimmed))
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(network_legend("RX", "icons/arrow-down.svg", rx_color))
                        .child(network_legend("TX", "icons/fe/up.svg", tx_color))
                        .child(scale.unit),
                )
                .child(
                    div()
                        .id("stats-network-peak")
                        .child(
                            t!("resourceMonitor.maxRate", value = rate_text(scale.peak))
                                .to_string(),
                        )
                        .tooltip(|window, cx| {
                            NyaTooltip::new(t!("resourceMonitor.networkPeakHint").to_string())
                                .build(window, cx)
                        }),
                ),
        )
        .child(
            div()
                .debug_selector(|| "stats-network-chart-frame".to_string())
                .h(px(84.))
                .w_full()
                .flex()
                .gap(px(4.))
                .rounded_sm()
                .border_1()
                .border_color(rgb(palette.border))
                .bg(rgb(palette.surface))
                .child(y_axis)
                .child(
                    div()
                        .debug_selector(|| "stats-network-chart-plot".to_string())
                        .relative()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .child(plot)
                        .when(scale.available_points < 2, |chart| {
                            chart.child(
                                div()
                                    .absolute()
                                    .inset_0()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_size(px(10.))
                                    .text_color(rgb(palette.text_dimmed))
                                    .child(
                                        t!("resourceMonitor.collectingNetworkHistory").to_string(),
                                    ),
                            )
                        }),
                ),
        )
        .child(
            div()
                .w_full()
                // Match the frame's border and axis column; the inner padding
                // matches NyaLinePlot's four-pixel paint inset.
                .px(px(1.))
                .flex()
                .gap(px(4.))
                .text_size(px(9.))
                .text_color(rgb(palette.text_dimmed))
                .child(div().w(px(NETWORK_AXIS_WIDTH_PX)).flex_none())
                .child(
                    div()
                        .debug_selector(|| "stats-network-chart-ticks".to_string())
                        .flex_1()
                        .min_w_0()
                        .px(px(4.))
                        .grid()
                        .grid_cols(3)
                        .child(
                            div().child(
                                t!("resourceMonitor.networkSecondsAgo", count = 60).to_string(),
                            ),
                        )
                        .child(
                            div().text_center().child(
                                t!("resourceMonitor.networkSecondsAgo", count = 30).to_string(),
                            ),
                        )
                        .child(
                            div()
                                .text_right()
                                .child(t!("resourceMonitor.networkNow").to_string()),
                        ),
                ),
        )
}

fn axis_number(value: f64) -> String {
    let value = format!("{value:.2}");
    value
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string()
}

fn network_interfaces(
    palette: ThemePalette,
    state: &StatsPresentationState,
    scroll: &ScrollHandle,
    rx_color: Hsla,
    tx_color: Hsla,
    cx: &mut Context<RemoteMonitorPanel>,
) -> NyaCollapsible {
    let count = state.data.as_ref().map_or(0, |stats| stats.networks.len());
    NyaCollapsible::new()
        .open(state.network_interfaces_expanded)
        .w_full()
        .gap_1()
        .child(
            div()
                .w_full()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_x_2()
                .gap_y_1()
                .child(
                    div()
                        .debug_selector(|| "stats-network-interfaces-trigger".to_string())
                        .flex()
                        .flex_none()
                        .child(
                            NyaButton::new(
                                "stats-network-interfaces-toggle",
                                t!("resourceMonitor.interfaceRates", count = count).to_string(),
                            )
                            .variant(NyaButtonVariant::Ghost)
                            .small()
                            .compact()
                            .height(px(24.))
                            .text_size(px(11.))
                            .icon(if state.network_interfaces_expanded {
                                "icons/chevron-down.svg"
                            } else {
                                "icons/menu/chevron-right.svg"
                            })
                            .on_click(cx.listener(
                                |panel, _, _, cx| {
                                    panel.with_app(cx, |app, _cx| {
                                        if let Some(id) =
                                            app.session.active_id().map(str::to_string)
                                        {
                                            app.remote_ops.toggle_stats_network_interfaces(&id);
                                        }
                                    });
                                },
                            )),
                        ),
                )
                .child(
                    div()
                        .debug_selector(|| "stats-network-recent-seconds".to_string())
                        .flex_none()
                        .ml_auto()
                        .whitespace_nowrap()
                        .text_right()
                        .text_size(px(10.))
                        .text_color(rgb(palette.text_dimmed))
                        .child(t!("resourceMonitor.recentSeconds", count = 60).to_string()),
                ),
        )
        .when(state.network_interfaces_expanded, |collapsible| {
            collapsible.content(network_interface_rates(
                palette, state, scroll, rx_color, tx_color,
            ))
        })
}

fn network_interface_rates(
    palette: ThemePalette,
    state: &StatsPresentationState,
    scroll: &ScrollHandle,
    rx_color: Hsla,
    tx_color: Hsla,
) -> gpui::Div {
    let Some(stats) = state
        .data
        .as_ref()
        .filter(|stats| !stats.networks.is_empty())
    else {
        return div()
            .debug_selector(|| "stats-network-empty".to_string())
            .py_1()
            .text_size(px(11.))
            .text_color(rgb(palette.text_dimmed))
            .child(t!("resourceMonitor.noNetworkInterfaces").to_string());
    };
    let body = div().flex().flex_col().gap_1().child(
        div()
            .flex()
            .gap_1()
            .text_size(px(10.))
            .text_color(rgb(palette.text_muted))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(t!("resourceMonitor.networkInterface").to_string()),
            )
            .child(
                div()
                    .w(px(72.))
                    .flex_none()
                    .text_right()
                    .text_color(rx_color)
                    .child("↓ RX"),
            )
            .child(
                div()
                    .w(px(72.))
                    .flex_none()
                    .text_right()
                    .text_color(tx_color)
                    .child("↑ TX"),
            ),
    );
    let rows = div()
        .id("stats-network-interface-rows")
        .w_full()
        .flex()
        .flex_col()
        .children(stats.networks.iter().map(|network| {
            network_interface_row(
                palette,
                network,
                state.selected_network_interface.as_deref(),
                rx_color,
                tx_color,
            )
        }));
    body.child(
        div()
            .debug_selector(|| "stats-network-interface-list".to_string())
            .w_full()
            .map(|viewport| {
                if stats.networks.len() > NETWORK_INTERFACE_VISIBLE_ROWS {
                    viewport
                        .h(px(
                            NETWORK_INTERFACE_VISIBLE_ROWS as f32 * NETWORK_INTERFACE_ROW_HEIGHT_PX
                        ))
                        .child(
                            rows.debug_selector(|| "stats-network-interface-scroll".to_string())
                                .h_full()
                                .overflow_y_scroll()
                                .track_scroll(scroll),
                        )
                        .vertical_scrollbar(scroll)
                } else {
                    viewport.child(rows)
                }
            }),
    )
}

fn network_interface_row(
    palette: ThemePalette,
    network: &NetworkInfo,
    selected: Option<&str>,
    rx_color: Hsla,
    tx_color: Hsla,
) -> gpui::Div {
    let traffic = network.traffic();
    let name = network.nic.clone();
    let hint = match network.rate_status {
        NetworkRateStatus::Available => name.clone(),
        NetworkRateStatus::WarmingUp => format!("{name} · {}", t!("resourceMonitor.sampling")),
        NetworkRateStatus::CounterReset => {
            format!("{name} · {}", t!("resourceMonitor.networkCounterReset"))
        }
        NetworkRateStatus::Interrupted => {
            format!("{name} · {}", t!("resourceMonitor.networkInterrupted"))
        }
        NetworkRateStatus::Unavailable => {
            format!("{name} · {}", t!("resourceMonitor.networkUnavailable"))
        }
    };
    div()
        .debug_selector({
            let name = name.clone();
            move || format!("stats-network-row-{name}")
        })
        .h(px(NETWORK_INTERFACE_ROW_HEIGHT_PX))
        .flex_none()
        .w_full()
        .flex()
        .items_center()
        .gap_1()
        .text_size(px(11.))
        .rounded_sm()
        .when(selected == Some(network.nic.as_str()), |row| {
            row.bg(rgb(palette.surface)).font_weight(FontWeight(600.))
        })
        .child(
            div()
                .id(format!("stats-network-name-{name}"))
                .min_w_0()
                .flex_1()
                .overflow_hidden()
                .text_ellipsis()
                .text_color(rgb(palette.text_muted))
                .child(name)
                .tooltip(move |window, cx| NyaTooltip::new(hint.clone()).build(window, cx)),
        )
        .child(
            div()
                .w(px(72.))
                .flex_none()
                .text_right()
                .font_family(gpui_code_font_family())
                .text_color(rx_color)
                .child(rate_text(
                    traffic.as_ref().map(|rate| rate.rx_bytes_per_sec),
                )),
        )
        .child(
            div()
                .w(px(72.))
                .flex_none()
                .text_right()
                .font_family(gpui_code_font_family())
                .text_color(tx_color)
                .child(rate_text(
                    traffic.as_ref().map(|rate| rate.tx_bytes_per_sec),
                )),
        )
}

pub(super) fn network_updated_text(
    state: &StatsPresentationState,
    now: Instant,
    interval: Duration,
) -> String {
    let Some(at) = state.last_success_at else {
        return if state.error {
            t!("resourceMonitor.networkRefreshFailed")
        } else {
            t!("resourceMonitor.sampling")
        }
        .to_string();
    };
    let seconds = now.saturating_duration_since(at).as_secs();
    let updated = if seconds == 0 {
        t!("resourceMonitor.networkUpdatedNow").to_string()
    } else {
        t!("resourceMonitor.networkUpdatedSeconds", count = seconds).to_string()
    };
    if state.error {
        t!("resourceMonitor.networkFailedUpdate", updated = updated).to_string()
    } else if state.stale(now, interval) {
        t!("resourceMonitor.networkStaleUpdate", updated = updated).to_string()
    } else {
        updated
    }
}

fn network_legend(label: &'static str, arrow: &'static str, color: Hsla) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap_1()
        .child(mono_icon(arrow, color, 11.))
        .child(label)
}
