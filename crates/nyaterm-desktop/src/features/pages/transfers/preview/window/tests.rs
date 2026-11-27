use gpui::{AppContext as _, Entity, TestAppContext};
use nyaterm_core::{AppRuntime, PreviewCategory, RuntimeMode};
use nyaterm_files::remote_file::RemoteTextGeneration;

use super::RemoteFilePreviewWindow;
use crate::entities::{OverlayStore, StartupRestoreStore, UiStoreHandles};
use crate::features::{NyaTermApp, view_widgets::ChildWindowSpec};
use crate::models::{PreviewContent, PreviewViewport, TransferPreviewState};
use crate::test_support::{TestAppCleanup, TestConfigDir};

fn test_app(cx: &mut TestAppContext, root: &std::path::Path) -> Entity<NyaTermApp> {
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
    cx.new(|cx| NyaTermApp::new(runtime, stores, cx))
}

fn tab(id: &str, source: &str) -> TransferPreviewState {
    TransferPreviewState {
        id: id.into(),
        session_id: None,
        remote_path: format!("/{id}.md"),
        raw_path_token: None,
        name: format!("{id}.md"),
        size: None,
        modified_at: None,
        category: PreviewCategory::Markdown,
        generation: RemoteTextGeneration::next(),
        content: PreviewContent::Markdown(source.into()),
        viewport: PreviewViewport::default(),
    }
}

#[test]
fn markdown_preview_notifications_preserve_selection_but_tab_switches_retire_it() {
    let directory = TestConfigDir::new("nyaterm-markdown-preview-window");
    let mut cx = TestAppContext::single();
    let _cleanup = TestAppCleanup::new(&cx);
    cx.update(nyaterm_ui::init_components);
    let app = test_app(&mut cx, directory.path());
    app.update(&mut cx, |app, cx| {
        app.transfer
            .open_preview_tab(tab("first", "# Shared content"));
        cx.notify();
    });
    let view = cx.new(|cx| {
        RemoteFilePreviewWindow::new(
            app.clone(),
            ChildWindowSpec::document("Preview".into(), 640., 480.).chrome(),
            cx,
        )
    });
    cx.run_until_parked();
    let first = view.read_with(&cx, |view, _| {
        view.markdown_preview.as_ref().unwrap().text.clone()
    });
    cx.update(|cx| first.select_all(cx));
    app.update(&mut cx, |_, cx| cx.notify());
    cx.run_until_parked();
    cx.update(|cx| assert_eq!(first.selected_text(cx), "Shared content"));

    // Equal contents still represent different documents and must not share a selection.
    app.update(&mut cx, |app, cx| {
        app.transfer
            .open_preview_tab(tab("second", "# Shared content"));
        cx.notify();
    });
    cx.run_until_parked();
    cx.update(|cx| {
        assert_eq!(first.selected_text(cx), "");
        let preview = view.read(cx).markdown_preview.as_ref().unwrap();
        assert_eq!(preview.tab_id, "second");
        assert!(preview.text.matches_source("# Shared content"));
        assert_eq!(preview.text.selected_text(cx), "");
    });
    app.update(&mut cx, |app, cx| {
        app.transfer.activate_preview_tab("first");
        cx.notify();
    });
    cx.run_until_parked();
    view.read_with(&cx, |view, cx| {
        let preview = view.markdown_preview.as_ref().unwrap();
        assert_eq!(preview.tab_id, "first");
        assert_eq!(preview.text.selected_text(cx), "");
    });
    app.update(&mut cx, |app, cx| {
        let mut plain = tab("plain", "");
        plain.content = PreviewContent::Text("plain".into());
        app.transfer.open_preview_tab(plain);
        cx.notify();
    });
    cx.run_until_parked();
    view.read_with(&cx, |view, _| assert!(view.markdown_preview.is_none()));
}
