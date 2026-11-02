use gpui::{Decorations, Tiling, WindowControls};
use nyaterm_ui::window_chrome::TitleDragBehavior;

use crate::window_chrome::{Platform, WindowChrome, WindowChromeInput};

#[test]
fn unmanaged_popups_keep_client_controls_when_system_decorations_are_selected() {
    use crate::window_chrome::window_decorations_for_kind;
    use gpui::{WindowDecorations, WindowKind};

    for requested in [WindowDecorations::Client, WindowDecorations::Server] {
        assert_eq!(
            window_decorations_for_kind(&WindowKind::PopUp, requested),
            WindowDecorations::Client
        );
        for kind in [WindowKind::Normal, WindowKind::Dialog, WindowKind::Floating] {
            assert_eq!(window_decorations_for_kind(&kind, requested), requested);
        }
    }
}

fn input(platform: Platform) -> WindowChromeInput {
    WindowChromeInput {
        platform,
        decorations: Decorations::Client {
            tiling: Tiling::default(),
        },
        controls: WindowControls::default(),
        minimizable: true,
        resizable: true,
        maximized: false,
        fullscreen: false,
        sheet: false,
        main: true,
    }
}

#[test]
fn linux_controls_follow_actual_decorations_even_when_client_was_requested() {
    let client = WindowChrome::derive(input(Platform::Linux));
    let server = WindowChrome::derive(WindowChromeInput {
        decorations: Decorations::Server,
        ..input(Platform::Linux)
    });
    assert!(client.close && client.minimize && client.maximize);
    assert!(!server.close && !server.minimize && !server.maximize);
    assert_eq!(client.drag, server.drag);
}

#[test]
fn linux_child_headers_follow_actual_decorations() {
    for decorations in [
        Decorations::Client {
            tiling: Tiling::default(),
        },
        Decorations::Server,
    ] {
        let policy = WindowChrome::derive(WindowChromeInput {
            main: false,
            decorations,
            ..input(Platform::Linux)
        });
        assert_eq!(
            policy.show_child_header,
            matches!(decorations, Decorations::Client { .. })
        );
    }
}

#[test]
fn window_flags_and_platform_capabilities_both_gate_controls() {
    for (minimizable, resizable) in [(false, true), (true, false), (false, false)] {
        let policy = WindowChrome::derive(WindowChromeInput {
            minimizable,
            resizable,
            maximized: true,
            ..input(Platform::Linux)
        });
        assert_eq!(policy.minimize, minimizable);
        assert_eq!(policy.maximize, resizable);
        assert!(policy.restore);
        assert_eq!(
            policy.drag,
            TitleDragBehavior::Linux {
                can_maximize: resizable
            }
        );
    }
    let policy = WindowChrome::derive(WindowChromeInput {
        controls: WindowControls {
            minimize: false,
            maximize: false,
            ..WindowControls::default()
        },
        ..input(Platform::Linux)
    });
    assert!(!policy.minimize && !policy.maximize && policy.close);
    assert_eq!(
        policy.drag,
        TitleDragBehavior::Linux {
            can_maximize: false
        }
    );
}

#[test]
fn fullscreen_and_sheets_remove_controls_gutters_and_dragging() {
    for platform in [Platform::Linux, Platform::Windows, Platform::MacOS] {
        for (fullscreen, sheet) in [(true, false), (false, true)] {
            let policy = WindowChrome::derive(WindowChromeInput {
                fullscreen,
                sheet,
                ..input(platform)
            });
            assert!(!policy.minimize && !policy.maximize && !policy.close);
            assert!(!policy.native_button_gutter);
            assert_eq!(policy.drag, TitleDragBehavior::Disabled);
            assert!(policy.show_child_header);
        }
    }
}

#[test]
fn macos_keeps_native_buttons_and_windows_keeps_custom_caption_buttons() {
    for main in [true, false] {
        let mac = WindowChrome::derive(WindowChromeInput {
            main,
            decorations: Decorations::Server,
            ..input(Platform::MacOS)
        });
        assert!(!mac.minimize && !mac.maximize && !mac.close);
        assert!(mac.show_child_header);
        assert!(mac.native_button_gutter);
        assert_eq!(
            mac.drag,
            if main {
                TitleDragBehavior::MacOS
            } else {
                TitleDragBehavior::Native
            }
        );
        let windows = WindowChrome::derive(WindowChromeInput {
            main,
            decorations: Decorations::Server,
            ..input(Platform::Windows)
        });
        assert!(windows.minimize && windows.maximize && windows.close);
        assert!(windows.show_child_header);
        assert_eq!(windows.drag, TitleDragBehavior::Native);
    }
}
