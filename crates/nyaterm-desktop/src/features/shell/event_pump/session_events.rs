use std::time::{Duration, Instant};

use gpui::Context;
use nyaterm_transport::SessionEvent;

use crate::features::shell::event_pump::helpers::{
    SESSION_EVENT_DRAIN_SLOW_CHUNK, SessionEventDrainBudget, SessionEventDrainTimings,
    connect_settle_active, session_event_backlog_active, session_event_drain_budget,
    session_event_drain_is_slow, session_event_drain_should_yield,
    session_event_input_wake_drain_budget, terminal_frame_backlog_active_from_counts,
    terminal_log_plain_text, terminal_output_dropped_marker,
};
use crate::features::{NyaTermApp, formatting::short_id};

#[derive(Clone, Copy)]
enum SessionOutputDrainStep {
    SidebandOnly {
        chunk_duration: Duration,
        root_chrome_dirty: bool,
    },
    Accepted {
        chunk_duration: Duration,
        root_chrome_dirty: bool,
    },
}

impl SessionOutputDrainStep {
    fn chunk_duration(self) -> Duration {
        match self {
            Self::SidebandOnly { chunk_duration, .. } | Self::Accepted { chunk_duration, .. } => {
                chunk_duration
            }
        }
    }

    fn root_chrome_dirty(self) -> bool {
        match self {
            Self::SidebandOnly {
                root_chrome_dirty, ..
            }
            | Self::Accepted {
                root_chrome_dirty, ..
            } => root_chrome_dirty,
        }
    }
}

fn terminal_output_has_error_keyword(output: &str) -> bool {
    let lower = output.to_ascii_lowercase();
    [
        "permission denied",
        "no space left on device",
        "connection refused",
        "segmentation fault",
        "out of memory",
        "cannot allocate memory",
        "command not found",
        "module not found",
        "port already in use",
    ]
    .iter()
    .any(|keyword| lower.contains(keyword))
        || ascii_word_present(&lower, "error")
        || ascii_word_present(&lower, "failed")
}

fn ascii_word_present(text: &str, word: &str) -> bool {
    text.match_indices(word).any(|(index, _)| {
        let before = index
            .checked_sub(1)
            .and_then(|before| text.as_bytes().get(before))
            .copied();
        let after = text.as_bytes().get(index + word.len()).copied();
        !before.is_some_and(|value| value.is_ascii_alphanumeric() || value == b'_')
            && !after.is_some_and(|value| value.is_ascii_alphanumeric() || value == b'_')
    })
}

fn terminal_error_notice_output(output: &str) -> String {
    const LIMIT: usize = 4_000;
    let char_count = output.chars().count();
    if char_count <= LIMIT {
        return output.to_string();
    }
    output
        .chars()
        .skip(char_count.saturating_sub(LIMIT))
        .collect()
}

impl NyaTermApp {
    pub(in crate::features) fn drain_session_events(&mut self, cx: &mut Context<Self>) -> bool {
        let settle = connect_settle_active(self.shell.runtime.connect_settle_until, Instant::now());
        let mut drain_budget =
            session_event_drain_budget(self.runtime_output_pressure_active() || settle);
        if settle {
            // First frames after connect: smaller wall budget leaves room for paint.
            drain_budget.wall_budget = Duration::from_millis(4);
            drain_budget.max_output_bytes = drain_budget.max_output_bytes.min(4 * 1024);
        }
        self.drain_session_events_with_budget(cx, drain_budget, true)
    }

    pub(in crate::features) fn drain_session_events_for_input_wake(
        &mut self,
        cx: &mut Context<Self>,
    ) -> bool {
        self.drain_session_events_with_budget(cx, session_event_input_wake_drain_budget(), false)
    }

    fn drain_session_events_with_budget(
        &mut self,
        cx: &mut Context<Self>,
        drain_budget: SessionEventDrainBudget,
        drain_sideband_workers: bool,
    ) -> bool {
        let drain_started_at = Instant::now();
        let mut root_chrome_dirty = false;
        // Common calm path: no local pending events, no bridge UI work, no file
        // transfer sideband. Skip harvest/atomics so idle and window-drag ticks
        // do not touch the session event pipeline at all.
        if self.session.pending_events_are_empty()
            && !self.session.event_bridge_has_pending_ui_work()
            && (!drain_sideband_workers || !self.session.has_protocol_runtime_sessions())
        {
            if self.shell.runtime.session_event_queued_events != 0
                || self.shell.runtime.session_event_queued_output_bytes != 0
                || self.shell.runtime.session_event_backlog_active
                || self.shell.runtime.session_event_last_output_event_count != 0
                || self.shell.runtime.session_event_last_drained_output_bytes != 0
            {
                self.shell.runtime.session_event_queued_events = 0;
                self.shell.runtime.session_event_queued_output_bytes = 0;
                self.shell.runtime.session_event_backlog_active = false;
                self.shell.runtime.session_event_last_output_event_count = 0;
                self.shell.runtime.session_event_last_drained_output_bytes = 0;
            }
            return false;
        }
        // Bridge encoding/scrollback and per-session routing are updated on the
        // state transitions that need them, not on every runtime tick.
        if drain_sideband_workers {
            root_chrome_dirty |= self.drain_xymodem_worker_events(cx);
            root_chrome_dirty |= self.drain_zmodem_worker_events(cx);
            root_chrome_dirty |= self.drain_trzsz_download_worker_events(cx);
            root_chrome_dirty |= self.drain_trzsz_upload_prepare_events(cx);
            root_chrome_dirty |= self.drain_trzsz_upload_worker_events(cx);
        }
        let mut drained_events = 0usize;
        let mut output_event_count = 0usize;
        let mut drain_timings = SessionEventDrainTimings::default();
        let mut max_output_chunk_duration = Duration::ZERO;
        let mut processed_output_bytes = 0usize;
        let mut transport_queued_events = 0usize;
        let mut transport_queued_output_bytes = 0usize;
        let mut bridge_direct_output_events = 0u64;
        let mut bridge_direct_output_bytes = 0u64;
        let mut bridge_direct_backpressure_events = 0u64;
        let mut bridge_direct_backpressure_bytes = 0u64;
        let mut bridge_drained_ui_events = 0usize;
        let mut bridge_drained_ui_output_bytes = 0usize;
        let mut pending_frame_outputs: Vec<(String, Vec<u8>)> = Vec::new();
        if self.session.pending_events_are_empty() {
            if self.session.event_bridge_has_pending_ui_work() {
                let drain = self
                    .session
                    .drain_event_bridge(drain_budget.max_events, drain_budget.max_output_bytes);
                transport_queued_events = drain
                    .stats
                    .source_queued_events
                    .saturating_add(drain.stats.ui_queued_events);
                transport_queued_output_bytes = drain
                    .stats
                    .source_queued_output_bytes
                    .saturating_add(drain.stats.ui_queued_output_bytes);
                bridge_direct_output_events = drain.stats.direct_output_events;
                bridge_direct_output_bytes = drain.stats.direct_output_bytes;
                bridge_direct_backpressure_events = drain.stats.direct_backpressure_events;
                bridge_direct_backpressure_bytes = drain.stats.direct_backpressure_bytes;
                bridge_drained_ui_events = drain.stats.drained_ui_events;
                bridge_drained_ui_output_bytes = drain.stats.drained_ui_output_bytes;
                if drain.stats.dropped_output_bytes > 0 {
                    self.shell.runtime.session_event_dropped_output_bytes = self
                        .shell
                        .runtime
                        .session_event_dropped_output_bytes
                        .saturating_add(drain.stats.dropped_output_bytes as u64);
                }
                self.session.extend_pending_events(drain.events);
            } else {
                // Direct-output-only ticks: harvest counters without UI queue lock.
                let stats = self.session.harvest_event_bridge_stats();
                transport_queued_events = stats
                    .source_queued_events
                    .saturating_add(stats.ui_queued_events);
                transport_queued_output_bytes = stats
                    .source_queued_output_bytes
                    .saturating_add(stats.ui_queued_output_bytes);
                bridge_direct_output_events = stats.direct_output_events;
                bridge_direct_output_bytes = stats.direct_output_bytes;
                bridge_direct_backpressure_events = stats.direct_backpressure_events;
                bridge_direct_backpressure_bytes = stats.direct_backpressure_bytes;
            }
        }

        if !self.session.pending_events_are_empty() {
            while let Some(event) = self.session.pop_pending_event() {
                drained_events += 1;
                match event {
                    SessionEvent::Output { session_id, data } => {
                        output_event_count += 1;
                        let chunk_input_bytes = data.len();
                        processed_output_bytes =
                            processed_output_bytes.saturating_add(chunk_input_bytes);
                        let step = self.handle_session_output_event(
                            session_id,
                            data,
                            &mut pending_frame_outputs,
                            &mut drain_timings,
                            cx,
                        );
                        max_output_chunk_duration =
                            max_output_chunk_duration.max(step.chunk_duration());
                        root_chrome_dirty |= step.root_chrome_dirty();
                        if matches!(step, SessionOutputDrainStep::SidebandOnly { .. })
                            && session_event_drain_should_yield(
                                drain_started_at,
                                !self.session.pending_events_are_empty(),
                                transport_queued_events,
                                transport_queued_output_bytes,
                                drain_budget,
                            )
                        {
                            break;
                        }
                    }
                    SessionEvent::OutputDropped { session_id, bytes } => {
                        self.flush_pending_session_frame_outputs(
                            &mut pending_frame_outputs,
                            &mut drain_timings,
                        );
                        root_chrome_dirty |=
                            self.handle_session_output_dropped_event(session_id, bytes, cx);
                    }
                    SessionEvent::CwdChanged { session_id, cwd } => {
                        self.flush_pending_session_frame_outputs(
                            &mut pending_frame_outputs,
                            &mut drain_timings,
                        );
                        if self.apply_session_cwd(&session_id, cwd) {
                            self.defer_transfer_panel_snapshot_flush(cx);
                        }
                    }
                    SessionEvent::CommandAccepted {
                        session_id,
                        command,
                    } => {
                        self.flush_pending_session_frame_outputs(
                            &mut pending_frame_outputs,
                            &mut drain_timings,
                        );
                        self.session.record_command_history(&session_id, &command);
                        if !self.commands.queue_command_history(vec![command]) {
                            self.settings.update_store_status(
                                "command history worker is unavailable",
                                false,
                            );
                            self.request_settings_panel_refresh(cx);
                        }
                    }
                    SessionEvent::Exited { session_id, reason } => {
                        self.flush_pending_session_frame_outputs(
                            &mut pending_frame_outputs,
                            &mut drain_timings,
                        );
                        root_chrome_dirty |=
                            self.handle_session_exited_event(session_id, reason, cx);
                    }
                    SessionEvent::Error {
                        session_id,
                        message,
                    } => {
                        self.flush_pending_session_frame_outputs(
                            &mut pending_frame_outputs,
                            &mut drain_timings,
                        );
                        root_chrome_dirty |= self.handle_session_error_event(session_id, message);
                    }
                }
                if session_event_drain_should_yield(
                    drain_started_at,
                    !self.session.pending_events_are_empty(),
                    transport_queued_events,
                    transport_queued_output_bytes,
                    drain_budget,
                ) {
                    break;
                }
            }
        }
        self.flush_pending_session_frame_outputs(&mut pending_frame_outputs, &mut drain_timings);

        let queued_events =
            transport_queued_events.saturating_add(self.session.pending_event_count());
        let queued_output_bytes =
            transport_queued_output_bytes.saturating_add(self.session.pending_event_output_bytes());
        let drained_output_bytes = processed_output_bytes;
        self.shell.runtime.session_event_queued_events = queued_events;
        self.shell.runtime.session_event_queued_output_bytes = queued_output_bytes;
        self.shell.runtime.session_event_last_output_event_count = output_event_count;
        self.shell.runtime.session_event_last_drained_output_bytes = drained_output_bytes;

        self.shell.runtime.session_event_backlog_active = session_event_backlog_active(
            drained_events,
            drained_output_bytes,
            queued_output_bytes,
            drain_budget,
        );

        if session_event_drain_is_slow(drain_timings.output_total, max_output_chunk_duration)
            && self.should_log_slow_diagnostic("session_event_drain", Instant::now())
        {
            tracing::warn!(
                diagnostic = "session_event_drain",
                drained_events,
                output_event_count,
                drained_output_bytes,
                drain_output_budget = drain_budget.max_output_bytes,
                queued_events,
                queued_output_bytes,
                bridge_direct_output_events,
                bridge_direct_output_bytes,
                bridge_direct_backpressure_events,
                bridge_direct_backpressure_bytes,
                bridge_drained_ui_events,
                bridge_drained_ui_output_bytes,
                dropped_output_bytes = self.shell.runtime.session_event_dropped_output_bytes,
                drain_total_ms = drain_started_at.elapsed().as_millis(),
                output_total_ms = drain_timings.output_total.as_millis(),
                max_output_chunk_ms = max_output_chunk_duration.as_millis(),
                zmodem_us = drain_timings.zmodem.as_micros(),
                trzsz_us = drain_timings.trzsz.as_micros(),
                decode_us = drain_timings.decode.as_micros(),
                recording_us = drain_timings.recording.as_micros(),
                terminal_append_us = drain_timings.terminal_append.as_micros(),
                credential_autofill_us = drain_timings.credential_autofill.as_micros(),
                ai_capture_us = drain_timings.ai_capture.as_micros(),
                "slow session event drain"
            );
        }
        root_chrome_dirty
    }

    fn handle_session_output_dropped_event(
        &mut self,
        session_id: String,
        bytes: usize,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.terminal.session_id_is_retired(&session_id) {
            return false;
        }
        self.note_trzsz_output_discontinuity(&session_id);
        self.cancel_xymodem_transfer(&session_id);
        let mut root_chrome_dirty = self.note_zmodem_output_discontinuity(&session_id, bytes, cx);
        self.note_ai_agent_output_discontinuity(&session_id, bytes, cx);
        self.session.route_session_events_to_ui(&session_id);
        let encoding = self.settings.summary().interaction_default_encoding.clone();
        self.terminal
            .note_session_output_discontinuity(session_id.clone(), &encoding, bytes);
        let marker = terminal_output_dropped_marker(bytes);
        self.recording
            .write_output(session_id.clone(), marker.clone());
        self.append_terminal_log_for_session(Some(&session_id), &marker, true);
        if self.session.active_id() == Some(session_id.as_str()) {
            self.shell.set_status(format!(
                "terminal output overloaded; dropped {} queued byte(s)",
                bytes
            ));
            root_chrome_dirty = true;
        }
        root_chrome_dirty
    }

    fn handle_session_exited_event(
        &mut self,
        session_id: String,
        reason: String,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.terminal.session_id_is_retired(&session_id) {
            return false;
        }
        let known_session = self.session.has_session(&session_id);
        tracing::warn!(
            diagnostic = "session_exited",
            session_id = %session_id,
            reason = %reason,
            known_session,
            "session exited or disconnected"
        );
        let log_reason = terminal_log_plain_text(&reason);
        let log = format!("\n# session disconnected: {log_reason}\n");
        if known_session {
            self.recording.write_output(session_id.clone(), log.clone());
            self.append_terminal_log_for_session(Some(&session_id), &log, true);
        }
        self.clear_trzsz_session(&session_id);
        self.clear_xymodem_session(&session_id);
        self.clear_zmodem_session(&session_id);
        self.session.clear_event_bridge_session(&session_id);
        self.cleanup_recording_for_session(&session_id);
        let _ = self.session.manager().close(&session_id);
        if known_session {
            // Keep the tab so the user can reconnect (Tauri disconnected pane).
            self.mark_session_disconnected(&session_id, cx);
            self.shell
                .set_status(format!("session disconnected {}", short_id(&session_id)));
        } else {
            self.shell
                .set_status(format!("session exited {}", short_id(&session_id)));
        }
        true
    }

    fn handle_session_error_event(&mut self, session_id: String, message: String) -> bool {
        if self.terminal.session_id_is_retired(&session_id) {
            return false;
        }
        tracing::warn!(
            diagnostic = "session_error",
            session_id = %session_id,
            message = %message,
            "session error"
        );
        let log_message = terminal_log_plain_text(&message);
        let log = format!("\n# session error: {log_message}\n");
        if !session_id.is_empty() {
            self.sync_session_event_bridge_session_policy(&session_id);
            self.recording.write_output(session_id.clone(), log.clone());
        }
        if session_id.is_empty() || self.session.active_id() == Some(session_id.as_str()) {
            self.shell.set_status(format!("session error: {message}"));
            self.append_terminal_log(log);
        } else {
            self.append_terminal_log_for_session(Some(&session_id), &log, true);
        }
        true
    }

    fn handle_session_output_event(
        &mut self,
        session_id: String,
        data: Vec<u8>,
        pending_frame_outputs: &mut Vec<(String, Vec<u8>)>,
        drain_timings: &mut SessionEventDrainTimings,
        cx: &mut Context<Self>,
    ) -> SessionOutputDrainStep {
        if self.terminal.session_id_is_retired(&session_id) {
            return SessionOutputDrainStep::SidebandOnly {
                chunk_duration: Duration::ZERO,
                root_chrome_dirty: false,
            };
        }
        let chunk_started_at = Instant::now();
        let chunk_input_bytes = data.len();
        let mut chunk_timings = SessionEventDrainTimings::default();
        let mut root_chrome_dirty = false;
        let sideband_bypass = self.session_output_can_bypass_sideband_detectors(&session_id, &data);
        let data = if sideband_bypass {
            data
        } else {
            if let Some(data) = self.process_xymodem_output(&session_id, &data)
                && data.is_empty()
            {
                let chunk_duration = chunk_started_at.elapsed();
                drain_timings.output_total += chunk_duration;
                chunk_timings.output_total += chunk_duration;
                return SessionOutputDrainStep::SidebandOnly {
                    chunk_duration,
                    root_chrome_dirty,
                };
            }
            let stage_started_at = Instant::now();
            let (data, zmodem_root_chrome_dirty) =
                self.process_zmodem_output(&session_id, &data, cx);
            root_chrome_dirty |= zmodem_root_chrome_dirty;
            let stage_duration = stage_started_at.elapsed();
            drain_timings.zmodem += stage_duration;
            chunk_timings.zmodem += stage_duration;
            if data.is_empty() {
                let chunk_duration = chunk_started_at.elapsed();
                drain_timings.output_total += chunk_duration;
                chunk_timings.output_total += chunk_duration;
                self.maybe_log_slow_session_output_chunk(
                    &session_id,
                    chunk_input_bytes,
                    chunk_duration,
                    &chunk_timings,
                );
                return SessionOutputDrainStep::SidebandOnly {
                    chunk_duration,
                    root_chrome_dirty,
                };
            }
            // Consume side-band markers after active transfer payloads are removed.
            let stage_started_at = Instant::now();
            let (data, trzsz_root_chrome_dirty) = self.process_trzsz_output(&session_id, &data, cx);
            root_chrome_dirty |= trzsz_root_chrome_dirty;
            let stage_duration = stage_started_at.elapsed();
            drain_timings.trzsz += stage_duration;
            chunk_timings.trzsz += stage_duration;
            if data.is_empty() {
                let chunk_duration = chunk_started_at.elapsed();
                drain_timings.output_total += chunk_duration;
                chunk_timings.output_total += chunk_duration;
                self.maybe_log_slow_session_output_chunk(
                    &session_id,
                    chunk_input_bytes,
                    chunk_duration,
                    &chunk_timings,
                );
                return SessionOutputDrainStep::SidebandOnly {
                    chunk_duration,
                    root_chrome_dirty,
                };
            }
            data
        };
        if self.session_has_active_ai_capture(&session_id) {
            self.flush_pending_session_frame_outputs(pending_frame_outputs, drain_timings);
            let stage_started_at = Instant::now();
            let text = self.decode_session_output_for_recording(&session_id, &data);
            let stage_duration = stage_started_at.elapsed();
            drain_timings.decode += stage_duration;
            chunk_timings.decode += stage_duration;
            let stage_started_at = Instant::now();
            let result = self.ai.process_agent_output(&text);
            let stage_duration = stage_started_at.elapsed();
            drain_timings.ai_capture += stage_duration;
            chunk_timings.ai_capture += stage_duration;
            if !result.visible_text.is_empty() {
                let stage_started_at = Instant::now();
                let visible_bytes =
                    self.encode_visible_terminal_text_for_output(&session_id, &result.visible_text);
                self.submit_terminal_frame_output(&session_id, visible_bytes);
                let stage_duration = stage_started_at.elapsed();
                drain_timings.terminal_append += stage_duration;
                chunk_timings.terminal_append += stage_duration;
            }
            let stage_started_at = Instant::now();
            for captured in result.completed {
                self.handle_ai_agent_captured_output(captured, cx);
            }
            let stage_duration = stage_started_at.elapsed();
            drain_timings.ai_capture += stage_duration;
            chunk_timings.ai_capture += stage_duration;
        } else {
            self.maybe_detect_ai_terminal_error(&session_id, &data, cx);
            pending_frame_outputs.push((session_id.clone(), data));
        }
        // Routing only changes when sideband detectors activate/deactivate.
        if !sideband_bypass {
            self.sync_session_event_bridge_session_policy(&session_id);
        }
        let chunk_duration = chunk_started_at.elapsed();
        drain_timings.output_total += chunk_duration;
        chunk_timings.output_total += chunk_duration;
        self.maybe_log_slow_session_output_chunk(
            &session_id,
            chunk_input_bytes,
            chunk_duration,
            &chunk_timings,
        );
        SessionOutputDrainStep::Accepted {
            chunk_duration,
            root_chrome_dirty,
        }
    }

    fn maybe_detect_ai_terminal_error(
        &mut self,
        session_id: &str,
        data: &[u8],
        cx: &mut Context<Self>,
    ) {
        if data.is_empty() {
            return;
        }
        let watched = self.session.active_id() == Some(session_id)
            || self.ai.chat_targets_session(session_id);
        if !watched {
            return;
        }

        let output = String::from_utf8_lossy(data);
        if !terminal_output_has_error_keyword(&output) {
            return;
        }

        if self.ai.note_detected_error(
            session_id.to_string(),
            terminal_error_notice_output(&output),
            Instant::now(),
        ) {
            self.defer_ai_panel_snapshot_flush(cx);
        }
    }

    pub(super) fn maybe_log_slow_session_output_chunk(
        &mut self,
        session_id: &str,
        chunk_input_bytes: usize,
        chunk_duration: Duration,
        timings: &SessionEventDrainTimings,
    ) {
        if chunk_duration < SESSION_EVENT_DRAIN_SLOW_CHUNK {
            return;
        }
        if !self.should_log_slow_diagnostic("session_event_output_chunk", Instant::now()) {
            return;
        }
        tracing::warn!(
            diagnostic = "session_event_drain",
            session_id = %session_id,
            chunk_input_bytes,
            chunk_duration_ms = chunk_duration.as_millis(),
            zmodem_us = timings.zmodem.as_micros(),
            trzsz_us = timings.trzsz.as_micros(),
            decode_us = timings.decode.as_micros(),
            recording_us = timings.recording.as_micros(),
            terminal_append_us = timings.terminal_append.as_micros(),
            credential_autofill_us = timings.credential_autofill.as_micros(),
            ai_capture_us = timings.ai_capture.as_micros(),
            "slow session output chunk"
        );
    }

    pub(super) fn session_has_active_ai_capture(&self, session_id: &str) -> bool {
        self.ai.agent_capture_is_active_for(session_id)
    }

    pub(super) fn flush_pending_session_frame_outputs(
        &self,
        pending_frame_outputs: &mut Vec<(String, Vec<u8>)>,
        timings: &mut SessionEventDrainTimings,
    ) {
        if pending_frame_outputs.is_empty() {
            return;
        }
        let stage_started_at = Instant::now();
        self.submit_terminal_frame_outputs(std::mem::take(pending_frame_outputs));
        let stage_duration = stage_started_at.elapsed();
        timings.terminal_append += stage_duration;
        timings.output_total += stage_duration;
    }

    pub(super) fn terminal_frame_backlog_active(&self) -> bool {
        terminal_frame_backlog_active_from_counts(
            self.terminal.frame_queue_metrics().pending_event_count,
            self.terminal.frame_queue_metrics().event_count,
            self.terminal.frame_queue_metrics().command_count,
        )
    }

    pub(super) fn session_sideband_detectors_idle(&self, session_id: &str) -> bool {
        !self.xymodem_transfer_active(session_id)
            && self.zmodem_output_can_bypass_detector(session_id, &[])
            && self.trzsz_output_can_bypass_detector(session_id, &[])
    }

    pub(super) fn session_output_can_bypass_sideband_detectors(
        &self,
        session_id: &str,
        data: &[u8],
    ) -> bool {
        !self.xymodem_transfer_active(session_id)
            && self.zmodem_output_can_bypass_detector(session_id, data)
            && self.trzsz_output_can_bypass_detector(session_id, data)
    }
}
