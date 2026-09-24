//! Process-wide authoritative state for native updates.

use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};

use nyaterm_core::NativeUpdateInfo;
use nyaterm_transport::connection_attempt::ConnectionAttempt;

use super::download::DownloadState;
use crate::blocking_jobs::BlockingJobScheduler;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UpdateCheckKind {
    Silent,
    Manual,
}

#[derive(Clone, Debug)]
pub(crate) enum UpdatePhase {
    Idle,
    Checking,
    Available,
    Downloading { received: u64, total: Option<u64> },
    Ready,
    Applying,
    UpToDate,
    Failed { message: String, download: bool },
}

pub(crate) enum UpdateEvent {
    Check {
        generation: u64,
        kind: UpdateCheckKind,
        result: Result<NativeUpdateInfo, String>,
    },
    Download {
        generation: u64,
        state: DownloadState,
    },
}

pub(crate) struct UpdateStore {
    phase: UpdatePhase,
    info: Option<NativeUpdateInfo>,
    status: String,
    last_silent_error: Option<String>,
    check_generation: u64,
    startup_check_started: bool,
    pub(in crate::features) download: DownloadState,
    pub(in crate::features) download_generation: u64,
    pub(in crate::features) download_cancel: ConnectionAttempt,
    pub(in crate::features) install_requested: bool,
    tx: UnboundedSender<UpdateEvent>,
    rx: Option<UnboundedReceiver<UpdateEvent>>,
    blocking_jobs: BlockingJobScheduler,
}

impl UpdateStore {
    pub(crate) fn new() -> Self {
        let (tx, rx) = unbounded();
        Self {
            phase: UpdatePhase::Idle,
            info: None,
            status: format!("Current version {}", env!("CARGO_PKG_VERSION")),
            last_silent_error: None,
            check_generation: 0,
            startup_check_started: false,
            download: DownloadState::Idle,
            download_generation: 0,
            download_cancel: Default::default(),
            install_requested: false,
            tx,
            rx: Some(rx),
            blocking_jobs: BlockingJobScheduler::new(),
        }
    }

    pub(in crate::features) fn phase(&self) -> &UpdatePhase {
        &self.phase
    }

    #[cfg(test)]
    fn status(&self) -> &str {
        &self.status
    }

    pub(in crate::features) fn info(&self) -> Option<&NativeUpdateInfo> {
        self.info.as_ref()
    }

    pub(in crate::features) fn is_pending(&self) -> bool {
        matches!(self.phase, UpdatePhase::Checking)
    }

    pub(crate) fn blocking_jobs(&self) -> BlockingJobScheduler {
        self.blocking_jobs.clone()
    }

    pub(crate) fn mark_startup_check_started(&mut self) -> bool {
        if self.startup_check_started {
            return false;
        }
        self.startup_check_started = true;
        true
    }

    pub(crate) fn begin_check(
        &mut self,
        _kind: UpdateCheckKind,
    ) -> Option<(UnboundedSender<UpdateEvent>, u64)> {
        if matches!(
            self.phase,
            UpdatePhase::Checking
                | UpdatePhase::Downloading { .. }
                | UpdatePhase::Ready
                | UpdatePhase::Applying
        ) {
            return None;
        }
        self.check_generation = self.check_generation.wrapping_add(1);
        self.phase = UpdatePhase::Checking;
        self.status = "checking for updates...".to_string();
        self.info = None;
        self.download = DownloadState::Idle;
        self.install_requested = false;
        Some((self.tx.clone(), self.check_generation))
    }

    pub(crate) fn take_event_receiver(&mut self) -> Option<UnboundedReceiver<UpdateEvent>> {
        self.rx.take()
    }

    pub(crate) fn begin_download(
        &mut self,
        cancel: ConnectionAttempt,
    ) -> Option<(NativeUpdateInfo, u64, UnboundedSender<UpdateEvent>)> {
        if matches!(
            self.phase,
            UpdatePhase::Downloading { .. } | UpdatePhase::Applying
        ) {
            return None;
        }
        let info = self.info.as_ref().filter(|info| info.available)?.clone();
        self.download_generation = self.download_generation.wrapping_add(1);
        self.download_cancel = cancel;
        self.download = DownloadState::Downloading {
            received: 0,
            total: None,
        };
        self.phase = UpdatePhase::Downloading {
            received: 0,
            total: None,
        };
        self.status = "downloading update...".to_string();
        Some((info, self.download_generation, self.tx.clone()))
    }

    pub(crate) fn cancel_download(&mut self) {
        self.download_cancel.cancel();
        self.download_generation = self.download_generation.wrapping_add(1);
        self.download = DownloadState::Idle;
        self.phase = if self.info.as_ref().is_some_and(|info| info.available) {
            UpdatePhase::Available
        } else {
            UpdatePhase::Idle
        };
        self.status = "update download cancelled".to_string();
    }

    pub(crate) fn mark_applying(&mut self) {
        self.phase = UpdatePhase::Applying;
        self.status = "installing update...".to_string();
    }

    pub(crate) fn clear_install_request(&mut self) {
        self.install_requested = false;
        if matches!(self.download, DownloadState::Ready(_)) {
            self.phase = UpdatePhase::Ready;
            self.status = "update ready to install".to_string();
        }
    }

    /// Apply one process-wide event. Stale generations cannot overwrite newer state.
    pub(crate) fn apply_event(&mut self, event: UpdateEvent) -> bool {
        match event {
            UpdateEvent::Check {
                generation,
                kind,
                result,
            } => {
                if generation != self.check_generation
                    || !matches!(self.phase, UpdatePhase::Checking)
                {
                    return false;
                }
                match result {
                    Ok(info) => {
                        self.last_silent_error = None;
                        self.status = if info.available {
                            format!(
                                "update available: {} -> {}",
                                info.current_version, info.latest_version
                            )
                        } else {
                            format!("NyaTerm is up to date ({})", info.current_version)
                        };
                        self.phase = if info.available {
                            UpdatePhase::Available
                        } else {
                            UpdatePhase::UpToDate
                        };
                        self.info = Some(info);
                    }
                    Err(error) if kind == UpdateCheckKind::Silent => {
                        self.last_silent_error = Some(error);
                        self.phase = UpdatePhase::Idle;
                        self.status = format!("Current version {}", env!("CARGO_PKG_VERSION"));
                        self.info = None;
                    }
                    Err(error) => {
                        self.status = format!("update check failed: {error}");
                        self.phase = UpdatePhase::Failed {
                            message: error,
                            download: false,
                        };
                        self.info = None;
                    }
                }
                true
            }
            UpdateEvent::Download { generation, state } => {
                if generation != self.download_generation {
                    return false;
                }
                self.phase = match &state {
                    DownloadState::Idle => UpdatePhase::Available,
                    DownloadState::Downloading { received, total } => UpdatePhase::Downloading {
                        received: *received,
                        total: *total,
                    },
                    DownloadState::Ready(_) => UpdatePhase::Ready,
                    DownloadState::Failed(error) => UpdatePhase::Failed {
                        message: error.clone(),
                        download: true,
                    },
                };
                self.status = match &state {
                    DownloadState::Idle => "update available".to_string(),
                    DownloadState::Downloading { .. } => "downloading update...".to_string(),
                    DownloadState::Ready(_) => "update ready to install".to_string(),
                    DownloadState::Failed(error) => format!("update download failed: {error}"),
                };
                self.download = state;
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{UpdateCheckKind, UpdateEvent, UpdatePhase, UpdateStore};

    fn check_event(
        generation: u64,
        kind: UpdateCheckKind,
        result: Result<nyaterm_core::NativeUpdateInfo, String>,
    ) -> UpdateEvent {
        UpdateEvent::Check {
            generation,
            kind,
            result,
        }
    }

    #[test]
    fn startup_check_is_admitted_once_per_process() {
        let mut state = UpdateStore::new();
        assert!(state.mark_startup_check_started());
        assert!(!state.mark_startup_check_started());
    }

    #[test]
    fn update_check_admission_prevents_overlapping_jobs() {
        let mut state = UpdateStore::new();
        assert!(state.begin_check(UpdateCheckKind::Manual).is_some());
        assert!(matches!(state.phase(), UpdatePhase::Checking));
        assert!(state.begin_check(UpdateCheckKind::Manual).is_none());
    }

    #[test]
    fn silent_failures_do_not_enter_the_user_visible_failed_state() {
        let mut state = UpdateStore::new();
        let (_, generation) = state.begin_check(UpdateCheckKind::Silent).unwrap();
        assert!(state.apply_event(check_event(
            generation,
            UpdateCheckKind::Silent,
            Err("offline".to_string()),
        )));
        assert!(matches!(state.phase(), UpdatePhase::Idle));
        assert!(!state.status().contains("offline"));
    }

    #[test]
    fn manual_failures_remain_visible_and_stale_results_are_ignored() {
        let mut state = UpdateStore::new();
        let (_, generation) = state.begin_check(UpdateCheckKind::Manual).unwrap();
        assert!(!state.apply_event(check_event(
            generation.wrapping_add(1),
            UpdateCheckKind::Manual,
            Err("stale".to_string()),
        )));
        assert!(state.apply_event(check_event(
            generation,
            UpdateCheckKind::Manual,
            Err("offline".to_string()),
        )));
        assert!(matches!(
            state.phase(),
            UpdatePhase::Failed {
                download: false,
                ..
            }
        ));
        assert!(state.status().contains("offline"));
    }
}
