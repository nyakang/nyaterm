//! Memoized browser listings and navigation history owned by desktop.
use nyaterm_files::file_browser::listing::{
    FileBrowserSortColumn, FileBrowserSortDirection, compare_entries, entry_is_visible,
};
use nyaterm_files::sftp::SftpFileEntry;
use std::collections::VecDeque;
use std::sync::Arc;

/// What the filtered, sorted listing is derived from.
///
/// `entries` is the address of the listing `Arc`, which is an exact signal because
/// every write to that field replaces the vector whole. The rest are small values,
/// so the whole comparison costs a pointer compare, one short string compare and
/// three enum compares -- cheap on every flush, and impossible for a new mutator to
/// forget, because there is no counter to bump.
#[derive(Clone, PartialEq)]
pub(in crate::features) struct BrowserFilterKey {
    entries: usize,
    query: String,
    show_hidden: bool,
    sort_column: FileBrowserSortColumn,
    sort_direction: FileBrowserSortDirection,
}

impl BrowserFilterKey {
    pub(in crate::features) fn new(
        entries: &Arc<Vec<SftpFileEntry>>,
        search: &str,
        show_hidden: bool,
        sort_column: FileBrowserSortColumn,
        sort_direction: FileBrowserSortDirection,
    ) -> Self {
        Self {
            entries: Arc::as_ptr(entries) as usize,
            query: search.trim().to_lowercase(),
            show_hidden,
            sort_column,
            sort_direction,
        }
    }
}

/// The memo. Holds the key it was built for and the listing it produced.
#[derive(Default)]
pub(in crate::features) struct BrowserFilterCache {
    key: Option<BrowserFilterKey>,
    entries: Option<Arc<[SftpFileEntry]>>,
    /// How many times the filter and sort actually ran.
    ///
    /// A coalesced progress batch changes job byte counts and nothing this listing
    /// is derived from, so it must leave this alone. Counting the recomputes is the
    /// only way to show that rather than assert it.
    #[cfg(test)]
    recomputes: usize,
}

impl BrowserFilterCache {
    /// The listing for `key`, filtering and sorting only if the memo has nothing.
    pub(in crate::features) fn entries(
        &mut self,
        key: BrowserFilterKey,
        entries: &Arc<Vec<SftpFileEntry>>,
    ) -> Arc<[SftpFileEntry]> {
        if self.key.as_ref() == Some(&key)
            && let Some(cached) = self.entries.as_ref()
        {
            return cached.clone();
        }

        let mut visible = entries
            .iter()
            .filter(|entry| entry_is_visible(entry, &key.query, key.show_hidden))
            .cloned()
            .collect::<Vec<_>>();
        visible.sort_by(|left, right| {
            compare_entries(left, right, key.sort_column, key.sort_direction)
        });
        let visible: Arc<[SftpFileEntry]> = visible.into();
        self.key = Some(key);
        self.entries = Some(visible.clone());
        #[cfg(test)]
        {
            self.recomputes += 1;
        }
        visible
    }

    #[cfg(test)]
    pub(in crate::features) fn recomputes(&self) -> usize {
        self.recomputes
    }
}

/// Shared back/forward history used by the sidebar browser and file-manager panes.
pub(in crate::features) fn record_browser_history(
    history: &mut VecDeque<String>,
    history_index: &mut usize,
    path: &str,
) -> bool {
    if history.get(*history_index).map(String::as_str) == Some(path) {
        return false;
    }
    if !history.is_empty() {
        let current_index = (*history_index).min(history.len() - 1);
        history.drain(..current_index);
    }
    history.push_front(path.to_string());
    *history_index = 0;
    true
}

#[cfg(test)]
mod tests {
    use super::{BrowserFilterCache, BrowserFilterKey};
    use nyaterm_files::file_browser::listing::{FileBrowserSortColumn, FileBrowserSortDirection};
    use nyaterm_files::sftp::{SftpFileEntry, SftpFileType};
    use std::sync::Arc;
    fn entry(name: &str, file_type: SftpFileType, target_is_directory: bool) -> SftpFileEntry {
        SftpFileEntry {
            name: name.to_string(),
            path: name.to_string(),
            file_type,
            size: None,
            permissions: None,
            owner: String::new(),
            group: String::new(),
            modified_at: None,
            raw_path_token: None,
            symlink_target_is_directory: target_is_directory,
        }
    }

    const COLUMNS: [FileBrowserSortColumn; 6] = [
        FileBrowserSortColumn::Name,
        FileBrowserSortColumn::Size,
        FileBrowserSortColumn::Modified,
        FileBrowserSortColumn::Permissions,
        FileBrowserSortColumn::Owner,
        FileBrowserSortColumn::Group,
    ];
    const DIRECTIONS: [FileBrowserSortDirection; 2] = [
        FileBrowserSortDirection::Ascending,
        FileBrowserSortDirection::Descending,
    ];
    const KINDS: [(SftpFileType, bool); 5] = [
        (SftpFileType::Directory, false),
        (SftpFileType::Symlink, true),
        (SftpFileType::Symlink, false),
        (SftpFileType::File, false),
        (SftpFileType::Other, false),
    ];

    #[test]
    fn browser_cache_sorts_large_mixed_listings_and_reuses_each_result() {
        let entries = Arc::new(
            (0..128)
                .map(|offset| {
                    let index = (offset * 73) % 128;
                    let (file_type, directory_link) = KINDS[index % KINDS.len()];
                    let mut item = entry(&format!("entry{index}"), file_type, directory_link);
                    item.size = Some(index as u64);
                    item.modified_at = Some(index as u32);
                    item.permissions = Some(index as u32);
                    item.owner = format!("owner{index}");
                    item.group = format!("group{index}");
                    item
                })
                .collect::<Vec<_>>(),
        );
        let mut cache = BrowserFilterCache::default();
        let mut recomputes = 0;
        for column in COLUMNS {
            for direction in DIRECTIONS {
                let key = BrowserFilterKey::new(&entries, "", true, column, direction);
                let sorted = cache.entries(key.clone(), &entries);
                let mut expected = Vec::new();
                for directory in [true, false] {
                    let mut indices: Vec<_> = (0..128)
                        .filter(|index| (*index % KINDS.len() < 2) == directory)
                        .collect();
                    if direction == FileBrowserSortDirection::Descending {
                        indices.reverse();
                    }
                    expected.extend(indices.into_iter().map(|index| format!("entry{index}")));
                }
                assert_eq!(
                    sorted.iter().map(|item| &item.name).collect::<Vec<_>>(),
                    expected.iter().collect::<Vec<_>>(),
                    "{column:?}, {direction:?}",
                );
                recomputes += 1;
                assert_eq!(cache.recomputes(), recomputes);
                assert!(Arc::ptr_eq(&sorted, &cache.entries(key, &entries)));
                assert_eq!(cache.recomputes(), recomputes);
            }
        }
    }
}
