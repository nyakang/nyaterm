//! Strict file-content codecs. File BOMs never belong to the editable body.
use crate::character_encoding::CharacterEncoding;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FileEncoding {
    #[default]
    Utf8,
    Utf16Le,
    Utf16Be,
    Gbk,
    Gb18030,
    Big5,
    ShiftJis,
    EucKr,
    Windows1252,
}

impl FileEncoding {
    pub const ALL: [Self; 9] = [
        Self::Utf8,
        Self::Utf16Le,
        Self::Utf16Be,
        Self::Gbk,
        Self::Gb18030,
        Self::Big5,
        Self::ShiftJis,
        Self::EucKr,
        Self::Windows1252,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Utf8 => "UTF-8",
            Self::Utf16Le => "UTF-16 LE",
            Self::Utf16Be => "UTF-16 BE",
            Self::Gbk => "GBK",
            Self::Gb18030 => "GB18030",
            Self::Big5 => "Big5",
            Self::ShiftJis => "Shift_JIS",
            Self::EucKr => "EUC-KR",
            Self::Windows1252 => "Windows-1252",
        }
    }

    fn legacy(self) -> Option<CharacterEncoding> {
        Some(match self {
            Self::Gbk => CharacterEncoding::Gbk,
            Self::Gb18030 => CharacterEncoding::Gb18030,
            Self::Big5 => CharacterEncoding::Big5,
            Self::ShiftJis => CharacterEncoding::ShiftJis,
            Self::EucKr => CharacterEncoding::EucKr,
            Self::Windows1252 => CharacterEncoding::Windows1252,
            _ => return None,
        })
    }

    pub const fn bom(self) -> &'static [u8] {
        match self {
            Self::Utf8 => b"\xef\xbb\xbf",
            Self::Utf16Le => b"\xff\xfe",
            Self::Utf16Be => b"\xfe\xff",
            _ => b"",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FileTextFormat {
    pub encoding: FileEncoding,
    pub bom: bool,
}

impl FileTextFormat {
    pub fn label(self) -> String {
        format!(
            "{}{}",
            self.encoding.label(),
            if self.bom { " BOM" } else { "" }
        )
    }

    pub fn all() -> impl Iterator<Item = Self> {
        FileEncoding::ALL.into_iter().flat_map(|encoding| {
            [
                Some(Self {
                    encoding,
                    bom: false,
                }),
                (!encoding.bom().is_empty()).then_some(Self {
                    encoding,
                    bom: true,
                }),
            ]
            .into_iter()
            .flatten()
        })
    }

    pub fn encode(self, text: &str, max_bytes: u64) -> Result<Vec<u8>, FileEncodingError> {
        if text.contains('\0') {
            return Err(FileEncodingError::Binary);
        }
        if self.bom && self.encoding.bom().is_empty() {
            return Err(FileEncodingError::InvalidBom);
        }
        let mut bytes = match self.encoding {
            FileEncoding::Utf8 => text.as_bytes().to_vec(),
            FileEncoding::Utf16Le | FileEncoding::Utf16Be => text
                .encode_utf16()
                .flat_map(|unit| {
                    if self.encoding == FileEncoding::Utf16Le {
                        unit.to_le_bytes()
                    } else {
                        unit.to_be_bytes()
                    }
                })
                .collect(),
            encoding => {
                let legacy = encoding.legacy().expect("legacy encoding");
                let encoded = legacy
                    .encode(text)
                    .map_err(|_| FileEncodingError::Unrepresentable)?;
                // Some legacy encoders accept aliases (for example a yen sign
                // as a Shift_JIS backslash) without reporting an error.
                if legacy
                    .codec()
                    .decode_without_bom_handling_and_without_replacement(&encoded)
                    .as_deref()
                    != Some(text)
                {
                    return Err(FileEncodingError::Unrepresentable);
                }
                encoded
            }
        };
        if self.bom {
            bytes.splice(..0, self.encoding.bom().iter().copied());
        }
        if bytes.len() as u64 > max_bytes {
            return Err(FileEncodingError::TooLarge);
        }
        Ok(bytes)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FileReadEncoding {
    #[default]
    Auto,
    Explicit(FileEncoding),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FileEncodingError {
    #[error("Choose the file's encoding to open this non-UTF-8 text")]
    SelectionRequired,
    #[error("Invalid byte sequence for the selected encoding")]
    InvalidSequence,
    #[error("The selected encoding conflicts with the file BOM")]
    BomConflict,
    #[error("UTF-32 files are not supported")]
    UnsupportedUtf32,
    #[error("This encoding does not support a BOM")]
    InvalidBom,
    #[error("Binary files cannot be edited as text")]
    Binary,
    #[error("Text cannot be represented in this encoding; choose UTF-8")]
    Unrepresentable,
    #[error("Encoded file exceeds the size limit")]
    TooLarge,
    #[error(
        "This file cannot be saved losslessly in its original encoding; explicitly convert to UTF-8"
    )]
    NonRoundTrip,
}

pub struct DecodedFileText {
    pub text: String,
    pub format: FileTextFormat,
    pub round_trips: bool,
}

impl std::fmt::Debug for DecodedFileText {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DecodedFileText")
            .field("bytes", &self.text.len())
            .field("format", &self.format)
            .field("round_trips", &self.round_trips)
            .finish()
    }
}

pub fn decode_file_text(
    bytes: &[u8],
    selection: FileReadEncoding,
) -> Result<DecodedFileText, FileEncodingError> {
    if bytes.starts_with(b"\xff\xfe\0\0") || bytes.starts_with(b"\0\0\xfe\xff") {
        return Err(FileEncodingError::UnsupportedUtf32);
    }
    let bom = [
        FileEncoding::Utf8,
        FileEncoding::Utf16Le,
        FileEncoding::Utf16Be,
    ]
    .into_iter()
    .find(|encoding| bytes.starts_with(encoding.bom()));
    let encoding = match (selection, bom) {
        (FileReadEncoding::Auto, bom) => bom.unwrap_or_default(),
        (FileReadEncoding::Explicit(requested), Some(found)) if requested != found => {
            return Err(FileEncodingError::BomConflict);
        }
        (FileReadEncoding::Explicit(requested), _) => requested,
    };
    let body = &bytes[bom.map_or(0, |encoding| encoding.bom().len())..];
    let invalid = if selection == FileReadEncoding::Auto && bom.is_none() {
        FileEncodingError::SelectionRequired
    } else {
        FileEncodingError::InvalidSequence
    };
    let text = match encoding {
        FileEncoding::Utf8 => std::str::from_utf8(body).map_err(|_| invalid)?.to_owned(),
        FileEncoding::Utf16Le | FileEncoding::Utf16Be => {
            let (pairs, remainder) = body.as_chunks::<2>();
            if !remainder.is_empty() {
                return Err(invalid);
            }
            let units = pairs.iter().map(|&pair| {
                if encoding == FileEncoding::Utf16Le {
                    u16::from_le_bytes(pair)
                } else {
                    u16::from_be_bytes(pair)
                }
            });
            char::decode_utf16(units)
                .collect::<Result<String, _>>()
                .map_err(|_| invalid)?
        }
        encoding => encoding
            .legacy()
            .expect("legacy encoding")
            .codec()
            .decode_without_bom_handling_and_without_replacement(body)
            .ok_or(invalid)?
            .into_owned(),
    };
    if text.contains('\0') {
        return Err(FileEncodingError::Binary);
    }
    let format = FileTextFormat {
        encoding,
        bom: bom.is_some(),
    };
    let round_trips = format
        .encode(&text, u64::MAX)
        .is_ok_and(|encoded| encoded == bytes);
    Ok(DecodedFileText {
        text,
        format,
        round_trips,
    })
}

#[cfg(test)]
mod tests;
