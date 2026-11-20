//! Process-wide Android cleanup, after workspace users have stopped.

use gpui::{AppContext as _, Task};
use nyaterm_android::adb::server::AdbServerShutdown;
use nyaterm_android::devices::{DeviceServiceShutdown, SharedDeviceService};
use std::time::{Duration, Instant};

const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);

pub(crate) struct AndroidShutdown {
    devices: DeviceServiceShutdown,
    servers: AdbServerShutdown,
}

impl AndroidShutdown {
    pub(crate) fn begin(devices: SharedDeviceService) -> Self {
        Self {
            devices: devices.begin_shutdown(),
            servers: devices.server_manager().begin_shutdown(),
        }
    }

    pub(crate) fn finish(self) {
        let started = Instant::now();
        // Discovery is already cancelled. Signal owned children before waiting
        // for the tracker so a slow handshake cannot postpone process cleanup.
        let report = self.servers.finish(SHUTDOWN_TIMEOUT);
        let tracker_stopped = self
            .devices
            .finish(SHUTDOWN_TIMEOUT.saturating_sub(started.elapsed()));
        tracing::info!(
            terminated = report.terminated,
            already_exited = report.already_exited,
            failed = report.failed,
            tracker_stopped,
            elapsed_ms = started.elapsed().as_millis(),
            "Android process cleanup finished"
        );
    }
}

pub(crate) fn init(cx: &mut gpui::App) {
    cx.on_app_quit(|cx| {
        let devices = cx
            .try_global::<crate::app_shell::DesktopControllerGlobal>()
            .map(|controller| controller.0.read(cx).android_devices(cx))
            .unwrap_or_default();
        let cleanup = AndroidShutdown::begin(devices);
        // Normal quit waits in the controller. This last-chance path must keep
        // running even if GPUI drops its quit futures after the 200ms budget.
        let (finished_tx, finished_rx) = futures::channel::oneshot::channel();
        cx.background_executor()
            .spawn(async move {
                cleanup.finish();
                let _ = finished_tx.send(());
            })
            .detach();
        async move {
            let _ = finished_rx.await;
        }
    })
    .detach();
}

pub(crate) fn spawn_cleanup(
    cx: &mut gpui::Context<crate::app_shell::DesktopController>,
    cleanup: impl FnOnce() + Send + 'static,
) -> Task<()> {
    cx.background_spawn(async move {
        cleanup();
    })
}

#[cfg(test)]
mod tests {
    use crate::app_shell::android_shutdown::spawn_cleanup;
    use crate::app_shell::{AppShellStartup, DesktopController};
    use crate::test_support::{TestAppCleanup, TestConfigDir};
    use gpui::{AppContext as _, TestAppContext};
    use nyaterm_core::{AppRuntime, RuntimeMode};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    };
    use std::time::Duration;

    #[test]
    fn process_cleanup_yields_the_ui_thread_until_the_background_owner_finishes() {
        let directory = TestConfigDir::new("nyaterm-android-process-shutdown");
        let runtime = AppRuntime::from_parts_for_test(
            RuntimeMode::Portable,
            directory.path().into(),
            directory.join("config"),
            directory.join("logs"),
            directory.join("cache"),
            None,
        );
        let startup = AppShellStartup::prepare(&runtime);
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let controller = cx.new(|cx| DesktopController::new(runtime, startup, cx));
        let (release_tx, release_rx) = mpsc::channel();
        let (responsive_tx, responsive_rx) = mpsc::channel();
        let blocked = Arc::new(AtomicBool::new(false));
        let watchdog_blocked = Arc::clone(&blocked);
        let watchdog = std::thread::spawn(move || {
            if responsive_rx.recv_timeout(Duration::from_secs(2)).is_err() {
                watchdog_blocked.store(true, Ordering::Release);
            }
            let _ = release_tx.send(());
        });
        let task = controller.update(&mut cx, |_, cx| {
            spawn_cleanup(cx, move || {
                let _ = release_rx.recv();
            })
        });
        controller.update(&mut cx, |_, _| {
            responsive_tx.send(()).unwrap();
        });
        watchdog.join().unwrap();
        task.detach();
        cx.run_until_parked();
        assert!(!blocked.load(Ordering::Acquire));
    }
}
