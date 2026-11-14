use super::FileManagerNameKind;
use super::interaction::{
    FileManagerDragPayload, FileManagerDragPreview, accepts_drop, drag_operation,
};
use crate::features::file_manager::layout::{COLUMNS, column_key};
use crate::features::file_manager::pane::PaneId;
use crate::features::file_manager::workspace::FileManagerWorkspace;
use crate::features::formatting::format_permissions_octal;
use crate::features::pages::transfers::{format_browser_file_size, format_sftp_modified};
use crate::features::transfers::file_open::FileOpenMode;
use crate::features::transfers::file_operations::FileTransferOperation;
use crate::features::view_widgets::transfer_entry_icon;
use crate::theme::ThemePalette;
use gpui::{
    AnyElement, Context, CursorStyle, DragMoveEvent, IntoElement, MouseButton, MouseDownEvent,
    SharedString, div, prelude::*, px, rgb, uniform_list,
};
use nyaterm_files::file_browser::listing::FileBrowserSortColumn;
use nyaterm_files::sftp::SftpFileEntry;
use nyaterm_ui::{
    NyaContextMenu, NyaHorizontalScrollbar, NyaMenuItem, NyaTooltip, NyaUniformListScrollbar,
};
use rust_i18n::t;
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

impl FileManagerWorkspace {
    pub(super) fn pane_view(
        &mut self,
        palette: ThemePalette,
        pane: PaneId,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let entries = self.current_visible(pane, cx);
        let state = self.pane(pane);
        let columns = state.table_columns(self.pane_width(pane) - 2.);
        let width: f32 = columns.iter().map(|(_, width)| f32::from(*width)).sum();
        let row_height = state.row_height;
        let scroll = state.list_scroll.clone();
        let horizontal = state.horizontal_scroll.clone();
        let endpoint = state.endpoint.clone();
        let directory = state.path.clone();
        let drop_directory = directory.clone();
        let external_directory = directory.clone();
        let can_write = !state.disconnected;
        let count = entries.len();
        let selected = state.selected_paths.len();
        let status = if state.loading {
            t!("fileManager.loading").to_string()
        } else if !state.status.is_empty()
            && state.status != format!("{} item(s)", state.entries.len())
        {
            state.status.clone()
        } else {
            t!(
                "fileManager.selectionStatus",
                count = count,
                selected = selected
            )
            .to_string()
        };
        let row_entries = entries.clone();
        let row_columns = columns.clone();
        let mut list = uniform_list(
            SharedString::from(format!("fm-list-{}-{}", self.id, pane.as_str())),
            entries.len() + 1,
            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                range
                    .map(|index| {
                        if index == 0 {
                            div()
                                .id(SharedString::from(format!("fm-parent-{}", pane.as_str())))
                                .h(px(row_height as f32))
                                .w(px(width))
                                .flex()
                                .items_center()
                                .px_2()
                                .text_xs()
                                .cursor_pointer()
                                .child("..")
                                .on_click(cx.listener(
                                    move |this, event: &gpui::ClickEvent, window, cx| {
                                        cx.stop_propagation();
                                        this.focus_pane(pane, window, cx);
                                        if event.click_count() >= 2 {
                                            this.navigate_parent(pane, window, cx);
                                        }
                                    },
                                ))
                                .into_any_element()
                        } else {
                            this.entry_row(
                                palette,
                                pane,
                                row_entries[index - 1].clone(),
                                &row_entries,
                                &row_columns,
                                cx,
                            )
                        }
                    })
                    .collect()
            }),
        );
        list.style().restrict_scroll_to_axis = Some(true);
        let drop_endpoint = endpoint.clone();
        let list = list.h_full().min_h_0().w(px(width)).track_scroll(&scroll);
        let table = div()
            .id(SharedString::from(format!(
                "fm-table-scroll-{}",
                pane.as_str()
            )))
            .size_full()
            .min_h_0()
            .overflow_x_scroll()
            .track_scroll(&horizontal)
            .child(
                div()
                    .w(px(width))
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(self.table_header(palette, pane, &columns, cx))
                    .child(list),
            );
        div()
            .size_full()
            .min_h_0()
            .min_w_0()
            .relative()
            .flex()
            .flex_col()
            .overflow_hidden()
            .border_1()
            .border_color(rgb(if self.active == pane {
                palette.primary
            } else {
                palette.border
            }))
            .track_focus(&self.pane(pane).focus)
            .child(self.measure(Some(pane), cx))
            .child(self.navigation_view(palette, pane, cx))
            .child(
                div()
                    .id(SharedString::from(format!(
                        "fm-list-viewport-{}",
                        pane.as_str()
                    )))
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .overflow_hidden()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.focus_pane(pane, window, cx);
                        this.pane_mut(pane).clear_selection();
                        cx.notify();
                    }))
                    .can_drop(move |drag, _, _| {
                        drag.is::<gpui::ExternalPaths>()
                            || drag.downcast_ref::<FileManagerDragPayload>().is_some_and(
                                |payload| {
                                    accepts_drop(payload, &drop_endpoint, &directory, can_write)
                                },
                            )
                    })
                    .on_drag_move(cx.listener(
                        move |this, event: &DragMoveEvent<FileManagerDragPayload>, window, cx| {
                            let payload = event.drag(cx);
                            payload
                                .same_pane_target
                                .set(payload.manager_id == this.id && payload.pane_id == pane);
                            let operation = drag_operation(
                                payload.manager_id == this.id && payload.pane_id == pane,
                                event.event.modifiers,
                            );
                            let valid = accepts_drop(
                                payload,
                                &this.pane(pane).endpoint,
                                &this.pane(pane).path,
                                can_write,
                            );
                            let changed = payload.feedback.replace(operation) != operation;
                            if payload.valid_target.replace(valid) != valid || changed {
                                window.refresh();
                            }
                        },
                    ))
                    .on_drop(cx.listener(
                        move |this, payload: &FileManagerDragPayload, window, cx| {
                            this.drop_files(
                                payload,
                                pane,
                                drop_directory.clone(),
                                can_write,
                                window,
                                cx,
                            );
                            cx.stop_propagation();
                        },
                    ))
                    .on_drop(
                        cx.listener(move |this, paths: &gpui::ExternalPaths, window, cx| {
                            this.drop_external_files(
                                paths,
                                pane,
                                external_directory.clone(),
                                can_write,
                                window,
                                cx,
                            );
                            cx.stop_propagation();
                        }),
                    )
                    .child(table)
                    .child(
                        div()
                            .absolute()
                            .top(px(28.))
                            .left_0()
                            .right_0()
                            .bottom(px(12.))
                            .child(NyaUniformListScrollbar::new(
                                format!("fm-scroll-{}", pane.as_str()),
                                &scroll,
                            )),
                    )
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .child(NyaHorizontalScrollbar::new(
                                format!("fm-hscroll-{}", pane.as_str()),
                                &horizontal,
                            )),
                    ),
            )
            .child(
                div()
                    .h(px(22.))
                    .min_w_0()
                    .px_2()
                    .flex()
                    .items_center()
                    .text_xs()
                    .text_color(rgb(palette.text_muted))
                    .truncate()
                    .child(status),
            )
            .into_any_element()
    }

    fn table_header(
        &self,
        palette: ThemePalette,
        pane: PaneId,
        columns: &[(FileBrowserSortColumn, gpui::Pixels)],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut header = div()
            .h(px(28.))
            .flex_none()
            .flex()
            .items_center()
            .border_b_1()
            .border_color(rgb(palette.border));
        for &(column, width) in columns {
            let label_key = format!("fileManager.columns.{}", column_key(column));
            let marker = if self.pane(pane).sort_column == column {
                crate::models::transfer_ui::browser_sort_direction_marker(
                    self.pane(pane).sort_direction,
                )
            } else {
                ""
            };
            let mut label = div()
                .id(SharedString::from(format!(
                    "fm-column-{}-{}",
                    pane.as_str(),
                    column_key(column)
                )))
                .w(width)
                .h_full()
                .flex_none()
                .relative()
                .flex()
                .items_center()
                .px_2()
                .text_xs()
                .cursor_pointer()
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .child(format!("{} {marker}", t!(&label_key))),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    let state = this.pane_mut(pane);
                    state.sort_direction = if state.sort_column == column {
                        state.sort_direction.toggled()
                    } else {
                        column.default_direction()
                    };
                    state.sort_column = column;
                    this.save_layout(window, cx);
                    cx.notify();
                }));
            label = label.child(
                div()
                    .id(SharedString::from(format!(
                        "fm-resize-{}-{}",
                        pane.as_str(),
                        column_key(column)
                    )))
                    .absolute()
                    .right_0()
                    .top_0()
                    .bottom_0()
                    .w(px(5.))
                    .cursor(CursorStyle::ResizeLeftRight)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            this.column_resize =
                                Some((pane, column, f32::from(event.position.x), width));
                            cx.stop_propagation();
                        }),
                    )
                    .on_click(|_, _, cx| cx.stop_propagation()),
            );
            header = header.child(label);
        }
        let mut items = Vec::new();
        for column in COLUMNS {
            let key = column_key(column);
            let label_key = format!("fileManager.columns.{key}");
            items.push(
                NyaMenuItem::action(t!(&label_key).to_string())
                    .checked(
                        self.pane(pane)
                            .visible_columns
                            .iter()
                            .any(|visible| visible == key),
                    )
                    .disabled(column == FileBrowserSortColumn::Name)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let visible = &mut this.pane_mut(pane).visible_columns;
                        if visible.iter().any(|visible| visible == key) {
                            visible.retain(|visible| visible != key);
                        } else {
                            visible.push(key.into());
                        }
                        this.save_layout(window, cx);
                        cx.notify();
                    })),
            );
        }
        items.push(NyaMenuItem::separator());
        items.push(
            NyaMenuItem::action(t!("fileManager.compactRows"))
                .checked(self.pane(pane).row_height == 28)
                .on_click(cx.listener(move |this, _, window, cx| {
                    let state = this.pane_mut(pane);
                    state.row_height = if state.row_height == 28 { 30 } else { 28 };
                    state.list_scroll = gpui::UniformListScrollHandle::new();
                    this.save_layout(window, cx);
                    cx.notify();
                })),
        );
        NyaContextMenu::new(header, items).into_any_element()
    }

    fn entry_row(
        &self,
        palette: ThemePalette,
        pane: PaneId,
        entry: SftpFileEntry,
        visible: &Arc<[SftpFileEntry]>,
        columns: &[(FileBrowserSortColumn, gpui::Pixels)],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = self.pane(pane);
        let identity = entry.identity_key();
        let selected = state.selected_paths.contains(&identity);
        let focused = state.selected_path.as_deref() == Some(&identity);
        let payload = FileManagerDragPayload {
            manager_id: self.id.clone(),
            pane_id: pane,
            access_generation: state
                .endpoint
                .session_id()
                .and_then(|session| self.read_app(cx, |app| app.file_access_generation(session))),
            endpoint: state.endpoint.clone(),
            entries: state.drag_entries(&entry),
            feedback: Rc::new(Cell::new(FileTransferOperation::Copy)),
            valid_target: Rc::new(Cell::new(false)),
            same_pane_target: Rc::new(Cell::new(false)),
        };
        let click_entry = entry.clone();
        let press_identity = identity.clone();
        let visible = visible.clone();
        let width: f32 = columns.iter().map(|(_, width)| f32::from(*width)).sum();
        let row_selector = format!("file-manager-row-{}-{}", pane.as_str(), entry.name);
        let mut row = div()
            .debug_selector(move || row_selector.clone())
            .id(SharedString::from(format!(
                "fm-row-{}-{identity}",
                pane.as_str()
            )))
            .w(px(width))
            .h(px(state.row_height as f32))
            .flex()
            .items_center()
            .overflow_hidden()
            .cursor_pointer()
            .bg(rgb(if selected { palette.hover } else { palette.bg }))
            .border_b_1()
            .border_color(rgb(if focused { palette.border } else { palette.bg }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    this.focus_pane(pane, window, cx);
                    let state = this.pane_mut(pane);
                    if event.modifiers.modified() || !state.selected_paths.contains(&press_identity)
                    {
                        state.select_visible(
                            press_identity.clone(),
                            event.modifiers.control || event.modifiers.platform,
                            event.modifiers.shift,
                            &visible,
                        );
                    }
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .on_click(
                cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                    cx.stop_propagation();
                    if !event.modifiers().modified() {
                        this.pane_mut(pane).select_entry(click_entry.identity_key());
                    }
                    if event.click_count() >= 2 {
                        this.open_entry(
                            pane,
                            click_entry.clone(),
                            FileOpenMode::Default,
                            window,
                            cx,
                        );
                    }
                    cx.notify();
                }),
            )
            .on_drag(payload, |payload, _, _, cx| {
                cx.new(|_| FileManagerDragPreview {
                    payload: payload.clone(),
                })
            });
        for &(column, width) in columns {
            let text = match column {
                FileBrowserSortColumn::Name => entry.name.clone(),
                FileBrowserSortColumn::Size => {
                    if entry.is_directory() {
                        "—".into()
                    } else {
                        format_browser_file_size(entry.size)
                    }
                }
                FileBrowserSortColumn::Modified => format_sftp_modified(entry.modified_at),
                FileBrowserSortColumn::Permissions => entry
                    .permissions
                    .map(format_permissions_octal)
                    .unwrap_or_else(|| "—".into()),
                FileBrowserSortColumn::Owner => entry.owner.clone(),
                FileBrowserSortColumn::Group => entry.group.clone(),
            };
            let mut cell = div()
                .w(width)
                .flex_none()
                .min_w_0()
                .h_full()
                .flex()
                .items_center()
                .px_2()
                .text_xs()
                .overflow_hidden();
            if column == FileBrowserSortColumn::Name {
                let tooltip = entry.name.clone();
                cell = cell
                    .gap_2()
                    .child(transfer_entry_icon(
                        palette,
                        &entry.name,
                        entry.is_directory(),
                        false,
                        false,
                    ))
                    .child(
                        div()
                            .id(SharedString::from(format!(
                                "fm-name-{}-{}",
                                pane.as_str(),
                                entry.identity_key()
                            )))
                            .min_w_0()
                            .truncate()
                            .child(text)
                            .tooltip(move |window, cx| {
                                NyaTooltip::new(tooltip.clone()).build(window, cx)
                            }),
                    );
            } else {
                if column == FileBrowserSortColumn::Size {
                    cell = cell.justify_end();
                }
                cell = cell.child(div().min_w_0().truncate().child(text));
            }
            row = row.child(cell);
        }
        let drop_directory = entry.remote_path();
        let external_directory = drop_directory.clone();
        let external_is_directory = entry.is_directory();
        let hover_directory = drop_directory.clone();
        let writable = entry.is_directory()
            && !entry.is_symlink()
            && !state.disconnected
            && (state.endpoint.session_id().is_some_and(|session| {
                self.read_app(cx, |app| app.file_sudo_status(session).is_sudo())
            }) || !entry
                .permissions
                .is_some_and(|permissions| permissions & 0o222 == 0));
        row = row
            .can_drop(move |drag, _, _| {
                drag.is::<gpui::ExternalPaths>()
                    || drag.downcast_ref::<FileManagerDragPayload>().is_some()
            })
            .drag_over::<FileManagerDragPayload>(move |style, _, _, _| style.bg(rgb(palette.hover)))
            .on_drag_move(cx.listener(
                move |this, event: &DragMoveEvent<FileManagerDragPayload>, window, cx| {
                    let payload = event.drag(cx);
                    payload
                        .same_pane_target
                        .set(payload.manager_id == this.id && payload.pane_id == pane);
                    let operation = drag_operation(
                        payload.manager_id == this.id && payload.pane_id == pane,
                        event.event.modifiers,
                    );
                    let valid = accepts_drop(
                        payload,
                        &this.pane(pane).endpoint,
                        &hover_directory,
                        writable,
                    );
                    let changed = payload.feedback.replace(operation) != operation;
                    if payload.valid_target.replace(valid) != valid || changed {
                        window.refresh();
                    }
                    cx.stop_propagation();
                },
            ))
            .on_drop(
                cx.listener(move |this, payload: &FileManagerDragPayload, window, cx| {
                    this.drop_files(payload, pane, drop_directory.clone(), writable, window, cx);
                    cx.stop_propagation();
                }),
            );
        row = row.on_drop(
            cx.listener(move |this, paths: &gpui::ExternalPaths, window, cx| {
                if external_is_directory {
                    this.drop_external_files(
                        paths,
                        pane,
                        external_directory.clone(),
                        writable,
                        window,
                        cx,
                    );
                    cx.stop_propagation();
                }
            }),
        );
        let workspace = cx.weak_entity();
        NyaContextMenu::new_dynamic(row, move |window, cx| {
            workspace
                .update(cx, |this, cx| {
                    this.focus_pane(pane, window, cx);
                    if !this.pane(pane).selected_paths.contains(&identity) {
                        this.pane_mut(pane).select_entry(identity.clone());
                    }
                    this.entry_menu(pane, entry.clone(), cx)
                })
                .unwrap_or_default()
        })
        .into_any_element()
    }

    fn entry_menu(
        &self,
        pane: PaneId,
        entry: SftpFileEntry,
        cx: &mut Context<Self>,
    ) -> Vec<NyaMenuItem> {
        let mut items = Vec::new();
        for (operation, label) in [
            (FileTransferOperation::Copy, t!("fileManager.copyToOther")),
            (FileTransferOperation::Move, t!("fileManager.moveToOther")),
        ] {
            let intent = self.transfer_intent(operation);
            items.push(NyaMenuItem::action(label).on_click(cx.listener(
                move |this, _, window, cx| {
                    let id = this.id.clone();
                    let intent = intent.clone();
                    this.dispatch_app(window, cx, move |app, _, cx| {
                        app.submit_file_transfer(id, intent, cx)
                    });
                },
            )));
        }
        for (kind, label) in [
            (FileManagerNameKind::Rename, t!("fileExplorer.cmRename")),
            (FileManagerNameKind::Folder, t!("fileExplorer.newFolder")),
            (FileManagerNameKind::File, t!("fileExplorer.newFile")),
        ] {
            let selection = self.selection(pane);
            items.push(
                NyaMenuItem::action(label)
                    .disabled(
                        matches!(kind, FileManagerNameKind::Rename) && selection.entries.len() != 1,
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let id = this.id.clone();
                        let selection = selection.clone();
                        this.dispatch_app(window, cx, move |app, window, cx| {
                            app.open_file_manager_name_target(id, selection, kind, window, cx)
                        });
                    })),
            );
        }
        let selection = self.selection(pane);
        items.push(
            NyaMenuItem::action(t!("fileExplorer.delete"))
                .danger()
                .on_click(cx.listener(move |this, _, window, cx| {
                    let id = this.id.clone();
                    let selection = selection.clone();
                    this.dispatch_app(window, cx, move |app, window, cx| {
                        app.confirm_file_manager_delete_selection(id, selection, window, cx)
                    });
                })),
        );
        for (mode, label) in [
            (FileOpenMode::Default, t!("fileExplorer.cmOpen")),
            (FileOpenMode::Preview, t!("fileManager.viewFile")),
            (FileOpenMode::Edit, t!("fileManager.editFile")),
        ] {
            let entry = entry.clone();
            let endpoint = self.pane(pane).endpoint.clone();
            items.push(NyaMenuItem::action(label).on_click(cx.listener(
                move |this, _, window, cx| {
                    if entry.is_directory() {
                        this.navigate_to(pane, entry.remote_path(), window, cx);
                    } else {
                        let entry = entry.clone();
                        let endpoint = endpoint.clone();
                        this.dispatch_app(window, cx, move |app, window, cx| {
                            app.open_file_for_endpoint(endpoint, entry, mode, window, cx)
                        });
                    }
                },
            )));
        }
        items
    }
}
