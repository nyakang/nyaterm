use rust_i18n::t;

use gpui::{
    Context, IntoElement, MouseButton, MouseDownEvent, Window, WindowControlArea, div, prelude::*,
    px, rgb, svg,
};
use nyaterm_transport::SessionKind;
use nyaterm_ui::NyaDropdownMenu;
use time::{OffsetDateTime, UtcOffset, Weekday, macros::format_description};

use crate::features::{
    NyaTermApp, formatting::format_rate, formatting::format_uptime, formatting::short_id,
    icons::IconDef, icons::resolve_connection_icon, view_widgets::connection_type_icon,
    view_widgets::logo_mark, view_widgets::window_control_button,
};
use crate::models::HeaderStatusMode;

struct HeaderStatusContent {
    icon: Option<IconDef>,
    label: String,
    parts: Vec<HeaderStatusPart>,
    hardware: Option<HeaderHardwareStatus>,
}

struct HeaderStatusPart {
    icon: IconDef,
    text: String,
    text_color: Option<u32>,
}

struct HeaderHardwareStatus {
    mode: HeaderStatusMode,
    cards: Vec<HeaderHardwareCard>,
    hidden_count: usize,
    page_count: usize,
}

struct HeaderHardwareCard {
    index: String,
    utilization_percent: Option<f64>,
    memory_percent: Option<f64>,
    memory_text: String,
}

impl HeaderStatusContent {
    fn simple(icon: IconDef, label: String) -> Self {
        Self {
            icon: Some(icon),
            label,
            parts: Vec::new(),
            hardware: None,
        }
    }

    fn parts(label: String, parts: Vec<HeaderStatusPart>) -> Self {
        Self {
            icon: None,
            label,
            parts,
            hardware: None,
        }
    }

    fn hardware(
        icon: IconDef,
        label: String,
        mode: HeaderStatusMode,
        cards: Vec<HeaderHardwareCard>,
        hidden_count: usize,
        page_count: usize,
    ) -> Self {
        Self {
            icon: Some(icon),
            label,
            parts: Vec::new(),
            hardware: Some(HeaderHardwareStatus {
                mode,
                cards,
                hidden_count,
                page_count,
            }),
        }
    }
}

impl NyaTermApp {
    pub(in crate::features) fn handle_title_bar_mouse_down(
        &mut self,
        _event: &MouseDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.mark_title_drag_activity();
        #[cfg(target_os = "linux")]
        {
            // Linux 的拖动标记不会主动移动窗口；阻止嵌套拖动区重复处理同一次按下。
            cx.stop_propagation();
            if _event.click_count == 2 {
                _window.zoom_window();
            } else {
                _window.start_window_move();
            }
        }
        cx.notify();
    }

    pub(in crate::features) fn title_bar(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let macos = cfg!(target_os = "macos");
        let compact_layout = !cfg!(target_os = "macos");
        let narrow_left = compact_layout && self.shell.viewport_size().0 < 1024.;
        let narrow_right = compact_layout && self.shell.viewport_size().0 < 768.;
        let header_status_visible = self.settings.summary().ui_header_status_visible;
        let header_status = self.header_status_content();
        // Match Tauri Header: h-10.
        div()
            .h(px(40.))
            .flex()
            .items_center()
            .justify_between()
            .border_b_1()
            .border_color(rgb(palette.border))
            .bg(self.shell_surface_color(palette.surface))
            .child(
                div()
                    .h_full()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .when(macos, |this| this.pl(px(78.)))
                    .window_control_area(WindowControlArea::Drag)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(Self::handle_title_bar_mouse_down),
                    )
                    .when(!macos, |this| {
                        this.child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .mr_2()
                                .child(logo_mark(palette)),
                        )
                    })
                    .when(narrow_left, |this| {
                        this.child(
                            div()
                                .id("title-mobile-left")
                                .group("title-mobile-left")
                                .size(px(28.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_sm()
                                .text_color(rgb(palette.text_muted))
                                .cursor_pointer()
                                .hover(|this| {
                                    this.bg(rgb(palette.hover)).text_color(rgb(palette.text))
                                })
                                .child(
                                    svg()
                                        .size(px(16.))
                                        .path("icons/menu/menu.svg")
                                        .text_color(rgb(palette.text_muted))
                                        .group_hover("title-mobile-left", |this| {
                                            this.text_color(rgb(palette.text))
                                        }),
                                )
                                .when(cfg!(target_os = "linux"), |this| {
                                    this.on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation();
                                    })
                                })
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.toggle_mobile_left_drawer(cx);
                                })),
                        )
                    })
                    .when_some(self.shell.title_menu_bar(), |this, menu_bar| {
                        this.child(menu_bar)
                    }),
            )
            .child(
                div()
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .flex_1()
                    .window_control_area(WindowControlArea::Drag)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(Self::handle_title_bar_mouse_down),
                    )
                    .when(header_status_visible, |this| {
                        this.child(self.header_status_control(header_status, cx))
                    }),
            )
            .child(
                div()
                    .h_full()
                    .flex()
                    .items_center()
                    .when(narrow_right, |this| {
                        this.child(
                            div()
                                .id("title-mobile-right")
                                .group("title-mobile-right")
                                .size(px(28.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_sm()
                                .text_color(rgb(palette.text_muted))
                                .cursor_pointer()
                                .hover(|this| {
                                    this.bg(rgb(palette.hover)).text_color(rgb(palette.text))
                                })
                                .child(
                                    svg()
                                        .size(px(16.))
                                        .path("icons/menu/sidebar.svg")
                                        .text_color(rgb(palette.text_muted))
                                        .group_hover("title-mobile-right", |this| {
                                            this.text_color(rgb(palette.text))
                                        }),
                                )
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.toggle_mobile_right_drawer(cx);
                                })),
                        )
                    })
                    .child(
                        div()
                            .w(px(10.))
                            .h_full()
                            .window_control_area(WindowControlArea::Drag)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(Self::handle_title_bar_mouse_down),
                            ),
                    )
                    .when(!macos, |this| {
                        this.child(window_control_button(
                            palette,
                            "window-min",
                            "icons/window/minimize.svg",
                            WindowControlArea::Min,
                            cx.listener(|this, _, window, cx| {
                                this.handle_window_minimize(window, cx);
                            }),
                        ))
                        .child(window_control_button(
                            palette,
                            "window-max",
                            if window.is_maximized() {
                                "icons/window/restore.svg"
                            } else {
                                "icons/window/maximize.svg"
                            },
                            WindowControlArea::Max,
                            |_, window, _| window.zoom_window(),
                        ))
                        .child(window_control_button(
                            palette,
                            "window-close",
                            "icons/window/close.svg",
                            WindowControlArea::Close,
                            cx.listener(|this, _, window, cx| {
                                this.handle_window_close_request(window, cx);
                            }),
                        ))
                    }),
            )
    }

    fn header_status_control(
        &self,
        content: HeaderStatusContent,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let select_label = t!("headerStatus.select");
        let selected =
            HeaderStatusMode::from_setting(&self.settings.summary().ui_header_status_mode);
        let mut items = HeaderStatusMode::ALL
            .into_iter()
            .map(|mode| {
                nyaterm_ui::NyaMenuItem::action(t!(mode.i18n_key()))
                    .checked(selected == mode)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.set_header_status_mode(mode, cx);
                    }))
            })
            .collect::<Vec<_>>();
        items.extend([
            nyaterm_ui::NyaMenuItem::separator(),
            nyaterm_ui::NyaMenuItem::action(t!("headerStatus.hide"))
                .icon("icons/eye-off.svg")
                .on_click(cx.listener(|this, _, window, cx| {
                    this.open_header_status_hide_confirm(window, cx);
                })),
        ]);

        div()
            .max_w(px(520.))
            .flex()
            .items_center()
            .gap_0()
            .rounded_sm()
            .text_xs()
            .text_color(rgb(palette.text_muted))
            .child(
                div()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap_2()
                    .overflow_hidden()
                    .px_2()
                    .py_1()
                    .window_control_area(WindowControlArea::Drag)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(Self::handle_title_bar_mouse_down),
                    )
                    .child(self.header_status_body(content, cx)),
            )
            .child(
                div()
                    .flex_none()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_, _, _, cx| cx.stop_propagation()),
                    )
                    .child(
                        NyaDropdownMenu::new("header-status-menu-trigger")
                            .icon("icons/chevron-down.svg")
                            .icon_size(px(14.))
                            .tooltip(select_label)
                            .min_width(px(196.))
                            .items(items)
                            .on_trigger(cx.listener(|this, _, _, cx| {
                                this.shell.close_open_tabs_menu();
                                this.shell.close_new_session_menu();
                                cx.notify();
                            })),
                    ),
            )
    }

    fn header_status_body(
        &self,
        content: HeaderStatusContent,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let palette = self.theme_palette();
        if let Some(hardware) = content.hardware {
            let mut cards = div().min_w_0().flex().items_center().gap_2();
            for card in hardware.cards {
                let utilization = format_optional_percent(card.utilization_percent);
                let utilization_color =
                    pressure_color(card.utilization_percent).unwrap_or(palette.text_muted);
                let memory_color =
                    pressure_color(card.memory_percent).unwrap_or(palette.text_muted);
                cards = cards.child(
                    div()
                        .w(px(92.))
                        .flex_none()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .font_family("monospace")
                        .text_size(px(10.))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1()
                                .child(div().w(px(14.)).text_right().child(card.index.clone()))
                                .child(header_mini_progress(
                                    card.utilization_percent,
                                    palette.primary,
                                    palette.border,
                                ))
                                .child(div().text_color(rgb(utilization_color)).child(utilization)),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1()
                                .child(div().w(px(14.)))
                                .child(header_mini_progress(
                                    card.memory_percent,
                                    palette.primary,
                                    palette.border,
                                ))
                                .child(div().text_color(rgb(memory_color)).child(card.memory_text)),
                        ),
                );
            }
            if hardware.page_count > 1 {
                let previous_mode = hardware.mode;
                let next_mode = hardware.mode;
                let hidden = hardware.hidden_count;
                cards = cards.child(
                    div()
                        .flex_none()
                        .flex()
                        .flex_col()
                        .rounded_sm()
                        .border_1()
                        .border_color(rgb(palette.border))
                        .child(
                            div()
                                .id(format!(
                                    "header-hardware-prev-{}",
                                    hardware.mode.persistence_id()
                                ))
                                .size(px(14.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .hover(|this| this.bg(rgb(palette.hover)))
                                .child(svg().size(px(10.)).path("icons/chevron-up.svg"))
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.cycle_header_hardware_page(previous_mode, -1, cx);
                                })),
                        )
                        .child(
                            div()
                                .id(format!(
                                    "header-hardware-next-{}-{hidden}",
                                    hardware.mode.persistence_id()
                                ))
                                .size(px(14.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .border_t_1()
                                .border_color(rgb(palette.border))
                                .hover(|this| this.bg(rgb(palette.hover)))
                                .child(svg().size(px(10.)).path("icons/chevron-down.svg"))
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.cycle_header_hardware_page(next_mode, 1, cx);
                                })),
                        ),
                );
            }
            return div()
                .min_w_0()
                .flex()
                .items_center()
                .gap_1()
                .child(connection_type_icon(
                    palette,
                    content.icon.expect("hardware status icon"),
                    false,
                    14.,
                ))
                .child(
                    div()
                        .flex_none()
                        .text_size(px(11.))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(match hardware.mode {
                            HeaderStatusMode::Gpu => "GPU",
                            HeaderStatusMode::Npu => "NPU",
                            _ => "",
                        }),
                )
                .child(cards)
                .into_any_element();
        }

        if !content.parts.is_empty() {
            let mut row = div().min_w_0().flex().items_center().gap_1();
            for (index, part) in content.parts.into_iter().enumerate() {
                if index > 0 {
                    row = row.child(
                        div()
                            .px(px(2.))
                            .text_color(rgb(palette.text_dimmed))
                            .child("-"),
                    );
                }
                row = row.child(
                    div()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(connection_type_icon(palette, part.icon, false, 13.))
                        .child(
                            div()
                                .min_w_0()
                                .overflow_hidden()
                                .text_color(rgb(part.text_color.unwrap_or(palette.text_muted)))
                                .child(part.text),
                        ),
                );
            }
            return row.into_any_element();
        }

        div()
            .min_w_0()
            .flex()
            .items_center()
            .gap_2()
            .when_some(content.icon, |this, icon| {
                this.child(connection_type_icon(palette, icon, false, 14.))
            })
            .child(div().min_w_0().overflow_hidden().child(content.label))
            .into_any_element()
    }

    fn header_status_content(&self) -> HeaderStatusContent {
        let mode = HeaderStatusMode::from_setting(&self.settings.summary().ui_header_status_mode);
        match mode {
            HeaderStatusMode::Session => HeaderStatusContent::simple(
                self.title_context_icon()
                    .unwrap_or_else(|| header_status_mode_icon(mode)),
                self.title_context_label(),
            ),
            HeaderStatusMode::DateTime => HeaderStatusContent::simple(
                header_status_mode_icon(mode),
                format_header_datetime(local_now(), &self.settings.summary().language),
            ),
            HeaderStatusMode::Resources | HeaderStatusMode::Host => {
                self.remote_stats_header_content(mode)
            }
            HeaderStatusMode::Gpu => self.gpu_header_content(),
            HeaderStatusMode::Npu => self.npu_header_content(),
        }
    }

    fn gpu_header_content(&self) -> HeaderStatusContent {
        let icon = header_status_mode_icon(HeaderStatusMode::Gpu);
        let fallback = |label| HeaderStatusContent::simple(icon, label);
        if self.session.active_ssh_config().is_none() {
            return fallback(t!("gpuMonitor.noSession").to_string());
        }
        if !self.settings.summary().ui_show_gpu_monitor {
            return fallback(t!("panel.gpuMonitorDisabled").to_string());
        }
        let gpu = self.remote_ops.gpu_presentation();
        let Some(overview) = gpu.data else {
            return fallback(if gpu.pending || gpu.consecutive_refresh_failures == 0 {
                t!("common.loading").to_string()
            } else {
                gpu.status
            });
        };
        if !overview.available {
            return fallback(t!("gpuMonitor.unavailable").to_string());
        }
        if overview.gpus.is_empty() {
            return fallback(t!("gpuMonitor.noGpus").to_string());
        }

        let mut cards = overview
            .gpus
            .iter()
            .map(|gpu| HeaderHardwareCard {
                index: gpu.index.to_string(),
                utilization_percent: gpu.utilization_gpu_percent,
                memory_percent: percent_from_parts(gpu.memory_used_mb, gpu.memory_total_mb),
                memory_text: format_memory_mb_compact(gpu.memory_used_mb, gpu.memory_total_mb),
            })
            .collect::<Vec<_>>();
        cards.sort_by_key(|card| card.index.parse::<u32>().unwrap_or(u32::MAX));
        self.paginate_header_hardware(HeaderStatusMode::Gpu, icon, cards)
    }

    fn npu_header_content(&self) -> HeaderStatusContent {
        let icon = header_status_mode_icon(HeaderStatusMode::Npu);
        let fallback = |label| HeaderStatusContent::simple(icon, label);
        if self.session.active_ssh_config().is_none() {
            return fallback(t!("ascendNpuMonitor.noSession").to_string());
        }
        if !self.settings.summary().ui_show_ascend_npu_monitor {
            return fallback(t!("panel.npuMonitorDisabled").to_string());
        }
        let npu = self.remote_ops.npu_presentation();
        let Some(overview) = npu.data else {
            return fallback(if npu.pending || npu.consecutive_refresh_failures == 0 {
                t!("common.loading").to_string()
            } else {
                npu.status
            });
        };
        if !overview.available {
            return fallback(t!("ascendNpuMonitor.unavailable").to_string());
        }
        if overview.npus.is_empty() {
            return fallback(t!("ascendNpuMonitor.noNpus").to_string());
        }

        let mut cards = overview
            .npus
            .iter()
            .map(|npu| {
                let total = npu.hbm_total_mb.unwrap_or(npu.memory_total_mb);
                let used = npu.hbm_used_mb.unwrap_or(npu.memory_used_mb);
                HeaderHardwareCard {
                    index: npu.index.to_string(),
                    utilization_percent: npu.utilization_aicore_percent,
                    memory_percent: percent_from_parts(used, total),
                    memory_text: format_memory_mb_compact(used, total),
                }
            })
            .collect::<Vec<_>>();
        cards.sort_by_key(|card| card.index.parse::<u32>().unwrap_or(u32::MAX));
        self.paginate_header_hardware(HeaderStatusMode::Npu, icon, cards)
    }

    fn remote_stats_header_content(&self, mode: HeaderStatusMode) -> HeaderStatusContent {
        let fallback = || {
            HeaderStatusContent::simple(
                header_status_mode_icon(mode),
                self.remote_stats_header_fallback(),
            )
        };
        if self.session.active_ssh_config().is_none()
            || !self.settings.summary().ui_show_remote_stats
        {
            return fallback();
        }
        let stats_state = self.remote_ops.stats_presentation();
        let Some(stats) = stats_state.data.as_ref() else {
            return fallback();
        };
        if mode == HeaderStatusMode::Host {
            let hostname = stats.system.hostname.trim();
            let hostname = if hostname.is_empty() {
                "remote host"
            } else {
                hostname
            };
            let system = format!("{}/{}", stats.system.os, stats.system.arch);
            let uptime = format_uptime(stats.system.uptime_sec);
            let label = format!("{hostname} - {system} - {uptime}");
            return HeaderStatusContent::parts(
                label,
                vec![
                    HeaderStatusPart {
                        icon: IconDef::mono("icons/conn/server.svg", 0x38bdf8),
                        text: hostname.to_string(),
                        text_color: None,
                    },
                    HeaderStatusPart {
                        icon: IconDef::mono("icons/workspace.svg", 0xa78bfa),
                        text: system,
                        text_color: None,
                    },
                    HeaderStatusPart {
                        icon: IconDef::mono("icons/clock.svg", 0x34d399),
                        text: uptime,
                        text_color: None,
                    },
                ],
            );
        }

        let memory_total = stats.memory.used.saturating_add(stats.memory.available);
        let memory_percent = percent_from_parts(stats.memory.used, memory_total);
        let tx = stats
            .networks
            .iter()
            .map(|network| network.tx_bytes_per_sec.max(0.))
            .sum::<f64>();
        let rx = stats
            .networks
            .iter()
            .map(|network| network.rx_bytes_per_sec.max(0.))
            .sum::<f64>();
        let cpu_text = stats
            .cpu
            .usage
            .map(|usage| format!("CPU {:.0}%", usage.clamp(0., 100.)))
            .unwrap_or_else(|| "CPU --".to_string());
        let memory_text = format!(
            "RAM {}/{}",
            format_binary_bytes(stats.memory.used),
            format_binary_bytes(memory_total)
        );
        let tx_text = format_rate(tx);
        let rx_text = format_rate(rx);
        let label = format!("{cpu_text} - {memory_text} - NET ↑ {tx_text} ↓ {rx_text}");
        HeaderStatusContent::parts(
            label,
            vec![
                HeaderStatusPart {
                    icon: IconDef::mono("icons/resources.svg", 0x38bdf8),
                    text: cpu_text,
                    text_color: pressure_color(stats.cpu.usage),
                },
                HeaderStatusPart {
                    icon: IconDef::mono("icons/processes.svg", 0xa78bfa),
                    text: memory_text,
                    text_color: pressure_color(memory_percent),
                },
                HeaderStatusPart {
                    icon: IconDef::mono("icons/fe/upload.svg", 0xf59e0b),
                    text: tx_text,
                    text_color: None,
                },
                HeaderStatusPart {
                    icon: IconDef::mono("icons/fe/download.svg", 0x34d399),
                    text: rx_text,
                    text_color: None,
                },
            ],
        )
    }

    fn paginate_header_hardware(
        &self,
        mode: HeaderStatusMode,
        icon: IconDef,
        cards: Vec<HeaderHardwareCard>,
    ) -> HeaderStatusContent {
        let limit = hardware_card_limit(self.shell.viewport_size().0).max(1);
        let total = cards.len();
        let page_count = total.div_ceil(limit).max(1);
        let page = self
            .shell
            .header_status_hardware_page(mode)
            .min(page_count.saturating_sub(1));
        let start = page * limit;
        let visible = cards
            .into_iter()
            .skip(start)
            .take(limit)
            .collect::<Vec<_>>();
        let hidden_count = total.saturating_sub(visible.len());
        let label = visible
            .iter()
            .map(|card| {
                format!(
                    "{} {} · {} · {}",
                    if mode == HeaderStatusMode::Gpu {
                        "GPU"
                    } else {
                        "NPU"
                    },
                    card.index,
                    format_optional_percent(card.utilization_percent),
                    card.memory_text
                )
            })
            .collect::<Vec<_>>()
            .join(" | ");
        HeaderStatusContent::hardware(icon, label, mode, visible, hidden_count, page_count)
    }

    fn cycle_header_hardware_page(
        &mut self,
        mode: HeaderStatusMode,
        delta: i32,
        cx: &mut Context<Self>,
    ) {
        let total = match mode {
            HeaderStatusMode::Gpu => self
                .remote_ops
                .gpu_presentation()
                .data
                .map_or(0, |overview| overview.gpus.len()),
            HeaderStatusMode::Npu => self
                .remote_ops
                .npu_presentation()
                .data
                .map_or(0, |overview| overview.npus.len()),
            _ => 0,
        };
        let page_count = total
            .div_ceil(hardware_card_limit(self.shell.viewport_size().0).max(1))
            .max(1);
        let current = self.shell.header_status_hardware_page(mode) % page_count;
        let next = if delta < 0 {
            (current + page_count - 1) % page_count
        } else {
            (current + 1) % page_count
        };
        self.shell.set_header_status_hardware_page(mode, next);
        cx.notify();
    }

    fn remote_stats_header_fallback(&self) -> String {
        if self.session.active_ssh_config().is_none() {
            t!("panel.resourceMonitorNoSession").to_string()
        } else if !self.settings.summary().ui_show_remote_stats {
            t!("panel.resourceMonitorDisabled").to_string()
        } else {
            let stats = self.remote_ops.stats_presentation();
            if stats.consecutive_refresh_failures > 0 && stats.data.is_none() {
                t!("panel.resourceMonitorError").to_string()
            } else {
                t!("common.loading").to_string()
            }
        }
    }

    pub(in crate::features) fn set_header_status_mode(
        &mut self,
        mode: HeaderStatusMode,
        cx: &mut Context<Self>,
    ) {
        self.settings
            .set_header_status_mode(mode.persistence_id().to_string());
        self.shell
            .set_header_status_rendered_minute(current_unix_minute());
        self.persist_header_status_settings();
        self.ensure_header_status_clock(cx);
        cx.notify();
    }

    pub(in crate::features) fn set_header_status_visible(
        &mut self,
        visible: bool,
        cx: &mut Context<Self>,
    ) {
        self.settings.set_header_status_visible(visible);
        self.persist_header_status_settings();
        self.ensure_header_status_clock(cx);
        cx.notify();
    }

    pub(in crate::features) fn open_header_status_hide_confirm(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_confirm_dialog(
            (
                t!("headerStatus.hideConfirmTitle").to_string(),
                t!("headerStatus.hideConfirmDesc").to_string(),
                t!("headerStatus.hideConfirmAction").to_string(),
                false,
                |app, _, cx| {
                    app.set_header_status_visible(false, cx);
                    true
                },
            ),
            window,
            cx,
        );
    }

    /// Both header-status setters funnel through here, so this is the one place the
    /// date/time clock has to be reconsidered when the header changes shape.
    fn persist_header_status_settings(&mut self) {
        if self.shell.has_settings_draft() {
            self.shell
                .set_status("header status changed; apply settings to persist".to_string());
        } else {
            self.persist_ui_layout();
        }
    }

    pub(in crate::features) fn header_status_clock_refresh_due(&self) -> bool {
        self.settings.summary().ui_header_status_visible
            && HeaderStatusMode::from_setting(&self.settings.summary().ui_header_status_mode)
                == HeaderStatusMode::DateTime
            && self.shell.header_status_rendered_minute() != current_unix_minute()
    }

    pub(in crate::features) fn refresh_header_status_clock(&mut self) -> bool {
        if !self.header_status_clock_refresh_due() {
            return false;
        }
        self.shell
            .set_header_status_rendered_minute(current_unix_minute());
        true
    }

    pub(in crate::features) fn title_context_label(&self) -> String {
        if self.session.start_has_active_pending()
            && let Some(pending) = self.session.start_pending_display_name()
        {
            return pending;
        }
        if self.session.start_has_active_failed()
            && let Some(failed) = self.session.start_failed_display_name()
        {
            return failed;
        }
        if let Some(session_id) = self.session.active_id() {
            let tab_root = self.tab_root_for_session(session_id);
            let name = self
                .session
                .display_name(&tab_root)
                .unwrap_or_else(|| short_id(&tab_root).to_string());
            let has_custom_name = self
                .session
                .custom_name(&tab_root)
                .is_some_and(|value| !value.trim().is_empty());
            if !has_custom_name
                && self
                    .session
                    .session_info(session_id)
                    .is_some_and(|session| session.kind == SessionKind::Ssh)
                && let Some(endpoint) = self.session.endpoint(session_id)
            {
                return format!("{name} — {endpoint}");
            }
            return name;
        }
        if let Some(pending) = self.session.start_pending_display_name() {
            return pending;
        }
        if let Some(failed) = self.session.start_failed_display_name() {
            return failed;
        }
        if let Some(failed) = self.shell.last_connect_failure_name() {
            return failed.to_string();
        }
        "NyaTerm".to_string()
    }

    fn title_context_icon(&self) -> Option<IconDef> {
        if self.session.start_has_active_pending() {
            return Some(IconDef::mono("icons/conn/connect.svg", 0x60a5fa));
        }
        if self.session.start_has_active_failed() {
            return Some(IconDef::mono("icons/session/disconnect.svg", 0xf87171));
        }
        if let Some(session_id) = self.session.active_id()
            && let Some(session) = self.session.session_info(session_id)
        {
            let icon_key = self
                .session
                .metadata(session_id)
                .and_then(|metadata| metadata.source_connection_id.as_deref())
                .and_then(|connection_id| self.connection_state.connection_by_id(connection_id))
                .and_then(|connection| connection.icon.as_deref())
                .filter(|icon| !icon.trim().is_empty());
            return Some(session_header_icon(session.kind, icon_key));
        }
        if self.session.start_has_pending() {
            return Some(IconDef::mono("icons/conn/connect.svg", 0x60a5fa));
        }
        if self.session.start_has_failed() || self.shell.last_connect_failure_name().is_some() {
            return Some(IconDef::mono("icons/session/disconnect.svg", 0xf87171));
        }
        None
    }
}

fn pressure_color(value: Option<f64>) -> Option<u32> {
    match value {
        Some(value) if value >= 90. => Some(0xf87171),
        Some(value) if value >= 75. => Some(0xf59e0b),
        _ => None,
    }
}

fn percent_from_parts(used: u64, total: u64) -> Option<f64> {
    (total > 0).then(|| (used as f64 / total as f64 * 100.).clamp(0., 100.))
}

fn format_optional_percent(value: Option<f64>) -> String {
    value.map_or_else(
        || "--".to_string(),
        |value| format!("{:.0}%", value.clamp(0., 100.)),
    )
}

fn format_binary_bytes(bytes: u64) -> String {
    const UNITS: [&str; 7] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];

    if bytes == 0 {
        return "0 B".to_string();
    }

    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024. && unit < UNITS.len() - 1 {
        value /= 1024.;
        unit += 1;
    }

    if unit == 0 {
        format!("{bytes} B")
    } else if value < 100. {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{value:.0} {}", UNITS[unit])
    }
}

fn format_memory_mb_value(value: u64) -> String {
    if value >= 1024 {
        let gib = value as f64 / 1024.;
        if gib < 10. && gib.fract() >= 0.05 {
            format!("{gib:.1}G")
        } else {
            format!("{gib:.0}G")
        }
    } else {
        format!("{value}M")
    }
}

fn format_memory_mb_compact(used: u64, total: u64) -> String {
    if total == 0 {
        "-".to_string()
    } else {
        format!(
            "{}/{}",
            format_memory_mb_value(used),
            format_memory_mb_value(total)
        )
    }
}

fn hardware_card_limit(width: f32) -> usize {
    if width >= 1180. {
        4
    } else if width >= 920. {
        3
    } else if width >= 700. {
        2
    } else {
        1
    }
}

fn header_mini_progress(value: Option<f64>, primary: u32, background: u32) -> impl IntoElement {
    let width = value.unwrap_or(0.).clamp(0., 100.) * 0.3;
    let color = pressure_color(value).unwrap_or(primary);
    div()
        .w(px(30.))
        .h(px(4.))
        .flex_none()
        .overflow_hidden()
        .rounded_full()
        .bg(rgb(background))
        .child(
            div()
                .w(px(width as f32))
                .h_full()
                .rounded_full()
                .bg(rgb(color)),
        )
}

fn current_unix_minute() -> i64 {
    OffsetDateTime::now_utc().unix_timestamp().div_euclid(60)
}

fn session_kind_label(kind: SessionKind) -> &'static str {
    match kind {
        SessionKind::LocalPty => "Local",
        SessionKind::Ssh | SessionKind::RawTcp => "SSH",
        SessionKind::Telnet => "Telnet",
        SessionKind::Serial => "Serial",
        SessionKind::Rdp => "RDP",
        SessionKind::Vnc => "VNC",
    }
}

fn session_header_icon(kind: SessionKind, icon_key: Option<&str>) -> IconDef {
    resolve_connection_icon(icon_key, session_kind_label(kind))
}

fn header_status_mode_icon(mode: HeaderStatusMode) -> IconDef {
    match mode {
        HeaderStatusMode::Session => IconDef::mono("icons/sessions.svg", 0x94a3b8),
        HeaderStatusMode::Resources => IconDef::mono("icons/resources.svg", 0x38bdf8),
        HeaderStatusMode::Host => IconDef::mono("icons/conn/server.svg", 0x38bdf8),
        HeaderStatusMode::DateTime => IconDef::mono("icons/clock.svg", 0x34d399),
        HeaderStatusMode::Gpu => IconDef::mono("icons/gpu.svg", 0x76b900),
        HeaderStatusMode::Npu => IconDef::mono("icons/npu.svg", 0xe60012),
    }
}

fn local_now() -> OffsetDateTime {
    let now = OffsetDateTime::now_utc();
    UtcOffset::current_local_offset().map_or(now, |offset| now.to_offset(offset))
}

fn format_header_datetime(datetime: OffsetDateTime, language: &str) -> String {
    // Resolve every catalog lookup against the stored language explicitly rather
    // than the process-wide locale, so this stays a pure function the tests can
    // drive for any language without racing `rust_i18n::set_locale`.
    let locale = crate::i18n::normalize_locale(language);
    let date = datetime
        .format(format_description!("[year]-[month]-[day]"))
        .unwrap_or_default();
    let time = datetime
        .format(format_description!("[hour]:[minute]"))
        .unwrap_or_default();
    let weekday = localized_weekday(datetime.weekday(), locale.as_ref());
    t!(
        "titleBar.dateTime",
        locale = locale.as_ref(),
        date = date,
        time = time,
        weekday = weekday
    )
    .into_owned()
}

fn localized_weekday(weekday: Weekday, locale: &str) -> String {
    let key = match weekday {
        Weekday::Monday => "titleBar.weekday.monday",
        Weekday::Tuesday => "titleBar.weekday.tuesday",
        Weekday::Wednesday => "titleBar.weekday.wednesday",
        Weekday::Thursday => "titleBar.weekday.thursday",
        Weekday::Friday => "titleBar.weekday.friday",
        Weekday::Saturday => "titleBar.weekday.saturday",
        Weekday::Sunday => "titleBar.weekday.sunday",
    };
    t!(key, locale = locale).into_owned()
}

#[cfg(test)]
mod tests {
    use nyaterm_transport::SessionKind;
    use time::{Date, Month, Time, UtcOffset};

    use crate::models::HeaderStatusMode;

    use super::{
        format_binary_bytes, format_header_datetime, format_memory_mb_compact, hardware_card_limit,
        header_status_mode_icon, localized_weekday, percent_from_parts, pressure_color,
        session_header_icon,
    };

    #[test]
    fn header_status_modes_follow_tauri_radio_order() {
        assert_eq!(
            HeaderStatusMode::ALL.map(HeaderStatusMode::persistence_id),
            ["session", "resources", "host", "datetime", "gpu", "npu"]
        );
    }

    #[test]
    fn status_modes_use_semantic_clock_and_accelerator_icons() {
        assert_eq!(
            header_status_mode_icon(HeaderStatusMode::DateTime).path,
            "icons/clock.svg"
        );
        assert_eq!(
            header_status_mode_icon(HeaderStatusMode::Gpu).path,
            "icons/gpu.svg"
        );
        assert_eq!(
            header_status_mode_icon(HeaderStatusMode::Npu).path,
            "icons/npu.svg"
        );
    }

    #[test]
    fn session_header_prefers_saved_icon_and_falls_back_by_kind() {
        let saved = session_header_icon(SessionKind::Ssh, Some("ubuntu"));
        let default_ssh = session_header_icon(SessionKind::Ssh, None);
        assert_ne!(saved, default_ssh);
        assert_eq!(saved.path, "color/os/ubuntu.svg");
        assert_eq!(
            session_header_icon(SessionKind::Ssh, Some("missing-icon")),
            default_ssh
        );
        assert_eq!(
            session_header_icon(SessionKind::LocalPty, None).path,
            "icons/conn/terminal.svg"
        );
        assert_eq!(
            session_header_icon(SessionKind::Telnet, None).path,
            "icons/conn/telnet.svg"
        );
        assert_eq!(
            session_header_icon(SessionKind::Serial, None).path,
            "icons/conn/serial.svg"
        );
    }

    #[test]
    fn resource_memory_uses_adaptive_binary_units() {
        const KIB: u64 = 1024;
        const MIB: u64 = KIB * 1024;
        const GIB: u64 = MIB * 1024;
        const TIB: u64 = GIB * 1024;

        assert_eq!(format_binary_bytes(0), "0 B");
        assert_eq!(format_binary_bytes(1023), "1023 B");
        assert_eq!(format_binary_bytes(KIB), "1.0 KiB");
        assert_eq!(format_binary_bytes(MIB), "1.0 MiB");
        assert_eq!(format_binary_bytes(128 * MIB), "128 MiB");
        assert_eq!(format_binary_bytes(16 * GIB), "16.0 GiB");
        assert_eq!(format_binary_bytes(31 * GIB + GIB / 2), "31.5 GiB");
        assert_eq!(format_binary_bytes(TIB), "1.0 TiB");
    }

    #[test]
    fn structured_status_formatting_matches_tauri_thresholds() {
        assert_eq!(pressure_color(Some(74.9)), None);
        assert_eq!(pressure_color(Some(75.)), Some(0xf59e0b));
        assert_eq!(pressure_color(Some(90.)), Some(0xf87171));
        assert_eq!(percent_from_parts(1, 4), Some(25.));
        assert_eq!(percent_from_parts(1, 0), None);
        assert_eq!(format_memory_mb_compact(1536, 4096), "1.5G/4G");
        assert_eq!(format_memory_mb_compact(0, 0), "-");
        assert_eq!(hardware_card_limit(699.), 1);
        assert_eq!(hardware_card_limit(700.), 2);
        assert_eq!(hardware_card_limit(920.), 3);
        assert_eq!(hardware_card_limit(1180.), 4);
    }

    #[test]
    fn formats_header_datetime_for_supported_languages() {
        let datetime = Date::from_calendar_date(2026, Month::July, 27)
            .expect("date")
            .with_time(Time::from_hms(9, 5, 0).expect("time"))
            .assume_offset(UtcOffset::from_hms(8, 0, 0).expect("offset"));

        for (locale, expected) in [
            ("en", "Mon, 2026-07-27 09:05"),
            ("zh-CN", "2026-07-27 09:05 周一"),
            ("zh-TW", "2026-07-27 09:05 週一"),
            ("ja", "2026-07-27 09:05 月"),
            ("ko", "2026-07-27 09:05 월"),
            ("fr", "lun. 2026-07-27 09:05"),
        ] {
            assert_eq!(
                format_header_datetime(datetime, locale),
                expected,
                "{locale}"
            );
        }
    }

    #[test]
    fn localizes_every_weekday_without_falling_back() {
        for locale in ["en", "zh-CN", "zh-TW", "ja", "ko", "fr"] {
            for weekday in [
                time::Weekday::Monday,
                time::Weekday::Tuesday,
                time::Weekday::Wednesday,
                time::Weekday::Thursday,
                time::Weekday::Friday,
                time::Weekday::Saturday,
                time::Weekday::Sunday,
            ] {
                let localized = localized_weekday(weekday, locale);
                assert!(!localized.is_empty(), "{locale}/{weekday:?}");
                assert!(!localized.starts_with("titleBar."), "{locale}/{weekday:?}");
            }
        }
    }
}
