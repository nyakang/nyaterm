//! First-grid QR decoding. No OTP interpretation, file I/O or UI state.
use std::io::{BufRead, Seek};

pub struct QrPayload(String);
impl QrPayload {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl std::fmt::Debug for QrPayload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("QrPayload([REDACTED])")
    }
}
#[derive(Debug)]
pub enum QrDecodeError {
    Image(image::ImageError),
    NotFound,
    Decode(rqrr::DeQRError),
}
impl std::fmt::Display for QrDecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Image(error) => write!(f, "failed to open image: {error}"),
            Self::NotFound => f.write_str("no QR code found in the image"),
            Self::Decode(error) => write!(f, "failed to decode QR code: {error}"),
        }
    }
}
impl std::error::Error for QrDecodeError {}

/// Uses the supplied extension hint without guessing, matching image::open.
/// Only the first detected grid is decoded, even if a later grid is valid.
pub fn decode_first_qr(
    reader: impl BufRead + Seek,
    format_hint: Option<image::ImageFormat>,
) -> Result<QrPayload, QrDecodeError> {
    let mut reader = image::ImageReader::new(reader);
    if let Some(format) = format_hint {
        reader.set_format(format);
    }
    let gray = reader.decode().map_err(QrDecodeError::Image)?.to_luma8();
    let mut prepared = rqrr::PreparedImage::prepare(gray);
    let grid = prepared
        .detect_grids()
        .into_iter()
        .next()
        .ok_or(QrDecodeError::NotFound)?;
    let (_, text) = grid.decode().map_err(QrDecodeError::Decode)?;
    Ok(QrPayload(text))
}

#[cfg(test)]
mod tests {
    use super::{QrDecodeError, decode_first_qr};
    use std::io::Cursor;
    fn qr(text: &str) -> image::GrayImage {
        let code = qrcode::QrCode::new(text).unwrap();
        let size = code.width();
        image::GrayImage::from_fn(((size + 8) * 6) as u32, ((size + 8) * 6) as u32, |x, y| {
            let x = x as usize / 6;
            let y = y as usize / 6;
            image::Luma([
                if x >= 4
                    && y >= 4
                    && x < size + 4
                    && y < size + 4
                    && code[(x - 4, y - 4)] == qrcode::Color::Dark
                {
                    0
                } else {
                    255
                },
            ])
        })
    }
    fn encoded(image: image::GrayImage) -> Vec<u8> {
        let mut bytes = Cursor::new(Vec::new());
        image::DynamicImage::ImageLuma8(image)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        bytes.into_inner()
    }
    #[test]
    fn payload_is_redacted_and_format_hint_is_required() {
        let bytes = encoded(qr("fixture-only-qr-content"));
        let payload = decode_first_qr(Cursor::new(&bytes), Some(image::ImageFormat::Png)).unwrap();
        assert_eq!(payload.as_str(), "fixture-only-qr-content");
        assert!(!format!("{payload:?}").contains(payload.as_str()));
        assert!(matches!(
            decode_first_qr(Cursor::new(&bytes), None),
            Err(QrDecodeError::Image(_))
        ));
        assert!(matches!(
            decode_first_qr(Cursor::new(b"bad"), Some(image::ImageFormat::Png)),
            Err(QrDecodeError::Image(_))
        ));
        let blank = encoded(image::GrayImage::from_pixel(100, 100, image::Luma([255])));
        assert!(matches!(
            decode_first_qr(Cursor::new(blank), Some(image::ImageFormat::Png)),
            Err(QrDecodeError::NotFound)
        ));
    }
    #[test]
    fn multiple_grids_use_the_first_detected_grid() {
        let first = qr("fixture-first");
        let second = qr("fixture-second");
        let mut combined = image::GrayImage::from_pixel(
            first.width() + second.width() + 40,
            first.height().max(second.height()),
            image::Luma([255]),
        );
        image::imageops::replace(&mut combined, &first, 0, 0);
        image::imageops::replace(&mut combined, &second, first.width() as i64 + 40, 0);
        let mut prepared = rqrr::PreparedImage::prepare(combined.clone());
        let grids = prepared.detect_grids();
        assert_eq!(grids.len(), 2);
        let expected = grids[0].decode().unwrap().1;
        assert_eq!(
            decode_first_qr(
                Cursor::new(encoded(combined)),
                Some(image::ImageFormat::Png)
            )
            .unwrap()
            .as_str(),
            expected
        );
    }
    #[test]
    fn damaged_grid_is_not_reported_as_a_successful_payload() {
        let fixture = "fixture-payload-with-enough-modules-to-damage".repeat(5);
        let mut damaged = qr(&fixture);
        // Keep finder patterns intact while destroying the data region.
        for y in 80..damaged.height() - 80 {
            for x in 80..damaged.width() - 80 {
                damaged.put_pixel(x, y, image::Luma([255]));
            }
        }
        assert!(matches!(
            decode_first_qr(
                Cursor::new(encoded(damaged.clone())),
                Some(image::ImageFormat::Png)
            ),
            Err(QrDecodeError::Decode(_))
        ));
        let valid = qr(&fixture);
        // Detection order belongs to rqrr. Arrange the fixtures so its first
        // grid is damaged and prove that a valid later grid is never substituted.
        let mut checked = false;
        for (left, right) in [(&damaged, &valid), (&valid, &damaged)] {
            let mut combined = image::GrayImage::from_pixel(
                left.width() + right.width() + 40,
                left.height().max(right.height()),
                image::Luma([255]),
            );
            image::imageops::replace(&mut combined, left, 0, 0);
            image::imageops::replace(&mut combined, right, left.width() as i64 + 40, 0);
            let mut prepared = rqrr::PreparedImage::prepare(combined.clone());
            let grids = prepared.detect_grids();
            if grids.len() == 2 && grids[0].decode().is_err() && grids[1].decode().is_ok() {
                assert!(matches!(
                    decode_first_qr(
                        Cursor::new(encoded(combined)),
                        Some(image::ImageFormat::Png)
                    ),
                    Err(QrDecodeError::Decode(_))
                ));
                checked = true;
            }
        }
        assert!(
            checked,
            "fixture must contain a damaged first grid and a valid second grid"
        );
    }
}
