use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
};

use gpui::{
    AnyElement, AppContext as _, Context, FontWeight, IntoElement, KeyDownEvent, MouseButton,
    SharedString, div, prelude::*, px, rgb, rgba, svg,
};
use nyaterm_core::{QuickCommand, QuickCommandCategory};
use nyaterm_ui::{
    NyaContextMenu, NyaDropdownMenu, NyaHoverCard, NyaMenuItem, NyaPopoverPlacement, NyaScrollable,
    NyaSearchInput, NyaTooltip,
};
use rust_i18n::t;

use super::super::super::{
    filtered_quick_commands, ordered_quick_command_categories, quick_command_icon_mark,
};
use super::super::detail_card::{QuickCommandCardExecutionMode, quick_command_hover_card};
use super::rows::{QUICK_COMMAND_MENU_WIDTH, quick_command_icon_slot};
use super::{
    QuickCommandDragKind, QuickCommandDragPayload, QuickCommandDragPreview,
    quick_command_sort_menu_button_with_id, quick_command_toolbar_icon_button,
    quick_command_view_menu_button_with_id,
};
use crate::features::{
    NyaTermApp,
    commands::{QuickCommandDropPosition, QuickCommandDropTarget},
    text_inputs::TextInputSetup,
};
use crate::models::{NavItem, PanelSide, QuickCommandSortMode};
use crate::widgets::small_button;

pub(super) const PINNED_VIRTUAL_CATEGORY_ID: &str = "__pinned__";
const SIDEBAR_CATEGORY_ROW_HEIGHT_PX: f32 = 28.;
const SIDEBAR_COMMAND_ROW_HEIGHT_PX: f32 = 27.;

#[derive(Debug)]
pub(super) struct SidebarCategoryNode {
    pub id: String,
    pub label: String,
    pub depth: usize,
    pub count: usize,
    pub expanded: bool,
    pub commands: Vec<QuickCommand>,
}

#[derive(Debug)]
pub(super) struct SidebarTreeProjection {
    pub total_matched: usize,
    pub pinned: Option<SidebarCategoryNode>,
    pub categories: Vec<SidebarCategoryNode>,
    pub uncategorized: Option<SidebarCategoryNode>,
}

/// Builds the single-column IDE Explorer tree projection for Sidebar mode.
///
/// While `query` is non-empty, categories whose subtree contains matches are
/// automatically expanded and non-matching branches are hidden, without mutating
/// `collapsed_ids`. Clearing `query` restores `collapsed_ids` verbatim.
pub(super) fn build_sidebar_tree_projection(
    commands: &[QuickCommand],
    categories: &[QuickCommandCategory],
    query: &str,
    sort_mode: QuickCommandSortMode,
    collapsed_ids: &HashSet<String>,
    pinned_label: String,
    uncategorized_label: String,
) -> SidebarTreeProjection {
    let is_searching = !query.trim().is_empty();
    let matched_all = filtered_quick_commands(commands, categories, query, "all", sort_mode);
    let total_matched = matched_all.len();

    let pinned_commands: Vec<QuickCommand> = matched_all
        .iter()
        .filter(|cmd| cmd.pinned.unwrap_or_default())
        .cloned()
        .collect();
    let pinned = (!pinned_commands.is_empty()).then(|| SidebarCategoryNode {
        id: PINNED_VIRTUAL_CATEGORY_ID.to_string(),
        label: pinned_label,
        depth: 0,
        count: pinned_commands.len(),
        expanded: is_searching || !collapsed_ids.contains(PINNED_VIRTUAL_CATEGORY_ID),
        commands: pinned_commands,
    });

    let ordered = ordered_quick_command_categories(categories);
    let known_category_ids: HashSet<&str> =
        ordered.iter().map(|(cat, _)| cat.id.as_str()).collect();

    let mut direct_by_category: HashMap<&str, Vec<QuickCommand>> = HashMap::new();
    let mut uncategorized_commands = Vec::new();
    for command in &matched_all {
        match command
            .category_id
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty() && known_category_ids.contains(id))
        {
            Some(category_id) => {
                direct_by_category
                    .entry(category_id)
                    .or_default()
                    .push(command.clone());
            }
            None => uncategorized_commands.push(command.clone()),
        }
    }

    // Subtree counts: in depth-first order, a node's subtree is the contiguous
    // run of following entries with strictly greater depth.
    let mut subtree_counts = vec![0_usize; ordered.len()];
    for index in 0..ordered.len() {
        let (category, base_depth) = ordered[index];
        let mut count = direct_by_category
            .get(category.id.as_str())
            .map_or(0, Vec::len);
        for (child, child_depth) in ordered.iter().skip(index + 1) {
            if *child_depth <= base_depth {
                break;
            }
            count += direct_by_category
                .get(child.id.as_str())
                .map_or(0, Vec::len);
        }
        subtree_counts[index] = count;
    }

    let mut category_nodes = Vec::new();
    let mut collapsed_ancestor_depth: Option<usize> = None;
    for (index, (category, depth)) in ordered.iter().copied().enumerate() {
        if let Some(collapsed_depth) = collapsed_ancestor_depth {
            if depth > collapsed_depth {
                continue;
            }
            collapsed_ancestor_depth = None;
        }
        let subtree_count = subtree_counts[index];
        if is_searching && subtree_count == 0 {
            continue;
        }
        let expanded = is_searching || !collapsed_ids.contains(&category.id);
        if !expanded {
            collapsed_ancestor_depth = Some(depth);
        }
        let direct_commands = if expanded {
            direct_by_category
                .remove(category.id.as_str())
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        category_nodes.push(SidebarCategoryNode {
            id: category.id.clone(),
            label: category.name.clone(),
            depth,
            count: subtree_count,
            expanded,
            commands: direct_commands,
        });
    }

    let uncategorized = (!uncategorized_commands.is_empty()).then(|| {
        let expanded = is_searching || !collapsed_ids.contains("uncategorized");
        let count = uncategorized_commands.len();
        SidebarCategoryNode {
            id: "uncategorized".to_string(),
            label: uncategorized_label,
            depth: 0,
            count,
            expanded,
            commands: if expanded {
                uncategorized_commands
            } else {
                Vec::new()
            },
        }
    });

    SidebarTreeProjection {
        total_matched,
        pinned,
        categories: category_nodes,
        uncategorized,
    }
}

impl NyaTermApp {
    pub(in crate::features) fn quick_commands_sidebar_meta(&self) -> SharedString {
        let total = self.commands.quick_commands().len();
        if total == 0 {
            return SharedString::from("");
        }
        let query = self.commands.quick_search_draft().trim();
        if query.is_empty() {
            SharedString::from(total.to_string())
        } else {
            let visible = filtered_quick_commands(
                self.commands.quick_commands(),
                self.commands.quick_command_categories(),
                query,
                "all",
                self.commands.quick_sort_mode(),
            )
            .len();
            SharedString::from(format!("{visible}/{total}"))
        }
    }

    pub(in crate::features) fn quick_commands_sidebar_header_actions(
        &mut self,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = self.theme_palette();
        let more_app = cx.entity().downgrade();
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap_1()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(quick_command_toolbar_icon_button(
                palette,
                "quick-command-sidebar-add",
                "icons/conn/add.svg",
                false,
                t!("quickCommands.addCommand"),
                cx.listener(|this, _, window, cx| {
                    this.close_quick_command_toolbar_popovers();
                    this.open_new_quick_command_editor(window, cx);
                }),
            ))
            .child(quick_command_sort_menu_button_with_id(
                "quick-command-sidebar-sort",
                self.commands.quick_sort_mode(),
                cx,
            ))
            .child(quick_command_view_menu_button_with_id(
                "quick-command-sidebar-view",
                self.commands.quick_view_mode(),
                cx,
            ))
            .child(
                NyaDropdownMenu::new("quick-command-sidebar-more")
                    .icon("icons/session/more.svg")
                    .icon_size(px(14.))
                    .min_width(px(164.))
                    .items_dynamic(move |_, cx| {
                        more_app
                            .update(cx, |_, cx| {
                                vec![
                                    NyaMenuItem::action(t!("quickCommands.addCategory"))
                                        .icon("icons/fe/new-folder.svg")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.open_new_quick_command_category(None, window, cx);
                                        })),
                                    NyaMenuItem::separator(),
                                    NyaMenuItem::action(t!("quickCommands.import"))
                                        .icon("icons/import.svg")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.open_quick_command_import_dialog(window, cx);
                                        })),
                                    NyaMenuItem::action(t!("quickCommands.export"))
                                        .icon("icons/menu/export.svg")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.prompt_quick_command_export(cx);
                                        })),
                                ]
                            })
                            .unwrap_or_default()
                    }),
            )
            .into_any_element()
    }

    pub(in crate::features) fn quick_commands_sidebar_panel(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let card_surface = self.shell_surface_color(palette.surface);
        let side = self
            .panel_side_for_item(NavItem::QuickCommands)
            .unwrap_or(PanelSide::Right);
        let hover_placement = match side {
            PanelSide::Right => NyaPopoverPlacement::Left,
            PanelSide::Left => NyaPopoverPlacement::Right,
        };

        let search_draft = self.commands.quick_search_draft().to_string();
        let is_searching = !search_draft.trim().is_empty();
        let search_field = self.text_input(
            "quick-command.search",
            &search_draft,
            TextInputSetup::placeholder(t!("quickCommands.search")),
            cx,
        );
        let clear_search = is_searching.then(|| {
            div()
                .id("quick-command-sidebar-search-clear")
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

        let collapsed_ids: HashSet<String> = {
            let mut set = HashSet::new();
            if self
                .commands
                .quick_sidebar_category_is_collapsed(PINNED_VIRTUAL_CATEGORY_ID)
            {
                set.insert(PINNED_VIRTUAL_CATEGORY_ID.to_string());
            }
            if self
                .commands
                .quick_sidebar_category_is_collapsed("uncategorized")
            {
                set.insert("uncategorized".to_string());
            }
            for category in self.commands.quick_command_categories() {
                if self
                    .commands
                    .quick_sidebar_category_is_collapsed(&category.id)
                {
                    set.insert(category.id.clone());
                }
            }
            set
        };

        let projection = build_sidebar_tree_projection(
            self.commands.quick_commands(),
            self.commands.quick_command_categories(),
            &search_draft,
            self.commands.quick_sort_mode(),
            &collapsed_ids,
            t!("quickCommands.pinnedCommands").to_string(),
            t!("quickCommands.uncategorized").to_string(),
        );

        let context_target = Rc::new(RefCell::new(None::<String>));
        let context_target_for_rows = context_target.clone();
        let context_target_for_reset = context_target.clone();
        let menu_app = cx.entity().downgrade();
        let sidebar_scroll = self.commands.quick_sidebar_scroll().clone();

        let mut tree_elements: Vec<AnyElement> = Vec::new();
        if projection.total_matched == 0 && self.commands.quick_command_categories().is_empty() {
            tree_elements.push(
                div()
                    .mt_6()
                    .mx_auto()
                    .w_full()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(palette.border))
                    .p_4()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .text_xs()
                    .text_color(rgb(palette.text_muted))
                    .opacity(0.75)
                    .child(
                        svg()
                            .size(px(22.))
                            .flex_none()
                            .text_color(rgb(palette.text_muted))
                            .path("icons/conn/terminal.svg"),
                    )
                    .child(t!("quickCommands.noCommandsFound"))
                    .child(small_button(
                        palette,
                        "quick-command-sidebar-empty-add",
                        t!("quickCommands.addCommand"),
                        cx.listener(|this, _, window, cx| {
                            this.open_new_quick_command_editor(window, cx);
                        }),
                    ))
                    .into_any_element(),
            );
        } else if projection.total_matched == 0 && is_searching {
            tree_elements.push(
                div()
                    .py_6()
                    .text_center()
                    .text_xs()
                    .text_color(rgb(palette.text_dimmed))
                    .child(t!("quickCommands.noCommandsFound"))
                    .into_any_element(),
            );
        } else {
            if let Some(pinned_node) = projection.pinned {
                tree_elements.extend(self.render_sidebar_category_section(
                    pinned_node,
                    SidebarSectionKind::Pinned,
                    is_searching,
                    palette,
                    card_surface,
                    hover_placement,
                    &context_target_for_rows,
                    cx,
                ));
            }
            for category_node in projection.categories {
                tree_elements.extend(self.render_sidebar_category_section(
                    category_node,
                    SidebarSectionKind::RealCategory,
                    is_searching,
                    palette,
                    card_surface,
                    hover_placement,
                    &context_target_for_rows,
                    cx,
                ));
            }
            if let Some(uncategorized_node) = projection.uncategorized {
                tree_elements.extend(self.render_sidebar_category_section(
                    uncategorized_node,
                    SidebarSectionKind::Uncategorized,
                    is_searching,
                    palette,
                    card_surface,
                    hover_placement,
                    &context_target_for_rows,
                    cx,
                ));
            }
        }

        div()
            .id("quick-commands-sidebar")
            .debug_selector(|| "quick-commands-sidebar".to_string())
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(
                div()
                    .flex_none()
                    .px_2()
                    .py(px(6.))
                    .border_b_1()
                    .border_color(rgb(palette.border))
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h(px(28.))
                            .rounded_md()
                            .bg(rgb(palette.surface_elevated))
                            .flex()
                            .items_center()
                            .child(
                                NyaSearchInput::new("quick-command-sidebar-search", &search_field)
                                    .compact()
                                    .bare()
                                    .on_key_down(cx.listener(
                                        |this, event: &KeyDownEvent, _, cx| {
                                            if event.keystroke.key == "escape"
                                                && !this
                                                    .commands
                                                    .quick_search_draft()
                                                    .trim()
                                                    .is_empty()
                                            {
                                                cx.stop_propagation();
                                                this.commands.set_quick_search_draft(String::new());
                                                this.reset_text_input(
                                                    "quick-command.search",
                                                    "",
                                                    cx,
                                                );
                                                cx.notify();
                                            }
                                        },
                                    ))
                                    .map(|input| match clear_search {
                                        Some(clear) => input.trailing(clear),
                                        None => input,
                                    }),
                            ),
                    )
                    .when(self.commands.staged_send_progress().is_some(), |bar| {
                        bar.child(quick_command_toolbar_icon_button(
                            palette,
                            "quick-command-sidebar-stop",
                            "icons/session/stop.svg",
                            false,
                            t!("serialSend.stop"),
                            cx.listener(|this, _, _, cx| this.stop_quick_command_sends(cx)),
                        ))
                    }),
            )
            .child(
                NyaContextMenu::new_dynamic(
                    div()
                        .id("quick-command-sidebar-pane")
                        .capture_any_mouse_down(move |event, _, _| {
                            if event.button == MouseButton::Right {
                                *context_target_for_reset.borrow_mut() = None;
                            }
                        })
                        .relative()
                        .flex_1()
                        .min_h_0()
                        .w_full()
                        .vertical_scrollbar(&sidebar_scroll)
                        .child(
                            div()
                                .id("quick-command-sidebar-scroll")
                                .size_full()
                                .min_h_0()
                                .overflow_y_scroll()
                                .track_scroll(&sidebar_scroll)
                                .p(px(6.))
                                .flex()
                                .flex_col()
                                .gap(px(2.))
                                .children(tree_elements),
                        ),
                    move |_, cx| {
                        menu_app
                            .update(cx, |app, cx| {
                                app.quick_command_extended_context_menu_items(
                                    context_target.borrow().clone(),
                                    cx,
                                )
                            })
                            .unwrap_or_default()
                    },
                )
                .min_width(px(QUICK_COMMAND_MENU_WIDTH)),
            )
    }

    #[allow(clippy::too_many_arguments)]
    fn render_sidebar_category_section(
        &mut self,
        node: SidebarCategoryNode,
        kind: SidebarSectionKind,
        is_searching: bool,
        palette: crate::theme::ThemePalette,
        card_surface: gpui::Rgba,
        hover_placement: NyaPopoverPlacement,
        context_target: &Rc<RefCell<Option<String>>>,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut out = Vec::with_capacity(1 + node.commands.len());
        let category_id = node.id.clone();
        let toggle_id = node.id.clone();
        let expanded = node.expanded;
        let depth = node.depth;
        let chevron_icon = if expanded {
            "icons/chevron-down.svg"
        } else {
            "icons/menu/chevron-right.svg"
        };
        let group_icon = match kind {
            SidebarSectionKind::Pinned => "icons/pin.svg",
            SidebarSectionKind::RealCategory | SidebarSectionKind::Uncategorized => {
                if expanded {
                    "icons/session/folder-open.svg"
                } else {
                    "icons/conn/folder.svg"
                }
            }
        };

        let drop_position = (kind == SidebarSectionKind::RealCategory)
            .then(|| {
                self.commands
                    .quick_drop_target()
                    .filter(|target| cx.has_active_drag() && target.id == category_id)
                    .map(|target| target.position)
            })
            .flatten();

        let mut header_row = div()
            .id(SharedString::from(format!(
                "quick-command-sidebar-group-{category_id}"
            )))
            .debug_selector({
                let id = category_id.clone();
                move || format!("quick-command-sidebar-group-{id}")
            })
            .relative()
            .h(px(SIDEBAR_CATEGORY_ROW_HEIGHT_PX))
            .w_full()
            .flex_none()
            .pl(px(6. + depth as f32 * 12.))
            .pr_2()
            .rounded_md()
            .flex()
            .items_center()
            .gap(px(5.))
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
            .when(
                matches!(
                    drop_position,
                    Some(QuickCommandDropPosition::Before | QuickCommandDropPosition::After)
                ),
                |row| {
                    let before = drop_position == Some(QuickCommandDropPosition::Before);
                    row.child(
                        div()
                            .absolute()
                            .left(px(6. + depth as f32 * 12.))
                            .right_0()
                            .h(px(2.))
                            .bg(rgb(palette.link))
                            .when(before, |line| line.top_0())
                            .when(!before, |line| line.bottom_0()),
                    )
                },
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                if !is_searching {
                    this.commands
                        .toggle_quick_sidebar_category(toggle_id.clone());
                    cx.notify();
                }
            }))
            .child(
                svg()
                    .size(px(12.))
                    .flex_none()
                    .path(chevron_icon)
                    .text_color(rgb(palette.text_dimmed)),
            )
            .child(svg().size(px(13.)).flex_none().path(group_icon).text_color(
                if kind == SidebarSectionKind::Pinned {
                    rgb(palette.link)
                } else {
                    rgb(palette.text_muted)
                },
            ))
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .text_xs()
                    .font_weight(if kind == SidebarSectionKind::Pinned {
                        FontWeight(600.)
                    } else {
                        FontWeight(500.)
                    })
                    .text_color(rgb(palette.text))
                    .truncate()
                    .child(node.label.clone()),
            )
            .child(
                div()
                    .flex_none()
                    .rounded_sm()
                    .px(px(5.))
                    .py(px(1.))
                    .bg(rgb(palette.hover))
                    .text_size(px(10.))
                    .line_height(px(12.))
                    .text_color(rgb(palette.text_dimmed))
                    .child(node.count.to_string()),
            );

        if kind == SidebarSectionKind::RealCategory {
            let drag_id = category_id.clone();
            let drag_label = node.label.clone();
            let move_target = category_id.clone();
            let drop_target = category_id.clone();
            header_row = header_row
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
                    move |this, event: &gpui::DragMoveEvent<QuickCommandDragPayload>, _, cx| {
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
                .on_drop(
                    cx.listener(move |this, payload: &QuickCommandDragPayload, _, cx| {
                        cx.stop_propagation();
                        let position = this
                            .commands
                            .quick_drop_target()
                            .filter(|target| target.id == drop_target)
                            .map(|target| target.position)
                            .unwrap_or(QuickCommandDropPosition::Inside);
                        let config =
                            match payload.kind {
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
                    }),
                );
        } else if kind == SidebarSectionKind::Uncategorized {
            header_row = header_row.on_drop(cx.listener(
                |this, payload: &QuickCommandDragPayload, _, cx| {
                    cx.stop_propagation();
                    let config = (payload.kind == QuickCommandDragKind::Command)
                        .then(|| {
                            this.commands
                                .move_quick_command_to_category(&payload.id, None)
                        })
                        .flatten();
                    this.finish_quick_command_reorder(config, cx);
                },
            ));
        }

        let header_element = match kind {
            SidebarSectionKind::RealCategory => NyaContextMenu::new(
                header_row,
                self.quick_command_category_menu_items(category_id.clone(), cx),
            )
            .into_any_element(),
            SidebarSectionKind::Uncategorized | SidebarSectionKind::Pinned => NyaContextMenu::new(
                header_row,
                self.quick_command_pseudo_category_menu_items(cx),
            )
            .into_any_element(),
        };
        out.push(header_element);

        if expanded {
            let section_prefix = match kind {
                SidebarSectionKind::Pinned => "pinned",
                SidebarSectionKind::RealCategory => category_id.as_str(),
                SidebarSectionKind::Uncategorized => "uncategorized",
            };
            let command_indent = 20. + depth as f32 * 12.;
            for command in node.commands {
                out.push(self.render_sidebar_command_row(
                    command,
                    section_prefix,
                    command_indent,
                    palette,
                    card_surface,
                    hover_placement,
                    context_target,
                    cx,
                ));
            }
        }

        out
    }

    #[allow(clippy::too_many_arguments)]
    fn render_sidebar_command_row(
        &mut self,
        command: QuickCommand,
        section_prefix: &str,
        left_indent_px: f32,
        palette: crate::theme::ThemePalette,
        card_surface: gpui::Rgba,
        hover_placement: NyaPopoverPlacement,
        context_target: &Rc<RefCell<Option<String>>>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let command_id = command.id.clone();
        let run_id = command.id.clone();
        let action_run_id = command.id.clone();
        let edit_id = command.id.clone();
        let row_group = SharedString::from(format!(
            "quick-command-sidebar-row-group-{section_prefix}-{command_id}"
        ));

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
        let badge_mode = QuickCommandCardExecutionMode {
            append: execution_mode == "append",
            segmented: execution_mode == "segmented",
            label: badge_label,
        };

        let run_tooltip = t!("quickCommands.run").to_string();
        let edit_tooltip = t!("quickCommands.edit").to_string();

        let inner_row = div()
            .id(SharedString::from(format!(
                "quick-command-sidebar-item-{section_prefix}-{command_id}"
            )))
            .group(row_group.clone())
            .h(px(SIDEBAR_COMMAND_ROW_HEIGHT_PX))
            .w_full()
            .pl(px(left_indent_px))
            .pr(px(4.))
            .rounded_md()
            .flex()
            .items_center()
            .gap(px(6.))
            .hover(move |this| this.bg(rgb(palette.hover)))
            .child(
                div()
                    .id(SharedString::from(format!(
                        "quick-command-sidebar-run-area-{section_prefix}-{command_id}"
                    )))
                    .min_w_0()
                    .flex_1()
                    .h_full()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.run_quick_command_by_id(run_id.clone(), cx);
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
                            .flex_1()
                            .text_xs()
                            .text_color(rgb(palette.text))
                            .truncate()
                            .child(command.label.clone()),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .opacity(0.)
                    .group_hover(row_group, |style| style.opacity(1.))
                    .child(
                        div()
                            .id(SharedString::from(format!(
                                "quick-command-sidebar-run-btn-{section_prefix}-{command_id}"
                            )))
                            .size(px(20.))
                            .rounded_sm()
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover(move |this| this.bg(rgb(palette.surface_elevated)))
                            .tooltip(move |window, cx| {
                                NyaTooltip::new(run_tooltip.clone()).build(window, cx)
                            })
                            .child(
                                svg()
                                    .size(px(12.))
                                    .path("icons/send.svg")
                                    .text_color(rgb(palette.text_muted)),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.run_quick_command_by_id(action_run_id.clone(), cx);
                            })),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!(
                                "quick-command-sidebar-edit-btn-{section_prefix}-{command_id}"
                            )))
                            .size(px(20.))
                            .rounded_sm()
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover(move |this| this.bg(rgb(palette.surface_elevated)))
                            .tooltip(move |window, cx| {
                                NyaTooltip::new(edit_tooltip.clone()).build(window, cx)
                            })
                            .child(
                                svg()
                                    .size(px(12.))
                                    .path("icons/net/edit.svg")
                                    .text_color(rgb(palette.text_muted)),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.open_edit_quick_command_editor(edit_id.clone(), window, cx);
                            })),
                    ),
            );

        let hover_wrapped = NyaHoverCard::new(
            format!("quick-command-sidebar-hover-{section_prefix}-{command_id}"),
            inner_row,
            quick_command_hover_card(
                palette,
                card_surface,
                &command,
                badge_mode,
                cx.entity().downgrade(),
            ),
        )
        .placement(hover_placement)
        .appearance(false)
        .into_any_element();

        let drag_command_id = command.id.clone();
        let drag_command_label = command.label.clone();
        let move_target_id = command.id.clone();
        let drop_target_id = command.id.clone();
        let row_context_target = context_target.clone();
        let row_context_id = command_id.clone();
        let selector = format!("quick-command-sidebar-row-{section_prefix}-{command_id}");

        div()
            .id(SharedString::from(format!(
                "quick-command-sidebar-drag-{section_prefix}-{command_id}"
            )))
            .debug_selector(move || selector.clone())
            .w_full()
            .relative()
            .capture_any_mouse_down(move |event, _, _| {
                if event.button == MouseButton::Right {
                    *row_context_target.borrow_mut() = Some(row_context_id.clone());
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
                        .is_some_and(|target| target.position == QuickCommandDropPosition::After);
                    let config = (payload.kind == QuickCommandDragKind::Command)
                        .then(|| {
                            this.commands
                                .reorder_quick_command(&payload.id, &drop_target_id, after)
                        })
                        .flatten();
                    this.finish_quick_command_reorder(config, cx);
                }),
            )
            .child(hover_wrapped)
            .into_any_element()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SidebarSectionKind {
    Pinned,
    RealCategory,
    Uncategorized,
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use nyaterm_core::{QuickCommand, QuickCommandCategory};

    use super::{PINNED_VIRTUAL_CATEGORY_ID, build_sidebar_tree_projection};
    use crate::models::QuickCommandSortMode;

    fn category(id: &str, name: &str, parent_id: Option<&str>, order: i32) -> QuickCommandCategory {
        QuickCommandCategory {
            id: id.to_string(),
            name: name.to_string(),
            parent_id: parent_id.map(ToOwned::to_owned),
            sort_order: order,
        }
    }

    fn command(
        id: &str,
        label: &str,
        script: &str,
        category_id: Option<&str>,
        pinned: bool,
    ) -> QuickCommand {
        QuickCommand {
            id: id.to_string(),
            label: label.to_string(),
            command: script.to_string(),
            category_id: category_id.map(ToOwned::to_owned),
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
    fn virtual_pinned_group_appears_only_when_pinned_exists_and_keeps_category_membership() {
        let categories = vec![category("docker", "Docker", None, 0)];
        let unpinned_only = vec![command(
            "c1",
            "列出镜像",
            "docker images",
            Some("docker"),
            false,
        )];
        let without_pinned = build_sidebar_tree_projection(
            &unpinned_only,
            &categories,
            "",
            QuickCommandSortMode::Name,
            &HashSet::new(),
            "Pinned".to_string(),
            "Uncategorized".to_string(),
        );
        assert!(without_pinned.pinned.is_none());
        assert_eq!(without_pinned.categories.len(), 1);
        assert_eq!(without_pinned.categories[0].commands.len(), 1);

        let with_pinned = vec![
            command("c1", "列出镜像", "docker images", Some("docker"), true),
            command("c2", "磁盘 IO", "iostat -xz 1", None, false),
        ];
        let projected = build_sidebar_tree_projection(
            &with_pinned,
            &categories,
            "",
            QuickCommandSortMode::Name,
            &HashSet::new(),
            "Pinned".to_string(),
            "Uncategorized".to_string(),
        );
        let pinned = projected.pinned.expect("virtual pinned group");
        assert_eq!(pinned.id, PINNED_VIRTUAL_CATEGORY_ID);
        assert_eq!(pinned.count, 1);
        assert_eq!(pinned.commands[0].id, "c1");
        // Original Docker category still retains c1.
        assert_eq!(projected.categories[0].count, 1);
        assert_eq!(projected.categories[0].commands[0].id, "c1");
        assert_eq!(projected.uncategorized.expect("uncategorized").count, 1);
    }

    #[test]
    fn search_expands_matching_branches_and_clearing_search_restores_collapsed_state() {
        let categories = vec![
            category("docker", "Docker", None, 0),
            category("docker-compose", "Compose", Some("docker"), 0),
            category("k8s", "Kubernetes", None, 1),
        ];
        let commands = vec![
            command(
                "c1",
                "重启服务",
                "docker compose restart",
                Some("docker-compose"),
                false,
            ),
            command("c2", "查看 Pod", "kubectl get pods", Some("k8s"), false),
        ];
        let collapsed = HashSet::from(["docker".to_string(), "k8s".to_string()]);

        // Idle: collapsed categories keep their children hidden.
        let idle = build_sidebar_tree_projection(
            &commands,
            &categories,
            "",
            QuickCommandSortMode::Name,
            &collapsed,
            "Pinned".to_string(),
            "Uncategorized".to_string(),
        );
        assert_eq!(idle.categories.len(), 2);
        assert!(!idle.categories[0].expanded);
        assert_eq!(idle.categories[0].count, 1); // Subtree count includes Compose child.
        assert!(idle.categories[0].commands.is_empty());

        // Searching "compose": expands Docker -> Compose and hides non-matching Kubernetes,
        // without mutating `collapsed`.
        let searching = build_sidebar_tree_projection(
            &commands,
            &categories,
            "compose",
            QuickCommandSortMode::Name,
            &collapsed,
            "Pinned".to_string(),
            "Uncategorized".to_string(),
        );
        assert_eq!(searching.categories.len(), 2);
        assert_eq!(searching.categories[0].id, "docker");
        assert!(searching.categories[0].expanded);
        assert_eq!(searching.categories[1].id, "docker-compose");
        assert!(searching.categories[1].expanded);
        assert_eq!(searching.categories[1].commands.len(), 1);

        // Exiting search with the same `collapsed` set restores collapsed state.
        let restored = build_sidebar_tree_projection(
            &commands,
            &categories,
            "",
            QuickCommandSortMode::Name,
            &collapsed,
            "Pinned".to_string(),
            "Uncategorized".to_string(),
        );
        assert_eq!(restored.categories.len(), 2);
        assert!(!restored.categories[0].expanded);
        assert!(!restored.categories[1].expanded);
    }
}
