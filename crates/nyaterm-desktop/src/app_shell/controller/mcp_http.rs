//! One authoritative HTTP service per process, bound to an explicit workspace.
use super::DesktopController;
use crate::features::NyaTermApp;
use futures::StreamExt;
use gpui::{Context, WeakEntity};
use nyaterm_core::{ExternalMcpSettings, SecretString, WorkspaceId, ai::mcp_http::McpTransport};
use nyaterm_mcp_host::{
    http_sidecar::{HttpSidecar, SidecarEvent, SidecarFailure},
    server::McpHostRuntime,
};
use nyaterm_mcp_protocol::http_control::{CONTROL_VERSION, HttpBootstrap};

#[derive(Default)]
pub(super) struct HttpMcpState {
    owner: Option<(WorkspaceId, WeakEntity<NyaTermApp>)>,
    ever_claimed: bool,
    settings: Option<ExternalMcpSettings>,
    generation: u64,
    bridge_generation: Option<String>,
    pub(super) child: Option<HttpSidecar>,
    pub(super) previous: Option<futures::channel::oneshot::Receiver<()>>,
    phase: HttpPhase,
    pub(super) token: Option<SecretString>,
}
#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum HttpPhase {
    #[default]
    Disabled,
    Starting,
    Running,
    Failed,
    PortInUse,
    NeedsOwner,
    Locked,
}
#[derive(Clone)]
pub(crate) struct HttpMcpView {
    pub status_key: &'static str,
    pub owner: Option<WorkspaceId>,
    pub running: bool,
    pub port: Option<u16>,
    pub owned_by_current_window: bool,
}
impl Default for HttpMcpView {
    fn default() -> Self {
        Self {
            status_key: "ai.mcp.disabled",
            owner: None,
            running: false,
            port: None,
            owned_by_current_window: false,
        }
    }
}
impl DesktopController {
    pub(crate) fn http_mcp_view(&self) -> HttpMcpView {
        HttpMcpView {
            status_key: match self.http_mcp.phase {
                HttpPhase::Disabled => "ai.mcp.disabled",
                HttpPhase::Starting => "ai.mcp.httpStarting",
                HttpPhase::Running => "ai.mcp.running",
                HttpPhase::Failed => "ai.mcp.httpFailed",
                HttpPhase::PortInUse => "ai.mcp.httpPortInUse",
                HttpPhase::NeedsOwner => "ai.mcp.httpNeedsOwner",
                HttpPhase::Locked => "ai.mcp.httpLocked",
            },
            owner: self.http_mcp.owner.as_ref().map(|(id, _)| *id),
            port: self
                .http_mcp
                .settings
                .as_ref()
                .map(|settings| settings.http_port),
            running: !self.screen_locked
                && !self.process_quitting
                && self.http_mcp.phase == HttpPhase::Running,
            owned_by_current_window: false,
        }
    }
    pub(crate) fn http_mcp_config(&self, client: &str) -> Result<String, String> {
        if self.screen_locked || self.process_quitting || self.http_mcp.phase != HttpPhase::Running
        {
            return Err("HTTP MCP is not running".into());
        }
        let token = self
            .http_mcp
            .token
            .as_ref()
            .ok_or("HTTP MCP is not running")?;
        let port = self
            .http_mcp
            .settings
            .as_ref()
            .ok_or("HTTP MCP is not configured")?
            .http_port;
        nyaterm_core::ai::mcp_http::client_config(client, port, token.expose_secret())
            .map_err(str::to_owned)
    }
    pub(super) fn http_mcp_workspace_ready(&mut self, id: WorkspaceId, cx: &mut Context<Self>) {
        if self.http_mcp.ever_claimed {
            return;
        }
        let app = self.windows.get(&id).and_then(|entry| {
            entry
                .shell
                .update(cx, |shell, _| shell.app.clone())
                .ok()
                .flatten()
        });
        if let Some(app) = app {
            let settings = app.read(cx).http_mcp_settings();
            self.configure_http_mcp(id, app.downgrade(), settings, false, cx);
        }
    }
    pub(crate) fn configure_http_mcp(
        &mut self,
        id: WorkspaceId,
        app: WeakEntity<NyaTermApp>,
        settings: ExternalMcpSettings,
        claim: bool,
        cx: &mut Context<Self>,
    ) {
        if self.process_quitting {
            return;
        }
        let enabled = settings.enabled && settings.transport == McpTransport::StreamableHttp;
        let switching = claim
            && self
                .http_mcp
                .owner
                .as_ref()
                .is_none_or(|(owner, _)| *owner != id);
        let changed = self
            .http_mcp
            .settings
            .as_ref()
            .is_none_or(|previous| !previous.runtime_eq(&settings));
        if !changed && !claim {
            return;
        }
        self.stop_http_mcp(cx);
        self.http_mcp.settings = Some(settings);
        if enabled && (switching || !self.http_mcp.ever_claimed) {
            self.http_mcp.owner = Some((id, app));
            self.http_mcp.ever_claimed = true;
        }
        if !enabled {
            self.http_mcp.phase = HttpPhase::Disabled;
            self.notify_http_mcp(cx);
            return;
        }
        self.start_http_mcp(cx);
    }
    pub(super) fn stop_http_mcp(&mut self, cx: &mut Context<Self>) {
        self.http_mcp.generation += 1;
        self.http_mcp.token = None;
        if let Some(child) = self.http_mcp.child.take() {
            self.http_mcp.previous = child.shutdown();
        }
        if let Some(generation) = self.http_mcp.bridge_generation.take()
            && let Some((_, app)) = &self.http_mcp.owner
        {
            let app = app.clone();
            // May be called while the owner is inside its own update callback.
            cx.defer(move |cx| {
                let _ = app.update(cx, |app, cx| {
                    app.retire_http_mcp_bridge(Some(&generation), cx)
                });
            });
        }
        self.http_mcp.phase = HttpPhase::Disabled;
    }
    pub(super) fn http_mcp_owner_closed(&mut self, id: WorkspaceId, cx: &mut Context<Self>) {
        if self
            .http_mcp
            .owner
            .as_ref()
            .is_some_and(|(owner, _)| *owner == id)
        {
            self.stop_http_mcp(cx);
            self.http_mcp.owner = None;
            self.http_mcp.phase = HttpPhase::NeedsOwner;
            self.notify_http_mcp(cx);
        }
    }
    pub(super) fn http_mcp_lock_changed(&mut self, cx: &mut Context<Self>) {
        self.stop_http_mcp(cx);
        if self
            .http_mcp
            .settings
            .as_ref()
            .is_some_and(|s| s.enabled && s.transport == McpTransport::StreamableHttp)
        {
            self.start_http_mcp(cx);
        }
    }
    fn start_http_mcp(&mut self, cx: &mut Context<Self>) {
        if self.screen_locked {
            self.http_mcp.phase = HttpPhase::Locked;
            self.notify_http_mcp(cx);
            return;
        }
        let Some((_, owner)) = self.http_mcp.owner.clone() else {
            self.http_mcp.phase = HttpPhase::NeedsOwner;
            self.notify_http_mcp(cx);
            return;
        };
        let Some(settings) = self.http_mcp.settings.clone() else {
            return;
        };
        let params = owner.update(cx, |app, _| app.http_mcp_bridge_parameters(&settings));
        let Ok((scope, sender)) = params else {
            self.http_mcp.phase = HttpPhase::NeedsOwner;
            return;
        };
        let generation = self.http_mcp.generation;
        self.http_mcp.phase = HttpPhase::Starting;
        self.notify_http_mcp(cx);
        let prepared = cx.background_executor().spawn(async move {
            nyaterm_core::ai::mcp_http::validate_http_settings(
                settings.http_port,
                &settings.http_allowed_origins,
            )
            .map_err(str::to_owned)?;
            let helper = NyaTermApp::http_mcp_helper()?;
            let runtime = McpHostRuntime::start(settings.permission_mode, scope, sender, None)
                .map_err(|_| "MCP bridge could not start".to_owned())?;
            let endpoint = runtime.endpoint();
            let bootstrap = HttpBootstrap {
                version: CONTROL_VERSION,
                port: settings.http_port,
                allowed_origins: settings.http_allowed_origins,
                bridge_port: endpoint.port,
                bridge_token: endpoint.token.clone(),
                bridge_generation: endpoint.generation.clone(),
            };
            Ok::<_, String>((helper, runtime, bootstrap))
        });
        cx.spawn(async move |this, cx| {
            let prepared = prepared.await;
            let _ = this.update(cx, |this, cx| {
                if this.http_mcp.generation != generation
                    || this.screen_locked
                    || this.process_quitting
                {
                    cx.background_executor()
                        .spawn(async move {
                            drop(prepared);
                        })
                        .detach();
                    return;
                }
                let Ok((helper, runtime, bootstrap)) = prepared else {
                    this.http_mcp.phase = HttpPhase::Failed;
                    this.notify_http_mcp(cx);
                    return;
                };
                let bridge_generation = runtime.endpoint().generation.clone();
                let Some(owner) = owner.upgrade() else {
                    cx.background_executor()
                        .spawn(async move {
                            drop(runtime);
                        })
                        .detach();
                    this.http_mcp.phase = HttpPhase::NeedsOwner;
                    return;
                };
                owner.update(cx, |app, cx| app.install_http_mcp_bridge(runtime, cx));
                this.http_mcp.bridge_generation = Some(bridge_generation);
                match HttpSidecar::spawn(helper, bootstrap, this.http_mcp.previous.take()) {
                    Ok((child, events)) => {
                        this.http_mcp.child = Some(child);
                        this.drain_http_mcp(generation, events, cx);
                    }
                    Err(_) => {
                        this.stop_http_mcp(cx);
                        this.http_mcp.phase = HttpPhase::Failed;
                        this.notify_http_mcp(cx);
                    }
                }
            });
        })
        .detach();
    }
    fn drain_http_mcp(
        &mut self,
        generation: u64,
        mut events: futures::channel::mpsc::UnboundedReceiver<SidecarEvent>,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            while let Some(event) = events.next().await {
                if this
                    .update(cx, |this, cx| {
                        if this.http_mcp.generation != generation
                            || this.screen_locked
                            || this.process_quitting
                        {
                            return;
                        }
                        match event {
                            SidecarEvent::Ready { token, .. } => {
                                this.http_mcp.token = Some(token);
                                this.http_mcp.phase = HttpPhase::Running;
                            }
                            SidecarEvent::Failed(failure) => {
                                this.stop_http_mcp(cx);
                                this.http_mcp.phase = if failure == SidecarFailure::PortInUse {
                                    HttpPhase::PortInUse
                                } else {
                                    HttpPhase::Failed
                                };
                            }
                            SidecarEvent::Stopped => {
                                this.stop_http_mcp(cx);
                                this.http_mcp.phase = HttpPhase::Failed;
                            }
                        }
                        this.notify_http_mcp(cx);
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }
    fn notify_http_mcp(&self, cx: &mut Context<Self>) {
        for entry in self.windows.values() {
            let shell = entry.shell.clone();
            cx.defer(move |cx| {
                let _ = shell.update(cx, |shell, cx| {
                    if let Some(app) = &shell.app {
                        app.update(cx, |app, cx| app.refresh_http_mcp_settings(cx));
                    }
                });
            });
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests;
