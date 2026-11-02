use std::time::{Duration, Instant};

use nyaterm_remote_desktop::activex::NativeRdpRect;
use nyaterm_remote_desktop::{RdpDisplayConfig, RdpDisplayMode};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DisplayParameters {
    pub width: u32,
    pub height: u32,
    pub scale: u32,
}

/// Optional server capability, scoped to one host/connection. Window movement
/// and SmartSizing remain available even when remote display updates fail.
pub(crate) struct DisplayUpdates {
    fit_window: bool,
    applied: DisplayParameters,
    pending: Option<(DisplayParameters, Instant)>,
    available: Option<bool>,
}

impl DisplayUpdates {
    pub fn new(display: &RdpDisplayConfig) -> Self {
        Self {
            fit_window: display.mode == RdpDisplayMode::FitWindow,
            applied: DisplayParameters {
                width: display.width,
                height: display.height,
                scale: display.desktop_scale_factor.max(100),
            },
            pending: None,
            available: None,
        }
    }

    pub fn viewport(&mut self, rect: NativeRdpRect, now: Instant) {
        if self.available == Some(false) {
            return;
        }
        let (width, height) = if self.fit_window {
            let dim = |value: i32| (value.clamp(200, 8192) as u32) & !1;
            (dim(rect.width), dim(rect.height))
        } else {
            (self.applied.width, self.applied.height)
        };
        let desired = DisplayParameters {
            width,
            height,
            scale: rect.scale_percent.max(100),
        };
        if desired == self.applied {
            self.pending = None;
        } else if self.pending.as_ref().map(|(value, _)| *value) != Some(desired) {
            self.pending = Some((desired, now));
        }
    }

    pub fn apply_due(
        &mut self,
        now: Instant,
        update: impl FnOnce(DisplayParameters) -> Result<(), String>,
    ) {
        let Some((parameters, at)) = self.pending else {
            return;
        };
        if now.duration_since(at) < Duration::from_millis(300) {
            return;
        }
        self.pending = None;
        if update(parameters).is_ok() {
            self.applied = parameters;
            self.available = Some(true);
        } else {
            self.available = Some(false);
        }
    }

    pub fn available(&self) -> Option<bool> {
        self.available
    }
}

#[cfg(test)]
mod tests {
    use super::DisplayUpdates;
    use nyaterm_remote_desktop::activex::NativeRdpRect;
    use nyaterm_remote_desktop::{RdpDisplayConfig, RdpDisplayMode};
    use std::time::{Duration, Instant};

    fn rect(width: i32, scale_percent: u32) -> NativeRdpRect {
        NativeRdpRect {
            x: 0,
            y: 0,
            width,
            height: 600,
            scale_percent,
        }
    }

    fn config(mode: RdpDisplayMode) -> RdpDisplayConfig {
        RdpDisplayConfig {
            mode,
            width: 800,
            height: 600,
            desktop_scale_factor: 150,
            ..Default::default()
        }
    }

    #[test]
    fn initial_negotiation_and_position_changes_do_not_resize() {
        for mode in [RdpDisplayMode::FitWindow, RdpDisplayMode::Fixed] {
            let mut updates = DisplayUpdates::new(&config(mode));
            let now = Instant::now();
            updates.viewport(rect(800, 150), now);
            updates.viewport(
                NativeRdpRect {
                    x: 40,
                    ..rect(800, 150)
                },
                now,
            );
            updates.apply_due(now + Duration::from_secs(1), |_| {
                panic!("already negotiated")
            });
            assert_eq!(updates.available(), None);
        }
    }

    #[test]
    fn fixed_desktop_keeps_its_resolution_and_only_updates_dpi() {
        let mut updates = DisplayUpdates::new(&config(RdpDisplayMode::Fixed));
        let now = Instant::now();
        updates.viewport(rect(1200, 150), now);
        updates.apply_due(now + Duration::from_secs(1), |_| panic!("SmartSizing only"));
        updates.viewport(rect(1200, 200), now);
        updates.apply_due(now + Duration::from_secs(1), |value| {
            assert_eq!((value.width, value.height, value.scale), (800, 600, 200));
            Ok(())
        });
        assert_eq!(updates.available(), Some(true));
    }

    #[test]
    fn rejected_update_stops_repeated_calls_until_a_new_host_is_created() {
        let display = config(RdpDisplayMode::FitWindow);
        let mut updates = DisplayUpdates::new(&display);
        let now = Instant::now();
        updates.viewport(rect(1200, 150), now);
        updates.apply_due(now, |_| panic!("debounce"));
        updates.apply_due(now + Duration::from_secs(1), |_| {
            Err("server rejected".into())
        });
        assert_eq!(updates.available(), Some(false));
        updates.viewport(rect(1600, 200), now);
        updates.apply_due(now + Duration::from_secs(2), |_| panic!("disabled"));
        let mut retry = DisplayUpdates::new(&display);
        retry.viewport(rect(1600, 200), now);
        retry.apply_due(now + Duration::from_secs(1), |_| Ok(()));
        assert_eq!(retry.available(), Some(true));
    }

    #[test]
    fn returning_to_negotiated_size_cancels_pending_update() {
        let mut updates = DisplayUpdates::new(&config(RdpDisplayMode::FitWindow));
        let now = Instant::now();
        updates.viewport(rect(1200, 150), now);
        updates.viewport(rect(800, 150), now);
        updates.apply_due(now + Duration::from_secs(1), |_| panic!("cancelled"));
    }
}
