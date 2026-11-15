//! Transient strip geometry and cancellable ordering; never writes session state.
use std::collections::HashMap;
use std::time::{Duration, Instant};

const MOTION_DURATION: Duration = Duration::from_millis(150);
const REORDER_HYSTERESIS: f32 = 8.;

#[derive(Clone, Copy)]
struct Motion {
    from: f32,
    to: f32,
    started: Instant,
    awaiting_layout: bool,
}

impl Motion {
    fn sample(self, now: Instant) -> f32 {
        if self.awaiting_layout {
            return self.from;
        }
        let t = (now.duration_since(self.started).as_secs_f32() / MOTION_DURATION.as_secs_f32())
            .min(1.);
        self.to + (self.from - self.to) * (1. - t).powi(3)
    }
}

#[derive(Default)]
pub(in crate::features) struct TabReorderMotion {
    order: Vec<String>,
    sessions: Vec<String>,
    geometry: HashMap<String, (f32, f32)>,
    motions: HashMap<String, Motion>,
    active: Option<ReorderPreview>,
    pointer_x: Option<f32>,
    frame_at: Option<Instant>,
}

struct ReorderPreview {
    source: String,
    order: Vec<String>,
    grab_x: f32,
}

impl TabReorderMotion {
    pub fn configure(&mut self, order: Vec<String>, sessions: Vec<String>) {
        if self.order != order || self.sessions != sessions {
            // A closed, added or moved tab invalidates the gesture's snapshot.
            self.active = None;
            self.geometry.retain(|id, _| order.contains(id));
            self.motions.retain(|id, _| order.contains(id));
            self.order = order;
            self.sessions = sessions;
        }
    }

    pub fn matches_sessions(&self, sessions: &[String]) -> bool {
        self.sessions == sessions
    }

    pub fn measure(&mut self, id: &str, x: f32, width: f32, now: Instant) {
        if let Some((old_x, _)) = self.geometry.insert(id.to_owned(), (x, width))
            && let Some(motion) = self.motions.get(id)
            && (motion.awaiting_layout || (old_x - x).abs() > 0.1)
        {
            // FLIP: committing the business order changes layout origins. Keep
            // the painted position continuous while converging on the new origin.
            let offset = self.offset(id, now).0 + old_x - x;
            self.motions.insert(
                id.to_owned(),
                Motion {
                    from: offset,
                    to: 0.,
                    started: now,
                    awaiting_layout: false,
                },
            );
        }
    }

    pub fn width(&self, id: &str) -> Option<f32> {
        self.geometry.get(id).map(|(_, width)| *width)
    }

    pub fn begin(&mut self, source: &str, grab_x: f32) {
        if !self.order.iter().any(|id| id == source)
            || self.order.iter().any(|id| !self.geometry.contains_key(id))
        {
            return;
        }
        self.active = Some(ReorderPreview {
            source: source.to_owned(),
            order: self.order.clone(),
            grab_x,
        });
    }

    pub fn update(&mut self, pointer_x: f32, now: Instant) -> bool {
        let Some(active) = &mut self.active else {
            return false;
        };
        let source_width = self.geometry[&active.source].1;
        let center = pointer_x - active.grab_x + source_width / 2.;
        let mut index = active
            .order
            .iter()
            .position(|id| id == &active.source)
            .unwrap();
        let previous_index = index;
        let origin = self.geometry[&self.order[0]].0;
        // Thresholds use target slots, never moving hitboxes. Hysteresis keeps
        // a stationary pointer from oscillating when unequal-width tabs swap.
        loop {
            let left = origin
                + active.order[..index]
                    .iter()
                    .map(|id| self.geometry[id].1)
                    .sum::<f32>();
            if index + 1 < active.order.len() {
                let next_width = self.geometry[&active.order[index + 1]].1;
                if center > left + (source_width + next_width) / 2. + REORDER_HYSTERESIS {
                    active.order.swap(index, index + 1);
                    index += 1;
                    continue;
                }
            }
            if index > 0 {
                let previous_width = self.geometry[&active.order[index - 1]].1;
                if center < left + (source_width - previous_width) / 2. - REORDER_HYSTERESIS {
                    active.order.swap(index, index - 1);
                    index -= 1;
                    continue;
                }
            }
            break;
        }
        let order = active.order.clone();
        let source = active.source.clone();
        let grab_x = active.grab_x;
        let mut x = origin;
        for id in order {
            let (base, width) = self.geometry[&id];
            let target = if id == source {
                pointer_x - grab_x - base
            } else {
                x - base
            };
            if id == source {
                self.motions.insert(
                    id,
                    Motion {
                        from: target,
                        to: target,
                        started: now,
                        awaiting_layout: false,
                    },
                );
            } else {
                self.retarget(&id, target, now);
            }
            x += width;
        }
        previous_index != index
    }

    fn retarget(&mut self, id: &str, target: f32, now: Instant) {
        if self
            .motions
            .get(id)
            .is_some_and(|motion| (motion.to - target).abs() < 0.1)
        {
            return;
        }
        let from = self.offset(id, now).0;
        self.motions.insert(
            id.to_owned(),
            Motion {
                from,
                to: target,
                started: now,
                awaiting_layout: false,
            },
        );
    }

    pub fn offset(&self, id: &str, now: Instant) -> (f32, bool) {
        self.motions.get(id).map_or((0., false), |motion| {
            (
                motion.sample(now),
                !motion.awaiting_layout
                    && now.duration_since(motion.started) < MOTION_DURATION
                    && (motion.from - motion.to).abs() > 0.1,
            )
        })
    }

    pub fn target_offset(&self, id: &str) -> f32 {
        self.motions.get(id).map_or(0., |motion| motion.to)
    }

    pub fn animate_frame(&mut self, now: Instant) -> bool {
        self.motions.retain(|_, motion| {
            motion.awaiting_layout
                || motion.to.abs() > 0.1
                || now.duration_since(motion.started) < MOTION_DURATION
        });
        self.motions.values().any(|motion| {
            !motion.awaiting_layout
                && now.duration_since(motion.started) < MOTION_DURATION
                && (motion.from - motion.to).abs() > 0.1
        })
    }

    pub fn set_pointer(&mut self, x: Option<f32>) {
        self.pointer_x = x;
    }

    pub fn advance_scroll(
        &mut self,
        left: f32,
        right: f32,
        current: f32,
        max: f32,
        now: Instant,
    ) -> Option<f32> {
        let x = self.pointer_x?;
        self.active.as_ref()?;
        let elapsed = self.frame_at.map_or(1. / 60., |previous| {
            now.duration_since(previous).as_secs_f32().min(0.032)
        });
        self.frame_at = Some(now);
        let speed = if x < left + 32. {
            ((left + 32. - x) / 32.).clamp(0., 1.) * 600.
        } else if x > right - 32. {
            -((x - right + 32.) / 32.).clamp(0., 1.) * 600.
        } else {
            0.
        };
        let next = (current + speed * elapsed).clamp(-max.max(0.), 0.);
        (next != current).then_some(next)
    }

    pub fn pointer(&self) -> Option<f32> {
        self.pointer_x
    }

    pub fn suspend(&mut self, now: Instant) {
        self.pointer_x = None;
        self.frame_at = None;
        if let Some(active) = &mut self.active {
            active.order.clone_from(&self.order);
        }
        for id in self.order.clone() {
            self.retarget(&id, 0., now);
        }
    }

    pub fn preview(&self, source: &str) -> Option<&[String]> {
        self.active
            .as_ref()
            .filter(|active| active.source == source)
            .map(|active| active.order.as_slice())
    }

    pub fn commit(&mut self, now: Instant) {
        self.active = None;
        self.pointer_x = None;
        self.frame_at = None;
        // Hold the release position until measure rebases it onto the committed
        // layout. Returning to zero now would animate toward the old slots.
        for motion in self.motions.values_mut() {
            motion.from = motion.sample(now);
            motion.awaiting_layout = true;
        }
    }

    pub fn finish(&mut self, now: Instant) {
        self.active = None;
        self.pointer_x = None;
        self.frame_at = None;
        for id in self.order.clone() {
            // Drop cleanup can run again before the committed layout is measured.
            if self
                .motions
                .get(&id)
                .is_some_and(|motion| motion.awaiting_layout)
            {
                continue;
            }
            self.retarget(&id, 0., now);
        }
    }
}

#[cfg(test)]
mod tests;
