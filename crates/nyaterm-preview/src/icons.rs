//! Static icon decoding. Callers own file/data-URL handling and presentation.

use std::io::Cursor;
use std::sync::Arc;

use crate::content::RasterImage;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconDecodeError {
    InvalidFormat,
    UnsupportedFormat,
    InvalidOrOversizedImage,
}

impl std::fmt::Display for IconDecodeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidFormat => "invalid icon format",
            Self::UnsupportedFormat => "unsupported icon format",
            Self::InvalidOrOversizedImage => "invalid or oversized icon",
        })
    }
}

impl std::error::Error for IconDecodeError {}

/// MIME values also used by existing persisted icon data URLs.
pub fn detect_icon_mime(bytes: &[u8]) -> Result<&'static str, IconDecodeError> {
    match image::guess_format(bytes).map_err(|_| IconDecodeError::InvalidFormat)? {
        image::ImageFormat::Png => Ok("image/png"),
        image::ImageFormat::Jpeg => Ok("image/jpeg"),
        image::ImageFormat::Gif => Ok("image/gif"),
        image::ImageFormat::Bmp => Ok("image/bmp"),
        image::ImageFormat::WebP => Ok("image/webp"),
        _ => Err(IconDecodeError::UnsupportedFormat),
    }
}

/// Decode one frame with the historical icon limits and thumbnail algorithm.
/// Encoded byte limits belong to the caller so loading existing data URLs keeps
/// its separate compatibility policy from importing a new file.
pub fn decode_icon(bytes: &[u8]) -> Result<RasterImage, IconDecodeError> {
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| IconDecodeError::InvalidFormat)?;
    #[allow(clippy::field_reassign_with_default)]
    let limits = {
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(4096);
        limits.max_image_height = Some(4096);
        limits.max_alloc = Some(64 * 1024 * 1024);
        limits
    };
    reader.limits(limits);
    let mut pixels = reader
        .decode()
        .map_err(|_| IconDecodeError::InvalidOrOversizedImage)?
        .thumbnail(128, 128)
        .to_rgba8();
    let (width, height) = pixels.dimensions();
    for pixel in pixels.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Ok(RasterImage {
        pixels: Arc::new(pixels.into_raw()),
        width,
        height,
    })
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use image::{DynamicImage, ImageFormat, Rgb, RgbImage, Rgba, RgbaImage};

    use super::{IconDecodeError, decode_icon, detect_icon_mime};

    fn encode(image: DynamicImage, format: ImageFormat) -> Vec<u8> {
        let mut bytes = Cursor::new(Vec::new());
        image.write_to(&mut bytes, format).unwrap();
        bytes.into_inner()
    }

    #[test]
    fn supported_formats_keep_their_persisted_mime_and_decode() {
        for (format, mime) in [
            (ImageFormat::Png, "image/png"),
            (ImageFormat::Jpeg, "image/jpeg"),
            (ImageFormat::Gif, "image/gif"),
            (ImageFormat::Bmp, "image/bmp"),
            (ImageFormat::WebP, "image/webp"),
        ] {
            let bytes = encode(
                DynamicImage::ImageRgb8(RgbImage::from_pixel(128, 128, Rgb([20, 40, 60]))),
                format,
            );
            assert_eq!(detect_icon_mime(&bytes), Ok(mime));
            let raster = decode_icon(&bytes).unwrap();
            assert_eq!((raster.width, raster.height), (128, 128));
            assert_eq!(raster.pixels.len(), 128 * 128 * 4);
        }
    }

    #[test]
    fn thumbnail_preserves_aspect_ratio_alpha_and_bgra_channels() {
        for (width, height, expected) in [(256, 128, (128, 64)), (128, 256, (64, 128))] {
            let bytes = encode(
                DynamicImage::ImageRgba8(RgbaImage::from_pixel(
                    width,
                    height,
                    Rgba([11, 33, 77, 123]),
                )),
                ImageFormat::Png,
            );
            let raster = decode_icon(&bytes).unwrap();
            assert_eq!((raster.width, raster.height), expected);
            assert!(
                raster
                    .pixels
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .all(|pixel| pixel == &[77, 33, 11, 123])
            );
        }
    }

    #[test]
    fn animated_gif_uses_only_the_first_frame() {
        let mut bytes = Vec::new();
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut bytes);
            for color in [Rgba([255, 0, 0, 255]), Rgba([0, 255, 0, 255])] {
                encoder
                    .encode_frame(image::Frame::new(RgbaImage::from_pixel(128, 128, color)))
                    .unwrap();
            }
        }
        let raster = decode_icon(&bytes).unwrap();
        assert!(
            raster
                .pixels
                .as_chunks::<4>()
                .0
                .iter()
                .all(|pixel| pixel == &[0, 0, 255, 255])
        );
    }

    #[test]
    fn malformed_and_oversized_images_are_rejected() {
        assert_eq!(
            detect_icon_mime(b"invalid"),
            Err(IconDecodeError::InvalidFormat)
        );
        assert!(matches!(
            decode_icon(b"invalid"),
            Err(IconDecodeError::InvalidOrOversizedImage)
        ));
        assert_eq!(
            detect_icon_mime(b"II\x2a\x00\x08\x00\x00\x00"),
            Err(IconDecodeError::UnsupportedFormat)
        );
        for (width, height) in [(4097, 1), (1, 4097)] {
            let bytes = encode(DynamicImage::new_rgba8(width, height), ImageFormat::Png);
            assert!(matches!(
                decode_icon(&bytes),
                Err(IconDecodeError::InvalidOrOversizedImage)
            ));
        }
    }
}
