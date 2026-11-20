//! Shared character sets for terminal streams and remote path labels.

use encoding_rs::Encoding;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CharacterEncoding {
    #[default]
    Utf8,
    Windows1252,
    Gbk,
    Gb18030,
    Big5,
    ShiftJis,
    EucKr,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum EncodingError {
    #[error("Unsupported character encoding")]
    UnsupportedEncoding,
    #[error("Logical input must be valid UTF-8")]
    InvalidUtf8Input,
    #[error("Text cannot be represented in {0}")]
    UnrepresentableText(CharacterEncoding),
}

impl std::fmt::Display for CharacterEncoding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

impl CharacterEncoding {
    pub const ALL: [Self; 7] = [
        Self::Utf8,
        Self::Windows1252,
        Self::Gbk,
        Self::Gb18030,
        Self::Big5,
        Self::ShiftJis,
        Self::EucKr,
    ];

    pub fn parse(label: &str) -> Result<Self, EncodingError> {
        match label.trim().to_ascii_lowercase().as_str() {
            "utf8" | "utf-8" => Ok(Self::Utf8),
            "windows-1252" | "windows1252" | "cp1252" | "1252" | "iso-8859-1" | "latin1"
            | "latin-1" => Ok(Self::Windows1252),
            "gbk" | "gb2312" | "cp936" => Ok(Self::Gbk),
            "gb18030" => Ok(Self::Gb18030),
            "big5" => Ok(Self::Big5),
            "shift_jis" | "shift-jis" | "sjis" => Ok(Self::ShiftJis),
            "euc-kr" | "euckr" => Ok(Self::EucKr),
            _ => Err(EncodingError::UnsupportedEncoding),
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Utf8 => "UTF-8",
            Self::Windows1252 => "Windows-1252",
            Self::Gbk => "GBK",
            Self::Gb18030 => "GB18030",
            Self::Big5 => "Big5",
            Self::ShiftJis => "Shift_JIS",
            Self::EucKr => "EUC-KR",
        }
    }

    pub const fn codec(self) -> &'static Encoding {
        match self {
            Self::Utf8 => encoding_rs::UTF_8,
            Self::Windows1252 => encoding_rs::WINDOWS_1252,
            Self::Gbk => encoding_rs::GBK,
            Self::Gb18030 => encoding_rs::GB18030,
            Self::Big5 => encoding_rs::BIG5,
            Self::ShiftJis => encoding_rs::SHIFT_JIS,
            Self::EucKr => encoding_rs::EUC_KR,
        }
    }

    pub fn encode(self, text: &str) -> Result<Vec<u8>, EncodingError> {
        let (bytes, _, had_errors) = self.codec().encode(text);
        // WHATWG encoders otherwise insert HTML references into shell input.
        if had_errors {
            return Err(EncodingError::UnrepresentableText(self));
        }
        Ok(bytes.into_owned())
    }

    pub fn resolve_connection(label: &str, default: &str) -> Result<Self, EncodingError> {
        if label.trim().is_empty() || label.trim().eq_ignore_ascii_case("global") {
            Self::parse(default)
        } else {
            Self::parse(label)
        }
    }

    pub fn resolve_filename(label: &str, terminal: &str) -> Result<Self, EncodingError> {
        if label.trim().is_empty()
            || label.trim().eq_ignore_ascii_case("terminal")
            || label.trim().eq_ignore_ascii_case("global")
        {
            Self::parse(terminal)
        } else {
            Self::parse(label)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CharacterEncoding, EncodingError};

    #[test]
    fn catalog_and_legacy_aliases_resolve_without_accepting_follow_tokens() {
        for encoding in CharacterEncoding::ALL {
            assert_eq!(CharacterEncoding::parse(encoding.label()), Ok(encoding));
        }
        for (label, expected) in [
            (" Utf8 ", CharacterEncoding::Utf8),
            ("windows-1252", CharacterEncoding::Windows1252),
            ("Windows1252", CharacterEncoding::Windows1252),
            ("CP1252", CharacterEncoding::Windows1252),
            ("1252", CharacterEncoding::Windows1252),
            ("ISO-8859-1", CharacterEncoding::Windows1252),
            ("latin1", CharacterEncoding::Windows1252),
            ("latin-1", CharacterEncoding::Windows1252),
            ("gb2312", CharacterEncoding::Gbk),
            ("CP936", CharacterEncoding::Gbk),
            ("shift-jis", CharacterEncoding::ShiftJis),
            ("SJIS", CharacterEncoding::ShiftJis),
            ("euckr", CharacterEncoding::EucKr),
        ] {
            assert_eq!(CharacterEncoding::parse(label), Ok(expected));
        }
        for label in ["", "global", "terminal", "KOI8-R", "UTF-16", "ISO-2022-JP"] {
            assert_eq!(
                CharacterEncoding::parse(label),
                Err(EncodingError::UnsupportedEncoding)
            );
        }
    }

    #[test]
    fn windows_1252_round_trips_and_rejects_unrepresentable_text() {
        let bytes = CharacterEncoding::Windows1252.encode("café €").unwrap();
        let (decoded, _, had_errors) = encoding_rs::WINDOWS_1252.decode(&bytes);
        assert!(!had_errors);
        assert_eq!(decoded, "café €");
        assert_eq!(
            CharacterEncoding::Windows1252.encode("中"),
            Err(EncodingError::UnrepresentableText(
                CharacterEncoding::Windows1252
            ))
        );
    }

    #[test]
    fn follow_rules_use_the_supplied_encoding_and_reject_invalid_defaults() {
        for label in ["", "global", " GLOBAL "] {
            assert_eq!(
                CharacterEncoding::resolve_connection(label, "cp936"),
                Ok(CharacterEncoding::Gbk)
            );
        }
        for label in ["", "terminal", "global"] {
            assert_eq!(
                CharacterEncoding::resolve_filename(label, "sjis"),
                Ok(CharacterEncoding::ShiftJis)
            );
        }
        assert!(CharacterEncoding::resolve_connection("", "unknown").is_err());
        assert_eq!(
            CharacterEncoding::resolve_filename("UTF-8", "unknown"),
            Ok(CharacterEncoding::Utf8)
        );
    }

    #[test]
    fn unmappable_input_is_rejected_without_exposing_its_content() {
        let error = CharacterEncoding::Gbk.encode("secret😀").unwrap_err();
        assert_eq!(
            error,
            EncodingError::UnrepresentableText(CharacterEncoding::Gbk)
        );
        assert!(!format!("{error:?} {error}").contains("secret"));
        let bytes = CharacterEncoding::Gb18030.encode("😀").unwrap();
        assert_eq!(
            encoding_rs::GB18030.decode_without_bom_handling(&bytes).0,
            "😀"
        );
    }
}
