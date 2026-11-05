use super::{DockerLabels, docker_labels};
use crate::features::{NyaTermApp, formatting::docker_state_color, shell::gpui_code_font_family};
use crate::theme::ThemePalette;
use crate::widgets::{small_button, status_pill};
use gpui::{
    Context, Entity, FontWeight, IntoElement, Render, SharedString, Subscription, Window, div,
    prelude::*, px, rgb,
};
use nyaterm_remote_ops::docker::{DockerContainer, DockerContainerDetails};
use nyaterm_ui::{NyaButton, NyaDialogHandle, NyaDialogWindowExt};

/// Owns only the dialog lifetime. RemoteOps remains authoritative for its data.
struct DockerDetailsView {
    app: Entity<NyaTermApp>,
    container_id: String,
    session_id: Option<String>,
    handle: Option<NyaDialogHandle>,
    _subscription: Subscription,
}

impl Render for DockerDetailsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let app = self.app.read(cx);
        let state = app.remote_ops.docker_presentation();
        let active = state.details_container_id.as_ref() == Some(&self.container_id)
            && app.session.active_id() == self.session_id.as_deref();
        if !active {
            if let Some(handle) = self.handle.clone() {
                window.defer(cx, move |window, cx| {
                    if handle.is_top(window, cx) {
                        window.close_nya_dialog(cx);
                    }
                });
            }
            return div().into_any_element();
        }
        // NyaDialog has 16px body padding on each side and a 1px border.
        let width = docker_dialog_width(f32::from(window.viewport_size().width)) - 34.;
        self.app.update(cx, |app, cx| {
            let palette = app.theme_palette();
            let container = state
                .overview
                .as_ref()
                .and_then(|overview| {
                    overview
                        .containers
                        .iter()
                        .find(|c| c.id == self.container_id)
                })
                .cloned();
            docker_details_panel(
                palette,
                width,
                &self.container_id,
                state.details.as_ref(),
                container.as_ref(),
                &docker_labels(),
                state.pending,
                cx,
            )
        })
    }
}

fn docker_dialog_width(viewport_width: f32) -> f32 {
    (viewport_width - 24.).clamp(1., 620.)
}

impl NyaTermApp {
    pub(in crate::features) fn open_docker_details(
        &mut self,
        container_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.load_docker_details(container_id.clone(), cx);
        if self
            .remote_ops
            .docker_presentation()
            .details_container_id
            .as_ref()
            != Some(&container_id)
        {
            return;
        }
        self.show_docker_details(container_id, window, cx);
    }

    fn show_docker_details(
        &mut self,
        container_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let app = cx.entity();
        let session_id = self.session.active_id().map(str::to_owned);
        let view = cx.new(|cx| DockerDetailsView {
            _subscription: cx.observe(&app, |_, _, cx| cx.notify()),
            app: app.clone(),
            container_id: container_id.clone(),
            session_id: session_id.clone(),
            handle: None,
        });
        let content = view.clone();
        let handle = window.open_tracked_nya_dialog(cx, move |dialog, window, _| {
            let app = app.clone();
            let container_id = container_id.clone();
            let session_id = session_id.clone();
            dialog
                .title(docker_labels().container_details.to_string())
                .width(docker_dialog_width(f32::from(window.viewport_size().width)))
                .overlay_closable(false)
                .content(content.clone())
                .on_close(move |_, _, cx| {
                    app.update(cx, |app, cx| {
                        if app.session.active_id() == session_id.as_deref()
                            && app
                                .remote_ops
                                .docker_presentation()
                                .details_container_id
                                .as_ref()
                                == Some(&container_id)
                        {
                            app.close_docker_details(cx);
                            app.defer_remote_panel_snapshot_flush(cx);
                        }
                    });
                })
        });
        view.update(cx, |view, _| view.handle = Some(handle));
    }
}

#[allow(clippy::too_many_arguments)]
fn docker_details_panel(
    palette: ThemePalette,
    width: f32,
    container_id: &str,
    details: Option<&DockerContainerDetails>,
    container: Option<&DockerContainer>,
    labels: &DockerLabels,
    pending: bool,
    cx: &mut Context<NyaTermApp>,
) -> gpui::AnyElement {
    let refresh_id = container_id.to_owned();
    let mut body = div().w_full().min_w_0().flex().flex_col().gap_3();
    let mut heading = div()
        .w_full()
        .min_w_0()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_2();
    if let Some(container) = container {
        heading = heading
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .whitespace_normal()
                    .text_sm()
                    .font_weight(FontWeight(600.))
                    .child(container.name.clone()),
            )
            .child(status_pill(
                labels.state_label(&container.state),
                docker_state_color(palette, &container.state),
                rgb(palette.surface),
            ));
    }
    body = body.child(
        heading.child(
            NyaButton::new("docker-details-refresh", labels.refresh.clone())
                .disabled(pending)
                .on_click(cx.listener(move |app, _, _, cx| {
                    app.load_docker_details(refresh_id.clone(), cx);
                })),
        ),
    );
    let Some(details) = details else {
        return body
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(palette.text_muted))
                    .child(if pending {
                        labels.loading.clone()
                    } else {
                        labels.error.clone()
                    }),
            )
            .into_any_element();
    };
    body = body.child(
        div()
            .w_full()
            .min_w_0()
            .grid()
            .grid_cols(if width < 360. { 1 } else { 3 })
            .gap_2()
            .child(docker_metric(
                palette,
                labels.cpu.clone(),
                details
                    .stats
                    .as_ref()
                    .map(|s| format!("{:.1}%", s.cpu_percent))
                    .unwrap_or_else(|| "-".into()),
            ))
            .child(docker_metric(
                palette,
                labels.memory.clone(),
                details
                    .stats
                    .as_ref()
                    .map(|s| format!("{:.1}%", s.memory_percent))
                    .unwrap_or_else(|| "-".into()),
            ))
            .child(docker_metric(
                palette,
                labels.pids.clone(),
                details
                    .stats
                    .as_ref()
                    .map(|s| s.pids.clone())
                    .unwrap_or_else(|| "-".into()),
            )),
    );
    if let Some(container) = container {
        let mut identity =
            docker_section(palette).child(section_title(palette, labels.identity.clone()));
        for (label, value, copyable) in [
            (labels.container_name.clone(), container.name.clone(), true),
            (labels.container_id.clone(), container.id.clone(), true),
            (labels.image.clone(), container.image.clone(), true),
            (labels.status.clone(), container.status.clone(), false),
            (
                labels.created_at.clone(),
                container.created_at.clone(),
                false,
            ),
            (labels.size.clone(), container.size.clone(), false),
        ] {
            identity = identity.child(docker_detail_line(
                palette,
                label,
                value,
                copyable,
                labels.copy.clone(),
                cx,
            ));
        }
        body = body.child(identity);
    }
    let mut lifecycle = docker_section(palette);
    for (label, value, copyable) in [
        (labels.started_at.clone(), details.started_at.clone(), false),
        (
            labels.finished_at.clone(),
            details.finished_at.clone(),
            false,
        ),
        (
            labels.restart_count.clone(),
            details.restart_count.to_string(),
            false,
        ),
        (labels.entrypoint.clone(), details.entrypoint.clone(), true),
        (labels.command.clone(), details.command.clone(), true),
    ] {
        lifecycle = lifecycle.child(docker_detail_line(
            palette,
            label,
            value,
            copyable,
            labels.copy.clone(),
            cx,
        ));
    }
    let mut network =
        docker_section(palette).child(section_title(palette, labels.networking.clone()));
    if let Some(container) = container {
        network = network.child(docker_detail_line(
            palette,
            labels.ports.clone(),
            docker_ports_value(&container.ports),
            true,
            labels.copy.clone(),
            cx,
        ));
    }
    network = network.child(docker_detail_line(
        palette,
        labels.networks.clone(),
        docker_networks_value(details),
        true,
        labels.copy.clone(),
        cx,
    ));
    body = body.child(
        div()
            .w_full()
            .min_w_0()
            .grid()
            .grid_cols(if width < 520. { 1 } else { 2 })
            .gap_3()
            .child(lifecycle)
            .child(network),
    );
    body.child(
        docker_section(palette)
            .child(section_title(palette, labels.io.clone()))
            .child(docker_detail_line(
                palette,
                labels.net_io.clone(),
                details
                    .stats
                    .as_ref()
                    .map(|s| s.net_io.clone())
                    .unwrap_or_default(),
                false,
                labels.copy.clone(),
                cx,
            ))
            .child(docker_detail_line(
                palette,
                labels.block_io.clone(),
                details
                    .stats
                    .as_ref()
                    .map(|s| s.block_io.clone())
                    .unwrap_or_default(),
                false,
                labels.copy.clone(),
                cx,
            )),
    )
    .child(docker_section(palette).child(docker_detail_line(
        palette,
        labels.mounts.clone(),
        docker_mounts_value(details),
        true,
        labels.copy.clone(),
        cx,
    )))
    .into_any_element()
}

fn docker_section(palette: ThemePalette) -> gpui::Div {
    div()
        .w_full()
        .min_w_0()
        .flex_none()
        .flex()
        .flex_col()
        .gap_2()
        .p_3()
        .rounded_md()
        .border_1()
        .border_color(rgb(palette.border))
}

fn section_title(palette: ThemePalette, label: impl Into<SharedString>) -> gpui::Div {
    div()
        .min_w_0()
        .whitespace_normal()
        .text_sm()
        .font_weight(FontWeight(600.))
        .text_color(rgb(palette.text))
        .child(label.into())
}

fn docker_metric(
    palette: ThemePalette,
    label: impl Into<SharedString>,
    value: String,
) -> gpui::Div {
    div()
        .min_w_0()
        .px_3()
        .py_2()
        .rounded_md()
        .bg(rgb(palette.surface))
        .child(
            div()
                .whitespace_normal()
                .text_xs()
                .text_color(rgb(palette.text_muted))
                .child(label.into()),
        )
        .child(
            div()
                .min_w_0()
                .whitespace_normal()
                .font_family(gpui_code_font_family())
                .text_sm()
                .text_color(rgb(palette.text))
                .child(value),
        )
}

fn docker_detail_line(
    palette: ThemePalette,
    label: impl Into<SharedString>,
    value: String,
    copyable: bool,
    copy_label: impl Into<SharedString>,
    cx: &mut Context<NyaTermApp>,
) -> gpui::Div {
    let label: SharedString = label.into();
    let value_selector = format!("docker-detail-value-{label}");
    let copy_value = value.clone();
    let copyable = copyable && !value.trim().is_empty() && value.trim() != "-";
    div()
        .w_full()
        .min_w_0()
        .flex_none()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .min_w_0()
                .flex()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    div()
                        .min_w_0()
                        .whitespace_normal()
                        .text_xs()
                        .text_color(rgb(palette.text_muted))
                        .child(label.clone()),
                )
                .when(copyable, |this| {
                    this.child(small_button(
                        palette,
                        format!("docker-copy-{label}"),
                        copy_label,
                        cx.listener(move |app, _, _, cx| {
                            app.copy_docker_text(copy_value.clone(), &label, cx)
                        }),
                    ))
                }),
        )
        .child(
            div()
                .debug_selector(move || value_selector.clone())
                .w_full()
                .min_w_0()
                .whitespace_normal()
                .font_family(gpui_code_font_family())
                .text_size(px(12.))
                .line_height(px(18.))
                .text_color(rgb(palette.text))
                .child(if value.trim().is_empty() {
                    "-".to_string()
                } else {
                    value
                }),
        )
}

fn docker_ports_value(ports: &str) -> String {
    let value = ports
        .split(',')
        .map(str::trim)
        .filter(|port| !port.is_empty())
        .map(|port| port.replace("->", " -> "))
        .collect::<Vec<_>>()
        .join("\n");
    if value.trim().is_empty() {
        "-".to_string()
    } else {
        value
    }
}

fn docker_networks_value(details: &DockerContainerDetails) -> String {
    let value = details
        .networks
        .iter()
        .map(|network| {
            if network.ip_address.trim().is_empty() {
                network.name.clone()
            } else {
                format!("{}: {}", network.name, network.ip_address)
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    if value.trim().is_empty() {
        "-".to_string()
    } else {
        value
    }
}

fn docker_mounts_value(details: &DockerContainerDetails) -> String {
    let value = details
        .mounts
        .iter()
        .map(|mount| {
            let access = if mount.rw { "rw" } else { "ro" };
            let mode = if mount.mode.trim().is_empty() {
                access.to_string()
            } else {
                format!("{access},{}", mount.mode)
            };
            format!(
                "{} {} -> {} ({mode})",
                if mount.kind.trim().is_empty() {
                    "mount"
                } else {
                    &mount.kind
                },
                if mount.source.trim().is_empty() {
                    "-"
                } else {
                    &mount.source
                },
                if mount.destination.trim().is_empty() {
                    "-"
                } else {
                    &mount.destination
                }
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    if value.trim().is_empty() {
        "-".to_string()
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::docker_detail_line;
    use crate::entities::{OverlayStore, StartupRestoreStore, UiStoreHandles};
    use crate::features::NyaTermApp;
    use crate::test_support::{TestAppCleanup, TestConfigDir};
    use gpui::{
        AppContext as _, Context, Entity, IntoElement, Render, TestAppContext, VisualTestContext,
        Window, div, prelude::*, px, size,
    };
    use nyaterm_core::{AppRuntime, RuntimeMode};
    use nyaterm_remote_ops::docker::DockerContainerDetails;
    use nyaterm_ui::NyaDialogWindowExt;

    struct Host {
        app: Entity<NyaTermApp>,
    }
    impl Render for Host {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.app.update(cx, |app, cx| {
                div().size_full().p_3().child(docker_detail_line(
                    app.theme_palette(),
                    "fixture",
                    "registry.example/long-path-without-spaces".repeat(30),
                    true,
                    "Copy",
                    cx,
                ))
            })
        }
    }

    fn app(cx: &mut TestAppContext, dir: &TestConfigDir) -> Entity<NyaTermApp> {
        let root = dir.path();
        let runtime = AppRuntime::from_parts_for_test(
            RuntimeMode::Portable,
            root.to_path_buf(),
            root.join("config"),
            root.join("logs"),
            root.join("cache"),
            None,
        );
        let stores = UiStoreHandles {
            startup_restore: cx.new(|_| StartupRestoreStore::default()),
            overlays: cx.new(|_| OverlayStore::default()),
        };
        let app = cx.new(|cx| NyaTermApp::new(runtime, stores, cx));
        app.update(cx, |app, cx| app.sync_component_theme(cx));
        app
    }

    fn draw(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| {
            _ = window.draw(cx);
        });
        cx.run_until_parked();
    }

    #[test]
    fn long_detail_values_wrap_inside_the_available_width_at_all_scales() {
        let dir = TestConfigDir::new("nyaterm-docker-wrap");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app(&mut cx, &dir);
        let (_, vcx) = cx.add_window_view(move |window, cx| {
            let host = cx.new(|_| Host { app });
            nyaterm_ui::nya_root(host, window, cx)
        });
        for scale in [1., 1.5, 2.] {
            vcx.simulate_scale_factor_change(scale);
            for width in [200., 280., 320., 430., 540., 600., 720.] {
                vcx.simulate_resize(size(px(width), px(900.)));
                draw(vcx);
                let bounds = vcx
                    .debug_bounds("docker-detail-value-fixture")
                    .expect("value painted");
                assert!(
                    bounds.right() <= px(width),
                    "value overflows at {width}/{scale}: {bounds:?}"
                );
                assert!(bounds.size.height > px(18.), "long unbroken text must wrap");
            }
        }
    }

    #[test]
    fn docker_dialog_escape_restores_focus_and_invalidated_details_close() {
        let dir = TestConfigDir::new("nyaterm-docker-dialog");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app(&mut cx, &dir);
        let host_app = app.clone();
        let (_, vcx) = cx.add_window_view(move |window, cx| {
            let host = cx.new(|_| Host { app: host_app });
            nyaterm_ui::nya_root(host, window, cx)
        });
        draw(vcx);
        let focus = vcx.update(|window, cx| {
            let focus = cx.focus_handle();
            window.focus(&focus, cx);
            focus
        });
        for reason in ["escape", "session", "removed"] {
            vcx.update(|window, cx| {
                app.update(cx, |app, cx| {
                    app.remote_ops
                        .start_docker_details("fixture".into(), "loading".into());
                    app.remote_ops
                        .apply_docker_details("fixture".into(), DockerContainerDetails::default());
                    app.show_docker_details("fixture".into(), window, cx);
                })
            });
            draw(vcx);
            assert!(vcx.update(|window, cx| window.has_active_nya_dialog(cx)));
            if reason == "escape" {
                vcx.simulate_keystrokes("escape");
            } else {
                vcx.update(|_, cx| {
                    app.update(cx, |app, cx| {
                        if reason == "session" {
                            app.remote_ops.reset_for_session_switch();
                        } else {
                            app.remote_ops.apply_docker_overview(Default::default());
                        }
                        cx.notify();
                    })
                });
            }
            draw(vcx);
            assert!(
                !vcx.update(|window, cx| window.has_active_nya_dialog(cx)),
                "{reason}"
            );
            assert!(
                vcx.update(|window, _| focus.is_focused(window)),
                "focus after {reason}"
            );
            assert!(vcx.update(|_, cx| {
                app.read(cx)
                    .remote_ops
                    .docker_presentation()
                    .details_container_id
                    .is_none()
            }));
        }
    }
}
