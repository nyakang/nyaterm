use std::collections::HashSet;
use std::time::Instant;

use futures::channel::mpsc::UnboundedReceiver;
use gpui::Subscription;

use crate::models::event_wake::{ANY_INTEREST, EventWake};

use super::state::ShellFeatureState;

/// GPUI event-pump, repaint and shell-persistence scheduling state.
pub(super) struct ShellRuntimeState {
    /// Application shortcut dispatch must not depend on the focused element's
    /// action ancestry. This subscription owns the single main-window interceptor.
    pub(super) shortcut_interceptor: Option<Subscription>,
    pub(super) session_event_backlog_active: bool,
    pub(super) session_event_queued_events: usize,
    pub(super) session_event_queued_output_bytes: usize,
    pub(super) session_event_dropped_output_bytes: u64,
    pub(super) session_event_last_output_event_count: usize,
    pub(super) session_event_last_drained_output_bytes: usize,
    pub(super) last_pending_session_status_at: Option<Instant>,
    pub(super) last_terminal_frame_apply_at: Option<Instant>,
    /// Last user-driven terminal scroll input. During this short window the
    /// terminal paint path favors text/position over enhanced decorations.
    pub(super) last_terminal_user_scroll_at: Option<Instant>,
    /// Last successful user terminal input write. During this short window the
    /// terminal paint path favors low-latency echo over enhanced decorations.
    pub(super) last_terminal_input_at: Option<Instant>,
    /// Sessions whose scroll position changed and should repaint on the next frame tick.
    pub(super) pending_terminal_scroll_position_sessions: HashSet<String>,
    /// Sessions already scrolled locally by TerminalSurface; only snapshot requests
    /// should run on the app sync tick.
    pub(super) pending_terminal_scroll_snapshot_only_sessions: HashSet<String>,
    /// True while a frame-coalesced scroll-position repaint task is armed.
    pub(super) terminal_scroll_position_notify_armed: bool,
    /// Sessions whose scrollbar drag position changed and should repaint soon.
    pub(super) pending_terminal_scrollbar_drag_sessions: HashSet<String>,
    /// True while a coalesced scrollbar-drag visual repaint task is armed.
    pub(super) terminal_scrollbar_drag_notify_armed: bool,
    /// Sessions whose selection drag changed and should repaint soon.
    pub(super) pending_terminal_selection_drag_sessions: HashSet<String>,
    /// True while a coalesced selection-drag visual repaint task is armed.
    pub(super) terminal_selection_drag_notify_armed: bool,
    /// Sessions that need a full decoration repaint once user scrolling idles.
    pub(super) pending_terminal_user_scroll_idle_sessions: HashSet<String>,
    /// True while a delayed scroll-idle repaint task is armed.
    pub(super) terminal_user_scroll_idle_notify_armed: bool,
    /// Sessions that need a full decoration repaint once typing idles.
    pub(super) pending_terminal_input_idle_sessions: HashSet<String>,
    /// True while a delayed input-idle repaint task is armed.
    pub(super) terminal_input_idle_notify_armed: bool,
    /// After connect success, demote idle/visual work until this time (no faster tick).
    pub(super) connect_settle_until: Option<Instant>,
    /// A short post-input pump task is armed to drain echo output/frame events.
    pub(super) terminal_input_wake_armed: bool,
    /// Incremented on every user input write so an armed wake can extend itself.
    pub(super) terminal_input_wake_generation: u64,
    /// Full NyaTermApp shell paints (chrome + workspace structure).
    pub(super) full_shell_paint_count: u64,
    /// Output frames that notified only a TerminalSurface.
    pub(super) terminal_surface_frame_notify_count: u64,
    /// Output frames that also dirtied chrome (unread/effects).
    pub(super) terminal_chrome_frame_notify_count: u64,
    /// Open-tabs / window-layout settings need a durable write.
    pub(super) open_tabs_persist_dirty: bool,
    pub(super) window_layout_persist_dirty: bool,
    pub(super) ui_layout_persist_pending: bool,
    /// Signalled whenever one of the three flags above is marked, so the debounce
    /// task wakes without anything polling them. Taken once at window open.
    persist_wake: EventWake,
    persist_wake_rx: Option<UnboundedReceiver<()>>,
    session_persistence_generation: u64,
    session_persistence_in_flight: Option<u64>,
    pub(super) cursor_blink_on: bool,
    /// True while the blink clock task is alive. The clock is its own deadline, so
    /// there is no "next toggle at" instant to keep here any more.
    cursor_blink_clock_armed: bool,
    /// True while the header date/time clock task is alive.
    header_status_clock_armed: bool,
    /// True while the "still connecting" status clock task is alive.
    pending_session_status_clock_armed: bool,
    /// True while the idle screen-lock clock task is alive.
    idle_lock_clock_armed: bool,
    /// True while the deferred-focus clock task is alive.
    pending_focus_clock_armed: bool,
    /// True while the terminal recovery-accounting clock task is alive.
    terminal_recovery_clock_armed: bool,
    /// True while the deferred bootstrap-work clock task is alive.
    post_start_work_clock_armed: bool,
    /// True while the drop-highlight clock task is alive.
    drop_hover_clock_armed: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(in crate::features) struct ShellPersistenceDirty {
    open_tabs: bool,
    window_layout: bool,
}

impl ShellPersistenceDirty {
    pub(in crate::features) fn open_tabs(self) -> bool {
        self.open_tabs
    }

    pub(in crate::features) fn window_layout(self) -> bool {
        self.window_layout
    }

    pub(in crate::features) fn is_empty(self) -> bool {
        !self.open_tabs && !self.window_layout
    }
}

impl Default for ShellRuntimeState {
    fn default() -> Self {
        let (persist_wake, persist_wake_rx) = EventWake::new();
        Self {
            shortcut_interceptor: None,
            session_event_backlog_active: false,
            session_event_queued_events: 0,
            session_event_queued_output_bytes: 0,
            session_event_dropped_output_bytes: 0,
            session_event_last_output_event_count: 0,
            session_event_last_drained_output_bytes: 0,
            last_pending_session_status_at: None,
            last_terminal_frame_apply_at: None,
            last_terminal_user_scroll_at: None,
            last_terminal_input_at: None,
            pending_terminal_scroll_position_sessions: HashSet::new(),
            pending_terminal_scroll_snapshot_only_sessions: HashSet::new(),
            terminal_scroll_position_notify_armed: false,
            pending_terminal_scrollbar_drag_sessions: HashSet::new(),
            terminal_scrollbar_drag_notify_armed: false,
            pending_terminal_selection_drag_sessions: HashSet::new(),
            terminal_selection_drag_notify_armed: false,
            pending_terminal_user_scroll_idle_sessions: HashSet::new(),
            terminal_user_scroll_idle_notify_armed: false,
            pending_terminal_input_idle_sessions: HashSet::new(),
            terminal_input_idle_notify_armed: false,
            connect_settle_until: None,
            terminal_input_wake_armed: false,
            terminal_input_wake_generation: 0,
            full_shell_paint_count: 0,
            terminal_surface_frame_notify_count: 0,
            terminal_chrome_frame_notify_count: 0,
            open_tabs_persist_dirty: false,
            window_layout_persist_dirty: false,
            ui_layout_persist_pending: false,
            persist_wake,
            persist_wake_rx: Some(persist_wake_rx),
            session_persistence_generation: 0,
            session_persistence_in_flight: None,
            cursor_blink_on: true,
            cursor_blink_clock_armed: false,
            header_status_clock_armed: false,
            pending_session_status_clock_armed: false,
            idle_lock_clock_armed: false,
            pending_focus_clock_armed: false,
            terminal_recovery_clock_armed: false,
            post_start_work_clock_armed: false,
            drop_hover_clock_armed: false,
        }
    }
}

impl ShellFeatureState {
    pub(in crate::features) fn note_full_shell_paint(&mut self) -> u64 {
        self.runtime.full_shell_paint_count = self.runtime.full_shell_paint_count.saturating_add(1);
        self.runtime.full_shell_paint_count
    }

    pub(in crate::features) fn connect_settle_active(&self, now: Instant) -> bool {
        self.runtime
            .connect_settle_until
            .is_some_and(|until| now < until)
    }

    pub(in crate::features) fn session_event_queued_output_bytes(&self) -> usize {
        self.runtime.session_event_queued_output_bytes
    }

    pub(in crate::features) fn terminal_frame_input_pressure(&self) -> (bool, usize) {
        (
            self.runtime.session_event_backlog_active,
            self.runtime.session_event_queued_output_bytes,
        )
    }

    pub(in crate::features) fn note_terminal_frame_apply(&mut self, at: Instant) {
        self.runtime.last_terminal_frame_apply_at = Some(at);
    }

    pub(in crate::features) fn note_terminal_surface_frame_notifies(&mut self, count: usize) {
        self.runtime.terminal_surface_frame_notify_count = self
            .runtime
            .terminal_surface_frame_notify_count
            .saturating_add(count as u64);
    }

    pub(in crate::features) fn note_terminal_chrome_frame_notify(&mut self) {
        self.runtime.terminal_chrome_frame_notify_count = self
            .runtime
            .terminal_chrome_frame_notify_count
            .saturating_add(1);
    }

    pub(in crate::features) fn last_terminal_input_at(&self) -> Option<Instant> {
        self.runtime.last_terminal_input_at
    }

    pub(in crate::features) fn last_terminal_user_scroll_at(&self) -> Option<Instant> {
        self.runtime.last_terminal_user_scroll_at
    }

    pub(in crate::features) fn terminal_user_scroll_idle_pending(&self, session_id: &str) -> bool {
        self.runtime
            .pending_terminal_user_scroll_idle_sessions
            .contains(session_id)
    }

    pub(in crate::features) fn cursor_blink_on(&self) -> bool {
        self.runtime.cursor_blink_on
    }

    pub(in crate::features) fn cursor_blink_clock_is_armed(&self) -> bool {
        self.runtime.cursor_blink_clock_armed
    }

    pub(in crate::features) fn set_cursor_blink_clock_armed(&mut self, armed: bool) {
        self.runtime.cursor_blink_clock_armed = armed;
    }

    pub(in crate::features) fn header_status_clock_is_armed(&self) -> bool {
        self.runtime.header_status_clock_armed
    }

    pub(in crate::features) fn set_header_status_clock_armed(&mut self, armed: bool) {
        self.runtime.header_status_clock_armed = armed;
    }

    pub(in crate::features) fn pending_session_status_clock_is_armed(&self) -> bool {
        self.runtime.pending_session_status_clock_armed
    }

    pub(in crate::features) fn set_pending_session_status_clock_armed(&mut self, armed: bool) {
        self.runtime.pending_session_status_clock_armed = armed;
    }

    pub(in crate::features) fn idle_lock_clock_is_armed(&self) -> bool {
        self.runtime.idle_lock_clock_armed
    }

    pub(in crate::features) fn set_idle_lock_clock_armed(&mut self, armed: bool) {
        self.runtime.idle_lock_clock_armed = armed;
    }

    pub(in crate::features) fn pending_focus_clock_is_armed(&self) -> bool {
        self.runtime.pending_focus_clock_armed
    }

    pub(in crate::features) fn set_pending_focus_clock_armed(&mut self, armed: bool) {
        self.runtime.pending_focus_clock_armed = armed;
    }

    pub(in crate::features) fn terminal_recovery_clock_is_armed(&self) -> bool {
        self.runtime.terminal_recovery_clock_armed
    }

    pub(in crate::features) fn set_terminal_recovery_clock_armed(&mut self, armed: bool) {
        self.runtime.terminal_recovery_clock_armed = armed;
    }

    pub(in crate::features) fn post_start_work_clock_is_armed(&self) -> bool {
        self.runtime.post_start_work_clock_armed
    }

    pub(in crate::features) fn set_post_start_work_clock_armed(&mut self, armed: bool) {
        self.runtime.post_start_work_clock_armed = armed;
    }

    pub(in crate::features) fn drop_hover_clock_is_armed(&self) -> bool {
        self.runtime.drop_hover_clock_armed
    }

    pub(in crate::features) fn set_drop_hover_clock_armed(&mut self, armed: bool) {
        self.runtime.drop_hover_clock_armed = armed;
    }

    pub(in crate::features) fn toggle_cursor_blink_phase(&mut self) {
        self.runtime.cursor_blink_on = !self.runtime.cursor_blink_on;
    }

    /// Restore the visible blink phase after cursor movement so the new position
    /// cannot remain hidden.
    pub(in crate::features) fn reset_cursor_blink_phase(&mut self) -> bool {
        self.set_cursor_blink_on(true)
    }

    /// Force the caret's blink phase. Returns whether it changed.
    pub(in crate::features) fn set_cursor_blink_on(&mut self, on: bool) -> bool {
        let changed = self.runtime.cursor_blink_on != on;
        self.runtime.cursor_blink_on = on;
        changed
    }

    /// Taken once by `NyaTermApp::start_shell_persistence_debounce`.
    pub(in crate::features) fn take_persist_wake_receiver(
        &mut self,
    ) -> Option<UnboundedReceiver<()>> {
        self.runtime.persist_wake_rx.take()
    }

    /// Declare interest in the next dirty mark. Call before checking the flags; see
    /// [`crate::models::event_wake`] for why the other order loses wakes.
    pub(in crate::features) fn arm_persist_wake(&self) {
        self.runtime.persist_wake.arm(ANY_INTEREST);
    }

    /// The one place a persistence mark signals its debounce task.
    ///
    /// Every `mark_*` below routes through here rather than writing the flag
    /// directly, so a mark added later cannot silently get no writer. That is the
    /// defect these three flags have already produced three times: a flag whose only
    /// driver was the idle plane, and a gate that had to name it.
    fn signal_persist_wake(&self) {
        self.runtime.persist_wake.signal(ANY_INTEREST);
    }

    pub(in crate::features) fn mark_open_tabs_persist_dirty(&mut self) {
        self.runtime.open_tabs_persist_dirty = true;
        self.signal_persist_wake();
    }

    pub(in crate::features) fn mark_window_layout_persist_dirty(&mut self) {
        self.runtime.window_layout_persist_dirty = true;
        self.signal_persist_wake();
    }

    pub(in crate::features) fn mark_ui_layout_persist_pending(&mut self) {
        self.runtime.ui_layout_persist_pending = true;
        self.signal_persist_wake();
    }

    pub(in crate::features) fn take_ui_layout_persist_pending(&mut self) -> bool {
        std::mem::take(&mut self.runtime.ui_layout_persist_pending)
    }

    #[cfg(test)]
    pub(in crate::features) fn ui_layout_persist_is_pending(&self) -> bool {
        self.runtime.ui_layout_persist_pending
    }

    /// Whether anything still owes a durable write, for the debounce task's decision
    /// to come back or park.
    pub(in crate::features) fn has_pending_persistence(&self) -> bool {
        self.runtime.open_tabs_persist_dirty
            || self.runtime.window_layout_persist_dirty
            || self.runtime.ui_layout_persist_pending
    }

    pub(in crate::features) fn mark_session_persistence_dirty(&mut self) {
        self.runtime.open_tabs_persist_dirty = true;
        self.runtime.window_layout_persist_dirty = true;
        self.signal_persist_wake();
    }

    pub(in crate::features) fn clear_session_persistence_dirty(&mut self) {
        self.runtime.open_tabs_persist_dirty = false;
        self.runtime.window_layout_persist_dirty = false;
    }

    pub(in crate::features) fn pending_session_persistence(
        &self,
        include_window_layout: bool,
    ) -> ShellPersistenceDirty {
        ShellPersistenceDirty {
            open_tabs: self.runtime.open_tabs_persist_dirty,
            window_layout: include_window_layout && self.runtime.window_layout_persist_dirty,
        }
    }

    pub(in crate::features) fn acknowledge_session_persistence(
        &mut self,
        dirty: ShellPersistenceDirty,
    ) {
        if dirty.open_tabs {
            self.runtime.open_tabs_persist_dirty = false;
        }
        if dirty.window_layout {
            self.runtime.window_layout_persist_dirty = false;
        }
    }

    pub(in crate::features) fn begin_session_persistence(
        &mut self,
        dirty: ShellPersistenceDirty,
    ) -> Option<u64> {
        if dirty.is_empty() || self.runtime.session_persistence_in_flight.is_some() {
            return None;
        }
        self.runtime.session_persistence_generation = self
            .runtime
            .session_persistence_generation
            .saturating_add(1);
        let generation = self.runtime.session_persistence_generation;
        self.runtime.session_persistence_in_flight = Some(generation);
        self.acknowledge_session_persistence(dirty);
        Some(generation)
    }

    pub(in crate::features) fn finish_session_persistence(
        &mut self,
        generation: u64,
        dirty: ShellPersistenceDirty,
        succeeded: bool,
    ) -> bool {
        if self.runtime.session_persistence_in_flight != Some(generation) {
            return false;
        }
        self.runtime.session_persistence_in_flight = None;
        if !succeeded {
            self.runtime.open_tabs_persist_dirty |= dirty.open_tabs;
            self.runtime.window_layout_persist_dirty |= dirty.window_layout;
            // A failed write is a fresh mark: the debounce task has to be told, or
            // the retry waits for some unrelated change to come along.
            if !dirty.is_empty() {
                self.signal_persist_wake();
            }
        }
        true
    }

    pub(in crate::features) fn queue_terminal_scroll_position(
        &mut self,
        session_id: &str,
        snapshot_only: bool,
    ) -> bool {
        if session_id.is_empty() {
            return false;
        }
        if snapshot_only {
            if !self
                .runtime
                .pending_terminal_scroll_position_sessions
                .contains(session_id)
            {
                self.runtime
                    .pending_terminal_scroll_snapshot_only_sessions
                    .insert(session_id.to_string());
            }
        } else {
            self.runtime
                .pending_terminal_scroll_snapshot_only_sessions
                .remove(session_id);
            self.runtime
                .pending_terminal_scroll_position_sessions
                .insert(session_id.to_string());
        }
        arm_once(&mut self.runtime.terminal_scroll_position_notify_armed)
    }

    pub(in crate::features) fn drain_terminal_scroll_position(
        &mut self,
    ) -> (Vec<String>, Vec<String>) {
        self.runtime.terminal_scroll_position_notify_armed = false;
        for session_id in &self.runtime.pending_terminal_scroll_position_sessions {
            self.runtime
                .pending_terminal_scroll_snapshot_only_sessions
                .remove(session_id);
        }
        (
            drain_sorted(&mut self.runtime.pending_terminal_scroll_position_sessions),
            drain_sorted(&mut self.runtime.pending_terminal_scroll_snapshot_only_sessions),
        )
    }

    pub(in crate::features) fn queue_terminal_scrollbar_drag(
        &mut self,
        session_id: String,
    ) -> bool {
        if session_id.is_empty() {
            return false;
        }
        self.runtime
            .pending_terminal_scrollbar_drag_sessions
            .insert(session_id);
        arm_once(&mut self.runtime.terminal_scrollbar_drag_notify_armed)
    }

    pub(in crate::features) fn drain_terminal_scrollbar_drag_sessions(&mut self) -> Vec<String> {
        self.runtime.terminal_scrollbar_drag_notify_armed = false;
        drain_sorted(&mut self.runtime.pending_terminal_scrollbar_drag_sessions)
    }

    pub(in crate::features) fn queue_terminal_selection_drag(
        &mut self,
        session_id: String,
    ) -> bool {
        if session_id.is_empty() {
            return false;
        }
        self.runtime
            .pending_terminal_selection_drag_sessions
            .insert(session_id);
        arm_once(&mut self.runtime.terminal_selection_drag_notify_armed)
    }

    pub(in crate::features) fn drain_terminal_selection_drag_sessions(&mut self) -> Vec<String> {
        self.runtime.terminal_selection_drag_notify_armed = false;
        drain_sorted(&mut self.runtime.pending_terminal_selection_drag_sessions)
    }

    pub(in crate::features) fn queue_terminal_user_scroll_idle(
        &mut self,
        session_id: &str,
        at: Instant,
    ) -> bool {
        if session_id.is_empty() {
            return false;
        }
        self.runtime.last_terminal_user_scroll_at = Some(at);
        self.runtime
            .pending_terminal_user_scroll_idle_sessions
            .insert(session_id.to_string());
        arm_once(&mut self.runtime.terminal_user_scroll_idle_notify_armed)
    }

    pub(in crate::features) fn drain_terminal_user_scroll_idle_sessions(&mut self) -> Vec<String> {
        self.runtime.terminal_user_scroll_idle_notify_armed = false;
        drain_sorted(&mut self.runtime.pending_terminal_user_scroll_idle_sessions)
    }
}

fn arm_once(armed: &mut bool) -> bool {
    if *armed {
        false
    } else {
        *armed = true;
        true
    }
}

fn drain_sorted(sessions: &mut HashSet<String>) -> Vec<String> {
    let mut sessions = sessions.drain().collect::<Vec<_>>();
    sessions.sort();
    sessions
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::models::{ActivityBarLayoutState, BottomPanelMode};

    use super::super::state::ShellFeatureInit;
    use super::ShellFeatureState;

    fn shell() -> ShellFeatureState {
        ShellFeatureState::new(ShellFeatureInit {
            status: String::new(),
            selected_nav: crate::models::NavItem::Workspace,
            bottom_panel_mode: BottomPanelMode::Hidden,
            command_bar_visible: false,
            quick_commands_height: 120.,
            command_send_height: 180.,
            active_left_panel: None,
            active_right_panel: None,
            left_open_panels: Vec::new(),
            right_open_panels: Vec::new(),
            panel_stack_sizes: HashMap::new(),
            panel_open_mode: crate::models::PanelOpenMode::Docked,
            panel_multi_open: false,
            left_sidebar_collapsed: true,
            right_inspector_collapsed: true,
            left_panel_width: 240.,
            right_panel_width: 320.,
            activity_bar_layout: ActivityBarLayoutState::default(),
        })
    }

    #[test]
    fn coalesced_queues_arm_once_deduplicate_and_drain_sorted() {
        let mut shell = shell();

        assert!(!shell.queue_terminal_selection_drag(String::new()));
        assert!(shell.queue_terminal_selection_drag("b".to_string()));
        assert!(!shell.queue_terminal_selection_drag("a".to_string()));
        assert!(!shell.queue_terminal_selection_drag("a".to_string()));
        assert_eq!(
            shell.drain_terminal_selection_drag_sessions(),
            vec!["a".to_string(), "b".to_string()]
        );
        assert!(shell.queue_terminal_selection_drag("c".to_string()));
    }

    #[test]
    fn scroll_position_queue_promotes_snapshot_only_work() {
        let mut shell = shell();

        assert!(shell.queue_terminal_scroll_position("snapshot", true));
        assert!(!shell.queue_terminal_scroll_position("paint", true));
        assert!(!shell.queue_terminal_scroll_position("paint", false));
        let (paint, snapshot_only) = shell.drain_terminal_scroll_position();

        assert_eq!(paint, vec!["paint".to_string()]);
        assert_eq!(snapshot_only, vec!["snapshot".to_string()]);
        assert!(shell.queue_terminal_scroll_position("next", false));
    }

    #[test]
    fn persistence_snapshot_masks_layout_and_clears_only_taken_work() {
        let mut shell = shell();
        shell.mark_session_persistence_dirty();

        let hidden = shell.pending_session_persistence(false);
        assert!(hidden.open_tabs());
        assert!(!hidden.window_layout());
        assert!(!hidden.is_empty());
        shell.acknowledge_session_persistence(hidden);

        let remaining = shell.pending_session_persistence(true);
        assert!(!remaining.open_tabs());
        assert!(remaining.window_layout());
    }

    #[test]
    fn session_persistence_failure_restores_only_the_submitted_dirty_generation() {
        let mut shell = shell();
        shell.mark_session_persistence_dirty();
        let submitted = shell.pending_session_persistence(true);
        let generation = shell
            .begin_session_persistence(submitted)
            .expect("first generation should start");
        assert!(shell.pending_session_persistence(true).is_empty());

        shell.mark_open_tabs_persist_dirty();
        assert!(shell.begin_session_persistence(submitted).is_none());
        assert!(shell.finish_session_persistence(generation, submitted, false));

        let retry = shell.pending_session_persistence(true);
        assert!(retry.open_tabs());
        assert!(retry.window_layout());
    }

    #[test]
    fn counters_and_resize_deadline_saturate_without_exposing_runtime() {
        let mut shell = shell();
        shell.runtime.full_shell_paint_count = u64::MAX;
        shell.runtime.terminal_surface_frame_notify_count = u64::MAX - 1;
        assert_eq!(shell.note_full_shell_paint(), u64::MAX);
        shell.note_terminal_surface_frame_notifies(4);
        assert_eq!(shell.runtime.terminal_surface_frame_notify_count, u64::MAX);
    }
}
