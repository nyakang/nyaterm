use rust_i18n::t;

use std::collections::HashMap;
use std::time::{Duration, Instant};

use gpui::{Context, KeyDownEvent};
use nyaterm_core::{
    TerminalInputState, TerminalResizeGeometry,
    terminal_resize_geometry_for_size_with_insets_and_scale, terminal_snapped_cell_height,
};
use nyaterm_transport::SessionKind;

use crate::features::NyaTermApp;
use crate::features::terminal::terminal_surface::{
    TERMINAL_SCROLLBAR_MIN_THUMB_HEIGHT, TerminalScrollbarDragState, TerminalScrollbarInput,
    terminal_scroll_offset_from_pointer, terminal_scrollbar_metrics, track_height,
};
use crate::models::TerminalPerformanceOverlay;

use super::view_io::{TerminalMouseReportRequest, terminal_visual_display_offset};

pub(in crate::features) fn terminal_scroll_delta_lines_from_raw(
    residual: &mut f32,
    raw_lines: f32,
) -> i32 {
    if raw_lines == 0.0 || !raw_lines.is_finite() {
        return 0;
    }
    if *residual != 0.0 && residual.signum() != raw_lines.signum() {
        *residual = 0.0;
    }
    let combined = *residual + raw_lines;
    if combined.abs() < 1.0 {
        *residual = combined;
        return 0;
    }
    let whole = combined.trunc();
    *residual = combined - whole;
    whole as i32
}

pub(in crate::features) fn terminal_local_scroll_delta_lines_from_state(
    scroll_offset: usize,
    residual_lines: f32,
    max_offset: usize,
    raw_lines: f32,
) -> (i32, f32) {
    if raw_lines == 0.0 || !raw_lines.is_finite() || max_offset == 0 {
        return (0, 0.0);
    }
    let residual_lines = if residual_lines.is_finite() {
        residual_lines
    } else {
        0.0
    };
    let combined = residual_lines + raw_lines;
    let whole = if combined.abs() >= 1.0 {
        combined.trunc() as i32
    } else {
        0
    };
    let mut delta_lines = whole;
    let mut next_residual = combined - whole as f32;
    let next_offset = scroll_offset as i32 + delta_lines;
    if next_offset <= 0 && next_residual < 0.0 {
        delta_lines = -(scroll_offset as i32);
        next_residual = 0.0;
    } else if next_offset >= max_offset as i32 && next_residual > 0.0 {
        delta_lines = max_offset as i32 - scroll_offset as i32;
        next_residual = 0.0;
    } else if next_offset < 0 {
        delta_lines = -(scroll_offset as i32);
        next_residual = 0.0;
    } else if next_offset > max_offset as i32 {
        delta_lines = max_offset as i32 - scroll_offset as i32;
        next_residual = 0.0;
    }
    if next_residual.abs() < f32::EPSILON * 8.0 {
        next_residual = 0.0;
    }
    (delta_lines, next_residual)
}

pub(in crate::features) fn terminal_display_offset_from_state(
    scroll_offset: usize,
    residual_lines: f32,
    scrollback_len: usize,
) -> usize {
    terminal_visual_display_offset(scroll_offset, residual_lines, scrollback_len)
}

fn terminal_scroll_residual_clamped_for_offset(
    scroll_offset: usize,
    residual_lines: f32,
    scrollback_len: usize,
) -> f32 {
    if !residual_lines.is_finite()
        || (scroll_offset == 0 && residual_lines < 0.0)
        || (scroll_offset >= scrollback_len && residual_lines > 0.0)
    {
        return 0.0;
    }
    if residual_lines.abs() < f32::EPSILON * 8.0 {
        0.0
    } else {
        residual_lines
    }
}

fn terminal_scroll_to_bottom_state_needs_update(
    scroll_offset: usize,
    residual_lines: f32,
    has_new_while_scrolled: bool,
) -> bool {
    scroll_offset != 0
        || has_new_while_scrolled
        || (residual_lines.is_finite() && residual_lines.abs() >= f32::EPSILON * 8.0)
}

fn terminal_scroll_offset_state_needs_update(
    current_offset: usize,
    current_residual_lines: f32,
    current_has_new_while_scrolled: bool,
    next_offset: usize,
) -> bool {
    current_offset != next_offset
        || (next_offset == 0 && current_has_new_while_scrolled)
        || (next_offset == 0
            && current_residual_lines.is_finite()
            && current_residual_lines.abs() >= f32::EPSILON * 8.0)
}

pub(in crate::features) fn terminal_visual_scroll_active_for_state(
    scroll_offset: usize,
    residual_lines: f32,
) -> bool {
    scroll_offset > 0 || (residual_lines.is_finite() && residual_lines.abs() >= f32::EPSILON * 8.0)
}

fn terminal_scroll_offset_reanchored_for_scrollback_growth(
    scroll_offset: usize,
    surface_scrollback_len: usize,
    current_scrollback_len: usize,
) -> usize {
    if scroll_offset == 0 {
        0
    } else {
        scroll_offset.saturating_add(current_scrollback_len.saturating_sub(surface_scrollback_len))
    }
}

fn terminal_fractional_scroll_prefetch_offset(
    scroll_offset: usize,
    residual_lines: f32,
    scrollback_len: usize,
) -> Option<usize> {
    if scrollback_len == 0 || residual_lines == 0.0 || !residual_lines.is_finite() {
        return None;
    }
    if residual_lines > 0.0 {
        return scroll_offset
            .checked_add(1)
            .filter(|offset| *offset <= scrollback_len && *offset > 0);
    }
    scroll_offset.checked_sub(1).filter(|offset| *offset > 0)
}

fn terminal_scroll_predictive_prefetch_offset(
    display_offset: usize,
    delta_lines: i32,
    viewport_rows: usize,
    scrollback_len: usize,
) -> Option<usize> {
    if display_offset == 0 || scrollback_len == 0 || delta_lines == 0 {
        return None;
    }
    let step = viewport_rows
        .max(1)
        .saturating_div(2)
        .max(delta_lines.unsigned_abs() as usize)
        .max(1);
    let offset = if delta_lines > 0 {
        display_offset.saturating_add(step).min(scrollback_len)
    } else {
        display_offset.saturating_sub(step)
    };
    (offset > 0 && offset != display_offset).then_some(offset)
}

fn terminal_scroll_key(session_id: Option<&str>) -> String {
    session_id
        .filter(|id| !id.is_empty())
        .unwrap_or_default()
        .to_string()
}

fn terminal_scroll_should_request_immediate_text_snapshot(
    display_offset: usize,
    text_snapshot_cached: bool,
) -> bool {
    display_offset > 0 && !text_snapshot_cached
}

fn terminal_scroll_should_consume_raw_lines(raw_lines: f32) -> bool {
    raw_lines != 0.0 && raw_lines.is_finite()
}

fn terminal_scrollbar_track_bounds_for_session_state(
    session_bounds: &HashMap<String, gpui::Bounds<gpui::Pixels>>,
    fallback_bounds: Option<gpui::Bounds<gpui::Pixels>>,
    session_id: Option<&str>,
) -> Option<gpui::Bounds<gpui::Pixels>> {
    if let Some(session_id) = session_id.filter(|id| !id.is_empty()) {
        session_bounds.get(session_id).copied()
    } else {
        fallback_bounds
    }
}

pub(in crate::features) fn terminal_scroll_needs_text_first_repaint(
    state: &TerminalScrollVisualState,
    text_updated: bool,
) -> bool {
    state.display_offset > 0 && !text_updated
}

const TERMINAL_SCROLL_POSITION_NOTIFY_DELAY: Duration = Duration::from_millis(16);
const TERMINAL_SCROLLBAR_DRAG_NOTIFY_DELAY: Duration = Duration::from_millis(8);
pub(in crate::features) const TERMINAL_USER_SCROLL_ACTIVE_WINDOW: Duration =
    Duration::from_millis(140);

fn terminal_user_scroll_idle_remaining_delay(
    last_scroll_at: Option<Instant>,
    now: Instant,
    active_window: Duration,
) -> Option<Duration> {
    let last_scroll_at = last_scroll_at?;
    active_window
        .checked_sub(now.saturating_duration_since(last_scroll_at))
        .filter(|delay| !delay.is_zero())
}

#[derive(Clone, Debug, PartialEq)]
pub(in crate::features) struct TerminalScrollVisualState {
    pub session_id: String,
    pub scroll_offset: usize,
    pub scroll_residual_lines: f32,
    pub display_offset: usize,
    pub scrollback_len: usize,
    pub viewport_rows: usize,
    pub has_new_while_scrolled: bool,
    pub performance_overlay: Option<TerminalPerformanceOverlay>,
    pub skipped_output_chars: u64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(in crate::features) struct TerminalScrollWheelStateResult {
    pub visual_state: Option<TerminalScrollVisualState>,
    pub defer_repaint: bool,
    pub handled: bool,
}

fn terminal_should_apply_session_cwd(
    changed: bool,
    active_session: bool,
    auto_sync_enabled: bool,
    path_editing: bool,
    cwd: &str,
) -> bool {
    changed && active_session && auto_sync_enabled && !path_editing && !cwd.trim().is_empty()
}

impl NyaTermApp {
    pub(in crate::features) fn active_terminal_display_offset(&self) -> usize {
        self.terminal_display_offset_for_session(self.session.active_id())
    }

    pub(in crate::features) fn active_terminal_visual_scroll_active(&self) -> bool {
        self.terminal_visual_scroll_active_for_session(self.session.active_id())
    }

    pub(in crate::features) fn terminal_visual_scroll_active_for_session(
        &self,
        session_id: Option<&str>,
    ) -> bool {
        if let Some(session_id) = session_id.filter(|id| !id.is_empty()) {
            let Some(view) = self.terminal.view.views.get(session_id) else {
                return false;
            };
            return terminal_visual_scroll_active_for_state(
                view.scroll_offset,
                self.terminal_scroll_residual_for_session(Some(session_id)),
            );
        }
        terminal_visual_scroll_active_for_state(
            self.terminal.view.scroll_offset,
            self.terminal_scroll_residual_for_session(None),
        )
    }

    pub(in crate::features) fn terminal_display_offset_for_session(
        &self,
        session_id: Option<&str>,
    ) -> usize {
        if let Some(session_id) = session_id.filter(|id| !id.is_empty()) {
            let Some(view) = self.terminal.view.views.get(session_id) else {
                return 0;
            };
            return terminal_display_offset_from_state(
                view.scroll_offset,
                self.terminal_scroll_residual_for_session(Some(session_id)),
                view.scrollback_len_for_ui(),
            );
        }
        terminal_display_offset_from_state(
            self.terminal.view.scroll_offset,
            self.terminal_scroll_residual_for_session(None),
            self.terminal.view.screen.scrollback_len(),
        )
    }

    pub(in crate::features) fn terminal_scroll_visual_state_for_session(
        &self,
        session_id: &str,
    ) -> Option<TerminalScrollVisualState> {
        let view = self.terminal.view.views.get(session_id)?;
        let scroll_offset = view.scroll_offset;
        let scroll_residual_lines = self.terminal_scroll_residual_for_session(Some(session_id));
        let scrollback_len = view.scrollback_len_for_ui();
        let viewport_rows = view.viewport_rows_for_ui();
        let display_offset = terminal_display_offset_from_state(
            scroll_offset,
            scroll_residual_lines,
            scrollback_len,
        );
        Some(TerminalScrollVisualState {
            session_id: session_id.to_string(),
            scroll_offset,
            scroll_residual_lines,
            display_offset,
            scrollback_len,
            viewport_rows,
            has_new_while_scrolled: view.has_new_while_scrolled,
            performance_overlay: view.performance_overlay,
            skipped_output_chars: view.skipped_output_chars,
        })
    }

    pub(in crate::features) fn terminal_scroll_wheel_state_for_session(
        &mut self,
        session_id: &str,
        raw_lines: f32,
        position: gpui::Point<gpui::Pixels>,
        modifiers: gpui::Modifiers,
        cx: &mut Context<Self>,
    ) -> TerminalScrollWheelStateResult {
        if session_id.is_empty() {
            return TerminalScrollWheelStateResult::default();
        }

        let local_scroll = self.terminal_local_scroll_enabled_for_session(session_id);
        let previous_residual = self.terminal_scroll_residual_for_session(Some(session_id));
        let delta = if local_scroll {
            self.terminal_local_scroll_delta_lines_for_session(Some(session_id), raw_lines)
        } else {
            self.terminal_scroll_delta_lines_for_session(Some(session_id), raw_lines)
        };
        if delta == 0 {
            let residual = self.terminal_scroll_residual_for_session(Some(session_id));
            if local_scroll && (residual != 0.0 || residual != previous_residual) {
                self.note_terminal_fractional_scroll_after_local_surface_update(session_id, cx);
                self.queue_terminal_selection_scroll_rehit(session_id, cx);
                return TerminalScrollWheelStateResult {
                    visual_state: self.terminal_scroll_visual_state_for_session(session_id),
                    defer_repaint: false,
                    handled: true,
                };
            }
            if terminal_scroll_should_consume_raw_lines(raw_lines) {
                return TerminalScrollWheelStateResult {
                    visual_state: None,
                    defer_repaint: false,
                    handled: true,
                };
            }
            return TerminalScrollWheelStateResult::default();
        }

        if let Some(cell) = self.point_to_terminal_cell_for_session(Some(session_id), position, cx)
        {
            let button = if delta > 0 { 64u8 } else { 65u8 };
            let steps = delta.unsigned_abs().min(8);
            let mut reported = false;
            for _ in 0..steps {
                if self.maybe_send_mouse_report_for_session(
                    TerminalMouseReportRequest {
                        session_id,
                        button,
                        col: cell.col as u16,
                        row: cell.row as u16,
                        press: true,
                        motion: false,
                        modifiers,
                    },
                    cx,
                ) {
                    reported = true;
                } else {
                    break;
                }
            }
            if reported {
                return TerminalScrollWheelStateResult {
                    visual_state: None,
                    defer_repaint: false,
                    handled: true,
                };
            }
        }

        if self.maybe_send_alternate_scroll_for_session(session_id, delta, cx) {
            return TerminalScrollWheelStateResult {
                visual_state: None,
                defer_repaint: false,
                handled: true,
            };
        }

        let previous_offset = self
            .terminal_scroll_visual_state_for_session(session_id)
            .map(|state| state.scroll_offset);
        let visual_state = self.scroll_terminal_by_for_session_state_only(Some(session_id), delta);
        if let (Some(previous_offset), Some(state)) = (previous_offset, visual_state.as_ref()) {
            self.reconcile_terminal_selection_after_scroll(
                session_id,
                position,
                previous_offset,
                state.scroll_offset,
                cx,
            );
            self.queue_terminal_selection_scroll_rehit(session_id, cx);
        }
        if let Some(state) = visual_state.as_ref()
            && terminal_visual_scroll_active_for_state(
                state.scroll_offset,
                state.scroll_residual_lines,
            )
        {
            if terminal_scroll_should_request_immediate_text_snapshot(
                state.display_offset,
                self.terminal_scroll_text_cached_for_session(session_id, state.display_offset),
            ) {
                let _ = self.request_terminal_frame_snapshot_for_user_scroll(
                    session_id,
                    state.display_offset,
                );
            }
            if let Some(prefetch_offset) = terminal_scroll_predictive_prefetch_offset(
                state.display_offset,
                delta,
                state.viewport_rows,
                state.scrollback_len,
            ) {
                let _ = self
                    .request_terminal_frame_snapshot_for_user_scroll(session_id, prefetch_offset);
            }
            self.queue_terminal_scroll_position_after_local_surface_update(session_id, cx);
        }
        TerminalScrollWheelStateResult {
            defer_repaint: visual_state
                .as_ref()
                .is_some_and(|state| state.scroll_offset == 0),
            visual_state,
            handled: true,
        }
    }

    pub(in crate::features) fn sync_terminal_local_scroll_visual_state_from_surface(
        &mut self,
        state: TerminalScrollVisualState,
        cx: &mut Context<Self>,
    ) -> Option<TerminalScrollVisualState> {
        let session_id = state.session_id;
        if session_id.is_empty() {
            return None;
        }
        if !self.terminal_local_scroll_enabled_for_session(&session_id) {
            return self.terminal_scroll_visual_state_for_session(&session_id);
        }

        let previous_display_offset = self.terminal_display_offset_for_session(Some(&session_id));
        let (target_display_offset, residual_lines) = {
            let view = self.terminal.view.views.get_mut(&session_id)?;
            let scrollback_len = view.scrollback_len_for_ui();
            let scroll_offset = terminal_scroll_offset_reanchored_for_scrollback_growth(
                state.scroll_offset,
                state.scrollback_len,
                scrollback_len,
            )
            .min(scrollback_len);
            let residual_lines = terminal_scroll_residual_clamped_for_offset(
                scroll_offset,
                state.scroll_residual_lines,
                scrollback_len,
            );
            view.scroll_offset = scroll_offset;
            if scroll_offset == 0 {
                view.has_new_while_scrolled = false;
            }
            (
                terminal_display_offset_from_state(scroll_offset, residual_lines, scrollback_len),
                residual_lines,
            )
        };

        let key = terminal_scroll_key(Some(&session_id));
        if residual_lines == 0.0 {
            self.terminal.view.scroll_delta_residuals.remove(&key);
        } else {
            self.terminal
                .view
                .scroll_delta_residuals
                .insert(key, residual_lines);
        }

        let visual_state = self.terminal_scroll_visual_state_for_session(&session_id)?;
        if terminal_visual_scroll_active_for_state(
            visual_state.scroll_offset,
            visual_state.scroll_residual_lines,
        ) {
            if terminal_scroll_should_request_immediate_text_snapshot(
                visual_state.display_offset,
                self.terminal_scroll_text_cached_for_session(
                    &session_id,
                    visual_state.display_offset,
                ),
            ) {
                let _ = self.request_terminal_frame_snapshot_for_user_scroll(
                    &session_id,
                    visual_state.display_offset,
                );
            }
            let delta = match visual_state.display_offset.cmp(&previous_display_offset) {
                std::cmp::Ordering::Greater => Some(1),
                std::cmp::Ordering::Less => Some(-1),
                std::cmp::Ordering::Equal => None,
            };
            if let Some(delta) = delta
                && let Some(prefetch_offset) = terminal_scroll_predictive_prefetch_offset(
                    visual_state.display_offset,
                    delta,
                    visual_state.viewport_rows,
                    visual_state.scrollback_len,
                )
            {
                let _ = self
                    .request_terminal_frame_snapshot_for_user_scroll(&session_id, prefetch_offset);
            }
            if visual_state.display_offset == target_display_offset
                && let Some(offset) = terminal_fractional_scroll_prefetch_offset(
                    visual_state.scroll_offset,
                    visual_state.scroll_residual_lines,
                    visual_state.scrollback_len,
                )
            {
                let _ = self.request_terminal_frame_snapshot_for_user_scroll(&session_id, offset);
            }
            self.queue_terminal_scroll_position_after_local_surface_update(&session_id, cx);
        }
        Some(visual_state)
    }

    pub(in crate::features) fn scroll_terminal_by(
        &mut self,
        delta_lines: i32,
        cx: &mut Context<Self>,
    ) {
        let session_id = self.session.active_id_owned();
        self.scroll_terminal_by_for_session(session_id.as_deref(), delta_lines, cx);
    }

    pub(in crate::features) fn scroll_terminal_by_for_session(
        &mut self,
        session_id: Option<&str>,
        delta_lines: i32,
        cx: &mut Context<Self>,
    ) {
        let Some(state) = self.scroll_terminal_by_for_session_state_only(session_id, delta_lines)
        else {
            return;
        };
        let session_id = Some(state.session_id.as_str());
        self.notify_terminal_scroll_after_state_change(session_id, cx);
    }

    pub(in crate::features) fn scroll_terminal_by_for_session_state_only(
        &mut self,
        session_id: Option<&str>,
        delta_lines: i32,
    ) -> Option<TerminalScrollVisualState> {
        if delta_lines == 0 {
            return None;
        }
        if let Some(session_id) = session_id.filter(|id| !id.is_empty()) {
            if let Some(view) = self.terminal.view.views.get_mut(session_id) {
                let max = view.scrollback_len_for_ui();
                let next = if delta_lines > 0 {
                    view.scroll_offset.saturating_add(delta_lines as usize)
                } else {
                    view.scroll_offset.saturating_sub((-delta_lines) as usize)
                };
                view.scroll_offset = next.min(max);
                if view.scroll_offset == 0 {
                    view.has_new_while_scrolled = false;
                    self.terminal.view.scroll_delta_residuals.remove(session_id);
                }
            }
            return self.terminal_scroll_visual_state_for_session(session_id);
        }

        let max = self.terminal.view.screen.scrollback_len();
        let next = if delta_lines > 0 {
            self.terminal
                .view
                .scroll_offset
                .saturating_add(delta_lines as usize)
        } else {
            self.terminal
                .view
                .scroll_offset
                .saturating_sub((-delta_lines) as usize)
        };
        self.terminal.view.scroll_offset = next.min(max);
        if self.terminal.view.scroll_offset == 0 {
            self.clear_terminal_scroll_residual_for_session(None);
            if let Some(session_id) = self.session.active_id_owned()
                && let Some(view) = self.terminal.view.views.get_mut(&session_id)
            {
                view.has_new_while_scrolled = false;
            }
        }
        self.session
            .active_id_owned()
            .and_then(|session_id| self.terminal_scroll_visual_state_for_session(&session_id))
    }

    pub(in crate::features) fn queue_terminal_scroll_position_notify(
        &mut self,
        session_id: &str,
        cx: &mut Context<Self>,
    ) {
        if session_id.is_empty() {
            return;
        }
        self.mark_terminal_user_scroll_activity(session_id, cx);
        // Keep the wheel hot path text-first: apply the local visual offset
        // immediately, but defer target snapshot/decorations to the coalesced
        // position notify below.
        self.notify_terminal_scroll_visual_only(session_id, cx);
        if self.shell.queue_terminal_scroll_position(session_id, false) {
            self.schedule_terminal_scroll_position_notify(cx);
        }
    }

    pub(in crate::features) fn queue_terminal_scroll_position_after_local_surface_update(
        &mut self,
        session_id: &str,
        cx: &mut Context<Self>,
    ) {
        if session_id.is_empty() {
            return;
        }
        self.mark_terminal_user_scroll_activity(session_id, cx);
        if self.shell.queue_terminal_scroll_position(session_id, true) {
            self.schedule_terminal_scroll_position_notify(cx);
        }
    }

    fn schedule_terminal_scroll_position_notify(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(TERMINAL_SCROLL_POSITION_NOTIFY_DELAY)
                .await;
            let _ = this.update(cx, |this, cx| {
                let (session_ids, snapshot_only_session_ids) =
                    this.shell.drain_terminal_scroll_position();
                for session_id in snapshot_only_session_ids {
                    if let Some(offset) = this
                        .terminal
                        .view
                        .views
                        .get(&session_id)
                        .map(|view| {
                            let residual =
                                this.terminal_scroll_residual_for_session(Some(&session_id));
                            terminal_visual_display_offset(
                                view.scroll_offset,
                                residual,
                                view.scrollback_len_for_ui(),
                            )
                        })
                        .filter(|offset| *offset > 0)
                    {
                        this.request_terminal_frame_snapshot_for_user_scroll(
                            session_id.as_str(),
                            offset,
                        );
                    }
                }
                for session_id in session_ids {
                    if let Some(offset) = this
                        .terminal
                        .view
                        .views
                        .get(&session_id)
                        .map(|view| {
                            let residual =
                                this.terminal_scroll_residual_for_session(Some(&session_id));
                            terminal_visual_display_offset(
                                view.scroll_offset,
                                residual,
                                view.scrollback_len_for_ui(),
                            )
                        })
                        .filter(|offset| *offset > 0)
                    {
                        this.request_terminal_frame_snapshot_for_user_scroll(
                            session_id.as_str(),
                            offset,
                        );
                    }
                    this.notify_terminal_scroll_position_only(session_id.as_str(), cx);
                }
            });
        })
        .detach();
    }

    pub(in crate::features) fn note_terminal_fractional_scroll_after_local_surface_update(
        &mut self,
        session_id: &str,
        cx: &mut Context<Self>,
    ) {
        if session_id.is_empty() {
            return;
        }
        self.mark_terminal_user_scroll_activity(session_id, cx);
        if let Some((scroll_offset, residual_lines, scrollback_len)) =
            self.terminal.view.views.get(session_id).map(|view| {
                (
                    view.scroll_offset,
                    self.terminal_scroll_residual_for_session(Some(session_id)),
                    view.scrollback_len_for_ui(),
                )
            })
            && let Some(offset) = terminal_fractional_scroll_prefetch_offset(
                scroll_offset,
                residual_lines,
                scrollback_len,
            )
        {
            let _ = self.request_terminal_frame_snapshot_for_user_scroll(session_id, offset);
        }
    }

    fn mark_terminal_user_scroll_activity(&mut self, session_id: &str, cx: &mut Context<Self>) {
        if session_id.is_empty() {
            return;
        }
        if !self
            .shell
            .queue_terminal_user_scroll_idle(session_id, Instant::now())
        {
            return;
        }
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(TERMINAL_USER_SCROLL_ACTIVE_WINDOW)
                .await;
            let _ = this.update(cx, |this, cx| {
                this.flush_terminal_user_scroll_idle_notify(cx);
            });
        })
        .detach();
    }

    fn flush_terminal_user_scroll_idle_notify(&mut self, cx: &mut Context<Self>) {
        let now = Instant::now();
        if let Some(delay) = terminal_user_scroll_idle_remaining_delay(
            self.shell.last_terminal_user_scroll_at(),
            now,
            TERMINAL_USER_SCROLL_ACTIVE_WINDOW,
        ) {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(delay).await;
                let _ = this.update(cx, |this, cx| {
                    this.flush_terminal_user_scroll_idle_notify(cx);
                });
            })
            .detach();
            return;
        }
        let session_ids = self.shell.drain_terminal_user_scroll_idle_sessions();
        for session_id in session_ids {
            if self.terminal_visual_scroll_active_for_session(Some(&session_id)) {
                let offset = self.terminal_display_offset_for_session(Some(&session_id));
                let _ = self.request_terminal_frame_snapshot_for_scroll_enrichment(
                    session_id.as_str(),
                    offset,
                    None,
                );
                self.notify_terminal_surface_only(Some(session_id.as_str()), cx);
            }
        }
    }

    pub(in crate::features) fn notify_terminal_scroll_after_state_change(
        &mut self,
        session_id: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let session_id = session_id
            .filter(|id| !id.is_empty())
            .map(str::to_string)
            .or_else(|| self.session.active_id_owned());
        let Some(session_id) = session_id else {
            return;
        };
        let is_scrolled = self.terminal_visual_scroll_active_for_session(Some(&session_id));
        if is_scrolled {
            self.queue_terminal_scroll_position_notify(session_id.as_str(), cx);
        } else {
            self.notify_terminal_surface_only(Some(session_id.as_str()), cx);
        }
    }

    pub(in crate::features) fn terminal_scroll_delta_lines_for_session(
        &mut self,
        session_id: Option<&str>,
        raw_lines: f32,
    ) -> i32 {
        let key = terminal_scroll_key(session_id);

        {
            let residual = self
                .terminal
                .view
                .scroll_delta_residuals
                .entry(key.clone())
                .or_insert(0.0);
            terminal_scroll_delta_lines_from_raw(residual, raw_lines)
        }
    }

    pub(in crate::features) fn terminal_local_scroll_delta_lines_for_session(
        &mut self,
        session_id: Option<&str>,
        raw_lines: f32,
    ) -> i32 {
        let max_offset = self.terminal_scroll_max_for_session(session_id);
        if raw_lines == 0.0 || !raw_lines.is_finite() || max_offset == 0 {
            self.clear_terminal_scroll_residual_for_session(session_id);
            return 0;
        }
        let scroll_offset = if let Some(session_id) = session_id.filter(|id| !id.is_empty()) {
            self.terminal
                .view
                .views
                .get(session_id)
                .map(|view| view.scroll_offset)
                .unwrap_or(0)
        } else {
            self.terminal.view.scroll_offset
        };
        let residual = self.terminal_scroll_residual_for_session(session_id);
        let (delta_lines, next_residual) = terminal_local_scroll_delta_lines_from_state(
            scroll_offset,
            residual,
            max_offset,
            raw_lines,
        );
        let key = terminal_scroll_key(session_id);
        if next_residual == 0.0 {
            self.terminal.view.scroll_delta_residuals.remove(&key);
        } else {
            self.terminal
                .view
                .scroll_delta_residuals
                .insert(key, next_residual);
        }
        delta_lines
    }

    pub(in crate::features) fn terminal_scroll_residual_for_session(
        &self,
        session_id: Option<&str>,
    ) -> f32 {
        self.terminal
            .view
            .scroll_delta_residuals
            .get(&terminal_scroll_key(session_id))
            .copied()
            .unwrap_or(0.0)
    }

    pub(in crate::features) fn clear_terminal_scroll_residual_for_session(
        &mut self,
        session_id: Option<&str>,
    ) {
        self.terminal
            .view
            .scroll_delta_residuals
            .remove(&terminal_scroll_key(session_id));
    }

    pub(in crate::features) fn terminal_local_scroll_enabled_for_session(
        &self,
        session_id: &str,
    ) -> bool {
        if session_id.is_empty() {
            return true;
        }
        let protocol = self.terminal_protocol_state_for_session(session_id);
        !protocol.mouse_reporting && protocol.alternate_scroll_payload(1).is_none()
    }

    /// Insert quoted local file paths into the active session (Tauri Local drop).
    pub(in crate::features) fn handle_terminal_external_file_drop(
        &mut self,
        session_id: String,
        paths: Vec<std::path::PathBuf>,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        self.terminal.clear_terminal_file_drop_hover();
        if session_id.is_empty() || paths.is_empty() {
            cx.notify();
            return;
        }
        if self.session.is_disconnected(&session_id) {
            self.shell
                .set_status("session disconnected — reconnect before dropping files".to_string());
            cx.notify();
            return;
        }
        let kind = self
            .session
            .ordered_sessions()
            .into_iter()
            .find(|s| s.id == session_id)
            .map(|s| s.kind);
        let path_strings: Vec<String> = paths
            .iter()
            .filter_map(|p| {
                if p.is_dir() {
                    None
                } else {
                    Some(p.display().to_string())
                }
            })
            .collect();
        let has_dirs = paths.iter().any(|p| p.is_dir());
        match kind {
            Some(SessionKind::LocalPty) | None => {
                if path_strings.is_empty() {
                    self.shell
                        .set_status("folders cannot be dropped into a local terminal".to_string());
                    cx.notify();
                    return;
                }
                // Activate target session if needed.
                if self.session.active_id() != Some(session_id.as_str()) {
                    self.activate_session_id_with_surface_sync(&session_id, cx);
                }
                let text = nyaterm_core::format_local_terminal_drop_input(&path_strings);
                if self.send_terminal_input(text.into_bytes(), cx) {
                    self.shell.set_status(format!(
                        "inserted {} path(s) into terminal",
                        path_strings.len()
                    ));
                    cx.notify();
                }
            }
            Some(SessionKind::Serial) => {
                if has_dirs {
                    self.shell
                        .set_status(t!("terminalCtx.serialFoldersUnsupported").to_string());
                    cx.notify();
                    return;
                }
                let files = path_strings
                    .into_iter()
                    .map(std::path::PathBuf::from)
                    .collect();
                self.open_serial_upload_protocol_dialog(session_id, files, window, cx);
            }
            Some(SessionKind::Ssh | SessionKind::Telnet | SessionKind::RawTcp) => {
                if has_dirs {
                    self.shell.set_status(
                        "folders cannot be uploaded via ZMODEM; use the remote file browser"
                            .to_string(),
                    );
                    cx.notify();
                    return;
                }
                if path_strings.is_empty() {
                    cx.notify();
                    return;
                }
                let files: Vec<std::path::PathBuf> = path_strings
                    .into_iter()
                    .map(std::path::PathBuf::from)
                    .collect();
                self.start_zmodem_upload(session_id, files, cx);
            }
            Some(SessionKind::Rdp | SessionKind::Vnc) => {
                self.shell.set_status(
                    "file drop is not supported for remote desktop sessions".to_string(),
                );
                cx.notify();
            }
        }
    }

    pub(in crate::features) fn set_terminal_file_drop_hover(
        &mut self,
        session_id: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if self.terminal.set_terminal_file_drop_hover(session_id) {
            cx.notify();
        }
        // A drag can end without a drop on this element, and only `has_active_drag`
        // reports that; the clock watches for it while the highlight is up.
        self.ensure_drop_hover_clock(cx);
    }

    pub(in crate::features) fn clear_terminal_file_drop_hover_for_session(
        &mut self,
        session_id: &str,
        cx: &mut Context<Self>,
    ) {
        if self
            .terminal
            .clear_terminal_file_drop_hover_for_session(session_id)
        {
            cx.notify();
        }
    }

    pub(in crate::features) fn apply_session_cwd(&mut self, session_id: &str, cwd: String) -> bool {
        let changed = self.session.update_cwd(session_id, cwd.clone());
        // Auto-sync the transfer browser path when enabled for the active SSH session.
        if terminal_should_apply_session_cwd(
            changed,
            self.session.active_id() == Some(session_id),
            self.transfer_browser_auto_sync_cwd_enabled(),
            self.transfer.browser_view().path_editing,
            &cwd,
        ) {
            return self.transfer.apply_terminal_cwd_to_browser(cwd);
        }
        false
    }

    /// Apply OSC 133 command-start / command-finish edges (Tauri shell integration).
    pub(in crate::features) fn apply_shell_integration_edges(
        &mut self,
        session_id: &str,
        started: bool,
        finished: bool,
        command_running: bool,
    ) {
        // Only affect the active session suggestion pipeline.
        if self.session.active_id() != Some(session_id) {
            return;
        }
        if started {
            // Command is running: clear tracker and suppress suggestions (Tauri C mark).
            self.terminal.assist.command_input_tracker = TerminalInputState::new();
            self.terminal.assist.command_suggestions = None;
            self.terminal.assist.command_suggestions_suppressed = true;
            self.terminal.assist.command_suggestion_search_gen = self
                .terminal
                .assist
                .command_suggestion_search_gen
                .saturating_add(1);
        }
        if finished {
            // Command finished: re-enable suggestion tracking (Tauri D mark).
            self.terminal.assist.command_suggestions_suppressed = false;
            self.terminal.assist.command_input_tracker = TerminalInputState::new();
            self.terminal.assist.command_suggestions = None;
            self.terminal.assist.command_suggestion_search_gen = self
                .terminal
                .assist
                .command_suggestion_search_gen
                .saturating_add(1);
        }
        let _ = command_running;
    }

    pub(in crate::features) fn scroll_terminal_to_bottom_state_only(&mut self) -> Option<String> {
        if let Some(session_id) = self.session.active_id_owned() {
            let residual_lines = self.terminal_scroll_residual_for_session(Some(&session_id));
            if let Some(view) = self.terminal.view.views.get_mut(&session_id) {
                let changed = terminal_scroll_to_bottom_state_needs_update(
                    view.scroll_offset,
                    residual_lines,
                    view.has_new_while_scrolled,
                );
                view.scroll_offset = 0;
                view.has_new_while_scrolled = false;
                self.clear_terminal_scroll_residual_for_session(Some(&session_id));
                if !changed {
                    return None;
                }
                return Some(session_id);
            }
            let changed = terminal_scroll_to_bottom_state_needs_update(
                self.terminal.view.scroll_offset,
                self.terminal_scroll_residual_for_session(None),
                false,
            );
            self.terminal.view.scroll_offset = 0;
            self.clear_terminal_scroll_residual_for_session(None);
            return changed.then_some(session_id);
        }
        self.terminal.view.scroll_offset = 0;
        self.clear_terminal_scroll_residual_for_session(None);
        None
    }

    pub(in crate::features) fn scroll_terminal_to_bottom(&mut self, cx: &mut Context<Self>) {
        let session_id = self.scroll_terminal_to_bottom_state_only();
        if session_id.is_some() {
            self.notify_terminal_surface_only(session_id.as_deref(), cx);
        }
    }

    pub(in crate::features) fn scroll_terminal_to_top(&mut self, cx: &mut Context<Self>) {
        if let Some(session_id) = self.session.active_id_owned() {
            if let Some(view) = self.terminal.view.views.get_mut(&session_id) {
                view.scroll_offset = view.scrollback_len_for_ui();
            }
        } else {
            self.terminal.view.scroll_offset = self.terminal.view.screen.scrollback_len();
        }
        self.notify_terminal_scroll_after_state_change(
            self.session.active_id_owned().as_deref(),
            cx,
        );
    }

    pub(in crate::features) fn terminal_scroll_max_for_session(
        &self,
        session_id: Option<&str>,
    ) -> usize {
        if let Some(session_id) = session_id.filter(|id| !id.is_empty()) {
            self.terminal
                .view
                .views
                .get(session_id)
                .map(|view| view.scrollback_len_for_ui())
                .unwrap_or(0)
        } else {
            self.terminal.view.screen.scrollback_len()
        }
    }

    pub(in crate::features) fn set_terminal_scroll_offset_for_session_state_only(
        &mut self,
        session_id: Option<&str>,
        offset: usize,
    ) -> Option<String> {
        if let Some(session_id) = session_id.filter(|id| !id.is_empty()) {
            let current_residual = self.terminal_scroll_residual_for_session(Some(session_id));
            if let Some(view) = self.terminal.view.views.get_mut(session_id) {
                let max = view.scrollback_len_for_ui();
                let next_offset = offset.min(max);
                if !terminal_scroll_offset_state_needs_update(
                    view.scroll_offset,
                    current_residual,
                    view.has_new_while_scrolled,
                    next_offset,
                ) {
                    return None;
                }
                view.scroll_offset = next_offset;
                if view.scroll_offset == 0 {
                    view.has_new_while_scrolled = false;
                    self.terminal.view.scroll_delta_residuals.remove(session_id);
                }
                return Some(session_id.to_string());
            }
            return None;
        } else {
            let max = self.terminal.view.screen.scrollback_len();
            let next_offset = offset.min(max);
            let current_residual = self.terminal_scroll_residual_for_session(None);
            if !terminal_scroll_offset_state_needs_update(
                self.terminal.view.scroll_offset,
                current_residual,
                false,
                next_offset,
            ) {
                return None;
            }
            self.terminal.view.scroll_offset = next_offset;
            if self.terminal.view.scroll_offset == 0 {
                self.clear_terminal_scroll_residual_for_session(None);
            }
        }
        self.session.active_id_owned()
    }

    pub(in crate::features) fn begin_terminal_scrollbar_drag(
        &mut self,
        session_id: Option<String>,
        grab_offset_y: f32,
        cx: &mut Context<Self>,
    ) {
        let repaint_session_id =
            self.begin_terminal_scrollbar_drag_state_only(session_id, grab_offset_y);
        self.notify_terminal_surface_only(repaint_session_id.as_deref(), cx);
    }

    pub(in crate::features) fn begin_terminal_scrollbar_drag_state_only(
        &mut self,
        session_id: Option<String>,
        grab_offset_y: f32,
    ) -> Option<String> {
        self.terminal.view.scrollbar_drag = Some(TerminalScrollbarDragState {
            session_id: session_id.clone(),
            grab_offset_y: if grab_offset_y.is_finite() {
                grab_offset_y.max(0.0)
            } else {
                0.0
            },
        });
        session_id.or_else(|| self.session.active_id_owned())
    }

    pub(in crate::features) fn update_terminal_scrollbar_drag(
        &mut self,
        event: &gpui::MouseMoveEvent,
        cx: &mut Context<Self>,
    ) {
        let Some(drag) = self.terminal.view.scrollbar_drag.clone() else {
            return;
        };
        let Some(bounds) =
            self.terminal_scrollbar_track_bounds_for_session(drag.session_id.as_deref())
        else {
            return;
        };
        let max = self.terminal_scroll_max_for_session(drag.session_id.as_deref());
        let metrics =
            self.terminal_scrollbar_metrics_for_session(drag.session_id.as_deref(), bounds);
        let offset = terminal_scroll_offset_from_pointer(
            f32::from(event.position.y),
            f32::from(bounds.origin.y),
            metrics,
            drag.grab_offset_y,
            max,
        );
        let repaint_session_id = self
            .set_terminal_scroll_offset_for_session_state_only(drag.session_id.as_deref(), offset);
        if repaint_session_id.is_some() {
            self.queue_terminal_scrollbar_drag_visual_notify(repaint_session_id.as_deref(), cx);
        }
    }

    pub(in crate::features) fn finish_terminal_scrollbar_drag(&mut self, cx: &mut Context<Self>) {
        if let Some(drag) = self.terminal.view.scrollbar_drag.take() {
            let session_id = drag.session_id;
            self.notify_terminal_surface_only(session_id.as_deref(), cx);
        }
    }

    fn queue_terminal_scrollbar_drag_visual_notify(
        &mut self,
        session_id: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let session_id = session_id
            .filter(|id| !id.is_empty())
            .map(str::to_string)
            .or_else(|| self.session.active_id_owned());
        let Some(session_id) = session_id else {
            return;
        };
        if session_id.is_empty() {
            return;
        }
        self.mark_terminal_user_scroll_activity(session_id.as_str(), cx);
        if !self.shell.queue_terminal_scrollbar_drag(session_id) {
            return;
        }
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(TERMINAL_SCROLLBAR_DRAG_NOTIFY_DELAY)
                .await;
            let _ = this.update(cx, |this, cx| {
                this.flush_terminal_scrollbar_drag_visual_notify(cx);
            });
        })
        .detach();
    }

    fn flush_terminal_scrollbar_drag_visual_notify(&mut self, cx: &mut Context<Self>) {
        let session_ids = self.shell.drain_terminal_scrollbar_drag_sessions();
        for session_id in session_ids {
            self.notify_terminal_scroll_visual_only(session_id.as_str(), cx);
        }
    }

    pub(in crate::features) fn terminal_surface_bounds_for_session(
        &self,
        session_id: Option<&str>,
    ) -> Option<gpui::Bounds<gpui::Pixels>> {
        if let Some(session_id) = session_id.filter(|id| !id.is_empty()) {
            self.terminal
                .layout
                .session_surface_bounds
                .get(session_id)
                .copied()
                .or(self.terminal.layout.surface_bounds)
        } else {
            self.terminal.layout.surface_bounds
        }
    }

    /// Bounds recorded for exactly this pane, with no fallback to the active
    /// pane's bounds. The bounds tracker must use this: every tab paints the
    /// same rect, so falling back to a sibling's bounds makes a brand-new
    /// session look already-measured and it never leaves the default grid.
    pub(in crate::features) fn recorded_terminal_surface_bounds_for_session(
        &self,
        session_id: Option<&str>,
    ) -> Option<gpui::Bounds<gpui::Pixels>> {
        match session_id.filter(|id| !id.is_empty()) {
            Some(session_id) => self
                .terminal
                .layout
                .session_surface_bounds
                .get(session_id)
                .copied(),
            None => self.terminal.layout.surface_bounds,
        }
    }

    /// True when the recorded bounds, scale factor, and the pane's live grid all
    /// already agree with what `bounds` implies, so re-recording would be a
    /// no-op. Checking the grid too keeps this self-healing when the surface is
    /// painted before the session's view exists.
    pub(in crate::features) fn terminal_surface_bounds_tracking_is_current(
        &self,
        session_id: Option<&str>,
        bounds: gpui::Bounds<gpui::Pixels>,
        scale_factor: f32,
    ) -> bool {
        if self.terminal.layout.scale_factor != scale_factor {
            return false;
        }
        if self.recorded_terminal_surface_bounds_for_session(session_id) != Some(bounds) {
            return false;
        }
        let TerminalResizeGeometry {
            cols,
            rows,
            pixel_width,
            pixel_height,
        } = self.terminal_resize_geometry_for_bounds_for_session(bounds, session_id);
        match session_id.filter(|id| !id.is_empty()) {
            Some(session_id) => match self.terminal.view.views.get(session_id) {
                Some(view) => {
                    view.screen.cols() as u16 == cols
                        && view.screen.rows() as u16 == rows
                        && !view.backend_resize_changed(cols, rows, pixel_width, pixel_height)
                }
                None => true,
            },
            None => {
                self.terminal.view.screen.cols() as u16 == cols
                    && self.terminal.view.screen.rows() as u16 == rows
            }
        }
    }

    pub(in crate::features) fn remember_terminal_scrollbar_track_bounds_for_session(
        &mut self,
        session_id: Option<&str>,
        bounds: gpui::Bounds<gpui::Pixels>,
    ) -> bool {
        if let Some(session_id) = session_id.filter(|id| !id.is_empty()) {
            if self
                .terminal
                .layout
                .session_scrollbar_track_bounds
                .get(session_id)
                .is_some_and(|previous| *previous == bounds)
            {
                return false;
            }
            self.terminal
                .layout
                .session_scrollbar_track_bounds
                .insert(session_id.to_string(), bounds);
            if self.session.active_id() == Some(session_id) {
                self.terminal.layout.scrollbar_track_bounds = Some(bounds);
            }
        } else {
            if self.terminal.layout.scrollbar_track_bounds == Some(bounds) {
                return false;
            }
            self.terminal.layout.scrollbar_track_bounds = Some(bounds);
        }
        true
    }

    pub(in crate::features) fn terminal_scrollbar_track_bounds_for_session(
        &self,
        session_id: Option<&str>,
    ) -> Option<gpui::Bounds<gpui::Pixels>> {
        terminal_scrollbar_track_bounds_for_session_state(
            &self.terminal.layout.session_scrollbar_track_bounds,
            self.terminal.layout.scrollbar_track_bounds,
            session_id,
        )
    }

    pub(in crate::features) fn terminal_scrollbar_metrics_for_session(
        &self,
        session_id: Option<&str>,
        bounds: gpui::Bounds<gpui::Pixels>,
    ) -> crate::features::terminal::terminal_surface::TerminalScrollbarMetrics {
        let viewport_rows = self.terminal_viewport_rows_for_session(session_id);
        let scrollback_rows = self.terminal_scroll_max_for_session(session_id);
        let scroll_offset = self.terminal_scroll_offset_for_session(session_id);
        terminal_scrollbar_metrics(TerminalScrollbarInput {
            viewport_rows,
            scrollback_rows,
            scroll_offset,
            track_height: track_height(bounds),
            min_thumb_height: TERMINAL_SCROLLBAR_MIN_THUMB_HEIGHT,
        })
    }

    fn terminal_scroll_offset_for_session(&self, session_id: Option<&str>) -> usize {
        if let Some(session_id) = session_id.filter(|id| !id.is_empty()) {
            return self
                .terminal
                .view
                .views
                .get(session_id)
                .map(|view| view.scroll_offset)
                .unwrap_or(0);
        }
        self.terminal.view.scroll_offset
    }

    pub(in crate::features) fn active_terminal_page_rows(&self) -> usize {
        // Prefer live screen rows when available; fall back to classic 24-row page.
        if let Some(session_id) = self.session.active_id()
            && let Some(view) = self.terminal.view.views.get(session_id)
        {
            let rows = view.viewport_rows_for_ui();
            if rows > 0 {
                return rows;
            }
        }
        let rows = self.terminal_snapshot_for_session(None, 0).row_count();
        if rows > 0 { rows } else { 24 }
    }

    pub(in crate::features) fn desired_terminal_resize_geometry(
        &self,
    ) -> Option<TerminalResizeGeometry> {
        self.desired_terminal_resize_geometry_for_session_hint(self.session.active_id())
    }

    pub(in crate::features) fn desired_terminal_resize_geometry_for_session_hint(
        &self,
        session_id: Option<&str>,
    ) -> Option<TerminalResizeGeometry> {
        let bounds = session_id
            .filter(|id| !id.is_empty())
            .and_then(|session_id| {
                self.terminal
                    .layout
                    .session_surface_bounds
                    .get(session_id)
                    .copied()
            })
            .or_else(|| {
                self.session.active_id().and_then(|session_id| {
                    self.terminal
                        .layout
                        .session_surface_bounds
                        .get(session_id)
                        .copied()
                })
            })
            .or(self.terminal.layout.surface_bounds)?;
        Some(self.terminal_resize_geometry_for_bounds_for_session(bounds, session_id))
    }

    pub(in crate::features) fn terminal_resize_geometry_for_bounds_for_session(
        &self,
        bounds: gpui::Bounds<gpui::Pixels>,
        session_id: Option<&str>,
    ) -> TerminalResizeGeometry {
        let (cell_w, cell_h) = self.terminal_cell_size();
        let insets = self.terminal_content_insets_for_bounds(session_id, bounds);
        let gutter = self.terminal_gutter_width_px_for_session(session_id);
        terminal_resize_geometry_for_size_with_insets_and_scale(
            f32::from(bounds.size.width),
            f32::from(bounds.size.height),
            cell_w,
            cell_h,
            insets,
            gutter,
            self.terminal.layout.scale_factor,
        )
    }

    pub(in crate::features) fn resize_all_known_terminal_surfaces(&mut self) -> bool {
        let mut dirty = false;
        if let Some(bounds) = self.terminal.layout.surface_bounds {
            dirty |= self.resize_terminal_to_bounds_for_session(None, bounds);
        }
        let bounds_by_session = self
            .terminal
            .layout
            .session_surface_bounds
            .iter()
            .map(|(session_id, bounds)| (session_id.clone(), *bounds))
            .collect::<Vec<_>>();
        for (session_id, bounds) in bounds_by_session {
            dirty |= self.resize_terminal_to_bounds_for_session(Some(&session_id), bounds);
        }
        dirty
    }

    pub(in crate::features) fn resize_terminal_to_bounds_for_session(
        &mut self,
        session_id: Option<&str>,
        bounds: gpui::Bounds<gpui::Pixels>,
    ) -> bool {
        let (cell_width, cell_height) = self.terminal_cell_size();
        let snapped_cell_height =
            terminal_snapped_cell_height(cell_height, self.terminal.layout.scale_factor);
        let TerminalResizeGeometry {
            cols,
            rows,
            pixel_width,
            pixel_height,
        } = self.terminal_resize_geometry_for_bounds_for_session(bounds, session_id);
        if let Some(session_id) = session_id.filter(|id| !id.is_empty()) {
            let Some(view) = self.terminal.view.views.get_mut(session_id) else {
                return false;
            };
            let current_rows = view.screen.rows() as u16;
            let current_cols = view.screen.cols() as u16;
            let grid_changed = current_rows != rows || current_cols != cols;
            let backend_changed =
                view.backend_resize_changed(cols, rows, pixel_width, pixel_height);
            if !grid_changed && !backend_changed {
                return false;
            }
            tracing::info!(
                diagnostic = "terminal_resize",
                session_id,
                scale_factor = self.terminal.layout.scale_factor,
                bounds_width = f32::from(bounds.size.width),
                bounds_height = f32::from(bounds.size.height),
                cell_width,
                cell_height,
                snapped_cell_height,
                cols,
                rows,
                pixel_width,
                pixel_height,
                grid_changed,
                backend_changed,
                "resizing terminal viewport"
            );
            if grid_changed {
                view.screen.resize(cols, rows);
                view.clear_scrollback_query_caches();
                view.grid_resize_pending = true;
                view.frame_action_links = None;
                self.terminal.view.scroll_delta_residuals.remove(session_id);
                self.terminal.view.frame_pipeline.resize_session(
                    session_id.to_string(),
                    cols,
                    rows,
                );
            }
            if backend_changed {
                view.remember_backend_resize(cols, rows, pixel_width, pixel_height);
                let _ = self.session.manager().resize_with_pixels(
                    session_id,
                    cols,
                    rows,
                    pixel_width,
                    pixel_height,
                );
            }
            if grid_changed {
                self.clear_terminal_selection_state_for_session(session_id);
            }
        } else {
            let current_rows = self.terminal.view.screen.rows() as u16;
            let current_cols = self.terminal.view.screen.cols() as u16;
            if current_rows == rows && current_cols == cols {
                return false;
            }
            self.terminal.view.screen.resize(cols, rows);
            self.clear_terminal_scroll_residual_for_session(None);
        }
        true
    }

    /// Shift+PageUp/PageDown/Home/End (and Ctrl+Shift+Up/Down) navigate local scrollback
    /// without sending CSI sequences to the remote PTY — common terminal emulator UX.
    pub(in crate::features) fn handle_terminal_scroll_key(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let keystroke = &event.keystroke;
        let key = keystroke.key.as_str();
        let shift = keystroke.modifiers.shift;
        let control = keystroke.modifiers.control;
        let alt = keystroke.modifiers.alt;
        let platform = keystroke.modifiers.platform;
        let function = keystroke.modifiers.function;
        if alt || platform || function {
            return false;
        }

        let page = self.active_terminal_page_rows().max(1) as i32;
        if shift && !control {
            match key {
                "pageup" => {
                    self.scroll_terminal_by(page, cx);
                    return true;
                }
                "pagedown" => {
                    self.scroll_terminal_by(-page, cx);
                    return true;
                }
                "home" => {
                    self.scroll_terminal_to_top(cx);
                    return true;
                }
                "end" => {
                    self.scroll_terminal_to_bottom(cx);
                    return true;
                }
                _ => {}
            }
        }
        if shift && control {
            match key {
                "up" => {
                    self.scroll_terminal_by(1, cx);
                    return true;
                }
                "down" => {
                    self.scroll_terminal_by(-1, cx);
                    return true;
                }
                _ => {}
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use gpui::{AppContext as _, Bounds, TestAppContext, point, px, size};
    use nyaterm_core::{
        AppRuntime, RuntimeMode, TerminalResizeGeometry, TerminalViewportInsets,
        terminal_resize_geometry_for_size_with_insets, uuid,
    };

    use crate::entities::{OverlayStore, StartupRestoreStore, UiStoreHandles};
    use crate::features::NyaTermApp;

    use super::{
        TERMINAL_SCROLL_POSITION_NOTIFY_DELAY, TERMINAL_SCROLLBAR_DRAG_NOTIFY_DELAY,
        TerminalScrollVisualState, terminal_display_offset_from_state,
        terminal_fractional_scroll_prefetch_offset, terminal_local_scroll_delta_lines_from_state,
        terminal_scroll_delta_lines_from_raw, terminal_scroll_needs_text_first_repaint,
        terminal_scroll_offset_reanchored_for_scrollback_growth,
        terminal_scroll_offset_state_needs_update, terminal_scroll_predictive_prefetch_offset,
        terminal_scroll_residual_clamped_for_offset, terminal_scroll_should_consume_raw_lines,
        terminal_scroll_should_request_immediate_text_snapshot,
        terminal_scroll_to_bottom_state_needs_update,
        terminal_scrollbar_track_bounds_for_session_state, terminal_should_apply_session_cwd,
        terminal_user_scroll_idle_remaining_delay, terminal_visual_scroll_active_for_state,
    };

    fn terminal_resize_geometry_for_bounds(
        bounds: Bounds<gpui::Pixels>,
        cell_width: f32,
        cell_height: f32,
        padding: f32,
        gutter_width: f32,
    ) -> TerminalResizeGeometry {
        terminal_resize_geometry_for_size_with_insets(
            f32::from(bounds.size.width),
            f32::from(bounds.size.height),
            cell_width,
            cell_height,
            TerminalViewportInsets::symmetric(padding),
            gutter_width,
        )
    }

    #[test]
    fn terminal_cwd_sync_applies_only_to_an_active_enabled_session() {
        assert!(terminal_should_apply_session_cwd(
            true,
            true,
            true,
            false,
            "/home/nya"
        ));
        assert!(!terminal_should_apply_session_cwd(
            false,
            true,
            true,
            false,
            "/home/nya"
        ));
        assert!(!terminal_should_apply_session_cwd(
            true,
            false,
            true,
            false,
            "/home/nya"
        ));
        assert!(!terminal_should_apply_session_cwd(
            true,
            true,
            false,
            false,
            "/home/nya"
        ));
        assert!(!terminal_should_apply_session_cwd(
            true, true, true, false, "   "
        ));
    }

    #[test]
    fn terminal_cwd_sync_does_not_replace_a_path_being_edited() {
        assert!(!terminal_should_apply_session_cwd(
            true,
            true,
            true,
            true,
            "/home/nya"
        ));
    }

    #[test]
    fn terminal_scroll_delta_accumulates_fractional_lines() {
        let mut residual = 0.0;

        assert_eq!(terminal_scroll_delta_lines_from_raw(&mut residual, 0.35), 0);
        assert_eq!(terminal_scroll_delta_lines_from_raw(&mut residual, 0.35), 0);
        assert_eq!(terminal_scroll_delta_lines_from_raw(&mut residual, 0.35), 1);
        assert!((residual - 0.05).abs() < f32::EPSILON * 4.0);
    }

    #[test]
    fn terminal_scroll_delta_resets_residual_on_direction_change() {
        let mut residual = 0.8;

        assert_eq!(terminal_scroll_delta_lines_from_raw(&mut residual, -0.4), 0);
        assert!((residual + 0.4).abs() < f32::EPSILON * 4.0);
        assert_eq!(
            terminal_scroll_delta_lines_from_raw(&mut residual, -0.7),
            -1
        );
        assert!((residual + 0.1).abs() < f32::EPSILON * 8.0);
    }

    #[test]
    fn terminal_user_scroll_idle_remaining_delay_uses_remainder_of_window() {
        let now = Instant::now();
        let active_window = Duration::from_millis(140);

        assert_eq!(
            terminal_user_scroll_idle_remaining_delay(Some(now), now, active_window),
            Some(active_window)
        );
        assert_eq!(
            terminal_user_scroll_idle_remaining_delay(
                Some(now - Duration::from_millis(1)),
                now,
                active_window
            ),
            Some(active_window - Duration::from_millis(1))
        );
        assert_eq!(
            terminal_user_scroll_idle_remaining_delay(
                Some(now - active_window),
                now,
                active_window
            ),
            None
        );
        assert_eq!(
            terminal_user_scroll_idle_remaining_delay(
                Some(now - active_window - Duration::from_millis(1)),
                now,
                active_window
            ),
            None
        );
        assert_eq!(
            terminal_user_scroll_idle_remaining_delay(None, now, active_window),
            None
        );
    }

    #[test]
    fn terminal_local_scroll_delta_reverses_fractional_motion_smoothly() {
        assert_eq!(
            terminal_local_scroll_delta_lines_from_state(0, 0.8, 10, -0.4),
            (0, 0.4)
        );
        assert_eq!(
            terminal_local_scroll_delta_lines_from_state(0, 0.8, 10, -1.2),
            (0, 0.0)
        );
    }

    #[test]
    fn terminal_local_scroll_delta_preserves_reverse_direction_for_prefetch() {
        let (delta, residual) = terminal_local_scroll_delta_lines_from_state(3, 0.2, 10, -0.5);

        assert_eq!(delta, 0);
        assert!((residual + 0.3).abs() < f32::EPSILON * 8.0);
    }

    #[test]
    fn terminal_local_scroll_delta_clamps_edges_without_negative_residual() {
        assert_eq!(
            terminal_local_scroll_delta_lines_from_state(10, 0.0, 10, 0.6),
            (0, 0.0)
        );
        assert_eq!(
            terminal_local_scroll_delta_lines_from_state(9, 0.8, 10, 0.6),
            (1, 0.0)
        );
        assert_eq!(
            terminal_local_scroll_delta_lines_from_state(0, 0.0, 10, -0.6),
            (0, 0.0)
        );
    }

    #[test]
    fn terminal_display_offset_from_state_keeps_fractional_scroll_visual_only() {
        assert_eq!(terminal_display_offset_from_state(0, 0.0, 8), 0);
        assert_eq!(terminal_display_offset_from_state(0, 0.2, 8), 0);
        assert_eq!(terminal_display_offset_from_state(0, 0.5, 8), 0);
        assert_eq!(terminal_display_offset_from_state(0, 0.95, 8), 0);
        assert_eq!(terminal_display_offset_from_state(3, -0.4, 8), 3);
        assert_eq!(terminal_display_offset_from_state(3, -0.6, 8), 3);
        assert_eq!(terminal_display_offset_from_state(8, 0.7, 8), 8);
    }

    #[test]
    fn terminal_visual_scroll_active_tracks_offset_or_fractional_residual() {
        assert!(!terminal_visual_scroll_active_for_state(0, 0.0));
        assert!(terminal_visual_scroll_active_for_state(1, 0.0));
        assert!(terminal_visual_scroll_active_for_state(0, 0.25));
        assert!(terminal_visual_scroll_active_for_state(0, -0.25));
        assert!(!terminal_visual_scroll_active_for_state(0, f32::NAN));
    }

    #[test]
    fn terminal_fractional_scroll_prefetch_offset_tracks_next_likely_row() {
        assert_eq!(
            terminal_fractional_scroll_prefetch_offset(0, 0.25, 8),
            Some(1)
        );
        assert_eq!(
            terminal_fractional_scroll_prefetch_offset(3, 0.25, 8),
            Some(4)
        );
        assert_eq!(
            terminal_fractional_scroll_prefetch_offset(3, -0.25, 8),
            Some(2)
        );
    }

    #[test]
    fn terminal_fractional_scroll_prefetch_offset_ignores_live_and_edges() {
        assert_eq!(terminal_fractional_scroll_prefetch_offset(0, 0.25, 0), None);
        assert_eq!(terminal_fractional_scroll_prefetch_offset(0, 0.0, 8), None);
        assert_eq!(
            terminal_fractional_scroll_prefetch_offset(0, -0.25, 8),
            None
        );
        assert_eq!(terminal_fractional_scroll_prefetch_offset(8, 0.25, 8), None);
    }

    #[test]
    fn terminal_scroll_reanchors_surface_offset_after_output_growth() {
        assert_eq!(
            terminal_scroll_offset_reanchored_for_scrollback_growth(0, 10, 14),
            0
        );
        assert_eq!(
            terminal_scroll_offset_reanchored_for_scrollback_growth(3, 10, 14),
            7
        );
        assert_eq!(
            terminal_scroll_offset_reanchored_for_scrollback_growth(3, 14, 10),
            3
        );
    }

    #[test]
    fn terminal_scroll_residual_clamps_at_edges() {
        assert_eq!(terminal_scroll_residual_clamped_for_offset(0, -0.2, 8), 0.0);
        assert_eq!(terminal_scroll_residual_clamped_for_offset(8, 0.2, 8), 0.0);
        assert_eq!(terminal_scroll_residual_clamped_for_offset(4, 0.2, 8), 0.2);
        assert_eq!(
            terminal_scroll_residual_clamped_for_offset(4, f32::NAN, 8),
            0.0
        );
    }

    #[test]
    fn terminal_scroll_predictive_prefetch_tracks_scroll_direction() {
        assert_eq!(
            terminal_scroll_predictive_prefetch_offset(20, 1, 40, 200),
            Some(40)
        );
        assert_eq!(
            terminal_scroll_predictive_prefetch_offset(20, -1, 40, 200),
            None
        );
        assert_eq!(
            terminal_scroll_predictive_prefetch_offset(80, -3, 40, 200),
            Some(60)
        );
        assert_eq!(
            terminal_scroll_predictive_prefetch_offset(190, 12, 40, 200),
            Some(200)
        );
    }

    #[test]
    fn terminal_scroll_position_notify_delay_is_frame_coalesced() {
        assert_eq!(
            TERMINAL_SCROLL_POSITION_NOTIFY_DELAY,
            Duration::from_millis(16)
        );
        assert_eq!(
            TERMINAL_SCROLLBAR_DRAG_NOTIFY_DELAY,
            Duration::from_millis(8)
        );
    }

    #[test]
    fn terminal_scroll_immediate_text_snapshot_only_for_uncached_scrolled_targets() {
        assert!(!terminal_scroll_should_request_immediate_text_snapshot(
            0, false
        ));
        assert!(!terminal_scroll_should_request_immediate_text_snapshot(
            4, true
        ));
        assert!(terminal_scroll_should_request_immediate_text_snapshot(
            4, false
        ));
    }

    #[test]
    fn terminal_scroll_to_bottom_state_detects_noop_bottom_state() {
        assert!(!terminal_scroll_to_bottom_state_needs_update(0, 0.0, false));
        assert!(!terminal_scroll_to_bottom_state_needs_update(
            0,
            f32::NAN,
            false
        ));
    }

    #[test]
    fn terminal_scroll_to_bottom_state_detects_pending_scroll_state() {
        assert!(terminal_scroll_to_bottom_state_needs_update(2, 0.0, false));
        assert!(terminal_scroll_to_bottom_state_needs_update(0, 0.25, false));
        assert!(terminal_scroll_to_bottom_state_needs_update(0, 0.0, true));
    }

    #[test]
    fn terminal_scroll_offset_state_skips_identical_offset() {
        assert!(!terminal_scroll_offset_state_needs_update(4, 0.0, false, 4));
        assert!(terminal_scroll_offset_state_needs_update(4, 0.0, false, 5));
    }

    #[test]
    fn terminal_scroll_offset_state_keeps_bottom_cleanup() {
        assert!(terminal_scroll_offset_state_needs_update(0, 0.25, false, 0));
        assert!(terminal_scroll_offset_state_needs_update(0, 0.0, true, 0));
        assert!(!terminal_scroll_offset_state_needs_update(
            0,
            f32::NAN,
            false,
            0
        ));
    }

    #[test]
    fn terminal_scroll_text_first_repaint_only_when_scrolled_text_missing() {
        let mut state = TerminalScrollVisualState {
            session_id: "session".to_string(),
            scroll_offset: 1,
            scroll_residual_lines: 0.0,
            display_offset: 1,
            scrollback_len: 10,
            viewport_rows: 4,
            has_new_while_scrolled: false,
            performance_overlay: None,
            skipped_output_chars: 0,
        };

        assert!(terminal_scroll_needs_text_first_repaint(&state, false));
        assert!(!terminal_scroll_needs_text_first_repaint(&state, true));
        state.display_offset = 0;
        assert!(!terminal_scroll_needs_text_first_repaint(&state, false));
    }

    #[test]
    fn terminal_scroll_consumes_finite_nonzero_wheel_delta() {
        assert!(terminal_scroll_should_consume_raw_lines(0.05));
        assert!(terminal_scroll_should_consume_raw_lines(-0.05));
        assert!(!terminal_scroll_should_consume_raw_lines(0.0));
        assert!(!terminal_scroll_should_consume_raw_lines(f32::NAN));
        assert!(!terminal_scroll_should_consume_raw_lines(f32::INFINITY));
    }

    #[test]
    fn terminal_resize_geometry_snaps_to_complete_rows() {
        let bounds = Bounds::new(point(px(0.), px(0.)), size(px(812.), px(612.)));

        let geometry = terminal_resize_geometry_for_bounds(bounds, 10., 20., 8., 72.);

        assert_eq!(
            geometry,
            TerminalResizeGeometry {
                cols: 72,
                rows: 29,
                pixel_width: 724,
                pixel_height: 580,
            }
        );
    }

    #[test]
    fn terminal_resize_geometry_uses_text_bounds_excluding_scrollbar_column() {
        let outer_width = 810.;
        let text_width = outer_width
            - crate::features::terminal::terminal_surface::TERMINAL_SCROLLBAR_COLUMN_WIDTH;
        let bounds = Bounds::new(point(px(0.), px(0.)), size(px(text_width), px(480.)));

        let geometry = terminal_resize_geometry_for_bounds(bounds, 10., 20., 0., 0.);

        assert_eq!(geometry.cols, 79);
        assert_eq!(geometry.pixel_width, text_width as u16);
    }

    #[test]
    fn terminal_resize_geometry_clamps_grid_but_keeps_nonzero_pixels() {
        let bounds = Bounds::new(point(px(0.), px(0.)), size(px(10.), px(10.)));

        let geometry = terminal_resize_geometry_for_bounds(bounds, 10., 20., 8., 72.);

        assert_eq!(geometry.cols, 20);
        assert_eq!(geometry.rows, 4);
        assert_eq!(geometry.pixel_width, 200);
        assert_eq!(geometry.pixel_height, 80);
    }

    #[test]
    fn terminal_scrollbar_track_bounds_never_fall_back_across_split_sessions() {
        let fallback = Bounds::new(point(px(1.), px(2.)), size(px(8.), px(120.)));
        let left = Bounds::new(point(px(101.), px(22.)), size(px(8.), px(80.)));
        let session_bounds = std::collections::HashMap::from([("left".to_string(), left)]);

        assert_eq!(
            terminal_scrollbar_track_bounds_for_session_state(
                &session_bounds,
                Some(fallback),
                Some("left")
            ),
            Some(left)
        );
        assert_eq!(
            terminal_scrollbar_track_bounds_for_session_state(
                &session_bounds,
                Some(fallback),
                Some("right")
            ),
            None
        );
        assert_eq!(
            terminal_scrollbar_track_bounds_for_session_state(
                &session_bounds,
                Some(fallback),
                None
            ),
            Some(fallback)
        );
    }

    fn app_for_resize(cx: &mut TestAppContext) -> gpui::Entity<NyaTermApp> {
        // A uuid rather than a clock reading: these tests run in parallel and
        // Windows' clock granularity lets a timestamp repeat, which would share
        // one config dir and so one settings database.
        let root = std::env::temp_dir().join(format!(
            "nyaterm-terminal-resize-{}-{}",
            std::process::id(),
            uuid()
        ));
        let runtime = AppRuntime::from_parts_for_test(
            RuntimeMode::Portable,
            root.clone(),
            root.join("config"),
            root.join("logs"),
            root.join("cache"),
            None,
        );
        let stores = UiStoreHandles {
            startup_restore: cx.new(|_| StartupRestoreStore::default()),
            overlays: cx.new(|_| OverlayStore::default()),
        };
        cx.new(|cx| NyaTermApp::new(runtime, stores, cx))
    }

    #[test]
    fn a_second_tab_is_measured_even_though_it_paints_the_same_rect_as_the_first() {
        let mut cx = TestAppContext::single();
        let app = app_for_resize(&mut cx);
        let bounds = Bounds::new(point(px(0.), px(0.)), size(px(1200.), px(700.)));

        cx.update_entity(&app, |app, _| {
            app.terminal
                .seed_session_view("first".to_string(), String::new(), "UTF-8");
            app.session.select_active_session("first");
            let scale_factor = app.terminal.layout.scale_factor;

            assert!(app.remember_terminal_surface_bounds_for_session(Some("first"), bounds));
            let first = app.terminal.view.views.get("first").expect("first view");
            let (cols, rows) = (first.screen.cols(), first.screen.rows());
            assert!(
                cols > 80 && rows > 24,
                "expected {cols}x{rows} to fill 1200x700"
            );

            // Opening a second tab: same rect, and `first` already recorded it.
            app.terminal
                .seed_session_view("second".to_string(), String::new(), "UTF-8");
            // The hit-test accessor deliberately falls back to the active pane's
            // bounds, which is exactly why the tracker must not use it here.
            assert_eq!(
                app.terminal_surface_bounds_for_session(Some("second")),
                Some(bounds)
            );
            assert!(!app.terminal_surface_bounds_tracking_is_current(
                Some("second"),
                bounds,
                scale_factor
            ));

            assert!(app.remember_terminal_surface_bounds_for_session(Some("second"), bounds));
            let second = app.terminal.view.views.get("second").expect("second view");
            assert_eq!((second.screen.cols(), second.screen.rows()), (cols, rows));
            assert!(app.terminal_surface_bounds_tracking_is_current(
                Some("second"),
                bounds,
                scale_factor
            ));
        });
    }

    #[test]
    fn a_view_created_after_its_bounds_were_recorded_still_gets_measured() {
        let mut cx = TestAppContext::single();
        let app = app_for_resize(&mut cx);
        let bounds = Bounds::new(point(px(0.), px(0.)), size(px(1200.), px(700.)));

        cx.update_entity(&app, |app, _| {
            let scale_factor = app.terminal.layout.scale_factor;
            // The surface paints while the session is still connecting, so there
            // is no view to resize yet.
            assert!(!app.remember_terminal_surface_bounds_for_session(Some("pending"), bounds));
            app.terminal
                .seed_session_view("pending".to_string(), String::new(), "UTF-8");

            assert!(!app.terminal_surface_bounds_tracking_is_current(
                Some("pending"),
                bounds,
                scale_factor
            ));
            assert!(app.remember_terminal_surface_bounds_for_session(Some("pending"), bounds));
            let view = app.terminal.view.views.get("pending").expect("view");
            assert!(view.screen.cols() > 80 && view.screen.rows() > 24);
        });
    }
}
