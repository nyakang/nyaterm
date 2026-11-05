use crate::features::NyaTermApp;
use crate::features::file_manager::pane::{PaneEndpoint, PaneId};
use crate::features::file_manager::workspace::{FileManagerSeed, FileManagerWorkspace};
use crate::features::settings::SettingsPersistenceDomain;
use crate::features::test_support::{app_with_visible_local_session, session_focus_test_window};
use gpui::{
    App, AppContext as _, Entity, KeyDownEvent, Keystroke, TestAppContext, VisualTestContext,
};
use nyaterm_core::test_support::TestTempDir;
use nyaterm_files::sftp::RemoteFilePath;
use std::time::{Duration, Instant};

fn workspace(
    cx: &mut TestAppContext,
    app: &Entity<NyaTermApp>,
    path: &str,
) -> Entity<FileManagerWorkspace> {
    let workspace = cx.new(|cx| {
        FileManagerWorkspace::new(
            app.clone(),
            "file-manager-reentrancy".into(),
            FileManagerSeed {
                left: PaneEndpoint::Local,
                left_path: Some(RemoteFilePath::new(path)),
                right: PaneEndpoint::Local,
                right_path: Some(RemoteFilePath::new(path)),
            },
            cx,
        )
    });
    app.update(cx, |app, _| {
        app.file_manager
            .insert("file-manager-reentrancy".into(), workspace.clone());
    });
    workspace
}

fn wait_for(cx: &mut VisualTestContext, description: &str, condition: impl Fn(&App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        cx.run_until_parked();
        if cx.read(&condition) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {description}"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn wait_for_listings(cx: &mut VisualTestContext, workspace: &Entity<FileManagerWorkspace>) {
    wait_for(cx, "directory listings", |cx| {
        [PaneId::Left, PaneId::Right]
            .into_iter()
            .all(|id| !workspace.read(cx).pane(id).loading)
    });
    cx.read(|cx| {
        for id in [PaneId::Left, PaneId::Right] {
            assert!(workspace.read(cx).pane(id).error.is_none());
        }
    });
}

#[gpui::test]
fn navigation_releases_workspace_before_app_updates_the_same_pane(cx: &mut TestAppContext) {
    // Directory listings use real blocking workers, which wake the test executor.
    cx.executor().allow_parking();
    let root = TestTempDir::new("nyaterm-file-manager-navigation-reentrancy");
    let app = app_with_visible_local_session(cx, root.path(), "file-manager-test");
    std::fs::write(root.path().join("navigation.txt"), "fixture").unwrap();
    let path = root.path().to_string_lossy().into_owned();
    let workspace = workspace(cx, &app, &path);
    let cx = session_focus_test_window(cx, &app);

    for pane_id in [PaneId::Left, PaneId::Right] {
        cx.update(|window, cx| {
            workspace.update(cx, |workspace, cx| {
                let id = workspace.id.clone();
                let path = RemoteFilePath::new(path.clone());
                workspace.dispatch_app(window, cx, move |app, _, cx| {
                    app.navigate_file_manager_pane(id, pane_id, path, cx);
                });
                assert_eq!(workspace.pane(pane_id).generation, 0);
            });
        });
        cx.read(|cx| {
            let pane = workspace.read(cx).pane(pane_id);
            assert_eq!(pane.generation, 1);
            assert_eq!(pane.path.display_path, path);
        });
    }
    // Finish both replies before GPUI teardown drops their foreground tasks.
    wait_for_listings(cx, &workspace);
    cx.read(|cx| {
        for id in [PaneId::Left, PaneId::Right] {
            assert!(
                workspace
                    .read(cx)
                    .pane(id)
                    .entries
                    .iter()
                    .any(|entry| entry.name == "navigation.txt")
            );
        }
    });
}

#[gpui::test]
fn copy_and_move_shortcuts_can_read_the_originating_workspace(cx: &mut TestAppContext) {
    // Even an empty same-endpoint move refreshes both panes on blocking workers.
    cx.executor().allow_parking();
    let root = TestTempDir::new("nyaterm-file-manager-shortcut-reentrancy");
    let app = app_with_visible_local_session(cx, root.path(), "file-manager-test");
    let workspace = workspace(cx, &app, &root.path().to_string_lossy());
    let cx = session_focus_test_window(cx, &app);

    // Both commands consult the workspace even when no files are selected.
    for key in ["f5", "f6"] {
        cx.update(|window, cx| {
            workspace.update(cx, |workspace, cx| {
                workspace.handle_key(
                    &KeyDownEvent {
                        keystroke: Keystroke::parse(key).unwrap(),
                        is_held: false,
                        prefer_character_input: false,
                    },
                    window,
                    cx,
                );
            });
        });
        cx.read(|cx| assert!(app.read(cx).transfer.transfer_jobs().is_empty()));
    }
    wait_for_listings(cx, &workspace);
}

#[gpui::test]
fn layout_save_reads_the_workspace_after_its_listener_finishes(cx: &mut TestAppContext) {
    // Saving settings wakes the executor from the real database worker.
    cx.executor().allow_parking();
    let root = TestTempDir::new("nyaterm-file-manager-layout-reentrancy");
    let app = app_with_visible_local_session(cx, root.path(), "file-manager-test");
    let workspace = workspace(cx, &app, &root.path().to_string_lossy());
    let cx = session_focus_test_window(cx, &app);

    cx.update(|window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.split_percent = 65;
            workspace.set_active(PaneId::Right);
            workspace.dispatch_app(window, cx, |app, _, cx| {
                app.persist_file_manager_settings(cx);
            });
        });
    });
    wait_for(cx, "file-manager settings save", |cx| {
        !app.read(cx)
            .settings
            .dirty_persistence_domains()
            .contains(&SettingsPersistenceDomain::FileExplorer)
    });
    cx.read(|cx| {
        let settings = &app.read(cx).settings.summary().ui_file_manager;
        assert_eq!(settings.split_percent, 65);
        assert_eq!(
            settings.active_pane,
            nyaterm_core::FileManagerPaneSide::Right
        );
    });
}

struct FileManagerLayoutFixture {
    workspace: Entity<FileManagerWorkspace>,
    width: f32,
}

impl gpui::Render for FileManagerLayoutFixture {
    fn render(
        &mut self,
        _: &mut gpui::Window,
        _: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        use gpui::{ParentElement as _, Styled as _, div, px};
        div()
            .w(px(self.width + 160.))
            .h(px(600.))
            .pl(px(160.))
            .child(
                div()
                    .w(px(self.width))
                    .h_full()
                    .child(self.workspace.clone()),
            )
    }
}

fn draw_file_manager(cx: &mut VisualTestContext) {
    for _ in 0..3 {
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }
}

#[gpui::test]
fn parent_listing_reveals_selected_directory_after_list_layout(cx: &mut TestAppContext) {
    use nyaterm_files::sftp::{SftpFileEntry, SftpFileType};

    let root = TestTempDir::new("nyaterm-file-manager-parent-scroll");
    let app = app_with_visible_local_session(cx, root.path(), "file-manager-test");
    cx.update(|cx| {
        nyaterm_ui::apply_component_theme(
            crate::theme::theme_palette("dark"),
            gpui::font("Segoe UI"),
            gpui::px(14.),
            cx,
        )
    });
    let workspace = workspace(cx, &app, "/parent/item1500");
    workspace.update(cx, |state, _| {
        let pane = &mut state.left;
        pane.endpoint = PaneEndpoint::Session("sftp".into());
        let parent = RemoteFilePath::new("/parent");
        let generation = pane.begin_load(parent.clone());
        let entries = (0..2000)
            .rev()
            .map(|index| SftpFileEntry {
                name: format!("item{index}"),
                path: format!("/parent/item{index}"),
                file_type: SftpFileType::Directory,
                size: None,
                permissions: None,
                owner: String::new(),
                group: String::new(),
                modified_at: None,
                raw_path_token: None,
                symlink_target_is_directory: false,
            })
            .collect();
        assert!(pane.apply_listing(generation, &pane.endpoint.clone(), &parent, entries, true));
    });
    let fixture = cx.new(|_| FileManagerLayoutFixture {
        workspace: workspace.clone(),
        width: 900.,
    });
    let (_, cx) =
        cx.add_window_view(move |window, cx| nyaterm_ui::nya_root(fixture.clone(), window, cx));
    draw_file_manager(cx);
    let row = cx.debug_bounds("file-manager-row-left-item1500").unwrap();
    cx.read(|cx| {
        let state = workspace.read(cx);
        let pane = state.pane_bounds[0].unwrap();
        assert!(row.origin.y >= pane.origin.y);
        assert!(row.bottom() <= pane.bottom());
        assert_eq!(
            state.left.selected_path.as_deref(),
            Some("/parent/item1500")
        );
        assert!(
            state
                .left
                .list_scroll
                .0
                .borrow()
                .deferred_scroll_to_item
                .is_none()
        );
    });
}

#[gpui::test]
fn measured_workspace_switches_to_single_pane_and_keeps_input_focus(cx: &mut TestAppContext) {
    let root = TestTempDir::new("nyaterm-file-manager-geometry");
    let app = app_with_visible_local_session(cx, root.path(), "file-manager-test");
    cx.update(|cx| {
        nyaterm_ui::apply_component_theme(
            crate::theme::theme_palette("dark"),
            gpui::font("Segoe UI"),
            gpui::px(14.),
            cx,
        )
    });
    let workspace = workspace(cx, &app, &root.path().to_string_lossy());
    let fixture = cx.new(|_| FileManagerLayoutFixture {
        workspace: workspace.clone(),
        width: 900.,
    });
    let (_, cx) = cx.add_window_view({
        let fixture = fixture.clone();
        move |window, cx| nyaterm_ui::nya_root(fixture.clone(), window, cx)
    });
    draw_file_manager(cx);
    cx.read(|cx| {
        let state = workspace.read(cx);
        assert!(!state.single_pane);
        let bounds = state.bounds.unwrap();
        assert_eq!(bounds.origin.x, gpui::px(160.));
        assert_eq!(bounds.size.width, gpui::px(900.));
        assert!(state.pane_bounds.iter().all(Option::is_some));
    });
    workspace.update(cx, |state, _| {
        state.left.select_entry("kept-left".into());
        state.right.select_entry("kept-right".into());
    });
    fixture.update(cx, |fixture, cx| {
        fixture.width = 540.;
        cx.notify();
    });
    draw_file_manager(cx);
    cx.read(|cx| {
        let state = workspace.read(cx);
        assert!(state.single_pane);
        assert_eq!(state.left.selected_path.as_deref(), Some("kept-left"));
        assert_eq!(state.right.selected_path.as_deref(), Some("kept-right"));
    });
    cx.update(|window, cx| {
        workspace.update(cx, |state, cx| {
            state.begin_path_edit(PaneId::Right, window, cx)
        })
    });
    draw_file_manager(cx);
    cx.update(|window, cx| {
        let state = workspace.read(cx);
        assert!(
            state
                .inputs(PaneId::Right)
                .path
                .read(cx)
                .component_focus_handle(cx)
                .contains_focused(window, cx)
        );
    });
    cx.simulate_keystrokes(if cfg!(target_os = "macos") {
        "cmd-a delete"
    } else {
        "ctrl-a delete"
    });
    cx.read(|cx| {
        assert_eq!(
            workspace.read(cx).right.selected_path.as_deref(),
            Some("kept-right")
        )
    });
    cx.simulate_keystrokes("escape");
    draw_file_manager(cx);
    cx.update(|window, cx| {
        let state = workspace.read(cx);
        assert!(!state.right.path_editing);
        assert_eq!(state.active, PaneId::Right);
        assert!(state.right.focus.is_focused(window));
    });
    for scale in [1., 1.25, 1.5, 2.] {
        cx.simulate_scale_factor_change(scale);
        draw_file_manager(cx);
    }
}

#[gpui::test]
fn frozen_workspace_intent_survives_source_selection_changes_and_swap(cx: &mut TestAppContext) {
    use crate::features::transfers::file_operations::FileTransferOperation;
    let root = TestTempDir::new("nyaterm-file-manager-freeze");
    let app = app_with_visible_local_session(cx, root.path(), "file-manager-test");
    let workspace = workspace(cx, &app, "/initial");
    workspace.update(cx, |state, cx| {
        let entry = nyaterm_files::sftp::SftpFileEntry {
            name: "�".into(),
            path: "/initial/�".into(),
            raw_path_token: RemoteFilePath::from_raw("", b"/initial/\xff").raw_path_token,
            file_type: nyaterm_files::sftp::SftpFileType::File,
            size: Some(3),
            permissions: None,
            owner: String::new(),
            group: String::new(),
            modified_at: None,
            symlink_target_is_directory: false,
        };
        state.left.endpoint = PaneEndpoint::Session("original".into());
        state.left.entries = std::sync::Arc::new(vec![entry.clone()]);
        state.left.select_entry(entry.identity_key());
        state.right.path = RemoteFilePath::from_raw("/target", b"/target");
        let intent = state.transfer_intent(FileTransferOperation::Move);
        let selection = state.selection(PaneId::Left);
        let left_generation = state.left.begin_load(RemoteFilePath::new("/pending"));
        state
            .left
            .set_endpoint(PaneEndpoint::Session("different".into()));
        state.left.select_entry("different-selection".into());
        state.swap_panes();
        assert_eq!(intent.source, PaneEndpoint::Session("original".into()));
        assert_eq!(
            intent.entries[0].remote_path().raw_path().unwrap().unwrap(),
            b"/initial/\xff"
        );
        assert_eq!(
            intent.target_directory.raw_path().unwrap().unwrap(),
            b"/target"
        );
        assert_eq!(selection.endpoint, PaneEndpoint::Session("original".into()));
        assert_eq!(selection.entries.len(), 1);
        assert!(!state.left.apply_listing(
            left_generation,
            &PaneEndpoint::Session("original".into()),
            &RemoteFilePath::new("/pending"),
            vec![],
            true
        ));
        cx.notify();
    });
}

#[gpui::test]
fn rendered_context_menu_keeps_multi_selection_and_activates_the_clicked_pane(
    cx: &mut TestAppContext,
) {
    use gpui::{Modifiers, MouseButton};
    let root = TestTempDir::new("nyaterm-file-manager-context-menu");
    std::fs::create_dir_all(root.join("data")).unwrap();
    for name in ["alpha", "beta", "gamma"] {
        std::fs::write(root.join("data").join(name), b"fixture").unwrap();
    }
    let app = app_with_visible_local_session(cx, root.path(), "file-manager-test");
    cx.update(|cx| {
        nyaterm_ui::apply_component_theme(
            crate::theme::theme_palette("dark"),
            gpui::font("Segoe UI"),
            gpui::px(14.),
            cx,
        )
    });
    let workspace = workspace(cx, &app, &root.join("data").to_string_lossy());
    let entries = nyaterm_files::file_browser::FileBrowserService::local()
        .list_dir(root.join("data").to_string_lossy())
        .unwrap();
    workspace.update(cx, |state, _| {
        state.left.entries = std::sync::Arc::new(entries.clone());
        state.right.entries = std::sync::Arc::new(entries.clone());
        state.left.select_entry(entries[0].identity_key());
        state.left.toggle_mark(entries[1].identity_key());
    });
    let fixture = cx.new(|_| FileManagerLayoutFixture {
        workspace: workspace.clone(),
        width: 900.,
    });
    let (_, cx) =
        cx.add_window_view(move |window, cx| nyaterm_ui::nya_root(fixture.clone(), window, cx));
    draw_file_manager(cx);
    let left = cx.debug_bounds("file-manager-row-left-alpha").unwrap();
    assert_eq!(left.size.height, gpui::px(30.));
    cx.simulate_mouse_down(left.center(), MouseButton::Right, Modifiers::none());
    cx.simulate_mouse_up(left.center(), MouseButton::Right, Modifiers::none());
    draw_file_manager(cx);
    cx.read(|cx| {
        let state = workspace.read(cx);
        assert_eq!(state.left.selected_paths.len(), 2);
        assert_eq!(state.active, PaneId::Left);
    });
    cx.simulate_keystrokes("escape");
    draw_file_manager(cx);
    let right = cx.debug_bounds("file-manager-row-right-gamma").unwrap();
    cx.simulate_mouse_down(right.center(), MouseButton::Right, Modifiers::none());
    cx.simulate_mouse_up(right.center(), MouseButton::Right, Modifiers::none());
    draw_file_manager(cx);
    cx.read(|cx| {
        let state = workspace.read(cx);
        assert_eq!(state.active, PaneId::Right);
        assert_eq!(state.right.selected_paths.len(), 1);
        assert_eq!(state.left.selected_paths.len(), 2);
        assert!(
            state
                .right
                .selected_entries()
                .iter()
                .any(|entry| entry.name == "gamma")
        );
    });
}
