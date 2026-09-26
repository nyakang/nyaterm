use rust_i18n::t;

use gpui::{
    AnyElement, Context, IntoElement, KeyDownEvent, ListHorizontalSizingBehavior, MouseButton,
    MouseDownEvent, SharedString, div, prelude::*, px, rgb, rgba, svg, uniform_list,
};
use nyaterm_core::truncate_preview;
use nyaterm_ui::{NyaContextMenu, NyaHorizontalScrollbar, NyaSearchInput, NyaUniformListScrollbar};

use crate::features::transfers::format_file_size;
use crate::models::TransferBrowserSortColumn;

use super::super::browser_filter::{TransferBrowserFooterStats, transfer_browser_footer_stats};
use super::super::panel::TransferPanel;
use super::super::{
    FILE_BROWSER_HEADER_HEIGHT_PX, TransferBrowserAvailability,
    TransferBrowserEntryRowPresentation, TransferBrowserSortHeaderState,
    normalized_transfer_browser_path, sort_header_cell, transfer_browser_entry_row,
    transfer_browser_parent_entry_row, transfer_browser_path_is_root, transfer_browser_table_width,
};
use super::helpers::{
    compact_transfer_footer_button, compact_transfer_footer_button_active,
    compact_transfer_toolbar_button, compact_transfer_toolbar_button_active,
    compact_transfer_toolbar_button_enabled, compact_transfer_upload_menu_button,
    transfer_toolbar_divider,
};

const FILE_BROWSER_SCROLLBAR_SIZE_PX: f32 = 16.;

/// The SFTP browser.
///
/// Takes no `NyaTermApp`, including inside the `uniform_list` processor: that runs
/// during the draw, so an app read there is recorded exactly like one in `render`.
pub(in crate::features::pages::transfers) fn transfer_browser_view(
    panel: &TransferPanel,
    _window: &mut gpui::Window,
    cx: &mut Context<TransferPanel>,
) -> impl IntoElement {
    {
        let snapshot = panel
            .snapshot()
            .expect("the caller returns early without a snapshot");
        let chrome = snapshot.chrome;
        let browser = &snapshot.browser;
        let availability = snapshot.availability;
        let palette = chrome.palette;
        let transparent_surface = chrome.transparent_surface;
        let section_header = chrome.transparent_section_header;
        if availability != TransferBrowserAvailability::Browsable {
            let (title, description) = match availability {
                TransferBrowserAvailability::UnsupportedSession => (
                    t!("fileExplorer.unsupportedSession"),
                    Some(t!("fileExplorer.unsupportedSessionDesc")),
                ),
                TransferBrowserAvailability::NoSession
                | TransferBrowserAvailability::DisconnectedSession => {
                    (t!("fileExplorer.connectToSession"), None)
                }
                TransferBrowserAvailability::Browsable => unreachable!(),
            };

            return div()
                .id(SharedString::from("transfer-browser-panel"))
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .overflow_hidden()
                .bg(transparent_surface)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .px_4()
                        .gap_1()
                        .text_center()
                        .child(
                            svg()
                                .size(px(28.))
                                .flex_none()
                                .path("icons/fe/folder-off.svg")
                                .text_color(rgb(palette.text_dimmed)),
                        )
                        .child(
                            div()
                                .mt_1()
                                .text_size(px(12.))
                                .text_color(rgb(palette.text_muted))
                                .child(title),
                        )
                        .when_some(description, |this, description| {
                            this.child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(palette.text_dimmed))
                                    .child(description),
                            )
                        }),
                );
        }

        let can_transfer = availability == TransferBrowserAvailability::Browsable;
        let local_backend = browser.local_backend;
        let tree_mode = browser.view_mode == nyaterm_core::TransferBrowserViewMode::Tree;
        let _selected = browser
            .selected_remote_path
            .as_deref()
            .map(|path| truncate_preview(path, 56))
            .unwrap_or_else(|| "none".to_string());
        let visible_entries = browser.visible_entries.clone();
        let column_widths = browser.column_widths;
        let table_width = transfer_browser_table_width(column_widths);
        let resizing_column = browser.column_resize.map(|state| state.column);
        let sort_header_state = TransferBrowserSortHeaderState {
            header_bg: section_header,
            active_column: browser.sort_column,
            direction: browser.sort_direction,
            resizing_column,
        };
        let show_hidden_files = browser.show_hidden_files;
        let footer_stats = if tree_mode {
            let mut stats = TransferBrowserFooterStats {
                selected_file_size: 0,
                selected_item_count: 0,
                total_file_size: 0,
                total_item_count: 0,
            };
            for row in browser.tree.rows.iter().filter(|row| !row.root) {
                stats.total_item_count += 1;
                if let Some(entry) = row.entry.as_ref()
                    && entry.file_type != nyaterm_transport::SftpFileType::Directory
                {
                    stats.total_file_size = stats
                        .total_file_size
                        .saturating_add(entry.size.unwrap_or(0));
                    if browser.tree.selected.contains(&row.key) {
                        stats.selected_file_size = stats
                            .selected_file_size
                            .saturating_add(entry.size.unwrap_or(0));
                    }
                }
                if browser.tree.selected.contains(&row.key) {
                    stats.selected_item_count += 1;
                }
            }
            stats
        } else {
            transfer_browser_footer_stats(
                &browser.all_entries,
                &visible_entries,
                browser.selected_remote_path.as_deref(),
                &browser.selected_remote_paths,
                browser.show_hidden_files,
            )
        };
        let footer_size_text =
            if footer_stats.selected_item_count > 0 && footer_stats.selected_file_size > 0 {
                format!(
                    "{}/{}",
                    format_file_size(Some(footer_stats.selected_file_size)),
                    format_file_size(Some(footer_stats.total_file_size))
                )
            } else {
                format_file_size(Some(footer_stats.total_file_size))
            };
        let search_active = !browser.search.trim().is_empty();
        let search_expanded = browser.search_expanded || search_active;
        let menu_app = panel.app_handle();
        // The field is built where the search is revealed, not here: this is a
        // render path, and `text_input` would create it on the first frame.
        let search_input = search_expanded
            .then(|| browser.search_field.clone())
            .flatten()
            .map(|field| {
                let close = div()
                    .id(SharedString::from("transfer-browser-clear-search"))
                    .size(px(20.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_sm()
                    .text_size(px(12.))
                    .text_color(rgb(palette.text_muted))
                    .cursor_pointer()
                    .hover(move |this| {
                        this.bg(rgb(palette.surface_elevated))
                            .text_color(rgb(palette.text))
                    })
                    .child(
                        svg()
                            .size(px(13.))
                            .path("icons/window/close.svg")
                            .text_color(rgb(palette.text_muted)),
                    )
                    .on_click(cx.listener(|panel, _, window, cx| {
                        cx.stop_propagation();
                        panel.with_app(cx, |this, cx| {
                            this.clear_or_close_transfer_browser_search(window, cx);
                        })
                    }));
                div()
                    .h_full()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .child(
                        NyaSearchInput::new("transfer-browser-search", &field)
                            .compact()
                            .bare()
                            .trailing(close)
                            .on_key_down(cx.listener(|panel, event: &KeyDownEvent, window, cx| {
                                panel.with_app(cx, |this, cx| {
                                    if event.keystroke.key == "escape" {
                                        cx.stop_propagation();
                                        this.clear_or_close_transfer_browser_search(window, cx);
                                    }
                                })
                            })),
                    )
                    .into_any_element()
            });
        let current_browser_path = normalized_transfer_browser_path(&browser.path);
        let has_parent_entry =
            can_transfer && !transfer_browser_path_is_root(&current_browser_path);
        let show_list_scrollbar = can_transfer && !browser.loading && !visible_entries.is_empty();
        let auto_sync_cwd = browser.auto_sync_cwd_enabled;
        let cwd_tracking_available = browser.connection_id.clone().is_some();
        let external_drop_hover = browser.external_drop_hover;
        let rows: AnyElement = if browser.loading {
            div()
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .px_4()
                        .py_8()
                        .text_size(px(12.))
                        .text_color(rgb(palette.text_dimmed))
                        .child(t!("fileExplorer.loading")),
                )
                .into_any_element()
        } else if browser.all_entries.is_empty() {
            if has_parent_entry && browser.error.is_none() {
                div()
                    .flex()
                    .flex_col()
                    .child(transfer_browser_parent_entry_row(
                        palette,
                        column_widths,
                        cx,
                    ))
                    .into_any_element()
            } else {
                div()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .justify_center()
                            .px_4()
                            .py_8()
                            .gap_1()
                            .child(if let Some(error) = browser.error.as_deref() {
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(palette.danger))
                                    .child(truncate_preview(error, 120))
                            } else {
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(palette.text_muted))
                                    .child(t!("fileExplorer.emptyDirectory"))
                            }),
                    )
                    .into_any_element()
            }
        } else if visible_entries.is_empty() {
            div()
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .px_4()
                        .py_8()
                        .text_size(px(11.))
                        .text_color(rgb(palette.text_dimmed))
                        .child(t!("fileExplorer.noSearchResults")),
                )
                .into_any_element()
        } else {
            let parent_count = usize::from(has_parent_entry);
            let total_entries = visible_entries.len() + parent_count;
            let _name_placeholder = t!("fileExplorer.name");
            let mut list = uniform_list(
                "transfer-browser-rows",
                total_entries,
                // Still only the visible range, and now built from the snapshot
                // rather than by re-entering the app for every row on screen.
                cx.processor(move |panel, range: std::ops::Range<usize>, _, cx| {
                    let mut items = Vec::with_capacity(range.len());
                    let Some(snapshot) = panel.snapshot() else {
                        return items;
                    };
                    let browser = &snapshot.browser;
                    let renaming = browser.rename.clone();
                    let selected_remote_path = browser.selected_remote_path.clone();
                    let selected_remote_paths = browser.selected_remote_paths.clone();
                    let visible_entries = browser.visible_entries.clone();
                    for index in range {
                        if has_parent_entry && index == 0 {
                            items.push(
                                transfer_browser_parent_entry_row(palette, column_widths, cx)
                                    .into_any_element(),
                            );
                            continue;
                        }
                        let Some(entry) = visible_entries
                            .get(index.saturating_sub(parent_count))
                            .cloned()
                        else {
                            continue;
                        };
                        let rename_input = renaming
                            .as_ref()
                            .filter(|state| state.old_path == entry.path)
                            // The field is built where the rename opens, not here.
                            .zip(browser.rename_field.clone())
                            .map(|(state, field)| {
                                nyaterm_ui::NyaInputShell::new(
                                    SharedString::from(format!(
                                        "transfer.rename.{}",
                                        state.old_path
                                    )),
                                    &field,
                                )
                                .compact()
                                .into_any_element()
                            });
                        items.push(
                            transfer_browser_entry_row(
                                TransferBrowserEntryRowPresentation {
                                    palette,
                                    entry,
                                    selected_remote_path: selected_remote_path.clone(),
                                    selected_remote_paths: &selected_remote_paths,
                                    column_widths,
                                    rename_state: renaming.clone(),
                                    rename_input,
                                },
                                cx,
                            )
                            .into_any_element(),
                        );
                    }
                    #[cfg(test)]
                    panel.note_rows_built(items.len());
                    items
                }),
            );
            list.style().restrict_scroll_to_axis = Some(true);
            list.h_full()
                .min_h_0()
                .min_w(table_width)
                .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::FitList)
                .track_scroll(&browser.list_scroll)
                .into_any_element()
        };

        div()
            .id(SharedString::from("transfer-browser-panel"))
            .key_context(crate::shortcuts::FILE_EXPLORER_KEY_CONTEXT)
            .size_full()
            .flex()
            .flex_col()
            .relative()
            .overflow_hidden()
            .bg(transparent_surface)
            .track_focus(&browser.focus)
            .can_drop(|drag, _, _| drag.is::<gpui::ExternalPaths>())
            .on_drag_move(cx.listener(
                |panel, event: &gpui::DragMoveEvent<gpui::ExternalPaths>, _, cx| panel.with_app(cx, |this, cx| {
                    this.set_transfer_browser_external_drop_hover(
                        event.bounds.contains(&event.event.position),
                        cx,
                    );
                }),
            ))
            .on_drop(cx.listener(|panel, paths: &gpui::ExternalPaths, _, cx| panel.with_app(cx, |this, cx| {
                this.handle_transfer_browser_external_file_drop(paths.paths().to_vec(), cx);
            })))
            .on_key_down(cx.listener(|panel, event: &KeyDownEvent, window, cx| panel.with_app(cx, |this, cx| {
                this.handle_transfer_browser_key_down(event, window, cx);
            })))
            .when(can_transfer, |this| {
                this.child(
                    div()
                        .relative()
                        .h(px(36.))
                        .px_1()
                        .border_b_1()
                        .border_color(rgb(palette.border))
                        .bg(section_header)
                        .flex()
                        .items_center()
                        .gap(px(2.))
                        .child(compact_transfer_toolbar_button(
                            palette,
                            "transfer-browser-new-file",
                            "icons/fe/new-file.svg",
                            t!("fileExplorer.newFile"),
                            cx.listener(|panel, _, window, cx| panel.with_app(cx, |this, cx| {
                                this.open_transfer_new_file_dialog(window, cx);
                            })),
                        ))
                        .child(compact_transfer_toolbar_button(
                            palette,
                            "transfer-browser-new-folder",
                            "icons/fe/new-folder.svg",
                            t!("fileExplorer.newFolder"),
                            cx.listener(|panel, _, window, cx| panel.with_app(cx, |this, cx| {
                                this.open_transfer_new_folder_dialog(window, cx);
                            })),
                        ))
                        .when(!local_backend, |toolbar| {
                            toolbar
                                .child(transfer_toolbar_divider(palette))
                                .child(compact_transfer_upload_menu_button(
                                    palette,
                                    t!("fileExplorer.upload"),
                                    cx,
                                ))
                                .child(compact_transfer_toolbar_button_enabled(
                                    palette,
                                    "transfer-browser-download-selected",
                                    "icons/fe/download.svg",
                                    t!("fileExplorer.downloadSelected"),
                                    footer_stats.selected_item_count > 0,
                                    cx.listener(|panel, _, window, cx| panel.with_app(cx, |this, cx| {
                                        this.start_selected_sftp_download_jobs(window, cx);
                                    })),
                                ))
                        })
                        .child(compact_transfer_toolbar_button_enabled(
                            palette,
                            "transfer-browser-delete-selected",
                            "icons/fe/delete.svg",
                            t!("fileExplorer.delete"),
                            footer_stats.selected_item_count > 0,
                            cx.listener(|panel, _, window, cx| panel.with_app(cx, |this, cx| {
                                this.open_selected_transfer_delete_dialog(window, cx);
                            })),
                        ))
                        .child(transfer_toolbar_divider(palette))
                        .when(!tree_mode, |toolbar| {
                            toolbar.child(compact_transfer_toolbar_button(
                                palette,
                                "transfer-browser-go-up",
                                "icons/fe/up.svg",
                                t!("fileExplorer.goUp"),
                                cx.listener(|panel, _, window, cx| panel.with_app(cx, |this, cx| {
                                    this.open_transfer_parent_directory(window, cx);
                                })),
                            ))
                        })
                        .child(compact_transfer_toolbar_button(
                            palette,
                            "transfer-browser-refresh",
                            "icons/fe/refresh.svg",
                            t!("fileExplorer.refresh"),
                            cx.listener(|panel, _, window, cx| panel.with_app(cx, |this, cx| {
                                this.refresh_transfer_browser(window, cx);
                            })),
                        ))
                        .child(div().flex_1())
                        .when(!tree_mode, |toolbar| {
                            toolbar
                                .child(compact_transfer_toolbar_button_active(
                                    palette,
                                    "transfer-browser-expand-search",
                                    "icons/fe/search.svg",
                                    t!("fileExplorer.search"),
                                    search_active || search_expanded,
                                    cx.listener(|panel, _, window, cx| panel.with_app(cx, |this, cx| {
                                        this.focus_transfer_browser_search(None, window, cx);
                                    })),
                                ))
                                .child(compact_transfer_toolbar_button_active(
                                    palette,
                                    "transfer-browser-toggle-hidden-files",
                                    "icons/eye.svg",
                                    if show_hidden_files {
                                        t!("fileExplorer.hideHiddenFiles")
                                    } else {
                                        t!("fileExplorer.showHiddenFiles")
                                    },
                                    show_hidden_files,
                                    cx.listener(|panel, _, _, cx| panel.with_app(cx, |this, cx| {
                                        this.toggle_transfer_browser_hidden_files(cx);
                                    })),
                                ))
                                .child(compact_transfer_toolbar_button(
                                    palette,
                                    "transfer-browser-toggle-view-mode",
                                    "icons/view-tree.svg",
                                    t!("fileExplorer.switchToTreeView"),
                                    cx.listener(|panel, _, _, cx| panel.with_app(cx, |this, cx| {
                                        this.toggle_transfer_browser_view_mode(cx);
                                    })),
                                ))
                        })
                        .when(tree_mode, |toolbar| {
                            toolbar
                                .child(transfer_toolbar_divider(palette))
                                .child(compact_transfer_toolbar_button(
                                    palette,
                                    "transfer-browser-reveal-tree-path",
                                    "icons/fe/locate.svg",
                                    t!("fileExplorer.revealCurrentPath"),
                                    cx.listener(|panel, _, _, cx| panel.with_app(cx, |this, cx| {
                                        this.reveal_transfer_tree_current_path(cx);
                                    })),
                                ))
                                .child(transfer_toolbar_divider(palette))
                                .child(compact_transfer_toolbar_button(
                                    palette,
                                    "transfer-browser-toggle-view-mode",
                                    "icons/view-list.svg",
                                    t!("fileExplorer.switchToListView"),
                                    cx.listener(|panel, _, _, cx| panel.with_app(cx, |this, cx| {
                                        this.toggle_transfer_browser_view_mode(cx);
                                    })),
                                ))
                        })
                        .when(search_expanded && !tree_mode, |toolbar| {
                            toolbar.child(
                                div()
                                    .id(SharedString::from("transfer-browser-search-overlay"))
                                    .absolute()
                                    .top(px(4.))
                                    .bottom(px(4.))
                                    .left(px(6.))
                                    .right(px(6.))
                                    .rounded_md()
                                    .border_1()
                                    .border_color(rgb(palette.primary))
                                    .bg(rgb(palette.surface))
                                    .shadow_lg()
                                    .occlude()
                                    .flex()
                                    .items_center()
                                    .on_click(|_, _, cx| cx.stop_propagation())
                                    .children(search_input),
                            )
                        }),
                )
                .when(!tree_mode, |this| {
                    this.child(super::super::path_bar::transfer_browser_path_row(
                        panel,
                        current_browser_path.clone(),
                        cx,
                    ))
                })
            })
            .child(if tree_mode {
                super::super::tree::transfer_tree_view(panel, cx).into_any_element()
            } else {
                div().flex().flex_1().min_h_0().min_w_0()
            .child(NyaContextMenu::new_dynamic(
                div()
                    .id(SharedString::from("transfer-browser-table-viewport"))
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .overflow_hidden()
                    .capture_any_mouse_down(cx.listener(|panel, event: &MouseDownEvent, _, cx| panel.with_app(cx, |this, cx| {
                        if event.button == MouseButton::Right {
                            this.begin_transfer_browser_context_menu(cx);
                        }
                    })))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|panel, _: &MouseDownEvent, window, cx| panel.with_app(cx, |this, cx| {
                            this.prepare_transfer_browser_current_context_menu(window, cx);
                        })),
                    )
                    .child(
                        div()
                            .id(SharedString::from("transfer-browser-table-scroll"))
                            .size_full()
                            .min_w_0()
                            .overflow_x_scroll()
                            .overflow_y_hidden()
                            .restrict_scroll_to_axis()
                            .track_scroll(&browser.horizontal_scroll)
                            .child(
                                div()
                                    .min_w(table_width)
                                    .h_full()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_0()
                                            .child(sort_header_cell(
                                                palette,
                                                TransferBrowserSortColumn::Name,
                                                t!("fileExplorer.name"),
                                                column_widths.name,
                                                sort_header_state,
                                                cx,
                                            ))
                                            .child(sort_header_cell(
                                                palette,
                                                TransferBrowserSortColumn::Modified,
                                                t!("fileExplorer.mtime"),
                                                column_widths.modified,
                                                sort_header_state,
                                                cx,
                                            ))
                                            .child(sort_header_cell(
                                                palette,
                                                TransferBrowserSortColumn::Size,
                                                t!("fileExplorer.size"),
                                                column_widths.size,
                                                sort_header_state,
                                                cx,
                                            ))
                                            .child(sort_header_cell(
                                                palette,
                                                TransferBrowserSortColumn::Permissions,
                                                t!("fileExplorer.permissions"),
                                                column_widths.permissions,
                                                sort_header_state,
                                                cx,
                                            ))
                                            .child(sort_header_cell(
                                                palette,
                                                TransferBrowserSortColumn::Owner,
                                                t!("fileExplorer.owner"),
                                                column_widths.owner,
                                                sort_header_state,
                                                cx,
                                            ))
                                            .child(sort_header_cell(
                                                palette,
                                                TransferBrowserSortColumn::Group,
                                                t!("fileExplorer.group"),
                                                column_widths.group,
                                                sort_header_state,
                                                cx,
                                            )),
                                    )
                                    .child(div().relative().flex_1().min_h_0().child(rows)),
                            ),
                    )
                    .when(show_list_scrollbar, |this| {
                        this.child(
                            // Spans the row viewport rather than just the track strip:
                            // the bar's hitbox is what hover-to-reveal watches, and it
                            // also makes the thumb proportional to the real viewport.
                            // `Scrollbar::vertical` still lays its track at the right edge.
                            div()
                                .absolute()
                                .top(px(FILE_BROWSER_HEADER_HEIGHT_PX))
                                .bottom_0()
                                .left_0()
                                .right_0()
                                .child(NyaUniformListScrollbar::new(
                                    "transfer-browser-vertical-scrollbar",
                                    &browser.list_scroll,
                                )),
                        )
                    })
                    .child(
                        // Also viewport-spanning, and it keeps the header so hovering the
                        // column titles reveals the bar too. Inset by a full track width
                        // on the right: the two axes are independent `Scrollbar` elements,
                        // so the vendor's own corner-avoidance does not apply.
                        div()
                            .absolute()
                            .inset_0()
                            .right(px(FILE_BROWSER_SCROLLBAR_SIZE_PX))
                            .child(NyaHorizontalScrollbar::new(
                                "transfer-browser-horizontal-scrollbar",
                                &browser.horizontal_scroll,
                            )),
                    ),
                // Items are still built on the app, and still only when the menu
                // opens, reached weakly rather than held.
                move |_, cx| {
                    menu_app
                        .update(cx, |this, cx| this.transfer_browser_context_menu_items(cx))
                        .unwrap_or_default()
                },
            )
            // Match the Tauri file-explorer menu width (w-52 / min-w-[200px]);
            // 208px is the 13rem (w-52) equivalent so labels and shortcuts do not
            // reflow between items when the menu opens.
            .min_width(px(208.)))
            .into_any_element()
            })
            // Tauri FileExplorer footer: totals left, cwd sync / send icons right.
            .child(
                div()
                    .h(px(28.))
                    .flex_none()
                    .px_2()
                    .border_t_1()
                    .border_color(rgb(palette.border))
                    .bg(transparent_surface)
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap_3()
                            .text_size(px(11.))
                            .text_color(rgb(palette.text_muted))
                            .when(
                                !browser.loading
                                    && browser.error.is_none()
                                    && footer_stats.total_item_count > 0,
                                |this| {
                                    this.child(if footer_stats.selected_item_count > 0 {
                                        t!("fileExplorer.selectedItems", selected = footer_stats.selected_item_count.to_string(),, total = footer_stats.total_item_count.to_string(),)
                                    } else {
                                        t!("fileExplorer.totalItems", count = footer_stats.total_item_count.to_string(),)
                                    })
                                    .child(footer_size_text)
                                },
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_0()
                            .child(compact_transfer_footer_button(
                                palette,
                                "transfer-browser-footer-sync-cwd",
                                "icons/fe/folder-sync.svg",
                                if cwd_tracking_available {
                                    t!("fileExplorer.syncTerminalPath")
                                } else {
                                    t!("fileExplorer.cwdTrackingUnavailable")
                                },
                                cwd_tracking_available,
                                cx.listener(|panel, _, _window, cx| panel.with_app(cx, |this, cx| {
                                    this.start_transfer_sync_cwd_job(cx);
                                })),
                            ))
                            .when(!tree_mode, |footer| {
                                footer.child(compact_transfer_footer_button_active(
                                    palette,
                                    "transfer-browser-footer-auto-sync",
                                    "icons/fe/sync.svg",
                                    if cwd_tracking_available {
                                        t!("fileExplorer.autoSyncTerminalPath")
                                    } else {
                                        t!("fileExplorer.cwdTrackingUnavailable")
                                    },
                                    auto_sync_cwd,
                                    cwd_tracking_available,
                                    cx.listener(|panel, _, _window, cx| panel.with_app(cx, |this, cx| {
                                        this.toggle_transfer_browser_auto_sync_cwd(cx);
                                    })),
                                ))
                            })
                            .child(compact_transfer_footer_button(
                                palette,
                                "transfer-browser-footer-send-path",
                                "icons/fe/send-path.svg",
                                t!("fileExplorer.sendToTerminal"),
                                true,
                                cx.listener(|panel, _, _, cx| panel.with_app(cx, |this, cx| {
                                    this.send_current_transfer_browser_path_to_terminal(cx);
                                })),
                            )),
                    ),
            )
            .when(external_drop_hover, |this| {
                this.child(
                    div()
                        .absolute()
                        .inset_2()
                        .rounded_lg()
                        .border_2()
                        .border_color(rgb(palette.link))
                        .bg(rgba(0x3b82f624))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .max_w(px(340.))
                                .rounded_lg()
                                .border_1()
                                .border_color(rgb(palette.link))
                                .bg(rgb(palette.surface))
                                .px_6()
                                .py_4()
                                .shadow_lg()
                                .flex()
                                .flex_col()
                                .items_center()
                                .gap_1()
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(gpui::FontWeight(700.))
                                        .text_color(rgb(palette.text))
                                        .child(t!("fileExplorer.externalDropOverlayTitle")),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(palette.text_muted))
                                        .child(t!("fileExplorer.externalDropOverlayHint")),
                                ),
                        ),
                )
            })
    }
}
