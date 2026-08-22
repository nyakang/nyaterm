use std::borrow::Cow;
use std::collections::{HashMap, VecDeque};
use std::fmt::Write as _;
use std::time::{Duration, Instant};

use gpui::{ClipboardItem, Context};
use nyaterm_terminal::{TerminalClipboardLoad, TerminalEffects, TerminalSnapshot};

use crate::features::NyaTermApp;
use crate::features::formatting::trim_terminal_output_to;
use crate::features::terminal::terminal_surface::terminal_snapshot_absolute_range;
use crate::features::terminal::terminal_surface_entity::{
    terminal_snapshot_covers_display_offset, terminal_surface_paint_count,
};
use crate::models::{
    MainMode, TERMINAL_UI_OUTPUT_TAIL_CAP, TerminalFrameActionLinks, TerminalFrameEvent,
    TerminalFrameOutputEvent, TerminalFrameOutputSubmission, TerminalFrameParts,
    TerminalFrameSearchEvent, TerminalFrameSearchKey, TerminalFrameSearchPurpose,
    TerminalFrameSnapshotEvent, TerminalSearchMode, TerminalViewState, TerminalWindowNode,
    WorkspacePaneNode, terminal_action_link_matcher_key, terminal_frame_scroll_window_extra_rows,
    terminal_frame_search_result_is_current, terminal_snapshot_matches_grid_geometry,
};

use super::view_io::terminal_visual_display_offset;

const MAX_OSC52_REPLY_CHARS: usize = 1_048_576;
const TERMINAL_LIVE_PREFETCH_IDLE_DELAY: Duration = Duration::from_millis(80);

fn terminal_live_scrollback_prefetch_offset(view: &TerminalViewState) -> Option<usize> {
    if view.scroll_offset != 0 {
        return None;
    }
    let snapshot = view.frame_snapshot.as_ref()?;
    if snapshot.scrollback_len == 0 || snapshot.row_count() > snapshot.viewport_rows {
        return None;
    }
    Some(
        snapshot
            .viewport_rows
            .saturating_mul(2)
            .min(snapshot.scrollback_len),
    )
}

fn terminal_live_scrollback_prefetch_request_offset(view: &TerminalViewState) -> Option<usize> {
    let offset = terminal_live_scrollback_prefetch_offset(view)?;
    (!terminal_view_has_cached_scrollback_snapshot_covering_offset(view, offset)).then_some(offset)
}

fn terminal_view_has_cached_scrollback_snapshot_covering_offset(
    view: &TerminalViewState,
    offset: usize,
) -> bool {
    if offset == 0 {
        return view.frame_snapshot.is_some();
    }
    if view.scrollback_snapshots.contains_key(&offset) {
        return true;
    }
    let viewport_rows = view.viewport_rows_for_ui();
    let scrollback_len = view.scrollback_len_for_ui();
    view.scrollback_snapshots.values().any(|snapshot| {
        terminal_snapshot_covers_display_offset(
            snapshot.as_ref(),
            offset,
            viewport_rows,
            scrollback_len,
        )
    })
}

fn terminal_scroll_snapshot_ready_margin_rows(viewport_rows: usize) -> usize {
    terminal_frame_scroll_window_extra_rows(viewport_rows, true)
}

fn terminal_snapshot_margin_for_display_offset(
    snapshot: &TerminalSnapshot,
    display_offset: usize,
    viewport_rows: usize,
    scrollback_len: usize,
) -> Option<(usize, usize)> {
    let viewport_rows = viewport_rows.max(1);
    let real_total_rows = scrollback_len.saturating_add(viewport_rows);
    let (snapshot_start, snapshot_end) = terminal_snapshot_absolute_range(snapshot);
    let desired_end = real_total_rows.saturating_sub(display_offset);
    let desired_start = desired_end.saturating_sub(viewport_rows);
    if snapshot_start > desired_start || desired_end > snapshot_end {
        return None;
    }
    Some((
        desired_start.saturating_sub(snapshot_start),
        snapshot_end.saturating_sub(desired_end),
    ))
}

fn terminal_snapshot_covers_display_offset_with_margin(
    snapshot: &TerminalSnapshot,
    display_offset: usize,
    viewport_rows: usize,
    scrollback_len: usize,
    margin_rows: usize,
) -> bool {
    let Some((older_margin, newer_margin)) = terminal_snapshot_margin_for_display_offset(
        snapshot,
        display_offset,
        viewport_rows,
        scrollback_len,
    ) else {
        return false;
    };
    let required_older = scrollback_len
        .saturating_sub(display_offset)
        .min(margin_rows);
    let required_newer = display_offset.min(margin_rows);
    older_margin >= required_older && newer_margin >= required_newer
}

fn terminal_view_has_cached_scroll_snapshot_ready_for_user_scroll(
    view: &TerminalViewState,
    offset: usize,
) -> bool {
    if offset == 0 {
        return view.frame_snapshot.is_some();
    }
    let viewport_rows = view.viewport_rows_for_ui();
    let scrollback_len = view.scrollback_len_for_ui();
    let margin_rows = terminal_scroll_snapshot_ready_margin_rows(viewport_rows);
    view.scrollback_snapshots.values().any(|snapshot| {
        terminal_snapshot_covers_display_offset_with_margin(
            snapshot.as_ref(),
            offset,
            viewport_rows,
            scrollback_len,
            margin_rows,
        )
    })
}

fn terminal_scroll_snapshot_request_should_enqueue(
    view: &mut TerminalViewState,
    offset: usize,
    priority: bool,
) -> bool {
    if offset == 0 {
        if view.frame_snapshot.is_some() {
            return false;
        }
        if priority {
            if !view.priority_pending_snapshot_offsets.insert(0) {
                return false;
            }
            view.pending_snapshot_offsets.insert(0);
            return true;
        }
        return view.pending_snapshot_offsets.insert(0);
    }

    if priority {
        if terminal_view_has_cached_scroll_snapshot_ready_for_user_scroll(view, offset) {
            return false;
        }
        if view.priority_pending_snapshot_offsets.contains(&offset) {
            return false;
        }
        for stale_offset in std::mem::take(&mut view.priority_pending_snapshot_offsets) {
            view.pending_snapshot_offsets.remove(&stale_offset);
        }
        view.pending_snapshot_offsets.insert(offset);
        view.priority_pending_snapshot_offsets.insert(offset);
        return true;
    }

    !terminal_view_has_cached_scrollback_snapshot_covering_offset(view, offset)
        && view.pending_snapshot_offsets.insert(offset)
}

fn terminal_scroll_snapshot_request_action_links_enabled(
    priority: bool,
    action_links_enabled: bool,
    low_latency_mode: bool,
) -> bool {
    priority && action_links_enabled && !low_latency_mode
}

fn terminal_live_action_link_enrichment_should_enqueue(
    view: &mut TerminalViewState,
    snapshot: &TerminalSnapshot,
    matcher_key: u64,
) -> bool {
    if view.frame_action_links.as_ref().is_some_and(|links| {
        links.matcher_key == matcher_key && links.covers_all_snapshot_rows(snapshot)
    }) {
        return false;
    }
    view.pending_snapshot_offsets.insert(0)
}

impl NyaTermApp {
    pub(in crate::features) fn terminal_scrollback_line_limit(&self) -> usize {
        self.settings
            .summary()
            .terminal_scrollback_lines
            .clamp(100, 100_000) as usize
    }

    pub(in crate::features) fn sync_terminal_scrollback_limits(&mut self) {
        let limit = self.terminal_scrollback_line_limit();
        self.terminal.view.screen.set_scrollback_limit(limit);
        for view in self.terminal.view.views.values_mut() {
            view.screen.set_scrollback_limit(limit);
            view.clamp_scroll_offset();
            view.clear_scrollback_query_caches();
        }
        if self.terminal.view.scroll_offset > self.terminal.view.screen.scrollback_len() {
            self.terminal.view.scroll_offset = self.terminal.view.screen.scrollback_len();
        }
    }

    pub(in crate::features) fn terminal_scrollback_max_bytes(&self) -> usize {
        self.terminal_scrollback_line_limit().saturating_mul(96)
    }

    pub(in crate::features) fn submit_terminal_frame_output(
        &self,
        session_id: &str,
        data: Vec<u8>,
    ) {
        self.terminal.view.frame_pipeline.submit_output(
            session_id.to_string(),
            data,
            self.settings.summary().interaction_default_encoding.clone(),
            self.terminal_scrollback_line_limit(),
        );
    }

    pub(in crate::features) fn submit_terminal_frame_outputs(
        &self,
        outputs: Vec<(String, Vec<u8>)>,
    ) {
        if outputs.is_empty() {
            return;
        }
        let encoding = self.settings.summary().interaction_default_encoding.clone();
        let scrollback_limit = self.terminal_scrollback_line_limit();
        let submissions = outputs
            .into_iter()
            .filter_map(|(session_id, data)| {
                (!data.is_empty()).then_some(TerminalFrameOutputSubmission {
                    session_id,
                    data,
                    encoding: encoding.clone(),
                    scrollback_limit,
                })
            })
            .collect::<Vec<_>>();
        self.terminal
            .view
            .frame_pipeline
            .submit_outputs(submissions);
    }

    pub(in crate::features) fn request_terminal_frame_snapshot(
        &mut self,
        session_id: &str,
        offset: usize,
    ) -> bool {
        if session_id.is_empty() {
            return false;
        }
        let Some(view) = self.terminal.view.views.get_mut(session_id) else {
            return false;
        };
        // offset 0 is live viewport recovery (after worker skipped hidden snaps).
        if offset == 0 {
            if view.frame_snapshot.is_some() || !view.pending_snapshot_offsets.insert(0) {
                return false;
            }
        } else if terminal_view_has_cached_scrollback_snapshot_covering_offset(view, offset)
            || !view.pending_snapshot_offsets.insert(offset)
        {
            return false;
        }
        self.terminal.view.frame_pipeline.request_snapshot(
            session_id.to_string(),
            offset,
            self.settings.summary().terminal_action_links_enabled
                && !self.settings.summary().terminal_low_latency_mode,
            self.settings
                .summary()
                .terminal_action_links_matchers
                .clone(),
        );
        true
    }

    pub(in crate::features) fn request_terminal_live_snapshot(&mut self, session_id: &str) -> bool {
        self.request_terminal_frame_snapshot(session_id, 0)
    }

    pub(in crate::features) fn request_terminal_live_action_link_enrichment(
        &mut self,
        session_id: &str,
        snapshot: &TerminalSnapshot,
    ) -> bool {
        if session_id.is_empty()
            || !self.settings.summary().terminal_action_links_enabled
            || self.settings.summary().terminal_low_latency_mode
        {
            return false;
        }
        let matcher_key = terminal_action_link_matcher_key(
            true,
            &self.settings.summary().terminal_action_links_matchers,
        );
        let Some(view) = self.terminal.view.views.get_mut(session_id) else {
            return false;
        };
        if !terminal_live_action_link_enrichment_should_enqueue(view, snapshot, matcher_key) {
            return false;
        }
        self.terminal
            .view
            .frame_pipeline
            .request_action_link_enrichment(
                session_id.to_string(),
                self.settings
                    .summary()
                    .terminal_action_links_matchers
                    .clone(),
            );
        true
    }

    fn request_terminal_live_scrollback_prefetch(&mut self, session_id: &str) -> bool {
        if session_id.is_empty() {
            return false;
        }
        let Some(view) = self.terminal.view.views.get_mut(session_id) else {
            return false;
        };
        let Some(offset) = terminal_live_scrollback_prefetch_request_offset(view) else {
            return false;
        };
        if !view.pending_snapshot_offsets.insert(offset) {
            return false;
        }
        self.terminal.view.frame_pipeline.request_snapshot(
            session_id.to_string(),
            offset,
            false,
            self.settings
                .summary()
                .terminal_action_links_matchers
                .clone(),
        );
        true
    }

    fn schedule_terminal_live_scrollback_prefetch(
        &mut self,
        session_id: String,
        cx: &mut Context<Self>,
    ) {
        self.terminal.view.live_prefetch_generation = self
            .terminal
            .view
            .live_prefetch_generation
            .saturating_add(1);
        let generation = self.terminal.view.live_prefetch_generation;
        self.terminal.view.live_prefetch_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(TERMINAL_LIVE_PREFETCH_IDLE_DELAY)
                .await;
            let _ = this.update(cx, |this, _cx| {
                if this.terminal.view.live_prefetch_generation == generation {
                    this.request_terminal_live_scrollback_prefetch(&session_id);
                }
            });
        }));
    }

    pub(in crate::features) fn terminal_scroll_text_cached_for_session(
        &self,
        session_id: &str,
        offset: usize,
    ) -> bool {
        self.terminal
            .view
            .views
            .get(session_id)
            .is_some_and(|view| {
                terminal_view_has_cached_scrollback_snapshot_covering_offset(view, offset)
            })
    }

    pub(in crate::features) fn sync_terminal_frame_snapshot_priority(&self) {
        let session_ids = self
            .visible_terminal_session_ids()
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        self.terminal
            .view
            .frame_pipeline
            .set_snapshot_priority(session_ids);
    }

    pub(in crate::features) fn request_terminal_frame_snapshot_for_user_scroll(
        &mut self,
        session_id: &str,
        offset: usize,
    ) -> bool {
        let requested =
            self.request_terminal_frame_snapshot_for_scroll_text(session_id, offset, true);
        if self.runtime_output_pressure_active()
            && requested
            && self.should_log_slow_diagnostic("terminal_user_scroll_snapshot", Instant::now())
        {
            tracing::warn!(
                diagnostic = "terminal_user_scroll_snapshot",
                session_id = %session_id,
                offset,
                "terminal user scroll snapshot requested during output pressure"
            );
        }
        requested
    }

    fn request_terminal_frame_snapshot_for_scroll_text(
        &mut self,
        session_id: &str,
        offset: usize,
        priority: bool,
    ) -> bool {
        if session_id.is_empty() {
            return false;
        }
        let Some(view) = self.terminal.view.views.get_mut(session_id) else {
            return false;
        };
        if !terminal_scroll_snapshot_request_should_enqueue(view, offset, priority) {
            return false;
        }
        if priority {
            let action_links_enabled = terminal_scroll_snapshot_request_action_links_enabled(
                priority,
                self.settings.summary().terminal_action_links_enabled,
                self.settings.summary().terminal_low_latency_mode,
            );
            self.terminal.view.frame_pipeline.request_priority_snapshot(
                session_id.to_string(),
                offset,
                action_links_enabled,
                self.settings
                    .summary()
                    .terminal_action_links_matchers
                    .clone(),
            );
        } else {
            self.terminal.view.frame_pipeline.request_snapshot(
                session_id.to_string(),
                offset,
                false,
                self.settings
                    .summary()
                    .terminal_action_links_matchers
                    .clone(),
            );
        }
        true
    }

    pub(in crate::features) fn request_terminal_frame_snapshot_for_scroll_enrichment(
        &mut self,
        session_id: &str,
        offset: usize,
        snapshot: Option<&TerminalSnapshot>,
    ) -> bool {
        if session_id.is_empty()
            || offset == 0
            || !self.settings.summary().terminal_action_links_enabled
            || self.settings.summary().terminal_low_latency_mode
        {
            return false;
        }
        let matcher_key = terminal_action_link_matcher_key(
            true,
            &self.settings.summary().terminal_action_links_matchers,
        );
        let Some(view) = self.terminal.view.views.get_mut(session_id) else {
            return false;
        };
        let action_links_current = if let Some(snapshot) = snapshot {
            terminal_action_links_current_for_snapshot(
                &view.scrollback_action_links,
                snapshot,
                matcher_key,
            )
        } else {
            terminal_action_links_current_for_offset(
                &view.scrollback_action_links,
                offset,
                matcher_key,
            )
        };
        if action_links_current || view.pending_snapshot_offsets.contains(&offset) {
            return false;
        }
        view.pending_snapshot_offsets.insert(offset);
        view.priority_pending_snapshot_offsets.insert(offset);
        self.terminal.view.frame_pipeline.request_priority_snapshot(
            session_id.to_string(),
            offset,
            true,
            self.settings
                .summary()
                .terminal_action_links_matchers
                .clone(),
        );
        true
    }

    pub(in crate::features) fn request_terminal_frame_search(
        &mut self,
        session_id: &str,
        purpose: TerminalFrameSearchPurpose,
        key: TerminalFrameSearchKey,
    ) -> bool {
        if session_id.is_empty() {
            return false;
        }
        let Some(view) = self.terminal.view.views.get_mut(session_id) else {
            return false;
        };
        match purpose {
            TerminalFrameSearchPurpose::Find => {
                if view.search_result.as_ref().is_some_and(|result| {
                    terminal_frame_search_result_is_current(result, &key, view.screen_revision)
                }) || view.pending_search_key.as_ref() == Some(&key)
                {
                    return false;
                }
                view.pending_search_key = Some(key.clone());
            }
            TerminalFrameSearchPurpose::SelectedOccurrenceVisible { .. } => {
                if view
                    .selected_occurrence_visible_result
                    .as_ref()
                    .is_some_and(|result| {
                        terminal_frame_search_result_is_current(result, &key, view.screen_revision)
                    })
                    || view.pending_selected_occurrence_visible_key.as_ref() == Some(&key)
                {
                    return false;
                }
                view.pending_selected_occurrence_visible_key = Some(key.clone());
            }
            TerminalFrameSearchPurpose::SelectedOccurrence => {
                if view
                    .selected_occurrence_result
                    .as_ref()
                    .is_some_and(|result| {
                        terminal_frame_search_result_is_current(result, &key, view.screen_revision)
                    })
                    || view.pending_selected_occurrence_key.as_ref() == Some(&key)
                {
                    return false;
                }
                view.pending_selected_occurrence_key = Some(key.clone());
            }
        }
        self.terminal
            .view
            .frame_pipeline
            .request_search(session_id.to_string(), purpose, key);
        true
    }

    pub(in crate::features) fn seed_terminal_frame_session(
        &self,
        session_id: &str,
        output: String,
        encoding: &str,
    ) {
        self.terminal.view.frame_pipeline.seed_session(
            session_id.to_string(),
            output,
            encoding.to_string(),
            self.terminal_scrollback_line_limit(),
        );
    }

    pub(in crate::features) fn drive_terminal_render_requests(
        &mut self,
        allow_deferred_work: bool,
    ) -> bool {
        if !allow_deferred_work {
            return false;
        }
        let visible_session_ids = self
            .visible_terminal_session_ids()
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        let live_recovery_ids = visible_session_ids
            .iter()
            .filter(|session_id| {
                self.terminal
                    .view
                    .views
                    .get(session_id.as_str())
                    .is_some_and(|view| view.frame_snapshot.is_none() && view.scroll_offset == 0)
            })
            .cloned()
            .collect::<Vec<_>>();
        let visible_refs = visible_session_ids
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        let snapshot_requests = terminal_frame_snapshot_request_candidates(
            &self.terminal.view.views,
            &self.terminal.view.scroll_delta_residuals,
            &visible_refs,
        );
        let mut requested = false;
        for session_id in live_recovery_ids {
            // Recover live paint after worker skipped hidden snapshots.
            requested |= self.request_terminal_live_snapshot(&session_id);
        }
        for (session_id, offset) in snapshot_requests {
            requested |= self.request_terminal_frame_snapshot(&session_id, offset);
        }
        requested |= self.request_active_terminal_buffer_search();
        requested
    }

    pub(in crate::features) fn drain_terminal_frame_events(
        &mut self,
        cx: &mut Context<Self>,
    ) -> bool {
        self.drain_terminal_frame_events_with_budget(
            cx,
            TERMINAL_FRAME_EVENT_DRAIN_BATCH,
            TERMINAL_FRAME_EVENT_DRAIN_WALL_BUDGET,
        )
    }

    pub(in crate::features) fn drain_terminal_frame_events_for_input_wake(
        &mut self,
        cx: &mut Context<Self>,
    ) -> bool {
        self.drain_terminal_frame_events_with_budget(
            cx,
            TERMINAL_FRAME_INPUT_WAKE_EVENT_DRAIN_BATCH,
            TERMINAL_FRAME_INPUT_WAKE_EVENT_DRAIN_WALL_BUDGET,
        )
    }

    fn drain_terminal_frame_events_with_budget(
        &mut self,
        cx: &mut Context<Self>,
        max_events: usize,
        wall_budget: Duration,
    ) -> bool {
        let started_at = Instant::now();
        let mut dirty = false;
        let surface_paint_count_before = terminal_surface_paint_count();
        let mut drained_events = 0usize;
        let mut output_events = 0usize;
        let mut coalesced_output_events = 0usize;
        let mut accepted_bytes = 0usize;
        let mut max_apply_duration = Duration::ZERO;
        let mut dirty_surface_sessions = Vec::new();
        let mut scroll_position_surface_sessions = Vec::new();
        let visible_session_ids = self
            .visible_terminal_session_ids()
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();

        self.fill_pending_terminal_frame_events(max_events);

        while drained_events < max_events {
            if self.terminal.view.pending_frame_events.is_empty()
                && self.fill_pending_terminal_frame_events(max_events) == 0
            {
                break;
            }

            let (session_event_backlog_active, session_event_queued_output_bytes) =
                self.shell.terminal_frame_input_pressure();
            let allow_deferred_events = terminal_frame_deferred_events_can_apply(
                session_event_backlog_active,
                session_event_queued_output_bytes,
                self.session.event_bridge_queued_output_bytes(),
                pending_terminal_frame_output_events(&self.terminal.view.pending_frame_events),
                self.terminal.view.frame_pipeline.queued_output_bytes(),
            );
            let (frames, coalesced) = pop_terminal_frame_events_for_apply(
                &mut self.terminal.view.pending_frame_events,
                &visible_session_ids,
                allow_deferred_events,
            );
            if frames.is_empty() {
                break;
            }
            coalesced_output_events = coalesced_output_events.saturating_add(coalesced);
            for frame in frames {
                if let TerminalFrameEvent::Output(output) = &frame {
                    output_events += 1;
                    accepted_bytes = accepted_bytes.saturating_add(output.accepted_bytes);
                }
                let apply_started_at = Instant::now();
                let result = self.apply_terminal_frame_event(frame, cx);
                dirty |= result.chrome_dirty;
                if let Some(surface_notify) = result.surface_notify {
                    push_unique_terminal_surface_notify(
                        &mut dirty_surface_sessions,
                        &mut scroll_position_surface_sessions,
                        surface_notify,
                    );
                }
                max_apply_duration = max_apply_duration.max(apply_started_at.elapsed());
                drained_events += 1;

                if drained_events >= max_events || started_at.elapsed() >= wall_budget {
                    break;
                }
            }
            if drained_events >= max_events || started_at.elapsed() >= wall_budget {
                break;
            }
        }

        if drained_events > 0 {
            self.shell.note_terminal_frame_apply(started_at);
        }
        let surface_notify_count =
            dirty_surface_sessions.len() + scroll_position_surface_sessions.len();
        for session_id in dirty_surface_sessions {
            self.sync_terminal_surface_paint(&session_id, cx);
            self.shell.note_terminal_surface_frame_notifies(1);
        }
        for session_id in scroll_position_surface_sessions {
            self.notify_terminal_scroll_position_only(&session_id, cx);
            self.shell.note_terminal_surface_frame_notifies(1);
        }
        let total_duration = started_at.elapsed();
        if (total_duration >= TERMINAL_FRAME_EVENT_DRAIN_SLOW_TOTAL
            || max_apply_duration >= TERMINAL_FRAME_EVENT_APPLY_SLOW)
            && self.should_log_slow_diagnostic("terminal_frame_event_drain", Instant::now())
        {
            let (layout_cache_hits, layout_cache_misses) = terminal_layout_cache_stats_for_sessions(
                &self.terminal.view.views,
                &visible_session_ids,
            );
            let surface_paint_delta =
                terminal_surface_paint_count().saturating_sub(surface_paint_count_before);
            tracing::warn!(
                diagnostic = "terminal_frame_event_drain",
                drained_events,
                output_events,
                coalesced_output_events,
                accepted_bytes,
                surface_notify_count,
                surface_paint_delta,
                layout_cache_hits,
                layout_cache_misses,
                connect_settle_active = self.shell.connect_settle_active(Instant::now()),
                pending_events = self.terminal.view.pending_frame_events.len(),
                total_ms = total_duration.as_millis(),
                max_apply_ms = max_apply_duration.as_millis(),
                "slow terminal frame event drain"
            );
        }
        dirty
    }

    fn fill_pending_terminal_frame_events(&mut self, max_events: usize) -> usize {
        let room = max_events.saturating_sub(self.terminal.view.pending_frame_events.len());
        self.terminal
            .view
            .frame_pipeline
            .drain_events_into(&mut self.terminal.view.pending_frame_events, room)
    }

    fn apply_terminal_frame_event(
        &mut self,
        event: TerminalFrameEvent,
        cx: &mut Context<Self>,
    ) -> TerminalFrameApplyResult {
        match event {
            TerminalFrameEvent::Output(frame) => self.apply_terminal_output_frame(frame, cx),
            TerminalFrameEvent::Snapshot(snapshot) => {
                self.apply_terminal_snapshot_frame(snapshot, cx)
            }
            TerminalFrameEvent::Search(search) => self.apply_terminal_search_frame(search),
        }
    }

    fn apply_terminal_output_frame(
        &mut self,
        frame: TerminalFrameOutputEvent,
        cx: &mut Context<Self>,
    ) -> TerminalFrameApplyResult {
        let TerminalFrameOutputEvent {
            session_id,
            visible_text,
            recording_text_bytes,
            snapshot,
            action_links,
            protocol_state,
            effects,
            command_running,
            accepted_bytes,
            skipped_output_bytes,
            revision,
            snapshot_duration,
            snapshot_stats,
            process_duration,
        } = frame;
        let has_snapshot = snapshot.is_some();
        let is_active = self.session.active_id() == Some(session_id.as_str());
        let is_visible = self.terminal_session_has_visible_surface(&session_id);
        if is_visible && accepted_bytes > 0 && !self.shell.connect_settle_active(Instant::now()) {
            self.enter_connect_settle();
        }
        let effects_need_ui_apply = terminal_effects_need_ui_apply(&effects);
        // Under output pressure, skip retaining full snapshots for hidden tabs.
        // Worker may already omit snapshot for low-priority sessions.
        let keep_hidden_snapshot = !self.runtime_output_pressure_active();
        let mut need_live_snapshot = false;
        let (unread_changed, output_scroll_offset) = {
            let view = self
                .terminal
                .view
                .views
                .entry(session_id.clone())
                .or_insert_with(TerminalViewState::new);
            let had_unread = view.has_unread;
            if !is_active {
                view.has_unread = true;
            }
            let unread_changed = !is_active && !had_unread;
            let snapshot = snapshot.filter(|snapshot| {
                terminal_snapshot_matches_grid_geometry(
                    snapshot.as_ref(),
                    view.screen.cols(),
                    view.screen.rows(),
                )
            });
            if is_visible {
                if let Some(snapshot) = snapshot {
                    view.apply_terminal_frame_parts(TerminalFrameParts {
                        visible_text: &visible_text,
                        snapshot,
                        action_links,
                        protocol_state,
                        accepted_bytes,
                        skipped_output_bytes,
                        revision,
                    });
                } else {
                    // Priority lag: visible surface without a snapshot yet — keep
                    // protocol/revision current and request a live snapshot.
                    view.apply_terminal_background_frame_parts(
                        None,
                        None,
                        &visible_text,
                        protocol_state,
                        skipped_output_bytes,
                        revision,
                    );
                    if accepted_bytes > 0 {
                        view.output_burst_bytes =
                            view.output_burst_bytes.saturating_add(accepted_bytes);
                        view.enter_render_degraded_mode();
                    }
                    if view.scroll_offset > 0 {
                        view.has_new_while_scrolled = true;
                    }
                    need_live_snapshot = true;
                }
            } else {
                let retain = keep_hidden_snapshot.then_some(snapshot).flatten();
                view.apply_terminal_background_frame_parts(
                    retain,
                    if keep_hidden_snapshot {
                        action_links
                    } else {
                        None
                    },
                    &visible_text,
                    protocol_state,
                    skipped_output_bytes,
                    revision,
                );
            }
            (unread_changed, view.scroll_offset)
        };
        if output_scroll_offset == 0 {
            self.clear_terminal_scroll_residual_for_session(Some(&session_id));
        }
        if need_live_snapshot {
            self.request_terminal_live_snapshot(&session_id);
        }
        if is_visible && output_scroll_offset == 0 && accepted_bytes > 0 {
            self.schedule_terminal_live_scrollback_prefetch(session_id.clone(), cx);
        }
        if effects_need_ui_apply {
            self.apply_terminal_effects(&session_id, effects, command_running, cx);
        }
        if process_duration >= Duration::from_millis(20)
            && self.should_log_slow_diagnostic("terminal_frame_processor", Instant::now())
        {
            tracing::warn!(
                diagnostic = "terminal_frame_processor",
                session_id = %session_id,
                accepted_bytes,
                skipped_output_bytes,
                visible_text_bytes = visible_text.len(),
                recording_text_bytes,
                snapshot_ms = snapshot_duration.as_millis(),
                snapshot_reused_rows = snapshot_stats.reused_rows,
                snapshot_rebuilt_rows = snapshot_stats.rebuilt_rows,
                snapshot_inspected_rows = snapshot_stats.inspected_rows,
                process_ms = process_duration.as_millis(),
                "slow terminal frame processing"
            );
        }
        if has_snapshot {
            tracing::debug!(
                diagnostic = "terminal_frame_snapshot_rows",
                session_id = %session_id,
                snapshot_us = snapshot_duration.as_micros(),
                snapshot_reused_rows = snapshot_stats.reused_rows,
                snapshot_rebuilt_rows = snapshot_stats.rebuilt_rows,
                snapshot_inspected_rows = snapshot_stats.inspected_rows,
                "terminal frame snapshot row reuse"
            );
        }
        let surface_notify =
            terminal_output_frame_surface_notify(is_visible, output_scroll_offset, accepted_bytes);
        let chrome_notify =
            terminal_output_frame_needs_chrome_notify(unread_changed, effects_need_ui_apply);
        if chrome_notify {
            self.shell.note_terminal_chrome_frame_notify();
        }
        // Only chrome dirtiness bubbles to NyaTermApp full-shell notify.
        TerminalFrameApplyResult {
            chrome_dirty: chrome_notify,
            surface_notify: surface_notify.map(|notify| notify.with_session(session_id)),
        }
    }

    fn terminal_session_has_visible_surface(&self, session_id: &str) -> bool {
        if session_id.is_empty() || self.shell.main_mode() != MainMode::Workspace {
            return false;
        }
        self.visible_terminal_session_ids().contains(&session_id)
    }

    pub(in crate::features) fn visible_terminal_session_ids(&self) -> Vec<&str> {
        if self.shell.main_mode() != MainMode::Workspace {
            return Vec::new();
        }
        if let Some(root) = self.terminal.windows.tree.as_ref()
            && matches!(root, TerminalWindowNode::Split { .. })
        {
            return terminal_window_node_visible_tab_ids(root);
        }
        if let Some(root) = self.shell.workspace_split() {
            return workspace_pane_node_visible_session_ids(root);
        }
        self.session.active_id().into_iter().collect()
    }

    fn apply_terminal_snapshot_frame(
        &mut self,
        frame: TerminalFrameSnapshotEvent,
        cx: &mut Context<Self>,
    ) -> TerminalFrameApplyResult {
        let current_revision = {
            let Some(view) = self.terminal.view.views.get_mut(&frame.session_id) else {
                return TerminalFrameApplyResult::default();
            };
            view.pending_snapshot_offsets.remove(&frame.offset);
            view.priority_pending_snapshot_offsets.remove(&frame.offset);
            if !terminal_snapshot_matches_grid_geometry(
                frame.snapshot.as_ref(),
                view.screen.cols(),
                view.screen.rows(),
            ) {
                return TerminalFrameApplyResult::default();
            }
            view.screen_revision
        };
        if frame.revision < current_revision {
            if self.should_log_slow_diagnostic("terminal_stale_snapshot", Instant::now()) {
                tracing::warn!(
                    diagnostic = "terminal_stale_snapshot",
                    session_id = %frame.session_id,
                    offset = frame.offset,
                    snapshot_revision = frame.revision,
                    current_revision,
                    "dropped stale terminal frame snapshot"
                );
            }
            return TerminalFrameApplyResult::default();
        }
        let Some(view) = self.terminal.view.views.get_mut(&frame.session_id) else {
            return TerminalFrameApplyResult::default();
        };
        if frame.offset == 0 {
            // Live viewport recovery for sessions that had worker snapshot skipped.
            view.apply_terminal_live_snapshot_frame(
                frame.snapshot.clone(),
                frame.action_links,
                frame.revision,
            );
        } else {
            view.remember_scrollback_snapshot(frame.offset, frame.snapshot.clone());
            if let Some(action_links) = frame.action_links {
                view.scrollback_action_links
                    .insert(frame.offset, action_links);
            } else {
                view.scrollback_action_links.remove(&frame.offset);
            }
            view.prune_scrollback_snapshot_cache(frame.offset);
        }
        if frame.process_duration >= Duration::from_millis(20)
            && self.should_log_slow_diagnostic("terminal_frame_snapshot", Instant::now())
        {
            tracing::warn!(
                diagnostic = "terminal_frame_snapshot",
                session_id = %frame.session_id,
                offset = frame.offset,
                revision = frame.revision,
                snapshot_ms = frame.snapshot_duration.as_millis(),
                snapshot_reused_rows = frame.snapshot_stats.reused_rows,
                snapshot_rebuilt_rows = frame.snapshot_stats.rebuilt_rows,
                snapshot_inspected_rows = frame.snapshot_stats.inspected_rows,
                action_link_reused_rows = frame.action_link_stats.reused_rows,
                action_link_rebuilt_rows = frame.action_link_stats.rebuilt_rows,
                process_ms = frame.process_duration.as_millis(),
                "slow terminal frame snapshot"
            );
        }
        tracing::debug!(
            diagnostic = "terminal_frame_snapshot_rows",
            session_id = %frame.session_id,
            offset = frame.offset,
            snapshot_us = frame.snapshot_duration.as_micros(),
            snapshot_reused_rows = frame.snapshot_stats.reused_rows,
            snapshot_rebuilt_rows = frame.snapshot_stats.rebuilt_rows,
            snapshot_inspected_rows = frame.snapshot_stats.inspected_rows,
            action_link_reused_rows = frame.action_link_stats.reused_rows,
            action_link_rebuilt_rows = frame.action_link_stats.rebuilt_rows,
            "terminal frame snapshot row reuse"
        );
        // Snapshot applies only dirties the surface, not chrome.
        let current_action_link_matcher_key =
            (self.settings.summary().terminal_action_links_enabled
                && !self.settings.summary().terminal_low_latency_mode)
                .then(|| {
                    terminal_action_link_matcher_key(
                        true,
                        &self.settings.summary().terminal_action_links_matchers,
                    )
                });
        let should_paint = self.terminal_session_has_visible_surface(&frame.session_id)
            && self
                .terminal
                .view
                .views
                .get(&frame.session_id)
                .is_some_and(|view| {
                    terminal_snapshot_frame_covers_scroll_target(
                        frame.offset,
                        frame.snapshot.as_ref(),
                        view.scroll_offset,
                        self.terminal_scroll_residual_for_session(Some(&frame.session_id)),
                        view.scrollback_len_for_ui(),
                        view.viewport_rows_for_ui(),
                    )
                });
        if !should_paint
            && self.terminal_session_has_visible_surface(&frame.session_id)
            && let Some(surface) = self.terminal.view.surfaces.get(&frame.session_id).cloned()
        {
            let snapshot = frame.snapshot.clone();
            surface.update(cx, |surface, _cx| {
                surface.retain_prefetched_snapshot(snapshot);
            });
        }
        let should_request_scroll_enrichment = terminal_scroll_enrichment_should_request(
            should_paint,
            frame.offset,
            current_action_link_matcher_key,
            self.terminal.view.views.get(&frame.session_id),
            Some(frame.snapshot.as_ref()),
        );
        if should_request_scroll_enrichment {
            let session_id = frame.session_id.clone();
            let _ = self.request_terminal_frame_snapshot_for_scroll_enrichment(
                session_id.as_str(),
                frame.offset,
                Some(frame.snapshot.as_ref()),
            );
        }
        TerminalFrameApplyResult {
            chrome_dirty: false,
            surface_notify: if should_paint {
                Some(TerminalSurfaceFrameNotify::Full(frame.session_id))
            } else {
                None
            },
        }
    }

    fn apply_terminal_search_frame(
        &mut self,
        frame: TerminalFrameSearchEvent,
    ) -> TerminalFrameApplyResult {
        let session_id = frame.session_id.clone();
        let result_key = frame.result.key.clone();
        let selected_occurrence_frame_is_current = terminal_selected_occurrence_frame_is_current(
            self.terminal
                .selection
                .selected_occurrence
                .session_id
                .as_deref(),
            self.terminal.selection.selected_occurrence.query.as_deref(),
            self.terminal
                .view
                .views
                .get(&frame.session_id)
                .and_then(|view| view.pending_selected_occurrence_key.as_ref()),
            self.terminal
                .view
                .views
                .get(&frame.session_id)
                .and_then(|view| view.pending_selected_occurrence_visible_key.as_ref()),
            frame.session_id.as_str(),
            frame.purpose,
            &frame.result.key,
        );
        let Some((current_revision, result_applied)) = self
            .terminal
            .view
            .views
            .get_mut(&frame.session_id)
            .map(|view| {
                let current_revision = view.screen_revision;
                let result_applied = terminal_apply_search_result_to_view(
                    view,
                    frame.purpose,
                    &frame.result,
                    selected_occurrence_frame_is_current,
                );
                (current_revision, result_applied)
            })
        else {
            return TerminalFrameApplyResult::default();
        };
        if frame.process_duration >= Duration::from_millis(20)
            && self.should_log_slow_diagnostic("terminal_frame_search", Instant::now())
        {
            let match_count = frame
                .result
                .matches
                .as_ref()
                .map(|matches| matches.len())
                .unwrap_or(0);
            tracing::warn!(
                diagnostic = "terminal_frame_search",
                session_id = %frame.session_id,
                query_len = frame.result.key.query.len(),
                revision = frame.result.revision,
                current_revision,
                stale = !result_applied,
                match_count,
                process_ms = frame.process_duration.as_millis(),
                "slow terminal frame search"
            );
        }
        if !result_applied {
            return TerminalFrameApplyResult::default();
        }
        let is_visible = self.terminal_session_has_visible_surface(&session_id);
        if frame.purpose == TerminalFrameSearchPurpose::Find {
            let current_search_key = self.terminal_search_key();
            terminal_search_frame_apply_result(
                session_id,
                true,
                is_visible,
                self.session.active_id(),
                self.terminal.search.open,
                self.terminal.search.mode,
                TerminalFrameSearchKeys {
                    current: current_search_key.as_ref(),
                    result: &result_key,
                },
            )
        } else {
            TerminalFrameApplyResult {
                chrome_dirty: false,
                surface_notify: is_visible.then_some(TerminalSurfaceFrameNotify::Full(session_id)),
            }
        }
    }

    pub(in crate::features) fn enforce_terminal_scrollback_limit(&mut self) {
        self.sync_terminal_scrollback_limits();
        let max_bytes = self.terminal_scrollback_max_bytes();
        trim_terminal_output_to(&mut self.terminal.view.output, max_bytes);
        let ui_output_tail_cap = max_bytes.min(TERMINAL_UI_OUTPUT_TAIL_CAP);
        for view in self.terminal.view.views.values_mut() {
            trim_terminal_output_to(&mut view.output, ui_output_tail_cap);
        }
        self.sync_session_event_bridge_config();
    }

    pub(in crate::features) fn decode_session_output_for_recording(
        &mut self,
        session_id: &str,
        data: &[u8],
    ) -> String {
        let encoding = self.settings.summary().interaction_default_encoding.clone();
        let view = self
            .terminal
            .view
            .views
            .entry(session_id.to_string())
            .or_insert_with(TerminalViewState::new);
        view.recording_decoder.set_encoding(&encoding);
        view.recording_decoder.decode_output_text(data)
    }

    pub(in crate::features) fn encode_visible_terminal_text_for_output(
        &self,
        session_id: &str,
        text: &str,
    ) -> Vec<u8> {
        self.encode_session_outgoing(session_id, text.as_bytes())
    }

    pub(in crate::features) fn append_terminal_log_for_session(
        &mut self,
        session_id: Option<&str>,
        text: &str,
        mark_unread: bool,
    ) {
        self.append_terminal_log_for_session_with_context(session_id, text, mark_unread, None);
    }

    pub(in crate::features) fn append_terminal_log_for_session_with_context(
        &mut self,
        session_id: Option<&str>,
        text: &str,
        mark_unread: bool,
        cx: Option<&mut Context<Self>>,
    ) {
        if text.is_empty() {
            return;
        }
        let text = terminal_local_log_text(text);
        let mut shell_started = false;
        let mut shell_finished = false;
        let mut shell_running = false;
        let mut pending_cwd: Option<String> = None;
        let mut pending_pty_writes: Vec<Vec<u8>>;
        let mut clipboard_store: Option<String>;
        let mut clipboard_loads;

        if let Some(session_id) = session_id {
            let is_active = self.session.active_id() == Some(session_id);
            let encoding = self.settings.summary().interaction_default_encoding.clone();
            let view = self
                .terminal
                .view
                .views
                .entry(session_id.to_string())
                .or_insert_with(TerminalViewState::new);
            view.set_encoding(&encoding);
            view.append_text(text.as_ref());
            if mark_unread && !is_active {
                view.has_unread = true;
            }
            let effects = view.screen.take_effects();
            pending_pty_writes = effects.pty_write;
            clipboard_store = effects.clipboard_store;
            clipboard_loads = effects.clipboard_loads;
            if let Some(title) = effects.title {
                self.session.set_dynamic_title(session_id, Some(title));
            }
            if effects.reset_title {
                self.session.set_dynamic_title(session_id, None);
            }
            let command_running = view.screen.command_running();
            shell_started |= effects.shell_command_started;
            shell_finished |= effects.shell_command_finished;
            shell_running = command_running;
            if let Some(cwd) = effects.cwd {
                pending_cwd = Some(cwd);
            }
        } else {
            self.terminal
                .view
                .screen
                .advance_decoded_text(text.as_ref());
            self.terminal.view.output.push_str(text.as_ref());
            let max_bytes = self.terminal_scrollback_max_bytes();
            trim_terminal_output_to(&mut self.terminal.view.output, max_bytes);
            let effects = self.terminal.view.screen.take_effects();
            pending_pty_writes = effects.pty_write;
            clipboard_store = effects.clipboard_store;
            clipboard_loads = effects.clipboard_loads;
        }

        self.handle_terminal_clipboard_effects(
            &mut clipboard_store,
            &mut clipboard_loads,
            &mut pending_pty_writes,
            cx,
        );

        if let Some(session_id) = session_id {
            self.write_terminal_pty_responses(session_id, pending_pty_writes);
        }
        if (shell_started || shell_finished)
            && let Some(session_id) = session_id
        {
            self.apply_shell_integration_edges(
                session_id,
                shell_started,
                shell_finished,
                shell_running,
            );
        }
        if let (Some(session_id), Some(cwd)) = (session_id, pending_cwd) {
            self.apply_session_cwd(session_id, cwd);
        }
    }

    fn write_terminal_pty_responses(&mut self, session_id: &str, responses: Vec<Vec<u8>>) {
        for response in responses {
            if response.is_empty() {
                continue;
            }
            if let Err(error) = self.write_session_protocol_response(session_id, &response) {
                self.shell
                    .set_status(format!("terminal response failed: {error}"));
                break;
            }
        }
    }

    fn apply_terminal_effects(
        &mut self,
        session_id: &str,
        effects: TerminalEffects,
        command_running: bool,
        cx: &mut Context<Self>,
    ) {
        let mut pending_pty_writes = effects.pty_write;
        let mut clipboard_store = effects.clipboard_store;
        let mut clipboard_loads = effects.clipboard_loads;
        if let Some(title) = effects.title {
            self.session.set_dynamic_title(session_id, Some(title));
        }
        if effects.reset_title {
            self.session.set_dynamic_title(session_id, None);
        }
        self.handle_terminal_clipboard_effects(
            &mut clipboard_store,
            &mut clipboard_loads,
            &mut pending_pty_writes,
            Some(cx),
        );
        self.write_terminal_pty_responses(session_id, pending_pty_writes);
        if effects.shell_command_started || effects.shell_command_finished {
            self.apply_shell_integration_edges(
                session_id,
                effects.shell_command_started,
                effects.shell_command_finished,
                command_running,
            );
        }
        if let Some(cwd) = effects.cwd {
            self.apply_session_cwd(session_id, cwd);
        }
    }

    fn handle_terminal_clipboard_effects(
        &mut self,
        clipboard_store: &mut Option<String>,
        clipboard_loads: &mut Vec<TerminalClipboardLoad>,
        pending_pty_writes: &mut Vec<Vec<u8>>,
        cx: Option<&mut Context<Self>>,
    ) {
        if let Some(cx) = cx {
            if let Some(text) = clipboard_store.take() {
                if self
                    .settings
                    .summary()
                    .interaction_allow_osc52_clipboard_write
                {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                    self.shell
                        .set_status("OSC 52 clipboard updated".to_string());
                } else {
                    self.shell
                        .set_status("OSC 52 clipboard write blocked by settings".to_string());
                }
            }
            if !clipboard_loads.is_empty() {
                let clipboard_text = cx
                    .read_from_clipboard()
                    .and_then(|item| item.text())
                    .unwrap_or_default();
                queue_osc52_clipboard_load_replies(
                    clipboard_loads,
                    &clipboard_text,
                    pending_pty_writes,
                );
            }
        } else {
            if clipboard_store.take().is_some() {
                self.shell
                    .set_status("OSC 52 clipboard update skipped: UI unavailable".to_string());
            }
            if !clipboard_loads.is_empty() {
                queue_osc52_clipboard_load_replies(clipboard_loads, "", pending_pty_writes);
            }
        }
    }
}

#[derive(Default)]
struct TerminalFrameApplyResult {
    chrome_dirty: bool,
    surface_notify: Option<TerminalSurfaceFrameNotify>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum TerminalSurfaceFrameNotify {
    Full(String),
    ScrollPositionOnly(String),
}

impl TerminalSurfaceFrameNotify {
    fn with_session(self, session_id: String) -> Self {
        match self {
            Self::Full(_) => Self::Full(session_id),
            Self::ScrollPositionOnly(_) => Self::ScrollPositionOnly(session_id),
        }
    }
}

fn push_unique_terminal_surface_session(sessions: &mut Vec<String>, session_id: String) {
    if !sessions.iter().any(|existing| existing == &session_id) {
        sessions.push(session_id);
    }
}

fn push_unique_terminal_surface_notify(
    full_sessions: &mut Vec<String>,
    scroll_position_sessions: &mut Vec<String>,
    notify: TerminalSurfaceFrameNotify,
) {
    match notify {
        TerminalSurfaceFrameNotify::Full(session_id) => {
            scroll_position_sessions.retain(|existing| existing != &session_id);
            push_unique_terminal_surface_session(full_sessions, session_id);
        }
        TerminalSurfaceFrameNotify::ScrollPositionOnly(session_id) => {
            if !full_sessions.iter().any(|existing| existing == &session_id) {
                push_unique_terminal_surface_session(scroll_position_sessions, session_id);
            }
        }
    }
}

fn terminal_layout_cache_stats_for_sessions(
    terminal_views: &HashMap<String, TerminalViewState>,
    session_ids: &[String],
) -> (u64, u64) {
    let mut hits = 0u64;
    let mut misses = 0u64;
    for session_id in session_ids {
        let Some(view) = terminal_views.get(session_id) else {
            continue;
        };
        let Ok(cache) = view.render_cache.layout_cache.lock() else {
            continue;
        };
        hits = hits.saturating_add(cache.hits);
        misses = misses.saturating_add(cache.misses);
    }
    (hits, misses)
}

fn pop_terminal_frame_events_for_apply(
    events: &mut VecDeque<TerminalFrameEvent>,
    visible_session_ids: &[String],
    allow_deferred_events: bool,
) -> (Vec<TerminalFrameEvent>, usize) {
    if !allow_deferred_events {
        return pop_terminal_frame_critical_events_for_apply(events, visible_session_ids);
    }
    let Some(first) = events.pop_front() else {
        return (Vec::new(), 0);
    };
    if !matches!(first, TerminalFrameEvent::Output(_)) {
        return (vec![first], 0);
    }

    let mut output_run = vec![first];
    while matches!(events.front(), Some(TerminalFrameEvent::Output(_))) {
        let Some(event) = events.pop_front() else {
            break;
        };
        output_run.push(event);
    }
    coalesce_terminal_output_run_for_apply(output_run, visible_session_ids)
}

fn pop_terminal_frame_critical_events_for_apply(
    events: &mut VecDeque<TerminalFrameEvent>,
    visible_session_ids: &[String],
) -> (Vec<TerminalFrameEvent>, usize) {
    let Some(first_critical_index) = events.iter().position(|event| {
        matches!(
            event,
            TerminalFrameEvent::Output(_) | TerminalFrameEvent::Snapshot(_)
        )
    }) else {
        return (Vec::new(), 0);
    };

    if matches!(
        events.get(first_critical_index),
        Some(TerminalFrameEvent::Snapshot(_))
    ) {
        let Some(event) = events.remove(first_critical_index) else {
            return (Vec::new(), 0);
        };
        return (vec![event], 0);
    }

    let mut output_run = Vec::new();
    let index = first_critical_index;
    while matches!(events.get(index), Some(TerminalFrameEvent::Output(_))) {
        let Some(event) = events.remove(index) else {
            break;
        };
        output_run.push(event);
    }
    coalesce_terminal_output_run_for_apply(output_run, visible_session_ids)
}

fn coalesce_terminal_output_run_for_apply(
    output_run: Vec<TerminalFrameEvent>,
    visible_session_ids: &[String],
) -> (Vec<TerminalFrameEvent>, usize) {
    let mut frames = Vec::new();
    let mut segment = Vec::new();
    let mut coalesced = 0usize;

    for event in output_run {
        let TerminalFrameEvent::Output(frame) = event else {
            continue;
        };
        if terminal_output_frame_is_apply_barrier(&frame) {
            let (mut coalesced_segment, segment_coalesced) =
                coalesce_terminal_pure_output_segment_for_apply(segment, visible_session_ids);
            frames.append(&mut coalesced_segment);
            coalesced = coalesced.saturating_add(segment_coalesced);
            segment = Vec::new();
            frames.push(TerminalFrameEvent::Output(frame));
        } else {
            segment.push(TerminalFrameEvent::Output(frame));
        }
    }

    let (mut coalesced_segment, segment_coalesced) =
        coalesce_terminal_pure_output_segment_for_apply(segment, visible_session_ids);
    frames.append(&mut coalesced_segment);
    coalesced = coalesced.saturating_add(segment_coalesced);

    (frames, coalesced)
}

fn coalesce_terminal_pure_output_segment_for_apply(
    output_run: Vec<TerminalFrameEvent>,
    visible_session_ids: &[String],
) -> (Vec<TerminalFrameEvent>, usize) {
    if output_run.is_empty() {
        return (Vec::new(), 0);
    }
    let original_count = output_run.len();
    let mut latest_by_session: HashMap<String, (usize, TerminalFrameOutputEvent)> = HashMap::new();
    for (index, event) in output_run.into_iter().enumerate() {
        let TerminalFrameEvent::Output(frame) = event else {
            continue;
        };
        if let Some((stored_index, stored)) = latest_by_session.get_mut(&frame.session_id) {
            *stored_index = index;
            merge_terminal_output_frame_for_apply(stored, frame);
        } else {
            latest_by_session.insert(frame.session_id.clone(), (index, frame));
        }
    }
    let mut latest = latest_by_session.into_values().collect::<Vec<_>>();
    latest.sort_by_key(|(index, frame)| {
        (
            terminal_output_frame_apply_priority(frame, visible_session_ids),
            *index,
        )
    });
    let events = latest
        .into_iter()
        .map(|(_, frame)| TerminalFrameEvent::Output(frame))
        .collect::<Vec<_>>();
    let coalesced = original_count.saturating_sub(events.len());
    (events, coalesced)
}

fn terminal_output_frame_is_apply_barrier(frame: &TerminalFrameOutputEvent) -> bool {
    terminal_effects_need_ui_apply(&frame.effects)
}

fn terminal_output_frame_apply_priority(
    frame: &TerminalFrameOutputEvent,
    visible_session_ids: &[String],
) -> u8 {
    if visible_session_ids
        .iter()
        .any(|session_id| session_id == &frame.session_id)
    {
        0
    } else {
        1
    }
}

fn merge_terminal_output_frame_for_apply(
    older: &mut TerminalFrameOutputEvent,
    newer: TerminalFrameOutputEvent,
) {
    older.recording_text_bytes = older
        .recording_text_bytes
        .saturating_add(newer.recording_text_bytes);
    older.accepted_bytes = older.accepted_bytes.saturating_add(newer.accepted_bytes);
    older.skipped_output_bytes = older
        .skipped_output_bytes
        .saturating_add(newer.skipped_output_bytes);
    append_terminal_apply_visible_tail(&mut older.visible_text, &newer.visible_text);
    // Prefer the newest revision's snapshot even when None (avoids stale grids).
    older.snapshot = newer.snapshot;
    older.action_links = newer.action_links;
    older.protocol_state = newer.protocol_state;
    older.command_running = newer.command_running;
    older.revision = newer.revision;
    older.snapshot_duration = newer.snapshot_duration;
    older.snapshot_stats = newer.snapshot_stats;
    older.process_duration = older
        .process_duration
        .saturating_add(newer.process_duration);
}

fn append_terminal_apply_visible_tail(output: &mut String, text: &str) {
    if text.is_empty() {
        return;
    }
    output.push_str(text);
    trim_terminal_apply_string_to_tail(output, TERMINAL_FRAME_APPLY_VISIBLE_TEXT_TAIL_CAP);
}

fn trim_terminal_apply_string_to_tail(output: &mut String, max_bytes: usize) {
    if output.len() <= max_bytes {
        return;
    }
    let mut start = output.len().saturating_sub(max_bytes);
    while start < output.len() && !output.is_char_boundary(start) {
        start += 1;
    }
    output.drain(..start);
}

fn terminal_frame_snapshot_request_candidates(
    terminal_views: &HashMap<String, TerminalViewState>,
    scroll_delta_residuals: &HashMap<String, f32>,
    visible_session_ids: &[&str],
) -> Vec<(String, usize)> {
    terminal_views
        .iter()
        .filter_map(|(session_id, view)| {
            if !visible_session_ids
                .iter()
                .any(|visible_id| *visible_id == session_id)
            {
                return None;
            }
            let residual = scroll_delta_residuals
                .get(session_id)
                .copied()
                .unwrap_or_default();
            let scrollback_len = view.scrollback_len_for_ui();
            let display_offset = terminal_frame_snapshot_prefetch_offset(
                view.scroll_offset,
                residual,
                scrollback_len,
            );
            (display_offset > 0).then(|| (session_id.clone(), display_offset))
        })
        .collect()
}

fn terminal_frame_snapshot_prefetch_offset(
    scroll_offset: usize,
    residual_lines: f32,
    scrollback_len: usize,
) -> usize {
    if scrollback_len == 0 {
        return 0;
    }
    if residual_lines > 0.0 && residual_lines.is_finite() {
        return scroll_offset.saturating_add(1).min(scrollback_len);
    }
    if residual_lines < 0.0 && residual_lines.is_finite() {
        return scroll_offset.saturating_sub(1);
    }
    terminal_visual_display_offset(scroll_offset, residual_lines, scrollback_len)
}

#[cfg(test)]
fn terminal_snapshot_frame_matches_scroll_target(
    frame_offset: usize,
    scroll_offset: usize,
    residual_lines: f32,
    scrollback_len: usize,
) -> bool {
    if frame_offset == 0 {
        return scroll_offset == 0;
    }
    terminal_frame_snapshot_prefetch_offset(scroll_offset, residual_lines, scrollback_len)
        == frame_offset
}

fn terminal_snapshot_frame_covers_scroll_target(
    frame_offset: usize,
    snapshot: &TerminalSnapshot,
    scroll_offset: usize,
    residual_lines: f32,
    scrollback_len: usize,
    viewport_rows: usize,
) -> bool {
    if frame_offset == 0 {
        return scroll_offset == 0;
    }
    let display_offset =
        terminal_visual_display_offset(scroll_offset, residual_lines, scrollback_len);
    display_offset > 0
        && terminal_snapshot_covers_display_offset(
            snapshot,
            display_offset,
            viewport_rows,
            scrollback_len,
        )
}

fn terminal_action_links_current_for_offset(
    scrollback_action_links: &HashMap<usize, TerminalFrameActionLinks>,
    offset: usize,
    matcher_key: u64,
) -> bool {
    scrollback_action_links
        .get(&offset)
        .is_some_and(|links| links.matcher_key == matcher_key)
}

fn terminal_action_links_current_for_snapshot(
    scrollback_action_links: &HashMap<usize, TerminalFrameActionLinks>,
    snapshot: &TerminalSnapshot,
    matcher_key: u64,
) -> bool {
    let links = scrollback_action_links
        .values()
        .filter(|links| {
            links.matcher_key == matcher_key
                && crate::features::terminal::terminal_surface::terminal_action_links_overlap_snapshot(
                    snapshot, links,
                )
        })
        .cloned()
        .collect::<Vec<_>>();
    crate::features::terminal::terminal_surface::terminal_action_links_cover_all_snapshot_rows(
        snapshot, &links,
    )
}

pub(super) fn terminal_scroll_enrichment_should_request(
    should_paint: bool,
    offset: usize,
    matcher_key: Option<u64>,
    view: Option<&TerminalViewState>,
    snapshot: Option<&TerminalSnapshot>,
) -> bool {
    should_paint
        && offset > 0
        && matcher_key.is_some_and(|matcher_key| {
            view.is_some_and(|view| {
                if let Some(snapshot) = snapshot {
                    !terminal_action_links_current_for_snapshot(
                        &view.scrollback_action_links,
                        snapshot,
                        matcher_key,
                    )
                } else {
                    !terminal_action_links_current_for_offset(
                        &view.scrollback_action_links,
                        offset,
                        matcher_key,
                    )
                }
            })
        })
}

fn terminal_frame_deferred_events_can_apply(
    session_event_backlog_active: bool,
    session_event_queued_output_bytes: usize,
    bridge_queued_output_bytes: usize,
    pending_terminal_frame_output_events: usize,
    queued_terminal_frame_output_bytes: usize,
) -> bool {
    !session_event_backlog_active
        && session_event_queued_output_bytes == 0
        && bridge_queued_output_bytes == 0
        && pending_terminal_frame_output_events == 0
        && queued_terminal_frame_output_bytes == 0
}

fn pending_terminal_frame_output_events(events: &VecDeque<TerminalFrameEvent>) -> usize {
    events
        .iter()
        .filter(|event| matches!(event, TerminalFrameEvent::Output(_)))
        .count()
}

fn workspace_pane_node_visible_session_ids(root: &WorkspacePaneNode) -> Vec<&str> {
    let mut ids = Vec::new();
    collect_workspace_pane_node_visible_session_ids(root, &mut ids);
    ids
}

fn collect_workspace_pane_node_visible_session_ids<'a>(
    node: &'a WorkspacePaneNode,
    ids: &mut Vec<&'a str>,
) {
    match node {
        WorkspacePaneNode::Leaf { session_id, .. } => {
            ids.push(session_id.as_str());
        }
        WorkspacePaneNode::Split { first, second, .. } => {
            collect_workspace_pane_node_visible_session_ids(first, ids);
            collect_workspace_pane_node_visible_session_ids(second, ids);
        }
    }
}

#[cfg(test)]
mod frame_event_queue_tests {
    use std::collections::{HashMap, VecDeque};
    use std::time::Duration;

    use nyaterm_terminal::TerminalEffects;

    use crate::models::{
        TerminalFrameActionLinks, TerminalFrameEvent, TerminalFrameOutputEvent,
        TerminalFrameSearchEvent, TerminalFrameSearchKey, TerminalFrameSearchPurpose,
        TerminalFrameSearchResult, TerminalFrameSnapshotEvent, TerminalProtocolState,
        TerminalViewState, prepare_terminal_frame_action_links,
    };
    use nyaterm_core::ActionLinksMatcherSettings;

    use super::{
        TerminalSurfaceFrameNotify, pop_terminal_frame_events_for_apply,
        push_unique_terminal_surface_notify, push_unique_terminal_surface_session,
        terminal_action_link_matcher_key, terminal_action_links_current_for_offset,
        terminal_frame_deferred_events_can_apply, terminal_frame_snapshot_request_candidates,
        terminal_live_action_link_enrichment_should_enqueue,
        terminal_live_scrollback_prefetch_offset, terminal_live_scrollback_prefetch_request_offset,
        terminal_scroll_enrichment_should_request, terminal_scroll_snapshot_ready_margin_rows,
        terminal_scroll_snapshot_request_action_links_enabled,
        terminal_scroll_snapshot_request_should_enqueue,
        terminal_snapshot_frame_covers_scroll_target,
        terminal_snapshot_frame_matches_scroll_target,
        terminal_view_has_cached_scroll_snapshot_ready_for_user_scroll,
        terminal_view_has_cached_scrollback_snapshot_covering_offset,
    };

    fn output_frame(session_id: &str, revision: u64) -> TerminalFrameEvent {
        TerminalFrameEvent::Output(TerminalFrameOutputEvent {
            session_id: session_id.to_string(),
            visible_text: format!("rev-{revision}"),
            recording_text_bytes: 0,
            snapshot: Some(std::sync::Arc::new(
                nyaterm_terminal::TerminalScreen::default().viewport_snapshot(0),
            )),
            action_links: None,
            protocol_state: TerminalProtocolState::default(),
            effects: TerminalEffects::default(),
            command_running: false,
            accepted_bytes: revision as usize,
            skipped_output_bytes: 0,
            revision,
            snapshot_duration: Duration::from_millis(revision),
            snapshot_stats: nyaterm_terminal::TerminalSnapshotBuildStats {
                reused_rows: revision as usize,
                rebuilt_rows: revision.saturating_add(1) as usize,
                inspected_rows: revision.saturating_add(1) as usize,
            },
            process_duration: Duration::ZERO,
        })
    }

    fn search_frame(session_id: &str) -> TerminalFrameEvent {
        TerminalFrameEvent::Search(TerminalFrameSearchEvent {
            session_id: session_id.to_string(),
            purpose: TerminalFrameSearchPurpose::Find,
            result: TerminalFrameSearchResult::new(
                TerminalFrameSearchKey {
                    query: "query".to_string(),
                    case_sensitive: false,
                    regex: false,
                    whole_word: false,
                    limit: 100,
                    request_generation: 0,
                },
                1,
                Ok(Vec::new()),
            ),
            process_duration: Duration::ZERO,
        })
    }

    fn snapshot_frame(session_id: &str, offset: usize) -> TerminalFrameEvent {
        TerminalFrameEvent::Snapshot(TerminalFrameSnapshotEvent {
            session_id: session_id.to_string(),
            offset,
            snapshot: std::sync::Arc::new(
                nyaterm_terminal::TerminalScreen::default().viewport_snapshot(offset),
            ),
            action_links: None,
            revision: 1,
            snapshot_duration: Duration::ZERO,
            snapshot_stats: Default::default(),
            action_link_stats: Default::default(),
            process_duration: Duration::ZERO,
        })
    }

    fn terminal_view_with_scrollback(scroll_offset: usize) -> TerminalViewState {
        let output = (0..80)
            .map(|index| format!("line-{index:02}"))
            .collect::<Vec<_>>()
            .join("\r\n");
        let mut view = TerminalViewState::from_output(output);
        view.scroll_offset = scroll_offset;
        assert!(view.scrollback_len_for_ui() >= scroll_offset);
        view
    }

    #[test]
    fn terminal_frame_snapshot_requests_only_include_visible_scrolled_sessions() {
        let mut views = HashMap::new();
        let visible_scrolled = terminal_view_with_scrollback(5);
        views.insert("visible-scrolled".to_string(), visible_scrolled);

        let hidden_scrolled = terminal_view_with_scrollback(7);
        views.insert("hidden-scrolled".to_string(), hidden_scrolled);

        let visible_at_bottom = terminal_view_with_scrollback(0);
        views.insert("visible-at-bottom".to_string(), visible_at_bottom);

        assert_eq!(
            terminal_frame_snapshot_request_candidates(
                &views,
                &HashMap::new(),
                &["visible-scrolled", "visible-at-bottom"]
            ),
            vec![("visible-scrolled".to_string(), 5)]
        );
    }

    #[test]
    fn terminal_frame_snapshot_requests_visual_display_offset_with_fractional_scroll() {
        let mut views = HashMap::new();
        views.insert("visible".to_string(), terminal_view_with_scrollback(4));
        let residuals = HashMap::from([("visible".to_string(), 0.25)]);

        assert_eq!(
            terminal_frame_snapshot_request_candidates(&views, &residuals, &["visible"]),
            vec![("visible".to_string(), 5)]
        );
    }

    #[test]
    fn terminal_snapshot_frame_matches_fractional_visual_scroll_target() {
        assert!(terminal_snapshot_frame_matches_scroll_target(0, 0, 0.0, 10));
        assert!(!terminal_snapshot_frame_matches_scroll_target(
            0, 1, 0.0, 10
        ));
        assert!(terminal_snapshot_frame_matches_scroll_target(
            5, 4, 0.25, 10
        ));
        assert!(!terminal_snapshot_frame_matches_scroll_target(
            4, 4, 0.25, 10
        ));
    }

    #[test]
    fn terminal_snapshot_frame_covers_nearby_scroll_target() {
        let view = terminal_view_with_scrollback(6);
        let mut snapshot = view.screen.viewport_snapshot(6);
        let viewport_rows = snapshot.row_count().max(1);
        snapshot.row_data = snapshot
            .rows()
            .iter()
            .cloned()
            .cycle()
            .take(snapshot.row_count().saturating_add(viewport_rows))
            .collect::<Vec<_>>()
            .into();

        assert!(terminal_snapshot_frame_covers_scroll_target(
            6,
            &snapshot,
            7,
            0.0,
            view.scrollback_len_for_ui(),
            viewport_rows,
        ));
        assert!(!terminal_snapshot_frame_matches_scroll_target(
            6,
            7,
            0.0,
            view.scrollback_len_for_ui(),
        ));
    }

    #[test]
    fn terminal_cached_snapshot_coverage_prevents_duplicate_scroll_request() {
        let mut view = terminal_view_with_scrollback(0);
        let cached_key = 4;
        let covered_offset = 6;
        view.scrollback_snapshots.insert(
            cached_key,
            std::sync::Arc::new(view.screen.viewport_snapshot(covered_offset)),
        );

        assert!(!view.scrollback_snapshots.contains_key(&covered_offset));
        assert!(
            terminal_view_has_cached_scrollback_snapshot_covering_offset(&view, covered_offset)
        );
    }

    #[test]
    fn terminal_live_scrollback_prefetch_only_runs_for_viewport_only_snapshot() {
        let mut view = terminal_view_with_scrollback(0);
        view.frame_snapshot = Some(std::sync::Arc::new(view.screen.viewport_snapshot(0)));
        let viewport_rows = view.viewport_rows_for_ui();
        assert_eq!(
            terminal_live_scrollback_prefetch_offset(&view),
            Some(viewport_rows.saturating_mul(2))
        );

        view.frame_snapshot = Some(std::sync::Arc::new(
            view.screen.viewport_snapshot_with_window(0, 32, 32),
        ));
        assert_eq!(terminal_live_scrollback_prefetch_offset(&view), None);

        view.frame_snapshot = Some(std::sync::Arc::new(view.screen.viewport_snapshot(0)));
        view.scroll_offset = 1;
        assert_eq!(terminal_live_scrollback_prefetch_offset(&view), None);
    }

    #[test]
    fn terminal_live_action_link_enrichment_deduplicates_and_skips_current_links() {
        let mut view = terminal_view_with_scrollback(0);
        let snapshot = view.screen.viewport_snapshot(0);
        let matchers = ActionLinksMatcherSettings::default();
        let matcher_key = terminal_action_link_matcher_key(true, &matchers);

        assert!(terminal_live_action_link_enrichment_should_enqueue(
            &mut view,
            &snapshot,
            matcher_key,
        ));
        assert!(!terminal_live_action_link_enrichment_should_enqueue(
            &mut view,
            &snapshot,
            matcher_key,
        ));

        view.pending_snapshot_offsets.clear();
        view.frame_action_links = prepare_terminal_frame_action_links(&snapshot, true, &matchers);
        assert!(!terminal_live_action_link_enrichment_should_enqueue(
            &mut view,
            &snapshot,
            matcher_key,
        ));
        assert!(!view.pending_snapshot_offsets.contains(&0));
    }

    #[test]
    fn terminal_live_scrollback_prefetch_skips_cached_target_window() {
        let mut view = terminal_view_with_scrollback(0);
        view.frame_snapshot = Some(std::sync::Arc::new(view.screen.viewport_snapshot(0)));
        let offset = terminal_live_scrollback_prefetch_offset(&view).expect("prefetch offset");
        view.scrollback_snapshots.insert(
            offset,
            std::sync::Arc::new(view.screen.viewport_snapshot(offset)),
        );

        assert!(terminal_view_has_cached_scrollback_snapshot_covering_offset(&view, offset));
        assert_eq!(
            terminal_live_scrollback_prefetch_request_offset(&view),
            None
        );
    }

    #[test]
    fn terminal_user_scroll_snapshot_cache_requires_edge_margin() {
        let mut view = terminal_view_with_scrollback(20);
        let offset = 20;
        view.scrollback_snapshots.insert(
            offset,
            std::sync::Arc::new(view.screen.viewport_snapshot(offset)),
        );

        assert!(terminal_view_has_cached_scrollback_snapshot_covering_offset(&view, offset));
        assert!(!terminal_view_has_cached_scroll_snapshot_ready_for_user_scroll(&view, offset));
    }

    #[test]
    fn terminal_user_scroll_snapshot_cache_accepts_margin_window() {
        let mut view = terminal_view_with_scrollback(20);
        let offset = 20;
        let mut snapshot = view.screen.viewport_snapshot(offset);
        let viewport_rows = snapshot.row_count().max(1);
        let margin_rows = terminal_scroll_snapshot_ready_margin_rows(viewport_rows);
        snapshot.row_data = snapshot
            .rows()
            .iter()
            .cloned()
            .cycle()
            .take(viewport_rows.saturating_add(margin_rows.saturating_mul(2)))
            .collect::<Vec<_>>()
            .into();
        snapshot.total_rows = snapshot.total_rows.saturating_add(margin_rows);
        view.scrollback_snapshots
            .insert(offset, std::sync::Arc::new(snapshot));

        assert!(terminal_view_has_cached_scroll_snapshot_ready_for_user_scroll(&view, offset));
    }

    #[test]
    fn terminal_user_scroll_snapshot_ready_margin_matches_priority_window_scale() {
        assert_eq!(terminal_scroll_snapshot_ready_margin_rows(12), 64);
        assert_eq!(terminal_scroll_snapshot_ready_margin_rows(40), 120);
        assert_eq!(terminal_scroll_snapshot_ready_margin_rows(160), 256);
    }

    #[test]
    fn terminal_user_scroll_snapshot_upgrades_existing_normal_pending_request() {
        let mut view = terminal_view_with_scrollback(20);
        let offset = 20;

        assert!(terminal_scroll_snapshot_request_should_enqueue(
            &mut view, offset, false
        ));
        assert!(view.pending_snapshot_offsets.contains(&offset));
        assert!(!view.priority_pending_snapshot_offsets.contains(&offset));

        assert!(terminal_scroll_snapshot_request_should_enqueue(
            &mut view, offset, true
        ));
        assert!(view.pending_snapshot_offsets.contains(&offset));
        assert!(view.priority_pending_snapshot_offsets.contains(&offset));
        assert!(!terminal_scroll_snapshot_request_should_enqueue(
            &mut view, offset, true
        ));

        let newer_offset = offset + 3;
        assert!(terminal_scroll_snapshot_request_should_enqueue(
            &mut view,
            newer_offset,
            true
        ));
        assert!(!view.pending_snapshot_offsets.contains(&offset));
        assert!(!view.priority_pending_snapshot_offsets.contains(&offset));
        assert!(view.pending_snapshot_offsets.contains(&newer_offset));
        assert!(
            view.priority_pending_snapshot_offsets
                .contains(&newer_offset)
        );
    }

    #[test]
    fn terminal_action_links_cache_requires_current_matcher_key() {
        let matchers = ActionLinksMatcherSettings::default();
        let current_key = terminal_action_link_matcher_key(true, &matchers);
        let lightweight_key = terminal_action_link_matcher_key(false, &matchers);
        let mut links = HashMap::from([(
            3,
            TerminalFrameActionLinks {
                matcher_key: lightweight_key,
                absolute_start_row: 0,
                absolute_end_row: 0,
                row_signatures: Vec::new(),
                matches_by_line: Vec::new(),
                cell_ranges_by_line: Vec::new(),
            },
        )]);

        assert!(!terminal_action_links_current_for_offset(
            &links,
            3,
            current_key
        ));

        links.insert(
            3,
            TerminalFrameActionLinks {
                matcher_key: current_key,
                absolute_start_row: 0,
                absolute_end_row: 0,
                row_signatures: Vec::new(),
                matches_by_line: Vec::new(),
                cell_ranges_by_line: Vec::new(),
            },
        );
        assert!(terminal_action_links_current_for_offset(
            &links,
            3,
            current_key
        ));
    }

    #[test]
    fn terminal_scroll_snapshot_action_links_only_for_priority_user_requests() {
        assert!(terminal_scroll_snapshot_request_action_links_enabled(
            true, true, false
        ));
        assert!(!terminal_scroll_snapshot_request_action_links_enabled(
            false, true, false
        ));
        assert!(!terminal_scroll_snapshot_request_action_links_enabled(
            true, false, false
        ));
        assert!(!terminal_scroll_snapshot_request_action_links_enabled(
            true, true, true
        ));
    }

    #[test]
    fn terminal_scroll_enrichment_requests_visible_missing_action_links_immediately() {
        let matchers = ActionLinksMatcherSettings::default();
        let matcher_key = terminal_action_link_matcher_key(true, &matchers);
        let mut view = terminal_view_with_scrollback(20);
        let snapshot = view.screen.viewport_snapshot(4);

        assert!(terminal_scroll_enrichment_should_request(
            true,
            4,
            Some(matcher_key),
            Some(&view),
            Some(&snapshot),
        ));
        let (absolute_start_row, absolute_end_row) =
            crate::features::terminal::terminal_surface::terminal_snapshot_absolute_range(
                &snapshot,
            );

        view.scrollback_action_links.insert(
            98,
            TerminalFrameActionLinks {
                matcher_key,
                absolute_start_row,
                absolute_end_row: absolute_start_row + 1,
                row_signatures: snapshot
                    .rows()
                    .iter()
                    .take(1)
                    .map(|row| row.signature)
                    .collect(),
                matches_by_line: vec![Vec::new()],
                cell_ranges_by_line: vec![Vec::new()],
            },
        );
        assert!(terminal_scroll_enrichment_should_request(
            true,
            4,
            Some(matcher_key),
            Some(&view),
            Some(&snapshot),
        ));

        view.scrollback_action_links.insert(
            99,
            TerminalFrameActionLinks {
                matcher_key,
                absolute_start_row,
                absolute_end_row,
                row_signatures: snapshot.rows().iter().map(|row| row.signature).collect(),
                matches_by_line: vec![Vec::new(); snapshot.row_count()],
                cell_ranges_by_line: vec![Vec::new(); snapshot.row_count()],
            },
        );
        assert!(!terminal_scroll_enrichment_should_request(
            true,
            4,
            Some(matcher_key),
            Some(&view),
            Some(&snapshot),
        ));
        assert!(!terminal_scroll_enrichment_should_request(
            false,
            4,
            Some(matcher_key),
            Some(&view),
            Some(&snapshot),
        ));
        assert!(!terminal_scroll_enrichment_should_request(
            true,
            0,
            Some(matcher_key),
            Some(&view),
            Some(&snapshot),
        ));
        assert!(!terminal_scroll_enrichment_should_request(
            true,
            4,
            None,
            Some(&view),
            Some(&snapshot),
        ));
    }

    #[test]
    fn terminal_frame_apply_coalesces_consecutive_output_to_latest_per_session() {
        let mut events = VecDeque::from([
            output_frame("a", 1),
            output_frame("a", 2),
            output_frame("b", 3),
        ]);

        let (frames, coalesced) = pop_terminal_frame_events_for_apply(&mut events, &[], true);

        assert_eq!(coalesced, 1);
        assert!(events.is_empty());
        assert_eq!(frames.len(), 2);
        assert!(matches!(
            &frames[0],
            TerminalFrameEvent::Output(frame) if frame.session_id == "a" && frame.revision == 2
        ));
        assert!(matches!(
            &frames[0],
            TerminalFrameEvent::Output(frame)
                if frame.accepted_bytes == 3 && frame.visible_text == "rev-1rev-2"
        ));
        assert!(matches!(
            &frames[0],
            TerminalFrameEvent::Output(frame)
                if frame.snapshot_duration == Duration::from_millis(2)
                    && frame.snapshot_stats.reused_rows == 2
                    && frame.snapshot_stats.rebuilt_rows == 3
        ));
        assert!(matches!(
            &frames[1],
            TerminalFrameEvent::Output(frame) if frame.session_id == "b" && frame.revision == 3
        ));
    }

    #[test]
    fn terminal_frame_apply_prioritizes_visible_output() {
        let mut events = VecDeque::from([
            output_frame("hidden", 1),
            output_frame("visible", 2),
            output_frame("hidden", 3),
        ]);

        let (frames, coalesced) =
            pop_terminal_frame_events_for_apply(&mut events, &["visible".to_string()], true);

        assert_eq!(coalesced, 1);
        assert!(events.is_empty());
        assert_eq!(frames.len(), 2);
        assert!(matches!(
            &frames[0],
            TerminalFrameEvent::Output(frame)
                if frame.session_id == "visible" && frame.revision == 2
        ));
        assert!(matches!(
            &frames[1],
            TerminalFrameEvent::Output(frame)
                if frame.session_id == "hidden" && frame.revision == 3
        ));
    }

    #[test]
    fn terminal_frame_apply_preserves_output_effect_barriers() {
        let mut effect = output_frame("a", 3);
        if let TerminalFrameEvent::Output(frame) = &mut effect {
            frame.effects.bell = true;
        }
        let mut events = VecDeque::from([
            output_frame("a", 1),
            output_frame("a", 2),
            effect,
            output_frame("a", 4),
        ]);

        let (frames, coalesced) = pop_terminal_frame_events_for_apply(&mut events, &[], true);

        assert_eq!(coalesced, 1);
        assert!(events.is_empty());
        assert_eq!(frames.len(), 3);
        assert!(matches!(
            &frames[0],
            TerminalFrameEvent::Output(frame) if frame.revision == 2
        ));
        assert!(matches!(
            &frames[1],
            TerminalFrameEvent::Output(frame) if frame.revision == 3 && frame.effects.bell
        ));
        assert!(matches!(
            &frames[2],
            TerminalFrameEvent::Output(frame) if frame.revision == 4
        ));
    }

    #[test]
    fn terminal_frame_apply_does_not_coalesce_across_non_output_events() {
        let mut events = VecDeque::from([
            output_frame("a", 1),
            search_frame("a"),
            output_frame("a", 2),
        ]);

        let (first, first_coalesced) = pop_terminal_frame_events_for_apply(&mut events, &[], true);
        let (second, second_coalesced) =
            pop_terminal_frame_events_for_apply(&mut events, &[], true);
        let (third, third_coalesced) = pop_terminal_frame_events_for_apply(&mut events, &[], true);

        assert_eq!(first_coalesced, 0);
        assert!(matches!(
            &first[0],
            TerminalFrameEvent::Output(frame) if frame.revision == 1
        ));
        assert_eq!(second_coalesced, 0);
        assert!(matches!(second[0], TerminalFrameEvent::Search(_)));
        assert_eq!(third_coalesced, 0);
        assert!(matches!(
            &third[0],
            TerminalFrameEvent::Output(frame) if frame.revision == 2
        ));
        assert!(events.is_empty());
    }

    #[test]
    fn terminal_frame_apply_skips_deferred_events_under_output_pressure() {
        let mut events = VecDeque::from([
            search_frame("a"),
            output_frame("a", 1),
            output_frame("a", 2),
        ]);

        let (frames, coalesced) = pop_terminal_frame_events_for_apply(&mut events, &[], false);

        assert_eq!(coalesced, 1);
        assert_eq!(frames.len(), 1);
        assert!(matches!(
            &frames[0],
            TerminalFrameEvent::Output(frame) if frame.revision == 2
        ));
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events.front(),
            Some(TerminalFrameEvent::Search(_))
        ));

        let (pressure_frames, _) = pop_terminal_frame_events_for_apply(&mut events, &[], false);
        assert!(pressure_frames.is_empty());
        assert_eq!(events.len(), 1);

        let (idle_frames, _) = pop_terminal_frame_events_for_apply(&mut events, &[], true);
        assert!(matches!(
            idle_frames.first(),
            Some(TerminalFrameEvent::Search(_))
        ));
        assert!(events.is_empty());
    }

    #[test]
    fn terminal_frame_apply_keeps_snapshots_critical_under_output_pressure() {
        let mut events = VecDeque::from([
            search_frame("a"),
            snapshot_frame("a", 4),
            output_frame("a", 1),
        ]);

        let (frames, coalesced) = pop_terminal_frame_events_for_apply(&mut events, &[], false);

        assert_eq!(coalesced, 0);
        assert_eq!(frames.len(), 1);
        assert!(matches!(
            &frames[0],
            TerminalFrameEvent::Snapshot(frame) if frame.offset == 4
        ));
        assert_eq!(events.len(), 2);
        assert!(matches!(
            events.front(),
            Some(TerminalFrameEvent::Search(_))
        ));

        let (frames, coalesced) = pop_terminal_frame_events_for_apply(&mut events, &[], false);

        assert_eq!(coalesced, 0);
        assert_eq!(frames.len(), 1);
        assert!(matches!(
            &frames[0],
            TerminalFrameEvent::Output(frame) if frame.revision == 1
        ));
        assert!(matches!(
            events.front(),
            Some(TerminalFrameEvent::Search(_))
        ));
    }

    #[test]
    fn terminal_surface_notify_sessions_are_unique_per_drain() {
        let mut sessions = Vec::new();

        push_unique_terminal_surface_session(&mut sessions, "a".to_string());
        push_unique_terminal_surface_session(&mut sessions, "a".to_string());
        push_unique_terminal_surface_session(&mut sessions, "b".to_string());

        assert_eq!(sessions, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn terminal_surface_notify_prefers_full_paint_over_scroll_only() {
        let mut full_sessions = Vec::new();
        let mut scroll_sessions = Vec::new();

        push_unique_terminal_surface_notify(
            &mut full_sessions,
            &mut scroll_sessions,
            TerminalSurfaceFrameNotify::ScrollPositionOnly("a".to_string()),
        );
        push_unique_terminal_surface_notify(
            &mut full_sessions,
            &mut scroll_sessions,
            TerminalSurfaceFrameNotify::Full("a".to_string()),
        );
        push_unique_terminal_surface_notify(
            &mut full_sessions,
            &mut scroll_sessions,
            TerminalSurfaceFrameNotify::ScrollPositionOnly("a".to_string()),
        );
        push_unique_terminal_surface_notify(
            &mut full_sessions,
            &mut scroll_sessions,
            TerminalSurfaceFrameNotify::ScrollPositionOnly("b".to_string()),
        );

        assert_eq!(full_sessions, vec!["a".to_string()]);
        assert_eq!(scroll_sessions, vec!["b".to_string()]);
    }

    #[test]
    fn terminal_frame_deferred_events_wait_for_output_backlog() {
        assert!(terminal_frame_deferred_events_can_apply(false, 0, 0, 0, 0));
        assert!(!terminal_frame_deferred_events_can_apply(true, 0, 0, 0, 0));
        assert!(!terminal_frame_deferred_events_can_apply(false, 1, 0, 0, 0));
        assert!(!terminal_frame_deferred_events_can_apply(false, 0, 1, 0, 0));
        assert!(!terminal_frame_deferred_events_can_apply(false, 0, 0, 1, 0));
        assert!(!terminal_frame_deferred_events_can_apply(false, 0, 0, 0, 1));
    }
}

const TERMINAL_FRAME_EVENT_DRAIN_BATCH: usize = 64;
const TERMINAL_FRAME_EVENT_DRAIN_WALL_BUDGET: Duration = Duration::from_millis(4);
const TERMINAL_FRAME_INPUT_WAKE_EVENT_DRAIN_BATCH: usize = 8;
const TERMINAL_FRAME_INPUT_WAKE_EVENT_DRAIN_WALL_BUDGET: Duration = Duration::from_millis(1);
const TERMINAL_FRAME_EVENT_DRAIN_SLOW_TOTAL: Duration = Duration::from_millis(12);
const TERMINAL_FRAME_EVENT_APPLY_SLOW: Duration = Duration::from_millis(8);
const TERMINAL_FRAME_APPLY_VISIBLE_TEXT_TAIL_CAP: usize = 16 * 1024;

fn terminal_local_log_text(text: &str) -> Cow<'_, str> {
    if !text.chars().any(terminal_local_log_control_needs_escape) {
        return Cow::Borrowed(text);
    }

    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\n' | '\r' | '\t' => out.push(ch),
            '\x1b' => out.push_str("\\x1b"),
            ch if ch.is_control() => {
                let _ = write!(out, "\\u{{{:x}}}", ch as u32);
            }
            ch => out.push(ch),
        }
    }
    Cow::Owned(out)
}

fn terminal_local_log_control_needs_escape(ch: char) -> bool {
    ch.is_control() && !matches!(ch, '\n' | '\r' | '\t')
}

fn limit_osc52_clipboard_reply_text(text: &str) -> std::borrow::Cow<'_, str> {
    match text.char_indices().nth(MAX_OSC52_REPLY_CHARS) {
        Some((boundary, _)) => std::borrow::Cow::Owned(text[..boundary].to_string()),
        None => std::borrow::Cow::Borrowed(text),
    }
}

fn queue_osc52_clipboard_load_replies(
    clipboard_loads: &mut Vec<TerminalClipboardLoad>,
    clipboard_text: &str,
    pending_pty_writes: &mut Vec<Vec<u8>>,
) {
    let clipboard_text = limit_osc52_clipboard_reply_text(clipboard_text);
    for formatter in clipboard_loads.drain(..) {
        let reply = formatter(clipboard_text.as_ref());
        if !reply.is_empty() {
            pending_pty_writes.push(reply.into_bytes());
        }
    }
}

fn terminal_effects_need_ui_apply(effects: &TerminalEffects) -> bool {
    effects.bell
        || effects.title.is_some()
        || effects.reset_title
        || effects.cwd.is_some()
        || effects.shell_command_started
        || effects.shell_command_finished
        || !effects.pty_write.is_empty()
        || effects.clipboard_store.is_some()
        || !effects.clipboard_loads.is_empty()
}

fn terminal_output_frame_surface_notify(
    is_visible: bool,
    scroll_offset: usize,
    accepted_bytes: usize,
) -> Option<TerminalSurfaceFrameNotify> {
    if !is_visible || accepted_bytes == 0 {
        return None;
    }
    if scroll_offset > 0 {
        Some(TerminalSurfaceFrameNotify::ScrollPositionOnly(String::new()))
    } else {
        Some(TerminalSurfaceFrameNotify::Full(String::new()))
    }
}

fn terminal_output_frame_needs_chrome_notify(
    unread_changed: bool,
    effects_need_ui_apply: bool,
) -> bool {
    // Chrome-level notify only: unread badges / effects that change shell chrome.
    // Visible grid updates are handled by TerminalSurface entity notify.
    unread_changed || effects_need_ui_apply
}

fn terminal_search_frame_apply_result(
    session_id: String,
    is_current_revision: bool,
    is_visible: bool,
    active_session_id: Option<&str>,
    search_open: bool,
    search_mode: TerminalSearchMode,
    keys: TerminalFrameSearchKeys<'_>,
) -> TerminalFrameApplyResult {
    let updates_active_buffer_search = is_current_revision
        && is_visible
        && search_open
        && search_mode == TerminalSearchMode::Buffer
        && active_session_id == Some(session_id.as_str())
        && keys.current == Some(keys.result);
    TerminalFrameApplyResult {
        // Search-bar count/status is shell chrome. Terminal highlights are owned
        // by the surface entity, so notify both only for the visible active
        // query instead of letting background/stale search frames repaint the
        // whole app.
        chrome_dirty: updates_active_buffer_search,
        surface_notify: updates_active_buffer_search
            .then_some(TerminalSurfaceFrameNotify::Full(session_id)),
    }
}

fn terminal_selected_occurrence_frame_is_current(
    current_session_id: Option<&str>,
    current_query: Option<&str>,
    pending_key: Option<&TerminalFrameSearchKey>,
    pending_visible_key: Option<&TerminalFrameSearchKey>,
    frame_session_id: &str,
    purpose: TerminalFrameSearchPurpose,
    result_key: &TerminalFrameSearchKey,
) -> bool {
    let pending_matches = match purpose {
        TerminalFrameSearchPurpose::SelectedOccurrenceVisible { .. } => {
            pending_visible_key == Some(result_key)
        }
        TerminalFrameSearchPurpose::SelectedOccurrence => pending_key == Some(result_key),
        TerminalFrameSearchPurpose::Find => false,
    };
    current_session_id == Some(frame_session_id)
        && current_query == Some(result_key.query.as_str())
        && pending_matches
}

fn terminal_apply_search_result_to_view(
    view: &mut TerminalViewState,
    purpose: TerminalFrameSearchPurpose,
    result: &crate::models::TerminalFrameSearchResult,
    selected_occurrence_is_current: bool,
) -> bool {
    match purpose {
        TerminalFrameSearchPurpose::Find => {
            if view.pending_search_key.as_ref() == Some(&result.key) {
                view.pending_search_key = None;
            }
        }
        TerminalFrameSearchPurpose::SelectedOccurrenceVisible { .. } => {
            if view.pending_selected_occurrence_visible_key.as_ref() == Some(&result.key) {
                view.pending_selected_occurrence_visible_key = None;
            }
        }
        TerminalFrameSearchPurpose::SelectedOccurrence => {
            if view.pending_selected_occurrence_key.as_ref() == Some(&result.key) {
                view.pending_selected_occurrence_key = None;
            }
        }
    }
    if result.revision != view.screen_revision
        || (matches!(
            purpose,
            TerminalFrameSearchPurpose::SelectedOccurrenceVisible { .. }
                | TerminalFrameSearchPurpose::SelectedOccurrence
        ) && !selected_occurrence_is_current)
    {
        return false;
    }
    match purpose {
        TerminalFrameSearchPurpose::Find => view.search_result = Some(result.clone()),
        TerminalFrameSearchPurpose::SelectedOccurrenceVisible { .. } => {
            view.selected_occurrence_visible_result = Some(result.clone())
        }
        TerminalFrameSearchPurpose::SelectedOccurrence => {
            view.selected_occurrence_result = Some(result.clone())
        }
    }
    true
}

struct TerminalFrameSearchKeys<'a> {
    current: Option<&'a TerminalFrameSearchKey>,
    result: &'a TerminalFrameSearchKey,
}

fn terminal_window_node_visible_tab_ids(root: &TerminalWindowNode) -> Vec<&str> {
    let mut ids = Vec::new();
    collect_terminal_window_node_visible_tab_ids(root, &mut ids);
    ids
}

fn collect_terminal_window_node_visible_tab_ids<'a>(
    node: &'a TerminalWindowNode,
    ids: &mut Vec<&'a str>,
) {
    match node {
        TerminalWindowNode::Leaf { active_tab_id, .. } => {
            if let Some(id) = active_tab_id.as_deref() {
                ids.push(id);
            }
        }
        TerminalWindowNode::Split { first, second, .. } => {
            collect_terminal_window_node_visible_tab_ids(first, ids);
            collect_terminal_window_node_visible_tab_ids(second, ids);
        }
    }
}

#[cfg(test)]
mod tests;
