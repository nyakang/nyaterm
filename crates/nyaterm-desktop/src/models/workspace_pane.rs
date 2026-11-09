//! GPUI resize state over the shared workspace tree contracts.
use gpui::Pixels;
pub(crate) use nyaterm_core::workspace_layout::pane::{
    SmartSplitMode, WorkspacePaneNode, WorkspaceSplitDirection, WorkspaceSplitState, uuid_v4_like,
};

#[derive(Debug, Clone)]
pub(crate) struct WorkspaceSplitResizeState {
    pub(crate) split_id: String,
    pub(crate) direction: WorkspaceSplitDirection,
    pub(crate) start_pos: Pixels,
    pub(crate) start_ratio: u8,
    pub(crate) container_size: f32,
}
