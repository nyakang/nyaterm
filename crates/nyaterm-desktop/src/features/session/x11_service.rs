//! Process-wide X11 preparation and UI routing. SSH transports see only the
//! core provider contract; installing a component never occupies an SSH worker.
use crate::app_shell::DesktopController;
use crate::features::NyaTermApp;
use futures::StreamExt;
use futures::channel::mpsc::{UnboundedSender, unbounded};
use gpui::Context;
use nyaterm_core::WorkspaceId;
use nyaterm_core::plugins::native::NativeComponentId;
use nyaterm_core::x11::{X11Connection, X11Error, X11PreparationChoice};
use nyaterm_plugin_host::native::{NativeAction, NativeService};
use nyaterm_ui::NyaDialogWindowExt;
use nyaterm_ui::notification::NyaNotificationKind;
use nyaterm_x11::manager::{StartContext, X11ServerManager};
use rust_i18n::t;
use std::sync::{Arc, mpsc};
use std::time::Duration;

enum Event {
    Install {
        workspace: WorkspaceId,
        context: StartContext,
        reply: mpsc::SyncSender<X11PreparationChoice>,
    },
    Finished {
        workspace: WorkspaceId,
        result: Result<(), X11Error>,
    },
}

pub(crate) struct X11Service {
    pub(crate) manager: X11ServerManager,
    events: UnboundedSender<Event>,
}
impl X11Service {
    pub(crate) fn new(cx: &mut Context<DesktopController>) -> Arc<Self> {
        let (events, mut receiver) = unbounded();
        cx.spawn(async move |this, cx| {
            while let Some(event) = receiver.next().await {
                match event {
                    Event::Finished { workspace, result } => {
                        let app = this
                            .update(cx, |controller, cx| {
                                controller.x11_prompt_app(workspace, cx)
                            })
                            .ok()
                            .flatten();
                        if let Some(app) = app {
                            let _ = app.update(cx, |app, cx| {
                                app.notify_operation_at(
                                    app.shell.main_window().map(Into::into),
                                    "x11-installed",
                                    if result.is_ok() {
                                        NyaNotificationKind::Success
                                    } else {
                                        NyaNotificationKind::Error
                                    },
                                    match result {
                                        Ok(()) => t!("x11.installedRetry").to_string(),
                                        Err(error) => error_message(error),
                                    },
                                    cx,
                                );
                            });
                        }
                    }
                    Event::Install {
                        workspace,
                        context,
                        reply,
                    } => {
                        // Re-evaluate the window while waiting: closing the first
                        // workspace must not strand another workspace's request.
                        let mut shown: Option<(
                            gpui::WeakEntity<NyaTermApp>,
                            nyaterm_ui::NyaDialogHandle,
                        )> = None;
                        let (choice_tx, choice_rx) = mpsc::channel();
                        while !context.cancelled() {
                            let target = this
                                .update(cx, |controller, cx| {
                                    controller.x11_prompt_app(workspace, cx)
                                })
                                .ok()
                                .flatten();
                            let Some(target) = target else { break };
                            if shown.as_ref().is_none_or(|(app, _)| app != &target) {
                                if let Some((previous, handle)) = shown.take() {
                                    let _ = NyaTermApp::update_in_main_window(
                                        &previous,
                                        cx,
                                        |_, window, cx| {
                                            if handle.is_top(window, cx) {
                                                window.close_nya_dialog(cx);
                                            }
                                        },
                                    );
                                }
                                let sender = choice_tx.clone();
                                let valid = context.clone();
                                let source = target.clone();
                                let opened = NyaTermApp::update_in_main_window(
                                    &target,
                                    cx,
                                    move |app, window, cx| {
                                        if window.has_active_nya_dialog(cx) {
                                            return None;
                                        }
                                        Some(app.open_x11_install_dialog(
                                            t!("x11.installDescription").to_string(),
                                            move |choice| {
                                                let accepted =
                                                    if choice == X11PreparationChoice::Install {
                                                        valid.commit_preparation()
                                                    } else {
                                                        !valid.cancelled()
                                                    };
                                                if accepted {
                                                    let _ = sender.send((source.clone(), choice));
                                                }
                                            },
                                            window,
                                            cx,
                                        ))
                                    },
                                )
                                .ok()
                                .flatten();
                                if let Some(handle) = opened {
                                    shown = Some((target.clone(), handle));
                                }
                            }
                            if let Ok((source, choice)) = choice_rx.try_recv()
                                && source == target
                            {
                                let _ = reply.send(choice);
                                shown = None;
                                break;
                            }
                            cx.background_executor()
                                .timer(Duration::from_millis(100))
                                .await;
                        }
                        if let Some((app, handle)) = shown {
                            // Another feature may have stacked a dialog above
                            // ours. Close only our identity when it resurfaces.
                            cx.spawn(async move |cx| {
                                while handle.is_active() {
                                    let closed = NyaTermApp::update_in_main_window(
                                        &app,
                                        cx,
                                        |_, window, cx| {
                                            if handle.is_top(window, cx) {
                                                window.close_nya_dialog(cx);
                                                true
                                            } else {
                                                false
                                            }
                                        },
                                    )
                                    .unwrap_or(true);
                                    if closed {
                                        break;
                                    }
                                    cx.background_executor()
                                        .timer(Duration::from_millis(100))
                                        .await;
                                }
                            })
                            .detach();
                        }
                    }
                }
            }
        })
        .detach();
        Arc::new(Self {
            manager: X11ServerManager::new(),
            events,
        })
    }

    pub(crate) fn start_managed(
        &self,
        service: Option<Arc<NativeService>>,
        workspace: WorkspaceId,
        context: StartContext,
    ) -> Result<X11Connection, X11Error> {
        let result = self.prepare_managed(service, workspace, context.clone());
        if let Err(error) = result
            && error != X11Error::Cancelled
            && !context.has_waiters()
        {
            let _ = self.events.unbounded_send(Event::Finished {
                workspace,
                result: Err(error),
            });
        }
        result
    }

    fn prepare_managed(
        &self,
        service: Option<Arc<NativeService>>,
        workspace: WorkspaceId,
        context: StartContext,
    ) -> Result<X11Connection, X11Error> {
        let service = service.ok_or(X11Error::Startup)?;
        let id = NativeComponentId::X11;
        let acquired = wait(service.acquire(id).map_err(|_| X11Error::Startup)?, &|| {
            context.cancelled()
        })?;
        let mut installed = false;
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
                let (reply, receive) = mpsc::sync_channel(1);
                self.events
                    .unbounded_send(Event::Install {
                        workspace,
                        context: context.clone(),
                        reply,
                    })
                    .map_err(|_| X11Error::Cancelled)?;
                loop {
                    if context.preparation_committed() {
                        break;
                    }
                    if context.cancelled() {
                        return Err(X11Error::Cancelled);
                    }
                    match receive.recv_timeout(Duration::from_millis(25)) {
                        Ok(X11PreparationChoice::Install) => break,
                        Ok(_) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                            return Err(X11Error::Cancelled);
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                }
                // Explicitly approved installation survives the last channel's
                // timeout. Only application shutdown cancels this preparation.
                wait(
                    service
                        .submit(id, NativeAction::Install)
                        .map_err(|_| X11Error::Startup)?,
                    &|| context.shutting_down(),
                )?
                .map_err(|_| X11Error::Startup)?;
                installed = true;
                wait(service.acquire(id).map_err(|_| X11Error::Startup)?, &|| {
                    context.shutting_down()
                })?
                .map_err(|_| X11Error::Startup)?
            }
            Err(_) => return Err(X11Error::Startup),
        };
        let result = nyaterm_x11::start_managed(launch, context.clipboard(), &|| {
            context.shutting_down() || (!installed && !context.has_waiters())
        });
        if installed && result.is_ok() && context.needs_retry() {
            let _ = self.events.unbounded_send(Event::Finished {
                workspace,
                result: Ok(()),
            });
        }
        result
    }
}

fn wait<T>(
    mut result: futures::channel::oneshot::Receiver<T>,
    cancelled: &impl Fn() -> bool,
) -> Result<T, X11Error> {
    loop {
        if cancelled() {
            return Err(X11Error::Cancelled);
        }
        match result.try_recv() {
            Ok(Some(value)) => return Ok(value),
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(_) => return Err(X11Error::Startup),
        }
    }
}

pub(in crate::features) fn error_message(error: X11Error) -> String {
    t!(match error {
        X11Error::Cancelled => "x11.cancelled",
        X11Error::Timeout => "x11.waitTimeout",
        X11Error::NoDisplay => "x11.noDisplay",
        X11Error::XQuartzUnavailable => "x11.xquartzUnavailable",
        X11Error::Unsupported => "x11.unsupported",
        X11Error::Authentication => "x11.authentication",
        X11Error::Startup => "x11.startup",
        X11Error::Protocol => "x11.protocol",
    })
    .to_string()
}
