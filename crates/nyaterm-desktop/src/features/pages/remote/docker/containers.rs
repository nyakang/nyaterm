use gpui::{
    Context, FontWeight, IntoElement, MouseButton, ScrollDelta, ScrollWheelEvent, SharedString,
    div, prelude::*, px, rgb, rgba,
};
use nyaterm_core::truncate_preview;
use nyaterm_transport::DockerContainer;

use super::super::panels::RemoteMonitorPanel;
use crate::features::remote::{DOCKER_VIEWPORT_ROWS, state_scrolled_list_range};
use crate::features::{
    formatting::compact_id, formatting::docker_state_color, formatting::docker_state_rank,
    shell::gpui_code_font_family,
};
use crate::models::{DockerConfirmAction, DockerConfirmState};
use crate::theme::ThemePalette;
use crate::widgets::{empty_panel, status_pill, svg_icon_button};

use super::{DockerRenderContext, docker_menu_layer};

pub(in crate::features::pages::remote) struct DockerContainersPanelState<'a> {
    pub has_snapshot: bool,
    pub has_session: bool,
    pub docker_available: bool,
    pub filtered_containers: &'a [DockerContainer],
    pub query_empty: bool,
    pub open_menu_id: Option<&'a str>,
    pub list_offset: usize,
}

pub(in crate::features::pages::remote) fn docker_containers_panel(
    context: DockerRenderContext,
    state: DockerContainersPanelState<'_>,
    cx: &mut Context<RemoteMonitorPanel>,
) -> impl IntoElement {
    let DockerRenderContext {
        palette,
        ref labels,
        ..
    } = context;
    let DockerContainersPanelState {
        has_snapshot,
        has_session,
        docker_available,
        filtered_containers,
        query_empty,
        open_menu_id,
        list_offset,
    } = state;
    // Tauri Docker containers tab: dense ~66px rows, left accent, ⋮ action menu.
    if !has_snapshot {
        return div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(empty_panel(
                if has_session {
                    labels.error.clone()
                } else {
                    labels.no_session.clone()
                },
                palette,
            ))
            .into_any_element();
    }
    if !docker_available {
        return div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(empty_panel(labels.unavailable.clone(), palette))
            .into_any_element();
    }
    if filtered_containers.is_empty() {
        return div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(empty_panel(
                if query_empty {
                    labels.no_containers.clone()
                } else {
                    labels.no_matches.clone()
                },
                palette,
            ))
            .into_any_element();
    }

    let mut containers = filtered_containers.to_vec();
    containers.sort_by(|left, right| {
        docker_state_rank(&left.state)
            .cmp(&docker_state_rank(&right.state))
            .then(left.name.cmp(&right.name))
    });

    // Wheel-driven row window. The offset replaces the rendered slice; this container
    // does not have a native scroll position that could consume spacer padding.
    const DOCKER_ROW_PX: f32 = 66.;
    const DOCKER_OVERSCAN: usize = 6;
    let total = containers.len();
    let visible_range =
        state_scrolled_list_range(total, list_offset, DOCKER_VIEWPORT_ROWS, DOCKER_OVERSCAN);

    let mut rows = div().flex().flex_col().gap(px(6.));
    for container in containers.get(visible_range).unwrap_or(&[]).iter().cloned() {
        let menu_open = open_menu_id == Some(container.id.as_str());
        rows = rows.child(docker_container_row(
            context.clone(),
            container,
            menu_open,
            cx,
        ));
    }
    div()
        .id(SharedString::from("docker-containers-scroll"))
        .size_full()
        .overflow_hidden()
        .flex()
        .flex_col()
        .on_scroll_wheel(cx.listener(move |panel, event: &ScrollWheelEvent, _, cx| {
            panel.with_app(cx, |this, cx| {
                let max_offset = total.saturating_sub(DOCKER_VIEWPORT_ROWS.min(total));
                if max_offset == 0 {
                    return;
                }
                let delta_rows = match event.delta {
                    ScrollDelta::Lines(delta) => delta.y,
                    ScrollDelta::Pixels(delta) => f32::from(delta.y) / DOCKER_ROW_PX,
                };
                let current = this.remote_ops.docker_presentation().list_offset;
                let next = (current as f32 - delta_rows)
                    .round()
                    .clamp(0., max_offset as f32) as usize;
                if this.remote_ops.set_docker_list_offset(next) {
                    cx.stop_propagation();
                    cx.notify();
                }
            });
        }))
        .child(rows)
        .into_any_element()
}

fn docker_container_row(
    context: DockerRenderContext,
    container: DockerContainer,
    menu_open: bool,
    cx: &mut Context<RemoteMonitorPanel>,
) -> impl IntoElement {
    let DockerRenderContext {
        palette,
        ref labels,
        ..
    } = context;
    let container_id = container.id.clone();
    let details_id = container.id.clone();
    let menu_id = container.id.clone();
    let state = container.state.clone();
    let running = state.trim().eq_ignore_ascii_case("running");
    let can_start = can_start_docker_container(&state);
    let can_stop = can_stop_docker_container(&state);
    let accent = docker_state_border_color(palette, &state);
    let short = compact_id(&container.id);

    div()
        .id(SharedString::from(format!("docker-container-{short}")))
        .relative()
        .h(px(60.))
        .rounded_md()
        .border_1()
        .border_color(rgb(palette.border))
        // Left accent bar painted as absolute child.
        .bg(rgba((palette.surface_elevated << 8) | 0x08))
        .hover(move |this| this.bg(rgba((palette.link << 8) | 0x0f)))
        .cursor_pointer()
        .overflow_hidden()
        .child(
            // Left state accent
            div()
                .absolute()
                .left_0()
                .top_0()
                .bottom_0()
                .w(px(3.))
                .bg(accent),
        )
        .child(
            div()
                .size_full()
                .px_3()
                .py_2()
                .pl(px(12.))
                .pr(px(36.))
                .flex()
                .flex_col()
                .justify_center()
                .gap(px(6.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .text_size(px(12.))
                                .font_weight(FontWeight(600.))
                                .text_color(rgb(palette.text))
                                .overflow_hidden()
                                .child(truncate_preview(&container.name, 40)),
                        )
                        .child(status_pill(
                            labels.state_label(&container.state),
                            docker_state_color(palette, &container.state),
                            rgb(0x17233a),
                        )),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .font_family(gpui_code_font_family())
                        .text_size(px(10.))
                        .text_color(rgb(palette.text_dimmed))
                        .child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .overflow_hidden()
                                .child(truncate_preview(&container.image, 48)),
                        )
                        .child(div().flex_none().child(short.clone())),
                ),
        )
        .on_click(cx.listener(move |panel, _, _window, cx| {
            panel.with_app(cx, |this, cx| {
                this.remote_ops.close_docker_container_menu();
                this.load_docker_details(details_id.clone(), cx);
            });
        }))
        .child(
            div().absolute().top(px(8.)).right(px(6.)).child(
                div()
                    .relative()
                    .child(svg_icon_button(
                        format!("docker-menu-toggle-{short}"),
                        "icons/session/more.svg",
                        14.,
                        palette,
                        cx.listener(move |panel, _, _, cx| {
                            panel.with_app(cx, |this, cx| {
                                cx.stop_propagation();
                                this.remote_ops
                                    .toggle_docker_container_menu(menu_id.clone());
                                cx.notify();
                            });
                        }),
                    ))
                    .when(menu_open, |this| {
                        this.child(docker_menu_layer(docker_container_action_menu(
                            context,
                            container_id.clone(),
                            container.name.clone(),
                            running,
                            can_start,
                            can_stop,
                            cx,
                        )))
                    }),
            ),
        )
}

fn docker_container_action_menu(
    context: DockerRenderContext,
    container_id: String,
    container_name: String,
    running: bool,
    can_start: bool,
    can_stop: bool,
    cx: &mut Context<RemoteMonitorPanel>,
) -> impl IntoElement {
    let DockerRenderContext {
        palette,
        menu_bg,
        labels,
    } = context;
    let short = compact_id(&container_id);
    let logs_id = container_id.clone();
    let enter_id = container_id.clone();
    let start_id = container_id.clone();
    let stop_id = container_id.clone();
    let restart_id = container_id.clone();
    let kill_id = container_id.clone();
    let remove_id = container_id.clone();
    let kill_name = container_name.clone();
    let remove_name = container_name;

    div()
        .id(SharedString::from(format!("docker-menu-{short}")))
        .w(px(148.))
        .rounded_md()
        .border_1()
        .border_color(rgb(palette.border))
        .bg(menu_bg)
        .shadow_lg()
        .py_1()
        .flex()
        .flex_col()
        .occlude()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(docker_menu_item(
            palette,
            format!("docker-menu-logs-{short}"),
            labels.logs.clone(),
            false,
            cx.listener(move |panel, _, _, cx| {
                panel.with_app(cx, |this, cx| {
                    this.remote_ops.close_docker_container_menu();
                    this.send_docker_container_logs_to_terminal(logs_id.clone(), cx);
                });
            }),
        ))
        .child(docker_menu_item(
            palette,
            format!("docker-menu-enter-{short}"),
            labels.enter.clone(),
            !running,
            cx.listener(move |panel, _, _, cx| {
                panel.with_app(cx, |this, cx| {
                    this.remote_ops.close_docker_container_menu();
                    this.enter_docker_container_terminal(enter_id.clone(), cx);
                });
            }),
        ))
        .child(docker_menu_separator(palette))
        .child(docker_menu_item(
            palette,
            format!("docker-menu-start-{short}"),
            labels.start.clone(),
            !can_start,
            cx.listener(move |panel, _, window, cx| {
                panel.with_app(cx, |this, cx| {
                    this.remote_ops.close_docker_container_menu();
                    this.docker_container_action(start_id.clone(), "start", window, cx);
                });
            }),
        ))
        .child(docker_menu_item(
            palette,
            format!("docker-menu-stop-{short}"),
            labels.stop.clone(),
            !can_stop,
            cx.listener(move |panel, _, window, cx| {
                panel.with_app(cx, |this, cx| {
                    this.remote_ops.close_docker_container_menu();
                    this.docker_container_action(stop_id.clone(), "stop", window, cx);
                });
            }),
        ))
        .child(docker_menu_item(
            palette,
            format!("docker-menu-restart-{short}"),
            labels.restart.clone(),
            false,
            cx.listener(move |panel, _, window, cx| {
                panel.with_app(cx, |this, cx| {
                    this.remote_ops.close_docker_container_menu();
                    this.docker_container_action(restart_id.clone(), "restart", window, cx);
                });
            }),
        ))
        .child(docker_menu_separator(palette))
        .child(docker_menu_item(
            palette,
            format!("docker-menu-kill-{short}"),
            labels.kill.clone(),
            !running,
            cx.listener({
                let kill_labels = labels.clone();
                move |panel, _, window, cx| {
                    panel.with_app(cx, |this, cx| {
                        this.remote_ops.close_docker_container_menu();
                        let target = if kill_name.trim().is_empty() {
                            compact_id(&kill_id)
                        } else {
                            kill_name.clone()
                        };
                        this.request_docker_confirm(
                            DockerConfirmState {
                                title: kill_labels.confirm_action_title.to_string(),
                                detail: kill_labels.confirm_description(&kill_labels.kill, &target),
                                action: DockerConfirmAction::ContainerAction {
                                    container_id: kill_id.clone(),
                                    action: "kill",
                                },
                            },
                            window,
                            cx,
                        );
                    });
                }
            }),
        ))
        .child(docker_menu_item(
            palette,
            format!("docker-menu-remove-{short}"),
            labels.delete.clone(),
            false,
            cx.listener({
                let remove_labels = labels.clone();
                move |panel, _, window, cx| {
                    panel.with_app(cx, |this, cx| {
                        this.remote_ops.close_docker_container_menu();
                        let target = if remove_name.trim().is_empty() {
                            compact_id(&remove_id)
                        } else {
                            remove_name.clone()
                        };
                        this.request_docker_confirm(
                            DockerConfirmState {
                                title: remove_labels.confirm_action_title.to_string(),
                                detail: remove_labels
                                    .confirm_description(&remove_labels.delete, &target),
                                action: DockerConfirmAction::ContainerAction {
                                    container_id: remove_id.clone(),
                                    action: "remove",
                                },
                            },
                            window,
                            cx,
                        );
                    });
                }
            }),
        ))
}

fn can_start_docker_container(state: &str) -> bool {
    matches!(
        state.trim().to_ascii_lowercase().as_str(),
        "created" | "exited"
    )
}

fn can_stop_docker_container(state: &str) -> bool {
    matches!(
        state.trim().to_ascii_lowercase().as_str(),
        "running" | "restarting"
    )
}

fn docker_menu_item(
    palette: ThemePalette,
    id: impl Into<String>,
    label: impl Into<SharedString>,
    disabled: bool,
    on_click: impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    let label: SharedString = label.into();
    div()
        .id(SharedString::from(id.into()))
        .h(px(28.))
        .px_3()
        .flex()
        .items_center()
        .text_size(px(12.))
        .text_color(if disabled {
            rgb(palette.border)
        } else {
            rgb(palette.text)
        })
        .when(!disabled, |this| {
            this.cursor_pointer()
                .hover(|s| s.bg(rgb(palette.surface_elevated)))
                .on_click(on_click)
        })
        .when(disabled, |this| this.opacity(0.5))
        .child(label)
}

fn docker_menu_separator(palette: ThemePalette) -> impl IntoElement {
    div().h(px(1.)).mx_2().my_1().bg(rgb(palette.border))
}

fn docker_state_border_color(palette: ThemePalette, state: &str) -> gpui::Hsla {
    match state.trim().to_ascii_lowercase().as_str() {
        "running" => rgb(0x22c55e).into(),
        "restarting" | "paused" => rgb(0xf59e0b).into(),
        "exited" | "dead" => rgb(0xef4444).into(),
        "created" => rgb(0x3b82f6).into(),
        _ => rgb(palette.text_dimmed).into(),
    }
}

#[cfg(test)]
mod tests {
    use super::{can_start_docker_container, can_stop_docker_container};

    #[test]
    fn docker_container_actions_follow_runtime_state() {
        for state in ["created", " exited ", "CREATED"] {
            assert!(can_start_docker_container(state));
            assert!(!can_stop_docker_container(state));
        }
        for state in ["running", " Restarting ", "RUNNING"] {
            assert!(!can_start_docker_container(state));
            assert!(can_stop_docker_container(state));
        }
        for state in ["paused", "dead", "unknown"] {
            assert!(!can_start_docker_container(state));
            assert!(!can_stop_docker_container(state));
        }
    }
}
