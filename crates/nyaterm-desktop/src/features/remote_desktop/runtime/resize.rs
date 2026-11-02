use crate::features::NyaTermApp;
use gpui::Bounds;
use gpui::Context;
use gpui::Pixels;
use nyaterm_remote_desktop::RdpDisplayMetrics;
use nyaterm_remote_desktop::RdpDisplayMode;
use std::time::Instant;

/// Shortest pane edge, in logical pixels, that counts as a panel measurement.
const MIN_PANEL_EDGE_PX: f32 = 48.;

/// A collapsed or mid-animation pane has no usable size, and the protocol floor
/// is far below it (a 200x200 desktop), so such a layout is treated as "not
/// measured yet" rather than as a size to negotiate or resize to.
pub(super) fn panel_is_measurable(bounds: Bounds<Pixels>) -> bool {
    f32::from(bounds.size.width) >= MIN_PANEL_EDGE_PX
        && f32::from(bounds.size.height) >= MIN_PANEL_EDGE_PX
}

impl NyaTermApp {
    pub(in crate::features) fn update_rdp_viewport(
        &mut self,
        session_id: &str,
        bounds: Bounds<gpui::Pixels>,
        scale_factor: f32,
        cx: &mut Context<Self>,
    ) {
        if !panel_is_measurable(bounds) {
            return;
        }
        let fit_window = self.session.metadata(session_id).is_some_and(|metadata| {
            matches!(
                &metadata.launch_config,
                crate::models::SessionLaunchConfig::Rdp(config)
                    if config.display.mode == RdpDisplayMode::FitWindow
            )
        });
        let Some(session) = self.remote_desktop.sessions.get_mut(session_id) else {
            return;
        };
        session.viewport = Some(bounds);
        session.viewport_scale_factor = scale_factor;
        if let Some((config, _)) = session.deferred_start.take() {
            // The panel is known now, so the connect itself asks for this size.
            self.begin_rdp_runtime(session_id, config, cx);
            return;
        }
        #[cfg(windows)]
        if session.native_rdp.is_some() || session.native_rdp_config.is_some() {
            session.pending_resize = None;
            return;
        }
        if !fit_window || session.dynamic_resize_disabled {
            session.pending_resize = None;
            return;
        }
        self.queue_rdp_resize(session_id, fit_window_display_metrics(bounds, scale_factor));
    }

    pub(in crate::features) fn queue_rdp_resize(
        &mut self,
        session_id: &str,
        metrics: RdpDisplayMetrics,
    ) {
        if let Some(session) = self.remote_desktop.sessions.get_mut(session_id) {
            session.queue_resize(metrics, Instant::now());
        }
    }

    pub(super) fn drive_rdp_resize_debounce(&mut self) -> bool {
        self.remote_desktop.drive_resize_debounce(Instant::now())
    }
}

pub(in crate::features) fn fit_window_display_metrics(
    bounds: Bounds<gpui::Pixels>,
    scale_factor: f32,
) -> RdpDisplayMetrics {
    let scale_factor = if scale_factor.is_finite() {
        scale_factor.max(1.0)
    } else {
        1.0
    };
    RdpDisplayMetrics {
        width: (f32::from(bounds.size.width) * scale_factor)
            .round()
            .max(1.0) as u32,
        height: (f32::from(bounds.size.height) * scale_factor)
            .round()
            .max(1.0) as u32,
        desktop_scale_factor: (scale_factor * 100.0).round().clamp(100.0, 500.0) as u32,
        physical_size_mm: None,
    }
}
