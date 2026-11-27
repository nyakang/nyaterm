use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, IntoElement, Render,
    Subscription, Window, div,
};
use nyaterm_ui::document_snapshot::NyaDocumentSnapshot;
use nyaterm_ui::{NyaDocumentEditorEvent, NyaDocumentEditorState};

use crate::features::NyaTermApp;
use crate::models::{TransferEditorField, TransferEditorState};

pub(in crate::features) struct RemoteTextEditor {
    app: gpui::WeakEntity<NyaTermApp>,
    tab_id: String,
    document: Entity<NyaDocumentEditorState>,
    synced_content: NyaDocumentSnapshot,
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
                tab.content.to_string(),
                language_for_path(&tab.remote_path),
            )
        });
        let tab_id = tab.id.clone();
        let app_for_events = app.downgrade();
        let subscription = cx.subscribe(
            &document,
            move |this, _, event: &NyaDocumentEditorEvent, cx| match event {
                NyaDocumentEditorEvent::Changed(content) => {
                    this.synced_content = content.clone();
                    let _ = app_for_events.update(cx, |app, cx| {
                        app.transfer.sync_editor_content(&tab_id, content.clone());
                        app.mark_user_activity();
                        cx.notify();
                    });
                }
                NyaDocumentEditorEvent::Updated => cx.notify(),
                NyaDocumentEditorEvent::Blurred(_) => {}
            },
        );
        if tab.focused_field == TransferEditorField::Search {
            document.update(cx, |document, cx| document.open_search(false, cx));
        } else {
            document.update(cx, |document, cx| document.move_cursor_to_end(cx));
        }
        Self {
            app: app.downgrade(),
            tab_id: tab.id.clone(),
            document,
            synced_content: tab.content.clone(),
            read_only: tab.loading || tab.saving || tab.revision.is_none(),
            _subscription: subscription,
        }
    }

    pub(in crate::features) fn new_read_only(
        app: Entity<NyaTermApp>,
        id: String,
        path: &str,
        content: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let document = cx.new(|cx| {
            NyaDocumentEditorState::new_source(window, cx, content, language_for_path(path))
        });
        let subscription = cx.subscribe(&document, |_, _, _: &NyaDocumentEditorEvent, _| {});
        Self {
            app: app.downgrade(),
            tab_id: id,
            document,
            synced_content: Default::default(),
            read_only: true,
            _subscription: subscription,
        }
    }

    pub(in crate::features) fn sync_read_only(
        &mut self,
        id: &str,
        path: &str,
        content: &str,
        cx: &mut Context<Self>,
    ) {
        self.document.update(cx, |document, cx| {
            document.set_source_language(language_for_path(path), cx);
        });
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
        // User changes already carry this exact immutable snapshot. Only external
        // replacements need a text comparison, which also preserves undo on save.
        if !self.synced_content.same_version(&tab.content) {
            if !self.document.read(cx).snapshot_equals(&tab.content, cx) {
                self.document.update(cx, |document, cx| {
                    document.set_content(tab.content.as_str(), cx)
                });
            }
            self.synced_content = tab.content.clone();
        }
        let read_only = tab.loading || tab.saving || tab.revision.is_none();
        if self.read_only != read_only {
            self.read_only = read_only;
            cx.notify();
        }
    }

    #[cfg(test)]
    pub(super) fn document_for_test(&self) -> Entity<NyaDocumentEditorState> {
        self.document.clone()
    }

    pub(in crate::features) fn cursor_position(&self, cx: &App) -> (usize, usize) {
        self.document.read(cx).cursor_position(cx)
    }

    pub(in crate::features) fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.document.read(cx).focus_handle(cx)
    }

    pub(in crate::features) fn open_search(&self, cx: &mut Context<Self>) {
        self.document
            .update(cx, |document, cx| document.open_search(false, cx));
    }
}

impl Focusable for RemoteTextEditor {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.focus_handle(cx)
    }
}

impl Render for RemoteTextEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(app) = self.app.upgrade() else {
            return div().into_any_element();
        };
        let (font_size, font) = app.read_with(cx, |app, _| {
            (
                app.settings
                    .summary()
                    .transfer_internal_editor_font_size
                    .clamp(8, 72) as f32,
                app.gpui_terminal_font().font(),
            )
        });
        self.document.update(cx, |document, cx| {
            document.set_read_only(self.read_only, cx);
            document.set_font_size(font_size, cx);
            document.set_font(font.clone(), cx);
        });
        let activity_app = self.app.clone();
        let save_app = self.app.clone();
        let tab_id = self.tab_id.clone();
        nyaterm_file_browser_gpui::editor::editor_surface(
            &self.document,
            font,
            font_size,
            std::rc::Rc::new(move |cx| {
                let _ = activity_app.update(cx, |app, _| app.mark_user_activity());
            }),
            move |window, cx| {
                let _ = save_app.update(cx, |app, cx| {
                    app.save_transfer_editor_tab(&tab_id, false, window, cx)
                });
            },
        )
    }
}

fn language_for_path(path: &str) -> &'static str {
    nyaterm_core::document_language::document_language_for_path(path)
        .map_or("plain", |language| language.id)
}

#[cfg(test)]
mod tests {
    use super::language_for_path;

    #[test]
    fn remote_editor_chooses_supported_syntax_from_extension() {
        assert_eq!(language_for_path("/root/test.py"), "python");
        assert_eq!(language_for_path("/home/user/main.rs"), "rust");
        assert_eq!(language_for_path("/home/user/config.yaml"), "yaml");
        assert_eq!(language_for_path("/home/user/component.tsx"), "tsx");
        assert_eq!(language_for_path("/home/user/query.SQL"), "sql");
        assert_eq!(language_for_path("/home/user/image.svg"), "plain");
        assert_eq!(language_for_path("/home/user/unknown.bin"), "plain");
    }
}
