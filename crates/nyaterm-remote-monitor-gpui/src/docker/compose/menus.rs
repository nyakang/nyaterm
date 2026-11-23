use crate::actions::RemoteMonitorAction;
use gpui::{Context, IntoElement, MouseButton, SharedString, div, prelude::*, px, rgb};

use super::super::super::panels::RemoteMonitorPanel;
use crate::models::{DockerConfirmAction, DockerConfirmState};
use nyaterm_ui::ThemePalette;

use super::super::DockerRenderContext;

pub(super) struct DockerComposeServiceMenu {
    pub project_name: String,
    pub config_files: Option<String>,
    pub service_name: String,
    pub running_container_id: Option<String>,
    pub can_enter: bool,
}

pub(super) fn docker_compose_project_action_menu(
    context: DockerRenderContext,
    project_name: String,
    config_files: Option<String>,
    project_key: &str,
    cx: &mut Context<RemoteMonitorPanel>,
) -> impl IntoElement {
    let DockerRenderContext {
        palette,
        menu_bg,
        labels,
    } = context;
    let short = project_key.replace(['/', ':', ' '], "-");
    div()
        .id(SharedString::from(format!(
            "docker-compose-project-menu-{short}"
        )))
        .w(px(140.))
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
        .child(compose_menu_item(
            palette,
            format!("compose-up-{short}"),
            labels.up.clone(),
            false,
            cx.listener({
                let project_name = project_name.clone();
                let config_files = config_files.clone();
                move |panel, _, window, cx| {
                    {
                        panel.dispatch(RemoteMonitorAction::CloseComposeMenu, cx);
                        panel.dispatch_in(
                            RemoteMonitorAction::ComposeAction(
                                project_name.clone(),
                                config_files.clone(),
                                "up",
                            ),
                            window,
                            cx,
                        );
                    };
                }
            }),
        ))
        .child(compose_menu_item(
            palette,
            format!("compose-restart-{short}"),
            labels.restart.clone(),
            false,
            cx.listener({
                let project_name = project_name.clone();
                let config_files = config_files.clone();
                move |panel, _, window, cx| {
                    {
                        panel.dispatch(RemoteMonitorAction::CloseComposeMenu, cx);
                        panel.dispatch_in(
                            RemoteMonitorAction::ComposeAction(
                                project_name.clone(),
                                config_files.clone(),
                                "restart",
                            ),
                            window,
                            cx,
                        );
                    };
                }
            }),
        ))
        .child(compose_menu_separator(palette))
        .child(compose_menu_item(
            palette,
            format!("compose-down-{short}"),
            labels.down.clone(),
            false,
            cx.listener(move |panel, _, window, cx| {
                {
                    panel.dispatch(RemoteMonitorAction::CloseComposeMenu, cx);
                    panel.dispatch_in(
                        RemoteMonitorAction::ConfirmDocker(DockerConfirmState {
                            title: labels.confirm_action_title.to_string(),
                            detail: labels.confirm_description(&labels.down, &project_name),
                            action: DockerConfirmAction::ComposeAction {
                                project_name: project_name.clone(),
                                config_files: config_files.clone(),
                                action: "down",
                            },
                        }),
                        window,
                        cx,
                    );
                };
            }),
        ))
}

pub(super) fn docker_compose_service_action_menu(
    context: DockerRenderContext,
    menu: DockerComposeServiceMenu,
    cx: &mut Context<RemoteMonitorPanel>,
) -> impl IntoElement {
    let DockerRenderContext {
        palette,
        menu_bg,
        labels,
    } = context;
    let DockerComposeServiceMenu {
        project_name,
        config_files,
        service_name,
        running_container_id,
        can_enter,
    } = menu;
    let short = format!("{project_name}-{service_name}").replace(['/', ':', ' '], "-");
    div()
        .id(SharedString::from(format!(
            "docker-compose-service-menu-{short}"
        )))
        .w(px(140.))
        .rounded_md()
        .border_1()
        .border_color(rgb(palette.border))
        .bg(menu_bg)
        .shadow_lg()
        .py_1()
        .flex()
        .flex_col()
        .occlude()
        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(compose_menu_item(
            palette,
            format!("compose-svc-logs-{short}"),
            labels.logs.clone(),
            false,
            cx.listener({
                let project_name = project_name.clone();
                let config_files = config_files.clone();
                let service_name = service_name.clone();
                move |panel, _, _, cx| {
                    {
                        panel.dispatch(RemoteMonitorAction::CloseComposeMenu, cx);
                        panel.dispatch(
                            RemoteMonitorAction::ComposeServiceLogs(
                                project_name.clone(),
                                config_files.clone(),
                                service_name.clone(),
                            ),
                            cx,
                        );
                    };
                }
            }),
        ))
        .child(compose_menu_item(
            palette,
            format!("compose-svc-enter-{short}"),
            labels.enter.clone(),
            !can_enter,
            cx.listener(move |panel, _, _, cx| {
                {
                    panel.dispatch(RemoteMonitorAction::CloseComposeMenu, cx);
                    if let Some(container_id) = running_container_id.clone() {
                        panel.dispatch(RemoteMonitorAction::ContainerTerminal(container_id), cx);
                    }
                };
            }),
        ))
        .child(compose_menu_separator(palette))
        .child(compose_menu_item(
            palette,
            format!("compose-svc-up-{short}"),
            labels.up.clone(),
            false,
            cx.listener({
                let project_name = project_name.clone();
                let config_files = config_files.clone();
                let service_name = service_name.clone();
                move |panel, _, window, cx| {
                    {
                        panel.dispatch(RemoteMonitorAction::CloseComposeMenu, cx);
                        panel.dispatch_in(
                            RemoteMonitorAction::ComposeServiceAction(
                                project_name.clone(),
                                config_files.clone(),
                                service_name.clone(),
                                "up",
                            ),
                            window,
                            cx,
                        );
                    };
                }
            }),
        ))
        .child(compose_menu_item(
            palette,
            format!("compose-svc-stop-{short}"),
            labels.stop.clone(),
            false,
            cx.listener({
                let project_name = project_name.clone();
                let config_files = config_files.clone();
                let service_name = service_name.clone();
                move |panel, _, window, cx| {
                    {
                        panel.dispatch(RemoteMonitorAction::CloseComposeMenu, cx);
                        panel.dispatch_in(
                            RemoteMonitorAction::ComposeServiceAction(
                                project_name.clone(),
                                config_files.clone(),
                                service_name.clone(),
                                "stop",
                            ),
                            window,
                            cx,
                        );
                    };
                }
            }),
        ))
        .child(compose_menu_item(
            palette,
            format!("compose-svc-restart-{short}"),
            labels.restart.clone(),
            false,
            cx.listener({
                let project_name = project_name.clone();
                let config_files = config_files.clone();
                let service_name = service_name.clone();
                move |panel, _, window, cx| {
                    {
                        panel.dispatch(RemoteMonitorAction::CloseComposeMenu, cx);
                        panel.dispatch_in(
                            RemoteMonitorAction::ComposeServiceAction(
                                project_name.clone(),
                                config_files.clone(),
                                service_name.clone(),
                                "restart",
                            ),
                            window,
                            cx,
                        );
                    };
                }
            }),
        ))
}

pub(crate) fn compose_menu_item(
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

pub(crate) fn compose_menu_separator(palette: ThemePalette) -> impl IntoElement {
    div().h(px(1.)).mx_2().my_1().bg(rgb(palette.border))
}
