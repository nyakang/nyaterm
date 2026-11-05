//! Pure filter and sort logic for the SFTP browser listing, and its memo.
//!
//! Deliberately free of GPUI and of anything under `pages`: the state layer owns
//! both the derivation and the cache for it, so the view has nothing to drive and
//! nothing to remember. These functions used to live in `pages/transfers`, which
//! made the state layer unable to reach them without inverting the dependency.

use crate::models::{TransferBrowserSortColumn, TransferBrowserSortDirection};
use nyaterm_files::sftp::SftpFileEntry;
use std::cmp::Ordering;
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
        && (normalized_query.is_empty()
            || contains_ascii_case_insensitive(&entry.name, normalized_query)
            || (!entry.name.is_ascii() && entry.name.to_lowercase().contains(normalized_query)))
}

pub(in crate::features) fn compare_transfer_browser_entries(
    left: &SftpFileEntry,
    right: &SftpFileEntry,
    column: TransferBrowserSortColumn,
    direction: TransferBrowserSortDirection,
) -> Ordering {
    // Directory links and file links share a file type, but must still belong to
    // different sort groups. Grouping cannot depend on the other entry's type.
    let directory_order = right.is_directory().cmp(&left.is_directory());
    if directory_order != Ordering::Equal {
        return directory_order;
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
    if left.is_ascii() && right.is_ascii() {
        return natural_compare_ascii_bytes(left.as_bytes(), right.as_bytes());
    }
    let left = left.to_lowercase();
    let right = right.to_lowercase();
    natural_compare_lowercase(left.as_str(), right.as_str())
}

fn natural_compare_ascii_bytes(left: &[u8], right: &[u8]) -> Ordering {
    let mut left_rest = left;
    let mut right_rest = right;

    loop {
        match (left_rest.is_empty(), right_rest.is_empty()) {
            (true, true) => return Ordering::Equal,
            (true, false) => return Ordering::Less,
            (false, true) => return Ordering::Greater,
            (false, false) => {}
        }

        if left_rest[0].is_ascii_digit() && right_rest[0].is_ascii_digit() {
            let left_end = left_rest
                .iter()
                .position(|value| !value.is_ascii_digit())
                .unwrap_or(left_rest.len());
            let right_end = right_rest
                .iter()
                .position(|value| !value.is_ascii_digit())
                .unwrap_or(right_rest.len());
            let ordering = compare_digit_runs(&left_rest[..left_end], &right_rest[..right_end]);
            if ordering != Ordering::Equal {
                return ordering;
            }
            left_rest = &left_rest[left_end..];
            right_rest = &right_rest[right_end..];
            continue;
        }

        let left_char = left_rest[0].to_ascii_lowercase();
        let right_char = right_rest[0].to_ascii_lowercase();
        if left_char != right_char {
            return left_char.cmp(&right_char);
        }
        left_rest = &left_rest[1..];
        right_rest = &right_rest[1..];
    }
}

fn natural_compare_lowercase(left: &str, right: &str) -> Ordering {
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
                let left_start = left_chars.clone();
                let right_start = right_chars.clone();
                while left_chars
                    .peek()
                    .is_some_and(|value| value.is_ascii_digit())
                {
                    left_chars.next();
                }
                while right_chars
                    .peek()
                    .is_some_and(|value| value.is_ascii_digit())
                {
                    right_chars.next();
                }
                let left_number = left_start
                    .take_while(|value| value.is_ascii_digit())
                    .collect::<String>();
                let right_number = right_start
                    .take_while(|value| value.is_ascii_digit())
                    .collect::<String>();
                let ordering = compare_digit_runs(left_number.as_bytes(), right_number.as_bytes());
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

fn compare_digit_runs(left: &[u8], right: &[u8]) -> Ordering {
    let left_trimmed = trim_leading_zeroes(left);
    let right_trimmed = trim_leading_zeroes(right);
    left_trimmed
        .len()
        .cmp(&right_trimmed.len())
        .then_with(|| left_trimmed.cmp(right_trimmed))
        .then_with(|| left.len().cmp(&right.len()))
}

fn trim_leading_zeroes(value: &[u8]) -> &[u8] {
    let first_nonzero = value
        .iter()
        .position(|byte| *byte != b'0')
        .unwrap_or(value.len().saturating_sub(1));
    &value[first_nonzero..]
}

fn contains_ascii_case_insensitive(haystack: &str, needle: &str) -> bool {
    if !haystack.is_ascii() || !needle.is_ascii() {
        return false;
    }
    let needle = needle.as_bytes();
    needle.is_empty()
        || haystack
            .as_bytes()
            .windows(needle.len())
            .any(|window| window.eq_ignore_ascii_case(needle))
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
    use super::{BrowserFilterCache, BrowserFilterKey, compare_transfer_browser_entries};
    use crate::models::{TransferBrowserSortColumn, TransferBrowserSortDirection};
    use nyaterm_files::sftp::{SftpFileEntry, SftpFileType};
    use std::cmp::Ordering;
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

    const COLUMNS: [TransferBrowserSortColumn; 6] = [
        TransferBrowserSortColumn::Name,
        TransferBrowserSortColumn::Size,
        TransferBrowserSortColumn::Modified,
        TransferBrowserSortColumn::Permissions,
        TransferBrowserSortColumn::Owner,
        TransferBrowserSortColumn::Group,
    ];
    const DIRECTIONS: [TransferBrowserSortDirection; 2] = [
        TransferBrowserSortDirection::Ascending,
        TransferBrowserSortDirection::Descending,
    ];
    const KINDS: [(SftpFileType, bool); 5] = [
        (SftpFileType::Directory, false),
        (SftpFileType::Symlink, true),
        (SftpFileType::Symlink, false),
        (SftpFileType::File, false),
        (SftpFileType::Other, false),
    ];

    #[test]
    fn all_browser_sort_modes_obey_ordering_laws_for_mixed_entries() {
        let mut entries = Vec::new();
        for (index, name) in ["", "a", "A", "m", "z", "file2", "file02", "file10", "文2"]
            .into_iter()
            .enumerate()
        {
            for (file_type, directory_link) in KINDS {
                let mut item = entry(name, file_type, directory_link);
                item.size = (index % 3 != 0).then_some((index % 4) as u64);
                item.modified_at = (index % 3 != 0).then_some((index % 4) as u32);
                item.permissions = (index % 3 != 0).then_some((index % 4) as u32);
                item.owner = format!("owner{}", index % 3);
                item.group = format!("group{}", index % 2);
                entries.push(item);
            }
        }
        // Missing metadata and explicit zero are intentionally equivalent.
        let mut zero = entries[0].clone();
        zero.size = Some(0);
        zero.modified_at = Some(0);
        zero.permissions = Some(0);
        entries.push(zero);

        for column in COLUMNS {
            for direction in DIRECTIONS {
                let orders: Vec<Vec<Ordering>> = entries
                    .iter()
                    .map(|left| {
                        entries
                            .iter()
                            .map(|right| {
                                compare_transfer_browser_entries(left, right, column, direction)
                            })
                            .collect()
                    })
                    .collect();
                for (a, left) in entries.iter().enumerate() {
                    assert_eq!(orders[a][a], Ordering::Equal);
                    for (b, right) in entries.iter().enumerate() {
                        assert_eq!(orders[a][b], orders[b][a].reverse());
                        if left.is_directory() != right.is_directory() {
                            assert_eq!(orders[a][b].is_lt(), left.is_directory());
                        }
                        for c in 0..entries.len() {
                            if orders[a][b].is_le() && orders[b][c].is_le() {
                                assert!(
                                    orders[a][c].is_le(),
                                    "non-transitive: {column:?}, {direction:?}, {a}/{b}/{c}"
                                );
                            }
                            if orders[a][b] == Ordering::Equal {
                                assert_eq!(orders[a][c], orders[b][c]);
                            }
                        }
                    }
                }
            }
        }
    }

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
                    if direction == TransferBrowserSortDirection::Descending {
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

    #[test]
    fn browser_sort_preserves_name_ties_and_missing_metadata_rules() {
        let mut a = entry("file2", SftpFileType::File, false);
        let mut b = entry("file10", SftpFileType::File, false);
        for column in COLUMNS {
            if column == TransferBrowserSortColumn::Name {
                continue;
            }
            for direction in DIRECTIONS {
                assert_eq!(
                    compare_transfer_browser_entries(&a, &b, column, direction),
                    Ordering::Less,
                    "name fallback stays ascending: {column:?}, {direction:?}",
                );
            }
        }
        a.size = Some(1);
        b.size = Some(0);
        assert_eq!(
            compare_transfer_browser_entries(
                &a,
                &b,
                TransferBrowserSortColumn::Size,
                TransferBrowserSortDirection::Ascending
            ),
            Ordering::Greater,
        );
        b.size = None;
        assert_eq!(
            compare_transfer_browser_entries(
                &a,
                &b,
                TransferBrowserSortColumn::Size,
                TransferBrowserSortDirection::Ascending
            ),
            Ordering::Greater,
        );
    }
}

#[cfg(test)]
mod natural_sort_equivalence_tests {
    use std::cmp::Ordering;

    use super::natural_compare_ascii;

    // Reference: the original allocation-heavy implementation (lowercase + chars).
    fn reference(left: &str, right: &str) -> Ordering {
        let (left, right) = (left.to_lowercase(), right.to_lowercase());
        let (mut l, mut r) = (left.chars().peekable(), right.chars().peekable());
        loop {
            match (l.peek().copied(), r.peek().copied()) {
                (None, None) => return Ordering::Equal,
                (None, Some(_)) => return Ordering::Less,
                (Some(_), None) => return Ordering::Greater,
                (Some(a), Some(b)) if a.is_ascii_digit() && b.is_ascii_digit() => {
                    let mut ln = String::new();
                    while let Some(c) = l.peek().copied().filter(char::is_ascii_digit) {
                        ln.push(c);
                        l.next();
                    }
                    let mut rn = String::new();
                    while let Some(c) = r.peek().copied().filter(char::is_ascii_digit) {
                        rn.push(c);
                        r.next();
                    }
                    let (lt, rt) = (ln.trim_start_matches('0'), rn.trim_start_matches('0'));
                    let (lk, rk) = (
                        if lt.is_empty() { "0" } else { lt },
                        if rt.is_empty() { "0" } else { rt },
                    );
                    let o = lk
                        .len()
                        .cmp(&rk.len())
                        .then_with(|| lk.cmp(rk))
                        .then_with(|| ln.len().cmp(&rn.len()));
                    if o != Ordering::Equal {
                        return o;
                    }
                }
                (Some(a), Some(b)) => {
                    l.next();
                    r.next();
                    if a != b {
                        return a.cmp(&b);
                    }
                }
            }
        }
    }

    const SAMPLES: &[&str] = &[
        "",
        "a",
        "A",
        "file1",
        "file01",
        "file001",
        "file2",
        "file10",
        "File10",
        "0",
        "00",
        "000",
        "007",
        "7",
        "a0",
        "a00",
        "a1b2",
        "a01b2",
        "x.tar.gz",
        ".hidden",
        "name (1)",
        "z",
        "123456789012345678901234567890123",
        "0123456789012345678901234567890123",
        "é",
        "É",
        "e\u{301}",
        "İ",
        "ß",
        "ẞ",
        "ǅ",
        "\u{212A}",
        "k",
        "K",
        "Σ",
        "ς",
        "σ",
        "中",
        "😀",
        "a中1",
        "A中01",
        "k1",
        "\u{212A}1",
        "ÉCOLE2",
        "école10",
    ];

    #[test]
    fn natural_compare_matches_reference_for_ascii_and_unicode_samples() {
        for a in SAMPLES {
            for b in SAMPLES {
                assert_eq!(
                    natural_compare_ascii(a, b),
                    reference(a, b),
                    "{a:?} vs {b:?}"
                );
            }
        }
    }

    #[test]
    fn natural_compare_matches_reference_on_pseudo_random_strings() {
        const ALPHABET: &[&str] = &[
            "a", "B", "k", "K", "0", "0", "1", "9", ".", " ", "-", "é", "İ", "ß", "\u{212A}", "中",
            "😀", "\u{301}",
        ];
        let mut state = 0x9E37_79B9_7F4A_7C15_u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let make = |next: &mut dyn FnMut() -> u64| {
            let len = (next() % 12) as usize;
            (0..len)
                .map(|_| ALPHABET[(next() % ALPHABET.len() as u64) as usize])
                .collect::<String>()
        };
        for _ in 0..20_000 {
            let (a, b) = (make(&mut next), make(&mut next));
            assert_eq!(
                natural_compare_ascii(&a, &b),
                reference(&a, &b),
                "{a:?} vs {b:?}"
            );
        }
    }
}

#[cfg(test)]
mod browser_filter_equivalence_tests {
    use nyaterm_files::sftp::{SftpFileEntry, SftpFileType};

    use super::transfer_browser_entry_is_visible;

    fn entry(name: &str) -> SftpFileEntry {
        SftpFileEntry {
            name: name.to_string(),
            path: name.to_string(),
            file_type: SftpFileType::File,
            size: None,
            permissions: None,
            owner: String::new(),
            group: String::new(),
            modified_at: None,
            raw_path_token: None,
            symlink_target_is_directory: false,
        }
    }

    // Callers pass `search.trim().to_lowercase()`; the contract is a lowercase query.
    fn reference(name: &str, query: &str, show_hidden: bool) -> bool {
        (show_hidden || !name.starts_with('.'))
            && (query.is_empty() || name.to_lowercase().contains(query))
    }

    #[test]
    fn filter_matches_reference_for_lowercase_queries() {
        let names = [
            "Notes.TXT",
            "notes",
            ".Env",
            "résumé.PDF",
            "\u{212A}elvin",
            "İstanbul",
            "ẞ",
            "中文.md",
            "",
            "a b",
        ];
        let queries = [
            "", "notes", "txt", "env", "résumé", "k", "kelvin", "i̇s", "ß", "中", "a b", "zzz", "é",
        ];
        for n in names {
            for q in queries {
                for hidden in [false, true] {
                    assert_eq!(
                        transfer_browser_entry_is_visible(&entry(n), q, hidden),
                        reference(n, q, hidden),
                        "{n:?} {q:?} {hidden}"
                    );
                }
            }
        }
    }

    #[test]
    fn filter_matches_non_ascii_case_folding_that_yields_ascii() {
        // Kelvin sign lowercases to ASCII 'k'; the non-ASCII fallback must keep this match.
        assert!(transfer_browser_entry_is_visible(
            &entry("\u{212A}elvin"),
            "kelvin",
            false
        ));
    }
}
