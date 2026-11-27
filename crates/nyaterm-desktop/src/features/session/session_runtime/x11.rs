use crate::features::NyaTermApp;
use futures::StreamExt;
use futures::channel::mpsc::{UnboundedSender, unbounded};
use gpui::Context;
use nyaterm_core::plugins::native::NativeComponentId;
use nyaterm_core::x11::{X11Error, X11Mode, X11PreparationChoice};
use nyaterm_plugin_host::native::{NativeAction, NativeService};
use nyaterm_transport::{SshSessionConfig, connection_attempt::ConnectionAttempt};
use rust_i18n::t;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, mpsc};
use std::time::Duration;

struct Prompt {
    install: bool,
    detail: String,
    attempt: ConnectionAttempt,
    reply: mpsc::SyncSender<X11PreparationChoice>,
}

pub(super) struct X11Startup {
    mode: X11Mode,
    display: String,
    clipboard: bool,
    service: Option<Arc<NativeService>>,
    prompts: UnboundedSender<Prompt>,
}

impl X11Startup {
    fn ask(
        &self,
        install: bool,
        detail: String,
        attempt: &ConnectionAttempt,
    ) -> Result<X11PreparationChoice, String> {
        let (reply, receive) = mpsc::sync_channel(1);
        self.prompts
            .unbounded_send(Prompt {
                install,
                detail,
                attempt: attempt.clone(),
                reply,
            })
            .map_err(|_| "X11 preparation closed".to_string())?;
        attempt.receive(&receive, Duration::from_secs(600))
    }

    fn wait<T>(
        &self,
        mut result: futures::channel::oneshot::Receiver<T>,
        attempt: &ConnectionAttempt,
    ) -> Result<T, String> {
        loop {
            attempt.check()?;
            match result.try_recv() {
                Ok(Some(value)) => return Ok(value),
                Ok(None) => std::thread::sleep(Duration::from_millis(25)),
                Err(_) => return Err("X11 component request closed".into()),
            }
        }
    }

    pub(super) fn prepare(&self, config: &mut SshSessionConfig) -> Result<(), String> {
        config.x11_connection = None;
        if !config.x11_forwarding {
            return Ok(());
        }
        let attempt = &config.attempt;
        attempt.check()?;
        let result = (|| {
            if self
                .mode
                .uses_managed(cfg!(all(windows, target_arch = "x86_64")), &self.display)
            {
                let service = self
                    .service
                    .as_ref()
                    .ok_or_else(|| t!("x11.componentUnavailable").to_string())?;
                let id = NativeComponentId::X11;
                let acquired = self.wait(
                    service.acquire(id).map_err(|error| error.to_string())?,
                    attempt,
                )?;
                let launch = match acquired {
                    Ok(launch) => launch,
                    Err(error)
                        if matches!(
                            error.code,
                            nyaterm_core::plugins::ErrorCode::NotFound
                                | nyaterm_core::plugins::ErrorCode::Incompatible
                                | nyaterm_core::plugins::ErrorCode::InvalidManifest
                                | nyaterm_core::plugins::ErrorCode::Io
                        ) =>
                    {
                        match self.ask(true, t!("x11.installDescription").to_string(), attempt)? {
                            X11PreparationChoice::Install => {}
                            X11PreparationChoice::ContinueWithout => return Ok(None),
                            X11PreparationChoice::Cancel => {
                                attempt.cancel();
                                return Err(t!("x11.cancelled").to_string());
                            }
                        }
                        self.wait(
                            service
                                .submit(id, NativeAction::Install)
                                .map_err(|error| error.to_string())?,
                            attempt,
                        )?
                        .map_err(|error| error.to_string())?;
                        self.wait(
                            service.acquire(id).map_err(|error| error.to_string())?,
                            attempt,
                        )?
                        .map_err(|error| error.to_string())?
                    }
                    Err(error) => return Err(error.to_string()),
                };
                nyaterm_x11::start_managed(launch, self.clipboard, &|| attempt.is_cancelled())
                    .map(Some)
                    .map_err(error_text)
            } else {
                nyaterm_x11::prepare_external(&self.mode, &self.display, &|| attempt.is_cancelled())
                    .map(Some)
                    .map_err(error_text)
            }
        })();
        attempt.check()?;
        match result {
            Ok(connection) => {
                config.x11_forwarding = connection.is_some();
                config.x11_connection = connection;
                Ok(())
            }
            Err(error) => {
                if self.ask(false, error, attempt)? == X11PreparationChoice::ContinueWithout {
                    config.x11_forwarding = false;
                    Ok(())
                } else {
                    attempt.cancel();
                    Err(t!("x11.cancelled").to_string())
                }
            }
        }
    }
}

fn error_text(error: X11Error) -> String {
    t!(match error {
        X11Error::Cancelled => "x11.cancelled",
        X11Error::NoDisplay => "x11.noDisplay",
        X11Error::XQuartzUnavailable => "x11.xquartzUnavailable",
        X11Error::Unsupported => "x11.unsupported",
        X11Error::Authentication => "x11.authentication",
        X11Error::Startup => "x11.startup",
        X11Error::Protocol => "x11.protocol",
    })
    .to_string()
}

impl NyaTermApp {
    pub(super) fn x11_startup(&self, cx: &mut Context<Self>) -> X11Startup {
        let (prompts, mut receiver) = unbounded::<Prompt>();
        cx.spawn(async move |this, cx| {
            while let Some(prompt) = receiver.next().await {
                if prompt.attempt.is_cancelled() {
                    continue;
                }
                let _ = NyaTermApp::update_in_main_window(&this, cx, move |app, window, cx| {
                    if prompt.attempt.is_cancelled() {
                        return;
                    }
                    let reply = Rc::new(RefCell::new(Some(prompt.reply)));
                    let attempt = prompt.attempt;
                    if prompt.install {
                        app.open_x11_install_dialog(
                            prompt.detail,
                            move |choice| {
                                if let Some(reply) = reply.borrow_mut().take() {
                                    let _ = reply.send(if attempt.is_cancelled() {
                                        X11PreparationChoice::Cancel
                                    } else {
                                        choice
                                    });
                                }
                            },
                            window,
                            cx,
                        );
                    } else {
                        let accept = reply.clone();
                        app.open_confirm_dialog_with_cancel(
                            (
                                t!("x11.unavailableTitle").to_string(),
                                prompt.detail,
                                t!("x11.continueWithout").to_string(),
                                false,
                                move |_, _, _| {
                                    if let Some(reply) = accept.borrow_mut().take() {
                                        let _ = reply.send(if attempt.is_cancelled() {
                                            X11PreparationChoice::Cancel
                                        } else {
                                            X11PreparationChoice::ContinueWithout
                                        });
                                    }
                                    true
                                },
                                move |_, _| {
                                    if let Some(reply) = reply.borrow_mut().take() {
                                        let _ = reply.send(X11PreparationChoice::Cancel);
                                    }
                                },
                            ),
                            window,
                            cx,
                        );
                    }
                });
            }
        })
        .detach();
        X11Startup {
            mode: self.settings.summary().x11_mode.clone(),
            display: self.settings.summary().x11_display.clone(),
            clipboard: self.settings.summary().x11_clipboard,
            service: self.native_component_service(cx),
            prompts,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::features::session::session_runtime::x11::{Prompt, X11Startup};
    use futures::channel::mpsc::{UnboundedReceiver, unbounded};
    use nyaterm_core::x11::{X11Mode, X11PreparationChoice};
    use nyaterm_transport::SshSessionConfig;
    use std::time::{Duration, Instant};

    fn fixture() -> (X11Startup, UnboundedReceiver<Prompt>) {
        let (prompts, receiver) = unbounded();
        (
            X11Startup {
                mode: X11Mode::Unknown("future".into()),
                display: String::new(),
                clipboard: false,
                service: None,
                prompts,
            },
            receiver,
        )
    }
    fn prompt(receiver: &mut UnboundedReceiver<Prompt>) -> Prompt {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Ok(value) = receiver.try_recv() {
                return value;
            }
            assert!(Instant::now() < deadline, "missing X11 prompt");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    #[test]
    fn x11_failure_only_disables_forwarding_after_explicit_continue() {
        let (startup, mut receiver) = fixture();
        let worker = std::thread::spawn(move || {
            let mut config = SshSessionConfig {
                x11_forwarding: true,
                ..Default::default()
            };
            startup.prepare(&mut config).unwrap();
            assert!(!config.x11_forwarding);
            assert!(config.x11_connection.is_none());
        });
        let request = prompt(&mut receiver);
        assert!(!request.install);
        request
            .reply
            .send(X11PreparationChoice::ContinueWithout)
            .unwrap();
        worker.join().unwrap();
    }
    #[test]
    fn x11_cancelled_request_cannot_resume_from_a_late_confirmation() {
        let (startup, mut receiver) = fixture();
        let worker = std::thread::spawn(move || {
            let mut config = SshSessionConfig {
                x11_forwarding: true,
                ..Default::default()
            };
            assert!(startup.prepare(&mut config).is_err());
            assert!(config.x11_connection.is_none());
        });
        let request = prompt(&mut receiver);
        request.attempt.cancel();
        let _ = request.reply.send(X11PreparationChoice::ContinueWithout);
        worker.join().unwrap();
    }
}
