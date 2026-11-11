use gpui::{
    AppContext as _, Context, Entity, EntityInputHandler as _, IntoElement, KeyDownEvent,
    Keystroke, Modifiers, ParentElement as _, Render, Styled as _, TestAppContext,
    VisualTestContext, Window, div, px,
};
use nyaterm_core::{AppRuntime, QuickCommandCategory, QuickCommandsConfig, RuntimeMode};
use nyaterm_store::StoreDomain;
use nyaterm_ui::{NyaInputState, nya_root};

use crate::entities::{OverlayStore, StartupRestoreStore, UiStoreHandles};
use crate::features::NyaTermApp;
use crate::models::QuickCommandEditorState;
use crate::test_support::{TestAppCleanup, TestConfigDir};

struct EditorHost {
    app: Entity<NyaTermApp>,
    native: bool,
}

impl Render for EditorHost {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let editor = self.app.update(cx, |app, cx| {
            if self.native {
                app.quick_command_editor_window_view(560., cx)
            } else {
                app.quick_command_editor_overlay(cx)
            }
        });
        div().relative().w(px(560.)).h(px(800.)).child(editor)
    }
}

fn category(name: &str, order: i32) -> QuickCommandCategory {
    QuickCommandCategory {
        id: name.into(),
        name: name.into(),
        parent_id: None,
        sort_order: order,
    }
}

fn hosted_editor<'a>(
    cx: &'a mut TestAppContext,
    root: &TestConfigDir,
    native: bool,
    categories: Vec<QuickCommandCategory>,
) -> (Entity<NyaTermApp>, &'a mut VisualTestContext) {
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
        app.commands
            .replace_quick_command_catalog(Vec::new(), categories);
        let config = app.commands.quick_command_config();
        app.store_blocking_client()
            .request_mutation_fn(StoreDomain::Commands, move |store| {
                store.save_quick_commands(config)
            })
            .unwrap();
        let mut editor = QuickCommandEditorState::blank();
        // Valid fields make an accidental parent save observable in the database.
        editor.label = "Category keyboard regression".into();
        editor.command = "echo test".into();
        app.commands.open_quick_editor(editor);
    });
    let host_app = app.clone();
    let (_, vcx) = cx.add_window_view(move |window, cx| {
        cx.observe(&host_app, |_, _, cx| cx.notify()).detach();
        let host = cx.new(|_| EditorHost {
            app: host_app,
            native,
        });
        nya_root(host, window, cx)
    });
    draw(vcx);
    draw(vcx);
    (app, vcx)
}

fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| {
        _ = window.draw(cx);
    });
    cx.run_until_parked();
}

fn open_picker(app: &Entity<NyaTermApp>, cx: &mut VisualTestContext) -> Entity<NyaInputState> {
    draw(cx);
    let trigger = cx
        .debug_bounds("quick-command-editor-category-trigger")
        .unwrap();
    cx.simulate_click(trigger.center(), Modifiers::none());
    draw(cx);
    draw(cx);
    let input = app.read_with(cx, |app, _| {
        assert!(app.commands.quick_editor_category_picker_is_open());
        assert!(
            app.existing_text_input("quick-command.editor.new-category")
                .is_none()
        );
        app.existing_text_input("quick-command.editor.category")
            .unwrap()
    });
    cx.update(|window, cx| {
        assert!(input.read(cx).component_focus_handle(cx).is_focused(window));
    });
    input
}

fn search(cx: &mut VisualTestContext, text: &str) {
    cx.simulate_keystrokes("secondary-a");
    cx.simulate_input(text);
    draw(cx);
}

fn stored(app: &Entity<NyaTermApp>, cx: &mut VisualTestContext) -> QuickCommandsConfig {
    let store = app.read_with(cx, |app, _| app.store_blocking_client());
    store
        .request_fn(StoreDomain::Commands, |store| store.load_quick_commands())
        .unwrap()
}

fn assert_picker_closed_without_save(app: &Entity<NyaTermApp>, cx: &mut VisualTestContext) {
    draw(cx);
    app.read_with(cx, |app, _| {
        assert!(app.commands.quick_editor().is_some());
        assert!(!app.commands.quick_editor_category_picker_is_open());
        assert!(!app.commands.quick_editor_save_in_flight());
        assert_eq!(app.commands.quick_editor_category_search_draft(), "");
    });
    cx.update(|window, cx| {
        assert!(
            app.read(cx)
                .commands
                .quick_editor_category_trigger_focus()
                .is_focused(window)
        );
    });
    assert!(stored(app, cx).commands.is_empty());
}

#[test]
fn category_enter_selects_existing_and_empty_search_without_saving_in_both_surfaces() {
    for native in [false, true] {
        let root = TestConfigDir::new("nyaterm-category-enter-existing");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let (app, cx) = hosted_editor(&mut cx, &root, native, vec![category("TEST123", 0)]);
        open_picker(&app, cx);
        search(cx, "TEST1");
        assert!(
            cx.debug_bounds("quick-command-editor-category-create")
                .is_some()
        );
        cx.simulate_keystrokes("enter");
        assert_picker_closed_without_save(&app, cx);
        app.read_with(cx, |app, _| {
            let editor = app.commands.quick_editor().unwrap();
            assert_eq!(editor.category_id.as_deref(), Some("TEST123"));
            assert!(editor.category_draft.is_empty());
        });
        let input = open_picker(&app, cx);
        app.read_with(cx, |app, _| {
            assert_eq!(app.commands.quick_editor_category_search_draft(), "");
            assert_eq!(app.commands.quick_editor_category_highlight(), 1);
        });
        assert_eq!(input.read_with(cx, |input, cx| input.value(cx)), "");
        cx.simulate_keystrokes("up");
        app.read_with(cx, |app, _| {
            assert_eq!(app.commands.quick_editor_category_highlight(), 0)
        });
        cx.simulate_keystrokes("enter");
        assert_picker_closed_without_save(&app, cx);
        app.read_with(cx, |app, _| {
            assert_eq!(app.commands.quick_editor().unwrap().category_id, None)
        });
    }
}

#[test]
fn category_modified_enter_never_selects_creates_or_saves_in_both_surfaces() {
    for native in [false, true] {
        let root = TestConfigDir::new("nyaterm-category-modified-enter");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let (app, cx) = hosted_editor(&mut cx, &root, native, vec![category("TEST123", 0)]);
        open_picker(&app, cx);
        for query in ["NEW", "TEST123", "   "] {
            search(cx, query);
            cx.simulate_keystrokes("shift-enter ctrl-enter alt-enter cmd-enter");
            let mut key = Keystroke::parse("enter").unwrap();
            key.modifiers.function = true;
            cx.simulate_event(KeyDownEvent {
                keystroke: key,
                is_held: false,
                prefer_character_input: false,
            });
            draw(cx);
            app.read_with(cx, |app, _| {
                assert!(app.commands.quick_editor_category_picker_is_open());
                let editor = app.commands.quick_editor().unwrap();
                assert_eq!(editor.category_id, None);
                assert!(editor.category_draft.is_empty());
                assert!(editor.error.is_none());
            });
            assert!(stored(&app, cx).commands.is_empty());
        }
        cx.simulate_keystrokes("escape");
        assert_picker_closed_without_save(&app, cx);
    }
}

#[test]
fn category_navigation_and_click_share_creation_draft_and_escape_preserves_it() {
    for native in [false, true] {
        let root = TestConfigDir::new("nyaterm-category-create-draft");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let (app, cx) = hosted_editor(&mut cx, &root, native, vec![category("TEST123", 0)]);
        open_picker(&app, cx);
        search(cx, "TEST1");
        cx.simulate_keystrokes("up down down enter");
        assert_picker_closed_without_save(&app, cx);
        app.read_with(cx, |app, _| {
            assert_eq!(app.commands.quick_editor().unwrap().category_draft, "TEST1")
        });
        assert_eq!(stored(&app, cx).categories.len(), 1);
        open_picker(&app, cx);
        search(cx, "TEST123");
        cx.simulate_keystrokes("escape");
        assert_picker_closed_without_save(&app, cx);
        app.read_with(cx, |app, _| {
            assert_eq!(app.commands.quick_editor().unwrap().category_draft, "TEST1")
        });
        open_picker(&app, cx);
        search(cx, "Operations");
        let row = cx
            .debug_bounds("quick-command-editor-category-create")
            .unwrap();
        cx.simulate_click(row.center(), Modifiers::none());
        assert_picker_closed_without_save(&app, cx);
        app.read_with(cx, |app, _| {
            assert_eq!(
                app.commands.quick_editor().unwrap().category_draft,
                "Operations"
            )
        });
        app.update(cx, |app, cx| app.close_quick_command_editor(cx));
        draw(cx);
        assert_eq!(stored(&app, cx).categories.len(), 1);
        assert!(stored(&app, cx).commands.is_empty());
    }
}

#[test]
fn category_ime_confirmation_does_not_create_or_save_before_a_separate_enter() {
    for native in [false, true] {
        let root = TestConfigDir::new("nyaterm-category-ime");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let (app, cx) = hosted_editor(&mut cx, &root, native, Vec::new());
        let input = open_picker(&app, cx);
        search(cx, "draft");
        cx.update(|window, cx| {
            let component = input.read(cx).component_state().unwrap();
            component.update(cx, |state, cx| {
                state.replace_and_mark_text_in_range(
                    Some(0..5),
                    "中文分类",
                    Some(0..4),
                    window,
                    cx,
                );
            });
            assert!(input.update(cx, |state, cx| state.is_composing(window, cx)));
        });
        draw(cx);
        let row = cx
            .debug_bounds("quick-command-editor-category-create")
            .unwrap();
        cx.simulate_click(row.center(), Modifiers::none());
        draw(cx);
        app.read_with(cx, |app, _| {
            assert!(app.commands.quick_editor_category_picker_is_open());
            assert!(
                app.commands
                    .quick_editor()
                    .unwrap()
                    .category_draft
                    .is_empty()
            );
        });
        for key in ["down", "up"] {
            cx.simulate_keystrokes(key);
            cx.update(|window, cx| {
                assert!(
                    input.update(cx, |state, cx| state.is_composing(window, cx)),
                    "{key} must leave composition active"
                );
            });
        }
        cx.simulate_keystrokes("enter");
        draw(cx);
        app.read_with(cx, |app, _| {
            assert!(app.commands.quick_editor_category_picker_is_open());
            assert!(
                app.commands
                    .quick_editor()
                    .unwrap()
                    .category_draft
                    .is_empty()
            );
        });
        assert!(stored(&app, cx).commands.is_empty());
        app.read_with(cx, |app, _| {
            assert_eq!(
                app.commands.quick_editor_category_search_draft(),
                "中文分类"
            )
        });
        assert!(
            cx.debug_bounds("quick-command-editor-category-create")
                .is_some()
        );
        cx.update(|window, cx| {
            let component = input.read(cx).component_state().unwrap();
            component.update(cx, |state, cx| state.unmark_text(window, cx));
        });
        draw(cx);
        cx.simulate_keystrokes("enter");
        assert_picker_closed_without_save(&app, cx);
        app.read_with(cx, |app, _| {
            assert_eq!(
                app.commands.quick_editor().unwrap().category_draft,
                "中文分类"
            )
        });
        assert!(stored(&app, cx).categories.is_empty());
        let input = open_picker(&app, cx);
        cx.update(|window, cx| {
            let component = input.read(cx).component_state().unwrap();
            component.update(cx, |state, cx| {
                state.replace_and_mark_text_in_range(None, "候选", Some(0..2), window, cx);
            });
        });
        draw(cx);
        cx.simulate_keystrokes("escape");
        draw(cx);
        app.read_with(cx, |app, _| {
            assert!(app.commands.quick_editor_category_picker_is_open())
        });
        cx.simulate_keystrokes("escape");
        assert_picker_closed_without_save(&app, cx);
        app.read_with(cx, |app, _| {
            assert_eq!(
                app.commands.quick_editor().unwrap().category_draft,
                "中文分类"
            )
        });
    }
}

#[test]
fn category_keyboard_navigation_scrolls_and_mouse_hover_updates_the_highlight() {
    for native in [false, true] {
        let root = TestConfigDir::new("nyaterm-category-scroll");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let categories = (0..20)
            .map(|i| category(&format!("Category{i:02}"), i))
            .collect();
        let (app, cx) = hosted_editor(&mut cx, &root, native, categories);
        open_picker(&app, cx);
        search(cx, "Category");
        for _ in 0..20 {
            cx.simulate_keystrokes("down");
            draw(cx);
        }
        let viewport = cx
            .debug_bounds("quick-command-editor-category-list")
            .unwrap();
        let create = cx
            .debug_bounds("quick-command-editor-category-create")
            .unwrap();
        assert!(create.top() >= viewport.top() && create.bottom() <= viewport.bottom());
        app.read_with(cx, |app, _| {
            assert_eq!(app.commands.quick_editor_category_highlight(), 20);
            assert!(app.commands.quick_editor_category_scroll().offset().y < px(0.));
        });
        let row = cx
            .debug_bounds("quick-command-editor-category-Category19")
            .unwrap();
        cx.simulate_mouse_move(row.center(), None, Modifiers::none());
        draw(cx);
        app.read_with(cx, |app, _| {
            assert_eq!(app.commands.quick_editor_category_highlight(), 19)
        });
        cx.simulate_keystrokes("enter");
        assert_picker_closed_without_save(&app, cx);
        app.read_with(cx, |app, _| {
            assert_eq!(
                app.commands.quick_editor().unwrap().category_id.as_deref(),
                Some("Category19")
            )
        });
    }
}

#[test]
fn category_creation_persists_once_only_after_explicit_editor_save() {
    for native in [false, true] {
        let root = TestConfigDir::new("nyaterm-category-save");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let (app, cx) = hosted_editor(&mut cx, &root, native, Vec::new());
        open_picker(&app, cx);
        search(cx, "Operations");
        cx.simulate_keystrokes("enter");
        assert_picker_closed_without_save(&app, cx);
        assert!(stored(&app, cx).categories.is_empty());
        cx.simulate_keystrokes("secondary-s");
        draw(cx);
        let config = stored(&app, cx);
        draw(cx);
        assert_eq!(config.categories.len(), 1);
        assert_eq!(config.categories[0].name, "Operations");
        assert_eq!(config.commands.len(), 1);
        assert_eq!(
            config.commands[0].category_id.as_deref(),
            Some(config.categories[0].id.as_str())
        );
        app.read_with(cx, |app, _| assert!(app.commands.quick_editor().is_none()));
    }
}
