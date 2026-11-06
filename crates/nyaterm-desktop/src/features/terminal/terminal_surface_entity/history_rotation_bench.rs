//! Manual CPU benchmark; excludes native window/GPU presentation.
use std::sync::Arc;
use std::time::Instant;

use nyaterm_terminal::{TerminalScreen, TerminalSnapshot};

use crate::features::terminal::terminal_runtime::TerminalScrollVisualState;
use crate::features::terminal::terminal_surface_entity::{
    TerminalSurface, TerminalSurfaceFrameSnapshot,
};

fn scroll(snapshot: &TerminalSnapshot, offset: usize) -> TerminalScrollVisualState {
    TerminalScrollVisualState {
        history: snapshot.history,
        session_id: "bench".into(),
        scroll_offset: offset,
        scroll_residual_lines: 0.0,
        display_offset: offset,
        scrollback_len: snapshot.scrollback_len,
        viewport_rows: snapshot.viewport_rows,
        has_new_while_scrolled: offset > 0,
        performance_overlay: None,
        skipped_output_chars: 0,
    }
}

fn frame(snapshot: Arc<TerminalSnapshot>, offset: usize) -> TerminalSurfaceFrameSnapshot {
    TerminalSurfaceFrameSnapshot {
        scroll: scroll(&snapshot, offset),
        snapshot,
        has_action_link_decorations: false,
        show_cursor: false,
        cursor_style: "block".into(),
    }
}

#[test]
#[ignore = "manual CPU history-rotation A/B benchmark"]
fn full_history_surface_update_benchmark() {
    const FRAMES: usize = 2000;
    let line = b"fixture 0123456789 abcdefghijklmnopqrstuvwxyz\r\n";
    for limit in [5000, 100000] {
        for scrolled in [false, true] {
            for repeat in 0..5 {
                let mut screen = TerminalScreen::new(80, 24);
                screen.set_scrollback_limit(limit);
                let block = line.repeat(1000);
                for _ in 0..limit / 1000 + 1 {
                    screen.advance(&block);
                }
                let initial = if scrolled { 256 } else { 0 };
                let snapshot = Arc::new(screen.viewport_snapshot_with_window(initial, 48, 48));
                let mut surface = TerminalSurface::new("bench");
                surface.apply_frame_snapshot(frame(snapshot, initial));
                let mut samples = Vec::with_capacity(FRAMES);
                let mut recoveries = 0;
                let started = Instant::now();
                for i in 1..=FRAMES {
                    screen.advance(line);
                    let update_started = Instant::now();
                    if scrolled {
                        // Match scroll notify's cheap history read; fetch rows only
                        // when the current surface cannot paint the anchored target.
                        let mut state =
                            scroll(surface.snapshot.as_ref().expect("paint"), initial + i);
                        state.history = screen.history_state();
                        surface.update_scroll_chrome_without_snapshot(&state);
                        if !surface.has_snapshot_covering_display_offset(initial + i, 24, limit) {
                            recoveries += 1;
                            let snapshot =
                                Arc::new(screen.viewport_snapshot_with_window(initial + i, 48, 48));
                            surface.apply_frame_snapshot(frame(snapshot, initial + i));
                        }
                    } else {
                        let snapshot = Arc::new(screen.viewport_snapshot_with_window(0, 48, 48));
                        surface.apply_frame_snapshot(frame(snapshot, 0));
                    }
                    samples.push(update_started.elapsed().as_nanos());
                    std::hint::black_box(&surface);
                }
                let elapsed = started.elapsed();
                samples.sort_unstable();
                println!(
                    "history_rotation limit={limit} scrolled={scrolled} repeat={repeat} bytes_per_sec={} update_p95_ns={} recoveries={recoveries}",
                    (line.len() * FRAMES) as f64 / elapsed.as_secs_f64(),
                    samples[FRAMES * 95 / 100]
                );
            }
        }
    }
}
