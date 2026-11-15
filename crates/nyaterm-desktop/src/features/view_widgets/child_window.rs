//! Application window options; shell widgets and placement live in nyaterm-ui.
use gpui::{App, WindowOptions};
use nyaterm_ui::NyaWindowHandle;
pub(in crate::features) use nyaterm_ui::child_window::shell::{
    ChildWindowChrome, ChildWindowCloseHandler, ChildWindowSpec, child_window_root,
    focus_child_window_shell_if_idle, init_key_bindings, modal_scrim_is_drawn,
};
pub(in crate::features) fn child_window_options(
    spec: &ChildWindowSpec,
    parent: Option<NyaWindowHandle>,
    cx: &mut App,
) -> WindowOptions {
    let decorations = if cfg!(target_os = "linux") {
        #[cfg(target_os = "linux")]
        {
            Some(crate::window_chrome::window_decorations_for_kind(
                &spec.kind,
                crate::window_chrome::requested_window_decorations(cx),
            ))
        }
        #[cfg(not(target_os = "linux"))]
        {
            None
        }
    } else {
        None
    };
    nyaterm_ui::child_window::shell::child_window_options(
        spec,
        parent,
        Some(
            nyaterm_core::app_identity::AppFlavor::current()
                .desktop_id()
                .to_string(),
        ),
        decorations,
        cx,
    )
}
