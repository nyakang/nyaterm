//! Synthetic completion benchmark. All text is generated fixture data.
use nyaterm_core::command_search::fuzzy_search_items;
use nyaterm_core::completion::history::TokenHistoryIndex;
use nyaterm_core::completion::matching::TextMatcher;
use nyaterm_core::completion::types::{CompletionScope, ScopedHistoryRecord, ShellDialect};
use nyaterm_core::completion::{CompletionEngine, TokenCompletionRequest};
use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::BTreeMap;
use std::hint::black_box;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

struct CountedAllocator;
static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static LIVE_BYTES: AtomicU64 = AtomicU64::new(0);
static PEAK_BYTES: AtomicU64 = AtomicU64::new(0);
unsafe impl GlobalAlloc for CountedAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            let live = LIVE_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed)
                + layout.size() as u64;
            PEAK_BYTES.fetch_max(live, Ordering::Relaxed);
        }
        ptr
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE_BYTES.fetch_sub(layout.size() as u64, Ordering::Relaxed);
        unsafe {
            System.dealloc(ptr, layout);
        }
    }
}
#[global_allocator]
static ALLOCATOR: CountedAllocator = CountedAllocator;

fn measure(label: &str, size: usize, query: &str, mut run: impl FnMut()) {
    run();
    let mut samples = Vec::with_capacity(25);
    let mut allocations = 0;
    let mut peak = 0;
    for _ in 0..25 {
        let count = ALLOCATIONS.load(Ordering::Relaxed);
        let live = LIVE_BYTES.load(Ordering::Relaxed);
        PEAK_BYTES.store(live, Ordering::Relaxed);
        let started = Instant::now();
        run();
        samples.push(started.elapsed().as_nanos());
        allocations += ALLOCATIONS.load(Ordering::Relaxed).saturating_sub(count);
        peak = peak.max(PEAK_BYTES.load(Ordering::Relaxed).saturating_sub(live));
    }
    samples.sort_unstable();
    println!(
        "{label},{size},{query},{:.3},{:.3},{},{peak}",
        samples[12] as f64 / 1e6,
        samples[23] as f64 / 1e6,
        allocations / 25
    );
}

fn main() {
    println!("engine,candidates,query,p50_ms,p95_ms,allocations,peak_increment_bytes");
    for size in [64, 1000, 10000, 50000] {
        let candidates: Vec<_> = (0..size)
            .map(|i| format!("docker compose run service-{i:05} 工具"))
            .collect();
        let items: Vec<_> = candidates
            .iter()
            .map(|text| (text.as_str(), text.as_str()))
            .collect();
        measure("search_adapter", size, "dcr", || {
            black_box(fuzzy_search_items(&items, "dcr", "history", 12, None, None));
        });
        for query in [
            "d",
            "doc",
            "dcr",
            "dockerco",
            "docker compose run service-000001",
            "zzzz",
            "工具",
        ] {
            measure("legacy", size, query, || {
                for text in &candidates {
                    black_box(legacy_match(black_box(text), query));
                }
            });
            let mut matcher = TextMatcher::default();
            matcher.set_query(query);
            measure("nucleo", size, query, || {
                for text in &candidates {
                    black_box(matcher.score(black_box(text)));
                }
            });
        }
    }
    let scope = CompletionScope::new(&["synthetic-host"]);
    let mut history = TokenHistoryIndex::default();
    for i in 0..5000 {
        history.upsert(ScopedHistoryRecord {
            version: 1,
            scope: scope.clone(),
            dialect: ShellDialect::Bash,
            command: format!("docker run image-{i:05}"),
            use_count: 1,
            last_used_at_ms: i,
            extensions: BTreeMap::new(),
        });
    }
    let mut engine = CompletionEngine::embedded();
    let quick = TokenHistoryIndex::default();
    for query in ["dock", "docker r", "docker run --na", "docker run im"] {
        measure("context", 5000, query, || {
            black_box(engine.complete(TokenCompletionRequest {
                input: query,
                cursor: query.len(),
                dialect: ShellDialect::Bash,
                scope: scope.clone(),
                history: &history,
                quick: &quick,
                limit: 12,
            }));
        });
    }
}

// The pre-migration algorithm, retained only for the reproducible baseline.
fn legacy_match(display: &str, pattern: &str) -> Option<(u32, Vec<u32>)> {
    let haystack: Vec<char> = display.chars().collect();
    let needle: Vec<char> = pattern.chars().filter(|ch| !ch.is_whitespace()).collect();
    if haystack.is_empty() || needle.is_empty() {
        return None;
    }

    let haystack_lower: Vec<char> = haystack.iter().flat_map(|ch| ch.to_lowercase()).collect();
    let needle_lower: Vec<char> = needle.iter().flat_map(|ch| ch.to_lowercase()).collect();
    let mut search_from = 0;
    let mut indices = Vec::new();
    for needle_char in needle_lower {
        let relative_index = haystack_lower[search_from..]
            .iter()
            .position(|haystack_char| *haystack_char == needle_char)?;
        let index = search_from + relative_index;
        indices.push(index as u32);
        search_from = index.saturating_add(1);
    }

    let mut score = (indices.len() as u32).saturating_mul(100);
    for pair in indices.windows(2) {
        if pair[1] == pair[0].saturating_add(1) {
            score = score.saturating_add(40);
        } else {
            score = score.saturating_sub(pair[1].saturating_sub(pair[0]).min(30));
        }
    }
    if let Some(first) = indices.first().copied() {
        score = score.saturating_add(80_u32.saturating_sub(first.min(80)));
    }
    for index in &indices {
        if is_word_boundary(&haystack, *index as usize) {
            score = score.saturating_add(20);
        }
    }
    let display_lower = display.to_lowercase();
    let pattern_lower = pattern.to_lowercase();
    if display_lower.contains(&pattern_lower) {
        score = score.saturating_add(160);
    }

    Some((score, indices))
}

fn is_word_boundary(chars: &[char], index: usize) -> bool {
    if index == 0 {
        return true;
    }
    let previous = chars[index - 1];
    let current = chars[index];
    !previous.is_alphanumeric() || (previous.is_lowercase() && current.is_uppercase())
}
