//! Strict parsing and presentation of 48-bit MAC addresses.

use std::{fmt, str::FromStr};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MacAddress([u8; 6]);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacAddressFormat {
    OctetHyphen,
    GroupHyphen,
    GroupDot,
    OctetColon,
    Plain,
}

impl MacAddressFormat {
    pub const ALL: [Self; 5] = [
        Self::OctetHyphen,
        Self::GroupHyphen,
        Self::GroupDot,
        Self::OctetColon,
        Self::Plain,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseMacAddressError;

impl fmt::Display for ParseMacAddressError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("expected a 48-bit MAC address with consistent grouping and separators")
    }
}

impl std::error::Error for ParseMacAddressError {}

impl FromStr for MacAddress {
    type Err = ParseMacAddressError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let (group_size, separator) = match text.len() {
            12 => (12, None),
            14 => (4, Some(text.as_bytes()[4])),
            17 => (2, Some(text.as_bytes()[2])),
            _ => return Err(ParseMacAddressError),
        };
        if !matches!(
            (group_size, separator),
            (12, None) | (4, Some(b'-' | b'.')) | (2, Some(b'-' | b':'))
        ) {
            return Err(ParseMacAddressError);
        }
        let mut bytes = [0u8; 6];
        let mut digit_index = 0;
        for (index, &byte) in text.as_bytes().iter().enumerate() {
            if separator.is_some() && (index + 1) % (group_size + 1) == 0 {
                if Some(byte) != separator {
                    return Err(ParseMacAddressError);
                }
                continue;
            }
            let nibble = match byte {
                b'0'..=b'9' => byte - b'0',
                b'a'..=b'f' => byte - b'a' + 10,
                b'A'..=b'F' => byte - b'A' + 10,
                _ => return Err(ParseMacAddressError),
            };
            bytes[digit_index / 2] = (bytes[digit_index / 2] << 4) | nibble;
            digit_index += 1;
        }
        Ok(Self(bytes))
    }
}

impl MacAddress {
    pub fn format(self, format: MacAddressFormat) -> String {
        let [a, b, c, d, e, f] = self.0;
        match format {
            MacAddressFormat::OctetHyphen => {
                format!("{a:02X}-{b:02X}-{c:02X}-{d:02X}-{e:02X}-{f:02X}")
            }
            MacAddressFormat::GroupHyphen => {
                format!("{a:02X}{b:02X}-{c:02X}{d:02X}-{e:02X}{f:02X}")
            }
            MacAddressFormat::GroupDot => format!("{a:02X}{b:02X}.{c:02X}{d:02X}.{e:02X}{f:02X}"),
            MacAddressFormat::OctetColon => {
                format!("{a:02X}:{b:02X}:{c:02X}:{d:02X}:{e:02X}:{f:02X}")
            }
            MacAddressFormat::Plain => format!("{a:02X}{b:02X}{c:02X}{d:02X}{e:02X}{f:02X}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{MacAddress, MacAddressFormat};

    #[test]
    fn every_supported_format_converts_to_all_five_uppercase_formats() {
        let expected = [
            "AA-BB-CC-DD-EE-FF",
            "AABB-CCDD-EEFF",
            "AABB.CCDD.EEFF",
            "AA:BB:CC:DD:EE:FF",
            "AABBCCDDEEFF",
        ];
        for input in expected {
            for input in [
                input.to_owned(),
                input.to_ascii_lowercase(),
                input.replace('A', "a"),
            ] {
                let mac = input.parse::<MacAddress>().unwrap();
                for (format, output) in MacAddressFormat::ALL.into_iter().zip(expected) {
                    assert_eq!(mac.format(format), output, "{input}");
                }
            }
        }
        for input in ["000000000000", "FFFFFFFFFFFF"] {
            let mac = input.parse::<MacAddress>().unwrap();
            for format in MacAddressFormat::ALL {
                assert_eq!(mac.format(format).parse::<MacAddress>(), Ok(mac));
            }
        }
    }

    #[test]
    fn rejects_partial_mixed_or_extended_addresses_without_trimming() {
        for input in [
            "",
            "AABBCCDDEEF",
            "AABBCCDDEEFF0",
            "AA-BB:CC-DD:EE-FF",
            "AABB.CCDD-EEFF",
            "AABB:CCDD:EEFF",
            "AA.BB.CC.DD.EE.FF",
            "A-ABB-CCDD-EEFF",
            "AA:BB:CC:DD:EE:FG",
            "AA:BB:CC:DD:EE:FF:00:11",
            " AABBCCDDEEFF",
            "AABBCCDDEEFF\n",
            "éABBCCDDEEFF",
        ] {
            assert!(input.parse::<MacAddress>().is_err(), "{input:?}");
        }
    }
}
