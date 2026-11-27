//! Background package installation and its password presentation.
use gpui::Context;
use nyaterm_core::secret::SecretString;
use nyaterm_updater::linux::{Authorization, InstallOutcome};

use super::{UpdateEvent, UpdatePhase};
use crate::features::NyaTermApp;

impl NyaTermApp {
    pub(in crate::features) fn start_package_install(
        &mut self,
        authorization: Authorization,
        cx: &mut Context<Self>,
    ) {
        if !self.cloud_sync.operations.accepting() {
            return;
        }
        self.discard_update_password(cx);
        let Some((package, generation, tx)) = self.update.update(cx, |store, cx| {
            let request = store.begin_package_install();
            cx.notify();
            request
        }) else {
            return;
        };
        let error_tx = tx.clone();
        let jobs = self.update.read(cx).blocking_jobs();
        if let Err(error) = jobs.submit_detached("update-package-install", move |_| {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| package.install(authorization)))
                .unwrap_or_else(|_| InstallOutcome::Failed("The installation worker failed; check the system package state before retrying".into()));
            let _ = tx.unbounded_send(UpdateEvent::Install { generation, outcome });
        }) {
            let _ = error_tx.unbounded_send(UpdateEvent::Install { generation, outcome: InstallOutcome::Failed(error.to_string()) });
        }
    }

    pub(in crate::features) fn discard_update_password(&mut self, cx: &mut Context<Self>) {
        if let Some(input) = self
            .update
            .update(cx, |store, _| store.password_input.take())
        {
            input.update(cx, |input, cx| input.discard_content(cx));
        }
    }

    pub(in crate::features) fn cancel_update_password(&mut self, cx: &mut Context<Self>) {
        self.discard_update_password(cx);
        self.update.update(cx, |store, cx| {
            store.cancel_password();
            cx.notify();
        });
    }

    pub(in crate::features) fn submit_update_password(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.update.read(cx).phase(), UpdatePhase::LinuxPassword) {
            return;
        }
        let Some(input) = self.update.read(cx).password_input.clone() else {
            return;
        };
        let password = SecretString::from(input.read(cx).value(cx));
        self.start_package_install(Authorization::Sudo(password), cx);
    }
}
