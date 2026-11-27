mod images;

#[cfg(test)]
mod tests;

pub use images::NyaMarkdownImagePolicy;

use std::sync::Arc;

use crate::text::{NyaCodeBlockActions, NyaTextState, NyaTextStyle};
use gpui::{App, IntoElement, RenderOnce, SharedString, Window};
use gpui_base::text::{CodeBlock, TextView};

/// Stable NyaTerm boundary for selectable Markdown rendered by gpui-kit.
#[derive(IntoElement)]
pub struct NyaMarkdown {
    id: SharedString,
    source: SharedString,
    state: Option<NyaTextState>,
    style: Option<NyaTextStyle>,
    code_block_actions: Option<NyaCodeBlockActions>,
    image_policy: NyaMarkdownImagePolicy,
}

impl NyaMarkdown {
    pub fn new(id: impl Into<SharedString>, source: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            source: source.into(),
            state: None,
            style: None,
            code_block_actions: None,
            image_policy: NyaMarkdownImagePolicy::Existing,
        }
    }

    pub fn from_state(state: &NyaTextState) -> Self {
        Self {
            id: "owned-markdown".into(),
            source: "".into(),
            state: Some(state.clone()),
            style: None,
            code_block_actions: None,
            image_policy: NyaMarkdownImagePolicy::Existing,
        }
    }

    pub fn style(mut self, style: NyaTextStyle) -> Self {
        self.style = Some(style);
        self
    }

    /// Selects resource access for this surface without changing other callers.
    pub fn image_policy(mut self, policy: NyaMarkdownImagePolicy) -> Self {
        self.image_policy = policy;
        self
    }

    /// Renders caller-provided actions in the corner of every fenced code
    /// block, replacing the block's language label.
    pub fn code_block_actions<F, E>(mut self, actions: F) -> Self
    where
        F: Fn(&CodeBlock, &mut Window, &mut App) -> E + Send + Sync + 'static,
        E: IntoElement,
    {
        self.code_block_actions = Some(Arc::new(move |block, window, cx| {
            actions(block, window, cx).into_any_element()
        }));
        self
    }
}

impl RenderOnce for NyaMarkdown {
    fn render(self, window: &mut gpui::Window, cx: &mut gpui::App) -> impl IntoElement {
        if let Some(state) = &self.state {
            // Retained selection views must use the same resource policy.
            state.set_image_policy(self.image_policy);
        }
        // The state carries the actions so the offscreen selection clones in
        // retention paint the same code-block chrome as the live view.
        if let (Some(state), Some(actions)) = (&self.state, &self.code_block_actions) {
            state.set_code_block_actions(actions.clone());
        }
        if self.state.is_none()
            && self.style.is_none()
            && self.image_policy == NyaMarkdownImagePolicy::Existing
        {
            let view = gpui_kit::component::text::TextView::markdown(self.id, self.source)
                .selectable(true)
                .on_link_click(open_link);
            let view = match &self.code_block_actions {
                Some(actions) => {
                    let actions = actions.clone();
                    view.code_block_actions(move |block, window, cx| actions(block, window, cx))
                }
                None => view,
            };
            return view.into_any_element();
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
        let view = self
            .image_policy
            .apply(style.apply(view).selectable(true).on_link_click(open_link));
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
