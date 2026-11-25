use std::time::Instant;

use crate::frame::{DirtyRect, Framebuffer, FramebufferError, VNC_FRAMEBUFFER_LIMITS};
use crate::pixel_memory::{PixelBudget, PixelBuffer};
use crate::protocol::{RemoteFrameEvent, VncError, VncErrorKind};

use super::{VncPresentation, VncPresentationId, VncUploadRegion};

const TILE_SIZE: u32 = 64;
const MAX_UPLOAD_REGIONS: usize = 64;

pub(crate) struct Compositor {
    framebuffer: Option<Framebuffer>,
    limits: crate::FramebufferLimits,
    changed: Vec<u64>,
    columns: u32,
    revision: u64,
    completed_revision: u64,
    update_sequence: u64,
    complete: bool,
    full_required: bool,
    generation: u64,
    completed_at: Instant,
}

impl Default for Compositor {
    fn default() -> Self {
        Self {
            framebuffer: None,
            limits: VNC_FRAMEBUFFER_LIMITS,
            changed: Vec::new(),
            columns: 0,
            revision: 0,
            completed_revision: 0,
            update_sequence: 0,
            complete: false,
            full_required: true,
            generation: 0,
            completed_at: Instant::now(),
        }
    }
}

impl Compositor {
    pub(crate) fn with_limits(limits: crate::FramebufferLimits) -> Self {
        Self {
            limits,
            ..Self::default()
        }
    }

    pub(crate) fn bounds(&self) -> Option<DirtyRect> {
        self.framebuffer.as_ref().map(|fb| DirtyRect {
            x: 0,
            y: 0,
            width: fb.width(),
            height: fb.height(),
        })
    }

    pub(crate) fn reset(&mut self, epoch: u64, width: u32, height: u32) -> Result<(), VncError> {
        // Release the old allocation before reserving the replacement quota.
        self.framebuffer = None;
        self.framebuffer =
            Some(Framebuffer::new(epoch, width, height, self.limits).map_err(frame_error)?);
        self.columns = width.div_ceil(TILE_SIZE);
        self.changed = vec![0; (self.columns * height.div_ceil(TILE_SIZE)) as usize];
        self.revision = 0;
        self.completed_revision = 0;
        self.update_sequence = 0;
        self.complete = false;
        self.full_required = true;
        Ok(())
    }

    pub(crate) fn apply(&mut self, frame: &RemoteFrameEvent) -> Result<(), VncError> {
        let Some(framebuffer) = self.framebuffer.as_mut() else {
            return Err(VncError::new(
                VncErrorKind::Protocol,
                "remote desktop pixels preceded a desktop reset",
            ));
        };
        let rect = match framebuffer.apply(frame) {
            Err(FramebufferError::StaleEpoch { .. }) => return Ok(()),
            other => other.map_err(frame_error)?,
        };
        let Some(rect) = rect else {
            return Ok(());
        };
        self.revision = self.revision.checked_add(1).ok_or_else(|| {
            VncError::new(
                VncErrorKind::Protocol,
                "remote desktop presentation sequence exhausted",
            )
        })?;
        for y in rect.y / TILE_SIZE..=(rect.y + rect.height - 1) / TILE_SIZE {
            for x in rect.x / TILE_SIZE..=(rect.x + rect.width - 1) / TILE_SIZE {
                self.changed[(y * self.columns + x) as usize] = self.revision;
            }
        }
        self.complete = false;
        Ok(())
    }

    pub(crate) fn complete(&mut self, epoch: u64, sequence: u64) -> Result<(), VncError> {
        let Some(framebuffer) = self.framebuffer.as_ref() else {
            return Err(VncError::new(
                VncErrorKind::Protocol,
                "remote desktop completion preceded a desktop reset",
            ));
        };
        if epoch != framebuffer.epoch() {
            return Ok(());
        }
        if sequence <= self.update_sequence {
            return Err(VncError::new(
                VncErrorKind::Protocol,
                "remote desktop update completion is out of order",
            ));
        }
        self.update_sequence = sequence;
        self.completed_revision = self.revision;
        self.completed_at = Instant::now();
        self.complete = true;
        Ok(())
    }

    pub(crate) fn set_generation(&mut self, generation: u64) {
        if self.generation != generation {
            self.generation = generation;
            self.full_required = true;
        }
    }

    pub(crate) fn acknowledge(&mut self, id: VncPresentationId) {
        if self
            .framebuffer
            .as_ref()
            .is_none_or(|fb| fb.epoch() != id.epoch)
            || id.generation != self.generation
            || id.sequence > self.completed_revision
        {
            return;
        }
        for changed in &mut self.changed {
            if *changed <= id.sequence {
                *changed = 0;
            }
        }
        self.full_required = false;
    }

    pub(crate) fn ready(&self) -> bool {
        self.complete && (self.full_required || self.changed.iter().any(|value| *value != 0))
    }

    pub(crate) fn id(&self) -> Option<VncPresentationId> {
        self.framebuffer.as_ref().map(|fb| VncPresentationId {
            epoch: fb.epoch(),
            generation: self.generation,
            sequence: self.completed_revision,
        })
    }

    pub(crate) fn prepare(&self) -> Result<VncPresentation, VncError> {
        self.prepare_crop(self.bounds().expect("prepare requires a ready compositor"))
    }

    pub(crate) fn prepare_crop(&self, crop: DirtyRect) -> Result<VncPresentation, VncError> {
        let fb = self
            .framebuffer
            .as_ref()
            .expect("prepare requires a ready compositor");
        if crop.width == 0
            || crop.height == 0
            || u64::from(crop.x) + u64::from(crop.width) > u64::from(fb.width())
            || u64::from(crop.y) + u64::from(crop.height) > u64::from(fb.height())
        {
            return Err(VncError::new(
                VncErrorKind::Protocol,
                "invalid presentation crop",
            ));
        }
        let mut rects: Vec<_> = tile_runs(&self.changed, self.columns, fb.width(), fb.height())
            .into_iter()
            .filter_map(|rect| {
                let x = rect.x.max(crop.x);
                let y = rect.y.max(crop.y);
                let right = (rect.x + rect.width).min(crop.x + crop.width);
                let bottom = (rect.y + rect.height).min(crop.y + crop.height);
                (right > x && bottom > y).then(|| DirtyRect {
                    x,
                    y,
                    width: right - x,
                    height: bottom - y,
                })
            })
            .collect();
        let area: u64 = rects
            .iter()
            .map(|r| u64::from(r.width) * u64::from(r.height))
            .sum();
        let full = self.full_required
            || rects.len() > MAX_UPLOAD_REGIONS
            || area * 100 >= u64::from(crop.width) * u64::from(crop.height) * 60;
        if full {
            rects = vec![crop];
        }
        let regions = rects
            .into_iter()
            .map(|rect| {
                let mut region = pack(fb, rect)?;
                region.rect.x -= crop.x;
                region.rect.y -= crop.y;
                Ok(region)
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(VncPresentation {
            id: self.id().expect("framebuffer is present"),
            width: crop.width,
            height: crop.height,
            full,
            regions,
            completed_at: self.completed_at,
            prepared_at: Instant::now(),
        })
    }
}

fn frame_error(error: FramebufferError) -> VncError {
    let kind = if matches!(error, FramebufferError::AllocationFailed { .. }) {
        VncErrorKind::ResourceExhausted
    } else {
        VncErrorKind::Protocol
    };
    VncError::new(kind, format!("remote desktop composition failed: {error}"))
}

fn pack(fb: &Framebuffer, rect: DirtyRect) -> Result<VncUploadRegion, VncError> {
    let bytes = rect.width as usize * rect.height as usize * 4;
    let lease = PixelBudget::shared()
        .reserve(bytes)
        .map_err(|error| VncError::new(VncErrorKind::ResourceExhausted, error.to_string()))?;
    let mut pixels = Vec::new();
    pixels.try_reserve_exact(bytes).map_err(|_| {
        VncError::new(
            VncErrorKind::ResourceExhausted,
            "failed to allocate remote desktop upload pixels",
        )
    })?;
    let stride = fb.width() as usize * 4;
    let row_bytes = rect.width as usize * 4;
    for y in rect.y..rect.y + rect.height {
        let start = y as usize * stride + rect.x as usize * 4;
        pixels.extend_from_slice(&fb.pixels()[start..start + row_bytes]);
    }
    Ok(VncUploadRegion {
        rect,
        pixels: PixelBuffer::with_lease(pixels, Some(std::sync::Arc::new(lease))),
    })
}

/// Merge equal horizontal runs vertically; no bounding-box union of sparse areas.
fn tile_runs(changed: &[u64], columns: u32, width: u32, height: u32) -> Vec<DirtyRect> {
    let mut result: Vec<DirtyRect> = Vec::new();
    let mut previous: Vec<(u32, u32, usize)> = Vec::new();
    for (y, row) in changed.chunks(columns as usize).enumerate() {
        let mut current = Vec::new();
        let mut x = 0;
        while x < columns {
            if row[x as usize] == 0 {
                x += 1;
                continue;
            }
            let start = x;
            while x < columns && row[x as usize] != 0 {
                x += 1;
            }
            let top = y as u32 * TILE_SIZE;
            let run_height = TILE_SIZE.min(height - top);
            let index = if let Some((_, _, index)) =
                previous.iter().find(|(a, b, _)| *a == start && *b == x)
            {
                result[*index].height += run_height;
                *index
            } else {
                result.push(DirtyRect {
                    x: start * TILE_SIZE,
                    y: top,
                    width: (x * TILE_SIZE).min(width) - start * TILE_SIZE,
                    height: run_height,
                });
                result.len() - 1
            };
            current.push((start, x, index));
        }
        previous = current;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::{Compositor, tile_runs};
    use crate::frame::DirtyRect;
    use crate::protocol::{PixelFormat, RemoteFrameEvent};

    fn pixel(epoch: u64, x: u32, y: u32, value: u8) -> RemoteFrameEvent {
        RemoteFrameEvent::Bitmap {
            epoch,
            full: false,
            x,
            y,
            width: 1,
            height: 1,
            stride: 4,
            format: PixelFormat::Bgra8,
            pixels: vec![value, 0, 0, 255].into(),
        }
    }

    #[test]
    fn replacement_covers_unacknowledged_changes_and_ack_preserves_newer_pixels() {
        let mut compositor = Compositor::default();
        compositor.reset(1, 256, 256).unwrap();
        compositor.set_generation(1);
        compositor.complete(1, 1).unwrap();
        compositor.acknowledge(compositor.prepare().unwrap().id);
        compositor.apply(&pixel(1, 0, 0, 1)).unwrap();
        compositor.complete(1, 2).unwrap();
        let old = compositor.prepare().unwrap();
        compositor.apply(&pixel(1, 0, 0, 2)).unwrap();
        compositor.apply(&pixel(1, 200, 200, 3)).unwrap();
        assert!(!compositor.ready(), "partial updates are never published");
        compositor.complete(1, 3).unwrap();
        compositor.acknowledge(old.id);
        let latest = compositor.prepare().unwrap();
        assert_eq!(latest.regions.len(), 2);
        assert_eq!(latest.regions[0].pixels[0], 2);
        compositor.acknowledge(latest.id);
        assert!(!compositor.ready());
    }

    #[test]
    fn visibility_generation_and_epoch_force_full_frames_and_ignore_old_acknowledgments() {
        let mut compositor = Compositor::default();
        compositor.reset(1, 65, 65).unwrap();
        compositor.set_generation(1);
        compositor.complete(1, 1).unwrap();
        let old = compositor.prepare().unwrap();
        compositor.acknowledge(old.id);
        compositor.set_generation(2);
        compositor.acknowledge(old.id);
        assert!(compositor.prepare().unwrap().full);
        compositor.reset(2, 65, 65).unwrap();
        compositor.apply(&pixel(1, 0, 0, 7)).unwrap();
        compositor.complete(2, 1).unwrap();
        compositor.acknowledge(old.id);
        let fresh = compositor.prepare().unwrap();
        assert!(fresh.full);
        assert_eq!(fresh.regions[0].pixels.len(), 65 * 65 * 4);
        assert_eq!(fresh.regions[0].pixels[0], 0);
        assert!(compositor.complete(2, 1).is_err());
    }

    #[test]
    fn dirty_tiles_merge_matching_runs_and_clip_edge_tiles_without_joining_sparse_regions() {
        let rects = tile_runs(&[1, 1, 0, 1, 1, 0, 0, 0, 1], 3, 130, 130);
        assert_eq!(
            rects,
            vec![
                DirtyRect {
                    x: 0,
                    y: 0,
                    width: 128,
                    height: 128
                },
                DirtyRect {
                    x: 128,
                    y: 128,
                    width: 2,
                    height: 2
                },
            ]
        );
    }

    #[test]
    fn fragmented_or_mostly_dirty_frames_fall_back_to_one_full_upload() {
        for (width, height, fragmented) in [(1024, 1024, true), (128, 128, false)] {
            let mut compositor = Compositor::default();
            compositor.reset(1, width, height).unwrap();
            compositor.complete(1, 1).unwrap();
            compositor.acknowledge(compositor.prepare().unwrap().id);
            for y in (0..height).step_by(64) {
                for x in (0..width).step_by(64) {
                    if !fragmented || (x / 64 + y / 64) % 2 == 0 {
                        compositor.apply(&pixel(1, x, y, 7)).unwrap();
                    }
                }
            }
            compositor.complete(1, 2).unwrap();
            let presentation = compositor.prepare().unwrap();
            assert!(presentation.full);
            assert_eq!(presentation.regions.len(), 1);
            assert_eq!(
                presentation.regions[0].pixels.len(),
                (width * height * 4) as usize
            );
        }
    }
}
