use std::collections::HashSet;
use std::ops::Range;

use gpui::{
    App, Bounds, Context, ElementInputHandler, EntityInputHandler, FocusHandle, Focusable,
    IntoElement, KeyDownEvent, Pixels, Point, Render, Subscription, UTF16Selection, WeakEntity,
    Window, canvas, div, prelude::*, px, size,
};

use crate::features::NyaTermApp;

/// Local composition state. No committed remote text is retained here.
pub(super) struct RemoteDesktopInput {
    app: WeakEntity<NyaTermApp>,
    session_id: String,
    focus: FocusHandle,
    marked: String,
    selection: Range<usize>,
    bounds: Option<Bounds<Pixels>>,
    suppressed_keys: HashSet<String>,
    blur: Option<Subscription>,
}

impl RemoteDesktopInput {
    pub(super) fn new(app: WeakEntity<NyaTermApp>, session_id: String, focus: FocusHandle) -> Self {
        Self {
            app,
            session_id,
            focus,
            marked: String::new(),
            selection: 0..0,
            bounds: None,
            suppressed_keys: HashSet::new(),
            blur: None,
        }
    }

    pub(super) fn clear(&mut self, cx: &mut Context<Self>) {
        self.marked.clear();
        self.selection = 0..0;
        self.suppressed_keys.clear();
        cx.notify();
    }

    pub(super) fn consumes_key(&mut self, event: &KeyDownEvent) -> bool {
        let key = &event.keystroke;
        let text = key
            .key_char
            .as_deref()
            .is_some_and(|text| !text.is_empty() && !text.chars().any(char::is_control));
        let consumed = !self.marked.is_empty()
            || (text && !key.modifiers.control && !key.modifiers.alt && !key.modifiers.platform);
        if consumed {
            self.suppressed_keys.insert(key.key.clone());
        }
        consumed
    }

    pub(super) fn consumes_key_up(&mut self, key: &str) -> bool {
        self.suppressed_keys.remove(key)
    }
}

impl Focusable for RemoteDesktopInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for RemoteDesktopInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.blur.is_none() {
            self.blur = Some(cx.on_focus_out(&self.focus, window, |this, _, _, cx| this.clear(cx)));
        }
        let entity = cx.entity();
        let paint_entity = entity.clone();
        let focus = self.focus.clone();
        div()
            .absolute()
            .inset_0()
            .size_full()
            .child(
                canvas(
                    move |bounds, _, cx| {
                        entity.update(cx, |this, _| this.bounds = Some(bounds));
                    },
                    move |bounds, _, window, cx| {
                        if focus.is_focused(window) {
                            window.handle_input(
                                &focus,
                                ElementInputHandler::new(bounds, paint_entity),
                                cx,
                            );
                        }
                    },
                )
                .size_full(),
            )
            .when(!self.marked.is_empty(), |this| {
                this.child(
                    div()
                        .absolute()
                        .bottom_0()
                        .left_0()
                        .px_2()
                        .bg(gpui::rgb(0x202020))
                        .text_color(gpui::rgb(0xffffff))
                        .child(self.marked.clone()),
                )
            })
    }
}

impl EntityInputHandler for RemoteDesktopInput {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let byte_range = utf16_byte_range(&self.marked, range);
        *adjusted = Some(
            self.marked[..byte_range.start].encode_utf16().count()
                ..self.marked[..byte_range.end].encode_utf16().count(),
        );
        Some(self.marked[byte_range].to_string())
    }
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.selection.clone(),
            reversed: false,
        })
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        (!self.marked.is_empty()).then(|| 0..self.marked.encode_utf16().count())
    }
    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.clear(cx);
    }
    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked.clear();
        self.selection = 0..0;
        if !text.is_empty() {
            let app = self.app.clone();
            let session_id = self.session_id.clone();
            let text = text.to_owned();
            cx.defer(move |cx| {
                let _ = app.update(cx, |app, _| {
                    if app.session.active_id() == Some(session_id.as_str()) {
                        app.send_remote_committed_text(&session_id, &text);
                        app.mark_user_activity();
                    }
                });
            });
        }
        cx.notify();
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        replacement: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = replacement
            .map(|range| utf16_byte_range(&self.marked, range))
            .unwrap_or(0..self.marked.len());
        let prefix_len = self.marked[..range.start].encode_utf16().count();
        self.marked.replace_range(range, text);
        let len = text.encode_utf16().count();
        self.selection = selected
            .map(|range| {
                let start = range.start.min(len);
                prefix_len + start..prefix_len + range.end.min(len).max(start)
            })
            .unwrap_or(prefix_len + len..prefix_len + len);
        cx.notify();
    }
    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let bounds = self.bounds.unwrap_or(bounds);
        Some(Bounds::new(bounds.origin, size(px(2.), px(20.))))
    }
    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.selection.end)
    }
}

fn utf16_byte_range(text: &str, range: Range<usize>) -> Range<usize> {
    let offset = |wanted: usize| {
        let mut units = 0;
        for (byte, ch) in text.char_indices() {
            if units + ch.len_utf16() > wanted {
                return byte;
            }
            units += ch.len_utf16();
        }
        text.len()
    };
    let start = offset(range.start);
    start..offset(range.end).max(start)
}

#[cfg(test)]
mod tests {
    use super::utf16_byte_range;

    #[test]
    fn ime_ranges_do_not_split_surrogate_pairs_or_reverse() {
        assert_eq!(utf16_byte_range("a😀中", 1..3), 1..5);
        assert_eq!(utf16_byte_range("a😀中", 2..3), 1..5);
        assert_eq!(
            utf16_byte_range("a😀中", std::ops::Range { start: 4, end: 1 }),
            8..8
        );
        assert_eq!(utf16_byte_range("a😀中", 99..100), 8..8);
    }
}
