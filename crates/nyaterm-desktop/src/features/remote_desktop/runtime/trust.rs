use super::errors::format_rdp_error;
use crate::features::NyaTermApp;
use crate::features::remote_desktop::state::RdpCertificatePrompt;
use gpui::Context;
use nyaterm_remote_desktop::CertificateDecision;
use nyaterm_remote_desktop::CertificateMatchState;
use nyaterm_remote_desktop::CertificatePromptReason;
use nyaterm_remote_desktop::RdpCertificatePolicy;
use nyaterm_remote_desktop::RdpCertificateRequest;
use nyaterm_remote_desktop::RdpCertificateResponse;
use nyaterm_remote_desktop::evaluate_certificate_match;
use nyaterm_store::RdpCertificateMetadata;
use nyaterm_store::RdpKnownHostCheck;
use nyaterm_store::StoreDomain;
use nyaterm_store::store_request;

impl NyaTermApp {
    fn rdp_certificate_is_current(
        &self,
        session_id: &str,
        request: &RdpCertificateRequest,
    ) -> bool {
        self.remote_desktop.sessions.get(session_id).is_some_and(|session| {
            !session.start_attempt.is_cancelled()
                && !matches!(session.state, nyaterm_remote_desktop::RemoteDesktopViewState::Failed | nyaterm_remote_desktop::RemoteDesktopViewState::Disconnected)
                && (session.redirection.matches_target(&request.host, request.port)
                    || (session.redirection.target.is_none() && !session.redirection.pending
                        && self.session.metadata(session_id).is_some_and(|metadata| {
                            matches!(&metadata.launch_config, crate::models::SessionLaunchConfig::Rdp(config)
                                if config.host == request.host && config.port == request.port)
                        })))
                && session.redirection.certificate_request_id.as_deref() == Some(request.request_id.as_str())
        }) && self.remote_desktop.manager.certificate_is_pending(session_id, &request.request_id)
    }

    pub(super) fn handle_vnc_key_request(
        &mut self,
        request: nyaterm_remote_desktop::VncServerKeyRequest,
        cx: &mut Context<Self>,
    ) {
        let Some(metadata) = self.session.metadata(&request.session_id) else {
            return;
        };
        let crate::models::SessionLaunchConfig::Vnc(config) = &metadata.launch_config else {
            return;
        };
        if config.host != request.host || config.port != request.port {
            let _ = self
                .remote_desktop
                .vnc_manager
                .respond_server_key(&request, false, false);
            return;
        }
        let host = request.host.clone();
        let port = request.port;
        let rejected = request.clone();
        let submitted = self.submit_store_request(
            0,
            store_request(StoreDomain::Security, move |store| {
                store.load_vnc_known_host(&host, port)
            }),
            move |this, event, cx| {
                let Some(trust) = this
                    .remote_desktop
                    .vnc_manager
                    .trust_state(&request.session_id)
                else {
                    return;
                };
                if !trust.is_pending(&request)
                    || !this
                        .remote_desktop
                        .sessions
                        .contains_key(&request.session_id)
                {
                    return;
                }
                match event.outcome {
                    Ok(record)
                        if record.as_ref().is_some_and(|record| {
                            record
                                .sha256_fingerprint
                                .eq_ignore_ascii_case(&request.sha256_fingerprint)
                        }) =>
                    {
                        let _ = this
                            .remote_desktop
                            .vnc_manager
                            .respond_server_key(&request, true, false);
                    }
                    Ok(record) => {
                        if let Some(session) =
                            this.remote_desktop.sessions.get_mut(&request.session_id)
                        {
                            session.vnc_key_request =
                                Some((request, record.map(|record| record.sha256_fingerprint)));
                        }
                    }
                    Err(_) => {
                        let _ = this
                            .remote_desktop
                            .vnc_manager
                            .respond_server_key(&request, false, false);
                    }
                }
                cx.notify();
            },
            cx,
        );
        if !submitted {
            let _ = self
                .remote_desktop
                .vnc_manager
                .respond_server_key(&rejected, false, false);
        }
    }

    pub(in crate::features::remote_desktop) fn resolve_vnc_key_request(
        &mut self,
        request: &nyaterm_remote_desktop::VncServerKeyRequest,
        accept: bool,
        remember: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.remote_desktop.sessions.get_mut(&request.session_id) else {
            return;
        };
        if !session
            .vnc_key_request
            .as_ref()
            .is_some_and(|(pending, _)| pending == request)
        {
            return;
        }
        let previous = session
            .vnc_key_request
            .take()
            .and_then(|(_, previous)| previous);
        session.vnc_trust_previous = previous;
        let _ = self
            .remote_desktop
            .vnc_manager
            .respond_server_key(request, accept, remember);
        cx.notify();
    }

    pub(super) fn handle_rdp_certificate_request(
        &mut self,
        session_id: &str,
        request: RdpCertificateRequest,
        cx: &mut Context<Self>,
    ) {
        let valid = self.remote_desktop.sessions.get(session_id).is_some_and(|session| {
            !session.start_attempt.is_cancelled()
                && !matches!(session.state, nyaterm_remote_desktop::RemoteDesktopViewState::Failed | nyaterm_remote_desktop::RemoteDesktopViewState::Disconnected)
                && (session.redirection.matches_target(&request.host, request.port)
                    || (session.redirection.target.is_none() && !session.redirection.pending
                        && self.session.metadata(session_id).is_some_and(|metadata| {
                            matches!(&metadata.launch_config, crate::models::SessionLaunchConfig::Rdp(config)
                                if config.host == request.host && config.port == request.port)
                        })))
        }) && self.remote_desktop.manager.certificate_is_pending(session_id, &request.request_id);
        if !valid {
            let _ = self
                .remote_desktop
                .manager
                .respond_certificate(&request.request_id, RdpCertificateResponse::Reject);
            return;
        }
        if let Some(session) = self.remote_desktop.sessions.get_mut(session_id) {
            session.redirection.certificate_request_id = Some(request.request_id.clone());
        }
        let policy = self
            .session
            .metadata(session_id)
            .and_then(|metadata| match &metadata.launch_config {
                crate::models::SessionLaunchConfig::Rdp(config) => Some(config.certificate_policy),
                _ => None,
            })
            .unwrap_or(RdpCertificatePolicy::Prompt);
        if policy == RdpCertificatePolicy::Insecure {
            self.apply_rdp_certificate_check(
                session_id,
                request,
                policy,
                RdpKnownHostCheck::UnknownHost,
                cx,
            );
            return;
        }
        let host = request.host.clone();
        let port = request.port;
        let fingerprint = request.sha256_fingerprint.clone();
        let request_id = request.request_id.clone();
        let failure_request_id = request_id.clone();
        let session_id = session_id.to_string();
        let submitted = self.submit_store_request(
            0,
            store_request(StoreDomain::Security, move |store| {
                store.check_rdp_known_host(&host, port, &fingerprint)
            }),
            move |this, event, cx| match event.outcome {
                Ok(check) if this.rdp_certificate_is_current(&session_id, &request) => {
                    this.apply_rdp_certificate_check(&session_id, request, policy, check, cx);
                }
                Ok(_) => {}
                Err(error) => {
                    if !this.rdp_certificate_is_current(&session_id, &request) {
                        return;
                    }
                    this.shell
                        .set_status(format!("RDP certificate verification failed: {error}"));
                    let _ = this
                        .remote_desktop
                        .manager
                        .respond_certificate(&request_id, RdpCertificateResponse::Reject);
                    cx.notify();
                }
            },
            cx,
        );
        if !submitted {
            let _ = self
                .remote_desktop
                .manager
                .respond_certificate(&failure_request_id, RdpCertificateResponse::Reject);
        }
    }

    pub(super) fn apply_rdp_certificate_check(
        &mut self,
        session_id: &str,
        request: RdpCertificateRequest,
        policy: RdpCertificatePolicy,
        check: RdpKnownHostCheck,
        cx: &mut Context<Self>,
    ) {
        if !self.rdp_certificate_is_current(session_id, &request) {
            return;
        }
        let match_state = match check {
            RdpKnownHostCheck::Match => CertificateMatchState::Match,
            RdpKnownHostCheck::UnknownHost => CertificateMatchState::FirstUse,
            RdpKnownHostCheck::Changed {
                remembered_fingerprint,
            } => CertificateMatchState::Changed {
                remembered_fingerprint,
            },
        };
        let expected_previous_fingerprint = match &match_state {
            CertificateMatchState::Changed {
                remembered_fingerprint,
            } => Some(remembered_fingerprint.clone()),
            CertificateMatchState::FirstUse | CertificateMatchState::Match => None,
        };
        let evaluation =
            evaluate_certificate_match(policy, match_state, &request.sha256_fingerprint);
        match evaluation.decision {
            CertificateDecision::Accept => {
                let _ = self
                    .remote_desktop
                    .manager
                    .respond_certificate(&request.request_id, RdpCertificateResponse::TrustOnce);
            }
            CertificateDecision::AcceptAndRemember => {
                self.persist_rdp_certificate_and_respond(
                    session_id,
                    request,
                    expected_previous_fingerprint,
                    cx,
                );
            }
            CertificateDecision::Reject => {
                let _ = self
                    .remote_desktop
                    .manager
                    .respond_certificate(&request.request_id, RdpCertificateResponse::Reject);
            }
            CertificateDecision::Prompt => {
                let Some(reason) = evaluation.prompt_reason else {
                    let _ = self
                        .remote_desktop
                        .manager
                        .respond_certificate(&request.request_id, RdpCertificateResponse::Reject);
                    return;
                };
                if let Some(session) = self.remote_desktop.sessions.get_mut(session_id) {
                    session.certificate_request = Some(RdpCertificatePrompt { request, reason });
                }
            }
        }
    }

    pub(in crate::features) fn resolve_rdp_certificate(
        &mut self,
        session_id: &str,
        request_id: &str,
        response: RdpCertificateResponse,
        cx: &mut Context<Self>,
    ) {
        let prompt = self
            .remote_desktop
            .sessions
            .get_mut(session_id)
            .filter(|session| {
                session
                    .certificate_request
                    .as_ref()
                    .is_some_and(|prompt| prompt.request.request_id == request_id)
            })
            .and_then(|session| session.certificate_request.take());
        let Some(prompt) = prompt else {
            return;
        };
        if !self.rdp_certificate_is_current(session_id, &prompt.request) {
            return;
        }
        let expected_previous_fingerprint = match &prompt.reason {
            CertificatePromptReason::FirstUse => None,
            CertificatePromptReason::Changed {
                previous_fingerprint,
                ..
            } => Some(previous_fingerprint.clone()),
        };
        if response == RdpCertificateResponse::TrustAndRemember {
            self.persist_rdp_certificate_and_respond(
                session_id,
                prompt.request,
                expected_previous_fingerprint,
                cx,
            );
            return;
        }
        let response = if matches!(prompt.reason, CertificatePromptReason::Changed { .. })
            && response == RdpCertificateResponse::TrustOnce
        {
            RdpCertificateResponse::Reject
        } else {
            response
        };
        if let Err(error) = self
            .remote_desktop
            .manager
            .respond_certificate(&prompt.request.request_id, response)
        {
            self.shell.set_status(format_rdp_error(&error));
        }
    }

    pub(super) fn persist_rdp_certificate_and_respond(
        &mut self,
        session_id: &str,
        request: RdpCertificateRequest,
        expected_previous_fingerprint: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if !self.rdp_certificate_is_current(session_id, &request) {
            return;
        }
        let attempt = self.remote_desktop.sessions[session_id]
            .start_attempt
            .clone();
        let manager = self.remote_desktop.manager.clone();
        let persistence_session_id = session_id.to_string();
        let persistence_request_id = request.request_id.clone();
        let session_id = session_id.to_string();
        let pending_request = request.clone();
        let request_id = request.request_id.clone();
        let failure_request_id = request_id.clone();
        let host = request.host;
        let port = request.port;
        let fingerprint = request.sha256_fingerprint;
        let metadata = RdpCertificateMetadata {
            subject: request.subject,
            issuer: request.issuer,
            valid_from: request.valid_from,
            valid_to: request.valid_to,
        };
        let submitted = self.submit_store_request(
            0,
            nyaterm_store::store_mutation(StoreDomain::Security, move |store| {
                if attempt.is_cancelled()
                    || !manager
                        .certificate_is_pending(&persistence_session_id, &persistence_request_id)
                {
                    return Ok(false);
                }
                // Authorization is checked when the queued write begins. A write already
                // authorized before cancellation may complete, but never releases a stale handshake.
                store.replace_rdp_known_host_if_matches(
                    &host,
                    port,
                    expected_previous_fingerprint.as_deref(),
                    &fingerprint,
                    metadata,
                )
            }),
            move |this, event, cx| {
                if !this.rdp_certificate_is_current(&session_id, &pending_request) {
                    return;
                }
                let response = match event.outcome {
                    Ok(true) => RdpCertificateResponse::TrustAndRemember,
                    Ok(false) => {
                        this.shell.set_status(
                            "RDP certificate changed again before confirmation; connection rejected"
                                .to_string(),
                        );
                        RdpCertificateResponse::Reject
                    }
                    Err(error) => {
                        this.shell.set_status(format!(
                            "RDP certificate could not be remembered: {error}"
                        ));
                        RdpCertificateResponse::Reject
                    }
                };
                if let Err(error) = this
                    .remote_desktop
                    .manager
                    .respond_certificate(&request_id, response)
                {
                    this.shell.set_status(format_rdp_error(&error));
                }
                cx.notify();
            },
            cx,
        );
        if !submitted {
            let _ = self
                .remote_desktop
                .manager
                .respond_certificate(&failure_request_id, RdpCertificateResponse::Reject);
        }
    }
}
