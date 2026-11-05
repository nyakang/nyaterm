//! View models for the read-only remote file preview workspace.
//!
//! These mirror the shape of the editor workspace/tab models, but the preview
//! is read-only: there is no dirty/saving/conflict state and no write path. A
//! tab instead carries a [`PreviewContent`] that the loader replaces once the
//! remote fetch and any decode finish. Staleness is guarded by `generation`, so
//! a slow load for a tab the user already refreshed or closed is dropped rather
//! than shown.

use gpui::RenderImage;
use nyaterm_core::PreviewCategory;
use nyaterm_files::remote_file::RemoteTextGeneration;
use std::collections::HashMap;
use std::sync::Arc;

/// A single decoded page of a rasterized PDF, ready to paint.
#[derive(Clone)]
pub(crate) struct PreviewPdfPage {
    pub(crate) image: Arc<RenderImage>,
    /// Row-major BGRA source pixels at the unrotated orientation.
    pub(crate) pixels: Arc<Vec<u8>>,
    /// Unrotated source dimensions, used to rebuild rotated surfaces.
    pub(crate) src_width: u32,
    pub(crate) src_height: u32,
    /// Current (possibly rotated) surface size, so the view scales without
    /// re-reading the surface.
    pub(crate) width: u32,
    pub(crate) height: u32,
}

impl std::fmt::Debug for PreviewPdfPage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreviewPdfPage")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish()
    }
}

/// A lazily-rasterized PDF document.
///
/// Only the active page (and a bounded cache of recently-viewed pages) is ever
/// rasterized: a long PDF no longer rasterizes every page up front and no
/// longer silently truncates at a page cap. Page rasterization runs on a
/// background job keyed by `generation`; a result for a superseded generation
/// (the tab was refreshed or rotated) is discarded rather than inserted.
#[derive(Clone)]
pub(crate) struct PreviewPdfDocument {
    /// Raw PDF bytes, shared with the background rasterizer.
    pub(crate) bytes: Arc<Vec<u8>>,
    /// Total number of pages in the document (all of them, never truncated).
    pub(crate) page_count: usize,
    /// Zero-based index of the page currently shown.
    pub(crate) current_page: usize,
    /// Bounded cache of rasterized pages, keyed by zero-based page index.
    pub(crate) cache: HashMap<usize, PreviewPdfPage>,
    /// Order pages entered the cache, oldest first, for bounded eviction.
    pub(crate) cache_order: Vec<usize>,
    /// Pages with a rasterization request currently in flight, so the view does
    /// not enqueue duplicates on every render.
    pub(crate) pending: Vec<usize>,
}

impl PreviewPdfDocument {
    /// The most pages kept rasterized at once. Bounds peak memory regardless of
    /// document length; older pages are evicted as the user pages through.
    pub(crate) const MAX_CACHED_PAGES: usize = 6;

    pub(crate) fn new(bytes: Arc<Vec<u8>>, page_count: usize) -> Self {
        Self {
            bytes,
            page_count,
            current_page: 0,
            cache: HashMap::new(),
            cache_order: Vec::new(),
            pending: Vec::new(),
        }
    }

    pub(crate) fn current_page(&self) -> Option<&PreviewPdfPage> {
        self.cache.get(&self.current_page)
    }

    /// Move to a page index, clamped into range. Returns whether it changed.
    pub(crate) fn go_to_page(&mut self, index: usize) -> bool {
        let clamped = index.min(self.page_count.saturating_sub(1));
        if clamped == self.current_page {
            return false;
        }
        self.current_page = clamped;
        true
    }

    pub(crate) fn next_page(&mut self) -> bool {
        self.go_to_page(self.current_page + 1)
    }

    pub(crate) fn previous_page(&mut self) -> bool {
        self.go_to_page(self.current_page.saturating_sub(1))
    }

    /// Whether the active page still needs rasterizing and has no request in
    /// flight. Callers enqueue a background job when this is true.
    pub(crate) fn active_page_needs_render(&self) -> bool {
        let index = self.current_page;
        !self.cache.contains_key(&index) && !self.pending.contains(&index)
    }

    pub(crate) fn mark_pending(&mut self, index: usize) {
        if !self.pending.contains(&index) {
            self.pending.push(index);
        }
    }

    /// Insert a freshly rasterized page, evicting the oldest cached page(s) past
    /// the bound. Clears the pending marker for `index`.
    pub(crate) fn insert_page(&mut self, index: usize, page: PreviewPdfPage) {
        self.pending.retain(|pending| *pending != index);
        if self.cache.insert(index, page).is_none() {
            self.cache_order.push(index);
        }
        while self.cache_order.len() > Self::MAX_CACHED_PAGES {
            // Never evict the page currently on screen even if it is oldest.
            let evict_pos = self
                .cache_order
                .iter()
                .position(|candidate| *candidate != self.current_page);
            let Some(pos) = evict_pos else { break };
            let evicted = self.cache_order.remove(pos);
            self.cache.remove(&evicted);
        }
    }
}

/// A decoded raster image, ready to paint with zoom/rotate applied by the view.
///
/// Keeps the raw BGRA pixels (already atlas-ordered) alongside the current
/// `RenderImage` so a rotation can rebuild the surface from the source rather
/// than repeatedly rotating an already-rotated buffer and accumulating error.
#[derive(Clone)]
pub(crate) struct PreviewImage {
    pub(crate) image: Arc<RenderImage>,
    pub(crate) pixels: Arc<Vec<u8>>,
    /// Unrotated source dimensions.
    pub(crate) src_width: u32,
    pub(crate) src_height: u32,
    /// Current (possibly rotated) surface size.
    pub(crate) width: u32,
    pub(crate) height: u32,
}

/// Which column a delimited preview is sorted by, and in what direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DelimitedSort {
    /// Ascending by the given column index.
    Ascending(usize),
    /// Descending by the given column index.
    Descending(usize),
}

/// Delimited (CSV/TSV) data with header-toggle and stable tri-state sorting.
///
/// Rows are stored once; sorting reorders an index vector rather than the rows
/// themselves, and the sort is stable, so clearing the sort restores the
/// original file order. The view renders through the ordering with
/// `uniform_list`, so a large file is never materialized into thousands of
/// elements per frame.
#[derive(Clone)]
pub(crate) struct PreviewDelimited {
    /// Every parsed record, header row included. The header is drawn from
    /// `records[0]` when `first_row_is_header` is set.
    pub(crate) records: Arc<Vec<Vec<String>>>,
    /// Whether the first record is treated as a header row.
    pub(crate) first_row_is_header: bool,
    /// Current sort, or `None` for original file order.
    pub(crate) sort: Option<DelimitedSort>,
    /// Order of *body* row indices (into `records`), reflecting the current
    /// sort. Recomputed by [`Self::resort`].
    pub(crate) order: Arc<Vec<usize>>,
    /// Whether the body was truncated at the row ceiling during parsing.
    pub(crate) truncated: bool,
    /// Widest column count across all records, so ragged rows still lay out.
    pub(crate) column_count: usize,
}

impl PreviewDelimited {
    pub(crate) fn new(records: Vec<Vec<String>>, truncated: bool) -> Self {
        let column_count = records
            .iter()
            .map(|row| row.len())
            .max()
            .unwrap_or(0)
            .max(1);
        let mut data = Self {
            records: Arc::new(records),
            first_row_is_header: true,
            sort: None,
            order: Arc::new(Vec::new()),
            truncated,
            column_count,
        };
        data.resort();
        data
    }

    /// Index of the first body record, honouring the header toggle.
    fn body_start(&self) -> usize {
        if self.first_row_is_header && !self.records.is_empty() {
            1
        } else {
            0
        }
    }

    /// The header labels, or synthesized `Column N` names when there is no
    /// header row.
    pub(crate) fn headers(&self) -> Vec<String> {
        if self.first_row_is_header {
            self.records.first().cloned().unwrap_or_default()
        } else {
            (0..self.column_count)
                .map(|index| format!("Column {}", index + 1))
                .collect()
        }
    }

    /// Number of body rows currently displayed.
    pub(crate) fn row_count(&self) -> usize {
        self.order.len()
    }

    /// A displayed body row by its position in the current ordering.
    pub(crate) fn row(&self, position: usize) -> Option<&Vec<String>> {
        self.order
            .get(position)
            .and_then(|record_index| self.records.get(*record_index))
    }

    pub(crate) fn toggle_header(&mut self) {
        self.first_row_is_header = !self.first_row_is_header;
        // A header index that no longer exists as a body row would be invalid;
        // recompute the ordering and re-clamp the sort column.
        if let Some(sort) = self.sort {
            let column = match sort {
                DelimitedSort::Ascending(column) | DelimitedSort::Descending(column) => column,
            };
            if column >= self.column_count {
                self.sort = None;
            }
        }
        self.resort();
    }

    /// Advance a column through the tri-state cycle ascending → descending →
    /// cleared. Clicking a different column starts fresh at ascending.
    pub(crate) fn cycle_sort(&mut self, column: usize) {
        self.sort = match self.sort {
            Some(DelimitedSort::Ascending(current)) if current == column => {
                Some(DelimitedSort::Descending(column))
            }
            Some(DelimitedSort::Descending(current)) if current == column => None,
            _ => Some(DelimitedSort::Ascending(column)),
        };
        self.resort();
    }

    /// Recompute [`Self::order`] from the current sort. Sorting is stable and
    /// compares numbers numerically, with empty cells always sorted last in
    /// both directions.
    pub(crate) fn resort(&mut self) {
        let start = self.body_start();
        let mut order: Vec<usize> = (start..self.records.len()).collect();
        let Some(sort) = self.sort else {
            self.order = Arc::new(order);
            return;
        };
        let (column, descending) = match sort {
            DelimitedSort::Ascending(column) => (column, false),
            DelimitedSort::Descending(column) => (column, true),
        };
        // Parse once per cell, not once per comparison. Replacing the immutable
        // index snapshot leaves in-flight views valid without copying records.
        let keys: Vec<_> = self.records[start..]
            .iter()
            .map(|row| DelimitedSortKey::new(row.get(column).map_or("", String::as_str)))
            .collect();
        order.sort_by(|left, right| keys[*left - start].compare(&keys[*right - start], descending));
        self.order = Arc::new(order);
    }
}

enum DelimitedSortKey {
    Number(f64),
    Text(String),
    Empty,
}

impl DelimitedSortKey {
    fn new(value: &str) -> Self {
        let value = value.trim();
        if value.is_empty() {
            Self::Empty
        } else if let Ok(number) = value.parse::<f64>()
            && number.is_finite()
        {
            Self::Number(number)
        } else {
            Self::Text(value.to_ascii_lowercase())
        }
    }

    fn compare(&self, other: &Self, descending: bool) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        // Category ordering must be independent of the other cell's value:
        // pairwise numeric-or-text comparisons produce cycles like 2 < 10 < 1a < 2.
        let order = match (self, other) {
            (Self::Empty, Self::Empty) => return Ordering::Equal,
            (Self::Empty, _) => return Ordering::Greater,
            (_, Self::Empty) => return Ordering::Less,
            (Self::Number(left), Self::Number(right)) => {
                left.partial_cmp(right).expect("finite sort keys")
            }
            (Self::Number(_), Self::Text(_)) => Ordering::Less,
            (Self::Text(_), Self::Number(_)) => Ordering::Greater,
            (Self::Text(left), Self::Text(right)) => left.cmp(right),
        };
        if descending { order.reverse() } else { order }
    }
}

/// The rendered payload of a preview tab once its load resolves.
///
/// `Loading` is the initial state; every other variant is terminal for a given
/// `generation`, except `Pdf`, whose pages fill in lazily. Text-shaped variants
/// keep the raw string so the view owns no parsing state of its own.
#[derive(Clone)]
pub(crate) enum PreviewContent {
    Loading,
    /// Plain / source / config / log text.
    Text(String),
    /// JSON. `text` is the pretty-printed document when it parsed, or the raw
    /// bytes when it did not; `parse_error` carries the parser message so the
    /// view can show an explicit banner instead of silently rendering raw text.
    Json {
        text: String,
        parse_error: Option<String>,
    },
    /// Markdown source, rendered by the shared block renderer in the view.
    Markdown(String),
    /// Delimited data with header/sort state.
    Delimited(PreviewDelimited),
    Image(PreviewImage),
    /// A lazily-rasterized PDF.
    Pdf(PreviewPdfDocument),
    /// A file whose format cannot be previewed. The window still opened; no
    /// bytes were fetched. Mirrors the Tauri "unsupported format" message.
    Unsupported,
    /// The load or decode failed; the message is safe to show to the user.
    Error(String),
}

impl std::fmt::Debug for PreviewContent {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Concise on purpose: `TransferJobOutput` derives `Debug`, and dumping a
        // decoded image or every rasterized PDF page would be huge and useless.
        match self {
            PreviewContent::Loading => formatter.write_str("Loading"),
            PreviewContent::Text(text) => formatter.debug_tuple("Text").field(&text.len()).finish(),
            PreviewContent::Json { text, parse_error } => formatter
                .debug_struct("Json")
                .field("len", &text.len())
                .field("parse_error", &parse_error.is_some())
                .finish(),
            PreviewContent::Markdown(text) => formatter
                .debug_tuple("Markdown")
                .field(&text.len())
                .finish(),
            PreviewContent::Delimited(data) => formatter
                .debug_struct("Delimited")
                .field("rows", &data.row_count())
                .field("truncated", &data.truncated)
                .finish(),
            PreviewContent::Image(image) => formatter
                .debug_struct("Image")
                .field("width", &image.width)
                .field("height", &image.height)
                .finish(),
            PreviewContent::Pdf(document) => formatter
                .debug_struct("Pdf")
                .field("pages", &document.page_count)
                .field("current", &document.current_page)
                .field("cached", &document.cache.len())
                .finish(),
            PreviewContent::Unsupported => formatter.write_str("Unsupported"),
            PreviewContent::Error(message) => {
                formatter.debug_tuple("Error").field(message).finish()
            }
        }
    }
}

/// How the image/PDF viewport is currently transformed. Rotation is quarter
/// turns; zoom is a multiplier the view clamps.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PreviewViewport {
    pub(crate) zoom: f32,
    /// Clockwise quarter turns, `0..=3`.
    pub(crate) rotation_quarter_turns: u8,
    /// Whether the viewport should fit-to-window rather than honour `zoom`. Set
    /// on load and on reset; cleared once the user zooms explicitly.
    pub(crate) fit_to_window: bool,
}

impl Default for PreviewViewport {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            rotation_quarter_turns: 0,
            fit_to_window: true,
        }
    }
}

impl PreviewViewport {
    /// 10% minimum, matching the parity requirement (was documented as 10%).
    pub(crate) const MIN_ZOOM: f32 = 0.1;
    /// 800% maximum.
    pub(crate) const MAX_ZOOM: f32 = 8.0;

    pub(crate) fn zoom_by(&mut self, factor: f32) {
        self.fit_to_window = false;
        self.zoom = (self.zoom * factor).clamp(Self::MIN_ZOOM, Self::MAX_ZOOM);
    }

    pub(crate) fn set_zoom(&mut self, zoom: f32) {
        self.fit_to_window = false;
        self.zoom = zoom.clamp(Self::MIN_ZOOM, Self::MAX_ZOOM);
    }

    /// Reset zoom, rotation, and fit — the reset control fits to window again
    /// rather than snapping to 100% at an unchanged rotation.
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }

    pub(crate) fn rotate_clockwise(&mut self) {
        self.rotation_quarter_turns = (self.rotation_quarter_turns + 1) % 4;
    }

    pub(crate) fn rotate_counter_clockwise(&mut self) {
        self.rotation_quarter_turns = (self.rotation_quarter_turns + 3) % 4;
    }
}

/// A single preview tab: one remote file, its category, and its current
/// content.
#[derive(Clone)]
pub(crate) struct TransferPreviewState {
    pub(crate) id: String,
    pub(crate) session_id: Option<String>,
    pub(crate) remote_path: String,
    pub(crate) raw_path_token: Option<String>,
    pub(crate) name: String,
    /// File size in bytes when known, for the status bar.
    pub(crate) size: Option<u64>,
    /// Last-modified time (unix seconds) when known, for the status bar.
    pub(crate) modified_at: Option<u32>,
    pub(crate) category: PreviewCategory,
    /// Bumped on open and on refresh; a load result for an older generation is
    /// discarded. Reuses the editor's monotonic generation source.
    pub(crate) generation: RemoteTextGeneration,
    pub(crate) content: PreviewContent,
    pub(crate) viewport: PreviewViewport,
}

impl TransferPreviewState {
    pub(crate) fn tab_id_for_remote_path(
        session_id: Option<&str>,
        remote_path: &nyaterm_files::sftp::RemoteFilePath,
    ) -> String {
        format!(
            "{}\n{}",
            session_id.unwrap_or_default(),
            remote_path.identity_key()
        )
    }

    pub(crate) fn remote_file_path(&self) -> nyaterm_files::sftp::RemoteFilePath {
        nyaterm_files::sftp::RemoteFilePath {
            display_path: self.remote_path.clone(),
            raw_path_token: self.raw_path_token.clone(),
        }
    }
}

/// The set of open preview tabs and which one is active.
#[derive(Clone)]
pub(crate) struct TransferPreviewWorkspaceState {
    pub(crate) tabs: Vec<TransferPreviewState>,
    pub(crate) active_tab_id: String,
}

impl TransferPreviewWorkspaceState {
    pub(crate) fn new(tab: TransferPreviewState) -> Self {
        Self {
            active_tab_id: tab.id.clone(),
            tabs: vec![tab],
        }
    }

    pub(crate) fn active_tab(&self) -> Option<&TransferPreviewState> {
        self.tabs
            .iter()
            .find(|tab| tab.id == self.active_tab_id)
            .or_else(|| self.tabs.first())
    }

    pub(crate) fn active_tab_mut(&mut self) -> Option<&mut TransferPreviewState> {
        let active_index = self
            .tabs
            .iter()
            .position(|tab| tab.id == self.active_tab_id)
            .unwrap_or(0);
        self.tabs.get_mut(active_index)
    }

    pub(crate) fn remove_tab(&mut self, tab_id: &str) -> bool {
        let Some(index) = self.tabs.iter().position(|tab| tab.id == tab_id) else {
            return false;
        };
        let removed_active = self.active_tab_id == tab_id;
        let session_id = self.tabs[index].session_id.clone();
        self.tabs.remove(index);
        if removed_active {
            self.active_tab_id = self
                .tabs
                .get(index.saturating_sub(1))
                .filter(|tab| tab.session_id == session_id)
                .or_else(|| self.tabs.iter().find(|tab| tab.session_id == session_id))
                .or_else(|| self.tabs.get(index.min(self.tabs.len().saturating_sub(1))))
                .map(|tab| tab.id.clone())
                .unwrap_or_default();
        }
        true
    }

    /// Distinguishing tab labels: a bare file name, disambiguated by its parent
    /// directory when two open tabs share the same file name. Mirrors the Tauri
    /// window's same-name parent-directory disambiguation.
    pub(crate) fn tab_labels(&self) -> Vec<(String, String)> {
        let mut name_counts: HashMap<&str, usize> = HashMap::new();
        for tab in &self.tabs {
            *name_counts.entry(tab.name.as_str()).or_default() += 1;
        }
        self.tabs
            .iter()
            .map(|tab| {
                let label = if name_counts.get(tab.name.as_str()).copied().unwrap_or(0) > 1 {
                    disambiguated_label(&tab.name, &tab.remote_path)
                } else if tab.name.trim().is_empty() {
                    tab.remote_path.clone()
                } else {
                    tab.name.clone()
                };
                (tab.id.clone(), label)
            })
            .collect()
    }
}

/// `parent/name` label used when two tabs share `name`.
fn disambiguated_label(name: &str, remote_path: &str) -> String {
    let trimmed = remote_path.trim_end_matches('/');
    let without_name = trimmed
        .strip_suffix(name)
        .map(|prefix| prefix.trim_end_matches('/'))
        .unwrap_or(trimmed);
    let parent = without_name.rsplit('/').next().unwrap_or("");
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent}/{name}")
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DelimitedSort, PreviewContent, PreviewDelimited, PreviewViewport, TransferPreviewState,
        TransferPreviewWorkspaceState,
    };
    use nyaterm_core::PreviewCategory;
    use nyaterm_files::remote_file::RemoteTextGeneration;
    use nyaterm_files::sftp::RemoteFilePath;

    fn preview_tab(session_id: &str, remote_path: &str) -> TransferPreviewState {
        TransferPreviewState {
            id: TransferPreviewState::tab_id_for_remote_path(
                Some(session_id),
                &RemoteFilePath::new(remote_path),
            ),
            session_id: Some(session_id.to_string()),
            remote_path: remote_path.to_string(),
            raw_path_token: None,
            name: remote_path
                .rsplit('/')
                .next()
                .unwrap_or(remote_path)
                .to_string(),
            size: None,
            modified_at: None,
            category: PreviewCategory::Text,
            generation: RemoteTextGeneration::next(),
            content: PreviewContent::Loading,
            viewport: PreviewViewport::default(),
        }
    }

    #[test]
    fn removing_active_preview_tab_prefers_left_same_session_tab() {
        let first = preview_tab("session", "/one.txt");
        let first_id = first.id.clone();
        let second = preview_tab("session", "/two.txt");
        let third = preview_tab("session", "/three.txt");
        let second_id = second.id.clone();
        let mut workspace = TransferPreviewWorkspaceState::new(first);
        workspace.tabs.extend([second, third]);
        workspace.active_tab_id = second_id.clone();

        assert!(workspace.remove_tab(&second_id));
        assert_eq!(workspace.active_tab_id, first_id);
        assert_eq!(
            workspace.active_tab().map(|tab| tab.remote_path.as_str()),
            Some("/one.txt")
        );
    }

    #[test]
    fn preview_tab_ids_include_raw_remote_path_identity() {
        let first = RemoteFilePath::from_raw("/srv/invalid-?", b"/srv/invalid-\xfe");
        let second = RemoteFilePath::from_raw("/srv/invalid-?", b"/srv/invalid-\xff");
        assert_ne!(
            TransferPreviewState::tab_id_for_remote_path(Some("session"), &first),
            TransferPreviewState::tab_id_for_remote_path(Some("session"), &second)
        );
    }

    #[test]
    fn same_name_tabs_are_disambiguated_by_parent_directory() {
        let first = preview_tab("session", "/app/config.toml");
        let second = preview_tab("session", "/lib/config.toml");
        let mut workspace = TransferPreviewWorkspaceState::new(first);
        workspace.tabs.push(second);
        let labels: Vec<String> = workspace
            .tab_labels()
            .into_iter()
            .map(|(_, label)| label)
            .collect();
        assert_eq!(labels, vec!["app/config.toml", "lib/config.toml"]);
    }

    #[test]
    fn unique_name_tabs_use_the_bare_name() {
        let first = preview_tab("session", "/app/config.toml");
        let second = preview_tab("session", "/lib/readme.md");
        let mut workspace = TransferPreviewWorkspaceState::new(first);
        workspace.tabs.push(second);
        let labels: Vec<String> = workspace
            .tab_labels()
            .into_iter()
            .map(|(_, label)| label)
            .collect();
        assert_eq!(labels, vec!["config.toml", "readme.md"]);
    }

    #[test]
    fn viewport_zoom_clamps_between_ten_and_eight_hundred_percent() {
        let mut viewport = PreviewViewport::default();
        viewport.zoom_by(100.0);
        assert_eq!(viewport.zoom, PreviewViewport::MAX_ZOOM);
        assert!((PreviewViewport::MAX_ZOOM - 8.0).abs() < f32::EPSILON);
        viewport.zoom_by(0.0001);
        assert_eq!(viewport.zoom, PreviewViewport::MIN_ZOOM);
        assert!((PreviewViewport::MIN_ZOOM - 0.1).abs() < f32::EPSILON);
        assert!(!viewport.fit_to_window);
    }

    #[test]
    fn rotation_wraps_both_directions_and_reset_refits() {
        let mut viewport = PreviewViewport::default();
        for _ in 0..4 {
            viewport.rotate_clockwise();
        }
        assert_eq!(viewport.rotation_quarter_turns, 0);
        viewport.rotate_counter_clockwise();
        assert_eq!(viewport.rotation_quarter_turns, 3);
        viewport.rotate_clockwise();
        assert_eq!(viewport.rotation_quarter_turns, 0);

        viewport.zoom_by(2.0);
        viewport.reset();
        assert_eq!(viewport.zoom, 1.0);
        assert_eq!(viewport.rotation_quarter_turns, 0);
        assert!(viewport.fit_to_window, "reset must re-enable fit-to-window");
    }

    #[test]
    fn content_debug_is_concise() {
        assert_eq!(format!("{:?}", PreviewContent::Loading), "Loading");
        assert_eq!(
            format!("{:?}", PreviewContent::Text("hello".into())),
            "Text(5)"
        );
        assert!(format!("{:?}", PreviewContent::Error("boom".into())).contains("boom"));
        assert_eq!(format!("{:?}", PreviewContent::Unsupported), "Unsupported");
    }

    fn delimited(records: &[&[&str]]) -> PreviewDelimited {
        let records = records
            .iter()
            .map(|row| row.iter().map(|cell| cell.to_string()).collect())
            .collect();
        PreviewDelimited::new(records, false)
    }

    #[test]
    fn header_toggle_changes_which_records_are_body_rows() {
        let mut data = delimited(&[&["name", "age"], &["b", "2"], &["a", "1"]]);
        assert!(data.first_row_is_header);
        assert_eq!(data.headers(), vec!["name", "age"]);
        assert_eq!(data.row_count(), 2);

        data.toggle_header();
        assert!(!data.first_row_is_header);
        // Synthesized headers, and the former header row becomes a body row.
        assert_eq!(data.headers(), vec!["Column 1", "Column 2"]);
        assert_eq!(data.row_count(), 3);
    }

    #[test]
    fn column_sort_cycles_asc_desc_clear_and_is_stable() {
        let mut data = delimited(&[&["k"], &["b"], &["a"], &["b"]]);
        // Ascending
        data.cycle_sort(0);
        assert_eq!(data.sort, Some(DelimitedSort::Ascending(0)));
        let ascending: Vec<&str> = (0..data.row_count())
            .map(|position| data.row(position).unwrap()[0].as_str())
            .collect();
        assert_eq!(ascending, vec!["a", "b", "b"]);
        // Descending
        data.cycle_sort(0);
        assert_eq!(data.sort, Some(DelimitedSort::Descending(0)));
        let descending: Vec<&str> = (0..data.row_count())
            .map(|position| data.row(position).unwrap()[0].as_str())
            .collect();
        assert_eq!(descending, vec!["b", "b", "a"]);
        // Cleared restores file order
        data.cycle_sort(0);
        assert_eq!(data.sort, None);
        let cleared: Vec<&str> = (0..data.row_count())
            .map(|position| data.row(position).unwrap()[0].as_str())
            .collect();
        assert_eq!(cleared, vec!["b", "a", "b"]);
    }

    #[test]
    fn numeric_columns_sort_numerically_not_lexically() {
        let mut data = delimited(&[&["n"], &["2"], &["10"], &["1"]]);
        data.cycle_sort(0);
        let ascending: Vec<&str> = (0..data.row_count())
            .map(|position| data.row(position).unwrap()[0].as_str())
            .collect();
        // Lexical order would be 1, 10, 2; numeric is 1, 2, 10.
        assert_eq!(ascending, vec!["1", "2", "10"]);
    }

    #[test]
    fn empty_cells_sort_last_in_both_directions() {
        let mut data = delimited(&[&["v"], &["b"], &[""], &["a"]]);
        data.cycle_sort(0);
        let ascending: Vec<&str> = (0..data.row_count())
            .map(|position| data.row(position).unwrap()[0].as_str())
            .collect();
        assert_eq!(ascending, vec!["a", "b", ""]);

        // Descending must still keep the blank last, not first.
        data.cycle_sort(0);
        let descending: Vec<&str> = (0..data.row_count())
            .map(|position| data.row(position).unwrap()[0].as_str())
            .collect();
        assert_eq!(descending, vec!["b", "a", ""]);
    }

    #[test]
    fn mixed_sort_keys_are_transitive_in_both_directions() {
        use super::DelimitedSortKey;
        let values = [
            "2", "10", "1a", "", " ", "-2", "1e2", "NaN", "inf", "-inf", "A", "a", "0", "-0",
        ];
        let keys: Vec<_> = values
            .iter()
            .map(|value| DelimitedSortKey::new(value))
            .collect();
        for descending in [false, true] {
            for a in &keys {
                for b in &keys {
                    assert_eq!(a.compare(b, descending), b.compare(a, descending).reverse());
                    for c in &keys {
                        if a.compare(b, descending).is_le() && b.compare(c, descending).is_le() {
                            assert!(a.compare(c, descending).is_le());
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn mixed_column_sort_preserves_equal_values_and_restores_original_order() {
        let mut data = delimited(&[
            &["v"],
            &["10"],
            &["1a"],
            &["2"],
            &["-2"],
            &["2.0"],
            &["NaN"],
            &[""],
            &["1e2"],
        ]);
        data.cycle_sort(0);
        assert_eq!(data.order.as_slice(), &[4, 3, 5, 1, 8, 2, 6, 7]);
        data.cycle_sort(0);
        assert_eq!(data.order.as_slice(), &[6, 2, 8, 1, 3, 5, 4, 7]);
        data.cycle_sort(0);
        assert_eq!(data.order.as_slice(), &[1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn ten_thousand_mixed_rows_sort_without_panicking() {
        let mut state = 42_u64;
        let records = std::iter::once(vec!["value".to_string()])
            .chain((0..10_000).map(|_| {
                state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                let value = (state >> 32) % 1000;
                vec![if value.is_multiple_of(3) {
                    format!("{value}a")
                } else {
                    value.to_string()
                }]
            }))
            .collect();
        let mut data = PreviewDelimited::new(records, false);
        for _ in 0..3 {
            data.cycle_sort(0);
        }
        assert_eq!(data.order.as_slice(), &(1..=10_000).collect::<Vec<_>>());
    }

    #[test]
    fn large_preview_snapshots_share_cells_and_order() {
        use std::sync::Arc;
        let mut data = PreviewDelimited::new(vec![vec!["abc".to_string(); 8]; 200_001], false);
        let started = std::time::Instant::now();
        for _ in 0..100 {
            let snapshot = std::hint::black_box(data.clone());
            assert!(Arc::ptr_eq(&data.records, &snapshot.records));
            assert!(Arc::ptr_eq(&data.order, &snapshot.order));
        }
        eprintln!("100 shared preview snapshots: {:?}", started.elapsed());
        let snapshot = data.clone();
        data.cycle_sort(0);
        assert!(Arc::ptr_eq(&data.records, &snapshot.records));
        assert!(!Arc::ptr_eq(&data.order, &snapshot.order));
        assert_eq!(snapshot.sort, None);
    }

    fn dummy_pdf_page() -> super::PreviewPdfPage {
        use gpui::RenderImage;
        use std::sync::Arc;
        // A 1x1 BGRA page; the surface content is irrelevant to cache logic.
        let bytes = vec![0u8, 0, 0, 255];
        let buffer = image::RgbaImage::from_raw(1, 1, bytes.clone()).unwrap();
        super::PreviewPdfPage {
            image: Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])),
            pixels: Arc::new(bytes),
            src_width: 1,
            src_height: 1,
            width: 1,
            height: 1,
        }
    }

    #[test]
    fn pdf_navigation_clamps_and_reports_render_need() {
        use std::sync::Arc;
        let mut document = super::PreviewPdfDocument::new(Arc::new(Vec::new()), 3);
        assert_eq!(document.current_page, 0);
        assert!(document.active_page_needs_render());

        assert!(document.next_page());
        assert_eq!(document.current_page, 1);
        assert!(!document.next_page() || document.current_page == 2);
        // Clamp at the last page.
        document.go_to_page(99);
        assert_eq!(document.current_page, 2);
        assert!(!document.next_page());
        // Clamp at the first page.
        assert!(document.previous_page());
        document.go_to_page(0);
        assert_eq!(document.current_page, 0);
        assert!(!document.previous_page());
    }

    #[test]
    fn pdf_page_cache_is_bounded_and_keeps_the_active_page() {
        use std::sync::Arc;
        let mut document = super::PreviewPdfDocument::new(Arc::new(Vec::new()), 100);
        for index in 0..super::PreviewPdfDocument::MAX_CACHED_PAGES + 3 {
            document.go_to_page(index);
            document.mark_pending(index);
            document.insert_page(index, dummy_pdf_page());
            assert!(!document.pending.contains(&index));
        }
        assert!(
            document.cache.len() <= super::PreviewPdfDocument::MAX_CACHED_PAGES,
            "cache must stay bounded"
        );
        // The current page is never evicted even under pressure.
        assert!(document.current_page().is_some());
    }
}
