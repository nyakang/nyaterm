use std::ffi::OsString;
use std::io;

const FALLBACK_CLIENT_NAME: &str = "NYATERM";
// MS-RDPBCGR's clientName is 16 WCHARs, including the terminating NUL.
const MAX_CLIENT_NAME_UTF16_UNITS: usize = 15;

pub(super) fn resolve_client_name() -> String {
    client_name_from_hostname(hostname::get())
}

fn client_name_from_hostname(hostname: io::Result<OsString>) -> String {
    let Some(hostname) = hostname.ok().and_then(|name| name.into_string().ok()) else {
        return FALLBACK_CLIENT_NAME.to_owned();
    };
    // Validate the whole name before trimming or truncating so neither operation
    // can hide control characters, including an embedded NUL.
    if hostname.chars().any(char::is_control) {
        return FALLBACK_CLIENT_NAME.to_owned();
    }
    let hostname = hostname.trim();
    if hostname.is_empty() {
        return FALLBACK_CLIENT_NAME.to_owned();
    }

    let mut name = String::new();
    let mut utf16_units = 0;
    for character in hostname.chars() {
        utf16_units += character.len_utf16();
        if utf16_units > MAX_CLIENT_NAME_UTF16_UNITS {
            break;
        }
        name.push(character);
    }
    name
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::io;

    use super::{FALLBACK_CLIENT_NAME, client_name_from_hostname};

    #[test]
    fn system_name_preserves_case_unicode_and_dots() {
        for name in ["DESKTOP-ABC", "Office-PC", "笔记本-PC", "host.example"] {
            assert_eq!(client_name_from_hostname(Ok(OsString::from(name))), name);
        }
    }

    #[test]
    fn surrounding_whitespace_is_trimmed() {
        assert_eq!(
            client_name_from_hostname(Ok(OsString::from(" \u{2003}Office-PC\u{2003} "))),
            "Office-PC"
        );
    }

    #[test]
    fn hostname_read_failure_uses_fallback() {
        assert_eq!(
            client_name_from_hostname(Err(io::Error::other("hostname unavailable"))),
            FALLBACK_CLIENT_NAME
        );
    }

    #[test]
    fn empty_or_whitespace_only_hostname_uses_fallback() {
        for name in ["", " ", " \u{2003} "] {
            assert_eq!(
                client_name_from_hostname(Ok(OsString::from(name))),
                FALLBACK_CLIENT_NAME
            );
        }
    }

    #[test]
    fn control_characters_are_rejected_before_trimming_or_truncation() {
        for name in [
            "PC\0NAME",
            "PC\nNAME",
            "PC\u{7f}NAME",
            "\tOffice-PC",
            "Office-PC\n",
            "123456789012345\0",
        ] {
            assert_eq!(
                client_name_from_hostname(Ok(OsString::from(name))),
                FALLBACK_CLIENT_NAME
            );
        }
    }

    #[test]
    fn ascii_hostname_is_limited_to_fifteen_utf16_units() {
        for name in ["123456789012345", "1234567890123456"] {
            assert_eq!(
                client_name_from_hostname(Ok(OsString::from(name))),
                "123456789012345"
            );
        }
    }

    #[test]
    fn bmp_unicode_is_truncated_at_a_character_boundary() {
        assert_eq!(
            client_name_from_hostname(Ok(OsString::from("机".repeat(16)))),
            "机".repeat(15)
        );
    }

    #[test]
    fn non_bmp_character_is_preserved_when_both_utf16_units_fit() {
        assert_eq!(
            client_name_from_hostname(Ok(OsString::from("1234567890123😀tail"))),
            "1234567890123😀"
        );
    }

    #[test]
    fn non_bmp_character_is_omitted_when_only_one_utf16_unit_remains() {
        assert_eq!(
            client_name_from_hostname(Ok(OsString::from("12345678901234😀tail"))),
            "12345678901234"
        );
        assert_eq!(
            client_name_from_hostname(Ok(OsString::from("😀".repeat(8)))),
            "😀".repeat(7)
        );
    }

    #[cfg(unix)]
    #[test]
    fn invalid_utf8_hostname_uses_fallback() {
        use std::os::unix::ffi::OsStringExt;

        assert_eq!(
            client_name_from_hostname(Ok(OsString::from_vec(vec![b'P', b'C', 0xff]))),
            FALLBACK_CLIENT_NAME
        );
    }

    #[cfg(windows)]
    #[test]
    fn unpaired_utf16_surrogate_in_hostname_uses_fallback() {
        use std::os::windows::ffi::OsStringExt;

        assert_eq!(
            client_name_from_hostname(Ok(OsString::from_wide(&[0x0050, 0x0043, 0xd800]))),
            FALLBACK_CLIENT_NAME
        );
    }
}
