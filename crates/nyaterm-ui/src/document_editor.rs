use std::ops::Range;

use gpui::{
    Action as _, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, Font,
    IntoElement, Render, SharedString, Subscription, Window, div, prelude::*, px,
};
use gpui_kit::component::input::{Editor, EditorState, InputEvent, Redo, Undo};

use crate::document_snapshot::NyaDocumentSnapshot;
use crate::input_focus::{preserve_nya_input_focus_on_pointer_down, register_nya_input_focus};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NyaDocumentEditorEvent {
    Changed(NyaDocumentSnapshot),
    Blurred(NyaDocumentSnapshot),
    /// Cursor position changed; visual-only notifications do not emit this.
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
    observed_cursor: usize,
    pending_content: Option<SharedString>,
    silent_content: Option<SharedString>,
    read_only: bool,
    font_size: Option<f32>,
    font: Option<Font>,
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
        Self::new_source_with_placeholder(window, cx, content, language, SharedString::default())
    }

    pub fn new_source_with_placeholder(
        window: &mut Window,
        cx: &mut Context<Self>,
        content: impl Into<SharedString>,
        language: impl Into<SharedString>,
        placeholder: impl Into<SharedString>,
    ) -> Self {
        Self::build(
            window,
            cx,
            content.into(),
            placeholder.into(),
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
        let subscription = cx.subscribe(
            &editor,
            |this, editor, event: &InputEvent, cx| match event {
                InputEvent::Change => {
                    if this
                        .silent_content
                        .take()
                        .is_some_and(|expected| editor.read(cx).text() == expected.as_ref())
                    {
                        return;
                    }
                    cx.emit(NyaDocumentEditorEvent::Changed(
                        NyaDocumentSnapshot::from_rope(editor.read(cx).text()),
                    ));
                }
                InputEvent::Blur => cx.emit(NyaDocumentEditorEvent::Blurred(
                    NyaDocumentSnapshot::from_rope(editor.read(cx).text()),
                )),
                InputEvent::Focus | InputEvent::PressEnter { .. } => {}
            },
        );
        let observed_cursor = editor.read(cx).cursor();
        let observation = cx.observe(&editor, |this, editor, cx| {
            let cursor = editor.read(cx).cursor();
            if this.observed_cursor != cursor {
                this.observed_cursor = cursor;
                cx.emit(NyaDocumentEditorEvent::Updated);
            }
            cx.notify();
        });
        Self {
            editor,
            subscription,
            observation,
            observed_cursor,
            pending_content: None,
            silent_content: None,
            read_only: false,
            font_size: None,
            font: None,
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

    pub fn set_font(&mut self, font: Font, cx: &mut Context<Self>) {
        if self.font.as_ref() != Some(&font) {
            self.font = Some(font);
            cx.notify();
        }
    }

    pub fn cursor_position(&self, cx: &App) -> (usize, usize) {
        let value = self.editor.read(cx).text();
        let cursor = self.editor.read(cx).cursor().min(value.len());
        let line = value.byte_to_line_idx(cursor, ropey::LineType::LF);
        let line_start = value.line_to_byte_idx(line, ropey::LineType::LF);
        (
            line + 1,
            value.slice(line_start..cursor).chars().count() + 1,
        )
    }

    pub fn open_search(&mut self, replace: bool, cx: &mut Context<Self>) {
        self.editor
            .update(cx, |editor, cx| editor.open_search(replace, cx));
    }

    pub fn value(&self, cx: &App) -> String {
        self.editor.read(cx).text().to_string()
    }

    pub fn content_equals(&self, content: &str, cx: &App) -> bool {
        if let Some(pending) = &self.pending_content {
            return pending.as_ref() == content;
        }
        self.editor.read(cx).text() == content
    }

    pub fn snapshot_equals(&self, content: &NyaDocumentSnapshot, cx: &App) -> bool {
        if let Some(pending) = &self.pending_content {
            return content == &pending.as_ref();
        }
        self.editor.read(cx).text() == content.text()
    }

    pub fn selected_range(&self, cx: &App) -> Range<usize> {
        self.editor.read(cx).selected_range()
    }

    pub fn move_cursor_to_end(&mut self, cx: &mut Context<Self>) {
        let end = self.editor.read(cx).text().len();
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
        if let Some(font) = &self.font {
            editor = editor.font(font.clone());
        }
        div()
            .size_full()
            .min_h_0()
            .min_w_0()
            .capture_any_mouse_down(|_, _, cx| {
                preserve_nya_input_focus_on_pointer_down(cx);
            })
            .child(editor)
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

#[cfg(test)]
mod tests {
    use super::{NyaDocumentEditorEvent, NyaDocumentEditorState};
    use crate::document_snapshot::NyaDocumentSnapshot;
    use crate::root::nya_root;
    use gpui::{AppContext as _, Focusable as _, Modifiers, MouseButton, TestAppContext};

    #[gpui::test]
    fn source_placeholder_preserves_bash_language_and_unicode_content(cx: &mut TestAppContext) {
        use gpui_kit::component::highlighter::{HighlightTheme, SyntaxHighlighter};

        cx.update(gpui_kit::init);
        let script = "if [ -n \"$HOME\" ]; then\n  echo \"你好 $HOME\" # 注释\nfi";
        let (document, cx) = cx.add_window_view(|window, cx| {
            NyaDocumentEditorState::new_source_with_placeholder(
                window,
                cx,
                script,
                "bash",
                "输入命令",
            )
        });
        cx.update(|window, cx| {
            _ = window.draw(cx);
            let editor = document.read(cx).editor.read(cx);
            assert_eq!(editor.language_name().as_ref(), "bash");
            assert_eq!(editor.presentation().placeholder().as_ref(), "输入命令");
            assert_eq!(editor.text().to_string(), script);
            let mut highlighter = SyntaxHighlighter::new(&editor.language_name());
            highlighter.update(None, editor.text(), None);
            for theme in [
                HighlightTheme::default_dark(),
                HighlightTheme::default_light(),
            ] {
                let styles = highlighter.styles(&(0..script.len()), theme.as_ref());
                assert!(!styles.is_empty());
                for (range, _) in styles {
                    assert!(script.is_char_boundary(range.start));
                    assert!(script.is_char_boundary(range.end));
                }
            }
        });
    }

    #[gpui::test]
    fn document_wheel_scrolls_with_or_without_modifiers_and_clamps_at_both_edges(
        cx: &mut TestAppContext,
    ) {
        use gpui::{ScrollDelta, ScrollWheelEvent, TouchPhase, point, px};

        cx.update(gpui_kit::init);
        let mut document = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let editor = cx.new(|cx| {
                NyaDocumentEditorState::new_source(window, cx, "line\n".repeat(500), "plain")
            });
            document = Some(editor.clone());
            nya_root(editor, window, cx)
        });
        let document = document.unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| _ = window.draw(cx));
        let position = cx.update(|_, cx| {
            document
                .read(cx)
                .editor
                .read(cx)
                .range_to_bounds(&(0..0))
                .unwrap()
                .center()
        });
        for modifiers in [
            Modifiers::default(),
            Modifiers {
                control: true,
                ..Modifiers::default()
            },
            Modifiers {
                platform: true,
                ..Modifiers::default()
            },
        ] {
            let mut last_y = px(0.);
            for (delta_y, movement) in [
                (100_000., 0),
                (-100., -1),
                (-100., -1),
                (-100_000., -1),
                (-100., 0),
                (100., 1),
                (100_000., 1),
                (100., 0),
            ] {
                cx.simulate_event(ScrollWheelEvent {
                    position,
                    delta: ScrollDelta::Pixels(point(px(0.), px(delta_y))),
                    modifiers,
                    touch_phase: TouchPhase::Moved,
                });
                cx.run_until_parked();
                cx.update(|window, cx| _ = window.draw(cx));
                let y = cx.update(|_, cx| document.read(cx).editor.read(cx).scroll_offset().y);
                match movement {
                    -1 => assert!(y < last_y),
                    1 => assert!(y > last_y),
                    _ => assert_eq!(y, last_y),
                }
                last_y = y;
            }
        }
    }

    #[gpui::test]
    fn clicking_another_line_moves_the_document_cursor(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let mut document = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let editor = cx.new(|cx| {
                NyaDocumentEditorState::new_source(
                    window,
                    cx,
                    "abcdefghij\nklmnopqrst\nuvwxyz",
                    "plain",
                )
            });
            document = Some(editor.clone());
            nya_root(editor, window, cx)
        });
        let document = document.unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| _ = window.draw(cx));
        let positions = cx.update(|_, cx| {
            let editor = document.read(cx).editor.read(cx);
            [2, 13].map(|offset| editor.range_to_bounds(&(offset..offset)).unwrap().center())
        });
        for (position, offset) in positions.into_iter().zip([2, 13]) {
            cx.simulate_click(position, Modifiers::default());
            cx.run_until_parked();
            cx.update(|window, cx| {
                _ = window.draw(cx);
                assert_eq!(document.read(cx).selected_range(cx), offset..offset);
                assert!(document.read(cx).focus_handle(cx).is_focused(window));
            });
        }
    }

    #[gpui::test]
    fn dragging_on_another_line_starts_a_new_document_selection(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let mut document = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let editor = cx.new(|cx| {
                NyaDocumentEditorState::new_source(
                    window,
                    cx,
                    "abcdefghij\nklmnopqrst\nuvwxyz",
                    "plain",
                )
            });
            document = Some(editor.clone());
            nya_root(editor, window, cx)
        });
        let document = document.unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| _ = window.draw(cx));
        let [first, start, end] = cx.update(|_, cx| {
            let editor = document.read(cx).editor.read(cx);
            [2, 13, 17].map(|offset| editor.range_to_bounds(&(offset..offset)).unwrap().center())
        });
        cx.simulate_click(first, Modifiers::default());
        cx.run_until_parked();
        cx.update(|window, cx| _ = window.draw(cx));
        cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        cx.run_until_parked();
        cx.update(|window, cx| _ = window.draw(cx));
        cx.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        cx.run_until_parked();
        cx.update(|_, cx| assert_eq!(document.read(cx).selected_range(cx), 13..17));
    }

    #[gpui::test]
    fn queued_replacement_is_compared_before_the_next_render(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (document, window_cx) =
            cx.add_window_view(|window, cx| NyaDocumentEditorState::new(window, cx, "original"));
        window_cx.update(|_, cx| {
            document.update(cx, |document, cx| {
                document.set_content("reload", cx);
                assert!(document.content_equals("reload", cx));
                assert!(!document.snapshot_equals(&NyaDocumentSnapshot::from("original"), cx));
                document.set_content("original", cx);
            });
        });
        window_cx.run_until_parked();
        window_cx.update(|_, cx| assert_eq!(document.read(cx).value(cx), "original"));
    }

    #[gpui::test]
    fn document_snapshots_preserve_undo_and_utf8_cursor_positions(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (document, window_cx) =
            cx.add_window_view(|window, cx| NyaDocumentEditorState::new(window, cx, "中\r\n🙂"));
        window_cx.update(|window, cx| {
            document.update(cx, |document, cx| {
                document.move_cursor_to_end(cx);
                document.apply_edit("猫".to_string(), 12..12, window, cx);
                assert_eq!(document.cursor_position(cx), (2, 3));
                let snapshot = NyaDocumentSnapshot::from_rope(document.editor.read(cx).text());
                assert!(document.snapshot_equals(&snapshot, cx));
                document.undo(window, cx);
            });
        });
        window_cx.run_until_parked();
        window_cx.update(|_, cx| assert_eq!(document.read(cx).value(cx), "中\r\n🙂"));
        window_cx
            .update(|window, cx| document.update(cx, |document, cx| document.redo(window, cx)));
        window_cx.run_until_parked();
        window_cx.update(|_, cx| assert_eq!(document.read(cx).value(cx), "中\r\n🙂猫"));
    }

    #[gpui::test]
    fn visual_editor_notifications_do_not_emit_document_activity(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (document, window_cx) =
            cx.add_window_view(|window, cx| NyaDocumentEditorState::new(window, cx, "hello"));
        let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        window_cx.update(|_, cx| {
            document.update(cx, |_, cx| {
                let collected = events.clone();
                cx.subscribe(&document, move |_, _, event: &NyaDocumentEditorEvent, _| {
                    collected.lock().unwrap().push(event.clone());
                })
                .detach();
            });
        });
        window_cx.update(|_, cx| {
            document
                .read(cx)
                .editor
                .clone()
                .update(cx, |_, cx| cx.notify());
        });
        window_cx.run_until_parked();
        assert!(events.lock().unwrap().is_empty());
        window_cx.update(|window, cx| {
            document.read(cx).editor.clone().update(cx, |editor, cx| {
                editor.set_value("edited", window, cx);
                cx.emit(gpui_kit::component::input::InputEvent::Change);
            });
        });
        window_cx.run_until_parked();
        assert!(
            events
                .lock()
                .unwrap()
                .contains(&NyaDocumentEditorEvent::Changed("edited".into()))
        );
        events.lock().unwrap().clear();
        window_cx.update(|_, cx| {
            document.read(cx).editor.clone().update(cx, |editor, cx| {
                editor.set_selected_range(1..1, cx);
            });
        });
        window_cx.run_until_parked();
        assert_eq!(
            *events.lock().unwrap(),
            vec![NyaDocumentEditorEvent::Updated]
        );
    }
}
