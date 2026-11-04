use crate::text::{NyaTextState, NyaTextStyle};
use gpui::{IntoElement, RenderOnce, SharedString};
use gpui_base::text::TextView;

/// Selectable plain text backed by gpui-kit's window selection support.
#[derive(IntoElement)]
pub struct NyaSelectableText {
    id: SharedString,
    text: SharedString,
    state: Option<NyaTextState>,
    style: Option<NyaTextStyle>,
}

impl NyaSelectableText {
    pub fn new(id: impl Into<SharedString>, text: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
            state: None,
            style: None,
        }
    }

    pub fn from_state(state: &NyaTextState) -> Self {
        Self {
            id: "owned-plain-text".into(),
            text: "".into(),
            state: Some(state.clone()),
            style: None,
        }
    }

    pub fn style(mut self, style: NyaTextStyle) -> Self {
        self.style = Some(style);
        self
    }
}

impl RenderOnce for NyaSelectableText {
    fn render(self, window: &mut gpui::Window, cx: &mut gpui::App) -> impl IntoElement {
        if self.state.is_none() && self.style.is_none() {
            let escaped = self
                .text
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;");
            return gpui_kit::component::text::TextView::html(
                self.id,
                format!("<pre>{escaped}</pre>"),
            )
            .selectable(true)
            .into_any_element();
        }
        let escaped = self
            .text
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;");
        let view = self
            .state
            .as_ref()
            .map(NyaTextState::view)
            .unwrap_or_else(|| TextView::html(self.id, format!("<pre>{escaped}</pre>")));
        let style = self
            .style
            .unwrap_or_else(|| NyaTextStyle::themed(cx))
            .raw_code(window)
            .foreground_color(window.text_style().color);
        let view = style.apply(view).selectable(true);
        if let Some(state) = self.state {
            state.present(view, style, window, cx).into_any_element()
        } else {
            view.into_any_element()
        }
    }
}
