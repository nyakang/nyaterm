use std::{cell::RefCell, rc::Rc};

use gpui::{
    AppContext as _, Context, Entity, FocusHandle, Focusable, IntoElement, MouseButton,
    MouseDownEvent, ParentElement as _, Render, ScrollStrategy, SharedString, Styled as _,
    Subscription, UniformListScrollHandle, WeakEntity, Window, div, prelude::*, px, rgb,
    uniform_list,
};

use nyaterm_core::NoteNodeKind;
use nyaterm_ui::{
    NyaButton, NyaButtonVariant, NyaContextMenu, NyaDropdownMenu, NyaInputEvent, NyaInputState,
    NyaMenuItem, NyaScrollable as _, NyaSearchInput,
};
use rust_i18n::t;

use crate::features::{NyaTermApp, view_widgets::mono_icon};

use super::NoteTreeRow;

use nyaterm_notes_gpui::tree::NotesDragPayload;
pub(in crate::features) struct NotesPanel {
    app: WeakEntity<NyaTermApp>,
    search: Entity<NyaInputState>,
    rename: Entity<NyaInputState>,
    renaming_id: Option<String>,
    context_target: Rc<RefCell<Option<String>>>,
    scroll: UniformListScrollHandle,
    focus: FocusHandle,
    #[cfg(test)]
    rows_built: usize,
    _app_subscription: Subscription,
    _search_subscription: Subscription,
    _rename_subscription: Subscription,
}

impl NotesPanel {
    pub(in crate::features) fn new(app: WeakEntity<NyaTermApp>, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| {
            NyaInputState::new(cx, String::new()).placeholder(t!("notes.searchPlaceholder"))
        });
        let rename = cx.new(|cx| NyaInputState::new(cx, String::new()).max_chars(Some(120)));
        let app_subscription = cx.observe(
            &app.upgrade().expect("Notes panel is created with its app"),
            |_, _, cx| cx.notify(),
        );
        let search_subscription = cx.subscribe(&search, |_, _, _: &NyaInputEvent, cx| cx.notify());
        let rename_subscription = cx.subscribe(&rename, |this, _, event: &NyaInputEvent, cx| {
            if let NyaInputEvent::Submitted(value) | NyaInputEvent::Blurred(value) = event {
                let value = value.clone();
                let Some(id) = this.renaming_id.take() else {
                    return;
                };
                this.with_app(cx, |app, cx| app.rename_note_node(id, value, cx));
                cx.notify();
            }
        });
        Self {
            app,
            search,
            rename,
            renaming_id: None,
            context_target: Rc::new(RefCell::new(None)),
            scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle(),
            #[cfg(test)]
            rows_built: 0,
            _app_subscription: app_subscription,
            _search_subscription: search_subscription,
            _rename_subscription: rename_subscription,
        }
    }

    fn with_app(
        &self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut NyaTermApp, &mut Context<NyaTermApp>),
    ) {
        if let Some(app) = self.app.upgrade() {
            app.update(cx, f);
        }
    }

    fn move_selection(&self, delta: isize, cx: &mut Context<Self>) {
        let search = self.search.read(cx).value(cx);
        let scroll = self.scroll.clone();
        self.with_app(cx, move |app, cx| {
            let rows = app.notes.visible_rows(&search);
            if rows.is_empty() {
                return;
            }
            let current = app
                .notes
                .selected_node_id()
                .and_then(|id| rows.iter().position(|row| row.id == id));
            let next = match current {
                Some(index) => (index as isize + delta).clamp(0, rows.len() as isize - 1) as usize,
                None if delta < 0 => rows.len() - 1,
                None => 0,
            };
            app.select_note_node(Some(rows[next].id.clone()), cx);
            scroll.scroll_to_item(next, ScrollStrategy::Nearest);
        });
    }

    fn activate_selected(&self, cx: &mut Context<Self>) {
        self.with_app(cx, |app, cx| {
            let Some(id) = app.notes.selected_node_id().map(str::to_string) else {
                return;
            };
            match app.notes.node(&id).map(|node| node.0) {
                Some(NoteNodeKind::Folder) => app.toggle_note_folder(&id, cx),
                Some(NoteNodeKind::Note) => app.open_note_editor(id, cx),
                None => {}
            }
        });
    }

    fn begin_rename(
        &mut self,
        id: String,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.renaming_id = Some(id);
        self.rename.update(cx, |field, cx| {
            field.set_content(&name, cx);
            field.select_all(window, cx);
        });
        cx.notify();
    }

    fn rename_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let selected = self.app.upgrade().and_then(|app| {
            app.read_with(cx, |app, _| {
                let id = app.notes.selected_node_id()?.to_string();
                let (_, _, name) = app.notes.node(&id)?;
                Some((id, name))
            })
        });
        if let Some((id, name)) = selected {
            self.begin_rename(id, name, window, cx);
        }
    }
}

impl Focusable for NotesPanel {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for NotesPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(app) = self.app.upgrade() else {
            return div().size_full().into_any_element();
        };
        let search = self.search.read(cx).value(cx);
        let (palette, panel_bg, toolbar_bg, rows, loading, error, selected, catalog_empty) = app
            .read_with(cx, |app, _| {
                let rows = app.notes.visible_rows(&search);
                let palette = app.theme_palette();
                (
                    palette,
                    app.shell_transparent_color(palette.surface),
                    app.shell_transparent_color(palette.section_header),
                    rows,
                    app.notes.loading(),
                    app.notes.error().map(str::to_string),
                    app.notes.selected_node_id().map(str::to_string),
                    app.notes.folders().is_empty() && app.notes.notes().is_empty(),
                )
            });
        let row_count = rows.len();
        let app_for_rows = self.app.clone();
        let selected_for_rows = selected.clone();
        let context_target_for_rows = self.context_target.clone();
        let list = uniform_list(
            "notes-tree-rows",
            row_count,
            cx.processor(move |panel, range: std::ops::Range<usize>, _, cx| {
                let items = range
                    .filter_map(|index| rows.get(index).cloned())
                    .map(|row| {
                        note_row(
                            panel,
                            row,
                            selected_for_rows.as_deref(),
                            palette,
                            app_for_rows.clone(),
                            context_target_for_rows.clone(),
                            cx,
                        )
                    })
                    .collect::<Vec<_>>();
                #[cfg(test)]
                {
                    panel.rows_built = panel.rows_built.saturating_add(items.len());
                }
                items
            }),
        )
        .flex_1()
        .min_h_0()
        .track_scroll(&self.scroll);

        let search_state = self.search.clone();
        let mut search_input = NyaSearchInput::new("notes-search", &search_state);
        if !search.is_empty() {
            let clear = self.search.clone();
            search_input = search_input.trailing(
                div()
                    .id("notes-clear-search")
                    .px_1()
                    .cursor_pointer()
                    .on_click(move |_, _, cx| clear.update(cx, |field, cx| field.clear(cx)))
                    .child("×"),
            );
        }
        let mut root = div()
            .id("notes-panel")
            .key_context("NotesPanel")
            .track_focus(&self.focus)
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(panel_bg)
            .on_key_down(
                cx.listener(|panel, event: &gpui::KeyDownEvent, window, cx| {
                    if panel.search.read(cx).has_focus() || panel.rename.read(cx).has_focus() {
                        return;
                    }
                    match event.keystroke.key.as_str() {
                        "up" => panel.move_selection(-1, cx),
                        "down" => panel.move_selection(1, cx),
                        "enter" => panel.activate_selected(cx),
                        "f2" => panel.rename_selected(window, cx),
                        "delete" => panel.with_app(cx, |app, cx| {
                            app.request_delete_selected_note_node(window, cx)
                        }),
                        _ => (),
                    }
                }),
            )
            .child(
                div()
                    .px_2()
                    .py(px(6.))
                    .bg(toolbar_bg)
                    .border_b_1()
                    .border_color(rgb(palette.border))
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(div().min_w_0().flex_1().h(px(28.)).child(search_input))
                    .child(toolbar_button(
                        "notes-new-folder",
                        "icons/fe/new-folder.svg",
                        t!("notes.newFolder"),
                        palette,
                        self.app.clone(),
                        |app, cx| app.create_note_folder_in_selected_folder(cx),
                    ))
                    .child(toolbar_button(
                        "notes-new-note",
                        "icons/conn/add.svg",
                        t!("notes.newNote"),
                        palette,
                        self.app.clone(),
                        |app, cx| app.create_note_in_selected_folder(cx),
                    ))
                    .child(notes_more_menu(self.app.clone())),
            );
        if loading && catalog_empty {
            root = root.child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(rgb(palette.text_muted))
                    .child(t!("common.loading").to_string()),
            );
        } else if let Some(error) = error {
            root = root.child(
                div()
                    .p_3()
                    .text_size(px(11.))
                    .text_color(rgb(palette.danger))
                    .child(error),
            );
        } else if catalog_empty {
            let new_note_app = self.app.clone();
            let new_folder_app = self.app.clone();
            root = root.child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .px_4()
                    .text_center()
                    .text_color(rgb(palette.text_muted))
                    .child(mono_icon(
                        "icons/file/description.svg",
                        rgb(palette.text_dimmed).into(),
                        24.,
                    ))
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(rgb(palette.text))
                            .child(t!("notes.emptyTitle").to_string()),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .child(t!("notes.emptyDescription").to_string()),
                    )
                    .child(
                        div()
                            .pt_1()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                NyaButton::new(
                                    "notes-empty-new-note",
                                    t!("notes.newNote").to_string(),
                                )
                                .small()
                                .variant(NyaButtonVariant::Primary)
                                .on_click(move |_, _, cx| {
                                    if let Some(app) = new_note_app.upgrade() {
                                        app.update(cx, |app, cx| app.create_root_note(cx));
                                    }
                                }),
                            )
                            .child(
                                NyaButton::new(
                                    "notes-empty-new-folder",
                                    t!("notes.newFolder").to_string(),
                                )
                                .small()
                                .on_click(move |_, _, cx| {
                                    if let Some(app) = new_folder_app.upgrade() {
                                        app.update(cx, |app, cx| app.create_root_note_folder(cx));
                                    }
                                }),
                            ),
                    ),
            );
        } else {
            let context_target_for_reset = self.context_target.clone();
            let context_target_for_menu = self.context_target.clone();
            let menu_app = self.app.clone();
            let menu_panel = cx.weak_entity();
            let tree = div()
                .flex_1()
                .min_h_0()
                .relative()
                .flex()
                .flex_col()
                .overflow_hidden()
                .p(px(6.))
                .capture_any_mouse_down(move |event: &MouseDownEvent, _, _| {
                    if event.button == MouseButton::Right {
                        *context_target_for_reset.borrow_mut() = None;
                    }
                })
                .on_drop(cx.listener(|panel, payload: &NotesDragPayload, _, cx| {
                    cx.stop_propagation();
                    let id = payload.id.clone();
                    panel.with_app(cx, |app, cx| app.move_note_node_to(id, None, cx));
                }))
                .child(list)
                .vertical_scrollbar(&self.scroll);
            root = root.child(
                NyaContextMenu::new_dynamic(tree, move |_, cx| {
                    note_context_menu_items(
                        context_target_for_menu.borrow().clone(),
                        menu_app.clone(),
                        menu_panel.clone(),
                        cx,
                    )
                })
                .min_width(px(160.)),
            );
        }
        if window.focused(cx).is_none() {
            window.focus(&self.focus, cx);
        }
        root.into_any_element()
    }
}

fn notes_more_menu(app: WeakEntity<NyaTermApp>) -> NyaDropdownMenu {
    nyaterm_notes_gpui::menus::more_menu(Rc::new(move |action, window, cx| {
        dispatch_menu_action(&app, action, window, cx);
    }))
}

fn toolbar_button(
    id: &'static str,
    icon: &'static str,
    tooltip: impl Into<SharedString>,
    palette: crate::theme::ThemePalette,
    app: WeakEntity<NyaTermApp>,
    action: impl Fn(&mut NyaTermApp, &mut Context<NyaTermApp>) + 'static,
) -> gpui::AnyElement {
    let tooltip = tooltip.into();
    div()
        .id(id)
        .size(px(26.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_sm()
        .cursor_pointer()
        .hover(move |this| this.bg(rgb(palette.hover)))
        .tooltip(move |window, cx| nyaterm_ui::NyaTooltip::new(tooltip.clone()).build(window, cx))
        .on_click(move |_, _, cx| {
            if let Some(app) = app.upgrade() {
                app.update(cx, |app, cx| action(app, cx));
            }
        })
        .child(mono_icon(icon, rgb(palette.text_muted).into(), 14.))
        .into_any_element()
}

fn note_row(
    panel: &mut NotesPanel,
    row: NoteTreeRow,
    selected_id: Option<&str>,
    palette: crate::theme::ThemePalette,
    app: WeakEntity<NyaTermApp>,
    context_target: Rc<RefCell<Option<String>>>,
    _cx: &mut Context<NotesPanel>,
) -> gpui::AnyElement {
    use nyaterm_notes_gpui::tree::{CanMoveHandler, TreeAction, TreeActionHandler, TreeHandlers};
    let move_app = app.clone();
    let can_move: CanMoveHandler = Rc::new(move |id, parent, cx| {
        move_app
            .upgrade()
            .is_some_and(|app| app.read(cx).notes.can_move_to(id, parent))
    });
    let handler: TreeActionHandler = Rc::new(move |action, cx| {
        let _ = app.update(cx, |app, cx| match action {
            TreeAction::Select(id) => app.select_note_node(Some(id), cx),
            TreeAction::Toggle(id) => app.toggle_note_folder(&id, cx),
            TreeAction::Open(id) => app.open_note_editor(id, cx),
            TreeAction::Move(id, parent) => app.move_note_node_to(id, parent, cx),
        });
    });
    let rename = (panel.renaming_id.as_deref() == Some(row.id.as_str())).then_some(&panel.rename);
    nyaterm_notes_gpui::tree::note_row(
        row,
        selected_id,
        palette,
        rename,
        panel.focus.clone(),
        context_target,
        TreeHandlers {
            action: handler,
            can_move,
        },
    )
}

fn note_context_menu_items(
    target_id: Option<String>,
    app: WeakEntity<NyaTermApp>,
    panel: WeakEntity<NotesPanel>,
    cx: &mut gpui::App,
) -> Vec<NyaMenuItem> {
    use nyaterm_notes_gpui::menus::{NoteMenuTarget, note_folder_targets};
    let Some(app_entity) = app.upgrade() else {
        return Vec::new();
    };
    let target = if let Some(id) = target_id {
        let state = &app_entity.read(cx).notes;
        let Some((kind, _, name)) = state.node(&id) else {
            return Vec::new();
        };
        let folders = note_folder_targets(state.folders())
            .into_iter()
            .filter(|folder| state.can_move_to(&id, Some(&folder.id)))
            .collect();
        Some(NoteMenuTarget {
            id,
            kind,
            name,
            folders,
        })
    } else {
        None
    };
    let handler = Rc::new(move |action, window: &mut Window, cx: &mut gpui::App| {
        use nyaterm_notes_gpui::menus::MenuAction;
        if let MenuAction::Rename { id, name } = action {
            let _ = panel.update(cx, |panel, cx| panel.begin_rename(id, name, window, cx));
            return;
        }
        dispatch_menu_action(&app, action, window, cx);
    });
    nyaterm_notes_gpui::menus::context_menu_items(target, handler)
}

fn dispatch_menu_action(
    app: &WeakEntity<NyaTermApp>,
    action: nyaterm_notes_gpui::menus::MenuAction,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    use nyaterm_notes_gpui::menus::MenuAction;
    if let Some(app) = app.upgrade() {
        app.update(cx, |app, cx| match action {
            MenuAction::CreateNote(parent) => app.create_note_in_folder(parent, cx),
            MenuAction::CreateFolder(parent) => app.create_note_folder_in_folder(parent, cx),
            MenuAction::Export => app.prompt_export_notes(cx),
            MenuAction::Expand(expanded) => app.set_all_notes_expanded(expanded, cx),
            MenuAction::Refresh => app.refresh_notes(cx),
            MenuAction::Open(id) => app.open_note_editor(id, cx),
            MenuAction::Move { id, parent } => app.move_note_node_to(id, parent, cx),
            MenuAction::Delete(id) => app.request_delete_note_node(id, window, cx),
            MenuAction::Rename { .. } => {}
        });
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, path::Path, time::Duration};

    use gpui::{
        AppContext as _, Entity, IntoElement, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent,
        ParentElement as _, Render, Styled as _, TestAppContext, VisualTestContext, div, point, px,
    };
    use nyaterm_core::{
        AppRuntime, NoteFolder, NoteSummary, NoteTreePayload, NotesUiState, RuntimeMode,
    };

    use crate::entities::{OverlayStore, StartupRestoreStore, UiStoreHandles};
    use crate::features::NyaTermApp;
    use crate::test_support::TestConfigDir;
    use nyaterm_ui::NyaDialogWindowExt as _;

    use super::note_context_menu_items;

    struct NotesHost {
        app: Entity<NyaTermApp>,
    }

    impl Render for NotesHost {
        fn render(
            &mut self,
            _window: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let panel = self.app.read(cx).notes_panel.clone();
            div().w(px(360.)).h(px(600.)).child(panel)
        }
    }

    fn test_app(cx: &mut TestAppContext, root: &Path) -> Entity<NyaTermApp> {
        let runtime = AppRuntime::from_parts_for_test(
            RuntimeMode::Portable,
            root.to_path_buf(),
            root.join("config"),
            root.join("logs"),
            root.join("cache"),
            None,
        );
        let stores = UiStoreHandles {
            startup_restore: cx.new(|_| StartupRestoreStore::default()),
            overlays: cx.new(|_| OverlayStore::default()),
        };
        cx.new(|cx| NyaTermApp::new(runtime, stores, cx))
    }

    fn draw(app: &Entity<NyaTermApp>, cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| {
            app.update(cx, |_, cx| cx.notify());
            _ = window.draw(cx);
        });
        cx.run_until_parked();
    }

    fn right_click(cx: &mut VisualTestContext, position: gpui::Point<gpui::Pixels>) {
        cx.simulate_event(MouseDownEvent {
            button: MouseButton::Right,
            position,
            modifiers: Modifiers::default(),
            click_count: 1,
            first_mouse: false,
        });
        cx.simulate_event(MouseUpEvent {
            button: MouseButton::Right,
            position,
            modifiers: Modifiers::default(),
            click_count: 1,
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            _ = window.draw(cx);
        });
        cx.run_until_parked();
    }

    fn assert_dialog_closed(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| {
            _ = window.draw(cx);
        });
        cx.executor().advance_clock(Duration::from_millis(400));
        cx.run_until_parked();
        cx.update(|window, cx| {
            _ = window.draw(cx);
            assert!(!window.has_active_nya_dialog(cx));
        });
    }

    fn open_note_delete_dialog(cx: &mut VisualTestContext) {
        let row = cx
            .debug_bounds("notes-row-note-1")
            .expect("note row should render");
        right_click(cx, row.center());
        cx.simulate_keystrokes("down down down down enter");
        cx.run_until_parked();
        cx.update(|window, cx| {
            _ = window.draw(cx);
            assert!(window.has_active_nya_dialog(cx));
        });
    }

    #[test]
    fn context_menu_items_match_blank_folder_and_note_targets() {
        let test_dir = TestConfigDir::new("nyaterm-notes-panel");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = test_app(&mut cx, test_dir.path());
        cx.update_entity(&app, |app, _| {
            let generation = app.notes.begin_load().expect("begin notes load");
            assert!(app.notes.apply_load(
                generation,
                NoteTreePayload {
                    folders: vec![
                        NoteFolder {
                            id: "root".into(),
                            parent_id: None,
                            name: "Root".into(),
                            sort_order: 0,
                            created_at_ms: 1,
                            updated_at_ms: 1,
                            extra: BTreeMap::new(),
                        },
                        NoteFolder {
                            id: "child".into(),
                            parent_id: Some("root".into()),
                            name: "Child".into(),
                            sort_order: 0,
                            created_at_ms: 1,
                            updated_at_ms: 1,
                            extra: BTreeMap::new(),
                        },
                        NoteFolder {
                            id: "sibling".into(),
                            parent_id: None,
                            name: "Sibling".into(),
                            sort_order: 1,
                            created_at_ms: 1,
                            updated_at_ms: 1,
                            extra: BTreeMap::new(),
                        },
                    ],
                    notes: vec![NoteSummary {
                        id: "note".into(),
                        parent_id: Some("root".into()),
                        title: "Note".into(),
                        sort_order: 1,
                        revision: 1,
                        created_at_ms: 1,
                        updated_at_ms: 1,
                        extra: BTreeMap::new(),
                    }],
                    ui: NotesUiState::default(),
                }
            ));
        });
        let panel = cx.read_entity(&app, |app, _| app.notes_panel.downgrade());
        let app_weak = app.downgrade();

        cx.update(|cx| {
            let blank = note_context_menu_items(None, app_weak.clone(), panel.clone(), cx);
            assert_eq!(blank.len(), 4);
            assert_eq!(
                blank
                    .iter()
                    .map(|item| item.test_presentation().1)
                    .collect::<Vec<_>>(),
                vec![
                    Some("icons/conn/add.svg".into()),
                    Some("icons/fe/new-folder.svg".into()),
                    None,
                    Some("icons/fe/refresh.svg".into()),
                ]
            );

            let folder =
                note_context_menu_items(Some("root".into()), app_weak.clone(), panel.clone(), cx);
            assert_eq!(folder.len(), 5);
            assert_eq!(
                folder
                    .iter()
                    .map(|item| item.test_presentation().1)
                    .collect::<Vec<_>>(),
                vec![
                    Some("icons/conn/add.svg".into()),
                    Some("icons/fe/new-folder.svg".into()),
                    Some("icons/net/edit.svg".into()),
                    Some("icons/net/move.svg".into()),
                    Some("icons/net/delete.svg".into()),
                ]
            );
            let folder_move_targets = folder[3].children().expect("move submenu");
            assert_eq!(folder_move_targets.len(), 2);
            assert_eq!(folder_move_targets[1].test_label(), "Sibling");

            let note =
                note_context_menu_items(Some("note".into()), app_weak.clone(), panel.clone(), cx);
            assert_eq!(note.len(), 4);
            assert_eq!(
                note[0].test_presentation().1.as_deref(),
                Some("icons/menu/external.svg")
            );
            let note_move_targets = note[2].children().expect("move submenu");
            assert_eq!(note_move_targets[2].test_label(), "Child");
            assert_eq!(note_move_targets[2].test_submenu_layout().3, Some(px(10.)));
        });
    }

    #[test]
    fn loaded_tauri_note_tree_builds_visible_rows() {
        let test_dir = TestConfigDir::new("nyaterm-notes-panel");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = test_app(&mut cx, test_dir.path());
        cx.update_entity(&app, |app, cx| {
            app.sync_component_theme(cx);
            let generation = app.notes.begin_load().expect("begin notes load");
            assert!(app.notes.apply_load(
                generation,
                NoteTreePayload {
                    folders: vec![NoteFolder {
                        id: "folder-1".into(),
                        parent_id: None,
                        name: "NyaTerm".into(),
                        sort_order: 0,
                        created_at_ms: 1,
                        updated_at_ms: 1,
                        extra: BTreeMap::new(),
                    }],
                    notes: vec![NoteSummary {
                        id: "note-1".into(),
                        parent_id: Some("folder-1".into()),
                        title: "v1.1.19".into(),
                        sort_order: 0,
                        revision: 1,
                        created_at_ms: 1,
                        updated_at_ms: 1,
                        extra: BTreeMap::new(),
                    }],
                    ui: NotesUiState {
                        expanded_folder_ids: vec!["folder-1".into()],
                        last_selected_node_id: None,
                    },
                }
            ));
        });

        let host_app = app.clone();
        let (_, cx) = cx.add_window_view(move |_, _| NotesHost { app: host_app });
        let cx: &mut VisualTestContext = cx;
        draw(&app, cx);

        cx.update(|_, cx| {
            let rows_built = app.read(cx).notes_panel.read(cx).rows_built;
            assert!(
                rows_built >= 2,
                "the expanded Tauri folder and its child note must enter the paint pipeline"
            );
        });
    }

    #[test]
    fn note_delete_dialog_controls_remain_clickable_after_context_menu() {
        let test_dir = TestConfigDir::new("nyaterm-notes-panel");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = test_app(&mut cx, test_dir.path());
        cx.update_entity(&app, |app, cx| {
            app.sync_component_theme(cx);
            let generation = app.notes.begin_load().expect("begin notes load");
            assert!(app.notes.apply_load(
                generation,
                NoteTreePayload {
                    folders: Vec::new(),
                    notes: vec![NoteSummary {
                        id: "note-1".into(),
                        parent_id: None,
                        title: "v1.1.19".into(),
                        sort_order: 0,
                        revision: 1,
                        created_at_ms: 1,
                        updated_at_ms: 1,
                        extra: BTreeMap::new(),
                    }],
                    ui: NotesUiState::default(),
                }
            ));
        });

        let host_app = app.clone();
        let (_, cx) = cx.add_window_view(move |window, cx| {
            let host = cx.new(|_| NotesHost { app: host_app });
            nyaterm_ui::nya_root(host, window, cx)
        });
        let cx: &mut VisualTestContext = cx;
        draw(&app, cx);

        open_note_delete_dialog(cx);
        let cancel = cx
            .debug_bounds("nya-dialog-cancel-button")
            .expect("cancel button should render");
        cx.simulate_click(cancel.center(), Modifiers::default());
        assert_dialog_closed(cx);

        open_note_delete_dialog(cx);
        let content = cx
            .debug_bounds("notes-delete-dialog-content")
            .expect("dialog content should render");
        let title = cx
            .debug_bounds("notes-delete-dialog-title")
            .expect("dialog title should render");
        let card_left = content.origin.x - px(16.);
        let card_top = title.origin.y - px(17.);
        let close = point(card_left + px(420.) - px(18.), card_top + px(18.));
        cx.simulate_click(close, Modifiers::default());
        assert_dialog_closed(cx);

        open_note_delete_dialog(cx);
        let confirm = cx
            .debug_bounds("nya-dialog-action-button")
            .expect("confirm button should render");
        cx.simulate_click(confirm.center(), Modifiers::default());
        assert_dialog_closed(cx);
    }
}
