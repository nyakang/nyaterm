use rust_i18n::t;

use gpui::{AppContext as _, Context, FontWeight, SharedString, div, prelude::*, px, rgb, rgba};
use nyaterm_ui::{NyaContextMenu, NyaMenuItem, NyaScrollable};

use super::super::super::QuickCommandCategoryOption;
use crate::features::{
    NyaTermApp, commands::QuickCommandDropPosition, commands::QuickCommandDropTarget,
};

use super::{QuickCommandDragKind, QuickCommandDragPayload, QuickCommandDragPreview};

impl NyaTermApp {
    pub(super) fn quick_command_category_sidebar(
        &mut self,
        categories: Vec<QuickCommandCategoryOption>,
        palette: crate::theme::ThemePalette,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let insertion_marker = self
            .commands
            .quick_drop_target()
            .filter(|_| cx.has_active_drag())
            .and_then(|target| {
                let index = categories
                    .iter()
                    .position(|option| option.id == target.id)?;
                let depth = categories[index].depth;
                let anchor = match target.position {
                    QuickCommandDropPosition::Before => index,
                    // "After" means after the whole subtree, rather than a line
                    // between a parent and its first child.
                    QuickCommandDropPosition::After => categories
                        .iter()
                        .enumerate()
                        .skip(index + 1)
                        .take_while(|(_, option)| option.depth > depth)
                        .last()
                        .map_or(index, |(index, _)| index),
                    QuickCommandDropPosition::Inside => return None,
                };
                Some((categories[anchor].id.clone(), target.position, depth))
            });
        let mut category_sidebar = div()
            .id(SharedString::from("quick-command-category-scroll"))
            .w_full()
            .flex_1()
            .min_h_0()
            // Vertical only. Scrolling both axes lets the rows size to their
            // intrinsic width, which pushed every count pill past the 176px clip.
            .overflow_y_scrollbar()
            .p(px(6.))
            .flex()
            .flex_col()
            .gap_1();
        let mut all_categories = None;
        for option in categories {
            let id = option.id.clone();
            let drag_option_id = option.id.clone();
            let drag_option_label = option.label.clone();
            let selected = self.commands.quick_selected_category() == option.id;
            let manageable = option.manageable;
            let depth = option.depth;
            let drop_position = self
                .commands
                .quick_drop_target()
                .filter(|target| cx.has_active_drag() && target.id == option.id)
                .map(|target| target.position);
            let insertion = insertion_marker
                .as_ref()
                .filter(|(id, _, _)| *id == option.id)
                .map(|(_, position, depth)| (*position, *depth));
            // Real categories get the full menu; `all` / `uncategorized` get the two
            // add actions only, since there is nothing there to rename or delete.
            let menu_items = if manageable {
                self.quick_command_category_menu_items(option.id.clone(), cx)
            } else {
                self.quick_command_pseudo_category_menu_items(cx)
            };
            let row = div()
                .id(SharedString::from(format!(
                    "quick-command-category-{}",
                    option.id
                )))
                .relative()
                .debug_selector({
                    let id = option.id.clone();
                    move || format!("quick-command-category-{id}")
                })
                .h(px(32.))
                .flex_shrink_0()
                .w_full()
                .px_2()
                .flex()
                .items_center()
                .gap_2()
                .pl(px(8. + depth as f32 * 12.))
                .rounded_md()
                .bg(if selected {
                    rgb(palette.hover)
                } else {
                    rgba(0x00000000)
                })
                .text_xs()
                .text_color(if selected {
                    rgb(palette.link)
                } else {
                    rgb(palette.text)
                })
                .cursor_pointer()
                .hover(move |this| this.bg(rgb(palette.hover)))
                .when(
                    drop_position == Some(QuickCommandDropPosition::Inside),
                    |row| {
                        row.bg(rgba((palette.primary << 8) | 0x24))
                            .border_1()
                            .border_color(rgb(palette.link))
                    },
                )
                .when_some(insertion, |row, (position, depth)| {
                    let before = position == QuickCommandDropPosition::Before;
                    row.child(
                        div()
                            .debug_selector({
                                let id = option.id.clone();
                                move || format!("quick-command-category-insertion-{id}")
                            })
                            .absolute()
                            .left(px(8. + depth as f32 * 12.))
                            .right(px(0.))
                            .h(px(2.))
                            .bg(rgb(palette.link))
                            .when(before, |line| line.top(px(0.)))
                            .when(!before, |line| line.bottom(px(0.))),
                    )
                })
                .child(
                    div()
                        .size(px(6.))
                        .flex_none()
                        .rounded_full()
                        .when(!selected, |this| this.opacity(0.6))
                        .bg(if selected {
                            rgb(palette.link)
                        } else {
                            rgb(palette.text_dimmed)
                        }),
                )
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .truncate()
                        .font_weight(FontWeight(500.))
                        .child(option.label),
                )
                .child(
                    div()
                        .flex_none()
                        .rounded_sm()
                        .px(px(6.))
                        .py(px(2.))
                        .bg(if selected {
                            rgba((palette.primary << 8) | 0x24)
                        } else {
                            rgb(palette.hover)
                        })
                        .text_size(px(10.))
                        .line_height(px(10.))
                        .text_color(if selected {
                            rgb(palette.primary)
                        } else {
                            rgb(palette.text_dimmed)
                        })
                        .child(option.count.to_string()),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.select_quick_command_category(id.clone(), cx);
                }))
                .when(manageable, |this| {
                    let drag_id = drag_option_id.clone();
                    let drag_label = drag_option_label.clone();
                    let move_target = drag_option_id.clone();
                    let drop_target = drag_option_id.clone();
                    this.cursor_move()
                        .on_drag(
                            QuickCommandDragPayload {
                                kind: QuickCommandDragKind::Category,
                                id: drag_id,
                                label: drag_label,
                            },
                            |payload, position, _, cx| {
                                cx.new(|_| QuickCommandDragPreview {
                                    payload: payload.clone(),
                                    position,
                                })
                            },
                        )
                        .on_drag_move(cx.listener(
                            move |this,
                                  event: &gpui::DragMoveEvent<QuickCommandDragPayload>,
                                  _,
                                  cx| {
                                let payload = event.drag(cx);
                                if !event.bounds.contains(&event.event.position)
                                    || (payload.kind == QuickCommandDragKind::Category
                                        && !this
                                            .commands
                                            .can_move_quick_category(&payload.id, &move_target))
                                {
                                    if this
                                        .commands
                                        .quick_drop_target()
                                        .is_some_and(|target| target.id == move_target)
                                    {
                                        this.commands.clear_quick_drop_target();
                                        cx.notify();
                                    }
                                    return;
                                }
                                let relative = if event.bounds.size.height > px(0.) {
                                    ((event.event.position.y - event.bounds.origin.y)
                                        / event.bounds.size.height)
                                        .clamp(0., 1.)
                                } else {
                                    0.5
                                };
                                let position = if payload.kind == QuickCommandDragKind::Command {
                                    QuickCommandDropPosition::Inside
                                } else if relative < 0.25 {
                                    QuickCommandDropPosition::Before
                                } else if relative > 0.75 {
                                    QuickCommandDropPosition::After
                                } else {
                                    QuickCommandDropPosition::Inside
                                };
                                if this.commands.set_quick_drop_target(QuickCommandDropTarget {
                                    id: move_target.clone(),
                                    position,
                                }) {
                                    this.ensure_drop_hover_clock(cx);
                                    cx.notify();
                                }
                            },
                        ))
                        .on_drop(cx.listener(
                            move |this, payload: &QuickCommandDragPayload, _, cx| {
                                cx.stop_propagation();
                                let position = this
                                    .commands
                                    .quick_drop_target()
                                    .filter(|target| target.id == drop_target)
                                    .map(|target| target.position)
                                    .unwrap_or(QuickCommandDropPosition::Inside);
                                let config = match payload.kind {
                                    QuickCommandDragKind::Command => {
                                        this.commands.move_quick_command_to_category(
                                            &payload.id,
                                            Some(drop_target.clone()),
                                        )
                                    }
                                    QuickCommandDragKind::Category => this
                                        .commands
                                        .move_quick_category(&payload.id, &drop_target, position),
                                };
                                this.finish_quick_command_reorder(config, cx);
                            },
                        ))
                })
                .when(option.id == "all", |this| {
                    this.on_drag_move(cx.listener(
                        |this, event: &gpui::DragMoveEvent<QuickCommandDragPayload>, _, cx| {
                            let payload = event.drag(cx);
                            if payload.kind != QuickCommandDragKind::Category
                                || !event.bounds.contains(&event.event.position)
                            {
                                if this
                                    .commands
                                    .quick_drop_target()
                                    .is_some_and(|target| target.id == "all")
                                {
                                    this.commands.clear_quick_drop_target();
                                    cx.notify();
                                }
                                return;
                            }
                            if this.commands.set_quick_drop_target(QuickCommandDropTarget {
                                id: "all".to_string(),
                                position: QuickCommandDropPosition::Inside,
                            }) {
                                this.ensure_drop_hover_clock(cx);
                                cx.notify();
                            }
                        },
                    ))
                    .on_drop(cx.listener(
                        |this, payload: &QuickCommandDragPayload, _, cx| {
                            cx.stop_propagation();
                            let config = (payload.kind == QuickCommandDragKind::Category)
                                .then(|| this.commands.move_quick_category_to_root(&payload.id))
                                .flatten();
                            this.finish_quick_command_reorder(config, cx);
                        },
                    ))
                })
                .when(option.id == "uncategorized", |this| {
                    this.on_drop(cx.listener(
                        move |this, payload: &QuickCommandDragPayload, _, cx| {
                            cx.stop_propagation();
                            let config = (payload.kind == QuickCommandDragKind::Command)
                                .then(|| {
                                    this.commands
                                        .move_quick_command_to_category(&payload.id, None)
                                })
                                .flatten();
                            this.finish_quick_command_reorder(config, cx);
                        },
                    ))
                });
            let row = NyaContextMenu::new(row, menu_items).into_any_element();
            if option.id == "all" {
                // Keep the root drop target reachable when the category tree scrolls.
                all_categories = Some(row);
            } else {
                category_sidebar = category_sidebar.child(row);
            }
        }
        category_sidebar = category_sidebar.child(
            NyaContextMenu::new(
                div()
                    .id("quick-command-category-blank")
                    .debug_selector(|| "quick-command-category-blank".to_string())
                    .w_full()
                    .flex_1()
                    .min_h(px(32.)),
                [NyaMenuItem::action(t!("quickCommands.addCategory"))
                    .icon("icons/fe/new-folder.svg")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_new_quick_command_category(None, window, cx);
                    }))],
            )
            .into_any_element(),
        );
        let drop_hint = self
            .commands
            .quick_drop_target()
            .filter(|_| cx.has_active_drag())
            .and_then(|target| {
                if target.id == "all" {
                    return Some(t!("quickCommands.dropRootCategory").to_string());
                }
                self.commands
                    .quick_command_categories()
                    .iter()
                    .find(|item| item.id == target.id)
                    .map(|category| match target.position {
                        QuickCommandDropPosition::Before => t!(
                            "quickCommands.dropBeforeCategory",
                            category = category.name.clone()
                        )
                        .to_string(),
                        QuickCommandDropPosition::After => t!(
                            "quickCommands.dropAfterCategory",
                            category = category.name.clone()
                        )
                        .to_string(),
                        QuickCommandDropPosition::Inside => t!(
                            "quickCommands.dropInsideCategory",
                            category = category.name.clone()
                        )
                        .to_string(),
                    })
            });
        div()
            .w(px(176.))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(rgb(palette.border))
            .when_some(all_categories, |sidebar, row| {
                sidebar.child(div().flex_shrink_0().p(px(6.)).pb(px(0.)).child(row))
            })
            .child(category_sidebar)
            .when_some(drop_hint, |sidebar, hint| {
                sidebar.child(
                    div()
                        .debug_selector(|| "quick-command-category-drop-hint".to_string())
                        .flex_shrink_0()
                        .p_2()
                        .text_size(px(10.))
                        .text_color(rgb(palette.link))
                        .child(hint),
                )
            })
            .into_any_element()
    }

    /// Group menu for a real category, mirroring Tauri's `QuickCommands.tsx`
    /// category `ContextMenuContent`: add, then reorder, then edit/delete.
    fn quick_command_category_menu_items(
        &self,
        category_id: String,
        cx: &mut Context<Self>,
    ) -> Vec<NyaMenuItem> {
        let add_category_parent = category_id.clone();
        let add_command_id = category_id.clone();
        let move_up_id = category_id.clone();
        let move_down_id = category_id.clone();
        let outdent_id = category_id.clone();
        let can_outdent = self.commands.can_outdent_quick_category(&category_id);
        let rename_id = category_id.clone();
        let delete_id = category_id.clone();
        let can_move_up = self
            .commands
            .quick_category_move_neighbor(&category_id, true)
            .is_some();
        let can_move_down = self
            .commands
            .quick_category_move_neighbor(&category_id, false)
            .is_some();
        vec![
            NyaMenuItem::action(t!("quickCommands.addCategory"))
                .icon("icons/fe/new-folder.svg")
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_new_quick_command_category(
                        Some(add_category_parent.clone()),
                        window,
                        cx,
                    );
                })),
            NyaMenuItem::action(t!("quickCommands.addCommand"))
                .icon("icons/conn/terminal.svg")
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_new_quick_command_editor_in_category(
                        Some(add_command_id.clone()),
                        window,
                        cx,
                    );
                })),
            NyaMenuItem::separator(),
            NyaMenuItem::action(t!("dialog.moveUp"))
                .icon("icons/chevron-up.svg")
                .disabled(!can_move_up)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.move_quick_command_category(move_up_id.clone(), true, cx);
                })),
            NyaMenuItem::action(t!("dialog.moveDown"))
                .icon("icons/chevron-down.svg")
                .disabled(!can_move_down)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.move_quick_command_category(move_down_id.clone(), false, cx);
                })),
            NyaMenuItem::separator(),
            NyaMenuItem::action(t!("quickCommands.moveToParentLevel"))
                .icon("icons/chevron-up.svg")
                .disabled(!can_outdent)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.outdent_quick_command_category(outdent_id.clone(), cx);
                })),
            NyaMenuItem::separator(),
            NyaMenuItem::action(t!("quickCommands.edit"))
                .icon("icons/net/edit.svg")
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_rename_quick_command_category(rename_id.clone(), window, cx);
                })),
            NyaMenuItem::action(t!("common.delete"))
                .icon("icons/net/delete.svg")
                .danger()
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_delete_quick_command_category_confirm(delete_id.clone(), window, cx);
                })),
        ]
    }

    /// Group menu for the synthetic `all` / `uncategorized` rows. Neither can be
    /// renamed or deleted, but Tauri still offers both add actions from them: a root
    /// category, and a command with no category.
    fn quick_command_pseudo_category_menu_items(&self, cx: &mut Context<Self>) -> Vec<NyaMenuItem> {
        vec![
            NyaMenuItem::action(t!("quickCommands.addCategory"))
                .icon("icons/fe/new-folder.svg")
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_new_quick_command_category(None, window, cx);
                })),
            NyaMenuItem::action(t!("quickCommands.addCommand"))
                .icon("icons/conn/terminal.svg")
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_new_quick_command_editor_in_category(None, window, cx);
                })),
        ]
    }

    #[cfg(test)]
    pub(crate) fn quick_command_category_menu_items_for_test(
        &self,
        category_id: String,
        cx: &mut Context<Self>,
    ) -> Vec<NyaMenuItem> {
        self.quick_command_category_menu_items(category_id, cx)
    }

    #[cfg(test)]
    pub(crate) fn quick_command_pseudo_category_menu_items_for_test(
        &self,
        cx: &mut Context<Self>,
    ) -> Vec<NyaMenuItem> {
        self.quick_command_pseudo_category_menu_items(cx)
    }
}

#[cfg(test)]
mod tests {
    use gpui::{AppContext as _, TestAppContext};
    use nyaterm_core::{AppRuntime, QuickCommandCategory, RuntimeMode, uuid};
    use nyaterm_ui::NyaMenuItem;

    use crate::entities::{OverlayStore, StartupRestoreStore, UiStoreHandles};
    use crate::features::NyaTermApp;

    fn menu_app(cx: &mut TestAppContext) -> gpui::Entity<NyaTermApp> {
        // A uuid rather than a clock reading: these tests run in parallel and a
        // nanosecond timestamp can repeat, which would share one config dir.
        let root = std::env::temp_dir().join(format!(
            "nyaterm-quick-group-menu-{}-{}",
            std::process::id(),
            uuid()
        ));
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
        cx.new(|cx| NyaTermApp::new(runtime, stores, cx))
    }

    fn category(id: &str, order: i32) -> QuickCommandCategory {
        QuickCommandCategory {
            id: id.to_string(),
            name: id.to_string(),
            parent_id: None,
            sort_order: order,
        }
    }

    fn labels(items: &[NyaMenuItem]) -> Vec<&str> {
        items.iter().map(NyaMenuItem::test_label).collect()
    }

    fn stored_selected_category(app: &gpui::Entity<NyaTermApp>, cx: &mut TestAppContext) -> String {
        cx.update_entity(app, |app, _| {
            app.store_blocking_client()
                .request_fn(nyaterm_store::StoreDomain::Settings, |store| {
                    store.load_app_settings_summary()
                })
                .expect("load stored settings")
                .ui_quick_cmd_selected_category
        })
    }

    #[test]
    fn selecting_and_deleting_a_category_updates_and_persists_settings() {
        let mut cx = TestAppContext::single();
        let app = menu_app(&mut cx);
        cx.update_entity(&app, |app, cx| {
            app.replace_quick_command_catalog(Vec::new(), vec![category("category-a", 0)], cx);
            app.select_quick_command_category("category-a".to_string(), cx);
            assert_eq!(app.commands.quick_selected_category(), "category-a");
            assert_eq!(
                app.settings.summary().ui_quick_cmd_selected_category,
                "category-a"
            );
        });
        cx.run_until_parked();
        assert_eq!(stored_selected_category(&app, &mut cx), "category-a");

        cx.update_entity(&app, |app, cx| {
            app.replace_quick_command_catalog(Vec::new(), Vec::new(), cx);
            app.commands.finish_quick_category_delete("category-a");
            app.sync_quick_command_selected_category(cx);
            assert_eq!(app.commands.quick_selected_category(), "all");
            assert_eq!(app.settings.summary().ui_quick_cmd_selected_category, "all");
        });
        cx.run_until_parked();
        assert_eq!(stored_selected_category(&app, &mut cx), "all");

        cx.update_entity(&app, |app, cx| {
            app.select_quick_command_category("uncategorized".to_string(), cx);
        });
        cx.run_until_parked();
        assert_eq!(stored_selected_category(&app, &mut cx), "uncategorized");

        cx.update_entity(&app, |app, cx| {
            let mut settings = app.settings.summary().clone();
            settings.ui_quick_cmd_selected_category = "removed-category".to_string();
            app.apply_gpui_settings(settings, cx);
            assert_eq!(app.commands.quick_selected_category(), "all");
            assert_eq!(app.settings.summary().ui_quick_cmd_selected_category, "all");
        });
        cx.run_until_parked();
        assert_eq!(stored_selected_category(&app, &mut cx), "all");
    }

    /// Mirrors Tauri's category `ContextMenuContent`: two add actions, the reorder
    /// pair, then edit/delete, each group separated.
    #[test]
    fn group_menu_matches_tauri_structure_and_marks_delete_dangerous() {
        let mut cx = TestAppContext::single();
        let app = menu_app(&mut cx);
        cx.update_entity(&app, |app, _| {
            app.commands.replace_quick_command_catalog(
                Vec::new(),
                vec![
                    category("first", 0),
                    category("middle", 1),
                    category("last", 2),
                ],
            );
        });
        let items = cx.update_entity(&app, |app, cx| {
            app.quick_command_category_menu_items_for_test("middle".to_string(), cx)
        });

        assert_eq!(
            labels(&items),
            vec![
                "Add Category",
                "Add Command",
                "",
                "Move up",
                "Move down",
                "",
                "Move to parent level",
                "",
                "Edit",
                "Delete",
            ]
        );
        // (label, shortcut, icon, disabled, checked, danger)
        assert!(items[9].test_presentation().5, "delete should be dangerous");
        assert!(items.iter().all(|item| item.children().is_none()));
    }

    #[test]
    fn group_menu_disables_the_move_that_would_leave_the_sibling_run() {
        let mut cx = TestAppContext::single();
        let app = menu_app(&mut cx);
        cx.update_entity(&app, |app, _| {
            app.commands.replace_quick_command_catalog(
                Vec::new(),
                vec![
                    category("first", 0),
                    category("middle", 1),
                    category("last", 2),
                ],
            );
        });

        let disabled = |app: &gpui::Entity<NyaTermApp>, cx: &mut TestAppContext, id: &str| {
            let items = cx.update_entity(app, |app, cx| {
                app.quick_command_category_menu_items_for_test(id.to_string(), cx)
            });
            (
                items[3].test_presentation().3,
                items[4].test_presentation().3,
            )
        };

        assert_eq!(disabled(&app, &mut cx, "first"), (true, false));
        assert_eq!(disabled(&app, &mut cx, "middle"), (false, false));
        assert_eq!(disabled(&app, &mut cx, "last"), (false, true));
    }

    #[test]
    fn group_menu_enables_outdent_only_for_nested_categories() {
        let mut cx = TestAppContext::single();
        let app = menu_app(&mut cx);
        cx.update_entity(&app, |app, _| {
            let mut child = category("child", 0);
            child.parent_id = Some("root".into());
            app.commands
                .replace_quick_command_catalog(Vec::new(), vec![category("root", 0), child]);
        });
        for (id, disabled) in [("root", true), ("child", false)] {
            let items = cx.update_entity(&app, |app, cx| {
                app.quick_command_category_menu_items_for_test(id.into(), cx)
            });
            assert_eq!(items[6].test_label(), "Move to parent level");
            assert_eq!(items[6].test_presentation().3, disabled);
        }
    }

    /// `all` / `uncategorized` cannot be renamed or deleted, so they offer the add
    /// actions only.
    #[test]
    fn pseudo_group_menu_offers_only_the_add_actions() {
        let mut cx = TestAppContext::single();
        let app = menu_app(&mut cx);
        let items = cx.update_entity(&app, |app, cx| {
            app.quick_command_pseudo_category_menu_items_for_test(cx)
        });

        assert_eq!(labels(&items), vec!["Add Category", "Add Command"]);
        assert!(items.iter().all(|item| !item.test_presentation().5));
    }
}
