//! Decode fetched bytes and text without constructing UI objects.
//!
//! Callers run decoding off the UI thread. PDF decoding remains guarded against
//! panics; pages are rendered on demand with the existing pixel budgets.

use std::sync::Arc;

use nyaterm_core::PreviewCategory;

use crate::content::{DecodedContent, DecodedDelimited, PdfDocument, RasterImage};

/// Rows past this are dropped from a delimited preview; a grid of millions of
/// rows would not be readable and would cost far more to lay out than to fetch.
const DELIMITED_MAX_ROWS: usize = 200_000;

/// Largest decoded image dimension (width or height) the preview will accept.
/// A "compression bomb" (a tiny file that decodes to an enormous canvas) is
/// rejected before the pixels are allocated.
const IMAGE_MAX_DIMENSION: u32 = 20_000;

/// Largest decoded image area, in pixels. Bounds peak allocation independently
/// of the per-dimension cap (a 20k x 20k image would still be 1.6 GB of RGBA).
const IMAGE_MAX_PIXELS: u64 = 64 * 1_000_000; // 64 megapixels

/// Largest decode allocation the `image` decoder is permitted, in bytes.
const IMAGE_MAX_ALLOC_BYTES: u64 = 512 * 1024 * 1024;

/// Scale applied when rasterizing PDF pages, trading memory for legibility.
const PDF_RENDER_SCALE: f32 = 1.5;

/// Largest rendered PDF page dimension accepted; guards a page whose media box
/// would rasterize to an enormous surface at [`PDF_RENDER_SCALE`].
const PDF_PAGE_MAX_DIMENSION: u32 = 10_000;

/// Largest rendered PDF page area, in pixels.
const PDF_PAGE_MAX_PIXELS: u64 = 40 * 1_000_000;

/// Decode fetched bytes into the content for `category`.
///
/// Byte inputs (image/PDF) are decoded here; text inputs arrive already read as
/// a string by the loader and go through [`decode_text_content`]. PDF only opens
/// the document and counts pages; individual pages rasterize lazily through
/// [`rasterize_pdf_page`].
pub fn decode_binary_content(category: PreviewCategory, bytes: Vec<u8>) -> DecodedContent {
    match category {
        PreviewCategory::Image => match decode_image(&bytes) {
            Ok(image) => DecodedContent::Image(image),
            Err(message) => DecodedContent::Error(message),
        },
        PreviewCategory::Pdf => match open_pdf_document(bytes) {
            Ok(document) => DecodedContent::Pdf(document),
            Err(message) => DecodedContent::Error(message),
        },
        // Text-shaped categories never reach the binary path.
        _ => DecodedContent::Error("unexpected binary preview category".to_string()),
    }
}

/// Build the content for a text-shaped `category` from the fetched text.
pub fn decode_text_content(category: PreviewCategory, content: String) -> DecodedContent {
    match category {
        PreviewCategory::Text => DecodedContent::Text(content),
        PreviewCategory::Json => {
            let (text, parse_error) = pretty_print_json(&content);
            DecodedContent::Json { text, parse_error }
        }
        PreviewCategory::Markdown => DecodedContent::Markdown(content),
        PreviewCategory::Delimited { tab } => {
            let (records, truncated) = parse_delimited(&content, tab);
            DecodedContent::Delimited(DecodedDelimited::new(records, truncated))
        }
        // Binary/unsupported categories never reach the text path.
        PreviewCategory::Image | PreviewCategory::Pdf | PreviewCategory::Unsupported => {
            DecodedContent::Error("unexpected text preview category".to_string())
        }
    }
}

/// Pretty-print `content` when it is valid JSON; otherwise return the raw text
/// unchanged **and** the parser error, so the view can show an explicit banner
/// rather than silently presenting the raw document as if it were fine.
fn pretty_print_json(content: &str) -> (String, Option<String>) {
    match serde_json::from_str::<serde_json::Value>(content) {
        Ok(value) => match serde_json::to_string_pretty(&value) {
            Ok(pretty) => (pretty, None),
            Err(error) => (
                content.to_string(),
                Some(format!("could not format JSON: {error}")),
            ),
        },
        Err(error) => (content.to_string(), Some(error.to_string())),
    }
}

/// Split `content` into records on the delimiter for the category, honouring
/// simple RFC 4180 double-quote quoting so a delimiter or newline inside quotes
/// does not split a field. Returns the records (header row included, at index 0)
/// and whether the body was truncated at [`DELIMITED_MAX_ROWS`].
pub fn parse_delimited(content: &str, tab: bool) -> (Vec<Vec<String>>, bool) {
    let delimiter = if tab { '\t' } else { ',' };
    let mut records = parse_delimited_records(content, delimiter);
    if records.is_empty() {
        return (Vec::new(), false);
    }
    // The header row (records[0]) is always kept; truncation applies to the body.
    let body_len = records.len().saturating_sub(1);
    let truncated = body_len > DELIMITED_MAX_ROWS;
    if truncated {
        records.truncate(1 + DELIMITED_MAX_ROWS);
    }
    (records, truncated)
}

fn parse_delimited_records(content: &str, delimiter: char) -> Vec<Vec<String>> {
    let mut records = Vec::new();
    let mut record = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut chars = content.chars().peekable();
    let mut saw_any = false;

    while let Some(ch) = chars.next() {
        saw_any = true;
        if in_quotes {
            if ch == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    in_quotes = false;
                }
            } else {
                field.push(ch);
            }
            continue;
        }
        match ch {
            '"' => in_quotes = true,
            '\r' => {
                // Swallow CRLF as a single record terminator.
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                record.push(std::mem::take(&mut field));
                records.push(std::mem::take(&mut record));
            }
            '\n' => {
                record.push(std::mem::take(&mut field));
                records.push(std::mem::take(&mut record));
            }
            ch if ch == delimiter => record.push(std::mem::take(&mut field)),
            ch => field.push(ch),
        }
    }

    // Trailing field/record with no terminating newline.
    if !field.is_empty() || !record.is_empty() {
        record.push(field);
        records.push(record);
    } else if saw_any && content.ends_with(['\n', '\r']) {
        // File ended exactly on a newline: nothing left to flush.
    }
    records
}

/// Decode a raster image into pixels, with compression-bomb
/// guards applied *before* the pixels are allocated.
///
/// Pixels use BGRA8 consistently across image decode and PDF rasterization.
fn decode_image(bytes: &[u8]) -> Result<RasterImage, String> {
    // Pre-flight: read only the header to learn the advertised dimensions and
    // reject a compression bomb *before* allocating any pixels.
    let probe = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| format!("could not read image: {error}"))?;
    if let Ok((width, height)) = probe.into_dimensions() {
        check_pixel_budget(width, height, IMAGE_MAX_DIMENSION, IMAGE_MAX_PIXELS)?;
    }

    // Bound both the canvas dimensions and the peak decode allocation before
    // decoding. `set_limits` is enforced by decoders that honour it; the
    // explicit dimension check above and the post-decode check below back it up.
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| format!("could not read image: {error}"))?;
    // `image::Limits` is `#[non_exhaustive]`, so it must be built by field
    // assignment rather than a struct literal.
    #[allow(clippy::field_reassign_with_default)]
    let limits = {
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(IMAGE_MAX_DIMENSION);
        limits.max_image_height = Some(IMAGE_MAX_DIMENSION);
        limits.max_alloc = Some(IMAGE_MAX_ALLOC_BYTES);
        limits
    };
    reader.limits(limits);

    let decoded = reader
        .decode()
        .map_err(|error| format!("could not decode image: {error}"))?;
    raster_from_rgba(decoded.into_rgba8())
}

/// Reject a canvas that exceeds the per-dimension or total-pixel budget.
fn check_pixel_budget(
    width: u32,
    height: u32,
    max_dimension: u32,
    max_pixels: u64,
) -> Result<(), String> {
    if width == 0 || height == 0 {
        return Err("image has no pixels".to_string());
    }
    if width > max_dimension || height > max_dimension {
        return Err(format!(
            "image is too large to preview ({width}×{height}); the limit is {max_dimension} px per side"
        ));
    }
    let pixels = u64::from(width) * u64::from(height);
    if pixels > max_pixels {
        return Err(format!(
            "image is too large to preview ({width}×{height}); the limit is {} megapixels",
            max_pixels / 1_000_000
        ));
    }
    Ok(())
}

fn raster_from_rgba(mut rgba: image::RgbaImage) -> Result<RasterImage, String> {
    let (width, height) = rgba.dimensions();
    check_pixel_budget(width, height, IMAGE_MAX_DIMENSION, IMAGE_MAX_PIXELS)?;
    for pixel in rgba.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    let bgra = rgba.into_raw();
    Ok(RasterImage {
        pixels: Arc::new(bgra),
        width,
        height,
    })
}

/// Rotate BGRA pixels by `quarter_turns` clockwise, returning the new buffer and
/// dimensions. Runs off the render path (on a rotate action), so an in-place
/// transform of an already-decoded buffer is acceptable.
pub fn rotate_bgra(
    pixels: &[u8],
    width: u32,
    height: u32,
    quarter_turns: u8,
) -> (Vec<u8>, u32, u32) {
    let turns = quarter_turns % 4;
    if turns == 0 {
        return (pixels.to_vec(), width, height);
    }
    let (w, h) = (width as usize, height as usize);
    let pixel_at = |x: usize, y: usize| {
        let index = (y * w + x) * 4;
        [
            pixels[index],
            pixels[index + 1],
            pixels[index + 2],
            pixels[index + 3],
        ]
    };
    let (new_w, new_h) = if turns == 2 { (w, h) } else { (h, w) };
    let mut out = vec![0u8; new_w * new_h * 4];
    for y in 0..h {
        for x in 0..w {
            let (nx, ny) = match turns {
                1 => (h - 1 - y, x),         // 90° clockwise
                2 => (w - 1 - x, h - 1 - y), // 180°
                _ => (y, w - 1 - x),         // 270° clockwise
            };
            let dst = (ny * new_w + nx) * 4;
            out[dst..dst + 4].copy_from_slice(&pixel_at(x, y));
        }
    }
    (out, new_w as u32, new_h as u32)
}

/// Open a PDF and count its pages **without** rasterizing any of them.
///
/// Wrapped in `catch_unwind` because `hayro` parses untrusted, server-controlled
/// bytes: a panic while opening a malformed document becomes an error card
/// instead of aborting the app. Rendering each page later is guarded the same
/// way in [`rasterize_pdf_page`].
fn open_pdf_document(bytes: Vec<u8>) -> Result<PdfDocument, String> {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        open_pdf_document_inner(bytes)
    }));
    match result {
        Ok(document) => document,
        Err(_) => Err("the PDF could not be opened (it may be malformed)".to_string()),
    }
}

fn open_pdf_document_inner(bytes: Vec<u8>) -> Result<PdfDocument, String> {
    use hayro::hayro_syntax::Pdf;

    let shared = Arc::new(bytes);
    let pdf = Pdf::new(shared.clone()).map_err(|error| format!("could not open PDF: {error:?}"))?;
    let page_count = pdf.pages().len();
    if page_count == 0 {
        return Err("the PDF has no pages".to_string());
    }
    Ok(PdfDocument {
        bytes: shared,
        page_count,
    })
}

/// Rasterize a single PDF page by zero-based index, on the background thread.
///
/// Returns an error when the page is out of range. Confined
/// to `catch_unwind` for the same reason as [`open_pdf_document`]: a panic in the
/// page interpreter must not take down the process.
pub fn rasterize_pdf_page(bytes: &Arc<Vec<u8>>, page_index: usize) -> Result<RasterImage, String> {
    let bytes = bytes.clone();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rasterize_pdf_page_inner(bytes, page_index)
    }));
    match result {
        Ok(page) => page,
        Err(_) => Err("this PDF page could not be rendered (it may be malformed)".to_string()),
    }
}

fn rasterize_pdf_page_inner(bytes: Arc<Vec<u8>>, page_index: usize) -> Result<RasterImage, String> {
    use hayro::hayro_interpret::InterpreterSettings;
    use hayro::hayro_syntax::Pdf;
    use hayro::{RenderSettings, render};

    let pdf = Pdf::new(bytes).map_err(|error| format!("could not open PDF: {error:?}"))?;
    let pages = pdf.pages();
    let page = pages
        .get(page_index)
        .ok_or_else(|| "PDF page is out of range".to_string())?;

    // `embed-fonts` supplies substitutes for the 14 standard fonts, so the
    // default interpreter settings resolve non-embedded base fonts on their own.
    let interpreter_settings = InterpreterSettings::default();
    let render_settings = RenderSettings {
        x_scale: PDF_RENDER_SCALE,
        y_scale: PDF_RENDER_SCALE,
        bg_color: hayro::vello_cpu::color::palette::css::WHITE,
        ..Default::default()
    };
    let cache = hayro::RenderCache::new();

    let pixmap = render(page, &cache, &interpreter_settings, &render_settings);
    let width = pixmap.width() as u32;
    let height = pixmap.height() as u32;
    check_pixel_budget(width, height, PDF_PAGE_MAX_DIMENSION, PDF_PAGE_MAX_PIXELS)?;

    // `take_unpremultiplied()` yields row-major RGBA8; reinterpret as bytes and
    // swap to the BGRA the GPUI atlas expects.
    let unpremultiplied = pixmap.take_unpremultiplied();
    let mut rgba: Vec<u8> = bytemuck::cast_slice(&unpremultiplied).to_vec();
    for pixel in rgba.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    Ok(RasterImage {
        pixels: Arc::new(rgba),
        width,
        height,
    })
}

#[cfg(test)]
mod tests {
    use nyaterm_core::PreviewCategory;

    use crate::content::DecodedContent;

    use super::{
        IMAGE_MAX_DIMENSION, IMAGE_MAX_PIXELS, check_pixel_budget, decode_text_content,
        parse_delimited, pretty_print_json,
    };

    #[test]
    fn rotating_bgra_90_degrees_moves_the_top_left_pixel_to_the_top_right() {
        // 2x1 image: pixel (0,0) red, (1,0) green (BGRA bytes).
        let width = 2;
        let height = 1;
        let pixels = vec![
            0, 0, 255, 255, // (0,0) red
            0, 255, 0, 255, // (1,0) green
        ];
        let (rotated, w, h) = super::rotate_bgra(&pixels, width, height, 1);
        // 90° clockwise: 2x1 -> 1x2; original (0,0) -> (0,0), (1,0) -> (0,1).
        assert_eq!((w, h), (1, 2));
        assert_eq!(&rotated[0..4], &[0, 0, 255, 255]);
        assert_eq!(&rotated[4..8], &[0, 255, 0, 255]);

        // Four turns is identity.
        let mut buffer = pixels.clone();
        let (mut cw, mut ch) = (width, height);
        for _ in 0..4 {
            let (next, nw, nh) = super::rotate_bgra(&buffer, cw, ch, 1);
            buffer = next;
            cw = nw;
            ch = nh;
        }
        assert_eq!((cw, ch), (width, height));
        assert_eq!(buffer, pixels);
    }

    #[test]
    fn json_is_pretty_printed_when_valid_and_kept_with_error_otherwise() {
        let (pretty, error) = pretty_print_json("{\"b\":1,\"a\":[2,3]}");
        assert!(pretty.contains('\n'), "valid JSON should be reflowed");
        assert!(error.is_none());

        // Invalid JSON keeps the raw text AND surfaces an explicit error.
        let (raw, error) = pretty_print_json("not json {");
        assert_eq!(raw, "not json {");
        assert!(error.is_some(), "invalid JSON must report a parse error");
    }

    #[test]
    fn json_content_carries_parse_error_for_the_banner() {
        match decode_text_content(PreviewCategory::Json, "oops".into()) {
            DecodedContent::Json { text, parse_error } => {
                assert_eq!(text, "oops");
                assert!(parse_error.is_some());
            }
            other => panic!("expected Json content, got {other:?}"),
        }
    }

    #[test]
    fn delimited_parsing_handles_quotes_and_embedded_delimiters() {
        let (records, truncated) = parse_delimited("a,b\n\"x,y\",\"line\nbreak\"\n1,2\n", false);
        assert_eq!(records[0], vec!["a", "b"]);
        assert_eq!(records[1], vec!["x,y", "line\nbreak"]);
        assert_eq!(records[2], vec!["1", "2"]);
        assert!(!truncated);
    }

    #[test]
    fn tsv_parsing_splits_on_tabs() {
        let (records, _) = parse_delimited("a\tb\n1\t2\n", true);
        assert_eq!(records[0], vec!["a", "b"]);
        assert_eq!(records[1], vec!["1", "2"]);
    }

    #[test]
    fn text_categories_map_to_matching_content_variants() {
        assert!(matches!(
            decode_text_content(PreviewCategory::Text, "hi".into()),
            DecodedContent::Text(text) if text == "hi"
        ));
        assert!(matches!(
            decode_text_content(PreviewCategory::Markdown, "# h".into()),
            DecodedContent::Markdown(_)
        ));
        assert!(matches!(
            decode_text_content(PreviewCategory::Json, "{\"a\":1}".into()),
            DecodedContent::Json {
                parse_error: None,
                ..
            }
        ));
        assert!(matches!(
            decode_text_content(PreviewCategory::Delimited { tab: false }, "a,b\n1,2".into()),
            DecodedContent::Delimited(_)
        ));
    }

    #[test]
    fn pixel_budget_rejects_compression_bombs() {
        // Within budget.
        assert!(check_pixel_budget(100, 100, IMAGE_MAX_DIMENSION, IMAGE_MAX_PIXELS).is_ok());
        // Zero dimension.
        assert!(check_pixel_budget(0, 100, IMAGE_MAX_DIMENSION, IMAGE_MAX_PIXELS).is_err());
        // Over the per-side cap.
        assert!(
            check_pixel_budget(
                IMAGE_MAX_DIMENSION + 1,
                1,
                IMAGE_MAX_DIMENSION,
                IMAGE_MAX_PIXELS
            )
            .is_err()
        );
        // Within per-side caps but over the total-pixel budget.
        assert!(check_pixel_budget(19_000, 19_000, IMAGE_MAX_DIMENSION, IMAGE_MAX_PIXELS).is_err());
    }
}

#[cfg(test)]
mod binary_tests {
    use super::{decode_binary_content, rasterize_pdf_page};
    use crate::content::DecodedContent;
    use nyaterm_core::PreviewCategory;
    use std::io::Cursor;

    #[test]
    fn decoded_image_preserves_dimensions_and_bgra_pixels() {
        let source =
            image::RgbaImage::from_raw(2, 1, vec![255, 0, 0, 255, 0, 255, 0, 128]).unwrap();
        let mut bytes = Cursor::new(Vec::new());
        source
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        let DecodedContent::Image(image) =
            decode_binary_content(PreviewCategory::Image, bytes.into_inner())
        else {
            panic!("expected image pixels");
        };
        assert_eq!((image.width, image.height), (2, 1));
        assert_eq!(image.pixels.as_slice(), &[0, 0, 255, 255, 0, 255, 0, 128]);
    }

    fn pdf_fixture() -> Vec<u8> {
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 20 10] /Resources << >> /Contents 4 0 R >>".to_string(),
            {
                let drawing = "1 0 0 rg 0 0 20 10 re f\n";
                format!("<< /Length {} >>\nstream\n{drawing}endstream", drawing.len())
            },
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 20 10] /Resources << >> /Contents 4 0 R >>".to_string(),
        ];
        let mut bytes = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (index, object) in objects.iter().enumerate() {
            offsets.push(bytes.len());
            bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
        }
        let xref = bytes.len();
        bytes.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
        );
        for offset in offsets {
            bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        bytes.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objects.len() + 1
            )
            .as_bytes(),
        );
        bytes
    }

    #[test]
    fn pdf_metadata_and_on_demand_page_rasterization_preserve_all_pages() {
        let DecodedContent::Pdf(document) =
            decode_binary_content(PreviewCategory::Pdf, pdf_fixture())
        else {
            panic!("expected PDF metadata");
        };
        assert_eq!(document.page_count, 2);
        let image = rasterize_pdf_page(&document.bytes, 1).unwrap();
        assert_eq!((image.width, image.height), (30, 15));
        assert_eq!(image.pixels.len(), 30 * 15 * 4);
        let center = (7 * 30 + 15) * 4;
        assert_eq!(&image.pixels[center..center + 4], &[0, 0, 255, 255]);
        assert!(rasterize_pdf_page(&document.bytes, 2).is_err());
    }

    #[test]
    fn malformed_binary_data_becomes_an_error_without_panicking() {
        for category in [PreviewCategory::Pdf, PreviewCategory::Image] {
            assert!(matches!(
                decode_binary_content(category, b"malformed".to_vec()),
                DecodedContent::Error(_)
            ));
        }
    }
}
