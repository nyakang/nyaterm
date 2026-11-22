//! Retains channel cursors before the first frame and across monitor switches.
use std::collections::BTreeMap;

use crate::protocol::{CursorPosition, CursorShape, CursorVisibility, RemoteCursorEvent};
use crate::spice::display::SpiceMonitor;

const MAX_CHANNELS: usize = 64;
const MAX_BYTES: usize = 16 * 1024 * 1024;

#[derive(Default)]
struct ChannelCursor {
    shape: Option<CursorShape>,
    position: Option<CursorPosition>,
    visible: Option<bool>,
}

#[derive(Default)]
pub struct SpiceCursorCache {
    channels: BTreeMap<u32, ChannelCursor>,
    bytes: usize,
}

impl SpiceCursorCache {
    /// IPC has validated the shape; this additionally bounds retained history.
    pub fn apply(&mut self, channel: u32, event: RemoteCursorEvent) -> Result<(), &'static str> {
        if !self.channels.contains_key(&channel) && self.channels.len() >= MAX_CHANNELS {
            return Err("too many SPICE cursor channels");
        }
        if let RemoteCursorEvent::Shape(shape) = &event {
            let previous = self
                .channels
                .get(&channel)
                .and_then(|c| c.shape.as_ref())
                .map_or(0, |s| s.pixels.len());
            let bytes = self.bytes - previous + shape.pixels.len();
            if bytes > MAX_BYTES {
                return Err("SPICE cursor cache budget exhausted");
            }
            self.bytes = bytes;
        }
        let cursor = self.channels.entry(channel).or_default();
        match event {
            RemoteCursorEvent::Shape(shape) => cursor.shape = Some(shape),
            RemoteCursorEvent::Position(position) => cursor.position = Some(position),
            RemoteCursorEvent::Visibility(visibility) => cursor.visible = Some(visibility.visible),
        }
        Ok(())
    }

    pub fn remove(&mut self, channel: u32) {
        if let Some(cursor) = self.channels.remove(&channel) {
            self.bytes -= cursor.shape.map_or(0, |s| s.pixels.len());
        }
    }

    pub fn for_monitor(&self, monitor: &SpiceMonitor) -> Vec<RemoteCursorEvent> {
        let Some(cursor) = self.channels.get(&monitor.id.channel) else {
            return vec![RemoteCursorEvent::Visibility(CursorVisibility {
                visible: false,
            })];
        };
        let mut events = Vec::with_capacity(3);
        events.extend(cursor.shape.clone().map(RemoteCursorEvent::Shape));
        let inside = cursor.position.is_none_or(|p| {
            p.x >= monitor.x
                && p.y >= monitor.y
                && p.x - monitor.x < monitor.width
                && p.y - monitor.y < monitor.height
        });
        if let Some(position) = cursor.position.filter(|_| inside) {
            events.push(RemoteCursorEvent::Position(CursorPosition {
                x: position.x - monitor.x,
                y: position.y - monitor.y,
            }));
        }
        events.push(RemoteCursorEvent::Visibility(CursorVisibility {
            visible: inside && cursor.visible.unwrap_or(true),
        }));
        events
    }
}

#[cfg(test)]
mod tests {
    use crate::protocol::{
        CursorPosition, CursorShape, CursorVisibility, RemoteCursorEvent, RemotePoint,
    };
    use crate::spice::cursor::SpiceCursorCache;
    use crate::spice::display::{SpiceDisplayId, SpiceMonitor};

    #[test]
    fn spice_cursor_arriving_before_frame_survives_monitor_switches_and_crops_position() {
        let mut cache = SpiceCursorCache::default();
        let shape = CursorShape {
            shape_id: 1,
            width: 1,
            height: 1,
            hotspot: RemotePoint { x: 0, y: 0 },
            pixels: vec![0; 4].into(),
        };
        cache
            .apply(0, RemoteCursorEvent::Shape(shape.clone()))
            .unwrap();
        cache
            .apply(
                0,
                RemoteCursorEvent::Position(CursorPosition { x: 110, y: 5 }),
            )
            .unwrap();
        let mut monitor = SpiceMonitor {
            id: SpiceDisplayId {
                channel: 0,
                monitor: 1,
            },
            surface: 0,
            x: 100,
            y: 0,
            width: 100,
            height: 100,
        };
        assert_eq!(
            cache.for_monitor(&monitor),
            vec![
                RemoteCursorEvent::Shape(shape),
                RemoteCursorEvent::Position(CursorPosition { x: 10, y: 5 }),
                RemoteCursorEvent::Visibility(CursorVisibility { visible: true })
            ]
        );
        monitor.x = 0;
        assert_eq!(
            cache.for_monitor(&monitor).last(),
            Some(&RemoteCursorEvent::Visibility(CursorVisibility {
                visible: false
            }))
        );
        monitor.x = 100;
        assert_eq!(cache.for_monitor(&monitor).len(), 3);
        cache.remove(0);
        assert_eq!(
            cache.for_monitor(&monitor),
            vec![RemoteCursorEvent::Visibility(CursorVisibility {
                visible: false
            })]
        );
    }

    #[test]
    fn spice_cursor_retention_is_bounded_and_removal_frees_capacity() {
        let mut cache = SpiceCursorCache::default();
        for channel in 0..64 {
            cache
                .apply(
                    channel,
                    RemoteCursorEvent::Visibility(CursorVisibility { visible: false }),
                )
                .unwrap();
        }
        assert!(
            cache
                .apply(
                    64,
                    RemoteCursorEvent::Visibility(CursorVisibility { visible: true })
                )
                .is_err()
        );
        cache.remove(0);
        assert!(
            cache
                .apply(
                    64,
                    RemoteCursorEvent::Visibility(CursorVisibility { visible: true })
                )
                .is_ok()
        );
    }
}
