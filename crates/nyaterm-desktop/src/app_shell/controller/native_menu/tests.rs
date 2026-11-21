use gpui::{
    AppContext as _, Context, Entity, IntoElement, Menu, MenuItem, Render, TestAppContext, Window,
    div,
};
use nyaterm_core::{AppRuntime, RuntimeMode, WorkspaceId, uuid};
use rust_i18n::t;

use super::{NativeMenuProjection, NativeMenuState, selected_workspace};
use crate::entities::{OverlayStore, StartupRestoreStore, UiStoreHandles};
use crate::features::NyaTermApp;
use crate::features::layout::title_bar::definition::{
    TitleMenuCommand, TitleMenuDefinition, TitleMenuKind, UnavailableTitleMenuCommand,
};
use crate::models::{SmartSplitMode, TitleMenu};
use nyaterm_core::app_identity::AppFlavor;

struct EmptyWindow;
impl Render for EmptyWindow {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

fn fixture() -> (
    TestAppContext,
    Entity<NyaTermApp>,
    crate::test_support::TestAppCleanup,
) {
    let mut cx = TestAppContext::single();
    let cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let root = std::env::temp_dir().join(format!("nyaterm-native-menu-{}", uuid()));
    let runtime = AppRuntime::from_parts_for_test(
        RuntimeMode::Portable,
        root.clone(),
        root.join("config"),
        root.join("logs"),
        root.join("cache"),
        None,
    );
    let stores = UiStoreHandles {
        startup_restore: cx.new(|_| StartupRestoreStore::default()),
        overlays: cx.new(|_| OverlayStore::default()),
    };
    let app = cx.new(|cx| NyaTermApp::new(runtime, stores, cx));
    (cx, app, cleanup)
}

fn projection(cx: &TestAppContext, app: &Entity<NyaTermApp>) -> NativeMenuProjection {
    cx.read_entity(app, |app, cx| {
        NativeMenuProjection::new(
            Some(WorkspaceId::legacy()),
            true,
            true,
            false,
            0,
            Some(app),
            cx,
        )
    })
}

fn commands(items: &[TitleMenuDefinition]) -> Vec<TitleMenuCommand> {
    items
        .iter()
        .flat_map(|item| match &item.kind {
            TitleMenuKind::Action(Some(command)) => vec![command.clone()],
            TitleMenuKind::Submenu(items) => commands(items),
            _ => Vec::new(),
        })
        .collect()
}

fn find_item<'a>(items: &'a [MenuItem], command: &TitleMenuCommand) -> Option<&'a MenuItem> {
    let expected = command.native_action();
    items.iter().find_map(|item| match item {
        MenuItem::Action { action, .. } if action.partial_eq(expected.as_ref()) => Some(item),
        MenuItem::Action { action, .. }
            if action
                .as_any()
                .downcast_ref::<UnavailableTitleMenuCommand>()
                .is_some_and(|unavailable| unavailable.0 == *command) =>
        {
            Some(item)
        }
        MenuItem::Submenu(menu) => find_item(&menu.items, command),
        _ => None,
    })
}

fn native_item<'a>(menus: &'a [Menu], command: &TitleMenuCommand) -> &'a MenuItem {
    menus
        .iter()
        .find_map(|menu| find_item(&menu.items, command))
        .expect("native command")
}

#[test]
fn native_menu_contains_every_window_menu_command_and_existing_extra_commands() {
    let (cx, app, _test_app_cleanup) = fixture();
    let expected = cx.read_entity(&app, |app, cx| {
        [
            TitleMenu::File,
            TitleMenu::View,
            TitleMenu::Terminal,
            TitleMenu::Help,
        ]
        .into_iter()
        .flat_map(|menu| commands(&app.title_menu_definitions(menu, cx)))
        .collect::<Vec<_>>()
    });
    let menus = projection(&cx, &app).into_menus(AppFlavor::Stable);
    assert!(
        expected.len() > 50,
        "nested themes, languages and panels must be represented"
    );
    for command in expected {
        native_item(&menus, &command);
    }
    for command in [
        TitleMenuCommand::OpenSettings,
        TitleMenuCommand::ToggleLeftSidebar,
        TitleMenuCommand::ToggleRightSidebar,
        TitleMenuCommand::TerminalCopy,
        TitleMenuCommand::TerminalPaste,
        TitleMenuCommand::TerminalFind,
        TitleMenuCommand::TerminalSelectAll,
    ] {
        native_item(&menus, &command);
    }
    assert_eq!(
        menus
            .iter()
            .map(|menu| menu.name.as_ref())
            .collect::<Vec<_>>(),
        vec![
            "NyaTerm",
            &t!("menu.file"),
            &t!("menu.edit"),
            &t!("menu.view"),
            &t!("menu.terminal"),
            &t!("menu.help")
        ]
    );
    assert!(find_item(&menus[5].items, &TitleMenuCommand::About).is_none());
}

#[test]
fn native_preview_menu_uses_preview_identity_and_preserves_os_edit_actions() {
    let (cx, app, _test_app_cleanup) = fixture();
    let menus = projection(&cx, &app).into_menus(AppFlavor::Preview);
    assert_eq!(menus[0].name.as_ref(), "NyaTerm Preview");
    let MenuItem::Action { name, .. } = &menus[0].items[0] else {
        panic!("about action")
    };
    assert!(name.ends_with("NyaTerm Preview"));
    let edit = menus[2]
        .items
        .iter()
        .filter_map(|item| match item {
            MenuItem::Action {
                action, os_action, ..
            } => Some((action.name(), *os_action)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(edit.len(), 6);
    assert!(
        edit.iter().map(|(_, role)| *role).collect::<Vec<_>>()
            == vec![
                Some(gpui::OsAction::Undo),
                Some(gpui::OsAction::Redo),
                Some(gpui::OsAction::Cut),
                Some(gpui::OsAction::Copy),
                Some(gpui::OsAction::Paste),
                Some(gpui::OsAction::SelectAll),
            ]
    );
    assert_eq!(edit[3].0, gpui::Action::name(&nyaterm_ui::NyaCopy));
}

#[test]
fn secondary_windows_disable_workspace_commands_and_keep_application_commands_available() {
    let (cx, app, _test_app_cleanup) = fixture();
    let mut snapshot = projection(&cx, &app);
    snapshot.main_active = false;
    let menus = snapshot.into_menus(AppFlavor::Stable);
    for command in [
        TitleMenuCommand::ToggleBroadcast,
        TitleMenuCommand::TerminalPaste,
        TitleMenuCommand::RefitTerminals,
        TitleMenuCommand::TogglePadding,
        TitleMenuCommand::ToggleWrapMarkers,
        TitleMenuCommand::ToggleCommandBlocks,
    ] {
        assert!(native_item(&menus, &command).is_disabled());
    }
    for command in [
        TitleMenuCommand::About,
        TitleMenuCommand::OpenSettings,
        TitleMenuCommand::SetLanguage("en".into()),
    ] {
        assert!(!native_item(&menus, &command).is_disabled());
    }
    assert!(
        !menus[2].disabled,
        "Edit still targets the focused child input"
    );
}

#[test]
fn locked_and_unready_menu_projections_reject_business_commands() {
    let (cx, app, _test_app_cleanup) = fixture();
    let mut snapshot = projection(&cx, &app);
    snapshot.locked = true;
    assert!(snapshot.command_enabled(&TitleMenuCommand::About));
    for command in [
        TitleMenuCommand::NewWindow,
        TitleMenuCommand::ImportConfig,
        TitleMenuCommand::ToggleBroadcast,
        TitleMenuCommand::ToggleWrapMarkers,
        TitleMenuCommand::ToggleCommandBlocks,
    ] {
        assert!(!snapshot.command_enabled(&command));
    }
    let menus = snapshot.clone().into_menus(AppFlavor::Stable);
    assert!(menus[2].disabled);
    assert!(
        menus[0].items[1].is_disabled(),
        "Services is disabled while locked"
    );
    assert!(
        menus[2].items.iter().all(|item| !matches!(
            item,
            MenuItem::Action {
                os_action: Some(_),
                ..
            }
        )),
        "locked Edit entries must not reach Cocoa's responder selectors"
    );
    assert!(native_item(&menus, &TitleMenuCommand::NewSession).is_disabled());
    assert!(native_item(&menus, &TitleMenuCommand::ToggleWrapMarkers).is_disabled());
    assert!(!native_item(&menus, &TitleMenuCommand::About).is_disabled());
    snapshot.ready = false;
    assert!(!snapshot.command_enabled(&TitleMenuCommand::About));
    assert!(!snapshot.command_enabled(&TitleMenuCommand::OpenSettings));
}

#[test]
fn cocoa_validation_cannot_reenable_an_unavailable_command() {
    let (cx, app, _test_app_cleanup) = fixture();
    cx.update(|cx| {
        cx.on_action(|_: &TitleMenuCommand, _| {});
    });
    let menus = projection(&cx, &app).into_menus(AppFlavor::Stable);
    let MenuItem::Action { action, .. } = native_item(&menus, &TitleMenuCommand::Unsplit) else {
        panic!("unsplit action")
    };
    cx.update(|cx| {
        assert!(cx.is_action_available(&TitleMenuCommand::About));
        assert!(!cx.is_action_available(action.as_ref()));
    });
}

#[test]
fn unchanged_projections_coalesce_without_reinstalling_and_keymap_changes_refresh() {
    let (cx, app, _test_app_cleanup) = fixture();
    let snapshot = projection(&cx, &app);
    let mut state = NativeMenuState::default();
    assert!(state.begin_refresh());
    assert!(!state.begin_refresh());
    assert!(state.accept(&snapshot));
    assert!(state.begin_refresh());
    assert!(
        !state.accept(&projection(&cx, &app)),
        "unrelated notifications keep the same projection"
    );
    let mut changed = snapshot.clone();
    changed.keymap_revision += 1;
    assert!(state.accept(&changed));
    assert!(!state.accept(&changed));
    changed.workspace = Some(WorkspaceId::new());
    assert!(state.accept(&changed));
}

#[test]
fn active_main_window_wins_over_recent_and_closed_windows_are_never_selected() {
    let mut cx = TestAppContext::single();
    let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let first = cx.add_window(|_, _| EmptyWindow).into();
    let second = cx.add_window(|_, _| EmptyWindow).into();
    let child = cx.add_window(|_, _| EmptyWindow).into();
    let first_id = WorkspaceId::new();
    let second_id = WorkspaceId::new();
    let windows = [(first_id, first), (second_id, second)];
    assert_eq!(
        selected_workspace(Some(first_id), Some(second), windows),
        Some(second_id)
    );
    assert_eq!(
        selected_workspace(Some(first_id), Some(child), windows),
        Some(first_id)
    );
    assert_eq!(
        selected_workspace(Some(second_id), Some(child), [(first_id, first)]),
        None
    );
    assert_eq!(
        selected_workspace(Some(first_id), None, [(first_id, first)]),
        Some(first_id)
    );
}

#[test]
fn checks_follow_current_state_and_disabled_commands_are_revalidated() {
    let (mut cx, app, _test_app_cleanup) = fixture();
    let window = cx.add_window(|_, _| EmptyWindow);
    let before = projection(&cx, &app).into_menus(AppFlavor::Stable);
    assert!(native_item(&before, &TitleMenuCommand::Unsplit).is_disabled());
    assert!(
        native_item(&before, &TitleMenuCommand::SmartSplit(SmartSplitMode::Auto)).is_disabled()
    );
    assert!(!native_item(&before, &TitleMenuCommand::ToggleLineNumbers).is_checked());
    window
        .update(&mut cx, |_, window, cx| {
            app.update(cx, |app, cx| {
                app.execute_title_menu_command(&TitleMenuCommand::ToggleLineNumbers, window, cx)
            });
        })
        .unwrap();
    let after = projection(&cx, &app).into_menus(AppFlavor::Stable);
    assert!(native_item(&after, &TitleMenuCommand::ToggleLineNumbers).is_checked());
    assert!(!cx.read_entity(&app, |app, cx| {
        app.title_menu_command_enabled(&TitleMenuCommand::Unsplit, cx)
    }));
    assert!(
        !cx.read_entity(&app, |app, cx| app.title_menu_command_enabled(
            &TitleMenuCommand::SmartSplit(SmartSplitMode::Auto),
            cx
        ))
    );
}

#[test]
fn all_supported_locales_translate_native_system_menu_labels() {
    for locale in ["en", "zh-CN", "zh-TW", "ja", "ko", "fr"] {
        for key in [
            "menu.services",
            "menu.hide",
            "menu.hideOthers",
            "menu.showAll",
            "menu.undo",
            "menu.redo",
        ] {
            assert_ne!(t!(key, locale = locale).as_ref(), key, "{locale}: {key}");
        }
    }
}
