//! Cross-domain projections and lifecycle transitions for terminal views.

use std::collections::hash_map::Entry;
use std::time::Instant;

use futures::channel::mpsc::UnboundedReceiver;

use super::state::TerminalFeatureState;
use crate::models::{
    TerminalFrameSearchKey, TerminalViewState, terminal_frame_search_result_is_current,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::features) struct TerminalFrameQueueMetrics {
    pub command_count: usize,
    pub output_bytes: usize,
    pub event_count: usize,
    pub event_wake_count: u64,
    pub pending_event_count: usize,
}

impl TerminalFeatureState {
    pub(in crate::features) fn take_frame_event_wake_receiver(
        &self,
    ) -> Option<UnboundedReceiver<()>> {
        self.view.frame_pipeline.take_event_wake_receiver()
    }

    pub(in crate::features) fn arm_frame_event_wakes(&self) {
        self.view.frame_pipeline.arm_event_wakes();
    }

    pub(in crate::features) fn frame_queue_metrics(&self) -> TerminalFrameQueueMetrics {
        TerminalFrameQueueMetrics {
            command_count: self.view.frame_pipeline.queued_command_count(),
            output_bytes: self.view.frame_pipeline.queued_output_bytes(),
            event_count: self.view.frame_pipeline.queued_event_count(),
            event_wake_count: self.view.frame_pipeline.event_wake_count(),
            pending_event_count: self.view.pending_frame_events.len(),
        }
    }

    pub(in crate::features) fn ensure_frame_session(
        &mut self,
        session_id: String,
        encoding: String,
        scrollback_limit: usize,
    ) {
        let view = self
            .view
            .views
            .entry(session_id.clone())
            .or_insert_with(TerminalViewState::new);
        view.set_encoding(&encoding);
        self.view
            .frame_pipeline
            .ensure_session(session_id, encoding, scrollback_limit);
    }

    pub(in crate::features) fn remove_frame_session(&mut self, session_id: &str) {
        self.view.views.remove(session_id);
        self.view
            .frame_pipeline
            .remove_session(session_id.to_string());
    }

    pub(in crate::features) fn seed_session_view(
        &mut self,
        session_id: String,
        output: String,
        encoding: &str,
    ) {
        // A live frame can arrive before the session-start result is drained.
        // Preserve that newer screen instead of replacing it with the reconnect
        // seed and losing the login banner.
        if self
            .view
            .views
            .get(&session_id)
            .is_some_and(|view| !view.output.is_empty())
        {
            return;
        }
        self.view.views.insert(
            session_id,
            TerminalViewState::from_output_with_encoding(output, encoding),
        );
    }

    pub(in crate::features) fn append_session_text_or_create(
        &mut self,
        session_id: &str,
        encoding: &str,
        text: &str,
    ) {
        match self.view.views.entry(session_id.to_string()) {
            Entry::Occupied(mut entry) => entry.get_mut().append_text(text),
            Entry::Vacant(entry) => {
                let mut view = TerminalViewState::new();
                view.set_encoding(encoding);
                view.append_text(text);
                entry.insert(view);
            }
        }
    }

    pub(in crate::features) fn append_existing_session_text(
        &mut self,
        session_id: &str,
        text: &str,
    ) {
        if let Some(view) = self.view.views.get_mut(session_id) {
            view.append_text(text);
        }
    }

    pub(in crate::features) fn session_output(&self, session_id: &str) -> Option<&str> {
        self.view
            .views
            .get(session_id)
            .map(|view| view.output.as_str())
    }

    pub(in crate::features) fn session_output_len_or_default(&self, session_id: &str) -> usize {
        self.view
            .views
            .get(session_id)
            .map_or(self.view.output.len(), |view| view.output.len())
    }

    pub(in crate::features) fn session_has_unread(&self, session_id: &str) -> bool {
        self.view
            .views
            .get(session_id)
            .is_some_and(|view| view.has_unread)
    }

    pub(in crate::features) fn session_scroll_offset(&self, session_id: &str) -> usize {
        self.view
            .views
            .get(session_id)
            .map_or(0, |view| view.scroll_offset)
    }

    pub(in crate::features) fn activate_session_view(&mut self, session_id: &str) -> bool {
        if let Some(view) = self.view.views.get_mut(session_id) {
            view.has_unread = false;
            return view.frame_snapshot.is_none();
        }
        self.view.output.clear();
        self.view.output_decoder.reset_decoder();
        self.view.screen.clear();
        false
    }

    pub(in crate::features) fn enter_session_render_degraded(&mut self, session_id: &str) {
        if let Some(view) = self.view.views.get_mut(session_id) {
            view.enter_render_degraded_mode();
        }
    }

    pub(in crate::features) fn note_session_output_discontinuity(
        &mut self,
        session_id: String,
        encoding: &str,
        bytes: usize,
    ) {
        let view = self
            .view
            .views
            .entry(session_id)
            .or_insert_with(TerminalViewState::new);
        view.set_encoding(encoding);
        view.note_output_discontinuity(bytes);
    }

    pub(in crate::features) fn invalidate_all_render_caches(&mut self) {
        for view in self.view.views.values_mut() {
            view.render_cache.clear();
        }
    }

    pub(in crate::features) fn visible_layout_cache_stats<'a>(
        &self,
        session_ids: impl IntoIterator<Item = &'a str>,
    ) -> (u64, u64) {
        session_ids
            .into_iter()
            .filter_map(|session_id| self.view.views.get(session_id))
            .filter_map(|view| view.render_cache.layout_cache.lock().ok())
            .fold((0u64, 0u64), |(hits, misses), cache| {
                (
                    hits.saturating_add(cache.hits),
                    misses.saturating_add(cache.misses),
                )
            })
    }

    pub(in crate::features) fn visible_performance_recovery_due<'a>(
        &self,
        session_ids: impl IntoIterator<Item = &'a str>,
    ) -> bool {
        session_ids.into_iter().any(|session_id| {
            self.view.views.get(session_id).is_some_and(|view| {
                view.render_degraded
                    || view.performance_overlay.is_some()
                    || view.output_burst_bytes > 0
            })
        })
    }

    /// Whether any of these sessions' terminals asked for a blinking caret.
    ///
    /// DECSCUSR / DECSET 12 arrive as a snapshot attribute, and both paint paths
    /// already honour it (`settings.cursor_blink || snapshot.cursor.blinking`), so
    /// the blink clock has to consider it too or the request paints as a solid
    /// caret forever.
    pub(in crate::features) fn visible_cursor_blink_requested<'a>(
        &self,
        session_ids: impl IntoIterator<Item = &'a str>,
    ) -> bool {
        session_ids.into_iter().any(|session_id| {
            self.view.views.get(session_id).is_some_and(|view| {
                view.frame_snapshot
                    .as_ref()
                    .is_some_and(|snapshot| snapshot.cursor.blinking)
            })
        })
    }

    pub(in crate::features) fn visible_live_snapshot_missing<'a>(
        &self,
        session_ids: impl IntoIterator<Item = &'a str>,
    ) -> bool {
        session_ids.into_iter().any(|session_id| {
            self.view
                .views
                .get(session_id)
                .is_some_and(|view| view.frame_snapshot.is_none() && view.scroll_offset == 0)
        })
    }

    pub(in crate::features) fn search_refresh_is_due(
        &self,
        session_id: &str,
        key: &TerminalFrameSearchKey,
    ) -> bool {
        self.view.views.get(session_id).is_some_and(|view| {
            view.pending_search_key.as_ref() != Some(key)
                && !view.search_result.as_ref().is_some_and(|result| {
                    terminal_frame_search_result_is_current(result, key, view.screen_revision)
                })
        })
    }

    pub(in crate::features) fn tick_session_performance<'a>(
        &mut self,
        session_ids: impl IntoIterator<Item = &'a str>,
        output_pressure: bool,
        now: Instant,
    ) -> Vec<String> {
        let mut changed = Vec::new();
        for session_id in session_ids {
            let Some(view) = self.view.views.get_mut(session_id) else {
                continue;
            };
            if !output_pressure
                && !view.render_degraded
                && view.performance_overlay.is_none()
                && view.output_burst_bytes == 0
            {
                continue;
            }
            let before = view.performance_overlay;
            let was_degraded = view.render_degraded;
            view.tick_performance_overlay(output_pressure, now);
            if view.performance_overlay != before || view.render_degraded != was_degraded {
                changed.push(session_id.to_string());
            }
        }
        changed
    }
}
