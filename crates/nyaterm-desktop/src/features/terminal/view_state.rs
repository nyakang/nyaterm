//! Cross-domain projections and lifecycle transitions for terminal views.

use std::collections::hash_map::Entry;
use std::time::Instant;

use futures::channel::mpsc::UnboundedReceiver;

use super::state::TerminalFeatureState;
use crate::models::TerminalFrameEvent;
use crate::models::TerminalSelection;
use crate::models::{TerminalFrameSession, TerminalViewState};

pub(in crate::features) struct TerminalSessionTransferBundle {
    entries: Vec<TerminalSessionTransferEntry>,
    pending_events: std::collections::VecDeque<TerminalFrameEvent>,
}

struct TerminalSessionTransferEntry {
    session_id: String,
    frame: Option<TerminalFrameSession>,
    view: Option<TerminalViewState>,
    search_wrap: Option<bool>,
    scroll_residual: Option<f32>,
    selection: Option<TerminalSelection>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::features) struct TerminalFrameQueueMetrics {
    pub command_count: usize,
    pub output_bytes: usize,
    pub event_count: usize,
    pub event_wake_count: u64,
    pub pending_event_count: usize,
}

impl TerminalFeatureState {
    pub(in crate::features) fn retains_transfer_session(&self, id: &str) -> bool {
        self.view.views.contains_key(id)
            || self.view.surfaces.contains_key(id)
            || self.view.scroll_delta_residuals.contains_key(id)
            || self.search.wrap_around_by_session.contains_key(id)
            || self.layout.session_surface_bounds.contains_key(id)
            || self.layout.session_scrollbar_track_bounds.contains_key(id)
            || self.selection.session_id.as_deref() == Some(id)
    }

    pub(in crate::features) fn prepare_sessions_for_transfer(
        &self,
        session_ids: &[String],
    ) -> Result<Vec<(String, Option<TerminalFrameSession>)>, &'static str> {
        let mut frames = Vec::with_capacity(session_ids.len());
        for session_id in session_ids {
            match self
                .view
                .frame_pipeline
                .take_session_for_transfer(session_id.clone())
            {
                Ok(frame) => frames.push((session_id.clone(), frame)),
                Err(error) => {
                    let rollback = frames
                        .into_iter()
                        .filter_map(|(id, frame)| frame.map(|frame| (id, frame)))
                        .collect();
                    let _ = self
                        .view
                        .frame_pipeline
                        .insert_sessions_from_transfer(rollback);
                    return Err(error);
                }
            }
        }
        Ok(frames)
    }

    pub(in crate::features) fn detach_sessions_for_transfer(
        &mut self,
        frames: Vec<(String, Option<TerminalFrameSession>)>,
    ) -> TerminalSessionTransferBundle {
        let session_ids = frames
            .iter()
            .map(|(session_id, _)| session_id.clone())
            .collect::<std::collections::HashSet<_>>();
        let mut pending_events = std::collections::VecDeque::new();
        self.view.pending_frame_events.retain(|event| {
            if session_ids.contains(event.session_id()) {
                pending_events.push_back(event.clone());
                false
            } else {
                true
            }
        });
        let selected = frames.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>();
        pending_events.extend(self.view.frame_pipeline.take_events_for_transfer(&selected));
        let entries = frames
            .into_iter()
            .map(|(session_id, frame)| {
                self.view.surfaces.remove(&session_id);
                self.layout.session_surface_bounds.remove(&session_id);
                self.layout
                    .session_scrollbar_track_bounds
                    .remove(&session_id);
                let selection = if self.selection.session_id.as_deref() == Some(&session_id) {
                    self.selection.session_id = None;
                    self.selection.dragging = false;
                    self.selection.selection.take()
                } else {
                    None
                };
                TerminalSessionTransferEntry {
                    view: self.view.views.remove(&session_id),
                    search_wrap: self.search.wrap_around_by_session.remove(&session_id),
                    scroll_residual: self.view.scroll_delta_residuals.remove(&session_id),
                    selection,
                    session_id,
                    frame,
                }
            })
            .collect();
        self.layout.surface_bounds = None;
        self.layout.scrollbar_track_bounds = None;
        self.view.scrollbar_drag = None;
        if self
            .selection
            .selected_occurrence
            .session_id
            .as_ref()
            .is_some_and(|id| session_ids.contains(id))
        {
            self.selection.selected_occurrence.session_id = None;
            self.selection.selected_occurrence.query = None;
            self.selection.selected_occurrence.generation = self
                .selection
                .selected_occurrence
                .generation
                .wrapping_add(1);
        }
        if self
            .selection
            .mouse_report_session_id
            .as_deref()
            .is_some_and(|id| session_ids.contains(id))
        {
            self.selection.mouse_report_session_id = None;
            self.selection.mouse_report_button = None;
            self.selection.mouse_report_position = None;
        }
        self.selection
            .mouse_report_peer_session_ids
            .retain(|id| !session_ids.contains(id));
        TerminalSessionTransferBundle {
            entries,
            pending_events,
        }
    }

    pub(in crate::features) fn attach_sessions_from_transfer(
        &mut self,
        mut bundle: TerminalSessionTransferBundle,
    ) -> Result<(), TerminalSessionTransferBundle> {
        let frames = bundle
            .entries
            .iter_mut()
            .filter_map(|entry| {
                entry
                    .frame
                    .take()
                    .map(|frame| (entry.session_id.clone(), frame))
            })
            .collect();
        if let Err(frames) = self
            .view
            .frame_pipeline
            .insert_sessions_from_transfer(frames)
        {
            let mut frames = frames
                .into_iter()
                .collect::<std::collections::HashMap<_, _>>();
            for entry in &mut bundle.entries {
                entry.frame = frames.remove(&entry.session_id);
            }
            return Err(bundle);
        }
        for entry in bundle.entries {
            if let Some(wrap) = entry.search_wrap {
                self.search
                    .wrap_around_by_session
                    .insert(entry.session_id.clone(), wrap);
            }
            if let Some(residual) = entry.scroll_residual {
                self.view
                    .scroll_delta_residuals
                    .insert(entry.session_id.clone(), residual);
            }
            if let Some(selection) = entry.selection
                && self.selection.selection.is_none()
            {
                self.selection.session_id = Some(entry.session_id.clone());
                self.selection.selection = Some(selection);
            }
            if let Some(view) = entry.view {
                self.view.views.insert(entry.session_id, view);
            }
        }
        self.view.pending_frame_events.extend(bundle.pending_events);
        Ok(())
    }

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

    pub(in crate::features) fn request_session_rekey(
        &self,
        old_id: &str,
        new_id: &str,
        encoding: &str,
        scrollback_limit: usize,
    ) -> bool {
        self.view.frame_pipeline.rekey_session(
            old_id.to_string(),
            new_id.to_string(),
            encoding.to_string(),
            scrollback_limit,
        )
    }

    pub(in crate::features) fn seed_session_view(
        &mut self,
        session_id: String,
        output: String,
        encoding: &str,
    ) {
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
            Entry::Occupied(mut entry) => {
                entry.get_mut().reset_reconnect_stream(encoding);
                entry.get_mut().append_text(text);
            }
            Entry::Vacant(entry) => {
                let mut view = TerminalViewState::new();
                view.set_encoding(encoding);
                view.append_text(text);
                entry.insert(view);
            }
        }
        self.view
            .frame_pipeline
            .append_local_text(session_id.to_string(), text.to_string());
    }

    pub(in crate::features) fn append_existing_session_text(
        &mut self,
        session_id: &str,
        text: &str,
    ) {
        if let Some(view) = self.view.views.get_mut(session_id) {
            view.screen.reset_stream_state();
            view.output_decoder.reset_decoder();
            view.recording_decoder.reset_decoder();
            view.append_text(text);
            self.view
                .frame_pipeline
                .append_local_text(session_id.to_string(), text.to_string());
        }
    }

    pub(in crate::features) fn rekey_session_view(
        &mut self,
        old_id: &str,
        new_id: &str,
        encoding: &str,
    ) {
        self.view.retired_session_ids.push_back(old_id.to_string());
        if self.view.retired_session_ids.len() > 256 {
            self.view.retired_session_ids.pop_front();
        }
        if let Some(mut view) = self.view.views.remove(old_id) {
            view.reset_reconnect_stream(encoding);
            self.view.views.insert(new_id.to_string(), view);
        }
        if let Some(residual) = self.view.scroll_delta_residuals.remove(old_id) {
            self.view
                .scroll_delta_residuals
                .insert(new_id.to_string(), residual);
        }
        if self.selection.session_id.as_deref() == Some(old_id) {
            self.selection.session_id = Some(new_id.to_string());
        }
        if self.selection.selected_occurrence.session_id.as_deref() == Some(old_id) {
            self.selection.selected_occurrence.session_id = Some(new_id.to_string());
        }
        self.view
            .pending_frame_events
            .retain(|event| event.session_id() != old_id);
        self.view
            .frame_pipeline
            .take_events_for_transfer(&[old_id.to_string()]);
    }

    pub(in crate::features) fn session_id_is_retired(&self, session_id: &str) -> bool {
        self.view
            .retired_session_ids
            .iter()
            .any(|id| id == session_id)
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
