use crate::features::android::ScreenState;
use gpui::{Pixels, Point};
use nyaterm_android::ipc::{Control, TouchPoint, TouchPointer};
use nyaterm_core::models::android::{gesture::ScreenGesture, screen_point};

impl ScreenState {
    pub(super) fn position(&self, position: Point<Pixels>) -> Option<(u32, u32, u32, u32, u64)> {
        if self.pan_drag.is_some() || self.bounds_generation != self.generation {
            return None;
        }
        let bounds = self.bounds?;
        if !self.viewport_bounds?.contains(&position) {
            return None;
        }
        let (width, height) = self.dimensions;
        let (x, y) = screen_point(
            f32::from(position.x - bounds.origin.x),
            f32::from(position.y - bounds.origin.y),
            f32::from(bounds.size.width),
            f32::from(bounds.size.height),
            width,
            height,
        )?;
        Some((x, y, width, height, self.generation))
    }

    pub(super) fn pointer_input(
        &mut self,
        position: Point<Pixels>,
        action: u8,
        begin: Option<ScreenGesture>,
    ) -> Option<Control> {
        if action != 0 && self.gesture.is_none() {
            return None;
        }
        let Some((x, y, width, height, generation)) = self.position(position) else {
            return self.gesture.take().map(|_| {
                self.physical_keys.clear();
                self.paste_shortcut = false;
                Control::ReleaseInputs
            });
        };
        let mode = if action == 0 {
            let mode = begin?;
            self.gesture = Some(mode);
            mode
        } else {
            self.gesture?
        };
        let mut points = vec![TouchPoint {
            pointer: if mode == ScreenGesture::Single {
                TouchPointer::Mouse
            } else {
                TouchPointer::Finger
            },
            x,
            y,
        }];
        if let Some((x, y)) = mode.mirror(x, y, width, height) {
            points.push(TouchPoint {
                pointer: TouchPointer::VirtualFinger,
                x,
                y,
            });
        }
        if action == 1 {
            self.gesture = None;
        }
        Some(Control::Touch {
            action,
            points,
            width,
            height,
            generation,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::features::android::ScreenState;
    use gpui::{Bounds, TestAppContext, point, px, size};
    use nyaterm_android::ipc::{Control, TouchPointer};
    use nyaterm_core::models::android::gesture::ScreenGesture;

    fn screen(cx: &mut TestAppContext) -> ScreenState {
        cx.update(|cx| {
            let mut screen = ScreenState::new(cx.focus_handle(), 1, Default::default());
            screen.dimensions = (100, 200);
            screen.generation = 1;
            screen.bounds_generation = 1;
            screen.bounds = Some(Bounds::new(point(px(0.), px(0.)), size(px(100.), px(200.))));
            screen.viewport_bounds = screen.bounds;
            screen
        })
    }

    #[test]
    fn gesture_latches_on_press_and_does_not_restart_after_interruption() {
        let mut cx = TestAppContext::single();
        for mode in [
            ScreenGesture::Single,
            ScreenGesture::Pinch,
            ScreenGesture::VerticalSlide,
            ScreenGesture::HorizontalSlide,
        ] {
            let mut screen = screen(&mut cx);
            let start = point(px(20.), px(30.));
            let Some(Control::Touch { points, .. }) = screen.pointer_input(start, 0, Some(mode))
            else {
                panic!("down");
            };
            assert_eq!(
                points.len(),
                if mode == ScreenGesture::Single { 1 } else { 2 }
            );
            assert!(
                points[0].pointer
                    == if mode == ScreenGesture::Single {
                        TouchPointer::Mouse
                    } else {
                        TouchPointer::Finger
                    }
            );
            let Some(Control::Touch { points, .. }) =
                screen.pointer_input(point(px(30.), px(40.)), 2, Some(ScreenGesture::Single))
            else {
                panic!("move");
            };
            if let Some((x, y)) = mode.mirror(30, 40, 100, 200) {
                assert_eq!((points[1].x, points[1].y), (x, y));
            }
            screen.physical_keys.insert(113);
            screen.paste_shortcut = true;
            assert!(matches!(
                screen.pointer_input(point(px(-1.), px(40.)), 2, None),
                Some(Control::ReleaseInputs)
            ));
            assert!(screen.physical_keys.is_empty());
            assert!(!screen.paste_shortcut);
            assert!(screen.pointer_input(start, 2, None).is_none());
            assert!(screen.pointer_input(start, 1, None).is_none());
            assert!(screen.pointer_input(start, 0, Some(mode)).is_some());
            assert!(matches!(
                screen.pointer_input(start, 1, None),
                Some(Control::Touch { action: 1, .. })
            ));
            assert!(screen.gesture.is_none());
        }
    }

    #[test]
    fn mirror_uses_capture_coordinates_after_zoom_and_pan() {
        let mut cx = TestAppContext::single();
        let mut screen = screen(&mut cx);
        // A 2x image panned left and up inside the original clipped viewport.
        screen.bounds = Some(Bounds::new(
            point(px(-50.), px(-100.)),
            size(px(200.), px(400.)),
        ));
        let Some(Control::Touch { points, .. }) =
            screen.pointer_input(point(px(10.), px(20.)), 0, Some(ScreenGesture::Pinch))
        else {
            panic!("down");
        };
        assert_eq!((points[0].x, points[0].y), (30, 60));
        assert_eq!((points[1].x, points[1].y), (69, 139));
        assert!(matches!(
            screen.pointer_input(point(px(110.), px(20.)), 2, None),
            Some(Control::ReleaseInputs)
        ));
    }

    #[test]
    fn stale_layout_or_pan_ends_gesture_without_affecting_other_screen() {
        let mut cx = TestAppContext::single();
        let mut first = screen(&mut cx);
        let mut second = screen(&mut cx);
        let position = point(px(10.), px(20.));
        first.pointer_input(position, 0, Some(ScreenGesture::Pinch));
        second.pointer_input(position, 0, Some(ScreenGesture::VerticalSlide));
        first.generation += 1;
        assert!(matches!(
            first.pointer_input(position, 2, None),
            Some(Control::ReleaseInputs)
        ));
        assert_eq!(second.gesture, Some(ScreenGesture::VerticalSlide));
        second.pan_drag = Some(position);
        assert!(matches!(
            second.pointer_input(position, 2, None),
            Some(Control::ReleaseInputs)
        ));
        second.pan_drag = None;
        assert!(second.pointer_input(position, 2, None).is_none());
    }
}
