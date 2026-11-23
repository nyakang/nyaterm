use crate::formatting::{format_browser_file_size, format_permissions_octal, format_sftp_modified};
use gpui::{AnyElement, IntoElement, SharedString, div, prelude::*};
use nyaterm_files::{file_browser::listing::FileBrowserSortColumn, sftp::SftpFileEntry};
use nyaterm_ui::icons::transfer_entry_icon;
use nyaterm_ui::{NyaTooltip, ThemePalette};
pub fn entry_cells(
    palette: ThemePalette,
    pane_id: &str,
    entry: &SftpFileEntry,
    columns: &[(FileBrowserSortColumn, gpui::Pixels)],
) -> Vec<AnyElement> {
    let mut cells = Vec::with_capacity(columns.len());
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
                            pane_id,
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
        cells.push(cell.into_any_element());
    }
    cells
}
