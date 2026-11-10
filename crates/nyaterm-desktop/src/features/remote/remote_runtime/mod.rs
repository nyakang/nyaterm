mod auto_refresh;
mod panel_layout;

mod docker;
mod gpu;
mod helpers;
pub(in crate::features) mod host_tools;

mod process;
mod stats;

pub(in crate::features) use host_tools::HostToolPanels;
