use std::sync::Arc;

use gpui::{
    Bounds, IntoElement, Pixels, SharedString, div, fill, point, prelude::*, px, rgba, size,
};

/// Zed's editor scrollbar reserves a substantial, stable hit target.
pub const TERMINAL_SCROLLBAR_COLUMN_WIDTH: f32 = 15.0;
pub const TERMINAL_SCROLLBAR_TRACK_PADDING_Y: f32 = 0.0;
pub const TERMINAL_SCROLLBAR_TRACK_PADDING_RIGHT: f32 = 0.0;
pub const TERMINAL_SCROLLBAR_THUMB_WIDTH: f32 = 15.0;
pub const TERMINAL_SCROLLBAR_THUMB_ACTIVE_WIDTH: f32 = 15.0;
pub const TERMINAL_SCROLLBAR_MIN_THUMB_HEIGHT: f32 = 25.0;
const TERMINAL_OVERVIEW_RULER_COLUMNS: f32 = 3.0;
const TERMINAL_OVERVIEW_MARKER_MIN_HEIGHT_PX: usize = 5;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerminalScrollbarMetrics {
    pub track_height: f32,
    pub thumb_height: f32,
    pub thumb_top: f32,
    pub thumb_travel: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerminalScrollbarInput {
    pub viewport_rows: usize,
    pub scrollback_rows: usize,
    pub scroll_offset: usize,
    pub track_height: f32,
    pub min_thumb_height: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TerminalOverviewMarkerKind {
    SelectedOccurrence,
    SearchMatch,
    ActiveSearchMatch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TerminalOverviewMarker {
    pub absolute_line: usize,
    pub kind: TerminalOverviewMarkerKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TerminalOverviewMarkerBucket {
    pub y_px: usize,
    pub kind: TerminalOverviewMarkerKind,
}

pub fn terminal_scrollbar_metrics(input: TerminalScrollbarInput) -> TerminalScrollbarMetrics {
    let track_height = finite_non_negative(input.track_height);
    let min_thumb_height = finite_non_negative(input.min_thumb_height);
    let viewport_rows = input.viewport_rows.max(1);
    let scrollback_rows = input.scrollback_rows;
    let content_rows = viewport_rows.saturating_add(scrollback_rows).max(1);
    let max_offset = scrollback_rows;
    let proportional = if track_height <= 0.0 {
        0.0
    } else {
        track_height * viewport_rows as f32 / content_rows as f32
    };
    let thumb_height = proportional.max(min_thumb_height).min(track_height);
    let thumb_travel = (track_height - thumb_height).max(0.0);
    let position_ratio = if max_offset == 0 {
        0.0
    } else {
        1.0 - (input.scroll_offset.min(max_offset) as f32 / max_offset as f32)
    };
    let thumb_top = thumb_travel * position_ratio.clamp(0.0, 1.0);
    TerminalScrollbarMetrics {
        track_height,
        thumb_height,
        thumb_top,
        thumb_travel,
    }
}

pub fn terminal_scroll_offset_from_pointer(
    pointer_y: f32,
    track_top: f32,
    metrics: TerminalScrollbarMetrics,
    grab_offset_y: f32,
    max_offset: usize,
) -> usize {
    if max_offset == 0 || metrics.thumb_travel <= 0.0 {
        return 0;
    }
    let local_thumb_top = (finite_or(pointer_y, track_top)
        - finite_or(track_top, 0.0)
        - finite_non_negative(grab_offset_y))
    .clamp(0.0, metrics.thumb_travel);
    let ratio = (local_thumb_top / metrics.thumb_travel).clamp(0.0, 1.0);
    ((1.0 - ratio) * max_offset as f32).round() as usize
}

pub fn terminal_scrollbar_grab_offset_for_pointer(
    pointer_y: f32,
    track_top: f32,
    metrics: TerminalScrollbarMetrics,
) -> f32 {
    let local_y = finite_or(pointer_y, track_top) - finite_or(track_top, 0.0);
    if local_y >= metrics.thumb_top && local_y <= metrics.thumb_top + metrics.thumb_height {
        (local_y - metrics.thumb_top).clamp(0.0, metrics.thumb_height)
    } else {
        metrics.thumb_height * 0.5
    }
}

pub fn terminal_scrollbar_thumb_color(palette: nyaterm_ui::ThemePalette, _active: bool) -> u32 {
    palette.scroll_thumb
}

pub fn terminal_scrollbar_thumb_element(
    id: SharedString,
    metrics: TerminalScrollbarMetrics,
    palette: nyaterm_ui::ThemePalette,
    active: bool,
    dragging: bool,
) -> impl IntoElement {
    let color = terminal_scrollbar_thumb_color(palette, active);
    let width = if dragging {
        TERMINAL_SCROLLBAR_THUMB_ACTIVE_WIDTH
    } else {
        TERMINAL_SCROLLBAR_THUMB_WIDTH
    };
    div()
        .id(id)
        .absolute()
        .right(px(0.0))
        .top(px(metrics.thumb_top))
        .w(px(width))
        .h(px(metrics.thumb_height))
        .bg(rgba(
            (color << 8) | if dragging || active { 0xa8 } else { 0x68 },
        ))
        .hover(move |this| {
            this.w(px(TERMINAL_SCROLLBAR_THUMB_ACTIVE_WIDTH))
                .bg(rgba((color << 8) | 0xc0))
        })
}

pub fn terminal_scrollbar_track_color(palette: nyaterm_ui::ThemePalette) -> u32 {
    (palette.border << 8) | 0x24
}

pub fn terminal_overview_marker_buckets(
    markers: &[TerminalOverviewMarker],
    total_rows: usize,
    track_height_px: usize,
) -> Vec<TerminalOverviewMarkerBucket> {
    if markers.is_empty() || total_rows == 0 || track_height_px == 0 {
        return Vec::new();
    }
    let max_line = total_rows.saturating_sub(1).max(1);
    let max_y = track_height_px.saturating_sub(TERMINAL_OVERVIEW_MARKER_MIN_HEIGHT_PX);
    let mut bucket_kinds = vec![None; max_y.saturating_add(1)];
    for marker in markers {
        let line = marker.absolute_line.min(max_line);
        let ratio = line as f32 / max_line as f32;
        let y_px = (ratio * max_y as f32).round() as usize;
        if let Some(slot) = bucket_kinds.get_mut(y_px)
            && slot.is_none_or(|kind| marker.kind > kind)
        {
            *slot = Some(marker.kind);
        }
    }
    bucket_kinds
        .into_iter()
        .enumerate()
        .filter_map(|(y_px, kind)| kind.map(|kind| TerminalOverviewMarkerBucket { y_px, kind }))
        .collect()
}

pub fn terminal_overview_marker_canvas(
    markers: Arc<[TerminalOverviewMarker]>,
    total_rows: usize,
    cached_track_height_px: usize,
    cached_buckets: Arc<[TerminalOverviewMarkerBucket]>,
    palette: nyaterm_ui::ThemePalette,
) -> impl IntoElement {
    gpui::canvas(
        |_bounds, _window, _cx| {},
        move |bounds, _state, window, _cx| {
            let track_height_px = f32::from(bounds.size.height).max(0.0).round() as usize;
            let fallback_buckets;
            let buckets = if track_height_px == cached_track_height_px {
                cached_buckets.as_ref()
            } else {
                fallback_buckets =
                    terminal_overview_marker_buckets(&markers, total_rows, track_height_px);
                fallback_buckets.as_slice()
            };
            if buckets.is_empty() {
                return;
            }
            let border_width = 1.0_f32.min(f32::from(bounds.size.width).max(0.0));
            let column_width = ((f32::from(bounds.size.width) - border_width).max(0.0)
                / TERMINAL_OVERVIEW_RULER_COLUMNS)
                .floor();
            // Selected text/search markers occupy Zed's middle overview-ruler column.
            let left = f32::from(bounds.left()) + border_width + column_width;
            let top = f32::from(bounds.top());
            for bucket in buckets {
                let color = terminal_overview_marker_color(bucket.kind, palette);
                let height = terminal_overview_marker_height_px(bucket.kind)
                    .min(track_height_px.saturating_sub(bucket.y_px));
                if height == 0 || column_width <= 0.0 {
                    continue;
                }
                window.paint_quad(fill(
                    Bounds::new(
                        point(px(left), px(top + bucket.y_px as f32)),
                        size(px(column_width), px(height as f32)),
                    ),
                    rgba(color),
                ));
            }
        },
    )
    .absolute()
    .inset_0()
    .size_full()
}

fn terminal_overview_marker_height_px(kind: TerminalOverviewMarkerKind) -> usize {
    match kind {
        TerminalOverviewMarkerKind::SelectedOccurrence
        | TerminalOverviewMarkerKind::SearchMatch
        | TerminalOverviewMarkerKind::ActiveSearchMatch => TERMINAL_OVERVIEW_MARKER_MIN_HEIGHT_PX,
    }
}

fn finite_non_negative(value: f32) -> f32 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}

fn finite_or(value: f32, fallback: f32) -> f32 {
    if value.is_finite() { value } else { fallback }
}

pub fn track_height(bounds: Bounds<Pixels>) -> f32 {
    finite_non_negative(f32::from(bounds.size.height))
}

fn terminal_overview_marker_color(
    kind: TerminalOverviewMarkerKind,
    palette: nyaterm_ui::ThemePalette,
) -> u32 {
    match kind {
        TerminalOverviewMarkerKind::SelectedOccurrence => (palette.text_muted << 8) | 0xc0,
        TerminalOverviewMarkerKind::SearchMatch => (palette.accent << 8) | 0xb0,
        TerminalOverviewMarkerKind::ActiveSearchMatch => (palette.warning << 8) | 0xd8,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        TerminalOverviewMarker, TerminalOverviewMarkerKind, TerminalScrollbarInput,
        terminal_overview_marker_buckets, terminal_scroll_offset_from_pointer,
        terminal_scrollbar_grab_offset_for_pointer, terminal_scrollbar_metrics,
    };

    #[test]
    fn terminal_scrollbar_metrics_handle_no_scrollback() {
        let metrics = terminal_scrollbar_metrics(TerminalScrollbarInput {
            viewport_rows: 24,
            scrollback_rows: 0,
            scroll_offset: 0,
            track_height: 120.0,
            min_thumb_height: 18.0,
        });
        assert_eq!(metrics.thumb_height, 120.0);
        assert_eq!(metrics.thumb_top, 0.0);
        assert_eq!(metrics.thumb_travel, 0.0);
    }

    #[test]
    fn terminal_scrollbar_metrics_place_live_offset_at_bottom_and_max_at_top() {
        let live = terminal_scrollbar_metrics(TerminalScrollbarInput {
            viewport_rows: 10,
            scrollback_rows: 90,
            scroll_offset: 0,
            track_height: 100.0,
            min_thumb_height: 18.0,
        });
        let history = terminal_scrollbar_metrics(TerminalScrollbarInput {
            scroll_offset: 90,
            ..TerminalScrollbarInput {
                viewport_rows: 10,
                scrollback_rows: 90,
                scroll_offset: 0,
                track_height: 100.0,
                min_thumb_height: 18.0,
            }
        });
        assert_eq!(live.thumb_top, live.thumb_travel);
        assert_eq!(history.thumb_top, 0.0);
    }

    #[test]
    fn terminal_scrollbar_metrics_enforce_min_thumb_and_short_track() {
        let metrics = terminal_scrollbar_metrics(TerminalScrollbarInput {
            viewport_rows: 10,
            scrollback_rows: 10_000,
            scroll_offset: 0,
            track_height: 100.0,
            min_thumb_height: 18.0,
        });
        assert_eq!(metrics.thumb_height, 18.0);
        let short = terminal_scrollbar_metrics(TerminalScrollbarInput {
            track_height: 8.0,
            ..TerminalScrollbarInput {
                viewport_rows: 10,
                scrollback_rows: 10_000,
                scroll_offset: 0,
                track_height: 100.0,
                min_thumb_height: 18.0,
            }
        });
        assert_eq!(short.thumb_height, 8.0);
        assert_eq!(short.thumb_travel, 0.0);
    }

    #[test]
    fn terminal_scrollbar_pointer_mapping_clamps_and_respects_track_origin_and_grab() {
        let metrics = terminal_scrollbar_metrics(TerminalScrollbarInput {
            viewport_rows: 10,
            scrollback_rows: 90,
            scroll_offset: 0,
            track_height: 100.0,
            min_thumb_height: 20.0,
        });
        assert_eq!(
            terminal_scroll_offset_from_pointer(40.0, 50.0, metrics, 10.0, 90),
            90
        );
        assert_eq!(
            terminal_scroll_offset_from_pointer(180.0, 50.0, metrics, 10.0, 90),
            0
        );
        assert_eq!(
            terminal_scroll_offset_from_pointer(100.0, 50.0, metrics, 10.0, 90),
            45
        );
        assert_eq!(
            terminal_scroll_offset_from_pointer(50.0, 50.0, metrics, 0.0, 90),
            90
        );
        assert_eq!(
            terminal_scroll_offset_from_pointer(150.0, 50.0, metrics, metrics.thumb_height, 90),
            0
        );
    }

    #[test]
    fn terminal_scrollbar_thumb_grab_and_track_click_use_distinct_offsets() {
        let metrics = terminal_scrollbar_metrics(TerminalScrollbarInput {
            viewport_rows: 10,
            scrollback_rows: 90,
            scroll_offset: 45,
            track_height: 100.0,
            min_thumb_height: 20.0,
        });
        let track_top = 30.0;
        let thumb_pointer = track_top + metrics.thumb_top + 3.0;
        let thumb_grab =
            terminal_scrollbar_grab_offset_for_pointer(thumb_pointer, track_top, metrics);
        let track_click = terminal_scrollbar_grab_offset_for_pointer(track_top, track_top, metrics);

        assert_eq!(thumb_grab, 3.0);
        assert_eq!(track_click, metrics.thumb_height * 0.5);
        assert_eq!(
            terminal_scroll_offset_from_pointer(thumb_pointer, track_top, metrics, thumb_grab, 90),
            45
        );
        assert_eq!(
            terminal_scroll_offset_from_pointer(track_top, track_top, metrics, track_click, 90),
            90
        );
    }

    #[test]
    fn terminal_scrollbar_metrics_handle_one_row_and_huge_scrollback() {
        let metrics = terminal_scrollbar_metrics(TerminalScrollbarInput {
            viewport_rows: 1,
            scrollback_rows: 1_000_000,
            scroll_offset: 500_000,
            track_height: 240.0,
            min_thumb_height: 18.0,
        });

        assert_eq!(metrics.thumb_height, 18.0);
        assert!(metrics.thumb_top.is_finite());
        assert!(metrics.thumb_top > 0.0 && metrics.thumb_top < metrics.thumb_travel);
    }

    #[test]
    fn terminal_scrollbar_metrics_defend_against_invalid_values() {
        let metrics = terminal_scrollbar_metrics(TerminalScrollbarInput {
            viewport_rows: 1,
            scrollback_rows: usize::MAX,
            scroll_offset: usize::MAX,
            track_height: f32::NAN,
            min_thumb_height: f32::NAN,
        });
        assert_eq!(metrics.track_height, 0.0);
        assert_eq!(metrics.thumb_height, 0.0);
        assert_eq!(metrics.thumb_travel, 0.0);
        assert_eq!(
            terminal_scroll_offset_from_pointer(f32::NAN, 0.0, metrics, 0.0, 10),
            0
        );
    }

    #[test]
    fn terminal_scrollbar_marker_buckets_merge_and_prioritize() {
        let buckets = terminal_overview_marker_buckets(
            &[
                TerminalOverviewMarker {
                    absolute_line: 0,
                    kind: TerminalOverviewMarkerKind::SelectedOccurrence,
                },
                TerminalOverviewMarker {
                    absolute_line: 50,
                    kind: TerminalOverviewMarkerKind::SearchMatch,
                },
                TerminalOverviewMarker {
                    absolute_line: 50,
                    kind: TerminalOverviewMarkerKind::ActiveSearchMatch,
                },
                TerminalOverviewMarker {
                    absolute_line: 99,
                    kind: TerminalOverviewMarkerKind::SelectedOccurrence,
                },
            ],
            100,
            10,
        );
        assert_eq!(buckets.len(), 3);
        assert_eq!(buckets[0].y_px, 0);
        assert_eq!(
            buckets[1].kind,
            TerminalOverviewMarkerKind::ActiveSearchMatch
        );
        assert_eq!(buckets[2].y_px, 5);
    }

    #[test]
    fn terminal_scrollbar_marker_buckets_handle_empty_and_single_line_buffers() {
        let marker = TerminalOverviewMarker {
            absolute_line: 0,
            kind: TerminalOverviewMarkerKind::SelectedOccurrence,
        };

        assert!(terminal_overview_marker_buckets(&[], 0, 120).is_empty());
        assert!(terminal_overview_marker_buckets(&[marker], 0, 120).is_empty());
        assert_eq!(
            terminal_overview_marker_buckets(&[marker], 1, 120),
            vec![super::TerminalOverviewMarkerBucket {
                y_px: 0,
                kind: TerminalOverviewMarkerKind::SelectedOccurrence,
            }]
        );
    }

    #[test]
    fn terminal_scrollbar_marker_buckets_bound_dense_match_work_to_track_pixels() {
        let markers = (0..2000)
            .map(|absolute_line| TerminalOverviewMarker {
                absolute_line,
                kind: TerminalOverviewMarkerKind::SelectedOccurrence,
            })
            .collect::<Vec<_>>();

        let buckets = terminal_overview_marker_buckets(&markers, 2000, 120);

        assert!(buckets.len() <= 119);
        assert_eq!(buckets.first().map(|bucket| bucket.y_px), Some(0));
        assert_eq!(buckets.last().map(|bucket| bucket.y_px), Some(115));
        assert!(buckets.windows(2).all(|pair| pair[0].y_px < pair[1].y_px));
    }
}
