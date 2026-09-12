mod single_instance;

use anyhow::Context as _;
use gpui::{App, AppContext, TitlebarOptions, WindowOptions, point, px};
use nyaterm_app::assets;
use nyaterm_core::{ActivationRequest, AppRuntime, LOG_FILE_PREFIX, LOG_FILE_SUFFIX};
use nyaterm_desktop::{AppShell, AppShellStartup};
use nyaterm_ui::nya_root;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

use single_instance::{SingleInstanceOutcome, acquire};

fn main() -> anyhow::Result<()> {
    let runtime = AppRuntime::resolve().context("resolve nyaterm runtime")?;
    runtime
        .ensure_directories()
        .context("prepare runtime directories")?;
    let initial_activation = ActivationRequest::from_os_args(
        *uuid::Uuid::new_v4().as_bytes(),
        std::env::args_os().skip(1),
    );
    let mut instance_owner = match acquire(runtime.config_dir(), initial_activation)? {
        SingleInstanceOutcome::Owner(owner) => owner,
        SingleInstanceOutcome::Forwarded => return Ok(()),
    };
    let activation_tx = instance_owner.activation_sender();
    let activation_rx = instance_owner.take_activation_receiver();
    let _log_guard = init_tracing(&runtime);
    nyaterm_desktop::preload_i18n()
        .map_err(anyhow::Error::msg)
        .context("preload translation catalogs")?;

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
        gpui_component::init(cx);
        nyaterm_desktop::init(cx);
        let startup = AppShellStartup::prepare(&runtime);
        let placement = startup.main_window_placement(cx);
        let app_runtime = runtime.clone();

        cx.open_window(
            WindowOptions {
                titlebar: Some(TitlebarOptions {
                    appears_transparent: true,
                    traffic_light_position: cfg!(target_os = "macos")
                        .then(|| point(px(9.), px(11.))),
                    ..Default::default()
                }),
                #[cfg(target_os = "linux")]
                window_decorations: Some(gpui::WindowDecorations::Client),
                window_bounds: Some(placement.window_bounds),
                display_id: placement.display_id,
                ..Default::default()
            },
            move |window, cx| {
                let shell = cx.new(|cx| AppShell::new(app_runtime, activation_rx, startup, cx));
                let close_shell = shell.clone();
                window.on_window_should_close(cx, move |window, cx| {
                    close_shell.update(cx, |shell, cx| shell.request_window_close(window, cx));
                    false
                });
                shell.update(cx, |shell, cx| {
                    shell.start_after_window_open(window, cx);
                });
                cx.new(|cx| nya_root(shell, window, cx))
            },
        )
        .expect("failed to open NyaTerm window");

        cx.activate(true);
    });

    Ok(())
}

fn init_tracing(runtime: &AppRuntime) -> Option<WorkerGuard> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new("nyaterm=info,nyaterm_core=info,nyaterm_transport=info,warn")
    });
    let file_appender = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix(LOG_FILE_PREFIX)
        .filename_suffix(LOG_FILE_SUFFIX)
        .build(runtime.log_dir())
        .ok();

    if let Some(file_appender) = file_appender {
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
            .ok();
        Some(guard)
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .try_init()
            .ok();
        None
    }
}
