use crate::models::TerminalViewState;
use nyaterm_terminal::action_links::TerminalFrameActionLinks;
use nyaterm_terminal_gpui::decorations::terminal_action_links_overlap_snapshot;

pub(in crate::features) fn terminal_action_links_for_paint_snapshot(
    view: Option<&TerminalViewState>,
    display_offset: usize,
    snapshot: &nyaterm_terminal::TerminalSnapshot,
    matcher_key: u64,
) -> Vec<TerminalFrameActionLinks> {
    let mut out = Vec::new();
    let Some(view) = view else {
        return out;
    };
    let mut push_if_matching = |links: &TerminalFrameActionLinks| {
        if links.matcher_key != matcher_key
            || !terminal_action_links_overlap_snapshot(snapshot, links)
            || !(0..snapshot.row_count()).any(|line_index| {
                links
                    .source_index_for_snapshot_row(snapshot, line_index)
                    .is_some()
            })
        {
            return;
        }
        let duplicate = out.iter().any(|existing: &TerminalFrameActionLinks| {
            existing.matcher_key == links.matcher_key
                && existing.absolute_start_row == links.absolute_start_row
                && existing.absolute_end_row == links.absolute_end_row
                && existing.row_signatures == links.row_signatures
        });
        if !duplicate {
            out.push(links.clone());
        }
    };
    if let Some(links) = view.frame_action_links.as_ref() {
        push_if_matching(links);
    }
    if display_offset > 0
        && let Some(links) = view.scrollback_action_links.get(&display_offset)
    {
        push_if_matching(links);
    }
    let mut fallback_links = view.scrollback_action_links.values().collect::<Vec<_>>();
    fallback_links.sort_unstable_by_key(|links| {
        (
            links.absolute_start_row,
            links.absolute_end_row,
            links.matcher_key,
        )
    });
    for links in fallback_links {
        push_if_matching(links);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::terminal_action_links_for_paint_snapshot;
    use crate::models::TerminalViewState;
    use nyaterm_terminal::action_links::TerminalFrameActionLinks;
    use nyaterm_terminal_gpui::decorations::{
        TerminalDecorationSources, build_terminal_line_decorations,
        terminal_snapshot_absolute_range,
    };
    use std::collections::HashMap;
    #[test]
    fn bottom_action_link_sources_skip_stale_frame_and_keep_valid_fallback() {
        let view = TerminalViewState::from_output("visit http://example.com\n".to_string());
        let snapshot = view.screen.viewport_snapshot(0);
        let (absolute_start_row, absolute_end_row) = terminal_snapshot_absolute_range(&snapshot);
        let matcher_key = 7;
        let stale_frame_links = TerminalFrameActionLinks {
            matcher_key,
            absolute_start_row,
            absolute_end_row,
            row_signatures: vec![0; snapshot.row_count()],
            matches_by_line: vec![Vec::new(); snapshot.row_count()],
            cell_ranges_by_line: vec![vec![(0, 5)]; snapshot.row_count()],
        };
        let mut valid_fallback_links = TerminalFrameActionLinks {
            matcher_key,
            absolute_start_row,
            absolute_end_row,
            row_signatures: snapshot.rows().iter().map(|row| row.signature).collect(),
            matches_by_line: vec![Vec::new(); snapshot.row_count()],
            cell_ranges_by_line: vec![Vec::new(); snapshot.row_count()],
        };
        valid_fallback_links.cell_ranges_by_line[0].push((6, 24));
        let mut view_with_links = view;
        view_with_links.frame_action_links = Some(stale_frame_links);
        view_with_links
            .scrollback_action_links
            .insert(1, valid_fallback_links);

        let sources = terminal_action_links_for_paint_snapshot(
            Some(&view_with_links),
            0,
            &snapshot,
            matcher_key,
        );
        let decorations = build_terminal_line_decorations(
            &snapshot,
            &TerminalDecorationSources {
                selected_occurrence_ranges_by_line: &HashMap::new(),
                search_ranges_by_line: &HashMap::new(),
                active_search_ranges_by_line: &HashMap::new(),
                frame_action_links: &sources,
                include_action_links: true,
                include_hyperlinks: false,
            },
        );

        assert_eq!(decorations[0].link_ranges, vec![(6, 24)]);
    }
}
