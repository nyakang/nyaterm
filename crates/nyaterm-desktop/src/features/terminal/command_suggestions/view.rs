//! Completion overlay layout, rendering and presentation mapping.
use super::state::CommandSuggestionItem;
use crate::features::NyaTermApp;
use crate::theme::ThemePalette;
use gpui::{
    Bounds, Context, FontWeight, IntoElement, Pixels, SharedString, div, prelude::*, px, rgb, rgba,
    svg,
};
use nyaterm_core::completion::types::{
    CandidateKind, CandidateSource, CompletionCandidate, CompletionMode,
};
use nyaterm_core::truncate_preview;
use nyaterm_ui::{NyaScrollable, NyaTooltip};
use rust_i18n::t;
use std::collections::HashSet;
pub(in crate::features) const SUGGESTION_OVERLAY_HEADER_HEIGHT: f32 = 24.0;
pub(in crate::features) const SUGGESTION_OVERLAY_FOOTER_HEIGHT: f32 = 24.0;
const SUGGESTION_OVERLAY_CHROME_HEIGHT: f32 =
    SUGGESTION_OVERLAY_HEADER_HEIGHT + SUGGESTION_OVERLAY_FOOTER_HEIGHT;
const SUGGESTION_OVERLAY_MAX_HEIGHT: f32 = 272.0;
const SUGGESTION_OVERLAY_CURSOR_GAP: f32 = 4.0;
const SUGGESTION_OVERLAY_VIEWPORT_MARGIN: f32 = 8.0;
const COMMAND_SUGGESTION_ROW_HEIGHT: f32 = 28.0;
const COMMAND_SUGGESTION_BORDER_HEIGHT: f32 = 2.0;
const SUGGESTION_PREVIEW_GAP: f32 = 6.0;
const SUGGESTION_PREVIEW_MIN_HEIGHT: f32 = 32.0;
const SUGGESTION_PREVIEW_MAX_HEIGHT: f32 = 120.0;

impl NyaTermApp {
    pub(in crate::features) fn suggestion_cursor_cell_for_session(
        &self,
        session_id: &str,
        cx: &gpui::App,
    ) -> Option<(usize, usize)> {
        if self.session.active_id() != Some(session_id) {
            return None;
        }
        if let Some(surface) = self.terminal.view.surfaces.get(session_id)
            && surface.read(cx).has_snapshot()
        {
            return surface.read(cx).suggestion_cursor_cell();
        }
        // Only read a committed frame: rendering must not synthesize history.
        let view = self.terminal.view.views.get(session_id)?;
        let snapshot = view.frame_snapshot.as_ref()?;
        nyaterm_terminal_gpui::geometry::terminal_cursor_display_cell(
            snapshot,
            self.active_terminal_display_offset(),
            view.viewport_rows_for_ui(),
            0.0,
        )
    }

    #[cfg(test)]
    pub(in crate::features) fn active_terminal_cursor_cell(&self) -> (usize, usize) {
        let session_id = self.session.active_id();
        let offset = self.active_terminal_display_offset();
        let snapshot = self.terminal_snapshot_for_session(session_id, offset);
        nyaterm_terminal_gpui::geometry::terminal_cursor_display_cell(
            &snapshot,
            offset,
            self.terminal_viewport_rows_for_session(session_id),
            0.0,
        )
        .unwrap_or((usize::MAX, snapshot.cursor.col))
    }

    pub(in crate::features) fn command_suggestions_overlay(
        &self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let Some(state) = self.terminal.assist.commands.popup.as_ref() else {
            return div().into_any_element();
        };
        if state.items.is_empty() {
            return div().into_any_element();
        }
        if self.session.active_id() != Some(state.session_id.as_str()) {
            return div().into_any_element();
        }
        if !self.command_suggestions_allowed_for_session(&state.session_id)
            || (if state.refreshing {
                !self.command_suggestion_refresh_can_remain_visible()
            } else {
                !self.command_suggestion_input_is_confirmed()
            })
        {
            return div().into_any_element();
        }
        let Some(cursor) = self.suggestion_cursor_cell_for_session(&state.session_id, cx) else {
            return div().into_any_element();
        };
        let menu_w = 380.0_f32;
        let menu_h = command_suggestion_desired_height(state.items.len());
        let title = t!("suggestions.title");
        let match_label = t!(if state.items.len() == 1 {
            "suggestions.match"
        } else {
            "suggestions.matches"
        });
        let footer = format!(
            "{} {} · Enter {} · Esc {}",
            if state.selected_index.is_some() {
                "↑↓ / Tab / Shift+Tab"
            } else {
                "↑↓"
            },
            t!("suggestions.select"),
            t!("suggestions.acceptSelected"),
            t!("suggestions.dismiss")
        );
        let Some(placement) = self.suggestion_overlay_position_for_session_with_policy(
            Some(&state.session_id),
            SuggestionOverlayTarget {
                cursor,
                menu_size: (menu_w, menu_h),
            },
            SuggestionOverlayPolicy::BelowFirst {
                minimum_height: command_suggestion_desired_height(state.items.len().min(2)),
            },
        ) else {
            return div().into_any_element();
        };
        let selected_preview = state
            .selected_index
            .and_then(|index| state.items.get(index))
            .filter(|item| item.command.chars().count() > 48)
            .and_then(|item| {
                suggestion_preview_position(placement, self.shell.viewport_size().1)
                    .map(|preview| (item.command.clone(), preview))
            });
        let scroll = &self.terminal.assist.commands.scroll;
        let list_height =
            placement.height - SUGGESTION_OVERLAY_CHROME_HEIGHT - COMMAND_SUGGESTION_BORDER_HEIGHT;
        if let Some(index) = state.selected_index
            && scroll.bounds().size.height != px(list_height)
        {
            // scroll_to_item uses the previous viewport during prepaint. Resolve
            // a resize with the new height; ordinary redraws preserve wheel scrolling.
            let mut offset = scroll.offset();
            let row_top = px(index as f32 * COMMAND_SUGGESTION_ROW_HEIGHT);
            let row_bottom = row_top + px(COMMAND_SUGGESTION_ROW_HEIGHT);
            if row_top + offset.y < px(0.) {
                offset.y = -row_top;
            } else if row_bottom + offset.y > px(list_height) {
                offset.y = px(list_height) - row_bottom;
            }
            scroll.set_offset(offset);
        }
        let mut list = div()
            .id(SharedString::from("command-suggestions-list"))
            .flex()
            .flex_col()
            .size_full()
            .track_scroll(scroll)
            .overflow_y_scroll();
        for (index, item) in state.items.iter().enumerate() {
            let input_version = state.input_version;
            let selected = state.selected_index == Some(index);
            let source_icon = match item.source.as_str() {
                "history" | "legacyHistory" | "tokenHistory" => "icons/history.svg",
                _ => "icons/commands.svg",
            };
            let label = if item.display.trim().is_empty() {
                item.command.clone()
            } else {
                item.display.clone()
            };
            let show_full_command = label.chars().count() > 48;
            let full_command = item.command.clone();
            let is_history = matches!(item.source.as_str(), "history" | "legacyHistory");
            let description = item.description.clone();
            let show_description = state.mode == CompletionMode::Token && !description.is_empty();
            let multiline = item.command.contains(['\r', '\n']);
            let delete_command = item.command.clone();
            let mut row = div()
                .id(SharedString::from(format!("command-suggestion-{index}")))
                .h(px(COMMAND_SUGGESTION_ROW_HEIGHT))
                .flex_none()
                .px_2()
                .flex()
                .items_center()
                .gap_2()
                .border_l_2()
                .border_color(rgb(if selected {
                    palette.primary
                } else {
                    palette.surface
                }))
                .bg(rgb(if selected {
                    palette.hover
                } else {
                    palette.surface
                }))
                .text_size(px(11.))
                .text_color(rgb(palette.text))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, window, cx| {
                    if this
                        .terminal
                        .assist
                        .commands
                        .popup
                        .as_ref()
                        .is_none_or(|popup| {
                            popup.refreshing || popup.input_version != input_version
                        })
                    {
                        return;
                    }
                    this.terminal.assist.commands.select_index(index);
                    let _ = this.apply_selected_command_suggestion(true, cx);
                    this.open_pending_suggestion_paste(window, cx);
                }))
                .child(
                    svg()
                        .size(px(13.))
                        .flex_none()
                        .path(source_icon)
                        .text_color(if selected {
                            rgb(palette.accent)
                        } else {
                            rgb(palette.text_dimmed)
                        }),
                )
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .when(show_description, |this| this.min_w(px(140.)))
                        .font_family(crate::features::shell::gpui_code_font_family())
                        .flex()
                        .items_center()
                        .overflow_hidden()
                        .children(command_suggestion_highlight_parts(
                            &label,
                            &item.indices,
                            palette,
                            selected,
                        )),
                );
            if multiline {
                row = row.child(
                    div()
                        .flex_none()
                        .text_color(rgb(palette.text_dimmed))
                        .child(t!("suggestions.openPasteReview")),
                );
            }
            if show_description {
                row = row.child(
                    div()
                        .id(SharedString::from(format!(
                            "command-suggestion-description-{index}"
                        )))
                        .min_w_0()
                        .max_w(px(180.))
                        .flex_initial()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_right()
                        .text_color(rgb(palette.text_dimmed))
                        .child(description.clone()),
                );
            }
            if show_full_command || !description.is_empty() || multiline {
                row = row.tooltip(move |window, cx| {
                    let label = if multiline {
                        t!("suggestions.openPasteReview").to_string()
                    } else if !description.is_empty() {
                        description.clone()
                    } else {
                        full_command.clone()
                    };
                    NyaTooltip::new(label).build(window, cx)
                });
            }
            if is_history {
                row =
                    row.child(
                        div()
                            .id(SharedString::from(format!(
                                "command-suggestion-del-{index}"
                            )))
                            .flex_none()
                            .w(px(20.))
                            .h(px(20.))
                            .rounded_sm()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(11.))
                            .text_color(rgb(palette.text_dimmed))
                            .hover(|this| {
                                this.bg(rgb(palette.surface_elevated))
                                    .text_color(rgb(palette.danger))
                            })
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                if this.terminal.assist.commands.popup.as_ref().is_none_or(
                                    |popup| {
                                        popup.refreshing || popup.input_version != input_version
                                    },
                                ) {
                                    return;
                                }
                                this.delete_command_suggestion_history(delete_command.clone(), cx);
                            }))
                            .child(
                                svg()
                                    .size(px(13.))
                                    .path("icons/fe/delete.svg")
                                    .text_color(rgb(palette.text_dimmed)),
                            ),
                    );
            }
            list = list.child(row);
        }

        div()
            .id(SharedString::from("command-suggestions-overlay"))
            .debug_selector(|| "command-suggestions-overlay".into())
            .absolute()
            .occlude()
            .left(px(placement.x))
            .top(px(placement.y))
            .w(px(placement.width))
            .h(px(placement.height))
            .flex()
            .flex_col()
            .rounded_lg()
            .border_1()
            .border_color(rgb(palette.border))
            .bg(rgba((palette.surface << 8) | 0xf2))
            .shadow_lg()
            .when_some(selected_preview, |this, (command, preview)| {
                this.child(
                    div()
                        .id("command-suggestion-preview")
                        .debug_selector(|| "command-suggestion-preview".into())
                        .absolute()
                        .map(|this| match placement.direction {
                            SuggestionOverlayDirection::Below => this
                                .top(px(placement.height + SUGGESTION_PREVIEW_GAP
                                    - COMMAND_SUGGESTION_BORDER_HEIGHT / 2.0)),
                            SuggestionOverlayDirection::Above => this
                                .bottom(px(placement.height + SUGGESTION_PREVIEW_GAP
                                    - COMMAND_SUGGESTION_BORDER_HEIGHT / 2.0)),
                        })
                        .left(px(-COMMAND_SUGGESTION_BORDER_HEIGHT / 2.0))
                        .w(px(placement.width))
                        .max_h(px(preview.height))
                        .flex()
                        .flex_col()
                        .rounded_sm()
                        .border_1()
                        .border_color(rgb(palette.border))
                        .bg(rgb(palette.surface_elevated))
                        .shadow_lg()
                        .font_family(crate::features::shell::gpui_code_font_family())
                        .text_size(px(11.))
                        .text_color(rgb(palette.text))
                        .whitespace_normal()
                        .child(
                            div()
                                .id("command-suggestion-preview-content")
                                .min_h_0()
                                .max_h(px(preview.height - COMMAND_SUGGESTION_BORDER_HEIGHT))
                                .p_2()
                                .overflow_y_scrollbar()
                                .child(command),
                        ),
                )
            })
            .child(
                div()
                    .h(px(SUGGESTION_OVERLAY_HEADER_HEIGHT))
                    .flex_none()
                    .px_2()
                    .border_b_1()
                    .border_color(rgb(palette.border))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .text_size(px(10.))
                            .font_weight(FontWeight(600.))
                            .text_color(rgb(palette.text_dimmed))
                            .child(
                                svg()
                                    .size(px(12.))
                                    .path("icons/commands.svg")
                                    .text_color(rgb(palette.text_dimmed)),
                            )
                            .child(title),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(palette.text_dimmed))
                            .child(format!("{} {match_label}", state.items.len())),
                    ),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(list)
                    .vertical_scrollbar(scroll),
            )
            .child(
                div()
                    .h(px(SUGGESTION_OVERLAY_FOOTER_HEIGHT))
                    .flex_none()
                    .px_2()
                    .border_t_1()
                    .border_color(rgb(palette.border))
                    .flex()
                    .items_center()
                    .text_size(px(10.))
                    .text_color(rgb(palette.text_dimmed))
                    .child(footer),
            )
            .into_any_element()
    }

    pub(in crate::features) fn suggestion_overlay_position_for_session(
        &self,
        session_id: Option<&str>,
        cursor_row: usize,
        cursor_col: usize,
        menu_w: f32,
        menu_h: f32,
    ) -> Option<SuggestionOverlayPlacement> {
        self.suggestion_overlay_position_for_session_with_policy(
            session_id,
            SuggestionOverlayTarget {
                cursor: (cursor_row, cursor_col),
                menu_size: (menu_w, menu_h),
            },
            SuggestionOverlayPolicy::Adaptive,
        )
    }

    pub(super) fn suggestion_overlay_position_for_session_with_policy(
        &self,
        session_id: Option<&str>,
        target: SuggestionOverlayTarget,
        policy: SuggestionOverlayPolicy,
    ) -> Option<SuggestionOverlayPlacement> {
        let session_id = session_id.or_else(|| self.session.active_id())?;
        // Command and credential popups share cached terminal geometry, which
        // remains available after a different workspace tab or page takes over.
        if !self.terminal_session_has_visible_surface(session_id) {
            return None;
        }
        let bounds = self.terminal_surface_bounds_for_session(Some(session_id))?;
        let insets = self.terminal_content_insets_for_bounds(Some(session_id), bounds);
        suggestion_overlay_position(
            SuggestionOverlayGeometry {
                bounds,
                cell_size: self.terminal_cell_size(),
                content_origin: (insets.left, insets.top),
                gutter: self.terminal_gutter_width_px_for_session(Some(session_id)),
                viewport_size: self.shell.viewport_size(),
            },
            target,
            policy,
        )
    }
}
pub(super) fn completion_suggestion_item(
    item: CompletionCandidate,
    input: &str,
) -> CommandSuggestionItem {
    let source = if item.kind == CandidateKind::QuickCommand {
        "quickCommand"
    } else if item.kind == CandidateKind::History {
        if item.sources.contains(&CandidateSource::History) {
            "history"
        } else {
            "legacyHistory"
        }
    } else if item.sources.contains(&CandidateSource::Catalog) {
        "static"
    } else if item.sources.contains(&CandidateSource::History) {
        "tokenHistory"
    } else {
        "quickToken"
    };
    CommandSuggestionItem {
        command: item.target_line(input).unwrap_or_default(),
        display: item.display,
        source: source.into(),
        score: item.score,
        indices: item.indices,
        fillable: item.fillable,
        description: item.description,
    }
}

pub(in crate::features) fn suggestion_overlay_desired_height(
    item_count: usize,
    row_height: f32,
) -> f32 {
    (item_count as f32 * row_height + SUGGESTION_OVERLAY_CHROME_HEIGHT)
        .min(SUGGESTION_OVERLAY_MAX_HEIGHT)
}

fn command_suggestion_desired_height(item_count: usize) -> f32 {
    (item_count as f32 * COMMAND_SUGGESTION_ROW_HEIGHT
        + SUGGESTION_OVERLAY_CHROME_HEIGHT
        + COMMAND_SUGGESTION_BORDER_HEIGHT)
        .min(SUGGESTION_OVERLAY_MAX_HEIGHT)
}

pub(in crate::features) fn suggestion_overlay_position(
    geometry: SuggestionOverlayGeometry,
    target: SuggestionOverlayTarget,
    policy: SuggestionOverlayPolicy,
) -> Option<SuggestionOverlayPlacement> {
    let SuggestionOverlayGeometry {
        bounds,
        cell_size,
        content_origin: (pad_left, pad_top),
        gutter,
        viewport_size,
    } = geometry;
    let SuggestionOverlayTarget {
        cursor: (cursor_row, cursor_col),
        menu_size: (menu_w, menu_h),
    } = target;
    let (cell_w, cell_h) = cell_size;
    let bounds_x = f32::from(bounds.origin.x);
    let bounds_y = f32::from(bounds.origin.y);
    let base_x = bounds_x + pad_left + gutter + cursor_col as f32 * cell_w;
    let cursor_top = bounds_y + pad_top + cursor_row as f32 * cell_h;
    let cursor_bottom = cursor_top + cell_h;
    let (viewport_w, viewport_h) = viewport_size;
    let surface_top = bounds_y.max(SUGGESTION_OVERLAY_VIEWPORT_MARGIN);
    // Suggestions live in the window overlay, so panels below the terminal do
    // not limit the available space. Keep the terminal bounds only as the anchor.
    let overlay_bottom = viewport_h - SUGGESTION_OVERLAY_VIEWPORT_MARGIN;
    if overlay_bottom <= surface_top || menu_h <= 0.0 {
        return None;
    }

    let below_y = cursor_bottom + SUGGESTION_OVERLAY_CURSOR_GAP;
    let above_bottom = cursor_top - SUGGESTION_OVERLAY_CURSOR_GAP;
    let space_below = (overlay_bottom - below_y).max(0.0);
    let space_above = (above_bottom - surface_top).max(0.0);
    let (height, direction) = match policy {
        SuggestionOverlayPolicy::BelowFirst { minimum_height } => {
            let minimum_height = minimum_height.min(menu_h);
            if space_below >= minimum_height {
                (menu_h.min(space_below), SuggestionOverlayDirection::Below)
            } else if space_above >= minimum_height {
                (menu_h.min(space_above), SuggestionOverlayDirection::Above)
            } else {
                return None;
            }
        }
        SuggestionOverlayPolicy::Adaptive => {
            if menu_h <= space_below {
                (menu_h, SuggestionOverlayDirection::Below)
            } else if menu_h <= space_above {
                (menu_h, SuggestionOverlayDirection::Above)
            } else if space_above > space_below {
                (space_above, SuggestionOverlayDirection::Above)
            } else {
                (space_below, SuggestionOverlayDirection::Below)
            }
        }
    };
    if height <= 0.0 {
        return None;
    }

    let width = match policy {
        SuggestionOverlayPolicy::Adaptive => menu_w,
        SuggestionOverlayPolicy::BelowFirst { .. } => {
            menu_w.min(viewport_w - 2.0 * SUGGESTION_OVERLAY_VIEWPORT_MARGIN)
        }
    };
    if width <= 0.0 {
        return None;
    }
    let max_x = (viewport_w - width - SUGGESTION_OVERLAY_VIEWPORT_MARGIN)
        .max(SUGGESTION_OVERLAY_VIEWPORT_MARGIN);
    Some(SuggestionOverlayPlacement {
        x: base_x.clamp(SUGGESTION_OVERLAY_VIEWPORT_MARGIN, max_x),
        y: match direction {
            SuggestionOverlayDirection::Below => below_y,
            SuggestionOverlayDirection::Above => above_bottom - height,
        },
        width,
        height,
        direction,
    })
}

#[derive(Clone, Copy)]
pub(in crate::features) enum SuggestionOverlayPolicy {
    Adaptive,
    BelowFirst { minimum_height: f32 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::features) enum SuggestionOverlayDirection {
    Below,
    Above,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::features) struct SuggestionOverlayPlacement {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub direction: SuggestionOverlayDirection,
}

struct SuggestionPreviewPlacement {
    height: f32,
}

fn suggestion_preview_position(
    placement: SuggestionOverlayPlacement,
    viewport_height: f32,
) -> Option<SuggestionPreviewPlacement> {
    // Stay on the far side of the popup so the preview never crosses the input row.
    let available = match placement.direction {
        SuggestionOverlayDirection::Below => {
            viewport_height
                - SUGGESTION_OVERLAY_VIEWPORT_MARGIN
                - placement.y
                - placement.height
                - SUGGESTION_PREVIEW_GAP
        }
        SuggestionOverlayDirection::Above => {
            placement.y - SUGGESTION_PREVIEW_GAP - SUGGESTION_OVERLAY_VIEWPORT_MARGIN
        }
    };
    (available >= SUGGESTION_PREVIEW_MIN_HEIGHT).then_some(SuggestionPreviewPlacement {
        height: available.min(SUGGESTION_PREVIEW_MAX_HEIGHT),
    })
}

#[derive(Clone, Copy)]
pub(in crate::features) struct SuggestionOverlayGeometry {
    pub bounds: Bounds<Pixels>,
    pub cell_size: (f32, f32),
    pub content_origin: (f32, f32),
    pub gutter: f32,
    pub viewport_size: (f32, f32),
}

#[derive(Clone, Copy)]
pub(in crate::features) struct SuggestionOverlayTarget {
    pub cursor: (usize, usize),
    pub menu_size: (f32, f32),
}

pub(super) fn command_suggestion_highlight_parts(
    text: &str,
    indices: &[u32],
    palette: ThemePalette,
    selected: bool,
) -> Vec<gpui::AnyElement> {
    let _selected = selected;
    let clipped = truncate_preview(text, 48);
    let index_set: HashSet<u32> = indices.iter().copied().collect();
    let chars: Vec<char> = clipped.chars().collect();
    let mut parts = Vec::new();
    let mut i = 0usize;
    while i < chars.len() {
        let highlighted = index_set.contains(&(i as u32));
        let start = i;
        i += 1;
        while i < chars.len() && index_set.contains(&(i as u32)) == highlighted {
            i += 1;
        }
        let chunk: String = chars[start..i].iter().collect();
        let color = if highlighted {
            palette.link
        } else {
            palette.text
        };
        parts.push(
            div()
                .text_color(rgb(color))
                .when(highlighted, |this| this.font_weight(FontWeight::SEMIBOLD))
                .child(chunk)
                .into_any_element(),
        );
    }
    if parts.is_empty() {
        parts.push(
            div()
                .text_color(rgb(palette.text))
                .child(clipped)
                .into_any_element(),
        );
    }
    parts
}

#[cfg(test)]
mod overlay_position_tests {
    use gpui::{Bounds, point, px, size};

    use super::{
        SuggestionOverlayDirection, SuggestionOverlayGeometry, SuggestionOverlayPlacement,
        SuggestionOverlayPolicy, SuggestionOverlayTarget, command_suggestion_desired_height,
        suggestion_overlay_desired_height, suggestion_overlay_position,
        suggestion_preview_position,
    };

    #[test]
    fn suggestion_overlay_position_anchors_below_cursor() {
        let placement = suggestion_overlay_position(
            SuggestionOverlayGeometry {
                bounds: Bounds::new(point(px(10.0), px(20.0)), size(px(800.0), px(400.0))),
                cell_size: (8.0, 16.0),
                content_origin: (8.0, 0.0),
                gutter: 0.0,
                viewport_size: (1024.0, 768.0),
            },
            SuggestionOverlayTarget {
                cursor: (2, 4),
                menu_size: (300.0, 120.0),
            },
            SuggestionOverlayPolicy::Adaptive,
        )
        .expect("surface has room below the cursor");

        assert_eq!(placement.x, 50.0);
        assert_eq!(placement.y, 72.0);
        assert_eq!(placement.height, 120.0);
    }

    #[test]
    fn suggestion_overlay_position_clamps_and_flips_above_cursor() {
        let placement = suggestion_overlay_position(
            SuggestionOverlayGeometry {
                bounds: Bounds::new(point(px(700.0), px(100.0)), size(px(200.0), px(500.0))),
                cell_size: (8.0, 16.0),
                content_origin: (8.0, 0.0),
                gutter: 0.0,
                viewport_size: (900.0, 620.0),
            },
            SuggestionOverlayTarget {
                cursor: (24, 30),
                menu_size: (300.0, 140.0),
            },
            SuggestionOverlayPolicy::Adaptive,
        )
        .expect("surface has room above the cursor");

        assert_eq!(placement.x, 592.0);
        assert_eq!(placement.y, 340.0);
        assert_eq!(placement.height, 140.0);
        assert_eq!(placement.y + placement.height, 480.0);
    }

    #[test]
    fn suggestion_overlay_position_limits_height_without_covering_cursor() {
        let placement = suggestion_overlay_position(
            SuggestionOverlayGeometry {
                bounds: Bounds::new(point(px(20.0), px(100.0)), size(px(600.0), px(180.0))),
                cell_size: (8.0, 16.0),
                content_origin: (0.0, 0.0),
                gutter: 0.0,
                viewport_size: (1024.0, 280.0),
            },
            SuggestionOverlayTarget {
                cursor: (6, 4),
                menu_size: (300.0, 140.0),
            },
            SuggestionOverlayPolicy::Adaptive,
        )
        .expect("surface has constrained room above the cursor");

        assert_eq!(placement.y, 100.0);
        assert_eq!(placement.height, 92.0);
        assert_eq!(placement.y + placement.height, 192.0);
    }

    #[test]
    fn suggestion_overlay_position_uses_space_below_terminal_surface() {
        let placement = suggestion_overlay_position(
            SuggestionOverlayGeometry {
                bounds: Bounds::new(point(px(20.0), px(100.0)), size(px(600.0), px(200.0))),
                cell_size: (8.0, 16.0),
                content_origin: (0.0, 0.0),
                gutter: 0.0,
                viewport_size: (1024.0, 768.0),
            },
            SuggestionOverlayTarget {
                cursor: (11, 4),
                menu_size: (380.0, 320.0),
            },
            SuggestionOverlayPolicy::Adaptive,
        )
        .expect("window has room below the terminal over the bottom panel");

        assert_eq!(placement.y, 296.0);
        assert_eq!(placement.height, 320.0);
    }

    #[test]
    fn suggestion_overlay_position_limits_height_at_window_bottom() {
        let placement = suggestion_overlay_position(
            SuggestionOverlayGeometry {
                bounds: Bounds::new(point(px(20.0), px(100.0)), size(px(600.0), px(120.0))),
                cell_size: (8.0, 16.0),
                content_origin: (0.0, 0.0),
                gutter: 0.0,
                viewport_size: (1024.0, 330.0),
            },
            SuggestionOverlayTarget {
                cursor: (6, 4),
                menu_size: (300.0, 140.0),
            },
            SuggestionOverlayPolicy::Adaptive,
        )
        .expect("window has more room below the cursor than above");

        assert_eq!(placement.y, 216.0);
        assert_eq!(placement.height, 106.0);
        assert_eq!(placement.y + placement.height, 322.0);
    }

    #[test]
    fn suggestion_overlay_height_is_content_sized_and_capped() {
        assert_eq!(suggestion_overlay_desired_height(1, 28.0), 76.0);
        assert_eq!(suggestion_overlay_desired_height(10, 28.0), 272.0);
        assert_eq!(suggestion_overlay_desired_height(1, 36.0), 84.0);
        assert_eq!(suggestion_overlay_desired_height(8, 36.0), 272.0);
    }

    fn command_placement(
        viewport_size: (f32, f32),
        cursor: (usize, usize),
        item_count: usize,
    ) -> Option<SuggestionOverlayPlacement> {
        suggestion_overlay_position(
            SuggestionOverlayGeometry {
                bounds: Bounds::new(point(px(20.0), px(100.0)), size(px(600.0), px(300.0))),
                cell_size: (8.0, 16.0),
                content_origin: (8.0, 0.0),
                gutter: 12.0,
                viewport_size,
            },
            SuggestionOverlayTarget {
                cursor,
                menu_size: (380.0, command_suggestion_desired_height(item_count)),
            },
            SuggestionOverlayPolicy::BelowFirst {
                minimum_height: command_suggestion_desired_height(item_count.min(2)),
            },
        )
    }

    #[test]
    fn command_overlay_anchors_at_cursor_with_content_insets_and_gutter() {
        let placement = command_placement((1024.0, 768.0), (2, 4), 3).unwrap();
        assert_eq!(placement.x, 72.0);
        assert_eq!(placement.y, 152.0);
        assert_eq!(placement.width, 380.0);
        assert_eq!(placement.height, 134.0);
        assert_eq!(placement.direction, SuggestionOverlayDirection::Below);
    }

    #[test]
    fn command_overlay_shrinks_below_even_when_full_menu_fits_above() {
        // Screenshot layout: bottom commands panel is available to the overlay.
        let placement = command_placement((1024.0, 650.0), (17, 4), 12).unwrap();
        assert_eq!(placement.y, 392.0);
        assert_eq!(placement.height, 250.0);
        assert_eq!(placement.direction, SuggestionOverlayDirection::Below);
    }

    #[test]
    fn command_overlay_flips_only_below_minimum_height() {
        let at_minimum = command_placement((1024.0, 650.0), (26, 4), 12).unwrap();
        assert_eq!(at_minimum.height, 106.0);
        assert_eq!(at_minimum.direction, SuggestionOverlayDirection::Below);

        let below_minimum = command_placement((1024.0, 649.0), (26, 4), 12).unwrap();
        assert_eq!(below_minimum.direction, SuggestionOverlayDirection::Above);
        assert_eq!(below_minimum.y + below_minimum.height, 512.0);
    }

    #[test]
    fn command_overlay_accepts_one_candidate_below_at_78_pixels() {
        let placement = command_placement((1024.0, 622.0), (26, 4), 1).unwrap();
        assert_eq!(placement.height, 78.0);
        assert_eq!(placement.direction, SuggestionOverlayDirection::Below);
        let placement = command_placement((1024.0, 621.0), (26, 4), 1).unwrap();
        assert_eq!(placement.direction, SuggestionOverlayDirection::Above);
    }

    #[test]
    fn command_overlay_shrinks_above_and_hides_when_neither_side_is_usable() {
        let placement = command_placement((1024.0, 330.0), (10, 4), 12).unwrap();
        assert_eq!(placement.direction, SuggestionOverlayDirection::Above);
        assert_eq!(placement.y, 100.0);
        assert_eq!(placement.height, 156.0);
        assert!(command_placement((1024.0, 300.0), (6, 4), 12).is_none());
    }

    #[test]
    fn command_overlay_clamps_right_edge_and_reduces_width_in_narrow_windows() {
        let placement = command_placement((800.0, 768.0), (2, 95), 3).unwrap();
        assert_eq!(placement.x, 412.0);
        assert_eq!(placement.x + placement.width, 792.0);
        let placement = command_placement((300.0, 768.0), (2, 4), 3).unwrap();
        assert_eq!(placement.x, 8.0);
        assert_eq!(placement.width, 284.0);
        assert!(command_placement((16.0, 768.0), (2, 4), 3).is_none());
    }

    #[test]
    fn command_overlay_height_includes_border_and_stays_capped() {
        assert_eq!(command_suggestion_desired_height(1), 78.0);
        assert_eq!(command_suggestion_desired_height(2), 106.0);
        assert_eq!(command_suggestion_desired_height(12), 272.0);
    }

    #[test]
    fn command_preview_uses_only_space_beyond_the_popup_and_caps_height() {
        let mut placement = command_placement((1024.0, 768.0), (2, 4), 3).unwrap();
        assert_eq!(
            suggestion_preview_position(placement, 768.0)
                .unwrap()
                .height,
            120.0
        );
        // 32px after the 6px gap is the minimum useful preview size.
        let bottom = placement.y + placement.height;
        assert_eq!(
            suggestion_preview_position(placement, bottom + 46.0)
                .unwrap()
                .height,
            32.0
        );
        assert!(suggestion_preview_position(placement, bottom + 45.0).is_none());
        placement.direction = SuggestionOverlayDirection::Above;
        placement.y = 46.0;
        assert_eq!(
            suggestion_preview_position(placement, 768.0)
                .unwrap()
                .height,
            32.0
        );
        placement.y = 45.0;
        assert!(suggestion_preview_position(placement, 768.0).is_none());
    }
    #[test]
    fn whole_candidate_types_keep_their_presentation_without_legacy_descriptions() {
        use super::completion_suggestion_item;
        use nyaterm_core::completion::types::{
            CandidateKind, CandidateSource, CompletionCandidate,
        };

        for (kind, sources, expected) in [
            (
                CandidateKind::History,
                vec![CandidateSource::LegacyHistory],
                "legacyHistory",
            ),
            (
                CandidateKind::History,
                vec![CandidateSource::Catalog, CandidateSource::History],
                "history",
            ),
            (
                CandidateKind::QuickCommand,
                vec![CandidateSource::Catalog, CandidateSource::QuickCommand],
                "quickCommand",
            ),
        ] {
            let item = completion_suggestion_item(
                CompletionCandidate {
                    display: "重启代理".into(),
                    insert_text: "systemctl restart example.service".into(),
                    description: String::new(),
                    range: 0.."重启代理".len(),
                    kind,
                    sources,
                    use_count: 1,
                    last_used_at_ms: 1,
                    score: 1,
                    indices: vec![0, 1],
                    fillable: true,
                    context_priority: 0,
                },
                "重启代理",
            );
            assert_eq!(item.source, expected);
            assert_eq!(item.display, "重启代理");
            assert_eq!(item.command, "systemctl restart example.service");
            assert!(item.description.is_empty());
        }
    }
}
