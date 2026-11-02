use rust_i18n::t;

use std::sync::Arc;
use std::time::Instant;

use gpui::{
    AnyElement, App, ClickEvent, Context, FontWeight, IntoElement, KeyDownEvent, SharedString,
    Window, div, list, prelude::*, px, rgb, rgba, svg,
};
use nyaterm_transport::{RecordingMode, RecordingStatus, RecordingStatusState, SessionInfo};
use nyaterm_ui::{NyaDropdownMenu, NyaMenuItem, NyaScrollable, NyaSearchInput};

use crate::features::formatting::{session_kind_label, short_id};
use crate::features::perf::record_gpui_perf_sample;
use crate::features::recording::state::RecordingPanelRowLayout;
use crate::features::{NyaTermApp, text_inputs::TextInputSetup};

#[derive(Clone)]
struct RecordingSessionPanelRow {
    session: SessionInfo,
    display_name: String,
    status: Option<RecordingStatus>,
}

pub(in crate::features) struct RecordingSessionsPanelModel {
    total_count: usize,
    rows: Arc<Vec<RecordingSessionPanelRow>>,
    query_active: bool,
}

impl RecordingSessionsPanelModel {
    pub(in crate::features) fn count_label(&self) -> String {
        session_panel_count_label(self.total_count, self.rows.len(), self.query_active)
    }
}

impl NyaTermApp {
    pub(in crate::features) fn recording_sessions_panel_model(
        &self,
    ) -> RecordingSessionsPanelModel {
        let started_at = Instant::now();
        let sessions = self.sorted_active_sessions();
        let total_count = sessions.len();
        let query = self.recording_session_filter_query();
        let query_active = !query.is_empty();
        let rows = sessions
            .into_iter()
            .filter_map(|session| {
                let display_name = self.session.display_name_by_info(&session);
                recording_session_matches_query(&session, &display_name, &query).then_some(
                    RecordingSessionPanelRow {
                        status: self.recording.status(&session.id),
                        session,
                        display_name,
                    },
                )
            })
            .collect::<Vec<_>>();
        let model = RecordingSessionsPanelModel {
            total_count,
            rows: Arc::new(rows),
            query_active,
        };
        record_gpui_perf_sample(
            "recording_sessions_model",
            started_at.elapsed(),
            self.gpui_perf_context(model.rows.len(), None),
        );
        model
    }

    pub(in crate::features) fn recording_panel(
        &mut self,
        model: RecordingSessionsPanelModel,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let RecordingSessionsPanelModel {
            total_count,
            rows,
            query_active: _,
        } = model;
        let palette = self.theme_palette();
        let active_session_id = self.session.active_id_owned();
        let no_sessions_label = t!("panel.noActiveSessions").to_string();
        let no_matches_label = t!("activeSessions.noMatches").to_string();
        let search_placeholder = t!("recording.searchPlaceholder").to_string();
        let search_draft = self.recording.search_draft().to_string();
        let search_field = self.text_input(
            "recording.search",
            &search_draft,
            TextInputSetup::placeholder(search_placeholder),
            cx,
        );

        let list_scroll = self.recording.panel_list(
            rows.iter()
                .map(|row| RecordingPanelRowLayout {
                    session_id: row.session.id.clone(),
                    has_path: row
                        .status
                        .as_ref()
                        .and_then(|status| status.file_path.as_ref())
                        .is_some_and(|path| !path.as_os_str().is_empty()),
                    has_error: row.status.as_ref().is_some_and(|status| {
                        matches!(
                            status.state,
                            RecordingStatusState::Degraded | RecordingStatusState::Failed
                        )
                    }),
                })
                .collect(),
        );
        let session_list: AnyElement = if total_count == 0 {
            div()
                .id(SharedString::from("recording-session-list"))
                .flex_1()
                .min_h_0()
                .child(crate::widgets::empty_panel_with_icon(
                    no_sessions_label,
                    palette,
                    "icons/record.svg",
                ))
                .into_any_element()
        } else if rows.is_empty() {
            div()
                .id(SharedString::from("recording-session-list"))
                .flex_1()
                .min_h_0()
                .child(crate::widgets::empty_panel_with_icon(
                    no_matches_label,
                    palette,
                    "icons/record.svg",
                ))
                .into_any_element()
        } else {
            list(
                list_scroll.clone(),
                cx.processor(move |this, index: usize, _, cx| {
                    let row = rows[index].clone();
                    div()
                        .px_2()
                        .pb_1()
                        .flex_none()
                        .child(this.recording_session_row(row, active_session_id.as_deref(), cx))
                        .into_any_element()
                }),
            )
            .flex_1()
            .min_h_0()
            .into_any_element()
        };

        // Tauri RecordingPanel: PanelHeader(meta count) + search strip + dense session rows.
        // Shared stack already renders PanelHeader; body is search + list only.
        div()
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(self.shell_transparent_color(palette.surface))
            .child(
                div()
                    .h(px(40.))
                    .flex_none()
                    .px_2()
                    .border_b_1()
                    .border_color(rgb(palette.border))
                    .bg(self.shell_transparent_color(palette.section_header))
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().relative().flex_1().min_w_0().child(
                        NyaSearchInput::new("recording-session-search", &search_field).on_key_down(
                            cx.listener(|this, event: &KeyDownEvent, _, cx| {
                                if event.keystroke.key == "escape" {
                                    cx.stop_propagation();
                                    this.recording.clear_search_draft();
                                    this.reset_text_input("recording.search", "", cx);
                                    this.shell
                                        .set_status(t!("recording.searchCleared").to_string());
                                    cx.notify();
                                }
                            }),
                        ),
                    )),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .py_2()
                    .child(session_list)
                    .vertical_scrollbar(&list_scroll),
            )
    }

    fn recording_session_row(
        &mut self,
        row: RecordingSessionPanelRow,
        active_session_id: Option<&str>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let RecordingSessionPanelRow {
            session,
            display_name,
            status,
        } = row;
        let palette = self.theme_palette();
        let session_id = session.id.clone();
        let start_session_id = session.id.clone();
        let select_session_id = session.id.clone();
        let menu_session_id = session.id.clone();
        let is_current = active_session_id == Some(session.id.as_str());
        let is_recording = status.is_some();
        let is_busy = self.recording.busy_action(&session.id).is_some();
        let is_problem = status.as_ref().is_some_and(|status| {
            matches!(
                status.state,
                RecordingStatusState::Degraded | RecordingStatusState::Failed
            )
        });
        let status_label = match status.as_ref().map(|status| status.state) {
            Some(RecordingStatusState::Failed) => t!("recording.failed"),
            Some(RecordingStatusState::Degraded) => t!("recording.degraded"),
            _ => t!("recording.recording"),
        }
        .to_string();
        let summary = status.as_ref().map_or_else(
            || short_id(&session.id).to_string(),
            |status| {
                format!(
                    "{} · {}",
                    match status.mode {
                        RecordingMode::Transcript => "Transcript",
                        RecordingMode::Raw => "Raw",
                    },
                    format_recording_bytes(status.written_bytes)
                )
            },
        );
        let path = status
            .as_ref()
            .and_then(|status| status.file_path.as_ref())
            .map(|path| path.display().to_string())
            .unwrap_or_default();
        let error = if is_problem {
            status
                .as_ref()
                .and_then(|status| status.last_error.clone())
                .filter(|error| !error.is_empty())
                .unwrap_or_else(|| status_label.clone())
        } else {
            String::new()
        };
        let app = cx.entity().downgrade();

        div()
            .id(SharedString::from(format!(
                "recording-session-row-{session_id}"
            )))
            .debug_selector({
                let session_id = session_id.clone();
                move || format!("recording-session-row-{session_id}")
            })
            .w_full()
            .min_w_0()
            .overflow_hidden()
            .rounded_md()
            .p_2()
            .border_1()
            .border_color(if is_current {
                rgba((palette.primary << 8) | 0x73)
            } else {
                rgba(0x00000000)
            })
            .when(is_busy, |this| this.opacity(0.72))
            .flex()
            .items_start()
            .gap_2()
            .cursor_pointer()
            .hover(|this| this.bg(rgb(palette.hover)))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.select_session(select_session_id.clone(), cx);
            }))
            .child(
                div()
                    .id(SharedString::from(format!(
                        "recording-session-state-{session_id}"
                    )))
                    .mt(px(6.))
                    .size(px(8.))
                    .rounded_full()
                    .flex_none()
                    .bg(rgb(if is_problem {
                        palette.warning
                    } else if is_recording {
                        palette.danger
                    } else {
                        palette.success
                    }))
                    .when(is_recording, |this| {
                        this.tooltip(move |window, cx| {
                            nyaterm_ui::NyaTooltip::new(status_label.clone()).build(window, cx)
                        })
                    }),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .w_full()
                            .min_w_0()
                            .h(px(20.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                recording_text_line(
                                    format!("recording-session-name-{session_id}"),
                                    display_name,
                                    palette.text,
                                )
                                .flex_1()
                                .text_size(px(12.))
                                .font_weight(FontWeight(600.)),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .px_1()
                                    .rounded_sm()
                                    .bg(rgb(palette.hover))
                                    .text_size(px(10.))
                                    .text_color(rgb(palette.text_muted))
                                    .child(session_kind_label(session.kind).to_ascii_uppercase()),
                            ),
                    )
                    .child(recording_text_line(
                        format!("recording-session-summary-{session_id}"),
                        summary,
                        palette.text_dimmed,
                    ))
                    .when(!path.is_empty(), |this| {
                        this.child(recording_text_line(
                            format!("recording-session-path-{session_id}"),
                            path,
                            palette.text_dimmed,
                        ))
                    })
                    .when(!error.is_empty(), |this| {
                        this.child(recording_text_line(
                            format!("recording-session-error-{session_id}"),
                            error,
                            palette.warning,
                        ))
                    }),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_0()
                    .child(recording_action_svg_button(
                        palette,
                        format!("recording-session-toggle-{session_id}"),
                        if is_recording {
                            "icons/session/stop.svg"
                        } else {
                            "icons/session/record.svg"
                        },
                        rgb(if is_busy {
                            palette.warning
                        } else if is_recording {
                            palette.danger
                        } else {
                            palette.text_muted
                        }),
                        if is_recording {
                            t!("recording.stop")
                        } else {
                            t!("recording.start")
                        }
                        .to_string(),
                        !is_busy,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.toggle_recording_for_session(&start_session_id, cx);
                        }),
                    ))
                    .child(
                        NyaDropdownMenu::new(format!("recording-session-more-{session_id}"))
                            .icon("icons/session/more.svg")
                            .icon_size(px(16.))
                            .disabled(is_busy)
                            .on_trigger(|_, _, cx| cx.stop_propagation())
                            .items_dynamic(move |_, cx| {
                                app.update(cx, |this, cx| {
                                    this.recording_panel_menu_items(&menu_session_id, cx)
                                })
                                .unwrap_or_default()
                            }),
                    ),
            )
    }

    fn recording_panel_menu_items(
        &self,
        session_id: &str,
        cx: &mut Context<Self>,
    ) -> Vec<NyaMenuItem> {
        let mut items = Vec::new();
        if let Some(status) = self.recording.status(session_id) {
            let open_path = status.file_path.clone();
            let reveal_path = status.file_path;
            items.push(
                NyaMenuItem::action(t!("recording.openLog"))
                    .icon("icons/file/description.svg")
                    .disabled(open_path.is_none())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        if let Some(path) = &open_path {
                            cx.open_with_system(path);
                            this.shell.set_status(t!("recording.opened").to_string());
                        }
                    })),
            );
            items.push(
                NyaMenuItem::action(t!("recording.showInFolder"))
                    .icon("icons/session/folder-open.svg")
                    .disabled(reveal_path.is_none())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        if let Some(path) = &reveal_path {
                            cx.reveal_path(path);
                            this.shell
                                .set_status(t!("recording.folderRevealed").to_string());
                        }
                    })),
            );
        } else {
            for (label, mode, icon) in [
                (
                    t!("recording.startTranscriptLog"),
                    RecordingMode::Transcript,
                    "icons/file/description.svg",
                ),
                (
                    t!("recording.startRawLog"),
                    RecordingMode::Raw,
                    "icons/session/record.svg",
                ),
            ] {
                let session_id = session_id.to_string();
                items.push(NyaMenuItem::action(label).icon(icon).on_click(cx.listener(
                    move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.start_recording_for_session(&session_id, mode, cx);
                    },
                )));
            }
        }
        let save_session_id = session_id.to_string();
        items.extend([
            NyaMenuItem::separator(),
            NyaMenuItem::action(t!("recording.saveTranscript"))
                .icon("icons/session/save.svg")
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.save_session_transcript_for_session(&save_session_id, cx);
                })),
        ]);
        let is_busy = self.recording.busy_action(session_id).is_some();
        if is_busy {
            items.into_iter().map(|item| item.disabled(true)).collect()
        } else {
            items
        }
    }

    fn recording_session_filter_query(&self) -> String {
        self.recording.search_draft().trim().to_lowercase()
    }
}

fn recording_session_matches_query(session: &SessionInfo, display_name: &str, query: &str) -> bool {
    query.is_empty()
        || format!(
            "{} {} {} {}",
            display_name,
            session.name,
            session_kind_label(session.kind),
            session.id
        )
        .to_lowercase()
        .contains(query)
}

fn session_panel_count_label(total: usize, visible: usize, query_active: bool) -> String {
    if query_active {
        format!("{visible}/{total}")
    } else {
        total.to_string()
    }
}

fn format_recording_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MiB", bytes as f64 / 1024. / 1024.)
    } else if bytes >= 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.)
    } else {
        format!("{bytes} B")
    }
}

// GPUI shapes explicit newlines even on a nowrap element; flatten only the display copy.
fn recording_single_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn recording_text_line(id: String, text: String, color: u32) -> gpui::Stateful<gpui::Div> {
    div()
        .id(SharedString::from(id.clone()))
        .debug_selector(move || id.clone())
        .w_full()
        .min_w_0()
        .h(px(16.))
        .flex_none()
        .line_height(px(16.))
        .text_size(px(10.))
        .text_color(rgb(color))
        .truncate()
        .child(recording_single_line(&text))
        .when(!text.is_empty(), |this| {
            this.tooltip(move |window, cx| {
                nyaterm_ui::NyaTooltip::new(text.clone()).build(window, cx)
            })
        })
}

fn recording_action_svg_button(
    palette: crate::theme::ThemePalette,
    id: impl Into<String>,
    icon_path: &'static str,
    color: impl Into<gpui::Hsla>,
    tooltip: impl Into<String>,
    enabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let color = color.into();
    let tooltip = tooltip.into();
    let id = id.into();
    div()
        .id(SharedString::from(id.clone()))
        .debug_selector(move || id.clone())
        .size(px(28.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .text_color(color)
        .when(enabled, |this| {
            this.cursor_pointer().hover(|this| {
                this.bg(rgb(palette.surface_elevated))
                    .text_color(rgb(palette.text))
            })
        })
        .when(!enabled, |this| this.opacity(0.4))
        .child(
            svg()
                .size(px(16.))
                .flex_none()
                .path(icon_path)
                .text_color(color),
        )
        .tooltip(move |window, cx| nyaterm_ui::NyaTooltip::new(tooltip.clone()).build(window, cx))
        .on_click(move |event, window, cx| {
            if enabled {
                on_click(event, window, cx);
            }
        })
}

#[cfg(test)]
mod tests {
    use nyaterm_transport::{SessionInfo, SessionKind};

    use super::{recording_session_matches_query, session_panel_count_label};

    fn session() -> SessionInfo {
        SessionInfo {
            id: "session-42".to_string(),
            name: "Production Recorder".to_string(),
            kind: SessionKind::Ssh,
            working_dir: None,
            cols: 80,
            rows: 24,
        }
    }

    #[test]
    fn recording_session_query_matches_dynamic_name_original_name_kind_and_id() {
        let session = session();

        assert!(recording_session_matches_query(
            &session,
            "Renamed Recorder",
            "renamed"
        ));
        assert!(recording_session_matches_query(
            &session,
            "Renamed Recorder",
            "production"
        ));
        assert!(recording_session_matches_query(
            &session,
            "Renamed Recorder",
            "ssh"
        ));
        assert!(recording_session_matches_query(
            &session,
            "Renamed Recorder",
            "session-42"
        ));
        assert!(!recording_session_matches_query(
            &session,
            "Renamed Recorder",
            "missing"
        ));
    }

    #[test]
    fn recording_session_count_label_tracks_filtered_and_unfiltered_counts() {
        assert_eq!(session_panel_count_label(100, 9, false), "100");
        assert_eq!(session_panel_count_label(100, 9, true), "9/100");
    }
}

#[cfg(test)]
mod layout_tests;
