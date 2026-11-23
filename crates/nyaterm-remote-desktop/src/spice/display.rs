use std::collections::HashSet;

use nyaterm_core::spice::settings::{MAX_SPICE_DIMENSION, MAX_SPICE_MONITORS};
use serde::{Deserialize, Serialize};

use crate::{FramebufferLimits, RemotePoint};

pub const SPICE_FRAMEBUFFER_LIMITS: FramebufferLimits = FramebufferLimits {
    max_width: MAX_SPICE_DIMENSION,
    max_height: MAX_SPICE_DIMENSION,
    max_bytes: 256 * 1024 * 1024,
};

/// Display channels can have one primary surface containing several monitors.
/// A monitor must not be mistaken for a separate decoder/framebuffer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SpiceDisplayId {
    pub channel: u32,
    pub monitor: u32,
}

impl SpiceDisplayId {
    /// SPICE's input/agent display ID: channel zero supports monitor IDs;
    /// the older multi-channel layout identifies a display by its channel.
    pub fn agent_id(self) -> Option<i32> {
        if self.channel > 255 || self.monitor > 255 {
            return None;
        }
        if self.channel == 0 {
            Some(self.monitor as i32)
        } else if self.monitor == 0 {
            Some(self.channel as i32)
        } else {
            None
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpiceMonitor {
    pub id: SpiceDisplayId,
    pub surface: u32,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl SpiceMonitor {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.id.agent_id().is_none() {
            return Err("invalid SPICE display identity");
        }
        if self.width == 0
            || self.height == 0
            || self
                .x
                .checked_add(self.width)
                .is_none_or(|v| v > MAX_SPICE_DIMENSION)
            || self
                .y
                .checked_add(self.height)
                .is_none_or(|v| v > MAX_SPICE_DIMENSION)
        {
            return Err("invalid SPICE monitor bounds");
        }
        Ok(())
    }

    /// Monitor-local input coordinates are independent of surface offsets.
    pub fn clamp_pointer(&self, position: RemotePoint) -> RemotePoint {
        RemotePoint {
            x: position.x.min(self.width.saturating_sub(1)),
            y: position.y.min(self.height.saturating_sub(1)),
        }
    }
}

pub fn validate_monitors(monitors: &[SpiceMonitor]) -> Result<(), &'static str> {
    if monitors.len() > MAX_SPICE_MONITORS {
        return Err("too many SPICE monitors");
    }
    let mut ids = HashSet::new();
    let mut agent_ids = HashSet::new();
    for monitor in monitors {
        monitor.validate()?;
        if !ids.insert(monitor.id) || !agent_ids.insert(monitor.id.agent_id()) {
            return Err("duplicate SPICE display identity");
        }
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpiceMonitorRequest {
    pub id: SpiceDisplayId,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub enabled: bool,
}

impl SpiceMonitorRequest {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.id.agent_id().is_none()
            || self.x.unsigned_abs() > MAX_SPICE_DIMENSION
            || self.y.unsigned_abs() > MAX_SPICE_DIMENSION
            || self.width > MAX_SPICE_DIMENSION
            || self.height > MAX_SPICE_DIMENSION
            || (self.enabled && (self.width == 0 || self.height == 0))
        {
            return Err("invalid SPICE monitor request");
        }
        Ok(())
    }
}

/// Preserve the guest layout when the requested size fits. If resizing creates
/// overlap, arrange the displays in their existing left-to-right order. Reject
/// an arrangement that would exceed the agent's coordinate space.
pub fn plan_monitor_resize(
    monitors: &[SpiceMonitor],
    selected: &SpiceMonitor,
    width: u32,
    height: u32,
) -> Result<Vec<SpiceMonitorRequest>, &'static str> {
    validate_monitors(monitors)?;
    selected.validate()?;
    let source = if monitors.is_empty() {
        std::slice::from_ref(selected)
    } else {
        monitors
    };
    if !source.iter().any(|m| m.id == selected.id) {
        return Err("selected SPICE monitor is unavailable");
    }
    let mut requests: Vec<_> = source
        .iter()
        .map(|m| SpiceMonitorRequest {
            id: m.id,
            x: m.x as i32,
            y: m.y as i32,
            width: if m.id == selected.id { width } else { m.width },
            height: if m.id == selected.id {
                height
            } else {
                m.height
            },
            enabled: true,
        })
        .collect();
    for request in &requests {
        request.validate()?;
    }
    let overlap = requests.iter().enumerate().any(|(i, a)| {
        requests[i + 1..].iter().any(|b| {
            a.x < b.x + b.width as i32
                && b.x < a.x + a.width as i32
                && a.y < b.y + b.height as i32
                && b.y < a.y + a.height as i32
        })
    });
    if overlap {
        requests.sort_by_key(|m| (m.x, m.y, m.id.channel, m.id.monitor));
        let mut x = 0;
        for request in &mut requests {
            request.x = x;
            request.y = 0;
            x += request.width as i32;
        }
    }
    for request in &requests {
        if request.x as u32 + request.width > MAX_SPICE_DIMENSION
            || request.y as u32 + request.height > MAX_SPICE_DIMENSION
        {
            return Err("resized SPICE topology exceeds coordinate limits");
        }
    }
    Ok(requests)
}

#[cfg(test)]
mod tests {
    use crate::RemotePoint;
    use crate::spice::display::{
        SpiceDisplayId, SpiceMonitor, plan_monitor_resize, validate_monitors,
    };

    #[test]
    fn spice_resize_preserves_space_and_reflows_overlapping_displays() {
        let monitors = vec![monitor(0, 0, 0), monitor(0, 1, 1920)];
        let requests = plan_monitor_resize(&monitors, &monitors[0], 1280, 720).unwrap();
        assert_eq!(requests[1].x, 1920);
        let requests = plan_monitor_resize(&monitors, &monitors[0], 2560, 1440).unwrap();
        assert_eq!(requests[1].x, 2560);
        assert!(plan_monitor_resize(&monitors, &monitors[0], 16384, 1440).is_err());
        assert!(plan_monitor_resize(&monitors, &monitors[0], 0, 1440).is_err());
        let requests = plan_monitor_resize(&[], &monitors[0], 1280, 720).unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].width, 1280);
    }

    fn monitor(channel: u32, id: u32, x: u32) -> SpiceMonitor {
        SpiceMonitor {
            id: SpiceDisplayId {
                channel,
                monitor: id,
            },
            surface: 0,
            x,
            y: 0,
            width: 1920,
            height: 1080,
        }
    }

    #[test]
    fn single_channel_multiple_monitors_and_multiple_channel_layouts_have_distinct_ids() {
        validate_monitors(&[monitor(0, 0, 0), monitor(0, 1, 1920)]).unwrap();
        validate_monitors(&[monitor(0, 0, 0), monitor(1, 0, 0)]).unwrap();
        assert_eq!(monitor(0, 1, 1920).id.agent_id(), Some(1));
        assert_eq!(monitor(1, 0, 0).id.agent_id(), Some(1));
        assert!(validate_monitors(&[monitor(0, 1, 0), monitor(1, 0, 0)]).is_err());
    }

    #[test]
    fn monitor_pointer_mapping_does_not_include_the_framebuffer_crop_offset() {
        let screen = monitor(0, 1, 1920);
        assert_eq!(
            screen.clamp_pointer(RemotePoint { x: 20, y: 30 }),
            RemotePoint { x: 20, y: 30 }
        );
        assert_eq!(
            screen.clamp_pointer(RemotePoint { x: 3000, y: 2000 }),
            RemotePoint { x: 1919, y: 1079 }
        );
    }

    #[test]
    fn invalid_and_overflowing_monitor_topologies_are_rejected() {
        assert!(validate_monitors(&[monitor(0, 0, u32::MAX)]).is_err());
        assert!(validate_monitors(&[monitor(0, 0, 0), monitor(0, 0, 0)]).is_err());
        assert!(validate_monitors(&[monitor(1, 1, 0)]).is_err());
        let monitors: Vec<_> = (0..17).map(|id| monitor(0, id, 0)).collect();
        assert!(validate_monitors(&monitors).is_err());
    }
}
