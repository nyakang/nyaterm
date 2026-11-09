//! Shared terminal layout and bounded decoration caches.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use crate::element::{NyaTerminalLayoutCache, TerminalLineDecorations};

#[derive(Debug, Default)]
pub struct TerminalRenderCache {
    pub layout_cache: Arc<Mutex<NyaTerminalLayoutCache>>,
    decoration_cache: Arc<Mutex<TerminalDecorationCache>>,
}

#[derive(Debug, Default)]
struct TerminalDecorationCache {
    decoration_lines: HashMap<u64, Arc<[TerminalLineDecorations]>>,
    decoration_order: VecDeque<u64>,
    estimated_bytes: usize,
    hits: u64,
    misses: u64,
}

const TERMINAL_DECORATION_CACHE_CAP: usize = 64;
const TERMINAL_DECORATION_CACHE_BYTE_CAP: usize = 8 * 1024 * 1024;

impl TerminalRenderCache {
    pub fn clear(&mut self) {
        if let Ok(mut cache) = self.layout_cache.lock() {
            cache.clear();
        }
        if let Ok(mut cache) = self.decoration_cache.lock() {
            cache.clear();
        }
    }

    pub fn line_decorations(
        &self,
        key: u64,
        build: impl FnOnce() -> Vec<TerminalLineDecorations>,
    ) -> Arc<[TerminalLineDecorations]> {
        let Ok(mut cache) = self.decoration_cache.lock() else {
            return build().into();
        };
        cache.line_decorations(key, build)
    }

    pub fn decoration_stats(&self) -> (u64, u64) {
        self.decoration_cache
            .lock()
            .map(|cache| (cache.hits, cache.misses))
            .unwrap_or((0, 0))
    }
}

impl TerminalDecorationCache {
    fn clear(&mut self) {
        self.decoration_lines.clear();
        self.decoration_order.clear();
        self.estimated_bytes = 0;
        self.hits = 0;
        self.misses = 0;
    }

    fn line_decorations(
        &mut self,
        key: u64,
        build: impl FnOnce() -> Vec<TerminalLineDecorations>,
    ) -> Arc<[TerminalLineDecorations]> {
        self.line_decorations_with_limits(
            key,
            build,
            TERMINAL_DECORATION_CACHE_CAP,
            TERMINAL_DECORATION_CACHE_BYTE_CAP,
        )
    }

    fn line_decorations_with_limits(
        &mut self,
        key: u64,
        build: impl FnOnce() -> Vec<TerminalLineDecorations>,
        max_entries: usize,
        max_bytes: usize,
    ) -> Arc<[TerminalLineDecorations]> {
        if let Some(decorations) = self.decoration_lines.get(&key).map(Arc::clone) {
            self.hits = self.hits.saturating_add(1);
            self.touch(key);
            return decorations;
        }
        self.misses = self.misses.saturating_add(1);
        let decorations: Arc<[TerminalLineDecorations]> = build().into();
        let estimated_bytes = terminal_line_decorations_estimated_bytes(&decorations);
        if max_entries == 0 || estimated_bytes > max_bytes {
            return decorations;
        }
        while self.decoration_lines.len() >= max_entries
            || self.estimated_bytes.saturating_add(estimated_bytes) > max_bytes
        {
            if !self.evict_oldest() {
                break;
            }
        }
        self.decoration_lines.insert(key, Arc::clone(&decorations));
        self.decoration_order.push_back(key);
        self.estimated_bytes = self.estimated_bytes.saturating_add(estimated_bytes);
        decorations
    }

    fn touch(&mut self, key: u64) {
        self.decoration_order.retain(|candidate| *candidate != key);
        self.decoration_order.push_back(key);
    }

    fn evict_oldest(&mut self) -> bool {
        let Some(key) = self.decoration_order.pop_front() else {
            return false;
        };
        if let Some(removed) = self.decoration_lines.remove(&key) {
            self.estimated_bytes = self
                .estimated_bytes
                .saturating_sub(terminal_line_decorations_estimated_bytes(&removed));
        }
        true
    }
}

fn terminal_line_decorations_estimated_bytes(lines: &[TerminalLineDecorations]) -> usize {
    lines
        .iter()
        .fold(std::mem::size_of_val(lines), |bytes, line| {
            bytes
                .saturating_add(
                    line.selected_occurrence_ranges
                        .capacity()
                        .saturating_mul(std::mem::size_of::<(usize, usize)>()),
                )
                .saturating_add(
                    line.search_ranges
                        .capacity()
                        .saturating_mul(std::mem::size_of::<(usize, usize)>()),
                )
                .saturating_add(
                    line.active_search_ranges
                        .capacity()
                        .saturating_mul(std::mem::size_of::<(usize, usize)>()),
                )
                .saturating_add(
                    line.link_ranges
                        .capacity()
                        .saturating_mul(std::mem::size_of::<(usize, usize)>()),
                )
        })
}

#[cfg(test)]
mod tests {
    use super::{
        TerminalDecorationCache, TerminalRenderCache, terminal_line_decorations_estimated_bytes,
    };
    use crate::element::TerminalLineDecorations;
    use std::sync::Arc;

    #[test]
    fn terminal_decoration_cache_reuses_shared_lines() {
        let cache = TerminalRenderCache::default();
        let first = cache.line_decorations(7, || {
            vec![TerminalLineDecorations {
                link_ranges: vec![(1, 3)],
                ..TerminalLineDecorations::default()
            }]
        });
        let second = cache.line_decorations(7, || panic!("cache hit should not rebuild"));

        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(cache.decoration_stats(), (1, 1));
    }

    #[test]
    fn terminal_decoration_cache_evicts_incrementally_by_lru() {
        let mut cache = TerminalDecorationCache::default();
        let build = || vec![TerminalLineDecorations::default()];

        cache.line_decorations_with_limits(1, build, 2, usize::MAX);
        cache.line_decorations_with_limits(2, build, 2, usize::MAX);
        cache.line_decorations_with_limits(1, build, 2, usize::MAX);
        cache.line_decorations_with_limits(3, build, 2, usize::MAX);

        assert!(cache.decoration_lines.contains_key(&1));
        assert!(!cache.decoration_lines.contains_key(&2));
        assert!(cache.decoration_lines.contains_key(&3));
        assert_eq!(cache.decoration_lines.len(), 2);
    }

    #[test]
    fn terminal_decoration_cache_respects_estimated_byte_budget() {
        let sample = vec![TerminalLineDecorations {
            search_ranges: vec![(1, 2); 8],
            ..TerminalLineDecorations::default()
        }];
        let entry_bytes = terminal_line_decorations_estimated_bytes(&sample);
        let mut cache = TerminalDecorationCache::default();

        for key in 1..=3 {
            cache.line_decorations_with_limits(key, || sample.clone(), 10, entry_bytes * 2);
        }

        assert!(cache.estimated_bytes <= entry_bytes * 2);
        assert_eq!(cache.decoration_lines.len(), 2);
        assert!(!cache.decoration_lines.contains_key(&1));
    }
}
