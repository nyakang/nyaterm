mod docker;
mod docker_view;
mod host_tools;
mod panels;
mod process;
mod process_view;
mod stats_view;

pub(in crate::features) use host_tools::{
    HostContext, HostResolution, HostToolKind, HostToolsPanel,
};
pub(in crate::features) use panels::{RemoteMonitorKind, RemotePanels};
