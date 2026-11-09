//! Match terminal text entities and reuse unchanged snapshot rows.
use crate::{TerminalSnapshot, terminal_cell_col_for_byte_index};
use nyaterm_core::ActionLinksMatcherSettings;
use nyaterm_core::action_links::{ActionLinkMatch, find_action_links};
use std::hash::{Hash, Hasher};

#[derive(Debug, Clone, Default)]
pub struct TerminalFrameActionLinks {
    pub matcher_key: u64,
    pub absolute_start_row: usize,
    pub absolute_end_row: usize,
    pub row_signatures: Vec<u64>,
    pub matches_by_line: Vec<Vec<ActionLinkMatch>>,
    pub cell_ranges_by_line: Vec<Vec<(usize, usize)>>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TerminalActionLinkBuildStats {
    pub reused_rows: usize,
    pub rebuilt_rows: usize,
}

impl TerminalFrameActionLinks {
    pub fn source_index_for_snapshot_row(
        &self,
        snapshot: &TerminalSnapshot,
        line_index: usize,
    ) -> Option<usize> {
        let row = snapshot.row(line_index)?;
        let absolute_end_row = snapshot.total_rows.saturating_sub(snapshot.display_offset);
        let absolute_start_row = absolute_end_row.saturating_sub(snapshot.row_count());
        let absolute_row = absolute_start_row.checked_add(line_index)?;
        if absolute_row >= absolute_end_row
            || absolute_row < self.absolute_start_row
            || absolute_row >= self.absolute_end_row
        {
            return None;
        }
        let source_index = absolute_row - self.absolute_start_row;
        if self.row_signatures.get(source_index).copied()? != row.signature {
            return None;
        }
        Some(source_index)
    }

    pub fn overlaps_snapshot(&self, snapshot: &TerminalSnapshot) -> bool {
        let absolute_end_row = snapshot.total_rows.saturating_sub(snapshot.display_offset);
        let absolute_start_row = absolute_end_row.saturating_sub(snapshot.row_count());
        self.absolute_start_row < absolute_end_row && absolute_start_row < self.absolute_end_row
    }

    pub fn covers_all_snapshot_rows(&self, snapshot: &TerminalSnapshot) -> bool {
        (0..snapshot.row_count()).all(|line_index| {
            self.source_index_for_snapshot_row(snapshot, line_index)
                .is_some()
        })
    }

    pub fn has_matching_decorated_snapshot_rows(&self, snapshot: &TerminalSnapshot) -> bool {
        (0..snapshot.row_count()).any(|line_index| {
            self.source_index_for_snapshot_row(snapshot, line_index)
                .is_some_and(|source_index| {
                    self.matches_by_line
                        .get(source_index)
                        .is_some_and(|matches| !matches.is_empty())
                        || self
                            .cell_ranges_by_line
                            .get(source_index)
                            .is_some_and(|ranges| !ranges.is_empty())
                })
        })
    }
}

pub fn terminal_action_link_matcher_key(
    enabled: bool,
    matchers: &ActionLinksMatcherSettings,
) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    enabled.hash(&mut hasher);
    matchers.mac.hash(&mut hasher);
    matchers.ipv4.hash(&mut hasher);
    matchers.archive.hash(&mut hasher);
    matchers.host_port.hash(&mut hasher);
    hasher.finish()
}

pub fn prepare_terminal_frame_action_links(
    snapshot: &TerminalSnapshot,
    enabled: bool,
    matchers: &ActionLinksMatcherSettings,
) -> Option<TerminalFrameActionLinks> {
    prepare_terminal_frame_action_links_reusing(snapshot, enabled, matchers, None).0
}

pub fn prepare_terminal_frame_action_links_reusing(
    snapshot: &TerminalSnapshot,
    enabled: bool,
    matchers: &ActionLinksMatcherSettings,
    previous: Option<&TerminalFrameActionLinks>,
) -> (
    Option<TerminalFrameActionLinks>,
    TerminalActionLinkBuildStats,
) {
    let absolute_end_row = snapshot.total_rows.saturating_sub(snapshot.display_offset);
    let absolute_start_row = absolute_end_row.saturating_sub(snapshot.row_count());
    let row_signatures = snapshot
        .rows()
        .iter()
        .map(|row| row.signature)
        .collect::<Vec<_>>();
    if !enabled {
        return (
            Some(TerminalFrameActionLinks {
                matcher_key: terminal_action_link_matcher_key(false, matchers),
                absolute_start_row,
                absolute_end_row,
                row_signatures,
                matches_by_line: vec![Vec::new(); snapshot.row_count()],
                cell_ranges_by_line: vec![Vec::new(); snapshot.row_count()],
            }),
            TerminalActionLinkBuildStats::default(),
        );
    }
    let matcher_key = terminal_action_link_matcher_key(true, matchers);
    let reusable = previous.filter(|links| links.matcher_key == matcher_key);
    let mut matches_by_line = Vec::with_capacity(snapshot.row_count());
    let mut cell_ranges_by_line = Vec::with_capacity(snapshot.row_count());
    let mut stats = TerminalActionLinkBuildStats::default();
    for (line_index, row) in snapshot.rows().iter().enumerate() {
        let absolute_row = absolute_start_row.saturating_add(line_index);
        let reused = reusable.and_then(|links| {
            if absolute_row < links.absolute_start_row || absolute_row >= links.absolute_end_row {
                return None;
            }
            let source_index = absolute_row - links.absolute_start_row;
            if links.row_signatures.get(source_index) != Some(&row.signature) {
                return None;
            }
            Some((
                links.matches_by_line.get(source_index)?.clone(),
                links.cell_ranges_by_line.get(source_index)?.clone(),
            ))
        });
        if let Some((matches, cell_ranges)) = reused {
            stats.reused_rows += 1;
            matches_by_line.push(matches);
            cell_ranges_by_line.push(cell_ranges);
            continue;
        }
        stats.rebuilt_rows += 1;
        let matches = if row.text.is_empty() {
            Vec::new()
        } else {
            find_action_links(&row.text, matchers, true)
        };
        let cell_ranges = matches
            .iter()
            .map(|item| {
                (
                    terminal_cell_col_for_byte_index(&row.text, item.start),
                    terminal_cell_col_for_byte_index(&row.text, item.end),
                )
            })
            .collect();
        matches_by_line.push(matches);
        cell_ranges_by_line.push(cell_ranges);
    }
    (
        Some(TerminalFrameActionLinks {
            matcher_key,
            absolute_start_row,
            absolute_end_row,
            row_signatures,
            matches_by_line,
            cell_ranges_by_line,
        }),
        stats,
    )
}

#[cfg(test)]
mod tests {
    use super::{prepare_terminal_frame_action_links, prepare_terminal_frame_action_links_reusing};
    use crate::TerminalScreen;
    use nyaterm_core::ActionLinksMatcherSettings;
    #[test]
    fn terminal_frame_action_links_align_with_snapshot_lines() {
        let mut screen = TerminalScreen::default();
        screen.advance_decoded_text("visit http://example.com\nping 10.0.0.1");
        let snapshot = screen.viewport_snapshot(0);
        let matchers = ActionLinksMatcherSettings::default();

        let links = prepare_terminal_frame_action_links(&snapshot, true, &matchers).unwrap();
        let absolute_end_row = snapshot.total_rows.saturating_sub(snapshot.display_offset);
        let absolute_start_row = absolute_end_row.saturating_sub(snapshot.row_count());

        assert_eq!(links.absolute_start_row, absolute_start_row);
        assert_eq!(links.absolute_end_row, absolute_end_row);
        assert_eq!(links.matches_by_line.len(), snapshot.row_count());
        assert_eq!(links.cell_ranges_by_line.len(), snapshot.row_count());
        assert!(
            links
                .matches_by_line
                .iter()
                .flatten()
                .any(|item| item.value == "http://example.com")
        );
        assert!(
            links
                .cell_ranges_by_line
                .iter()
                .flatten()
                .any(|range| *range == (6, 24))
        );
        assert!(
            links
                .matches_by_line
                .iter()
                .flatten()
                .any(|item| item.value == "10.0.0.1")
        );

        let disabled = prepare_terminal_frame_action_links(&snapshot, false, &matchers).unwrap();
        assert!(disabled.matches_by_line.iter().all(Vec::is_empty));
        assert!(disabled.cell_ranges_by_line.iter().all(Vec::is_empty));
        assert_ne!(links.matcher_key, disabled.matcher_key);
    }
    #[test]
    fn mac_ranges_map_wide_text_and_invalidate_only_when_the_matcher_changes() {
        let mut screen = TerminalScreen::new(80, 3);
        screen.advance_decoded_text("设备 [905d.7c16.b6d9]");
        let snapshot = screen.viewport_snapshot(0);
        let mut matchers = ActionLinksMatcherSettings::default();
        let links = prepare_terminal_frame_action_links(&snapshot, true, &matchers).unwrap();
        assert_eq!(
            links.matches_by_line[0][0].kind,
            nyaterm_core::action_links::ActionLinkKind::Mac
        );
        assert_eq!(links.cell_ranges_by_line[0], [(6, 20)]);
        let (_, stats) =
            prepare_terminal_frame_action_links_reusing(&snapshot, true, &matchers, Some(&links));
        assert_eq!(stats.reused_rows, snapshot.row_count());
        assert_eq!(stats.rebuilt_rows, 0);
        matchers.mac = false;
        let (disabled, stats) =
            prepare_terminal_frame_action_links_reusing(&snapshot, true, &matchers, Some(&links));
        let disabled = disabled.unwrap();
        assert_ne!(links.matcher_key, disabled.matcher_key);
        assert_eq!(stats.reused_rows, 0);
        assert_eq!(stats.rebuilt_rows, snapshot.row_count());
        assert!(disabled.matches_by_line.iter().all(Vec::is_empty));
        assert!(disabled.cell_ranges_by_line.iter().all(Vec::is_empty));
    }
    #[test]
    fn terminal_frame_action_links_reuse_unchanged_rows() {
        let mut screen = TerminalScreen::new(80, 8);
        screen.advance_decoded_text("visit http://example.com");
        let first_snapshot = screen.viewport_snapshot(0);
        let matchers = ActionLinksMatcherSettings::default();
        let first = prepare_terminal_frame_action_links(&first_snapshot, true, &matchers).unwrap();

        let (_, unchanged_stats) = prepare_terminal_frame_action_links_reusing(
            &first_snapshot,
            true,
            &matchers,
            Some(&first),
        );
        assert_eq!(unchanged_stats.reused_rows, first_snapshot.row_count());
        assert_eq!(unchanged_stats.rebuilt_rows, 0);

        screen.advance_decoded_text(" updated");
        let changed_snapshot = screen.viewport_snapshot(0);
        let (_, changed_stats) = prepare_terminal_frame_action_links_reusing(
            &changed_snapshot,
            true,
            &matchers,
            Some(&first),
        );
        assert!(changed_stats.reused_rows > 0);
        assert!(changed_stats.rebuilt_rows > 0);
        assert_eq!(
            changed_stats.reused_rows + changed_stats.rebuilt_rows,
            changed_snapshot.row_count()
        );
    }
    #[test]
    fn terminal_frame_action_link_matcher_change_rebuilds_all_rows() {
        let mut screen = TerminalScreen::new(80, 8);
        screen.advance_decoded_text("visit http://example.com and 10.0.0.1");
        let snapshot = screen.viewport_snapshot(0);
        let matchers = ActionLinksMatcherSettings::default();
        let first = prepare_terminal_frame_action_links(&snapshot, true, &matchers).unwrap();
        let mut changed_matchers = matchers.clone();
        changed_matchers.ipv4 = false;

        let (_, stats) = prepare_terminal_frame_action_links_reusing(
            &snapshot,
            true,
            &changed_matchers,
            Some(&first),
        );
        assert_eq!(stats.reused_rows, 0);
        assert_eq!(stats.rebuilt_rows, snapshot.row_count());
    }
}
