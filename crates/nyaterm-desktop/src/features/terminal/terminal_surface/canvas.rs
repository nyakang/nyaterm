use rust_i18n::t;

use std::collections::HashMap;
use std::time::{Duration, Instant};

use gpui::{
    ClickEvent, Context, FontWeight, IntoElement, KeyDownEvent, KeyUpEvent, MouseButton,
    SharedString, div, prelude::*, px, rgb, rgba,
};
use nyaterm_terminal::{TerminalScreen, TerminalSnapshot};
use nyaterm_ui::{NyaContextMenu, NyaCopy, NyaPaste, NyaSelectAll};

use crate::features::NyaTermApp;
use crate::features::formatting::{
    TerminalTimestampFormatter, terminal_gutter_labels, terminal_gutter_row_count,
    terminal_timestamp_format_width_chars,
};
use crate::features::terminal::terminal_runtime::TerminalMouseReportRequest;
use crate::features::terminal::terminal_selection_runtime::{
    terminal_bounds_tracker, terminal_gutter_metrics,
};
use crate::features::terminal::{
    TERMINAL_KEY_CONTEXT, TerminalControlC, TerminalShiftTab, TerminalTab,
};
use crate::models::{
    SessionLaunchConfig, TerminalPerformanceMode, TerminalPerformanceOverlay, TerminalSearchMode,
    terminal_action_link_matcher_key, terminal_expensive_interactions_enabled,
};
use crate::terminal::{NyaTerminalElement, TerminalGridSelection, TerminalLineDecorations};
use crate::widgets::small_button;

use super::decorations::terminal_action_links_for_paint_snapshot;
use super::helpers::terminal_plain_text_input_event;
use nyaterm_terminal_gpui::decorations::{
    TerminalDecorationSources, build_terminal_line_decorations,
    terminal_action_links_have_ranges_for_snapshot, terminal_line_decorations_cache_key,
    terminal_line_decorations_needed, terminal_snapshot_absolute_range,
};

fn terminal_shell_placeholder_snapshot() -> std::sync::Arc<TerminalSnapshot> {
    static SNAPSHOT: std::sync::OnceLock<std::sync::Arc<TerminalSnapshot>> =
        std::sync::OnceLock::new();
    std::sync::Arc::clone(
        SNAPSHOT
            .get_or_init(|| std::sync::Arc::new(TerminalScreen::default().viewport_snapshot(0))),
    )
}

impl NyaTermApp {
    pub(in crate::features::terminal) fn terminal_tab_key_event(
        &mut self,
        shift: bool,
    ) -> KeyDownEvent {
        let is_held = self
            .session
            .active_id()
            .and_then(|id| self.terminal.view.views.get_mut(id))
            .is_some_and(|view| std::mem::replace(&mut view.tab_action_pressed, true));
        KeyDownEvent {
            keystroke: gpui::Keystroke {
                modifiers: gpui::Modifiers {
                    shift,
                    ..gpui::Modifiers::default()
                },
                key: "tab".to_string(),
                key_char: None,
            },
            is_held,
            prefer_character_input: false,
        }
    }

    fn terminal_control_c_key_event() -> KeyDownEvent {
        KeyDownEvent {
            keystroke: gpui::Keystroke {
                modifiers: gpui::Modifiers {
                    control: true,
                    ..gpui::Modifiers::default()
                },
                key: "c".to_string(),
                key_char: Some("c".to_string()),
            },
            is_held: false,
            prefer_character_input: false,
        }
    }

    fn handle_terminal_surface_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        if !self.terminal.input.focus.is_focused(window) {
            return;
        }
        self.mark_user_activity();
        let smart_input_selection = self
            .terminal
            .selection
            .selection
            .is_some()
            .then(|| self.smart_cursor_selected_input_range())
            .flatten();
        let is_plain_text_input = terminal_plain_text_input_event(event);
        if is_plain_text_input {
            if self.terminal.assist.credential_suggestions.is_some()
                && self.handle_credential_suggestion_key(event, cx)
            {
                cx.stop_propagation();
                return;
            }
            if self.terminal_should_defer_key_text_to_input_handler(event) {
                return;
            }
            cx.stop_propagation();
            if smart_input_selection.is_some()
                && let Some(text) = event.keystroke.key_char.as_deref()
            {
                self.commit_terminal_text(text, cx);
            } else {
                self.send_terminal_key_event(event, true, cx);
            }
            return;
        }
        if self.handle_global_shortcut(event, window, cx) {
            cx.stop_propagation();
            return;
        }
        if self.handle_credential_suggestion_key(event, cx) {
            cx.stop_propagation();
            return;
        }
        if self.handle_command_suggestion_key(event, cx) {
            self.open_pending_suggestion_paste(window, cx);
            cx.stop_propagation();
            return;
        }
        if self.handle_terminal_scroll_key(event, cx) {
            cx.stop_propagation();
            return;
        }
        if self.handle_smart_input_selection_key(event, cx) {
            cx.stop_propagation();
            return;
        }
        // Disconnected tab: Enter reconnects; other keys show status (Tauri).
        if let Some(session_id) = self.session.active_id_owned()
            && self.session.is_disconnected(&session_id)
        {
            cx.stop_propagation();
            let keystroke = &event.keystroke;
            if !keystroke.modifiers.control
                && !keystroke.modifiers.platform
                && !keystroke.modifiers.alt
                && keystroke.key.as_str() == "enter"
            {
                self.reconnect_session(session_id, window, cx);
            } else if keystroke.key.as_str() == "d"
                && keystroke.modifiers.control
                && !keystroke.modifiers.platform
                && !keystroke.modifiers.alt
            {
                // Ctrl+D closes disconnected tab (Tauri onDisconnectedClose).
                self.close_session(session_id, cx);
            } else if self
                .set_terminal_status_if_changed("session disconnected — press Enter to reconnect")
            {
                cx.notify();
            }
            return;
        }
        #[cfg(target_os = "macos")]
        {
            let keystroke = &event.keystroke;
            if keystroke.modifiers.platform
                && !keystroke.modifiers.control
                && !keystroke.modifiers.alt
                && !keystroke.modifiers.function
                && matches!(keystroke.key.as_str(), "v" | "V")
            {
                cx.stop_propagation();
                self.paste_from_clipboard(window, cx);
                return;
            }
        }
        if self.terminal_should_defer_key_text_to_input_handler(event) {
            // Text goes through the IME adapter, but its physical release must
            // still belong to the same connection as this press.
            if let Some(id) = self.session.active_id_owned() {
                let mut targets = self.sync_peer_session_ids(&id);
                targets.push(id);
                for id in targets {
                    if let Some(view) = self.terminal.view.views.get_mut(&id)
                        && !event.is_held
                    {
                        view.pressed_keys.insert(event.keystroke.key.clone());
                    }
                }
            }
            return;
        }
        cx.stop_propagation();
        // When a non-smart buffer selection is painted, still send
        // keystrokes but skip suggestion tracking so the selection
        // edit path stays isolated (Tauri preserves selection).
        let has_buffer_selection =
            self.terminal.selection.selection.is_some() && smart_input_selection.is_none();
        if has_buffer_selection {
            self.send_terminal_key_event(event, false, cx);
        } else {
            self.send_terminal_key_event(event, true, cx);
        }
    }

    pub(in crate::features) fn terminal_canvas_for(
        &mut self,
        session_id: String,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let render_started_at = Instant::now();
        self.ensure_paint_theme_caches();
        let palette = self.terminal_theme_palette();
        let is_active = self.session.active_id() == Some(session_id.as_str());
        let is_disconnected = !session_id.is_empty() && self.session.is_disconnected(&session_id);
        let terminal_mouse_reporting = !session_id.is_empty()
            && self
                .terminal_protocol_state_for_session(&session_id)
                .mouse_reporting;
        let action_links_enabled = self.settings.summary().terminal_action_links_enabled;
        let render_output_pressure = self.runtime_output_pressure_active();
        let render_pressure = self
            .terminal
            .view
            .views
            .get(&session_id)
            .map(|view| {
                terminal_render_pressure_active(
                    render_output_pressure,
                    view.output_burst_bytes,
                    view.performance_mode,
                )
            })
            .unwrap_or(render_output_pressure);
        if render_pressure && let Some(view) = self.terminal.view.views.get_mut(&session_id) {
            view.enter_render_degraded_mode();
        }
        let render_degraded = self
            .terminal
            .view
            .views
            .get(&session_id)
            .is_some_and(|view| view.render_degraded || render_pressure);
        let render_profile = terminal_render_profile(render_degraded);
        let (output_burst_bytes, performance_mode) = self
            .terminal
            .view
            .views
            .get(&session_id)
            .map(|view| (view.output_burst_bytes, view.performance_mode))
            .unwrap_or((0, TerminalPerformanceMode::Normal));
        let expensive_interactions_enabled = terminal_expensive_interactions_enabled(
            action_links_enabled,
            is_active,
            render_degraded,
            render_output_pressure,
            output_burst_bytes,
            performance_mode,
        );
        let action_link_matcher_key = terminal_action_link_matcher_key(
            action_links_enabled,
            &self.settings.summary().terminal_action_links_matchers,
        );
        let keyword_rules = if session_id.is_empty() {
            if render_degraded || !is_active {
                std::sync::Arc::new(Vec::new())
            } else {
                self.resolved_keyword_highlight_rules()
            }
        } else {
            // Live sessions paint keywords on TerminalSurface.
            std::sync::Arc::new(Vec::new())
        };
        let snapshot_stage_started_at = Instant::now();
        let layout_cache = self
            .terminal
            .view
            .views
            .get(&session_id)
            .map(|view| view.render_cache.layout_cache.clone());
        let scroll_offset = self
            .terminal
            .view
            .views
            .get(&session_id)
            .map(|view| view.scroll_offset)
            .unwrap_or(self.terminal.view.scroll_offset);
        let display_offset = self.terminal_display_offset_for_session(
            (!session_id.is_empty()).then_some(session_id.as_str()),
        );
        // Live grid and IME preedit are painted together by TerminalSurface.
        let snapshot = if session_id.is_empty() {
            self.terminal_snapshot_for_session(None, display_offset)
        } else {
            terminal_shell_placeholder_snapshot()
        };
        let line_count = snapshot.row_count();
        let cursor_row = snapshot.cursor.row;
        let snapshot_rows = snapshot.row_count();
        let snapshot_cols = snapshot.cols;
        let viewport_snapshot_duration = snapshot_stage_started_at.elapsed();
        let show_line_numbers = self.settings.summary().terminal_show_line_numbers;
        let show_timestamps = self.settings.summary().terminal_show_timestamps;
        let timestamp_format = self.settings.summary().terminal_timestamp_format.clone();
        let timestamp_width_chars = terminal_timestamp_format_width_chars(&timestamp_format);
        let timestamp_formatter = TerminalTimestampFormatter::new(&timestamp_format);
        let gutter_enabled = show_line_numbers || show_timestamps;
        // Prefer remote cursor visibility/shape from the terminal model; settings
        // supply the default paint style when the model reports a block cursor.
        let remote_cursor_visible = snapshot.cursor.visible
            && snapshot.cursor.shape != nyaterm_terminal::CursorShape::Hidden
            && cursor_row != usize::MAX;
        let blink_enabled = self.settings.summary().cursor_blink || snapshot.cursor.blinking;
        let show_cursor = is_active
            && !session_id.is_empty()
            && !is_disconnected
            && display_offset == 0
            && remote_cursor_visible
            && (!blink_enabled || self.shell.cursor_blink_on());
        let cursor_style = match snapshot.cursor.shape {
            nyaterm_terminal::CursorShape::Underline => "underline".to_string(),
            nyaterm_terminal::CursorShape::Beam => "bar".to_string(),
            nyaterm_terminal::CursorShape::Hidden => self.settings.summary().cursor_style.clone(),
            nyaterm_terminal::CursorShape::Block => self.settings.summary().cursor_style.clone(),
        };
        let (abs_start, abs_end) = terminal_snapshot_absolute_range(&snapshot);
        let _ = abs_end;
        let terminal_selection = is_active
            .then_some(self.terminal.selection.selection)
            .flatten();
        let (
            line_decorations,
            search_mapping_duration,
            action_link_duration,
            decorations_duration,
            search_matches_len,
        ) = if !session_id.is_empty() {
            // Decorations live on TerminalSurface (sync_terminal_surface_paint).
            (
                std::sync::Arc::from(Vec::<TerminalLineDecorations>::new()),
                Duration::ZERO,
                Duration::ZERO,
                Duration::ZERO,
                0usize,
            )
        } else {
            let frame_action_links = if session_id.is_empty() {
                terminal_action_links_for_paint_snapshot(
                    self.terminal.view.views.get(&session_id),
                    display_offset,
                    &snapshot,
                    action_link_matcher_key,
                )
            } else {
                // Live surfaces own action-link paint; shell only needs links for empty session.
                Vec::new()
            };
            let search_stage_started_at = Instant::now();
            let search_matches = if render_profile.enhanced_decorations_enabled()
                && is_active
                && self.terminal.search.open
                && self.terminal.search.mode == TerminalSearchMode::Buffer
            {
                self.terminal_buffer_matches().unwrap_or_default()
            } else {
                std::sync::Arc::from([])
            };
            // Buffer matches use absolute history indices; map into current viewport rows.
            let occurrences = self.terminal_buffer_occurrence_ranges();
            let active_range = occurrences
                .get(
                    self.terminal
                        .search
                        .active_index
                        .min(occurrences.len().saturating_sub(1)),
                )
                .cloned();
            let mut search_ranges_by_line: HashMap<usize, Vec<(usize, usize)>> = HashMap::new();
            let mut active_search_ranges_by_line: HashMap<usize, Vec<(usize, usize)>> =
                HashMap::new();
            let mut selected_occurrence_ranges_by_line: HashMap<usize, Vec<(usize, usize)>> =
                HashMap::new();
            // Selected occurrences are explicit user feedback. Keep them
            // visible while optional decorations are degraded.
            if is_active {
                let selected_matches = self
                    .terminal_selected_occurrence_matches_for_session(&session_id)
                    .unwrap_or_default();
                for search_match in selected_matches.iter_in_absolute_range(abs_start..abs_end) {
                    let abs = search_match.line_index;
                    selected_occurrence_ranges_by_line
                        .entry(abs - abs_start)
                        .or_default()
                        .push((search_match.start_col, search_match.end_col));
                }
            }
            for (match_index, search_match) in search_matches.iter().enumerate() {
                let abs = search_match.line_index;
                if abs < abs_start || abs >= abs_end {
                    continue;
                }
                let view_row = abs - abs_start;
                let range = (search_match.start_col, search_match.end_col);
                search_ranges_by_line
                    .entry(view_row)
                    .or_default()
                    .push(range);
                if active_range
                    .as_ref()
                    .is_some_and(|range| range.contains(&match_index))
                {
                    active_search_ranges_by_line
                        .entry(view_row)
                        .or_default()
                        .push(range);
                }
            }
            let search_mapping_duration = search_stage_started_at.elapsed();
            let decoration_stage_started_at = Instant::now();
            let mut action_link_duration = Duration::ZERO;
            let has_selected_occurrences = !selected_occurrence_ranges_by_line.is_empty();
            let has_search_decorations =
                !search_ranges_by_line.is_empty() || !active_search_ranges_by_line.is_empty();
            let has_frame_action_links = action_links_enabled
                && terminal_action_links_have_ranges_for_snapshot(&snapshot, &frame_action_links);
            let has_hyperlinks = action_links_enabled
                && snapshot.rows().iter().any(|row| !row.hyperlinks.is_empty());
            let needs_line_decorations = terminal_line_decorations_needed(
                has_selected_occurrences,
                has_search_decorations,
                has_frame_action_links,
                has_hyperlinks,
            );
            let line_decorations = if needs_line_decorations {
                let include_action_links = action_links_enabled;
                let include_hyperlinks = action_links_enabled;
                let decoration_sources = TerminalDecorationSources {
                    selected_occurrence_ranges_by_line: &selected_occurrence_ranges_by_line,
                    search_ranges_by_line: &search_ranges_by_line,
                    active_search_ranges_by_line: &active_search_ranges_by_line,
                    frame_action_links: &frame_action_links,
                    include_action_links,
                    include_hyperlinks,
                };
                let decoration_cache_key =
                    terminal_line_decorations_cache_key(&snapshot, &decoration_sources);
                let mut build = || {
                    let action_link_started_at = Instant::now();
                    let decorations =
                        build_terminal_line_decorations(&snapshot, &decoration_sources);
                    action_link_duration += action_link_started_at.elapsed();
                    decorations
                };
                if let Some(view) = self.terminal.view.views.get(&session_id) {
                    view.render_cache
                        .line_decorations(decoration_cache_key, build)
                } else {
                    build().into()
                }
            } else {
                std::sync::Arc::from(Vec::<TerminalLineDecorations>::new())
            };
            let decorations_duration = decoration_stage_started_at.elapsed();
            (
                line_decorations,
                search_mapping_duration,
                action_link_duration,
                decorations_duration,
                search_matches.len(),
            )
        };
        let element_stage_started_at = Instant::now();
        let (cell_w, cell_h) = self.terminal_cell_size();
        let terminal_font = self.gpui_terminal_font();
        let terminal_font_family = terminal_font.family.clone();
        let terminal_font_fallbacks = terminal_font.fallbacks.clone();
        let terminal_gpui_font = terminal_font.font();
        let gutter = if session_id.is_empty() && gutter_enabled {
            let line_number_digits =
                self.terminal_line_number_digits_for_session(None, snapshot.as_ref());
            let gutter_metrics = terminal_gutter_metrics(
                cell_w,
                show_timestamps,
                timestamp_width_chars,
                show_line_numbers,
                line_number_digits,
            );
            let ts_w = gutter_metrics.timestamp_width;
            let ln_w = gutter_metrics.line_number_width;
            let mut gutter = div()
                .flex()
                .flex_col()
                .flex_none()
                .mr(px(10.))
                .border_r_1()
                .border_color(rgb(palette.border));
            let gutter_row_count = terminal_gutter_row_count(snapshot.as_ref());
            for line_index in 0..line_count.min(gutter_row_count) {
                let snapshot_row = snapshot.row(line_index);
                let labels = terminal_gutter_labels(
                    snapshot_row,
                    abs_start + line_index + 1,
                    show_timestamps,
                    show_line_numbers,
                    line_number_digits,
                    &timestamp_formatter,
                );
                gutter = gutter.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .h(px(cell_h))
                        .line_height(px(cell_h))
                        .gap(px(gutter_metrics.gap_width))
                        .flex_none()
                        .pr(px(8.))
                        .text_color(rgb(palette.text_dimmed))
                        .font(terminal_gpui_font.clone())
                        .font_weight(FontWeight(
                            self.settings.summary().terminal_font_weight as f32,
                        ))
                        .text_size(px(self.settings.summary().terminal_font_size as f32))
                        .when(show_timestamps, |this| {
                            this.child(div().w(px(ts_w)).flex_none().child(labels.timestamp))
                        })
                        .when(show_line_numbers, |this| {
                            this.child(div().w(px(ln_w)).flex_none().child(labels.line_number))
                        }),
                );
            }
            Some(gutter)
        } else {
            None
        };
        // Grid paint is owned by TerminalSurface so frame notify does not rebuild
        // the full shell. Push current paint inputs then embed the entity.
        let surface_entity = if session_id.is_empty() {
            None
        } else {
            // Do not rebuild surface paint state on every shell paint — frames and
            // local interactions already sync the entity. Only cold-start once.
            let surface = self.ensure_terminal_surface(&session_id, cx);
            let needs_seed = !surface.read(cx).has_snapshot();
            if needs_seed {
                self.sync_terminal_surface_paint(&session_id, cx);
            }
            Some(surface)
        };
        let output = if let Some(surface) = surface_entity {
            div()
                .size_full()
                .flex()
                .flex_row()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .child(
                    div()
                        .size_full()
                        .flex_1()
                        .min_h_0()
                        .min_w_0()
                        .child(surface),
                )
        } else if let Some(gutter) = gutter {
            // Fallback empty session: keep a local element path.
            let mut grid = NyaTerminalElement::new(
                snapshot,
                keyword_rules,
                line_decorations,
                show_cursor,
                cursor_style,
                cell_w,
                cell_h,
                palette,
                terminal_font_family.clone(),
                self.settings.summary().terminal_font_size as f32,
                self.settings.summary().terminal_font_weight as f32,
                self.settings.summary().terminal_font_weight_bold as f32,
            );
            grid = grid
                .with_bold_default_foreground(self.settings.summary().bold_default_foreground)
                .with_minimum_contrast_ratio(self.settings.terminal_minimum_contrast_ratio())
                .with_selection(terminal_selection.map(TerminalGridSelection::from_selection))
                .with_font_fallbacks(terminal_font_fallbacks.clone());
            if let Some(cache) = layout_cache {
                grid = grid.with_layout_cache(cache);
            }
            div()
                .flex()
                .flex_row()
                .child(gutter)
                .child(div().flex_1().min_h_0().child(grid.with_fill_height(true)))
        } else {
            let mut grid = NyaTerminalElement::new(
                snapshot,
                keyword_rules,
                line_decorations,
                show_cursor,
                cursor_style,
                cell_w,
                cell_h,
                palette,
                terminal_font_family.clone(),
                self.settings.summary().terminal_font_size as f32,
                self.settings.summary().terminal_font_weight as f32,
                self.settings.summary().terminal_font_weight_bold as f32,
            );
            grid = grid
                .with_bold_default_foreground(self.settings.summary().bold_default_foreground)
                .with_minimum_contrast_ratio(self.settings.terminal_minimum_contrast_ratio())
                .with_selection(terminal_selection.map(TerminalGridSelection::from_selection))
                .with_font_fallbacks(terminal_font_fallbacks.clone());
            if let Some(cache) = layout_cache {
                grid = grid.with_layout_cache(cache);
            }
            div()
                .flex()
                .flex_row()
                .child(div().flex_1().min_h_0().child(grid.with_fill_height(true)))
        };
        let active_sync_group = self.sync_input.active_group_for_session(&session_id);
        let show_sync_action_overlay = active_sync_group.is_some() && !session_id.is_empty();
        let sync_is_paused = self
            .sync_input
            .session_is_paused_in_active_group(&session_id);
        let sync_group_color = active_sync_group
            .map(|group| group.color)
            .unwrap_or(palette.link);
        let sync_status_label = if sync_is_paused {
            t!("syncGroup.paused")
        } else {
            t!("syncGroup.broadcastActive")
        };
        let output_session_id = session_id.clone();
        let performance_overlay = self
            .terminal
            .view
            .views
            .get(&session_id)
            .and_then(|view| view.performance_overlay);
        let performance_overlay_copy = performance_overlay.map(|overlay| match overlay {
            TerminalPerformanceOverlay::Overloaded => (
                t!("terminal.largeOutputProtectionActive").to_string(),
                t!("terminal.largeOutputProtectionActiveDetail").to_string(),
            ),
            TerminalPerformanceOverlay::Recovered => (
                t!("terminal.largeOutputProtectionRecovered").to_string(),
                t!("terminal.largeOutputProtectionRecoveredDetail").to_string(),
            ),
        });
        let (render_cache_hits, render_cache_misses) = self
            .terminal
            .view
            .views
            .get(&session_id)
            .map(|view| view.render_cache.decoration_stats())
            .unwrap_or((0, 0));
        let (layout_cache_hits, layout_cache_misses, layout_shape_calls, layout_shape_duration_ms) =
            self.terminal
                .view
                .views
                .get(&session_id)
                .and_then(|view| {
                    view.render_cache.layout_cache.lock().ok().map(|cache| {
                        (
                            cache.hits,
                            cache.misses,
                            cache.shape_calls,
                            cache.shape_duration_us / 1_000,
                        )
                    })
                })
                .unwrap_or((0, 0, 0, 0));
        let file_drop_hover = self
            .shell
            .terminal_file_drop_hover_matches(session_id.as_str());
        let drop_session_kind = self
            .session
            .metadata(&session_id)
            .map(|metadata| terminal_canvas_session_kind_label(&metadata.launch_config))
            .unwrap_or("Local");
        let (drop_title, drop_hint) = nyaterm_core::terminal_drop_overlay_copy(drop_session_kind);
        let context_menu_enabled = !session_id.is_empty()
            && self
                .settings
                .summary()
                .interaction_terminal_right_click_action
                == nyaterm_core::TerminalRightClickAction::Menu
            && (!terminal_mouse_reporting
                || self.settings.summary().interaction_mouse_events_require_alt);
        let context_menu_app = cx.entity().downgrade();
        let context_menu_session_id = session_id.clone();

        // Menus and terminal chrome inherit the UI typography. Terminal text
        // surfaces set their own font so it cannot leak into popup labels.
        let canvas = div()
            .flex_1()
            .h_full()
            .min_h_0()
            .bg(self.shell_terminal_surface_color(palette.terminal_bg))
            .text_color(rgb(palette.terminal_fg))
            .child(
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .relative()
                    .key_context(TERMINAL_KEY_CONTEXT)
                    .track_focus(&self.terminal.input.focus)
                    .on_action(cx.listener(|this, _: &TerminalTab, window, cx| {
                        let event = this.terminal_tab_key_event(false);
                        this.handle_terminal_surface_key_down(&event, window, cx);
                    }))
                    .on_action(cx.listener(|this, _: &TerminalShiftTab, window, cx| {
                        let event = this.terminal_tab_key_event(true);
                        this.handle_terminal_surface_key_down(&event, window, cx);
                    }))
                    .on_action(cx.listener(|this, _: &TerminalControlC, window, cx| {
                        let event = NyaTermApp::terminal_control_c_key_event();
                        this.handle_terminal_surface_key_down(&event, window, cx);
                    }))
                    .on_action(cx.listener(|this, _: &NyaCopy, window, cx| {
                        if this.terminal.input.focus.is_focused(window) {
                            this.copy_terminal_selection_or_visible(cx);
                            cx.stop_propagation();
                        }
                    }))
                    .on_action(cx.listener(|this, _: &NyaPaste, window, cx| {
                        if this.terminal.input.focus.is_focused(window) {
                            this.paste_from_clipboard(window, cx);
                            cx.stop_propagation();
                        }
                    }))
                    .on_action(cx.listener(|this, _: &NyaSelectAll, window, cx| {
                        if this.terminal.input.focus.is_focused(window) {
                            this.select_all_terminal(cx);
                            cx.stop_propagation();
                        }
                    }))
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                        this.handle_terminal_surface_key_down(event, window, cx);
                    }))
                    .on_key_up(cx.listener(|this, event: &KeyUpEvent, window, cx| {
                        if this.terminal.input.focus.is_focused(window)
                            && this.send_terminal_key_release_event(event, cx)
                        {
                            cx.stop_propagation();
                            this.mark_user_activity();
                        }
                    }))
                    // Empty-workspace bootstrap actions stay available when no session is selected.
                    .when(session_id.is_empty(), |this| {
                        this.child(
                            div()
                                .h(px(36.))
                                .flex()
                                .items_center()
                                .gap_2()
                                .px_3()
                                .border_b_1()
                                .border_color(rgb(palette.border))
                                .bg(self.shell_surface_color(palette.input))
                                .child(small_button(
                                    palette,
                                    "terminal-start-local",
                                    "Start Local",
                                    cx.listener(|this, _, window, cx| {
                                        this.start_local_session(window, cx);
                                    }),
                                ))
                                .child(small_button(
                                    palette,
                                    "terminal-actions",
                                    "Actions",
                                    cx.listener(|this, _, window, cx| {
                                        this.open_terminal_actions(window, cx);
                                    }),
                                ))
                                .child(div().flex_1())
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(palette.text_muted))
                                        .child(self.shell.status().to_string()),
                                ),
                        )
                    })
                    .child(
                        NyaContextMenu::new_dynamic(
                            div()
                                .id(SharedString::from(format!(
                                    "terminal-output-{output_session_id}"
                                )))
                                .relative()
                                .flex_1()
                                .min_h_0()
                                .when(
                                    !is_disconnected
                                        && (!terminal_mouse_reporting
                                            || self
                                                .settings
                                                .summary()
                                                .interaction_mouse_events_require_alt),
                                    |this| this.cursor_text(),
                                )
                                .when(
                                    is_active && self.terminal.menus.action_link_tooltip.is_some(),
                                    |this| this.cursor_pointer(),
                                )
                                .pl(px(self.terminal_content_insets().left))
                                .pr(px(self.terminal_content_insets().right))
                                .pt(px(self.terminal_content_insets().top))
                                .pb(px(self.terminal_content_insets().bottom))
                                .overflow_hidden()
                                .can_drop(|drag, _, _| drag.is::<gpui::ExternalPaths>())
                                .on_drag_move({
                                    let session_id = output_session_id.clone();
                                    cx.listener(
                                        move |this,
                                              event: &gpui::DragMoveEvent<gpui::ExternalPaths>,
                                              _,
                                              cx| {
                                            if event.bounds.contains(&event.event.position) {
                                                this.set_terminal_file_drop_hover(
                                                    Some(session_id.clone()),
                                                    cx,
                                                );
                                            } else {
                                                this.clear_terminal_file_drop_hover_for_session(
                                                    &session_id,
                                                    cx,
                                                );
                                            }
                                        },
                                    )
                                })
                                .on_drop({
                                    let session_id = output_session_id.clone();
                                    cx.listener(
                                        move |this, paths: &gpui::ExternalPaths, window, cx| {
                                            this.handle_terminal_external_file_drop(
                                                session_id.clone(),
                                                paths.paths().to_vec(),
                                                window,
                                                cx,
                                            );
                                        },
                                    )
                                })
                                .on_mouse_down(MouseButton::Left, {
                                    let session_id = output_session_id.clone();
                                    cx.listener(
                                        move |this, event: &gpui::MouseDownEvent, window, cx| {
                                            this.activate_workspace_pane(session_id.clone(), cx);
                                            window.focus(&this.terminal.input.focus, cx);
                                            this.close_action_link_menu(cx);
                                            this.terminal.selection.block_gesture = None;
                                            this.terminal.selection.suppress_action_link_click = false;
                                            let mods = event.modifiers;
                                            let block_candidate = mods.alt
                                                && this.settings.summary().terminal_wrap_mode
                                                    != nyaterm_core::models::settings::TerminalWrapMode::None;
                                            let skip_selection = this
                                                .settings
                                                .summary()
                                                .terminal_action_links_enabled
                                                && (mods.alt || mods.control || mods.platform)
                                                && !block_candidate;
                                            if !skip_selection {
                                                this.start_terminal_selection_for_session(
                                                    Some(session_id.as_str()),
                                                    event,
                                                    cx,
                                                );
                                            }
                                            cx.stop_propagation();
                                        },
                                    )
                                })
                                .on_mouse_down(MouseButton::Right, {
                                    let session_id = output_session_id.clone();
                                    cx.listener(
                                        move |this, event: &gpui::MouseDownEvent, window, cx| {
                                            this.activate_workspace_pane(session_id.clone(), cx);
                                            window.focus(&this.terminal.input.focus, cx);
                                            if let Some(cell) = this
                                                .point_to_terminal_cell_for_session(
                                                    Some(session_id.as_str()),
                                                    event.position,
                                                    cx,
                                                )
                                                && this.maybe_send_mouse_report_for_session(
                                                    TerminalMouseReportRequest {
                                                        session_id: &session_id,
                                                        button: 2,
                                                        col: cell.col as u16,
                                                        row: cell.row as u16,
                                                        press: true,
                                                        motion: false,
                                                        modifiers: event.modifiers,
                                                    },
                                                    cx,
                                                )
                                            {
                                                cx.stop_propagation();
                                                return;
                                            }
                                            match this
                                                .settings
                                                .summary()
                                                .interaction_terminal_right_click_action
                                            {
                                                nyaterm_core::TerminalRightClickAction::Paste => {
                                                    this.paste_from_clipboard(window, cx);
                                                    this.clear_terminal_selection(cx);
                                                }
                                                nyaterm_core::TerminalRightClickAction::Menu => {
                                                    this.prepare_terminal_context_menu(cx);
                                                }
                                                nyaterm_core::TerminalRightClickAction::None => {}
                                            }
                                            cx.stop_propagation();
                                        },
                                    )
                                })
                                .on_mouse_down(MouseButton::Middle, {
                                    let session_id = output_session_id.clone();
                                    cx.listener(
                                        move |this, event: &gpui::MouseDownEvent, window, cx| {
                                            // xterm/Linux middle-click paste convention.
                                            this.activate_workspace_pane(session_id.clone(), cx);
                                            window.focus(&this.terminal.input.focus, cx);
                                            this.close_action_link_menu(cx);
                                            if let Some(cell) = this
                                                .point_to_terminal_cell_for_session(
                                                    Some(session_id.as_str()),
                                                    event.position,
                                                    cx,
                                                )
                                                && this.maybe_send_mouse_report_for_session(
                                                    TerminalMouseReportRequest {
                                                        session_id: &session_id,
                                                        button: 1,
                                                        col: cell.col as u16,
                                                        row: cell.row as u16,
                                                        press: true,
                                                        motion: false,
                                                        modifiers: event.modifiers,
                                                    },
                                                    cx,
                                                )
                                            {
                                                cx.stop_propagation();
                                                return;
                                            }
                                            this.paste_from_clipboard(window, cx);
                                            cx.stop_propagation();
                                        },
                                    )
                                })
                                .on_click({
                                    let session_id = output_session_id.clone();
                                    cx.listener(move |this, event: &ClickEvent, window, cx| {
                                        this.activate_workspace_pane(session_id.clone(), cx);
                                        if event.is_right_click() {
                                            // Right-click is handled on mouse_down for Tauri-like context menu.
                                            cx.stop_propagation();
                                            return;
                                        }
                                        window.focus(&this.terminal.input.focus, cx);
                                        let modifiers = event.modifiers();
                                        if let ClickEvent::Mouse(mouse) = event
                                            && !this.terminal.selection.confirm_block_drag(mouse.up.position)
                                        {
                                            this.clear_terminal_selection(cx);
                                        }
                                        if this.terminal.selection.suppress_action_link_click {
                                            // The root MouseUp handler must still finish the
                                            // selection or release the remote mouse capture.
                                            return;
                                        }
                                        if this.settings.summary().terminal_action_links_enabled {
                                            if modifiers.alt {
                                                if this
                                                    .try_open_action_link_menu_at_click(event, cx)
                                                {
                                                    cx.stop_propagation();
                                                    return;
                                                }
                                            } else if (modifiers.control || modifiers.platform)
                                                && this.try_activate_action_link_at_click(
                                                    event, window, cx,
                                                )
                                            {
                                                cx.stop_propagation();
                                                return;
                                            }
                                        }
                                        if this.terminal.selection.selection.is_none()
                                            && this.shell.status() != "terminal focused"
                                        {
                                            this.shell.set_status("terminal focused".to_string());
                                            cx.notify();
                                        }
                                    })
                                })
                                .child(
                                    div()
                                        .size_full()
                                        .flex()
                                        .flex_row()
                                        .min_h_0()
                                        .relative()
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .min_h_0()
                                                .relative()
                                                .child(output)
                                                .when(
                                                    matches!(
                                                        self.session.metadata(&output_session_id).map(|metadata| &metadata.launch_config),
                                                        Some(SessionLaunchConfig::AdbShell(_))
                                                    ),
                                                    |this| this.child(self.android_screen_button(output_session_id.clone(), cx)),
                                                )
                                                .when(output_session_id.is_empty(), |this| {
                                                    this.child(terminal_bounds_tracker(
                                                        cx.entity(),
                                                        None,
                                                        is_active,
                                                    ))
                                                }),
                                        )
                                        // Scrollbar is painted by TerminalSurface for live sessions.
                                        .when(session_id.is_empty(), |this| {
                                            this.child(self.terminal_scrollbar_element(
                                                &session_id,
                                                is_active,
                                                scroll_offset,
                                                cx,
                                            ))
                                        }),
                                )
                                .when(file_drop_hover && !session_id.is_empty(), |this| {
                                    this.child(
                                        div()
                                            .absolute()
                                            .inset_2()
                                            .rounded_lg()
                                            .border_2()
                                            .border_color(rgb(palette.link))
                                            .bg(rgba(0x3b82f624))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .child(
                                                div()
                                                    .max_w(px(320.))
                                                    .rounded_lg()
                                                    .border_1()
                                                    .border_color(rgb(palette.link))
                                                    .bg(rgb(palette.surface))
                                                    .px_6()
                                                    .py_4()
                                                    .shadow_lg()
                                                    .flex()
                                                    .flex_col()
                                                    .items_center()
                                                    .gap_1()
                                                    .child(
                                                        div()
                                                            .text_sm()
                                                            .font_weight(FontWeight(700.))
                                                            .text_color(rgb(palette.text))
                                                            .child(drop_title),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_xs()
                                                            .text_color(rgb(palette.text_muted))
                                                            .child(drop_hint),
                                                    ),
                                            ),
                                    )
                                })
                                .when(show_sync_action_overlay, |this| {
                                    let pause_session_id = output_session_id.clone();
                                    let leave_session_id = output_session_id.clone();
                                    let close_session_id = output_session_id.clone();
                                    this.child(
                                    div()
                                        .id(SharedString::from(format!(
                                            "terminal-sync-overlay-{output_session_id}"
                                        )))
                                        .absolute()
                                        .right(px(8.))
                                        .top(px(4.))
                                        .flex()
                                        .items_center()
                                        .gap_1()
                                        .rounded_md()
                                        .px_1()
                                        .py(px(2.))
                                        .border_1()
                                        .border_color(rgba((sync_group_color << 8) | 0x4d))
                                        .bg(rgba((palette.surface << 8) | 0xeb))
                                        .shadow_sm()
                                        .child(
                                            div()
                                                .text_xs()
                                                .font_weight(FontWeight(700.))
                                                .text_color(rgb(sync_group_color))
                                                .mr(px(4.))
                                                .child(sync_status_label),
                                        )
                                        .child(
                                            div()
                                                .id(SharedString::from(format!(
                                                    "terminal-sync-pause-{output_session_id}"
                                                )))
                                                .rounded_sm()
                                                .px_1()
                                                .py(px(2.))
                                                .cursor_pointer()
                                                .hover(|style| style.bg(rgb(palette.hover)))
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.toggle_session_paused_in_active_sync_group(
                                                        pause_session_id.clone(),
                                                        cx,
                                                    );
                                                }))
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(rgb(sync_group_color))
                                                        .child(if sync_is_paused {
                                                            t!("syncGroup.resumeSync")
                                                        } else {
                                                            t!("syncGroup.pauseSync")
                                                        }),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .id(SharedString::from(format!(
                                                    "terminal-sync-leave-{output_session_id}"
                                                )))
                                                .rounded_sm()
                                                .px_1()
                                                .py(px(2.))
                                                .cursor_pointer()
                                                .hover(|style| style.bg(rgb(palette.hover)))
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.leave_active_sync_group(
                                                        leave_session_id.clone(),
                                                        cx,
                                                    );
                                                }))
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(rgb(sync_group_color))
                                                        .child(t!("syncGroup.leaveGroup")),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .id(SharedString::from(format!(
                                                    "terminal-sync-close-{output_session_id}"
                                                )))
                                                .rounded_sm()
                                                .px_1()
                                                .py(px(2.))
                                                .cursor_pointer()
                                                .hover(|style| {
                                                    style.bg(rgba((palette.danger << 8) | 0x1a))
                                                })
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.close_active_sync_group_for_session(
                                                        close_session_id.clone(),
                                                        cx,
                                                    );
                                                }))
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(rgb(palette.danger))
                                                        .child(t!("syncGroup.closeGroup")),
                                                ),
                                        ),
                                )
                                })
                                .when_some(performance_overlay_copy, |this, (title, detail)| {
                                    this.child(
                                        div()
                                            .id(SharedString::from(format!(
                                                "terminal-perf-overlay-{output_session_id}"
                                            )))
                                            .absolute()
                                            .left(px(12.))
                                            .right(px(12.))
                                            .top(px(12.))
                                            .flex()
                                            .justify_end()
                                            .child(
                                                div()
                                                    .max_w(px(360.))
                                                    .rounded_md()
                                                    .border_1()
                                                    .border_color(rgb(palette.border))
                                                    .bg(rgb(palette.surface))
                                                    .px_3()
                                                    .py_2()
                                                    .shadow_lg()
                                                    .child(
                                                        div()
                                                            .text_xs()
                                                            .font_weight(FontWeight(700.))
                                                            .text_color(rgb(palette.text))
                                                            .child(title),
                                                    )
                                                    .child(
                                                        div()
                                                            .mt_1()
                                                            .text_xs()
                                                            .text_color(rgb(palette.text_dimmed))
                                                            .child(detail),
                                                    ),
                                            ),
                                    )
                                }),
                            move |_, cx| {
                                context_menu_app
                                    .update(cx, |this, cx| {
                                        this.terminal_context_menu_items_for_session(
                                            context_menu_session_id.clone(),
                                            cx,
                                        )
                                    })
                                    .unwrap_or_default()
                            },
                        )
                        .min_width(px(200.))
                        .enabled(context_menu_enabled),
                    )
                    .when(is_active && self.terminal.search.open, |this| {
                        this.child(self.terminal_search_bar(cx))
                    }),
            );
        let element_construction_duration = element_stage_started_at.elapsed();
        let total_duration = render_started_at.elapsed();
        let render_slow = viewport_snapshot_duration >= TERMINAL_RENDER_SLOW_STAGE
            || search_mapping_duration >= TERMINAL_RENDER_SLOW_STAGE
            || action_link_duration >= TERMINAL_RENDER_SLOW_STAGE
            || decorations_duration >= TERMINAL_RENDER_SLOW_STAGE
            || element_construction_duration >= TERMINAL_RENDER_SLOW_STAGE
            || total_duration >= TERMINAL_RENDER_SLOW_TOTAL;
        if render_slow
            && !render_degraded
            && (search_mapping_duration >= TERMINAL_RENDER_SLOW_STAGE
                || action_link_duration >= TERMINAL_RENDER_SLOW_STAGE
                || decorations_duration >= TERMINAL_RENDER_SLOW_STAGE)
            && let Some(view) = self.terminal.view.views.get_mut(&session_id)
        {
            view.enter_render_degraded_mode();
        }
        if render_slow && self.should_log_slow_diagnostic("terminal_render", Instant::now()) {
            tracing::warn!(
                diagnostic = "terminal_render",
                session_id = %session_id,
                rows = snapshot_rows,
                cols = snapshot_cols,
                line_count,
                is_active,
                render_degraded,
                render_profile = render_profile.label(),
                render_output_pressure,
                expensive_interactions_enabled,
                output_burst_bytes,
                ?performance_mode,
                action_links_enabled = self.settings.summary().terminal_action_links_enabled,
                search_open = self.terminal.search.open,
                search_matches = search_matches_len,
                render_cache_hits,
                render_cache_misses,
                layout_cache_hits,
                layout_cache_misses,
                layout_shape_calls,
                layout_shape_duration_ms,
                viewport_snapshot_ms = viewport_snapshot_duration.as_millis(),
                search_mapping_ms = search_mapping_duration.as_millis(),
                action_links_ms = action_link_duration.as_millis(),
                decorations_ms = decorations_duration.as_millis(),
                element_construction_ms = element_construction_duration.as_millis(),
                total_ms = total_duration.as_millis(),
                "slow terminal render"
            );
        }
        canvas
    }
}
fn terminal_render_pressure_active(
    runtime_output_pressure: bool,
    output_burst_bytes: usize,
    performance_mode: TerminalPerformanceMode,
) -> bool {
    runtime_output_pressure
        || output_burst_bytes > 0
        || performance_mode == TerminalPerformanceMode::Overloaded
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TerminalRenderProfile {
    Full,
    PlainViewport,
}

impl TerminalRenderProfile {
    fn enhanced_decorations_enabled(self) -> bool {
        matches!(self, Self::Full)
    }

    fn label(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::PlainViewport => "plain_viewport",
        }
    }
}

fn terminal_render_profile(render_degraded: bool) -> TerminalRenderProfile {
    if render_degraded {
        TerminalRenderProfile::PlainViewport
    } else {
        TerminalRenderProfile::Full
    }
}

fn terminal_canvas_session_kind_label(config: &SessionLaunchConfig) -> &'static str {
    match config {
        SessionLaunchConfig::Local(_) => "Local",
        SessionLaunchConfig::Ssh(_) => "SSH",
        SessionLaunchConfig::Telnet(config) if config.raw_tcp => "Raw TCP",
        SessionLaunchConfig::Telnet(_) => "Telnet",
        SessionLaunchConfig::Serial(_) => "Serial",
        SessionLaunchConfig::Rdp(_) => "RDP",
        SessionLaunchConfig::Vnc(_) => "VNC",
        SessionLaunchConfig::AdbShell(_)
        | SessionLaunchConfig::AdbFiles(_)
        | SessionLaunchConfig::AdbScreen(_) => "Android",
    }
}

const TERMINAL_RENDER_SLOW_STAGE: Duration = Duration::from_millis(12);
const TERMINAL_RENDER_SLOW_TOTAL: Duration = Duration::from_millis(25);

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::models::TerminalPerformanceMode;

    use super::{TerminalRenderProfile, terminal_render_pressure_active, terminal_render_profile};

    #[test]
    fn terminal_render_pressure_tracks_runtime_bursts_and_overload() {
        assert!(terminal_render_pressure_active(
            true,
            0,
            TerminalPerformanceMode::Normal
        ));
        assert!(terminal_render_pressure_active(
            false,
            1,
            TerminalPerformanceMode::Normal
        ));
        assert!(terminal_render_pressure_active(
            false,
            0,
            TerminalPerformanceMode::Overloaded
        ));
        assert!(!terminal_render_pressure_active(
            false,
            0,
            TerminalPerformanceMode::Normal
        ));
    }

    #[test]
    fn terminal_render_profile_uses_plain_viewport_while_degraded() {
        assert_eq!(terminal_render_profile(false), TerminalRenderProfile::Full);
        assert_eq!(
            terminal_render_profile(true),
            TerminalRenderProfile::PlainViewport
        );
        assert!(terminal_render_profile(false).enhanced_decorations_enabled());
        assert!(!terminal_render_profile(true).enhanced_decorations_enabled());
        assert_eq!(terminal_render_profile(true).label(), "plain_viewport");
    }

    /// Ctrl+Insert must be consumed by the terminal copy action. The canvas
    /// `on_key_down` handler is the PTY fallback, so reaching it writes
    /// `\x1b[2;5~` to the session.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn ctrl_insert_is_consumed_before_the_terminal_key_fallback() {
        assert_terminal_shortcut_dispatch(&HashMap::new(), "ctrl-insert", TestShortcutEffect::Copy);
    }

    #[test]
    fn local_copy_shortcuts_are_consumed_before_the_terminal_key_fallback() {
        let chord = if cfg!(target_os = "macos") {
            "cmd-shift-c"
        } else {
            "ctrl-shift-c"
        };
        assert_terminal_shortcut_dispatch(&HashMap::new(), chord, TestShortcutEffect::Copy);
    }

    #[test]
    fn local_paste_shortcuts_send_only_clipboard_bytes() {
        let chord = if cfg!(target_os = "macos") {
            "cmd-shift-v"
        } else {
            "ctrl-shift-v"
        };
        for chord in [chord, "shift-insert"] {
            assert_terminal_shortcut_dispatch(&HashMap::new(), chord, TestShortcutEffect::Paste);
        }
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn ctrl_insert_copy_override_and_file_explorer_binding_keep_local_copy() {
        for rename in [false, true] {
            let mut overrides = HashMap::from([("terminal.copy".into(), "ctrl+alt+c".into())]);
            if rename {
                overrides.insert("fileExplorer.rename".into(), "ctrl+insert".into());
            }
            assert_terminal_shortcut_dispatch(&overrides, "ctrl-insert", TestShortcutEffect::Copy);
            assert_terminal_shortcut_dispatch(&overrides, "ctrl-alt-c", TestShortcutEffect::Copy);
        }
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn ctrl_insert_assigned_to_another_terminal_action_does_not_copy_or_leak() {
        for (id, effect) in [
            ("terminal.paste", TestShortcutEffect::Paste),
            ("terminal.selectAll", TestShortcutEffect::SelectAll),
        ] {
            let overrides = HashMap::from([
                ("terminal.copy".into(), "ctrl+shift+c".into()),
                (id.into(), "ctrl+insert".into()),
            ]);
            assert_terminal_shortcut_dispatch(&overrides, "ctrl-insert", effect);
        }
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn ctrl_insert_in_nested_input_preserves_input_shortcuts_without_terminal_bytes() {
        let overrides = HashMap::from([("terminal.copy".into(), "ctrl+alt+c".into())]);
        assert_terminal_shortcut_dispatch(&overrides, "ctrl-insert", TestShortcutEffect::InputCopy);
    }

    #[test]
    fn repeated_and_held_command_navigation_does_not_write_terminal_bytes_or_change_selection() {
        let chord = if cfg!(target_os = "macos") {
            "cmd-shift-left"
        } else {
            "ctrl-shift-left"
        };
        assert_terminal_shortcut_dispatch(&HashMap::new(), chord, TestShortcutEffect::Navigation);
    }

    #[derive(Clone, Copy)]
    enum TestShortcutEffect {
        Navigation,
        Copy,
        Paste,
        #[cfg(not(target_os = "macos"))]
        SelectAll,
        #[cfg(not(target_os = "macos"))]
        InputCopy,
    }

    fn assert_terminal_shortcut_dispatch(
        overrides: &HashMap<String, String>,
        chord: &'static str,
        effect: TestShortcutEffect,
    ) {
        use std::io::Read;
        use std::net::TcpListener;
        use std::time::Duration;

        use gpui::{
            AppContext as _, Bounds, Entity, Point, Render, Size, TestAppContext, Window, px,
        };
        use nyaterm_transport::TelnetSessionConfig;
        use nyaterm_ui::{NyaInput, NyaInputState};

        use crate::features::NyaTermApp;
        use crate::features::test_support::app_with_visible_local_session;
        use crate::shortcuts::{WORKSPACE_KEY_CONTEXT, rebuild_keymap};
        use crate::test_support::{TestAppCleanup, TestConfigDir};

        struct TerminalKeyDispatchView {
            app: Entity<NyaTermApp>,
            input: Option<Entity<NyaInputState>>,
        }

        impl Render for TerminalKeyDispatchView {
            fn render(
                &mut self,
                _window: &mut Window,
                cx: &mut gpui::Context<Self>,
            ) -> impl gpui::IntoElement {
                use gpui::prelude::*;

                let app = self.app.clone();
                let input = self.input.clone();
                let terminal = app.update(cx, |app, cx| {
                    let focus = app.terminal.input.focus.clone();
                    gpui::div()
                        .id("terminal-key-surface")
                        .size_full()
                        .key_context(super::TERMINAL_KEY_CONTEXT)
                        .track_focus(&focus)
                        .on_key_down(cx.listener(
                            |app: &mut NyaTermApp, event: &gpui::KeyDownEvent, window, cx| {
                                app.handle_terminal_surface_key_down(event, window, cx);
                            },
                        ))
                        .when_some(input, |terminal, input| {
                            terminal.child(
                                gpui::div()
                                    .w(px(300.0))
                                    .h(px(32.0))
                                    .child(NyaInput::new(&input)),
                            )
                        })
                });
                app.update(cx, |app, cx| {
                    app.with_shortcut_action_handlers(
                        gpui::div()
                            .id("terminal-key-root")
                            .size_full()
                            .key_context(WORKSPACE_KEY_CONTEXT)
                            .child(terminal),
                        cx,
                    )
                })
            }
        }

        let dir = TestConfigDir::new("nyaterm-terminal-ctrl-insert-dispatch");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        cx.update(nyaterm_ui::init_components);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "s1");
        let manager = cx.update_entity(&app, |app, _| app.session.manager_handle());
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let info = manager
            .create_telnet_session(TelnetSessionConfig {
                host: "127.0.0.1".into(),
                port: listener.local_addr().unwrap().port(),
                raw_tcp: true,
                ..TelnetSessionConfig::default()
            })
            .unwrap();
        let (mut peer, _) = listener.accept().unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        cx.update_entity(&app, |app, cx| {
            let metadata = app.session.metadata("s1").unwrap().clone();
            app.session.register_session_metadata(&info.id, metadata);
            app.session.select_active_session(&info.id);
            app.terminal.layout.surface_bounds = Some(Bounds {
                origin: Point::default(),
                size: Size {
                    width: px(800.0),
                    height: px(600.0),
                },
            });
            let text = if matches!(effect, TestShortcutEffect::Navigation) {
                "\x1b]133;A\x07$ \x1b]133;B\x07echo same\r\n\x1b]133;C\x07out\r\n\x1b]133;D;0\x07"
                    .repeat(5)
            } else {
                "root@host:~# ls\n".into()
            };
            app.terminal
                .seed_session_view(info.id.clone(), text.clone(), "UTF-8");
            if matches!(effect, TestShortcutEffect::Navigation) {
                app.terminal
                    .ensure_frame_session(info.id.clone(), "UTF-8".into(), 10000);
                app.terminal
                    .view
                    .frame_pipeline
                    .append_local_text(info.id.clone(), text);
                app.terminal.selection.session_id = Some(info.id.clone());
                app.terminal.selection.selection =
                    Some(crate::models::TerminalSelection::from_range(
                        crate::models::TerminalBufferCellPos::new(0, 0),
                        crate::models::TerminalBufferCellPos::new(0, 3),
                    ));
                app.terminal.view.frame_pipeline.flush_for_test();
                app.drain_terminal_frame_events(cx);
            }
        });
        cx.update(|cx| rebuild_keymap(overrides, cx));
        #[cfg(not(target_os = "macos"))]
        let input = matches!(effect, TestShortcutEffect::InputCopy)
            .then(|| cx.new(|cx| NyaInputState::new(cx, "search input text")));
        #[cfg(target_os = "macos")]
        let input: Option<Entity<NyaInputState>> = None;
        let (_, vcx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|_| TerminalKeyDispatchView {
                app: app.clone(),
                input: input.clone(),
            });
            nyaterm_ui::nya_root(view, window, cx)
        });
        vcx.update(|window, cx| {
            let main = window.window_handle().downcast().unwrap();
            app.update(cx, |app, cx| {
                app.shell.remember_main_window(main);
                app.ensure_shortcut_interceptor(cx);
            });
        });
        vcx.update(|window, cx| {
            let _ = window.draw(cx);
            let focus = app.read(cx).terminal.input.focus.clone();
            window.focus(&focus, cx);
            if let Some(input) = input.as_ref() {
                input.update(cx, |input, cx| {
                    input.select_all_with_cursor_at_end(window, cx)
                });
            }
            cx.write_to_clipboard(gpui::ClipboardItem::new_string("paste payload".into()));
        });
        vcx.run_until_parked();
        let selection_before = vcx.update(|_, cx| app.read(cx).terminal.selection.selection);
        vcx.simulate_keystrokes(chord);
        vcx.run_until_parked();

        #[cfg(not(target_os = "macos"))]
        if matches!(effect, TestShortcutEffect::InputCopy) {
            // NyaInput's current baseline uses Ctrl+C, not Ctrl+Insert. An
            // unbound Ctrl+Insert must leave both clipboard and session alone.
            assert_eq!(
                vcx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text()))
                    .as_deref(),
                Some("paste payload")
            );
            vcx.simulate_keystrokes("ctrl-c");
            vcx.run_until_parked();
        }

        let copied = vcx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text()));
        match effect {
            TestShortcutEffect::Navigation => {
                let target = |app: &mut NyaTermApp, cx: &mut gpui::Context<NyaTermApp>| {
                    app.terminal.view.frame_pipeline.flush_for_test();
                    app.drain_terminal_frame_events(cx);
                    app.terminal
                        .view
                        .views
                        .get(&info.id)
                        .unwrap()
                        .frame_snapshot
                        .as_ref()
                        .unwrap()
                        .command_navigation_target
                };
                assert_eq!(
                    vcx.update(|_, cx| app.read(cx).terminal.selection.selection),
                    selection_before
                );
                let mut previous = vcx.update(|_, cx| app.update(cx, target)).unwrap();
                for _ in 0..4 {
                    vcx.update(|window, cx| {
                        window.dispatch_event(
                            gpui::PlatformInput::KeyDown(gpui::KeyDownEvent {
                                keystroke: gpui::Keystroke::parse(chord).unwrap(),
                                is_held: true,
                                prefer_character_input: false,
                            }),
                            cx,
                        );
                    });
                    let next = vcx.update(|_, cx| app.update(cx, target)).unwrap();
                    assert!(next.logical_line < previous.logical_line);
                    previous = next;
                }
                assert_eq!(
                    vcx.update(|_, cx| app.read(cx).terminal.selection.selection),
                    selection_before
                );
                let next = if cfg!(target_os = "macos") {
                    "cmd-shift-right"
                } else {
                    "ctrl-shift-right"
                };
                for _ in 0..5 {
                    vcx.simulate_keystrokes(next);
                    vcx.update(|_, cx| app.update(cx, target));
                }
                assert!(vcx.update(|_, cx| app.update(cx, target)).is_none());
                assert_eq!(
                    vcx.update(|_, cx| app
                        .read(cx)
                        .terminal
                        .view
                        .views
                        .get(&info.id)
                        .unwrap()
                        .scroll_offset),
                    0
                );
            }
            TestShortcutEffect::Copy => assert!(
                copied.unwrap_or_default().contains("root@host"),
                "{chord} did not copy terminal text"
            ),
            TestShortcutEffect::Paste => assert_eq!(copied.as_deref(), Some("paste payload")),
            #[cfg(not(target_os = "macos"))]
            TestShortcutEffect::SelectAll => {
                assert_eq!(
                    copied.as_deref(),
                    Some("paste payload"),
                    "{chord} unexpectedly copied"
                );
                assert!(vcx.update(|_, cx| {
                    app.read(cx)
                        .terminal
                        .selection
                        .selection
                        .as_ref()
                        .is_some_and(|selection| selection.all_buffer)
                }));
            }
            #[cfg(not(target_os = "macos"))]
            TestShortcutEffect::InputCopy => {
                assert_eq!(copied.as_deref(), Some("search input text"))
            }
        }

        cx.update_entity(&app, |app, _| {
            app.write_session_protocol_response(&info.id, b"CLICK")
                .unwrap();
        });
        let expected = if matches!(effect, TestShortcutEffect::Paste) {
            b"paste payloadCLICK".as_slice()
        } else {
            b"CLICK".as_slice()
        };
        let mut received = vec![0u8; expected.len()];
        peer.read_exact(&mut received)
            .unwrap_or_else(|error| panic!("session received no sentinel bytes: {error}"));
        assert_eq!(received, expected, "{chord} wrote unexpected session bytes");
        manager.close(&info.id).unwrap();
    }
}
