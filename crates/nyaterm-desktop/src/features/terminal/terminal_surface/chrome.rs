use gpui::{
    Context, IntoElement, KeyDownEvent, MouseButton, SharedString, div, prelude::*, px, rgb, rgba,
};
use nyaterm_ui::{NyaButton, NyaButtonVariant, NyaIconButton, NyaInput};
use rust_i18n::t;

use crate::features::{NyaTermApp, text_inputs::TextInputSetup};

use super::{
    TERMINAL_SCROLLBAR_COLUMN_WIDTH, TERMINAL_SCROLLBAR_MIN_THUMB_HEIGHT,
    TERMINAL_SCROLLBAR_TRACK_PADDING_RIGHT, TERMINAL_SCROLLBAR_TRACK_PADDING_Y,
    TerminalScrollbarInput, terminal_overview_marker_buckets, terminal_overview_marker_canvas,
    terminal_scroll_offset_from_pointer, terminal_scrollbar_grab_offset_for_pointer,
    terminal_scrollbar_metrics, terminal_scrollbar_thumb_element,
    terminal_scrollbar_track_bounds_tracker, terminal_scrollbar_track_color, track_height,
};

impl NyaTermApp {
    pub(in crate::features) fn terminal_scrollbar_element(
        &self,
        session_id: &str,
        is_active: bool,
        scroll_offset: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let max = self
            .terminal
            .view
            .views
            .get(session_id)
            .map(|view| view.scrollback_len_for_ui())
            .unwrap_or_else(|| {
                if session_id.is_empty() {
                    self.terminal.view.screen.scrollback_len()
                } else {
                    0
                }
            });
        let viewport_rows = self
            .terminal
            .view
            .views
            .get(session_id)
            .map(|view| {
                // viewport height equals live screen rows
                view.viewport_rows_for_ui()
            })
            .unwrap_or_else(|| self.active_terminal_page_rows().max(1));
        let show = max > 0;
        let drag_active = self
            .terminal
            .view
            .scrollbar_drag
            .as_ref()
            .is_some_and(|drag| drag.session_id.as_deref().unwrap_or("") == session_id);
        let track_bounds = self.terminal_scrollbar_track_bounds_for_session(
            (!session_id.is_empty()).then_some(session_id),
        );
        let track_height = track_bounds.map(track_height).unwrap_or(1.0);
        let metrics = terminal_scrollbar_metrics(TerminalScrollbarInput {
            viewport_rows,
            scrollback_rows: max,
            scroll_offset,
            track_height,
            min_thumb_height: TERMINAL_SCROLLBAR_MIN_THUMB_HEIGHT,
        });
        let track_id = format!("terminal-scrollbar-track-{session_id}");
        let thumb_id = format!("terminal-scrollbar-thumb-{session_id}");
        let (overview_markers, overview_total_rows) =
            self.terminal_overview_markers_for_session(session_id);
        let overview_track_height_px = track_height.max(0.0).round() as usize;
        let overview_marker_buckets = terminal_overview_marker_buckets(
            &overview_markers,
            overview_total_rows,
            overview_track_height_px,
        )
        .into();

        div()
            .id(SharedString::from(format!(
                "terminal-scrollbar-{session_id}"
            )))
            .w(px(TERMINAL_SCROLLBAR_COLUMN_WIDTH))
            .flex_none()
            .h_full()
            .py(px(TERMINAL_SCROLLBAR_TRACK_PADDING_Y))
            .pr(px(TERMINAL_SCROLLBAR_TRACK_PADDING_RIGHT))
            .when(!show, |this| this.invisible())
            .child(
                div()
                    .id(SharedString::from(track_id))
                    .relative()
                    .size_full()
                    .border_l_1()
                    .border_color(rgb(palette.border))
                    .bg(rgba(terminal_scrollbar_track_color(palette)))
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, {
                        let session_id = session_id.to_string();
                        cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                            if !session_id.is_empty() {
                                this.activate_workspace_pane(session_id.clone(), cx);
                            }
                            let drag_session_id =
                                (!session_id.is_empty()).then_some(session_id.clone());
                            let Some(bounds) = this.terminal_scrollbar_track_bounds_for_session(
                                drag_session_id.as_deref(),
                            ) else {
                                return;
                            };
                            let max =
                                this.terminal_scroll_max_for_session(drag_session_id.as_deref());
                            let metrics = this.terminal_scrollbar_metrics_for_session(
                                drag_session_id.as_deref(),
                                bounds,
                            );
                            let grab_offset_y = terminal_scrollbar_grab_offset_for_pointer(
                                f32::from(event.position.y),
                                f32::from(bounds.origin.y),
                                metrics,
                            );
                            this.begin_terminal_scrollbar_drag(
                                drag_session_id.clone(),
                                grab_offset_y,
                                cx,
                            );
                            let offset = terminal_scroll_offset_from_pointer(
                                f32::from(event.position.y),
                                f32::from(bounds.origin.y),
                                metrics,
                                grab_offset_y,
                                max,
                            );
                            let repaint_session_id = this
                                .set_terminal_scroll_offset_for_session_state_only(
                                    drag_session_id.as_deref(),
                                    offset,
                                );
                            if repaint_session_id.is_some() {
                                this.notify_terminal_scroll_after_state_change(
                                    repaint_session_id.as_deref(),
                                    cx,
                                );
                            }
                            cx.stop_propagation();
                        })
                    })
                    .child(terminal_scrollbar_track_bounds_tracker(
                        cx.entity(),
                        (!session_id.is_empty()).then_some(session_id.to_string()),
                    ))
                    .child(terminal_overview_marker_canvas(
                        overview_markers.into(),
                        overview_total_rows,
                        overview_track_height_px,
                        overview_marker_buckets,
                        palette,
                    ))
                    .when(show, |this| {
                        this.child(terminal_scrollbar_thumb_element(
                            SharedString::from(thumb_id),
                            metrics,
                            palette,
                            is_active,
                            drag_active,
                        ))
                    }),
            )
    }

    pub(in crate::features) fn terminal_search_bar(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let query = self.terminal.search.query.clone();
        let search_input = self.text_input(
            "terminal.search.query",
            &query,
            TextInputSetup::placeholder(t!("terminalCtx.find")),
            cx,
        );
        let search_app = cx.entity();
        let search_focus = search_input.read(cx).focus_handle();
        let search_focused = search_input.read(cx).has_focus();
        let buffer_matches = self.terminal_buffer_matches();
        let buffer_occurrence_count = self.terminal_buffer_occurrence_ranges().len();
        let buffer_search_complete = self.terminal_buffer_search_complete();
        let buffer_search_truncated = self.terminal_buffer_search_truncated();
        let status = if buffer_matches.is_err() {
            t!("terminalCtx.findInvalidRegex").to_string()
        } else if query.is_empty() {
            String::new()
        } else if buffer_occurrence_count == 0 {
            if buffer_search_complete {
                t!("terminalCtx.findNoResults")
            } else {
                t!("terminalCtx.findSearching")
            }
            .to_string()
        } else {
            format!(
                "{}/{}{}",
                self.terminal
                    .search
                    .active_index
                    .min(buffer_occurrence_count - 1)
                    + 1,
                buffer_occurrence_count,
                if buffer_search_truncated {
                    "+"
                } else if !buffer_search_complete {
                    "…"
                } else {
                    ""
                }
            )
        };
        let disabled = buffer_matches.is_err() || buffer_occurrence_count == 0;
        div()
            .id(SharedString::from("terminal-search-bar"))
            .absolute()
            .top(px(4.))
            .right(px(4.))
            .w(px(420.))
            .max_w_full()
            .rounded_sm()
            .border_1()
            .border_color(rgb(palette.border))
            .bg(self.shell_surface_color(palette.surface))
            .shadow_lg()
            .px_2()
            .py_1()
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if this.handle_terminal_search_key_down(event, window, cx) {
                    cx.stop_propagation();
                }
            }))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Middle, |_, _, cx| cx.stop_propagation())
            .on_mouse_move(|_, _, cx| cx.stop_propagation())
            .on_click(|_, _, cx| cx.stop_propagation())
            .flex()
            .items_center()
            .gap_1()
            .child(
                div()
                    .id(SharedString::from("terminal-search-input"))
                    .h(px(24.))
                    .min_w_0()
                    .flex_1()
                    .rounded_sm()
                    .border_1()
                    .border_color(rgb(if search_focused {
                        palette.primary
                    } else {
                        palette.border
                    }))
                    .bg(rgb(palette.input_surface()))
                    .pl_2()
                    .pr(px(2.))
                    .flex()
                    .items_center()
                    .cursor_text()
                    .text_xs()
                    .text_color(rgb(palette.text))
                    .on_click(move |_, window, cx| {
                        window.focus(&search_focus, cx);
                    })
                    .child(div().min_w_0().flex_1().h_full().child(
                        NyaInput::new(&search_input).on_enter(move |shift, _, cx| {
                            search_app.update(cx, |this, cx| {
                                this.mark_user_activity();
                                this.navigate_terminal_search(if shift { -1 } else { 1 }, cx);
                            });
                        }),
                    ))
                    .child(
                        NyaButton::new("terminal-search-case", "Aa")
                            .compact()
                            .height(px(20.))
                            .variant(NyaButtonVariant::Ghost)
                            .selected(self.terminal.search.case_sensitive)
                            .tooltip(t!("terminalCtx.searchCaseSensitive"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.terminal.search.case_sensitive =
                                    !this.terminal.search.case_sensitive;
                                this.terminal.search.active_index = 0;
                                this.refresh_terminal_search_state(cx);
                            })),
                    )
                    .child(
                        NyaButton::new("terminal-search-word", "ab")
                            .compact()
                            .height(px(20.))
                            .variant(NyaButtonVariant::Ghost)
                            .selected(self.terminal.search.whole_word)
                            .tooltip(t!("terminalCtx.searchWholeWord"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.terminal.search.whole_word = !this.terminal.search.whole_word;
                                this.terminal.search.active_index = 0;
                                this.refresh_terminal_search_state(cx);
                            })),
                    )
                    .child(
                        NyaButton::new("terminal-search-regex", ".*")
                            .compact()
                            .height(px(20.))
                            .variant(NyaButtonVariant::Ghost)
                            .selected(self.terminal.search.regex)
                            .tooltip(t!("terminalCtx.searchRegex"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.terminal.search.regex = !this.terminal.search.regex;
                                this.terminal.search.active_index = 0;
                                this.refresh_terminal_search_state(cx);
                            })),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(rgb(if buffer_matches.is_err() {
                        palette.danger
                    } else {
                        palette.text_muted
                    }))
                    .child(status),
            )
            .child(
                NyaIconButton::new("terminal-search-prev", "icons/chevron-up.svg")
                    .size(px(22.))
                    .disabled(disabled)
                    .tooltip(t!("terminalCtx.findPrevious"))
                    .on_click(cx.listener(|this, _, _, cx| this.navigate_terminal_search(-1, cx))),
            )
            .child(
                NyaIconButton::new("terminal-search-next", "icons/chevron-down.svg")
                    .size(px(22.))
                    .disabled(disabled)
                    .tooltip(t!("terminalCtx.findNext"))
                    .on_click(cx.listener(|this, _, _, cx| this.navigate_terminal_search(1, cx))),
            )
            .child(
                NyaIconButton::new("terminal-search-close", "icons/window/close.svg")
                    .size(px(22.))
                    .tooltip(t!("common.close"))
                    .on_click(
                        cx.listener(|this, _, window, cx| this.close_terminal_search(window, cx)),
                    ),
            )
    }
}
