//! The network tools side panel.
//!
//! One row per registered service, rendered from the registry's descriptor so a
//! new service does not need its own panel code. Reads runtime snapshots only;
//! every mutation goes through a listener. The panel never performs network or
//! filesystem IO while rendering.

use gpui::{AnyElement, Context, IntoElement, div, prelude::*, px, rgb};
use nyaterm_transport::networktools::{NetworkToolKind, TransferDirection, TransferSnapshot};
use nyaterm_ui::{NyaButton, NyaScrollable, NyaSwitch, NyaTooltip};
use rust_i18n::t;

use crate::features::NyaTermApp;
use crate::features::networktools::common::ids;
use crate::theme::ThemePalette;

/// At most ten transfer rows stay visible before the group scrolls.
const TRANSFER_LIST_MAX_HEIGHT: f32 = 220.;

impl NyaTermApp {
    pub(in crate::features) fn networktools_panel(&mut self, cx: &mut Context<Self>) -> AnyElement {
        self.ensure_networktools_pump(cx);
        let palette = self.theme_palette();
        let rows = NetworkToolKind::ALL
            .iter()
            .map(|&kind| {
                let running = self.networktools.is_running(kind);
                let status = self.networktools.status(kind);
                let subtitle = if running {
                    t!(
                        "networktools.common.panel.listening",
                        addr = status.addr.map(|addr| addr.to_string()).unwrap_or_default()
                    )
                    .to_string()
                } else {
                    t!("networktools.common.panel.notRunning").to_string()
                };
                let toggle_tooltip = if running {
                    t!("networktools.common.panel.stop").to_string()
                } else {
                    t!("networktools.common.panel.start").to_string()
                };
                let status_message = self.networktools.status_message(kind).map(str::to_string);
                let name = t!(kind.name_key()).to_string();
                let description = t!(kind.description_key()).to_string();
                let transfers = self.networktools.transfers(kind);
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .id(ids::panel_element(kind, "name"))
                                            .text_sm()
                                            .text_color(rgb(palette.text))
                                            .tooltip(move |window, cx| {
                                                NyaTooltip::new(description.clone())
                                                    .build(window, cx)
                                            })
                                            .child(name),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(rgb(palette.text_muted))
                                            .child(subtitle),
                                    ),
                            )
                            .child(
                                NyaSwitch::new(ids::panel_element(kind, "toggle"))
                                    .checked(running)
                                    .disabled(self.networktools.is_busy(kind))
                                    .tooltip(toggle_tooltip)
                                    .on_click(cx.listener(move |this, _: &bool, _, cx| {
                                        this.toggle_networktools_service(kind, cx);
                                    })),
                            )
                            .child(
                                NyaButton::new(ids::panel_element(kind, "configure"), "")
                                    .icon("icons/settings.svg")
                                    .disabled(self.networktools.is_busy(kind))
                                    .small()
                                    .compact()
                                    .tooltip(t!("networktools.common.panel.configure"))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.open_networktools_config(kind, window, cx);
                                    })),
                            ),
                    )
                    .when_some(status_message, |this, message| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(rgb(palette.danger))
                                .child(message),
                        )
                    })
                    // Active transfers belong to this service's group and are
                    // omitted entirely when the list is empty.
                    .when(!transfers.is_empty(), |this| {
                        this.child(transfers_view(transfers, palette))
                    })
                    .into_any_element()
            })
            .collect::<Vec<_>>();

        div()
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(div().p_3().flex().flex_col().gap_3().children(rows))
            .into_any_element()
    }
}

/// Live transfers for one service: direction, file, progress and peer, capped
/// so at most ten rows are visible before the group scrolls.
fn transfers_view(transfers: &[TransferSnapshot], palette: ThemePalette) -> AnyElement {
    let rows = transfers
        .iter()
        .map(|snapshot| {
            let marker = match snapshot.direction {
                TransferDirection::Download => "↓",
                TransferDirection::Upload => "↑",
            };
            let total = snapshot
                .total
                .map(format_bytes)
                .unwrap_or_else(|| "?".to_string());
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap_2()
                .text_xs()
                .child(div().flex_none().child(marker))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(snapshot.file.clone()),
                )
                .child(
                    div()
                        .flex_none()
                        .text_color(rgb(palette.text_muted))
                        .child(format!("{} / {total}", format_bytes(snapshot.done))),
                )
                .child(
                    div()
                        .flex_none()
                        .text_color(rgb(palette.text_dimmed))
                        .child(snapshot.remote.clone()),
                )
        })
        .collect::<Vec<_>>();
    div()
        .flex()
        .flex_col()
        .gap_1()
        .max_h(px(TRANSFER_LIST_MAX_HEIGHT))
        .overflow_y_scrollbar()
        .children(rows)
        .into_any_element()
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024. && unit + 1 < UNITS.len() {
        value /= 1024.;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}
