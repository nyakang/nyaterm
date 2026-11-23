use nyaterm_remote_desktop::spice::display::{SpiceDisplayId, SpiceMonitor};
use nyaterm_remote_desktop::spice::protocol::SpiceCapabilities;

pub(in crate::features::remote_desktop) struct SpicePointerCapture {
    pub session_id: String,
    pub release_requested: bool,
    remainder: gpui::Point<f32>,
}

impl SpicePointerCapture {
    pub fn new(session_id: String) -> Self {
        Self {
            session_id,
            release_requested: false,
            remainder: gpui::point(0., 0.),
        }
    }

    pub fn motion(&mut self, delta: gpui::Point<f32>) -> Option<(i32, i32)> {
        if self.release_requested || !delta.x.is_finite() || !delta.y.is_finite() {
            return None;
        }
        self.remainder.x += delta.x.clamp(-32767., 32767.);
        self.remainder.y += delta.y.clamp(-32767., 32767.);
        let dx = self.remainder.x.trunc().clamp(-32767., 32767.) as i32;
        let dy = self.remainder.y.trunc().clamp(-32767., 32767.) as i32;
        self.remainder.x -= dx as f32;
        self.remainder.y -= dy as f32;
        (dx != 0 || dy != 0).then_some((dx, dy))
    }
}

#[cfg(test)]
mod pointer_tests {
    use super::SpicePointerCapture;
    use gpui::point;

    #[test]
    fn relative_motion_preserves_subpixel_travel_in_both_directions() {
        let mut capture = SpicePointerCapture::new("guest".into());
        assert_eq!(capture.motion(point(0.25, -0.25)), None);
        assert_eq!(capture.motion(point(0.5, -0.5)), None);
        assert_eq!(capture.motion(point(0.5, -0.5)), Some((1, -1)));
        assert_eq!(capture.motion(point(0.75, -0.75)), Some((1, -1)));
    }

    #[test]
    fn invalid_or_released_motion_never_reaches_guest() {
        let mut capture = SpicePointerCapture::new("guest".into());
        assert_eq!(capture.motion(point(f32::NAN, 1.)), None);
        assert_eq!(capture.motion(point(f32::INFINITY, 1.)), None);
        assert_eq!(
            capture.motion(point(f32::MAX, -f32::MAX)),
            Some((32767, -32767))
        );
        assert_eq!(capture.motion(point(0., 0.)), None);
        capture.release_requested = true;
        assert_eq!(capture.motion(point(2., 3.)), None);
    }
}

#[derive(Default)]
pub(in crate::features::remote_desktop) struct SpiceViewState {
    pub monitors: Vec<SpiceMonitor>,
    pub selected: Option<SpiceDisplayId>,
    pub displayed: Option<SpiceMonitor>,
    pub cursors: nyaterm_remote_desktop::spice::cursor::SpiceCursorCache,
    pub capabilities: SpiceCapabilities,
    pub visible: Option<bool>,
    pub muted: bool,
    pub transfer: Option<SpiceTransferView>,
    pub next_transfer_id: u64,
    pub file_picker_open: bool,
    pub pending_resize: Option<(u32, u32, std::time::Instant)>,
    pub last_resize: Option<(SpiceDisplayId, u32, u32)>,
}

pub(in crate::features::remote_desktop) struct SpiceTransferView {
    pub id: u64,
    pub completed: u64,
    pub total: u64,
    pub state: nyaterm_remote_desktop::spice::protocol::SpiceTransferState,
}

impl SpiceViewState {
    pub fn fail_transfer(&mut self) {
        use nyaterm_remote_desktop::spice::protocol::SpiceTransferState;
        if let Some(transfer) = self.transfer.as_mut()
            && transfer.state == SpiceTransferState::Running
        {
            transfer.state = SpiceTransferState::Failed;
        }
    }
}
