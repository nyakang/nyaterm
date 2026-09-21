use futures::StreamExt as _;
use rust_i18n::t;

use gpui::{Context, Window};
use nyaterm_transport::DockerService;

use crate::blocking_jobs::BlockingJobScheduler;
use crate::features::NyaTermApp;
use crate::features::formatting::{compact_id, docker_compose_project_key};
use crate::features::runtime_jobs::{DockerJobOutput, DockerJobResult, DockerResource};
use crate::models::{DockerConfirmAction, DockerConfirmState, DockerTab, NavItem};

use super::helpers::{
    ActiveSshRuntimeContext, DOCKER_SHELL_SELECTOR, docker_compose_terminal_base,
    docker_overview_status, shell_quote,
};
use crate::features::remote::job_state::RemoteJobTicket;

fn submit_docker_job(
    scheduler: &BlockingJobScheduler,
    name: &'static str,
    ticket: RemoteJobTicket<DockerJobResult>,
    session_id: String,
    run: impl FnOnce() -> Result<DockerJobOutput, String> + Send + 'static,
) {
    let job_id = ticket.job_id;
    let tx = ticket.tx;
    let rejected_tx = tx.clone();
    let rejected_session_id = session_id.clone();
    if let Err(error) = scheduler.submit_detached(name, move |_| {
        let _ = tx.unbounded_send(DockerJobResult {
            job_id,
            session_id,
            result: run(),
        });
    }) {
        let _ = rejected_tx.unbounded_send(DockerJobResult {
            job_id,
            session_id: rejected_session_id,
            result: Err(error.to_string()),
        });
    }
}

fn docker_error_kind(error: &str) -> &'static str {
    let error = error.to_ascii_lowercase();
    if error.contains("timed out") {
        "timeout"
    } else if error.contains("not installed") || error.contains("executable is unavailable") {
        "executable_unavailable"
    } else if error.contains("authorization") || error.contains("authentication") {
        "authorization"
    } else if error.contains("cancel") {
        "cancelled"
    } else if error.contains("session") || error.contains("multiplex") {
        "session"
    } else {
        "remote_operation"
    }
}

fn fetch_docker_resource(
    service: &DockerService,
    tab: DockerTab,
) -> Option<anyhow::Result<DockerResource>> {
    match tab {
        DockerTab::Containers => None,
        DockerTab::Images => Some(service.images().map(DockerResource::Images)),
        DockerTab::Volumes => Some(service.volumes().map(DockerResource::Volumes)),
        DockerTab::Networks => Some(service.networks().map(DockerResource::Networks)),
        DockerTab::Compose => Some(service.compose_projects().map(DockerResource::Compose)),
    }
}

impl NyaTermApp {
    fn active_docker_runtime_context(
        &mut self,
        action: &str,
        cx: &mut Context<Self>,
    ) -> Option<ActiveSshRuntimeContext> {
        match self.active_ssh_runtime_context(action) {
            Ok(context) => Some(context),
            Err(message) => {
                self.remote_ops.set_docker_status(message);
                self.shell
                    .set_status(self.remote_ops.docker_status().to_string());
                cx.notify();
                None
            }
        }
    }

    pub(in crate::features) fn refresh_docker(&mut self, cx: &mut Context<Self>) {
        let Some(context) = self.active_docker_runtime_context("inspecting Docker", cx) else {
            return;
        };
        let config = context.config;
        let multiplex = context.multiplex;
        let job_session_id = context.session_id;
        if self.remote_ops.docker_is_pending_for(&job_session_id) {
            self.remote_ops
                .set_docker_status("Docker operation already running");
            cx.notify();
            return;
        }

        let tab = self.remote_ops.docker_effective_tab();
        let ticket = self.remote_ops.begin_docker_job(job_session_id.clone());
        self.remote_ops.mark_docker_refresh_started();
        if tab != DockerTab::Containers {
            self.remote_ops.mark_docker_resource_started(tab);
        }
        self.remote_ops.set_docker_status("loading Docker overview");
        submit_docker_job(
            &self.blocking_jobs,
            "docker-overview",
            ticket,
            job_session_id,
            move || {
                (|| {
                    let service = DockerService::with_multiplex(config, multiplex)?;
                    let overview = service.overview()?;
                    let resource = overview
                        .available
                        .then(|| fetch_docker_resource(&service, tab))
                        .flatten()
                        .map(|result| result.map_err(|error| error.to_string()));
                    Ok(DockerJobOutput::Overview { overview, resource })
                })()
                .map_err(|error: anyhow::Error| error.to_string())
            },
        );
        cx.notify();
    }

    pub(in crate::features) fn load_docker_resource_if_needed(&mut self, cx: &mut Context<Self>) {
        let interval = self.settings.summary().ui_docker_manager_interval.max(3);
        if !self.remote_ops.docker_is_pending()
            && self.remote_ops.docker_resource_load_due(interval)
        {
            self.refresh_docker_resource(cx);
        }
    }

    pub(in crate::features) fn refresh_docker_resource(&mut self, cx: &mut Context<Self>) {
        let tab = self.remote_ops.docker_effective_tab();
        if tab == DockerTab::Containers || !self.remote_ops.docker_can_prune() {
            return;
        }
        let Some(context) = self.active_docker_runtime_context("reading Docker resources", cx)
        else {
            return;
        };
        let job_session_id = context.session_id;
        if self.remote_ops.docker_is_pending_for(&job_session_id) {
            return;
        }
        let ticket = self.remote_ops.begin_docker_job(job_session_id.clone());
        self.remote_ops.mark_docker_resource_started(tab);
        self.remote_ops
            .set_docker_status(format!("loading Docker {}", tab.label()));
        submit_docker_job(
            &self.blocking_jobs,
            "docker-resource",
            ticket,
            job_session_id,
            move || {
                (|| {
                    let service = DockerService::with_multiplex(context.config, context.multiplex)?;
                    fetch_docker_resource(&service, tab)
                        .expect("resource tab has a Docker command")
                        .map(DockerJobOutput::Resource)
                })()
                .map_err(|error: anyhow::Error| error.to_string())
            },
        );
        cx.notify();
    }

    pub(in crate::features) fn docker_container_action(
        &mut self,
        container_id: String,
        action: &'static str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(context) = self.active_docker_runtime_context("changing containers", cx) else {
            return;
        };
        let config = context.config;
        let multiplex = context.multiplex;
        let job_session_id = context.session_id;
        if self.remote_ops.docker_is_pending_for(&job_session_id) {
            self.remote_ops
                .set_docker_status("Docker operation already running");
            cx.notify();
            return;
        }

        let ticket = self.remote_ops.begin_docker_job(job_session_id.clone());
        self.remote_ops.start_docker_container_action(format!(
            "Docker {action} {}",
            compact_id(&container_id)
        ));
        submit_docker_job(
            &self.blocking_jobs,
            "docker-container-action",
            ticket,
            job_session_id,
            move || {
                (|| {
                    let service = DockerService::with_multiplex(config, multiplex)?;
                    service.container_action(&container_id, action)?;
                    let overview = service.overview()?;
                    Ok(DockerJobOutput::RefreshedAfterAction {
                        label: format!("Docker {action} {}", compact_id(&container_id)),
                        overview,
                    })
                })()
                .map_err(|error: anyhow::Error| error.to_string())
            },
        );
        cx.notify();
    }

    pub(in crate::features) fn load_docker_details(
        &mut self,
        container_id: String,
        cx: &mut Context<Self>,
    ) {
        let Some(context) = self.active_docker_runtime_context("reading Docker details", cx) else {
            return;
        };
        let config = context.config;
        let multiplex = context.multiplex;
        let job_session_id = context.session_id;
        if self.remote_ops.docker_is_pending_for(&job_session_id) {
            self.remote_ops
                .set_docker_status("Docker operation already running");
            cx.notify();
            return;
        }

        let ticket = self.remote_ops.begin_docker_job(job_session_id.clone());
        self.remote_ops.start_docker_details(
            container_id.clone(),
            format!("loading details for {}", compact_id(&container_id)),
        );
        submit_docker_job(
            &self.blocking_jobs,
            "docker-details",
            ticket,
            job_session_id,
            move || {
                (|| {
                    DockerService::with_multiplex(config, multiplex)?
                        .container_details(&container_id)
                        .map(|details| DockerJobOutput::Details {
                            container_id,
                            details,
                        })
                })()
                .map_err(|error: anyhow::Error| error.to_string())
            },
        );
        cx.notify();
    }

    pub(in crate::features) fn close_docker_details(&mut self, cx: &mut Context<Self>) {
        self.remote_ops.close_docker_details();
        self.shell
            .set_status(self.remote_ops.docker_status().to_string());
        cx.notify();
    }

    pub(in crate::features) fn send_docker_container_logs_to_terminal(
        &mut self,
        container_id: String,
        cx: &mut Context<Self>,
    ) {
        self.send_docker_terminal_command(
            format!("docker logs -f --tail 100 {}", shell_quote(&container_id)),
            format!("following logs for {}", compact_id(&container_id)),
            cx,
        );
    }

    pub(in crate::features) fn enter_docker_container_terminal(
        &mut self,
        container_id: String,
        cx: &mut Context<Self>,
    ) {
        self.send_docker_terminal_command(
            format!(
                "docker exec -it {} sh -lc {}",
                shell_quote(&container_id),
                shell_quote(DOCKER_SHELL_SELECTOR)
            ),
            format!("entering container {}", compact_id(&container_id)),
            cx,
        );
    }

    pub(in crate::features) fn send_docker_compose_service_logs_to_terminal(
        &mut self,
        project_name: String,
        config_files: Option<String>,
        service_name: String,
        cx: &mut Context<Self>,
    ) {
        self.send_docker_terminal_command(
            format!(
                "{} logs -f --tail 100 {}",
                docker_compose_terminal_base(&project_name, config_files.as_deref()),
                shell_quote(&service_name)
            ),
            format!("following compose logs for {service_name}"),
            cx,
        );
    }

    pub(in crate::features) fn send_docker_terminal_command(
        &mut self,
        mut command: String,
        status: String,
        cx: &mut Context<Self>,
    ) {
        if self.session.active_id().is_none() {
            self.remote_ops
                .set_docker_status("start a terminal session before sending Docker commands");
            self.shell
                .set_status(self.remote_ops.docker_status().to_string());
            cx.notify();
            return;
        }
        if !command.ends_with('\n') {
            command.push('\n');
        }
        self.shell.select_nav(NavItem::Workspace);
        if self.send_terminal_input(command.into_bytes(), cx) {
            self.remote_ops.set_docker_status(status);
            self.shell
                .set_status(self.remote_ops.docker_status().to_string());
            cx.notify();
        } else {
            self.remote_ops
                .set_docker_status(self.shell.status().to_string());
        }
    }

    pub(in crate::features) fn toggle_docker_compose_project(
        &mut self,
        project_name: String,
        config_files: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = docker_compose_project_key(&project_name, config_files.as_deref());
        if self.remote_ops.toggle_compose_project(key, &project_name) {
            self.load_docker_compose_services(project_name, config_files, window, cx);
        } else {
            cx.notify();
        }
    }

    pub(in crate::features) fn load_docker_compose_services(
        &mut self,
        project_name: String,
        config_files: Option<String>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(context) = self.active_docker_runtime_context("reading compose services", cx)
        else {
            return;
        };
        let config = context.config;
        let multiplex = context.multiplex;
        let job_session_id = context.session_id;
        if self.remote_ops.docker_is_pending_for(&job_session_id) {
            self.remote_ops
                .set_docker_status("Docker operation already running");
            cx.notify();
            return;
        }

        let key = docker_compose_project_key(&project_name, config_files.as_deref());
        let ticket = self.remote_ops.begin_docker_job(job_session_id.clone());
        self.remote_ops
            .set_docker_status(format!("loading compose services for {project_name}"));
        self.remote_ops.clear_compose_service_error(&key);
        submit_docker_job(
            &self.blocking_jobs,
            "docker-compose-services",
            ticket,
            job_session_id,
            move || {
                (|| {
                    DockerService::with_multiplex(config, multiplex)?
                        .compose_services(&project_name, config_files.as_deref())
                        .map(|services| DockerJobOutput::ComposeServices {
                            key,
                            project_name,
                            services,
                        })
                })()
                .map_err(|error: anyhow::Error| error.to_string())
            },
        );
        cx.notify();
    }

    pub(in crate::features) fn docker_compose_service_action(
        &mut self,
        project_name: String,
        config_files: Option<String>,
        service_name: String,
        action: &'static str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(context) = self.active_docker_runtime_context("changing compose services", cx)
        else {
            return;
        };
        let config = context.config;
        let multiplex = context.multiplex;
        let job_session_id = context.session_id;
        if self.remote_ops.docker_is_pending_for(&job_session_id) {
            self.remote_ops
                .set_docker_status("Docker operation already running");
            cx.notify();
            return;
        }

        let key = docker_compose_project_key(&project_name, config_files.as_deref());
        let ticket = self.remote_ops.begin_docker_job(job_session_id.clone());
        self.remote_ops
            .set_docker_status(format!("compose {action} {service_name}"));
        submit_docker_job(
            &self.blocking_jobs,
            "docker-compose-service-action",
            ticket,
            job_session_id,
            move || {
                (|| {
                    let service = DockerService::with_multiplex(config, multiplex)?;
                    service.compose_service_action(
                        &project_name,
                        config_files.as_deref(),
                        &service_name,
                        action,
                    )?;
                    let overview = service.overview()?;
                    let services =
                        service.compose_services(&project_name, config_files.as_deref())?;
                    Ok(DockerJobOutput::ComposeServiceAction {
                        key,
                        service_name,
                        action: action.to_string(),
                        overview,
                        services,
                    })
                })()
                .map_err(|error: anyhow::Error| error.to_string())
            },
        );
        cx.notify();
    }

    pub(in crate::features) fn docker_compose_action(
        &mut self,
        project_name: String,
        config_files: Option<String>,
        action: &'static str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(context) = self.active_docker_runtime_context("changing compose projects", cx)
        else {
            return;
        };
        let config = context.config;
        let multiplex = context.multiplex;
        let job_session_id = context.session_id;
        if self.remote_ops.docker_is_pending_for(&job_session_id) {
            self.remote_ops
                .set_docker_status("Docker operation already running");
            cx.notify();
            return;
        }

        let key = docker_compose_project_key(&project_name, config_files.as_deref());
        let ticket = self.remote_ops.begin_docker_job(job_session_id.clone());
        self.remote_ops
            .set_docker_status(format!("compose {action} {project_name}"));
        self.remote_ops.clear_compose_service_error(&key);
        submit_docker_job(
            &self.blocking_jobs,
            "docker-compose-action",
            ticket,
            job_session_id,
            move || {
                (|| {
                    let service = DockerService::with_multiplex(config, multiplex)?;
                    service.compose_action(&project_name, config_files.as_deref(), action)?;
                    let overview = service.overview()?;
                    let service_result =
                        service.compose_services(&project_name, config_files.as_deref());
                    let (services, service_error) = match service_result {
                        Ok(services) => (Some(services), None),
                        Err(error) => (None, Some(error.to_string())),
                    };
                    Ok(DockerJobOutput::ComposeProjectAction {
                        key,
                        project_name,
                        action: action.to_string(),
                        overview,
                        services,
                        service_error,
                    })
                })()
                .map_err(|error: anyhow::Error| error.to_string())
            },
        );
        cx.notify();
    }

    pub(in crate::features) fn request_docker_confirm(
        &mut self,
        confirm: DockerConfirmState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let title = confirm.title.clone();
        let detail = confirm.detail.clone();
        self.open_confirm_dialog(
            (
                title,
                detail,
                t!("common.confirm").to_string(),
                true,
                move |app, window, cx| {
                    app.run_confirmed_docker_action(confirm.clone(), window, cx);
                    true
                },
            ),
            window,
            cx,
        );
    }

    fn run_confirmed_docker_action(
        &mut self,
        confirm: DockerConfirmState,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(context) = self.active_docker_runtime_context("changing Docker resources", cx)
        else {
            return;
        };
        let config = context.config;
        let multiplex = context.multiplex;
        let job_session_id = context.session_id;
        if self.remote_ops.docker_is_pending_for(&job_session_id) {
            self.remote_ops
                .set_docker_status("Docker operation already running");
            cx.notify();
            return;
        }

        let ticket = self.remote_ops.begin_docker_job(job_session_id.clone());
        self.remote_ops
            .set_docker_status(format!("running {}", confirm.title));
        submit_docker_job(
            &self.blocking_jobs,
            "docker-confirmed-action",
            ticket,
            job_session_id,
            move || {
                (|| {
                    let label = confirm.title.clone();
                    let service = DockerService::with_multiplex(config, multiplex)?;
                    match confirm.action {
                        DockerConfirmAction::ContainerAction {
                            container_id,
                            action,
                        } => {
                            service.container_action(&container_id, action)?;
                        }
                        DockerConfirmAction::ImageRemove { image_id, force } => {
                            service.image_remove(&image_id, force)?;
                        }
                        DockerConfirmAction::VolumeRemove { volume_name, force } => {
                            service.volume_remove(&volume_name, force)?;
                        }
                        DockerConfirmAction::NetworkRemove { network_id } => {
                            service.network_remove(&network_id)?;
                        }
                        DockerConfirmAction::ComposeAction {
                            project_name,
                            config_files,
                            action,
                        } => {
                            service.compose_action(
                                &project_name,
                                config_files.as_deref(),
                                action,
                            )?;
                            let key =
                                docker_compose_project_key(&project_name, config_files.as_deref());
                            let overview = service.overview()?;
                            let service_result =
                                service.compose_services(&project_name, config_files.as_deref());
                            let (services, service_error) = match service_result {
                                Ok(services) => (Some(services), None),
                                Err(error) => (None, Some(error.to_string())),
                            };
                            return Ok(DockerJobOutput::ComposeProjectAction {
                                key,
                                project_name,
                                action: action.to_string(),
                                overview,
                                services,
                                service_error,
                            });
                        }
                        DockerConfirmAction::Prune { volumes } => {
                            service.system_prune(volumes)?;
                        }
                    }
                    let overview = service.overview()?;
                    Ok(DockerJobOutput::RefreshedAfterAction { label, overview })
                })()
                .map_err(|error: anyhow::Error| error.to_string())
            },
        );
        cx.notify();
    }

    pub(in crate::features) fn prune_docker_system(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.request_docker_confirm(
            DockerConfirmState {
                title: "Docker system prune".to_string(),
                detail: "docker system prune -f --volumes".to_string(),
                action: DockerConfirmAction::Prune { volumes: true },
            },
            window,
            cx,
        );
    }

    /// Deliver Docker job replies as they arrive.
    ///
    /// Started once at window open. Before this the runtime tick polled
    /// `next_docker_event`, which meant a reply waited for the next tick and
    /// forced `runtime_quiet_tick_allowed` to carry a `remote_ops` term to keep
    /// that wait short.
    pub(in crate::features) fn start_docker_event_drain(&mut self, cx: &mut Context<Self>) {
        let Some(mut rx) = self.remote_ops.take_docker_event_receiver() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            while let Some(event) = rx.next().await {
                if this
                    .update(cx, |this, cx| {
                        let active_session =
                            this.session.active_id() == Some(event.session_id.as_str());
                        let refresh_resource = matches!(
                            &event.result,
                            Ok(DockerJobOutput::RefreshedAfterAction { .. }
                                | DockerJobOutput::ComposeServiceAction { .. }
                                | DockerJobOutput::ComposeProjectAction { .. })
                        );
                        if this.apply_docker_event(event) {
                            if active_session {
                                if refresh_resource {
                                    this.refresh_docker_resource(cx);
                                } else {
                                    this.load_docker_resource_if_needed(cx);
                                }
                            }
                            cx.notify();
                        }
                        // Flush boundary: a reply changed the pane, so its panel
                        // gets the snapshot before the next paint.
                        this.flush_remote_panel_snapshots(cx);
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    /// Apply one reply, reporting whether the UI needs a repaint.
    fn apply_docker_event(&mut self, event: DockerJobResult) -> bool {
        if !self
            .remote_ops
            .complete_docker_event(event.job_id, &event.session_id)
        {
            // A superseded job's reply; the pane has already moved on.
            return false;
        }
        if self.session.active_id() != Some(event.session_id.as_str()) {
            // Another session is active now, but completing the job is
            // itself a state change worth painting.
            return true;
        }
        let was_overview_refresh = self.remote_ops.docker_status() == "loading Docker overview";
        match event.result {
            Ok(DockerJobOutput::Overview { overview, resource }) => {
                self.remote_ops.reset_docker_refresh_failures();
                self.remote_ops
                    .set_docker_status(docker_overview_status(&overview));
                self.remote_ops.apply_docker_summary(overview);
                if let Some(resource) = resource {
                    match resource {
                        Ok(resource) => self.remote_ops.apply_docker_resource(resource),
                        Err(error) => self
                            .remote_ops
                            .set_docker_status(format!("Docker resource refresh failed: {error}")),
                    }
                }
                self.shell
                    .set_status(self.remote_ops.docker_status().to_string());
            }
            Ok(DockerJobOutput::Resource(resource)) => {
                self.remote_ops.apply_docker_resource(resource);
                self.remote_ops.set_docker_status("Docker resources loaded");
                self.shell
                    .set_status(self.remote_ops.docker_status().to_string());
            }
            Ok(DockerJobOutput::Details {
                container_id,
                details,
            }) => {
                self.remote_ops
                    .set_docker_status(format!("loaded details for {}", compact_id(&container_id)));
                self.shell
                    .set_status(self.remote_ops.docker_status().to_string());
                self.remote_ops.apply_docker_details(container_id, details);
            }
            Ok(DockerJobOutput::ComposeServices {
                key,
                project_name,
                services,
            }) => {
                self.remote_ops.set_docker_status(format!(
                    "loaded {} service(s) for {project_name}",
                    services.len()
                ));
                self.shell
                    .set_status(self.remote_ops.docker_status().to_string());
                self.remote_ops.set_compose_services(key, services);
            }
            Ok(DockerJobOutput::ComposeServiceAction {
                key,
                service_name,
                action,
                overview,
                services,
            }) => {
                self.remote_ops
                    .set_docker_status(format!("compose {action} {service_name}"));
                self.shell
                    .set_status(self.remote_ops.docker_status().to_string());
                self.remote_ops.apply_docker_summary(overview);
                self.remote_ops.set_compose_services(key, services);
            }
            Ok(DockerJobOutput::ComposeProjectAction {
                key,
                project_name,
                action,
                overview,
                services,
                service_error,
            }) => {
                self.remote_ops
                    .set_docker_status(format!("compose {action} {project_name}"));
                self.shell
                    .set_status(self.remote_ops.docker_status().to_string());
                self.remote_ops.apply_docker_summary(overview);
                if let Some(services) = services {
                    self.remote_ops.set_compose_services(key.clone(), services);
                } else if let Some(error) = service_error {
                    self.remote_ops
                        .set_compose_service_error(key.clone(), error);
                }
            }
            Ok(DockerJobOutput::RefreshedAfterAction { label, overview }) => {
                let container_count = overview.containers.len();
                self.remote_ops.apply_docker_summary(overview);
                self.remote_ops.set_docker_status(format!(
                    "{label} completed · {container_count} container(s)"
                ));
                self.shell
                    .set_status(self.remote_ops.docker_status().to_string());
            }
            Err(error) => {
                tracing::warn!(
                    job_id = event.job_id,
                    session_id = %event.session_id,
                    overview_refresh = was_overview_refresh,
                    error_kind = docker_error_kind(&error),
                    "Docker background operation failed"
                );
                if was_overview_refresh && self.remote_ops.record_docker_refresh_failure() >= 3 {
                    self.remote_ops.clear_docker_overview();
                }
                self.remote_ops
                    .set_docker_status(format!("Docker operation failed: {error}"));
                self.shell
                    .set_status(self.remote_ops.docker_status().to_string());
            }
        }
        true
    }
}
