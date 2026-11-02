use crate::{PixelFormat, RdpFrameEvent};

/// Keep delta order while bounding a UI turn. A single oversized full frame
/// still makes progress; subsequent deltas wait for the next turn.
pub(crate) fn frame_batch_len(frames: &std::collections::VecDeque<RdpFrameEvent>) -> usize {
    let mut bytes = 0usize;
    let mut count = 0;
    for frame in frames.iter().take(8) {
        let cost = match frame {
            RdpFrameEvent::Bitmap { pixels, .. } => pixels.len(),
            _ => 0,
        };
        if count > 0 && bytes.saturating_add(cost) > 8 * 1024 * 1024 {
            break;
        }
        count += 1;
        bytes = bytes.saturating_add(cost);
    }
    count
}

/// Resource limits applied before allocating a server-controlled framebuffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FramebufferLimits {
    pub max_width: u32,
    pub max_height: u32,
    pub max_bytes: usize,
}

pub const RDP_FRAMEBUFFER_LIMITS: FramebufferLimits = FramebufferLimits {
    max_width: 8192,
    max_height: 8192,
    max_bytes: 256 * 1024 * 1024,
};

pub const VNC_FRAMEBUFFER_LIMITS: FramebufferLimits = FramebufferLimits {
    max_width: 7680,
    max_height: 4320,
    max_bytes: 128 * 1024 * 1024,
};

pub fn validate_framebuffer_dimensions(
    width: u32,
    height: u32,
    limits: FramebufferLimits,
) -> Result<usize, FramebufferError> {
    if width == 0 || height == 0 || width > limits.max_width || height > limits.max_height {
        return Err(FramebufferError::DimensionsTooLarge { width, height });
    }
    let len = usize::try_from(width)
        .ok()
        .and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or(FramebufferError::SizeOverflow)?;
    if len > limits.max_bytes {
        return Err(FramebufferError::DimensionsTooLarge { width, height });
    }
    Ok(len)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DirtyRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl DirtyRect {
    fn right(self) -> u32 {
        self.x.saturating_add(self.width)
    }
    fn bottom(self) -> u32 {
        self.y.saturating_add(self.height)
    }
    fn intersects_or_touches(self, other: Self) -> bool {
        self.x <= other.right()
            && other.x <= self.right()
            && self.y <= other.bottom()
            && other.y <= self.bottom()
    }
    fn union(self, other: Self) -> Self {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        let right = self.right().max(other.right());
        let bottom = self.bottom().max(other.bottom());
        Self {
            x,
            y,
            width: right - x,
            height: bottom - y,
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum FramebufferError {
    #[error("frame epoch {actual} does not match framebuffer epoch {expected}")]
    StaleEpoch { expected: u64, actual: u64 },
    #[error("frame rectangle is outside the framebuffer")]
    OutOfBounds,
    #[error("frame stride or pixel payload is invalid")]
    InvalidPayload,
    #[error("frame dimensions overflow addressable memory")]
    SizeOverflow,
    #[error("framebuffer {width}x{height} exceeds the supported size")]
    DimensionsTooLarge { width: u32, height: u32 },
    #[error("failed to allocate {bytes} bytes for the framebuffer")]
    AllocationFailed { bytes: usize },
}

pub struct Framebuffer {
    width: u32,
    height: u32,
    epoch: u64,
    bgra: Vec<u8>,
    _lease: crate::pixel_memory::PixelLease,
}

impl std::fmt::Debug for Framebuffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Framebuffer")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("epoch", &self.epoch)
            .field("bytes", &self.bgra.len())
            .finish()
    }
}

impl Framebuffer {
    pub fn new(
        epoch: u64,
        width: u32,
        height: u32,
        limits: FramebufferLimits,
    ) -> Result<Self, FramebufferError> {
        let len = validate_framebuffer_dimensions(width, height, limits)?;
        // Fallible allocation: a server-driven reset must degrade to a typed
        // error, never abort the process the way `vec![0; len]` would on OOM.
        let lease = crate::pixel_memory::PixelBudget::shared()
            .reserve(len)
            .map_err(|_| FramebufferError::AllocationFailed { bytes: len })?;
        let mut bgra = Vec::new();
        bgra.try_reserve_exact(len)
            .map_err(|_| FramebufferError::AllocationFailed { bytes: len })?;
        bgra.resize(len, 0);
        Ok(Self {
            width,
            height,
            epoch,
            bgra,
            _lease: lease,
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }
    pub fn height(&self) -> u32 {
        self.height
    }
    pub fn epoch(&self) -> u64 {
        self.epoch
    }
    pub fn pixels(&self) -> &[u8] {
        &self.bgra
    }

    pub fn reset(
        &mut self,
        epoch: u64,
        width: u32,
        height: u32,
        limits: FramebufferLimits,
    ) -> Result<(), FramebufferError> {
        *self = Self::new(epoch, width, height, limits)?;
        Ok(())
    }

    pub fn apply(&mut self, frame: &RdpFrameEvent) -> Result<Option<DirtyRect>, FramebufferError> {
        let RdpFrameEvent::Bitmap {
            epoch,
            x,
            y,
            width,
            height,
            stride,
            format,
            pixels,
            ..
        } = frame
        else {
            return Ok(None);
        };
        if *epoch != self.epoch {
            return Err(FramebufferError::StaleEpoch {
                expected: self.epoch,
                actual: *epoch,
            });
        }
        if *width == 0
            || *height == 0
            || x.checked_add(*width).is_none_or(|right| right > self.width)
            || y.checked_add(*height)
                .is_none_or(|bottom| bottom > self.height)
        {
            return Err(FramebufferError::OutOfBounds);
        }
        let row_bytes = usize::try_from(*width)
            .ok()
            .and_then(|v| v.checked_mul(4))
            .ok_or(FramebufferError::InvalidPayload)?;
        let source_stride =
            usize::try_from(*stride).map_err(|_| FramebufferError::InvalidPayload)?;
        if source_stride < row_bytes {
            return Err(FramebufferError::InvalidPayload);
        }
        let required = usize::try_from(*height - 1)
            .ok()
            .and_then(|rows| rows.checked_mul(source_stride))
            .and_then(|offset| offset.checked_add(row_bytes))
            .ok_or(FramebufferError::InvalidPayload)?;
        if pixels.len() != required {
            return Err(FramebufferError::InvalidPayload);
        }
        let destination_stride = usize::try_from(self.width).unwrap() * 4;
        if matches!(format, PixelFormat::Bgra8)
            && source_stride == destination_stride
            && row_bytes == destination_stride
        {
            let start = usize::try_from(*y).unwrap() * destination_stride;
            self.bgra[start..start + required].copy_from_slice(&pixels[..required]);
        } else {
            for row in 0..usize::try_from(*height).unwrap() {
                let source = &pixels[row * source_stride..row * source_stride + row_bytes];
                let destination_offset = (usize::try_from(*y).unwrap() + row) * destination_stride
                    + usize::try_from(*x).unwrap() * 4;
                let destination =
                    &mut self.bgra[destination_offset..destination_offset + row_bytes];
                match format {
                    PixelFormat::Bgra8 => destination.copy_from_slice(source),
                    PixelFormat::Rgba8 => {
                        for (src, dst) in source
                            .as_chunks::<4>()
                            .0
                            .iter()
                            .zip(destination.as_chunks_mut::<4>().0.iter_mut())
                        {
                            dst.copy_from_slice(&[src[2], src[1], src[0], src[3]]);
                        }
                    }
                }
            }
        }
        Ok(Some(DirtyRect {
            x: *x,
            y: *y,
            width: *width,
            height: *height,
        }))
    }
}

pub fn merge_dirty_rects(rects: impl IntoIterator<Item = DirtyRect>) -> Vec<DirtyRect> {
    let mut merged: Vec<DirtyRect> = Vec::new();
    for mut rect in rects {
        let mut index = 0;
        while index < merged.len() {
            if rect.intersects_or_touches(merged[index]) {
                rect = rect.union(merged.swap_remove(index));
                index = 0;
            } else {
                index += 1;
            }
        }
        merged.push(rect);
    }
    merged
}

#[cfg(test)]
mod tests {
    use crate::{
        Framebuffer, FramebufferError, FramebufferLimits, PixelFormat, RDP_FRAMEBUFFER_LIMITS,
        RdpFrameEvent, VNC_FRAMEBUFFER_LIMITS, validate_framebuffer_dimensions,
    };

    fn bitmap(epoch: u64, x: u32, y: u32, pixels: Vec<u8>) -> RdpFrameEvent {
        RdpFrameEvent::Bitmap {
            epoch,
            full: false,
            x,
            y,
            width: 2,
            height: 2,
            stride: 8,
            format: PixelFormat::Bgra8,
            pixels: pixels.into(),
        }
    }

    #[test]
    fn applies_two_by_two_update_to_four_by_four_framebuffer() {
        let mut framebuffer = Framebuffer::new(3, 4, 4, RDP_FRAMEBUFFER_LIMITS).unwrap();
        let pixels = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
        framebuffer.apply(&bitmap(3, 1, 1, pixels)).unwrap();
        assert_eq!(&framebuffer.pixels()[20..28], &[1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(
            &framebuffer.pixels()[36..44],
            &[9, 10, 11, 12, 13, 14, 15, 16]
        );
        assert!(framebuffer.pixels()[0..20].iter().all(|byte| *byte == 0));
    }

    #[test]
    fn rejects_stale_epoch_and_bad_payload() {
        let mut framebuffer = Framebuffer::new(4, 4, 4, RDP_FRAMEBUFFER_LIMITS).unwrap();
        assert!(matches!(
            framebuffer.apply(&bitmap(3, 0, 0, vec![0; 16])),
            Err(FramebufferError::StaleEpoch { .. })
        ));
        assert_eq!(
            framebuffer.apply(&bitmap(4, 0, 0, vec![0; 15])),
            Err(FramebufferError::InvalidPayload)
        );
    }

    #[test]
    fn applies_a_full_width_update_as_one_contiguous_copy() {
        let mut framebuffer = Framebuffer::new(1, 4, 4, RDP_FRAMEBUFFER_LIMITS).unwrap();
        let frame = RdpFrameEvent::Bitmap {
            epoch: 1,
            full: false,
            x: 0,
            y: 1,
            width: 4,
            height: 2,
            stride: 16,
            format: PixelFormat::Bgra8,
            pixels: (1..=32).collect::<Vec<u8>>().into(),
        };

        framebuffer.apply(&frame).unwrap();

        assert_eq!(
            &framebuffer.pixels()[16..48],
            &(1..=32).collect::<Vec<u8>>()
        );
        assert!(framebuffer.pixels()[..16].iter().all(|byte| *byte == 0));
        assert!(framebuffer.pixels()[48..].iter().all(|byte| *byte == 0));
    }

    #[test]
    fn converts_rgba_to_bgra() {
        let mut framebuffer = Framebuffer::new(1, 2, 2, RDP_FRAMEBUFFER_LIMITS).unwrap();
        let frame = RdpFrameEvent::Bitmap {
            epoch: 1,
            full: true,
            x: 0,
            y: 0,
            width: 2,
            height: 2,
            stride: 8,
            format: PixelFormat::Rgba8,
            pixels: vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16].into(),
        };
        framebuffer.apply(&frame).unwrap();
        assert_eq!(&framebuffer.pixels()[0..4], &[3, 2, 1, 4]);
    }

    #[test]
    fn rejects_zero_and_oversized_dimensions() {
        assert_eq!(
            Framebuffer::new(1, 0, 4, RDP_FRAMEBUFFER_LIMITS).err(),
            Some(FramebufferError::DimensionsTooLarge {
                width: 0,
                height: 4
            })
        );
        assert_eq!(
            Framebuffer::new(1, 4, 0, RDP_FRAMEBUFFER_LIMITS).err(),
            Some(FramebufferError::DimensionsTooLarge {
                width: 4,
                height: 0
            })
        );
        let oversize = RDP_FRAMEBUFFER_LIMITS.max_width + 1;
        assert_eq!(
            Framebuffer::new(1, oversize, 4, RDP_FRAMEBUFFER_LIMITS).err(),
            Some(FramebufferError::DimensionsTooLarge {
                width: oversize,
                height: 4
            })
        );
        assert_eq!(
            Framebuffer::new(1, 4, oversize, RDP_FRAMEBUFFER_LIMITS).err(),
            Some(FramebufferError::DimensionsTooLarge {
                width: 4,
                height: oversize
            })
        );
    }

    #[test]
    fn rejects_total_byte_budget_even_when_edges_are_legal() {
        let limits = FramebufferLimits {
            max_width: 16,
            max_height: 16,
            max_bytes: 32,
        };
        assert_eq!(
            validate_framebuffer_dimensions(4, 4, limits),
            Err(FramebufferError::DimensionsTooLarge {
                width: 4,
                height: 4
            })
        );
    }

    #[test]
    fn allocates_a_large_but_legal_framebuffer() {
        // 4K fits comfortably below the byte budget and must allocate fallibly
        // without aborting.
        let framebuffer = Framebuffer::new(1, 3840, 2160, VNC_FRAMEBUFFER_LIMITS)
            .expect("4K framebuffer allocates");
        assert_eq!(framebuffer.pixels().len(), 3840 * 2160 * 4);
    }
}
