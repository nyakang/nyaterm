use gpui::{AppContext as _, TestAppContext};
use nyaterm_core::{AppRuntime, NoteDocument, RuntimeMode};
use nyaterm_notes_gpui::editor::NoteViewMode;
use nyaterm_ui::{NyaDocumentEditorEvent, nya_root};

use super::NoteEditorWindow;
use crate::entities::{OverlayStore, StartupRestoreStore, UiStoreHandles};
use crate::features::{NyaTermApp, view_widgets::ChildWindowSpec};
use crate::test_support::{TestAppCleanup, TestConfigDir};

#[test]
fn markdown_preview_follows_edits_and_survives_note_view_mode_changes() {
    let directory = TestConfigDir::new("nyaterm-note-markdown-preview");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    cx.update(nyaterm_ui::init_components);
    let root = directory.path();
    let runtime = AppRuntime::from_parts_for_test(
        RuntimeMode::Portable,
        root.to_path_buf(),
        root.join("config"),
        root.join("logs"),
        root.join("cache"),
        None,
    );
    let stores = UiStoreHandles {
        startup_restore: cx.new(|_| StartupRestoreStore::default()),
        overlays: cx.new(|_| OverlayStore::default()),
    };
    let app = cx.new(|cx| NyaTermApp::new(runtime, stores, cx));
    app.update(&mut cx, |app, cx| app.sync_component_theme(cx));
    let note = NoteDocument {
        id: "preview-note".into(),
        parent_id: None,
        title: "Preview".into(),
        markdown: "# Original".into(),
        sort_order: 0,
        revision: 1,
        created_at_ms: 1,
        updated_at_ms: 1,
        extra: Default::default(),
    };
    let mut editor = None;
    let (_, vcx) = cx.add_window_view(|window, cx| {
        let view = cx.new(|cx| {
            NoteEditorWindow::new(
                app,
                note,
                ChildWindowSpec::document("Note".into(), 640., 480.).chrome(),
                window,
                cx,
            )
        });
        editor = Some(view.clone());
        nya_root(view, window, cx)
    });
    let editor = editor.unwrap();
    vcx.run_until_parked();
    let text = editor.read_with(vcx, |editor, _| editor.markdown_preview.clone());
    let surface = editor.read_with(vcx, |editor, _| editor.editor.clone());
    surface.update(vcx, |surface, cx| {
        surface.set_content("# Updated 中文", cx);
        cx.emit(NyaDocumentEditorEvent::Changed("# Updated 中文".into()));
    });
    vcx.run_until_parked();
    editor.read_with(vcx, |editor, _| {
        assert_eq!(editor.note.markdown, "# Updated 中文");
        assert!(
            editor
                .markdown_preview
                .matches_source(&editor.note.markdown)
        );
        assert!(editor.dirty);
    });
    for mode in [
        NoteViewMode::Source,
        NoteViewMode::Preview,
        NoteViewMode::Split,
    ] {
        editor.update(vcx, |editor, cx| {
            editor.mode = mode;
            cx.notify();
        });
        vcx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        vcx.run_until_parked();
        vcx.update(|_, cx| {
            assert!(text.matches_source("# Updated 中文"));
            text.select_all(cx);
            assert_eq!(text.selected_text(cx), "Updated 中文");
        });
    }
}
