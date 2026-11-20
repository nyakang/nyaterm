//! Built-in network tools: panel state, event pump and configuration windows.
//!
//! Every event carries the service that produced it. Transfer notifications are
//! attributed with that service's short name, and each tracing event carries a
//! `service` field so the shared log can be filtered per service.
//!
//! Shared form widgets, identifiers and field rules live under
//! `common`; each service keeps its layout, rules and save flow under its own
//! module (`tftp`).

mod common;
mod panel;
mod runtime;
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

use self::runtime::RuntimeCommand;
use crate::features::NyaTermApp;
use crate::features::networktools::common::ids;

pub(in crate::features) use state::NetworkToolsFeatureState;

const PUMP_INTERVAL: Duration = Duration::from_millis(200);
/// Tracing requires a constant target per callsite, so each service maps to its
/// own target here. Events use the shared diagnostics sink and retain a service
/// field for filtering. Log only runtime identifiers and counters; filenames,
/// peer addresses and free-form errors stay in the interactive UI.
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
        let snapshot_changed = self.networktools.refresh_runtime_snapshot();
        let events = self.networktools.take_events();
        let results = self.networktools.take_runtime_results();
        let changed = snapshot_changed || !events.is_empty() || !results.is_empty();
        for event in events {
            self.apply_networktools_event(event, cx);
        }
        for result in results {
            self.networktools.finish_operation(result.kind);
            let close = self.networktools.take_close_on_restart(result.kind);
            match result.result {
                Ok(()) => {
                    self.networktools.set_status_message(result.kind, None);
                    if close {
                        self.end_networktools_config_session(result.kind, false, cx);
                    }
                }
                Err(message) => self.networktools_operation_failed(result.kind, message, cx),
            }
        }
        if changed {
            cx.notify();
        }
    }

    fn apply_networktools_event(&mut self, event: NetworkToolEvent, cx: &mut Context<Self>) {
        // Old instances still get lifecycle logs, but cannot mutate the current
        // service's status or transfer rows after a restart.
        let current = self.networktools.accept_event(&event);
        match event {
            NetworkToolEvent::Started {
                kind,
                generation,
                addr,
            } => {
                let status = self.networktools.status(kind);
                if current {
                    self.networktools.set_status_message(kind, status.message);
                }
                networktools_log!(
                    kind,
                    tracing::Level::INFO,
                    service = kind.log_name(),
                    generation,
                    port = addr.port(),
                    ipv6 = addr.is_ipv6(),
                    "listening"
                );
            }
            NetworkToolEvent::Stopped { kind, generation } => {
                let status = self.networktools.status(kind);
                if current {
                    self.networktools.set_status_message(kind, status.message);
                }
                networktools_log!(
                    kind,
                    tracing::Level::INFO,
                    service = kind.log_name(),
                    generation,
                    "stopped"
                );
            }
            NetworkToolEvent::TransferStarted {
                kind,
                generation,
                id,
                direction,
                file,
                total,
                ..
            } => {
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
                    generation,
                    transfer_id = id,
                    total = ?total,
                    direction = direction_marker(direction),
                    "transfer started"
                );
            }
            NetworkToolEvent::TransferFinished {
                kind,
                generation,
                id,
                file,
                outcome,
                ..
            } => {
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
                            generation,
                            transfer_id = id,
                            bytes,
                            "transfer finished"
                        );
                    }
                    TransferOutcome::Failed { message } => {
                        // Keep detailed errors in the notification: peer-provided
                        // messages can contain user data and must not enter diagnostics.
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
                            generation,
                            transfer_id = id,
                            "transfer failed"
                        );
                    }
                }
            }
            NetworkToolEvent::Failed {
                kind,
                generation,
                message,
            } => {
                if current {
                    self.networktools
                        .set_status_message(kind, Some(message.clone()));
                }
                let text = t!(
                    "networktools.common.status.serviceFailed",
                    service = service_label(kind),
                    message = message.clone()
                )
                .to_string();
                if current && !self.networktools.is_busy(kind) {
                    self.notify_background_operation(
                        ids::status_notification(kind),
                        NyaNotificationKind::Error,
                        text,
                        cx,
                    );
                }
                networktools_log!(
                    kind,
                    tracing::Level::ERROR,
                    service = kind.log_name(),
                    generation,
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
    /// Runtime commands are serialized by the background owner. Repeated clicks
    /// stay disabled until the matching result has been consumed.
    pub(in crate::features) fn toggle_networktools_service(
        &mut self,
        kind: NetworkToolKind,
        cx: &mut Context<Self>,
    ) {
        self.ensure_networktools_pump(cx);
        if !self.networktools.begin_operation(kind) {
            return;
        }
        let command = if self.networktools.is_running(kind) {
            RuntimeCommand::Stop(kind)
        } else {
            RuntimeCommand::Start(kind, self.networktools_runtime_settings())
        };
        if let Err(message) = self.networktools.submit(command) {
            self.networktools.finish_operation(kind);
            self.networktools_operation_failed(kind, message, cx);
        }
        cx.notify();
    }

    fn networktools_runtime_settings(
        &self,
    ) -> nyaterm_core::models::networktools::NetworkToolsSettings {
        let mut settings = self.networktools.settings().clone();
        if settings.tftp.root_dir.trim().is_empty()
            && let Some(directory) = default_networktools_share_dir()
        {
            settings.tftp.root_dir = directory.display().to_string();
        }
        settings
    }

    fn networktools_operation_failed(
        &mut self,
        kind: NetworkToolKind,
        message: String,
        cx: &mut Context<Self>,
    ) {
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
