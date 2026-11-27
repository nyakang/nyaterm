use rust_i18n::t;

use std::{borrow::Cow, cell::RefCell, rc::Rc};

use gpui::{
    Anchor, AnyElement, App, ClickEvent, Context, FontWeight, IntoElement, KeyDownEvent,
    MouseButton, Point, Render, SharedString, Window, anchored, deferred, div, point, prelude::*,
    px, rgb, rgba, svg, uniform_list,
};

use super::super::{filtered_quick_commands, quick_command_category_options};
use crate::features::{
    NyaTermApp, text_inputs::TextInputSetup, view_widgets::APP_OVERLAY_PRIORITY,
};
use crate::models::{QuickCommandSortMode, QuickCommandViewMode};
use crate::widgets::small_button;
use nyaterm_ui::{
    NyaContextMenu, NyaDropdownMenu, NyaMenuItem, NyaScrollable, NyaSearchInput, NyaTooltip,
};

mod command_bar;
mod rows;
mod sidebar;
mod sidebar_tree;

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum QuickCommandDragKind {
    Command,
    Category,
}

#[derive(Clone, Debug)]
pub(super) struct QuickCommandDragPayload {
    pub kind: QuickCommandDragKind,
    pub id: String,
    pub label: String,
}

pub(super) struct QuickCommandDragPreview {
    pub payload: QuickCommandDragPayload,
    pub position: Point<gpui::Pixels>,
}

impl Render for QuickCommandDragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let icon = match self.payload.kind {
            QuickCommandDragKind::Command => "icons/conn/terminal.svg",
            QuickCommandDragKind::Category => "icons/conn/folder.svg",
        };
        div()
            .absolute()
            .left(self.position.x - px(88.))
            .top(self.position.y - px(16.))
            .w(px(196.))
            .h(px(34.))
            .px_3()
            .flex()
            .items_center()
            .gap_2()
            .rounded_md()
            .border_1()
            .border_color(rgb(0x388bfd))
            .bg(rgba(0x0d1117ee))
            .shadow_lg()
            .child(svg().size(px(13.)).path(icon).text_color(rgb(0x58a6ff)))
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .text_xs()
                    .font_weight(FontWeight(600.))
                    .text_color(rgb(0xe5edf7))
                    .overflow_hidden()
                    .child(nyaterm_core::truncate_preview(&self.payload.label, 24)),
            )
    }
}

#[derive(Clone, Copy)]
struct QuickCommandToolbarContext {
    palette: crate::theme::ThemePalette,
    popover_bg: gpui::Rgba,
}

struct QuickCommandSortMenuConfig {
    current: QuickCommandSortMode,
    sort_label: Cow<'static, str>,
    created_label: Cow<'static, str>,
    name_label: Cow<'static, str>,
    usage_label: Cow<'static, str>,
    custom_label: Cow<'static, str>,
}

struct QuickCommandViewMenuConfig {
    id: &'static str,
    current: QuickCommandViewMode,
    icon_path: &'static str,
    view_label: Cow<'static, str>,
    list_label: Cow<'static, str>,
    compact_label: Cow<'static, str>,
    tile_label: Cow<'static, str>,
    command_bar_label: Cow<'static, str>,
    sidebar_label: Cow<'static, str>,
}

struct QuickCommandAiPopoverConfig {
    open: bool,
    prompt: String,
    prompt_input: AnyElement,
    button_label: Cow<'static, str>,
    generate_label: Cow<'static, str>,
}

impl NyaTermApp {
    pub(in crate::features) fn quick_commands_panel(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let filtered_commands = filtered_quick_commands(
            self.commands.quick_commands(),
            self.commands.quick_command_categories(),
            self.commands.quick_search_draft(),
            self.commands.quick_selected_category(),
            self.commands.quick_sort_mode(),
        );
        let total_commands = self.commands.quick_commands().len();
        let visible_commands = filtered_commands.len();
        let _pinned_commands = self
            .commands
            .quick_commands()
            .iter()
            .filter(|command| command.pinned.unwrap_or_default())
            .count();
        let categories = quick_command_category_options(
            self.commands.quick_commands(),
            self.commands.quick_command_categories(),
            t!("quickCommands.allCategories"),
            t!("quickCommands.uncategorized"),
        );
        let palette = self.theme_palette();
        let popover_bg = self.shell_surface_color(palette.surface);
        let toolbar_context = QuickCommandToolbarContext {
            palette,
            popover_bg,
        };
        let search_draft = self.commands.quick_search_draft().to_string();
        let ai_prompt_draft = self.commands.quick_ai_prompt_draft().to_string();
        let search_field = self.text_input(
            "quick-command.search",
            &search_draft,
            TextInputSetup::placeholder(t!("quickCommands.search")),
            cx,
        );
        let ai_prompt_input = self
            .text_input_box(
                "quick-command.ai-prompt",
                &ai_prompt_draft,
                TextInputSetup::placeholder(t!("ai.placeholder")),
                cx,
            )
            .into_any_element();
        // Tauri shows a clear button inside the box while a query is present. It
        // clears only the query; Escape below still drops the category filter too.
        let clear_search = (!search_draft.trim().is_empty()).then(|| {
            div()
                .id(SharedString::from("quick-command-search-clear"))
                .size(px(16.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded_sm()
                .cursor_pointer()
                .hover(|this| this.bg(rgb(palette.hover)))
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
        let view_icon = self.commands.quick_view_mode().icon_path();

        let category_sidebar = self.quick_command_category_sidebar(categories, palette, cx);
        let view_mode = self.commands.quick_view_mode();
        let row_scroll = self.commands.quick_row_scroll().clone();
        // Row pitch for the virtualized modes: Tauri's row height plus its gap-1.5.
        let logical_row_height = match view_mode {
            QuickCommandViewMode::Compact => 38.,
            QuickCommandViewMode::List
            | QuickCommandViewMode::Tile
            | QuickCommandViewMode::CommandBar
            | QuickCommandViewMode::Sidebar => 50.,
        };
        // The pane owns one menu; capture resets its target before a row aims it.
        let context_target = Rc::new(RefCell::new(None::<String>));
        let context_target_for_rows = context_target.clone();
        let context_target_for_reset = context_target.clone();
        let menu_app = cx.entity().downgrade();
        let rows = if filtered_commands.is_empty() {
            div()
                .flex_1()
                .min_h_0()
                .child(
                    div()
                        .mt_8()
                        .mx_auto()
                        .w_full()
                        .max_w(px(384.))
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
                        .line_height(px(18.))
                        .opacity(0.72)
                        .child(
                            svg()
                                .size(px(24.))
                                .flex_none()
                                .text_color(rgb(palette.text_muted))
                                .path("icons/conn/terminal.svg"),
                        )
                        .child(t!("quickCommands.noCommandsFound"))
                        .child(
                            div()
                                .debug_selector(|| "quick-command-empty-add".to_string())
                                .child(small_button(
                                    palette,
                                    "quick-command-empty-add",
                                    t!("quickCommands.addCommand"),
                                    cx.listener(|this, _, window, cx| {
                                        this.open_new_quick_command_editor(window, cx);
                                    }),
                                )),
                        ),
                )
                .into_any_element()
        } else if view_mode == QuickCommandViewMode::Tile {
            // Tauri lays tiles out with `flex flex-wrap`: chips keep their content
            // width and wrap when the row fills. That cannot be expressed as
            // uniform_list rows without guessing how many chips fit, and a wrong
            // guess is exactly the ragged padding this replaces.
            let tiles =
                self.quick_command_items(&filtered_commands, palette, &context_target_for_rows, cx);
            div()
                .id(SharedString::from("quick-command-tiles-scroll"))
                .flex_1()
                .min_h_0()
                .flex()
                .flex_wrap()
                .content_start()
                .gap(px(6.))
                .children(tiles)
                // In-flow container, so it owns its own bar; `quick_row_scroll` is a
                // UniformListScrollHandle that only uniform_list can drive.
                .overflow_y_scrollbar()
                .into_any_element()
        } else {
            let logical_row_count = filtered_commands.len();
            uniform_list(
                "quick-command-rows-scroll",
                logical_row_count,
                cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                    let mut rows = Vec::with_capacity(range.len());
                    for index in range {
                        let Some(command) = filtered_commands.get(index) else {
                            continue;
                        };
                        let command_items = this.quick_command_items(
                            std::slice::from_ref(command),
                            palette,
                            &context_target_for_rows,
                            cx,
                        );
                        rows.push(
                            div()
                                .h(px(logical_row_height))
                                .w_full()
                                .flex_none()
                                .flex()
                                .items_start()
                                .children(command_items),
                        );
                    }
                    rows
                }),
            )
            .flex_1()
            .min_h_0()
            .track_scroll(&row_scroll)
            .into_any_element()
        };

        // Tauri QuickCommands: PanelHeader-like strip with search + compact actions,
        // then category sidebar + command list (no page metrics cards).
        div()
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(self.shell_transparent_color(palette.surface))
            .child(
                div()
                    .h(px(36.))
                    .flex_none()
                    .px_2()
                    .border_b_1()
                    .border_color(rgb(palette.border))
                    .bg(self.shell_transparent_color(palette.section_header))
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(11.))
                            .font_weight(FontWeight(700.))
                            .text_color(rgb(palette.text_muted))
                            .child(t!("panel.quickCommands")),
                    )
                    .when(total_commands > 0, |this| {
                        this.child(
                            div()
                                .text_size(px(11.))
                                .text_color(rgb(palette.text_dimmed))
                                .child(if visible_commands == total_commands {
                                    total_commands.to_string()
                                } else {
                                    format!("{visible_commands}/{total_commands}")
                                }),
                        )
                    })
                    .child(div().flex_1())
                    .child(
                        div()
                            .w(px(144.))
                            .h(px(28.))
                            .rounded_md()
                            .bg(rgb(palette.surface_elevated))
                            .flex()
                            .items_center()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.close_quick_command_toolbar_popovers();
                                    cx.notify();
                                }),
                            )
                            .child(
                                NyaSearchInput::new("quick-command-search-input", &search_field)
                                    .compact()
                                    .bare()
                                    .on_key_down(cx.listener(
                                        |this, event: &KeyDownEvent, _, cx| {
                                            if event.keystroke.key == "escape" {
                                                cx.stop_propagation();
                                                this.commands.clear_quick_filters();
                                                this.sync_quick_command_selected_category(cx);
                                                this.reset_text_input(
                                                    "quick-command.search",
                                                    "",
                                                    cx,
                                                );
                                                this.shell.set_status(
                                                    "quick command filters cleared".to_string(),
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
                    .child(quick_command_toolbar_divider(palette))
                    .child(quick_command_sort_menu_button(
                        QuickCommandSortMenuConfig {
                            current: self.commands.quick_sort_mode(),
                            sort_label: t!("quickCommands.sort"),
                            created_label: t!("quickCommands.sortByCreated"),
                            name_label: t!("quickCommands.sortByName"),
                            usage_label: t!("quickCommands.sortByUseCount"),
                            custom_label: t!("quickCommands.sortCustom"),
                        },
                        cx,
                    ))
                    .child(quick_command_view_menu_button(
                        QuickCommandViewMenuConfig {
                            id: "quick-command-view",
                            current: self.commands.quick_view_mode(),
                            icon_path: view_icon,
                            view_label: t!("quickCommands.viewMode"),
                            list_label: t!("quickCommands.listMode"),
                            compact_label: t!("quickCommands.compactListMode"),
                            tile_label: t!("quickCommands.tileMode"),
                            command_bar_label: t!("quickCommands.commandBarMode"),
                            sidebar_label: t!("quickCommands.sidebarMode"),
                        },
                        cx,
                    ))
                    .child(quick_command_toolbar_divider(palette))
                    .when(self.commands.staged_send_progress().is_some(), |toolbar| {
                        toolbar
                            .child(quick_command_toolbar_icon_button(
                                palette,
                                "quick-command-stop",
                                "icons/session/stop.svg",
                                false,
                                t!("serialSend.stop"),
                                cx.listener(|this, _, _, cx| this.stop_quick_command_sends(cx)),
                            ))
                            .child(div().text_size(px(11.)).child(t!(
                                "serialSend.shellProgressCompleted",
                                count = self.commands.staged_send_progress().unwrap_or_default()
                            )))
                    })
                    .child(quick_command_toolbar_icon_button(
                        palette,
                        "quick-command-add",
                        "icons/conn/add.svg",
                        false,
                        t!("quickCommands.addCommand"),
                        cx.listener(|this, _, window, cx| {
                            this.close_quick_command_toolbar_popovers();
                            this.open_new_quick_command_editor(window, cx);
                        }),
                    ))
                    .child(quick_command_toolbar_icon_button(
                        palette,
                        "quick-command-import",
                        "icons/import.svg",
                        self.commands.quick_import_path_prompt().is_some(),
                        t!("quickCommands.import"),
                        cx.listener(|this, _, window, cx| {
                            this.close_quick_command_toolbar_popovers();
                            this.open_quick_command_import_dialog(window, cx);
                        }),
                    ))
                    .child(quick_command_toolbar_icon_button(
                        palette,
                        "quick-command-export",
                        "icons/menu/export.svg",
                        false,
                        t!("quickCommands.export"),
                        cx.listener(|this, _, _, cx| {
                            this.close_quick_command_toolbar_popovers();
                            this.prompt_quick_command_export(cx);
                        }),
                    ))
                    .child(quick_command_toolbar_divider(palette))
                    .child(quick_command_ai_popover_button(
                        toolbar_context,
                        QuickCommandAiPopoverConfig {
                            open: self.commands.quick_ai_popover_is_open(),
                            prompt: self.commands.quick_ai_prompt_draft().to_string(),
                            prompt_input: ai_prompt_input,
                            button_label: t!("ai.generateCommand"),
                            generate_label: t!("ai.generate"),
                        },
                        cx,
                    )),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            let changed = this.commands.close_quick_toolbar_popovers();
                            if changed {
                                cx.notify();
                            }
                        }),
                    )
                    .child(category_sidebar)
                    .child(
                        NyaContextMenu::new_dynamic(
                            div()
                                .id("quick-command-pane")
                                .debug_selector(|| "quick-command-pane".to_string())
                                .capture_any_mouse_down(move |event, _, _| {
                                    if event.button == MouseButton::Right {
                                        *context_target_for_reset.borrow_mut() = None;
                                    }
                                })
                                .min_w_0()
                                .flex_1()
                                .min_h_0()
                                .h_full()
                                .p(px(6.))
                                .relative()
                                .flex()
                                .flex_col()
                                .child(rows)
                                // Tile mode scrolls in-flow with its own bar; attaching this
                                // one too would paint a second, idle track on the same edge.
                                .when(view_mode != QuickCommandViewMode::Tile, |this| {
                                    this.vertical_scrollbar(&row_scroll)
                                }),
                            move |_, cx| {
                                menu_app
                                    .update(cx, |app, cx| {
                                        app.quick_command_context_menu_items(
                                            context_target.borrow().clone(),
                                            cx,
                                        )
                                    })
                                    .unwrap_or_default()
                            },
                        )
                        .min_width(px(rows::QUICK_COMMAND_MENU_WIDTH)),
                    ),
            )
    }
}

pub(super) fn quick_command_sort_menu_button_with_id(
    id: &'static str,
    current: QuickCommandSortMode,
    cx: &mut Context<NyaTermApp>,
) -> impl IntoElement {
    let sort_label = t!("quickCommands.sort");
    let created_label = t!("quickCommands.sortByCreated");
    let name_label = t!("quickCommands.sortByName");
    let usage_label = t!("quickCommands.sortByUseCount");
    let custom_label = t!("quickCommands.sortCustom");
    NyaDropdownMenu::new(id)
        .icon("icons/conn/sort.svg")
        .icon_size(px(14.))
        .selected(current != QuickCommandSortMode::Created)
        .tooltip(format!(
            "{} · {}",
            sort_label,
            quick_command_sort_mode_label(
                current,
                created_label.clone(),
                name_label.clone(),
                usage_label.clone(),
                custom_label.clone(),
            )
        ))
        .min_width(px(154.))
        .items([
            NyaMenuItem::action(created_label)
                .checked(current == QuickCommandSortMode::Created)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.set_quick_command_sort_mode(QuickCommandSortMode::Created, cx);
                })),
            NyaMenuItem::action(name_label)
                .checked(current == QuickCommandSortMode::Name)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.set_quick_command_sort_mode(QuickCommandSortMode::Name, cx);
                })),
            NyaMenuItem::action(usage_label)
                .checked(current == QuickCommandSortMode::Usage)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.set_quick_command_sort_mode(QuickCommandSortMode::Usage, cx);
                })),
            NyaMenuItem::action(custom_label)
                .checked(current == QuickCommandSortMode::Custom)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.set_quick_command_sort_mode(QuickCommandSortMode::Custom, cx);
                })),
        ])
}

fn quick_command_sort_menu_button(
    config: QuickCommandSortMenuConfig,
    cx: &mut Context<NyaTermApp>,
) -> impl IntoElement {
    let QuickCommandSortMenuConfig {
        current,
        sort_label,
        created_label,
        name_label,
        usage_label,
        custom_label,
    } = config;
    NyaDropdownMenu::new("quick-command-sort")
        .icon("icons/conn/sort.svg")
        .icon_size(px(14.))
        .selected(current != QuickCommandSortMode::Created)
        .tooltip(format!(
            "{} · {}",
            sort_label,
            quick_command_sort_mode_label(
                current,
                created_label.clone(),
                name_label.clone(),
                usage_label.clone(),
                custom_label.clone(),
            )
        ))
        .min_width(px(154.))
        .items([
            NyaMenuItem::action(created_label)
                .checked(current == QuickCommandSortMode::Created)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.set_quick_command_sort_mode(QuickCommandSortMode::Created, cx);
                })),
            NyaMenuItem::action(name_label)
                .checked(current == QuickCommandSortMode::Name)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.set_quick_command_sort_mode(QuickCommandSortMode::Name, cx);
                })),
            NyaMenuItem::action(usage_label)
                .checked(current == QuickCommandSortMode::Usage)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.set_quick_command_sort_mode(QuickCommandSortMode::Usage, cx);
                })),
            NyaMenuItem::action(custom_label)
                .checked(current == QuickCommandSortMode::Custom)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.set_quick_command_sort_mode(QuickCommandSortMode::Custom, cx);
                })),
        ])
}

pub(super) fn quick_command_view_menu_button_with_id(
    id: &'static str,
    current: QuickCommandViewMode,
    cx: &mut Context<NyaTermApp>,
) -> impl IntoElement {
    quick_command_view_menu_button(
        QuickCommandViewMenuConfig {
            id,
            current,
            icon_path: current.icon_path(),
            view_label: t!("quickCommands.viewMode"),
            list_label: t!("quickCommands.listMode"),
            compact_label: t!("quickCommands.compactListMode"),
            tile_label: t!("quickCommands.tileMode"),
            command_bar_label: t!("quickCommands.commandBarMode"),
            sidebar_label: t!("quickCommands.sidebarMode"),
        },
        cx,
    )
}

fn quick_command_view_menu_button(
    config: QuickCommandViewMenuConfig,
    cx: &mut Context<NyaTermApp>,
) -> impl IntoElement {
    let QuickCommandViewMenuConfig {
        id,
        current,
        icon_path,
        view_label,
        list_label,
        compact_label,
        tile_label,
        command_bar_label,
        sidebar_label,
    } = config;
    NyaDropdownMenu::new(id)
        .icon(icon_path)
        .icon_size(px(14.))
        .selected(true)
        .tooltip(format!(
            "{} · {}",
            view_label,
            quick_command_view_mode_label(
                current,
                list_label.clone(),
                compact_label.clone(),
                tile_label.clone(),
                command_bar_label.clone(),
                sidebar_label.clone(),
            )
        ))
        .min_width(px(154.))
        .items([
            NyaMenuItem::action(list_label)
                .checked(current == QuickCommandViewMode::List)
                .on_click(cx.listener(|this, _, window, cx| {
                    this.set_quick_command_view_mode_in_window(
                        QuickCommandViewMode::List,
                        window,
                        cx,
                    );
                })),
            NyaMenuItem::action(compact_label)
                .checked(current == QuickCommandViewMode::Compact)
                .on_click(cx.listener(|this, _, window, cx| {
                    this.set_quick_command_view_mode_in_window(
                        QuickCommandViewMode::Compact,
                        window,
                        cx,
                    );
                })),
            NyaMenuItem::action(tile_label)
                .checked(current == QuickCommandViewMode::Tile)
                .on_click(cx.listener(|this, _, window, cx| {
                    this.set_quick_command_view_mode_in_window(
                        QuickCommandViewMode::Tile,
                        window,
                        cx,
                    );
                })),
            NyaMenuItem::separator(),
            NyaMenuItem::action(command_bar_label)
                .checked(current == QuickCommandViewMode::CommandBar)
                .on_click(cx.listener(|this, _, window, cx| {
                    this.set_quick_command_view_mode_in_window(
                        QuickCommandViewMode::CommandBar,
                        window,
                        cx,
                    );
                })),
            NyaMenuItem::action(sidebar_label)
                .checked(current == QuickCommandViewMode::Sidebar)
                .on_click(cx.listener(|this, _, window, cx| {
                    this.set_quick_command_view_mode_in_window(
                        QuickCommandViewMode::Sidebar,
                        window,
                        cx,
                    );
                })),
        ])
}

fn quick_command_ai_popover_button(
    context: QuickCommandToolbarContext,
    config: QuickCommandAiPopoverConfig,
    cx: &mut Context<NyaTermApp>,
) -> impl IntoElement {
    let QuickCommandToolbarContext {
        palette,
        popover_bg,
    } = context;
    let QuickCommandAiPopoverConfig {
        open,
        prompt,
        prompt_input,
        button_label,
        generate_label,
    } = config;
    let can_generate = !prompt.trim().is_empty();
    div()
        .relative()
        .child(quick_command_toolbar_icon_button(
            palette,
            "quick-command-ai",
            "icons/ai.svg",
            open,
            button_label.clone(),
            cx.listener(|this, _, window, cx| {
                this.toggle_quick_command_ai_popover(window, cx);
            }),
        ))
        .when(open, |this| {
            // Deferred so the rows, which are the panel body and therefore paint
            // after this header, cannot cover the form; deferring also drops the
            // panel's `overflow_hidden` mask, so a short panel no longer clips it.
            // `anchored()` with no position anchors to this element's own layout
            // origin, and the offset restates the old `top: 28px; right: 0`
            // against the 24px-square trigger.
            this.child(
                deferred(
                    anchored()
                        .anchor(Anchor::TopRight)
                        .offset(point(px(24.), px(28.)))
                        .snap_to_window_with_margin(px(8.))
                        .child(
                            div()
                                .id(SharedString::from("quick-command-ai-popover"))
                                .occlude()
                                .w(px(320.))
                                .rounded_md()
                                .border_1()
                                .border_color(rgb(palette.border))
                                .bg(popover_bg)
                                .shadow_lg()
                                .p_3()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                    cx.stop_propagation();
                                })
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .font_weight(FontWeight(600.))
                                        .text_color(rgb(palette.text))
                                        .child(button_label),
                                )
                                .child(
                                    div()
                                        .id(SharedString::from("quick-command-ai-input"))
                                        .on_key_down(cx.listener(
                                            |this, event: &KeyDownEvent, window, cx| {
                                                if this.handle_quick_command_ai_prompt_key_down(
                                                    event, window, cx,
                                                ) {
                                                    cx.stop_propagation();
                                                }
                                            },
                                        ))
                                        .child(prompt_input),
                                )
                                .child(
                                    div().flex().justify_end().child(
                                        div()
                                            .id(SharedString::from("quick-command-ai-submit"))
                                            .h(px(24.))
                                            .px_2()
                                            .rounded_md()
                                            .flex()
                                            .items_center()
                                            .gap_1()
                                            .text_size(px(11.))
                                            .font_weight(FontWeight(600.))
                                            .text_color(if prompt.trim().is_empty() {
                                                rgb(palette.text_dimmed)
                                            } else {
                                                rgb(palette.text)
                                            })
                                            .bg(if prompt.trim().is_empty() {
                                                rgb(palette.input)
                                            } else {
                                                rgb(palette.hover)
                                            })
                                            .when(can_generate, |this| {
                                                this.cursor_pointer().hover(|this| {
                                                    this.bg(rgb(palette.surface_elevated))
                                                        .text_color(rgb(palette.text))
                                                })
                                            })
                                            .child(
                                                svg()
                                                    .size(px(13.))
                                                    .flex_none()
                                                    .path("icons/ai.svg")
                                                    .text_color(if prompt.trim().is_empty() {
                                                        rgb(palette.text_dimmed)
                                                    } else {
                                                        rgb(palette.text)
                                                    }),
                                            )
                                            .child(generate_label)
                                            .when(can_generate, |this| {
                                                this.on_click(cx.listener(|this, _, window, cx| {
                                                    this.submit_quick_command_ai_prompt(window, cx);
                                                }))
                                            }),
                                    ),
                                ),
                        ),
                )
                .with_priority(APP_OVERLAY_PRIORITY),
            )
        })
}

pub(super) fn quick_command_toolbar_icon_button(
    palette: crate::theme::ThemePalette,
    id: &'static str,
    icon_path: &'static str,
    active: bool,
    tooltip: impl Into<String>,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let tooltip = tooltip.into();
    div()
        .id(SharedString::from(id))
        .debug_selector(move || id.to_string())
        .size(px(24.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .text_color(if active {
            rgb(palette.link)
        } else {
            rgb(palette.text_muted)
        })
        .cursor_pointer()
        .hover(|this| this.bg(rgb(palette.hover)).text_color(rgb(palette.text)))
        .child(
            svg()
                .size(px(15.))
                .flex_none()
                .path(icon_path)
                .text_color(if active {
                    rgb(palette.link)
                } else {
                    rgb(palette.text_muted)
                }),
        )
        .tooltip(move |window, cx| NyaTooltip::new(tooltip.clone()).build(window, cx))
        .on_click(on_click)
}

pub(super) fn quick_command_toolbar_divider(
    palette: crate::theme::ThemePalette,
) -> impl IntoElement {
    div()
        .mx_1()
        .h(px(16.))
        .w(px(1.))
        .flex_none()
        .bg(rgb(palette.border))
}

fn quick_command_view_mode_label(
    mode: QuickCommandViewMode,
    list_label: Cow<'static, str>,
    compact_label: Cow<'static, str>,
    tile_label: Cow<'static, str>,
    command_bar_label: Cow<'static, str>,
    sidebar_label: Cow<'static, str>,
) -> Cow<'static, str> {
    match mode {
        QuickCommandViewMode::List => list_label,
        QuickCommandViewMode::Compact => compact_label,
        QuickCommandViewMode::Tile => tile_label,
        QuickCommandViewMode::CommandBar => command_bar_label,
        QuickCommandViewMode::Sidebar => sidebar_label,
    }
}

fn quick_command_sort_mode_label(
    mode: QuickCommandSortMode,
    created_label: Cow<'static, str>,
    name_label: Cow<'static, str>,
    usage_label: Cow<'static, str>,
    custom_label: Cow<'static, str>,
) -> Cow<'static, str> {
    match mode {
        QuickCommandSortMode::Created => created_label,
        QuickCommandSortMode::Name => name_label,
        QuickCommandSortMode::Usage => usage_label,
        QuickCommandSortMode::Custom => custom_label,
    }
}
