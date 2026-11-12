use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use crate::element::TerminalLineDecorations;
use nyaterm_terminal::action_links::TerminalFrameActionLinks;

#[derive(Clone, Copy)]
pub struct TerminalDecorationSources<'a> {
    pub selected_occurrence_ranges_by_line: &'a HashMap<usize, Vec<(usize, usize)>>,
    pub search_ranges_by_line: &'a HashMap<usize, Vec<(usize, usize)>>,
    pub active_search_ranges_by_line: &'a HashMap<usize, Vec<(usize, usize)>>,
    pub frame_action_links: &'a [TerminalFrameActionLinks],
    pub include_action_links: bool,
    pub include_hyperlinks: bool,
}

pub fn terminal_snapshot_absolute_range(
    snapshot: &nyaterm_terminal::TerminalSnapshot,
) -> (usize, usize) {
    let end = snapshot.total_rows.saturating_sub(snapshot.display_offset);
    let start = end.saturating_sub(snapshot.row_count());
    (start, end)
}

pub fn terminal_absolute_line_for_snapshot_row(
    snapshot: &nyaterm_terminal::TerminalSnapshot,
    snapshot_row: usize,
) -> Option<usize> {
    if snapshot_row >= snapshot.row_count() {
        return None;
    }
    let (start, _) = terminal_snapshot_absolute_range(snapshot);
    Some(start.saturating_add(snapshot_row))
}

pub fn terminal_line_decorations_cache_key(
    snapshot: &nyaterm_terminal::TerminalSnapshot,
    sources: &TerminalDecorationSources<'_>,
) -> u64 {
    let TerminalDecorationSources {
        selected_occurrence_ranges_by_line,
        search_ranges_by_line,
        active_search_ranges_by_line,
        frame_action_links,
        include_action_links,
        include_hyperlinks,
    } = *sources;
    let mut hasher = DefaultHasher::new();
    snapshot.row_count().hash(&mut hasher);
    snapshot.cols.hash(&mut hasher);
    snapshot.display_offset.hash(&mut hasher);
    for row in snapshot.rows() {
        row.signature.hash(&mut hasher);
    }
    hash_ranges_by_line(selected_occurrence_ranges_by_line, &mut hasher);
    include_action_links.hash(&mut hasher);
    include_hyperlinks.hash(&mut hasher);
    hash_ranges_by_line(search_ranges_by_line, &mut hasher);
    hash_ranges_by_line(active_search_ranges_by_line, &mut hasher);
    if include_action_links {
        for line_index in 0..snapshot.row_count() {
            terminal_action_link_ranges_for_snapshot_row(snapshot, line_index, frame_action_links)
                .unwrap_or(&[])
                .hash(&mut hasher);
        }
    }
    if include_hyperlinks {
        snapshot.row_count().hash(&mut hasher);
        for row in snapshot.rows() {
            let spans = &row.hyperlinks;
            spans.len().hash(&mut hasher);
            for span in spans {
                span.start_col.hash(&mut hasher);
                span.end_col.hash(&mut hasher);
                span.uri.hash(&mut hasher);
            }
        }
    }
    hasher.finish()
}

fn terminal_action_link_ranges_for_source_row<'a>(
    snapshot: &nyaterm_terminal::TerminalSnapshot,
    line_index: usize,
    links: &'a TerminalFrameActionLinks,
) -> Option<&'a [(usize, usize)]> {
    let source_index = links.source_index_for_snapshot_row(snapshot, line_index)?;
    Some(
        links
            .cell_ranges_by_line
            .get(source_index)
            .map(Vec::as_slice)
            .unwrap_or(&[]),
    )
}

pub fn terminal_action_link_ranges_for_snapshot_row<'a>(
    snapshot: &nyaterm_terminal::TerminalSnapshot,
    line_index: usize,
    links: &'a [TerminalFrameActionLinks],
) -> Option<&'a [(usize, usize)]> {
    links
        .iter()
        .find_map(|links| terminal_action_link_ranges_for_source_row(snapshot, line_index, links))
}

pub fn terminal_action_links_overlap_snapshot(
    snapshot: &nyaterm_terminal::TerminalSnapshot,
    links: &TerminalFrameActionLinks,
) -> bool {
    links.overlaps_snapshot(snapshot)
}

pub fn terminal_action_links_have_ranges_for_snapshot(
    snapshot: &nyaterm_terminal::TerminalSnapshot,
    links: &[TerminalFrameActionLinks],
) -> bool {
    (0..snapshot.row_count()).any(|line_index| {
        terminal_action_link_ranges_for_snapshot_row(snapshot, line_index, links)
            .is_some_and(|ranges| !ranges.is_empty())
    })
}

pub fn terminal_action_links_cover_all_snapshot_rows(
    snapshot: &nyaterm_terminal::TerminalSnapshot,
    links: &[TerminalFrameActionLinks],
) -> bool {
    (0..snapshot.row_count()).all(|line_index| {
        links.iter().any(|links| {
            links
                .source_index_for_snapshot_row(snapshot, line_index)
                .is_some()
        })
    })
}

fn hash_ranges_by_line<H: Hasher>(
    ranges_by_line: &HashMap<usize, Vec<(usize, usize)>>,
    hasher: &mut H,
) {
    let mut lines = ranges_by_line.keys().copied().collect::<Vec<_>>();
    lines.sort_unstable();
    lines.len().hash(hasher);
    for line in lines {
        line.hash(hasher);
        ranges_by_line
            .get(&line)
            .map(Vec::as_slice)
            .unwrap_or(&[])
            .hash(hasher);
    }
}

pub fn build_terminal_line_decorations(
    snapshot: &nyaterm_terminal::TerminalSnapshot,
    sources: &TerminalDecorationSources<'_>,
) -> Vec<TerminalLineDecorations> {
    let TerminalDecorationSources {
        selected_occurrence_ranges_by_line,
        search_ranges_by_line,
        active_search_ranges_by_line,
        frame_action_links,
        include_action_links,
        include_hyperlinks,
    } = *sources;
    let line_count = snapshot.row_count();
    let mut line_decorations = Vec::with_capacity(line_count);
    let empty_ranges: [(usize, usize); 0] = [];
    for line_index in 0..line_count {
        let mut link_ranges: Vec<(usize, usize)> = if include_action_links {
            terminal_action_link_ranges_for_snapshot_row(snapshot, line_index, frame_action_links)
                .map(<[_]>::to_vec)
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        if include_hyperlinks
            && let Some(spans) = snapshot.row(line_index).map(|row| &row.hyperlinks)
        {
            for span in spans {
                let start = span.start_col;
                let end = span.end_col.saturating_add(1);
                if end > start {
                    link_ranges.push((start, end));
                }
            }
        }
        let line_search_ranges = search_ranges_by_line
            .get(&line_index)
            .map(|ranges| ranges.as_slice())
            .unwrap_or(&empty_ranges);
        let line_active_search_ranges = active_search_ranges_by_line
            .get(&line_index)
            .map(|ranges| ranges.as_slice())
            .unwrap_or(&empty_ranges);
        line_decorations.push(TerminalLineDecorations {
            selected_occurrence_ranges: selected_occurrence_ranges_by_line
                .get(&line_index)
                .map(|ranges| ranges.as_slice())
                .unwrap_or(&empty_ranges)
                .to_vec(),
            search_ranges: line_search_ranges.to_vec(),
            active_search_ranges: line_active_search_ranges.to_vec(),
            link_ranges,
        });
    }
    line_decorations
}

pub fn terminal_line_decorations_needed(
    has_selected_occurrences: bool,
    has_search_decorations: bool,
    has_frame_action_links: bool,
    has_hyperlinks: bool,
) -> bool {
    has_selected_occurrences || has_search_decorations || has_frame_action_links || has_hyperlinks
}

#[cfg(test)]
mod tests {
    use nyaterm_terminal::TerminalScreen;
    use std::collections::HashMap;

    use nyaterm_terminal::action_links::TerminalFrameActionLinks;

    use super::{
        TerminalDecorationSources, build_terminal_line_decorations,
        terminal_line_decorations_cache_key, terminal_line_decorations_needed,
        terminal_snapshot_absolute_range,
    };

    #[test]
    fn link_decorations_merge_action_links_and_hyperlinks_when_included() {
        let mut screen = nyaterm_terminal::TerminalScreen::new(40, 3);
        screen.advance(b"\x1b]8;;https://example.com\x07click\x1b]8;;\x07 plain");
        let snapshot = screen.viewport_snapshot(0);
        let (absolute_start_row, absolute_end_row) = terminal_snapshot_absolute_range(&snapshot);
        let mut links = TerminalFrameActionLinks {
            matcher_key: 7,
            absolute_start_row,
            absolute_end_row,
            row_signatures: snapshot.rows().iter().map(|row| row.signature).collect(),
            matches_by_line: vec![Vec::new(); snapshot.row_count()],
            cell_ranges_by_line: vec![Vec::new(); snapshot.row_count()],
        };
        links.cell_ranges_by_line[0].push((6, 11));

        let decorations = build_terminal_line_decorations(
            &snapshot,
            &TerminalDecorationSources {
                selected_occurrence_ranges_by_line: &HashMap::new(),
                search_ranges_by_line: &HashMap::new(),
                active_search_ranges_by_line: &HashMap::new(),
                frame_action_links: std::slice::from_ref(&links),
                include_action_links: true,
                include_hyperlinks: true,
            },
        );

        assert_eq!(decorations[0].link_ranges, vec![(6, 11), (0, 5)]);
    }

    #[test]
    fn action_link_decorations_map_from_absolute_link_window() {
        let mut screen = nyaterm_terminal::TerminalScreen::new(40, 3);
        screen.advance(b"first\nsecond\nthird");
        let snapshot = screen.viewport_snapshot(0);
        let (snapshot_start, snapshot_end) = terminal_snapshot_absolute_range(&snapshot);
        let mut links = TerminalFrameActionLinks {
            matcher_key: 7,
            absolute_start_row: snapshot_start.saturating_sub(1),
            absolute_end_row: snapshot_end.saturating_add(1),
            row_signatures: vec![0; snapshot.row_count() + 2],
            matches_by_line: vec![Vec::new(); snapshot.row_count() + 2],
            cell_ranges_by_line: vec![Vec::new(); snapshot.row_count() + 2],
        };
        let relative_row = snapshot_start + 1 - links.absolute_start_row;
        links.row_signatures[relative_row] = snapshot.rows()[1].signature;
        links.cell_ranges_by_line[relative_row].push((2, 5));

        let decorations = build_terminal_line_decorations(
            &snapshot,
            &TerminalDecorationSources {
                selected_occurrence_ranges_by_line: &HashMap::new(),
                search_ranges_by_line: &HashMap::new(),
                active_search_ranges_by_line: &HashMap::new(),
                frame_action_links: std::slice::from_ref(&links),
                include_action_links: true,
                include_hyperlinks: false,
            },
        );

        assert!(decorations[0].link_ranges.is_empty());
        assert_eq!(decorations[1].link_ranges, vec![(2, 5)]);
        assert!(decorations[2].link_ranges.is_empty());
    }

    #[test]
    fn action_link_decorations_keep_partially_covered_bottom_rows() {
        let mut screen = nyaterm_terminal::TerminalScreen::new(40, 4);
        screen.advance(b"first\nsecond\nthird\nfourth");
        let snapshot = screen.viewport_snapshot(0);
        let (snapshot_start, snapshot_end) = terminal_snapshot_absolute_range(&snapshot);
        let top_links = TerminalFrameActionLinks {
            matcher_key: 7,
            absolute_start_row: snapshot_start,
            absolute_end_row: snapshot_start + 2,
            row_signatures: snapshot
                .rows()
                .iter()
                .take(2)
                .map(|row| row.signature)
                .collect(),
            matches_by_line: vec![Vec::new(); 2],
            cell_ranges_by_line: vec![vec![(1, 3)], Vec::new()],
        };
        let bottom_links = TerminalFrameActionLinks {
            matcher_key: 7,
            absolute_start_row: snapshot_end - 1,
            absolute_end_row: snapshot_end,
            row_signatures: vec![snapshot.rows()[snapshot.row_count() - 1].signature],
            matches_by_line: vec![Vec::new()],
            cell_ranges_by_line: vec![vec![(2, 6)]],
        };

        let decorations = build_terminal_line_decorations(
            &snapshot,
            &TerminalDecorationSources {
                selected_occurrence_ranges_by_line: &HashMap::new(),
                search_ranges_by_line: &HashMap::new(),
                active_search_ranges_by_line: &HashMap::new(),
                frame_action_links: &[top_links, bottom_links],
                include_action_links: true,
                include_hyperlinks: false,
            },
        );

        assert_eq!(decorations[0].link_ranges, vec![(1, 3)]);
        assert!(decorations[1].link_ranges.is_empty());
        assert!(decorations[2].link_ranges.is_empty());
        assert_eq!(decorations[3].link_ranges, vec![(2, 6)]);
    }

    #[test]
    fn action_link_decorations_reject_signature_mismatch() {
        let mut screen = nyaterm_terminal::TerminalScreen::new(40, 2);
        screen.advance(b"visit http://example.com");
        let snapshot = screen.viewport_snapshot(0);
        let (absolute_start_row, absolute_end_row) = terminal_snapshot_absolute_range(&snapshot);
        let links = TerminalFrameActionLinks {
            matcher_key: 7,
            absolute_start_row,
            absolute_end_row,
            row_signatures: vec![0; snapshot.row_count()],
            matches_by_line: vec![Vec::new(); snapshot.row_count()],
            cell_ranges_by_line: vec![vec![(6, 24)]; snapshot.row_count()],
        };

        let decorations = build_terminal_line_decorations(
            &snapshot,
            &TerminalDecorationSources {
                selected_occurrence_ranges_by_line: &HashMap::new(),
                search_ranges_by_line: &HashMap::new(),
                active_search_ranges_by_line: &HashMap::new(),
                frame_action_links: &[links],
                include_action_links: true,
                include_hyperlinks: false,
            },
        );

        assert!(decorations.iter().all(|line| line.link_ranges.is_empty()));
    }
    #[test]
    fn terminal_line_decorations_skip_plain_viewport() {
        assert!(!terminal_line_decorations_needed(
            false, false, false, false
        ));
    }

    #[test]
    fn terminal_line_decorations_keep_interactive_marks() {
        assert!(terminal_line_decorations_needed(true, false, false, false));
        assert!(terminal_line_decorations_needed(false, true, false, false));
        assert!(terminal_line_decorations_needed(false, false, true, false));
        assert!(terminal_line_decorations_needed(false, false, false, true));
    }

    #[test]
    fn terminal_line_decorations_cache_key_tracks_action_links() {
        let snapshot = TerminalScreen::default().viewport_snapshot(0);
        let search = HashMap::new();
        let active = HashMap::new();
        let (absolute_start_row, absolute_end_row) = terminal_snapshot_absolute_range(&snapshot);
        let mut links = TerminalFrameActionLinks {
            matcher_key: 42,
            absolute_start_row,
            absolute_end_row,
            row_signatures: snapshot.rows().iter().map(|row| row.signature).collect(),
            matches_by_line: Vec::new(),
            cell_ranges_by_line: vec![vec![(1, 4)]],
        };
        let first = terminal_line_decorations_cache_key(
            &snapshot,
            &TerminalDecorationSources {
                selected_occurrence_ranges_by_line: &HashMap::new(),
                search_ranges_by_line: &search,
                active_search_ranges_by_line: &active,
                frame_action_links: std::slice::from_ref(&links),
                include_action_links: true,
                include_hyperlinks: false,
            },
        );
        links.cell_ranges_by_line[0] = vec![(2, 5)];
        let second = terminal_line_decorations_cache_key(
            &snapshot,
            &TerminalDecorationSources {
                selected_occurrence_ranges_by_line: &HashMap::new(),
                search_ranges_by_line: &search,
                active_search_ranges_by_line: &active,
                frame_action_links: std::slice::from_ref(&links),
                include_action_links: true,
                include_hyperlinks: false,
            },
        );

        assert_ne!(first, second);
    }

    #[test]
    fn command_mark_only_snapshot_does_not_need_line_decorations() {
        let mut screen = TerminalScreen::default();
        screen.advance(b"prompt\x1b]133;A\x07");
        let snapshot = screen.viewport_snapshot(0);
        assert!(snapshot.rows().iter().any(|row| row.command_mark.is_some()));
        assert!(!terminal_line_decorations_needed(
            false, false, false, false
        ));
    }
}
