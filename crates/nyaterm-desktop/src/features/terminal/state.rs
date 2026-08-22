//! Grouped terminal feature state.
//!
//! This is presentation state only: which terminals exist, what the user has
//! selected, where the surface was painted. Parsing, snapshots and the wire
//! protocol stay in `nyaterm-terminal` and `nyaterm-transport`.

use std::collections::{HashMap, VecDeque};
use std::ops::Range;
use std::sync::Arc;
use std::time::Instant;

use gpui::{Entity, FocusHandle, Subscription};
use nyaterm_core::ResolvedKeywordHighlightRule;
use nyaterm_terminal::{TerminalOutputDecoder, TerminalScreen};

use super::assist_state::TerminalAssistState;
use super::terminal_surface::TerminalScrollbarDragState;
use super::terminal_surface_entity::TerminalSurface;
use super::window_state::TerminalWindowState;
use crate::features::shell::ResolvedAppearanceFont;
use crate::models::{
    ActionLinkMenuState, ActionLinkTooltipState, MultiLinePasteDraft, RecordingHistorySearchEvent,
    RecordingHistorySearchKey, TerminalFrameEvent, TerminalFramePipeline, TerminalSearchMode,
    TerminalSelection, TerminalViewState, normalize_paste_newlines,
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
    pub paste: FocusHandle,
}

/// In-terminal find bar and recording history search.
pub(super) struct TerminalSearchState {
    pub(super) open: bool,
    pub(super) query: String,
    pub(super) mode: TerminalSearchMode,
    pub(super) case_sensitive: bool,
    pub(super) regex: bool,
    pub(super) whole_word: bool,
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
}

/// Keyboard focus and IME composition for the terminal surface.
pub(super) struct TerminalInputState {
    pub(super) focus: FocusHandle,
    pub(super) focus_active: bool,
    pub(super) focus_subscriptions: Vec<Subscription>,
    pub(super) ime_marked_text: String,
}

/// Dedicated multi-line paste editor state.
///
/// This remains separate from registry-backed single-line inputs because it
/// owns a byte cursor, selection anchor and IME composition range.
pub(super) struct TerminalPasteReviewState {
    pub(super) draft: Option<MultiLinePasteDraft>,
    pub(super) marked_text: String,
    pub(super) marked_range: Option<Range<usize>>,
    pub(super) cursor: usize,
    pub(super) anchor: Option<usize>,
    pub(super) focus: FocusHandle,
}

/// Text selection and mouse reporting.
pub(super) struct TerminalSelectionState {
    pub(super) selection: Option<TerminalSelection>,
    pub(super) session_id: Option<String>,
    pub(super) selected_occurrence: TerminalSelectedOccurrenceState,
    pub(super) dragging: bool,
    pub(super) mouse_report_button: Option<u8>,
    pub(super) mouse_report_session_id: Option<String>,
    pub(super) mouse_report_peer_session_ids: Vec<String>,
    pub(super) mouse_report_position: Option<(u16, u16)>,
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
    /// Runtime-only fallback when the configured terminal font is unavailable or proportional.
    pub(super) terminal_font_override: Option<ResolvedAppearanceFont>,
}

/// Cached terminal-font validation for one configured family/size/weight tuple.
///
/// Font enumeration and glyph measurement are expensive TextSystem operations. Keep
/// the result in runtime state, while leaving the persisted appearance unchanged.
#[derive(Clone, Debug)]
pub(super) struct TerminalFontMetricsCache {
    pub(super) configured_family: String,
    pub(super) font_size: u16,
    pub(super) font_weight: u16,
    pub(super) resolved_font: Option<ResolvedAppearanceFont>,
    pub(super) cell_width: f32,
}

/// Terminal actions overlay and context menu.
pub(super) struct TerminalMenuState {
    pub(super) actions_open: bool,
    pub(super) actions_focus: FocusHandle,
    pub(super) action_link_menu: Option<ActionLinkMenuState>,
    pub(super) action_link_tooltip: Option<ActionLinkTooltipState>,
    /// Pending action-link hover (Tauri 250ms delay before showing tooltip).
    pub(super) action_link_hover_pending: Option<(String, Instant, ActionLinkTooltipState)>,
}

/// Paint-time caches invalidated whenever appearance settings change.
pub(super) struct TerminalPaintCacheState {
    pub(super) cached_terminal_theme_palette: Option<(String, String, String, ThemePalette)>,
    pub(super) cached_keyword_highlight_rules: Option<Arc<Vec<ResolvedKeywordHighlightRule>>>,
}

pub(in crate::features) struct TerminalPasteReviewView<'a> {
    pub draft: Option<&'a MultiLinePasteDraft>,
    pub selected_byte_range: Range<usize>,
    pub cursor: usize,
    pub marked_range: Option<Range<usize>>,
    pub focus: &'a FocusHandle,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::features) struct TerminalOverlayVisibility {
    pub paste_review: bool,
    pub actions: bool,
    pub action_link_menu: bool,
    pub action_link_tooltip: bool,
}

impl TerminalFeatureState {
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
            },
            input: TerminalInputState {
                focus: focus.terminal,
                focus_active: false,
                focus_subscriptions: Vec::new(),
                ime_marked_text: String::new(),
            },
            paste: TerminalPasteReviewState::new(focus.paste),
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
            },
            menus: TerminalMenuState {
                actions_open: false,
                actions_focus: focus.actions,
                action_link_menu: None,
                action_link_tooltip: None,
                action_link_hover_pending: None,
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

    pub(in crate::features) fn paste_review(&self) -> TerminalPasteReviewView<'_> {
        TerminalPasteReviewView {
            draft: self.paste.draft.as_ref(),
            selected_byte_range: self.paste.selected_byte_range(),
            cursor: self.paste.cursor,
            marked_range: self.paste.marked_range.clone(),
            focus: &self.paste.focus,
        }
    }

    pub(in crate::features) fn overlay_visibility(&self) -> TerminalOverlayVisibility {
        TerminalOverlayVisibility {
            paste_review: self.paste.draft.is_some(),
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

    pub(in crate::features) fn action_link_hover_is_pending(&self) -> bool {
        self.menus.action_link_hover_pending.is_some()
    }

    pub(in crate::features) fn clear_activation_interaction(&mut self) -> bool {
        let had_interaction = self.selection.selection.take().is_some()
            || self.selection.dragging
            || self.menus.action_link_menu.is_some()
            || self.menus.action_link_tooltip.is_some();
        self.selection.dragging = false;
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
    }

    pub(in crate::features) fn terminal_font_override(&self) -> Option<&ResolvedAppearanceFont> {
        self.layout.terminal_font_override.as_ref()
    }

    pub(in crate::features) fn set_terminal_font_override(
        &mut self,
        font: Option<ResolvedAppearanceFont>,
    ) {
        self.layout.terminal_font_override = font;
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
    fn new(focus: FocusHandle) -> Self {
        Self {
            draft: None,
            marked_text: String::new(),
            marked_range: None,
            cursor: 0,
            anchor: None,
            focus,
        }
    }

    pub(super) fn open(&mut self, text: String) {
        let text = normalize_paste_newlines(&text);
        self.cursor = text.len();
        self.anchor = None;
        self.marked_range = None;
        self.draft = Some(MultiLinePasteDraft::new(text));
        self.marked_text.clear();
    }

    pub(super) fn clear(&mut self) {
        self.draft = None;
        self.reset_editing_state();
    }

    pub(super) fn take_normalized_text(&mut self) -> Option<String> {
        let text = self.draft.take().map(|draft| draft.normalized_text());
        self.reset_editing_state();
        text
    }

    pub(super) fn text(&self) -> &str {
        self.draft
            .as_ref()
            .map(|draft| draft.text.as_str())
            .unwrap_or_default()
    }

    pub(super) fn selected_byte_range(&self) -> Range<usize> {
        let cursor = floor_char_boundary(self.text(), self.cursor);
        let anchor = floor_char_boundary(self.text(), self.anchor.unwrap_or(cursor));
        if anchor <= cursor {
            anchor..cursor
        } else {
            cursor..anchor
        }
    }

    pub(super) fn select_all(&mut self) {
        self.anchor = Some(0);
        self.cursor = self.text().len();
        self.clear_marked_text();
    }

    pub(super) fn previous_char_boundary(&self) -> usize {
        previous_char_boundary(self.text(), self.cursor)
    }

    pub(super) fn next_char_boundary(&self) -> usize {
        next_char_boundary(self.text(), self.cursor)
    }

    pub(super) fn current_line_start(&self) -> usize {
        line_start(self.text(), self.cursor)
    }

    pub(super) fn current_line_end(&self) -> usize {
        line_end(self.text(), self.cursor)
    }

    pub(super) fn move_cursor(&mut self, cursor: usize, extend: bool) {
        let cursor = floor_char_boundary(self.text(), cursor);
        if extend {
            self.anchor.get_or_insert(self.cursor);
        } else {
            self.anchor = None;
        }
        self.cursor = cursor;
        self.clear_marked_text();
    }

    pub(super) fn move_vertical(&mut self, delta: isize, extend: bool) {
        let text = self.text();
        let cursor = floor_char_boundary(text, self.cursor);
        let current_start = line_start(text, cursor);
        let column = text[current_start..cursor].chars().count();
        let target_start = if delta < 0 {
            if current_start == 0 {
                0
            } else {
                line_start(text, current_start - 1)
            }
        } else {
            let current_end = line_end(text, cursor);
            if current_end >= text.len() {
                current_start
            } else {
                current_end + 1
            }
        };
        let target_end = line_end(text, target_start);
        let target = text[target_start..target_end]
            .char_indices()
            .nth(column)
            .map(|(offset, _)| target_start + offset)
            .unwrap_or(target_end);
        self.move_cursor(target, extend);
    }

    pub(super) fn replace_selection(&mut self, text: &str) -> bool {
        self.replace_range(self.selected_byte_range(), text)
    }

    pub(super) fn replace_range(&mut self, range: Range<usize>, text: &str) -> bool {
        let Some(draft) = self.draft.as_mut() else {
            return false;
        };
        let start = floor_char_boundary(&draft.text, range.start);
        let end = floor_char_boundary(&draft.text, range.end).max(start);
        draft.text.replace_range(start..end, text);
        self.cursor = start + text.len();
        self.anchor = None;
        self.clear_marked_text();
        true
    }

    fn reset_editing_state(&mut self) {
        self.marked_text.clear();
        self.marked_range = None;
        self.cursor = 0;
        self.anchor = None;
    }

    fn clear_marked_text(&mut self) {
        self.marked_text.clear();
        self.marked_range = None;
    }
}

fn floor_char_boundary(text: &str, offset: usize) -> usize {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

fn previous_char_boundary(text: &str, offset: usize) -> usize {
    let offset = floor_char_boundary(text, offset);
    text[..offset]
        .char_indices()
        .next_back()
        .map(|(index, _)| index)
        .unwrap_or(0)
}

fn next_char_boundary(text: &str, offset: usize) -> usize {
    let offset = floor_char_boundary(text, offset);
    text[offset..]
        .chars()
        .next()
        .map(|ch| offset + ch.len_utf8())
        .unwrap_or(offset)
}

fn line_start(text: &str, offset: usize) -> usize {
    text[..floor_char_boundary(text, offset)]
        .rfind('\n')
        .map(|index| index + 1)
        .unwrap_or(0)
}

fn line_end(text: &str, offset: usize) -> usize {
    let offset = floor_char_boundary(text, offset);
    text[offset..]
        .find('\n')
        .map(|index| offset + index)
        .unwrap_or(text.len())
}

#[cfg(test)]
mod tests {
    use gpui::{Bounds, TestAppContext, point, px, size};
    use nyaterm_core::TerminalInputState as CommandInputState;
    use nyaterm_terminal::{TerminalOutputDecoder, TerminalScreen};

    use super::super::window_state::{TerminalWindowDockResult, TerminalWindowReconcileResult};
    use super::{
        LostTerminalSelectionRecovery, TerminalFeatureFocus, TerminalFeatureState,
        TerminalPasteReviewState,
    };
    use crate::models::{
        SmartSplitMode, TabDockEdge, TabDockZone, TerminalFramePipeline, TerminalSearchMode,
    };

    fn paste_state() -> TerminalPasteReviewState {
        let cx = TestAppContext::single();
        let focus = cx.update(|cx| cx.focus_handle());
        TerminalPasteReviewState::new(focus)
    }

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
                    paste: cx.focus_handle(),
                },
            )
        })
    }

    #[test]
    fn terminal_owner_projects_overlay_visibility_and_search_mode() {
        let mut state = terminal_state();
        state.paste.open("echo hello".to_string());
        state.menus.actions_open = true;
        state.search.open = true;
        state.set_search_mode(TerminalSearchMode::History);

        let overlays = state.overlay_visibility();
        assert!(overlays.paste_review);
        assert!(overlays.actions);
        assert!(!state.buffer_search_is_open());

        state.set_search_mode(TerminalSearchMode::Buffer);
        assert!(state.buffer_search_is_open());
    }

    #[test]
    fn terminal_owner_clears_activation_interaction_as_one_transition() {
        let mut state = terminal_state();
        state.selection.dragging = true;
        state.menus.action_link_hover_pending = Some((
            "https://example.com".to_string(),
            std::time::Instant::now(),
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

    #[test]
    fn paste_cursor_operations_stay_on_utf8_boundaries() {
        let mut state = paste_state();
        state.open("a你🙂b".to_string());

        state.move_cursor(2, false);
        assert_eq!(state.cursor, 1);
        assert_eq!(state.next_char_boundary(), 4);

        state.move_cursor(4, false);
        assert_eq!(state.previous_char_boundary(), 1);
        assert_eq!(state.next_char_boundary(), 8);
    }

    #[test]
    fn paste_selection_replacement_resets_selection_and_ime_state() {
        let mut state = paste_state();
        state.open("alpha\nβeta".to_string());
        state.move_cursor(0, false);
        state.move_cursor(5, true);
        state.marked_text = "composition".to_string();
        state.marked_range = Some(0..5);

        assert_eq!(state.selected_byte_range(), 0..5);
        assert!(state.replace_selection("替换"));

        assert_eq!(state.text(), "替换\nβeta");
        assert_eq!(state.cursor, "替换".len());
        assert!(state.anchor.is_none());
        assert!(state.marked_text.is_empty());
        assert!(state.marked_range.is_none());
    }

    #[test]
    fn paste_vertical_movement_preserves_character_column() {
        let mut state = paste_state();
        state.open("ab\n你cde\nz".to_string());

        state.move_cursor(7, false);
        state.move_vertical(-1, false);
        assert_eq!(state.cursor, 2);

        state.move_vertical(1, true);
        assert_eq!(state.cursor, 7);
        assert_eq!(state.anchor, Some(2));
        assert_eq!(state.selected_byte_range(), 2..7);
    }

    #[test]
    fn taking_or_clearing_paste_draft_resets_editor_transients() {
        let mut state = paste_state();
        state.open("first\r\nsecond\rthird".to_string());
        state.marked_text = "ime".to_string();
        state.marked_range = Some(0..3);
        state.anchor = Some(1);

        assert_eq!(
            state.take_normalized_text().as_deref(),
            Some("first\nsecond\nthird")
        );
        assert!(state.draft.is_none());
        assert_eq!(state.cursor, 0);
        assert!(state.anchor.is_none());
        assert!(state.marked_text.is_empty());
        assert!(state.marked_range.is_none());

        state.open("another draft".to_string());
        state.clear();
        assert!(state.draft.is_none());
        assert_eq!(state.cursor, 0);
    }
}
