//! One-shot SPICE authentication probe used by the connection editor's "test
//! connection" action.
//!
//! The full SPICE client runs in the isolated `nyaterm-spice-helper`. This
//! probe reuses that helper through [`SpiceSessionManager`], waits for the
//! authenticated session to come up, and then tears it down. It never creates a
//! tab and never keeps a session.

use std::time::{Duration, Instant};

use nyaterm_core::plugins::native::NativeLaunch;

use super::protocol::{SpiceControl, SpiceErrorKind, SpiceSessionConfig, SpiceSessionState};
use super::session::{SpiceEvent, SpiceSessionManager};

const POLL_INTERVAL: Duration = Duration::from_millis(50);
const DRAIN_LIMIT: usize = 256;

/// Connect to a SPICE server, complete TLS/ticket authentication, then close the
/// throwaway helper session.
pub fn probe_spice_connection(
    config: SpiceSessionConfig,
    launch: Option<NativeLaunch>,
    timeout: Duration,
    is_cancelled: impl Fn() -> bool,
) -> Result<(), SpiceErrorKind> {
    let manager = SpiceSessionManager::new();
    let session_id = match launch {
        Some(launch) => manager.create_session_with_launch(config, launch)?,
        None => manager.create_session(config)?,
    };

    let deadline = Instant::now() + timeout;
    let mut outcome: Option<Result<(), SpiceErrorKind>> = None;
    while Instant::now() < deadline {
        if is_cancelled() {
            outcome = Some(Err(SpiceErrorKind::Timeout));
            break;
        }

        for event in manager.drain(&session_id, DRAIN_LIMIT) {
            let SpiceEvent::Control(control) = event else {
                continue;
            };
            match control {
                SpiceControl::State {
                    state: SpiceSessionState::Connected,
                } => {
                    outcome = Some(Ok(()));
                    break;
                }
                SpiceControl::State {
                    state: SpiceSessionState::Disconnected,
                } => {
                    outcome = Some(Err(SpiceErrorKind::Connection));
                    break;
                }
                SpiceControl::Error { error, fatal } if fatal => {
                    outcome = Some(Err(error));
                    break;
                }
                _ => {}
            }
        }

        if outcome.is_some() {
            break;
        }
        std::thread::sleep(POLL_INTERVAL);
    }

    manager.close(&session_id);
    outcome.unwrap_or(Err(SpiceErrorKind::Timeout))
}
