use crate::features::NyaTermApp;
use crate::features::session::x11_service::X11Service;
use futures::future::BoxFuture;
use gpui::{BackgroundExecutor, Context};
use nyaterm_core::WorkspaceId;
use nyaterm_core::x11::{X11ChannelConnection, X11Error, X11Mode, X11ServerProvider};
use nyaterm_plugin_host::native::NativeService;
use nyaterm_transport::SshSessionConfig;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub(super) struct X11Startup {
    provider: Arc<dyn X11ServerProvider>,
}
impl X11Startup {
    pub(super) fn prepare(&self, config: &mut SshSessionConfig) -> Result<(), String> {
        config.attempt.check()?;
        config.x11_connection = None;
        config.x11_provider = config.x11_forwarding.then(|| self.provider.clone());
        Ok(())
    }
}

struct Provider {
    mode: X11Mode,
    display: String,
    workspace: WorkspaceId,
    service: Option<Arc<X11Service>>,
    native: Option<Arc<NativeService>>,
    executor: BackgroundExecutor,
}
struct CancelOnDrop(Arc<AtomicBool>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}
impl X11ServerProvider for Provider {
    fn acquire(&self) -> BoxFuture<'static, Result<X11ChannelConnection, X11Error>> {
        if self
            .mode
            .uses_managed(cfg!(all(windows, target_arch = "x86_64")), &self.display)
        {
            let Some(service) = self.service.clone() else {
                return Box::pin(async { Err(X11Error::Startup) });
            };
            let native = self.native.clone();
            let workspace = self.workspace;
            let starter = service.clone();
            service
                .manager
                .acquire(move |context| starter.start_managed(native, workspace, context))
        } else {
            let mode = self.mode.clone();
            let display = self.display.clone();
            let service = self.service.clone();
            let cancelled = Arc::new(AtomicBool::new(false));
            let guard = CancelOnDrop(cancelled.clone());
            let task = self.executor.spawn(async move {
                nyaterm_x11::prepare_external(&mode, &display, &|| {
                    cancelled.load(Ordering::Acquire)
                        || service
                            .as_ref()
                            .is_some_and(|s| s.manager.is_shutting_down())
                })
                .map(|connection| X11ChannelConnection {
                    connection,
                    guard: None,
                })
            });
            Box::pin(async move {
                let _guard = guard;
                task.await
            })
        }
    }
}
impl NyaTermApp {
    pub(super) fn x11_startup(&self, cx: &mut Context<Self>) -> X11Startup {
        let service = self
            .desktop_controller
            .as_ref()
            .and_then(|controller| controller.upgrade())
            .map(|controller| controller.read(cx).x11_service());
        X11Startup {
            provider: Arc::new(Provider {
                mode: self.settings.summary().x11_mode.clone(),
                display: self.settings.summary().x11_display.clone(),
                workspace: self.workspace_id,
                service,
                native: self.native_component_service(cx),
                executor: cx.background_executor().clone(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::features::session::session_runtime::x11::X11Startup;
    use futures::future::BoxFuture;
    use nyaterm_core::x11::{X11ChannelConnection, X11Error, X11ServerProvider};
    use nyaterm_transport::SshSessionConfig;
    use std::sync::Arc;

    struct UnusedProvider;
    impl X11ServerProvider for UnusedProvider {
        fn acquire(&self) -> BoxFuture<'static, Result<X11ChannelConnection, X11Error>> {
            panic!("opening SSH must not prepare an X11 server");
        }
    }
    #[test]
    fn ssh_start_only_binds_a_lazy_provider_and_preserves_forwarding() {
        let startup = X11Startup {
            provider: Arc::new(UnusedProvider),
        };
        let mut config = SshSessionConfig {
            x11_forwarding: true,
            ..Default::default()
        };
        startup.prepare(&mut config).unwrap();
        assert!(config.x11_forwarding);
        assert!(config.x11_provider.is_some());
        assert!(config.x11_connection.is_none());
        config.x11_forwarding = false;
        startup.prepare(&mut config).unwrap();
        assert!(config.x11_provider.is_none());
    }
    #[test]
    fn cancelled_ssh_attempt_does_not_bind_a_provider() {
        let startup = X11Startup {
            provider: Arc::new(UnusedProvider),
        };
        let mut config = SshSessionConfig {
            x11_forwarding: true,
            ..Default::default()
        };
        config.attempt.cancel();
        assert!(startup.prepare(&mut config).is_err());
        assert!(config.x11_provider.is_none());
    }
}
