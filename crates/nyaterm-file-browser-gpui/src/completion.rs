use gpui::{
    App, IntoElement, MouseButton, ScrollHandle, SharedString, Window, div, prelude::*, px, rgb,
};
use nyaterm_files::sftp::RemoteFilePath;
use nyaterm_ui::{NyaScrollable, ThemePalette};
use std::rc::Rc;
use std::sync::Arc;

#[derive(Clone)]
pub struct PathCandidate {
    pub label: String,
    pub path: RemoteFilePath,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum CompletionStatus {
    #[default]
    Inactive,
    Loading,
    Ready,
    Unavailable,
    TimedOut,
    WaitingHome,
}

#[derive(Clone, Default)]
pub struct CompletionPresentation {
    pub status: CompletionStatus,
    pub candidates: Arc<[PathCandidate]>,
    pub selected: Option<usize>,
    pub truncated: bool,
}

impl CompletionPresentation {
    pub fn visible(&self) -> bool {
        self.status != CompletionStatus::Inactive
    }
}

pub fn path_candidates(
    id: impl Into<SharedString>,
    model: &CompletionPresentation,
    palette: ThemePalette,
    scroll: &ScrollHandle,
    max_height: f32,
    accept: impl Fn(usize, &RemoteFilePath, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let accept = Rc::new(accept);
    let message = match model.status {
        CompletionStatus::Loading => Some("fileExplorer.pathCompletionLoading"),
        CompletionStatus::Unavailable => Some("fileExplorer.pathCompletionUnavailable"),
        CompletionStatus::TimedOut => Some("fileExplorer.pathCompletionTimeout"),
        CompletionStatus::WaitingHome => Some("fileExplorer.pathCompletionHome"),
        CompletionStatus::Ready if model.candidates.is_empty() => {
            Some("fileExplorer.pathCompletionEmpty")
        }
        _ => None,
    };
    div()
        .id(id.into())
        .w_full()
        .min_w_0()
        .occlude()
        .rounded_sm()
        .border_1()
        .border_color(rgb(palette.border))
        .bg(rgb(palette.input_surface()))
        .text_color(rgb(palette.text))
        .text_size(px(13.))
        .child(
            div()
                .id("path-completion-body")
                .max_h(px(max_height))
                .overflow_y_scroll()
                .track_scroll(scroll)
                .children(
                    model
                        .candidates
                        .iter()
                        .enumerate()
                        .map(|(index, candidate)| {
                            let accept = accept.clone();
                            let path = candidate.path.clone();
                            div()
                                .id(index)
                                .h(px(28.))
                                .px_2()
                                .flex()
                                .items_center()
                                .min_w_0()
                                .when(model.selected == Some(index), |row| {
                                    row.bg(rgb(palette.border))
                                })
                                .hover(|style| style.bg(rgb(palette.border)))
                                .cursor_pointer()
                                .child(
                                    div()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .child(candidate.label.clone()),
                                )
                                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                                    window.prevent_default();
                                    cx.stop_propagation();
                                    accept(index, &path, window, cx);
                                })
                        }),
                )
                .when_some(message, |body, message| {
                    body.child(
                        div()
                            .px_2()
                            .py_1()
                            .child(rust_i18n::t!(message).to_string()),
                    )
                })
                .when(model.truncated, |body| {
                    body.child(
                        div().px_2().py_1().child(
                            rust_i18n::t!("fileExplorer.pathCompletionTruncated").to_string(),
                        ),
                    )
                }),
        )
        .vertical_scrollbar(scroll)
}
