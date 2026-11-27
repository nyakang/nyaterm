use gpui::Context;
use nyaterm_remote_desktop::{RdpRedirectRequest, RdpRedirectResponse};

use crate::features::NyaTermApp;

impl NyaTermApp {
    pub(super) fn handle_rdp_redirect_request(
        &mut self,
        session_id: &str,
        request: RdpRedirectRequest,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.remote_desktop.sessions.get_mut(session_id) else {
            return;
        };
        if session.start_attempt.is_cancelled()
            || matches!(
                session.state,
                nyaterm_remote_desktop::RemoteDesktopViewState::Failed
                    | nyaterm_remote_desktop::RemoteDesktopViewState::Disconnected
            )
            || request.session_id != session_id
            || !session.redirection.begin(&request)
        {
            let _ = self.remote_desktop.manager.respond_redirect(
                &request,
                RdpRedirectResponse::Rejected {
                    message: "Invalid or stale RDP redirection request".into(),
                },
            );
            return;
        }
        session.certificate_request = None;
        let token = session.start_token.clone();
        if !session.redirection.requires_relay {
            if self
                .remote_desktop
                .manager
                .respond_redirect(&request, RdpRedirectResponse::Ready { relay: None })
                .is_ok()
            {
                session.redirection.accept(&request);
            }
            return;
        }
        let Some(source) = self.remote_desktop.routes.get(session_id).cloned() else {
            let _ = self.remote_desktop.manager.respond_redirect(
                &request,
                RdpRedirectResponse::Rejected {
                    message: "Cannot reconstruct the original RDP proxy or SSH route".into(),
                },
            );
            return;
        };
        let attempt = session.start_attempt.clone();
        let destination = (
            request.host.clone(),
            request.port,
            request.connect_hosts.clone(),
        );
        let task = self
            .blocking_jobs
            .submit_task("rdp-redirect-route", move |_| {
                attempt.check()?;
                let route = source
                    .retarget(destination.0, destination.1, destination.2)
                    .map_err(|error| error.to_string())?;
                attempt.check()?;
                Ok::<_, String>(route)
            });
        cx.spawn(async move |this, cx| {
            let result = crate::features::runtime_jobs::await_blocking_job(task)
                .await
                .and_then(|result| result);
            let _ = this.update(cx, |app, cx| {
                if !app
                    .remote_desktop
                    .start_matches(&request.session_id, &token)
                    || !app
                        .remote_desktop
                        .sessions
                        .get(&request.session_id)
                        .is_some_and(|session| session.redirection.is_pending(&request))
                {
                    return;
                }
                match result {
                    Ok(route) => {
                        let response = RdpRedirectResponse::Ready {
                            relay: Some(route.endpoint.clone().into()),
                        };
                        if app
                            .remote_desktop
                            .manager
                            .respond_redirect(&request, response)
                            .is_ok()
                        {
                            // Replacing the route cancels the old endpoint without waiting on GPUI.
                            app.remote_desktop
                                .routes
                                .insert(request.session_id.clone(), route);
                            if let Some(session) =
                                app.remote_desktop.sessions.get_mut(&request.session_id)
                            {
                                session.redirection.accept(&request);
                            }
                        }
                    }
                    Err(message) => {
                        let _ = app
                            .remote_desktop
                            .manager
                            .respond_redirect(&request, RdpRedirectResponse::Rejected { message });
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}
