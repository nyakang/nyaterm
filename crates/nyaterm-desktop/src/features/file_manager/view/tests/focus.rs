use super::{FileManagerLayoutFixture, draw_file_manager, workspace};
use crate::features::NyaTermApp;
use crate::features::file_manager::pane::PaneId;
use crate::features::file_manager::workspace::FileManagerWorkspace;
use crate::features::test_support::app_with_visible_local_session;
use gpui::{AppContext as _, Entity, MouseButton, TestAppContext, VisualTestContext, px};
use nyaterm_core::test_support::TestTempDir;

fn fixture<'a>(
    cx: &'a mut TestAppContext,
    root: &TestTempDir,
) -> (
    Entity<NyaTermApp>,
    Entity<FileManagerWorkspace>,
    &'a mut VisualTestContext,
) {
    cx.executor().allow_parking();
    let app = app_with_visible_local_session(cx, root.path(), "file-manager-focus");
    cx.update(|cx| {
        nyaterm_ui::apply_component_theme(
            crate::theme::theme_palette("dark"),
            gpui::font("Segoe UI"),
            px(14.),
            cx,
        );
    });
    let workspace = workspace(cx, &app, &root.path().to_string_lossy());
    let fixture = cx.new(|_| FileManagerLayoutFixture {
        workspace: workspace.clone(),
        width: 900.,
    });
    let (_, cx) =
        cx.add_window_view(move |window, cx| nyaterm_ui::nya_root(fixture.clone(), window, cx));
    draw_file_manager(cx);
    (app, workspace, cx)
}

#[gpui::test]
fn mouse_path_selection_survives_a_late_workspace_focus_request(cx: &mut TestAppContext) {
    let root = TestTempDir::new("nyaterm-path-pointer-focus");
    let (_app, workspace, cx) = fixture(cx, &root);
    for (pane, edit_selector) in [
        (PaneId::Left, "fm-path-edit-left"),
        (PaneId::Right, "fm-path-edit-right"),
    ] {
        let edit = cx.debug_bounds(edit_selector).unwrap();
        cx.simulate_click(edit.center(), Default::default());
        draw_file_manager(cx);
        let input = cx.read(|cx| workspace.read(cx).inputs(pane).path.clone());
        let component = cx.read(|cx| input.read(cx).component_state().unwrap());
        // Use a short value so selection coordinates do not depend on the temp path.
        cx.update(|window, cx| {
            input.update(cx, |input, cx| {
                input.set_content_silent("abcdefghij", cx);
                input.focus_with_cursor_at_end(window, cx);
            });
        });
        draw_file_manager(cx);
        let [start, end] = cx.read(|cx| {
            [2, 7].map(|offset| {
                component
                    .read(cx)
                    .range_to_bounds(&(offset..offset))
                    .unwrap()
                    .center()
            })
        });
        cx.simulate_mouse_down(start, MouseButton::Left, Default::default());
        cx.simulate_mouse_move(end, MouseButton::Left, Default::default());
        cx.simulate_mouse_up(end, MouseButton::Left, Default::default());
        draw_file_manager(cx);
        let range = cx.read(|cx| component.read(cx).selected_range());
        assert_eq!(range, 2..7, "dragging inside the path selects text");
        let mut expected = cx.read(|cx| input.read(cx).value(cx).to_string());
        expected.replace_range(range.clone(), "z");

        workspace.update(cx, |state, cx| state.request_focus(cx));
        draw_file_manager(cx);
        cx.update(|window, cx| {
            assert!(input.read(cx).component_focus_handle(cx).is_focused(window));
            assert_eq!(component.read(cx).selected_range(), range);
        });
        cx.simulate_keystrokes("z");
        draw_file_manager(cx);
        cx.read(|cx| {
            assert_eq!(input.read(cx).value(cx).as_str(), expected);
            let state = workspace.read(cx);
            assert!(state.pane(pane).path_editing);
            assert!(state.left.search.is_empty());
            assert!(state.right.search.is_empty());
        });
    }
}

#[gpui::test]
fn reactivating_workspace_restores_the_path_draft_and_selection(cx: &mut TestAppContext) {
    let root = TestTempDir::new("nyaterm-path-restore-focus");
    let (_app, workspace, cx) = fixture(cx, &root);
    cx.update(|window, cx| {
        workspace.update(cx, |state, cx| {
            state.begin_path_edit(PaneId::Right, window, cx)
        });
    });
    draw_file_manager(cx);
    cx.simulate_input("draft");
    cx.simulate_keystrokes("shift-left shift-left");
    let input = cx.read(|cx| workspace.read(cx).inputs(PaneId::Right).path.clone());
    let component = cx.read(|cx| input.read(cx).component_state().unwrap());
    let range = cx.read(|cx| component.read(cx).selected_range());
    assert!(!range.is_empty());
    cx.update(|window, cx| window.blur(cx));
    workspace.update(cx, |state, cx| state.request_focus(cx));
    draw_file_manager(cx);
    cx.update(|window, cx| {
        assert!(input.read(cx).component_focus_handle(cx).is_focused(window));
        assert_eq!(input.read(cx).value(cx).as_str(), "draft");
        assert_eq!(component.read(cx).selected_range(), range);
    });
    cx.simulate_keystrokes("z");
    draw_file_manager(cx);
    cx.read(|cx| {
        assert_eq!(input.read(cx).value(cx).as_str(), "draz");
        assert!(workspace.read(cx).right.search.is_empty());
    });
}

#[gpui::test]
fn workspace_focus_request_preserves_search_input_and_list_typing(cx: &mut TestAppContext) {
    let root = TestTempDir::new("nyaterm-search-restore-focus");
    let (_app, workspace, cx) = fixture(cx, &root);
    cx.update(|window, cx| {
        workspace.update(cx, |state, cx| {
            state.focus_search(PaneId::Left, Some("ab".into()), window, cx);
            state.request_focus(cx);
        });
    });
    draw_file_manager(cx);
    cx.simulate_keystrokes("c");
    draw_file_manager(cx);
    cx.read(|cx| assert_eq!(workspace.read(cx).left.search, "abc"));
    cx.simulate_keystrokes("escape escape");
    workspace.update(cx, |state, cx| state.request_focus(cx));
    draw_file_manager(cx);
    cx.update(|window, cx| assert!(workspace.read(cx).left.focus.is_focused(window)));
    cx.simulate_keystrokes("z");
    draw_file_manager(cx);
    cx.read(|cx| assert_eq!(workspace.read(cx).left.search, "z"));
}
