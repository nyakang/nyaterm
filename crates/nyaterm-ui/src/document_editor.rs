use std::ops::Range;

use gpui::{
    Action as _, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable,
    IntoElement, Render, SharedString, Subscription, Window, div, prelude::*,
};
use gpui_kit::component::input::{Editor, EditorState, InputEvent, Redo, Undo};

use crate::input_focus::register_nya_input_focus;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NyaDocumentEditorEvent {
    Changed(String),
    Blurred(String),
}

/// Full-size native document editor used by modeless document windows.
///
/// This type is the stable NyaTerm boundary around gpui-kit's editor;
/// desktop features never need to import gpui-kit directly.
pub struct NyaDocumentEditorState {
    editor: Entity<EditorState>,
    subscription: Subscription,
    pending_content: Option<SharedString>,
    silent_content: Option<SharedString>,
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
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .default_value(content)
                .placeholder(placeholder)
                .soft_wrap(true)
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
        Self {
            editor,
            subscription,
            pending_content: None,
            silent_content: None,
        }
    }

    pub fn value(&self, cx: &App) -> String {
        self.editor.read(cx).value().to_string()
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
        if let Some(content) = self.pending_content.take() {
            self.editor
                .update(cx, |editor, cx| editor.set_value(content, window, cx));
        }
        div().size_full().min_h_0().min_w_0().child(
            Editor::new(&self.editor)
                .appearance(false)
                .bordered(false)
                .size_full(),
        )
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
