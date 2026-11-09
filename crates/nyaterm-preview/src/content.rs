//! Decoded data with no window, cache or render-object ownership.
use std::sync::Arc;

#[derive(Clone)]
pub struct RasterImage {
    /// Row-major BGRA8 source pixels; presentation adapters create their own surfaces.
    pub pixels: Arc<Vec<u8>>,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone)]
pub struct PdfDocument {
    pub bytes: Arc<Vec<u8>>,
    pub page_count: usize,
}

#[derive(Clone)]
pub struct DecodedDelimited {
    pub records: Vec<Vec<String>>,
    pub truncated: bool,
}

impl DecodedDelimited {
    pub(crate) fn new(records: Vec<Vec<String>>, truncated: bool) -> Self {
        Self { records, truncated }
    }
}

#[derive(Clone)]
pub enum DecodedContent {
    Text(String),
    Json {
        text: String,
        parse_error: Option<String>,
    },
    Markdown(String),
    Delimited(DecodedDelimited),
    Image(RasterImage),
    Pdf(PdfDocument),
    Error(String),
}

impl std::fmt::Debug for DecodedContent {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Preview data can contain documents and pixel buffers from a remote host.
        // Diagnostics report shape only, never the decoded user content.
        match self {
            Self::Text(text) => formatter.debug_tuple("Text").field(&text.len()).finish(),
            Self::Json { text, parse_error } => formatter
                .debug_struct("Json")
                .field("len", &text.len())
                .field("parse_error", &parse_error.is_some())
                .finish(),
            Self::Markdown(text) => formatter
                .debug_tuple("Markdown")
                .field(&text.len())
                .finish(),
            Self::Delimited(data) => formatter
                .debug_struct("Delimited")
                .field("rows", &data.records.len())
                .field("truncated", &data.truncated)
                .finish(),
            Self::Image(image) => formatter
                .debug_struct("Image")
                .field("width", &image.width)
                .field("height", &image.height)
                .finish(),
            Self::Pdf(document) => formatter
                .debug_struct("Pdf")
                .field("pages", &document.page_count)
                .finish(),
            Self::Error(_) => formatter.write_str("Error { .. }"),
        }
    }
}
