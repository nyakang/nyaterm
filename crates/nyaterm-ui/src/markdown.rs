pub mod lightweight;

use crate::text::{NyaTextState, NyaTextStyle};
use gpui::{IntoElement, RenderOnce, SharedString};
use gpui_base::text::TextView;

/// Stable NyaTerm boundary for selectable Markdown rendered by gpui-kit.
#[derive(IntoElement)]
pub struct NyaMarkdown {
    id: SharedString,
    source: SharedString,
    state: Option<NyaTextState>,
    style: Option<NyaTextStyle>,
}

impl NyaMarkdown {
    pub fn new(id: impl Into<SharedString>, source: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            source: source.into(),
            state: None,
            style: None,
        }
    }

    pub fn from_state(state: &NyaTextState) -> Self {
        Self {
            id: "owned-markdown".into(),
            source: "".into(),
            state: Some(state.clone()),
            style: None,
        }
    }

    pub fn style(mut self, style: NyaTextStyle) -> Self {
        self.style = Some(style);
        self
    }
}

impl RenderOnce for NyaMarkdown {
    fn render(self, window: &mut gpui::Window, cx: &mut gpui::App) -> impl IntoElement {
        if self.state.is_none() && self.style.is_none() {
            return gpui_kit::component::text::TextView::markdown(self.id, self.source)
                .selectable(true)
                .on_link_click(open_link)
                .into_any_element();
        }
        let view = self
            .state
            .as_ref()
            .map(NyaTextState::view)
            .unwrap_or_else(|| TextView::markdown(self.id, self.source));
        let style = self
            .style
            .unwrap_or_else(|| NyaTextStyle::themed(cx))
            .foreground_color(window.text_style().color);
        let view = style.apply(view).selectable(true).on_link_click(open_link);
        if let Some(state) = self.state {
            state.present(view, style, window, cx).into_any_element()
        } else {
            view.into_any_element()
        }
    }
}

fn open_link(url: &SharedString, _: &gpui::ClickEvent, _: &mut gpui::Window, cx: &mut gpui::App) {
    let Ok(parsed) = url::Url::parse(url) else {
        return;
    };
    if matches!(parsed.scheme(), "http" | "https" | "mailto") {
        cx.open_url(url.as_ref());
    }
}
