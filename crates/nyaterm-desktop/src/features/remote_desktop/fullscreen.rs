//! RDP fullscreen: window-level for ActiveX, detached window for IronRDP.
//!
//! ActiveX (`RdpBackend::Windows`) pins an in-process COM control to the main
//! window's HWND. Moving that child into another top-level window produces a
//! black surface, so it uses the main window's native fullscreen mode instead.
//! The connecting overlay is a native overlay above the COM child, so the
//! negotiation text stays visible without blocking the desktop once logged on.
//!
//! Builtin/IronRDP sessions open a **separate native window** that hosts the
//! remote surface. The main window keeps running, but while a session is
//! fullscreen it renders a placeholder instead of the surface, so the surface
//! has exactly one owner. All window-coupled RDP work follows `rdp_host_window()`.
//!
//! `fullscreen_session` + `fullscreen_window` are the single source of truth.
//! `fullscreen_window` is `None` for ActiveX window-level fullscreen. The session
//! menu, window close and Ctrl+Shift+Alt+F11 all unwind the same state; Escape
//! is never used to leave RDP fullscreen.

use std::rc::Rc;

use gpui::{
    App, AppContext, Context, Entity, FocusHandle, IntoElement, KeyDownEvent, Render, Styled,
    Subscription, WeakEntity, Window, div, prelude::*, rgb,
};
use nyaterm_ui::{NyaWindowHandle, nya_root};

use crate::features::{
    NyaTermApp,
    view_widgets::{child_window_root, focus_child_window_shell_if_idle},
};

fn is_rdp_fullscreen_shortcut(keystroke: &gpui::Keystroke) -> bool {
    keystroke.key == "f11"
        && keystroke.modifiers.control
        && keystroke.modifiers.shift
        && keystroke.modifiers.alt
}

#[cfg(windows)]
fn is_native_rdp_session(app: &NyaTermApp, session_id: &str) -> bool {
    app.remote_desktop
        .sessions
        .get(session_id)
        .is_some_and(|session| {
            session.native_rdp.is_some() || session.native_rdp_config.is_some()
        })
}

/// Root view of the IronRDP detached fullscreen window. Renders the normal
/// remote desktop surface (canvas + pointer/keyboard bindings) and owns the
/// window lifecycle: it closes itself when the session is no longer the
/// fullscreen owner. Ctrl+Shift+Alt+F11 exits locally without reaching the
/// remote; other keys, including Escape and F11, forward as usual.
struct RemoteDesktopFullscreenWindow {
    app: WeakEntity<NyaTermApp>,
    session_id: String,
    shell_focus: FocusHandle,
    _app_observation: Option<Subscription>,
    /// The session's render-boundary entity. In the main window this is mounted
    /// and receives cursor-repaint notifications directly; the detached window
    /// renders the same content but a different root, so it must observe the
    /// surface as well to repaint the software cursor on pointer moves.
    _surface_observation: Option<Subscription>,
    focused_once: bool,
}

impl RemoteDesktopFullscreenWindow {
    fn new(app: WeakEntity<NyaTermApp>, session_id: String, cx: &mut Context<Self>) -> Self {
        let observation = app
            .upgrade()
            .map(|app| cx.observe(&app, |_, _, cx| cx.notify()));
        let surface_observation = app.upgrade().and_then(|app| {
            let surface = app
                .read(cx)
                .remote_desktop
                .surfaces
                .get(&session_id)
                .cloned()?;
            Some(cx.observe(&surface, |_, _, cx| cx.notify()))
        });
        Self {
            app,
            session_id,
            shell_focus: cx.focus_handle(),
            _app_observation: observation,
            _surface_observation: surface_observation,
            focused_once: false,
        }
    }
}

impl Render for RemoteDesktopFullscreenWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let still_owner = self
            .app
            .read_with(cx, |app, _| {
                app.is_remote_desktop_fullscreen(&self.session_id)
            })
            .unwrap_or(false);
        if !still_owner {
            // State was cleared elsewhere (session closed, shortcut/menu). The
            // window is redundant; remove it on the next frame.
            window.defer(cx, |window, _| window.remove_window());
            return div().size_full().into_any_element();
        }

        let app_entity = self
            .app
            .upgrade()
            .expect("fullscreen window outlives its app");
        let palette = app_entity.read(cx).theme_palette();
        let close_app = self.app.clone();
        let on_close: Rc<dyn Fn(&mut Window, &mut App)> = Rc::new(move |window, cx| {
            if let Some(app) = close_app.upgrade() {
                app.update(cx, |app, cx| app.exit_remote_desktop_fullscreen(cx));
            }
            window.remove_window();
        });

        if !self.focused_once {
            self.focused_once = true;
            // Focus our own shell so the RDP fullscreen shortcut lands here
            // first, then hand the remote surface its shared focus handle for
            // normal key forwarding.
            focus_child_window_shell_if_idle(&self.shell_focus, window, cx);
            let focus = app_entity.read(cx).remote_desktop.focus().clone();
            window.focus(&focus, cx);
        }

        let content = self
            .app
            .update(cx, |app, cx| {
                app.prepare_remote_desktop_textures(&self.session_id, window);
                app.remote_desktop_view_content(self.session_id.clone(), cx)
            })
            .unwrap_or_else(|_| div().into_any_element());

        child_window_root(&self.shell_focus, false, on_close)
            .bg(rgb(palette.bg))
            .capture_key_down(cx.listener({
                let session_id = self.session_id.clone();
                move |this, event: &KeyDownEvent, _, cx| {
                    if is_rdp_fullscreen_shortcut(&event.keystroke) {
                        if let Some(app) = this.app.upgrade() {
                            app.update(cx, |app, cx| {
                                app.exit_remote_desktop_fullscreen_from_window(&session_id, cx)
                            });
                        }
                        cx.stop_propagation();
                    }
                }
            }))
            .child(content)
            .into_any_element()
    }
}

impl NyaTermApp {
    /// Session id currently in detached fullscreen, if any.
    pub(in crate::features) fn remote_desktop_fullscreen_session(&self) -> Option<&str> {
        self.remote_desktop.fullscreen_session.as_deref()
    }

    /// Whether the given session is the one currently shown fullscreen.
    pub(in crate::features) fn is_remote_desktop_fullscreen(&self, session_id: &str) -> bool {
        self.remote_desktop_fullscreen_session() == Some(session_id)
    }

    /// Whether the given session is rendered in a detached fullscreen window.
    /// ActiveX uses window-level native fullscreen and therefore returns false.
    pub(in crate::features) fn is_remote_desktop_detached_fullscreen(
        &self,
        session_id: &str,
    ) -> bool {
        self.is_remote_desktop_fullscreen(session_id)
            && self.remote_desktop.fullscreen_window.is_some()
    }

    /// True when any remote-desktop session owns fullscreen state.
    pub(in crate::features) fn has_remote_desktop_fullscreen(&self) -> bool {
        self.remote_desktop.fullscreen_session.is_some()
    }

    /// Both RDP backends support fullscreen. ActiveX enters native fullscreen
    /// on the main window; IronRDP opens a detached fullscreen window.
    pub(in crate::features) fn remote_desktop_fullscreen_supported(
        &self,
        session_id: &str,
    ) -> bool {
        self.remote_desktop.sessions.contains_key(session_id)
            && self.session.metadata(session_id).is_some_and(|metadata| {
                matches!(
                    &metadata.launch_config,
                    crate::models::SessionLaunchConfig::Rdp(_)
                )
            })
    }

    /// Drop window-bound textures for a session before its rendering window
    /// changes. Dynamic textures live in the rendering window's sprite atlas,
    /// so a texture uploaded in the main window cannot be painted by the
    /// detached window (and vice versa). Clearing them forces a fresh upload in
    /// the new host on its next frame.
    fn invalidate_host_textures(&mut self, session_id: &str) {
        if let Some(session) = self.remote_desktop.sessions.get_mut(session_id) {
            if let Some(texture) = session.texture.take() {
                self.remote_desktop.pending_texture_removals.push(texture);
            }
            if let Some(texture) = session.cursor_texture.take() {
                self.remote_desktop.pending_texture_removals.push(texture);
            }
            session.texture_dirty = session.framebuffer.is_some();
        }
    }

    /// The window that currently hosts RDP rendering: the detached fullscreen
    /// window while one is open, otherwise the main window. Frame upload,
    /// native child hosting and keyboard capture all route through this.
    pub(in crate::features) fn rdp_host_window(&self) -> Option<NyaWindowHandle> {
        self.remote_desktop
            .fullscreen_window
            .or_else(|| self.shell.main_window())
    }

    /// Open (or close) RDP fullscreen for a session.
    pub(in crate::features) fn toggle_remote_desktop_fullscreen(
        &mut self,
        session_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_remote_desktop_fullscreen(session_id) {
            self.exit_remote_desktop_fullscreen(cx);
            return;
        }
        if !self.remote_desktop_fullscreen_supported(session_id) {
            self.exit_remote_desktop_fullscreen(cx);
            return;
        }

        // Any previous fullscreen owner must be closed first.
        self.exit_remote_desktop_fullscreen(cx);
        self.select_session(session_id.to_string(), cx);

        // ActiveX must stay on the main HWND; native fullscreen keeps the COM
        // child alive and above the black negotiation surface.
        #[cfg(windows)]
        if is_native_rdp_session(self, session_id) {
            self.remote_desktop.fullscreen_session = Some(session_id.to_string());
            self.remote_desktop.fullscreen_window = None;
            window.toggle_fullscreen();
            cx.notify();
            return;
        }

        // IronRDP renders through a GPUI texture, so it owns a detached window.
        // The detached window observes this render-boundary entity to repaint
        // the remote cursor. Ensure it exists even if fullscreen was entered
        // before the source pane ever laid out its surface.
        let app = cx.weak_entity();
        let id = session_id.to_string();
        self.remote_desktop
            .surfaces
            .entry(id.clone())
            .or_insert_with(|| cx.new(|cx| super::surface::RemoteDesktopSurface::new(app, id, cx)));
        self.remote_desktop.fullscreen_session = Some(session_id.to_string());
        // The surface is about to move to a different window atlas.
        self.invalidate_host_textures(session_id);
        self.open_fullscreen_window(session_id.to_string(), cx);
        cx.notify();
    }

    /// Request opening the detached window. Actual `open_window` is deferred to
    /// avoid re-entering the app entity while it is being updated.
    fn open_fullscreen_window(&mut self, session_id: String, cx: &mut Context<Self>) {
        if self.remote_desktop.fullscreen_window.is_some() {
            return;
        }
        let app = cx.entity();
        cx.defer(move |cx| {
            let still_owner = app.read(cx).is_remote_desktop_fullscreen(&session_id);
            if !still_owner {
                return;
            }
            open_fullscreen_window_now(app, session_id, cx);
        });
    }

    /// Exit fullscreen from inside the detached window (shortcut / close).
    /// Only acts when `session_id` still owns fullscreen, so a stale window
    /// cannot tear down a newer fullscreen session.
    pub(in crate::features) fn exit_remote_desktop_fullscreen_from_window(
        &mut self,
        session_id: &str,
        cx: &mut Context<Self>,
    ) {
        if self.is_remote_desktop_fullscreen(session_id) {
            self.exit_remote_desktop_fullscreen(cx);
        }
    }

    /// Unwind fullscreen completely: clear state, close the detached window (or
    /// restore the main window for ActiveX) and return focus so the session
    /// renders in its pane again.
    pub(in crate::features) fn exit_remote_desktop_fullscreen(&mut self, cx: &mut Context<Self>) {
        let session_id = self.remote_desktop.fullscreen_session.take();
        let fullscreen_window = self.remote_desktop.fullscreen_window.take();
        let main_window = self.shell.main_window();

        if let Some(session_id) = session_id.as_deref() {
            // Surface returns to the main window atlas; force a fresh upload.
            self.invalidate_host_textures(session_id);
        }

        if let Some(handle) = fullscreen_window {
            cx.defer(move |cx| {
                let _ = handle.update(cx, |_, window, _| window.remove_window());
                if let Some(main) = main_window {
                    let _ = main.update(cx, |_, window, cx| {
                        window.activate_window();
                        cx.activate(true);
                    });
                }
            });
        } else if session_id.is_some() {
            // ActiveX owns the main window's native fullscreen state. Defer so
            // an exit triggered by that window's own shortcut does not re-enter
            // it while the keystroke is still being dispatched.
            cx.defer(move |cx| {
                if let Some(main) = main_window {
                    let _ = main.update(cx, |_, window, cx| {
                        if window.is_fullscreen() || window.is_simple_fullscreen() {
                            window.toggle_fullscreen();
                        }
                        window.activate_window();
                        cx.activate(true);
                    });
                }
            });
        }
        cx.notify();
    }
}

fn open_fullscreen_window_now(app: Entity<NyaTermApp>, session_id: String, cx: &mut App) {
    // Build fullscreen options on the primary display (matches the remote
    // desktop use case) while keeping a windowed restore size.
    let restore = gpui::Bounds::centered(None, gpui::size(gpui::px(1280.), gpui::px(800.)), cx);
    let options = gpui::WindowOptions {
        native_child_surfaces: cfg!(target_os = "windows"),
        app_id: Some(
            nyaterm_core::app_identity::AppFlavor::current()
                .desktop_id()
                .to_string(),
        ),
        window_bounds: Some(gpui::WindowBounds::Fullscreen(restore)),
        show: true,
        focus: true,
        titlebar: None,
        ..Default::default()
    };

    let view_app = app.downgrade();
    let window_app = app.downgrade();
    let view_session = session_id.clone();
    let close_session = session_id.clone();
    let result: anyhow::Result<NyaWindowHandle> = cx.open_window(options, move |window, cx| {
        window.on_window_should_close(cx, move |_, cx| {
            if let Some(app) = window_app.upgrade() {
                app.update(cx, |app, cx| {
                    app.exit_remote_desktop_fullscreen_from_window(&close_session, cx)
                });
            }
            // Let the window close; state is already cleared.
            true
        });
        let view = cx.new(|cx| RemoteDesktopFullscreenWindow::new(view_app, view_session, cx));
        cx.new(|cx| nya_root(view, window, cx))
    });

    app.update(cx, |app, cx| match result {
        Ok(handle) => {
            app.remote_desktop.fullscreen_window = Some(handle);
            cx.notify();
        }
        Err(_) => {
            // Opening failed: restore pane rendering instead of leaving a
            // fullscreen session with no window to host it.
            app.remote_desktop.fullscreen_session = None;
            cx.notify();
        }
    });
}
