use gpui::{Pixels, px};
use nyaterm_core::models::settings::FileManagerPaneUiSettings;

use super::pane::{BrowserPaneState, PaneEndpoint};
use nyaterm_files::file_browser::listing::{FileBrowserSortColumn, FileBrowserSortDirection};

pub(super) const COLUMNS: [FileBrowserSortColumn; 6] = [
    FileBrowserSortColumn::Name,
    FileBrowserSortColumn::Size,
    FileBrowserSortColumn::Modified,
    FileBrowserSortColumn::Permissions,
    FileBrowserSortColumn::Owner,
    FileBrowserSortColumn::Group,
];

pub(super) fn column_key(column: FileBrowserSortColumn) -> &'static str {
    match column {
        FileBrowserSortColumn::Name => "name",
        FileBrowserSortColumn::Size => "size",
        FileBrowserSortColumn::Modified => "modified",
        FileBrowserSortColumn::Permissions => "permissions",
        FileBrowserSortColumn::Owner => "owner",
        FileBrowserSortColumn::Group => "group",
    }
}

impl BrowserPaneState {
    pub(super) fn restore_view_settings(&mut self, settings: &FileManagerPaneUiSettings) {
        for column in COLUMNS {
            if let Some(width) = settings.column_widths.get(column_key(column)) {
                self.column_widths
                    .set(column, px((*width).clamp(72, 2000) as f32));
            }
        }
        self.visible_columns = settings.visible_columns.clone();
        if !self.visible_columns.iter().any(|key| key == "name") {
            self.visible_columns.insert(0, "name".into());
        }
        self.sort_column = COLUMNS
            .into_iter()
            .find(|column| column_key(*column) == settings.sort_column)
            .unwrap_or(FileBrowserSortColumn::Name);
        self.sort_direction = if settings.descending {
            FileBrowserSortDirection::Descending
        } else {
            FileBrowserSortDirection::Ascending
        };
        self.row_height = if settings.row_height == 28 { 28 } else { 30 };
        self.remembered_directories = settings.directories.clone();
    }

    pub(super) fn view_settings(&self) -> FileManagerPaneUiSettings {
        FileManagerPaneUiSettings {
            column_widths: COLUMNS
                .into_iter()
                .map(|column| {
                    (
                        column_key(column).into(),
                        f32::from(self.column_widths.get(column)).round() as u32,
                    )
                })
                .collect(),
            visible_columns: self.visible_columns.clone(),
            sort_column: column_key(self.sort_column).into(),
            descending: self.sort_direction == FileBrowserSortDirection::Descending,
            row_height: self.row_height,
            directories: self.remembered_directories.clone(),
        }
    }

    pub(super) fn table_columns(&self, viewport: f32) -> Vec<(FileBrowserSortColumn, Pixels)> {
        let mut columns: Vec<_> = COLUMNS
            .into_iter()
            .filter(|column| {
                (*column == FileBrowserSortColumn::Name
                    || self
                        .visible_columns
                        .iter()
                        .any(|key| key == column_key(*column)))
                    && !(self.endpoint == PaneEndpoint::Local
                        && *column == FileBrowserSortColumn::Permissions)
            })
            .map(|column| (column, self.column_widths.get(column)))
            .collect();
        let other: f32 = columns
            .iter()
            .filter(|(column, _)| *column != FileBrowserSortColumn::Name)
            .map(|(_, width)| f32::from(*width))
            .sum();
        if let Some((_, name_width)) = columns.first_mut() {
            *name_width = px((viewport - other).max(f32::from(*name_width)));
        }
        columns
    }
}
