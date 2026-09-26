use std::ops::Range;

use gpui::{
    Action as _, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable,
    IntoElement, Render, SharedString, Subscription, Window, div, prelude::*, px,
};
use gpui_kit::component::input::{Editor, EditorState, InputEvent, Redo, Undo};

use crate::input_focus::register_nya_input_focus;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NyaDocumentEditorEvent {
    Changed(String),
    Blurred(String),
    Updated,
}

/// Full-size native document editor used by modeless document windows.
///
/// This type is the stable NyaTerm boundary around gpui-kit's editor;
/// desktop features never need to import gpui-kit directly.
pub struct NyaDocumentEditorState {
    editor: Entity<EditorState>,
    subscription: Subscription,
    observation: Subscription,
    pending_content: Option<SharedString>,
    silent_content: Option<SharedString>,
    read_only: bool,
    font_size: Option<f32>,
    font_family: Option<SharedString>,
}

impl NyaDocumentEditorState {
    pub fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        content: impl Into<SharedString>,
    ) -> Self {
        Self::new_with_placeholder(window, cx, content, SharedString::default())
    }

    pub fn new_with_placeholder(
        window: &mut Window,
        cx: &mut Context<Self>,
        content: impl Into<SharedString>,
        placeholder: impl Into<SharedString>,
    ) -> Self {
        Self::build(window, cx, content.into(), placeholder.into(), None)
    }

    pub fn new_source(
        window: &mut Window,
        cx: &mut Context<Self>,
        content: impl Into<SharedString>,
        language: impl Into<SharedString>,
    ) -> Self {
        Self::build(
            window,
            cx,
            content.into(),
            SharedString::default(),
            Some(language.into()),
        )
    }

    fn build(
        window: &mut Window,
        cx: &mut Context<Self>,
        content: SharedString,
        placeholder: SharedString,
        language: Option<SharedString>,
    ) -> Self {
        let editor = cx.new(|cx| {
            let state = EditorState::new(window, cx)
                .default_value(content)
                .placeholder(placeholder)
                .soft_wrap(true);
            if let Some(language) = language {
                state.language(language).folding(true).line_number(true)
            } else {
                state
            }
        });
        register_nya_input_focus(&editor.read(cx).focus_handle(cx), cx);
        let subscription = cx.subscribe(&editor, |this, editor, event: &InputEvent, cx| {
            let value = editor.read(cx).value().to_string();
            match event {
                InputEvent::Change => {
                    if this
                        .silent_content
                        .take()
                        .is_some_and(|expected| expected.as_ref() == value)
                    {
                        return;
                    }
                    cx.emit(NyaDocumentEditorEvent::Changed(value));
                }
                InputEvent::Blur => cx.emit(NyaDocumentEditorEvent::Blurred(value)),
                InputEvent::Focus | InputEvent::PressEnter { .. } => {}
            }
        });
        let observation = cx.observe(&editor, |_, _, cx| {
            cx.emit(NyaDocumentEditorEvent::Updated);
        });
        Self {
            editor,
            subscription,
            observation,
            pending_content: None,
            silent_content: None,
            read_only: false,
            font_size: None,
            font_family: None,
        }
    }

    pub fn set_read_only(&mut self, read_only: bool, cx: &mut Context<Self>) {
        if self.read_only != read_only {
            self.read_only = read_only;
            cx.notify();
        }
    }

    pub fn set_font_size(&mut self, font_size: f32, cx: &mut Context<Self>) {
        if self.font_size != Some(font_size) {
            self.font_size = Some(font_size);
            cx.notify();
        }
    }

    pub fn set_font_family(
        &mut self,
        font_family: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        let font_family = font_family.into();
        if self.font_family.as_ref() != Some(&font_family) {
            self.font_family = Some(font_family);
            cx.notify();
        }
    }

    pub fn cursor_position(&self, cx: &App) -> (usize, usize) {
        let value = self.editor.read(cx).value();
        let cursor = self.editor.read(cx).cursor().min(value.len());
        let before = &value[..cursor];
        let line = before.bytes().filter(|byte| *byte == b'\n').count() + 1;
        let line_start = before.rfind('\n').map(|index| index + 1).unwrap_or(0);
        (line, before[line_start..].chars().count() + 1)
    }

    pub fn open_search(&mut self, replace: bool, cx: &mut Context<Self>) {
        self.editor
            .update(cx, |editor, cx| editor.open_search(replace, cx));
    }

    pub fn value(&self, cx: &App) -> String {
        self.editor.read(cx).value().to_string()
    }

    pub fn content_equals(&self, content: &str, cx: &App) -> bool {
        self.editor.read(cx).value().as_ref() == content
    }

    pub fn selected_range(&self, cx: &App) -> Range<usize> {
        self.editor.read(cx).selected_range()
    }

    pub fn move_cursor_to_end(&mut self, cx: &mut Context<Self>) {
        let end = self.editor.read(cx).value().len();
        self.editor
            .update(cx, |editor, cx| editor.set_selected_range(end..end, cx));
    }

    pub fn replace_content(&mut self, content: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.silent_content = Some(SharedString::from(content.to_string()));
        self.editor.update(cx, |editor, cx| {
            editor.set_value(content.to_string(), window, cx)
        });
    }

    /// Queue a non-user content replacement for the next render, when a Window
    /// is available to synchronize IME and selection state.
    pub fn set_content(&mut self, content: &str, cx: &mut Context<Self>) {
        let content = SharedString::from(content.to_string());
        self.silent_content = Some(content.clone());
        self.pending_content = Some(content);
        cx.notify();
    }

    /// Replace the selection and select a range in the resulting document.
    pub fn apply_edit(
        &mut self,
        replacement: String,
        selected_after: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editor.update(cx, |editor, cx| {
            editor.replace(replacement, window, cx);
            editor.set_selected_range(selected_after, cx);
            editor.focus(window, cx);
        });
    }

    pub fn undo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editor.update(cx, |editor, cx| {
            editor.focus(window, cx);
            window.dispatch_action(Undo.boxed_clone(), cx);
        });
    }

    pub fn redo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editor.update(cx, |editor, cx| {
            editor.focus(window, cx);
            window.dispatch_action(Redo.boxed_clone(), cx);
        });
    }

    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.editor
            .update(cx, |editor, cx| editor.focus(window, cx));
    }
}

impl EventEmitter<NyaDocumentEditorEvent> for NyaDocumentEditorState {}

impl Focusable for NyaDocumentEditorState {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.editor.read(cx).focus_handle(cx)
    }
}

impl Render for NyaDocumentEditorState {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _keep_subscription_alive = &self.subscription;
        let _keep_observation_alive = &self.observation;
        if let Some(content) = self.pending_content.take() {
            self.editor
                .update(cx, |editor, cx| editor.set_value(content, window, cx));
        }
        let mut editor = Editor::new(&self.editor)
            .appearance(false)
            .bordered(false)
            .readonly(self.read_only)
            .size_full();
        if let Some(font_size) = self.font_size {
            editor = editor.text_size(px(font_size));
        }
        if let Some(font_family) = &self.font_family {
            editor = editor.font_family(font_family.clone());
        }
        div().size_full().min_h_0().min_w_0().child(editor)
    }
}

#[derive(IntoElement)]
pub struct NyaDocumentEditor {
    state: Entity<NyaDocumentEditorState>,
}

impl NyaDocumentEditor {
    pub fn new(state: &Entity<NyaDocumentEditorState>) -> Self {
        Self {
            state: state.clone(),
        }
    }
}

impl gpui::RenderOnce for NyaDocumentEditor {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div().size_full().min_h_0().min_w_0().child(self.state)
    }
}
