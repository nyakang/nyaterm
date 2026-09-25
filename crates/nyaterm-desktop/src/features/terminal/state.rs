//! Grouped terminal feature state.
//!
//! This is presentation state only: which terminals exist, what the user has
//! selected, where the surface was painted. Parsing, snapshots and the wire
//! protocol stay in `nyaterm-terminal` and `nyaterm-transport`.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use gpui::{Entity, FocusHandle, Subscription};
use nyaterm_core::ResolvedKeywordHighlightRule;
use nyaterm_terminal::{TerminalOutputDecoder, TerminalScreen};
use nyaterm_ui::NyaDocumentEditorState;

use super::assist_state::TerminalAssistState;
use super::terminal_surface::TerminalScrollbarDragState;
use super::terminal_surface_entity::TerminalSurface;
use super::window_state::TerminalWindowState;
use crate::features::{FontResolutionStatus, terminal::ResolvedAppearanceFont};
use crate::models::{
    ActionLinkMenuState, ActionLinkTooltipState, RecordingHistorySearchEvent,
    RecordingHistorySearchKey, TerminalFrameEvent, TerminalFramePipeline, TerminalSearchMode,
    TerminalSelection, TerminalViewState,
};
use crate::theme::ThemePalette;

pub(in crate::features) struct TerminalFeatureState {
    pub(super) search: TerminalSearchState,
    pub(super) view: TerminalViewRuntimeState,
    pub(super) input: TerminalInputState,
    pub(super) paste: TerminalPasteReviewState,
    pub(super) assist: TerminalAssistState,
    pub(super) selection: TerminalSelectionState,
    pub(super) layout: TerminalLayoutState,
    pub(super) menus: TerminalMenuState,
    pub(super) paint: TerminalPaintCacheState,
    pub(super) windows: TerminalWindowState,
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
    pub(super) ime_marked_text: String,
}

/// Dedicated multi-line paste editor state backed by gpui-kit's editor.
pub(super) struct TerminalPasteReviewState {
    pub(super) editor: Option<Entity<NyaDocumentEditorState>>,
    _subscription: Option<Subscription>,
}

/// Text selection and mouse reporting.
pub(super) struct TerminalSelectionState {
    pub(super) selection: Option<TerminalSelection>,
    pub(super) session_id: Option<String>,
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
    pub(super) cached_terminal_theme_palette: Option<(String, String, String, ThemePalette)>,
    pub(super) cached_keyword_highlight_rules: Option<Arc<Vec<ResolvedKeywordHighlightRule>>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::features) struct TerminalOverlayVisibility {
    pub paste_review: bool,
    pub actions: bool,
    pub action_link_menu: bool,
    pub action_link_tooltip: bool,
}

impl TerminalFeatureState {
    pub(in crate::features) fn shutdown_workers(&mut self) {
        self.assist.shutdown_workers();
        self.view.frame_pipeline.shutdown();
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
                ime_marked_text: String::new(),
            },
            paste: TerminalPasteReviewState::new(),
            assist: TerminalAssistState::new(),
            selection: TerminalSelectionState {
                selection: None,
                session_id: None,
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
                action_link_tooltip: None,
                action_link_hover_pending: None,
                action_link_hover_generation: 0,
            },
            paint: TerminalPaintCacheState {
                cached_terminal_theme_palette: None,
                cached_keyword_highlight_rules: None,
            },
            windows: TerminalWindowState {
                tree: None,
                drop: None,
                restored: false,
                file_drop_hover: None,
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
        let had_interaction = self.selection.selection.take().is_some()
            || self.selection.dragging
            || self.menus.action_link_menu.is_some()
            || self.menus.action_link_tooltip.is_some();
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
            self.selection.selection = None;
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
        self.paint.cached_keyword_highlight_rules = Some(rules);
    }

    pub(in crate::features) fn cached_terminal_theme_palette(
        &self,
    ) -> Option<(&str, &str, &str, ThemePalette)> {
        self.paint.cached_terminal_theme_palette.as_ref().map(
            |(ui, terminal, contrast, palette)| {
                (ui.as_str(), terminal.as_str(), contrast.as_str(), *palette)
            },
        )
    }

    pub(in crate::features) fn cache_terminal_theme_palette(
        &mut self,
        ui_theme: String,
        terminal_theme: String,
        contrast: String,
        palette: ThemePalette,
    ) {
        self.paint.cached_terminal_theme_palette =
            Some((ui_theme, terminal_theme, contrast, palette));
    }

    pub(in crate::features) fn invalidate_paint_caches(&mut self) {
        self.paint.cached_terminal_theme_palette = None;
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
    use nyaterm_core::TerminalInputState as CommandInputState;
    use nyaterm_terminal::{TerminalOutputDecoder, TerminalScreen};

    use super::super::window_state::{TerminalWindowDockResult, TerminalWindowReconcileResult};
    use super::{LostTerminalSelectionRecovery, TerminalFeatureFocus, TerminalFeatureState};
    use crate::models::{
        SmartSplitMode, TabDockEdge, TabDockZone, TerminalFramePipeline, TerminalSearchMode,
    };

    fn terminal_state() -> TerminalFeatureState {
        let cx = TestAppContext::single();
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
        state.selection.dragging = true;
        state.menus.action_link_hover_pending = Some((
            "https://example.com".to_string(),
            1,
            crate::models::ActionLinkTooltipState {
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
    fn terminal_view_owner_groups_session_and_frame_lifecycle() {
        let mut state = terminal_state();
        state.ensure_frame_session("session-a".to_string(), "UTF-8".to_string(), 1_000);
        state.append_session_text_or_create("session-a", "UTF-8", "hello");

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
        state.append_session_text_or_create("old", "UTF-8", "earlier output");
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
    fn terminal_window_owner_reconciles_tabs_and_reconnect_ids_atomically() {
        let mut state = terminal_state();
        let initial = vec!["alpha".to_string(), "beta".to_string()];
        let focused = state
            .apply_smart_split(&initial, SmartSplitMode::Vertical, Some("beta"))
            .expect("layout");

        let live = vec!["beta".to_string(), "gamma".to_string()];
        let result = state.reconcile_terminal_windows(&live, focused.as_deref(), Some("gamma"));
        assert!(matches!(
            result,
            TerminalWindowReconcileResult::Reconciled { .. }
        ));
        assert!(state.replace_terminal_window_tab_id("beta", "beta-reconnected"));

        let root = state.windows.tree.as_ref().expect("window tree");
        assert_eq!(
            root.collect_tab_ids()
                .into_iter()
                .collect::<std::collections::HashSet<_>>(),
            ["beta-reconnected".to_string(), "gamma".to_string()]
                .into_iter()
                .collect()
        );
    }

    #[test]
    fn terminal_window_owner_docks_and_clears_transient_targets() {
        let mut state = terminal_state();
        let leaf_id = state
            .ensure_terminal_windows_root(
                vec!["alpha".to_string(), "beta".to_string()],
                Some("alpha".to_string()),
            )
            .expect("leaf");
        assert!(
            state.set_terminal_window_drop(leaf_id.clone(), TabDockZone::Edge(TabDockEdge::Right),)
        );
        assert_eq!(
            state.terminal_window_drop_for_leaf(&leaf_id),
            Some(TabDockZone::Edge(TabDockEdge::Right))
        );

        let result = state.dock_tab_on_terminal_window_leaf(
            "alpha",
            &leaf_id,
            TabDockZone::Edge(TabDockEdge::Right),
        );
        assert!(matches!(result, TerminalWindowDockResult::Docked { .. }));
        assert!(state.terminal_windows_is_multi_leaf());
        assert!(state.terminal_window_drop_for_leaf(&leaf_id).is_none());

        assert!(state.set_terminal_file_drop_hover(Some("alpha".to_string())));
        assert!(state.terminal_file_drop_hover_matches("alpha"));
        assert!(!state.clear_terminal_file_drop_hover_for_session("beta"));
        assert!(state.terminal_file_drop_hover_matches("alpha"));
        assert!(state.clear_terminal_file_drop_hover_for_session("alpha"));
        assert!(!state.terminal_file_drop_hover_is_pending());

        assert!(state.set_terminal_file_drop_hover(Some("alpha".to_string())));
        assert!(state.clear_terminal_file_drop_hover());
        assert!(!state.terminal_file_drop_hover_is_pending());
    }

    #[test]
    fn terminal_window_owner_round_trips_restorable_multi_leaf_layout() {
        let mut state = terminal_state();
        let ordered = vec!["alpha".to_string(), "beta".to_string()];
        state
            .apply_smart_split(&ordered, SmartSplitMode::Horizontal, Some("beta"))
            .expect("layout");
        let layout = state
            .serialize_terminal_window_layout(&ordered)
            .expect("serialized layout");

        let mut restored = terminal_state();
        restored.complete_terminal_windows_restore();
        assert!(restored.terminal_windows_restore_is_complete());
        restored.mark_terminal_windows_restore_pending();
        assert!(!restored.terminal_windows_restore_is_complete());
        restored
            .restore_terminal_window_layout(&layout, &ordered, Some("beta"))
            .expect("restored layout");

        assert!(restored.terminal_windows_is_multi_leaf());
        assert_eq!(
            restored
                .windows
                .tree
                .as_ref()
                .expect("window tree")
                .collect_tab_ids()
                .into_iter()
                .collect::<std::collections::HashSet<_>>(),
            ordered.into_iter().collect()
        );
    }

    #[test]
    fn session_switch_reset_clears_terminal_assist_transients() {
        let mut state = terminal_state();
        state.assist.command_input_tracker.value = "git status".to_string();
        state.assist.command_suggestions_suppressed = true;
        state.assist.pending_command_history_entry = Some("git status".to_string());
        state.assist.credential_autofill_buffer = "login:".to_string();
        state
            .assist
            .credential_autofill_recent
            .insert("username:login:".to_string(), 42);
        state.assist.credential_autofill_sending = true;
        state.assist.credential_prompt_input_until_ms = 99;
        let search_generation = state.assist.command_suggestion_search_gen;

        state.reset_assist_for_session_switch();

        assert_eq!(state.assist.command_input_tracker, CommandInputState::new());
        assert!(!state.assist.command_suggestions_suppressed);
        assert!(state.assist.pending_command_history_entry.is_none());
        assert!(state.assist.credential_autofill_buffer.is_empty());
        assert!(state.assist.credential_autofill_recent.is_empty());
        assert!(!state.assist.credential_autofill_sending);
        assert_eq!(state.assist.credential_prompt_input_until_ms, 0);
        assert_eq!(
            state.assist.command_suggestion_search_gen,
            search_generation.saturating_add(1)
        );
    }
}
