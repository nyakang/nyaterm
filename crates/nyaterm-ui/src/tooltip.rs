use gpui::{AnyView, App, SharedString, Window};
use gpui_kit::component::tooltip::Tooltip;

/// Theme-aware text tooltip backed by `gpui-kit`.
pub struct NyaTooltip {
    inner: Tooltip,
}

impl NyaTooltip {
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self {
            inner: Tooltip::new(text.into()),
        }
    }

    pub fn build(self, window: &mut Window, cx: &mut App) -> AnyView {
        self.inner.build(window, cx)
    }
}
