use std::time::{Duration, Instant};

use gpui::{
    AppContext as _, Context, Entity, IntoElement, Modifiers, MouseButton, MouseDownEvent,
    MouseUpEvent, Pixels, Point, Render, ScrollStrategy, TestAppContext, VisualTestContext, Window,
    div, point, prelude::*, px,
};
use nyaterm_core::{
    AppRuntime, AssetSortKey, AssetViewMode, Group, RuntimeMode, SavedConnection, SessionsConfig,
    test_support::TestTempDir,
};
use nyaterm_store::{StoreDomain, store_mutation};
use nyaterm_ui::{NyaDialogWindowExt as _, NyaInputEvent, nya_root};

use crate::entities::{OverlayStore, StartupRestoreStore, UiStoreHandles};
use crate::features::NyaTermApp;
use crate::features::assets::AssetBatchSaveSummary;

use super::{
    asset_action_left_in_decoration, asset_row_fills, contrast_ratio, relative_luminance,
    responsive_asset_card_columns,
};

struct AssetFixture {
    app: Entity<NyaTermApp>,
}

struct AssetBatchFormFixture {
    app: Entity<NyaTermApp>,
}

impl Render for AssetBatchFormFixture {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = self.app.update(cx, |app, cx| {
            app.asset_batch_edit_window_view((620., 700.), cx)
        });
        div().w(px(620.)).h(px(700.)).child(body)
    }
}

impl Render for AssetFixture {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = self.app.update(cx, |app, cx| app.asset_workspace_state(cx));
        div()
            .track_focus(self.app.read(cx).terminal.input_focus())
            .w(px(1000.))
            .max_w_full()
            .h(px(700.))
            .flex()
            .flex_col()
            .child(body)
    }
}

fn connection(id: &str) -> SavedConnection {
    // Invalid arguments fail before a PTY or network connection can be started.
    serde_json::from_value(serde_json::json!({
        "id": id, "name": id,
        "type": "local_terminal", "shell_args": "'",
    }))
    .unwrap()
}

fn app(cx: &mut TestAppContext, root: &TestTempDir, mode: AssetViewMode) -> Entity<NyaTermApp> {
    // Pointer assertions need stationary dialog bounds on every platform.
    cx.update(|cx| cx.set_reduce_motion(true));
    let runtime = AppRuntime::from_parts_for_test(
        RuntimeMode::Portable,
        root.path().to_path_buf(),
        root.path().join("config"),
        root.path().join("logs"),
        root.path().join("cache"),
        None,
    );
    let stores = UiStoreHandles {
        startup_restore: cx.new(|_| StartupRestoreStore::default()),
        overlays: cx.new(|_| OverlayStore::default()),
    };
    let app = cx.new(|cx| NyaTermApp::new(runtime, stores, cx));
    app.update(cx, |app, cx| {
        app.sync_component_theme(cx);
        app.start_workspace.set_view_mode(mode);
        app.connection_state.replace_loaded(
            vec![
                connection("first"),
                connection("second"),
                connection("third"),
            ],
            vec![Group {
                id: "destination".into(),
                name: "Destination".into(),
                parent_id: None,
                sort_order: 0,
                created_at_ms: None,
                updated_at_ms: None,
            }],
        );
        let ids = vec!["first".into(), "second".into(), "third".into()];
        app.connection_state
            .select_list_connection("first".into(), &ids, false, false);
        app.connection_state
            .select_list_connection("second".into(), &ids, true, false);
    });
    let sessions = app.read_with(cx, |app, _| SessionsConfig {
        connections: app.connection_state.connections().to_vec(),
        groups: app.connection_state.groups().to_vec(),
        ..SessionsConfig::default()
    });
    let store = app.read_with(cx, |app, _| app.store_blocking_client());
    store
        .request(
            0,
            store_mutation(StoreDomain::Connections, move |store| {
                store.replace_sessions(&sessions)
            }),
        )
        .unwrap()
        .outcome
        .unwrap();
    app
}

fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| {
        _ = window.draw(cx);
    });
}

fn host<'a>(cx: &'a mut TestAppContext, app: &Entity<NyaTermApp>) -> &'a mut VisualTestContext {
    let app = app.clone();
    let (_, cx) = cx.add_window_view(move |window, cx| {
        cx.observe(&app, |_, _, cx| cx.notify()).detach();
        let fixture = cx.new(|_| AssetFixture { app });
        nya_root(fixture, window, cx)
    });
    draw(cx);
    cx
}

fn click(cx: &mut VisualTestContext, position: Point<Pixels>, button: MouseButton, count: usize) {
    cx.simulate_event(MouseDownEvent {
        button,
        position,
        modifiers: Modifiers::default(),
        click_count: count,
        first_mouse: false,
    });
    cx.simulate_event(MouseUpEvent {
        button,
        position,
        modifiers: Modifiers::default(),
        click_count: count,
    });
    draw(cx);
}

fn item_position(cx: &mut VisualTestContext, mode: AssetViewMode, id: &str) -> Point<Pixels> {
    let kind = if mode == AssetViewMode::List {
        "row"
    } else {
        "card"
    };
    let bounds = cx
        .debug_bounds(Box::leak(format!("asset-{kind}-{id}").into_boxed_str()))
        .unwrap();
    point(bounds.left() + px(80.), bounds.top() + px(30.))
}

fn wait_for(
    cx: &mut VisualTestContext,
    app: &Entity<NyaTermApp>,
    predicate: impl Fn(&NyaTermApp) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        draw(cx);
        if app.read_with(cx, |app, _| predicate(app)) {
            break;
        }
        assert!(Instant::now() < deadline, "asset operation did not finish");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn assets_connect_only_on_left_double_click_and_keep_editor_buttons_separate() {
    for mode in [AssetViewMode::List, AssetViewMode::Cards] {
        let root = TestTempDir::new("nyaterm-assets-double-click");
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx, &root, mode);
        let cx = host(&mut cx, &app);
        for count in [1, 2, 3] {
            app.update(cx, |app, _| app.shell.set_status("untouched"));
            let position = item_position(cx, mode, "first");
            click(cx, position, MouseButton::Left, count);
            app.read_with(cx, |app, _| {
                if count == 2 {
                    assert_eq!(
                        app.shell.status(),
                        rust_i18n::t!("dialog.shellArgsUnclosedQuote")
                    );
                } else {
                    assert_eq!(app.shell.status(), "untouched");
                }
            });
        }
        let position = item_position(cx, mode, "first");
        click(cx, position, MouseButton::Right, 1);
        cx.simulate_keystrokes("down enter");
        draw(cx);
        cx.update(|window, cx| {
            assert!(app.read(cx).terminal.input_focus().is_focused(window));
        });
        app.update(cx, |app, _| app.shell.set_status("untouched"));
        let edit = cx.debug_bounds("asset-edit-first").unwrap();
        if mode == AssetViewMode::Cards {
            let card = cx.debug_bounds("asset-card-first").unwrap();
            let footer = cx.debug_bounds("asset-card-footer-first").unwrap();
            let actions = cx.debug_bounds("asset-card-actions-first").unwrap();
            assert!(
                edit.left() >= card.left() && edit.right() <= card.right(),
                "card={card:?}, footer={footer:?}, actions={actions:?}, edit={edit:?}"
            );
        }
        click(cx, edit.center(), MouseButton::Left, 2);
        app.read_with(cx, |app, _| {
            assert_ne!(
                app.shell.status(),
                rust_i18n::t!("dialog.shellArgsUnclosedQuote")
            );
            assert_eq!(
                app.connection_state
                    .active_editor_draft()
                    .unwrap()
                    .id
                    .as_deref(),
                Some("first")
            );
            assert_eq!(app.session.session_order_len(), 0);
        });
    }
}

#[test]
fn asset_context_targets_follow_rows_fixed_actions_sort_and_search_and_reset_on_blank() {
    for mode in [AssetViewMode::List, AssetViewMode::Cards] {
        let root = TestTempDir::new("nyaterm-assets-context-target");
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx, &root, mode);
        let cx = host(&mut cx, &app);
        for id in ["first", "second", "third"] {
            let position = item_position(cx, mode, id);
            click(cx, position, MouseButton::Right, 1);
            app.read_with(cx, |app, _| {
                assert_eq!(app.start_workspace.context_connection_id(), Some(id));
                assert_eq!(app.connection_state.selected_connections().len(), 2);
            });
            cx.simulate_keystrokes("escape");
            draw(cx);
        }
        if mode == AssetViewMode::List {
            let actions = cx.debug_bounds("asset-actions-second").unwrap();
            click(cx, actions.center(), MouseButton::Right, 1);
            app.read_with(cx, |app, _| {
                assert_eq!(app.start_workspace.context_connection_id(), Some("second"))
            });
            cx.simulate_keystrokes("escape");
        } else {
            cx.simulate_resize(gpui::size(px(640.), px(768.)));
            draw(cx);
            app.read_with(cx, |app, _| {
                assert_eq!(app.start_workspace.card_columns(), 2)
            });
        }
        app.update(cx, |app, cx| {
            app.start_workspace.cycle_sort(AssetSortKey::Name);
            app.start_workspace.cycle_sort(AssetSortKey::Name);
            app.start_workspace.search_field().update(cx, |_, cx| {
                cx.emit(NyaInputEvent::Changed("second".into()));
            });
            cx.notify();
        });
        draw(cx);
        let position = item_position(cx, mode, "second");
        click(cx, position, MouseButton::Right, 1);
        cx.simulate_keystrokes("down down enter");
        draw(cx);
        app.read_with(cx, |app, _| {
            assert_eq!(
                app.connection_state
                    .active_editor_draft()
                    .unwrap()
                    .id
                    .as_deref(),
                Some("second")
            );
        });
        app.update(cx, |app, cx| app.close_connection_editor(cx));
        draw(cx);
        let selector = if mode == AssetViewMode::List {
            "asset-table"
        } else {
            "asset-cards"
        };
        let bounds = cx.debug_bounds(selector).unwrap();
        click(
            cx,
            point(bounds.left() + px(20.), bounds.bottom() - px(25.)),
            MouseButton::Right,
            1,
        );
        app.read_with(cx, |app, _| {
            assert_eq!(app.start_workspace.context_connection_id(), None)
        });
        cx.simulate_keystrokes("down enter");
        draw(cx);
        app.read_with(cx, |app, _| {
            assert!(!app.connection_state.editor_has_draft())
        });
    }
}

#[test]
fn asset_copy_move_and_delete_ignore_connection_panel_multi_selection() {
    for mode in [AssetViewMode::List, AssetViewMode::Cards] {
        let root = TestTempDir::new("nyaterm-assets-single-target");
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx, &root, mode);
        let cx = host(&mut cx, &app);
        let position = item_position(cx, mode, "third");
        click(cx, position, MouseButton::Right, 1);
        cx.simulate_keystrokes("down down down down enter");
        wait_for(cx, &app, |app| {
            app.connection_state.connections().len() == 4
        });
        app.read_with(cx, |app, _| {
            assert_eq!(app.connection_state.selected_connections().len(), 2);
            let copied = app
                .connection_state
                .connections()
                .iter()
                .find(|item| !["first", "second", "third"].contains(&item.id.as_str()))
                .unwrap();
            assert!(copied.name.starts_with("third"));
        });
        let position = item_position(cx, mode, "third");
        click(cx, position, MouseButton::Right, 1);
        cx.simulate_keystrokes("down down down down down right down enter");
        wait_for(cx, &app, |app| {
            app.connection_state
                .connection_by_id("third")
                .unwrap()
                .group_id
                .as_deref()
                == Some("destination")
        });
        app.read_with(cx, |app, _| {
            assert!(
                app.connection_state
                    .connection_by_id("first")
                    .unwrap()
                    .group_id
                    .is_none()
            );
            assert!(
                app.connection_state
                    .connection_by_id("second")
                    .unwrap()
                    .group_id
                    .is_none()
            );
        });
        let position = item_position(cx, mode, "third");
        click(cx, position, MouseButton::Right, 1);
        cx.simulate_keystrokes("down down down down down down enter");
        draw(cx);
        cx.update(|window, cx| assert!(window.has_active_nya_dialog(cx)));
        cx.simulate_keystrokes("escape");
        draw(cx);
        cx.update(|window, cx| assert!(!window.has_active_nya_dialog(cx)));
        let position = item_position(cx, mode, "third");
        click(cx, position, MouseButton::Right, 1);
        cx.simulate_keystrokes("down down down down down down enter");
        draw(cx);
        // Mount the dialog, then paint its final layout before reading bounds.
        draw(cx);
        let confirm = cx.debug_bounds("nya-dialog-action-button").unwrap();
        cx.simulate_click(confirm.center(), Modifiers::default());
        draw(cx);
        cx.update(|window, cx| assert!(!window.has_active_nya_dialog(cx)));
        wait_for(cx, &app, |app| {
            app.connection_state.connection_by_id("third").is_none()
        });
        app.read_with(cx, |app, _| {
            assert!(app.connection_state.connection_by_id("first").is_some());
            assert!(app.connection_state.connection_by_id("second").is_some());
        });
    }
}

#[test]
fn asset_virtual_list_context_uses_the_current_scrolled_connection() {
    for mode in [AssetViewMode::List, AssetViewMode::Cards] {
        let root = TestTempDir::new("nyaterm-assets-scroll-target");
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx, &root, mode);
        app.update(&mut cx, |app, _| {
            let connections = (0..100)
                .map(|i| connection(&format!("host-{i:03}")))
                .collect();
            app.connection_state.replace_loaded(connections, Vec::new());
        });
        let cx = host(&mut cx, &app);
        app.update(cx, |app, cx| {
            let scroll = if mode == AssetViewMode::List {
                app.start_workspace.list_scroll()
            } else {
                app.start_workspace.card_scroll()
            };
            scroll.scroll_to_item(20, ScrollStrategy::Top);
            cx.notify();
        });
        draw(cx);
        let index = app.read_with(cx, |app, _| {
            if mode == AssetViewMode::List {
                20
            } else {
                20 * app.start_workspace.card_columns()
            }
        });
        let id = format!("host-{index:03}");
        let position = item_position(cx, mode, &id);
        click(cx, position, MouseButton::Right, 1);
        app.read_with(cx, |app, _| {
            assert_eq!(
                app.start_workspace.context_connection_id(),
                Some(id.as_str())
            )
        });
    }
}

#[test]
fn asset_menu_rechecks_a_connection_removed_after_opening() {
    for mode in [AssetViewMode::List, AssetViewMode::Cards] {
        let root = TestTempDir::new("nyaterm-assets-stale-menu");
        let mut cx = TestAppContext::single();
        let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app(&mut cx, &root, mode);
        let cx = host(&mut cx, &app);
        let position = item_position(cx, mode, "first");
        click(cx, position, MouseButton::Right, 1);
        app.update(cx, |app, _| {
            app.connection_state
                .replace_loaded(vec![connection("second")], Vec::new());
        });
        cx.simulate_keystrokes("down enter");
        draw(cx);
        app.read_with(cx, |app, _| {
            assert_eq!(app.shell.status(), "connection is no longer available");
            assert_eq!(app.session.session_order_len(), 0);
        });
    }
}

#[test]
fn asset_card_columns_follow_available_viewport_width() {
    assert_eq!(responsive_asset_card_columns(f32::NAN), 1);
    assert_eq!(responsive_asset_card_columns(631.), 1);
    assert_eq!(responsive_asset_card_columns(632.), 2);
    assert_eq!(responsive_asset_card_columns(939.), 2);
    assert_eq!(responsive_asset_card_columns(940.), 3);
    assert_eq!(responsive_asset_card_columns(1920.), 3);
}

#[test]
fn asset_actions_stay_fixed_at_the_viewport_right_edge() {
    let viewport_width = px(960.);

    for horizontal_scroll in [px(0.), px(-120.), px(-480.)] {
        let decoration_left = asset_action_left_in_decoration(viewport_width, horizontal_scroll);
        assert_eq!(
            horizontal_scroll + decoration_left,
            px(856.),
            "the 104px action column must stay pinned to the 960px viewport"
        );
    }
}

/// Every built-in dark theme must show a measurable zebra step above its base
/// row, and a hover fill one step above that stripe.
#[test]
fn asset_row_stripes_are_visible_on_every_dark_theme() {
    let mut failures = Vec::new();
    for id in nyaterm_ui::APPEARANCE_THEME_IDS {
        let palette = nyaterm_ui::theme_palette(id);
        if !palette.is_dark() {
            continue;
        }
        let fills = asset_row_fills(palette);
        let base = relative_luminance(palette.bg);
        let stripe = relative_luminance(fills.stripe);
        let hover = relative_luminance(fills.hover);
        if base >= stripe || stripe >= hover {
            failures.push(format!("{id}: luminance order base < stripe < hover"));
        }
        if contrast_ratio(palette.bg, fills.stripe) < 1.10 {
            failures.push(format!("{id}: base->stripe contrast below 1.10"));
        }
        if contrast_ratio(fills.stripe, fills.hover) < 1.10 {
            failures.push(format!("{id}: stripe->hover contrast below 1.10"));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

/// Light themes keep the established subtle tint, but it still has to read as
/// a stripe instead of collapsing into the white base.
#[test]
fn asset_row_stripes_stay_subtle_but_visible_on_every_light_theme() {
    let mut failures = Vec::new();
    for id in nyaterm_ui::APPEARANCE_THEME_IDS {
        let palette = nyaterm_ui::theme_palette(id);
        if palette.is_dark() {
            continue;
        }
        let fills = asset_row_fills(palette);
        if fills.stripe != palette.surface {
            failures.push(format!("{id}: light stripe must stay on the surface token"));
        }
        // Some light palettes raise `surface` above `bg` (windterm-light), so the
        // stripe may be lighter or darker; it only has to be a distinct step.
        if (relative_luminance(fills.stripe) - relative_luminance(palette.bg)).abs() <= f32::EPSILON
        {
            failures.push(format!("{id}: light stripe must differ from the base"));
        }
        if contrast_ratio(palette.bg, fills.stripe) < 1.05 {
            failures.push(format!("{id}: light stripe is indistinguishable"));
        }
        if contrast_ratio(fills.stripe, fills.hover) < 1.05 {
            failures.push(format!(
                "{id}: light hover is indistinguishable from the stripe"
            ));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

fn ssh_connection(id: &str) -> SavedConnection {
    serde_json::from_value(serde_json::json!({
        "id": id, "name": id, "type": "ssh", "host": "example.com", "username": "root",
    }))
    .unwrap()
}

fn rdp_connection(id: &str) -> SavedConnection {
    serde_json::from_value(serde_json::json!({
        "id": id, "name": id, "type": "rdp", "host": "example.invalid",
    }))
    .unwrap()
}

/// Three SSH connections, one local terminal, and two RDP connections, all
/// selected, so every protocol in the selection has a distinct count.
fn mixed_protocol_app(cx: &mut TestAppContext, root: &TestTempDir) -> Entity<NyaTermApp> {
    let app = app(cx, root, AssetViewMode::List);
    app.update(cx, |app, _| {
        let connections = vec![
            ssh_connection("ssh-1"),
            ssh_connection("ssh-2"),
            ssh_connection("ssh-3"),
            connection("local-1"),
            rdp_connection("rdp-1"),
            rdp_connection("rdp-2"),
        ];
        let ids: Vec<String> = connections.iter().map(|record| record.id.clone()).collect();
        app.connection_state.replace_loaded(connections, Vec::new());
        app.start_workspace.clear_asset_selection();
        for id in &ids {
            app.start_workspace.toggle_asset_selection(id);
        }
    });
    app
}

#[test]
fn asset_batch_edit_opens_mixed_selection_in_common_scope_without_confirmation() {
    let root = TestTempDir::new("nyaterm-assets-batch-mixed-protocols");
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app = mixed_protocol_app(&mut cx, &root);
    let cx = host(&mut cx, &app);
    let selected = app.read_with(cx, |app, _| app.start_workspace.selected_asset_ids());
    cx.update(|window, cx| app.update(cx, |app, cx| app.open_asset_batch_edit(window, cx)));
    draw(cx);
    cx.update(|window, cx| assert!(!window.has_active_nya_dialog(cx)));
    app.read_with(cx, |app, _| {
        assert!(app.start_workspace.asset_batch_is_open());
        assert_eq!(app.start_workspace.asset_batch().display_kind(), None);
        assert_eq!(app.start_workspace.selected_asset_ids(), selected);
    });
}

#[test]
fn asset_batch_edit_opens_directly_when_the_selection_has_one_protocol() {
    let root = TestTempDir::new("nyaterm-assets-batch-single-protocol");
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app = app(&mut cx, &root, AssetViewMode::List);
    let cx = host(&mut cx, &app);
    app.update(cx, |app, _| {
        app.start_workspace.clear_asset_selection();
        app.start_workspace.toggle_asset_selection("first");
        app.start_workspace.toggle_asset_selection("second");
    });
    cx.update(|window, cx| {
        app.update(cx, |app, cx| app.open_asset_batch_edit(window, cx));
    });
    draw(cx);
    cx.update(|window, cx| assert!(!window.has_active_nya_dialog(cx)));
    app.read_with(cx, |app, _| {
        assert!(app.start_workspace.asset_batch_is_open())
    });
}

#[test]
fn visible_selection_keeps_shift_anchor_and_preserves_hidden_items() {
    let root = TestTempDir::new("nyaterm-visible-selection");
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app = mixed_protocol_app(&mut cx, &root);
    let cx = host(&mut cx, &app);
    draw(cx);
    app.update(cx, |app, cx| {
        let records = app.start_workspace.visible_records().to_vec();
        let ids = records
            .iter()
            .map(|record| record.connection.id.clone())
            .collect::<Vec<_>>();
        app.start_workspace.select_asset_only(&ids[1]);
        app.start_workspace
            .select_asset_range(&records, &ids[4], false);
        assert_eq!(app.start_workspace.selected_asset_count(), 4);
        app.start_workspace
            .select_asset_range(&records, &ids[2], false);
        assert_eq!(app.start_workspace.selected_asset_count(), 2);
        assert!(app.start_workspace.is_asset_selected(&ids[1]));
        app.start_workspace
            .select_asset_range(&records, &ids[5], true);
        assert_eq!(app.start_workspace.selected_asset_count(), 5);
        let filtered = &records[3..];
        app.start_workspace.deselect_visible_assets(filtered);
        assert_eq!(app.start_workspace.selected_asset_count(), 2);
        assert_eq!(app.start_workspace.hidden_selected_count(filtered), 2);
        app.start_workspace
            .select_asset_range(filtered, &ids[4], false);
        assert_eq!(
            app.start_workspace.selected_asset_ids(),
            vec![ids[4].clone()]
        );
        app.start_workspace
            .select_asset_range(filtered, &ids[5], false);
        assert_eq!(app.start_workspace.selected_asset_count(), 2);
        app.start_workspace.clear_asset_selection();
        assert_eq!(app.start_workspace.hidden_selected_count(filtered), 0);
        app.start_workspace.select_asset_only(&ids[0]);
        app.start_workspace.toggle_asset_selection(&ids[1]);
        let connections = app
            .connection_state
            .connections()
            .iter()
            .filter(|connection| connection.id != ids[1])
            .cloned()
            .collect();
        app.connection_state.replace_loaded(connections, Vec::new());
        _ = app.asset_workspace_state(cx);
        assert_eq!(
            app.start_workspace.selected_asset_ids(),
            vec![ids[0].clone()]
        );
    });
}

#[test]
fn batch_text_events_update_apply_state_and_keep_removes_patch() {
    use crate::features::assets::BatchEditMode;
    use crate::features::assets::BatchFieldId;
    let root = TestTempDir::new("nyaterm-batch-live-input");
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app = mixed_protocol_app(&mut cx, &root);
    let cx = host(&mut cx, &app);
    cx.update(|window, cx| app.update(cx, |app, cx| app.open_asset_batch_edit(window, cx)));
    app.update(cx, |app, cx| {
        app.set_asset_batch_mode(BatchFieldId::Description, BatchEditMode::Set, cx);
        assert!(app.start_workspace.asset_batch().build_edits().is_err());
        let input = app
            .existing_text_input(BatchFieldId::Description.text_input_id())
            .unwrap();
        input.update(cx, |_, cx| {
            cx.emit(NyaInputEvent::Changed("description".into()))
        });
    });
    cx.run_until_parked();
    app.read_with(cx, |app, _| {
        assert!(app.start_workspace.asset_batch().build_edits().is_ok())
    });
    app.update(cx, |app, cx| {
        app.set_asset_batch_mode(BatchFieldId::Description, BatchEditMode::Keep, cx);
        assert!(app.start_workspace.asset_batch().build_edits().is_err());
    });
    let password = app.update(cx, |app, cx| {
        app.set_asset_batch_scope(Some(nyaterm_core::ConnectionBatchKind::Ssh), cx);
        app.set_asset_batch_mode(BatchFieldId::Password, BatchEditMode::Set, cx);
        let input = app
            .existing_text_input(BatchFieldId::Password.text_input_id())
            .unwrap();
        input.update(cx, |state, cx| {
            // This offscreen field has not mounted its component yet. Queue its
            // content and send the user-edit event explicitly, as for description.
            state.set_content("  replacement  ", cx);
            cx.emit(NyaInputEvent::Changed("  replacement  ".into()));
        });
        input
    });
    cx.run_until_parked();
    app.read_with(cx, |app, _| {
        assert_eq!(
            app.start_workspace.asset_batch().replacement_password(),
            "  replacement  "
        )
    });
    app.update(cx, |app, cx| {
        app.set_asset_batch_mode(BatchFieldId::Password, BatchEditMode::Keep, cx)
    });
    password.read_with(cx, |input, cx| assert_eq!(input.value(cx), ""));
    app.read_with(cx, |app, _| {
        assert_eq!(app.start_workspace.asset_batch().replacement_password(), "")
    });
}

#[test]
fn stale_batch_completion_preserves_a_new_editor_and_refresh_failure_reports_saved() {
    let root = TestTempDir::new("nyaterm-batch-completion");
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app = app(&mut cx, &root, AssetViewMode::List);
    let cx = host(&mut cx, &app);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.start_workspace.select_asset_only("first");
            app.open_asset_batch_edit(window, cx);
            let old = app.start_workspace.asset_batch().generation();
            app.close_asset_batch_edit(cx);
            app.open_asset_batch_edit(window, cx);
            let new = app.start_workspace.asset_batch().generation();
            app.start_workspace.set_asset_batch_saving(true);
            app.finish_asset_batch_save(
                old,
                Err(nyaterm_store::StorageError::InvalidData("old request".into()).into()),
                cx,
            );
            assert!(app.start_workspace.asset_batch().matches_generation(new));
            assert!(app.start_workspace.asset_batch_saving());
            assert!(app.start_workspace.asset_batch().error().is_none());
            app.finish_asset_batch_save(
                old,
                Ok(AssetBatchSaveSummary {
                    updated_connections: 1,
                    ..Default::default()
                }),
                cx,
            );
            assert!(app.start_workspace.asset_batch().matches_generation(new));
            assert!(app.start_workspace.asset_batch_saving());
        })
    });
    draw(cx);
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.finish_asset_batch_refresh(
                AssetBatchSaveSummary {
                    updated_connections: 1,
                    ..Default::default()
                },
                Err(nyaterm_store::StorageError::InvalidData("refresh fixture".into()).into()),
                cx,
            );
            assert!(
                app.shell
                    .status()
                    .contains(rust_i18n::t!("assets.batch.refreshFailed").as_ref())
            );
            assert!(
                app.shell.status().contains(
                    rust_i18n::t!(
                        "assets.batch.saved",
                        updated = 1,
                        unchanged = 0,
                        skippedFields = 0
                    )
                    .as_ref()
                )
            );
            assert!(app.start_workspace.asset_batch_is_open());
            assert!(app.start_workspace.asset_batch_saving());
            assert!(app.start_workspace.asset_batch().error().is_none());
        })
    });
}

#[test]
fn narrow_batch_form_retains_zero_applicability_rows_and_enables_them_after_backend_change() {
    use crate::features::assets::{BatchEditMode, BatchFieldId};
    use nyaterm_core::models::connection_batch::ConnectionBatchKind;
    let root = TestTempDir::new("nyaterm-batch-rdp-controls");
    let mut cx = TestAppContext::single();
    let _cleanup = crate::test_support::TestAppCleanup::new(&cx);
    let app = app(&mut cx, &root, AssetViewMode::List);
    {
        let visual = host(&mut cx, &app);
        visual.update(|window, cx| {
            app.update(cx, |app, cx| {
                let connection: SavedConnection = serde_json::from_value(serde_json::json!({
                    "id": "rdp", "name": "RDP", "type": "rdp", "host": "example.test",
                    "backend": "mstsc", "display": { "mode": "native" }
                }))
                .unwrap();
                app.connection_state
                    .replace_loaded(vec![connection], Vec::new());
                app.start_workspace.select_asset_only("rdp");
                app.open_asset_batch_edit(window, cx);
                app.set_asset_batch_scope(Some(ConnectionBatchKind::Rdp), cx);
            });
        });
    }
    let form_app = app.clone();
    let (_, visual) = cx.add_window_view(move |window, cx| {
        cx.observe(&form_app, |_, _, cx| cx.notify()).detach();
        let fixture = cx.new(|_| AssetBatchFormFixture { app: form_app });
        nya_root(fixture, window, cx)
    });
    draw(visual);
    assert!(
        visual
            .debug_bounds("asset-batch-field-rdp-use-nla")
            .is_some()
    );
    let position = visual
        .debug_bounds("asset-batch-mode-rdp-use-nla-Set")
        .unwrap()
        .center();
    click(visual, position, MouseButton::Left, 1);
    app.read_with(visual, |app, _| {
        let draft = app.start_workspace.asset_batch();
        assert_eq!(draft.mode(BatchFieldId::RdpUseNla), BatchEditMode::Keep);
        let state = draft.rdp_applicability(BatchFieldId::RdpUseNla).unwrap();
        assert_eq!((state.applicable, state.total), (0, 1));
    });
    app.update(visual, |app, cx| {
        app.set_asset_batch_mode(BatchFieldId::RdpBackend, BatchEditMode::Set, cx);
        app.apply_asset_batch_select(&BatchFieldId::RdpBackend.select_id(), "builtin", cx);
    });
    draw(visual);
    let position = visual
        .debug_bounds("asset-batch-mode-rdp-use-nla-Set")
        .unwrap()
        .center();
    click(visual, position, MouseButton::Left, 1);
    app.read_with(visual, |app, _| {
        let draft = app.start_workspace.asset_batch();
        assert_eq!(draft.mode(BatchFieldId::RdpUseNla), BatchEditMode::Set);
        assert_eq!(
            draft
                .rdp_applicability(BatchFieldId::RdpUseNla)
                .unwrap()
                .applicable,
            1
        );
        assert!(draft.field_available(BatchFieldId::RdpWidth));
    });
}
