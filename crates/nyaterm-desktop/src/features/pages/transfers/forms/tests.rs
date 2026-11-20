use gpui::{
    AppContext as _, Context, FocusHandle, InteractiveElement as _, IntoElement, Render,
    Styled as _, TestAppContext, VisualTestContext, Window, div, px,
};
use nyaterm_ui::{NyaDialogWindowExt as _, init_components, nya_root};

use crate::features::{NyaTermApp, test_support::app_with_visible_local_session};
use crate::test_support::{TestAppCleanup, TestConfigDir};

struct Host {
    focus: FocusHandle,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().track_focus(&self.focus)
    }
}

fn open_creation_dialog(
    app: &mut NyaTermApp,
    folder: bool,
    window: &mut Window,
    cx: &mut Context<NyaTermApp>,
) {
    if folder {
        app.open_transfer_new_folder_dialog_at("/test".into(), window, cx);
    } else {
        app.open_transfer_new_file_dialog_at("/test".into(), window, cx);
    }
}

fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| {
        _ = window.draw(cx);
    });
    cx.run_until_parked();
}

#[test]
fn creation_dialogs_focus_names_once_restore_focus_and_reopen_empty() {
    for folder in [false, true] {
        let root = TestConfigDir::new("nyaterm-create-dialog-focus");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        cx.update(init_components);
        let app = app_with_visible_local_session(&mut cx, root.path(), "create-focus");
        app.update(&mut cx, |app, cx| app.sync_component_theme(cx));
        let host = cx.new(|cx| Host {
            focus: cx.focus_handle(),
        });
        let previous_focus = cx.update(|cx| host.read(cx).focus.clone());
        let (_, cx) = cx.add_window_view(move |window, cx| nya_root(host, window, cx));
        let input_id = if folder {
            "transfer.new-folder.name"
        } else {
            "transfer.new-file.name"
        };

        cx.update(|window, cx| {
            window.focus(&previous_focus, cx);
            app.update(cx, |app, cx| open_creation_dialog(app, folder, window, cx));
        });
        draw(cx);
        cx.update(|window, cx| {
            let input = app.read(cx).existing_text_input(input_id).unwrap();
            assert!(input.read(cx).component_focus_handle(cx).is_focused(window));
        });
        cx.simulate_input("中文 config.yaml");
        draw(cx);
        cx.update(|_, cx| {
            let state = app.read(cx);
            let value = if folder {
                &state.transfer.new_folder_dialog().unwrap().value
            } else {
                &state.transfer.new_file_dialog().unwrap().value
            };
            assert_eq!(value, "中文 config.yaml");
        });

        // Tab must move to another control, and subsequent app updates must not
        // pull the caret back into the name field.
        cx.simulate_keystrokes("tab");
        let moved_focus = cx.update(|window, cx| {
            let input = app.read(cx).existing_text_input(input_id).unwrap();
            assert!(!input.read(cx).component_focus_handle(cx).is_focused(window));
            let focus = window.focused(cx).unwrap();
            app.update(cx, |_, cx| cx.notify());
            focus
        });
        draw(cx);
        cx.update(|window, _| assert!(moved_focus.is_focused(window)));
        cx.simulate_keystrokes("escape");
        draw(cx);
        cx.update(|window, cx| {
            assert!(!window.has_active_nya_dialog(cx));
            assert!(previous_focus.is_focused(window));
            assert!(app.read(cx).existing_text_input(input_id).is_none());
            app.update(cx, |app, cx| open_creation_dialog(app, folder, window, cx));
        });
        draw(cx);
        cx.update(|window, cx| {
            let input = app.read(cx).existing_text_input(input_id).unwrap();
            assert_eq!(input.read(cx).value(cx), "");
            assert!(input.read(cx).component_focus_handle(cx).is_focused(window));
        });
        // An empty name must leave the dialog open rather than starting a job.
        cx.simulate_keystrokes("enter");
        draw(cx);
        cx.update(|window, cx| assert!(window.has_active_nya_dialog(cx)));
    }
}

#[test]
fn creation_dialogs_keep_permission_bits_independent_and_use_full_width_names() {
    for folder in [false, true] {
        let root = TestConfigDir::new("nyaterm-create-dialog-permissions");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        cx.update(init_components);
        let app = app_with_visible_local_session(&mut cx, root.path(), "create-permissions");
        app.update(&mut cx, |app, cx| app.sync_component_theme(cx));
        let host = cx.new(|cx| Host {
            focus: cx.focus_handle(),
        });
        let (_, cx) = cx.add_window_view(move |window, cx| nya_root(host, window, cx));
        cx.update(|window, cx| {
            app.update(cx, |app, cx| open_creation_dialog(app, folder, window, cx))
        });
        draw(cx);
        let input = cx
            .debug_bounds(if folder {
                "transfer.new-folder.name"
            } else {
                "transfer.new-file.name"
            })
            .unwrap();
        assert!(input.size.width > px(400.));
        let octal = cx
            .debug_bounds("transfer-create-permissions-octal")
            .unwrap();
        assert!(octal.top() > input.bottom());
        assert!(octal.right() <= input.right());
        let mut expected = if folder { 0o755 } else { 0o644 };
        for (selector, mask) in [
            ("transfer-create-perm-user-r", 0o400),
            ("transfer-create-perm-user-w", 0o200),
            ("transfer-create-perm-user-x", 0o100),
            ("transfer-create-perm-group-r", 0o040),
            ("transfer-create-perm-group-w", 0o020),
            ("transfer-create-perm-group-x", 0o010),
            ("transfer-create-perm-other-r", 0o004),
            ("transfer-create-perm-other-w", 0o002),
            ("transfer-create-perm-other-x", 0o001),
            ("transfer-create-perm-setuid", 0o4000),
            ("transfer-create-perm-setgid", 0o2000),
            ("transfer-create-perm-sticky", 0o1000),
        ] {
            let bounds = cx
                .debug_bounds(selector)
                .expect("permission control must render");
            cx.simulate_click(bounds.center(), Default::default());
            draw(cx);
            expected ^= mask;
            cx.update(|_, cx| {
                let app = app.read(cx);
                let actual = if folder {
                    app.transfer.new_folder_dialog().unwrap().mode
                } else {
                    app.transfer.new_file_dialog().unwrap().mode
                };
                assert_eq!(actual, expected, "{selector}");
            });
        }
        assert_eq!(
            expected & 0o7000,
            0o7000,
            "special permissions support multiple selections"
        );
    }
}
