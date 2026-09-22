use gpui::{Pixels, px};
use gpui_kit::component::Size;

/// Standard height for ordinary single-line form controls.
pub const NYA_FORM_CONTROL_HEIGHT_PX: f32 = 32.;

pub(crate) fn form_control_height() -> Pixels {
    px(NYA_FORM_CONTROL_HEIGHT_PX)
}

pub(crate) fn form_control_size() -> Size {
    Size::Medium
}
