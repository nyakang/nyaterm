//! Built-in network tools: panel state, event pump and configuration windows.
//!
//! Every event carries the service that produced it. Transfer notifications are
//! attributed with that service's short name, and each tracing event carries a
//! `service` field so the shared log can be filtered per service.
//!
//! Shared form widgets, identifiers, field rules and the log reader live under
//! `common`; each service keeps its layout, rules and save flow under its own
//! module (`tftp`).

mod common;
mod panel;
mod state;
mod tftp;
mod window;

use std::time::Duration;

use gpui::{AnyElement, Context};
use nyaterm_transport::networktools::{
    NetworkToolEvent, NetworkToolKind, TransferDirection, TransferOutcome,
};
use nyaterm_ui::notification::NyaNotificationKind;
use rust_i18n::t;

use crate::features::NyaTermApp;
use crate::features::networktools::common::{ids, log};

pub(in crate::features) use state::NetworkToolsFeatureState;

const PUMP_INTERVAL: Duration = Duration::from_millis(200);
/// Lines shown in the configuration window's log view. This is a display cap
/// over the service's own log file, not a logging parameter.
const LOG_TAIL_LINES: usize = 200;

/// Tracing requires a constant target per callsite, so each service maps to its
/// own target here. File names derive from `log_name()` in the app's routing
/// writer, not from this macro, so a new service only adds one arm.
macro_rules! networktools_log {
    ($kind:expr, $level:expr, $($fields:tt)*) => {
        match $kind {
            NetworkToolKind::Tftp => {
                tracing::event!(
                    target: "nyaterm::networktools::tftp",
                    $level,
                    $($fields)*
                )
            }
        }
    };
}

impl NyaTermApp {
    /// Start the 200 ms event pump once. The task is owned by the feature so it
    /// is cancelled with the application state.
    pub(in crate::features) fn ensure_networktools_pump(&mut self, cx: &mut Context<Self>) {
        if self.networktools.pump.is_some() {
            return;
        }
        self.networktools.pump = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(PUMP_INTERVAL).await;
                if this
                    .update(cx, |this, cx| this.drain_networktools_events(cx))
                    .is_err()
                {
                    break;
                }
            }
        }));
    }

    fn drain_networktools_events(&mut self, cx: &mut Context<Self>) {
        let events = self.networktools.take_events();
        if events.is_empty() {
            return;
        }
        for event in events {
            self.apply_networktools_event(event, cx);
        }
        cx.notify();
    }

    fn apply_networktools_event(&mut self, event: NetworkToolEvent, cx: &mut Context<Self>) {
        match event {
            NetworkToolEvent::Started { kind, addr } => {
                let status = self.networktools.status(kind);
                self.networktools.set_status_message(kind, status.message);
                networktools_log!(
                    kind,
                    tracing::Level::INFO,
                    service = kind.log_name(),
                    remote = %addr,
                    "listening"
                );
            }
            NetworkToolEvent::Stopped { kind } => {
                let status = self.networktools.status(kind);
                self.networktools.set_status_message(kind, status.message);
                self.networktools.clear_transfers(kind);
                networktools_log!(
                    kind,
                    tracing::Level::INFO,
                    service = kind.log_name(),
                    "stopped"
                );
            }
            NetworkToolEvent::TransferStarted {
                kind,
                id,
                direction,
                file,
                total,
                remote,
            } => {
                self.networktools.push_transfer(
                    kind,
                    nyaterm_transport::networktools::TransferSnapshot::new(
                        id,
                        direction,
                        file.clone(),
                        remote.clone(),
                        total,
                    ),
                );
                let message = t!(
                    "networktools.common.transfer.started",
                    service = service_label(kind),
                    file = file.clone()
                )
                .to_string();
                self.notify_background_operation(
                    ids::transfer_notification(kind, id),
                    NyaNotificationKind::Success,
                    message,
                    cx,
                );
                networktools_log!(
                    kind,
                    tracing::Level::INFO,
                    service = kind.log_name(),
                    file = %file,
                    remote = %remote,
                    total = ?total,
                    direction = direction_marker(direction),
                    "transfer started"
                );
            }
            NetworkToolEvent::TransferProgress {
                kind,
                id,
                done,
                total,
            } => {
                // Progress only drives the panel's live list. It is not logged:
                // the log keeps lifecycle events (start, finish, failure) only.
                self.networktools.update_transfer(kind, id, done, total);
            }
            NetworkToolEvent::TransferFinished {
                kind,
                id,
                direction: _,
                file,
                remote,
                outcome,
            } => {
                self.networktools.remove_transfer(kind, id);
                match outcome {
                    TransferOutcome::Completed { bytes } => {
                        let message = t!(
                            "networktools.common.transfer.finished",
                            service = service_label(kind),
                            file = file.clone(),
                            bytes = bytes
                        )
                        .to_string();
                        self.notify_background_operation(
                            ids::transfer_notification(kind, id),
                            NyaNotificationKind::Success,
                            message,
                            cx,
                        );
                        networktools_log!(
                            kind,
                            tracing::Level::INFO,
                            service = kind.log_name(),
                            file = %file,
                            remote = %remote,
                            bytes,
                            "transfer finished"
                        );
                    }
                    TransferOutcome::Failed { message } => {
                        // The reason must survive to both the toast and the log;
                        // an empty reason would leave a dangling separator.
                        let reason = if message.trim().is_empty() {
                            t!("networktools.common.transfer.failedUnknown").to_string()
                        } else {
                            message.clone()
                        };
                        let text = t!(
                            "networktools.common.transfer.failed",
                            service = service_label(kind),
                            file = file.clone(),
                            message = reason.clone()
                        )
                        .to_string();
                        self.notify_background_operation(
                            ids::transfer_notification(kind, id),
                            NyaNotificationKind::Error,
                            text.clone(),
                            cx,
                        );
                        networktools_log!(
                            kind,
                            tracing::Level::ERROR,
                            service = kind.log_name(),
                            file = %file,
                            remote = %remote,
                            error = %reason,
                            "transfer failed"
                        );
                    }
                }
            }
            NetworkToolEvent::Failed { kind, message } => {
                self.networktools
                    .set_status_message(kind, Some(message.clone()));
                let text = t!(
                    "networktools.common.status.serviceFailed",
                    service = service_label(kind),
                    message = message.clone()
                )
                .to_string();
                self.notify_background_operation(
                    ids::status_notification(kind),
                    NyaNotificationKind::Error,
                    text,
                    cx,
                );
                networktools_log!(
                    kind,
                    tracing::Level::ERROR,
                    service = kind.log_name(),
                    error = %message,
                    "service failed"
                );
            }
        }
    }

    /// Render one service's configuration form in its own window.
    pub(in crate::features) fn networktools_config_window_view(
        &mut self,
        kind: NetworkToolKind,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match kind {
            NetworkToolKind::Tftp => self.tftp_config_window_view(cx),
        }
    }

    /// Validate, persist and (when running) restart one service's configuration.
    pub(in crate::features) fn save_networktools_config(
        &mut self,
        kind: NetworkToolKind,
        cx: &mut Context<Self>,
    ) {
        match kind {
            NetworkToolKind::Tftp => self.save_tftp_config(cx),
        }
    }
}

fn direction_marker(direction: TransferDirection) -> &'static str {
    match direction {
        TransferDirection::Download => "download",
        TransferDirection::Upload => "upload",
    }
}

/// Short service label used to attribute notifications and logs.
fn service_label(kind: NetworkToolKind) -> String {
    t!(kind.short_name_key()).to_string()
}

/// Default service directory: the user's Downloads folder, falling back to the
/// desktop and then the working directory.
pub(in crate::features) fn default_networktools_share_dir() -> Option<std::path::PathBuf> {
    dirs::download_dir()
        .or_else(dirs::desktop_dir)
        .or_else(|| std::env::current_dir().ok())
}

impl NyaTermApp {
    /// Start or stop exactly one service. A failure only touches that service's
    /// status message and transfer list.
    pub(in crate::features) fn toggle_networktools_service(
        &mut self,
        kind: NetworkToolKind,
        cx: &mut Context<Self>,
    ) {
        if self.networktools.is_running(kind) {
            self.networktools.registry.stop(kind);
            self.networktools.clear_transfers(kind);
            self.networktools.set_status_message(kind, None);
            cx.notify();
            return;
        }

        match self.start_networktools_service(kind) {
            Ok(()) => self.networktools.set_status_message(kind, None),
            Err(message) => {
                self.networktools
                    .set_status_message(kind, Some(message.clone()));
                let text = t!(
                    "networktools.common.status.serviceFailed",
                    service = service_label(kind),
                    message = message
                )
                .to_string();
                self.notify_background_operation(
                    ids::status_notification(kind),
                    NyaNotificationKind::Error,
                    text,
                    cx,
                );
            }
        }
        cx.notify();
    }

    /// Bind and start exactly one service from the committed settings.
    ///
    /// The registry binds on this thread and joins the listener thread when it
    /// stops, so a failure leaves neither a half-started runtime nor a previous
    /// listener behind. Callers surface the returned message.
    fn start_networktools_service(&mut self, kind: NetworkToolKind) -> Result<(), String> {
        let mut settings = self.networktools.settings().clone();
        if kind.capabilities().needs_root_dir {
            match kind {
                NetworkToolKind::Tftp => {
                    if settings.tftp.root_dir.trim().is_empty()
                        && let Some(directory) = default_networktools_share_dir()
                    {
                        settings.tftp.root_dir = directory.display().to_string();
                    }
                }
            }
        }

        self.networktools
            .registry
            .start(kind, &settings)
            .map_err(|error| {
                let message = error.to_string();
                networktools_log!(
                    kind,
                    tracing::Level::ERROR,
                    service = kind.log_name(),
                    error = %message,
                    "start failed"
                );
                message
            })
    }

    /// Read today's log tail for this feature. Called when a configuration
    /// window opens, never from a render path.
    pub(in crate::features) fn refresh_networktools_log(
        &mut self,
        kind: NetworkToolKind,
        cx: &mut Context<Self>,
    ) {
        let dir = self.networktools.log_dir().to_path_buf();
        let read = cx
            .background_executor()
            .spawn(async move { log::read_today(&dir, kind, LOG_TAIL_LINES) });
        cx.spawn(async move |this, cx| {
            let lines = read.await;
            let _ = this.update(cx, |app, cx| {
                app.networktools.set_log_lines(kind, lines);
                cx.notify();
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod descriptor_tests {
    use nyaterm_transport::networktools::NetworkToolKind;
    use rust_i18n::t;

    #[test]
    fn every_service_descriptor_resolves_in_the_catalog() {
        for kind in NetworkToolKind::ALL {
            for key in [
                kind.name_key(),
                kind.short_name_key(),
                kind.description_key(),
            ] {
                assert_ne!(
                    t!(key).to_string(),
                    key,
                    "missing locale key {key} for {}",
                    kind.log_name()
                );
            }
        }
    }
}
