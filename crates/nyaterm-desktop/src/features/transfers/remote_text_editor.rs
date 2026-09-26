use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, IntoElement, KeyDownEvent,
    Render, ScrollDelta, ScrollWheelEvent, Subscription, Window, div, prelude::*, px,
};
use nyaterm_ui::{NyaDocumentEditor, NyaDocumentEditorEvent, NyaDocumentEditorState};

use crate::features::{NyaTermApp, shell::gpui_code_font_family};
use crate::models::{TransferEditorField, TransferEditorState};

pub(in crate::features) struct RemoteTextEditor {
    app: Entity<NyaTermApp>,
    tab_id: String,
    document: Entity<NyaDocumentEditorState>,
    read_only: bool,
    _subscription: Subscription,
}

impl RemoteTextEditor {
    pub(in crate::features) fn new(
        app: Entity<NyaTermApp>,
        tab: &TransferEditorState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let document = cx.new(|cx| {
            NyaDocumentEditorState::new_source(
                window,
                cx,
                tab.content.clone(),
                language_for_path(&tab.remote_path),
            )
        });
        let tab_id = tab.id.clone();
        let app_for_events = app.clone();
        let subscription = cx.subscribe(
            &document,
            move |_, _, event: &NyaDocumentEditorEvent, cx| match event {
                NyaDocumentEditorEvent::Changed(content) => {
                    app_for_events.update(cx, |app, cx| {
                        app.transfer.sync_editor_content(&tab_id, content.clone());
                        app.mark_user_activity();
                        cx.notify();
                    });
                }
                NyaDocumentEditorEvent::Updated => {
                    app_for_events.update(cx, |app, cx| {
                        app.mark_user_activity();
                        cx.notify();
                    });
                }
                NyaDocumentEditorEvent::Blurred(_) => {}
            },
        );
        if tab.focused_field == TransferEditorField::Search {
            document.update(cx, |document, cx| document.open_search(false, cx));
        } else {
            document.update(cx, |document, cx| document.move_cursor_to_end(cx));
        }
        Self {
            app,
            tab_id: tab.id.clone(),
            document,
            read_only: tab.loading || tab.saving,
            _subscription: subscription,
        }
    }

    pub(in crate::features) fn new_read_only(
        app: Entity<NyaTermApp>,
        id: String,
        content: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let document = cx.new(|cx| {
            NyaDocumentEditorState::new_source(window, cx, content, language_for_path(&id))
        });
        let subscription = cx.subscribe(&document, |_, _, _: &NyaDocumentEditorEvent, _| {});
        Self {
            app,
            tab_id: id,
            document,
            read_only: true,
            _subscription: subscription,
        }
    }

    pub(in crate::features) fn sync_read_only(
        &mut self,
        id: &str,
        content: &str,
        cx: &mut Context<Self>,
    ) {
        if self.tab_id != id || !self.document.read(cx).content_equals(content, cx) {
            self.tab_id = id.to_string();
            self.document
                .update(cx, |document, cx| document.set_content(content, cx));
        }
    }

    pub(in crate::features) fn sync_from_tab(
        &mut self,
        tab: &TransferEditorState,
        cx: &mut Context<Self>,
    ) {
        if !self.document.read(cx).content_equals(&tab.content, cx) {
            self.document
                .update(cx, |document, cx| document.set_content(&tab.content, cx));
        }
        let read_only = tab.loading || tab.saving;
        if self.read_only != read_only {
            self.read_only = read_only;
            cx.notify();
        }
    }

    pub(in crate::features) fn cursor_position(&self, cx: &App) -> (usize, usize) {
        self.document.read(cx).cursor_position(cx)
    }

    pub(in crate::features) fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.document.read(cx).focus_handle(cx)
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let modifiers = event.keystroke.modifiers;
        if (modifiers.platform || modifiers.control) && event.keystroke.key == "s" {
            self.app
                .update(cx, |app, cx| app.save_transfer_editor(false, window, cx));
            cx.stop_propagation();
        }
    }
}

impl Focusable for RemoteTextEditor {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.focus_handle(cx)
    }
}

impl Render for RemoteTextEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let font_size = self
            .app
            .read(cx)
            .settings
            .summary()
            .transfer_internal_editor_font_size
            .clamp(8, 72) as f32;
        self.document.update(cx, |document, cx| {
            document.set_read_only(self.read_only, cx);
            document.set_font_size(font_size, cx);
            document.set_font_family(gpui_code_font_family(), cx);
        });
        div()
            .size_full()
            .min_h_0()
            .min_w_0()
            .font_family(gpui_code_font_family())
            .text_size(px(font_size))
            .on_key_down(cx.listener(Self::on_key_down))
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _, cx| {
                if !event.modifiers.control && !event.modifiers.platform {
                    return;
                }
                let delta_y = match event.delta {
                    ScrollDelta::Pixels(delta) => f32::from(delta.y),
                    ScrollDelta::Lines(delta) => delta.y,
                };
                if delta_y != 0.0 {
                    let step = if delta_y < 0.0 { 1 } else { -1 };
                    this.app.update(cx, |app, cx| {
                        app.adjust_transfer_internal_editor_font_size(step, cx);
                    });
                    cx.stop_propagation();
                }
            }))
            .child(NyaDocumentEditor::new(&self.document))
    }
}

fn language_for_path(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "json" => "json",
        "yaml" | "yml" => "yaml",
        "toml" => "toml",
        "rs" => "rust",
        "py" => "python",
        "js" | "jsx" => "javascript",
        "ts" | "tsx" => "typescript",
        "sh" => "bash",
        "go" => "go",
        "c" | "h" => "c",
        "cpp" | "hpp" => "cpp",
        "css" => "css",
        "html" | "htm" => "html",
        "xml" | "svg" => "xml",
        "md" => "markdown",
        _ => "plain",
    }
}

#[cfg(test)]
mod tests {
    use super::language_for_path;

    #[test]
    fn remote_editor_chooses_supported_syntax_from_extension() {
        assert_eq!(language_for_path("/home/user/main.rs"), "rust");
        assert_eq!(language_for_path("/home/user/config.yaml"), "yaml");
        assert_eq!(language_for_path("/home/user/unknown.bin"), "plain");
    }
}
