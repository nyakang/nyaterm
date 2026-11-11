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
    latest: Option<Video>,
    bgra: Video,
    serial: u64,
    pts: u64,
    #[cfg(test)]
    pub conversions: usize,
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
            latest: None,
            bgra: Video::empty(),
            serial: 0,
            pts: 0,
            #[cfg(test)]
            conversions: 0,
        })
    }

    pub fn packet(&mut self, bytes: &[u8], pts: u64) -> Result<(), ffmpeg_next::Error> {
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
            self.pts = decoded.pts().map(|value| value as u64).unwrap_or(pts);
            self.serial = self.serial.wrapping_add(1).max(1);
            self.latest = Some(decoded);
            decoded = Video::empty();
        }
        Ok(())
    }
    pub fn serial(&self) -> u64 {
        self.serial
    }
    /// Decode every media packet, but convert only when the consumer can present.
    pub fn present(
        &mut self,
        binding_generation: u64,
        generation: u64,
        mut pixels: Vec<u8>,
    ) -> Result<Option<Frame>, ffmpeg_next::Error> {
        let Some(decoded) = self.latest.as_ref() else {
            return Ok(None);
        };
        let (width, height) = (decoded.width(), decoded.height());
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
            self.bgra = Video::empty();
        }
        self.scaler
            .as_mut()
            .ok_or(ffmpeg_next::Error::InvalidData)?
            .run(decoded, &mut self.bgra)?;
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
        Ok(Some(Frame {
            sequence: 0,
            binding_generation,
            generation,
            pts: self.pts,
            width,
            height,
            stride: stride as u32,
            pixels,
        }))
    }
}
