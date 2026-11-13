use super::{format_permissions_octal, format_sftp_modified};
use crate::features::pages::transfers::panel::TransferPanel;
use crate::features::{shell::gpui_code_font_family, view_widgets::transfer_entry_icon};
use crate::models::{TransferBrowserColumnWidths, TransferRenameState};
use crate::theme::ThemePalette;
use gpui::{
    AnyElement, ClickEvent, Context, FontWeight, InteractiveElement as _, IntoElement,
    KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _,
    Pixels, SharedString, StatefulInteractiveElement as _, Styled as _, div, prelude::*, px, rgb,
};
use nyaterm_files::sftp::{SftpFileEntry, SftpFileType};
use nyaterm_ui::{NyaHoverCard, NyaPopoverAlign, NyaPopoverPlacement};
use std::collections::HashSet;

pub(in crate::features) fn format_browser_file_size(size: Option<u64>) -> String {
    match size {
        None | Some(0) => "-".to_string(),
        Some(size) if size < 1024 => format!("{size} B"),
        Some(size) => {
            const UNITS: [&str; 6] = ["KB", "MB", "GB", "TB", "PB", "EB"];
            let mut unit_index = 0;
            let mut divisor = 1024_u64;
            while size / divisor >= 1024 && unit_index < UNITS.len() - 1 {
                divisor *= 1024;
                unit_index += 1;
            }
            format!("{:.1} {}", size as f64 / divisor as f64, UNITS[unit_index])
        }
    }
}

pub(super) fn transfer_browser_parent_entry_row(
    palette: ThemePalette,
    column_widths: TransferBrowserColumnWidths,
    cx: &mut Context<TransferPanel>,
) -> impl IntoElement {
    div()
        .id(SharedString::from("transfer-browser-entry-parent"))
        .h(px(30.))
        .flex()
        .items_center()
        .rounded_sm()
        .bg(gpui::rgba(0x00000000))
        .cursor_pointer()
        .hover(|this| this.bg(rgb(palette.hover)))
        .on_click(cx.listener(|panel, event: &ClickEvent, window, cx| {
            if event.click_count() >= 2 && !event.modifiers().modified() {
                panel.with_app(cx, |this, cx| {
                    this.open_transfer_parent_directory(window, cx);
                });
            }
        }))
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |panel, _: &MouseDownEvent, window, cx| {
                panel.with_app(cx, |this, cx| {
                    this.prepare_transfer_browser_parent_context_menu(window, cx);
                })
            }),
        )
        .child(
            div()
                .min_w_0()
                .w(column_widths.name)
                .flex_none()
                .flex()
                .items_center()
                .gap_2()
                .px_2()
                .text_size(px(12.))
                .text_color(rgb(palette.text))
                .child(transfer_entry_icon(palette, "..", true, false, false))
                .child(".."),
        )
        .child(transfer_browser_text_cell(
            palette,
            column_widths.modified,
            "",
        ))
        .child(transfer_browser_text_cell(palette, column_widths.size, "-"))
        .child(transfer_browser_text_cell(
            palette,
            column_widths.permissions,
            "",
        ))
        .child(transfer_browser_text_cell(palette, column_widths.owner, ""))
        .child(transfer_browser_text_cell(palette, column_widths.group, ""))
}

fn transfer_browser_text_cell(
    palette: ThemePalette,
    width: Pixels,
    value: &'static str,
) -> impl IntoElement {
    div()
        .w(width)
        .flex_none()
        .px_2()
        .truncate()
        .text_xs()
        .text_color(rgb(palette.text_muted))
        .child(value)
}

fn transfer_browser_detail_row(
    palette: ThemePalette,
    label: impl Into<SharedString>,
    value: impl Into<SharedString>,
) -> impl IntoElement {
    div()
        .flex()
        .items_start()
        .gap_3()
        .child(
            div()
                .w(px(88.))
                .flex_none()
                .text_color(rgb(palette.text_muted))
                .child(label.into()),
        )
        .child(
            div()
                .min_w_0()
                .flex_1()
                .whitespace_normal()
                .child(value.into()),
        )
}

pub(super) fn transfer_browser_entry_row(
    presentation: TransferBrowserEntryRowPresentation<'_>,
    cx: &mut Context<TransferPanel>,
) -> impl IntoElement {
    let TransferBrowserEntryRowPresentation {
        palette,
        entry,
        selected_remote_path,
        selected_remote_paths,
        column_widths,
        rename_state,
        rename_input,
        local_backend,
        virtual_drag_supported,
        app_handle,
    } = presentation;
    let entry_identity = entry.identity_key();
    let drag =
        crate::features::transfers::drag_export::DraggedSelection::new(entry_identity.clone());
    let drag_app = app_handle;
    let drag_name = entry.name.clone();
    let mouse_down_path = entry_identity.clone();
    let mouse_move_path = entry_identity.clone();
    let mouse_up_out_path = entry_identity.clone();
    let context_path = entry_identity.clone();
    let is_selected = selected_remote_path.as_deref() == Some(entry_identity.as_str());
    let is_marked = selected_remote_paths.contains(&entry_identity);
    let inline_rename = rename_state.filter(|state| {
        state.old_path == entry.path && state.raw_path_token == entry.raw_path_token
    });
    let is_renaming = inline_rename.is_some();
    let name_click_path = entry_identity.clone();
    let rename_double_click_path = entry_identity.clone();
    let rename_input_path = entry.path.clone();
    let mut rename_input = rename_input;
    let rename_has_error = inline_rename.as_ref().is_some_and(|state| {
        let trimmed = state.value.trim();
        trimmed.is_empty() || trimmed.contains('/') || trimmed == "." || trimmed == ".."
    });
    let is_directory = entry.is_directory();
    let is_marked_or_selected = is_selected || is_marked;
    let size_display = if is_directory {
        "-".to_string()
    } else {
        format_browser_file_size(entry.size)
    };
    let modified_display = format_sftp_modified(entry.modified_at);
    let permissions_display = entry
        .permissions
        .map(format_permissions_octal)
        .unwrap_or_else(|| "-".to_string());
    let detail = div()
        .w(px(296.))
        .max_w(px(296.))
        .flex()
        .flex_col()
        .gap_2()
        .text_size(px(12.))
        .line_height(px(18.))
        .text_color(rgb(palette.text))
        .child(
            div()
                .w_full()
                .pb_2()
                .border_b_1()
                .border_color(rgb(palette.border))
                .text_size(px(13.))
                .font_weight(FontWeight(600.))
                .whitespace_normal()
                .child(entry.name.clone()),
        )
        .child(transfer_browser_detail_row(
            palette,
            rust_i18n::t!("fileExplorer.mtime").to_string(),
            modified_display.clone(),
        ))
        .child(transfer_browser_detail_row(
            palette,
            rust_i18n::t!("fileExplorer.size").to_string(),
            size_display.clone(),
        ))
        .child(transfer_browser_detail_row(
            palette,
            rust_i18n::t!("fileExplorer.permissions").to_string(),
            permissions_display.clone(),
        ))
        .child(transfer_browser_detail_row(
            palette,
            rust_i18n::t!("fileExplorer.owner").to_string(),
            if entry.owner.is_empty() {
                "-".to_string()
            } else {
                entry.owner.clone()
            },
        ))
        .child(transfer_browser_detail_row(
            palette,
            rust_i18n::t!("fileExplorer.group").to_string(),
            if entry.group.is_empty() {
                "-".to_string()
            } else {
                entry.group.clone()
            },
        ));
    div()
        .id(SharedString::from(format!(
            "transfer-browser-entry-{entry_identity}"
        )))
        .debug_selector({
            let entry_identity = entry_identity.clone();
            move || format!("transfer-browser-entry-{entry_identity}")
        })
        .h(px(30.))
        .flex()
        .items_center()
        .bg(if is_marked_or_selected {
            gpui::rgba((palette.primary << 8) | 0x1a)
        } else {
            gpui::rgba(0x00000000)
        })
        .cursor_pointer()
        .when(!is_marked_or_selected, |this| {
            this.hover(|this| this.bg(rgb(palette.hover)))
        })
        .when(
            !is_renaming
                && (local_backend
                    || ((virtual_drag_supported || cfg!(target_os = "linux"))
                        && matches!(
                            entry.file_type,
                            SftpFileType::File | SftpFileType::Directory
                        ))),
            |this| {
                super::drag_preview::with_transfer_drag(this, drag, drag_app, drag_name, palette)
            },
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |panel, event: &MouseDownEvent, window, cx| {
                panel.with_app(cx, |this, cx| {
                    if !is_renaming {
                        this.handle_transfer_browser_entry_mouse_down(
                            mouse_down_path.clone(),
                            event,
                            window,
                            cx,
                        );
                    }
                })
            }),
        )
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |panel, _: &MouseDownEvent, window, cx| {
                panel.with_app(cx, |this, cx| {
                    this.prepare_transfer_browser_entry_context_menu(
                        context_path.clone(),
                        window,
                        cx,
                    );
                })
            }),
        )
        .on_mouse_move(cx.listener(move |panel, event: &MouseMoveEvent, _, cx| {
            panel.with_app(cx, |this, cx| {
                if !is_renaming {
                    this.handle_transfer_browser_entry_mouse_move(
                        mouse_move_path.clone(),
                        event,
                        cx,
                    );
                }
            })
        }))
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(|panel, event: &MouseUpEvent, _, cx| {
                panel.with_app(cx, |this, cx| {
                    this.finish_transfer_browser_selection_drag(event, cx);
                })
            }),
        )
        .on_click(cx.listener(move |panel, event: &ClickEvent, window, cx| {
            panel.with_app(cx, |this, cx| {
                if !is_renaming {
                    this.select_transfer_browser_entry_from_click(
                        entry_identity.clone(),
                        event,
                        window,
                        cx,
                    );
                }
            })
        }))
        .on_mouse_up_out(
            MouseButton::Left,
            cx.listener(move |panel, _, _, cx| {
                panel.with_app(cx, |this, _| {
                    this.transfer
                        .consume_browser_selection_click(&mouse_up_out_path);
                });
            }),
        )
        .child(
            div()
                .min_w_0()
                .w(column_widths.name)
                .flex_none()
                .flex()
                .items_center()
                .gap_2()
                .px_2()
                .text_size(px(12.))
                .text_color(if is_marked_or_selected {
                    rgb(palette.primary)
                } else {
                    rgb(palette.text)
                })
                .child(transfer_entry_icon(
                    palette,
                    &entry.name,
                    is_directory,
                    entry.file_type == SftpFileType::Symlink,
                    is_marked_or_selected,
                ))
                .when(is_renaming, |this| {
                    this.child(
                        div()
                            .id(SharedString::from(format!(
                                "transfer-browser-rename-input-{rename_input_path}"
                            )))
                            .min_w_0()
                            .flex_1()
                            .font_family(gpui_code_font_family())
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |panel, event: &MouseDownEvent, window, cx| {
                                    panel.with_app(cx, |this, cx| {
                                        if event.click_count >= 2 && !event.modifiers.modified() {
                                            this.open_transfer_browser_entry_from_double_click(
                                                rename_double_click_path.clone(),
                                                window,
                                                cx,
                                            );
                                        }
                                        cx.stop_propagation();
                                    })
                                }),
                            )
                            .on_mouse_down(
                                MouseButton::Right,
                                cx.listener(|panel, _, _, cx| {
                                    panel.with_app(cx, |this, cx| {
                                        this.suppress_transfer_browser_context_menu(cx);
                                        cx.stop_propagation();
                                    })
                                }),
                            )
                            .on_click(|_, _, cx| {
                                cx.stop_propagation();
                            })
                            // A name that cannot be saved reddens the box.
                            .when(rename_has_error, |this| {
                                this.rounded_sm().border_1().border_color(rgb(0x7f1d1d))
                            })
                            // Escape and Enter belong to the row, which the box
                            // leaves unconsumed.
                            .on_key_down(cx.listener(|panel, event: &KeyDownEvent, window, cx| {
                                panel.with_app(cx, |this, cx| {
                                    this.handle_transfer_rename_key_down(event, window, cx);
                                })
                            }))
                            .children(rename_input.take()),
                    )
                })
                .when(!is_renaming, |this| {
                    this.child(
                        NyaHoverCard::new(
                            format!("transfer-browser-entry-detail-{name_click_path}"),
                            div()
                                .id(SharedString::from(format!(
                                    "transfer-browser-entry-name-{name_click_path}"
                                )))
                                .min_w_0()
                                .flex_1()
                                .on_click(cx.listener(move |panel, event: &ClickEvent, _, cx| {
                                    panel.with_app(cx, |this, cx| {
                                        this.schedule_transfer_browser_name_rename(
                                            name_click_path.clone(),
                                            event,
                                            cx,
                                        );
                                    })
                                }))
                                .truncate()
                                .child(entry.name.clone()),
                            detail,
                        )
                        .placement(NyaPopoverPlacement::Top)
                        .flexible_trigger()
                        .align(NyaPopoverAlign::Start)
                        .open_delay(std::time::Duration::from_millis(800))
                        .close_delay(std::time::Duration::from_millis(100)),
                    )
                }),
        )
        .child(
            div()
                .w(column_widths.modified)
                .flex_none()
                .px_2()
                .truncate()
                .text_xs()
                .font_family(gpui_code_font_family())
                .text_color(rgb(palette.text_muted))
                .child(modified_display),
        )
        .child(
            div()
                .w(column_widths.size)
                .flex_none()
                .px_2()
                .truncate()
                .text_right()
                .text_xs()
                .text_color(rgb(palette.text_muted))
                .child(size_display),
        )
        .child(
            div()
                .w(column_widths.permissions)
                .flex_none()
                .px_2()
                .truncate()
                .text_xs()
                .font_family(gpui_code_font_family())
                .text_color(rgb(palette.text_muted))
                .child(permissions_display),
        )
        .child(
            div()
                .w(column_widths.owner)
                .flex_none()
                .px_2()
                .truncate()
                .text_xs()
                .text_color(rgb(palette.text_muted))
                .child(if entry.owner.is_empty() {
                    "-".to_string()
                } else {
                    entry.owner.clone()
                }),
        )
        .child(
            div()
                .w(column_widths.group)
                .flex_none()
                .px_2()
                .truncate()
                .text_xs()
                .text_color(rgb(palette.text_muted))
                .child(if entry.group.is_empty() {
                    "-".to_string()
                } else {
                    entry.group.clone()
                }),
        )
}

pub(super) struct TransferBrowserEntryRowPresentation<'a> {
    pub palette: ThemePalette,
    pub entry: SftpFileEntry,
    pub selected_remote_path: Option<String>,
    pub selected_remote_paths: &'a HashSet<String>,
    pub column_widths: TransferBrowserColumnWidths,
    pub rename_state: Option<TransferRenameState>,
    pub rename_input: Option<AnyElement>,
    pub local_backend: bool,
    pub virtual_drag_supported: bool,
    pub app_handle: gpui::WeakEntity<crate::features::NyaTermApp>,
}

#[cfg(test)]
mod format_tests {
    use super::format_browser_file_size;

    #[test]
    fn browser_sizes_follow_tauri_labels_without_changing_transfer_progress() {
        assert_eq!(format_browser_file_size(None), "-");
        assert_eq!(format_browser_file_size(Some(0)), "-");
        assert_eq!(format_browser_file_size(Some(512)), "512 B");
        assert_eq!(format_browser_file_size(Some(1536)), "1.5 KB");
        assert_eq!(format_browser_file_size(Some(1024 * 1024)), "1.0 MB");
    }

    #[test]
    fn browser_sizes_promote_units_at_each_binary_threshold() {
        for (exponent, unit, previous_unit) in [
            (1, "KB", "B"),
            (2, "MB", "KB"),
            (3, "GB", "MB"),
            (4, "TB", "GB"),
            (5, "PB", "TB"),
            (6, "EB", "PB"),
        ] {
            let threshold = 1024_u64.pow(exponent);
            let below_threshold = if exponent == 1 {
                "1023 B".to_string()
            } else {
                format!("1024.0 {previous_unit}")
            };
            assert_eq!(
                format_browser_file_size(Some(threshold - 1)),
                below_threshold
            );
            assert_eq!(
                format_browser_file_size(Some(threshold)),
                format!("1.0 {unit}")
            );
            assert_eq!(
                format_browser_file_size(Some(threshold + threshold / 2)),
                format!("1.5 {unit}")
            );
        }
    }

    #[test]
    fn browser_sizes_handle_the_largest_byte_count() {
        assert_eq!(format_browser_file_size(Some(u64::MAX)), "16.0 EB");
    }
}
