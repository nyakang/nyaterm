//! Full-size wallpaper pixels. Callers own I/O, scheduling and presentation.
use crate::content::RasterImage;
use std::io::{BufRead, Seek};
use std::sync::Arc;

#[derive(Debug)]
pub enum WallpaperDecodeError {
    Image(image::ImageError),
    EmptyImage,
}
impl std::fmt::Display for WallpaperDecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Image(error) => error.fmt(f),
            Self::EmptyImage => f.write_str("empty wallpaper image"),
        }
    }
}
impl std::error::Error for WallpaperDecodeError {}

pub fn decode_wallpaper(
    reader: impl BufRead + Seek,
    format_hint: Option<image::ImageFormat>,
) -> Result<RasterImage, WallpaperDecodeError> {
    let mut reader = image::ImageReader::new(reader);
    if let Some(format) = format_hint {
        reader.set_format(format);
    }
    let reader = reader
        .with_guessed_format()
        .map_err(|error| WallpaperDecodeError::Image(error.into()))?;
    let mut rgba = reader
        .decode()
        .map_err(WallpaperDecodeError::Image)?
        .into_rgba8();
    let (width, height) = rgba.dimensions();
    if width == 0 || height == 0 {
        return Err(WallpaperDecodeError::EmptyImage);
    }
    for pixel in rgba.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    Ok(RasterImage {
        pixels: Arc::new(rgba.into_raw()),
        width,
        height,
    })
}

#[cfg(test)]
mod tests {
    use super::decode_wallpaper;
    use std::io::Cursor;
    #[test]
    fn full_size_alpha_and_bgra_survive_format_guessing() {
        let mut bytes = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            257,
            133,
            image::Rgba([13, 37, 91, 123]),
        ))
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
        for hint in [None, Some(image::ImageFormat::Jpeg)] {
            let decoded = decode_wallpaper(Cursor::new(bytes.get_ref()), hint).unwrap();
            assert_eq!((decoded.width, decoded.height), (257, 133));
            assert!(
                decoded
                    .pixels
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .all(|pixel| *pixel == [91, 37, 13, 123])
            );
        }
        assert!(decode_wallpaper(Cursor::new(b"invalid"), None).is_err());
    }
}
