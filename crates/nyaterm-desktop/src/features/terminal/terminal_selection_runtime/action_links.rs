use std::time::{Duration, Instant};

use gpui::{
    App, Bounds, ClickEvent, ClipboardItem, Context, MouseMoveEvent, Pixels, Point, Window,
};

use crate::action_links::{
    ActionLinkAction, ActionLinkMatch, ActionLinkTarget, actions_for_match, match_at_offset,
};
use crate::features::NyaTermApp;
use crate::features::terminal::terminal_runtime::{
    TERMINAL_INPUT_LATENCY_WINDOW, TERMINAL_USER_SCROLL_ACTIVE_WINDOW,
};
use crate::models::{
    ActionLinkMenuAction, ActionLinkMenuState, ActionLinkTooltipState, TerminalPerformanceMode,
    terminal_action_link_matcher_key, terminal_expensive_interactions_enabled,
};
use crate::terminal::terminal_byte_index_for_cell_col;

fn action_link_hover_should_yield_to_terminal_latency(
    last_input_at: Option<Instant>,
    last_user_scroll_at: Option<Instant>,
    visual_scroll_active: bool,
    now: Instant,
) -> bool {
    last_input_at
        .is_some_and(|last| now.saturating_duration_since(last) < TERMINAL_INPUT_LATENCY_WINDOW)
        || (visual_scroll_active
            && last_user_scroll_at.is_some_and(|last| {
                now.saturating_duration_since(last) < TERMINAL_USER_SCROLL_ACTIVE_WINDOW
            }))
}

/// Hover dwell before an action-link tooltip appears (Tauri ActionLinkTooltip).
const ACTION_LINK_TOOLTIP_DELAY: Duration = Duration::from_millis(250);

impl NyaTermApp {
    pub(in crate::features) fn clear_action_link_tooltip(&mut self, cx: &mut Context<Self>) {
        let visible_changed = self.clear_action_link_tooltip_state();
        if visible_changed {
            cx.notify();
        }
    }

    fn clear_action_link_tooltip_state(&mut self) -> bool {
        clear_action_link_tooltip_state(
            &mut self.terminal.menus.action_link_tooltip,
            &mut self.terminal.menus.action_link_hover_pending,
        )
    }

    /// Promote the hover this timer was armed for. Returns whether anything visible
    /// changed.
    ///
    /// `generation` is the arming timer's own; a hover that has since moved on has
    /// bumped it, so the stale timer returns without touching anything and the newer
    /// hover's timer does the work. This is the `hover.begin` / `hover.activate`
    /// shape used by the resize handles, and it means the timer is the deadline
    /// rather than something that re-reads the clock to see whether it should have
    /// fired.
    fn apply_action_link_tooltip_delay(&mut self, generation: u64) -> bool {
        let pending = self.terminal.menus.action_link_hover_pending.clone();
        if !action_link_tooltip_timer_is_current(
            pending.as_ref().map(|(_, generation, _)| *generation),
            generation,
        ) {
            return false;
        }
        let Some((key, _, tip)) = pending else {
            return false;
        };
        self.terminal.menus.action_link_hover_pending = None;
        if !self.action_link_tooltip_has_visible_source(&tip) {
            return self.clear_action_link_tooltip_state();
        }
        // Only show if still matching the pending key (not superseded).
        if self
            .terminal
            .menus
            .action_link_tooltip
            .as_ref()
            .is_some_and(|current| current.match_key == key)
        {
            return true;
        }
        self.terminal.menus.action_link_tooltip = Some(tip);
        true
    }

    pub(in crate::features) fn action_link_tooltip_has_visible_source(
        &self,
        tip: &ActionLinkTooltipState,
    ) -> bool {
        self.terminal_session_at_point(Point::new(tip.x, tip.y))
            .flatten()
            .as_deref()
            == Some(tip.session_id.as_str())
    }

    pub(in crate::features) fn update_action_link_hover(
        &mut self,
        event: &MouseMoveEvent,
        cx: &mut Context<Self>,
    ) {
        if !self.settings.summary().terminal_action_links_enabled
            || self.runtime_output_pressure_active()
        {
            self.clear_action_link_tooltip(cx);
            return;
        }
        // Hide while menus are open or while selecting text.
        if self.terminal.menus.action_link_menu.is_some()
            || self.terminal.selection.dragging
            || self.translation.dialog_is_open()
        {
            self.clear_action_link_tooltip(cx);
            return;
        }
        let now = Instant::now();
        let hover_session = self.terminal_session_at_point(event.position);
        let hover_session_id = hover_session
            .as_ref()
            .and_then(|session| session.as_deref());
        let visual_scroll_active = hover_session_id
            .filter(|session_id| !session_id.is_empty())
            .is_some_and(|session_id| {
                self.terminal_visual_scroll_active_for_session(Some(session_id))
            });
        if action_link_hover_should_yield_to_terminal_latency(
            self.shell.last_terminal_input_at(),
            self.shell.last_terminal_user_scroll_at(),
            visual_scroll_active,
            now,
        ) {
            self.clear_action_link_tooltip(cx);
            return;
        }
        let Some((item, actions)) = self.action_link_at_point(event.position, cx) else {
            self.clear_action_link_tooltip(cx);
            return;
        };
        if actions.is_empty() {
            self.clear_action_link_tooltip(cx);
            return;
        }
        let default = actions
            .iter()
            .find(|action| action.is_default)
            .cloned()
            .or_else(|| actions.first().cloned());
        let Some(default) = default else {
            self.clear_action_link_tooltip(cx);
            return;
        };
        let Some(session_id) = hover_session.flatten() else {
            self.clear_action_link_tooltip(cx);
            return;
        };
        let match_key = format!(
            "{}|{}|{}|{}|{}",
            session_id,
            item.kind.label(),
            item.value,
            item.start,
            item.end
        );
        let preview = match &default.target {
            ActionLinkTarget::CopyText(text) | ActionLinkTarget::PasteText(text) => {
                format!("{} {text}", default.label)
            }
            target => target.text().to_owned(),
        };
        let next = ActionLinkTooltipState {
            session_id,
            x: event.position.x,
            y: event.position.y,
            kind_label: item.kind.label().to_string(),
            value: item.value.clone(),
            default_action_label: default.label.clone(),
            default_action_preview: preview,
            has_more_actions: actions.len() > 1,
            match_key: match_key.clone(),
        };
        // Already visible for this link: track position.
        if let Some(current) = self.terminal.menus.action_link_tooltip.as_ref()
            && current.match_key == match_key
        {
            return;
        }
        // Pending same link: update the tooltip its timer will show, keeping that
        // timer's generation so the delay is not restarted by mouse movement.
        if let Some((key, generation, _)) = self.terminal.menus.action_link_hover_pending.clone()
            && key == match_key
        {
            self.terminal.menus.action_link_hover_pending = Some((match_key, generation, next));
            return;
        }
        // New link under cursor: start the delay (Tauri ActionLinkTooltip). The idle
        // plane used to poll for this, which also meant a hovered link had to be named
        // in `runtime_quiet_tick_allowed` to be noticed at all.
        let visible_changed = self.terminal.menus.action_link_tooltip.take().is_some();
        self.terminal.menus.action_link_hover_generation = self
            .terminal
            .menus
            .action_link_hover_generation
            .wrapping_add(1);
        let generation = self.terminal.menus.action_link_hover_generation;
        self.terminal.menus.action_link_hover_pending = Some((match_key, generation, next));
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(ACTION_LINK_TOOLTIP_DELAY)
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.apply_action_link_tooltip_delay(generation) {
                    cx.notify();
                }
            });
        })
        .detach();
        if visible_changed {
            cx.notify();
        }
    }

    pub(in crate::features) fn action_link_at_point(
        &self,
        position: Point<Pixels>,
        cx: &App,
    ) -> Option<(ActionLinkMatch, Vec<ActionLinkAction>)> {
        if !self.settings.summary().terminal_action_links_enabled {
            return None;
        }
        let session_id = self.terminal_session_at_point(position)?;
        let session_id = session_id.as_deref();
        if !self.terminal_expensive_interactions_enabled_for_session(session_id) {
            return None;
        }
        // Only hit-test when the pointer is over the painted terminal content area.
        let bounds = self.terminal_surface_bounds_for_session(session_id)?;
        let (cell_w, cell_h) = self.terminal_cell_size();
        let insets = self.terminal_content_insets_for_bounds(session_id, bounds);
        let gutter = self.terminal_gutter_width_px_for_session(session_id);
        let local_x = f32::from(position.x - bounds.origin.x) - insets.left - gutter;
        let local_y = f32::from(position.y - bounds.origin.y) - insets.top;
        if local_x < 0. || local_y < 0. {
            return None;
        }
        let (rows, cols) = self.terminal_grid_size_for_session(session_id);
        if local_y >= cell_h * rows as f32 || local_x >= cell_w * cols as f32 {
            return None;
        }
        let cell = self.point_to_terminal_cell_for_session(session_id, position, cx)?;
        let action_link_matcher_key = terminal_action_link_matcher_key(
            self.settings.summary().terminal_action_links_enabled,
            &self.settings.summary().terminal_action_links_matchers,
        );
        let offset = self.terminal_display_offset_for_session(session_id);
        let snapshot = self.terminal_snapshot_for_session(session_id, offset);
        let frame_action_links = if let Some(session_id) = session_id.filter(|id| !id.is_empty()) {
            let view = self.terminal.view.views.get(session_id)?;
            crate::features::terminal::terminal_surface::terminal_action_links_for_paint_snapshot(
                Some(view),
                offset,
                snapshot.as_ref(),
                action_link_matcher_key,
            )
        } else {
            Vec::new()
        };
        let snapshot_row = self.terminal_snapshot_row_for_session_viewport_row(
            session_id,
            snapshot.as_ref(),
            offset,
            cell.row,
        )?;
        let line = snapshot.line(snapshot_row)?;
        if line.is_empty() {
            return None;
        }
        let byte_offset = terminal_byte_index_for_cell_col(line, cell.col);
        let item = frame_action_links
            .iter()
            .find_map(|links| {
                let source_index =
                    links.source_index_for_snapshot_row(snapshot.as_ref(), snapshot_row)?;
                links
                    .matches_by_line
                    .get(source_index)?
                    .iter()
                    .find(|item| byte_offset >= item.start && byte_offset < item.end)
                    .cloned()
            })
            .or_else(|| {
                let matchers = &self.settings.summary().terminal_action_links_matchers;
                match_at_offset(line, byte_offset, matchers)
            })?;
        let actions = actions_for_match(&item);
        Some((item, actions))
    }

    pub(in crate::features) fn terminal_session_at_point(
        &self,
        position: Point<Pixels>,
    ) -> Option<Option<String>> {
        // Bounds survive tab/page switches. Only the authoritative visible set
        // may participate in root-level hover and mouse-report hit-testing.
        let visible_session_ids = self.visible_terminal_session_ids();
        for session_id in &visible_session_ids {
            let bounds = self
                .terminal
                .layout
                .session_surface_bounds
                .get(*session_id)
                .copied()
                .or_else(|| {
                    (visible_session_ids.len() == 1
                        && self.shell.terminal_window_tree_ref().is_none()
                        && self.shell.workspace_split().is_none()
                        && self.session.active_id() == Some(*session_id))
                    .then_some(self.terminal.layout.surface_bounds)
                    .flatten()
                });
            if let Some(bounds) = bounds
                && terminal_bounds_contains(bounds, position)
            {
                return Some(Some((*session_id).to_string()));
            }
        }
        None
    }

    pub(in crate::features) fn dismiss_action_link_menu(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(menu) = self.terminal.menus.action_link_menu.as_ref() else {
            return false;
        };
        let restore_focus = self.action_link_source_is_active(&menu.session_id);
        self.close_action_link_menu(cx);
        if restore_focus {
            window.focus(&self.terminal.input.focus, cx);
        }
        true
    }

    pub(in crate::features) fn close_action_link_menu(&mut self, cx: &mut Context<Self>) {
        let mut changed = false;
        if self.terminal.menus.action_link_menu.take().is_some() {
            self.shell.set_status("action link menu closed".to_string());
            changed = true;
        }
        changed |= self.clear_action_link_tooltip_state();
        if changed {
            cx.notify();
        }
    }

    pub(in crate::features) fn try_open_action_link_menu_at_click(
        &mut self,
        event: &ClickEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some((item, actions)) = self.action_link_at_click(event, cx) else {
            return false;
        };
        if actions.is_empty() {
            return false;
        }
        let menu_actions = actions
            .into_iter()
            .map(|action| ActionLinkMenuAction {
                id: action.id,
                label: action.label,
                target: action.target,
                mac_format: action.mac_format,
                is_default: action.is_default,
            })
            .collect::<Vec<_>>();
        self.terminal.menus.action_link_tooltip = None;
        self.terminal.assist.command_suggestions = None;
        self.terminal.assist.credential_suggestions = None;
        let Some(session_id) = self.terminal_session_at_point(event.position()).flatten() else {
            return false;
        };
        self.terminal.menus.action_link_menu_generation = self
            .terminal
            .menus
            .action_link_menu_generation
            .wrapping_add(1);
        self.terminal.menus.action_link_menu = Some(ActionLinkMenuState {
            generation: self.terminal.menus.action_link_menu_generation,
            session_id,
            x: event.position().x,
            y: event.position().y,
            kind_label: item.kind.label().to_string(),
            value: item.value,
            actions: menu_actions,
        });
        self.shell
            .set_status(format!("action link menu: {}", item.kind.label()));
        cx.notify();
        true
    }

    pub(in crate::features) fn action_link_at_click(
        &self,
        event: &ClickEvent,
        cx: &App,
    ) -> Option<(ActionLinkMatch, Vec<ActionLinkAction>)> {
        self.action_link_at_point(event.position(), cx)
    }

    /// Ctrl/Cmd-click OSC 8 hyperlinks (uri from the terminal screen model).
    pub(in crate::features) fn try_activate_osc8_hyperlink_at_click(
        &mut self,
        event: &ClickEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.session.active_id().is_some_and(|session_id| {
            self.terminal_expensive_interactions_enabled_for_session(Some(session_id))
        }) {
            return false;
        }
        let Some(pos) = self.point_to_terminal_cell(event.position(), cx) else {
            return false;
        };
        let session_id = self.session.active_id_owned().unwrap_or_default();
        let display_offset = self.active_terminal_display_offset();
        let snapshot =
            self.terminal_snapshot_for_session(Some(session_id.as_str()), display_offset);
        let Some(snapshot_row) = self.terminal_snapshot_row_for_session_viewport_row(
            Some(session_id.as_str()),
            snapshot.as_ref(),
            display_offset,
            pos.row,
        ) else {
            return false;
        };
        let Some(spans) = snapshot.row(snapshot_row).map(|row| &row.hyperlinks) else {
            return false;
        };
        let col = pos.col;
        let Some(span) = spans
            .iter()
            .find(|span| col >= span.start_col && col <= span.end_col)
        else {
            return false;
        };
        let url = span.uri.clone();
        // Only open common URL schemes for safety (Tauri oscLinkHandler parity).
        let lower = url.to_ascii_lowercase();
        if !(lower.starts_with("http://")
            || lower.starts_with("https://")
            || lower.starts_with("mailto:"))
        {
            self.shell
                .set_status(format!("blocked OSC 8 scheme: {url}"));
            cx.notify();
            return true;
        }
        self.open_external_url_for_ui(&url, cx);
        true
    }

    pub(in crate::features) fn try_activate_action_link_at_click(
        &mut self,
        event: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.try_activate_osc8_hyperlink_at_click(event, cx) {
            return true;
        }
        let Some((_item, actions)) = self.action_link_at_click(event, cx) else {
            return false;
        };
        self.terminal.menus.action_link_tooltip = None;
        let Some(default) = actions
            .iter()
            .find(|action| action.is_default)
            .cloned()
            .or_else(|| actions.first().cloned())
        else {
            return false;
        };
        let Some(session_id) = self.terminal_session_at_point(event.position()).flatten() else {
            return false;
        };
        self.execute_action_link_target(default.target, &session_id, window, cx);
        true
    }

    pub(in crate::features) fn execute_action_link_menu_action(
        &mut self,
        menu: &ActionLinkMenuState,
        target: ActionLinkTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.terminal.menus.action_link_menu.as_ref() != Some(menu) {
            return;
        }
        self.close_action_link_menu(cx);
        self.execute_action_link_target(target, &menu.session_id, window, cx);
    }

    pub(in crate::features) fn action_link_source_is_active(&self, session_id: &str) -> bool {
        !self.security.screen_locked()
            && self.shell.selected_nav() == crate::models::NavItem::Workspace
            && self.session.active_id() == Some(session_id)
            && self.active_workspace_tab().is_some_and(|key| {
                crate::models::WorkspaceTabId::from_layout_key(&key).is_session()
            })
            && self.visible_terminal_session_ids().contains(&session_id)
    }

    /// The menu and modifier click share this dispatch. Pasting never becomes a
    /// shell command, and stale menu callbacks cannot send to another session.
    pub(in crate::features) fn execute_action_link_target(
        &mut self,
        target: ActionLinkTarget,
        session_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.action_link_source_is_active(session_id) {
            return;
        }
        match target {
            ActionLinkTarget::ExecuteCommand(command) => {
                self.execute_action_link_command(command, cx)
            }
            ActionLinkTarget::OpenUrl(url) => self.open_external_url_for_ui(&url, cx),
            ActionLinkTarget::CopyText(text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                self.shell
                    .set_status(rust_i18n::t!("terminal.macCopied").to_string());
                cx.notify();
            }
            ActionLinkTarget::PasteText(text) => self.paste_terminal_text(text, window, cx),
        }
        window.focus(&self.terminal.input.focus, cx);
    }

    fn terminal_expensive_interactions_enabled_for_session(
        &self,
        session_id: Option<&str>,
    ) -> bool {
        let Some(session_id) = session_id.filter(|id| !id.is_empty()) else {
            return false;
        };
        let is_active = self.session.active_id() == Some(session_id);
        let Some(view) = self.terminal.view.views.get(session_id) else {
            return false;
        };
        let runtime_output_pressure = self.runtime_output_pressure_active();
        let render_degraded = view.render_degraded
            || runtime_output_pressure
            || view.output_burst_bytes > 0
            || view.performance_mode == TerminalPerformanceMode::Overloaded;
        terminal_expensive_interactions_enabled(
            self.settings.summary().terminal_action_links_enabled,
            is_active,
            render_degraded,
            runtime_output_pressure,
            view.output_burst_bytes,
            view.performance_mode,
        )
    }
}

/// Whether the timer arming this call still owns the pending hover.
fn action_link_tooltip_timer_is_current(pending_generation: Option<u64>, armed: u64) -> bool {
    pending_generation == Some(armed)
}

fn clear_action_link_tooltip_state(
    tooltip: &mut Option<ActionLinkTooltipState>,
    pending: &mut Option<(String, u64, ActionLinkTooltipState)>,
) -> bool {
    let visible_changed = tooltip.take().is_some();
    *pending = None;
    visible_changed
}

fn terminal_bounds_contains(bounds: Bounds<Pixels>, position: Point<Pixels>) -> bool {
    let min_x = f32::from(bounds.origin.x);
    let min_y = f32::from(bounds.origin.y);
    let max_x = min_x + f32::from(bounds.size.width);
    let max_y = min_y + f32::from(bounds.size.height);
    let x = f32::from(position.x);
    let y = f32::from(position.y);
    x >= min_x && x <= max_x && y >= min_y && y <= max_y
}

#[cfg(test)]
mod mac_tests;

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::action_link_tooltip_timer_is_current;

    use gpui::{AppContext as _, Bounds, TestAppContext, point, px, size};
    use nyaterm_core::{InternalEditorDisplay, test_support::TestTempDir};

    use crate::features::file_manager::{FileManagerSeed, FileManagerWorkspace, PaneEndpoint};
    use crate::features::terminal::terminal_runtime::{
        TERMINAL_INPUT_LATENCY_WINDOW, TERMINAL_USER_SCROLL_ACTIVE_WINDOW,
    };
    use crate::features::test_support::{app_with_visible_local_session, editor_document};
    use crate::models::{SmartSplitMode, TerminalWindowNode, WorkspaceTabId};
    use crate::test_support::TestAppCleanup;

    use super::{
        ActionLinkTooltipState, action_link_hover_should_yield_to_terminal_latency,
        clear_action_link_tooltip_state,
    };

    #[test]
    fn action_link_hover_yields_to_recent_input() {
        let now = Instant::now();

        assert!(action_link_hover_should_yield_to_terminal_latency(
            Some(now),
            None,
            false,
            now,
        ));
        assert!(!action_link_hover_should_yield_to_terminal_latency(
            Some(now - TERMINAL_INPUT_LATENCY_WINDOW - Duration::from_millis(1)),
            None,
            false,
            now,
        ));
    }

    #[test]
    fn action_link_hover_yields_to_recent_visual_scroll_only_for_scrolled_surface() {
        let now = Instant::now();

        assert!(action_link_hover_should_yield_to_terminal_latency(
            None,
            Some(now),
            true,
            now,
        ));
        assert!(!action_link_hover_should_yield_to_terminal_latency(
            None,
            Some(now),
            false,
            now,
        ));
        assert!(!action_link_hover_should_yield_to_terminal_latency(
            None,
            Some(now - TERMINAL_USER_SCROLL_ACTIVE_WINDOW - Duration::from_millis(1)),
            true,
            now,
        ));
    }

    fn tooltip(match_key: &str) -> ActionLinkTooltipState {
        ActionLinkTooltipState {
            session_id: "local".to_string(),
            x: px(10.0),
            y: px(20.0),
            kind_label: "host".to_string(),
            value: "example.com".to_string(),
            default_action_label: "Open".to_string(),
            default_action_preview: "https://example.com".to_string(),
            has_more_actions: false,
            match_key: match_key.to_string(),
        }
    }

    /// Two hover timers are briefly in flight whenever the cursor crosses from one
    /// link to another, so the older one has to recognise itself as stale. It does
    /// that by generation, because the timer *is* the delay -- there is no clock left
    /// to re-read.
    #[test]
    fn only_the_newest_hover_timer_may_show_a_tooltip() {
        assert!(action_link_tooltip_timer_is_current(Some(2), 2));
        assert!(
            !action_link_tooltip_timer_is_current(Some(2), 1),
            "the cursor moved to a new link; the old timer must do nothing"
        );
        assert!(
            !action_link_tooltip_timer_is_current(None, 1),
            "the cursor left every link; the timer must not resurrect a tooltip"
        );
    }

    #[test]
    fn action_link_clear_pending_only_is_not_visible_change() {
        let mut visible = None;
        let mut pending = Some(("pending".to_string(), 1, tooltip("pending")));

        assert!(!clear_action_link_tooltip_state(&mut visible, &mut pending));
        assert!(pending.is_none());
    }

    #[test]
    fn action_link_clear_visible_tooltip_is_visible_change() {
        let mut visible = Some(tooltip("visible"));
        let mut pending = Some(("pending".to_string(), 1, tooltip("pending")));

        assert!(clear_action_link_tooltip_state(&mut visible, &mut pending));
        assert!(visible.is_none());
        assert!(pending.is_none());
    }

    #[test]
    fn cached_terminal_bounds_do_not_hit_file_manager_editor_or_page() {
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let root = TestTempDir::new("nyaterm-hidden-terminal-hit-test");
        let app = app_with_visible_local_session(&mut cx, root.path(), "local");
        app.update(&mut cx, |app, cx| {
            let bounds = Bounds::new(point(px(0.), px(0.)), size(px(500.), px(300.)));
            let position = bounds.center();
            app.terminal.layout.surface_bounds = Some(bounds);
            assert_eq!(
                app.terminal_session_at_point(position),
                Some(Some("local".to_string()))
            );
            assert!(
                app.suggestion_overlay_position_for_session(Some("local"), 0, 0, 340., 100.)
                    .is_some()
            );
            app.terminal
                .layout
                .session_surface_bounds
                .insert("local".into(), bounds);
            for tab in [
                WorkspaceTabId::FileManager("files".into()),
                WorkspaceTabId::Editor("document".into()),
            ] {
                let key = tab.layout_key();
                app.shell
                    .restore_terminal_window_tree(Some(TerminalWindowNode::leaf(
                        vec!["local".into(), key.clone()],
                        Some(key),
                    )));
                assert_eq!(app.session.active_id(), Some("local"));
                assert!(app.terminal_session_at_point(position).is_none());
                assert!(app.action_link_at_point(position, cx).is_none());
                assert!(
                    app.suggestion_overlay_position_for_session(Some("local"), 0, 0, 340., 100.)
                        .is_none()
                );
            }
            app.shell
                .restore_terminal_window_tree(Some(TerminalWindowNode::leaf(
                    vec!["local".into(), WorkspaceTabId::Settings.layout_key()],
                    Some(WorkspaceTabId::Settings.layout_key()),
                )));
            assert!(app.terminal_session_at_point(position).is_none());
            assert!(
                app.suggestion_overlay_position_for_session(Some("local"), 0, 0, 340., 100.)
                    .is_none()
            );
        });
    }

    #[test]
    fn split_hit_testing_never_falls_back_to_hidden_active_session_bounds() {
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let root = TestTempDir::new("nyaterm-split-terminal-hit-test");
        let app = app_with_visible_local_session(&mut cx, root.path(), "local");
        app.update(&mut cx, |app, _| {
            let left = Bounds::new(point(px(0.), px(0.)), size(px(500.), px(300.)));
            let right = Bounds::new(point(px(500.), px(0.)), size(px(500.), px(300.)));
            let key = WorkspaceTabId::FileManager("files".into()).layout_key();
            app.shell
                .restore_terminal_window_tree(TerminalWindowNode::build_smart_split_layout(
                    &[key, "peer".into()],
                    SmartSplitMode::Vertical,
                ));
            app.terminal.layout.surface_bounds = Some(left);
            app.terminal
                .layout
                .session_surface_bounds
                .insert("local".into(), left);
            app.terminal
                .layout
                .session_surface_bounds
                .insert("peer".into(), right);
            assert!(app.terminal_session_at_point(left.center()).is_none());
            assert_eq!(
                app.terminal_session_at_point(right.center()),
                Some(Some("peer".into()))
            );
            assert!(
                app.suggestion_overlay_position_for_session(Some("local"), 0, 0, 340., 100.)
                    .is_none()
            );
            assert!(
                app.suggestion_overlay_position_for_session(Some("peer"), 0, 0, 340., 100.)
                    .is_some()
            );
            // Even the visible terminal's old aggregate bounds cannot enlarge
            // its per-session hit area over the file manager.
            app.session.select_active_session("peer");
            assert!(app.terminal_session_at_point(left.center()).is_none());
            app.shell.set_pane_focus_mode(true);
            app.session.select_active_session("local");
            assert!(app.terminal_session_at_point(right.center()).is_none());
        });
    }

    #[test]
    fn delayed_action_link_tooltip_requires_its_original_visible_surface() {
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let root = TestTempDir::new("nyaterm-hidden-tooltip-delay");
        let app = app_with_visible_local_session(&mut cx, root.path(), "local");
        app.update(&mut cx, |app, _| {
            let bounds = Bounds::new(point(px(0.), px(0.)), size(px(500.), px(300.)));
            app.terminal.layout.surface_bounds = Some(bounds);
            let tip = tooltip("pending");
            app.terminal.menus.action_link_hover_pending = Some(("pending".into(), 1, tip.clone()));
            assert!(app.apply_action_link_tooltip_delay(1));
            assert!(app.action_link_tooltip_has_visible_source(&tip));

            for tab in [
                WorkspaceTabId::FileManager("files".into()).layout_key(),
                WorkspaceTabId::Editor("document".into()).layout_key(),
                "peer".to_string(),
            ] {
                app.terminal
                    .layout
                    .session_surface_bounds
                    .insert(tab.clone(), bounds);
                app.shell
                    .restore_terminal_window_tree(Some(TerminalWindowNode::leaf(
                        vec!["local".into(), tab.clone()],
                        Some(tab),
                    )));
                assert!(!app.action_link_tooltip_has_visible_source(&tip));
                app.terminal.menus.action_link_tooltip = Some(tip.clone());
                app.terminal.menus.action_link_hover_pending =
                    Some(("pending".into(), 2, tip.clone()));
                assert!(app.apply_action_link_tooltip_delay(2));
                assert!(app.terminal.menus.action_link_tooltip.is_none());
                assert!(app.terminal.menus.action_link_hover_pending.is_none());
            }
        });
    }

    #[test]
    fn activating_non_terminal_tabs_cancels_visible_and_pending_action_links() {
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let root = TestTempDir::new("nyaterm-action-link-tab-switch");
        let app = app_with_visible_local_session(&mut cx, root.path(), "local");
        let workspace = cx.new(|cx| {
            FileManagerWorkspace::new(
                app.clone(),
                "files".into(),
                FileManagerSeed {
                    left: PaneEndpoint::Local,
                    left_path: None,
                    right: PaneEndpoint::Local,
                    right_path: None,
                },
                cx,
            )
        });
        app.update(&mut cx, |app, cx| {
            app.reconcile_terminal_windows();
            app.file_manager.insert("files".into(), workspace);
            let document =
                editor_document("local", "/example.txt", InternalEditorDisplay::Workspace);
            let editor_id = document.id.clone();
            app.transfer.open_editor_tab(document);
            for tab in [
                WorkspaceTabId::FileManager("files".into()),
                WorkspaceTabId::Editor(editor_id),
            ] {
                let key = tab.layout_key();
                let mut tree = app.shell.terminal_window_tree().unwrap();
                tree.ensure_tab(&key, None);
                app.shell.restore_terminal_window_tree(Some(tree));
                let tip = tooltip("pending");
                app.terminal.menus.action_link_tooltip = Some(tip.clone());
                app.terminal.menus.action_link_hover_pending = Some(("pending".into(), 1, tip));
                app.activate_workspace_tab(&key, cx);
                assert!(app.terminal.menus.action_link_tooltip.is_none());
                assert!(app.terminal.menus.action_link_hover_pending.is_none());
                assert!(!app.apply_action_link_tooltip_delay(1));
            }
        });
    }
}
