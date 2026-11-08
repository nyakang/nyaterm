use rust_i18n::t;

use gpui::{
    AnyElement, Context, IntoElement, KeyDownEvent, ListHorizontalSizingBehavior, MouseButton,
    MouseDownEvent, SharedString, UniformListScrollHandle, div,
    prelude::{
        FluentBuilder as _, InteractiveElement, ParentElement, StatefulInteractiveElement, Styled,
    },
    px, rgb, svg, uniform_list,
};
use nyaterm_ui::{
    NyaContextMenu, NyaDropdownMenu, NyaScrollable, NyaScrollbarAxis, NyaSearchInput,
};

use crate::features::connections::{
    ConnectionDragKind, ConnectionDragPayload, ConnectionImportReportView,
};
use crate::models::ConnectionSortMode;
use crate::theme::ThemePalette;

use super::super::list::{
    ConnectionListRow, connection_tree_indent_px, icon_action_button, icon_action_button_styled,
};
use super::super::panel::ConnectionPanel;
use super::CONNECTION_LIST_ROW_HEIGHT_PX;
use super::actions::ConnectionRowActionsDecoration;
use super::rows::{connection_inline_group_editor_row, connection_section, saved_connection_row};

/// The panel body.
///
/// Takes no `NyaTermApp`. GPUI records every entity read during a draw, so an app
/// read here -- even a diagnostic one -- would put the panel back on the app's
/// invalidation path and undo the isolation this shape exists for. Everything it
/// draws comes from the snapshot; everything it changes goes back through
/// `ConnectionPanel::with_app`.
pub(in crate::features::pages::connections) fn connections_panel(
    panel: &mut ConnectionPanel,
    window: &mut gpui::Window,
    cx: &mut Context<ConnectionPanel>,
) -> AnyElement {
    // Before the first flush there is nothing to draw and nothing to draw it from.
    // The catalog arrives on a store reply, which flushes on its way out.
    let Some(snapshot) = panel.snapshot() else {
        return div().into_any_element();
    };
    let chrome = snapshot.chrome;
    let palette = chrome.palette;

    let empty_connections_label = t!("savedConnections.empty");
    let empty_connections_hint = t!("savedConnections.emptyHint");
    let no_results_label = t!("savedConnections.noResults");
    let empty_group_label = t!("savedConnections.emptyGroup");

    let flat_rows = snapshot.rows.clone();
    let import_report = snapshot.import_report.clone();
    let store_is_empty = snapshot.store_is_empty;
    let nothing_matched = flat_rows.is_empty();
    let widest_row = snapshot.widest_row;

    let list_scroll = window
        .use_keyed_state(
            SharedString::from("connections-list-scroll-handle"),
            cx,
            |_, _| UniformListScrollHandle::new(),
        )
        .read(cx)
        .clone();

    let mut list = div()
        .id(SharedString::from("connections-list-scroll"))
        .debug_selector(|| "connections-list-scroll".to_string())
        .relative()
        .flex_1()
        .min_h_0()
        .p(px(6.))
        .flex()
        .flex_col()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|panel, _, _, cx| {
                // Click empty background clears multi-select (Tauri list onMouseDown).
                panel.with_app(cx, |app, cx| {
                    if app.connection_state.list_has_selection() {
                        app.clear_selected_connections(cx);
                    }
                });
            }),
        )
        .on_drop(
            cx.listener(|panel, payload: &ConnectionDragPayload, _, cx| {
                let payload = payload.clone();
                panel.with_app(cx, |app, cx| {
                    app.connection_state.clear_list_drop_target();
                    match payload.kind {
                        ConnectionDragKind::Connection => {
                            app.move_connection_into_group(payload.id.clone(), None, cx);
                        }
                        ConnectionDragKind::Group => {
                            app.move_group_into_group(payload.id.clone(), None, cx);
                        }
                    }
                });
            }),
        );
    if store_is_empty {
        list = list.child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .px_4()
                .py_8()
                .gap_2()
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(palette.text_muted))
                        .child(empty_connections_label),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(rgb(palette.text_dimmed))
                        .child(empty_connections_hint),
                ),
        );
    } else if nothing_matched {
        list = list.child(
            div()
                .px_4()
                .py_8()
                .text_size(px(11.))
                .text_color(rgb(palette.text_dimmed))
                .child(no_results_label),
        );
    } else {
        let row_count = flat_rows.len();
        let row_actions =
            ConnectionRowActionsDecoration::new(flat_rows.clone(), cx.weak_entity(), palette);
        // `uniform_list` derives its scrollable width from one measured row, so
        // point it at the row most likely to be the widest or long names would
        // still be unreachable.
        list = list.child(
            uniform_list(
                "connections-list-rows",
                row_count,
                // Still only the visible range, and now built from the snapshot
                // rather than by re-entering the app for every row on screen.
                cx.processor(move |panel, range: std::ops::Range<usize>, _, cx| {
                    let mut items = Vec::with_capacity(range.len());
                    let Some(snapshot) = panel.snapshot() else {
                        return items;
                    };
                    for index in range {
                        let Some(row) = snapshot.rows.get(index).cloned() else {
                            continue;
                        };
                        // A definite width here, so the rows inside can resolve
                        // their `min_w(relative(1.))` and still overflow it when
                        // the name is long. It tracks the horizontal scroll.
                        let item = div()
                            .h(px(CONNECTION_LIST_ROW_HEIGHT_PX))
                            .w_full()
                            .flex_none()
                            .flex()
                            .items_center();
                        items.push(match row {
                            ConnectionListRow::Separator => {
                                item.child(div().mx_2().h(px(1.)).w_full().bg(rgb(palette.border)))
                            }
                            ConnectionListRow::GroupHeader(section) => item.child(
                                div()
                                    .w_full()
                                    .child(connection_section(snapshot, section, cx)),
                            ),
                            ConnectionListRow::InlineGroupEditor { parent_id, depth } => item
                                .child(div().w_full().child(connection_inline_group_editor_row(
                                    snapshot, parent_id, depth, cx,
                                ))),
                            ConnectionListRow::EmptyGroup { depth } => item.child(
                                div()
                                    .w_full()
                                    .px_2()
                                    .pl(px(connection_tree_indent_px(depth)))
                                    .h(px(28.))
                                    .flex()
                                    .items_center()
                                    .text_size(px(11.))
                                    .text_color(rgb(palette.text_dimmed))
                                    .child(empty_group_label.clone()),
                            ),
                            ConnectionListRow::Connection {
                                connection_id,
                                depth,
                            } => {
                                let Some(connection) = snapshot.connection(&connection_id).cloned()
                                else {
                                    continue;
                                };
                                item.child(
                                    div().w_full().child(saved_connection_row(
                                        snapshot, connection, depth, cx,
                                    )),
                                )
                            }
                        });
                    }
                    #[cfg(test)]
                    panel.note_rows_built(items.len());
                    items
                }),
            )
            .debug_selector(|| "connections-list-rows".to_string())
            .with_decoration(row_actions)
            .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
            .with_width_from_item(widest_row)
            .flex_1()
            .min_h_0()
            .track_scroll(&list_scroll),
        );
    }

    // Both axes ride this one handle: rows are `Unconstrained` and their names
    // are never clipped, so a long or deeply nested name overflows sideways.
    // One `Scrollbar` for both is the right shape here - a shared reveal state
    // matches a single scroll surface, and the vendor skips painting an axis
    // whose content fits.
    let list = list.scrollbar(&list_scroll, NyaScrollbarAxis::Both);
    // One context menu for the whole list, aimed by whatever the press landed
    // on. Rows and group headers re-aim it from their own capture handlers,
    // which run inside this one, so the reset here is what a press on empty
    // space keeps. Nesting a menu per row instead opens both menus on a single
    // right-click, and the one that never receives the click goes on
    // re-focusing itself every layout pass - stranding any dialog opened
    // afterwards, because a dialog is dismissed through actions routed along
    // the focused element's path.
    //
    // The items are still built on the app, and still only when the menu opens.
    let menu_app = panel.app_handle();
    let list = NyaContextMenu::new_dynamic(
        list.capture_any_mouse_down(cx.listener(|panel, event: &MouseDownEvent, _, cx| {
            if event.button == MouseButton::Right {
                panel.with_app(cx, |app, _| {
                    app.connection_state.prepare_list_context_menu();
                });
            }
        })),
        move |_, cx| {
            menu_app
                .update(cx, |app, cx| {
                    app.connection_list_target_context_menu_items(cx)
                })
                .unwrap_or_default()
        },
    );

    // Tauri: PanelHeader (shared stack) + search/action strip + flat tree list.
    // Count is shown in the shared panel header via meta; strip hosts search + icons.
    div()
        .relative()
        .key_context(crate::shortcuts::SAVED_CONNECTIONS_KEY_CONTEXT)
        .track_focus(panel.focus_handle())
        .flex()
        .flex_col()
        .size_full()
        .overflow_hidden()
        .bg(chrome.transparent_surface)
        .child(connections_search_bar(panel, cx))
        .children(import_report.map(|report| connection_import_report_banner(palette, report, cx)))
        .child(list)
        .into_any_element()
}

/// Inline result banner shown above the list after an import. It reuses the
/// panel snapshot rather than opening another dialog, and is dismissible without
/// touching the imported data.
fn connection_import_report_banner(
    palette: ThemePalette,
    report: ConnectionImportReportView,
    cx: &mut Context<ConnectionPanel>,
) -> AnyElement {
    let mut parts = vec![
        t!(
            "savedConnections.windtermImportReportImported",
            count = report.imported
        )
        .to_string(),
    ];
    if report.skipped > 0 {
        parts.push(
            t!(
                "savedConnections.windtermImportReportSkipped",
                count = report.skipped
            )
            .to_string(),
        );
    }
    if report.failed > 0 {
        parts.push(
            t!(
                "savedConnections.windtermImportReportCredentialWarning",
                count = report.failed
            )
            .to_string(),
        );
    }
    if !report.missing_one_key.is_empty() {
        parts.push(
            t!(
                "savedConnections.windtermImportReportMissingOneKey",
                count = report.missing_one_key.len()
            )
            .to_string(),
        );
    }
    if !report.skipped_account_sessions.is_empty() {
        parts.push(
            t!(
                "savedConnections.windtermImportReportSkippedAccounts",
                count = report.skipped_account_sessions.len()
            )
            .to_string(),
        );
    }
    if !report.unsupported_one_key_sessions.is_empty() {
        parts.push(
            t!(
                "savedConnections.windtermImportReportUnsupported",
                count = report.unsupported_one_key_sessions.len()
            )
            .to_string(),
        );
    }
    let mut summary = parts.join(" · ");
    if let Some(state) = report.one_key_state {
        match state {
            nyaterm_core::WindtermOneKeyConfigState::Absent => {
                summary.push_str(" · ");
                summary.push_str(&t!("savedConnections.windtermOneKeysConfigMissing"));
            }
            nyaterm_core::WindtermOneKeyConfigState::Unreadable => {
                summary.push_str(" · ");
                summary.push_str(&t!("savedConnections.windtermOneKeysConfigUnreadable"));
            }
            nyaterm_core::WindtermOneKeyConfigState::Readable => {}
        }
    }

    let expanded = report.expanded;
    let has_details = report.has_details();
    let toggle_label = if expanded {
        t!("savedConnections.windtermImportReportHideDetails")
    } else {
        t!("savedConnections.windtermImportReportShowDetails")
    };

    let toggle = div()
        .id("connection-import-report-toggle")
        .flex_none()
        .cursor_pointer()
        .text_size(px(10.))
        .text_color(rgb(palette.link))
        .child(toggle_label)
        .on_click(cx.listener(|panel, _, _, cx| {
            panel.with_app(cx, |app, cx| {
                app.toggle_connection_import_report(cx);
            });
        }));

    let close = div()
        .id("connection-import-report-close")
        .flex_none()
        .size(px(16.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_sm()
        .cursor_pointer()
        .hover(move |this| this.bg(rgb(palette.surface_elevated)))
        .child(
            svg()
                .size(px(11.))
                .path("icons/window/close.svg")
                .text_color(rgb(palette.text_muted)),
        )
        .on_click(cx.listener(|panel, _, _, cx| {
            panel.with_app(cx, |app, cx| {
                app.dismiss_connection_import_report(cx);
            });
        }));

    let details = expanded && has_details;

    div()
        .id("connection-import-report-banner")
        .w_full()
        .flex_none()
        .px_2()
        .py(px(6.))
        .border_b_1()
        .border_color(rgb(palette.border))
        .bg(rgb(palette.surface))
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(11.))
                        .text_color(rgb(palette.text))
                        .child(summary),
                )
                .when(has_details, |this| this.child(toggle))
                .child(close),
        )
        .when(details, |this| {
            this.child(
                div()
                    .id("connection-import-report-details")
                    .max_h(px(140.))
                    .overflow_y_scrollbar()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .children(import_report_sections(palette, &report)),
            )
        })
        .into_any_element()
}

fn import_report_sections(
    palette: ThemePalette,
    report: &ConnectionImportReportView,
) -> Vec<AnyElement> {
    let mut sections = Vec::new();
    if !report.missing_one_key.is_empty() {
        sections.push(import_report_section(
            palette,
            t!("savedConnections.windtermImportReportMissingOneKeyList").to_string(),
            &report.missing_one_key,
        ));
    }
    if !report.skipped_account_sessions.is_empty() {
        sections.push(import_report_section(
            palette,
            t!("savedConnections.windtermImportReportSkippedAccountsList").to_string(),
            &report.skipped_account_sessions,
        ));
    }
    if !report.unsupported_one_key_sessions.is_empty() {
        sections.push(import_report_section(
            palette,
            t!("savedConnections.windtermImportReportUnsupportedList").to_string(),
            &report.unsupported_one_key_sessions,
        ));
    }
    if !report.notice_messages.is_empty() {
        sections.push(import_report_section(
            palette,
            t!("savedConnections.importReportNotices").to_string(),
            &report.notice_messages,
        ));
    }
    sections
}

fn import_report_section(palette: ThemePalette, label: String, names: &[String]) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .text_size(px(10.))
                .text_color(rgb(palette.text_muted))
                .child(label),
        )
        .children(names.iter().map(|name| {
            div()
                .text_size(px(10.))
                .text_color(rgb(palette.text_dimmed))
                .child(SharedString::from(name.clone()))
                .into_any_element()
        }))
        .into_any_element()
}

fn connections_search_bar(
    panel: &ConnectionPanel,
    cx: &mut Context<ConnectionPanel>,
) -> impl IntoElement {
    let snapshot = panel
        .snapshot()
        .expect("the caller returns early without a snapshot");
    let chrome = snapshot.chrome;
    let palette = chrome.palette;
    let search_expanded = snapshot.search_expanded;
    let search_field = snapshot.search_field.clone();
    // Tauri swaps the glyph, flips it for Z-A and tints it while a name sort is
    // active, so the current mode is readable without hovering for the tooltip.
    let sort_mode = snapshot.sort_mode;
    let sort_label = match sort_mode {
        ConnectionSortMode::Default => "icons/conn/sort.svg",
        ConnectionSortMode::NameAsc | ConnectionSortMode::NameDesc => "icons/conn/sort-alpha.svg",
    };
    let sort_tint = (sort_mode != ConnectionSortMode::Default).then_some(palette.primary);
    let sort_flipped = sort_mode == ConnectionSortMode::NameDesc;
    let sort_tooltip = t!(match sort_mode {
        ConnectionSortMode::Default => "savedConnections.sortDefault",
        ConnectionSortMode::NameAsc => "savedConnections.sortNameAsc",
        ConnectionSortMode::NameDesc => "savedConnections.sortNameDesc",
    });
    // Built when the menu opens, not on every frame that draws this strip. The
    // items and their twenty-one handlers stay on the app, reached weakly.
    let more_menu_app = panel.app_handle();
    let more_menu = NyaDropdownMenu::new("connections-more")
        .icon("icons/conn/more.svg")
        .icon_size(px(14.))
        .tooltip(t!("common.more"))
        .min_width(px(180.))
        .items_dynamic(move |_, cx| {
            more_menu_app
                .update(cx, |app, cx| app.connection_more_menu_items(cx))
                .unwrap_or_default()
        })
        .on_trigger(|_, _, cx| cx.stop_propagation());
    let search_key_restore_focus = panel.focus_handle().clone();
    let search_input = NyaSearchInput::new("connection-search-input", &search_field).on_key_down(
        cx.listener(move |panel, event: &KeyDownEvent, window, cx| {
            let event = event.clone();
            panel.with_app(cx, |app, cx| {
                app.handle_connection_search_key_down(
                    &event,
                    &search_key_restore_focus,
                    window,
                    cx,
                );
            });
        }),
    );
    let (compact_search, expanded_search): (Option<AnyElement>, Option<AnyElement>) =
        if search_expanded {
            let close_restore_focus = panel.focus_handle().clone();
            let close = div()
                .id(SharedString::from("connection-search-clear"))
                .size(px(20.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_sm()
                .text_size(px(10.))
                .text_color(rgb(palette.text_muted))
                .cursor_pointer()
                .hover(move |this| {
                    this.bg(rgb(palette.surface_elevated))
                        .text_color(rgb(palette.text))
                })
                .on_click(cx.listener(move |panel, _, window, cx| {
                    cx.stop_propagation();
                    panel.with_app(cx, |app, cx| {
                        app.clear_connection_search(&close_restore_focus, window, cx);
                    });
                }))
                .child(
                    svg()
                        .size(px(13.))
                        .path("icons/window/close.svg")
                        .text_color(rgb(palette.text_muted)),
                );
            let overlay = div()
                .id(SharedString::from("connection-search-overlay"))
                .absolute()
                .top(px(4.))
                .bottom(px(4.))
                .left(px(8.))
                .right(px(8.))
                .rounded_md()
                .border_1()
                .border_color(rgb(palette.primary))
                .bg(rgb(palette.surface))
                .shadow_lg()
                .occlude()
                .flex()
                .items_center()
                .on_click(|_, _, cx| cx.stop_propagation())
                .child(search_input.compact().bare().trailing(close));
            (None, Some(overlay.into_any_element()))
        } else {
            let compact = div()
                .id(SharedString::from("connection-search-expand"))
                .flex_1()
                .min_w_0()
                .h(px(28.))
                .rounded_md()
                .bg(rgb(palette.surface_elevated))
                .flex()
                .items_center()
                .on_click(cx.listener(|panel, _, window, cx| {
                    panel.with_app(cx, |app, cx| {
                        app.focus_connection_search(window, cx);
                    });
                }))
                .child(search_input.compact().bare());
            (Some(compact.into_any_element()), None)
        };

    // Tauri search strip: px-2 py-1.5, input h-7.
    div()
        .relative()
        .h(px(36.))
        .px_2()
        .flex()
        .items_center()
        .gap_1()
        .border_b_1()
        .border_color(rgb(palette.border))
        .bg(chrome.transparent_section_header)
        .child(div().flex_1().min_w_0().children(compact_search))
        // Count lives in PanelHeader (Tauri).
        .child(icon_action_button_styled(
            palette,
            "connections-sort",
            sort_label,
            sort_tooltip,
            sort_tint,
            sort_flipped,
            cx.listener(|panel, _, _, cx| {
                panel.with_app(cx, |app, cx| {
                    app.cycle_connection_sort_mode(cx);
                });
            }),
        ))
        .child(icon_action_button(
            palette,
            "connections-temp-ssh",
            "icons/conn/flash.svg",
            t!("temporarySsh.title"),
            cx.listener(|panel, _, window, cx| {
                panel.with_app(cx, |app, cx| {
                    app.open_temporary_ssh_link_dialog(window, cx);
                });
            }),
        ))
        .child(icon_action_button(
            palette,
            "connections-new-group",
            "icons/fe/new-folder.svg",
            t!("savedConnections.newFolder"),
            cx.listener(|panel, _, window, cx| {
                panel.with_app(cx, |app, cx| {
                    app.open_connection_group_editor(None, None, window, cx);
                });
            }),
        ))
        .child(icon_action_button(
            palette,
            "connections-new",
            "icons/conn/add.svg",
            t!("savedConnections.newConnection"),
            cx.listener(|panel, _, window, cx| {
                panel.with_app(cx, |app, cx| {
                    app.open_connection_editor(None, None, false, window, cx);
                });
            }),
        ))
        .child(more_menu)
        .children(expanded_search)
}
