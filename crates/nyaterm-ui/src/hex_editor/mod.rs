//! A reusable byte editor with one document, selection and virtualized viewport.

mod view;

use gpui::{
    App, ClipboardItem, Context, EventEmitter, FocusHandle, Focusable, KeyDownEvent,
    UniformListScrollHandle,
};
use nyaterm_core::hex_document::{
    HexCopyFormat, HexCursor, HexDocument, HexNibble, HexPasteError, format_hex, parse_hex_paste,
};

pub use view::NyaHexEditor;

pub enum NyaHexEditorEvent {
    Changed,
    SelectionChanged,
}

pub struct NyaHexEditorState {
    document: HexDocument,
    focus: FocusHandle,
    scroll: UniformListScrollHandle,
    undo: Vec<HexDocument>,
    redo: Vec<HexDocument>,
    error: Option<HexPasteError>,
    pub(super) width: f32,
    dragging: Option<usize>,
    bytes_per_row: usize,
}

impl NyaHexEditorState {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            document: HexDocument::default(),
            focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            error: None,
            width: 0.,
            dragging: None,
            bytes_per_row: 8,
        }
    }

    pub fn document(&self) -> &HexDocument {
        &self.document
    }
    pub fn error(&self) -> Option<HexPasteError> {
        self.error
    }

    fn edit(&mut self, edit: impl FnOnce(&mut HexDocument), cx: &mut Context<Self>) {
        let before = self.document.clone();
        edit(&mut self.document);
        if before == self.document {
            if self.error.take().is_some() {
                cx.emit(NyaHexEditorEvent::SelectionChanged);
                cx.notify();
            }
            return;
        }
        // Bound history; snapshots include nibble-in-progress and selection.
        if self.undo.len() == 100 {
            self.undo.remove(0);
        }
        self.undo.push(before);
        self.redo.clear();
        self.error = None;
        self.reveal_cursor();
        cx.emit(NyaHexEditorEvent::Changed);
        cx.notify();
    }

    /// Seed an independent editor without inheriting another editor's undo history.
    pub fn replace_document(&mut self, document: HexDocument, cx: &mut Context<Self>) {
        self.document = document;
        self.undo.clear();
        self.redo.clear();
        self.error = None;
        self.dragging = None;
        self.reveal_cursor();
        cx.emit(NyaHexEditorEvent::Changed);
        cx.notify();
    }

    pub fn set_bytes(&mut self, bytes: Vec<u8>, cx: &mut Context<Self>) {
        self.edit(|doc| doc.set_bytes(bytes), cx);
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.set_bytes(Vec::new(), cx);
    }

    pub fn insert_bytes(&mut self, bytes: &[u8], cx: &mut Context<Self>) {
        self.edit(|doc| doc.insert_bytes(bytes), cx);
    }

    pub fn paste(&mut self, text: &str, cx: &mut Context<Self>) {
        match parse_hex_paste(text) {
            Ok(bytes) => self.edit(|doc| doc.insert_bytes(&bytes), cx),
            Err(error) => {
                self.error = Some(error);
                cx.emit(NyaHexEditorEvent::SelectionChanged);
                cx.notify();
            }
        }
    }

    pub fn copy(&self, format: HexCopyFormat, cx: &mut App) {
        cx.write_to_clipboard(ClipboardItem::new_string(format_hex(
            self.document.selected_bytes(),
            format,
        )));
    }

    pub fn select_all(&mut self, cx: &mut Context<Self>) {
        self.document.select_all();
        cx.emit(NyaHexEditorEvent::SelectionChanged);
        cx.notify();
    }

    pub fn cancel_selection(&mut self, cx: &mut Context<Self>) {
        self.document.cancel_selection();
        self.dragging = None;
        self.error = None;
        cx.emit(NyaHexEditorEvent::SelectionChanged);
        cx.notify();
    }

    fn reveal_cursor(&self) {
        self.scroll.scroll_to_item(
            self.document.cursor().byte_index / self.bytes_per_row,
            gpui::ScrollStrategy::Top,
        );
    }

    fn drag_to(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(anchor) = self.dragging else {
            return;
        };
        let anchor_edge = if index < anchor { anchor + 1 } else { anchor };
        self.document.move_cursor(
            HexCursor {
                byte_index: anchor_edge,
                nibble: HexNibble::High,
            },
            false,
        );
        let end = if index >= anchor {
            (index + 1).min(self.document.bytes().len())
        } else {
            index
        };
        self.move_cursor(
            HexCursor {
                byte_index: end,
                nibble: HexNibble::High,
            },
            true,
            cx,
        );
    }

    fn move_cursor(&mut self, cursor: HexCursor, extend: bool, cx: &mut Context<Self>) {
        self.document.move_cursor(cursor, extend);
        self.reveal_cursor();
        cx.emit(NyaHexEditorEvent::SelectionChanged);
        cx.notify();
    }

    fn key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        let key = &event.keystroke;
        if key.modifiers.alt || key.modifiers.function {
            return false;
        }
        let accel = key.modifiers.control || key.modifiers.platform;
        if accel {
            match key.key.as_str() {
                "a" => self.select_all(cx),
                "c" => self.copy(HexCopyFormat::Hex, cx),
                "x" => {
                    self.copy(HexCopyFormat::Hex, cx);
                    self.edit(
                        |doc| {
                            if doc.selection().is_none() {
                                doc.select_all();
                            }
                            doc.delete(false);
                        },
                        cx,
                    );
                }
                "v" => {
                    if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                        self.paste(&text, cx);
                    }
                }
                "z" | "y" => {
                    let redo = key.key == "y" || key.modifiers.shift;
                    let (source, target) = if redo {
                        (&mut self.redo, &mut self.undo)
                    } else {
                        (&mut self.undo, &mut self.redo)
                    };
                    if let Some(doc) = source.pop() {
                        target.push(std::mem::replace(&mut self.document, doc));
                        self.error = None;
                        self.reveal_cursor();
                        cx.emit(NyaHexEditorEvent::Changed);
                        cx.notify();
                    }
                }
                _ => return false,
            }
            return true;
        }
        let cursor = self.document.cursor();
        let length = self.document.bytes().len();
        let mut next = cursor;
        match key.key.as_str() {
            "backspace" | "delete" => self.edit(|doc| doc.delete(key.key == "backspace"), cx),
            "left" => {
                if key.modifiers.shift {
                    next.byte_index = next.byte_index.saturating_sub(1);
                    next.nibble = HexNibble::High;
                } else if next.nibble == HexNibble::Low {
                    next.nibble = HexNibble::High;
                } else if next.byte_index > 0 {
                    next.byte_index -= 1;
                    next.nibble = HexNibble::Low;
                }
                self.move_cursor(next, key.modifiers.shift, cx);
            }
            "right" => {
                if key.modifiers.shift || next.nibble == HexNibble::Low {
                    next.byte_index = (next.byte_index + 1).min(length);
                    next.nibble = HexNibble::High;
                } else {
                    next.nibble = HexNibble::Low;
                }
                self.move_cursor(next, key.modifiers.shift, cx);
            }
            "up" | "down" | "home" | "end" => {
                next.byte_index = match key.key.as_str() {
                    "up" => next.byte_index.saturating_sub(self.bytes_per_row),
                    "down" => (next.byte_index + self.bytes_per_row).min(length),
                    "home" => next.byte_index / self.bytes_per_row * self.bytes_per_row,
                    _ => ((next.byte_index / self.bytes_per_row + 1) * self.bytes_per_row)
                        .min(length),
                };
                next.nibble = HexNibble::High;
                self.move_cursor(next, key.modifiers.shift, cx);
            }
            "enter" | "space" => {}
            _ => {
                let Some(digit) = key
                    .key_char
                    .as_deref()
                    .unwrap_or(&key.key)
                    .chars()
                    .next()
                    .filter(|_| key.key_char.as_deref().unwrap_or(&key.key).len() == 1)
                    .and_then(|c| c.to_digit(16))
                else {
                    return false;
                };
                self.edit(|doc| doc.type_nibble(digit as u8), cx);
            }
        }
        true
    }
}

impl EventEmitter<NyaHexEditorEvent> for NyaHexEditorState {}
impl Focusable for NyaHexEditorState {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::NyaHexEditorState;
    use gpui::{
        AppContext as _, ClipboardItem, Context, Entity, Focusable as _, IntoElement, Render,
        TestAppContext, Window, div, prelude::*, px,
    };

    struct EditorFixture {
        state: Entity<NyaHexEditorState>,
        width: f32,
    }

    impl Render for EditorFixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .w(px(self.width))
                .h(px(180.))
                .flex()
                .flex_col()
                .child(super::NyaHexEditor::new(
                    &self.state,
                    crate::theme_palette("github-dark"),
                ))
        }
    }

    #[gpui::test]
    fn copied_document_keeps_pending_nibble_and_has_independent_empty_history(
        cx: &mut TestAppContext,
    ) {
        let source = cx.new(NyaHexEditorState::new);
        let copy = cx.new(NyaHexEditorState::new);
        source.update(cx, |state, cx| {
            state.edit(
                |doc| {
                    doc.set_bytes(vec![1]);
                    doc.type_nibble(10);
                },
                cx,
            );
        });
        let document = source.read_with(cx, |state, _| state.document().clone());
        copy.update(cx, |state, cx| {
            state.replace_document(document.clone(), cx);
            assert!(state.document() == &document);
            assert!(state.undo.is_empty());
            assert!(state.redo.is_empty());
            state.clear(cx);
        });
        source.read_with(cx, |state, _| assert!(state.document() == &document));
    }

    #[gpui::test]
    fn keyboard_editing_copy_cut_undo_redo_and_resize_preserve_bytes(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (fixture, cx) = cx.add_window_view(|_, cx| {
            let state = cx.new(NyaHexEditorState::new);
            cx.observe(&state, |_, _, cx| cx.notify()).detach();
            EditorFixture {
                state,
                width: 1000.,
            }
        });
        let state = fixture.read_with(cx, |fixture, _| fixture.state.clone());
        cx.update(|window, cx| {
            _ = window.draw(cx);
            window.focus(&state.read(cx).focus_handle(cx), cx);
        });
        cx.simulate_keystrokes("4");
        state.read_with(cx, |state, _| {
            assert!(state.document().bytes().is_empty());
            assert_eq!(state.document().pending_nibble(), Some(4));
        });
        cx.simulate_keystrokes("8 6 9 shift-left");
        cx.simulate_keystrokes(if cfg!(target_os = "macos") {
            "cmd-c"
        } else {
            "ctrl-c"
        });
        cx.update(|_, cx| {
            assert_eq!(
                cx.read_from_clipboard()
                    .and_then(|item| item.text())
                    .as_deref(),
                Some("69")
            )
        });
        cx.simulate_keystrokes(if cfg!(target_os = "macos") {
            "cmd-x"
        } else {
            "ctrl-x"
        });
        state.read_with(cx, |state, _| assert_eq!(state.document().bytes(), b"H"));
        cx.simulate_keystrokes(if cfg!(target_os = "macos") {
            "cmd-z"
        } else {
            "ctrl-z"
        });
        state.read_with(cx, |state, _| assert_eq!(state.document().bytes(), b"Hi"));
        cx.simulate_keystrokes(if cfg!(target_os = "macos") {
            "cmd-shift-z"
        } else {
            "ctrl-shift-z"
        });
        state.read_with(cx, |state, _| assert_eq!(state.document().bytes(), b"H"));
        cx.update(|_, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string("0x65,0x79".to_string()))
        });
        cx.simulate_keystrokes(if cfg!(target_os = "macos") {
            "cmd-v"
        } else {
            "ctrl-v"
        });
        state.read_with(cx, |state, _| assert_eq!(state.document().bytes(), b"Hey"));
        for (width, expected) in [(1000., 16), (650., 12), (420., 8)] {
            fixture.update(cx, |fixture, cx| {
                fixture.width = width;
                cx.notify();
            });
            cx.run_until_parked();
            cx.update(|window, cx| {
                _ = window.draw(cx);
            });
            cx.run_until_parked();
            state.read_with(cx, |state, _| {
                assert_eq!(state.document().bytes(), b"Hey");
                assert_eq!(state.bytes_per_row, expected);
            });
        }
    }

    #[test]
    fn dragging_selects_both_endpoint_bytes_in_either_direction() {
        let mut cx = TestAppContext::single();
        let state = cx.new(NyaHexEditorState::new);
        state.update(&mut cx, |state, cx| {
            state.set_bytes(b"Hello".to_vec(), cx);
            state.dragging = Some(1);
            state.drag_to(3, cx);
            assert_eq!(state.document().selected_bytes(), b"ell");
            state.dragging = Some(3);
            state.drag_to(1, cx);
            assert_eq!(state.document().selected_bytes(), b"ell");
        });
    }

    #[test]
    fn invalid_paste_preserves_document_cursor_and_selection() {
        let mut cx = TestAppContext::single();
        let state = cx.new(NyaHexEditorState::new);
        state.update(&mut cx, |state, cx| {
            state.paste("48 65", cx);
            state.select_all(cx);
            let before = state.document.clone();
            state.paste("Hello", cx);
            assert!(state.document == before);
            assert!(state.error().is_some());
            state.paste("\\xFF", cx);
            assert_eq!(state.document().bytes(), &[255]);
            assert!(state.error().is_none());
        });
    }
}
