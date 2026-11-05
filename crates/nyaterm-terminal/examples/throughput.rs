#[cfg(feature = "benchmark")]
mod throughput_matrix;

use nyaterm_terminal::TerminalScreen;
use std::time::{Duration, Instant};

fn plain(n: usize) -> Vec<u8> {
    let mut v = Vec::new();
    for i in 0..n {
        v.extend_from_slice(
            format!(
                "line {i} hello world the quick brown fox jumps over the lazy dog 0123456789\r\n"
            )
            .as_bytes(),
        );
    }
    v
}
fn colored(n: usize) -> Vec<u8> {
    let mut v = Vec::new();
    for i in 0..n {
        v.extend_from_slice(format!("\x1b[3{}m\x1b[1mline {i}\x1b[0m \x1b[48;5;{}m hello world \x1b[0m the quick brown fox 日本語\r\n", i % 8, i % 256).as_bytes());
    }
    v
}
fn run(name: &str, data: &[u8], chunk: usize, snap_every: Option<usize>, offset: usize) {
    let mut t = TerminalScreen::new(200, 50);
    // prefill scrollback so offset views are valid
    t.advance(&plain(20000));
    let start = Instant::now();
    let mut snaps = 0;
    for (i, c) in data.chunks(chunk).enumerate() {
        t.advance(c);
        if let Some(n) = snap_every
            && i % n == 0
        {
            let s = t.viewport_snapshot(offset);
            std::hint::black_box(&s);
            snaps += 1;
        }
    }
    let e = start.elapsed();
    println!(
        "{name:<44} {:>8.1} MB/s  {:>7.0} ms  snaps={snaps}",
        data.len() as f64 / 1e6 / e.as_secs_f64(),
        e.as_secs_f64() * 1e3
    );
}
/// Same chunked advance path as `run`, but varying the viewport height. The
/// signature rescan is per `advance` call, so this shows how much it costs after
/// amortization across a realistic output chunk.
fn run_advance_rows(name: &str, data: &[u8], rows: u16) {
    let mut t = TerminalScreen::new(200, rows);
    t.advance(&plain(20000));
    let start = Instant::now();
    for c in data.chunks(16384) {
        t.advance(c);
    }
    let e = start.elapsed();
    println!(
        "{name:<44} {:>8.1} MB/s  {:>7.0} ms",
        data.len() as f64 / 1e6 / e.as_secs_f64(),
        e.as_secs_f64() * 1e3
    );
}

fn run_scroll_snapshot(
    name: &str,
    lines: usize,
    viewport_rows: u16,
    offset_step: usize,
    extra_rows: usize,
) {
    let mut t = TerminalScreen::new(200, viewport_rows);
    t.set_scrollback_limit(lines.saturating_add(viewport_rows as usize));
    t.advance(&plain(lines));
    let scrollback = t.scrollback_len();
    let mut samples = Vec::new();
    let mut offset = 0usize;
    while offset.saturating_add(16) <= scrollback {
        let started = Instant::now();
        let snapshot = t.viewport_snapshot_with_window(offset, extra_rows, extra_rows);
        std::hint::black_box(&snapshot);
        samples.push(started.elapsed());
        offset = offset.saturating_add(offset_step);
    }
    samples.sort_unstable();
    let total: f64 = samples.iter().map(|d| d.as_secs_f64() * 1e3).sum();
    let percentile =
        |p: f64| samples[((samples.len() as f64 - 1.0) * p) as usize].as_secs_f64() * 1e3;
    println!(
        "{name:<44} frames={:<5} avg={:>6.2} ms p50={:>6.2} p95={:>6.2} max={:>6.2} ms (total {:.0} ms)",
        samples.len(),
        total / samples.len() as f64,
        percentile(0.5),
        percentile(0.95),
        samples.last().expect("samples").as_secs_f64() * 1e3,
        total
    );
}

fn snapshot_window(snapshot: &nyaterm_terminal::TerminalSnapshot) -> (usize, usize) {
    let end = snapshot.total_rows.saturating_sub(snapshot.display_offset);
    (end.saturating_sub(snapshot.row_count()), end)
}

/// Simulates upward wheel scrolling over a long scrollback. Compares the old
/// retained-row eviction policy (always drop the smallest absolute row) with the
/// distance-aware policy used by the surface, counting how many scroll frames
/// the retained rows could answer without a worker round trip.
fn run_retained_row_coverage(
    name: &str,
    lines: usize,
    viewport_rows: usize,
    extra_rows: usize,
    offset_step: usize,
    max_rows: usize,
) {
    use std::collections::BTreeMap;

    let mut t = TerminalScreen::new(200, viewport_rows as u16);
    t.set_scrollback_limit(lines.saturating_add(viewport_rows));
    t.advance(&plain(lines));
    let scrollback = t.scrollback_len();
    let total_rows = scrollback.saturating_add(viewport_rows);

    let mut new_rows = BTreeMap::new();
    let mut old_rows = BTreeMap::new();
    let seed = |rows: &mut BTreeMap<usize, ()>, snapshot: &nyaterm_terminal::TerminalSnapshot| {
        let (start, end) = snapshot_window(snapshot);
        for abs_row in start..end {
            rows.insert(abs_row, ());
        }
    };
    let live = t.viewport_snapshot_with_window(0, extra_rows, extra_rows);
    seed(&mut new_rows, &live);
    seed(&mut old_rows, &live);

    let mut new_covered = 0usize;
    let mut old_covered = 0usize;
    let mut total = 0usize;
    let mut offset = 0usize;
    while offset.saturating_add(offset_step) <= scrollback {
        offset += offset_step;
        let snapshot = t.viewport_snapshot_with_window(offset, extra_rows, extra_rows);
        let (window_start, window_end) = snapshot_window(&snapshot);

        seed(&mut new_rows, &snapshot);
        while new_rows.len() > max_rows {
            let Some(first) = new_rows.keys().next().copied() else {
                break;
            };
            let Some(last) = new_rows.keys().next_back().copied() else {
                break;
            };
            let last_row = window_end.saturating_sub(1);
            let first_distance = if first < window_start {
                window_start - first
            } else {
                first.saturating_sub(last_row)
            };
            let last_distance = if last < window_start {
                window_start - last
            } else {
                last.saturating_sub(last_row)
            };
            let drop_key = if first_distance >= last_distance {
                first
            } else {
                last
            };
            new_rows.remove(&drop_key);
        }

        seed(&mut old_rows, &snapshot);
        while old_rows.len() > max_rows {
            let Some(oldest) = old_rows.keys().next().copied() else {
                break;
            };
            old_rows.remove(&oldest);
        }

        let desired_end = total_rows.saturating_sub(offset);
        let desired_start = desired_end.saturating_sub(viewport_rows);
        total += 1;
        if new_rows.range(desired_start..desired_end).count() == viewport_rows {
            new_covered += 1;
        }
        if old_rows.range(desired_start..desired_end).count() == viewport_rows {
            old_covered += 1;
        }
    }
    println!("{name:<44} frames={total:<5} old={old_covered}/{total} new={new_covered}/{total}");
}

fn wide_plain(n: usize, width: usize) -> Vec<u8> {
    let mut v = Vec::new();
    for i in 0..n {
        let mut line = format!("line {i} ");
        while line.len() < width {
            line.push('x');
        }
        v.extend_from_slice(line.as_bytes());
        v.extend_from_slice(b"\r\n");
    }
    v
}

fn average(durations: &[Duration]) -> f64 {
    let total: f64 = durations.iter().map(|d| d.as_secs_f64()).sum();
    total / durations.len().max(1) as f64 * 1e6
}

/// Isolates the cost of the full-screen `line_signature` scan that
/// `stamp_changed_lines` runs whenever the grid scrolls (which is every output
/// line once the screen is full):
/// * scroll path: a bare newline scrolls one row and parses almost nothing, so
///   the measured time is effectively the full scan plus per-advance bookkeeping;
/// * rewrite path: the same terminal rewrites one cell on the last row, which
///   damages a single line and skips the full scan. The delta is the scan.
fn run_scroll_scan_cost(name: &str, cols: u16, rows: u16, width: usize, iterations: usize) {
    let mut t = TerminalScreen::new(cols, rows);
    t.set_scrollback_limit(rows as usize * 8);
    t.advance(&wide_plain(rows as usize * 4, width));

    let mut scroll = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let started = Instant::now();
        t.advance(b"\r\n");
        scroll.push(started.elapsed());
    }

    t.advance(b"\rX");
    let mut rewrite = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let started = Instant::now();
        t.advance(b"\rX");
        rewrite.push(started.elapsed());
    }

    let scroll_avg = average(&scroll);
    let rewrite_avg = average(&rewrite);
    println!(
        "{name:<44} scroll={:>7.2} us  rewrite={:>7.2} us  scan_delta={:>7.2} us",
        scroll_avg,
        rewrite_avg,
        (scroll_avg - rewrite_avg).max(0.0),
    );
}

/// Per-line cost of ordinary output on a full screen, i.e. what a realistic
/// advance pays including one full signature scan per reported line.
fn run_single_line_cost(name: &str, cols: u16, rows: u16, iterations: usize) {
    let mut t = TerminalScreen::new(cols, rows);
    t.set_scrollback_limit(rows as usize * 8);
    t.advance(&plain(rows as usize * 4));
    let lines: Vec<Vec<u8>> = (0..iterations)
        .map(|i| {
            format!(
                "line {i} hello world the quick brown fox jumps over the lazy dog 0123456789\r\n"
            )
            .into_bytes()
        })
        .collect();
    let mut samples = Vec::with_capacity(iterations);
    for line in &lines {
        let started = Instant::now();
        t.advance(line);
        samples.push(started.elapsed());
    }
    println!("{name:<44} us/line={:>7.2}", average(&samples));
}

/// One `advance` call that scrolls several rows, matching a chunked output
/// burst. Isolates whether cost scales with the number of scrolled rows rather
/// than only with the viewport height.
fn run_scroll_burst_cost(name: &str, cols: u16, rows: u16, newlines: usize, iterations: usize) {
    let mut t = TerminalScreen::new(cols, rows);
    t.set_scrollback_limit(20_000);
    t.advance(&wide_plain(rows as usize * 4, 72));
    let mut burst = Vec::new();
    for _ in 0..newlines {
        burst.extend_from_slice(b"\r\n");
    }
    let mut samples = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let started = Instant::now();
        t.advance(&burst);
        samples.push(started.elapsed());
    }
    println!("{name:<44} us/call={:>7.2}", average(&samples));
}

fn main() {
    #[cfg(feature = "benchmark")]
    if std::env::args().any(|arg| arg == "--matrix") {
        throughput_matrix::run();
        return;
    }
    let p = plain(300_000);
    let c = colored(300_000);
    println!(
        "plain {:.1} MB, colored {:.1} MB",
        p.len() as f64 / 1e6,
        c.len() as f64 / 1e6
    );
    run("plain advance only (16K chunks)", &p, 16384, None, 0);
    run("colored advance only (16K chunks)", &c, 16384, None, 0);
    run(
        "plain + snapshot each 4K chunk (bottom)",
        &p,
        4096,
        Some(1),
        0,
    );
    run(
        "colored + snapshot each 4K chunk (bottom)",
        &c,
        4096,
        Some(1),
        0,
    );
    run(
        "plain + snapshot each 4K chunk (scrolled 5000)",
        &p,
        4096,
        Some(1),
        5000,
    );
    run(
        "colored + snapshot each 4K chunk (scrolled)",
        &c,
        4096,
        Some(1),
        5000,
    );
    println!();
    // Fast scrollback browsing: every wheel frame asks the worker for a window
    // around a different display offset. Measures the worker-side snapshot cost
    // the UI waits on when the retained row cache cannot answer the frame.
    run_scroll_snapshot(
        "scroll 12k lines, 50 rows, step 8, extra 192",
        12_000,
        50,
        8,
        192,
    );
    run_scroll_snapshot(
        "scroll 12k lines, 50 rows, step 8, extra 256",
        12_000,
        50,
        8,
        256,
    );
    println!();
    // Retained-row cache coverage while scrolling up. A miss is a frame the
    // surface cannot synthesize locally, so it freezes and then jumps when the
    // worker snapshot arrives (the reported flicker).
    run_retained_row_coverage(
        "retained rows, 12k lines, 50 rows, step 8, limit 4096",
        12_000,
        50,
        192,
        8,
        4096,
    );
    run_retained_row_coverage(
        "retained rows, 12k lines, 50 rows, step 1, limit 4096",
        12_000,
        50,
        192,
        1,
        4096,
    );
    println!();
    // Cost of `stamp_changed_lines`'s full line-signature scan on every scroll.
    for rows in [10, 50, 200] {
        run_advance_rows(
            &format!("plain advance only 16K chunks, rows={rows}"),
            &p,
            rows,
        );
    }
    for rows in [10, 25, 50, 100, 200] {
        run_scroll_scan_cost(
            &format!("signature scan, cols=200 rows={rows}"),
            200,
            rows,
            72,
            3000,
        );
    }
    for cols in [80, 200, 400] {
        run_scroll_scan_cost(
            &format!("signature scan, rows=50 cols={cols}"),
            cols,
            50,
            (cols - 2) as usize,
            3000,
        );
    }
    run_single_line_cost("ordinary output, cols=200 rows=10", 200, 10, 3000);
    run_single_line_cost("ordinary output, cols=200 rows=50", 200, 50, 3000);
    run_single_line_cost("ordinary output, cols=200 rows=200", 200, 200, 3000);
    for rows in [10, 50, 200] {
        run_scroll_burst_cost(
            &format!("scroll burst, cols=200 rows={rows} newlines=1"),
            200,
            rows,
            1,
            2000,
        );
        run_scroll_burst_cost(
            &format!("scroll burst, cols=200 rows={rows} newlines=205"),
            200,
            rows,
            205,
            2000,
        );
    }
}
