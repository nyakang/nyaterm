use crate::features::NyaTermApp;
use futures::channel::oneshot;
use gpui::Context;
use nyaterm_core::plugins::native::{NativeComponentId, NativeLaunch};
use nyaterm_core::plugins::{ErrorCode, PluginError, PluginResult};
use nyaterm_plugin_host::native::{NativeAction, NativeService};
use rust_i18n::t;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

fn interrupted() -> PluginError {
    PluginError::new(ErrorCode::Cancelled, "Native component request cancelled")
}

pub(in crate::features::plugins) fn native_error_message(error: &PluginError) -> String {
    let key = match error.code {
        ErrorCode::NativeCatalogUnavailable => "plugins.nativeCatalogUnavailable",
        ErrorCode::NativeNetwork => "plugins.nativeNetworkError",
        ErrorCode::Incompatible => "plugins.nativeIncompatibleError",
        ErrorCode::InvalidManifest => "plugins.nativeVerificationError",
        _ => return error.message.clone(),
    };
    t!(key).to_string()
}

pub(in crate::features::plugins) fn native_display_name(id: NativeComponentId) -> String {
    let key = format!("plugins.nativeNames.{}", id.slug());
    t!(&key).to_string()
}

pub(in crate::features::plugins) fn native_description(id: NativeComponentId) -> String {
    let key = format!("plugins.nativeDescriptions.{}", id.slug());
    t!(&key).to_string()
}

impl NyaTermApp {
    pub(in crate::features) fn native_component_service(
        &self,
        cx: &gpui::App,
    ) -> Option<Arc<NativeService>> {
        self.plugins
            .panel
            .read(cx)
            .process
            .read(cx)
            .service()
            .map(|s| s.native.clone())
    }

    pub(in crate::features) fn ensure_native_component(
        &mut self,
        id: NativeComponentId,
        valid: impl Fn(&Self) -> bool + 'static,
        complete: impl FnOnce(&mut Self, PluginResult<NativeLaunch>, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) {
        #[cfg(test)]
        if let Some(launch) = self.plugins.native_launch_fixture.clone() {
            let result = if valid(self) {
                Ok(launch)
            } else {
                Err(interrupted())
            };
            complete(self, result, cx);
            return;
        }
        let Some(service) = self.native_component_service(cx) else {
            complete(self, Err(interrupted()), cx);
            return;
        };
        cx.spawn(async move |this, cx| {
            let result = async {
                let acquired = service.acquire(id)?.await.map_err(|_| interrupted())?;
                match acquired {
                    Ok(launch) => return Ok(launch),
                    Err(error)
                        if matches!(
                            error.code,
                            ErrorCode::NotFound
                                | ErrorCode::Incompatible
                                | ErrorCode::InvalidManifest
                                | ErrorCode::Io
                        ) => {}
                    Err(error) => return Err(error),
                }
                service
                    .submit(id, NativeAction::Probe)?
                    .await
                    .map_err(|_| interrupted())??;
                let size = service
                    .snapshot()
                    .into_iter()
                    .find(|s| s.id == id)
                    .and_then(|s| s.download_size)
                    .map(|n| format!("{:.1} MiB", n as f64 / 1048576.0))
                    .unwrap_or_else(|| t!("plugins.nativeSizeUnknown").to_string());
                if !this
                    .update(cx, |app, _| valid(app))
                    .map_err(|_| interrupted())?
                {
                    return Err(interrupted());
                }
                let (reply, answer) = oneshot::channel();
                let reply = Rc::new(RefCell::new(Some(reply)));
                NyaTermApp::update_in_main_window(&this, cx, move |app, window, cx| {
                    let confirm = reply.clone();
                    let display_name = native_display_name(id);
                    app.open_confirm_dialog_with_cancel(
                        (
                            t!("plugins.nativeInstallTitle", name = display_name.as_str())
                                .to_string(),
                            t!(
                                "plugins.nativeInstallMessage",
                                name = display_name.as_str(),
                                size = size
                            )
                            .to_string(),
                            t!("plugins.nativeInstallContinue").to_string(),
                            false,
                            move |_, _, _| {
                                if let Some(reply) = confirm.borrow_mut().take() {
                                    let _ = reply.send(true);
                                }
                                true
                            },
                            move |_, _| {
                                if let Some(reply) = reply.borrow_mut().take() {
                                    let _ = reply.send(false);
                                }
                            },
                        ),
                        window,
                        cx,
                    );
                })
                .map_err(|_| interrupted())?;
                if !answer.await.map_err(|_| interrupted())? {
                    return Err(interrupted());
                }
                if !this
                    .update(cx, |app, _| valid(app))
                    .map_err(|_| interrupted())?
                {
                    return Err(interrupted());
                }
                service
                    .submit(id, NativeAction::Install)?
                    .await
                    .map_err(|_| interrupted())??;
                service.acquire(id)?.await.map_err(|_| interrupted())?
            }
            .await;
            let result = result.map_err(|mut error| {
                error.message = native_error_message(&error);
                error
            });
            let _ = this.update(cx, |app, cx| complete(app, result, cx));
        })
        .detach();
    }
}
