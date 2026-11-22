//! Bounded software-surface conversion and coalesced display damage.
//!
//! The GLib owner reads the current decoder surface only when an output slot is
//! available. Damage survives backpressure, so no dependent delta is dropped.

use nyaterm_remote_desktop::spice::display::SPICE_FRAMEBUFFER_LIMITS;
use nyaterm_remote_desktop::{
    DirtyRect, PixelFormat, RemoteFrameEvent, validate_framebuffer_dimensions,
};
use thiserror::Error;

mod convert;

const MAX_DAMAGE_RECTS: usize = 32;
pub const FRAME_CHUNK_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceFormat {
    Xrgb32,
    Argb32,
    Rgb555,
    Rgb565,
}

impl SurfaceFormat {
    pub fn from_spice(value: i32) -> Result<Self, SurfaceError> {
        // spice-protocol's public SpiceSurfaceFmt constants.
        match value {
            32 => Ok(Self::Xrgb32),
            96 => Ok(Self::Argb32),
            16 => Ok(Self::Rgb555),
            80 => Ok(Self::Rgb565),
            _ => Err(SurfaceError::UnsupportedFormat),
        }
    }

    fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Xrgb32 | Self::Argb32 => 4,
            Self::Rgb555 | Self::Rgb565 => 2,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SurfaceLayout {
    pub width: u32,
    pub height: u32,
    pub stride: i32,
    pub format: SurfaceFormat,
}

impl SurfaceLayout {
    pub fn source_bytes(self) -> Result<usize, SurfaceError> {
        validate_framebuffer_dimensions(self.width, self.height, SPICE_FRAMEBUFFER_LIMITS)
            .map_err(|_| SurfaceError::Dimensions)?;
        let stride = self.stride.unsigned_abs() as usize;
        let row = self.width as usize * self.format.bytes_per_pixel();
        if stride < row {
            return Err(SurfaceError::Stride);
        }
        let bytes = stride
            .checked_mul(self.height as usize - 1)
            .and_then(|n| n.checked_add(row))
            .ok_or(SurfaceError::Dimensions)?;
        if bytes > SPICE_FRAMEBUFFER_LIMITS.max_bytes {
            return Err(SurfaceError::Dimensions);
        }
        Ok(bytes)
    }

    /// Source starts at the lowest address. A negative stride reverses its row
    /// order; the native adapter performs that one pointer adjustment.
    pub fn bitmap(
        self,
        source: &[u8],
        rect: DirtyRect,
        epoch: u64,
    ) -> Result<RemoteFrameEvent, SurfaceError> {
        let source_bytes = self.source_bytes()?;
        if source.len() < source_bytes {
            return Err(SurfaceError::Truncated);
        }
        validate_rect(rect, self.width, self.height)?;
        let row_bytes = rect.width as usize * 4;
        let output_bytes = row_bytes
            .checked_mul(rect.height as usize)
            .ok_or(SurfaceError::Dimensions)?;
        let mut output = Vec::new();
        output
            .try_reserve_exact(output_bytes)
            .map_err(|_| SurfaceError::Allocation)?;
        let stride = self.stride.unsigned_abs() as usize;
        let pixel_bytes = self.format.bytes_per_pixel();
        for row in 0..rect.height as usize {
            let y = rect.y as usize + row;
            let source_y = if self.stride < 0 {
                self.height as usize - y - 1
            } else {
                y
            };
            let start = source_y * stride + rect.x as usize * pixel_bytes;
            let pixels = &source[start..start + rect.width as usize * pixel_bytes];
            match self.format {
                SurfaceFormat::Argb32 => output.extend_from_slice(pixels),
                SurfaceFormat::Xrgb32 => {
                    let destination_start = output.len();
                    // Initialize in one pass. Copying first and patching alpha
                    // afterwards rereads/writes the whole destination cache.
                    convert::opaque_bgra(pixels, &mut output.spare_capacity_mut()[..row_bytes]);
                    // SAFETY: exactly row_bytes reserved bytes were initialized
                    // above; each source pixel writes all four destination bytes.
                    unsafe {
                        output.set_len(destination_start + row_bytes);
                    }
                }
                SurfaceFormat::Rgb555 | SurfaceFormat::Rgb565 => {
                    let green_bits = if self.format == SurfaceFormat::Rgb565 {
                        6
                    } else {
                        5
                    };
                    let green_mask = (1 << green_bits) - 1;
                    for packed in pixels.as_chunks::<2>().0 {
                        let value = u16::from_le_bytes([packed[0], packed[1]]);
                        let blue = (value & 31) as u8;
                        let green = ((value >> 5) & green_mask) as u8;
                        let red = ((value >> (5 + green_bits)) & 31) as u8;
                        let green = if green_bits == 6 {
                            (green << 2) | (green >> 4)
                        } else {
                            (green << 3) | (green >> 2)
                        };
                        output.extend_from_slice(&[
                            (blue << 3) | (blue >> 2),
                            green,
                            (red << 3) | (red >> 2),
                            255,
                        ]);
                    }
                }
            }
        }
        Ok(RemoteFrameEvent::Bitmap {
            epoch,
            full: rect.x == 0
                && rect.y == 0
                && rect.width == self.width
                && rect.height == self.height,
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: rect.height,
            stride: row_bytes as u32,
            format: PixelFormat::Bgra8,
            pixels: output.into(),
        })
    }
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum SurfaceError {
    #[error("unsupported SPICE surface format")]
    UnsupportedFormat,
    #[error("SPICE surface exceeds the pixel budget")]
    Dimensions,
    #[error("invalid SPICE surface stride")]
    Stride,
    #[error("truncated SPICE surface pixels")]
    Truncated,
    #[error("invalid SPICE damage rectangle")]
    Rectangle,
    #[error("SPICE pixel allocation failed")]
    Allocation,
    #[error("SPICE output budget cannot fit one pixel row")]
    ChunkBudget,
}

fn validate_rect(rect: DirtyRect, width: u32, height: u32) -> Result<(), SurfaceError> {
    if rect.width == 0
        || rect.height == 0
        || rect
            .x
            .checked_add(rect.width)
            .is_none_or(|right| right > width)
        || rect
            .y
            .checked_add(rect.height)
            .is_none_or(|bottom| bottom > height)
    {
        return Err(SurfaceError::Rectangle);
    }
    Ok(())
}

pub struct DamageTracker {
    width: u32,
    height: u32,
    rects: Vec<DirtyRect>,
}

impl DamageTracker {
    pub fn new(width: u32, height: u32) -> Result<Self, SurfaceError> {
        validate_framebuffer_dimensions(width, height, SPICE_FRAMEBUFFER_LIMITS)
            .map_err(|_| SurfaceError::Dimensions)?;
        Ok(Self {
            width,
            height,
            rects: Vec::with_capacity(MAX_DAMAGE_RECTS),
        })
    }

    pub fn mark_full(&mut self) {
        self.rects.clear();
        self.rects.push(DirtyRect {
            x: 0,
            y: 0,
            width: self.width,
            height: self.height,
        });
    }

    pub fn mark(&mut self, mut rect: DirtyRect) -> Result<(), SurfaceError> {
        validate_rect(rect, self.width, self.height)?;
        // Merge in place; a busy decoder cannot grow a region list forever.
        let mut index = 0;
        while index < self.rects.len() {
            let other = self.rects[index];
            if rect.x <= other.x + other.width
                && other.x <= rect.x + rect.width
                && rect.y <= other.y + other.height
                && other.y <= rect.y + rect.height
            {
                let right = (rect.x + rect.width).max(other.x + other.width);
                let bottom = (rect.y + rect.height).max(other.y + other.height);
                rect.x = rect.x.min(other.x);
                rect.y = rect.y.min(other.y);
                rect.width = right - rect.x;
                rect.height = bottom - rect.y;
                self.rects.swap_remove(index);
                index = 0;
            } else {
                index += 1;
            }
        }
        if self.rects.len() == MAX_DAMAGE_RECTS {
            self.mark_full();
        } else {
            self.rects.push(rect);
        }
        Ok(())
    }

    /// On a failed enqueue, mark the returned rectangle again before pumping
    /// GLib. Its next snapshot will include all intervening decoder changes.
    pub fn take_chunk(&mut self, max_bytes: usize) -> Result<Option<DirtyRect>, SurfaceError> {
        let Some(rect) = self.rects.last_mut() else {
            return Ok(None);
        };
        let rows = max_bytes / (rect.width as usize * 4);
        if rows == 0 {
            return Err(SurfaceError::ChunkBudget);
        }
        let rows = rows.min(rect.height as usize) as u32;
        let chunk = DirtyRect {
            height: rows,
            ..*rect
        };
        rect.y += rows;
        rect.height -= rows;
        if rect.height == 0 {
            self.rects.pop();
        }
        Ok(Some(chunk))
    }

    pub fn is_empty(&self) -> bool {
        self.rects.is_empty()
    }
}

#[cfg(test)]
mod tests;
