//! The native adapter is replaceable with controlled jobs in GPUI regressions.
use super::{HostContext, HostToolKind, LOG_LINES, PendingAction, RefreshOutput};
use nyaterm_remote_ops::host::{Elevation, signal_pid};
use nyaterm_remote_ops::ports::{ProcessDetails, list_ports, process_details};
use nyaterm_remote_ops::services::{
    ServiceBackend, ServiceEntry, fetch_logs, list_services, run_action,
};

pub(super) enum Request {
    Refresh(HostToolKind),
    Action(PendingAction, Option<ServiceBackend>),
    Logs(ServiceEntry, ServiceBackend),
    Details(u32),
}
pub(super) enum Output {
    Refresh(RefreshOutput),
    Action,
    Logs(String),
    Details(ProcessDetails),
}
pub(super) trait HostToolsExecutor: Send + Sync {
    fn execute(
        &self,
        host: &HostContext,
        request: Request,
        elevation: Elevation,
    ) -> Result<Output, String>;
}
pub(super) struct NativeExecutor;
impl HostToolsExecutor for NativeExecutor {
    fn execute(
        &self,
        host: &HostContext,
        request: Request,
        elevation: Elevation,
    ) -> Result<Output, String> {
        (|| match request {
            Request::Refresh(HostToolKind::Ports) => list_ports(&host.target, host.os, elevation)
                .map(RefreshOutput::Ports)
                .map(Output::Refresh),
            Request::Refresh(HostToolKind::Services) => {
                list_services(&host.target, host.os, elevation)
                    .map(RefreshOutput::Services)
                    .map(Output::Refresh)
            }
            Request::Action(PendingAction::Service(entry, action), Some(backend)) => {
                run_action(&host.target, host.os, backend, &entry, action, elevation)
                    .map(|()| Output::Action)
            }
            Request::Action(PendingAction::Kill { pid, force }, _) => {
                signal_pid(&host.target, host.os, pid, force, elevation).map(|()| Output::Action)
            }
            Request::Action(_, None) => anyhow::bail!("service backend unavailable"),
            Request::Logs(entry, backend) => {
                fetch_logs(&host.target, host.os, backend, &entry, LOG_LINES, elevation)
                    .map(Output::Logs)
            }
            Request::Details(pid) => {
                process_details(&host.target, host.os, pid, elevation).map(Output::Details)
            }
        })()
        .map_err(|error| error.to_string())
    }
}
