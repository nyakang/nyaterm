use nyaterm_remote_desktop::RdpRedirectRequest;
use std::time::{Duration, Instant};

/// Runtime identity is separate from the saved connection and its reconnect target.
#[derive(Default)]
pub(in crate::features::remote_desktop) struct RdpRedirectState {
    pub(in crate::features::remote_desktop) target: Option<(String, u16)>,
    pub(in crate::features::remote_desktop) requires_relay: bool,
    pub(in crate::features::remote_desktop) request: Option<RdpRedirectRequest>,
    pub(in crate::features::remote_desktop) pending: bool,
    deadline: Option<Instant>,
    pub(in crate::features::remote_desktop) certificate_request_id: Option<String>,
}

impl RdpRedirectState {
    pub(in crate::features::remote_desktop) fn cancel(&mut self) {
        self.pending = false;
        self.deadline = None;
        self.certificate_request_id = None;
    }

    pub(in crate::features::remote_desktop) fn begin(
        &mut self,
        request: &RdpRedirectRequest,
    ) -> bool {
        if request.validate().is_err() || self.pending {
            return false;
        }
        let expected_hop = self.request.as_ref().map_or(1, |previous| previous.hop + 1);
        let generation = self
            .request
            .as_ref()
            .map_or(1, |previous| previous.generation);
        if request.hop != expected_hop || request.generation != generation {
            return false;
        }
        self.request = Some(request.clone());
        self.pending = true;
        self.deadline = Some(Instant::now() + Duration::from_secs(120));
        self.certificate_request_id = None;
        true
    }

    pub(in crate::features::remote_desktop) fn is_pending(
        &self,
        request: &RdpRedirectRequest,
    ) -> bool {
        self.pending
            && self.request.as_ref() == Some(request)
            && self
                .deadline
                .is_some_and(|deadline| Instant::now() < deadline)
    }

    pub(in crate::features::remote_desktop) fn accept(&mut self, request: &RdpRedirectRequest) {
        self.target = Some((request.host.clone(), request.port));
        self.pending = false;
    }

    pub(in crate::features::remote_desktop) fn matches_target(
        &self,
        host: &str,
        port: u16,
    ) -> bool {
        !self.pending
            && self
                .target
                .as_ref()
                .is_some_and(|(expected_host, expected_port)| {
                    expected_host
                        .trim_matches(['[', ']'])
                        .eq_ignore_ascii_case(host.trim_matches(['[', ']']))
                        && *expected_port == port
                })
    }
}

#[cfg(test)]
mod tests {
    use super::RdpRedirectState;
    use nyaterm_remote_desktop::RdpRedirectRequest;

    fn request(hop: u32) -> RdpRedirectRequest {
        RdpRedirectRequest {
            session_id: "session".into(),
            request_id: format!("request-{hop}"),
            generation: 1,
            hop,
            host: "worker.example".into(),
            connect_hosts: Vec::new(),
            port: 3389,
        }
    }

    #[test]
    fn redirect_requires_sequential_hops_and_rejects_overlapping_requests() {
        let mut state = RdpRedirectState::default();
        assert!(!state.begin(&request(2)));
        assert!(state.begin(&request(1)));
        assert!(!state.begin(&request(2)));
        assert!(!state.matches_target("worker.example", 3389));
        state.accept(&request(1));
        assert!(state.matches_target("worker.example", 3389));
        assert!(!state.begin(&request(1)));
        assert!(state.begin(&request(2)));
    }

    #[test]
    fn redirect_invalidates_certificate_and_rejects_previous_generation() {
        let mut state = RdpRedirectState {
            certificate_request_id: Some("old-certificate".into()),
            ..Default::default()
        };
        let mut next = request(1);
        next.generation = 2;
        assert!(!state.begin(&next));
        assert!(state.begin(&request(1)));
        assert!(state.certificate_request_id.is_none());
        assert!(!state.is_pending(&next));
    }
    #[test]
    fn failed_or_expired_route_does_not_authorize_new_certificate_identity() {
        let mut state = RdpRedirectState {
            target: Some(("broker.example".into(), 3389)),
            ..Default::default()
        };
        let redirect = request(1);
        assert!(state.begin(&redirect));
        assert!(!state.matches_target("worker.example", 3389));
        assert!(!state.matches_target("broker.example", 3389));
        state.deadline = Some(std::time::Instant::now());
        assert!(!state.is_pending(&redirect));
    }

    #[test]
    fn certificate_identity_uses_target_not_alternative_dial_host() {
        let mut state = RdpRedirectState::default();
        let mut redirect = request(1);
        redirect.connect_hosts = vec!["192.0.2.10".into()];
        assert!(state.begin(&redirect));
        state.accept(&redirect);
        assert!(state.matches_target("WORKER.EXAMPLE", 3389));
        assert!(!state.matches_target("192.0.2.10", 3389));
        assert!(!state.matches_target("worker.example", 3390));
    }
    #[test]
    fn terminating_session_revokes_pending_route_and_certificate() {
        let mut state = RdpRedirectState::default();
        let redirect = request(1);
        assert!(state.begin(&redirect));
        state.certificate_request_id = Some("certificate".into());
        state.cancel();
        assert!(!state.is_pending(&redirect));
        assert!(state.certificate_request_id.is_none());
    }
}
