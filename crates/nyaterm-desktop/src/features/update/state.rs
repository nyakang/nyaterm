//! Authoritative transient state for native update checks.

use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};

use nyaterm_core::NativeUpdateInfo;

pub(super) struct UpdateJobResult {
    result: Result<NativeUpdateInfo, String>,
}

impl UpdateJobResult {
    pub(super) fn new(result: Result<NativeUpdateInfo, String>) -> Self {
        Self { result }
    }
}

pub(in crate::features) struct UpdateFeatureState {
    pub(in crate::features) download: super::download::DownloadState,
    pub(in crate::features) download_generation: u64,
    pub(in crate::features) download_cancel:
        nyaterm_transport::connection_attempt::ConnectionAttempt,
    pub(in crate::features) install_requested: bool,
    pub(in crate::features) install_launch_pending: bool,
    tx: UnboundedSender<UpdateJobResult>,
    /// Taken once by `NyaTermApp::start_update_event_drain`, which owns delivery
    /// from then on. `None` afterwards, so a second start is a no-op.
    rx: Option<UnboundedReceiver<UpdateJobResult>>,
    status: String,
    info: Option<NativeUpdateInfo>,
    pending: bool,
}

impl UpdateFeatureState {
    pub(in crate::features) fn new() -> Self {
        let (tx, rx) = unbounded();
        Self {
            download: super::download::DownloadState::Idle,
            download_generation: 0,
            download_cancel: Default::default(),
            install_requested: false,
            install_launch_pending: false,
            tx,
            rx: Some(rx),
            status: format!("Current version {}", env!("CARGO_PKG_VERSION")),
            info: None,
            pending: false,
        }
    }

    pub(in crate::features) fn status(&self) -> &str {
        &self.status
    }

    pub(in crate::features) fn info(&self) -> Option<&NativeUpdateInfo> {
        self.info.as_ref()
    }

    pub(in crate::features) fn is_pending(&self) -> bool {
        self.pending
    }

    pub(super) fn begin_check(&mut self) -> Option<UnboundedSender<UpdateJobResult>> {
        if self.pending {
            self.status = "update check already running".to_string();
            return None;
        }
        self.pending = true;
        self.status = "checking GitHub releases...".to_string();
        self.info = None;
        Some(self.tx.clone())
    }

    pub(super) fn take_event_receiver(&mut self) -> Option<UnboundedReceiver<UpdateJobResult>> {
        self.rx.take()
    }

    /// Apply one job result, reporting whether the UI needs a repaint.
    pub(super) fn apply_event(&mut self, event: UpdateJobResult) -> bool {
        if !self.pending {
            // No check is outstanding, so this can only be a late duplicate.
            // Dropping it keeps a stale status off a check that already settled.
            return false;
        }
        self.pending = false;
        match event.result {
            Ok(info) => {
                self.status = if info.available {
                    format!(
                        "update available: {} -> {}",
                        info.current_version, info.latest_version
                    )
                } else {
                    format!("NyaTerm is up to date ({})", info.current_version)
                };
                self.info = Some(info);
            }
            Err(error) => {
                self.status = format!("update check failed: {error}");
                self.info = None;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::{UpdateFeatureState, UpdateJobResult};

    #[test]
    fn update_state_owns_job_channel_and_initial_status() {
        let mut state = UpdateFeatureState::new();

        assert!(state.status().contains(env!("CARGO_PKG_VERSION")));
        assert!(
            state
                .take_event_receiver()
                .expect("a fresh state still holds its receiver")
                .try_recv()
                .is_err(),
            "the job channel starts empty"
        );
        assert!(state.info().is_none());
        assert!(!state.is_pending());
    }

    #[test]
    fn update_check_admission_prevents_overlapping_jobs() {
        let mut state = UpdateFeatureState::new();

        assert!(state.begin_check().is_some());
        assert!(state.is_pending());
        assert_eq!(state.status(), "checking GitHub releases...");
        assert!(state.begin_check().is_none());
        assert_eq!(state.status(), "update check already running");
    }

    #[test]
    fn update_event_completes_failed_job() {
        let mut state = UpdateFeatureState::new();
        let mut rx = state
            .take_event_receiver()
            .expect("state should retain its event receiver");
        let tx = state.begin_check().expect("first check should start");
        tx.unbounded_send(UpdateJobResult::new(Err("offline".to_string())))
            .expect("the drain receiver is still alive");
        let event = rx.try_recv().expect("the job result should be queued");

        assert!(state.apply_event(event));
        assert!(!state.is_pending());
        assert_eq!(state.status(), "update check failed: offline");
        assert!(state.info().is_none());
    }

    #[test]
    fn update_event_arriving_without_an_outstanding_check_is_dropped() {
        let mut state = UpdateFeatureState::new();

        assert!(
            !state.apply_event(UpdateJobResult::new(Err("stale".to_string()))),
            "a result with no check outstanding must not rewrite the status"
        );
        assert!(state.status().contains(env!("CARGO_PKG_VERSION")));
    }
}
