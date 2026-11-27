use std::{cell::RefCell, rc::Rc};

use gpui::{
    Anchor, AnyElement, AppContext as _, Context, FontWeight, IntoElement, KeyDownEvent,
    MouseButton, SharedString, Window, anchored, canvas, deferred, div, point, prelude::*, px, rgb,
    rgba, svg,
};
use nyaterm_core::QuickCommand;
use nyaterm_ui::{
    NyaContextMenu, NyaDropdownMenu, NyaHoverCard, NyaMenuItem, NyaPopoverPlacement, NyaScrollable,
    NyaSearchInput, NyaTooltip,
};
use rust_i18n::t;

use super::super::super::{
    filtered_quick_commands, quick_command_category_options, quick_command_icon_mark,
    quick_command_pin_mark,
};
use super::super::detail_card::{QuickCommandCardExecutionMode, quick_command_hover_card};
use super::rows::{QUICK_COMMAND_MENU_WIDTH, quick_command_icon_slot};
use super::{
    QuickCommandDragKind, QuickCommandDragPayload, QuickCommandDragPreview,
    quick_command_toolbar_divider, quick_command_toolbar_icon_button,
    quick_command_view_menu_button_with_id,
};
use crate::features::{
    NyaTermApp,
    commands::{QuickCommandDropPosition, QuickCommandDropTarget},
    text_inputs::TextInputSetup,
    view_widgets::APP_OVERLAY_PRIORITY,
};

pub(super) const COMMAND_BAR_HEIGHT_PX: f32 = 36.;
pub(super) const COMMAND_BAR_CHIP_MAX_WIDTH_PX: f32 = 168.;
const COMMAND_BAR_CHIP_MIN_WIDTH_PX: f32 = 48.;
const COMMAND_BAR_TRAILING_FIT_MIN_PX: f32 = 86.;
const COMMAND_BAR_CHIP_GAP_PX: f32 = 4.;
const COMMAND_BAR_SAFETY_MARGIN_PX: f32 = 6.;
/// 8 rows × 30px + 7 gaps × 2px + 8px vertical padding = 262px exact row boundary.
const COMMAND_BAR_POPUP_LIST_MAX_HEIGHT_PX: f32 = 262.;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct CommandBarLayoutPlan {
    pub visible_count: usize,
    /// Optional width cap applied to the last visible chip when it is squeezed
    /// into the remaining trailing space rather than leaving a wide gap.
    pub trailing_max_width: Option<f32>,
}

/// Estimates the rendered pixel width of a single command chip on the Command Bar.
pub(super) fn estimate_command_bar_chip_width(command: &QuickCommand) -> f32 {
    // px_2 (16px) + icon slot (14px) + gap (5px) = 35px base chrome.
    let fixed = 35.0;
    let text_width: f32 = command
        .label
        .chars()
        .map(|ch| match ch {
            ' ' => 3.6,
            'i' | 'l' | '1' | '.' | ',' | ':' | ';' | '/' | '-' | '_' | '|' | 'I' => 4.6,
            ch if ch.is_ascii() => 6.2,
            _ => 11.0,
        })
        .sum();
    (fixed + text_width.max(18.0))
        .clamp(COMMAND_BAR_CHIP_MIN_WIDTH_PX, COMMAND_BAR_CHIP_MAX_WIDTH_PX)
}

/// Computes how many leading commands fit on the single-line Command Bar for
/// `available_width_px`, capping the trailing chip when enough space remains to
/// show a meaningful truncated label instead of leaving an empty gap.
pub(super) fn plan_command_bar_layout(
    commands: &[QuickCommand],
    available_width_px: f32,
) -> CommandBarLayoutPlan {
    if commands.is_empty() || !available_width_px.is_finite() || available_width_px <= 0.0 {
        return CommandBarLayoutPlan {
            visible_count: 0,
            trailing_max_width: None,
        };
    }
    let budget = (available_width_px - COMMAND_BAR_SAFETY_MARGIN_PX).max(0.0);
    let mut used = 0.0_f32;
    let mut count = 0_usize;
    for command in commands {
        let chip_width = estimate_command_bar_chip_width(command);
        let needed = if count == 0 {
            chip_width
        } else {
            COMMAND_BAR_CHIP_GAP_PX + chip_width
        };
        if used + needed <= budget {
            used += needed;
            count += 1;
        } else {
            let remaining = if count == 0 {
                budget
            } else {
                budget - used - COMMAND_BAR_CHIP_GAP_PX
            };
            if remaining >= COMMAND_BAR_TRAILING_FIT_MIN_PX {
                return CommandBarLayoutPlan {
                    visible_count: count + 1,
                    trailing_max_width: Some(remaining.min(COMMAND_BAR_CHIP_MAX_WIDTH_PX)),
                };
            }
            break;
        }
    }
    if count == 0 && available_width_px >= 64.0 {
        CommandBarLayoutPlan {
            visible_count: 1,
            trailing_max_width: Some(budget.max(COMMAND_BAR_CHIP_MIN_WIDTH_PX)),
        }
    } else {
        CommandBarLayoutPlan {
            visible_count: count,
            trailing_max_width: None,
        }
    }
}

fn command_execution_badge_mode(command: &QuickCommand) -> QuickCommandCardExecutionMode {
    let execution_mode = if command.segmented_send.is_some() {
        "segmented"
    } else if command.execution_mode.as_deref() == Some("append") {
        "append"
    } else {
        "execute"
    };
    let badge_label = match execution_mode {
        "segmented" => t!("quickCommands.segmentedSendBadge"),
        "append" => t!("quickCommands.appendOnlyBadge"),
        _ => t!("quickCommands.executeImmediately"),
    };
    QuickCommandCardExecutionMode {
        append: execution_mode == "append",
        segmented: execution_mode == "segmented",
        label: badge_label,
    }
}

impl NyaTermApp {
    pub(in crate::features) fn quick_command_bar_view(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let card_surface = self.shell_surface_color(palette.surface);

        // Bar chips follow the selected category and sort mode (pinned first) without
        // applying an invisible search query; the popup search filters interactively.
        let bar_commands = filtered_quick_commands(
            self.commands.quick_commands(),
            self.commands.quick_command_categories(),
            "",
            self.commands.quick_selected_category(),
            self.commands.quick_sort_mode(),
        );
        let categories = quick_command_category_options(
            self.commands.quick_commands(),
            self.commands.quick_command_categories(),
            t!("quickCommands.allCategories"),
            t!("quickCommands.uncategorized"),
        );
        let selected_category_id = self.commands.quick_selected_category().to_string();
        let selected_category_label = categories
            .iter()
            .find(|option| option.id == selected_category_id)
            .map(|option| option.label.clone())
            .unwrap_or_else(|| t!("quickCommands.allCategories").to_string());

        let fallback_strip_width = {
            let (viewport_width, _) = self.shell.viewport_size();
            let left_w = if self.left_side_open() && !self.shell.panel_is_floating() {
                self.shell.left_panel_width()
            } else {
                0.0
            };
            let right_w = if self.right_side_open() && !self.shell.panel_is_floating() {
                self.shell.right_panel_width()
            } else {
                0.0
            };
            (viewport_width - left_w - right_w - 260.0).max(120.0)
        };
        let available_strip_width = self
            .commands
            .quick_command_bar_strip_width()
            .map(f32::from)
            .filter(|width| *width > 0.0)
            .unwrap_or(fallback_strip_width);

        let plan = plan_command_bar_layout(&bar_commands, available_strip_width);
        let visible_count = plan.visible_count;
        let overflow_count = bar_commands.len().saturating_sub(visible_count);

        let category_app = cx.entity().downgrade();
        let category_filtered = selected_category_id != "all";
        let category_trigger_content = div()
            .flex()
            .items_center()
            .gap(px(5.))
            .child(
                svg()
                    .size(px(12.5))
                    .flex_none()
                    .path("icons/conn/folder.svg")
                    .text_color(if category_filtered {
                        rgb(palette.link)
                    } else {
                        rgb(palette.text_muted)
                    }),
            )
            .child(
                div()
                    .max_w(px(108.))
                    .text_size(px(11.))
                    .font_weight(FontWeight(500.))
                    .text_color(if category_filtered {
                        rgb(palette.link)
                    } else {
                        rgb(palette.text)
                    })
                    .truncate()
                    .child(selected_category_label.clone()),
            )
            .child(
                svg()
                    .size(px(11.))
                    .flex_none()
                    .path("icons/chevron-down.svg")
                    .text_color(rgb(palette.text_dimmed)),
            );

        let category_dropdown = div().flex_none().child(
            NyaDropdownMenu::new("quick-command-bar-category")
                .content(category_trigger_content)
                .selected(category_filtered)
                .tooltip(selected_category_label)
                .min_width(px(200.))
                .max_height(px(320.))
                .scrollable(true)
                .items_dynamic(move |_, cx| {
                    category_app
                        .update(cx, |app, cx| {
                            let palette = app.theme_palette();
                            let selected_id = app.commands.quick_selected_category().to_string();
                            let options = quick_command_category_options(
                                app.commands.quick_commands(),
                                app.commands.quick_command_categories(),
                                t!("quickCommands.allCategories"),
                                t!("quickCommands.uncategorized"),
                            );
                            options
                                .into_iter()
                                .map(|option| {
                                    let id = option.id.clone();
                                    let checked = option.id == selected_id;
                                    NyaMenuItem::action(option.label)
                                        .checked(checked)
                                        .label_indent(px(option.depth as f32 * 12.))
                                        .status(option.count.to_string(), palette.text_dimmed)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.select_quick_command_category(id.clone(), cx);
                                        }))
                                })
                                .collect()
                        })
                        .unwrap_or_default()
                }),
        );

        let context_target = Rc::new(RefCell::new(None::<String>));
        let context_target_for_chips = context_target.clone();
        let context_target_for_reset = context_target.clone();
        let bar_menu_app = cx.entity().downgrade();

        let chips = self.quick_command_bar_chips(
            &bar_commands[..visible_count],
            plan.trailing_max_width,
            palette,
            card_surface,
            &context_target_for_chips,
            cx,
        );

        let measure_app = cx.weak_entity();
        let width_probe = canvas(
            move |bounds, _, cx| {
                let width = bounds.size.width;
                let app = measure_app.clone();
                cx.defer(move |cx| {
                    let _ = app.update(cx, |this, cx| {
                        if this.commands.set_quick_command_bar_strip_width(width) {
                            cx.notify();
                        }
                    });
                });
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0();

        let popup_open = self.commands.quick_command_bar_popup_is_open();
        let popup_element = popup_open.then(|| {
            self.quick_command_bar_overflow_popup(
                &bar_commands[visible_count..],
                overflow_count,
                palette,
                card_surface,
                cx,
            )
        });

        let overflow_or_search_trigger = if overflow_count > 0 {
            let tooltip_text = t!("quickCommands.moreCommands", count = overflow_count).to_string();
            div()
                .id("quick-command-bar-overflow")
                .debug_selector(|| "quick-command-bar-overflow".to_string())
                .h(px(26.))
                .px_2()
                .flex_none()
                .flex()
                .items_center()
                .gap_1()
                .rounded_md()
                .border_1()
                .border_color(if popup_open {
                    rgb(palette.link)
                } else {
                    rgba((palette.border << 8) | 0x66)
                })
                .bg(if popup_open {
                    rgba((palette.primary << 8) | 0x24)
                } else {
                    rgba((palette.surface_elevated << 8) | 0x4d)
                })
                .text_size(px(11.))
                .font_weight(FontWeight(600.))
                .text_color(if popup_open {
                    rgb(palette.link)
                } else {
                    rgb(palette.text_muted)
                })
                .cursor_pointer()
                .hover(move |this| this.bg(rgb(palette.hover)).text_color(rgb(palette.text)))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .tooltip(move |window, cx| NyaTooltip::new(tooltip_text.clone()).build(window, cx))
                .on_click(cx.listener(|this, _, window, cx| {
                    this.toggle_quick_command_bar_overflow_popup(window, cx);
                }))
                .child("»")
                .child(overflow_count.to_string())
                .into_any_element()
        } else {
            div()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(quick_command_toolbar_icon_button(
                    palette,
                    "quick-command-bar-search",
                    "icons/fe/search.svg",
                    popup_open,
                    t!("quickCommands.search"),
                    cx.listener(|this, _, window, cx| {
                        this.toggle_quick_command_bar_overflow_popup(window, cx);
                    }),
                ))
                .into_any_element()
        };

        let bar_row = div()
            .id("quick-command-bar")
            .debug_selector(|| "quick-command-bar".to_string())
            .capture_any_mouse_down(move |event, _, _| {
                if event.button == MouseButton::Right {
                    *context_target_for_reset.borrow_mut() = None;
                }
            })
            .h(px(COMMAND_BAR_HEIGHT_PX))
            .w_full()
            .flex_none()
            .px_2()
            .border_t_1()
            .border_color(rgb(palette.border))
            .bg(self.shell_surface_color(palette.section_header))
            .flex()
            .items_center()
            .gap(px(4.))
            .child(category_dropdown)
            .child(quick_command_toolbar_divider(palette))
            .child(
                div()
                    .id("quick-command-bar-strip")
                    .debug_selector(|| "quick-command-bar-strip".to_string())
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_hidden()
                    .flex()
                    .items_center()
                    .gap(px(COMMAND_BAR_CHIP_GAP_PX))
                    .child(width_probe)
                    .when(bar_commands.is_empty(), |strip| {
                        strip.child(
                            div()
                                .text_size(px(11.))
                                .text_color(rgb(palette.text_dimmed))
                                .truncate()
                                .child(t!("quickCommands.noCommandsFound")),
                        )
                    })
                    .children(chips),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .child(overflow_or_search_trigger)
                    .when_some(popup_element, |anchor_host, popup| {
                        // Zero-width anchor at the right edge of the trigger so
                        // `Anchor::BottomRight` always aligns with the trigger's right edge.
                        anchor_host.child(div().relative().w_0().h_0().child(popup))
                    }),
            )
            .child(quick_command_toolbar_divider(palette))
            .when(self.commands.staged_send_progress().is_some(), |bar| {
                bar.child(quick_command_toolbar_icon_button(
                    palette,
                    "quick-command-bar-stop",
                    "icons/session/stop.svg",
                    false,
                    t!("serialSend.stop"),
                    cx.listener(|this, _, _, cx| this.stop_quick_command_sends(cx)),
                ))
                .child(
                    div()
                        .flex_none()
                        .text_size(px(11.))
                        .text_color(rgb(palette.text_muted))
                        .child(t!(
                            "serialSend.shellProgressCompleted",
                            count = self.commands.staged_send_progress().unwrap_or_default()
                        )),
                )
            })
            .child(quick_command_toolbar_icon_button(
                palette,
                "quick-command-bar-add",
                "icons/conn/add.svg",
                false,
                t!("quickCommands.addCommand"),
                cx.listener(|this, _, window, cx| {
                    this.close_quick_command_toolbar_popovers();
                    this.open_new_quick_command_editor(window, cx);
                }),
            ))
            .child(quick_command_view_menu_button_with_id(
                "quick-command-bar-view",
                self.commands.quick_view_mode(),
                cx,
            ))
            .child(quick_command_toolbar_icon_button(
                palette,
                "quick-command-bar-close",
                "icons/window/close.svg",
                false,
                t!("common.close"),
                cx.listener(|this, _, window, cx| {
                    this.close_quick_command_bar(window, cx);
                }),
            ));

        NyaContextMenu::new_dynamic(bar_row, move |_, cx| {
            bar_menu_app
                .update(cx, |app, cx| {
                    app.quick_command_extended_context_menu_items(
                        context_target.borrow().clone(),
                        cx,
                    )
                })
                .unwrap_or_default()
        })
        .min_width(px(QUICK_COMMAND_MENU_WIDTH))
    }

    fn quick_command_bar_chips(
        &mut self,
        commands: &[QuickCommand],
        trailing_max_width: Option<f32>,
        palette: crate::theme::ThemePalette,
        card_surface: gpui::Rgba,
        context_target: &Rc<RefCell<Option<String>>>,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let total = commands.len();
        let mut items = Vec::with_capacity(total);
        for (index, command) in commands.iter().cloned().enumerate() {
            let is_last = index + 1 == total;
            let chip_max_width = if is_last {
                trailing_max_width.unwrap_or(COMMAND_BAR_CHIP_MAX_WIDTH_PX)
            } else {
                COMMAND_BAR_CHIP_MAX_WIDTH_PX
            };
            let command_id = command.id.clone();
            let run_command_id = command.id.clone();
            let badge_mode = command_execution_badge_mode(&command);

            let drop_pos = self
                .commands
                .quick_drop_target()
                .filter(|target| cx.has_active_drag() && target.id == command_id)
                .map(|target| target.position);

            // Omit the pin glyph on bar chips: pinned items already lead the bar,
            // and keeping one icon per chip saves ~15px each while staying clean.
            let chip_trigger = div()
                .id(SharedString::from(format!(
                    "quick-command-bar-chip-{command_id}"
                )))
                .relative()
                .h(px(26.))
                .max_w(px(chip_max_width))
                .flex_none()
                .rounded_md()
                .border_1()
                .border_color(rgba((palette.border << 8) | 0x59))
                .bg(rgba((palette.surface_elevated << 8) | 0x3b))
                .px_2()
                .flex()
                .items_center()
                .gap(px(5.))
                .cursor_pointer()
                .hover(move |this| this.bg(rgba((palette.surface_elevated << 8) | 0x8c)))
                .when_some(drop_pos, |chip, pos| {
                    let before = pos == QuickCommandDropPosition::Before;
                    chip.child(
                        div()
                            .absolute()
                            .top(px(3.))
                            .bottom(px(3.))
                            .w(px(2.))
                            .bg(rgb(palette.link))
                            .when(before, |line| line.left(px(0.)))
                            .when(!before, |line| line.right(px(0.))),
                    )
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.commands.close_quick_command_bar_popup();
                    this.run_quick_command_by_id(run_command_id.clone(), cx);
                }))
                .child(quick_command_icon_slot(
                    14.,
                    quick_command_icon_mark(
                        palette,
                        command.icon_tag.as_deref(),
                        command.color_tag.as_deref(),
                        12.,
                    ),
                ))
                .child(
                    div()
                        .min_w_0()
                        .text_size(px(11.))
                        .font_weight(FontWeight(500.))
                        .text_color(rgb(palette.text))
                        .truncate()
                        .child(command.label.clone()),
                );

            let hover_wrapped = NyaHoverCard::new(
                format!("quick-command-bar-hover-{command_id}"),
                chip_trigger,
                quick_command_hover_card(
                    palette,
                    card_surface,
                    &command,
                    badge_mode,
                    cx.entity().downgrade(),
                ),
            )
            .placement(NyaPopoverPlacement::Top)
            .appearance(false)
            .into_any_element();

            let drag_command_id = command.id.clone();
            let drag_command_label = command.label.clone();
            let move_target_id = command.id.clone();
            let drop_target_id = command.id.clone();
            let chip_context_target = context_target.clone();
            let chip_context_id = command_id.clone();
            let selector = format!("quick-command-bar-item-{command_id}");

            items.push(
                div()
                    .id(SharedString::from(format!(
                        "quick-command-bar-drag-{command_id}"
                    )))
                    .debug_selector(move || selector.clone())
                    .flex_none()
                    .min_w_0()
                    .max_w(px(chip_max_width))
                    .relative()
                    .capture_any_mouse_down(move |event, _, _| {
                        if event.button == MouseButton::Right {
                            *chip_context_target.borrow_mut() = Some(chip_context_id.clone());
                        }
                    })
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
                            let payload = event.drag(cx);
                            if payload.kind != QuickCommandDragKind::Command
                                || payload.id == move_target_id
                                || !event.bounds.contains(&event.event.position)
                            {
                                if this
                                    .commands
                                    .quick_drop_target()
                                    .is_some_and(|target| target.id == move_target_id)
                                {
                                    this.commands.clear_quick_drop_target();
                                    cx.notify();
                                }
                                return;
                            }
                            let after = event.event.position.x
                                >= event.bounds.origin.x + event.bounds.size.width / 2.;
                            if this.commands.set_quick_drop_target(QuickCommandDropTarget {
                                id: move_target_id.clone(),
                                position: if after {
                                    QuickCommandDropPosition::After
                                } else {
                                    QuickCommandDropPosition::Before
                                },
                            }) {
                                this.ensure_drop_hover_clock(cx);
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
                    .child(hover_wrapped)
                    .into_any_element(),
            );
        }
        items
    }

    pub(in crate::features) fn toggle_quick_command_bar_overflow_popup(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let opened = self.commands.toggle_quick_command_bar_popup();
        if opened {
            self.focus_quick_command_search(window, cx);
        } else {
            self.focus_active_workspace_surface(window, cx);
        }
        cx.notify();
    }

    fn quick_command_bar_popup_commands(
        &self,
        overflow_slice: &[QuickCommand],
        overflow_count: usize,
    ) -> (String, Vec<QuickCommand>) {
        let search_draft = self.commands.quick_search_draft().trim();
        if search_draft.is_empty() && overflow_count > 0 {
            (
                t!("quickCommands.moreCommands", count = overflow_count).to_string(),
                overflow_slice.to_vec(),
            )
        } else {
            let matched = filtered_quick_commands(
                self.commands.quick_commands(),
                self.commands.quick_command_categories(),
                search_draft,
                self.commands.quick_selected_category(),
                self.commands.quick_sort_mode(),
            );
            let title = if search_draft.is_empty() {
                format!("{} · {}", t!("quickCommands.title"), matched.len())
            } else {
                format!(
                    "{} · {}/{}",
                    t!("quickCommands.title"),
                    matched.len(),
                    self.commands.quick_commands().len()
                )
            };
            (title, matched)
        }
    }

    fn quick_command_bar_overflow_popup(
        &mut self,
        overflow_slice: &[QuickCommand],
        overflow_count: usize,
        palette: crate::theme::ThemePalette,
        popover_bg: gpui::Rgba,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (section_title, popup_commands) =
            self.quick_command_bar_popup_commands(overflow_slice, overflow_count);
        let popup_len = popup_commands.len();
        let highlight = self.commands.quick_command_bar_popup_highlight(popup_len);
        let highlighted_id = popup_commands.get(highlight).map(|cmd| cmd.id.clone());
        let popup_scroll = self.commands.quick_command_bar_popup_scroll().clone();

        let search_draft = self.commands.quick_search_draft().to_string();
        let search_field = self.text_input(
            "quick-command.search",
            &search_draft,
            TextInputSetup::placeholder(t!("quickCommands.search")),
            cx,
        );
        let clear_search = (!search_draft.trim().is_empty()).then(|| {
            div()
                .id("quick-command-bar-search-clear")
                .size(px(16.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded_sm()
                .cursor_pointer()
                .hover(move |this| this.bg(rgb(palette.hover)))
                .child(
                    svg()
                        .size(px(11.))
                        .flex_none()
                        .path("icons/close.svg")
                        .text_color(rgb(palette.text_dimmed)),
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.commands.set_quick_search_draft(String::new());
                    this.reset_text_input("quick-command.search", "", cx);
                    cx.notify();
                }))
                .into_any_element()
        });

        let popup_context_target = Rc::new(RefCell::new(None::<String>));
        let popup_context_for_rows = popup_context_target.clone();
        let popup_context_for_reset = popup_context_target.clone();
        let popup_menu_app = cx.entity().downgrade();

        let mut list_rows = Vec::with_capacity(popup_len);
        for (index, command) in popup_commands.into_iter().enumerate() {
            let command_id = command.id.clone();
            let run_id = command.id.clone();
            let row_context_id = command.id.clone();
            let row_context_target = popup_context_for_rows.clone();
            let is_highlighted = index == highlight;
            let badge_mode = command_execution_badge_mode(&command);

            // Keep every icon and label strictly left-aligned on the same vertical
            // column; pinned status sits quietly at the trailing edge, and full
            // script preview opens on hover rather than crowding the label.
            let row_trigger = div()
                .id(SharedString::from(format!(
                    "quick-command-bar-popup-row-{command_id}"
                )))
                .debug_selector({
                    let id = command_id.clone();
                    move || format!("quick-command-bar-popup-row-{id}")
                })
                .h(px(30.))
                .w_full()
                .px_2()
                .rounded_md()
                .flex()
                .items_center()
                .gap_2()
                .cursor_pointer()
                .bg(if is_highlighted {
                    rgb(palette.hover)
                } else {
                    rgba(0x00000000)
                })
                .capture_any_mouse_down(move |event, _, _| {
                    if event.button == MouseButton::Right {
                        *row_context_target.borrow_mut() = Some(row_context_id.clone());
                    }
                })
                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                    if *hovered
                        && this
                            .commands
                            .set_quick_command_bar_popup_highlight(index, popup_len)
                    {
                        cx.notify();
                    }
                }))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.commands.close_quick_command_bar_popup();
                    this.run_quick_command_by_id(run_id.clone(), cx);
                    this.focus_active_workspace_surface(window, cx);
                    cx.notify();
                }))
                .child(quick_command_icon_slot(
                    15.,
                    quick_command_icon_mark(
                        palette,
                        command.icon_tag.as_deref(),
                        command.color_tag.as_deref(),
                        12.5,
                    ),
                ))
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .text_xs()
                        .font_weight(FontWeight(500.))
                        .text_color(rgb(palette.text))
                        .truncate()
                        .child(command.label.clone()),
                )
                .when(command.pinned.unwrap_or_default(), |this| {
                    this.child(quick_command_pin_mark(palette, 10.))
                });

            let hover_row = NyaHoverCard::new(
                format!("quick-command-bar-popup-hover-{command_id}"),
                row_trigger,
                quick_command_hover_card(
                    palette,
                    popover_bg,
                    &command,
                    badge_mode,
                    cx.entity().downgrade(),
                ),
            )
            .placement(NyaPopoverPlacement::Left)
            .appearance(false)
            .into_any_element();

            list_rows.push(div().w_full().child(hover_row).into_any_element());
        }

        let search_header = div()
            .p_2()
            .border_b_1()
            .border_color(rgb(palette.border))
            .child(
                div()
                    .h(px(28.))
                    .w_full()
                    .rounded_md()
                    .bg(rgb(palette.surface_elevated))
                    .flex()
                    .items_center()
                    .child(
                        NyaSearchInput::new("quick-command-bar-popup-search", &search_field)
                            .compact()
                            .bare()
                            .on_key_down(cx.listener(
                                move |this, event: &KeyDownEvent, window, cx| {
                                    this.handle_quick_command_bar_popup_key_down(
                                        event,
                                        popup_len,
                                        highlighted_id.as_deref(),
                                        window,
                                        cx,
                                    );
                                },
                            ))
                            .map(|input| match clear_search {
                                Some(clear) => input.trailing(clear),
                                None => input,
                            }),
                    ),
            );

        let popup_list_area = NyaContextMenu::new_dynamic(
            div()
                .id("quick-command-bar-popup-list-pane")
                .capture_any_mouse_down(move |event, _, _| {
                    if event.button == MouseButton::Right {
                        *popup_context_for_reset.borrow_mut() = None;
                    }
                })
                .relative()
                .min_h_0()
                .vertical_scrollbar(&popup_scroll)
                .child(
                    div()
                        .id("quick-command-bar-popup-list")
                        .max_h(px(COMMAND_BAR_POPUP_LIST_MAX_HEIGHT_PX))
                        .min_h_0()
                        .overflow_y_scroll()
                        .track_scroll(&popup_scroll)
                        .px(px(6.))
                        .py(px(4.))
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .when(popup_len == 0, |list| {
                            list.child(
                                div()
                                    .py_4()
                                    .text_center()
                                    .text_xs()
                                    .text_color(rgb(palette.text_dimmed))
                                    .child(t!("quickCommands.noCommandsFound")),
                            )
                        })
                        .children(list_rows),
                ),
            move |_, cx| {
                popup_menu_app
                    .update(cx, |app, cx| {
                        app.quick_command_extended_context_menu_items(
                            popup_context_target.borrow().clone(),
                            cx,
                        )
                    })
                    .unwrap_or_default()
            },
        )
        .min_width(px(QUICK_COMMAND_MENU_WIDTH));

        deferred(
            anchored()
                .anchor(Anchor::BottomRight)
                .offset(point(px(0.), px(-18.)))
                .snap_to_window_with_margin(px(8.))
                .child(
                    div()
                        .id("quick-command-bar-popup")
                        .debug_selector(|| "quick-command-bar-popup".to_string())
                        .occlude()
                        .w(px(312.))
                        .rounded_lg()
                        .border_1()
                        .border_color(rgb(palette.border))
                        .bg(popover_bg)
                        .shadow_lg()
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                            cx.stop_propagation();
                        })
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            if this.commands.close_quick_command_bar_popup() {
                                cx.notify();
                            }
                        }))
                        .child(search_header)
                        .child(
                            div()
                                .px_3()
                                .pt(px(6.))
                                .pb(px(2.))
                                .text_size(px(10.))
                                .font_weight(FontWeight(600.))
                                .text_color(rgb(palette.text_dimmed))
                                .child(section_title),
                        )
                        .child(popup_list_area)
                        .child(
                            div()
                                .border_t_1()
                                .border_color(rgb(palette.border))
                                .p(px(6.))
                                .child(
                                    div()
                                        .id("quick-command-bar-manage")
                                        .debug_selector(|| "quick-command-bar-manage".to_string())
                                        .h(px(28.))
                                        .w_full()
                                        .px_2()
                                        .rounded_md()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .text_xs()
                                        .font_weight(FontWeight(500.))
                                        .text_color(rgb(palette.text_muted))
                                        .cursor_pointer()
                                        .hover(move |this| {
                                            this.bg(rgb(palette.hover))
                                                .text_color(rgb(palette.text))
                                        })
                                        .child(
                                            svg()
                                                .size(px(13.))
                                                .flex_none()
                                                .path("icons/view-grid.svg")
                                                .text_color(rgb(palette.text_muted)),
                                        )
                                        .child(t!("quickCommands.manageCommands"))
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            let target_mode =
                                                this.commands.quick_last_bottom_view_mode();
                                            this.commands.close_quick_command_bar_popup();
                                            this.set_quick_command_view_mode_in_window(
                                                target_mode,
                                                window,
                                                cx,
                                            );
                                        })),
                                ),
                        ),
                ),
        )
        .with_priority(APP_OVERLAY_PRIORITY)
        .into_any_element()
    }

    fn handle_quick_command_bar_popup_key_down(
        &mut self,
        event: &KeyDownEvent,
        popup_len: usize,
        highlighted_id: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.as_str();
        match key {
            "down" | "up" => {
                cx.stop_propagation();
                if self
                    .commands
                    .move_quick_command_bar_popup_highlight(key == "down", popup_len)
                {
                    cx.notify();
                }
            }
            "enter" => {
                cx.stop_propagation();
                if let Some(id) = highlighted_id {
                    self.commands.close_quick_command_bar_popup();
                    self.run_quick_command_by_id(id.to_string(), cx);
                    self.focus_active_workspace_surface(window, cx);
                    cx.notify();
                }
            }
            "escape" => {
                cx.stop_propagation();
                if !self.commands.quick_search_draft().trim().is_empty() {
                    self.commands.set_quick_search_draft(String::new());
                    self.reset_text_input("quick-command.search", "", cx);
                } else {
                    self.commands.close_quick_command_bar_popup();
                    self.focus_active_workspace_surface(window, cx);
                }
                cx.notify();
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use nyaterm_core::QuickCommand;

    use super::{estimate_command_bar_chip_width, plan_command_bar_layout};

    fn sample_command(id: &str, label: &str, pinned: bool) -> QuickCommand {
        QuickCommand {
            id: id.to_string(),
            label: label.to_string(),
            command: "uptime".to_string(),
            category_id: None,
            description: None,
            color_tag: None,
            icon_tag: None,
            pinned: pinned.then_some(true),
            execution_mode: None,
            segmented_send: None,
            raw_send: None,
            source: None,
            risk_level: None,
            updated_at: None,
            created_at: None,
            use_count: None,
            sort_order: None,
            extra: Default::default(),
        }
    }

    #[test]
    fn command_bar_plan_fits_exact_chips_and_squeezes_trailing_chip_when_space_permits() {
        let commands: Vec<QuickCommand> = (0..10)
            .map(|index| {
                sample_command(
                    &format!("c-{index}"),
                    &format!("查看集群节点状态-{index}"),
                    index < 2,
                )
            })
            .collect();

        assert_eq!(plan_command_bar_layout(&[], 500.0).visible_count, 0);
        assert_eq!(plan_command_bar_layout(&commands, 0.0).visible_count, 0);

        let w0 = estimate_command_bar_chip_width(&commands[0]);
        let w1 = estimate_command_bar_chip_width(&commands[1]);
        let w2 = estimate_command_bar_chip_width(&commands[2]);

        // Exact budget for 3 full chips + small remainder (< 86px): keeps 3 without trailing cap.
        let exact_three = w0 + 4.0 + w1 + 4.0 + w2 + 20.0;
        let plan_three = plan_command_bar_layout(&commands, exact_three);
        assert_eq!(plan_three.visible_count, 3);
        assert!(plan_three.trailing_max_width.is_none());

        // Budget with ~100px left after 3 chips (< full w3, >= 86px): fits a 4th capped chip
        // instead of leaving a 100px blank gap before `» N`.
        let with_trailing = w0 + 4.0 + w1 + 4.0 + w2 + 4.0 + 100.0 + 6.0;
        let plan_four = plan_command_bar_layout(&commands, with_trailing);
        assert_eq!(plan_four.visible_count, 4);
        assert_eq!(plan_four.trailing_max_width, Some(100.0));

        // Large width fits all 10 with zero overflow.
        let plan_all = plan_command_bar_layout(&commands, 2400.0);
        assert_eq!(plan_all.visible_count, 10);
        assert!(plan_all.trailing_max_width.is_none());
    }

    #[test]
    fn command_bar_plan_keeps_one_truncated_chip_in_narrow_non_zero_strip() {
        let long = vec![
            sample_command("long-1", "列出全部工作负载与容器镜像仓库标签详情", true),
            sample_command("long-2", "扫描默认网关", false),
        ];
        let plan = plan_command_bar_layout(&long, 96.0);
        assert_eq!(plan.visible_count, 1);
        assert!(plan.trailing_max_width.is_some());
    }
}
