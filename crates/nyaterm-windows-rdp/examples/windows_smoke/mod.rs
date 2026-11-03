use std::{
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result, bail};
use gpui::StatefulInteractiveElement as _;
use gpui::{
    App, AppContext as _, Bounds, Context, InteractiveElement as _, IntoElement, MouseButton,
    NativeOverlayInput, ParentElement as _, Pixels, Render, Styled as _, Window, WindowBounds,
    WindowOptions, canvas, div, native_overlay, px, rgb, size,
};
use nyaterm_core::{AppRuntime, ConnectionType};
use nyaterm_remote_desktop::{
    RdpBackend, RdpClipboardConfig, RdpDisplayConfig, RdpReconnectConfig, RdpSessionConfig,
    parse_rdp_certificate_policy, parse_rdp_clipboard_mode,
};
use nyaterm_windows_rdp::{
    NativeRdpEventKind, NativeRdpHost, Viewport, lifecycle::NativeRdpLifecycle, maintain_retired,
};
use raw_window_handle::{HasWindowHandle as _, RawWindowHandle};

#[derive(Default)]
struct Layout {
    viewport: Option<Viewport>,
    close: bool,
    local_input: bool,
}

struct Smoke {
    layout: Rc<RefCell<Layout>>,
    status: String,
    menu: bool,
    modal: bool,
    tooltip: bool,
    clicks: usize,
}

impl Render for Smoke {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.layout.borrow_mut().local_input = self.menu || self.modal;
        let layout = self.layout.clone();
        let mut root =
            div()
                .size_full()
                .bg(rgb(0x20242c))
                .text_color(rgb(0xffffff))
                .flex()
                .flex_col()
                .child(
                    div()
                        .h(px(44.))
                        .flex()
                        .gap_4()
                        .items_center()
                        .px_4()
                        .child(div().id("menu").cursor_pointer().child("Menu").on_click(
                            cx.listener(|this, _, _, cx| {
                                this.menu = !this.menu;
                                cx.notify();
                            }),
                        ))
                        .child(div().id("modal").cursor_pointer().child("Modal").on_click(
                            cx.listener(|this, _, _, cx| {
                                this.modal = true;
                                cx.notify();
                            }),
                        ))
                        .child(
                            div()
                                .id("tooltip")
                                .cursor_pointer()
                                .child("Passive tooltip")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.tooltip = !this.tooltip;
                                    cx.notify();
                                })),
                        )
                        .child(format!("Menu actions: {}", self.clicks)),
                )
                .child(div().px_4().h(px(28.)).child(self.status.clone()))
                .child(
                    canvas(
                        move |bounds: Bounds<Pixels>, window, _| {
                            let scale = window.scale_factor();
                            layout.borrow_mut().viewport = Some(Viewport {
                                x: (bounds.left().as_f32() * scale).round() as i32,
                                y: (bounds.top().as_f32() * scale).round() as i32,
                                width: (bounds.size.width.as_f32() * scale).round().max(1.) as i32,
                                height: (bounds.size.height.as_f32() * scale).round().max(1.)
                                    as i32,
                                scale: (scale * 100.).round().clamp(100., 500.) as u32,
                            });
                        },
                        |_, _, _, _| {},
                    )
                    .flex_1()
                    .w_full(),
                );
        if self.menu {
            root = root.child(native_overlay(
                div()
                    .id("popup")
                    .absolute()
                    .left(px(24.))
                    .top(px(92.))
                    .w(px(230.))
                    .p_4()
                    .bg(rgb(0x3a465a))
                    .shadow_lg()
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.menu = false;
                        cx.notify();
                    }))
                    .child(
                        div()
                            .id("action")
                            .cursor_pointer()
                            .p_2()
                            .child("Count once and close")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.clicks += 1;
                                this.menu = false;
                                cx.notify();
                            })),
                    ),
                NativeOverlayInput::Region,
            ));
        }
        if self.tooltip {
            root = root.child(native_overlay(
                div()
                    .absolute()
                    .left(px(300.))
                    .top(px(160.))
                    .p_3()
                    .bg(rgb(0x765834))
                    .child("Click through to the remote desktop"),
                NativeOverlayInput::Passive,
            ));
        }
        if self.modal {
            root = root.child(native_overlay(
                div()
                    .absolute()
                    .inset_0()
                    .bg(gpui::rgba(0x00000088))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .id("dialog")
                            .p_6()
                            .bg(rgb(0x3a465a))
                            .child("Remote input is blocked. Click here to close.")
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.modal = false;
                                    cx.notify();
                                }),
                            ),
                    ),
                NativeOverlayInput::Modal,
            ));
        }
        root
    }
}

/// Use the same saved authentication/resource policy without changing the saved
/// backend or recording plaintext credentials in files, command lines or logs.
fn saved_config(name: &str) -> Result<RdpSessionConfig> {
    let runtime = AppRuntime::resolve()?;
    let store = nyaterm_store::ConnectionStore::open_with_portable_key_path(
        runtime.config_dir(),
        runtime.portable_key_path().map(ToOwned::to_owned),
    )?;
    let mut matches = store
        .list_connections()?
        .into_iter()
        .filter(|c| c.name == name);
    let summary = matches.next().context("saved smoke connection not found")?;
    if matches.next().is_some() {
        bail!("saved smoke connection name is ambiguous");
    }
    let connection = store
        .get_connection(&summary.id)?
        .context("saved smoke connection disappeared")?;
    let ConnectionType::Rdp {
        host,
        port,
        username,
        domain,
        security,
        display,
        clipboard,
        reconnect,
        ..
    } = connection.config
    else {
        bail!("smoke connection must be RDP");
    };
    let password = match connection.auth.as_ref() {
        Some(auth) if auth.mode != "none" => {
            if !auth.has_password
                && auth
                    .password
                    .as_ref()
                    .is_some_and(|value| !value.is_empty())
            {
                auth.password.clone()
            } else if let Some(id) = auth.saved_account_id() {
                store
                    .load_decrypted_password_by_id(id)?
                    .context("saved credential missing")?
                    .password
            } else {
                None
            }
        }
        _ => None,
    };
    let mut config = RdpSessionConfig {
        relay: None,
        backend: RdpBackend::Windows,
        windows_authentication: security.windows_authentication,
        name: "Native overlay smoke".into(),
        host,
        port,
        username,
        domain,
        password,
        use_nla: security.use_nla,
        certificate_policy: parse_rdp_certificate_policy(&security.certificate_policy),
        administrative_session: security.administrative_session,
        display: RdpDisplayConfig::from(&display),
        clipboard: RdpClipboardConfig {
            mode: parse_rdp_clipboard_mode(&clipboard.mode),
        },
        reconnect: RdpReconnectConfig {
            enabled: reconnect.enabled,
            max_attempts: reconnect.max_attempts,
        },
    };
    config.normalize_backend_settings();
    config.backend.validate_config(&config)?;
    Ok(config)
}

pub fn run() -> Result<()> {
    let name = std::env::args()
        .nth(1)
        .context("usage: native_overlay_smoke <saved-connection-name>")?;
    let mut config = saved_config(&name)?;
    gpui_platform::application().run(move |cx: &mut App| {
        cx.set_quit_mode(gpui::QuitMode::Explicit);
        let layout = Rc::new(RefCell::new(Layout::default()));
        let close = layout.clone();
        let view_layout = layout.clone();
        let window = cx.open_window(WindowOptions {
            native_child_surfaces: true,
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(1100.), px(760.)), cx))),
            titlebar: Some(gpui::TitlebarOptions { title: Some("NyaTerm native overlay smoke".into()), ..Default::default() }),
            ..Default::default()
        }, move |window, cx| {
            window.set_native_child_surface_mode(true).expect("Native composition unavailable");
            window.on_window_should_close(cx, move |_, _| { close.borrow_mut().close = true; false });
            cx.new(|_| Smoke { layout: view_layout, status: "Waiting for measured layout".into(), menu: false, modal: false, tooltip: false, clicks: 0 })
        }).expect("open smoke window");
        let parent = window.update(cx, |_, window, _| match window.window_handle().unwrap().as_raw() {
            RawWindowHandle::Win32(handle) => handle.hwnd.get() as usize,
            _ => unreachable!(),
        }).unwrap();
        cx.spawn(async move |cx| {
            let mut host: Option<NativeRdpHost> = None;
            let mut lifecycle = NativeRdpLifecycle::new(1);
            let mut initialized = false;
            let mut local_input = false;
            loop {
                cx.background_executor().timer(Duration::from_millis(16)).await;
                let (rect, close, blocked) = { let layout = layout.borrow(); (layout.viewport, layout.close, layout.local_input) };
                let now = Instant::now();
                if !close && !initialized && let Some(rect) = rect {
                    initialized = true;
                    if config.display.mode == nyaterm_remote_desktop::RdpDisplayMode::FitWindow {
                        config.display.width = (rect.width as u32).clamp(200, 8192) & !1;
                        config.display.height = (rect.height as u32).clamp(200, 8192) & !1;
                        config.display.desktop_scale_factor = rect.scale;
                    }
                    let result = unsafe { NativeRdpHost::create(parent, 1, &config) };
                    match result {
                        Ok(mut native) => {
                            let _ = window.update(cx, |view, _, cx| { view.status = "Connecting with system authentication; a Windows dialog may require attention".into(); cx.notify(); });
                            let _ = native.viewport(rect, false);
                            match native.start() {
                            Ok(()) => host = Some(native),
                            Err(error) => { let message = error.to_string(); let _ = window.update(cx, |view, _, cx| { view.status = message; cx.notify(); }); }
                        }},
                        Err(error) => { let message = error.to_string(); let _ = window.update(cx, |view, _, cx| { view.status = message; cx.notify(); }); }
                    }
                    config.password = None;
                }
                if let Some(native) = host.as_mut() {
                    let mut login_complete = false;
                    while let Ok(Some(event)) = native.poll() {
                        lifecycle.apply(event);
                        let message = format!("{:?}", event.kind);
                        let _ = window.update(cx, |view, _, cx| { view.status = message; cx.notify(); });
                        login_complete |= matches!(event.kind, NativeRdpEventKind::Connected | NativeRdpEventKind::LoginComplete);
                    }
                    if let Some(rect) = rect {
                        let ready = native.status().is_ok_and(|status| status.drawing_ready != 0);
                        let _ = native.viewport(rect, lifecycle.can_present(ready) && !close);
                    }
                    if blocked && !local_input { let _ = native.focus(false); }
                    if !close && !blocked && (local_input || login_complete) { let _ = native.focus(true); }
                    local_input = blocked;
                    if close {
                        lifecycle.begin_close(now);
                        if native.request_close().is_ok_and(|wait| !wait) || lifecycle.may_destroy(now) {
                            let native = host.take().unwrap();
                            if let Err((native, _)) = native.try_destroy() { drop(native); }
                        }
                    }
                }
                let abandoned = maintain_retired(now);
                if abandoned != 0 { eprintln!("Native smoke retained {abandoned} unsafe-to-release host(s)"); }
                if close && host.is_none() && nyaterm_windows_rdp::retired_count() == 0 { cx.update(|cx| cx.quit()); break; }
            }
        }).detach();
        cx.activate(true);
    });
    Ok(())
}
