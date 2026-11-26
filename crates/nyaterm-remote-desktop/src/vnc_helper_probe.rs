//! Helper-backed VNC authentication probe for security types the lightweight
//! RFB probe cannot evaluate (RA2, VeNCrypt, Tight, ...).
//!
//! The full VNC client runs in the isolated `nyaterm-vnc-helper`. This probe
//! reuses that helper through [`VncSessionManager`], waits for authentication
//! to complete, and tears the session down. It never creates a tab and never
//! remembers an RA2 server key.

use std::time::{Duration, Instant};

use nyaterm_core::plugins::native::NativeLaunch;

use crate::protocol::{VncError, VncErrorKind, VncRuntimeEvent, VncSessionConfig, VncSessionState};
use crate::vnc::VncSessionManager;

const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Connect to a VNC server through the isolated helper, complete whatever
/// security exchange the server offers, then close the throwaway session.
pub fn probe_vnc_connection_with_launch(
    config: VncSessionConfig,
    launch: Option<NativeLaunch>,
    timeout: Duration,
    is_cancelled: impl Fn() -> bool,
) -> Result<(), VncError> {
    let manager = VncSessionManager::new();
    let session_id = match launch {
        Some(launch) => {
            manager.create_session_with_launch(uuid::Uuid::new_v4().to_string(), config, launch)?
        }
        None => manager.create_session(config)?,
    };

    let deadline = Instant::now() + timeout;
    let mut outcome: Option<Result<(), VncError>> = None;
    while Instant::now() < deadline {
        if is_cancelled() {
            outcome = Some(Err(VncError::new(
                VncErrorKind::Transport,
                "connection test cancelled",
            )));
            break;
        }

        for event in manager.drain_batch(&session_id).control {
            match event {
                VncRuntimeEvent::State {
                    state: VncSessionState::Connected,
                    ..
                } => {
                    outcome = Some(Ok(()));
                    break;
                }
                VncRuntimeEvent::State {
                    state: VncSessionState::Failed,
                    message,
                    ..
                } => {
                    outcome = Some(Err(VncError::new(
                        VncErrorKind::Authentication,
                        message.unwrap_or_else(|| "VNC authentication failed".to_string()),
                    )));
                    break;
                }
                VncRuntimeEvent::Error { error, fatal, .. } if fatal => {
                    outcome = Some(Err(error));
                    break;
                }
                VncRuntimeEvent::ServerKeyRequest(request) => {
                    // Test-only trust: accept the RA2 server key for this
                    // attempt, but never persist it for later sessions.
                    let _ = manager.respond_server_key(&request, true, false);
                }
                _ => {}
            }
        }

        if outcome.is_some() {
            break;
        }

        match manager.state(&session_id) {
            Some(VncSessionState::Connected) => {
                outcome = Some(Ok(()));
                break;
            }
            Some(VncSessionState::Failed) => {
                outcome = Some(Err(VncError::new(
                    VncErrorKind::Authentication,
                    "VNC authentication failed",
                )));
                break;
            }
            _ => {}
        }

        std::thread::sleep(POLL_INTERVAL);
    }

    let _ = manager.close(&session_id);
    outcome.unwrap_or_else(|| {
        Err(VncError::new(
            VncErrorKind::Timeout,
            "VNC connection test timed out",
        ))
    })
}
