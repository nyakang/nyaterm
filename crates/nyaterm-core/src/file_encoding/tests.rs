use crate::file_encoding::{
    FileEncoding, FileEncodingError, FileReadEncoding, FileTextFormat, decode_file_text,
};

#[test]
fn every_format_round_trips_without_changing_newlines() {
    for format in FileTextFormat::all() {
        for text in ["", "hello\r\nworld\nlast\r", "hello\n"] {
            let bytes = format.encode(text, 1000).unwrap();
            let decoded =
                decode_file_text(&bytes, FileReadEncoding::Explicit(format.encoding)).unwrap();
            assert_eq!(decoded.text, text);
            assert_eq!(decoded.format, format);
            assert!(decoded.round_trips);
        }
        let text = match format.encoding {
            FileEncoding::Windows1252 => "café €",
            FileEncoding::ShiftJis => "日本語",
            FileEncoding::EucKr => "한국어",
            FileEncoding::Big5 => "繁體中文",
            _ => "中文",
        };
        let bytes = format.encode(text, 1000).unwrap();
        assert_eq!(
            decode_file_text(&bytes, FileReadEncoding::Explicit(format.encoding))
                .unwrap()
                .text,
            text
        );
    }
}

#[test]
fn auto_recognizes_boms_and_never_guesses_legacy_encodings() {
    for format in FileTextFormat::all().filter(|format| format.bom) {
        for text in ["", "中文😀\r\n"] {
            let decoded =
                decode_file_text(&format.encode(text, 1000).unwrap(), FileReadEncoding::Auto)
                    .unwrap();
            assert_eq!(decoded.text, text);
            assert_eq!(decoded.format, format);
        }
    }
    assert_eq!(
        decode_file_text(b"\xd6\xd0", FileReadEncoding::Auto).unwrap_err(),
        FileEncodingError::SelectionRequired
    );
}

#[test]
fn malformed_sequences_bom_conflicts_and_binary_files_are_rejected() {
    for bytes in [
        &b"\xff\xfe\0\xd8"[..],
        &b"\xfe\xff\xdc\0"[..],
        &b"\xff\xfeA"[..],
        &b"\xef\xbb\xbf\xff"[..],
    ] {
        assert_eq!(
            decode_file_text(bytes, FileReadEncoding::Auto).unwrap_err(),
            FileEncodingError::InvalidSequence
        );
    }
    assert_eq!(
        decode_file_text(
            b"\xef\xbb\xbftext",
            FileReadEncoding::Explicit(FileEncoding::Gbk)
        )
        .unwrap_err(),
        FileEncodingError::BomConflict
    );
    for bytes in [&b"\xff\xfe\0\0"[..], &b"\0\0\xfe\xff"[..]] {
        assert_eq!(
            decode_file_text(bytes, FileReadEncoding::Auto).unwrap_err(),
            FileEncodingError::UnsupportedUtf32
        );
    }
    assert_eq!(
        decode_file_text(b"a\0b", FileReadEncoding::Auto).unwrap_err(),
        FileEncodingError::Binary
    );
}

#[test]
fn encoding_errors_are_strict_bounded_and_redacted() {
    let gbk = FileTextFormat {
        encoding: FileEncoding::Gbk,
        bom: false,
    };
    let error = gbk.encode("secret😀", 100).unwrap_err();
    assert_eq!(error, FileEncodingError::Unrepresentable);
    assert!(!format!("{error:?} {error}").contains("secret"));
    assert_eq!(
        FileTextFormat::default().encode("abc", 2),
        Err(FileEncodingError::TooLarge)
    );
}

#[test]
fn legacy_aliases_do_not_silently_change_characters_or_original_bytes() {
    let shift_jis = FileTextFormat {
        encoding: FileEncoding::ShiftJis,
        bom: false,
    };
    assert_eq!(
        shift_jis.encode("¥", 100),
        Err(FileEncodingError::Unrepresentable)
    );
    let decoded = decode_file_text(
        &[0x87, 0x90],
        FileReadEncoding::Explicit(FileEncoding::ShiftJis),
    )
    .unwrap();
    assert!(!decoded.round_trips);
}
