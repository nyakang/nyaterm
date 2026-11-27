use crate::file_browser::FileBrowserService;
use crate::file_browser::text_document::EncodedDocumentWrite;
use crate::local_fs::LocalFileService;
use crate::remote_file::RemoteTextWriteResult;
use crate::sftp::RemoteFilePath;
use nyaterm_core::file_encoding::{FileEncoding, FileReadEncoding, FileTextFormat};

#[test]
fn encoded_local_documents_preserve_bytes_and_detect_external_changes() {
    let directory = tempfile::tempdir().unwrap();
    let local = directory.path().join("document.txt");
    let path = RemoteFilePath::new(local.to_str().unwrap());
    let service = FileBrowserService::Local(LocalFileService);
    for format in FileTextFormat::all() {
        let original = format.encode("hello\r\nworld\n", 1024).unwrap();
        std::fs::write(&local, &original).unwrap();
        let file = service
            .read_encoded_document_path(&path, 1024, FileReadEncoding::Explicit(format.encoding))
            .unwrap();
        assert!(file.round_trips);
        let result = service
            .write_encoded_document_path(
                &path,
                EncodedDocumentWrite {
                    content: &file.document.content,
                    format: file.format,
                    expected_revision: Some(&file.document.revision),
                    force: false,
                    max_bytes: 1024,
                },
            )
            .unwrap();
        let RemoteTextWriteResult::Saved { revision } = result else {
            panic!("expected saved document");
        };
        assert_eq!(std::fs::read(&local).unwrap(), original);
        std::fs::write(&local, format.encode("jello\r\nworld\n", 1024).unwrap()).unwrap();
        assert_eq!(
            service
                .write_encoded_document_path(
                    &path,
                    EncodedDocumentWrite {
                        content: "edited",
                        format,
                        expected_revision: Some(&revision),
                        force: false,
                        max_bytes: 1024,
                    }
                )
                .unwrap(),
            RemoteTextWriteResult::Conflict
        );
    }
}

#[test]
fn invalid_or_oversized_output_never_overwrites_even_when_forced() {
    let directory = tempfile::tempdir().unwrap();
    let local = directory.path().join("document.txt");
    std::fs::write(&local, b"original").unwrap();
    let service = FileBrowserService::Local(LocalFileService);
    let path = RemoteFilePath::new(local.to_str().unwrap());
    for (format, content, max_bytes) in [
        (
            FileTextFormat {
                encoding: FileEncoding::Gbk,
                bom: false,
            },
            "secret😀",
            1024,
        ),
        (FileTextFormat::default(), "too large", 2),
    ] {
        assert!(
            service
                .write_encoded_document_path(
                    &path,
                    EncodedDocumentWrite {
                        content,
                        format,
                        expected_revision: None,
                        force: true,
                        max_bytes,
                    }
                )
                .is_err()
        );
        assert_eq!(std::fs::read(&local).unwrap(), b"original");
    }
}

#[test]
fn legacy_utf8_interface_does_not_start_accepting_other_encodings() {
    let directory = tempfile::tempdir().unwrap();
    let local = directory.path().join("document.txt");
    std::fs::write(&local, [0xd6, 0xd0]).unwrap();
    let service = FileBrowserService::Local(LocalFileService);
    let path = RemoteFilePath::new(local.to_str().unwrap());
    assert!(service.read_text_document_path(&path, 1024).is_err());
    assert_eq!(
        service
            .read_encoded_document_path(&path, 1024, FileReadEncoding::Explicit(FileEncoding::Gbk))
            .unwrap()
            .document
            .content,
        "中"
    );
}
