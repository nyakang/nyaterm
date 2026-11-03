use gpui::{Modifiers, ScrollDelta, ScrollEvent, TouchPhase};

const ZOOM_PIXELS_PER_STEP: f32 = 40.0;
// No single event needs to traverse more than the entire supported font range.
const MAX_ZOOM_STEPS: f32 = 64.0;

#[derive(Default)]
pub(super) struct TerminalWheelZoom {
    residual_pixels: f32,
}

impl TerminalWheelZoom {
    /// `Some(0)` consumes a zoom gesture while accumulating sub-step motion.
    pub(super) fn handle_event(
        &mut self,
        event: &ScrollEvent,
        enabled: bool,
        is_macos: bool,
    ) -> Option<i16> {
        if !enabled || !zoom_modifiers(event.modifiers, is_macos) {
            self.residual_pixels = 0.0;
            return None;
        }
        if matches!(
            event.touch_phase,
            TouchPhase::Started | TouchPhase::Cancelled
        ) {
            self.residual_pixels = 0.0;
        }
        if event.touch_phase == TouchPhase::Cancelled {
            return Some(0);
        }
        let delta = match event.delta {
            ScrollDelta::Lines(delta) => {
                self.residual_pixels = 0.0;
                if delta.y.is_finite() && delta.y != 0.0 {
                    delta.y.signum() as i16
                } else {
                    0
                }
            }
            ScrollDelta::Pixels(delta) => {
                let pixels = f32::from(delta.y);
                if !pixels.is_finite() {
                    self.residual_pixels = 0.0;
                    0
                } else {
                    if pixels != 0.0 && pixels.signum() != self.residual_pixels.signum() {
                        self.residual_pixels = 0.0;
                    }
                    let total = self.residual_pixels + pixels;
                    self.residual_pixels = total % ZOOM_PIXELS_PER_STEP;
                    (total / ZOOM_PIXELS_PER_STEP)
                        .trunc()
                        .clamp(-MAX_ZOOM_STEPS, MAX_ZOOM_STEPS) as i16
                }
            }
        };
        if event.touch_phase == TouchPhase::Ended {
            self.residual_pixels = 0.0;
        }
        Some(delta)
    }
}

fn zoom_modifiers(modifiers: Modifiers, is_macos: bool) -> bool {
    !modifiers.alt
        && !modifiers.shift
        && !modifiers.function
        && if is_macos {
            modifiers.platform && !modifiers.control
        } else {
            modifiers.control && !modifiers.platform
        }
}

#[cfg(test)]
mod tests {
    use gpui::{Modifiers, ScrollDelta, ScrollEvent, TouchPhase, point, px};

    use super::{TerminalWheelZoom, zoom_modifiers};

    fn pixel_event(y: f32) -> ScrollEvent {
        ScrollEvent {
            delta: ScrollDelta::Pixels(point(px(0.0), px(y))),
            modifiers: Modifiers::control(),
            ..ScrollEvent::default()
        }
    }

    #[test]
    fn zoom_requires_only_the_platform_zoom_modifier() {
        for is_macos in [false, true] {
            for control in [false, true] {
                for platform in [false, true] {
                    for alt in [false, true] {
                        for shift in [false, true] {
                            for function in [false, true] {
                                assert_eq!(
                                    zoom_modifiers(
                                        Modifiers {
                                            control,
                                            platform,
                                            alt,
                                            shift,
                                            function
                                        },
                                        is_macos,
                                    ),
                                    !alt && !shift
                                        && !function
                                        && control != is_macos
                                        && platform == is_macos,
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn line_wheel_zooms_once_per_event_and_ignores_invalid_vertical_motion() {
        let mut zoom = TerminalWheelZoom::default();
        for (y, expected) in [
            (3.0, 1),
            (-3.0, -1),
            (0.2, 1),
            (0.0, 0),
            (f32::NAN, 0),
            (f32::INFINITY, 0),
        ] {
            let event = ScrollEvent {
                delta: ScrollDelta::Lines(point(5.0, y)),
                modifiers: Modifiers::control(),
                ..ScrollEvent::default()
            };
            assert_eq!(zoom.handle_event(&event, true, false), Some(expected));
        }
    }

    #[test]
    fn pixel_zoom_accumulates_small_motion_and_resets_on_direction_change() {
        let mut zoom = TerminalWheelZoom::default();
        for (y, expected) in [
            (15.0, 0),
            (30.0, 1),
            (35.0, 1),
            (30.0, 0),
            (-15.0, 0),
            (-25.0, -1),
        ] {
            assert_eq!(
                zoom.handle_event(&pixel_event(y), true, false),
                Some(expected)
            );
        }
        // Each surface owns its own unfinished gesture.
        let mut other = TerminalWheelZoom::default();
        assert_eq!(zoom.handle_event(&pixel_event(30.0), true, false), Some(0));
        assert_eq!(other.handle_event(&pixel_event(15.0), true, false), Some(0));
        assert_eq!(zoom.handle_event(&pixel_event(10.0), true, false), Some(1));
    }

    #[test]
    fn zoom_resets_residual_when_disabled_or_returning_to_ordinary_scrolling() {
        let mut zoom = TerminalWheelZoom::default();
        for disabled in [false, true] {
            assert_eq!(zoom.handle_event(&pixel_event(30.0), true, false), Some(0));
            let mut event = pixel_event(5.0);
            if !disabled {
                event.modifiers = Modifiers::default();
            }
            assert_eq!(zoom.handle_event(&event, !disabled, false), None);
            assert_eq!(zoom.handle_event(&pixel_event(10.0), true, false), Some(0));
            assert_eq!(zoom.handle_event(&pixel_event(-10.0), true, false), Some(0));
        }
    }

    #[test]
    fn gesture_boundaries_and_invalid_motion_clear_pixel_residual() {
        for phase in [
            TouchPhase::Started,
            TouchPhase::Ended,
            TouchPhase::Cancelled,
        ] {
            let mut zoom = TerminalWheelZoom::default();
            assert_eq!(zoom.handle_event(&pixel_event(30.0), true, false), Some(0));
            let mut event = pixel_event(0.0);
            event.touch_phase = phase;
            assert_eq!(zoom.handle_event(&event, true, false), Some(0));
            assert_eq!(zoom.handle_event(&pixel_event(10.0), true, false), Some(0));
        }
        for y in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut zoom = TerminalWheelZoom::default();
            assert_eq!(zoom.handle_event(&pixel_event(30.0), true, false), Some(0));
            assert_eq!(zoom.handle_event(&pixel_event(y), true, false), Some(0));
            assert_eq!(zoom.handle_event(&pixel_event(10.0), true, false), Some(0));
        }
    }

    #[test]
    fn huge_pixel_motion_is_bounded_without_retaining_whole_steps() {
        let mut zoom = TerminalWheelZoom::default();
        for y in [f32::MAX, -f32::MAX] {
            assert_eq!(
                zoom.handle_event(&pixel_event(y), true, false),
                Some(if y > 0.0 { 64 } else { -64 })
            );
            assert!(zoom.residual_pixels.is_finite());
            assert!(zoom.residual_pixels.abs() < 40.0);
        }
    }
}
