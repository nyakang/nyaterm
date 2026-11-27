use nyaterm_core::file_encoding::{FileEncoding, FileReadEncoding, FileTextFormat};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct EditorFileEncoding {
    pub read: FileReadEncoding,
    pub pending_read: Option<FileReadEncoding>,
    pub target: FileTextFormat,
    pub original: FileTextFormat,
    pub non_round_trip: bool,
    pub content_dirty: bool,
}

impl EditorFileEncoding {
    pub fn can_save(&self) -> bool {
        !self.non_round_trip || self.target.encoding == FileEncoding::Utf8
    }

    pub fn dirty(&self) -> bool {
        self.content_dirty || self.target != self.original
    }

    pub fn loaded(&mut self, format: FileTextFormat, round_trips: bool) {
        self.read = self.pending_read.take().unwrap_or(self.read);
        self.target = format;
        self.original = format;
        self.non_round_trip = !round_trips;
        self.content_dirty = false;
    }

    pub fn saved(&mut self) {
        // A converted file must reload in the format just written, even if the
        // original was opened with an explicit, different character encoding.
        if self.target != self.original {
            self.read = FileReadEncoding::Explicit(self.target.encoding);
        }
        self.original = self.target;
        self.non_round_trip = false;
        self.content_dirty = false;
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct PreviewFileEncoding {
    pub read: FileReadEncoding,
    pub format: Option<FileTextFormat>,
}

#[cfg(test)]
mod tests {
    use crate::models::file_encoding::EditorFileEncoding;
    use nyaterm_core::file_encoding::{FileEncoding, FileReadEncoding, FileTextFormat};

    #[test]
    fn reverting_encoding_keeps_content_changes_dirty() {
        let mut state = EditorFileEncoding::default();
        state.target.encoding = FileEncoding::Gbk;
        assert!(state.dirty());
        state.target = state.original;
        assert!(!state.dirty());
        state.content_dirty = true;
        state.target.encoding = FileEncoding::Gbk;
        state.target = state.original;
        assert!(state.dirty());
        state.saved();
        assert_eq!(state.read, FileReadEncoding::Auto);
    }

    #[test]
    fn non_round_trip_files_require_utf8_conversion_and_saved_format_controls_reload() {
        let format = FileTextFormat {
            encoding: FileEncoding::ShiftJis,
            bom: false,
        };
        let mut state = EditorFileEncoding {
            pending_read: Some(FileReadEncoding::Explicit(format.encoding)),
            ..Default::default()
        };
        state.loaded(format, false);
        assert!(!state.can_save());
        state.target = FileTextFormat::default();
        assert!(state.can_save());
        assert!(state.dirty());
        state.saved();
        assert_eq!(state.read, FileReadEncoding::Explicit(FileEncoding::Utf8));
        assert!(!state.dirty());
    }
}
