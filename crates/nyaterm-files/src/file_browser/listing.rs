//! File listing policies. Callers own query normalization, caches and presentation.
use crate::sftp::SftpFileEntry;
use std::cmp::Ordering;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileBrowserSortColumn {
    Name,
    Modified,
    Size,
    Permissions,
    Owner,
    Group,
}

impl FileBrowserSortColumn {
    pub fn default_direction(self) -> FileBrowserSortDirection {
        match self {
            Self::Name | Self::Permissions | Self::Owner | Self::Group => {
                FileBrowserSortDirection::Ascending
            }
            Self::Size | Self::Modified => FileBrowserSortDirection::Descending,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileBrowserSortDirection {
    Ascending,
    Descending,
}

impl FileBrowserSortDirection {
    pub fn toggled(self) -> Self {
        match self {
            Self::Ascending => Self::Descending,
            Self::Descending => Self::Ascending,
        }
    }
}

/// `normalized_query` must be trimmed and lowercased once by the caller.
pub fn entry_is_visible(
    entry: &SftpFileEntry,
    normalized_query: &str,
    show_hidden_files: bool,
) -> bool {
    (show_hidden_files || !entry.name.starts_with('.'))
        && (normalized_query.is_empty()
            || contains_ascii_case_insensitive(&entry.name, normalized_query)
            || (!entry.name.is_ascii() && entry.name.to_lowercase().contains(normalized_query)))
}

pub fn compare_entries(
    left: &SftpFileEntry,
    right: &SftpFileEntry,
    column: FileBrowserSortColumn,
    direction: FileBrowserSortDirection,
) -> Ordering {
    // Directory links and file links share a file type, but must still belong to
    // different sort groups. Grouping cannot depend on the other entry's type.
    let directory_order = right.is_directory().cmp(&left.is_directory());
    if directory_order != Ordering::Equal {
        return directory_order;
    }

    let result = match column {
        FileBrowserSortColumn::Name => natural_compare(&left.name, &right.name),
        FileBrowserSortColumn::Size => left.size.unwrap_or(0).cmp(&right.size.unwrap_or(0)),
        FileBrowserSortColumn::Modified => left
            .modified_at
            .unwrap_or(0)
            .cmp(&right.modified_at.unwrap_or(0)),
        FileBrowserSortColumn::Permissions => left
            .permissions
            .unwrap_or(0)
            .cmp(&right.permissions.unwrap_or(0)),
        FileBrowserSortColumn::Owner => natural_compare(&left.owner, &right.owner),
        FileBrowserSortColumn::Group => natural_compare(&left.group, &right.group),
    };

    let directed = match direction {
        FileBrowserSortDirection::Ascending => result,
        FileBrowserSortDirection::Descending => result.reverse(),
    };
    directed.then_with(|| natural_compare(&left.name, &right.name))
}

/// Case-insensitive natural order; case-folded ties intentionally remain equal.
/// Unlike core asset ordering, this preserves the original order of tied entries.
pub fn natural_compare(left: &str, right: &str) -> Ordering {
    if left.is_ascii() && right.is_ascii() {
        return natural_compare_bytes(left.as_bytes(), right.as_bytes());
    }
    let left = left.to_lowercase();
    let right = right.to_lowercase();
    natural_compare_lowercase(left.as_str(), right.as_str())
}

fn natural_compare_bytes(left: &[u8], right: &[u8]) -> Ordering {
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

#[cfg(test)]
mod regression_tests {
    use super::{FileBrowserSortColumn, FileBrowserSortDirection, compare_entries};
    use crate::sftp::{SftpFileEntry, SftpFileType};
    use std::cmp::Ordering;

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
            compare_entries(
                left,
                right,
                FileBrowserSortColumn::Name,
                FileBrowserSortDirection::Ascending,
            )
            .is_lt()
        };

        // A strict ordering cannot contain a cycle.
        assert!(
            !(before(&b, &a) && before(&a, &c) && before(&c, &b)),
            "browser sorting comparator is not transitive"
        );
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
                            .map(|right| compare_entries(left, right, column, direction))
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
    fn browser_sort_preserves_name_ties_and_missing_metadata_rules() {
        let mut a = entry("file2", SftpFileType::File, false);
        let mut b = entry("file10", SftpFileType::File, false);
        for column in COLUMNS {
            if column == FileBrowserSortColumn::Name {
                continue;
            }
            for direction in DIRECTIONS {
                assert_eq!(
                    compare_entries(&a, &b, column, direction),
                    Ordering::Less,
                    "name fallback stays ascending: {column:?}, {direction:?}",
                );
            }
        }
        a.size = Some(1);
        b.size = Some(0);
        assert_eq!(
            compare_entries(
                &a,
                &b,
                FileBrowserSortColumn::Size,
                FileBrowserSortDirection::Ascending
            ),
            Ordering::Greater,
        );
        b.size = None;
        assert_eq!(
            compare_entries(
                &a,
                &b,
                FileBrowserSortColumn::Size,
                FileBrowserSortDirection::Ascending
            ),
            Ordering::Greater,
        );
    }
}

#[cfg(test)]
mod natural_sort_equivalence_tests {
    use std::cmp::Ordering;

    use super::natural_compare;

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
                assert_eq!(natural_compare(a, b), reference(a, b), "{a:?} vs {b:?}");
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
            assert_eq!(natural_compare(&a, &b), reference(&a, &b), "{a:?} vs {b:?}");
        }
    }
}

#[cfg(test)]
mod browser_filter_equivalence_tests {
    use crate::sftp::{SftpFileEntry, SftpFileType};

    use super::entry_is_visible;

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
                        entry_is_visible(&entry(n), q, hidden),
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
        assert!(entry_is_visible(&entry("\u{212A}elvin"), "kelvin", false));
    }
}
