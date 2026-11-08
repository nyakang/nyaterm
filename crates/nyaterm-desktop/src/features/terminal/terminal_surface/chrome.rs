use gpui::{
    App, ClickEvent, Context, FontWeight, IntoElement, KeyDownEvent, MouseButton, SharedString,
    Window, div, prelude::*, px, rgb, rgba, svg,
};
use nyaterm_core::truncate_preview;
use nyaterm_ui::{NyaInput, NyaScrollable};
use rust_i18n::t;

use crate::features::{NyaTermApp, text_inputs::TextInputSetup};
use crate::models::TerminalSearchMode;
use crate::theme::ThemePalette;

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
            TextInputSetup::placeholder("Find"),
            cx,
        );
        let search_focus = search_input.read(cx).focus_handle();
        let search_focused = search_input.read(cx).has_focus();
        let buffer_matches = self.terminal_buffer_matches();
        let buffer_occurrence_count = self.terminal_buffer_occurrence_ranges().len();
        let buffer_search_complete = self.terminal_buffer_search_complete();
        let buffer_search_truncated = self.terminal_buffer_search_truncated();
        let history_results = self.terminal_history_search_results();
        let history_pending = self.terminal_history_search_pending_for_current_query();
        let (status, is_error) = match self.terminal.search.mode {
            TerminalSearchMode::Buffer => match &buffer_matches {
                Ok(_) if self.terminal.search.query.trim().is_empty() => (String::new(), false),
                Ok(_) if buffer_occurrence_count == 0 && !buffer_search_complete => {
                    ("searching".to_string(), false)
                }
                Ok(_) if buffer_occurrence_count == 0 => ("not found".to_string(), false),
                Ok(_) => {
                    let count = buffer_occurrence_count;
                    let count_label = if buffer_search_truncated {
                        "1000+".to_string()
                    } else {
                        count.to_string()
                    };
                    (
                        format!(
                            "{}/{}{}",
                            self.terminal
                                .search
                                .active_index
                                .min(count.saturating_sub(1))
                                + 1,
                            count_label,
                            if buffer_search_complete {
                                ""
                            } else {
                                " searching"
                            },
                        ),
                        false,
                    )
                }
                Err(error) => (truncate_preview(error, 40), true),
            },
            TerminalSearchMode::History if history_pending => ("searching".to_string(), false),
            TerminalSearchMode::History => match &history_results {
                Ok(response) if self.terminal.search.query.trim().is_empty() => {
                    (String::new(), false)
                }
                Ok(response) if response.results.is_empty() => ("not found".to_string(), false),
                Ok(response) => (
                    format!(
                        "{} result(s){}",
                        response.total,
                        if response.truncated { " truncated" } else { "" }
                    ),
                    false,
                ),
                Err(error) => (truncate_preview(error, 40), true),
            },
        };
        let show_history_results = self.terminal.search.mode == TerminalSearchMode::History
            && !self.terminal.search.query.trim().is_empty();
        let mut history_rows = div().id(SharedString::from("terminal-search-history-results"));
        if show_history_results {
            history_rows = history_rows
                .mt_1()
                .max_h(px(256.))
                .flex()
                .flex_col()
                .gap_1()
                .border_t_1()
                .border_color(rgb(palette.border))
                .pt_1();
            match history_results {
                Ok(response) if response.results.is_empty() => {
                    history_rows = history_rows.child(
                        div()
                            .px_2()
                            .py_2()
                            .text_xs()
                            .text_color(rgb(palette.text_muted))
                            .child("No history matches."),
                    );
                }
                Ok(response) => {
                    history_rows = history_rows.child(
                        div()
                            .px_1()
                            .pb_1()
                            .text_xs()
                            .text_color(rgb(palette.text_dimmed))
                            .child(format!(
                                "{} match(es) · {} ms{}",
                                response.total,
                                response.elapsed_ms,
                                if response.truncated {
                                    " · truncated"
                                } else {
                                    ""
                                }
                            )),
                    );
                    for result in response.results.into_iter().take(8) {
                        let before = result.before.join("\n");
                        let after = result.after.join("\n");
                        let mut context_parts = Vec::new();
                        if !before.trim().is_empty() {
                            context_parts.push(truncate_preview(&before, 120));
                        }
                        context_parts.push(format!("> {}", truncate_preview(&result.preview, 120)));
                        if !after.trim().is_empty() {
                            context_parts.push(truncate_preview(&after, 120));
                        }
                        let context = context_parts.join("\n");
                        history_rows =
                            history_rows.child(
                                div()
                                    .rounded_sm()
                                    .border_1()
                                    .border_color(rgb(palette.border))
                                    .bg(rgb(palette.input))
                                    .p_2()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(FontWeight(700.))
                                            .text_color(rgb(palette.text_muted))
                                            .child(format!("line {}", result.line_number)),
                                    )
                                    .child(
                                        div()
                                            .mt_1()
                                            .font_family(
                                                crate::features::shell::gpui_code_font_family(),
                                            )
                                            .text_xs()
                                            .text_color(rgb(palette.text))
                                            .line_height(px(16.))
                                            .child(truncate_preview(&result.preview, 96)),
                                    )
                                    .when(
                                        !result.before.is_empty() || !result.after.is_empty(),
                                        |this| {
                                            this.child(
                                            div()
                                                .mt_1()
                                                .font_family(
                                                    crate::features::shell::gpui_code_font_family(),
                                                )
                                                .text_size(px(10.))
                                                .text_color(rgb(palette.text_dimmed))
                                                .line_height(px(14.))
                                                .child(context),
                                        )
                                        },
                                    ),
                            );
                    }
                }
                Err(error) => {
                    history_rows = history_rows.child(
                        div()
                            .px_2()
                            .py_2()
                            .text_xs()
                            .text_color(rgb(palette.danger))
                            .child(truncate_preview(&error, 96)),
                    );
                }
            }
        }

        // Only the populated dropdown scrolls; wrapping the empty div would give
        // the scroll wrapper a full-size root with nothing in it.
        let history_rows = if show_history_results {
            history_rows.overflow_y_scrollbar().into_any_element()
        } else {
            history_rows.into_any_element()
        };

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
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(terminal_search_mode_button(
                        "terminal-search-mode-buffer",
                        TerminalSearchMode::Buffer.label(),
                        self.terminal.search.mode == TerminalSearchMode::Buffer,
                        self.theme_palette(),
                        cx.listener(|this, _, _, cx| {
                            this.terminal.search.mode = TerminalSearchMode::Buffer;
                            this.terminal.search.active_index = 0;
                            this.refresh_terminal_search_state(cx);
                        }),
                    ))
                    .child(terminal_search_mode_button(
                        "terminal-search-mode-history",
                        TerminalSearchMode::History.label(),
                        self.terminal.search.mode == TerminalSearchMode::History,
                        self.theme_palette(),
                        cx.listener(|this, _, _, cx| {
                            this.terminal.search.mode = TerminalSearchMode::History;
                            this.terminal.search.active_index = 0;
                            this.refresh_terminal_search_state(cx);
                        }),
                    ))
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_xs()
                            .text_color(if is_error {
                                rgb(palette.danger)
                            } else {
                                rgb(palette.text_muted)
                            })
                            .child(status),
                    )
                    .child(terminal_search_icon_button(
                        "terminal-search-close",
                        "icons/window/close.svg",
                        self.theme_palette(),
                        cx.listener(|this, _, window, cx| {
                            this.close_terminal_search(window, cx);
                        }),
                    )),
            )
            .child(
                div()
                    .mt_1()
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
                            .px_2()
                            .flex()
                            .items_center()
                            .cursor_text()
                            .text_xs()
                            .text_color(rgb(palette.text))
                            .on_click(move |_, window, cx| {
                                window.focus(&search_focus, cx);
                            })
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .h_full()
                                    .child(NyaInput::new(&search_input)),
                            ),
                    )
                    .child(terminal_search_flag_button(
                        "terminal-search-case",
                        "Aa",
                        self.terminal.search.case_sensitive,
                        self.theme_palette(),
                        cx.listener(|this, _, _, cx| {
                            this.terminal.search.case_sensitive =
                                !this.terminal.search.case_sensitive;
                            this.terminal.search.active_index = 0;
                            this.refresh_terminal_search_state(cx);
                        }),
                    ))
                    .child(terminal_search_flag_button(
                        "terminal-search-regex",
                        ".*",
                        self.terminal.search.regex,
                        self.theme_palette(),
                        cx.listener(|this, _, _, cx| {
                            this.terminal.search.regex = !this.terminal.search.regex;
                            this.terminal.search.active_index = 0;
                            this.refresh_terminal_search_state(cx);
                        }),
                    ))
                    .child(terminal_search_flag_button(
                        "terminal-search-word",
                        "Word",
                        self.terminal.search.whole_word,
                        self.theme_palette(),
                        cx.listener(|this, _, _, cx| {
                            this.terminal.search.whole_word = !this.terminal.search.whole_word;
                            this.terminal.search.active_index = 0;
                            this.refresh_terminal_search_state(cx);
                        }),
                    ))
                    .child(terminal_search_flag_button(
                        "terminal-search-wrap",
                        t!("terminalCtx.searchWrapAround"),
                        self.terminal.search_wrap_around(self.session.active_id()),
                        self.theme_palette(),
                        cx.listener(|this, _, _, cx| {
                            let session_id = this.session.active_id_owned();
                            this.terminal
                                .toggle_search_wrap_around(session_id.as_deref());
                            cx.notify();
                        }),
                    ))
                    .when(
                        self.terminal.search.mode == TerminalSearchMode::Buffer,
                        |this| {
                            this.child(terminal_search_icon_button(
                                "terminal-search-prev",
                                "icons/chevron-up.svg",
                                self.theme_palette(),
                                cx.listener(|this, _, _, cx| {
                                    this.navigate_terminal_search(-1, cx);
                                }),
                            ))
                            .child(terminal_search_icon_button(
                                "terminal-search-next",
                                "icons/chevron-down.svg",
                                self.theme_palette(),
                                cx.listener(|this, _, _, cx| {
                                    this.navigate_terminal_search(1, cx);
                                }),
                            ))
                        },
                    ),
            )
            .child(history_rows)
    }
}

fn terminal_search_mode_button(
    id: impl Into<String>,
    label: &'static str,
    active: bool,
    palette: ThemePalette,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(SharedString::from(id.into()))
        .h(px(20.))
        .px_2()
        .flex()
        .items_center()
        .rounded_sm()
        .bg(if active {
            rgb(palette.accent)
        } else {
            rgba(0x00000000)
        })
        .text_color(if active {
            rgb(palette.bg)
        } else {
            rgb(palette.text_muted)
        })
        .text_size(px(11.))
        .cursor_pointer()
        .hover(|this| this.opacity(0.9))
        .child(label)
        .on_click(on_click)
}

fn terminal_search_icon_button(
    id: impl Into<String>,
    icon_path: &'static str,
    palette: ThemePalette,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(SharedString::from(id.into()))
        .size(px(20.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_sm()
        .text_color(rgb(palette.text_muted))
        .cursor_pointer()
        .hover(|this| this.opacity(0.8))
        .child(
            svg()
                .size(px(14.))
                .path(icon_path)
                .text_color(rgb(palette.text_muted)),
        )
        .on_click(on_click)
}

fn terminal_search_flag_button(
    id: impl Into<String>,
    label: impl Into<SharedString>,
    active: bool,
    palette: ThemePalette,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let label = label.into();
    div()
        .id(SharedString::from(id.into()))
        .h(px(24.))
        .px_1()
        .flex()
        .items_center()
        .rounded_sm()
        .border_1()
        .border_color(rgb(palette.border))
        .bg(if active {
            rgb(palette.accent)
        } else {
            rgba(0x00000000)
        })
        .text_color(if active {
            rgb(palette.bg)
        } else {
            rgb(palette.text_muted)
        })
        .text_size(px(11.))
        .cursor_pointer()
        .hover(|this| this.opacity(0.9))
        .child(label)
        .on_click(on_click)
}
