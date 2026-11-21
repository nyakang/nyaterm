//! Mouse-driven gestures in capture coordinates, independent of the local viewport.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScreenGesture {
    Single,
    Pinch,
    VerticalSlide,
    HorizontalSlide,
}

impl ScreenGesture {
    pub fn from_modifiers(control: bool, shift: bool) -> Self {
        match (control, shift) {
            (false, false) => Self::Single,
            (true, false) => Self::Pinch,
            (false, true) => Self::VerticalSlide,
            (true, true) => Self::HorizontalSlide,
        }
    }

    pub fn mirror(self, x: u32, y: u32, width: u32, height: u32) -> Option<(u32, u32)> {
        if x >= width || y >= height {
            return None;
        }
        match self {
            Self::Single => None,
            Self::Pinch => Some((width - 1 - x, height - 1 - y)),
            Self::VerticalSlide => Some((width - 1 - x, y)),
            Self::HorizontalSlide => Some((x, height - 1 - y)),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::models::android::gesture::ScreenGesture;

    #[test]
    fn modifiers_select_all_four_gestures() {
        for (ctrl, shift, expected) in [
            (false, false, ScreenGesture::Single),
            (true, false, ScreenGesture::Pinch),
            (false, true, ScreenGesture::VerticalSlide),
            (true, true, ScreenGesture::HorizontalSlide),
        ] {
            assert_eq!(ScreenGesture::from_modifiers(ctrl, shift), expected);
        }
    }

    #[test]
    fn mirrors_are_symmetric_and_stay_inside_even_odd_and_single_pixel_frames() {
        for (width, height) in [(8, 10), (9, 11), (1, 1)] {
            for x in 0..width {
                for y in 0..height {
                    for mode in [
                        ScreenGesture::Pinch,
                        ScreenGesture::VerticalSlide,
                        ScreenGesture::HorizontalSlide,
                    ] {
                        let (mx, my) = mode.mirror(x, y, width, height).unwrap();
                        assert!(mx < width && my < height);
                        assert_eq!(mode.mirror(mx, my, width, height), Some((x, y)));
                        match mode {
                            ScreenGesture::Pinch => {
                                assert_eq!((x + mx, y + my), (width - 1, height - 1))
                            }
                            ScreenGesture::VerticalSlide => {
                                assert_eq!((x + mx, my), (width - 1, y))
                            }
                            ScreenGesture::HorizontalSlide => {
                                assert_eq!((mx, y + my), (x, height - 1))
                            }
                            ScreenGesture::Single => unreachable!(),
                        }
                    }
                }
            }
        }
        assert_eq!(ScreenGesture::Single.mirror(1, 1, 8, 8), None);
        assert_eq!(ScreenGesture::Pinch.mirror(0, 0, 0, 0), None);
        assert_eq!(ScreenGesture::Pinch.mirror(8, 0, 8, 8), None);
    }
}
