//! Caller-handled interactions. Host tokens are minted and checked by the desktop.
use crate::models::{DockerConfirmState, DockerTab, RemoteProcessSortKey};
use crate::panels::RemoteMonitorKind;
use gpui::{App, Window};
use std::rc::Rc;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HostInteractionToken(pub u64);
pub type RemoteMonitorActionHandler =
    Rc<dyn Fn(RemoteMonitorAction, Option<&mut Window>, &mut App) -> bool>;
pub enum RemoteMonitorAction {
    Tick(RemoteMonitorKind),
    CheckHost(HostInteractionToken),
    CopyProcess(String, &'static str),
    SelectProcess(u32),
    CloseProcessMenu,
    ToggleProcessMenu(u32),
    SignalProcess(u32, &'static str),
    ApplyProcessNice,
    SortProcess(RemoteProcessSortKey),
    ToggleCurrentProcessSort,
    ToggleGpu(String),
    ToggleNpu(String),
    ProcessOffset(RemoteMonitorKind, usize),
    ToggleCpu,
    ToggleMemory,
    ToggleNetworkInterfaces,
    CloseContainerMenu,
    ToggleContainerMenu(String),
    ContainerDetails(String),
    ContainerLogs(String),
    ContainerTerminal(String),
    ContainerAction(String, &'static str),
    ConfirmDocker(DockerConfirmState),
    SetDockerTab(DockerTab),
    ToggleDockerTabMenu,
    CloseComposeMenu,
    ToggleComposeMenu(String),
    ComposeAction(String, Option<String>, &'static str),
    ComposeServiceLogs(String, Option<String>, String),
    ComposeServiceAction(String, Option<String>, String, &'static str),
    ToggleComposeProject(String, Option<String>),
    LoadComposeServices(String, Option<String>),
}
