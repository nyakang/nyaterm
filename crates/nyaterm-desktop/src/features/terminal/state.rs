//! Grouped terminal feature state.
//!
//! This is presentation state only: which terminals exist, what the user has
//! selected, where the surface was painted. Parsing, snapshots and the wire
//! protocol stay in `nyaterm-terminal` and `nyaterm-transport`.

use nyaterm_terminal::selection::TerminalSelectionEndpoint;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Instant;

use gpui::{Entity, FocusHandle, Subscription, Task};
use nyaterm_core::ResolvedKeywordHighlightRule;
use nyaterm_terminal::{TerminalOutputDecoder, TerminalScreen};
use nyaterm_ui::NyaDocumentEditorState;

use super::assist_state::TerminalAssistState;
use super::editing_state::TerminalEditingState;
use super::ime_state::TerminalImeState;
use super::terminal_surface::TerminalScrollbarDragState;
use super::terminal_surface_entity::TerminalSurface;
use crate::features::{FontResolutionStatus, terminal::ResolvedAppearanceFont};
use crate::models::recording::ExecutionRecordingEvent;
use crate::models::{
    ActionLinkMenuState, ActionLinkTooltipState, RecordingHistorySearchEvent,
    RecordingHistorySearchKey, TerminalFrameEvent, TerminalFramePipeline, TerminalSearchMode,
    TerminalSelection, TerminalSelectionTextError, TerminalViewState,
};
use crate::terminal::terminal_keyword_rules_key;

pub(in crate::features) struct TerminalFeatureState {
    pub(super) search: TerminalSearchState,
    pub(super) view: TerminalViewRuntimeState,
    pub(super) input: TerminalInputState,
    pub(super) paste: TerminalPasteReviewState,
    pub(super) assist: TerminalAssistState,
    pub(super) editing: TerminalEditingState,
    pub(super) selection: TerminalSelectionState,
    pub(super) layout: TerminalLayoutState,
    pub(super) menus: TerminalMenuState,
    pub(super) paint: TerminalPaintCacheState,
}

/// Focus handles the terminal feature needs at construction time.
pub(in crate::features) struct TerminalFeatureFocus {
    pub actions: FocusHandle,
    pub terminal: FocusHandle,
}

/// In-terminal find bar and recording history search.
pub(super) struct TerminalSearchState {
    pub(super) open: bool,
    pub(super) query: String,
    pub(super) mode: TerminalSearchMode,
    pub(super) case_sensitive: bool,
    pub(super) regex: bool,
    pub(super) whole_word: bool,
    /// Runtime-only per-session preference; missing sessions use the default `true`.
    pub(super) wrap_around_by_session: HashMap<String, bool>,
    pub(super) active_index: usize,
    pub(super) full_search_ready_at: Instant,
    pub(super) history_pending_key: Option<RecordingHistorySearchKey>,
    pub(super) history_result: Option<RecordingHistorySearchEvent>,
}

/// Live terminal views, their surfaces, and the frame/scroll pipeline.
pub(super) struct TerminalViewRuntimeState {
    pub views: HashMap<String, TerminalViewState>,
    /// Per-session terminal grid entities (frame notify isolation).
    pub surfaces: HashMap<String, Entity<TerminalSurface>>,
    pub output: String,
    pub output_decoder: TerminalOutputDecoder,
    pub screen: TerminalScreen,
    pub frame_pipeline: TerminalFramePipeline,
    pub live_prefetch_generation: u64,
    pub live_prefetch_task: Option<gpui::Task<()>>,
    pub scroll_offset: usize,
    pub scroll_delta_residuals: HashMap<String, f32>,
    pub scrollbar_drag: Option<TerminalScrollbarDragState>,
    pub pending_frame_events: VecDeque<TerminalFrameEvent>,
    pub retired_session_ids: VecDeque<String>,
}

/// Keyboard focus and IME composition for the terminal surface.
pub(super) struct TerminalInputState {
    pub(super) focus: FocusHandle,
    pub(super) focus_active: bool,
    pub(super) focus_subscriptions: Vec<Subscription>,
    pub(super) ime: TerminalImeState,
}

/// Dedicated multi-line paste editor state backed by gpui-kit's editor.
pub(super) struct TerminalPasteReviewState {
    pub(super) editor: Option<Entity<NyaDocumentEditorState>>,
    _subscription: Option<Subscription>,
}

/// Text selection and mouse reporting.
pub(super) struct TerminalSelectionState {
    pub(super) selection: Option<TerminalSelection>,
    /// A released empty selection retains only its anchor for a later Shift+click.
    /// It shares `session_id` with selection and is never painted or copied.
    pub(super) click_anchor: Option<TerminalSelectionEndpoint>,
    pub(super) session_id: Option<String>,
    pub(super) text: TerminalSelectionText,
    pub(super) source_history: Option<nyaterm_terminal::TerminalHistoryState>,
    pub(super) selected_occurrence: TerminalSelectedOccurrenceState,
    pub(super) dragging: bool,
    pub(super) drag_pointer_position: Option<gpui::Point<gpui::Pixels>>,
    pub(super) autoscroll: Option<TerminalSelectionAutoscroll>,
    pub(super) autoscroll_generation: u64,
    pub(super) scroll_rehit_armed: bool,
    pub(super) mouse_report_button: Option<u8>,
    pub(super) mouse_report_session_id: Option<String>,
    pub(super) mouse_report_peer_session_ids: Vec<String>,
    pub(super) mouse_report_position: Option<(u16, u16)>,
}

#[derive(Default)]
pub(super) enum TerminalSelectionTextStatus {
    #[default]
    Unrequested,
    Pending,
    Ready(Option<String>),
    Failed(TerminalSelectionTextError),
}

impl std::fmt::Debug for TerminalSelectionTextStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unrequested => f.write_str("Unrequested"),
            Self::Pending => f.write_str("Pending"),
            Self::Ready(_) => f.write_str("Ready(<redacted>)"),
            Self::Failed(error) => f.debug_tuple("Failed").field(error).finish(),
        }
    }
}

#[derive(Default)]
pub(super) struct TerminalSelectionText {
    pub(super) status: TerminalSelectionTextStatus,
    pub(super) key: Option<(String, TerminalSelection)>,
    pub(super) generation: u64,
    pub(super) task: Option<Task<()>>,
    /// Coalesce automatic and explicit copies; true requests success/error feedback.
    pub(super) pending_copy: Option<bool>,
    pub(super) deferred_copy_completed: bool,
}

impl TerminalSelectionText {
    pub(super) fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.status = TerminalSelectionTextStatus::Unrequested;
        self.task = None;
        self.key = None;
        self.pending_copy = None;
        self.deferred_copy_completed = false;
    }
}

#[derive(Clone)]
pub(super) struct TerminalSelectionAutoscroll {
    pub(super) session_id: String,
    pub(super) position: gpui::Point<gpui::Pixels>,
    pub(super) direction: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::features) enum LostTerminalSelectionRecovery {
    None,
    ClearedEmpty,
    Committed,
}

pub(super) struct TerminalSelectedOccurrenceState {
    pub(super) session_id: Option<String>,
    pub(super) query: Option<String>,
    pub(super) generation: u64,
}

/// Last painted geometry, used to map pointer positions onto cells.
pub(super) struct TerminalLayoutState {
    /// Last painted bounds of the active terminal text area (window coords).
    pub(super) surface_bounds: Option<gpui::Bounds<gpui::Pixels>>,
    pub(super) session_surface_bounds: HashMap<String, gpui::Bounds<gpui::Pixels>>,
    pub(super) scrollbar_track_bounds: Option<gpui::Bounds<gpui::Pixels>>,
    pub(super) session_scrollbar_track_bounds: HashMap<String, gpui::Bounds<gpui::Pixels>>,
    pub(super) scale_factor: f32,
    pub(super) cell_metrics: Option<(f32, f32)>,
    pub(super) font_metrics_cache: Option<TerminalFontMetricsCache>,
    /// Runtime-only fallback used when the configured font is unavailable or proportional.
    pub(super) terminal_font_override: Option<ResolvedAppearanceFont>,
    pub(super) terminal_font_resolution: Option<FontResolutionStatus>,
}

/// Runtime validation cache for one configured family, size, and weight tuple.
///
/// Font enumeration and glyph measurement are expensive TextSystem operations. Keep
/// the result in runtime state without changing persisted appearance settings.
#[derive(Clone, Debug)]
pub(super) struct TerminalFontMetricsCache {
    pub(super) configured_family: String,
    pub(super) font_size: u16,
    pub(super) font_weight: u16,
    pub(super) catalog_generation: u64,
    pub(super) configured_font_valid: bool,
    pub(super) resolved_font: Option<ResolvedAppearanceFont>,
    pub(super) resolution: FontResolutionStatus,
    pub(super) cell_width: f32,
}

/// Terminal actions overlay and context menu.
pub(super) struct TerminalMenuState {
    pub(super) actions_open: bool,
    pub(super) actions_focus: FocusHandle,
    pub(super) action_link_menu: Option<ActionLinkMenuState>,
    pub(super) action_link_menu_generation: u64,
    pub(super) action_link_tooltip: Option<ActionLinkTooltipState>,
    /// Pending action-link hover, with the generation of the timer that owns it. The
    /// timer is the delay; the generation is how a superseded one recognises itself.
    pub(super) action_link_hover_pending: Option<(String, u64, ActionLinkTooltipState)>,
    /// Incremented for every new hover, so an in-flight timer can tell whether the
    /// cursor has moved on since it was armed.
    pub(super) action_link_hover_generation: u64,
}

/// Paint-time caches invalidated whenever appearance settings change.
pub(super) struct TerminalPaintCacheState {
    pub(super) cached_keyword_highlight_rules: Option<Arc<Vec<ResolvedKeywordHighlightRule>>>,
    /// Hash of `cached_keyword_highlight_rules`, computed once per cached set.
    pub(super) cached_keyword_highlight_rules_key: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::features) struct TerminalOverlayVisibility {
    pub paste_review: bool,
    pub actions: bool,
    pub action_link_menu: bool,
    pub action_link_tooltip: bool,
}

impl TerminalFeatureState {
    pub(in crate::features) fn submit_execution_recording(
        &self,
        session_id: &str,
        event: ExecutionRecordingEvent,
    ) {
        self.view
            .frame_pipeline
            .submit_execution_recording(session_id, event);
    }

    pub(in crate::features) fn recording_output_fence(
        &self,
    ) -> impl FnOnce() -> Result<(), String> + Send + 'static {
        let pipeline = self.view.frame_pipeline.clone();
        move || pipeline.finish_pending_output()
    }

    pub(in crate::features) fn take_frame_shutdown(
        &mut self,
    ) -> Option<impl FnOnce() + Send + 'static> {
        self.assist.shutdown_workers();
        self.view.frame_pipeline.take_shutdown()
    }

    #[allow(clippy::too_many_arguments)]
    pub(in crate::features) fn new(
        screen: TerminalScreen,
        output_decoder: TerminalOutputDecoder,
        frame_pipeline: TerminalFramePipeline,
        output: String,
        scale_factor: f32,
        focus: TerminalFeatureFocus,
    ) -> Self {
        Self {
            search: TerminalSearchState {
                open: false,
                query: String::new(),
                mode: TerminalSearchMode::Buffer,
                case_sensitive: false,
                regex: false,
                whole_word: false,
                wrap_around_by_session: HashMap::new(),
                active_index: 0,
                full_search_ready_at: Instant::now(),
                history_pending_key: None,
                history_result: None,
            },
            view: TerminalViewRuntimeState {
                views: HashMap::new(),
                surfaces: HashMap::new(),
                output,
                output_decoder,
                screen,
                frame_pipeline,
                live_prefetch_generation: 0,
                live_prefetch_task: None,
                scroll_offset: 0,
                scroll_delta_residuals: HashMap::new(),
                scrollbar_drag: None,
                pending_frame_events: VecDeque::new(),
                retired_session_ids: VecDeque::new(),
            },
            input: TerminalInputState {
                focus: focus.terminal,
                focus_active: false,
                focus_subscriptions: Vec::new(),
                ime: TerminalImeState::default(),
            },
            paste: TerminalPasteReviewState::new(),
            assist: TerminalAssistState::new(),
            editing: TerminalEditingState::default(),
            selection: TerminalSelectionState {
                selection: None,
                click_anchor: None,
                session_id: None,
                text: TerminalSelectionText::default(),
                source_history: None,
                selected_occurrence: TerminalSelectedOccurrenceState {
                    session_id: None,
                    query: None,
                    generation: 0,
                },
                dragging: false,
                drag_pointer_position: None,
                autoscroll: None,
                autoscroll_generation: 0,
                scroll_rehit_armed: false,
                mouse_report_button: None,
                mouse_report_session_id: None,
                mouse_report_peer_session_ids: Vec::new(),
                mouse_report_position: None,
            },
            layout: TerminalLayoutState {
                surface_bounds: None,
                session_surface_bounds: HashMap::new(),
                scrollbar_track_bounds: None,
                session_scrollbar_track_bounds: HashMap::new(),
                scale_factor,
                cell_metrics: None,
                font_metrics_cache: None,
                terminal_font_override: None,
                terminal_font_resolution: None,
            },
            menus: TerminalMenuState {
                actions_open: false,
                actions_focus: focus.actions,
                action_link_menu: None,
                action_link_menu_generation: 0,
                action_link_tooltip: None,
                action_link_hover_pending: None,
                action_link_hover_generation: 0,
            },
            paint: TerminalPaintCacheState {
                cached_keyword_highlight_rules: None,
                cached_keyword_highlight_rules_key: 0,
            },
        }
    }

    pub(in crate::features) fn set_search_mode(&mut self, mode: TerminalSearchMode) {
        self.search.mode = mode;
    }

    pub(in crate::features) fn search_wrap_around(&self, session_id: Option<&str>) -> bool {
        session_id
            .filter(|session_id| !session_id.is_empty())
            .and_then(|session_id| self.search.wrap_around_by_session.get(session_id).copied())
            .unwrap_or(true)
    }

    pub(in crate::features) fn toggle_search_wrap_around(
        &mut self,
        session_id: Option<&str>,
    ) -> bool {
        let Some(session_id) = session_id.filter(|session_id| !session_id.is_empty()) else {
            return true;
        };
        let enabled = !self.search_wrap_around(Some(session_id));
        if enabled {
            self.search.wrap_around_by_session.remove(session_id);
        } else {
            self.search
                .wrap_around_by_session
                .insert(session_id.to_string(), false);
        }
        enabled
    }

    pub(in crate::features) fn move_search_session_state(&mut self, from: &str, to: &str) {
        if let Some(wrap_around) = self.search.wrap_around_by_session.remove(from) {
            self.search
                .wrap_around_by_session
                .insert(to.to_string(), wrap_around);
        }
    }

    pub(in crate::features) fn remove_search_session_state(&mut self, session_id: &str) {
        self.search.wrap_around_by_session.remove(session_id);
    }

    #[cfg(test)]
    pub(in crate::features) fn buffer_search_is_open(&self) -> bool {
        self.search.open && self.search.mode == TerminalSearchMode::Buffer
    }

    /// Raise the find-bar flag without the focus and text-input choreography
    /// `NyaTermApp::open_terminal_search` performs, which needs a `Window`.
    #[cfg(test)]
    pub(in crate::features) fn open_search_for_test(&mut self) {
        self.search.open = true;
    }

    pub(in crate::features) fn input_focus(&self) -> &FocusHandle {
        &self.input.focus
    }

    pub(in crate::features) fn input_focus_is_active(&self) -> bool {
        self.input.focus_active
    }

    pub(in crate::features) fn paste_review_editor(
        &self,
    ) -> Option<Entity<NyaDocumentEditorState>> {
        self.paste.editor.clone()
    }

    pub(in crate::features) fn overlay_visibility(&self) -> TerminalOverlayVisibility {
        TerminalOverlayVisibility {
            paste_review: self.paste.editor.is_some(),
            actions: self.menus.actions_open,
            action_link_menu: self.menus.action_link_menu.is_some(),
            action_link_tooltip: self.menus.action_link_tooltip.is_some(),
        }
    }

    pub(in crate::features) fn actions_focus(&self) -> &FocusHandle {
        &self.menus.actions_focus
    }

    pub(in crate::features) fn close_actions(&mut self) {
        self.menus.actions_open = false;
    }

    #[cfg(test)]
    pub(in crate::features) fn mark_credential_autofill_detection_for_test(&mut self) {
        self.assist.credential_autofill_detection_pending = true;
    }

    /// A credential-prompt detection was marked while output was being processed and
    /// has not run yet. The data-plane drain task watches this so it comes back for
    /// it rather than parking.
    pub(in crate::features) fn credential_autofill_detection_is_pending(&self) -> bool {
        self.assist.credential_autofill_detection_pending
    }

    #[cfg(test)]
    pub(in crate::features) fn action_link_hover_is_pending(&self) -> bool {
        self.menus.action_link_hover_pending.is_some()
    }

    pub(in crate::features) fn clear_activation_interaction(&mut self) -> bool {
        self.selection.click_anchor = None;
        let had_interaction = self.selection.selection.take().is_some()
            || self.selection.dragging
            || self.menus.action_link_menu.is_some()
            || self.menus.action_link_tooltip.is_some();
        self.selection.text.invalidate();
        self.selection.source_history = None;
        self.selection.dragging = false;
        self.selection.drag_pointer_position = None;
        self.selection.autoscroll = None;
        self.selection.scroll_rehit_armed = false;
        self.menus.action_link_menu = None;
        self.menus.action_link_tooltip = None;
        self.menus.action_link_hover_pending = None;
        had_interaction
    }

    pub(in crate::features) fn recover_lost_selection_mouse_up(
        &mut self,
    ) -> LostTerminalSelectionRecovery {
        if !self.selection.dragging {
            return LostTerminalSelectionRecovery::None;
        }
        self.selection.dragging = false;
        self.selection.drag_pointer_position = None;
        self.selection.autoscroll = None;
        self.selection.scroll_rehit_armed = false;
        if self
            .selection
            .selection
            .as_ref()
            .is_none_or(TerminalSelection::is_empty)
        {
            self.selection.text.invalidate();
            self.selection.source_history = None;
            self.selection.selection = None;
            self.selection.click_anchor = None;
            self.selection.session_id = None;
            LostTerminalSelectionRecovery::ClearedEmpty
        } else {
            LostTerminalSelectionRecovery::Committed
        }
    }

    pub(in crate::features) fn cell_metrics(&self) -> Option<(f32, f32)> {
        self.layout.cell_metrics
    }

    pub(in crate::features) fn invalidate_cell_metrics(&mut self) {
        self.layout.cell_metrics = None;
        self.layout.font_metrics_cache = None;
        self.layout.terminal_font_override = None;
        self.layout.terminal_font_resolution = None;
    }

    pub(in crate::features) fn terminal_font_override(&self) -> Option<&ResolvedAppearanceFont> {
        self.layout.terminal_font_override.as_ref()
    }

    pub(in crate::features) fn terminal_font_resolution(&self) -> Option<&FontResolutionStatus> {
        self.layout.terminal_font_resolution.as_ref()
    }

    pub(in crate::features) fn terminal_font_metrics_need_catalog_refresh(
        &self,
        catalog_generation: u64,
    ) -> bool {
        self.layout
            .font_metrics_cache
            .as_ref()
            .is_some_and(|cache| {
                // The first catalog commit only validates the system list. A valid metric
                // cache from generation zero already measured the same TextSystem, so do not
                // trigger an avoidable terminal resize. Later catalog generations represent a
                // real system-font change and must invalidate the cache.
                !cache.configured_font_valid
                    || (cache.catalog_generation != 0
                        && cache.catalog_generation != catalog_generation)
            })
    }

    pub(in crate::features) fn set_terminal_font_override(
        &mut self,
        font: Option<ResolvedAppearanceFont>,
    ) {
        self.layout.terminal_font_override = font;
    }

    pub(in crate::features) fn set_terminal_font_resolution(
        &mut self,
        resolution: FontResolutionStatus,
    ) {
        self.layout.terminal_font_resolution = Some(resolution);
    }

    pub(in crate::features) fn move_session_surface_bounds(&mut self, from: &str, to: String) {
        if let Some(bounds) = self.layout.session_surface_bounds.remove(from) {
            self.layout
                .session_surface_bounds
                .insert(to.clone(), bounds);
        }
        if let Some(bounds) = self.layout.session_scrollbar_track_bounds.remove(from) {
            self.layout
                .session_scrollbar_track_bounds
                .insert(to, bounds);
        }
    }

    pub(in crate::features) fn remove_session_surface_bounds(&mut self, session_id: &str) {
        self.layout.session_surface_bounds.remove(session_id);
        self.layout
            .session_scrollbar_track_bounds
            .remove(session_id);
    }

    pub(in crate::features) fn cached_keyword_highlight_rules(
        &self,
    ) -> Option<&Arc<Vec<ResolvedKeywordHighlightRule>>> {
        self.paint.cached_keyword_highlight_rules.as_ref()
    }

    pub(in crate::features) fn cache_keyword_highlight_rules(
        &mut self,
        rules: Arc<Vec<ResolvedKeywordHighlightRule>>,
    ) {
        self.view.frame_pipeline.set_keyword_rules(rules.clone());
        self.paint.cached_keyword_highlight_rules_key = terminal_keyword_rules_key(&rules);
        self.paint.cached_keyword_highlight_rules = Some(rules);
    }

    pub(in crate::features) fn keyword_highlight_rules_key(
        &self,
        rules: &Arc<Vec<ResolvedKeywordHighlightRule>>,
    ) -> u64 {
        match self.paint.cached_keyword_highlight_rules.as_ref() {
            Some(cached) if Arc::ptr_eq(cached, rules) => {
                self.paint.cached_keyword_highlight_rules_key
            }
            _ => terminal_keyword_rules_key(rules),
        }
    }

    pub(in crate::features) fn invalidate_paint_caches(&mut self) {
        self.paint.cached_keyword_highlight_rules = None;
    }
}

impl TerminalPasteReviewState {
    fn new() -> Self {
        Self {
            editor: None,
            _subscription: None,
        }
    }

    pub(super) fn open(
        &mut self,
        editor: Entity<NyaDocumentEditorState>,
        subscription: Subscription,
    ) {
        self.editor = Some(editor);
        self._subscription = Some(subscription);
    }

    pub(super) fn clear(&mut self) {
        self.editor = None;
        self._subscription = None;
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Bounds, TestAppContext, point, px, size};
    use nyaterm_terminal::{TerminalOutputDecoder, TerminalScreen};

    use super::{LostTerminalSelectionRecovery, TerminalFeatureFocus, TerminalFeatureState};
    use crate::models::{TerminalFramePipeline, TerminalSearchMode};

    fn terminal_state() -> TerminalFeatureState {
        let cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        cx.update(|cx| {
            TerminalFeatureState::new(
                TerminalScreen::new(80, 24),
                TerminalOutputDecoder::default(),
                TerminalFramePipeline::default(),
                String::new(),
                1.0,
                TerminalFeatureFocus {
                    actions: cx.focus_handle(),
                    terminal: cx.focus_handle(),
                },
            )
        })
    }

    #[test]
    fn terminal_owner_projects_overlay_visibility_and_search_mode() {
        let mut state = terminal_state();
        state.menus.actions_open = true;
        state.search.open = true;
        state.set_search_mode(TerminalSearchMode::History);

        let overlays = state.overlay_visibility();
        assert!(!overlays.paste_review);
        assert!(overlays.actions);
        assert!(!state.buffer_search_is_open());

        state.set_search_mode(TerminalSearchMode::Buffer);
        assert!(state.buffer_search_is_open());
    }

    #[test]
    fn terminal_search_wrap_around_is_session_local_and_runtime_only() {
        let mut state = terminal_state();

        assert!(state.search_wrap_around(Some("session-a")));
        assert!(state.search_wrap_around(Some("session-b")));
        assert!(!state.toggle_search_wrap_around(Some("session-a")));
        assert!(!state.search_wrap_around(Some("session-a")));
        assert!(state.search_wrap_around(Some("session-b")));

        state.move_search_session_state("session-a", "session-c");
        assert!(state.search_wrap_around(Some("session-a")));
        assert!(!state.search_wrap_around(Some("session-c")));

        state.remove_search_session_state("session-c");
        assert!(state.search_wrap_around(Some("session-c")));
        assert!(state.search_wrap_around(None));
    }

    #[test]
    fn terminal_owner_clears_activation_interaction_as_one_transition() {
        let mut state = terminal_state();
        state.selection.click_anchor = Some(crate::models::TerminalBufferCellPos::new(4, 2).into());
        state.selection.dragging = true;
        state.menus.action_link_hover_pending = Some((
            "https://example.com".to_string(),
            1,
            crate::models::ActionLinkTooltipState {
                session_id: "local".to_string(),
                x: px(10.),
                y: px(20.),
                kind_label: "URL".to_string(),
                value: "https://example.com".to_string(),
                default_action_label: "Open".to_string(),
                default_action_preview: "https://example.com".to_string(),
                has_more_actions: false,
                match_key: "url|https://example.com|0|19".to_string(),
            },
        ));

        assert!(state.clear_activation_interaction());
        assert!(state.selection.click_anchor.is_none());
        assert!(!state.selection.dragging);
        assert!(!state.action_link_hover_is_pending());
        assert!(!state.clear_activation_interaction());
    }

    #[test]
    fn terminal_owner_recovers_empty_and_non_empty_lost_selection_mouse_up() {
        let mut state = terminal_state();
        state.selection.dragging = true;
        state.selection.selection = Some(crate::models::TerminalSelection::with_anchor(
            crate::models::TerminalBufferCellPos::new(4, 2),
        ));
        assert_eq!(
            state.recover_lost_selection_mouse_up(),
            LostTerminalSelectionRecovery::ClearedEmpty
        );
        assert!(state.selection.selection.is_none());
        assert_eq!(
            state.recover_lost_selection_mouse_up(),
            LostTerminalSelectionRecovery::None
        );

        state.selection.dragging = true;
        state.selection.selection = Some(crate::models::TerminalSelection::from_range(
            crate::models::TerminalBufferCellPos::new(4, 2),
            crate::models::TerminalBufferCellPos::new(4, 5),
        ));
        assert_eq!(
            state.recover_lost_selection_mouse_up(),
            LostTerminalSelectionRecovery::Committed
        );
        assert!(state.selection.selection.is_some());
        assert!(!state.selection.dragging);
    }

    #[test]
    fn lost_mouse_up_distinguishes_a_single_cell_from_adjacent_empty_sides() {
        use nyaterm_terminal::selection::{
            TerminalBufferCellPos, TerminalCellSide, TerminalSelection, TerminalSelectionEndpoint,
        };

        let mut state = terminal_state();
        state.selection.dragging = true;
        state.selection.selection = Some(TerminalSelection::from_range(
            TerminalBufferCellPos::new(4, 2),
            TerminalBufferCellPos::new(4, 2),
        ));
        assert_eq!(
            state.recover_lost_selection_mouse_up(),
            LostTerminalSelectionRecovery::Committed
        );
        state.selection.dragging = true;
        state.selection.selection = Some(TerminalSelection {
            anchor: TerminalSelectionEndpoint::new(
                TerminalBufferCellPos::new(4, 2),
                TerminalCellSide::Right,
            ),
            head: TerminalBufferCellPos::new(4, 3).into(),
            all_buffer: false,
        });
        assert_eq!(
            state.recover_lost_selection_mouse_up(),
            LostTerminalSelectionRecovery::ClearedEmpty
        );
        assert!(state.selection.selection.is_none());
    }

    #[test]
    fn terminal_view_owner_groups_session_and_frame_lifecycle() {
        let mut state = terminal_state();
        state.ensure_frame_session("session-a".to_string(), "UTF-8".to_string(), 1_000);
        state.append_session_text_or_create("session-a", "UTF-8", 1_000, "hello");

        assert_eq!(state.session_output("session-a"), Some("hello"));
        assert!(!state.session_has_unread("session-a"));
        assert_eq!(state.session_scroll_offset("session-a"), 0);
        assert_eq!(state.frame_queue_metrics().pending_event_count, 0);

        state.remove_frame_session("session-a");
        assert_eq!(state.session_output("session-a"), None);
    }

    #[test]
    fn reconnect_view_rekey_preserves_scroll_and_selection() {
        let mut state = terminal_state();
        state.ensure_frame_session("old".to_string(), "UTF-8".to_string(), 1_000);
        state.append_session_text_or_create("old", "UTF-8", 1_000, "earlier output");
        state.view.views.get_mut("old").unwrap().scroll_offset = 7;
        state.selection.session_id = Some("old".to_string());
        state.selection.selection = Some(crate::models::TerminalSelection::from_range(
            crate::models::TerminalBufferCellPos::new(0, 0),
            crate::models::TerminalBufferCellPos::new(0, 3),
        ));

        state.rekey_session_view("old", "new", "UTF-8");

        assert!(!state.view.views.contains_key("old"));
        assert_eq!(state.session_output("new"), Some("earlier output"));
        assert_eq!(state.session_scroll_offset("new"), 7);
        assert_eq!(state.selection.session_id.as_deref(), Some("new"));
        assert!(state.selection.selection.is_some());
        assert!(state.session_id_is_retired("old"));
    }

    #[test]
    fn terminal_owner_migrates_session_surface_bounds_atomically() {
        let mut state = terminal_state();
        let bounds = Bounds::new(point(px(10.), px(20.)), size(px(800.), px(480.)));
        state
            .layout
            .session_surface_bounds
            .insert("old-session".to_string(), bounds);

        state.move_session_surface_bounds("old-session", "new-session".to_string());

        assert!(
            !state
                .layout
                .session_surface_bounds
                .contains_key("old-session")
        );
        assert_eq!(
            state.layout.session_surface_bounds.get("new-session"),
            Some(&bounds)
        );
    }

    #[test]
    fn session_switch_reset_clears_terminal_assist_transients() {
        let mut state = terminal_state();
        state.editing.input_mut().value = "git status".to_string();
        state.assist.command_suggestions_suppressed = true;
        state.assist.pending_command_history_entry = Some("git status".to_string());
        state.assist.credential_autofill_buffer = "login:".to_string();
        state.assist.credential_autofill_recent = Some((
            nyaterm_core::CredentialPromptKind::Username,
            "login:".to_string(),
        ));
        state.assist.credential_autofill_sending = true;
        state.assist.credential_prompt_input_until_ms = 99;
        let search_generation = state.assist.command_suggestion_search_gen;

        state.reset_assist_for_session_switch();

        assert_eq!(state.editing.predicted_input().value, "git status");
        assert!(!state.assist.command_suggestions_suppressed);
        assert!(state.assist.pending_command_history_entry.is_none());
        assert!(state.assist.credential_autofill_buffer.is_empty());
        assert!(state.assist.credential_autofill_recent.is_none());
        assert!(!state.assist.credential_autofill_sending);
        assert_eq!(state.assist.credential_prompt_input_until_ms, 0);
        assert_eq!(
            state.assist.command_suggestion_search_gen,
            search_generation.saturating_add(1)
        );
    }
}
