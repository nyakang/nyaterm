use super::{RemoteDesktopFeatureState, RemoteDesktopSessionState};
use nyaterm_remote_desktop::{RdpDisplayMetrics, RdpSessionState};
use std::time::{Duration, Instant};

pub(in crate::features::remote_desktop) const RESIZE_MIN_DELTA: u32 = 32;

pub(in crate::features::remote_desktop) const RESIZE_FAILURE_WINDOW: Duration =
    Duration::from_secs(3);

pub(in crate::features::remote_desktop) const RESIZE_DEBOUNCE: Duration =
    Duration::from_millis(150);

impl RemoteDesktopSessionState {
    pub(in crate::features::remote_desktop) fn queue_resize(
        &mut self,
        metrics: RdpDisplayMetrics,
        now: Instant,
    ) {
        let metrics = metrics.clamped();
        let remote_size = self
            .framebuffer
            .as_ref()
            .map(|framebuffer| (framebuffer.width(), framebuffer.height()));
        if self.dynamic_resize_disabled
            || !rdp_resize_is_material(remote_size, self.last_resize, metrics)
        {
            return;
        }
        self.pending_resize = Some((metrics, now));
    }
}

impl RemoteDesktopFeatureState {
    pub(in crate::features::remote_desktop) fn drive_resize_debounce(
        &mut self,
        now: Instant,
    ) -> bool {
        let mut sent = false;
        for (session_id, session) in &mut self.sessions {
            let Some((metrics, queued_at)) = session.pending_resize else {
                continue;
            };
            if now.saturating_duration_since(queued_at) < RESIZE_DEBOUNCE {
                continue;
            }
            session.pending_resize = None;
            if self
                .manager
                .resize_with_metrics(session_id, metrics)
                .is_ok()
            {
                session.last_resize = Some(metrics);
                session.last_resize_sent_at = Some(now);
                sent = true;
            }
        }
        sent
    }
}

pub(in crate::features::remote_desktop) fn rdp_resize_is_material(
    remote_size: Option<(u32, u32)>,
    last_resize: Option<RdpDisplayMetrics>,
    requested: RdpDisplayMetrics,
) -> bool {
    if last_resize == Some(requested) {
        return false;
    }
    if last_resize.is_some_and(|last| {
        last.desktop_scale_factor != requested.desktop_scale_factor
            || last.physical_size_mm != requested.physical_size_mm
    }) || (last_resize.is_none() && requested.desktop_scale_factor != 100)
    {
        return true;
    }
    let Some((remote_width, remote_height)) = remote_size else {
        return true;
    };
    remote_width.abs_diff(requested.width) >= RESIZE_MIN_DELTA
        || remote_height.abs_diff(requested.height) >= RESIZE_MIN_DELTA
}

pub(in crate::features::remote_desktop) fn should_disable_dynamic_resize_after_state(
    state: &RdpSessionState,
    last_resize_sent_at: Option<Instant>,
    now: Instant,
) -> bool {
    matches!(
        state,
        RdpSessionState::Reconnecting | RdpSessionState::Failed(_)
    ) && last_resize_sent_at
        .is_some_and(|sent_at| now.saturating_duration_since(sent_at) <= RESIZE_FAILURE_WINDOW)
}

#[cfg(test)]
mod tests {
    use super::RemoteDesktopSessionState;
    use super::{rdp_resize_is_material, should_disable_dynamic_resize_after_state};
    use nyaterm_remote_desktop::{RdpDisplayMetrics, RdpError, RdpErrorKind};
    use std::time::{Duration, Instant};

    #[test]
    fn resize_owner_clamps_metrics_coalesces_requests_and_ignores_disabled_sessions() {
        let mut session = RemoteDesktopSessionState::default();
        let now = Instant::now();
        session.queue_resize(
            RdpDisplayMetrics {
                width: 199,
                height: 9001,
                desktop_scale_factor: 700,
                physical_size_mm: None,
            },
            now,
        );
        let (clamped, queued_at) = session.pending_resize.expect("queued resize");
        assert_eq!((clamped.width, clamped.height), (200, 8192));
        assert_eq!(clamped.desktop_scale_factor, 500);
        assert_eq!(queued_at, now);

        let latest = RdpDisplayMetrics {
            width: 1281,
            height: 721,
            desktop_scale_factor: 150,
            physical_size_mm: Some((340, 190)),
        };
        let later = now + Duration::from_millis(100);
        session.queue_resize(latest, later);
        let (latest, queued_at) = session.pending_resize.expect("latest request wins");
        assert_eq!((latest.width, latest.height), (1280, 720));
        assert_eq!(latest.physical_size_mm, Some((340, 190)));
        assert_eq!(queued_at, later);

        session.pending_resize = None;
        session.last_resize = Some(latest);
        session.queue_resize(latest, later);
        assert!(
            session.pending_resize.is_none(),
            "duplicate resize is ignored"
        );
        session.last_resize = None;
        session.dynamic_resize_disabled = true;
        session.queue_resize(latest, later);
        assert!(
            session.pending_resize.is_none(),
            "disabled resize is ignored"
        );
    }

    #[test]
    fn resize_filter_ignores_duplicate_and_sub_threshold_changes() {
        let metrics = |width, height, desktop_scale_factor| RdpDisplayMetrics {
            width,
            height,
            desktop_scale_factor,
            physical_size_mm: None,
        };
        assert!(!rdp_resize_is_material(
            Some((1280, 720)),
            None,
            metrics(1300, 740, 100)
        ));
        assert!(rdp_resize_is_material(
            Some((1280, 720)),
            None,
            metrics(1312, 720, 100)
        ));
        assert!(!rdp_resize_is_material(
            None,
            Some(metrics(1280, 720, 100)),
            metrics(1280, 720, 100)
        ));
        assert!(rdp_resize_is_material(None, None, metrics(1280, 720, 100)));
        assert!(rdp_resize_is_material(
            Some((1280, 720)),
            None,
            metrics(1280, 720, 150)
        ));
    }

    #[test]
    fn resize_related_failure_disables_dynamic_resize_only_inside_window() {
        let now = Instant::now();
        let error = RdpError::new(RdpErrorKind::Session, "resize failed");
        assert!(should_disable_dynamic_resize_after_state(
            &nyaterm_remote_desktop::RdpSessionState::Failed(error.clone()),
            Some(now - Duration::from_secs(2)),
            now,
        ));
        assert!(!should_disable_dynamic_resize_after_state(
            &nyaterm_remote_desktop::RdpSessionState::Failed(error),
            Some(now - Duration::from_secs(4)),
            now,
        ));
        assert!(!should_disable_dynamic_resize_after_state(
            &nyaterm_remote_desktop::RdpSessionState::Connected,
            Some(now),
            now,
        ));
    }
}
