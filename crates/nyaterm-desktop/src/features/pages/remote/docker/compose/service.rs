use super::super::super::panels::RemoteMonitorPanel;
use super::super::DockerRenderContext;
use super::menus::{DockerComposeServiceMenu, docker_compose_service_action_menu};
use super::status::{compose_status_color, compose_status_label};
use crate::features::{formatting::compact_id, shell::gpui_code_font_family};
use crate::widgets::{small_button, status_pill, svg_icon_button};
use gpui::{Context, FontWeight, IntoElement, SharedString, div, prelude::*, px, rgb};
use nyaterm_core::truncate_preview;
use nyaterm_remote_ops::docker::DockerComposeService;
use rust_i18n::t;

pub(super) struct DockerComposeServicesPanel<'a> {
    pub project_name: String,
    pub config_files: Option<String>,
    pub project_key: String,
    pub services: Option<Vec<DockerComposeService>>,
    pub error: Option<String>,
    pub open_menu_id: Option<&'a str>,
}

struct DockerComposeServiceRow {
    project_name: String,
    config_files: Option<String>,
    service: DockerComposeService,
    menu_open: bool,
    menu_id: String,
}

pub(super) fn docker_compose_services_panel(
    context: DockerRenderContext,
    panel: DockerComposeServicesPanel<'_>,
    cx: &mut Context<RemoteMonitorPanel>,
) -> impl IntoElement {
    let DockerRenderContext {
        palette,
        ref labels,
        ..
    } = context;
    let DockerComposeServicesPanel {
        project_name,
        config_files,
        project_key,
        services,
        error,
        open_menu_id,
    } = panel;
    let mut rows = div()
        .border_t_1()
        .border_color(rgb(palette.border))
        .px_2()
        .pb_2()
        .pt_1()
        .flex()
        .flex_col()
        .gap_1();
    if let Some(_error) = error {
        rows = rows.child(
            div()
                .h(px(36.))
                .px_2()
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .text_size(px(11.))
                        .text_color(rgb(0xfca5a5))
                        .overflow_hidden()
                        .child(labels.service_load_failed.clone()),
                )
                .child(small_button(
                    palette,
                    format!("docker-compose-retry-{project_name}"),
                    labels.retry.clone(),
                    cx.listener({
                        let project_name = project_name.clone();
                        let config_files = config_files.clone();
                        move |panel, _, window, cx| {
                            panel.with_app(cx, |this, cx| {
                                this.remote_ops.close_docker_compose_menu();
                                this.load_docker_compose_services(
                                    project_name.clone(),
                                    config_files.clone(),
                                    window,
                                    cx,
                                );
                            });
                        }
                    }),
                )),
        );
    } else if let Some(services) = services {
        if services.is_empty() {
            rows = rows.child(
                div()
                    .h(px(40.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(12.))
                    .text_color(rgb(palette.text_dimmed))
                    .child(labels.no_services.clone()),
            );
        } else {
            for service in services {
                let service_menu_id = format!("compose-service:{project_key}:{}", service.name);
                let menu_open = open_menu_id == Some(service_menu_id.as_str());
                rows = rows.child(docker_compose_service_row(
                    context.clone(),
                    DockerComposeServiceRow {
                        project_name: project_name.clone(),
                        config_files: config_files.clone(),
                        service,
                        menu_open,
                        menu_id: service_menu_id,
                    },
                    cx,
                ));
            }
        }
    } else {
        rows = rows.child(
            div()
                .h(px(36.))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(11.))
                .text_color(rgb(palette.text_muted))
                .child(labels.loading_services.clone()),
        );
    }
    rows
}

fn docker_compose_service_row(
    context: DockerRenderContext,
    row: DockerComposeServiceRow,
    cx: &mut Context<RemoteMonitorPanel>,
) -> impl IntoElement {
    let DockerRenderContext {
        palette,
        ref labels,
        ..
    } = context;
    let DockerComposeServiceRow {
        project_name,
        config_files,
        service,
        menu_open,
        menu_id,
    } = row;
    let container_summary = if service.containers.is_empty() {
        labels.no_containers.to_string()
    } else {
        service
            .containers
            .iter()
            .take(3)
            .map(|container| {
                let name = if container.name.trim().is_empty() {
                    compact_id(&container.id)
                } else {
                    truncate_preview(&container.name, 24)
                };
                format!("{name} {}", labels.state_label(&container.state))
            })
            .collect::<Vec<_>>()
            .join(" · ")
    };
    let service_name = service.name.clone();
    let service_status_label = compose_status_label(&service.status);
    let service_status_color = compose_status_color(palette, service_status_label);
    let display_status = if service.status.trim().is_empty() {
        labels.not_created.clone()
    } else {
        labels.state_label(service_status_label)
    };
    let running_container_id = service
        .containers
        .iter()
        .filter(|container| container.state.eq_ignore_ascii_case("running"))
        .min_by(|left, right| left.name.cmp(&right.name).then(left.id.cmp(&right.id)))
        .map(|container| container.id.clone());
    let can_enter = running_container_id.is_some();
    let row_id = format!("docker-compose-service-{project_name}-{service_name}");

    div()
        .id(SharedString::from(row_id.clone()))
        .relative()
        .h(px(58.))
        .rounded_md()
        .bg(rgb(palette.bg))
        .hover(|this| this.bg(rgb(0x151b24)))
        .px_2()
        .pr(px(36.))
        .flex()
        .items_center()
        .child(
            div()
                .min_w_0()
                .flex_1()
                .flex()
                .flex_col()
                .gap_1()
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
                                .child(truncate_preview(&service.name, 42)),
                        )
                        .child(status_pill(
                            display_status,
                            service_status_color,
                            rgb(0x17233a),
                        )),
                )
                .child(
                    div()
                        .font_family(gpui_code_font_family())
                        .text_size(px(10.))
                        .text_color(rgb(palette.text_dimmed))
                        .overflow_hidden()
                        .child(truncate_preview(&container_summary, 72)),
                ),
        )
        .child(
            div().absolute().top(px(14.)).right(px(4.)).child(
                div()
                    .relative()
                    .child(svg_icon_button(
                        format!("{row_id}-menu"),
                        "icons/session/more.svg",
                        14.,
                        t!("dockerManager.moreActions").to_string(),
                        palette,
                        cx.listener({
                            let menu_id = menu_id.clone();
                            move |panel, _, _, cx| {
                                panel.with_app(cx, |this, cx| {
                                    cx.stop_propagation();
                                    this.remote_ops.toggle_docker_compose_menu(menu_id.clone());
                                    cx.notify();
                                });
                            }
                        }),
                    ))
                    .when(menu_open, |this| {
                        this.child(super::super::docker_menu_layer(
                            docker_compose_service_action_menu(
                                context,
                                DockerComposeServiceMenu {
                                    project_name,
                                    config_files,
                                    service_name,
                                    running_container_id,
                                    can_enter,
                                },
                                cx,
                            ),
                        ))
                    }),
            ),
        )
}
