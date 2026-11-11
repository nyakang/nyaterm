use std::sync::Arc;

use gpui::Context;
use nyaterm_remote_desktop::{RdpSessionManager, RemoteDesktopError, VncSessionManager};

use crate::features::NyaTermApp;
use crate::features::runtime_jobs::await_blocking_job;
use crate::models::SessionLaunchConfig;

enum HelperManager {
    Rdp(Arc<RdpSessionManager>),
    Vnc(Arc<VncSessionManager>),
}

/// A successful launch is owned by its result until the window accepts it.
/// Dropping an undeliverable result reaps only this helper. A pending launch
/// remains reserved until delivery, preventing another launch under the same id.
struct HelperStartResult {
    manager: HelperManager,
    session_id: String,
    result: Result<(), RemoteDesktopError>,
    accepted: bool,
}

impl Drop for HelperStartResult {
    fn drop(&mut self) {
        if self.accepted || self.result.is_err() {
            return;
        }
        match &self.manager {
            HelperManager::Rdp(manager) => {
                if manager.close_detached(&self.session_id).is_err() {
                    let manager = manager.clone();
                    let id = self.session_id.clone();
                    let _ = nyaterm_core::runtime::worker::spawn(
                        "remote-helper-cleanup".into(),
                        move || {
                            let _ = manager.close(&id);
                        },
                    );
                }
            }
            HelperManager::Vnc(manager) => {
                if manager.close_detached(&self.session_id).is_err() {
                    let manager = manager.clone();
                    let id = self.session_id.clone();
                    let _ = nyaterm_core::runtime::worker::spawn(
                        "remote-helper-cleanup".into(),
                        move || {
                            let _ = manager.close(&id);
                        },
                    );
                }
            }
        }
    }
}

impl NyaTermApp {
    pub(super) fn spawn_remote_desktop_helper(
        &mut self,
        session_id: &str,
        config: SessionLaunchConfig,
        cx: &mut Context<Self>,
    ) {
        if self.remote_desktop.helper_launches.contains_key(session_id) {
            return;
        }
        let Some(session) = self.remote_desktop.sessions.get(session_id) else {
            return;
        };
        let token = session.start_token.clone();
        let attempt = session.start_attempt.clone();
        self.remote_desktop
            .helper_launches
            .insert(session_id.to_string(), token.clone());
        let manager = match &config {
            SessionLaunchConfig::Rdp(_) => HelperManager::Rdp(self.remote_desktop.manager.clone()),
            SessionLaunchConfig::Vnc(_) => {
                HelperManager::Vnc(self.remote_desktop.vnc_manager.clone())
            }
            _ => unreachable!("remote desktop helper config"),
        };
        let id = session_id.to_string();
        let task = self
            .blocking_jobs
            .submit_task("remote-desktop-helper-start", move |_| {
                let result = if attempt.is_cancelled() {
                    Err(nyaterm_remote_desktop::RdpError::new(
                        nyaterm_remote_desktop::RdpErrorKind::Transport,
                        "connection attempt cancelled",
                    )
                    .into())
                } else {
                    match (&manager, config) {
                        (HelperManager::Rdp(manager), SessionLaunchConfig::Rdp(config)) => manager
                            .create_session_with_id(id.clone(), config)
                            .map(|_| ())
                            .map_err(Into::into),
                        (HelperManager::Vnc(manager), SessionLaunchConfig::Vnc(config)) => manager
                            .create_session_with_id(id.clone(), config)
                            .map(|_| ())
                            .map_err(Into::into),
                        _ => unreachable!("matching helper manager"),
                    }
                };
                HelperStartResult {
                    manager,
                    session_id: id,
                    result,
                    accepted: false,
                }
            });
        let id = session_id.to_string();
        cx.spawn(async move |this, cx| {
            let result = await_blocking_job(task).await;
            let _ = this.update(cx, |this, cx| {
                if this.remote_desktop.helper_launches.get(&id) != Some(&token) {
                    return;
                }
                this.remote_desktop.helper_launches.remove(&id);
                if !this.remote_desktop.start_matches(&id, &token) {
                    return;
                }
                match result {
                    Ok(mut started) => {
                        started.accepted = true;
                        if let Err(error) = &started.result {
                            this.remote_desktop.routes.remove(&id);
                            this.shell.set_status(format!(
                                "remote desktop connection failed: {}",
                                error.message
                            ));
                            if let Some(session) = this.remote_desktop.sessions.get_mut(&id) {
                                session.start_attempt.cancel();
                                super::events::set_remote_view_error(session, error.clone());
                            }
                        } else if this
                            .remote_desktop
                            .sessions
                            .get(&id)
                            .is_some_and(|session| {
                                matches!(session.state,
                                nyaterm_remote_desktop::RemoteDesktopViewState::Connecting
                                | nyaterm_remote_desktop::RemoteDesktopViewState::Connected
                                | nyaterm_remote_desktop::RemoteDesktopViewState::Reconnecting)
                            })
                            && let Some(metadata) = this.session.metadata_mut(&id)
                        {
                            metadata.disconnected = false;
                        }
                    }
                    Err(error) => {
                        this.remote_desktop.routes.remove(&id);
                        this.shell
                            .set_status(format!("remote desktop connection failed: {error}"));
                        this.remote_desktop.insert_failed_session(
                            id,
                            nyaterm_remote_desktop::RdpErrorKind::Transport,
                            error,
                        );
                    }
                }
                this.settle_remote_desktop_restore(cx);
                cx.notify();
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use gpui::TestAppContext;
    use nyaterm_transport::network_route::{NetworkRoute, NetworkRouteConfig};

    use crate::blocking_jobs::BlockingJobScheduler;
    use crate::features::test_support::app_with_visible_local_session;
    use crate::test_support::{TestAppCleanup, TestConfigDir};

    fn occupied_workers(queue_capacity: usize) -> (BlockingJobScheduler, Vec<mpsc::Sender<()>>) {
        let scheduler = BlockingJobScheduler::with_limits(2, queue_capacity).unwrap();
        let (entered, waiting) = mpsc::channel();
        let mut releases = Vec::new();
        for _ in 0..2 {
            let (send, receive) = mpsc::channel::<()>();
            releases.push(send);
            let entered = entered.clone();
            scheduler
                .submit_detached("hold-test-worker", move |_| {
                    entered.send(()).unwrap();
                    let _ = receive.recv();
                })
                .unwrap();
            waiting.recv_timeout(Duration::from_secs(5)).unwrap();
        }
        (scheduler, releases)
    }

    #[test]
    fn helper_launches_queue_independently_suppress_reentry_and_cancel_before_spawn() {
        for protocol in ["rdp", "vnc"] {
            let (scheduler, release) = occupied_workers(8);
            let root = TestConfigDir::new("nyaterm-cancel-queued-helpers");
            let mut cx = TestAppContext::single();
            let _cleanup = TestAppCleanup::new(&cx);
            let worker_release = release;
            let app = app_with_visible_local_session(&mut cx, root.path(), "existing");
            app.update(&mut cx, |app, cx| {
                app.blocking_jobs = scheduler.clone();
                let saved = serde_json::from_value(serde_json::json!({
                    "id":"test", "name":"test", "type":protocol, "host":"127.0.0.1"
                }))
                .unwrap();
                let mut config = app
                    .saved_remote_desktop_launch_config(&saved, None)
                    .unwrap();
                match &mut config {
                    crate::models::SessionLaunchConfig::Rdp(config) => {
                        config.password = Some("test-fixture".into());
                        config.display.mode = nyaterm_remote_desktop::RdpDisplayMode::Fixed;
                    }
                    crate::models::SessionLaunchConfig::Vnc(config) => {
                        config.password = Some("test-fixture".into());
                    }
                    _ => unreachable!(),
                }
                app.session.metadata_mut("existing").unwrap().launch_config = config.clone();
                let attempt = nyaterm_transport::connection_attempt::ConnectionAttempt::default();
                app.remote_desktop.insert_connecting_for_request(
                    "existing".into(),
                    "prepared-request".into(),
                    attempt.clone(),
                );
                if protocol == "rdp" {
                    app.restart_rdp_runtime("existing", true, cx);
                } else {
                    app.restart_vnc_runtime("existing", true, cx);
                }
                assert_eq!(
                    app.remote_desktop.sessions["existing"].start_token,
                    "prepared-request"
                );
                assert!(!attempt.is_cancelled());
                app.remote_desktop.insert_connecting("second".into());
                app.spawn_remote_desktop_helper("second", config.clone(), cx);
                assert_eq!(app.remote_desktop.helper_launches.len(), 2);
                let submitted = scheduler.metrics().submitted;
                app.spawn_remote_desktop_helper("existing", config.clone(), cx);
                app.retry_rdp_runtime("existing", cx);
                assert_eq!(scheduler.metrics().submitted, submitted);
                app.remote_desktop.remove_session("existing");
                assert!(attempt.is_cancelled());
                assert!(app.remote_desktop.start_is_pending("second"));
                app.remote_desktop.remove_session("second");
            });
            drop(worker_release);
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                cx.run_until_parked();
                if app.update(&mut cx, |app, _| {
                    app.remote_desktop.helper_launches.is_empty()
                }) {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "cancelled launch results were not delivered"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
            app.update(&mut cx, |app, _| {
                assert!(!app.remote_desktop.has_sessions());
                assert!(app.remote_desktop.vnc_manager.state("existing").is_none());
                assert!(app.remote_desktop.manager.state("existing").is_none());
            });
        }
    }

    #[test]
    fn rejected_helper_start_keeps_its_failure_tab_and_releases_only_its_route() {
        for protocol in ["rdp", "vnc"] {
            let (scheduler, release) = occupied_workers(1);
            scheduler
                .submit_detached("fill-test-queue", |_| {})
                .unwrap();
            let root = TestConfigDir::new("nyaterm-rejected-helper-route");
            let mut cx = TestAppContext::single();
            let _cleanup = TestAppCleanup::new(&cx);
            let worker_release = release;
            let app = app_with_visible_local_session(&mut cx, root.path(), "existing");
            let (route, attempt) = app.update(&mut cx, |app, cx| {
                app.blocking_jobs = scheduler.clone();
                let route = NetworkRoute::start(NetworkRouteConfig {
                    host: "127.0.0.1".into(),
                    port: 9,
                    proxy: None,
                    jump: None,
                })
                .unwrap();
                let weak = std::sync::Arc::downgrade(&route);
                let relay = route.endpoint.clone();
                app.remote_desktop.routes.insert("existing".into(), route);
                let sibling_route = NetworkRoute::start(NetworkRouteConfig {
                    host: "127.0.0.1".into(),
                    port: 9,
                    proxy: None,
                    jump: None,
                })
                .unwrap();
                app.remote_desktop
                    .routes
                    .insert("sibling".into(), sibling_route);
                app.remote_desktop.insert_connecting("sibling".into());
                let attempt = nyaterm_transport::connection_attempt::ConnectionAttempt::default();
                app.remote_desktop.insert_connecting_for_request(
                    "existing".into(),
                    "rejected-request".into(),
                    attempt.clone(),
                );
                let saved = serde_json::from_value(serde_json::json!({
                    "id":"test", "name":"test", "type":protocol, "host":"127.0.0.1"
                }))
                .unwrap();
                let config = app
                    .saved_remote_desktop_launch_config(&saved, Some(relay))
                    .unwrap();
                app.session.metadata_mut("existing").unwrap().launch_config = config.clone();
                app.spawn_remote_desktop_helper("existing", config, cx);
                (weak, attempt)
            });
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                cx.run_until_parked();
                if app.update(&mut cx, |app, _| {
                    app.remote_desktop.helper_launches.is_empty()
                }) {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "queue rejection was not delivered"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
            app.update(&mut cx, |app, _| {
                let session = &app.remote_desktop.sessions["existing"];
                assert_eq!(
                    session.state,
                    nyaterm_remote_desktop::RemoteDesktopViewState::Failed
                );
                assert!(session.error.as_ref().unwrap().message.contains("queue"));
                assert!(app.session.has_session("existing"));
                assert!(attempt.is_cancelled());
                assert!(route.upgrade().is_none());
                assert!(app.remote_desktop.routes.contains_key("sibling"));
                assert!(
                    !app.remote_desktop.sessions["sibling"]
                        .start_attempt
                        .is_cancelled()
                );
            });
            drop(worker_release);
        }
    }
}
