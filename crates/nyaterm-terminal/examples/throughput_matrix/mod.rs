//! Reproducible parser benchmark: warmup, five samples and CSV output.
use nyaterm_terminal::TerminalScreen;
use std::time::Instant;

#[cfg(windows)]
fn cpu_seconds() -> f64 {
    #[repr(C)]
    #[derive(Default)]
    struct FileTime {
        low: u32,
        high: u32,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetProcessTimes(
            process: isize,
            creation: *mut FileTime,
            exit: *mut FileTime,
            kernel: *mut FileTime,
            user: *mut FileTime,
        ) -> i32;
    }
    let mut creation = FileTime::default();
    let mut exit = FileTime::default();
    let mut kernel = FileTime::default();
    let mut user = FileTime::default();
    // Windows' current-process pseudo handle needs no ownership or closing.
    let result = unsafe { GetProcessTimes(-1, &mut creation, &mut exit, &mut kernel, &mut user) };
    assert_ne!(result, 0);
    let ticks = |time: FileTime| (u64::from(time.high) << 32) | u64::from(time.low);
    (ticks(kernel) + ticks(user)) as f64 / 10_000_000.0
}

#[cfg(not(windows))]
fn cpu_seconds() -> f64 {
    f64::NAN
}

const WORKLOADS: [(&str, &[u8]); 4] = [
    ("plain", b"the quick brown fox jumps over the lazy dog 0123456789\r\n"),
    ("sgr", b"\x1b[31mthe quick brown fox\x1b[0m jumps over the lazy dog 0123456789\r\n"),
    ("wrap", "the quick 日本語 brown fox jumps over the lazy dog 0123456789 repeated twice for wrapping\r\n".as_bytes()),
    ("rewrite", b"progress 0123456789\r\x1b[2Kupdated 9876543210\r\n"),
];

struct Sample {
    mbps: f64,
    cpu_ms: f64,
    latency_us: [f64; 3],
    scanned_rows: u64,
    batches: u64,
    bytes: usize,
}

fn measure(data: &[u8], line: &[u8], rows: u16, history: usize, chunk: usize) -> Sample {
    let mut terminal = TerminalScreen::new(80, rows);
    terminal.set_scrollback_limit(history);
    terminal.advance(&line.repeat(300));
    let before = terminal.benchmark_counts();
    let mut latency = Vec::with_capacity(data.len().div_ceil(chunk));
    let cpu = cpu_seconds();
    let started = Instant::now();
    let mut bytes = 0;
    loop {
        for part in data.chunks(chunk) {
            let batch = Instant::now();
            terminal.advance(part);
            latency.push(batch.elapsed().as_secs_f64() * 1e6);
        }
        bytes += data.len();
        // Avoid measuring a handful of timer ticks for fast cases.
        if started.elapsed().as_millis() >= 250 {
            break;
        }
    }
    let elapsed = started.elapsed().as_secs_f64();
    let cpu_ms = (cpu_seconds() - cpu) * 1000.;
    let after = terminal.benchmark_counts();
    std::hint::black_box(terminal.snapshot());
    latency.sort_by(f64::total_cmp);
    let percentile = |p: f64| latency[((latency.len() - 1) as f64 * p) as usize];
    Sample {
        mbps: bytes as f64 / 1e6 / elapsed,
        cpu_ms,
        latency_us: [percentile(0.5), percentile(0.95), percentile(0.99)],
        scanned_rows: after.1 - before.1,
        batches: after.0 - before.0,
        bytes,
    }
}

pub fn run() {
    println!("kind,rows,history,chunk,mbps,cpu_ms,p50_us,p95_us,p99_us,scanned_rows,batches,bytes");
    let quick = std::env::args().any(|arg| arg == "--quick");
    let list = std::env::args().any(|arg| arg == "--list");
    let selected = std::env::args().find_map(|arg| arg.strip_prefix("--case=").map(str::to_owned));
    for (kind, line) in WORKLOADS {
        if quick && !["plain", "sgr"].contains(&kind) {
            continue;
        }
        for rows in [2, 24, 200] {
            if quick && rows == 200 {
                continue;
            }
            for history in [0, 2, 24, 10_000] {
                for chunk in [line.len(), 16_384, 65_536] {
                    if quick && chunk != 16_384 {
                        continue;
                    }
                    let key = format!("{kind},{rows},{history},{chunk}");
                    if selected.as_ref().is_some_and(|selected| *selected != key) {
                        continue;
                    }
                    if list {
                        println!("{key},0,0,0,0,0,0,0,0");
                        continue;
                    }
                    let count = if chunk == line.len() { 2000 } else { 32768 };
                    let data = line.repeat(count);
                    std::hint::black_box(measure(&data, line, rows, history, chunk));
                    let mut samples: Vec<_> = (0..5)
                        .map(|_| measure(&data, line, rows, history, chunk))
                        .collect();
                    samples.sort_by(|left, right| left.mbps.total_cmp(&right.mbps));
                    let sample = &samples[2];
                    let [p50, p95, p99] = sample.latency_us;
                    println!(
                        "{key},{:.3},{:.3},{p50:.3},{p95:.3},{p99:.3},{},{},{}",
                        sample.mbps,
                        sample.cpu_ms,
                        sample.scanned_rows,
                        sample.batches,
                        sample.bytes,
                    );
                }
            }
        }
    }
}
