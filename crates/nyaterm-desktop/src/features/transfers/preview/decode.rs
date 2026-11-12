//! Convert UI-independent preview decoding results to GPUI presentation.
//!
//! These adapters are called by background loaders. Window state and PDF page
//! caches stay in desktop; parsing and rasterization live in nyaterm-preview.
use crate::image_presentation::render_image_from_bgra;
use crate::models::{
    PreviewContent, PreviewDelimited, PreviewImage, PreviewPdfDocument, PreviewPdfPage,
};
use gpui::RenderImage;
use nyaterm_core::PreviewCategory;
use nyaterm_preview::content::{DecodedContent, RasterImage};
use nyaterm_preview::decode;
use std::sync::Arc;

pub(in crate::features) fn decode_binary_content(
    category: PreviewCategory,
    bytes: Vec<u8>,
) -> PreviewContent {
    presentation_content(decode::decode_binary_content(category, bytes))
}

pub(in crate::features) fn decode_text_content(
    category: PreviewCategory,
    text: String,
) -> PreviewContent {
    presentation_content(decode::decode_text_content(category, text))
}

fn presentation_content(content: DecodedContent) -> PreviewContent {
    match content {
        DecodedContent::Text(text) => PreviewContent::Text(text),
        DecodedContent::Json { text, parse_error } => PreviewContent::Json { text, parse_error },
        DecodedContent::Markdown(text) => PreviewContent::Markdown(text),
        DecodedContent::Delimited(data) => {
            PreviewContent::Delimited(PreviewDelimited::new(data.records, data.truncated))
        }
        DecodedContent::Image(raster) => {
            match render_image_from_bgra(raster.width, raster.height, &raster.pixels) {
                Ok(image) => PreviewContent::Image(PreviewImage {
                    image,
                    pixels: raster.pixels,
                    src_width: raster.width,
                    src_height: raster.height,
                    width: raster.width,
                    height: raster.height,
                }),
                Err(error) => PreviewContent::Error(error),
            }
        }
        DecodedContent::Pdf(document) => {
            PreviewContent::Pdf(PreviewPdfDocument::new(document.bytes, document.page_count))
        }
        DecodedContent::Error(error) => PreviewContent::Error(error),
    }
}

pub(in crate::features) fn rasterize_pdf_page(
    bytes: &Arc<Vec<u8>>,
    page_index: usize,
) -> Result<PreviewPdfPage, String> {
    let RasterImage {
        pixels,
        width,
        height,
    } = decode::rasterize_pdf_page(bytes, page_index)?;
    let image = render_image_from_bgra(width, height, &pixels)?;
    Ok(PreviewPdfPage {
        image,
        pixels,
        src_width: width,
        src_height: height,
        width,
        height,
    })
}

pub(in crate::features) fn rotated_render_image(
    pixels: &[u8],
    width: u32,
    height: u32,
    quarter_turns: u8,
) -> Option<(Arc<RenderImage>, u32, u32)> {
    let (rotated, width, height) = decode::rotate_bgra(pixels, width, height, quarter_turns);
    render_image_from_bgra(width, height, &rotated)
        .ok()
        .map(|image| (image, width, height))
}
