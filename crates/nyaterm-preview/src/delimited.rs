//! Shared delimited data and ordering; callers own presentation and lifecycle.
use std::sync::Arc;

/// Which column a delimited preview is sorted by, and in what direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DelimitedSort {
    /// Ascending by the given column index.
    Ascending(usize),
    /// Descending by the given column index.
    Descending(usize),
}

/// Delimited (CSV/TSV) data with header-toggle and stable tri-state sorting.
///
/// Rows are stored once; sorting reorders an index vector rather than the rows
/// themselves, and the sort is stable, so clearing the sort restores the
/// original file order. Callers can render only the visible rows
/// through this ordering without materializing a full table.
#[derive(Clone)]
pub struct PreviewDelimited {
    /// Every parsed record, header row included. The header is drawn from
    /// `records[0]` when `first_row_is_header` is set.
    records: Arc<Vec<Vec<String>>>,
    /// Whether the first record is treated as a header row.
    first_row_is_header: bool,
    /// Current sort, or `None` for original file order.
    sort: Option<DelimitedSort>,
    /// Order of *body* row indices (into `records`), reflecting the current
    /// sort. Recomputed by [`Self::resort`].
    order: Arc<Vec<usize>>,
    /// Whether the body was truncated at the row ceiling during parsing.
    truncated: bool,
    /// Widest column count across all records, so ragged rows still lay out.
    column_count: usize,
}

impl PreviewDelimited {
    pub fn new(records: Vec<Vec<String>>, truncated: bool) -> Self {
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

    /// The original header row, if header interpretation is enabled.
    pub fn header_row(&self) -> Option<&[String]> {
        self.first_row_is_header
            .then(|| self.records.first().map(Vec::as_slice))
            .flatten()
    }

    pub fn first_row_is_header(&self) -> bool {
        self.first_row_is_header
    }

    pub fn sort(&self) -> Option<DelimitedSort> {
        self.sort
    }

    pub fn column_count(&self) -> usize {
        self.column_count
    }

    pub fn truncated(&self) -> bool {
        self.truncated
    }

    /// Number of body rows currently displayed.
    pub fn row_count(&self) -> usize {
        self.order.len()
    }

    /// A displayed body row by its position in the current ordering.
    pub fn row(&self, position: usize) -> Option<&[String]> {
        self.order
            .get(position)
            .and_then(|record_index| self.records.get(*record_index))
            .map(Vec::as_slice)
    }

    pub fn toggle_header(&mut self) {
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
    pub fn cycle_sort(&mut self, column: usize) {
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
    fn resort(&mut self) {
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

#[cfg(test)]
mod tests {
    use super::{DelimitedSort, PreviewDelimited};

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
        assert_eq!(data.header_row().unwrap(), &["name", "age"]);
        assert_eq!(data.row_count(), 2);

        data.toggle_header();
        assert!(!data.first_row_is_header);
        // The former header row becomes a body row.
        assert_eq!(data.header_row(), None);
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

    #[test]
    fn empty_and_ragged_tables_keep_safe_row_access_and_column_extent() {
        let mut empty = PreviewDelimited::new(Vec::new(), true);
        assert_eq!(empty.column_count(), 1);
        assert_eq!(empty.header_row(), None);
        assert_eq!(empty.row(0), None);
        assert!(empty.truncated());
        empty.cycle_sort(5);
        empty.toggle_header();
        assert_eq!(empty.sort(), None);
        assert_eq!(empty.row_count(), 0);

        let mut ragged = delimited(&[&["name"], &["b", "2"], &[], &["a"]]);
        assert_eq!(ragged.column_count(), 2);
        assert_eq!(ragged.header_row().unwrap(), &["name"]);
        ragged.cycle_sort(1);
        assert_eq!(ragged.row(0).unwrap(), &["b", "2"]);
        assert_eq!(ragged.row(1).unwrap(), Vec::<String>::new());
        assert_eq!(ragged.row(3), None);
    }
}
