//! Encoding-aware documents preserve revisions of the original on-disk bytes.
use crate::remote_file::{RemoteTextDocument, RemoteTextRevision};
use nyaterm_core::file_encoding::{FileReadEncoding, FileTextFormat, decode_file_text};

pub(crate) struct ByteDocument {
    pub path: String,
    pub bytes: Vec<u8>,
    pub revision: RemoteTextRevision,
}

#[derive(Clone)]
pub struct EncodedTextDocument {
    pub document: RemoteTextDocument,
    pub format: FileTextFormat,
    pub round_trips: bool,
}

impl std::fmt::Debug for EncodedTextDocument {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EncodedTextDocument")
            .field("format", &self.format)
            .field("round_trips", &self.round_trips)
            .finish_non_exhaustive()
    }
}

impl ByteDocument {
    pub fn decode(self, selection: FileReadEncoding) -> anyhow::Result<EncodedTextDocument> {
        let decoded = decode_file_text(&self.bytes, selection)?;
        Ok(EncodedTextDocument {
            document: RemoteTextDocument {
                path: self.path,
                content: decoded.text,
                revision: self.revision,
            },
            format: decoded.format,
            round_trips: decoded.round_trips,
        })
    }
}
