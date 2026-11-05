use gpui::{IntoElement, RenderOnce};

use crate::text::{NyaCodeHighlights, NyaTextState, NyaTextStyle};

/// Selectable code whose syntax has already been parsed by a background job.
#[derive(IntoElement)]
pub struct NyaSelectableCode {
    state: NyaTextState,
    style: NyaTextStyle,
    highlights: NyaCodeHighlights,
}

impl NyaSelectableCode {
    pub fn from_state(
        state: &NyaTextState,
        style: NyaTextStyle,
        highlights: NyaCodeHighlights,
    ) -> Self {
        Self {
            state: state.clone(),
            style,
            highlights,
        }
    }
}

impl RenderOnce for NyaSelectableCode {
    fn render(self, window: &mut gpui::Window, cx: &mut gpui::App) -> impl IntoElement {
        let style = self.style.raw_code(window);
        self.state.set_code_highlights(self.highlights);
        let view = style.apply(self.state.view());
        self.state.present(view, style, window, cx)
    }
}
