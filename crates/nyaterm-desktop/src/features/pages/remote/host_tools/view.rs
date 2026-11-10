//! Rendering for the Ports and Services panels. Pure presentation: every value
//! comes from the panel entity, nothing is read from the app.

use super::jobs::Lane;
use super::state::{
    BindKind, PortProtocolFilter, PortRow, PortScopeFilter, PortSortKey, ServiceSortKey,
    ServiceStateFilter, SortDirection, bind_kind, count_services, well_known_service,
};
use super::{HostResolution, HostToolsPanel, LoadState};
use crate::features::pages::remote::panels::PanelChrome;
use crate::features::shell::gpui_code_font_family;
use crate::theme::ThemePalette;
use crate::widgets::empty_panel_with_icon;
use gpui::{
    AnyElement, App, ClickEvent, Context, FontWeight, IntoElement, SharedString, Window, div, list,
    prelude::*, px, rgb, svg,
};
use nyaterm_remote_ops::ports::PortExposure;
use nyaterm_remote_ops::services::{ServiceAction, ServiceEntry, ServiceStartup, ServiceState};
use nyaterm_ui::{
    NyaButton, NyaButtonVariant, NyaDropdownMenu, NyaInputShell, NyaMenuItem, NyaScrollable,
    NyaSearchInput, NyaTooltip,
};
use rust_i18n::t;

const PORT_ROW_HEIGHT: f32 = 52.;
const SERVICE_ROW_HEIGHT: f32 = 54.;

fn icon_button(
    palette: ThemePalette,
    id: impl Into<SharedString>,
    icon: &'static str,
    tooltip: impl Into<SharedString>,
    enabled: bool,
    tint: Option<u32>,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let tooltip: SharedString = tooltip.into();
    let color = tint.unwrap_or(palette.text_muted);
    let base = div()
        .id(id.into())
        .size(px(26.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .child(
            svg()
                .size(px(15.))
                .flex_none()
                .path(icon)
                .text_color(rgb(if enabled { color } else { palette.text_dimmed })),
        );
    if !enabled {
        return base.opacity(0.45).into_any_element();
    }
    base.cursor_pointer()
        .hover(move |this| this.bg(rgb(palette.hover)).text_color(rgb(palette.text)))
        .tooltip(move |window, cx| NyaTooltip::new(tooltip.clone()).build(window, cx))
        .on_click(on_click)
        .into_any_element()
}

fn chip(
    palette: ThemePalette,
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    active: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id.into())
        .h(px(22.))
        .px_2()
        .flex()
        .items_center()
        .rounded_md()
        .bg(if active {
            rgb(palette.surface_elevated)
        } else {
            rgb(palette.surface).alpha(0.0)
        })
        .text_size(px(10.5))
        .font_weight(FontWeight(if active { 700. } else { 500. }))
        .text_color(rgb(if active {
            palette.text
        } else {
            palette.text_dimmed
        }))
        .cursor_pointer()
        .hover(move |this| {
            this.bg(rgb(palette.surface_elevated))
                .text_color(rgb(palette.text))
        })
        .child(label.into())
        .on_click(on_click)
        .into_any_element()
}

fn badge(label: impl Into<SharedString>, color: u32) -> AnyElement {
    div()
        .flex_none()
        .font_family(gpui_code_font_family())
        .text_size(px(10.5))
        .font_weight(FontWeight(600.))
        .text_color(rgb(color))
        .child(label.into())
        .into_any_element()
}

fn state_color(palette: ThemePalette, state: ServiceState) -> u32 {
    match state {
        ServiceState::Running => palette.success,
        ServiceState::Failed => palette.danger,
        ServiceState::Transitioning => palette.warning,
        ServiceState::Stopped | ServiceState::Unknown => palette.text_dimmed,
    }
}

fn state_label(state: ServiceState) -> String {
    match state {
        ServiceState::Running => t!("hostTools.stateRunning"),
        ServiceState::Stopped => t!("hostTools.stateStopped"),
        ServiceState::Failed => t!("hostTools.stateFailed"),
        ServiceState::Transitioning => t!("hostTools.stateChanging"),
        ServiceState::Unknown => t!("hostTools.stateUnknown"),
    }
    .to_string()
}

fn startup_label(startup: ServiceStartup) -> Option<String> {
    Some(
        match startup {
            ServiceStartup::Enabled => t!("hostTools.startupEnabled"),
            ServiceStartup::Disabled => t!("hostTools.startupDisabled"),
            ServiceStartup::Static => t!("hostTools.startupStatic"),
            ServiceStartup::Masked => t!("hostTools.startupMasked"),
            ServiceStartup::Unknown => return None,
        }
        .to_string(),
    )
}

fn message(chrome: PanelChrome, text: impl Into<SharedString>, icon: &'static str) -> AnyElement {
    div()
        .size_full()
        .bg(chrome.transparent_surface)
        .child(empty_panel_with_icon(text, chrome.palette, icon))
        .into_any_element()
}

/// The states every panel shares before it has anything to list.
fn guard(
    panel: &HostToolsPanel,
    chrome: PanelChrome,
    icon: &'static str,
    has_data: bool,
    cx: &mut Context<HostToolsPanel>,
) -> Option<AnyElement> {
    match &panel.host {
        None => return Some(message(chrome, t!("common.loading").to_string(), icon)),
        Some(HostResolution::Unavailable(reason)) => {
            return Some(message(chrome, reason.clone(), icon));
        }
        Some(HostResolution::Ready(_)) => {}
    }
    if panel.sudo_prompt.is_some() {
        return None;
    }
    if has_data {
        return None;
    }
    match &panel.load {
        LoadState::Idle => Some(message(chrome, t!("common.loading").to_string(), icon)),
        LoadState::Failed(error) => Some(
            div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_3()
                .px_4()
                .bg(chrome.transparent_surface)
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(chrome.palette.text_muted))
                        .text_center()
                        .child(error.clone()),
                )
                .child(
                    NyaButton::new("host-tools-retry", t!("common.retry").to_string())
                        .on_click(cx.listener(|panel, _, _, cx| panel.refresh(cx))),
                )
                .into_any_element(),
        ),
        LoadState::Ready => None,
    }
}

/// The two rows every host panel starts with, laid out like Processes: the
/// search box with the item count (and the sudo shield) on one row, filters and
/// sorting on the next. The panel header already names the panel, so there is
/// no extra title line.
fn top_rows(
    panel: &HostToolsPanel,
    palette: ThemePalette,
    search_id: &'static str,
    count: String,
    filters: AnyElement,
    sort: AnyElement,
    cx: &mut Context<HostToolsPanel>,
) -> Vec<AnyElement> {
    let elevated = panel.elevated;
    let epoch = panel.jobs.epoch;
    let shield = panel.supports_elevation().then(|| {
        icon_button(
            palette,
            "host-tools-elevate",
            "icons/shield.svg",
            if elevated {
                t!("hostTools.elevatedOn").to_string()
            } else {
                t!("hostTools.elevatedOff").to_string()
            },
            !panel.jobs.pending(Lane::Action),
            Some(if elevated {
                palette.warning
            } else {
                palette.text_muted
            }),
            cx.listener(move |panel, _, _, cx| {
                if epoch == panel.jobs.epoch {
                    panel.toggle_elevation(cx);
                }
            }),
        )
    });
    vec![
        div()
            .flex_none()
            .h(px(32.))
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(NyaSearchInput::new(search_id, &panel.search)),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(px(11.))
                    .text_color(rgb(palette.text_muted))
                    .child(count),
            )
            .children(shield)
            .into_any_element(),
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap_2()
            .child(div().flex_1().min_w_0().child(filters))
            .child(sort)
            .into_any_element(),
    ]
}

fn sudo_banner(
    panel: &HostToolsPanel,
    palette: ThemePalette,
    cx: &mut Context<HostToolsPanel>,
) -> Option<AnyElement> {
    let epoch = panel.jobs.epoch;
    let reason = panel.sudo_prompt.clone()?;
    Some(
        div()
            .flex_none()
            .flex()
            .flex_col()
            .gap_2()
            .p_2()
            .rounded_md()
            .border_1()
            .border_color(rgb(palette.warning).alpha(0.5))
            .bg(rgb(palette.warning).alpha(0.08))
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(palette.text))
                    .child(reason),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h(px(32.))
                            .child(NyaInputShell::new("host-tools-sudo", &panel.sudo_input)),
                    )
                    .child(
                        NyaButton::new("host-tools-sudo-ok", t!("hostTools.authorize").to_string())
                            .variant(NyaButtonVariant::Primary)
                            .on_click(cx.listener(move |panel, _, _, cx| {
                                if epoch == panel.jobs.epoch {
                                    panel.submit_sudo(cx);
                                }
                            })),
                    )
                    .child(
                        NyaButton::new("host-tools-sudo-cancel", t!("common.cancel").to_string())
                            .variant(NyaButtonVariant::Ghost)
                            .on_click(cx.listener(move |panel, _, _, cx| {
                                if epoch == panel.jobs.epoch {
                                    panel.cancel_sudo(cx);
                                }
                            })),
                    ),
            )
            .into_any_element(),
    )
}

fn status_line(panel: &HostToolsPanel, palette: ThemePalette) -> Option<AnyElement> {
    let (text, color) = if let Some(busy) = panel.busy_label() {
        (busy.clone(), palette.text_muted)
    } else {
        let (text, is_error) = panel.status.clone()?;
        (
            text,
            if is_error {
                palette.danger
            } else {
                palette.success
            },
        )
    };
    Some(
        div()
            .flex_none()
            .text_size(px(11.))
            .text_color(rgb(color))
            .truncate()
            .child(text)
            .into_any_element(),
    )
}

fn frame(chrome: PanelChrome, children: Vec<AnyElement>) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .size_full()
        .min_w_0()
        .relative()
        .overflow_hidden()
        .p(px(10.))
        .gap_2()
        .bg(chrome.transparent_surface)
        .children(children)
        .into_any_element()
}

fn sort_menu(id: &'static str, label: String, items: Vec<NyaMenuItem>) -> AnyElement {
    div()
        .flex_none()
        .child(NyaDropdownMenu::new(id).label(label).items(items))
        .into_any_element()
}

// ------------------------------------------------------------------ ports --

pub(super) fn ports_panel(
    panel: &mut HostToolsPanel,
    chrome: PanelChrome,
    _window: &mut Window,
    cx: &mut Context<HostToolsPanel>,
) -> AnyElement {
    let palette = chrome.palette;
    if let Some(early) = guard(
        panel,
        chrome,
        "icons/ports.svg",
        !panel.ports.is_empty(),
        cx,
    ) {
        return early;
    }
    let view = panel.ports_view;
    let total = panel.ports.len();
    let shown = panel.ports_rows.len();

    let count = if panel.query.trim().is_empty()
        && view.protocol == PortProtocolFilter::All
        && view.scope == PortScopeFilter::All
    {
        total.to_string()
    } else {
        format!("{shown} / {total}")
    };

    let chips = div()
        .flex_none()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_1()
        .children(
            [
                (
                    PortProtocolFilter::All,
                    t!("hostTools.filterAll").to_string(),
                ),
                (PortProtocolFilter::Tcp, "TCP".to_string()),
                (PortProtocolFilter::Udp, "UDP".to_string()),
            ]
            .map(|(filter, label)| {
                chip(
                    palette,
                    format!("ports-proto-{label}"),
                    label,
                    view.protocol == filter,
                    cx.listener(move |panel, _, _, cx| {
                        panel.ports_view.protocol = filter;
                        panel.rederive();
                        cx.notify();
                    }),
                )
            }),
        )
        .child(div().w(px(6.)))
        .children(
            [
                (PortScopeFilter::All, t!("hostTools.filterAll").to_string()),
                (
                    PortScopeFilter::Exposed,
                    t!("hostTools.scopeExposed").to_string(),
                ),
                (
                    PortScopeFilter::Loopback,
                    t!("hostTools.scopeLocal").to_string(),
                ),
            ]
            .map(|(filter, label)| {
                chip(
                    palette,
                    format!("ports-scope-{label}"),
                    label,
                    view.scope == filter,
                    cx.listener(move |panel, _, _, cx| {
                        panel.ports_view.scope = filter;
                        panel.rederive();
                        cx.notify();
                    }),
                )
            }),
        );

    let sort_items = PortSortKey::ALL
        .into_iter()
        .map(|key| {
            let label = match key {
                PortSortKey::Port => t!("hostTools.sortPort"),
                PortSortKey::Process => t!("hostTools.sortProcess"),
                PortSortKey::Protocol => t!("hostTools.sortProtocol"),
                PortSortKey::Pid => t!("hostTools.sortPid"),
                PortSortKey::User => t!("hostTools.sortUser"),
            }
            .to_string();
            NyaMenuItem::action(label)
                .checked(view.sort == key)
                .on_click(cx.listener(move |panel, _, _, cx| {
                    if panel.ports_view.sort == key {
                        panel.ports_view.direction = panel.ports_view.direction.flipped();
                    } else {
                        panel.ports_view.sort = key;
                        panel.ports_view.direction = SortDirection::Ascending;
                    }
                    panel.rederive();
                    cx.notify();
                }))
        })
        .collect::<Vec<_>>();
    let sort_label = format!(
        "{} {}",
        match view.sort {
            PortSortKey::Port => t!("hostTools.sortPort"),
            PortSortKey::Process => t!("hostTools.sortProcess"),
            PortSortKey::Protocol => t!("hostTools.sortProtocol"),
            PortSortKey::Pid => t!("hostTools.sortPid"),
            PortSortKey::User => t!("hostTools.sortUser"),
        },
        view.direction.marker()
    );

    let mut children = top_rows(
        panel,
        palette,
        "host-tools-ports-search",
        count,
        chips.into_any_element(),
        sort_menu("ports-sort", sort_label, sort_items),
        cx,
    );
    children.extend(sudo_banner(panel, palette, cx));
    if panel.ports_owners_hidden && !panel.elevated && panel.supports_elevation() {
        children.push(
            div()
                .flex_none()
                .text_size(px(10.5))
                .text_color(rgb(palette.text_dimmed))
                .child(t!("hostTools.ownersHidden").to_string())
                .into_any_element(),
        );
    }
    children.extend(status_line(panel, palette));

    let body = if shown == 0 {
        empty_panel_with_icon(
            t!("hostTools.noMatches").to_string(),
            palette,
            "icons/ports.svg",
        )
        .into_any_element()
    } else {
        let list = list(
            panel.list.clone(),
            cx.processor(move |panel, index: usize, _, cx| {
                let Some(entry) = panel.ports_rows.get(index).cloned() else {
                    return div().into_any_element();
                };
                div()
                    .w_full()
                    .pr(px(12.))
                    .child(port_row(panel, palette, entry, cx))
                    .into_any_element()
            }),
        )
        .size_full();
        div()
            .id("host-tools-ports-scroll")
            .relative()
            .size_full()
            .overflow_hidden()
            .child(list)
            .vertical_scrollbar(&panel.list)
            .into_any_element()
    };
    children.push(
        div()
            .flex_1()
            .min_h(px(96.))
            .min_w_0()
            .overflow_hidden()
            .child(body)
            .into_any_element(),
    );
    frame(chrome, children)
}

fn port_row(
    panel: &HostToolsPanel,
    palette: ThemePalette,
    entry: PortRow,
    cx: &mut Context<HostToolsPanel>,
) -> AnyElement {
    let key = entry.key();
    let selected = panel.selected.as_deref() == Some(key.as_str());
    let details = selected
        .then(|| port_details_block(panel, palette))
        .flatten();
    let exposure = entry.exposure();
    let (exposure_label, exposure_color) = match exposure {
        PortExposure::AllInterfaces => {
            (t!("hostTools.exposurePublic").to_string(), palette.warning)
        }
        PortExposure::Interface => (t!("hostTools.exposureInterface").to_string(), palette.link),
        PortExposure::Loopback => (t!("hostTools.exposureLocal").to_string(), palette.success),
    };
    let bind = entry.bind_summary();
    let detail = [
        Some(entry.protocol.label().to_string()),
        Some(bind).filter(|bind| !bind.is_empty()),
        entry.pid.map(|pid| format!("pid {pid}")),
        entry.user.clone(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ");
    // With a hidden owner the port number still names the likely service.
    let title = match (&entry.process, well_known_service(entry.port)) {
        (Some(process), _) => process.clone(),
        (None, Some(service)) => format!("{service} · {}", t!("hostTools.unknownOwner")),
        (None, None) => t!("hostTools.unknownOwner").to_string(),
    };
    let owner_hidden = entry.process.is_none();
    let click_key = key.clone();
    let epoch = panel.jobs.epoch;

    let header = div()
        .id(SharedString::from(format!("port-row-{key}")))
        .h(px(PORT_ROW_HEIGHT - 4.))
        .w_full()
        .min_w_0()
        .px_2()
        .flex()
        .flex_none()
        .items_center()
        .gap_2()
        .cursor_pointer()
        .hover(move |this| this.bg(rgb(palette.hover)))
        .on_click(cx.listener(move |panel, _, _, cx| {
            if epoch == panel.jobs.epoch {
                panel.select(click_key.clone(), cx);
            }
        }))
        .child(
            div()
                .w(px(58.))
                .flex_none()
                .font_family(gpui_code_font_family())
                .text_size(px(15.))
                .font_weight(FontWeight(700.))
                .text_color(rgb(palette.text))
                .child(entry.port.to_string()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .truncate()
                        .text_size(px(12.))
                        .font_weight(FontWeight(600.))
                        .text_color(rgb(if owner_hidden {
                            palette.text_muted
                        } else {
                            palette.text
                        }))
                        .child(title),
                )
                .child(
                    div()
                        .truncate()
                        .text_size(px(10.5))
                        .text_color(rgb(palette.text_dimmed))
                        .child(detail),
                ),
        )
        .child(badge(exposure_label, exposure_color))
        .child(div().flex_none().child(port_menu(panel, &entry, cx)))
        .into_any_element();
    card(palette, selected, header, details)
}

impl HostToolsPanel {
    pub(super) fn confirm_kill(
        &mut self,
        pid: u32,
        force: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let panel = cx.weak_entity();
        let epoch = self.jobs.epoch;
        let Some(app) = self.app.upgrade() else {
            return;
        };
        let title = if force {
            t!("hostTools.killTitle").to_string()
        } else {
            t!("hostTools.stopTitle").to_string()
        };
        let description = t!(
            if force {
                "hostTools.confirmKill"
            } else {
                "hostTools.confirmStop"
            },
            pid = pid
        )
        .to_string();
        let confirm = if force {
            t!("hostTools.killProcess").to_string()
        } else {
            t!("hostTools.stopProcess").to_string()
        };
        app.update(cx, |app, cx| {
            app.open_confirm_dialog(
                (
                    title,
                    description,
                    confirm,
                    true,
                    move |_app, _window, cx| {
                        let _ = panel.update(cx, |panel, cx| {
                            panel.kill_pid_for_epoch(epoch, pid, force, cx)
                        });
                        true
                    },
                ),
                window,
                cx,
            );
        });
    }
}

// --------------------------------------------------------------- services --

pub(super) fn services_panel(
    panel: &mut HostToolsPanel,
    chrome: PanelChrome,
    _window: &mut Window,
    cx: &mut Context<HostToolsPanel>,
) -> AnyElement {
    let palette = chrome.palette;
    if let Some(early) = guard(
        panel,
        chrome,
        "icons/services.svg",
        !panel.services.is_empty(),
        cx,
    ) {
        return early;
    }
    let view = panel.services_view;
    let counts = count_services(&panel.services);
    let shown = panel.services_rows.len();
    let chips = div()
        .flex_none()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_1()
        .children(
            [
                (
                    ServiceStateFilter::All,
                    t!("hostTools.filterAll").to_string(),
                    counts.total,
                ),
                (
                    ServiceStateFilter::Running,
                    t!("hostTools.stateRunning").to_string(),
                    counts.running,
                ),
                (
                    ServiceStateFilter::Stopped,
                    t!("hostTools.stateStopped").to_string(),
                    counts.stopped,
                ),
                (
                    ServiceStateFilter::Failed,
                    t!("hostTools.stateFailed").to_string(),
                    counts.failed,
                ),
            ]
            .map(|(filter, label, count)| {
                chip(
                    palette,
                    format!("services-state-{label}"),
                    format!("{label} {count}"),
                    view.state == filter,
                    cx.listener(move |panel, _, _, cx| {
                        panel.services_view.state = filter;
                        panel.rederive();
                        cx.notify();
                    }),
                )
            }),
        );

    let sort_name = |key: ServiceSortKey| {
        match key {
            ServiceSortKey::Name => t!("hostTools.sortName"),
            ServiceSortKey::State => t!("hostTools.sortState"),
            ServiceSortKey::Startup => t!("hostTools.sortStartup"),
            ServiceSortKey::Pid => t!("hostTools.sortPid"),
        }
        .to_string()
    };
    let sort_items = ServiceSortKey::ALL
        .into_iter()
        .map(|key| {
            NyaMenuItem::action(sort_name(key))
                .checked(view.sort == key)
                .on_click(cx.listener(move |panel, _, _, cx| {
                    if panel.services_view.sort == key {
                        panel.services_view.direction = panel.services_view.direction.flipped();
                    } else {
                        panel.services_view.sort = key;
                        panel.services_view.direction = SortDirection::Ascending;
                    }
                    panel.rederive();
                    cx.notify();
                }))
        })
        .collect::<Vec<_>>();
    let sort_label = format!("{} {}", sort_name(view.sort), view.direction.marker());

    let mut children = top_rows(
        panel,
        palette,
        "host-tools-services-search",
        counts.total.to_string(),
        chips.into_any_element(),
        sort_menu("services-sort", sort_label, sort_items),
        cx,
    );
    if panel.services_partial {
        children.push(
            div()
                .flex_none()
                .text_size(px(10.5))
                .text_color(rgb(palette.text_dimmed))
                .child(t!("hostTools.partialSupport").to_string())
                .into_any_element(),
        );
    }
    children.extend(sudo_banner(panel, palette, cx));
    children.extend(status_line(panel, palette));

    let body = if shown == 0 {
        empty_panel_with_icon(
            t!("hostTools.noMatches").to_string(),
            palette,
            "icons/services.svg",
        )
        .into_any_element()
    } else {
        let list = list(
            panel.list.clone(),
            cx.processor(move |panel, index: usize, _, cx| {
                let Some(entry) = panel.services_rows.get(index).cloned() else {
                    return div().into_any_element();
                };
                div()
                    .w_full()
                    .pr(px(12.))
                    .child(service_row(panel, palette, entry, cx))
                    .into_any_element()
            }),
        )
        .size_full();
        div()
            .id("host-tools-services-scroll")
            .relative()
            .size_full()
            .overflow_hidden()
            .child(list)
            .vertical_scrollbar(&panel.list)
            .into_any_element()
    };
    children.push(
        div()
            .flex_1()
            .min_h(px(96.))
            .min_w_0()
            .overflow_hidden()
            .child(body)
            .into_any_element(),
    );
    frame(chrome, children)
}

fn service_row(
    panel: &HostToolsPanel,
    palette: ThemePalette,
    entry: ServiceEntry,
    cx: &mut Context<HostToolsPanel>,
) -> AnyElement {
    let selected = panel.selected.as_deref() == Some(entry.key().as_str());
    let details = selected
        .then(|| service_details_block(panel, palette, cx))
        .flatten();
    let color = state_color(palette, entry.state);
    let click_name = entry.key();
    let epoch = panel.jobs.epoch;

    let mut meta = vec![state_label(entry.state)];
    if let Some(startup) = startup_label(entry.startup) {
        meta.push(startup);
    }
    if let Some(pid) = entry.pid {
        meta.push(format!("pid {pid}"));
    }
    let description = if entry.description.is_empty() {
        entry.detail.clone()
    } else {
        entry.description.clone()
    };

    let header = div()
        .id(SharedString::from(format!("service-row-{}", entry.key())))
        .h(px(SERVICE_ROW_HEIGHT - 4.))
        .w_full()
        .min_w_0()
        .px_2()
        .flex()
        .flex_none()
        .items_center()
        .gap_2()
        .cursor_pointer()
        .hover(move |this| this.bg(rgb(palette.hover)))
        .on_click(cx.listener(move |panel, _, _, cx| {
            if epoch == panel.jobs.epoch {
                panel.select(click_name.clone(), cx);
            }
        }))
        .child(div().size(px(8.)).flex_none().rounded_full().bg(rgb(color)))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .truncate()
                        .font_family(gpui_code_font_family())
                        .text_size(px(12.))
                        .font_weight(FontWeight(700.))
                        .text_color(rgb(palette.text))
                        .child(entry.name.clone()),
                )
                .child(
                    div()
                        .truncate()
                        .text_size(px(10.5))
                        .text_color(rgb(palette.text_dimmed))
                        .child(if description.is_empty() {
                            meta.join(" · ")
                        } else {
                            format!("{} · {}", meta.join(" · "), description)
                        }),
                ),
        )
        .child(div().flex_none().child(service_menu(panel, &entry, cx)))
        .into_any_element();
    card(palette, selected, header, details)
}

fn logs_drawer(
    panel: &HostToolsPanel,
    palette: ThemePalette,
    cx: &mut Context<HostToolsPanel>,
) -> AnyElement {
    let Some(drawer) = panel.logs.as_ref() else {
        return div().into_any_element();
    };
    let epoch = panel.jobs.epoch;
    let text = drawer.lines.join("\n");
    let title: SharedString = drawer.title.clone();
    let loading = panel.jobs.pending_key(Lane::Logs, &drawer.entry.key());
    let lines = drawer.lines.clone();
    let entry = drawer.entry.clone();
    div()
        .flex_none()
        .h(px(190.))
        .flex()
        .flex_col()
        .rounded_md()
        .border_1()
        .border_color(rgb(palette.border))
        .bg(rgb(palette.terminal_bg))
        .child(
            div()
                .h(px(28.))
                .flex_none()
                .px_2()
                .flex()
                .items_center()
                .gap_1()
                .border_b_1()
                .border_color(rgb(palette.border))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(px(11.))
                        .font_weight(FontWeight(700.))
                        .text_color(rgb(palette.text))
                        .child(format!("{} · {}", t!("hostTools.logs"), title)),
                )
                .child(icon_button(
                    palette,
                    "logs-refresh",
                    "icons/fe/refresh.svg",
                    t!("common.refresh").to_string(),
                    !loading,
                    None,
                    cx.listener(move |panel, _, _, cx| {
                        if epoch == panel.jobs.epoch {
                            panel.open_logs(entry.clone(), cx);
                        }
                    }),
                ))
                .child(icon_button(
                    palette,
                    "logs-copy",
                    "icons/copy.svg",
                    t!("hostTools.copyLogs").to_string(),
                    !loading,
                    None,
                    cx.listener(move |panel, _, _, cx| panel.copy(text.clone(), cx)),
                ))
                .child(icon_button(
                    palette,
                    "logs-close",
                    "icons/close.svg",
                    t!("common.close").to_string(),
                    true,
                    None,
                    cx.listener(move |panel, _, _, cx| {
                        if epoch != panel.jobs.epoch {
                            return;
                        }
                        panel.close_logs(cx);
                    }),
                )),
        )
        .child(
            div()
                .id("host-tools-logs-scroll")
                .flex_1()
                .min_h_0()
                .overflow_y_scrollbar()
                .px_2()
                .py_1()
                .font_family(gpui_code_font_family())
                .text_size(px(10.5))
                .text_color(rgb(palette.terminal_fg))
                .children(if loading {
                    vec![SharedString::from(t!("common.loading").to_string())]
                } else {
                    lines
                }),
        )
        .into_any_element()
}

// ---------------------------------------------------------------- details --

fn kv(palette: ThemePalette, label: String, value: impl IntoElement) -> AnyElement {
    div()
        .flex()
        .items_start()
        .gap_2()
        .child(
            div()
                .w(px(78.))
                .flex_none()
                .text_size(px(10.5))
                .text_color(rgb(palette.text_dimmed))
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(11.5))
                .text_color(rgb(palette.text))
                .child(value),
        )
        .into_any_element()
}

fn mono(text: impl Into<SharedString>) -> gpui::Div {
    div()
        .font_family(gpui_code_font_family())
        .child(text.into())
}

/// A list row and, when selected, its details. Flat like the Processes rows: a
/// hairline between rows and a thin accent bar on the selected one.
fn card(
    palette: ThemePalette,
    selected: bool,
    header: AnyElement,
    details: Option<AnyElement>,
) -> AnyElement {
    div()
        .relative()
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .border_b_1()
        .border_color(rgb(palette.surface_elevated))
        .bg(if selected {
            rgb(palette.hover)
        } else {
            rgb(palette.surface)
        })
        .when(selected, |this| {
            this.child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .bottom_0()
                    .w(px(2.))
                    .bg(rgb(palette.primary)),
            )
        })
        .child(header)
        .when_some(details, |this, details| this.child(details))
        .into_any_element()
}

fn inline_details(
    palette: ThemePalette,
    body: Vec<AnyElement>,
    actions: Vec<AnyElement>,
) -> AnyElement {
    inline_details_with(palette, body, actions, Vec::new())
}

fn inline_details_with(
    palette: ThemePalette,
    body: Vec<AnyElement>,
    actions: Vec<AnyElement>,
    extra: Vec<AnyElement>,
) -> AnyElement {
    div()
        .w_full()
        .min_w_0()
        .border_t_1()
        .border_color(rgb(palette.border))
        .bg(rgb(palette.bg))
        .px_2()
        .py_2()
        .flex()
        .flex_col()
        .gap_2()
        .children(body)
        .when(!actions.is_empty(), |this| {
            this.child(div().flex().flex_wrap().gap_2().children(actions))
        })
        .children(extra)
        .into_any_element()
}

fn bind_text(address: &str) -> String {
    match bind_kind(address) {
        BindKind::AllV4 => t!("hostTools.bindAllV4"),
        BindKind::AllV6 => t!("hostTools.bindAllV6"),
        BindKind::AllDual => t!("hostTools.bindAllDual"),
        BindKind::LoopV4 => t!("hostTools.bindLoopV4"),
        BindKind::LoopV6 => t!("hostTools.bindLoopV6"),
        BindKind::IfaceV4 => t!("hostTools.bindIfaceV4"),
        BindKind::IfaceV6 => t!("hostTools.bindIfaceV6"),
        BindKind::LinkLocal => t!("hostTools.bindLinkLocal"),
        BindKind::Mapped => t!("hostTools.bindMapped"),
    }
    .to_string()
}

/// The most useful address to copy: a specific interface, else the bare port.
fn copy_address_of(row: &PortRow) -> String {
    row.addresses
        .iter()
        .find(|a| !matches!(a.as_str(), "*" | "0.0.0.0" | "::" | ""))
        .map(|a| {
            if a.contains(':') {
                format!("[{a}]:{}", row.port)
            } else {
                format!("{a}:{}", row.port)
            }
        })
        .unwrap_or_else(|| row.port.to_string())
}

/// The "⋯" menu at the right of a port row, like the one on process rows.
fn port_menu(
    panel: &HostToolsPanel,
    row: &PortRow,
    cx: &mut Context<HostToolsPanel>,
) -> AnyElement {
    NyaDropdownMenu::new(format!("port-menu-{}", row.key()))
        .icon("icons/conn/more.svg")
        .icon_size(px(14.))
        .tooltip(t!("common.more").to_string())
        .min_width(px(190.))
        .items(port_menu_items(panel, row, cx))
        .on_trigger(|_, _, cx| cx.stop_propagation())
        .into_any_element()
}

pub(super) fn port_menu_items(
    panel: &HostToolsPanel,
    row: &PortRow,
    cx: &mut Context<HostToolsPanel>,
) -> Vec<NyaMenuItem> {
    let key = row.key();
    let pid = row.pid;
    let epoch = panel.jobs.epoch;
    let posix = panel.context().is_some_and(|host| {
        nyaterm_remote_ops::process_control::ProcessControlCapabilities::for_os(host.os)
            .ensure_signal("TERM")
            .is_ok()
    });
    let address = copy_address_of(row);
    let command = panel
        .port_details
        .as_ref()
        .filter(|state| state.key == key)
        .and_then(|state| state.details.command.clone());
    let mut items = vec![
        NyaMenuItem::action(t!("hostTools.copyAddress").to_string())
            .icon("icons/copy.svg")
            .on_click(cx.listener(move |panel, _, _, cx| panel.copy(address.clone(), cx))),
        NyaMenuItem::action(t!("hostTools.copyPid").to_string())
            .disabled(pid.is_none())
            .on_click(cx.listener(move |panel, _, _, cx| {
                if let Some(pid) = pid {
                    panel.copy(pid.to_string(), cx);
                }
            })),
        NyaMenuItem::action(t!("hostTools.copyCommand").to_string())
            .disabled(command.is_none())
            .on_click(cx.listener(move |panel, _, _, cx| {
                if let Some(command) = command.clone() {
                    panel.copy(command, cx);
                }
            })),
        NyaMenuItem::separator(),
        NyaMenuItem::action(t!("hostTools.killProcess").to_string())
            .icon("icons/close.svg")
            .danger()
            .disabled(pid.is_none())
            .on_click(cx.listener(move |panel, _, window, cx| {
                if epoch == panel.jobs.epoch
                    && let Some(pid) = pid
                {
                    panel.confirm_kill(pid, true, window, cx);
                }
            })),
    ];
    if posix {
        items.insert(
            items.len() - 1,
            NyaMenuItem::action(t!("hostTools.stopProcess").to_string())
                .icon("icons/stop.svg")
                .disabled(pid.is_none())
                .on_click(cx.listener(move |panel, _, window, cx| {
                    if epoch == panel.jobs.epoch
                        && let Some(pid) = pid
                    {
                        panel.confirm_kill(pid, false, window, cx);
                    }
                })),
        );
    }
    items
}

/// The "⋯" menu at the right of a service row.
fn service_menu(
    panel: &HostToolsPanel,
    entry: &ServiceEntry,
    cx: &mut Context<HostToolsPanel>,
) -> AnyElement {
    let running = matches!(
        entry.state,
        ServiceState::Running | ServiceState::Transitioning
    );
    let enabled = entry.startup == ServiceStartup::Enabled;
    let can_toggle_startup = matches!(
        entry.startup,
        ServiceStartup::Enabled | ServiceStartup::Disabled
    );
    let idle = !panel.jobs.pending(Lane::Action);
    let name = entry.name.clone();
    let epoch = panel.jobs.epoch;
    let act = |action: ServiceAction| {
        let entry = entry.clone();
        cx.listener(move |panel, _, _, cx| {
            panel.service_action_for_epoch(epoch, entry.clone(), action, cx)
        })
    };
    let logs_entry = entry.clone();
    NyaDropdownMenu::new(format!("service-menu-{}", entry.key()))
        .icon("icons/conn/more.svg")
        .icon_size(px(14.))
        .tooltip(t!("common.more").to_string())
        .min_width(px(200.))
        .items([
            NyaMenuItem::action(t!("hostTools.start").to_string())
                .icon("icons/play.svg")
                .disabled(!(idle && !running))
                .on_click(act(ServiceAction::Start)),
            NyaMenuItem::action(t!("hostTools.stop").to_string())
                .icon("icons/stop.svg")
                .disabled(!(idle && running))
                .on_click(act(ServiceAction::Stop)),
            NyaMenuItem::action(t!("hostTools.restart").to_string())
                .icon("icons/fe/refresh.svg")
                .disabled(!(idle && running))
                .on_click(act(ServiceAction::Restart)),
            NyaMenuItem::separator(),
            NyaMenuItem::action(if enabled {
                t!("hostTools.disableStartup").to_string()
            } else {
                t!("hostTools.enableStartup").to_string()
            })
            .disabled(!(idle && can_toggle_startup))
            .on_click(act(if enabled {
                ServiceAction::Disable
            } else {
                ServiceAction::Enable
            })),
            NyaMenuItem::separator(),
            NyaMenuItem::action(t!("hostTools.logs").to_string())
                .icon("icons/menu/article.svg")
                .on_click(cx.listener(move |panel, _, _, cx| {
                    if epoch == panel.jobs.epoch {
                        panel.open_logs(logs_entry.clone(), cx);
                    }
                })),
            NyaMenuItem::action(t!("hostTools.copyName").to_string())
                .icon("icons/copy.svg")
                .on_click(cx.listener(move |panel, _, _, cx| panel.copy(name.clone(), cx))),
        ])
        .on_trigger(|_, _, cx| cx.stop_propagation())
        .into_any_element()
}

fn port_details_block(panel: &HostToolsPanel, palette: ThemePalette) -> Option<AnyElement> {
    let row = panel.selected_port()?.clone();
    let key = row.key();
    let loaded = panel.port_details.as_ref().filter(|state| state.key == key);
    let loading = panel.jobs.pending_key(Lane::Details, &key);
    let fetched = loaded
        .map(|state| state.details.clone())
        .unwrap_or_default();
    let dash = || "—".to_string();

    let (exposure_text, exposure_color) = match row.exposure() {
        PortExposure::AllInterfaces => (t!("hostTools.explainPublic"), palette.warning),
        PortExposure::Interface => (t!("hostTools.explainInterface"), palette.link),
        PortExposure::Loopback => (t!("hostTools.explainLocal"), palette.success),
    };
    let service = well_known_service(row.port);
    let user = fetched.user.clone().or_else(|| row.user.clone());
    let process_name = row
        .process
        .clone()
        .unwrap_or_else(|| t!("hostTools.unknownOwner").to_string());

    let mut body = vec![
        kv(
            palette,
            t!("hostTools.sortProcess").to_string(),
            process_name,
        ),
        kv(
            palette,
            t!("hostTools.sortPid").to_string(),
            row.pid.map(|pid| pid.to_string()).unwrap_or_else(dash),
        ),
        kv(
            palette,
            t!("hostTools.sortUser").to_string(),
            match (&user, loading) {
                (Some(user), _) => user.clone(),
                (None, true) => t!("hostTools.detailLoading").to_string(),
                (None, false) => dash(),
            },
        ),
    ];
    if let Some(error) = loaded.and_then(|state| state.error.as_ref()) {
        body.push(
            div()
                .text_size(px(11.))
                .text_color(rgb(palette.danger))
                .child(error.clone())
                .into_any_element(),
        );
    }
    if let Some(command) = &fetched.command {
        body.push(kv(
            palette,
            t!("hostTools.detailCommand").to_string(),
            mono(command.clone()).text_size(px(10.5)),
        ));
    } else if loading {
        body.push(kv(
            palette,
            t!("hostTools.detailCommand").to_string(),
            t!("hostTools.detailLoading").to_string(),
        ));
    }
    if let Some(service) = service {
        body.push(kv(
            palette,
            t!("hostTools.detailService").to_string(),
            service.to_string(),
        ));
    }
    let mut binds = div().flex().flex_col().gap(px(5.));
    for address in &row.addresses {
        let shown = if address == "*" {
            "*".to_string()
        } else {
            address.clone()
        };
        binds = binds.child(
            div()
                .flex()
                .flex_col()
                .child(mono(format!("{shown}:{}", row.port)).font_weight(FontWeight(700.)))
                .child(
                    div()
                        .text_size(px(10.5))
                        .text_color(rgb(palette.text_muted))
                        .child(bind_text(address)),
                ),
        );
    }
    body.push(kv(
        palette,
        t!("hostTools.detailBindings").to_string(),
        binds,
    ));
    body.push(
        div()
            .text_size(px(11.))
            .text_color(rgb(exposure_color))
            .child(exposure_text.to_string())
            .into_any_element(),
    );

    Some(inline_details(palette, body, Vec::new()))
}

fn service_details_block(
    panel: &HostToolsPanel,
    palette: ThemePalette,
    cx: &mut Context<HostToolsPanel>,
) -> Option<AnyElement> {
    let entry = panel.selected_service()?.clone();
    let dash = || "—".to_string();

    let mut body = vec![kv(
        palette,
        t!("hostTools.sortName").to_string(),
        mono(entry.name.clone()).font_weight(FontWeight(700.)),
    )];
    if !entry.description.is_empty() {
        body.push(kv(
            palette,
            t!("hostTools.detailDescription").to_string(),
            entry.description.clone(),
        ));
    }
    body.push(kv(
        palette,
        t!("hostTools.sortState").to_string(),
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .size(px(8.))
                    .rounded_full()
                    .bg(rgb(state_color(palette, entry.state))),
            )
            .child(if entry.detail.is_empty() {
                state_label(entry.state)
            } else {
                format!("{} · {}", state_label(entry.state), entry.detail)
            }),
    ));
    body.push(kv(
        palette,
        t!("hostTools.sortStartup").to_string(),
        startup_label(entry.startup).unwrap_or_else(dash),
    ));
    body.push(kv(
        palette,
        t!("hostTools.sortPid").to_string(),
        entry.pid.map(|pid| pid.to_string()).unwrap_or_else(dash),
    ));
    if !entry.scope.is_empty() {
        body.push(kv(
            palette,
            t!("hostTools.detailDomain").to_string(),
            mono(entry.scope.clone()),
        ));
    }

    let mut extra: Vec<AnyElement> = Vec::new();
    if panel
        .logs
        .as_ref()
        .is_some_and(|logs| logs.entry.key() == entry.key())
    {
        extra.push(logs_drawer(panel, palette, cx));
    }
    Some(inline_details_with(palette, body, Vec::new(), extra))
}
