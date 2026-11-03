//! Pure filter and sort logic for the SFTP browser listing, and its memo.
//!
//! Deliberately free of GPUI and of anything under `pages`: the state layer owns
//! both the derivation and the cache for it, so the view has nothing to drive and
//! nothing to remember. These functions used to live in `pages/transfers`, which
//! made the state layer unable to reach them without inverting the dependency.

use std::cmp::Ordering;
use std::collections::VecDeque;
use std::sync::Arc;

use nyaterm_transport::SftpFileEntry;

use crate::models::{TransferBrowserSortColumn, TransferBrowserSortDirection};

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
    sort_column: TransferBrowserSortColumn,
    sort_direction: TransferBrowserSortDirection,
}

impl BrowserFilterKey {
    pub(in crate::features) fn new(
        entries: &Arc<Vec<SftpFileEntry>>,
        search: &str,
        show_hidden: bool,
        sort_column: TransferBrowserSortColumn,
        sort_direction: TransferBrowserSortDirection,
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
            .filter(|entry| transfer_browser_entry_is_visible(entry, &key.query, key.show_hidden))
            .cloned()
            .collect::<Vec<_>>();
        visible.sort_by(|left, right| {
            compare_transfer_browser_entries(left, right, key.sort_column, key.sort_direction)
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

pub(in crate::features) fn transfer_browser_entry_is_visible(
    entry: &SftpFileEntry,
    normalized_query: &str,
    show_hidden_files: bool,
) -> bool {
    (show_hidden_files || !entry.name.starts_with('.'))
        && (normalized_query.is_empty() || entry.name.to_lowercase().contains(normalized_query))
}

pub(in crate::features) fn compare_transfer_browser_entries(
    left: &SftpFileEntry,
    right: &SftpFileEntry,
    column: TransferBrowserSortColumn,
    direction: TransferBrowserSortDirection,
) -> Ordering {
    if left.file_type != right.file_type {
        let left_dir = left.is_directory();
        let right_dir = right.is_directory();
        if left_dir != right_dir {
            return if left_dir {
                Ordering::Less
            } else {
                Ordering::Greater
            };
        }
    }

    let result = match column {
        TransferBrowserSortColumn::Name => natural_compare_ascii(&left.name, &right.name),
        TransferBrowserSortColumn::Size => left.size.unwrap_or(0).cmp(&right.size.unwrap_or(0)),
        TransferBrowserSortColumn::Modified => left
            .modified_at
            .unwrap_or(0)
            .cmp(&right.modified_at.unwrap_or(0)),
        TransferBrowserSortColumn::Permissions => left
            .permissions
            .unwrap_or(0)
            .cmp(&right.permissions.unwrap_or(0)),
        TransferBrowserSortColumn::Owner => natural_compare_ascii(&left.owner, &right.owner),
        TransferBrowserSortColumn::Group => natural_compare_ascii(&left.group, &right.group),
    };

    let directed = match direction {
        TransferBrowserSortDirection::Ascending => result,
        TransferBrowserSortDirection::Descending => result.reverse(),
    };
    directed.then_with(|| natural_compare_ascii(&left.name, &right.name))
}

pub(in crate::features) fn natural_compare_ascii(left: &str, right: &str) -> Ordering {
    let left = left.to_lowercase();
    let right = right.to_lowercase();
    let mut left_chars = left.chars().peekable();
    let mut right_chars = right.chars().peekable();

    loop {
        match (left_chars.peek().copied(), right_chars.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(left_char), Some(right_char))
                if left_char.is_ascii_digit() && right_char.is_ascii_digit() =>
            {
                let mut left_number = String::new();
                while let Some(value) = left_chars.peek().copied() {
                    if value.is_ascii_digit() {
                        left_number.push(value);
                        left_chars.next();
                    } else {
                        break;
                    }
                }
                let mut right_number = String::new();
                while let Some(value) = right_chars.peek().copied() {
                    if value.is_ascii_digit() {
                        right_number.push(value);
                        right_chars.next();
                    } else {
                        break;
                    }
                }
                let left_trimmed = left_number.trim_start_matches('0');
                let right_trimmed = right_number.trim_start_matches('0');
                let left_key = if left_trimmed.is_empty() {
                    "0"
                } else {
                    left_trimmed
                };
                let right_key = if right_trimmed.is_empty() {
                    "0"
                } else {
                    right_trimmed
                };
                let ordering = left_key
                    .len()
                    .cmp(&right_key.len())
                    .then_with(|| left_key.cmp(right_key))
                    .then_with(|| left_number.len().cmp(&right_number.len()));
                if ordering != Ordering::Equal {
                    return ordering;
                }
            }
            (Some(left_char), Some(right_char)) => {
                left_chars.next();
                right_chars.next();
                let ordering = left_char.cmp(&right_char);
                if ordering != Ordering::Equal {
                    return ordering;
                }
            }
        }
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
mod regression_tests {
    use super::*;
    use nyaterm_transport::SftpFileType;

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

    #[test]
    fn browser_sort_must_be_transitive() {
        let a = entry("z", SftpFileType::Symlink, true);
        let b = entry("m", SftpFileType::Symlink, false);
        let c = entry("a", SftpFileType::File, false);

        let before = |left: &SftpFileEntry, right: &SftpFileEntry| {
            compare_transfer_browser_entries(
                left,
                right,
                TransferBrowserSortColumn::Name,
                TransferBrowserSortDirection::Ascending,
            )
            .is_lt()
        };

        // A strict ordering cannot contain a cycle.
        assert!(
            !(before(&b, &a) && before(&a, &c) && before(&c, &b)),
            "browser sorting comparator is not transitive"
        );
    }
}
