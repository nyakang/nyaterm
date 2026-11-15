//! Synthetic completion benchmark. All text is generated fixture data.
use nyaterm_core::command_search::{
    RankedCommandSourceQuery, fuzzy_search_items, search_ranked_command_sources_using,
};
use nyaterm_core::completion::history::TokenHistoryIndex;
use nyaterm_core::completion::matching::TextMatcher;
use nyaterm_core::completion::types::{CompletionScope, ScopedHistoryRecord, ShellDialect};
use nyaterm_core::completion::whole_history::WholeCommandHistoryIndex;
use nyaterm_core::completion::{CompletionEngine, TokenCompletionRequest};
use nyaterm_core::models::quick_commands::CommandHistoryEntry;
use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::BTreeMap;
use std::hint::black_box;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

struct CountedAllocator;
static COUNT_ALLOCATIONS: AtomicBool = AtomicBool::new(false);
static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static LIVE_BYTES: AtomicU64 = AtomicU64::new(0);
static PEAK_BYTES: AtomicU64 = AtomicU64::new(0);
unsafe impl GlobalAlloc for CountedAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() && COUNT_ALLOCATIONS.load(Ordering::Relaxed) {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            let live = LIVE_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed)
                + layout.size() as u64;
            PEAK_BYTES.fetch_max(live, Ordering::Relaxed);
        }
        ptr
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if COUNT_ALLOCATIONS.load(Ordering::Relaxed) {
            let _ = LIVE_BYTES.try_update(Ordering::Relaxed, Ordering::Relaxed, |live| {
                Some(live.saturating_sub(layout.size() as u64))
            });
        }
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
    // Timing runs have accounting disabled; collect allocation evidence separately.
    for _ in 0..25 {
        let started = Instant::now();
        run();
        samples.push(started.elapsed().as_nanos());
    }
    ALLOCATIONS.store(0, Ordering::Relaxed);
    LIVE_BYTES.store(0, Ordering::Relaxed);
    PEAK_BYTES.store(0, Ordering::Relaxed);
    COUNT_ALLOCATIONS.store(true, Ordering::Relaxed);
    run();
    COUNT_ALLOCATIONS.store(false, Ordering::Relaxed);
    let allocations = ALLOCATIONS.load(Ordering::Relaxed);
    let peak = PEAK_BYTES.load(Ordering::Relaxed);
    samples.sort_unstable();
    println!(
        "{label},{size},{query},{:.3},{:.3},{},{peak}",
        samples[12] as f64 / 1e6,
        samples[23] as f64 / 1e6,
        allocations
    );
}

fn main() {
    let optimized_only = std::env::args().any(|arg| arg == "--optimized-only");
    let weighted_only = std::env::args().any(|arg| arg == "--weighted-only");
    println!("engine,candidates,query,p50_ms,p95_ms,allocations,peak_increment_bytes");
    for size in [64, 1000, 5000, 10000, 50000] {
        let candidates: Vec<_> = (0..size)
            .map(|i| format!("docker compose run service-{i:05} 工具"))
            .collect();
        let items: Vec<_> = candidates
            .iter()
            .map(|text| (text.as_str(), text.as_str()))
            .collect();
        if !weighted_only && !optimized_only {
            measure("search_adapter", size, "dcr", || {
                black_box(fuzzy_search_items(&items, "dcr", "history", 12, None, None));
            });
        }
        let scope = CompletionScope::new(&["synthetic-current-host"]);
        let other = CompletionScope::new(&["synthetic-other-host"]);
        let global: Vec<_> = candidates
            .iter()
            .enumerate()
            .map(|(i, command)| CommandHistoryEntry {
                command: command.clone(),
                use_count: (i % 32 + 1) as u32,
                last_used_at_ms: i as u64,
            })
            .collect();
        let scoped: Vec<_> = candidates
            .iter()
            .enumerate()
            .step_by(4)
            .map(|(i, command)| ScopedHistoryRecord {
                version: 1,
                scope: if i % 8 == 0 {
                    scope.clone()
                } else {
                    other.clone()
                },
                dialect: ShellDialect::Bash,
                command: command.clone(),
                use_count: 3,
                last_used_at_ms: i as u64,
                extensions: BTreeMap::new(),
            })
            .collect();
        let mut whole = WholeCommandHistoryIndex::default();
        whole.reconcile(&global, &scoped);
        let mut weighted_matcher = TextMatcher::default();
        for query in ["dcr", "dockerco", "工具", "zzzz"] {
            measure("weighted_whole", size, query, || {
                black_box(search_ranked_command_sources_using(
                    &mut weighted_matcher,
                    RankedCommandSourceQuery {
                        fill_policy: None,
                        history: &whole,
                        scope: &scope,
                        quick_commands: &[],
                        pattern: query,
                        limit: 12,
                        min_history_command_length: Some(2),
                        max_history_command_length: Some(64),
                        now_ms: 100 * 86_400_000,
                    },
                ));
            });
        }
        if weighted_only || optimized_only {
            continue;
        }
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
    if weighted_only {
        return;
    }
    let scope = CompletionScope::new(&["synthetic-host"]);
    let mut history = TokenHistoryIndex::default();
    for size in [1000, 5000, 10000] {
        let records: Vec<_> = (0..size)
            .map(|i| ScopedHistoryRecord {
                version: 1,
                scope: scope.clone(),
                dialect: ShellDialect::Bash,
                command: format!("docker run image-{i:05}"),
                use_count: 1,
                last_used_at_ms: i as u64,
                extensions: BTreeMap::new(),
            })
            .collect();
        measure("token_cold", size, "shared-prefix", || {
            let mut index = TokenHistoryIndex::default();
            for record in &records {
                index.upsert(record.clone());
            }
            black_box(index);
        });
        let mut sample_history = TokenHistoryIndex::default();
        sample_history.reconcile(records);
        let mut count = 0;
        measure("token_update", size, "existing", || {
            count += 1;
            sample_history.upsert(ScopedHistoryRecord {
                version: 1,
                scope: scope.clone(),
                dialect: ShellDialect::Bash,
                command: "docker run image-00000".into(),
                use_count: count,
                last_used_at_ms: 10000,
                extensions: BTreeMap::new(),
            });
        });
        if size == 5000 {
            history = sample_history;
        }
    }
    let mut engine = CompletionEngine::embedded();
    let quick = TokenHistoryIndex::default();
    for query in ["dock", "docker r", "docker run --na", "docker run im"] {
        measure("context", 5000, query, || {
            black_box(engine.complete(TokenCompletionRequest {
                fill_policy: None,
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
    use nyaterm_core::completion::path::{PathEntry, path_candidates, path_query_for_input};
    let entries: Vec<_> = (0..20000)
        .map(|i| PathEntry {
            name: format!("project-{i:05}"),
            is_dir: i % 3 == 0,
            is_symlink: false,
            size: None,
        })
        .collect();
    let input = "cd pr";
    let path = path_query_for_input(input, ShellDialect::Bash, scope.clone()).unwrap();
    let mut matcher = TextMatcher::default();
    measure("paths", 20000, "pr", || {
        black_box(path_candidates(&path, &entries, input, &mut matcher, 12));
    });
    let globals: Vec<_> = (0..50000)
        .map(|i| CommandHistoryEntry {
            command: format!("fixture command-{i:05}"),
            use_count: 1,
            last_used_at_ms: i,
        })
        .collect();
    let mut whole = WholeCommandHistoryIndex::default();
    whole.reconcile(&globals, &[]);
    measure("cancelled_whole", 50000, "fixture", || {
        let mut checks = 0;
        black_box(
            nyaterm_core::command_search::search_ranked_command_sources_with_cancel(
                &mut matcher,
                RankedCommandSourceQuery {
                    history: &whole,
                    scope: &scope,
                    quick_commands: &[],
                    pattern: "fixture",
                    limit: 12,
                    min_history_command_length: None,
                    max_history_command_length: None,
                    now_ms: 10000,
                    fill_policy: None,
                },
                &mut || {
                    checks += 1;
                    checks >= 3
                },
            ),
        );
    });
    let scoped: Vec<_> = (0..5000)
        .map(|i| ScopedHistoryRecord {
            version: 1,
            scope: CompletionScope::new(&["fixture-scope", &(i % 16).to_string()]),
            dialect: ShellDialect::Bash,
            command: globals[i].command.clone(),
            use_count: 1,
            last_used_at_ms: i as u64,
            extensions: BTreeMap::new(),
        })
        .collect();
    whole.reconcile(&globals, &scoped);
    measure("whole_reconcile", 50000, "16-scopes", || {
        whole.reconcile(&globals, &scoped);
    });
    measure("whole_update", 50000, "existing", || {
        whole.upsert_scoped(&scoped[0]);
    });
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
