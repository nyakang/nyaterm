use super::panel::TransferPanel;
use crate::features::transfers::TransferTreePresentation;
use crate::features::view_widgets::transfer_entry_icon;
use crate::theme::ThemePalette;
use gpui::{
    AnyElement, App, Bounds, BoxShadow, Context, FontWeight, IntoElement, KeyDownEvent,
    MouseButton, MouseDownEvent, ParentElement as _, Pixels, Point, SharedString, Styled as _,
    UniformListDecoration, Window, div, prelude::*, px, rgb, svg, uniform_list,
};
use nyaterm_files::file_browser::{
    FileBrowserBackendKind, file_browser_parent, file_browser_path_eq,
};
use nyaterm_files::sftp::{SftpFileEntry, SftpFileType};
use nyaterm_ui::{NyaButton, NyaButtonVariant, NyaContextMenu, NyaUniformListScrollbar};
use rust_i18n::t;
use std::ops::Range;

pub(super) fn transfer_tree_view(
    panel: &TransferPanel,
    cx: &mut Context<TransferPanel>,
) -> impl IntoElement {
    let snapshot = panel.snapshot().expect("transfer snapshot");
    let tree = &snapshot.browser.tree;
    let count = tree.rows.len();
    let scroll = tree.scroll.clone();
    let menu_app = panel.app_handle();
    let position_hint = TreePositionHint {
        tree: tree.clone(),
        local_backend: snapshot.browser.local_backend,
        palette: snapshot.chrome.palette,
    };
    div()
        .id("transfer-directory-tree")
        .w_full()
        .flex_1()
        .h_full()
        .min_h_0()
        .overflow_hidden()
        .track_focus(&snapshot.browser.tree_focus)
        .on_key_down(cx.listener(|panel, event: &KeyDownEvent, window, cx| {
            panel.with_app(cx, |app, cx| {
                app.handle_transfer_browser_key_down(event, window, cx)
            });
        }))
        .child(
            NyaContextMenu::new_dynamic(
                div()
                    .id("transfer-tree-viewport")
                    .debug_selector(|| "transfer-tree-viewport".to_string())
                    .relative()
                    .size_full()
                    .min_h_0()
                    .overflow_hidden()
                    .capture_any_mouse_down(cx.listener(
                        |panel, event: &MouseDownEvent, _, cx| {
                            if event.button == MouseButton::Right {
                                panel.with_app(cx, |app, cx| {
                                    app.begin_transfer_browser_context_menu(cx);
                                });
                            }
                        },
                    ))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|panel, _: &MouseDownEvent, window, cx| {
                            panel.with_app(cx, |app, cx| {
                                app.prepare_transfer_tree_current_context_menu(window, cx);
                            });
                        }),
                    )
                    .child(
                    uniform_list(
                        "transfer-tree-rows",
                        count,
                        cx.processor(|panel, range: std::ops::Range<usize>, window, cx| {
                            let Some(snapshot) = panel.snapshot() else {
                                return Vec::new();
                            };
                            #[cfg(test)]
                            panel.note_rows_built(range.len());
                            let palette = snapshot.chrome.palette;
                            let tree = &snapshot.browser.tree;
                            let rows = tree.rows.clone();
                            let selected = tree.selected.clone();
                            let focused = tree.focused.clone();
                            let rename = snapshot.browser.rename.clone();
                            let rename_field = snapshot.browser.rename_field.clone();
                            range
                                .filter_map(|index| rows.get(index).cloned())
                                .map(|row| {
                                    let id = row.key.clone();
                                    let expand_id = row.key.clone();
                                    let context_id = row.key.clone();
                                    let context_is_root = row.root;
                                    let is_selected = selected.contains(&row.key);
                                    let is_focused = focused.as_ref() == Some(&row.key);
                                    let is_terminal_cwd = row.directory
                                        && snapshot.browser.terminal_cwd.as_deref().is_some_and(|cwd| {
                                            file_browser_path_eq(
                                                if snapshot.browser.local_backend {
                                                    FileBrowserBackendKind::Local
                                                } else {
                                                    FileBrowserBackendKind::Remote
                                                },
                                                &row.path.display_path,
                                                cwd,
                                            )
                                        });
                                    let is_symlink = row
                                        .entry
                                        .as_ref()
                                        .is_some_and(|entry| entry.file_type == SftpFileType::Symlink);
                                    let hover_details =
                                        row.entry.as_ref().map(transfer_tree_hover_details);
                                    let is_renaming = row.entry.as_ref().is_some_and(|entry| {
                                        rename
                                            .as_ref()
                                            .is_some_and(|state| state.old_path == entry.path)
                                    });
                                    let rename_input = is_renaming
                                        .then(|| rename_field.clone())
                                        .flatten()
                                        .map(|field| {
                                            nyaterm_ui::NyaInputShell::new(
                                                SharedString::from(format!(
                                                    "transfer.tree.rename.{}",
                                                    row.key
                                                )),
                                                &field,
                                            )
                                            .compact()
                                            .into_any_element()
                                        });
                                    let drag_app = panel.app_handle();
                                    let drag_name = row.label.clone();
                                    let drag_entry = row.entry.clone().filter(|_| !is_renaming && crate::features::transfers::drag_export::transfer_drag_supported(snapshot.browser.local_backend, window.supports_virtual_file_drag(), window.supports_file_promise_drag())).filter(|entry| matches!(entry.file_type, SftpFileType::File | SftpFileType::Directory));
                                    let row_group = SharedString::from(format!("transfer-tree-row:{}", row.key));
                                    let content = div()
                                        .id(row_group.clone())
                                        .group(row_group.clone())
                                        .debug_selector({
                                            let key = row.key.clone();
                            move || format!("transfer-tree-row:{key}")
                                        })
                                        .h(px(24.))
                                        .w_full()
                                        .min_w_0()
                                        .flex()
                                        .items_center()
                                        .gap_1()
                                        .pl(px((row.depth.min(8) * 12) as f32))
                                        .pr_1()
                                        .rounded_md()
                                        .border_l_2()
                                        .border_color(if is_terminal_cwd || is_focused {
                                            rgb(palette.primary)
                                        } else {
                                            gpui::rgba(0)
                                        })
                                        .bg(if is_selected {
                                            gpui::rgba((palette.primary << 8) | 0x30)
                                        } else if is_focused || is_terminal_cwd {
                                            gpui::rgba((palette.primary << 8) | 0x16)
                                        } else {
                                            gpui::rgba(0)
                                        })
                                        .text_size(px(11.))
                                        .text_color(if is_selected {
                                            rgb(palette.primary)
                                        } else {
                                            rgb(palette.text)
                                        })
                                        .cursor_pointer()
                                        .hover(|this| {
                                            this.bg(if is_selected {
                                                gpui::rgba((palette.primary << 8) | 0x38)
                                            } else {
                                                rgb(palette.surface_elevated)
                                                    .blend(rgb(palette.text).alpha(0.045))
                                            })
                                            // Keep hover neutral; the primary accent denotes selection and focus.
                                            .shadow(vec![
                                                BoxShadow::new(px(0.), px(0.), rgb(palette.text).alpha(0.08).into())
                                                    .spread_radius(px(1.)),
                                                BoxShadow::new(px(0.), px(2.), gpui::rgba(palette.shadow).opacity(0.8).into())
                                                    .blur_radius(px(8.)),
                                            ])
                                        })
                                        .when_some(drag_entry, |this, entry| {
                                            let drag = crate::features::transfers::drag_export::DraggedSelection::new_tree(entry);
                                            super::drag_preview::with_transfer_drag(this, drag, drag_app, drag_name, palette)
                                        })
                                        .on_click(cx.listener(
                                            move |panel, event: &gpui::ClickEvent, window, cx| {
                                                cx.stop_propagation();
                                                panel.with_app(cx, |app, cx| {
                                                    let modifiers = event.modifiers();
                                                    app.select_transfer_tree_row(
                                                        id.clone(),
                                                        modifiers.platform || modifiers.control,
                                                        modifiers.shift,
                                                        window,
                                                        cx,
                                                    );
                                                    if event.click_count() == 2 {
                                                        app.double_click_transfer_tree_row(
                                                            window, cx,
                                                        );
                                                    }
                                                });
                                            },
                                        ))
                                        .on_mouse_down(
                                            MouseButton::Right,
                                            cx.listener(move |panel, _, window, cx| {
                                                cx.stop_propagation();
                                                panel.with_app(cx, |app, cx| {
                                                    if context_is_root {
                                                        app.prepare_transfer_tree_current_context_menu(
                                                            window, cx,
                                                        );
                                                    } else {
                                                        app.prepare_transfer_tree_entry_context_menu(
                                                            context_id.clone(),
                                                            window,
                                                            cx,
                                                        );
                                                    }
                                                });
                                            }),
                                        )
                                        .child(div().w(px(24.)).h(px(24.)).flex_none().when(
                                            row.expandable,
                                            |this| {
                                                this.child(
                                                    NyaButton::new(
                                                        SharedString::from(format!(
                                                            "tree-expand:{}",
                                                            row.key
                                                        )),
                                                        "",
                                                    )
                                                    .icon(if row.expanded {
                                                        "icons/chevron-down.svg"
                                                    } else {
                                                        "icons/menu/chevron-right.svg"
                                                    })
                                                    .small()
                                                    .compact()
                                                    .variant(NyaButtonVariant::Ghost)
                                                    .loading(row.loading)
                                                    .tooltip(if row.expanded {
                                                        t!("fileExplorer.collapseFolder")
                                                    } else {
                                                        t!("fileExplorer.expandFolder")
                                                    })
                                                    .on_click(cx.listener(
                                                        move |panel, _, window, cx| {
                                                            cx.stop_propagation();
                                                            panel.with_app(cx, |app, cx| {
                                                                app.select_transfer_tree_row(
                                                                    expand_id.clone(),
                                                                    false,
                                                                    false,
                                                                    window,
                                                                    cx,
                                                                );
                                                                app.expand_transfer_tree_row(
                                                                    &expand_id, None, cx,
                                                                );
                                                            });
                                                        },
                                                    )),
                                                )
                                            },
                                        ))
                                        .child(
                                            div()
                                                .size(px(20.))
                                                .flex_none()
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .rounded_sm()
                                                .group_hover(row_group, |this| {
                                                    this.bg(rgb(palette.text).alpha(0.08))
                                                        .shadow(vec![BoxShadow::new(
                                                            px(0.), px(1.), gpui::rgba(palette.shadow).opacity(0.45).into(),
                                                        ).blur_radius(px(2.))])
                                                })
                                                .child(transfer_entry_icon(
                                                    palette,
                                                    &row.label,
                                                    row.directory,
                                                    is_symlink,
                                                    is_selected,
                                                )),
                                        )
                                        .when(is_renaming, |this| {
                                            this.child(
                                                div()
                                                    .h(px(24.))
                                                    .min_w(px(80.))
                                                    .flex_1()
                                                    .on_mouse_down(
                                                        MouseButton::Left,
                                                        |_, _, cx| cx.stop_propagation(),
                                                    )
                                                    .on_key_down(cx.listener(
                                                        |panel, event: &KeyDownEvent, window, cx| {
                                                            panel.with_app(cx, |app, cx| {
                                                                app.handle_transfer_rename_key_down(
                                                                    event, window, cx,
                                                                );
                                                            });
                                                        },
                                                    ))
                                                    .children(rename_input),
                                            )
                                        })
                                        .when(!is_renaming, |this| {
                                            this.child(
                                                div()
                                                    .min_w_0()
                                                    .flex_1()
                                                    .overflow_hidden()
                                                    .text_ellipsis()
                                                    .child(row.label),
                                            )
                                        })
                                        .when(is_terminal_cwd, |this| {
                                            this.child(
                                                div()
                                                    .id(SharedString::from(format!(
                                                        "tree-terminal-cwd:{}", row.key
                                                    )))
                                                    .debug_selector({
                                                        let key = row.key.clone();
                                                        move || format!("tree-terminal-cwd:{key}")
                                                    })
                                                    .flex_none()
                                                    .flex()
                                                    .items_center()
                                                    .px_1()
                                                    .text_color(rgb(palette.primary))
                                                    .child(svg().path("icons/conn/terminal.svg").size(px(14.)))
                                                    .tooltip(|window, cx| {
                                                        nyaterm_ui::NyaTooltip::new(
                                                            t!("fileExplorer.currentTerminalDirectory").to_string(),
                                                        )
                                                        .build(window, cx)
                                                    }),
                                            )
                                        })
                                        .when_some(row.error, |this, error| {
                                            this.child(
                                                div()
                                                    .id(SharedString::from(format!(
                                                        "tree-error:{}",
                                                        row.key
                                                    )))
                                                    .flex_none()
                                                    .text_color(rgb(palette.text_muted))
                                                    .child("!")
                                                    .tooltip(move |window, cx| {
                                                        nyaterm_ui::NyaTooltip::new(error.clone())
                                                            .build(window, cx)
                                                    }),
                                            )
                                        })
                                        .when_some(hover_details, |this, details| {
                                            this.tooltip(move |window, cx| {
                                                let details = details.clone();
                                                nyaterm_ui::NyaTooltip::element(move |_, _| {
                                                    transfer_tree_hover_card(&details, palette)
                                                })
                                                .build(window, cx)
                                            })
                                        });
                                    div()
                                        .h(px(28.))
                                        .w_full()
                                        .px(px(4.))
                                        .py(px(2.))
                                        .child(content)
                                        .into_any_element()
                                })
                                .collect()
                        }),
                    )
                    .size_full()
                    .with_decoration(position_hint)
                    .track_scroll(&scroll),
                )
                    .child(
                    div()
                        .absolute()
                        .inset_0()
                        .child(NyaUniformListScrollbar::new(
                            "transfer-tree-scrollbar",
                            &scroll,
                        )),
                ),
                move |_, cx| {
                    menu_app
                        .update(cx, |app, cx| app.transfer_browser_context_menu_items(cx))
                        .unwrap_or_default()
                },
            )
            .min_width(px(208.)),
        )
}

struct TreePositionHint {
    tree: TransferTreePresentation,
    local_backend: bool,
    palette: ThemePalette,
}

impl UniformListDecoration for TreePositionHint {
    fn compute(
        &self,
        visible_range: Range<usize>,
        bounds: Bounds<Pixels>,
        scroll_offset: Point<Pixels>,
        _item_height: Pixels,
        _item_count: usize,
        _window: &mut Window,
        _cx: &mut App,
    ) -> AnyElement {
        let Some(row) = self
            .tree
            .rows
            .get(visible_range.start)
            .filter(|_| visible_range.start > 0)
        else {
            return div().into_any_element();
        };
        let path = if row.directory {
            row.path.display_path.clone()
        } else {
            file_browser_parent(
                if self.local_backend {
                    FileBrowserBackendKind::Local
                } else {
                    FileBrowserBackendKind::Remote
                },
                &row.path.display_path,
            )
        };
        // Decorations use content coordinates; cancel the offset to pin this to the viewport.
        div()
            .relative()
            .size_full()
            .child(
                div()
                    .id("transfer-tree-position")
                    .debug_selector(|| "transfer-tree-position".to_string())
                    .absolute()
                    .left(-scroll_offset.x + px(8.))
                    .top(-scroll_offset.y + bounds.size.height - px(26.))
                    .h(px(20.))
                    .max_w((bounds.size.width - px(24.)).max(px(0.)))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_1()
                    .rounded_sm()
                    .border_1()
                    .border_color(rgb(self.palette.border))
                    .bg(rgb(self.palette.surface))
                    .text_size(px(11.))
                    .text_color(rgb(self.palette.text_muted))
                    .child(
                        svg()
                            .path("icons/conn/folder.svg")
                            .size(px(12.))
                            .flex_none(),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(path),
                    ),
            )
            .into_any_element()
    }
}

#[derive(Clone)]
struct TransferTreeHoverDetails {
    name: String,
    kind: String,
    metadata: Vec<(String, String)>,
}

fn transfer_tree_hover_card(
    details: &TransferTreeHoverDetails,
    palette: ThemePalette,
) -> AnyElement {
    div()
        .id("transfer-tree-details")
        .debug_selector(|| "transfer-tree-details".to_string())
        .w(px(208.))
        .py_1()
        .flex()
        .flex_col()
        .gap_2()
        .text_size(px(11.))
        .text_color(rgb(palette.text))
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .w_full()
                        .text_size(px(12.))
                        .font_weight(FontWeight(600.))
                        .child(details.name.clone()),
                )
                .child(
                    div().flex().child(
                        div()
                            .px_1()
                            .rounded_sm()
                            .bg(rgb(palette.hover))
                            .text_size(px(10.))
                            .text_color(rgb(palette.text_muted))
                            .child(details.kind.clone()),
                    ),
                ),
        )
        .when(!details.metadata.is_empty(), |this| {
            this.child(
                div()
                    .pt_2()
                    .border_t_1()
                    .border_color(rgb(palette.border))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .children(details.metadata.iter().map(|(label, value)| {
                        div()
                            .flex()
                            .items_start()
                            .gap_2()
                            .child(
                                div()
                                    .w(px(72.))
                                    .flex_none()
                                    .text_color(rgb(palette.text_muted))
                                    .child(label.clone()),
                            )
                            .child(div().min_w_0().flex_1().child(value.clone()))
                    })),
            )
        })
        .into_any_element()
}

fn transfer_tree_hover_details(entry: &SftpFileEntry) -> TransferTreeHoverDetails {
    let kind = match entry.file_type {
        SftpFileType::File => t!("fileExplorer.typeFile"),
        SftpFileType::Directory => t!("fileExplorer.typeDirectory"),
        SftpFileType::Symlink if entry.symlink_target_is_directory => {
            t!("fileExplorer.typeDirectorySymlink")
        }
        SftpFileType::Symlink => t!("fileExplorer.typeSymlink"),
        SftpFileType::Other => t!("fileExplorer.typeOther"),
    };
    let mut metadata = Vec::new();
    if let Some(size) = entry.size.filter(|_| !entry.is_directory()) {
        metadata.push((
            t!("fileExplorer.size").to_string(),
            crate::features::transfers::format_file_size(Some(size)),
        ));
    }
    if let Some(mode) = entry.permissions {
        metadata.push((
            t!("fileExplorer.permissions").to_string(),
            crate::features::formatting::format_permissions_octal(mode),
        ));
    }
    if !entry.owner.is_empty() || !entry.group.is_empty() {
        metadata.push((
            t!("fileExplorer.ownership").to_string(),
            format!("{}: {}", entry.owner, entry.group),
        ));
    }
    if entry.modified_at.is_some() {
        metadata.push((
            t!("fileExplorer.mtime").to_string(),
            super::format_sftp_modified(entry.modified_at),
        ));
    }
    TransferTreeHoverDetails {
        name: entry.name.clone(),
        kind: kind.to_string(),
        metadata,
    }
}

#[cfg(test)]
mod tests {

    use super::transfer_tree_hover_details;
    use nyaterm_files::sftp::{SftpFileEntry, SftpFileType};

    #[test]
    fn hover_details_omit_unknown_metadata_and_directory_sizes() {
        let entry = SftpFileEntry {
            name: "folder".into(),
            path: "/folder".into(),
            file_type: SftpFileType::Directory,
            size: Some(4096),
            permissions: None,
            owner: String::new(),
            group: String::new(),
            modified_at: None,
            raw_path_token: None,
            symlink_target_is_directory: false,
        };
        let details = transfer_tree_hover_details(&entry);
        assert_eq!(details.name, "folder");
        assert!(!details.kind.is_empty());
        assert!(details.metadata.is_empty());
    }

    #[test]
    fn hover_details_use_only_cached_entry_metadata() {
        let entry = SftpFileEntry {
            name: "report.txt".into(),
            path: "/report.txt".into(),
            file_type: SftpFileType::File,
            size: Some(1536),
            permissions: Some(0o100640),
            owner: "alice".into(),
            group: "staff".into(),
            modified_at: Some(1_700_000_000),
            raw_path_token: None,
            symlink_target_is_directory: false,
        };
        let details = transfer_tree_hover_details(&entry);
        assert_eq!(details.name, "report.txt");
        assert!(!details.kind.is_empty());
        assert_eq!(details.metadata.len(), 4);
        assert_eq!(details.metadata[0].1, "1.5 KiB");
        assert_eq!(details.metadata[1].1, "0640");
        assert_eq!(details.metadata[2].1, "alice: staff");
        assert!(!details.metadata[3].1.is_empty());
    }
}
