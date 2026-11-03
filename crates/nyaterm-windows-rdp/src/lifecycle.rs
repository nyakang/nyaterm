use std::time::{Duration, Instant};

use crate::{NativeRdpEvent, NativeRdpEventKind};

pub const GRACEFUL_CLOSE_TIMEOUT: Duration = Duration::from_secs(3);
pub const RETIREMENT_TIMEOUT: Duration = Duration::from_secs(60);
pub const RETIREMENT_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NativeRdpPhase {
    #[default]
    Connecting,
    Connected,
    Active,
    Reconnecting,
    Disconnected,
    Closing,
    Closed,
}

/// Session transitions are independent of HWNDs and cannot be reopened by late events.
pub struct NativeRdpLifecycle {
    pub generation: u64,
    phase: NativeRdpPhase,
    login_complete: bool,
    close_confirmed: bool,
    closing_at: Option<Instant>,
}

impl NativeRdpLifecycle {
    pub fn new(generation: u64) -> Self {
        Self {
            generation,
            phase: NativeRdpPhase::Connecting,
            login_complete: false,
            close_confirmed: false,
            closing_at: None,
        }
    }

    pub fn phase(&self) -> NativeRdpPhase {
        self.phase
    }

    pub fn can_present(&self, drawing_ready: bool) -> bool {
        // Remote logon, expired-password, and session-selection screens can
        // require input before OnLoginComplete. Hiding them would prevent login.
        drawing_ready
            && (self.phase == NativeRdpPhase::Connected
                || (self.phase == NativeRdpPhase::Active && self.login_complete))
    }

    pub fn apply(&mut self, event: NativeRdpEvent) -> bool {
        if event.generation != self.generation
            || matches!(
                self.phase,
                NativeRdpPhase::Closed | NativeRdpPhase::Disconnected
            )
        {
            return false;
        }
        if self.phase == NativeRdpPhase::Closing {
            if event.kind == NativeRdpEventKind::CloseConfirmed {
                self.close_confirmed = true;
            }
            return event.kind == NativeRdpEventKind::CloseConfirmed;
        }
        match event.kind {
            NativeRdpEventKind::Connected => {
                if matches!(
                    self.phase,
                    NativeRdpPhase::Connecting | NativeRdpPhase::Reconnecting
                ) {
                    self.phase = NativeRdpPhase::Connected;
                }
            }
            NativeRdpEventKind::LoginComplete | NativeRdpEventKind::Reconnected => {
                self.phase = NativeRdpPhase::Active;
                self.login_complete = true;
            }
            NativeRdpEventKind::Reconnecting => {
                self.phase = NativeRdpPhase::Reconnecting;
                self.login_complete = false;
            }
            NativeRdpEventKind::Disconnected { .. } | NativeRdpEventKind::Fatal { .. } => {
                self.phase = NativeRdpPhase::Disconnected;
                self.login_complete = false;
            }
            NativeRdpEventKind::CloseConfirmed => return false,
            NativeRdpEventKind::FocusReleased
            | NativeRdpEventKind::Connecting
            | NativeRdpEventKind::AuthenticationWarningDisplayed
            | NativeRdpEventKind::AuthenticationWarningDismissed
            | NativeRdpEventKind::LogonError { .. } => {}
        }
        true
    }

    pub fn begin_close(&mut self, now: Instant) {
        if matches!(self.phase, NativeRdpPhase::Closing | NativeRdpPhase::Closed) {
            return;
        }
        self.phase = NativeRdpPhase::Closing;
        self.login_complete = false;
        self.close_confirmed = false;
        self.closing_at = Some(now);
    }

    pub fn may_destroy(&self, now: Instant) -> bool {
        self.close_confirmed
            || self
                .closing_at
                .is_some_and(|at| now.saturating_duration_since(at) >= GRACEFUL_CLOSE_TIMEOUT)
    }

    pub fn closed(&mut self) {
        self.phase = NativeRdpPhase::Closed;
    }
}

#[cfg(test)]
mod tests {
    use crate::lifecycle::{GRACEFUL_CLOSE_TIMEOUT, NativeRdpLifecycle, NativeRdpPhase};
    use crate::{NativeRdpEvent, NativeRdpEventKind};
    use std::time::Instant;

    #[test]
    fn connected_logon_surface_is_visible_before_login_completes() {
        let mut state = NativeRdpLifecycle::new(7);
        assert!(!state.can_present(true));
        state.apply(NativeRdpEvent {
            generation: 7,
            kind: NativeRdpEventKind::Connected,
        });
        assert!(!state.can_present(false));
        assert!(state.can_present(true));
        state.apply(NativeRdpEvent {
            generation: 7,
            kind: NativeRdpEventKind::LoginComplete,
        });
        assert!(!state.can_present(false));
        assert!(state.can_present(true));
        state.apply(NativeRdpEvent {
            generation: 7,
            kind: NativeRdpEventKind::Reconnecting,
        });
        assert!(!state.can_present(true));
        state.apply(NativeRdpEvent {
            generation: 7,
            kind: NativeRdpEventKind::Connected,
        });
        assert!(state.can_present(true));
        state.begin_close(Instant::now());
        assert!(!state.can_present(true));
    }

    #[test]
    fn stale_and_late_login_cannot_reopen_a_closing_session() {
        let mut state = NativeRdpLifecycle::new(2);
        assert!(!state.apply(NativeRdpEvent {
            generation: 1,
            kind: NativeRdpEventKind::LoginComplete
        }));
        let now = Instant::now();
        state.begin_close(now);
        assert!(!state.apply(NativeRdpEvent {
            generation: 2,
            kind: NativeRdpEventKind::LoginComplete
        }));
        assert_eq!(state.phase(), NativeRdpPhase::Closing);
        assert!(!state.may_destroy(now));
        assert!(state.may_destroy(now + GRACEFUL_CLOSE_TIMEOUT));
        state.closed();
        assert!(!state.apply(NativeRdpEvent {
            generation: 2,
            kind: NativeRdpEventKind::CloseConfirmed
        }));
    }

    #[test]
    fn authentication_notifications_do_not_claim_login_or_close_the_logon_surface() {
        let mut state = NativeRdpLifecycle::new(7);
        let notifications = [
            NativeRdpEventKind::Connecting,
            NativeRdpEventKind::AuthenticationWarningDisplayed,
            NativeRdpEventKind::AuthenticationWarningDismissed,
            // Session arbitration and interactive logon also use OnLogonError.
            NativeRdpEventKind::LogonError { code: -2 },
        ];
        for kind in notifications {
            assert!(state.apply(NativeRdpEvent {
                generation: 7,
                kind
            }));
            assert_eq!(state.phase(), NativeRdpPhase::Connecting);
        }
        state.apply(NativeRdpEvent {
            generation: 7,
            kind: NativeRdpEventKind::Connected,
        });
        for kind in notifications {
            assert!(state.apply(NativeRdpEvent {
                generation: 7,
                kind
            }));
            assert_eq!(state.phase(), NativeRdpPhase::Connected);
            assert!(state.can_present(true));
        }
    }
}
