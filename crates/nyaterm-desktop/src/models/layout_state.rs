use gpui::Pixels;

use super::{BottomPanelMode, PanelSide};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PanelResizeSide {
    Left,
    Right,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct PanelResizeState {
    pub(crate) side: PanelResizeSide,
    pub(crate) start_x: Pixels,
    pub(crate) start_width: Pixels,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct TransferHeightResizeState {
    pub(crate) start_y: Pixels,
    pub(crate) start_height: Pixels,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct BottomPanelResizeState {
    pub(crate) mode: BottomPanelMode,
    pub(crate) start_y: Pixels,
    pub(crate) start_height: Pixels,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PanelStackResizeState {
    pub(crate) side: PanelSide,
    pub(crate) above_id: String,
    pub(crate) below_id: String,
    pub(crate) start_y: Pixels,
    pub(crate) above_weight: f32,
    pub(crate) below_weight: f32,
    pub(crate) pair_height: f64,
}
