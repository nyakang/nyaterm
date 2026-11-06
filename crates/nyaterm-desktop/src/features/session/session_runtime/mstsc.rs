use gpui::Context;
use rust_i18n::t;

use crate::features::NyaTermApp;
use crate::features::runtime_jobs::await_blocking_job;
use crate::platform::mstsc::{MstscError, MstscTarget};

pub(super) fn error_message(error: MstscError) -> String {
    match error {
        MstscError::InvalidHost => t!("dialog.rdpMstscHostInvalid").into(),
        MstscError::InvalidPort => t!("dialog.portInvalid").into(),
        MstscError::UnsupportedPlatform => t!("dialog.rdpMstscWindowsOnly").into(),
        MstscError::RelayUnsupported => t!("dialog.rdpMstscRelayUnsupported").into(),
        MstscError::Spawn(error) => t!("dialog.rdpMstscLaunchFailed", error = error).into(),
    }
}

impl NyaTermApp {
    pub(super) fn start_mstsc_connection(
        &mut self,
        connection: nyaterm_core::SavedConnection,
        launcher: impl FnOnce(MstscTarget) -> Result<(), MstscError> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        let nyaterm_core::ConnectionType::Rdp { host, port, .. } = &connection.config else {
            return;
        };
        let has_relay = connection
            .network
            .as_ref()
            .is_some_and(|network| network.proxy_id.is_some() || network.proxy_jump_id.is_some());
        let target = MstscTarget::validate_route(cfg!(windows), has_relay)
            .and_then(|()| MstscTarget::new(host, *port));
        let target = match target {
            Ok(target) => target,
            Err(error) => {
                self.finish_mstsc_connection(connection.id, Err(error_message(error)), cx);
                return;
            }
        };
        let task = self
            .blocking_jobs
            .submit_task("mstsc-launch", move |_| launcher(target));
        cx.spawn(async move |this, cx| {
            let result = match await_blocking_job(task).await {
                Ok(result) => result.map_err(error_message),
                Err(error) => Err(t!("dialog.rdpMstscLaunchFailed", error = error).into()),
            };
            let _ = this.update(cx, |this, cx| {
                this.finish_mstsc_connection(connection.id, result, cx);
            });
        })
        .detach();
    }

    fn finish_mstsc_connection(
        &mut self,
        connection_id: String,
        result: Result<(), String>,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(()) => {
                self.persist_connection_used(connection_id, cx);
                self.shell
                    .set_status(t!("dialog.rdpMstscLaunched").to_string());
            }
            Err(message) => {
                self.shell.set_status(message.clone());
                self.notify_operation(
                    "mstsc-launch",
                    nyaterm_ui::notification::NyaNotificationKind::Error,
                    message,
                    cx,
                );
            }
        }
        cx.notify();
    }
}
