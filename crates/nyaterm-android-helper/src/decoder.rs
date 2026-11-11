use ffmpeg_next::{
    codec,
    software::scaling,
    util::{format::pixel::Pixel, frame::Video},
};
use nyaterm_android::ipc::{Frame, MAX_CAPTURE_EDGE, presentation_size, valid_capture};
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
    signature: Option<(Pixel, u32, u32)>,
    bgra: Video,
    #[cfg(test)]
    pub conversions: usize,
}
impl Presenter {
    pub fn new() -> Self {
        Self {
            scaler: None,
            signature: None,
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
        let signature = (video.format(), capture_width, capture_height);
        if self.signature != Some(signature) {
            self.scaler = Some(scaling::Context::get(
                video.format(),
                capture_width,
                capture_height,
                Pixel::BGRA,
                width,
                height,
                scaling::Flags::FAST_BILINEAR,
            )?);
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
        let stride = width as usize * 4;
        pixels.clear();
        pixels.reserve(stride * height as usize);
        for row in self
            .bgra
            .data(0)
            .chunks(self.bgra.stride(0))
            .take(height as usize)
        {
            pixels.extend_from_slice(&row[..stride]);
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
            stride: stride as u32,
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
