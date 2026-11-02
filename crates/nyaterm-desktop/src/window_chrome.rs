//! Derived window policy. GPUI remains the authority for decoration and capabilities.

#[cfg(any(target_os = "linux", test))]
use gpui::WindowKind;
use gpui::{App, Decorations, Window, WindowControls, WindowDecorations, px};
use nyaterm_ui::window_chrome::TitleDragBehavior;

use crate::app_shell::DesktopControllerGlobal;

pub(crate) fn requested_window_decorations(cx: &App) -> WindowDecorations {
    cx.try_global::<DesktopControllerGlobal>()
        .map(|global| global.0.read(cx).effective_window_decorations(cx))
        .unwrap_or(WindowDecorations::Client)
}

#[cfg(any(target_os = "linux", test))]
pub(crate) fn window_decorations_for_kind(
    kind: &WindowKind,
    requested: WindowDecorations,
) -> WindowDecorations {
    // X11 override-redirect popups are unmanaged: asking for server decorations
    // would hide our controls without a window manager supplying replacements.
    if matches!(kind, WindowKind::PopUp) {
        WindowDecorations::Client
    } else {
        requested
    }
}

/// Resolve at execution time, after entity and window leases have been released.
/// Queued previews must never replay an obsolete preference over a newer edit.
pub(crate) fn request_window_decoration_refresh(cx: &mut App) {
    if !cfg!(target_os = "linux") {
        return;
    }
    cx.defer(|cx| {
        let requested = requested_window_decorations(cx);
        for handle in cx.windows() {
            let client_only = cx
                .try_global::<DesktopControllerGlobal>()
                .is_some_and(|global| global.0.read(cx).requires_client_decorations(handle, cx));
            let requested = if client_only {
                WindowDecorations::Client
            } else {
                requested
            };
            let _ = handle.update(cx, |_, window, _| {
                window.request_decorations(requested);
                // WindowBorder only writes shadow insets in client mode. Clear
                // stale X11 frame extents after switching to system decorations.
                if matches!(window.window_decorations(), Decorations::Server) {
                    window.set_client_inset(px(0.));
                }
                window.refresh();
            });
        }
    });
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Platform {
    Windows,
    MacOS,
    Linux,
}

impl Platform {
    fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::MacOS
        } else if cfg!(target_os = "linux") {
            Self::Linux
        } else {
            Self::Windows
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WindowChrome {
    pub show_child_header: bool,
    pub minimize: bool,
    pub maximize: bool,
    pub close: bool,
    pub restore: bool,
    pub native_button_gutter: bool,
    pub drag: TitleDragBehavior,
}

impl WindowChrome {
    pub fn main(window: &Window) -> Self {
        Self::for_window(window, false, true)
    }

    pub fn child(window: &Window, sheet: bool) -> Self {
        Self::for_window(window, sheet, false)
    }

    fn for_window(window: &Window, sheet: bool, main: bool) -> Self {
        Self::derive(WindowChromeInput {
            platform: Platform::current(),
            decorations: window.window_decorations(),
            controls: window.window_controls(),
            minimizable: window.is_minimizable(),
            resizable: window.is_resizable(),
            maximized: window.is_maximized(),
            fullscreen: window.is_fullscreen() || window.is_simple_fullscreen(),
            sheet,
            main,
        })
    }

    fn derive(input: WindowChromeInput) -> Self {
        let WindowChromeInput {
            platform,
            decorations,
            controls,
            minimizable,
            resizable,
            maximized,
            fullscreen,
            sheet,
            main,
        } = input;
        let buttons = !fullscreen
            && !sheet
            && match platform {
                Platform::MacOS => false,
                Platform::Windows => true,
                Platform::Linux => matches!(decorations, Decorations::Client { .. }),
            };
        Self {
            show_child_header: platform != Platform::Linux
                || !matches!(decorations, Decorations::Server),
            minimize: buttons && minimizable && controls.minimize,
            maximize: buttons && resizable && controls.maximize,
            close: buttons,
            restore: maximized,
            native_button_gutter: platform == Platform::MacOS && !sheet && !fullscreen,
            drag: if sheet || fullscreen {
                TitleDragBehavior::Disabled
            } else {
                match platform {
                    Platform::Linux => TitleDragBehavior::Linux {
                        can_maximize: resizable && controls.maximize,
                    },
                    Platform::MacOS if main => TitleDragBehavior::MacOS,
                    Platform::MacOS | Platform::Windows => TitleDragBehavior::Native,
                }
            },
        }
    }
}

struct WindowChromeInput {
    platform: Platform,
    decorations: Decorations,
    controls: WindowControls,
    minimizable: bool,
    resizable: bool,
    maximized: bool,
    fullscreen: bool,
    sheet: bool,
    main: bool,
}

#[cfg(test)]
mod tests;
