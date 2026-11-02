//! Shared open/placement/keyboard policy for secondary windows.
//!
//! Each feature still owns its own window state and view, but the parts that
//! were copied five times live here: the `WindowOptions` construction, where a
//! child window is placed, and the close keybinding.
//!
//! Placement is the reason this module exists at all. Every site used to call
//! `Bounds::centered(None, size, cx)`, and `Bounds::centered` resolves `None` to
//! `cx.primary_display()` -- so with the main window on a second monitor every
//! child window opened on the *first* one. The pre-GPUI implementation centred
//! on the parent's monitor (`center_child_in_parent_monitor`); this restores
//! that.

use std::rc::Rc;

use gpui::{
    App, Bounds, DisplayId, Div, FocusHandle, KeyBinding, Pixels, Size, Window, WindowBounds,
    WindowKind, WindowOptions, actions, div, point, prelude::*, px, size,
};
use nyaterm_ui::{NyaDialogWindowExt as _, NyaWindowHandle};

use super::chrome::child_window_titlebar;

/// Key context for the shell every child window renders around its content.
pub(in crate::features) const CHILD_WINDOW_KEY_CONTEXT: &str = "ChildWindow";

/// Added on top of [`CHILD_WINDOW_KEY_CONTEXT`] by windows that also close on
/// `escape`.
///
/// Opt-in rather than universal: losing a long edit to one stray `escape` is a
/// worse trade than reaching for `ctrl-w`, so long-edit surfaces do not take it
/// while the short prompts and single-form editors do.
const CHILD_WINDOW_ESCAPE_KEY_CONTEXT: &str = "ChildWindowEscape";

actions!(child_window, [CloseChildWindow]);

/// Bind the child-window close shortcuts.
///
/// Deliberately narrow: the other 32 shortcuts still live in `crate::shortcuts`
/// as strings matched against raw `KeyDownEvent`s on the main window's root,
/// which is why none of them reach a child window. This is the first binding
/// that goes through GPUI's own keymap; widening that is a separate change.
pub(in crate::features) fn init_key_bindings(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-w", CloseChildWindow, Some(CHILD_WINDOW_KEY_CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-w", CloseChildWindow, Some(CHILD_WINDOW_KEY_CONTEXT)),
        KeyBinding::new(
            "escape",
            CloseChildWindow,
            Some(CHILD_WINDOW_ESCAPE_KEY_CONTEXT),
        ),
    ]);
}

/// Whether a close request from the keyboard should be ignored.
///
/// A component dialog, select popover or context menu inside the child window
/// binds its own dismissal; closing the whole window out from under it would
/// lose whatever the dialog was asking about.
pub(in crate::features) fn child_window_close_is_blocked(
    window: &mut Window,
    cx: &mut App,
) -> bool {
    window.has_active_nya_dialog(cx)
}

/// What a child window does when asked to close, from the header or the keyboard.
pub(in crate::features) type ChildWindowCloseHandler = Rc<dyn Fn(&mut Window, &mut App)>;

/// The root element every child window builds its content on.
///
/// Carries the key context and close action, so `ctrl-w` works in a child window
/// even though the app's other 32 shortcuts are matched against raw key events on
/// the main window's root and never reach here.
///
/// `track_focus` is what makes the action reachable at all: GPUI dispatches along
/// the focused element's ancestor path, so a window with nothing focused would
/// dispatch nowhere. Callers focus this handle only when the window has no focus
/// of its own -- claiming it unconditionally would steal focus from the inputs
/// the editors focus on their first frame.
pub(in crate::features) fn child_window_root(
    focus: &FocusHandle,
    escape_closes: bool,
    on_close: ChildWindowCloseHandler,
) -> Div {
    let key_context = if escape_closes {
        format!("{CHILD_WINDOW_KEY_CONTEXT} {CHILD_WINDOW_ESCAPE_KEY_CONTEXT}")
    } else {
        CHILD_WINDOW_KEY_CONTEXT.to_string()
    };
    div()
        .size_full()
        .flex()
        .flex_col()
        .overflow_hidden()
        .track_focus(focus)
        .key_context(key_context.as_str())
        .on_action(move |_: &CloseChildWindow, window, cx| {
            if child_window_close_is_blocked(window, cx) {
                return;
            }
            on_close(window, cx);
        })
}

/// Give the window's shell focus when nothing else in it has any.
pub(in crate::features) fn focus_child_window_shell_if_idle(
    focus: &FocusHandle,
    window: &mut Window,
    cx: &mut App,
) {
    if window.focused(cx).is_none() {
        window.focus(focus, cx);
    }
}

/// How a child window's own title strip should be drawn.
///
/// Not derivable from the `Window` at render time: GPUI exposes the size, the
/// resize flags and the maximised state, but not whether the platform chose to
/// present this window as an attached sheet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::features) enum ChildWindowChrome {
    /// A real title bar sits behind the header, so leave room for whatever
    /// controls the platform draws there and supply the ones it does not.
    Window,
    /// An AppKit sheet. It has no title bar, so there are no traffic lights to
    /// leave a gutter for and no title bar to drag it by; a sheet is not movable
    /// and is dismissed by its own action buttons.
    Sheet,
}

/// How a child window is presented. One per feature, built at its open site.
pub(in crate::features) struct ChildWindowSpec {
    pub title: String,
    pub size: Size<Pixels>,
    pub min_size: Option<Size<Pixels>>,
    pub kind: WindowKind,
    pub resizable: bool,
    pub minimizable: bool,
}

impl ChildWindowSpec {
    /// An ordinary, independently usable secondary window.
    ///
    /// The main window stays live beside it, it gets its own taskbar entry, and it
    /// can be minimised and restored. For a task the user may sit in for a long
    /// time and keep referring back to the terminal, that is the point: a modal
    /// window would lock away the output being edited against.
    pub(in crate::features) fn document(title: String, width: f32, height: f32) -> Self {
        Self {
            title,
            size: size(px(width), px(height)),
            min_size: None,
            kind: WindowKind::Normal,
            resizable: true,
            minimizable: true,
        }
    }

    /// A scoped edit that owns a draft until it is saved or cancelled.
    ///
    /// `WindowKind::Dialog` is a real platform modal, so the owner is blocked by
    /// the OS rather than by anything here: Windows disables the owner HWND, macOS
    /// runs it as an AppKit sheet, X11 sets `_NET_WM_STATE_MODAL` and Wayland
    /// `xdg_dialog_v1.set_modal`, and on X11/Wayland GPUI also drops input to a
    /// blocked parent itself. That is what makes it safe to have no app-level
    /// modality: there is no entry point left to forget about.
    ///
    /// Not minimisable on purpose. An owned window carries no taskbar button, so a
    /// minimised one is hard to get back while its owner is still disabled.
    pub(in crate::features) fn modal_editor(title: String, width: f32, height: f32) -> Self {
        Self {
            title,
            size: size(px(width), px(height)),
            min_size: None,
            kind: WindowKind::Dialog,
            resizable: true,
            minimizable: false,
        }
    }

    /// A small always-on-top prompt.
    ///
    /// `WindowKind::PopUp` is the only kind this GPUI fork actually maps to
    /// `WS_EX_TOPMOST` on Windows; `WindowKind::Floating` has no Windows branch
    /// at all and behaves exactly like `Normal` there. On macOS this is a
    /// non-activating `NSPanel` at `NSPopUpWindowLevel`, so the prompt floats up
    /// without pulling the user out of whatever app they are in -- which is the
    /// point, since these prompts fire while the user is in an external editor.
    pub(in crate::features) fn topmost_prompt(title: String, width: f32, height: f32) -> Self {
        Self {
            title,
            size: size(px(width), px(height)),
            min_size: None,
            kind: WindowKind::PopUp,
            resizable: false,
            minimizable: false,
        }
    }

    pub(in crate::features) fn min_size(mut self, width: f32, height: f32) -> Self {
        self.min_size = Some(size(px(width), px(height)));
        self
    }

    /// Which chrome the platform will leave for this window to draw.
    ///
    /// macOS presents `WindowKind::Dialog` through `beginSheet:`, and a sheet has
    /// no title bar: the style mask still carries `NSClosableWindowMask`, but
    /// AppKit never draws the buttons, so a gutter reserved for them stays empty.
    pub(in crate::features) fn chrome(&self) -> ChildWindowChrome {
        if cfg!(target_os = "macos") && matches!(self.kind, WindowKind::Dialog) {
            ChildWindowChrome::Sheet
        } else {
            ChildWindowChrome::Window
        }
    }
}

/// Build `WindowOptions` for a child window, placed against its parent.
pub(in crate::features) fn child_window_options(
    spec: &ChildWindowSpec,
    parent: Option<NyaWindowHandle>,
    cx: &mut App,
) -> WindowOptions {
    let (display_id, bounds) = child_window_target(parent, spec.size, cx);
    WindowOptions {
        app_id: Some(
            nyaterm_core::app_identity::AppFlavor::current()
                .desktop_id()
                .to_string(),
        ),
        titlebar: child_window_titlebar(spec.title.clone()),
        #[cfg(target_os = "linux")]
        window_decorations: Some(crate::window_chrome::window_decorations_for_kind(
            &spec.kind,
            crate::window_chrome::requested_window_decorations(cx),
        )),
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: spec.min_size,
        display_id,
        kind: spec.kind.clone(),
        is_resizable: spec.resizable,
        is_minimizable: spec.minimizable,
        ..Default::default()
    }
}

/// Whether the main window should dim itself behind a modal child window.
///
/// The dim is only ever a visual cue -- the blocking is the platform's job. On
/// Windows `EnableWindow` does not grey a window's content, and on X11/Wayland
/// GPUI blocks input without changing how the parent looks, so without this the
/// main window would look completely live while being unclickable. macOS is the
/// exception: an AppKit sheet dims its parent itself, and drawing a second scrim
/// over that would double it.
pub(in crate::features) fn modal_scrim_is_drawn() -> bool {
    !cfg!(target_os = "macos")
}

/// Resolve the display and screen-space bounds a child window should open at.
fn child_window_target(
    parent: Option<NyaWindowHandle>,
    child: Size<Pixels>,
    cx: &mut App,
) -> (Option<DisplayId>, Bounds<Pixels>) {
    let parent_geometry = parent.and_then(|handle| {
        handle
            .update(cx, |_, window, cx| {
                let display = window.display(cx);
                (
                    window.window_bounds().get_bounds(),
                    display.as_ref().map(|display| display.id()),
                    display.as_ref().map(|display| display.bounds()),
                )
            })
            .ok()
    });

    let Some((parent_bounds, display_id, display_bounds)) = parent_geometry else {
        // No parent to sit beside: fall back to the primary display, which is
        // what every site used to do unconditionally.
        return (None, Bounds::centered(None, child, cx));
    };

    (
        display_id,
        child_window_placement(Some(parent_bounds), display_bounds, child),
    )
}

/// Centre `child` on its parent, then keep it inside the display.
///
/// Clamping matters because the parent can be larger than the child by less than
/// the child's own margin, or be partly off-screen itself, and a child window
/// that opens half outside the monitor is worse than one that is not perfectly
/// centred.
pub(in crate::features) fn child_window_placement(
    parent: Option<Bounds<Pixels>>,
    display: Option<Bounds<Pixels>>,
    child: Size<Pixels>,
) -> Bounds<Pixels> {
    let origin = match parent {
        Some(parent) => point(
            parent.origin.x + (parent.size.width - child.width) / 2.,
            parent.origin.y + (parent.size.height - child.height) / 2.,
        ),
        None => point(px(0.), px(0.)),
    };
    let mut bounds = Bounds {
        origin,
        size: child,
    };

    if let Some(display) = display {
        // `max` after `min` so a child wider than the display pins to the
        // display's own origin rather than being pushed off the left edge.
        bounds.origin.x = bounds
            .origin
            .x
            .min(display.origin.x + display.size.width - child.width)
            .max(display.origin.x);
        bounds.origin.y = bounds
            .origin
            .y
            .min(display.origin.y + display.size.height - child.height)
            .max(display.origin.y);
    }
    bounds
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui::{Bounds, point, px, size};

    use super::{
        ChildWindowChrome, ChildWindowCloseHandler, ChildWindowSpec, WindowKind,
        child_window_placement,
    };

    fn display() -> Bounds<gpui::Pixels> {
        Bounds {
            origin: point(px(0.), px(0.)),
            size: size(px(1920.), px(1080.)),
        }
    }

    /// The second monitor in a left-to-right arrangement: negative-free but
    /// offset, which is what broke `Bounds::centered(None, ..)`.
    fn secondary_display() -> Bounds<gpui::Pixels> {
        Bounds {
            origin: point(px(1920.), px(0.)),
            size: size(px(1920.), px(1080.)),
        }
    }

    #[test]
    fn a_child_centres_on_its_parent() {
        let parent = Bounds {
            origin: point(px(100.), px(200.)),
            size: size(px(1000.), px(800.)),
        };
        let bounds =
            child_window_placement(Some(parent), Some(display()), size(px(400.), px(300.)));
        assert_eq!(bounds.origin, point(px(400.), px(450.)));
        assert_eq!(bounds.size, size(px(400.), px(300.)));
    }

    #[test]
    fn a_child_follows_its_parent_onto_a_second_monitor() {
        let parent = Bounds {
            origin: point(px(2000.), px(100.)),
            size: size(px(1280.), px(800.)),
        };
        let bounds = child_window_placement(
            Some(parent),
            Some(secondary_display()),
            size(px(800.), px(560.)),
        );
        assert_eq!(bounds.origin, point(px(2240.), px(220.)));
        assert!(bounds.origin.x >= secondary_display().origin.x);
    }

    #[test]
    fn a_child_is_clamped_back_inside_the_display() {
        // Parent hugging the right edge: centring alone would push the child off.
        let parent = Bounds {
            origin: point(px(1700.), px(900.)),
            size: size(px(200.), px(160.)),
        };
        let bounds =
            child_window_placement(Some(parent), Some(display()), size(px(800.), px(560.)));
        assert_eq!(bounds.origin, point(px(1120.), px(520.)));
    }

    #[test]
    fn a_child_larger_than_the_display_pins_to_the_display_origin() {
        let parent = Bounds {
            origin: point(px(1920.), px(0.)),
            size: size(px(1920.), px(1080.)),
        };
        let bounds = child_window_placement(
            Some(parent),
            Some(secondary_display()),
            size(px(2400.), px(1400.)),
        );
        assert_eq!(bounds.origin, secondary_display().origin);
    }

    #[test]
    fn without_a_parent_or_display_the_child_lands_at_the_origin() {
        let bounds = child_window_placement(None, None, size(px(400.), px(300.)));
        assert_eq!(bounds.origin, point(px(0.), px(0.)));
        assert_eq!(bounds.size, size(px(400.), px(300.)));
    }

    /// The window-kind matrix, in one place, because each kind buys something
    /// different and mixing them up is silent: a `Normal` window that should have
    /// been modal simply lets the user edit two drafts at once, and a `Dialog` that
    /// should have been a prompt blocks the workspace it is reporting on.
    #[test]
    fn each_child_window_gets_the_kind_its_task_needs() {
        let scoped_edit = ChildWindowSpec::modal_editor("Edit".to_string(), 520., 620.);
        assert!(matches!(scoped_edit.kind, WindowKind::Dialog));
        assert!(
            !scoped_edit.minimizable,
            "an owned window has no taskbar button, so a minimised modal is hard to get back"
        );

        let document = ChildWindowSpec::document("File".to_string(), 980., 720.);
        assert!(matches!(document.kind, WindowKind::Normal));
        assert!(document.minimizable);

        // Topmost, because it fires while the user is in an external editor, and a
        // prompt behind that editor never gets answered. `Floating` would not do:
        // this fork has no Windows branch for it at all.
        let prompt = ChildWindowSpec::topmost_prompt("Modified".to_string(), 440., 240.);
        assert!(matches!(prompt.kind, WindowKind::PopUp));
        assert!(!prompt.resizable);
    }

    /// A sheet has no title bar, so it must not reserve the traffic-light gutter
    /// or claim a drag region -- AppKit draws neither, and the gutter would just be
    /// an empty indent to the left of the title.
    #[test]
    fn only_a_window_with_a_title_bar_reserves_room_for_platform_controls() {
        let scoped_edit = ChildWindowSpec::modal_editor("Edit".to_string(), 520., 620.);
        let document = ChildWindowSpec::document("File".to_string(), 980., 720.);
        let prompt = ChildWindowSpec::topmost_prompt("Modified".to_string(), 440., 240.);

        // A `Dialog` is only a sheet on macOS; everywhere else it keeps a real
        // title bar and, on Windows, the controls we draw into it ourselves.
        if cfg!(target_os = "macos") {
            assert_eq!(scoped_edit.chrome(), ChildWindowChrome::Sheet);
        } else {
            assert_eq!(scoped_edit.chrome(), ChildWindowChrome::Window);
        }
        assert_eq!(document.chrome(), ChildWindowChrome::Window);
        assert_eq!(prompt.chrome(), ChildWindowChrome::Window);
    }

    struct CloseKeyFixture {
        focus: gpui::FocusHandle,
        escape_closes: bool,
        closes: Rc<Cell<usize>>,
    }

    impl gpui::Render for CloseKeyFixture {
        fn render(
            &mut self,
            window: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            let closes = self.closes.clone();
            let on_close: ChildWindowCloseHandler = Rc::new(move |_, _| {
                closes.set(closes.get() + 1);
            });
            super::focus_child_window_shell_if_idle(&self.focus, window, cx);
            super::child_window_root(&self.focus, self.escape_closes, on_close)
        }
    }

    fn close_key_window(
        cx: &mut gpui::TestAppContext,
        escape_closes: bool,
    ) -> (Rc<Cell<usize>>, &mut gpui::VisualTestContext) {
        cx.update(super::init_key_bindings);
        let closes = Rc::new(Cell::new(0usize));
        let fixture_closes = closes.clone();
        let (_, cx) = cx.add_window_view(move |_, cx| CloseKeyFixture {
            focus: cx.focus_handle(),
            escape_closes,
            closes: fixture_closes,
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            _ = window.draw(cx);
        });
        (closes, cx)
    }

    /// The close binding has to reach the window at all: the app's other
    /// shortcuts are matched against raw key events on the *main* window's root,
    /// so a child window only responds because of this key context.
    #[gpui::test]
    fn ctrl_w_closes_a_child_window(cx: &mut gpui::TestAppContext) {
        let (closes, cx) = close_key_window(cx, false);
        cx.simulate_keystrokes("ctrl-w");
        cx.run_until_parked();
        assert_eq!(closes.get(), 1);
    }

    /// `escape` is opt-in per window, because one stray press should not be able
    /// to discard a long edit. Long-edit surfaces opt out.
    #[gpui::test]
    fn escape_closes_only_the_windows_that_opt_in(cx: &mut gpui::TestAppContext) {
        let (closes, cx) = close_key_window(cx, false);
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert_eq!(closes.get(), 0, "escape must not close an opted-out window");
    }

    #[gpui::test]
    fn escape_closes_a_window_that_opts_in(cx: &mut gpui::TestAppContext) {
        let (closes, cx) = close_key_window(cx, true);
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert_eq!(closes.get(), 1);
    }
}
