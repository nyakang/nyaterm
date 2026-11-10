use ffmpeg_next::{
    codec,
    software::scaling,
    util::{format::pixel::Pixel, frame::Video},
};
use nyaterm_android::ipc::{Frame, MAX_FRAME_BYTES};

pub struct Decoder {
    decoder: codec::decoder::Video,
    scaler: Option<scaling::Context>,
    scale_input: Option<(Pixel, u32, u32)>,
}

impl Decoder {
    pub fn new() -> Result<Self, ffmpeg_next::Error> {
        let codec =
            codec::decoder::find(codec::Id::H264).ok_or(ffmpeg_next::Error::DecoderNotFound)?;
        let mut context = codec::context::Context::new_with_codec(codec);
        // SPS dimensions are controlled by the media stream independently of
        // the Session header. Bound native allocation before decoding a frame.
        // SAFETY: this uniquely owned context is live and has not been opened.
        unsafe {
            (*context.as_mut_ptr()).max_pixels = (MAX_FRAME_BYTES / 4) as i64;
        }
        context.set_flags(codec::Flags::LOW_DELAY);
        context.set_threading(codec::threading::Config {
            kind: codec::threading::Type::Slice,
            count: 2,
        });
        Ok(Self {
            decoder: context.decoder().video()?,
            scaler: None,
            scale_input: None,
        })
    }

    pub fn packet(
        &mut self,
        bytes: &[u8],
        pts: u64,
        binding_generation: u64,
        generation: u64,
        mut emit: impl FnMut(Frame),
    ) -> Result<(), ffmpeg_next::Error> {
        let mut packet = ffmpeg_next::Packet::copy(bytes);
        packet.set_pts(Some(pts as i64));
        self.decoder.send_packet(&packet)?;
        let mut decoded = Video::empty();
        loop {
            match self.decoder.receive_frame(&mut decoded) {
                Ok(()) => {}
                Err(ffmpeg_next::Error::Other {
                    errno: ffmpeg_next::util::error::EAGAIN,
                }) => break,
                Err(ffmpeg_next::Error::Eof) => break,
                Err(error) => return Err(error),
            }
            let width = decoded.width();
            let height = decoded.height();
            if width == 0
                || height == 0
                || width > 4096
                || height > 4096
                || u64::from(width) * u64::from(height) * 4 > MAX_FRAME_BYTES as u64
            {
                return Err(ffmpeg_next::Error::InvalidData);
            }
            let signature = (decoded.format(), width, height);
            if self.scale_input != Some(signature) {
                self.scaler = Some(scaling::Context::get(
                    decoded.format(),
                    width,
                    height,
                    Pixel::BGRA,
                    width,
                    height,
                    scaling::Flags::FAST_BILINEAR,
                )?);
                self.scale_input = Some(signature);
            }
            let mut bgra = Video::empty();
            self.scaler
                .as_mut()
                .ok_or(ffmpeg_next::Error::InvalidData)?
                .run(&decoded, &mut bgra)?;
            let stride = width as usize * 4;
            let mut pixels = Vec::with_capacity(stride * height as usize);
            for row in bgra.data(0).chunks(bgra.stride(0)).take(height as usize) {
                pixels.extend_from_slice(&row[..stride]);
            }
            emit(Frame {
                sequence: 0,
                binding_generation,
                generation,
                pts: decoded.pts().map(|value| value as u64).unwrap_or(pts),
                width,
                height,
                stride: stride as u32,
                pixels,
            });
        }
        Ok(())
    }
}
