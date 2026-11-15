use ffmpeg_next::{
    codec,
    software::scaling,
    util::{format::pixel::Pixel, frame::Video},
};
use nyaterm_android::ipc::{
    ColorMatrix, Frame, FrameColor, MAX_CAPTURE_EDGE, PixelFormat, presentation_size, valid_capture,
};
use std::sync::Arc;

// SPS dimensions can disagree with scrcpy's session header. Enforce the edge
// limit before FFmpeg allocates a decoded surface, including on codec threads.
unsafe extern "C" fn bounded_buffer(
    context: *mut ffmpeg_next::ffi::AVCodecContext,
    frame: *mut ffmpeg_next::ffi::AVFrame,
    flags: std::ffi::c_int,
) -> std::ffi::c_int {
    if frame.is_null() || context.is_null() {
        return ffmpeg_next::ffi::AVERROR_INVALIDDATA;
    }
    // SAFETY: FFmpeg passes live codec/frame pointers for the duration of this callback.
    let (width, height) = unsafe { ((*frame).width, (*frame).height) };
    if width <= 0 || height <= 0 || !valid_capture(width as u32, height as u32) {
        return ffmpeg_next::ffi::AVERROR_INVALIDDATA;
    }
    // SAFETY: validated pointers and dimensions; default allocation retains FFmpeg ownership.
    unsafe { ffmpeg_next::ffi::avcodec_default_get_buffer2(context, frame, flags) }
}

pub struct Decoded {
    pub video: Arc<Video>,
    pub serial: u64,
    pub pts: u64,
    pub generation: u64,
}

pub struct Decoder {
    decoder: codec::decoder::Video,
    serial: u64,
}
impl Decoder {
    pub fn new() -> Result<Self, ffmpeg_next::Error> {
        let codec =
            codec::decoder::find(codec::Id::H264).ok_or(ffmpeg_next::Error::DecoderNotFound)?;
        let mut context = codec::context::Context::new_with_codec(codec);
        // Bound SPS-controlled native allocation before opening the decoder.
        // SAFETY: this context is uniquely owned, live, and not yet opened.
        unsafe {
            (*context.as_mut_ptr()).max_pixels = i64::from(MAX_CAPTURE_EDGE).pow(2);
            (*context.as_mut_ptr()).get_buffer2 = Some(bounded_buffer);
        }
        context.set_flags(codec::Flags::LOW_DELAY);
        context.set_threading(codec::threading::Config {
            kind: codec::threading::Type::Slice,
            count: 2,
        });
        Ok(Self {
            decoder: context.decoder().video()?,
            serial: 0,
        })
    }
    pub fn packet(
        &mut self,
        bytes: &[u8],
        pts: u64,
        generation: u64,
    ) -> Result<Option<Decoded>, ffmpeg_next::Error> {
        let mut packet = ffmpeg_next::Packet::copy(bytes);
        packet.set_pts(Some(pts as i64));
        self.decoder.send_packet(&packet)?;
        let mut latest = None;
        loop {
            let mut decoded = Video::empty();
            match self.decoder.receive_frame(&mut decoded) {
                Ok(()) => {}
                Err(ffmpeg_next::Error::Other {
                    errno: ffmpeg_next::util::error::EAGAIN,
                })
                | Err(ffmpeg_next::Error::Eof) => break,
                Err(error) => return Err(error),
            }
            if !valid_capture(decoded.width(), decoded.height()) {
                return Err(ffmpeg_next::Error::InvalidData);
            }
            self.serial = self.serial.wrapping_add(1).max(1);
            latest = Some(Decoded {
                pts: decoded.pts().map(|p| p as u64).unwrap_or(pts),
                video: Arc::new(decoded),
                serial: self.serial,
                generation,
            });
        }
        Ok(latest)
    }
}

/// Created and used only on the presentation thread; Video::clone is a deep copy.
pub struct Presenter {
    scaler: Option<scaling::Context>,
    signature: Option<(
        Pixel,
        u32,
        u32,
        PixelFormat,
        FrameColor,
        ffmpeg_next::util::color::Space,
    )>,
    pub format: PixelFormat,
    bgra: Video,
    #[cfg(test)]
    pub conversions: usize,
}
impl Presenter {
    pub fn new() -> Self {
        Self {
            scaler: None,
            signature: None,
            format: PixelFormat::Bgra,
            bgra: Video::empty(),
            #[cfg(test)]
            conversions: 0,
        }
    }
    pub fn present(
        &mut self,
        decoded: &Decoded,
        binding_generation: u64,
        mut pixels: Vec<u8>,
    ) -> Result<Frame, ffmpeg_next::Error> {
        let video = &decoded.video;
        let (capture_width, capture_height) = (video.width(), video.height());
        let (width, height) = presentation_size(capture_width, capture_height)
            .ok_or(ffmpeg_next::Error::InvalidData)?;
        use ffmpeg_next::util::color::{Range, Space};
        let matrix = match video.color_space() {
            Space::BT709 => ColorMatrix::Bt709,
            Space::BT2020NCL => ColorMatrix::Bt2020,
            _ => ColorMatrix::Bt601,
        };
        let color = FrameColor {
            matrix,
            full_range: video.color_range() == Range::JPEG || video.format() == Pixel::YUVJ420P,
        };
        let supported_color = matches!(
            video.color_space(),
            Space::Unspecified
                | Space::BT470BG
                | Space::SMPTE170M
                | Space::BT709
                | Space::BT2020NCL
        );
        let format = if self.format == PixelFormat::I420
            && supported_color
            && matches!(video.format(), Pixel::YUV420P | Pixel::YUVJ420P)
        {
            PixelFormat::I420
        } else {
            PixelFormat::Bgra
        };
        let signature = (
            video.format(),
            capture_width,
            capture_height,
            format,
            color,
            video.color_space(),
        );
        let direct =
            format == PixelFormat::I420 && (width, height) == (capture_width, capture_height);
        if !direct {
            if self.signature != Some(signature) {
                let destination = if format == PixelFormat::I420 {
                    Pixel::YUV420P
                } else {
                    Pixel::BGRA
                };
                let mut scaler = scaling::Context::get(
                    video.format(),
                    capture_width,
                    capture_height,
                    destination,
                    width,
                    height,
                    scaling::Flags::BILINEAR,
                )?;
                // swscale defaults do not inherit AVFrame color metadata. Set both
                // range and coefficients explicitly for parity with GPU conversion.
                unsafe {
                    let space = match video.color_space() {
                        Space::BT709 => ffmpeg_next::ffi::SWS_CS_ITU709,
                        Space::BT2020NCL | Space::BT2020CL => ffmpeg_next::ffi::SWS_CS_BT2020,
                        Space::FCC => ffmpeg_next::ffi::SWS_CS_FCC,
                        Space::SMPTE240M => ffmpeg_next::ffi::SWS_CS_SMPTE240M,
                        _ => ffmpeg_next::ffi::SWS_CS_ITU601,
                    };
                    let coefficients = ffmpeg_next::ffi::sws_getCoefficients(space);
                    let result = ffmpeg_next::ffi::sws_setColorspaceDetails(
                        scaler.as_mut_ptr(),
                        coefficients,
                        i32::from(color.full_range),
                        coefficients,
                        i32::from(format == PixelFormat::Bgra || color.full_range),
                        0,
                        1 << 16,
                        1 << 16,
                    );
                    if result < 0 {
                        return Err(ffmpeg_next::Error::InvalidData);
                    }
                }
                self.scaler = Some(scaler);
                self.signature = Some(signature);
                self.bgra = Video::empty();
            }
            self.scaler
                .as_mut()
                .ok_or(ffmpeg_next::Error::InvalidData)?
                .run(video, &mut self.bgra)?;
            #[cfg(test)]
            {
                self.conversions += 1;
            }
        }
        let source = if direct { video.as_ref() } else { &self.bgra };
        let (stride, length) = format
            .layout(width, height)
            .ok_or(ffmpeg_next::Error::InvalidData)?;
        pixels.clear();
        pixels.reserve(length);
        let planes = if format == PixelFormat::I420 { 3 } else { 1 };
        for plane in 0..planes {
            let (row_width, rows) = if plane == 0 {
                (stride as usize, height as usize)
            } else {
                (width.div_ceil(2) as usize, height.div_ceil(2) as usize)
            };
            for row in source.data(plane).chunks(source.stride(plane)).take(rows) {
                pixels.extend_from_slice(
                    row.get(..row_width)
                        .ok_or(ffmpeg_next::Error::InvalidData)?,
                );
            }
        }
        Ok(Frame {
            sequence: 0,
            binding_generation,
            generation: decoded.generation,
            pts: decoded.pts,
            capture_width,
            capture_height,
            width,
            height,
            stride,
            format,
            color,
            pixels,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::decoder::{Decoded, Decoder, Presenter};
    use ffmpeg_next::util::{format::pixel::Pixel, frame::Video};
    use std::sync::Arc;
    #[test]
    fn direct_i420_preserves_color_and_odd_planes_without_conversion() {
        use nyaterm_android::ipc::{ColorMatrix, PixelFormat};
        let mut video = Video::new(Pixel::YUV420P, 5, 3);
        video.set_color_space(ffmpeg_next::util::color::Space::BT709);
        video.set_color_range(ffmpeg_next::util::color::Range::JPEG);
        for plane in 0..3 {
            video.data_mut(plane).fill(50 + plane as u8);
        }
        let decoded = Decoded {
            video: Arc::new(video),
            serial: 1,
            pts: 1,
            generation: 1,
        };
        let mut presenter = Presenter::new();
        presenter.format = PixelFormat::I420;
        let frame = presenter.present(&decoded, 1, Vec::new()).unwrap();
        assert_eq!(frame.format, PixelFormat::I420);
        assert_eq!(frame.pixels.len(), 27);
        assert_eq!(&frame.pixels[..15], &[50; 15]);
        assert_eq!(&frame.pixels[15..21], &[51; 6]);
        assert_eq!(frame.color.matrix, ColorMatrix::Bt709);
        assert!(frame.color.full_range);
        assert_eq!(presenter.conversions, 0);
        presenter.format = PixelFormat::Bgra;
        let frame = presenter.present(&decoded, 1, frame.pixels).unwrap();
        assert_eq!(frame.format, PixelFormat::Bgra);
        assert_eq!(frame.pixels.len(), 60);
        assert_eq!(presenter.conversions, 1);
    }
    #[test]
    fn sps_edge_limit_is_enforced_even_when_total_pixels_are_small() {
        ffmpeg_next::init().unwrap();
        let mut decoder = Decoder::new().unwrap();
        assert!(
            decoder
                .packet(
                    include_bytes!("../tests/fixtures/oversized-edge.h264"),
                    1,
                    1
                )
                .is_err()
        );
    }
    #[test]
    fn large_captures_convert_inside_ipc_limit_and_preserve_full_touch_geometry() {
        ffmpeg_next::init().unwrap();
        let mut presenter = Presenter::new();
        for (width, height) in [(2560, 1920), (2560, 2560), (1920, 2560)] {
            let mut video = Video::new(Pixel::YUV420P, width, height);
            for plane in 0..3 {
                video.data_mut(plane).fill(128);
            }
            let decoded = Decoded {
                video: Arc::new(video),
                serial: 1,
                pts: 1,
                generation: 1,
            };
            let mut frame = presenter.present(&decoded, 1, Vec::new()).unwrap();
            frame.sequence = 1;
            frame.validate().unwrap();
            assert_eq!((frame.capture_width, frame.capture_height), (width, height));
            assert!(frame.pixels.len() <= nyaterm_android::ipc::MAX_FRAME_BYTES);
            assert!(frame.width <= width && frame.height <= height);
        }
    }
}
