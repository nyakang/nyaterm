#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod single_instance;
#[cfg(windows)]
mod url_protocols;

use anyhow::Context as _;
use gpui::{App, AppContext};
use nyaterm_app::{assets, crash_report};
use nyaterm_core::app_identity::AppFlavor;
use nyaterm_core::{ActivationRequest, AppRuntime, LOG_FILE_PREFIX, LOG_FILE_SUFFIX};
use nyaterm_desktop::{AppShellStartup, DesktopController, DesktopControllerGlobal};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

use single_instance::{SingleInstanceOutcome, acquire};

fn main() -> anyhow::Result<()> {
    #[cfg(windows)]
    if let Some(result) = url_protocols::run_if_requested() {
        return result;
    }
    if let Some(status) = nyaterm_store::run_cloud_snapshot_decode_helper_if_requested() {
        std::process::exit(status);
    }
    if nyaterm_desktop::run_update_helper_if_requested() {
        return Ok(());
    }
    let runtime = AppRuntime::resolve().context("resolve nyaterm runtime")?;
    runtime
        .ensure_directories()
        .context("prepare runtime directories")?;
    let crash_report_status = crash_report::install(runtime.crash_dir());
    let initial_activation = ActivationRequest::from_os_args(
        *uuid::Uuid::new_v4().as_bytes(),
        std::env::args_os().skip(1),
    );
    let mut instance_owner = match acquire(runtime.config_dir(), initial_activation.clone())? {
        SingleInstanceOutcome::Owner(owner) => owner,
        SingleInstanceOutcome::Forwarded => return Ok(()),
    };
    let activation_tx = instance_owner.activation_sender();
    let activation_rx = instance_owner.take_activation_receiver();
    let _log_guard = init_tracing(&runtime)?;
    if let Err(error) = crash_report_status {
        tracing::warn!(%error, "panic reporting unavailable");
    }
    if let Err(error) = crash_report::cleanup_old_reports(&runtime.crash_dir()) {
        tracing::warn!(%error, "could not clean up old panic reports");
    }
    #[cfg(windows)]
    if let Err(error) = nyaterm_transport::local_conpty::configure_from_executable() {
        tracing::warn!(%error, "could not configure bundled ConPTY");
    }
    nyaterm_desktop::preload_i18n()
        .map_err(anyhow::Error::msg)
        .context("preload translation catalogs")?;

    nyaterm_desktop::platform::init_native_menu_theme();
    let application = gpui_platform::application().with_assets(assets::NyaTermAssets);
    let open_url_tx = activation_tx.clone();
    application.on_open_urls(move |urls| {
        let request = ActivationRequest::from_os_args(
            *uuid::Uuid::new_v4().as_bytes(),
            urls.into_iter().map(std::ffi::OsString::from),
        );
        let _ = open_url_tx.try_send(request);
    });
    application.on_reopen(move |cx| {
        let request =
            ActivationRequest::from_os_args(*uuid::Uuid::new_v4().as_bytes(), std::iter::empty());
        let _ = activation_tx.try_send(request);
        cx.activate(true);
    });

    application.run(move |cx: &mut App| {
        let flavor = AppFlavor::current();
        cx.set_app_identity(flavor.application_identifier(), flavor.display_name());
        gpui_kit::init(cx);
        cx.set_quit_mode(gpui::QuitMode::Explicit);
        nyaterm_desktop::init(cx);
        let startup = AppShellStartup::prepare(&runtime);
        let controller = cx.new(|cx| DesktopController::new(runtime.clone(), startup, cx));
        cx.set_global(DesktopControllerGlobal(controller.clone()));
        controller
            .update(cx, |controller, cx| {
                controller.launch(initial_activation, activation_rx, cx)
            })
            .expect("failed to open NyaTerm windows");

        cx.activate(true);
    });

    Ok(())
}

fn init_tracing(runtime: &AppRuntime) -> anyhow::Result<Option<WorkerGuard>> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new("nyaterm=info,nyaterm_core=info,nyaterm_transport=info,warn")
    });
    let file_appender = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix(LOG_FILE_PREFIX)
        .filename_suffix(LOG_FILE_SUFFIX)
        .build(runtime.log_dir());

    if let Ok(file_appender) = file_appender {
        let (file_writer, guard) = tracing_appender::non_blocking(file_appender);
        tracing_subscriber::registry()
            .with(filter)
            .with(tracing_subscriber::fmt::layer().compact())
            .with(
                tracing_subscriber::fmt::layer()
                    .json()
                    .with_writer(file_writer),
            )
            .try_init()
            .context("initialize tracing subscriber and log bridge")?;
        Ok(Some(guard))
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .finish()
            .try_init()
            .context("initialize tracing subscriber and log bridge")?;
        if let Err(error) = file_appender {
            tracing::warn!(%error, "file logging unavailable; using console logging");
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::init_tracing;
    use nyaterm_core::AppRuntime;
    use nyaterm_core::runtime::RuntimeMode;
    use std::path::PathBuf;
    use std::process::Command;

    #[test]
    fn logging_child() {
        let Some(root) = std::env::var_os("NYATERM_LOGGING_TEST_DIR") else {
            return;
        };
        let root = PathBuf::from(root);
        let log_dir = root.join("logs");
        let fallback = std::env::var_os("NYATERM_LOGGING_TEST_FALLBACK").is_some();
        if fallback {
            std::fs::write(&log_dir, b"blocked").unwrap();
        }
        let runtime = AppRuntime::from_parts_for_test(
            RuntimeMode::Portable,
            root.clone(),
            root.clone(),
            log_dir,
            root.join("cache"),
            None,
        );
        let guard = init_tracing(&runtime).expect("initialize logging once");
        assert_eq!(guard.is_none(), fallback);
        assert!(
            init_tracing(&runtime).is_err(),
            "duplicate initialization must be visible"
        );
        tracing::trace!(target: "nyaterm", "trace-event-marker");
        log::trace!(target: "nyaterm", "trace-log-marker");
        tracing::debug!(target: "nyaterm", "debug-event-marker");
        log::debug!(target: "nyaterm", "debug-log-marker");
        tracing::error!(target: "nyaterm", "error-event-marker");
        log::error!(target: "nyaterm", "error-log-marker");
        drop(guard);
    }

    #[test]
    fn logging_bridges_once_and_filters_console_and_jsonl_in_subprocesses() {
        for (filter, fallback) in [
            ("nyaterm=trace", false),
            ("nyaterm=debug", false),
            ("nyaterm=error", false),
            ("nyaterm=trace", true),
        ] {
            let root =
                std::env::temp_dir().join(format!("nyaterm-logging-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&root).unwrap();
            let mut command = Command::new(std::env::current_exe().unwrap());
            command
                .args(["--exact", "tests::logging_child", "--nocapture"])
                .env("NYATERM_LOGGING_TEST_DIR", &root)
                .env("RUST_LOG", filter);
            if fallback {
                command.env("NYATERM_LOGGING_TEST_FALLBACK", "1");
            }
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let console = String::from_utf8_lossy(&output.stdout);
            let jsonl = if fallback {
                String::new()
            } else {
                std::fs::read_dir(root.join("logs"))
                    .unwrap()
                    .map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap())
                    .collect::<String>()
            };
            for (level, expected) in [
                ("trace", filter.ends_with("trace")),
                ("debug", !filter.ends_with("error")),
                ("error", true),
            ] {
                for source in ["event", "log"] {
                    let marker = format!("{level}-{source}-marker");
                    assert_eq!(
                        console.matches(&marker).count(),
                        usize::from(expected),
                        "console: {filter}, {marker}"
                    );
                    if !fallback {
                        let events: Vec<serde_json::Value> = jsonl
                            .lines()
                            .map(|line| serde_json::from_str(line).unwrap())
                            .collect();
                        assert_eq!(
                            events
                                .iter()
                                .filter(|event| event["fields"]["message"] == marker)
                                .count(),
                            usize::from(expected),
                            "jsonl: {filter}, {marker}"
                        );
                    }
                }
            }
            if fallback {
                assert!(console.contains("file logging unavailable"));
            }
            std::fs::remove_dir_all(root).unwrap();
        }
    }
}
