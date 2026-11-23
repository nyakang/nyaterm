mod docker;
mod host_tools;
mod panels;
pub(in crate::features) use host_tools::{
    HostContext, HostResolution, HostToolKind, HostToolsPanel,
};
pub(in crate::features) use panels::{RemoteMonitorKind, RemotePanels};
