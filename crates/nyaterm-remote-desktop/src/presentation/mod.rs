//! Background-owned remote desktop composition and immutable, cumulative GPU uploads.
pub(crate) mod compositor;
mod worker;

use std::time::{Duration, Instant};

use crate::frame::DirtyRect;
use crate::pixel_memory::PixelBuffer;

pub(crate) use worker::{PresentationClient, PresentationWorker};

#[derive(Clone, Copy, Debug)]
pub struct RemotePresentationConfig {
    pub frame_interval: Duration,
}

impl Default for RemotePresentationConfig {
    fn default() -> Self {
        Self {
            frame_interval: Duration::from_millis(16),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RemotePresentationId {
    pub epoch: u64,
    pub generation: u64,
    /// Pixel revision at the last accepted update boundary, scoped to the epoch.
    pub sequence: u64,
}

#[derive(Clone, Debug)]
pub struct RemoteUploadRegion {
    pub rect: DirtyRect,
    pub pixels: PixelBuffer,
}

/// Every region changed since the last successful acknowledgment is included.
/// Replacing an unconsumed presentation therefore never drops a pixel delta.
#[derive(Clone, Debug)]
pub struct RemotePresentation {
    pub id: RemotePresentationId,
    pub width: u32,
    pub height: u32,
    pub full: bool,
    pub regions: Vec<RemoteUploadRegion>,
    pub completed_at: Instant,
    pub prepared_at: Instant,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RemotePresentationMetrics {
    pub rectangles: u64,
    pub completed_updates: u64,
    pub publications: u64,
    pub replaced_publications: u64,
    pub packed_bytes: u64,
    pub compose_nanoseconds: u64,
    pub ingress_items_high_water: usize,
    pub ingress_bytes_high_water: usize,
    pub ingress_blocked_nanoseconds: u64,
}

// Preserve protocol-specific APIs while sharing immutable upload contracts.
pub type RdpPresentationConfig = RemotePresentationConfig;
pub type RdpPresentationMetrics = RemotePresentationMetrics;
pub type RdpPresentationId = RemotePresentationId;
pub type RdpPresentation = RemotePresentation;
pub type VncPresentationConfig = RemotePresentationConfig;
pub type VncPresentationMetrics = RemotePresentationMetrics;
pub type VncPresentationId = RemotePresentationId;
pub type VncUploadRegion = RemoteUploadRegion;
pub type VncPresentation = RemotePresentation;
