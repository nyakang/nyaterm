use nyaterm_terminal::TerminalScreen;
use std::time::Instant;

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
fn main() {
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
}
