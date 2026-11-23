use gpui::{App, Entity, IntoElement, ScrollEvent, Window, div, prelude::*, px};
use nyaterm_ui::{NyaDocumentEditor, NyaDocumentEditorState};
pub fn editor_surface(
    document: &Entity<NyaDocumentEditorState>,
    font: gpui::Font,
    font_size: f32,
    on_activity: std::rc::Rc<dyn Fn(&mut App)>,
    on_save: impl Fn(&mut Window, &mut App) + 'static,
) -> gpui::AnyElement {
    div()
        .size_full()
        .min_h_0()
        .min_w_0()
        .font(font)
        .text_size(px(font_size))
        .capture_key_down({
            let activity = on_activity.clone();
            move |_, _, cx| activity(cx)
        })
        .on_key_down(move |event, window, cx| {
            let modifiers = event.keystroke.modifiers;
            if (modifiers.platform || modifiers.control) && event.keystroke.key == "s" {
                on_save(window, cx);
                cx.stop_propagation();
            }
        })
        .capture_any_mouse_down({
            let activity = on_activity.clone();
            move |_, _, cx| activity(cx)
        })
        .on_scroll(move |_: &ScrollEvent, _, cx| on_activity(cx))
        .child(NyaDocumentEditor::new(document))
        .into_any_element()
}
