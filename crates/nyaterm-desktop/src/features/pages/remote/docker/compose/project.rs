use super::super::super::panels::RemoteMonitorPanel;
use super::super::{DockerRenderContext, resources::docker_resource_static_panel};
use super::menus::docker_compose_project_action_menu;
use super::service::docker_compose_services_panel;
use super::status::{compose_status_color, compose_status_label};
use crate::features::{formatting::docker_compose_project_key, shell::gpui_code_font_family};
use crate::widgets::{empty_panel, status_pill, svg_icon_button};
use gpui::{Context, FontWeight, IntoElement, SharedString, div, prelude::*, px, rgb, svg};
use nyaterm_core::truncate_preview;
use nyaterm_remote_ops::docker::{DockerComposeProject, DockerComposeService};
use rust_i18n::t;
use std::collections::{HashMap, HashSet};

pub(in crate::features::pages::remote) struct DockerComposePanelState<'a> {
    pub projects: &'a [DockerComposeProject],
    pub expanded_projects: &'a HashSet<String>,
    pub services_by_project: &'a HashMap<String, Vec<DockerComposeService>>,
    pub service_errors: &'a HashMap<String, String>,
    pub open_menu_id: Option<&'a str>,
}

struct DockerComposeProjectRow<'a> {
    project: &'a DockerComposeProject,
    project_key: &'a str,
    expanded: bool,
    menu_open: bool,
    menu_id: String,
    services: Option<Vec<DockerComposeService>>,
    error: Option<String>,
    open_menu_id: Option<&'a str>,
}

pub(in crate::features::pages::remote) fn docker_compose_panel(
    context: DockerRenderContext,
    state: DockerComposePanelState<'_>,
    cx: &mut Context<RemoteMonitorPanel>,
) -> impl IntoElement {
    let DockerRenderContext {
        palette,
        ref labels,
        ..
    } = context;
    let DockerComposePanelState {
        projects,
        expanded_projects,
        services_by_project,
        service_errors,
        open_menu_id,
    } = state;
    // Tauri Compose tab: dense project rows (≈74px) + chevron + ⋮ overflow; services ≈58px.
    let mut rows = div().flex().flex_col().gap_1();
    if projects.is_empty() {
        rows = rows.child(empty_panel(labels.no_matches.clone(), palette));
    } else {
        for project in projects {
            let config_files = Some(project.config_files.clone()).filter(|value| {
                !value.trim().is_empty() && !value.trim().eq_ignore_ascii_case("n/a")
            });
            let key = docker_compose_project_key(&project.name, config_files.as_deref());
            let expanded = expanded_projects.contains(&key);
            let services = services_by_project.get(&key).cloned();
            let error = service_errors.get(&key).cloned();
            let project_menu_id = format!("compose-project:{key}");
            let project_menu_open = open_menu_id == Some(project_menu_id.as_str());
            rows = rows.child(docker_compose_project_row(
                context.clone(),
                DockerComposeProjectRow {
                    project,
                    project_key: &key,
                    expanded,
                    menu_open: project_menu_open,
                    menu_id: project_menu_id,
                    services,
                    error,
                    open_menu_id,
                },
                cx,
            ));
        }
    }

    docker_resource_static_panel(palette, "Compose", projects.len(), rows)
}

fn docker_compose_project_row(
    context: DockerRenderContext,
    row: DockerComposeProjectRow<'_>,
    cx: &mut Context<RemoteMonitorPanel>,
) -> impl IntoElement {
    let DockerRenderContext {
        palette,
        ref labels,
        ..
    } = context;
    let DockerComposeProjectRow {
        project,
        project_key,
        expanded,
        menu_open,
        menu_id,
        services,
        error,
        open_menu_id,
    } = row;
    let project_name = project.name.clone();
    let config_files = Some(project.config_files.clone())
        .filter(|value| !value.trim().is_empty() && !value.trim().eq_ignore_ascii_case("n/a"));
    let status_label = compose_status_label(&project.status);
    let status_color = compose_status_color(palette, status_label);
    let display_status = if project.status.trim().is_empty() {
        "-"
    } else {
        &labels.state_label(status_label)
    };
    let key_for_toggle = project_key.to_string();

    div()
        .id(SharedString::from(format!(
            "docker-compose-project-{project_key}"
        )))
        .rounded_md()
        .border_1()
        .border_color(rgb(palette.border))
        .bg(rgb(palette.section_header))
        .hover(|this| this.bg(rgb(palette.hover)))
        .overflow_hidden()
        .flex()
        .flex_col()
        .child(
            div()
                .relative()
                .h(px(74.))
                .px_2()
                .flex()
                .items_start()
                .gap_2()
                .child(
                    div()
                        .id(SharedString::from(format!(
                            "docker-compose-chevron-{project_key}"
                        )))
                        .mt(px(10.))
                        .h(px(24.))
                        .w(px(24.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_md()
                        .text_color(rgb(palette.text_muted))
                        .cursor_pointer()
                        .hover(|this| {
                            this.bg(rgb(palette.surface_elevated))
                                .text_color(rgb(palette.text))
                        })
                        .child(
                            svg()
                                .size(px(14.))
                                .path(if expanded {
                                    "icons/chevron-down.svg"
                                } else {
                                    "icons/fe/forward.svg"
                                })
                                .text_color(rgb(palette.text_muted)),
                        )
                        .on_click(cx.listener({
                            let project_name = project_name.clone();
                            let config_files = config_files.clone();
                            move |panel, _, window, cx| {
                                panel.with_app(cx, |this, cx| {
                                    this.remote_ops.close_docker_compose_menu();
                                    this.toggle_docker_compose_project(
                                        project_name.clone(),
                                        config_files.clone(),
                                        window,
                                        cx,
                                    );
                                });
                            }
                        })),
                )
                .child(
                    div()
                        .id(SharedString::from(format!(
                            "docker-compose-body-{project_key}"
                        )))
                        .min_w_0()
                        .flex_1()
                        .pt(px(8.))
                        .pr(px(34.))
                        .flex()
                        .flex_col()
                        .gap_1()
                        .cursor_pointer()
                        .on_click(cx.listener({
                            let project_name = project_name.clone();
                            let config_files = config_files.clone();
                            move |panel, _, window, cx| {
                                panel.with_app(cx, |this, cx| {
                                    this.remote_ops.close_docker_compose_menu();
                                    this.toggle_docker_compose_project(
                                        project_name.clone(),
                                        config_files.clone(),
                                        window,
                                        cx,
                                    );
                                });
                            }
                        }))
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
                                        .font_weight(FontWeight(700.))
                                        .text_color(rgb(palette.text))
                                        .overflow_hidden()
                                        .child(truncate_preview(&project.name, 42)),
                                )
                                .child(status_pill(display_status, status_color, rgb(0x17233a))),
                        )
                        .child(
                            div()
                                .font_family(gpui_code_font_family())
                                .text_size(px(10.))
                                .text_color(rgb(palette.text_dimmed))
                                .overflow_hidden()
                                .child(truncate_preview(&project.config_files, 64)),
                        ),
                )
                .child(
                    div().absolute().top(px(8.)).right(px(6.)).child(
                        div()
                            .relative()
                            .child(svg_icon_button(
                                format!("docker-compose-menu-{project_key}"),
                                "icons/session/more.svg",
                                14.,
                                t!("dockerManager.moreActions").to_string(),
                                palette,
                                cx.listener({
                                    let menu_id = menu_id.clone();
                                    move |panel, _, _, cx| {
                                        panel.with_app(cx, |this, cx| {
                                            cx.stop_propagation();
                                            this.remote_ops
                                                .toggle_docker_compose_menu(menu_id.clone());
                                            cx.notify();
                                        });
                                    }
                                }),
                            ))
                            .when(menu_open, |this| {
                                this.child(super::super::docker_menu_layer(
                                    docker_compose_project_action_menu(
                                        context.clone(),
                                        project_name.clone(),
                                        config_files.clone(),
                                        &key_for_toggle,
                                        cx,
                                    ),
                                ))
                            }),
                    ),
                ),
        )
        .when(expanded, |this| {
            this.child(docker_compose_services_panel(
                context,
                super::service::DockerComposeServicesPanel {
                    project_name,
                    config_files,
                    project_key: project_key.to_string(),
                    services,
                    error,
                    open_menu_id,
                },
                cx,
            ))
        })
}
