//! Issuing the RDP connect with the desktop the hosting panel will actually show.

use crate::features::NyaTermApp;
use gpui::Context;
#[cfg(windows)]
use nyaterm_remote_desktop::RdpBackend;
use nyaterm_remote_desktop::RdpDisplayConfig;
use nyaterm_remote_desktop::RdpDisplayMetrics;
use nyaterm_remote_desktop::RdpDisplayMode;
use nyaterm_remote_desktop::RdpSessionConfig;
use std::time::Duration;
use std::time::Instant;

use super::resize::fit_window_display_metrics;

/// How long a fit-window session waits for its first layout before connecting
/// with the saved size anyway.
///
/// A tab that is on screen is measured within one frame, so this only covers a
/// session started in a tab nobody is looking at; it bounds the delay, never the
/// outcome.
pub(in crate::features::remote_desktop) const FIT_WINDOW_START_TIMEOUT: Duration =
    Duration::from_millis(600);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::features::remote_desktop) enum RdpStartPlan {
    /// Negotiate this desktop with the server.
    Connect(RdpDisplayConfig),
    /// Hold the connect until the panel has been measured.
    WaitForPanel,
}

/// Decide what the first negotiation should ask for, given what is known about
/// the panel the session will be drawn in.
///
/// Connecting a fit-window session at the saved size and resizing it to the
/// panel a moment later shows the same desktop twice at two scales, which is
/// what reads as a stutter while the tab is still opening. A fixed-size session
/// never resizes, so it has nothing to wait for.
pub(in crate::features::remote_desktop) fn plan_rdp_start(
    display: &RdpDisplayConfig,
    metrics: Option<RdpDisplayMetrics>,
) -> RdpStartPlan {
    if display.mode != RdpDisplayMode::FitWindow {
        return RdpStartPlan::Connect(display.clone());
    }
    let Some(metrics) = metrics else {
        return RdpStartPlan::WaitForPanel;
    };
    let mut display = display.clone();
    display.fit_to_panel(metrics);
    RdpStartPlan::Connect(display)
}

impl NyaTermApp {
    /// Open an RDP session and return the id its tab is registered under.
    pub(in crate::features) fn schedule_rdp_runtime(
        &mut self,
        config: RdpSessionConfig,
        cx: &mut Context<Self>,
    ) -> String {
        let session_id = nyaterm_core::uuid();
        self.remote_desktop.insert_connecting(session_id.clone());
        self.begin_rdp_runtime(&session_id, config, cx);
        session_id
    }

    /// Start the helper for `session_id`, whose view state must already exist.
    pub(in crate::features) fn begin_rdp_runtime(
        &mut self,
        session_id: &str,
        mut config: RdpSessionConfig,
        cx: &mut Context<Self>,
    ) {
        if let Err(error) = config.backend.validate_config(&config) {
            self.remote_desktop.insert_failed_session(
                session_id.to_string(),
                error.kind,
                error.message,
            );
            return;
        }
        let metrics = self
            .remote_desktop
            .sessions
            .get(session_id)
            .and_then(|session| {
                session.viewport.map(|viewport| {
                    fit_window_display_metrics(viewport, session.viewport_scale_factor)
                })
            });
        match plan_rdp_start(&config.display, metrics) {
            RdpStartPlan::WaitForPanel => {
                if let Some(session) = self.remote_desktop.sessions.get_mut(session_id) {
                    session.deferred_start = Some((config, Instant::now()));
                }
                // A session without a helper produces no protocol events, and the
                // fallback deadline is only checked by the maintenance clock.
                self.ensure_remote_desktop_periodic_clock(cx);
            }
            RdpStartPlan::Connect(display) => {
                let panel_desktop = metrics.filter(|_| display.mode == RdpDisplayMode::FitWindow);
                config.display = display;
                self.launch_rdp_runtime(session_id, config);
                self.ensure_remote_desktop_periodic_clock(cx);
                if let Some(session) = self.remote_desktop.sessions.get_mut(session_id)
                    && let Some(metrics) = panel_desktop
                {
                    // The size is negotiated by the connect itself; recording it as
                    // an applied resize keeps the layout that follows from asking for
                    // it a second time. Recorded after the launch, because a launch
                    // that never reached a helper replaces this state.
                    session.last_resize = Some(metrics.clamped());
                }
            }
        }
    }

    pub(in crate::features) fn launch_rdp_runtime(
        &mut self,
        session_id: &str,
        config: RdpSessionConfig,
    ) {
        if let Err(error) = config.backend.validate_config(&config) {
            self.remote_desktop.insert_failed_session(
                session_id.to_string(),
                error.kind,
                error.message,
            );
            return;
        }
        #[cfg(windows)]
        if config.backend == RdpBackend::Windows {
            if let Some(session) = self.remote_desktop.sessions.get_mut(session_id) {
                session.native_rdp_config = Some(config);
                session.state = nyaterm_remote_desktop::RemoteDesktopViewState::Connecting;
                session.error = None;
            }
            if let Some(metadata) = self.session.metadata_mut(session_id) {
                metadata.disconnected = false;
            }
            return;
        }

        match self
            .remote_desktop
            .manager
            .create_session_with_id(session_id.to_string(), config)
        {
            Ok(_) => {
                if let Some(metadata) = self.session.metadata_mut(session_id) {
                    metadata.disconnected = false;
                }
            }
            // Left on the tab as a failure the user can retry, which is what a
            // connect that failed before the session was registered showed.
            Err(error) => self.remote_desktop.insert_failed_session(
                session_id.to_string(),
                error.kind,
                error.message,
            ),
        }
    }

    /// Connect the fit-window sessions whose panel was never measured.
    pub(in crate::features) fn drive_deferred_rdp_starts(&mut self) -> bool {
        let now = Instant::now();
        let due = self
            .remote_desktop
            .sessions
            .iter()
            .filter(|(_, session)| {
                session
                    .deferred_start
                    .as_ref()
                    .is_some_and(|(_, queued_at)| now - *queued_at >= FIT_WINDOW_START_TIMEOUT)
            })
            .map(|(session_id, _)| session_id.clone())
            .collect::<Vec<_>>();
        for session_id in &due {
            let deferred = self
                .remote_desktop
                .sessions
                .get_mut(session_id)
                .and_then(|session| session.deferred_start.take());
            let Some((config, _)) = deferred else {
                continue;
            };
            self.launch_rdp_runtime(session_id, config);
        }
        !due.is_empty()
    }
}
