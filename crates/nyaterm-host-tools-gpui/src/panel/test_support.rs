//! Controlled fixtures for cross-crate integration tests; disabled in production.
use super::executor::{HostToolsExecutor, Output, Request};
use super::jobs::Lane;
use super::{HostContext, HostResolution, HostToolKind, HostToolsPanel, RefreshOutput};
use gpui::{Context, Window};
use nyaterm_remote_ops::host::{Elevation, HostOs, HostTarget};
use nyaterm_remote_ops::ports::{PortEntry, PortProtocol, PortsSnapshot, ProcessDetails};
use nyaterm_remote_ops::services::{ServiceBackend, ServiceEntry, ServiceStartup, ServiceState};
use std::collections::VecDeque;
use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;
#[derive(Default)]
pub struct Gate {
    result: Mutex<Option<Result<Output, String>>>,
    ready: Condvar,
}
impl Gate {
    pub(super) fn release(&self, result: Result<Output, String>) {
        *self.result.lock().unwrap() = Some(result);
        self.ready.notify_one();
    }
    fn wait(&self) -> Result<Output, String> {
        let (mut result, _) = self
            .ready
            .wait_timeout_while(
                self.result.lock().unwrap(),
                Duration::from_secs(5),
                |result| result.is_none(),
            )
            .unwrap();
        result
            .take()
            .unwrap_or_else(|| Err("controlled job was not released".into()))
    }
}
pub(super) enum Behavior {
    Block(Arc<Gate>),
    #[cfg(test)]
    Error(&'static str),
    #[cfg(test)]
    Panic,
}
#[derive(Default)]
pub struct ControlledExecutor {
    pub(super) calls: AtomicUsize,
    action_calls: AtomicUsize,
    behaviors: Mutex<VecDeque<(Lane, Behavior)>>,
}
impl ControlledExecutor {
    pub(super) fn push(&self, lane: Lane, behavior: Behavior) {
        self.behaviors.lock().unwrap().push_back((lane, behavior));
    }
}
impl HostToolsExecutor for ControlledExecutor {
    fn execute(&self, _: &HostContext, request: Request, _: Elevation) -> Result<Output, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if matches!(&request, Request::Action(_, _)) {
            self.action_calls.fetch_add(1, Ordering::SeqCst);
        }
        let lane = match &request {
            Request::Refresh(_) => Lane::Refresh,
            Request::Action(_, _) => Lane::Action,
            Request::Logs(_, _) => Lane::Logs,
            Request::Details(_) => Lane::Details,
        };
        let mut behaviors = self.behaviors.lock().unwrap();
        let behavior = behaviors
            .iter()
            .position(|(expected, _)| *expected == lane)
            .map(|index| behaviors.remove(index).unwrap().1);
        drop(behaviors);
        match behavior {
            Some(Behavior::Block(gate)) => gate.wait(),
            #[cfg(test)]
            Some(Behavior::Error(error)) => Err(error.into()),
            #[cfg(test)]
            Some(Behavior::Panic) => panic!("controlled worker failure"),
            None => match request {
                Request::Refresh(HostToolKind::Ports) => Ok(ports(80)),
                Request::Refresh(HostToolKind::Services) => Ok(Output::Refresh(
                    RefreshOutput::Services(nyaterm_remote_ops::services::ServicesSnapshot {
                        backend: ServiceBackend::Systemd,
                        entries: vec![service("system")],
                        partial: false,
                    }),
                )),
                Request::Action(_, _) => Ok(Output::Action),
                Request::Logs(_, _) => Ok(Output::Logs("current host log".into())),
                Request::Details(_) => Ok(Output::Details(ProcessDetails::default())),
            },
        }
    }
}
pub(super) fn ports(port: u16) -> Output {
    Output::Refresh(RefreshOutput::Ports(PortsSnapshot {
        entries: vec![PortEntry {
            protocol: PortProtocol::Tcp,
            address: "127.0.0.1".into(),
            port,
            pid: Some(42),
            process: Some("fixture".into()),
            user: None,
        }],
        owners_hidden: false,
    }))
}
pub(super) fn service(scope: &str) -> ServiceEntry {
    ServiceEntry {
        name: "same-service".into(),
        description: String::new(),
        state: ServiceState::Running,
        detail: String::new(),
        startup: ServiceStartup::Enabled,
        pid: Some(42),
        scope: scope.into(),
    }
}
pub fn host(key: &str) -> HostResolution {
    HostResolution::Ready(HostContext {
        key: key.into(),
        target: HostTarget::Local,
        os: HostOs::Linux,
    })
}

impl ControlledExecutor {
    pub fn action_calls(&self) -> usize {
        self.action_calls.load(Ordering::SeqCst)
    }
    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
    pub fn block_next_refresh(&self) -> Arc<Gate> {
        let gate = Arc::new(Gate::default());
        self.push(Lane::Refresh, Behavior::Block(gate.clone()));
        gate
    }
}
impl Gate {
    pub fn release_ports(&self, port: u16) {
        self.release(Ok(ports(port)));
    }
}
pub fn install_executor(panel: &mut HostToolsPanel, executor: Arc<ControlledExecutor>) {
    panel.executor = executor;
}
pub fn has_pending_reads(panel: &HostToolsPanel) -> bool {
    [Lane::Refresh, Lane::Logs, Lane::Details]
        .into_iter()
        .any(|lane| panel.jobs.pending(lane))
}
pub fn has_pending_action(panel: &HostToolsPanel) -> bool {
    panel.jobs.pending(Lane::Action)
}
pub fn cancellation_count(panel: &HostToolsPanel) -> usize {
    panel.cancellations.len()
}
pub fn all_cancelled(panel: &HostToolsPanel) -> bool {
    panel
        .cancellations
        .values()
        .all(|handle| handle.is_cancelled())
}
pub fn probe_hidden_reads(panel: &mut HostToolsPanel, cx: &mut Context<HostToolsPanel>) {
    panel.refresh(cx);
    panel.open_logs(service("system"), cx);
    panel.submit_job(Lane::Details, "fixture".into(), Request::Details(42), cx);
    panel.tick(cx);
}
pub fn request_kill(
    panel: &mut HostToolsPanel,
    pid: u32,
    force: bool,
    window: &mut Window,
    cx: &mut Context<HostToolsPanel>,
) {
    panel.confirm_kill(pid, force, window, cx);
}
