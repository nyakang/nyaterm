//! ANSI transcript microbenchmark, including allocation counts.
//! Run: cargo run --release -p nyaterm-terminal --example recording_ansi
//! The baseline reproduces PR #1151's String + mem::take CSI/SGR hot path.
//! It intentionally handles only this benchmark's ASCII/SGR workload.

use nyaterm_terminal::recording_sanitizer::{RecordingEvent, RecordingSanitizer};
use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

struct CountingAllocator;
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

// SAFETY: All allocations and their matching deallocations are delegated to
// System with the original layouts; only an atomic counter is added.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

#[derive(Default)]
struct PreviousSgrPath {
    state: u8,
    params: String,
}

impl PreviousSgrPath {
    fn feed(&mut self, text: &str) -> String {
        let mut output = String::with_capacity(text.len());
        for ch in text.chars() {
            match self.state {
                0 if ch == '\x1b' => self.state = 1,
                0 => output.push(ch),
                1 if ch == '[' => {
                    self.params.clear();
                    self.state = 2;
                }
                1 => self.state = 0,
                _ if ('\x30'..='\x3f').contains(&ch) => self.params.push(ch),
                _ if ('\x40'..='\x7e').contains(&ch) => {
                    self.state = 0;
                    black_box(std::mem::take(&mut self.params));
                }
                _ => {}
            }
        }
        output
    }
}

fn measure(name: &str, input: &str, mut run: impl FnMut(&str) -> usize) -> (usize, usize) {
    let started = Instant::now();
    let before = ALLOCATIONS.load(Ordering::Relaxed);
    let mut characters = 0;
    for chunk in input.as_bytes().chunks(4096) {
        characters += black_box(run(std::str::from_utf8(chunk).unwrap()));
    }
    let allocations = ALLOCATIONS.load(Ordering::Relaxed) - before;
    let elapsed = started.elapsed().as_secs_f64();
    println!(
        "{name}: {:.1} MB/s, {:.2} ms, {allocations} allocations",
        input.len() as f64 / 1e6 / elapsed,
        elapsed * 1e3
    );
    (characters, allocations)
}

fn main() {
    let input = "\x1b[31mERROR\x1b[0m \x1b[32mINFO\x1b[0m\n".repeat(100_000);
    for _ in 0..3 {
        let mut previous = PreviousSgrPath::default();
        let baseline = measure("PR String/take SGR path", &input, |text| {
            previous.feed(text).len()
        });
        let mut current = RecordingSanitizer::default();
        let updated = measure("streaming event parser", &input, |text| {
            let mut characters = 0;
            current.feed_events(text, |event| {
                if matches!(
                    event,
                    RecordingEvent::Character(_) | RecordingEvent::Newline
                ) {
                    characters += 1;
                }
            });
            characters
        });
        assert_eq!(baseline.0, updated.0);
        assert_eq!(updated.1, 0, "CSI/event delivery must not allocate");
    }
}
