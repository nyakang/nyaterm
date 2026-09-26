use gpui::{
    Context, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, ParentElement as _,
    SharedString, Styled as _, div, prelude::*, px, rgb, uniform_list,
};
use nyaterm_transport::{SftpFileEntry, SftpFileType};
use nyaterm_ui::{NyaButton, NyaButtonVariant, NyaContextMenu, NyaUniformListScrollbar};
use rust_i18n::t;

use crate::features::view_widgets::transfer_entry_icon;

use super::panel::TransferPanel;

pub(super) fn transfer_tree_view(
    panel: &TransferPanel,
    cx: &mut Context<TransferPanel>,
) -> impl IntoElement {
    let snapshot = panel.snapshot().expect("transfer snapshot");
    let tree = &snapshot.browser.tree;
    let count = tree.rows.len();
    let scroll = tree.scroll.clone();
    let menu_app = panel.app_handle();
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
                app.handle_transfer_tree_key_down(event, window, cx)
            });
        }))
        .child(
            NyaContextMenu::new_dynamic(
                div()
                    .id("transfer-tree-viewport")
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
                        cx.processor(|panel, range: std::ops::Range<usize>, _, cx| {
                            let Some(snapshot) = panel.snapshot() else {
                                return Vec::new();
                            };
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
                                    div()
                                        .id(SharedString::from(format!(
                                            "transfer-tree-row:{}",
                                            row.key
                                        )))
                                        .h(px(28.))
                                        .w_full()
                                        .min_w_0()
                                        .flex()
                                        .items_center()
                                        .gap_1()
                                        .pl(px((row.depth.min(8) * 12) as f32))
                                        .pr_1()
                                        .bg(if is_selected {
                                            gpui::rgba((palette.primary << 8) | 0x1a)
                                        } else if is_focused {
                                            rgb(palette.hover)
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
                                        .when(!is_selected, |this| {
                                            this.hover(|this| this.bg(rgb(palette.hover)))
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
                                        .child(transfer_entry_icon(
                                            palette,
                                            &row.label,
                                            row.directory,
                                            is_symlink,
                                            is_selected,
                                        ))
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
                                                nyaterm_ui::NyaTooltip::new(details.clone())
                                                    .build(window, cx)
                                            })
                                        })
                                        .into_any_element()
                                })
                                .collect()
                        }),
                    )
                    .size_full()
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

fn transfer_tree_hover_details(entry: &SftpFileEntry) -> String {
    let kind = match entry.file_type {
        SftpFileType::File => t!("fileExplorer.typeFile"),
        SftpFileType::Directory => t!("fileExplorer.typeDirectory"),
        SftpFileType::Symlink if entry.symlink_target_is_directory => {
            t!("fileExplorer.typeDirectorySymlink")
        }
        SftpFileType::Symlink => t!("fileExplorer.typeSymlink"),
        SftpFileType::Other => t!("fileExplorer.typeOther"),
    };
    let mut lines = vec![entry.name.clone(), kind.to_string()];
    if let Some(size) = entry.size.filter(|_| !entry.is_directory()) {
        lines.push(
            t!(
                "fileExplorer.detailSize",
                value = crate::features::transfers::format_file_size(Some(size))
            )
            .to_string(),
        );
    }
    if let Some(mode) = entry.permissions {
        lines.push(
            t!(
                "fileExplorer.detailPermissions",
                value = crate::features::formatting::format_permissions_octal(mode)
            )
            .to_string(),
        );
    }
    if !entry.owner.is_empty() || !entry.group.is_empty() {
        lines.push(
            t!(
                "fileExplorer.detailOwner",
                value = format!("{}:{}", entry.owner, entry.group)
            )
            .to_string(),
        );
    }
    let modified = super::format_sftp_modified(entry.modified_at);
    if !modified.is_empty() {
        lines.push(t!("fileExplorer.detailModified", value = modified).to_string());
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use nyaterm_transport::{SftpFileEntry, SftpFileType};

    use super::transfer_tree_hover_details;

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
        assert!(details.starts_with("report.txt\n"));
        assert!(details.contains("1.5 KiB"));
        assert!(details.contains("0640"));
        assert!(details.contains("alice:staff"));
    }
}
