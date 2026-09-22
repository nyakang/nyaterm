use rust_i18n::t;

use futures::StreamExt as _;
use gpui::{ClipboardItem, Context};

use crate::features::NyaTermApp;
use crate::http::cloud_sync::run_github_gist_device_flow;
use crate::models::{GithubGistAuthEvent, GithubGistAuthJobEvent};

impl NyaTermApp {
    pub(in crate::features) fn start_github_gist_auth(&mut self, cx: &mut Context<Self>) {
        if !self.cloud_sync_form_enabled() {
            return;
        }
        let waiting_message = t!("settings.githubGistWaitingForAuth").to_string();
        let Some(job) = self.cloud_sync.begin_github_auth(waiting_message) else {
            return;
        };
        let job_id = job.job_id();
        let existing_gist_id = job.existing_gist_id();
        let cancel = job.cancel();
        let tx = job.sender();
        let rejected_tx = tx.clone();
        if let Err(error) = self
            .blocking_jobs
            .submit_detached("github-gist-auth", move |_| {
                run_github_gist_device_flow(job_id, existing_gist_id, cancel, tx);
            })
        {
            let _ = rejected_tx.unbounded_send(GithubGistAuthJobEvent {
                job_id,
                event: GithubGistAuthEvent::Failed(error.to_string()),
            });
        }
        cx.notify();
    }

    pub(in crate::features) fn cancel_github_gist_auth(&mut self, cx: &mut Context<Self>) {
        self.cloud_sync.cancel_github_auth();
        cx.notify();
    }

    pub(in crate::features) fn copy_github_gist_user_code(&mut self, cx: &mut Context<Self>) {
        let Some(code) = self.cloud_sync.github_auth().user_code.clone() else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(code));
        self.cloud_sync
            .set_status(t!("settings.githubGistUserCodeCopied").to_string());
        cx.notify();
    }

    pub(in crate::features) fn open_github_gist_verification_url(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        let Some(url) = self.cloud_sync.github_auth().verification_uri.clone() else {
            return;
        };
        self.open_external_url_for_ui(&url, cx);
    }

    /// Deliver GitHub Gist device-flow events as they arrive.
    ///
    /// Started once at window open. Before this the runtime tick polled
    /// `rx.try_recv`, and cloud sync was named nowhere in
    /// `runtime_quiet_tick_allowed`, so on an otherwise idle app -- exactly the
    /// state of a user waiting on device auth -- every event, including the
    /// `Started` that shows the user code and opens the verification URL, waited
    /// up to one quiet interval.
    pub(in crate::features) fn start_github_gist_auth_event_drain(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        let Some(mut rx) = self.cloud_sync.take_github_auth_event_receiver() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            while let Some(job) = rx.next().await {
                if this
                    .update(cx, |this, cx| {
                        if let Some(event) = this.cloud_sync.accept_github_auth_event(job) {
                            this.apply_github_gist_auth_event(event, cx);
                            this.request_settings_panel_refresh(cx);
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    fn apply_github_gist_auth_event(&mut self, event: GithubGistAuthEvent, cx: &mut Context<Self>) {
        match event {
            GithubGistAuthEvent::Started {
                user_code,
                verification_uri,
            } => {
                let message = t!("settings.githubGistWaitingForAuth").to_string();
                self.cloud_sync.apply_github_auth_started(
                    user_code,
                    verification_uri.clone(),
                    message,
                );
                self.open_external_url_for_ui(&verification_uri, cx);
            }
            GithubGistAuthEvent::Polling { slow_down } => {
                let message = t!(if slow_down {
                    "settings.githubGistSlowDown"
                } else {
                    "settings.githubGistWaitingForAuth"
                })
                .to_string();
                self.cloud_sync.apply_github_auth_polling(message);
            }
            GithubGistAuthEvent::Succeeded {
                access_token,
                gist_id,
                login,
            } => {
                let message = t!("settings.githubGistConnected").to_string();
                self.cloud_sync
                    .apply_github_auth_succeeded(access_token, gist_id, login, message);
                self.shell.set_status(self.cloud_sync.status().to_string());
            }
            GithubGistAuthEvent::Failed(error) => {
                let message = if error.contains("OAuth Client ID is not configured") {
                    t!("settings.githubGistClientIdMissing").to_string()
                } else {
                    error
                };
                self.cloud_sync.apply_github_auth_failed(message);
                self.shell.set_status(self.cloud_sync.status().to_string());
            }
            GithubGistAuthEvent::Cancelled => {
                self.cloud_sync.apply_github_auth_cancelled();
            }
        }
    }
}
