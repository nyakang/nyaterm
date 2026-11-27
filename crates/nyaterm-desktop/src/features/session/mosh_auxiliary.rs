//! Auxiliary SSH can reconnect without replacing the Mosh terminal or its UDP key.
use crate::features::NyaTermApp;
use crate::models::SessionLaunchConfig;
use gpui::{Context, Task};
use nyaterm_transport::connection_attempt::ConnectionAttempt;
use std::time::Duration;

pub(super) struct AuxiliaryJob {
    pub _task: Task<()>,
    pub attempt: ConnectionAttempt,
}
impl Drop for AuxiliaryJob {
    fn drop(&mut self) {
        self.attempt.cancel();
    }
}

impl NyaTermApp {
    pub(in crate::features) fn ensure_mosh_auxiliary_services(&mut self, cx: &mut Context<Self>) {
        let ids = self.session.mosh_auxiliary_candidates();
        for id in ids {
            let attempt = ConnectionAttempt::default();
            let worker_attempt = attempt.clone();
            let worker_id = id.clone();
            let task = cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_secs(10))
                        .await;
                    let preparation = this.update(cx, |app, _| {
                        let metadata = app.session.metadata(&worker_id)?;
                        if metadata.disconnected {
                            return None;
                        }
                        let SessionLaunchConfig::Mosh(config) = &metadata.launch_config else {
                            return None;
                        };
                        let key = metadata.ssh_multiplex_key.clone()?;
                        let mut config = config.ssh.clone();
                        config.bind_attempt(worker_attempt.clone());
                        Some((
                            app.session.ssh_connection_for_session(&worker_id).is_some(),
                            key,
                            config,
                            app.session.ssh_connection_pool(),
                            app.blocking_jobs.clone(),
                        ))
                    });
                    let Ok(Some((connected, key, config, pool, scheduler))) = preparation else {
                        break;
                    };
                    if connected {
                        continue;
                    }
                    let (tx, rx) = futures::channel::oneshot::channel();
                    let expected_key = key.clone();
                    if scheduler
                        .submit_detached("mosh-auxiliary-ssh", move |_| {
                            let result = nyaterm_transport::open_ssh_multiplex_handle(config)
                                .map(|handle| pool.register(key, handle));
                            let _ = tx.send(result);
                        })
                        .is_err()
                    {
                        continue;
                    }
                    if let Ok(Ok(lease)) = rx.await {
                        let applied = this
                            .update(cx, |app, cx| {
                                let valid =
                                    app.session.metadata(&worker_id).is_some_and(|metadata| {
                                        !metadata.disconnected
                                            && metadata.ssh_multiplex_key.as_ref()
                                                == Some(&expected_key)
                                    });
                                if valid {
                                    app.session
                                        .register_ssh_connection(worker_id.clone(), lease);
                                    cx.notify();
                                }
                                valid
                            })
                            .unwrap_or(false);
                        if !applied {
                            break;
                        }
                    }
                }
            });
            self.session.register_mosh_auxiliary(
                id,
                AuxiliaryJob {
                    _task: task,
                    attempt,
                },
            );
        }
    }
}
