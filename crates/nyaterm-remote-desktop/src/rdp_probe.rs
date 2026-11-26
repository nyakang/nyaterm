//! One-shot RDP authentication probe used by the connection editor's "test
//! connection" action.
//!
//! The full RDP client runs in the isolated `nyaterm-rdp-helper` process. This
//! probe reuses that helper through [`RdpSessionManager`], waits for the
//! authenticated session to come up, and then tears it down. It never creates a
//! tab and never persists a certificate.
//!
//! Certificate handling mirrors the test-only trust used by the SSH and VNC
//! probes: `Prompt`, `TrustOnFirstUse` and `Insecure` are narrowed to
//! `Insecure` for this single attempt, so an accepted certificate is never
//! written to the trust store. `Strict` and `RejectChanged` are left alone so a
//! real certificate problem still fails the test.

use std::time::{Duration, Instant};

use nyaterm_core::plugins::native::NativeLaunch;

use crate::protocol::{
    RdpCertificatePolicy, RdpCertificateResponse, RdpError, RdpErrorKind, RdpRuntimeEvent,
    RdpSessionConfig, RdpSessionState,
};
use crate::session::RdpSessionManager;

const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Connect to an RDP server, complete the TLS + NLA authentication, then close
/// the throwaway session.
pub fn probe_rdp_connection(
    config: RdpSessionConfig,
    timeout: Duration,
    is_cancelled: impl Fn() -> bool,
) -> Result<(), RdpError> {
    probe_rdp_connection_with_launch(config, None, timeout, is_cancelled)
}

/// Like [`probe_rdp_connection`], but runs the helper resolved from the native
/// component registry. Packaged builds install the helper as a separate
/// component instead of beside the executable.
pub fn probe_rdp_connection_with_launch(
    mut config: RdpSessionConfig,
    launch: Option<NativeLaunch>,
    timeout: Duration,
    is_cancelled: impl Fn() -> bool,
) -> Result<(), RdpError> {
    let requested_policy = config.certificate_policy;
    config.certificate_policy = test_certificate_policy(requested_policy);
    // A test must surface the first failure instead of retrying until the
    // editor's overall timeout expires.
    config.reconnect.enabled = false;
    config.reconnect.max_attempts = 0;

    let manager = RdpSessionManager::new();
    let session_id = match launch {
        Some(launch) => {
            manager.create_session_with_launch(uuid::Uuid::new_v4().to_string(), config, launch)?
        }
        None => manager.create_session(config)?,
    };
    let deadline = Instant::now() + timeout;
    let mut outcome: Option<Result<(), RdpError>> = None;

    while Instant::now() < deadline {
        if is_cancelled() {
            outcome = Some(Err(RdpError::new(
                RdpErrorKind::Transport,
                "connection test cancelled",
            )));
            break;
        }

        for event in manager.drain_batch(&session_id).control {
            match event {
                RdpRuntimeEvent::State { state, .. } => match state {
                    RdpSessionState::Connected => {
                        outcome = Some(Ok(()));
                        break;
                    }
                    RdpSessionState::Failed(error) => {
                        outcome = Some(Err(error));
                        break;
                    }
                    _ => {}
                },
                RdpRuntimeEvent::Error { error, fatal, .. } if fatal => {
                    outcome = Some(Err(error));
                    break;
                }
                RdpRuntimeEvent::CertificateRequest(request) => {
                    // Prompt/TOFU were narrowed above, so a request here means
                    // an explicit strict policy. Reject it rather than silently
                    // weakening the test.
                    let response = match requested_policy {
                        RdpCertificatePolicy::Strict | RdpCertificatePolicy::RejectChanged => {
                            RdpCertificateResponse::Reject
                        }
                        _ => RdpCertificateResponse::TrustOnce,
                    };
                    let _ = manager.respond_certificate(&request.request_id, response);
                }
                _ => {}
            }
        }

        if outcome.is_some() {
            break;
        }

        match manager.state(&session_id) {
            Some(RdpSessionState::Connected) => {
                outcome = Some(Ok(()));
                break;
            }
            Some(RdpSessionState::Failed(error)) => {
                outcome = Some(Err(error));
                break;
            }
            _ => {}
        }

        std::thread::sleep(POLL_INTERVAL);
    }

    let _ = manager.close(&session_id);
    drop(manager);

    outcome.unwrap_or_else(|| {
        Err(RdpError::new(
            RdpErrorKind::Timeout,
            "RDP connection test timed out".to_string(),
        ))
    })
}

fn test_certificate_policy(policy: RdpCertificatePolicy) -> RdpCertificatePolicy {
    match policy {
        // These policies would either prompt or persist. A connection test
        // accepts for this attempt only and never remembers the certificate.
        RdpCertificatePolicy::Prompt
        | RdpCertificatePolicy::TrustOnFirstUse
        | RdpCertificatePolicy::Insecure => RdpCertificatePolicy::Insecure,
        RdpCertificatePolicy::Strict | RdpCertificatePolicy::RejectChanged => policy,
    }
}

#[cfg(test)]
mod tests {
    use super::test_certificate_policy;
    use crate::protocol::RdpCertificatePolicy;

    #[test]
    fn prompt_and_tofu_become_test_only_acceptance() {
        assert_eq!(
            test_certificate_policy(RdpCertificatePolicy::Prompt),
            RdpCertificatePolicy::Insecure
        );
        assert_eq!(
            test_certificate_policy(RdpCertificatePolicy::TrustOnFirstUse),
            RdpCertificatePolicy::Insecure
        );
        assert_eq!(
            test_certificate_policy(RdpCertificatePolicy::Insecure),
            RdpCertificatePolicy::Insecure
        );
    }

    #[test]
    fn strict_policies_are_preserved() {
        assert_eq!(
            test_certificate_policy(RdpCertificatePolicy::Strict),
            RdpCertificatePolicy::Strict
        );
        assert_eq!(
            test_certificate_policy(RdpCertificatePolicy::RejectChanged),
            RdpCertificatePolicy::RejectChanged
        );
    }
}
