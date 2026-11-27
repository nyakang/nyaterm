mod focus;

use crate::features::NyaTermApp;
use crate::features::file_manager::pane::{PaneEndpoint, PaneId};
use crate::features::file_manager::workspace::{FileManagerSeed, FileManagerWorkspace};
use crate::features::settings::SettingsPersistenceDomain;
use crate::features::test_support::{
    app_with_visible_local_session, app_with_visible_ssh_session, session_focus_test_window,
};
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

fn menu_fixture(name: &str, directory: bool) -> nyaterm_files::sftp::SftpFileEntry {
    nyaterm_files::sftp::SftpFileEntry {
        name: name.into(),
        path: format!("/fixture/{name}"),
        file_type: if directory {
            nyaterm_files::sftp::SftpFileType::Directory
        } else {
            nyaterm_files::sftp::SftpFileType::File
        },
        size: Some(1),
        permissions: None,
        owner: String::new(),
        group: String::new(),
        modified_at: None,
        raw_path_token: None,
        symlink_target_is_directory: false,
    }
}

#[gpui::test]
fn context_menu_keeps_five_slots_and_disables_rename_and_properties_for_multiple_entries(
    cx: &mut TestAppContext,
) {
    let root = TestTempDir::new("nyaterm-file-manager-menu");
    let app = app_with_visible_local_session(cx, root.path(), "file-manager-menu");
    let workspace = workspace(cx, &app, &root.path().to_string_lossy());
    workspace.update(cx, |workspace, cx| {
        let first = menu_fixture("first.txt", false);
        let second = menu_fixture("second.txt", false);
        let pane = workspace.pane_mut(PaneId::Left);
        pane.entries = std::sync::Arc::new(vec![first.clone(), second.clone()]);
        pane.selected_paths
            .extend([first.identity_key(), second.identity_key()]);
        let items = workspace.entry_menu(PaneId::Left, first, cx);
        let commands = items[0].test_action_bar_items().expect("top action bar");
        assert_eq!(commands.len(), 5);
        assert_eq!(
            commands
                .iter()
                .map(|item| item.test_label().to_string())
                .collect::<Vec<_>>(),
            [
                rust_i18n::t!("menu.cut").to_string(),
                rust_i18n::t!("menu.copy").to_string(),
                rust_i18n::t!("menu.paste").to_string(),
                rust_i18n::t!("fileExplorer.cmRename").to_string(),
                rust_i18n::t!("fileExplorer.delete").to_string(),
            ]
        );
        assert!(!commands[0].test_presentation().3);
        assert!(!commands[1].test_presentation().3);
        assert!(
            commands[2].test_presentation().3,
            "empty clipboard disables paste"
        );
        assert!(
            commands[3].test_presentation().3,
            "multiple selection disables rename"
        );
        assert!(
            commands[4].test_presentation().5,
            "delete keeps danger styling"
        );
        assert!(
            items.last().unwrap().test_presentation().3,
            "multiple selection disables properties"
        );
        let paths = items
            .iter()
            .find(|item| item.test_label() == rust_i18n::t!("fileExplorer.cmCopyPath"))
            .unwrap();
        assert_eq!(paths.children().unwrap().len(), 3);
        for shortcut in ["Enter", "F3", "F4", "F5", "F6"] {
            assert!(
                items
                    .iter()
                    .any(|item| item.test_presentation().2.as_deref() == Some(shortcut))
            );
        }
    });
}

#[gpui::test]
fn directory_context_menu_only_offers_open_and_labels_the_opposite_pane(cx: &mut TestAppContext) {
    let root = TestTempDir::new("nyaterm-file-manager-directory-menu");
    let app = app_with_visible_local_session(cx, root.path(), "file-manager-menu");
    let workspace = workspace(cx, &app, &root.path().to_string_lossy());
    workspace.update(cx, |workspace, cx| {
        let entry = menu_fixture("folder", true);
        for pane in [PaneId::Left, PaneId::Right] {
            workspace.pane_mut(pane).entries = std::sync::Arc::new(vec![entry.clone()]);
            workspace.pane_mut(pane).select_entry(entry.identity_key());
            let items = workspace.entry_menu(pane, entry.clone(), cx);
            assert!(!items.last().unwrap().test_presentation().3);
            assert!(
                items.iter().all(|item| !matches!(
                    item.test_presentation().2.as_deref(),
                    Some("F3" | "F4")
                ))
            );
            let side = rust_i18n::t!(if pane == PaneId::Left {
                "fileManager.rightSide"
            } else {
                "fileManager.leftSide"
            });
            let expected = rust_i18n::t!(
                "fileManager.copyToPane",
                side = side.as_ref(),
                target = rust_i18n::t!("fileManager.local").as_ref()
            );
            assert!(items.iter().any(|item| item.test_label() == expected));
            workspace.pane_mut(pane).disconnected = true;
            let items = workspace.entry_menu(pane, entry.clone(), cx);
            assert!(
                items[0]
                    .test_action_bar_items()
                    .unwrap()
                    .iter()
                    .all(|item| item.test_presentation().3)
            );
            workspace.pane_mut(pane).disconnected = false;
        }
    });
}

#[gpui::test]
fn local_properties_load_from_the_captured_pane_without_changing_the_sidebar(
    cx: &mut TestAppContext,
) {
    cx.executor().allow_parking();
    let root = TestTempDir::new("nyaterm-file-manager-properties");
    let app = app_with_visible_ssh_session(cx, root.path(), "unrelated-ssh");
    std::fs::write(root.path().join("properties.txt"), "property fixture").unwrap();
    let workspace = workspace(cx, &app, &root.path().to_string_lossy());
    let entry = nyaterm_files::file_browser::FileBrowserService::local()
        .list_dir(root.path().to_string_lossy())
        .unwrap()
        .into_iter()
        .find(|entry| entry.name == "properties.txt")
        .unwrap();
    workspace.update(cx, |workspace, _| {
        workspace.left.entries = std::sync::Arc::new(vec![entry.clone()]);
        workspace.left.select_entry(entry.identity_key());
    });
    cx.update(|cx| {
        nyaterm_ui::init_components(cx);
        nyaterm_ui::apply_component_theme(
            crate::theme::theme_palette("dark"),
            gpui::font("Segoe UI"),
            gpui::px(14.),
            cx,
        );
    });
    let fixture = cx.new(|_| FileManagerLayoutFixture {
        workspace: workspace.clone(),
        width: 900.,
    });
    let (_, cx) =
        cx.add_window_view(move |window, cx| nyaterm_ui::nya_root(fixture.clone(), window, cx));
    draw_file_manager(cx);
    let sidebar_path = cx.read(|cx| app.read(cx).transfer.browser_view().path.to_string());
    cx.update(|window, cx| {
        let selection = workspace.read(cx).selection(PaneId::Left);
        app.update(cx, |app, cx| {
            app.open_file_manager_properties(
                "file-manager-reentrancy".into(),
                PaneId::Left,
                selection,
                window,
                cx,
            )
        });
        workspace.update(cx, |workspace, _| workspace.active = PaneId::Right);
    });
    wait_for(cx, "local property load", |cx| {
        app.read(cx)
            .transfer
            .properties_dialog()
            .is_some_and(|state| state.properties.is_some() || state.error.is_some())
    });
    cx.read(|cx| {
        let app = app.read(cx);
        let state = app.transfer.properties_dialog().unwrap();
        assert_eq!(state.entry.path, entry.path);
        assert!(state.error.is_none(), "{:?}", state.error);
        assert_eq!(state.properties.as_ref().unwrap().size, Some(16));
        assert_eq!(app.transfer.browser_view().path, &sidebar_path);
        assert_eq!(
            app.transfer.properties_target().unwrap().endpoint,
            PaneEndpoint::Local
        );
    });
}

#[gpui::test]
fn reopening_properties_drops_the_previous_request_and_loading_errors_stay_in_the_dialog(
    cx: &mut TestAppContext,
) {
    cx.executor().allow_parking();
    let root = TestTempDir::new("nyaterm-file-manager-properties-stale");
    let app = app_with_visible_local_session(cx, root.path(), "properties-stale");
    let workspace = workspace(cx, &app, &root.path().to_string_lossy());
    cx.update(|cx| {
        nyaterm_ui::init_components(cx);
        nyaterm_ui::apply_component_theme(
            crate::theme::theme_palette("dark"),
            gpui::font("Segoe UI"),
            gpui::px(14.),
            cx,
        );
    });
    let fixture = cx.new(|_| FileManagerLayoutFixture {
        workspace: workspace.clone(),
        width: 900.,
    });
    let (_, cx) =
        cx.add_window_view(move |window, cx| nyaterm_ui::nya_root(fixture.clone(), window, cx));
    draw_file_manager(cx);
    let selection = |name: &str| crate::features::file_manager::operations::FileManagerSelection {
        endpoint: PaneEndpoint::Local,
        directory: RemoteFilePath::new(root.path().to_string_lossy().into_owned()),
        entries: vec![nyaterm_files::sftp::SftpFileEntry {
            path: root.path().join(name).to_string_lossy().into_owned(),
            ..menu_fixture(name, false)
        }],
    };
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.open_file_manager_properties(
                "file-manager-reentrancy".into(),
                PaneId::Left,
                selection("first-missing"),
                window,
                cx,
            );
            let previous = app.transfer.properties_target().unwrap().request_id.clone();
            app.open_file_manager_properties(
                "file-manager-reentrancy".into(),
                PaneId::Right,
                selection("second-missing"),
                window,
                cx,
            );
            assert_ne!(
                app.transfer.properties_target().unwrap().request_id,
                previous
            );
        });
    });
    wait_for(cx, "property load failure", |cx| {
        app.read(cx)
            .transfer
            .properties_dialog()
            .is_some_and(|state| state.error.is_some())
    });
    cx.read(|cx| {
        let app = app.read(cx);
        assert_eq!(
            app.transfer.properties_dialog().unwrap().entry.name,
            "second-missing"
        );
        assert!(
            app.transfer
                .properties_dialog()
                .unwrap()
                .properties
                .is_none()
        );
        assert!(app.transfer.properties_dialog().unwrap().error.is_some());
        assert_eq!(
            app.transfer.properties_target().unwrap().pane,
            PaneId::Right
        );
        assert_eq!(workspace.read(cx).active, PaneId::Left);
    });
}

#[gpui::test]
fn opening_an_sftp_session_loads_the_local_pane_without_a_manual_refresh(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = TestTempDir::new("nyaterm-sftp-initial-local-listing");
    let session_id = "sftp-initial-local-listing";
    let app = app_with_visible_ssh_session(cx, root.path(), session_id);
    let workspace = app.update(cx, |app, cx| {
        let metadata = app.session.metadata_mut(session_id).unwrap();
        metadata.confirmed_ssh_runtime_mode =
            Some(nyaterm_core::models::ssh_runtime::SshRuntimeMode::Sftp);
        metadata.disconnected = true;
        app.ensure_sftp_session_view(session_id, cx);
        app.file_manager
            .workspace(&format!("sftp-{session_id}"))
            .unwrap()
    });
    cx.read(|cx| {
        let local = workspace.read(cx).pane(PaneId::Right);
        assert_eq!(local.endpoint, PaneEndpoint::Local);
        assert!(
            local.generation > 0,
            "opening the view must start the local load"
        );
    });
    let cx = session_focus_test_window(cx, &app);
    wait_for(cx, "initial local listing", |cx| {
        let local = workspace.read(cx).pane(PaneId::Right);
        !local.loading && local.last_successful_path.is_some()
    });
    let generation = cx.read(|cx| {
        let local = workspace.read(cx).pane(PaneId::Right);
        assert!(local.error.is_none());
        assert!(!local.home_dir_pending);
        assert!(!local.home_dir.is_empty());
        assert_eq!(local.path.display_path, local.home_dir);
        assert_eq!(local.last_successful_path.as_ref(), Some(&local.path));
        // The local listing must complete even without a connected remote backend.
        assert!(workspace.read(cx).pane(PaneId::Left).error.is_some());
        local.generation
    });
    app.update(cx, |app, cx| app.ensure_sftp_session_view(session_id, cx));
    cx.read(|cx| {
        assert_eq!(
            workspace.read(cx).pane(PaneId::Right).generation,
            generation
        );
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
fn context_action_bar_copy_keeps_the_original_selection_after_focus_changes(
    cx: &mut TestAppContext,
) {
    use gpui::{Modifiers, MouseButton};
    let root = TestTempDir::new("nyaterm-file-manager-menu-copy");
    std::fs::create_dir_all(root.join("files")).unwrap();
    for name in ["alpha", "beta", "gamma"] {
        std::fs::write(root.join("files").join(name), b"fixture").unwrap();
    }
    let app = app_with_visible_local_session(cx, root.path(), "file-manager-test");
    cx.update(|cx| {
        nyaterm_ui::init_components(cx);
        nyaterm_ui::apply_component_theme(
            crate::theme::theme_palette("dark"),
            gpui::font("Segoe UI"),
            gpui::px(14.),
            cx,
        );
    });
    let workspace = workspace(cx, &app, &root.join("files").to_string_lossy());
    let entries = nyaterm_files::file_browser::FileBrowserService::local()
        .list_dir(root.join("files").to_string_lossy())
        .unwrap();
    workspace.update(cx, |workspace, _| {
        workspace.left.entries = std::sync::Arc::new(entries.clone());
        workspace.left.selected_paths.extend(
            entries
                .iter()
                .filter(|entry| entry.name != "gamma")
                .map(|entry| entry.identity_key()),
        );
        workspace.right.entries = std::sync::Arc::new(entries.clone());
        workspace.right.select_entry(
            entries
                .iter()
                .find(|entry| entry.name == "gamma")
                .unwrap()
                .identity_key(),
        );
    });
    let fixture = cx.new(|_| FileManagerLayoutFixture {
        workspace: workspace.clone(),
        width: 900.,
    });
    let (_, cx) =
        cx.add_window_view(move |window, cx| nyaterm_ui::nya_root(fixture.clone(), window, cx));
    draw_file_manager(cx);
    let row = cx.debug_bounds("file-manager-row-left-alpha").unwrap();
    cx.simulate_mouse_down(row.center(), MouseButton::Right, Modifiers::none());
    cx.simulate_mouse_up(row.center(), MouseButton::Right, Modifiers::none());
    draw_file_manager(cx);
    workspace.update(cx, |workspace, _| {
        workspace.active = PaneId::Right;
        workspace.left.clear_selection();
    });
    cx.update(|window, cx| {
        window.focus_next(cx);
        window.focus_next(cx);
    });
    cx.simulate_keystrokes("enter");
    draw_file_manager(cx);
    cx.read(|cx| {
        let clipboard = app
            .read(cx)
            .transfer
            .file_clipboard()
            .expect("copy command publishes files");
        assert!(!clipboard.cut);
        assert_eq!(
            clipboard
                .entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            ["alpha", "beta"]
        );
    });
}

#[gpui::test]
fn source_dropdown_switches_a_narrow_pane_and_releases_the_workspace(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = TestTempDir::new("nyaterm-file-manager-source-dropdown");
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
    workspace.update(cx, |workspace, _| {
        workspace.split_percent = 20;
        workspace.left.endpoint = PaneEndpoint::OfflineConnection("offline-source".into());
        workspace.right.select_entry("kept-right".into());
    });
    let fixture = cx.new(|_| FileManagerLayoutFixture {
        workspace: workspace.clone(),
        width: 900.,
    });
    let (_, cx) =
        cx.add_window_view(move |window, cx| nyaterm_ui::nya_root(fixture.clone(), window, cx));
    draw_file_manager(cx);
    let source = cx.debug_bounds("file-manager-source-left").unwrap();
    cx.simulate_click(
        gpui::point(source.right() - gpui::px(10.), source.center().y),
        Default::default(),
    );
    draw_file_manager(cx);
    cx.update(|window, cx| assert!(nyaterm_ui::nya_popup_is_active(window, cx)));
    cx.simulate_keystrokes("down enter");
    draw_file_manager(cx);
    wait_for_listings(cx, &workspace);
    cx.read(|cx| {
        let state = workspace.read(cx);
        assert_eq!(state.left.endpoint, PaneEndpoint::Local);
        assert!(state.left.generation > 0);
        assert_eq!(state.right.selected_path.as_deref(), Some("kept-right"));
    });
}

#[gpui::test]
fn search_opens_on_typing_and_escape_clears_then_restores_list_focus(cx: &mut TestAppContext) {
    let root = TestTempDir::new("nyaterm-file-manager-search-overlay");
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
    let (_, cx) =
        cx.add_window_view(move |window, cx| nyaterm_ui::nya_root(fixture.clone(), window, cx));
    draw_file_manager(cx);
    cx.update(|window, cx| {
        workspace.update(cx, |state, cx| state.focus_pane(PaneId::Left, window, cx))
    });
    cx.simulate_keystrokes("a");
    draw_file_manager(cx);
    cx.update(|window, cx| {
        let state = workspace.read(cx);
        assert!(state.left.search_expanded);
        assert_eq!(state.left.search, "a");
        assert!(
            state
                .inputs(PaneId::Left)
                .search
                .read(cx)
                .component_focus_handle(cx)
                .contains_focused(window, cx)
        );
        assert!(state.right.search.is_empty());
    });
    cx.simulate_keystrokes("escape");
    draw_file_manager(cx);
    cx.read(|cx| {
        let state = workspace.read(cx);
        assert!(state.left.search_expanded);
        assert!(state.left.search.is_empty());
    });
    cx.simulate_keystrokes("escape");
    draw_file_manager(cx);
    cx.update(|window, cx| {
        let state = workspace.read(cx);
        assert!(!state.left.search_expanded);
        assert!(state.left.focus.is_focused(window));
    });
    cx.simulate_keystrokes(if cfg!(target_os = "macos") {
        "cmd-f"
    } else {
        "ctrl-f"
    });
    draw_file_manager(cx);
    cx.read(|cx| assert!(workspace.read(cx).left.search_expanded));
}

#[gpui::test]
fn child_menu_keeps_endpoint_identity_and_discards_a_closed_request(cx: &mut TestAppContext) {
    use crate::features::file_manager::navigation_runtime::ChildrenRequest;
    use crate::features::file_manager::pane::PanePathMenu;
    cx.executor().allow_parking();
    let root = TestTempDir::new("nyaterm-file-manager-children");
    std::fs::create_dir_all(root.join("child")).unwrap();
    let app = app_with_visible_local_session(cx, root.path(), "file-manager-test");
    let path = RemoteFilePath::new(root.path().to_string_lossy());
    let workspace = workspace(cx, &app, &path.display_path);
    let cx = session_focus_test_window(cx, &app);
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.file_manager_children(
                "file-manager-reentrancy".into(),
                PaneId::Right,
                ChildrenRequest {
                    endpoint: PaneEndpoint::Local,
                    generation: 0,
                    path: path.clone(),
                    branch: None,
                },
                cx,
            )
        })
    });
    wait_for(cx, "child-directory menu", |cx| {
        matches!(
            workspace.read(cx).right.path_menu,
            Some(PanePathMenu::Children {
                result: Some(_),
                ..
            })
        )
    });
    cx.read(|cx| {
        let state = workspace.read(cx);
        let Some(PanePathMenu::Children {
            result: Some(Ok(entries)),
            ..
        }) = &state.right.path_menu
        else {
            panic!("missing directory listing")
        };
        assert!(entries.iter().any(|entry| entry.name == "child"));
        assert!(state.left.path_menu.is_none());
    });
    cx.update(|_, cx| {
        workspace.update(cx, |state, _| {
            state.right.path_menu = None;
            assert!(!state.right.apply_children_result(
                0,
                &PaneEndpoint::Local,
                1,
                &path,
                Ok(vec![])
            ));
        })
    });
    // A deferred click from an earlier source must not open a menu on the new source.
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.file_manager_children(
                "file-manager-reentrancy".into(),
                PaneId::Left,
                ChildrenRequest {
                    endpoint: PaneEndpoint::Session("old-source".into()),
                    generation: 0,
                    path: path.clone(),
                    branch: None,
                },
                cx,
            )
        })
    });
    cx.read(|cx| assert!(workspace.read(cx).left.path_menu.is_none()));
}

#[gpui::test]
fn favorites_share_saved_connection_settings_and_keep_temporary_sources_in_memory(
    cx: &mut TestAppContext,
) {
    cx.executor().allow_parking();
    let root = TestTempDir::new("nyaterm-file-manager-favorites");
    let app = app_with_visible_local_session(cx, root.path(), "file-manager-test");
    app.update(cx, |app, cx| {
        let saved = PaneEndpoint::OfflineConnection("saved-source".into());
        app.set_file_manager_favorite(saved.clone(), "/favorite".into(), true, cx);
        assert_eq!(app.file_manager_favorites(&saved), ["/favorite"]);
        assert_eq!(
            app.settings
                .summary()
                .ui_file_explorer_favorite_dirs_by_connection_id["saved-source"],
            ["/favorite"]
        );
        app.set_file_manager_favorite(PaneEndpoint::Local, "C:\\temporary".into(), true, cx);
        assert_eq!(
            app.file_manager_favorites(&PaneEndpoint::Local),
            ["C:\\temporary"]
        );
        assert_eq!(app.file_manager_favorites(&saved), ["/favorite"]);
        app.set_file_manager_favorite(saved.clone(), "/favorite".into(), false, cx);
        assert!(app.file_manager_favorites(&saved).is_empty());
        assert!(
            !app.settings
                .summary()
                .ui_file_explorer_favorite_dirs_by_connection_id
                .contains_key("saved-source")
        );
    });
}

#[gpui::test]
fn breadcrumb_popup_mouse_and_keyboard_navigation_keep_the_original_path(cx: &mut TestAppContext) {
    use crate::features::file_manager::pane::PanePathMenu;
    use gpui::{Modifiers, MouseButton};
    cx.executor().allow_parking();
    let root = TestTempDir::new("nyaterm-file-manager-path-popup");
    let app = app_with_visible_local_session(cx, root.path(), "file-manager-test");
    std::fs::create_dir_all(root.join("child")).unwrap();
    let path = RemoteFilePath::new(root.join("child").to_string_lossy());
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
    let (_, cx) =
        cx.add_window_view(move |window, cx| nyaterm_ui::nya_root(fixture.clone(), window, cx));
    for keyboard in [false, true] {
        cx.update(|window, cx| {
            workspace.update(cx, |state, cx| {
                state.focus_pane(PaneId::Right, window, cx);
                state
                    .right
                    .open_path_menu(PanePathMenu::Paths(vec![("child".into(), path.clone())]));
                cx.notify();
            })
        });
        draw_file_manager(cx);
        if keyboard {
            cx.simulate_keystrokes("enter");
        } else {
            let row = cx.debug_bounds("fm-path-item-right-0").unwrap();
            cx.simulate_mouse_down(row.center(), MouseButton::Left, Modifiers::none());
            cx.simulate_mouse_up(row.center(), MouseButton::Left, Modifiers::none());
        }
        wait_for_listings(cx, &workspace);
        cx.read(|cx| {
            let state = workspace.read(cx);
            assert_eq!(state.right.path, path);
            assert!(state.right.path_menu.is_none());
        });
    }
}

#[gpui::test]
fn editing_a_restored_path_resolves_home_without_navigating_and_expands_tilde(
    cx: &mut TestAppContext,
) {
    cx.executor().allow_parking();
    let root = TestTempDir::new("nyaterm-file-manager-path-home");
    let app = app_with_visible_local_session(cx, root.path(), "file-manager-test");
    std::fs::create_dir_all(root.join("home")).unwrap();
    cx.update(|cx| {
        nyaterm_ui::apply_component_theme(
            crate::theme::theme_palette("dark"),
            gpui::font("Segoe UI"),
            gpui::px(14.),
            cx,
        )
    });
    let original = root.path().to_string_lossy().into_owned();
    let home = root.join("home").to_string_lossy().into_owned();
    let workspace = workspace(cx, &app, &original);
    let fixture = cx.new(|_| FileManagerLayoutFixture {
        workspace: workspace.clone(),
        width: 900.,
    });
    let (_, cx) =
        cx.add_window_view(move |window, cx| nyaterm_ui::nya_root(fixture.clone(), window, cx));
    draw_file_manager(cx);
    cx.update(|window, cx| {
        workspace.update(cx, |state, cx| {
            state.begin_path_edit(PaneId::Right, window, cx)
        })
    });
    wait_for(cx, "path home lookup", |cx| {
        !workspace.read(cx).right.home_dir_pending
    });
    cx.update(|window, cx| {
        workspace.update(cx, |state, cx| {
            assert_eq!(state.right.path.display_path, original);
            assert!(!state.right.home_dir.is_empty());
            state.right.home_dir = home.clone();
            state.inputs(PaneId::Right).path.update(cx, |input, cx| {
                input.set_content_silent("~", cx);
                input.focus_with_cursor_at_end(window, cx);
            });
        })
    });
    draw_file_manager(cx);
    cx.simulate_keystrokes("enter");
    wait_for_listings(cx, &workspace);
    cx.read(|cx| {
        let state = workspace.read(cx);
        assert_eq!(state.right.path.display_path, home);
        assert!(!state.right.path_editing);
    });
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
    cx.executor().allow_parking();
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
    workspace.update(cx, |state, cx| {
        state.split_percent = 20;
        cx.notify();
    });
    draw_file_manager(cx);
    for (pane, selector) in [
        (PaneId::Left, "file-manager-actions-left"),
        (PaneId::Right, "file-manager-actions-right"),
    ] {
        let actions = cx.debug_bounds(selector).unwrap();
        let source = cx
            .debug_bounds(match pane {
                PaneId::Left => "file-manager-source-left",
                PaneId::Right => "file-manager-source-right",
            })
            .unwrap();
        cx.read(|cx| {
            let bounds =
                workspace.read(cx).pane_bounds[if pane == PaneId::Left { 0 } else { 1 }].unwrap();
            assert!(actions.origin.x >= bounds.origin.x);
            assert!(actions.right() <= bounds.right());
            assert!(source.origin.x >= bounds.origin.x);
            assert!(source.right() <= bounds.right());
            assert!(source.size.width > gpui::px(24.));
        });
    }
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
    cx.read(|cx| assert!(workspace.read(cx).right.path_editing));
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
        let actions = cx.debug_bounds("file-manager-actions-right").unwrap();
        cx.read(|cx| {
            let pane = workspace.read(cx).pane_bounds[1].unwrap();
            assert!(actions.origin.x >= pane.origin.x);
            assert!(actions.right() <= pane.right());
        });
    }
}

#[gpui::test]
fn either_pane_swap_button_keeps_search_selection_and_focus_with_the_source(
    cx: &mut TestAppContext,
) {
    use gpui::{Modifiers, MouseButton};

    cx.executor().allow_parking();
    let root = TestTempDir::new("nyaterm-file-manager-toolbar-swap");
    for name in ["left", "right"] {
        let directory = root.join(name);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join(format!("{name}.txt")), b"fixture").unwrap();
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
    let workspace = workspace(cx, &app, &root.join("left").to_string_lossy());
    workspace.update(cx, |state, cx| {
        for (pane, name) in [(PaneId::Left, "left"), (PaneId::Right, "right")] {
            let directory = root.join(name).to_string_lossy().into_owned();
            let entries = nyaterm_files::file_browser::FileBrowserService::local()
                .list_dir(&directory)
                .unwrap();
            let state = state.pane_mut(pane);
            state.path = RemoteFilePath::new(directory);
            state.select_entry(entries[0].identity_key());
            state.entries = std::sync::Arc::new(entries);
            state.search = name.to_string();
        }
        for pane in [PaneId::Left, PaneId::Right] {
            let search = state.pane(pane).search.clone();
            state
                .inputs(pane)
                .search
                .update(cx, |input, cx| input.set_content_silent(&search, cx));
        }
        state.active = PaneId::Right;
    });
    let fixture = cx.new(|_| FileManagerLayoutFixture {
        workspace: workspace.clone(),
        width: 900.,
    });
    let (_, cx) =
        cx.add_window_view(move |window, cx| nyaterm_ui::nya_root(fixture.clone(), window, cx));
    draw_file_manager(cx);

    for (clicked, selector) in [
        (PaneId::Left, "file-manager-swap-left"),
        (PaneId::Right, "file-manager-swap-right"),
    ] {
        workspace.update(cx, |state, cx| {
            state.pane_mut(clicked).search.clear();
            state.pane_mut(clicked).search_expanded = false;
            state
                .inputs(clicked)
                .search
                .update(cx, |input, cx| input.set_content_silent("", cx));
            cx.notify();
        });
        draw_file_manager(cx);
        let previous = cx.read(|cx| {
            let state = workspace.read(cx);
            [PaneId::Left, PaneId::Right].map(|pane| {
                let pane = state.pane(pane);
                (
                    pane.path.clone(),
                    pane.search.clone(),
                    pane.selected_paths.clone(),
                )
            })
        });
        let button = cx.debug_bounds(selector).unwrap();
        cx.simulate_mouse_down(button.center(), MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_up(button.center(), MouseButton::Left, Modifiers::none());
        draw_file_manager(cx);
        wait_for_listings(cx, &workspace);
        cx.update(|window, cx| {
            let state = workspace.read(cx);
            assert_eq!(state.active, clicked.other());
            assert!(state.pane(state.active).focus.is_focused(window));
            for (pane, expected) in [PaneId::Left, PaneId::Right]
                .into_iter()
                .zip(previous.iter().rev())
            {
                assert_eq!(state.pane(pane).path, expected.0);
                assert_eq!(state.pane(pane).search, expected.1);
                assert_eq!(state.pane(pane).selected_paths, expected.2);
                if !expected.1.is_empty() {
                    assert_eq!(state.inputs(pane).search.read(cx).value(cx), expected.1);
                }
            }
        });
        cx.update(|window, cx| {
            workspace.update(cx, |state, cx| {
                for (pane, expected) in [PaneId::Left, PaneId::Right]
                    .into_iter()
                    .zip(previous.iter().rev())
                {
                    if expected.1.is_empty() {
                        // Hidden inputs apply their queued programmatic value when revealed.
                        state.focus_search(pane, None, window, cx);
                        assert_eq!(state.inputs(pane).search.read(cx).value(cx), expected.1);
                        state.clear_or_close_search(pane, window, cx);
                    }
                }
                state.focus_pane(clicked.other(), window, cx);
            })
        });
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

#[gpui::test]
fn system_file_drop_on_an_offline_adb_pane_paints_a_localized_error_notification(
    cx: &mut TestAppContext,
) {
    use gpui::{ExternalPaths, FileDropEvent};
    cx.executor().allow_parking();
    let root = TestTempDir::new("nyaterm-file-manager-offline-adb-drop");
    std::fs::create_dir_all(root.path().join("target")).unwrap();
    let input = root.path().join("input.txt");
    std::fs::write(&input, b"offline drop").unwrap();
    let app = crate::features::test_support::app_with_visible_android_session(
        cx,
        root.path(),
        "adb-drop-test",
        "/device",
    );
    let palette = crate::theme::theme_palette("dark");
    cx.update(|cx| {
        cx.set_reduce_motion(true);
        nyaterm_ui::apply_component_theme(palette, gpui::font("Segoe UI"), gpui::px(14.), cx);
    });
    let workspace = workspace(cx, &app, "/device");
    let key = cx.read(|cx| {
        app.read(cx)
            .android_config_for_session("adb-drop-test")
            .unwrap()
            .device_key
    });
    workspace.update(cx, |state, _| {
        state.left.endpoint = PaneEndpoint::Adb(key);
        state.left.disconnected = true;
        state.left.entries = std::sync::Arc::new(
            nyaterm_files::file_browser::FileBrowserService::local()
                .list_dir(root.path().to_string_lossy())
                .unwrap(),
        );
    });
    let fixture = cx.new(|_| FileManagerLayoutFixture {
        workspace: workspace.clone(),
        width: 900.,
    });
    let (_, cx) =
        cx.add_window_view(move |window, cx| nyaterm_ui::nya_root(fixture.clone(), window, cx));
    cx.update(|window, cx| {
        app.update(cx, |app, _| {
            app.shell
                .remember_main_window(window.window_handle().downcast().unwrap());
        });
    });
    draw_file_manager(cx);
    let surface = gpui::Background::from(gpui::rgb(palette.surface));
    let painted_surfaces = |window: &gpui::Window| {
        window
            .painted_quads()
            .iter()
            .filter(|quad| quad.background == surface)
            .count()
    };
    let before = cx.update(|window, _| painted_surfaces(window));
    let position = cx
        .debug_bounds("file-manager-row-left-target")
        .unwrap()
        .center();
    cx.simulate_event(FileDropEvent::Entered {
        position,
        paths: ExternalPaths(vec![input].into()),
    });
    draw_file_manager(cx);
    cx.simulate_event(FileDropEvent::Submit { position });
    draw_file_manager(cx);
    cx.read(|cx| {
        let app = app.read(cx);
        assert_eq!(
            app.shell.status(),
            rust_i18n::t!("fileExplorer.transferSourceUnavailable").to_string(),
        );
        assert!(app.transfer.transfer_jobs().is_empty());
    });
    // The component toast adds a painted popover surface above the unchanged panes.
    assert!(cx.update(|window, _| painted_surfaces(window)) > before);
}

#[gpui::test]
fn system_file_drop_on_a_directory_row_uses_its_local_identity_and_actual_destination(
    cx: &mut TestAppContext,
) {
    use gpui::{ExternalPaths, FileDropEvent};
    cx.executor().allow_parking();
    let root = TestTempDir::new("nyaterm-file-manager-external-drop");
    let directory = root.path().join("target");
    std::fs::create_dir_all(&directory).unwrap();
    let input = root.path().join("input.txt");
    std::fs::write(&input, b"external drop content").unwrap();
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
    workspace.update(cx, |state, _| {
        state.left.entries = std::sync::Arc::new(
            nyaterm_files::file_browser::FileBrowserService::local()
                .list_dir(root.path().to_string_lossy())
                .unwrap(),
        );
    });
    let fixture = cx.new(|_| FileManagerLayoutFixture {
        workspace: workspace.clone(),
        width: 900.,
    });
    let (_, cx) =
        cx.add_window_view(move |window, cx| nyaterm_ui::nya_root(fixture.clone(), window, cx));
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.shell
                .remember_main_window(window.window_handle().downcast().unwrap());
            app.start_transfer_event_drain(cx);
        });
    });
    draw_file_manager(cx);
    let position = cx
        .debug_bounds("file-manager-row-left-target")
        .unwrap()
        .center();
    cx.simulate_event(FileDropEvent::Entered {
        position,
        paths: ExternalPaths(vec![input.clone()].into()),
    });
    draw_file_manager(cx);
    cx.simulate_event(FileDropEvent::Submit { position });
    wait_for(cx, "external upload", |_| {
        directory.join("input.txt").exists()
    });
    cx.read(|cx| {
        let app = app.read(cx);
        let job = app
            .transfer
            .transfer_jobs()
            .iter()
            .find(|job| matches!(job.kind, crate::models::TransferJobKind::Upload { .. }))
            .unwrap();
        let crate::models::TransferJobKind::Upload { remote_path, .. } = &job.kind else {
            unreachable!()
        };
        assert_eq!(
            std::path::Path::new(remote_path),
            directory.join("input.txt")
        );
    });
    wait_for(cx, "transfer completion", |cx| {
        app.read(cx).transfer.transfer_jobs().iter().all(|job| {
            !matches!(
                job.status,
                crate::models::TransferJobStatus::Running
                    | crate::models::TransferJobStatus::Queued
            )
        })
    });
    // The file exists as soon as the upload starts writing it, so read it
    // only once the transfer is over
    assert_eq!(
        std::fs::read(directory.join("input.txt")).unwrap(),
        b"external drop content"
    );
}

#[gpui::test]
fn clipboard_internal_cut_to_local_runs_once_and_refreshes_the_target(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = TestTempDir::new("nyaterm-clipboard-local-move");
    std::fs::create_dir_all(root.join("source")).unwrap();
    std::fs::create_dir_all(root.join("target")).unwrap();
    std::fs::write(root.join("source/clipboard.txt"), b"clipboard content").unwrap();
    let app = app_with_visible_local_session(cx, root.path(), "clipboard-test");
    let target = RemoteFilePath::new(root.join("target").to_string_lossy());
    let entries = nyaterm_files::file_browser::FileBrowserService::local()
        .list_dir_path(&RemoteFilePath::new(root.join("source").to_string_lossy()))
        .unwrap();
    let cx = session_focus_test_window(cx, &app);
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.start_transfer_event_drain(cx);
            app.capture_file_clipboard(PaneEndpoint::Local, entries, true, cx);
            app.paste_file_clipboard(PaneEndpoint::Local, target.clone(), cx);
            app.paste_file_clipboard(PaneEndpoint::Local, target.clone(), cx);
            assert_eq!(
                app.transfer
                    .transfer_jobs()
                    .iter()
                    .filter(|job| app.transfer.file_operation(&job.id).is_some())
                    .count(),
                1
            );
        });
    });
    wait_for(cx, "clipboard move", |cx| {
        app.read(cx)
            .transfer
            .transfer_jobs()
            .iter()
            .any(|job| job.status == crate::models::TransferJobStatus::Completed)
    });
    assert!(!root.join("source/clipboard.txt").exists());
    assert_eq!(
        std::fs::read(root.join("target/clipboard.txt")).unwrap(),
        b"clipboard content"
    );
    cx.read(|cx| {
        let app = app.read(cx);
        assert!(app.transfer.file_clipboard().is_none());
        let job = app
            .transfer
            .transfer_jobs()
            .iter()
            .find(|job| app.transfer.file_operation(&job.id).is_some())
            .unwrap();
        assert_eq!(
            job.source_type,
            Some(nyaterm_files::sftp::SftpFileType::File)
        );
        assert_eq!(
            job.summary.as_ref().unwrap().local_path,
            root.join("target/clipboard.txt")
        );
    });
}

#[gpui::test]
fn replacing_file_clipboard_with_text_prevents_stale_file_paste(cx: &mut TestAppContext) {
    let root = TestTempDir::new("nyaterm-clipboard-replaced");
    let app = app_with_visible_local_session(cx, root.path(), "clipboard-test");
    std::fs::write(root.join("source.txt"), b"source").unwrap();
    let service = nyaterm_files::file_browser::FileBrowserService::local();
    let path = RemoteFilePath::new(root.path().to_string_lossy());
    let entries = service
        .list_dir_path(&path)
        .unwrap()
        .into_iter()
        .filter(|entry| entry.name == "source.txt")
        .collect();
    let cx = session_focus_test_window(cx, &app);
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.capture_file_clipboard(PaneEndpoint::Local, entries, false, cx);
            assert!(
                app.transfer.file_clipboard().is_some(),
                "capture must populate internal clipboard"
            );
            assert!(app.can_paste_transfer_file_clipboard(cx));
            cx.write_to_clipboard(gpui::ClipboardItem::new_string("ordinary text".into()));
            assert!(!app.can_paste_transfer_file_clipboard(cx));
            app.paste_file_clipboard(PaneEndpoint::Local, path, cx);
            assert!(
                app.transfer
                    .transfer_jobs()
                    .iter()
                    .all(|job| app.transfer.file_operation(&job.id).is_none())
            );
        });
    });
    assert_eq!(std::fs::read(root.join("source.txt")).unwrap(), b"source");
}

#[gpui::test]
fn path_completion_fills_without_navigating_then_enters_and_escapes_in_two_steps(
    cx: &mut TestAppContext,
) {
    use nyaterm_file_browser_gpui::completion::CompletionStatus;
    fn wait_for_completion(
        cx: &mut VisualTestContext,
        description: &str,
        condition: impl Fn(&App) -> bool,
    ) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            cx.run_until_parked();
            cx.executor().advance_clock(Duration::from_millis(20));
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
    cx.executor().allow_parking();
    let root = TestTempDir::new("nyaterm-path-completion");
    std::fs::create_dir_all(root.join("alpha").join("child")).unwrap();
    std::fs::create_dir_all(root.join("alpine")).unwrap();
    cx.update(|cx| {
        nyaterm_ui::apply_component_theme(
            crate::theme::theme_palette("dark"),
            gpui::font("Segoe UI"),
            gpui::px(14.),
            cx,
        )
    });
    // Match startup: component bindings must exist before the app captures its
    // shortcut baseline; later settings saves rebuild that baseline.
    let app = app_with_visible_local_session(cx, root.path(), "path-completion-test");
    let original = root.path().to_string_lossy().into_owned();
    let workspace = workspace(cx, &app, &original);
    let fixture = cx.new(|_| FileManagerLayoutFixture {
        workspace: workspace.clone(),
        width: 900.,
    });
    let (_, cx) =
        cx.add_window_view(move |window, cx| nyaterm_ui::nya_root(fixture.clone(), window, cx));
    draw_file_manager(cx);
    cx.update(|window, cx| {
        workspace.update(cx, |state, cx| {
            state.begin_path_edit(PaneId::Right, window, cx)
        })
    });
    draw_file_manager(cx);
    cx.simulate_input("al");
    wait_for_completion(cx, "directory candidates", |cx| {
        workspace.read(cx).right.completion.presentation.status == CompletionStatus::Ready
    });
    cx.read(|cx| {
        let state = workspace.read(cx);
        assert_eq!(
            state
                .right
                .completion
                .presentation
                .candidates
                .iter()
                .map(|c| c.label.as_str())
                .collect::<Vec<_>>(),
            ["alpha", "alpine"]
        );
        assert!(state.right.completion.presentation.selected.is_none());
        assert!(!state.left.completion.presentation.visible());
        assert_eq!(state.right.path.display_path, original);
    });
    draw_file_manager(cx);
    cx.simulate_keystrokes("down tab");
    wait_for_completion(cx, "next directory candidates", |cx| {
        let state = workspace.read(cx);
        state.right.completion.presentation.status == CompletionStatus::Ready
            && state
                .right
                .completion
                .presentation
                .candidates
                .first()
                .is_some_and(|c| c.label == "child")
    });
    cx.read(|cx| {
        let state = workspace.read(cx);
        assert!(state.right.path_editing);
        assert_eq!(state.right.path.display_path, original);
        assert!(
            state
                .inputs(PaneId::Right)
                .path
                .read(cx)
                .value(cx)
                .ends_with(if cfg!(windows) { "alpha\\" } else { "alpha/" })
        );
    });
    draw_file_manager(cx);
    cx.simulate_keystrokes("escape");
    cx.read(|cx| {
        assert!(workspace.read(cx).right.path_editing);
        assert!(!workspace.read(cx).right.completion.presentation.visible());
    });
    draw_file_manager(cx);
    cx.simulate_keystrokes("enter");
    wait_for_listings(cx, &workspace);
    cx.read(|cx| {
        assert_eq!(
            workspace
                .read(cx)
                .right
                .path
                .display_path
                .trim_end_matches(['/', '\\']),
            root.join("alpha").to_str().unwrap()
        );
        assert!(!workspace.read(cx).right.path_editing);
    });
    draw_file_manager(cx);
    cx.update(|window, cx| {
        workspace.update(cx, |state, cx| {
            state.begin_path_edit(PaneId::Right, window, cx)
        })
    });
    draw_file_manager(cx);
    cx.simulate_keystrokes("backspace");
    cx.read(|cx| {
        assert_eq!(
            workspace
                .read(cx)
                .inputs(PaneId::Right)
                .path
                .read(cx)
                .value(cx),
            ""
        )
    });
    wait_for_completion(cx, "child completion", |cx| {
        workspace.read(cx).right.completion.presentation.status == CompletionStatus::Ready
    });
    draw_file_manager(cx);
    cx.read(|cx| {
        assert_eq!(
            workspace
                .read(cx)
                .right
                .completion
                .presentation
                .candidates
                .iter()
                .map(|c| c.label.as_str())
                .collect::<Vec<_>>(),
            ["child"]
        );
    });
    cx.update(|window, cx| {
        assert!(
            workspace
                .read(cx)
                .inputs(PaneId::Right)
                .path
                .read(cx)
                .component_focus_handle(cx)
                .is_focused(window)
        );
    });
    cx.update(|window, cx| {
        let field = workspace.read(cx).inputs(PaneId::Right).path.clone();
        assert!(!field.update(cx, |input, cx| input.is_composing(window, cx)));
        assert!(field.read(cx).selection_at_end(cx));
    });
    cx.simulate_keystrokes("down");
    cx.read(|cx| {
        assert_eq!(
            workspace
                .read(cx)
                .right
                .completion
                .selected_path()
                .unwrap()
                .display_path,
            root.join("alpha").join("child").to_str().unwrap()
        );
    });
    cx.simulate_keystrokes("enter");
    wait_for_listings(cx, &workspace);
    cx.read(|cx| {
        assert!(!workspace.read(cx).right.path_editing);
        assert_eq!(
            workspace.read(cx).right.path.display_path,
            root.join("alpha").join("child").to_str().unwrap()
        )
    });
}
