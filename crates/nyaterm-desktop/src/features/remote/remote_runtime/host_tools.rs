//! App-side half of the Ports and Services panels.
//!
//! The panels own their data and refresh clocks. The app only answers two
//! questions on their behalf: *which machine are they inspecting* (this one, or
//! the active SSH connection) and *are they on screen*. Both are pushed into the
//! panels at the same reconciliation point as the other remote panels, so a
//! panel never reads the app while rendering.

use crate::app_shell::session_hub::ssh_connections::SshConnectionLease;
use crate::blocking_jobs::BlockingJobScheduler;
use crate::features::NyaTermApp;
use crate::features::pages::remote::{HostContext, HostResolution, HostToolKind, HostToolsPanel};
use gpui::{AppContext as _, Context, Entity, WeakEntity};
use nyaterm_remote_ops::host::HostTarget;
use rust_i18n::t;

/// A captured interaction stays bound to both its session and concrete lease.
#[derive(Clone)]
pub(in crate::features) struct HostInteractionIdentity {
    signature: String,
    epoch: u64,
    connection: Option<SshConnectionLease>,
}
impl PartialEq for HostInteractionIdentity {
    fn eq(&self, other: &Self) -> bool {
        self.epoch == other.epoch
            && self.signature == other.signature
            && match (&self.connection, &other.connection) {
                (Some(a), Some(b)) => a.same_connection(b),
                (None, None) => true,
                _ => false,
            }
    }
}
impl std::fmt::Debug for HostInteractionIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostInteractionIdentity")
            .field("signature", &self.signature)
            .field("connected", &self.connection.is_some())
            .finish()
    }
}

pub(in crate::features) struct HostToolPanels {
    ports: Entity<HostToolsPanel>,
    services: Entity<HostToolsPanel>,
    connection: Option<SshConnectionLease>,
}

impl HostToolPanels {
    pub(in crate::features) const ALL: [HostToolKind; 2] =
        [HostToolKind::Ports, HostToolKind::Services];

    pub(in crate::features) fn new(
        app: WeakEntity<NyaTermApp>,
        scheduler: BlockingJobScheduler,
        cx: &mut Context<NyaTermApp>,
    ) -> Self {
        let make = |kind: HostToolKind, cx: &mut Context<NyaTermApp>| {
            let (app, scheduler) = (app.clone(), scheduler.clone());
            cx.new(|cx| HostToolsPanel::new(kind, app, scheduler, cx))
        };
        Self {
            ports: make(HostToolKind::Ports, cx),
            services: make(HostToolKind::Services, cx),
            connection: None,
        }
    }

    pub(in crate::features) fn entity(&self, kind: HostToolKind) -> &Entity<HostToolsPanel> {
        match kind {
            HostToolKind::Ports => &self.ports,
            HostToolKind::Services => &self.services,
        }
    }
}

impl NyaTermApp {
    pub(in crate::features) fn host_interaction_identity(
        &self,
        cx: &gpui::App,
    ) -> HostInteractionIdentity {
        let connection = self.session.active_id().and_then(|id| {
            self.session
                .ssh_connection_for_session(&self.ssh_scoped_session_id(id))
        });
        HostInteractionIdentity {
            epoch: self
                .host_tool_panels
                .entity(HostToolKind::Ports)
                .read(cx)
                .host_epoch(),
            signature: self.host_tools_signature(),
            connection,
        }
    }

    /// A cheap identity for "which host would the panels inspect right now".
    /// Compared on every reconciliation, so it must not build the target.
    fn host_tools_signature(&self) -> String {
        let Some(id) = self.session.active_id() else {
            return "none".to_string();
        };
        if self.session.active_ssh_config().is_some() {
            let scoped = self.ssh_scoped_session_id(id);
            let connected = self.session.ssh_connection_for_session(&scoped).is_some();
            return format!("ssh:{id}:{}:{connected}", self.session.is_disconnected(id));
        }
        if self.session.is_local_pty(id) {
            if self.session.active_is_local_pty() {
                format!("local:{id}")
            } else {
                format!("local-disconnected:{id}")
            }
        } else {
            format!("unsupported:{id}")
        }
    }

    /// Like Resources and Processes, the host tools need an open terminal: a
    /// local shell inspects this machine, an SSH session inspects its host.
    fn resolve_host_tools_host(&mut self, kind: HostToolKind) -> HostResolution {
        let Some(id) = self.session.active_id_owned() else {
            return HostResolution::Unavailable(no_session_message(kind));
        };
        if self.session.active_ssh_config().is_some() {
            return match self.active_ssh_runtime_context("opening host tools") {
                Ok(context) => {
                    let os = context.target.os_hint();
                    HostResolution::Ready(HostContext {
                        key: format!("ssh:{id}"),
                        target: context.target,
                        os,
                    })
                }
                Err(message) => HostResolution::Unavailable(message),
            };
        }
        if self.session.is_local_pty(&id) {
            if self.session.active_is_local_pty() {
                local_host()
            } else {
                HostResolution::Unavailable(t!("hostTools.sessionDisconnected").to_string())
            }
        } else {
            HostResolution::Unavailable(t!("hostTools.unsupportedSession").to_string())
        }
    }

    /// Push the host, theme and visibility into the Ports/Services panels.
    ///
    /// Runs wherever the other remote panels reconcile (session switches,
    /// settings, render). Does nothing but a string compare when nothing moved.
    pub(in crate::features) fn sync_host_tools(&mut self, cx: &mut Context<Self>) {
        let connection = self.session.active_id().and_then(|id| {
            let scoped = self.ssh_scoped_session_id(id);
            self.session.ssh_connection_for_session(&scoped)
        });
        let connection_changed = match (&self.host_tool_panels.connection, &connection) {
            (Some(old), Some(new)) => !old.same_connection(new),
            (None, None) => false,
            _ => true,
        };
        self.host_tool_panels.connection = connection;
        let signature = self.host_tools_signature();
        let chrome = self.panel_chrome();
        for kind in HostToolPanels::ALL {
            let panel = self.host_tool_panels.entity(kind).clone();
            if connection_changed || panel.read(cx).signature() != signature {
                let resolved = self.resolve_host_tools_host(kind);
                panel.update(cx, |panel, cx| {
                    if connection_changed {
                        panel.invalidate_connection(cx);
                    }
                    panel.set_host(signature.clone(), resolved, cx)
                });
            }
            panel.update(cx, |panel, cx| panel.set_chrome(chrome, cx));
            let demand = self.panel_is_rendered(kind.nav_item()) && panel.read(cx).host_ready();
            panel.update(cx, |panel, cx| panel.set_demand(demand, cx));
        }
    }

    pub(in crate::features) fn refresh_host_tool(
        &mut self,
        kind: HostToolKind,
        cx: &mut Context<Self>,
    ) {
        let panel = self.host_tool_panels.entity(kind).clone();
        panel.update(cx, |panel, cx| panel.refresh(cx));
    }
}

fn no_session_message(kind: HostToolKind) -> String {
    match kind {
        HostToolKind::Ports => t!("hostTools.noSessionPorts").to_string(),
        HostToolKind::Services => t!("hostTools.noSessionServices").to_string(),
    }
}

fn local_host() -> HostResolution {
    let target = HostTarget::Local;
    let os = target.os_hint();
    HostResolution::Ready(HostContext {
        key: "local".into(),
        target,
        os,
    })
}
