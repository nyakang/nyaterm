use std::io;

use nyaterm_core::character_encoding::CharacterEncoding;
use suppaftp::Encoding as SuppaEncoding;

/// Bridges NyaTerm's shared character catalog to suppaftp's command/list encoding hook.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FtpEncoding(pub CharacterEncoding);

impl SuppaEncoding for FtpEncoding {
    fn decode(&self, bytes: &[u8]) -> String {
        let (text, _, _) = self.0.codec().decode(bytes);
        text.into_owned()
    }

    fn encode(&self, text: &str) -> io::Result<Vec<u8>> {
        self.0.encode(text).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "text is not representable in the FTP filename encoding",
            )
        })
    }
}

/// Returns the host's native filename encoding.
///
/// Windows uses the ANSI code page. Unix-like systems use the locale encoding and
/// fall back to UTF-8 when no usable locale is configured.
pub fn system_filename_encoding() -> CharacterEncoding {
    #[cfg(windows)]
    {
        // SAFETY: GetACP has no preconditions and does not retain pointers.
        let code_page = unsafe { windows_sys::Win32::Globalization::GetACP() };
        encoding_for_windows_code_page(code_page).unwrap_or(CharacterEncoding::Utf8)
    }
    #[cfg(unix)]
    {
        locale_filename_encoding().unwrap_or(CharacterEncoding::Utf8)
    }
    #[cfg(not(any(windows, unix)))]
    {
        CharacterEncoding::Utf8
    }
}

/// Maps a Windows ANSI code page to the supported filename encodings.
pub fn encoding_for_windows_code_page(code_page: u32) -> Option<CharacterEncoding> {
    match code_page {
        65001 => Some(CharacterEncoding::Utf8),
        1252 => Some(CharacterEncoding::Windows1252),
        936 => Some(CharacterEncoding::Gbk),
        54936 => Some(CharacterEncoding::Gb18030),
        950 => Some(CharacterEncoding::Big5),
        932 => Some(CharacterEncoding::ShiftJis),
        949 => Some(CharacterEncoding::EucKr),
        _ => None,
    }
}

/// Reads the locale encoding on Unix-like systems.
pub fn locale_filename_encoding() -> Option<CharacterEncoding> {
    for key in ["LC_ALL", "LC_CTYPE", "LANG"] {
        let Ok(value) = std::env::var(key) else {
            continue;
        };
        if value.trim().is_empty() {
            continue;
        }
        if let Some(encoding) = encoding_from_locale(&value) {
            return Some(encoding);
        }
    }
    None
}

/// Parses `ll_CC.encoding@modifier` into a supported encoding.
pub fn encoding_from_locale(locale: &str) -> Option<CharacterEncoding> {
    let locale = locale.split('@').next().unwrap_or(locale).trim();
    if locale.is_empty() || locale.eq_ignore_ascii_case("C") || locale.eq_ignore_ascii_case("POSIX")
    {
        return None;
    }
    let encoding = locale
        .rsplit_once('.')
        .map(|(_, encoding)| encoding)
        .unwrap_or(locale)
        .trim()
        .replace('_', "-")
        .to_ascii_uppercase();
    match encoding.as_str() {
        "UTF-8" | "UTF8" => Some(CharacterEncoding::Utf8),
        "WINDOWS-1252" | "WINDOWS1252" | "CP1252" | "ISO-8859-1" | "ISO8859-1" | "LATIN1" => {
            Some(CharacterEncoding::Windows1252)
        }
        "GBK" | "GB2312" | "CP936" => Some(CharacterEncoding::Gbk),
        "GB18030" => Some(CharacterEncoding::Gb18030),
        "BIG5" | "BIG5-HKSCS" => Some(CharacterEncoding::Big5),
        "SHIFT-JIS" | "SHIFTJIS" | "SJIS" => Some(CharacterEncoding::ShiftJis),
        "EUC-KR" | "EUCKR" => Some(CharacterEncoding::EucKr),
        "US-ASCII" | "ASCII" | "ANSI-X3.4-1968" => Some(CharacterEncoding::Utf8),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        FtpEncoding, encoding_for_windows_code_page, encoding_from_locale, system_filename_encoding,
    };
    use nyaterm_core::character_encoding::CharacterEncoding;
    use suppaftp::Encoding as SuppaEncoding;

    #[test]
    fn windows_code_pages_map_to_supported_encodings() {
        assert_eq!(
            encoding_for_windows_code_page(936),
            Some(CharacterEncoding::Gbk)
        );
        assert_eq!(
            encoding_for_windows_code_page(932),
            Some(CharacterEncoding::ShiftJis)
        );
        assert_eq!(
            encoding_for_windows_code_page(949),
            Some(CharacterEncoding::EucKr)
        );
        assert_eq!(
            encoding_for_windows_code_page(950),
            Some(CharacterEncoding::Big5)
        );
        assert_eq!(
            encoding_for_windows_code_page(65001),
            Some(CharacterEncoding::Utf8)
        );
        assert_eq!(
            encoding_for_windows_code_page(1252),
            Some(CharacterEncoding::Windows1252)
        );
        assert_eq!(encoding_for_windows_code_page(437), None);
    }

    #[test]
    fn locales_map_to_supported_encodings() {
        assert_eq!(
            encoding_from_locale("zh_CN.GBK"),
            Some(CharacterEncoding::Gbk)
        );
        assert_eq!(
            encoding_from_locale("en_US.UTF-8"),
            Some(CharacterEncoding::Utf8)
        );
        assert_eq!(
            encoding_from_locale("en_US.ISO-8859-1"),
            Some(CharacterEncoding::Windows1252)
        );
        assert_eq!(
            encoding_from_locale("ja_JP.SJIS@modifier"),
            Some(CharacterEncoding::ShiftJis)
        );
        assert_eq!(
            encoding_from_locale("ko_KR.EUC-KR"),
            Some(CharacterEncoding::EucKr)
        );
        assert_eq!(encoding_from_locale("C"), None);
        assert_eq!(encoding_from_locale("POSIX"), None);
        assert_eq!(encoding_from_locale(""), None);
    }

    #[test]
    fn ftp_encoding_round_trips_and_rejects_unrepresentable_commands() {
        let encoding = FtpEncoding(CharacterEncoding::Windows1252);
        assert_eq!(encoding.decode(b"caf\xe9.txt"), "café.txt");
        assert_eq!(encoding.encode("café.txt").unwrap(), b"caf\xe9.txt");
        assert!(encoding.encode("中").is_err());
    }

    #[test]
    fn system_encoding_is_always_supported() {
        assert!(CharacterEncoding::ALL.contains(&system_filename_encoding()));
    }
}
