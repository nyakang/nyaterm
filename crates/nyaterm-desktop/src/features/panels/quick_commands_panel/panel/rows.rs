use std::rc::Rc;

use rust_i18n::t;

use gpui::{
    AnyElement, AppContext as _, ClickEvent, Context, FontWeight, IntoElement, MouseButton,
    SharedString, div, prelude::*, px, relative, rgb, rgba,
};
use nyaterm_core::QuickCommand;
use nyaterm_ui::{NyaContextMenu, NyaMenuItem};

use super::super::super::{
    quick_command_icon_mark, quick_command_pin_mark, quick_command_single_line,
};
use super::super::detail_card::{QuickCommandCardExecutionMode, QuickCommandTooltip};
use super::super::helpers::{
    QuickCommandRowHandlers, QuickCommandRowPresentation, quick_command_row_actions,
};
use crate::features::NyaTermApp;
use crate::features::commands::quick_command_category_label;
use crate::models::QuickCommandViewMode;

use super::{QuickCommandDragKind, QuickCommandDragPayload, QuickCommandDragPreview};
use crate::features::{commands::QuickCommandDropPosition, commands::QuickCommandDropTarget};

const QUICK_COMMAND_MENU_WIDTH: f32 = 148.;

/// Tauri wraps every command icon in a fixed square so that a color dot and a
/// brand glyph occupy the same slot and the label does not shift between the two.
fn quick_command_icon_slot(slot_px: f32, icon: impl IntoElement) -> impl IntoElement {
    div()
        .size(px(slot_px))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .child(icon)
}

impl NyaTermApp {
    pub(super) fn quick_command_items(
        &mut self,
        commands: &[QuickCommand],
        palette: crate::theme::ThemePalette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let view_mode = self.commands.quick_view_mode();
        // Tile hover cards render outside the app entity, so they need the resolved
        // wallpaper-aware surface rather than the raw palette token.
        let card_surface = self.shell_surface_color(palette.surface);
        let mut items = Vec::with_capacity(commands.len());
        for command in commands.iter().cloned() {
            let command_id = command.id.clone();
            let run_command_id = command.id.clone();
            let compact_click_command_id = command.id.clone();
            let list_header_command_id = command.id.clone();
            let detail_command_id = command.id.clone();
            let menu_items = self.quick_command_row_menu_items(command.id.clone(), cx);
            let execution_mode = if command.execution_mode.as_deref() == Some("append") {
                "append"
            } else {
                "execute"
            };
            let badge_label = if execution_mode == "append" {
                t!("quickCommands.appendOnlyBadge")
            } else {
                t!("quickCommands.executeImmediately")
            };
            let badge_mode = QuickCommandCardExecutionMode {
                append: execution_mode == "append",
                label: badge_label.clone(),
            };
            // Flattened once per row: the preview lines are `.truncate()`d, and GPUI
            // still splits on newlines when wrapping is off.
            let command_preview = quick_command_single_line(&command.command);
            let tile_tooltip_control = self.commands.quick_tooltip_control();
            let command_item = match view_mode {
                QuickCommandViewMode::Tile => {
                    let tooltip_control_for_right_click = tile_tooltip_control.clone();
                    let tooltip_command_id_for_right_click = command_id.clone();
                    let tooltip_control_for_hover = tile_tooltip_control.clone();
                    let tooltip_command_id_for_hover = command_id.clone();
                    NyaContextMenu::new(
                        div()
                            .id(SharedString::from(format!(
                                "quick-command-tile-{command_id}"
                            )))
                            .relative()
                            // Content-width chip in a wrapping row, like Tauri's
                            // `max-w-full shrink-0`: it never pads out to a grid cell, and
                            // only a chip wider than the row truncates.
                            .flex_none()
                            .max_w_full()
                            .rounded_md()
                            .border_1()
                            .border_color(rgba((palette.border << 8) | 0x59))
                            .bg(rgba((palette.surface_elevated << 8) | 0x33))
                            .px_2()
                            .py(px(4.))
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .cursor_pointer()
                            .hover(move |this| {
                                this.bg(rgba((palette.surface_elevated << 8) | 0x80))
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.run_quick_command_by_id(run_command_id.clone(), cx);
                            }))
                            .on_mouse_down(MouseButton::Right, move |_, _, cx| {
                                tooltip_control_for_right_click
                                    .dismiss(&tooltip_command_id_for_right_click, cx);
                            })
                            .on_hover(move |hovered, _, _| {
                                if *hovered {
                                    tooltip_control_for_hover
                                        .begin_hover(&tooltip_command_id_for_hover);
                                }
                            })
                            .child(quick_command_icon_slot(
                                14.,
                                quick_command_icon_mark(
                                    palette,
                                    command.icon_tag.as_deref(),
                                    command.color_tag.as_deref(),
                                    12.,
                                ),
                            ))
                            .when(command.pinned.unwrap_or_default(), |this| {
                                this.child(quick_command_pin_mark(palette, 10.))
                            })
                            .child(
                                div()
                                    .min_w_0()
                                    .text_size(px(11.))
                                    .font_weight(FontWeight(500.))
                                    .text_color(rgb(palette.text))
                                    .truncate()
                                    .child(command.label.clone()),
                            )
                            // Tauri's tile hover is the full command card, not a text
                            // blurb: in tile mode it is the only way to read the command.
                            // Hoverable so the pointer can enter the card and copy, as
                            // it can in Tauri's non-`disableHoverableContent` tooltip.
                            .hoverable_tooltip({
                                let tooltip_command = command.clone();
                                let tooltip_category = quick_command_category_label(
                                    self.commands.quick_command_categories(),
                                    &command,
                                );
                                let tooltip_app = cx.entity().downgrade();
                                let tooltip_badge = badge_mode.clone();
                                let tile_tooltip_control = tile_tooltip_control.clone();
                                let tooltip_command_id = command_id.clone();
                                move |_, cx| {
                                    let tooltip = cx.new(|_| {
                                        QuickCommandTooltip::new(
                                            palette,
                                            card_surface,
                                            tooltip_command.clone(),
                                            tooltip_category.clone(),
                                            tooltip_badge.clone(),
                                            tooltip_app.clone(),
                                            tile_tooltip_control.is_suppressed(&tooltip_command_id),
                                        )
                                    });
                                    let tooltip_to_dismiss = tooltip.downgrade();
                                    tile_tooltip_control.register(
                                        tooltip_command_id.clone(),
                                        Rc::new(move |cx| {
                                            if let Some(tooltip) = tooltip_to_dismiss.upgrade() {
                                                tooltip
                                                    .update(cx, |tooltip, cx| tooltip.dismiss(cx));
                                            }
                                        }),
                                    );
                                    tooltip.into()
                                }
                            }),
                        menu_items,
                    )
                    .min_width(px(QUICK_COMMAND_MENU_WIDTH))
                    .into_any_element()
                }
                QuickCommandViewMode::Compact => {
                    // Tauri compact: send + details + more (edit / send-all / delete).
                    let actions = quick_command_row_actions(
                        palette,
                        QuickCommandRowPresentation {
                            command_id: &command_id,
                            show_badge: false,
                            execution_mode,
                            badge_label,
                        },
                        QuickCommandRowHandlers {
                            on_run: cx.listener(move |this, _, _, cx| {
                                this.run_quick_command_by_id(run_command_id.clone(), cx);
                            }),
                            on_details: cx.listener(move |this, event: &ClickEvent, window, cx| {
                                let position = event.position();
                                this.open_quick_command_details(
                                    detail_command_id.clone(),
                                    position.x,
                                    position.y,
                                    window,
                                    cx,
                                );
                            }),
                            menu_items: menu_items.clone(),
                        },
                    );

                    NyaContextMenu::new(
                        div()
                            .id(SharedString::from(format!(
                                "quick-command-compact-{command_id}"
                            )))
                            .relative()
                            .h(px(32.))
                            .w_full()
                            .rounded_sm()
                            .px(px(6.))
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .hover(move |this| {
                                this.bg(rgba((palette.surface_elevated << 8) | 0x73))
                            })
                            .child(
                                div()
                                    .id(SharedString::from(format!(
                                        "quick-command-compact-run-area-{command_id}"
                                    )))
                                    .min_w_0()
                                    .flex_1()
                                    .h_full()
                                    .px(px(2.))
                                    .flex()
                                    .items_center()
                                    .gap(px(6.))
                                    .cursor_pointer()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.run_quick_command_by_id(
                                            compact_click_command_id.clone(),
                                            cx,
                                        );
                                    }))
                                    .child(quick_command_icon_slot(
                                        16.,
                                        quick_command_icon_mark(
                                            palette,
                                            command.icon_tag.as_deref(),
                                            command.color_tag.as_deref(),
                                            12.8,
                                        ),
                                    ))
                                    .when(command.pinned.unwrap_or_default(), |this| {
                                        this.child(quick_command_pin_mark(palette, 10.4))
                                    })
                                    .child(
                                        // Tauri `min-w-[4rem] max-w-[38%]`: the label grows
                                        // with the panel instead of stopping at a fixed
                                        // width and handing the rest to the command.
                                        div()
                                            .min_w(px(64.))
                                            .max_w(relative(0.38))
                                            .text_xs()
                                            .font_weight(FontWeight(500.))
                                            .text_color(rgb(palette.text))
                                            .truncate()
                                            .child(command.label.clone()),
                                    )
                                    .child(
                                        div()
                                            .min_w_0()
                                            .flex_1()
                                            .font_family(
                                                crate::features::shell::gpui_code_font_family(),
                                            )
                                            .text_size(px(11.))
                                            .text_color(rgb(palette.text_muted))
                                            .truncate()
                                            .child(command_preview.clone()),
                                    ),
                            )
                            .child(actions),
                        menu_items,
                    )
                    .min_width(px(QUICK_COMMAND_MENU_WIDTH))
                    .into_any_element()
                }
                QuickCommandViewMode::List => {
                    // Tauri list: badge + send + details + more.
                    let actions = quick_command_row_actions(
                        palette,
                        QuickCommandRowPresentation {
                            command_id: &command_id,
                            show_badge: true,
                            execution_mode,
                            badge_label,
                        },
                        QuickCommandRowHandlers {
                            on_run: cx.listener(move |this, _, _, cx| {
                                this.run_quick_command_by_id(run_command_id.clone(), cx);
                            }),
                            on_details: cx.listener(move |this, event: &ClickEvent, window, cx| {
                                let position = event.position();
                                this.open_quick_command_details(
                                    detail_command_id.clone(),
                                    position.x,
                                    position.y,
                                    window,
                                    cx,
                                );
                            }),
                            menu_items: menu_items.clone(),
                        },
                    );

                    NyaContextMenu::new(
                        div()
                            .id(SharedString::from(format!(
                                "quick-command-list-{command_id}"
                            )))
                            .relative()
                            .min_h(px(44.))
                            .w_full()
                            .rounded_md()
                            .border_1()
                            .border_color(rgba((palette.border << 8) | 0x59))
                            .bg(rgba((palette.surface_elevated << 8) | 0x26))
                            .px_2()
                            .py(px(6.))
                            .flex()
                            .items_center()
                            .gap_2()
                            .hover(move |this| {
                                this.bg(rgba((palette.surface_elevated << 8) | 0x73))
                            })
                            .child(
                                div()
                                    .id(SharedString::from(format!(
                                        "quick-command-list-run-head-{command_id}"
                                    )))
                                    .min_w_0()
                                    .flex_1()
                                    .px_1()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .cursor_pointer()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.run_quick_command_by_id(
                                            list_header_command_id.clone(),
                                            cx,
                                        );
                                    }))
                                    .child(quick_command_icon_slot(
                                        16.,
                                        quick_command_icon_mark(
                                            palette,
                                            command.icon_tag.as_deref(),
                                            command.color_tag.as_deref(),
                                            14.4,
                                        ),
                                    ))
                                    .child(
                                        div()
                                            .min_w_0()
                                            .flex_1()
                                            .flex()
                                            .flex_col()
                                            .gap(px(2.))
                                            .child(
                                                // The pin sits left of the label, as in
                                                // Tauri. Trailing a `flex_1` label parked
                                                // it at the far edge of the row instead.
                                                div()
                                                    .min_w_0()
                                                    .flex()
                                                    .items_center()
                                                    .gap(px(6.))
                                                    .when(
                                                        command.pinned.unwrap_or_default(),
                                                        |this| {
                                                            this.child(quick_command_pin_mark(
                                                                palette, 11.2,
                                                            ))
                                                        },
                                                    )
                                                    .child(
                                                        div()
                                                            .min_w_0()
                                                            .text_xs()
                                                            .font_weight(FontWeight(500.))
                                                            .text_color(rgb(palette.text))
                                                            .truncate()
                                                            .child(command.label.clone()),
                                                    ),
                                            )
                                            .child(
                                                div()
                                                    .min_w_0()
                                                    .font_family(
                                                        crate::features::shell::gpui_code_font_family(),
                                                    )
                                                    .text_size(px(11.))
                                                    .line_height(px(14.))
                                                    .text_color(rgb(palette.text_muted))
                                                    .truncate()
                                                    .child(command_preview.clone()),
                                            ),
                                    ),
                            )
                            .child(actions),
                        menu_items,
                    )
                    .min_width(px(QUICK_COMMAND_MENU_WIDTH))
                    .into_any_element()
                }
            };

            let drag_command_id = command.id.clone();
            let drag_command_label = command.label.clone();
            let move_target_id = command.id.clone();
            let drop_target_id = command.id.clone();
            let tile = view_mode == QuickCommandViewMode::Tile;
            items.push(
                div()
                    .id(SharedString::from(format!(
                        "quick-command-drag-{command_id}"
                    )))
                    // A tile is one item in a wrapping row, so its wrapper must not claim
                    // the full width the way a list row's does.
                    .when(tile, |this| this.flex_none().min_w_0().max_w_full())
                    .when(!tile, |this| this.w_full())
                    .relative()
                    .cursor_move()
                    .on_drag(
                        QuickCommandDragPayload {
                            kind: QuickCommandDragKind::Command,
                            id: drag_command_id,
                            label: drag_command_label,
                        },
                        |payload, position, _, cx| {
                            cx.new(|_| QuickCommandDragPreview {
                                payload: payload.clone(),
                                position,
                            })
                        },
                    )
                    .on_drag_move(cx.listener(
                        move |this, event: &gpui::DragMoveEvent<QuickCommandDragPayload>, _, cx| {
                            let _ = event.drag(cx);
                            let after = event.event.position.y
                                >= event.bounds.origin.y + event.bounds.size.height / 2.;
                            if this.commands.set_quick_drop_target(QuickCommandDropTarget {
                                id: move_target_id.clone(),
                                position: if after {
                                    QuickCommandDropPosition::After
                                } else {
                                    QuickCommandDropPosition::Before
                                },
                            }) {
                                cx.notify();
                            }
                        },
                    ))
                    .on_drop(
                        cx.listener(move |this, payload: &QuickCommandDragPayload, _, cx| {
                            let after = this
                                .commands
                                .quick_drop_target()
                                .filter(|target| target.id == drop_target_id)
                                .is_some_and(|target| {
                                    target.position == QuickCommandDropPosition::After
                                });
                            let config = (payload.kind == QuickCommandDragKind::Command)
                                .then(|| {
                                    this.commands.reorder_quick_command(
                                        &payload.id,
                                        &drop_target_id,
                                        after,
                                    )
                                })
                                .flatten();
                            this.finish_quick_command_reorder(config, cx);
                        }),
                    )
                    .child(command_item)
                    .into_any_element(),
            );
        }
        items
    }

    fn quick_command_row_menu_items(
        &self,
        command_id: String,
        cx: &mut Context<Self>,
    ) -> Vec<NyaMenuItem> {
        let edit_command_id = command_id.clone();
        let all_command_id = command_id.clone();
        let delete_command_id = command_id;
        let send_to_all_disabled = self.session.live_session_count() == 0;
        let mut items = vec![
            NyaMenuItem::action(t!("quickCommands.edit"))
                .icon("icons/net/edit.svg")
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_edit_quick_command_editor(edit_command_id.clone(), window, cx);
                })),
            NyaMenuItem::action(t!("quickCommands.sendToAll"))
                .icon("icons/menu/broadcast.svg")
                .disabled(send_to_all_disabled)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.send_quick_command_to_all_by_id(all_command_id.clone(), cx);
                })),
        ];
        items.extend([
            NyaMenuItem::separator(),
            NyaMenuItem::action(t!("common.delete"))
                .icon("icons/net/delete.svg")
                .danger()
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_delete_quick_command_confirm(delete_command_id.clone(), window, cx);
                })),
        ]);
        items
    }
}

#[cfg(test)]
mod tests {
    use gpui::{AppContext as _, TestAppContext};
    use nyaterm_core::{AppRuntime, RuntimeMode, uuid};

    use super::QUICK_COMMAND_MENU_WIDTH;
    use crate::entities::{OverlayStore, StartupRestoreStore, UiStoreHandles};
    use crate::features::NyaTermApp;

    fn menu_app(cx: &mut TestAppContext) -> gpui::Entity<NyaTermApp> {
        let root = std::env::temp_dir().join(format!(
            "nyaterm-quick-command-menu-{}-{}",
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

    #[test]
    fn command_menu_keeps_send_to_all_visible_without_a_live_session() {
        let mut cx = TestAppContext::single();
        let app = menu_app(&mut cx);
        let items = cx.update_entity(&app, |app, cx| {
            app.quick_command_row_menu_items("command-1".to_string(), cx)
        });

        assert_eq!(
            items
                .iter()
                .map(|item| item.test_label())
                .collect::<Vec<_>>(),
            vec!["Edit", "Send to All", "", "Delete"]
        );
        assert!(items[1].test_presentation().3);
        assert_eq!(QUICK_COMMAND_MENU_WIDTH, 148.);
    }
}
