use crate::features::transfers::state::tests::{editor_tab, preview_tab, transfer_state};
use crate::models::PreviewContent;
use gpui::TestAppContext;
use nyaterm_core::file_encoding::{FileEncoding, FileReadEncoding, FileTextFormat};
use nyaterm_files::remote_file::{RemoteTextDocument, RemoteTextGeneration, RemoteTextWriteResult};
use nyaterm_files::text_document::EncodedTextDocument;

#[test]
fn editor_encoding_changes_participate_in_save_all_and_failed_reopen_preserves_edits() {
    let cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut transfer = transfer_state(&cx);
    let tab = editor_tab("session", "/file.txt");
    let id = tab.id.clone();
    let revision = tab.revision.clone();
    transfer.open_editor_tab(tab);
    let format = FileTextFormat {
        encoding: FileEncoding::Utf16Le,
        bom: true,
    };
    transfer.set_editor_save_format(&id, format);
    assert_eq!(transfer.dirty_editor_tab_ids(), vec![id.clone()]);
    transfer.set_editor_save_format(&id, FileTextFormat::default());
    assert!(transfer.dirty_editor_tab_ids().is_empty());
    transfer.sync_editor_content(&id, "unsaved\r\n");
    transfer.set_editor_save_format(&id, format);
    let stale = transfer.editor_tab_snapshot(&id).unwrap().generation;
    transfer.set_editor_read_encoding(&id, FileReadEncoding::Explicit(FileEncoding::Gbk));
    transfer.begin_editor_document_reload(&id);
    let current = transfer.editor_tab_snapshot(&id).unwrap().generation;
    assert!(!transfer.complete_encoded_editor_load_tab(
        &id,
        stale,
        EncodedTextDocument {
            document: RemoteTextDocument {
                path: "/file.txt".into(),
                content: "stale".into(),
                revision: revision.clone().unwrap()
            },
            format: FileTextFormat::default(),
            round_trips: true,
        }
    ));
    assert!(transfer.fail_editor_operation_tab(&id, current, "decode failed".into()));
    let tab = transfer.editor_tab_snapshot(&id).unwrap();
    assert_eq!(tab.content, "unsaved\r\n");
    assert_eq!(tab.revision, revision);
    assert!(tab.dirty);
    assert_eq!(tab.encoding.target, format);
    assert_eq!(tab.encoding.read, FileReadEncoding::Auto);
    assert_eq!(tab.encoding.pending_read, None);
}

#[test]
fn unsuccessful_initial_load_cannot_save_and_conversion_failure_keeps_document_open() {
    let cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut transfer = transfer_state(&cx);
    let mut tab = editor_tab("session", "/file.txt");
    let id = tab.id.clone();
    let revision = tab.revision.take().unwrap();
    let generation = tab.generation;
    transfer.open_editor_tab(tab);
    transfer.fail_editor_operation_tab(&id, generation, "choose encoding".into());
    assert!(!transfer.begin_editor_tab_save(&id));
    let format = FileTextFormat {
        encoding: FileEncoding::Gbk,
        bom: false,
    };
    transfer.complete_encoded_editor_load_tab(
        &id,
        generation,
        EncodedTextDocument {
            document: RemoteTextDocument {
                path: "/file.txt".into(),
                content: "中文".into(),
                revision: revision.clone(),
            },
            format,
            round_trips: true,
        },
    );
    transfer.sync_editor_content(&id, "😀");
    assert!(transfer.begin_editor_tab_save(&id));
    transfer.fail_editor_operation_tab(&id, generation, "unrepresentable".into());
    let tab = transfer.editor_tab_snapshot(&id).unwrap();
    assert!(tab.dirty);
    assert!(!tab.saving);
    assert!(!tab.close_after_save);
    assert_eq!(tab.encoding.target, format);
    transfer.set_editor_save_format(&id, FileTextFormat::default());
    assert!(transfer.begin_editor_tab_save(&id));
    transfer.complete_editor_save_tab(&id, generation, RemoteTextWriteResult::Saved { revision });
    let tab = transfer.editor_tab_snapshot(&id).unwrap();
    assert!(!tab.dirty);
    assert_eq!(
        tab.encoding.read,
        FileReadEncoding::Explicit(FileEncoding::Utf8)
    );
}

#[test]
fn preview_ignores_stale_encoding_results() {
    let cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let mut transfer = transfer_state(&cx);
    let tab = preview_tab("session", "/file.txt");
    let id = tab.id.clone();
    let stale = tab.generation;
    transfer.open_preview_tab(tab);
    transfer.set_preview_read_encoding(FileReadEncoding::Explicit(FileEncoding::Gbk));
    let current = transfer.begin_preview_tab_reload(&id).unwrap();
    assert!(!transfer.complete_encoded_preview_tab(
        &id,
        stale,
        PreviewContent::Text("old".into()),
        Some(FileTextFormat::default())
    ));
    let format = FileTextFormat {
        encoding: FileEncoding::Gbk,
        bom: false,
    };
    assert!(transfer.complete_encoded_preview_tab(
        &id,
        current,
        PreviewContent::Text("中文".into()),
        Some(format)
    ));
    assert_eq!(
        transfer
            .preview_tab_snapshot(&id)
            .unwrap()
            .text_encoding
            .format,
        Some(format)
    );
    assert!(!transfer.complete_encoded_preview_tab(
        &id,
        RemoteTextGeneration::next(),
        PreviewContent::Error("stale".into()),
        None
    ));
}
