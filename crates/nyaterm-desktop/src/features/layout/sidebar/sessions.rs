use rust_i18n::t;

use std::sync::Arc;
use std::time::Instant;

use gpui::{
    AnyElement, Context, FontWeight, IntoElement, SharedString, div, prelude::*, px, rgb, rgba,
    uniform_list,
};
use nyaterm_core::{RuntimeMode, truncate_preview};
use nyaterm_transport::SessionInfo;
use nyaterm_ui::{NyaDropdownMenu, NyaMenuItem, NyaScrollable};

use crate::features::formatting::{session_kind_label, status_label};
use crate::features::perf::record_gpui_perf_sample;
use crate::features::{NyaTermApp, text_inputs::TextInputSetup};
use crate::widgets::{capability_line, empty_panel, small_button, status_pill};

use super::super::view_helpers::session_action_svg_button;

const SESSION_PANEL_LOGICAL_ROW_HEIGHT_PX: f32 = 52.;

#[derive(Clone)]
struct ActiveSessionPanelRow {
    session: SessionInfo,
    display_name: String,
}

pub(in crate::features) struct ActiveSessionsPanelModel {
    total_count: usize,
    rows: Arc<Vec<ActiveSessionPanelRow>>,
    query_active: bool,
}

impl ActiveSessionsPanelModel {
    pub(in crate::features) fn count_label(&self) -> String {
        session_panel_count_label(self.total_count, self.rows.len(), self.query_active)
    }
}

impl NyaTermApp {
    pub(in crate::features) fn sorted_active_sessions(&self) -> Vec<SessionInfo> {
        let mut sessions = self.session.ordered_sessions();
        sort_active_sessions(&mut sessions);
        sessions
    }

    pub(in crate::features) fn active_sessions_panel_model(&self) -> ActiveSessionsPanelModel {
        let started_at = Instant::now();
        let sessions = self.sorted_active_sessions();
        let total_count = sessions.len();
        let query = self.session.active_search_draft().trim().to_lowercase();
        let query_active = !query.is_empty();
        let rows = sessions
            .into_iter()
            .filter_map(|session| {
                let display_name = self.session.display_name_by_info(&session);
                active_session_matches_query(&session, &display_name, &query).then_some(
                    ActiveSessionPanelRow {
                        session,
                        display_name,
                    },
                )
            })
            .collect::<Vec<_>>();
        let model = ActiveSessionsPanelModel {
            total_count,
            rows: Arc::new(rows),
            query_active,
        };
        record_gpui_perf_sample(
            "active_sessions_model",
            started_at.elapsed(),
            self.gpui_perf_context(model.rows.len(), None),
        );
        model
    }

    pub(in crate::features) fn active_sessions_panel(
        &mut self,
        model: ActiveSessionsPanelModel,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let ActiveSessionsPanelModel {
            total_count,
            rows,
            query_active: _,
        } = model;
        let palette = self.theme_palette();
        let list_scroll = self.session.active_list_scroll().clone();
        // Built before the panel, which reads `self` throughout: creating the
        // box needs it mutably.
        let search_draft = self.session.active_search_draft().to_string();
        let search_input = self
            .search_input_box(
                "sessions.filter",
                &search_draft,
                TextInputSetup::placeholder(t!("activeSessions.searchPlaceholder")),
                cx,
            )
            .into_any_element();
        let list: AnyElement = if total_count == 0 {
            div()
                .id(SharedString::from("active-sessions-list"))
                .flex_1()
                .min_h_0()
                .child(crate::widgets::empty_panel_with_icon(
                    t!("panel.noActiveSessions"),
                    palette,
                    "icons/sessions.svg",
                ))
                .into_any_element()
        } else if rows.is_empty() {
            div()
                .id(SharedString::from("active-sessions-list"))
                .flex_1()
                .min_h_0()
                .child(crate::widgets::empty_panel_with_icon(
                    t!("activeSessions.noMatches"),
                    palette,
                    "icons/sessions.svg",
                ))
                .into_any_element()
        } else {
            uniform_list(
                "active-sessions-list",
                rows.len(),
                cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                    let mut items = Vec::with_capacity(range.len());
                    for index in range {
                        let Some(row) = rows.get(index).cloned() else {
                            continue;
                        };
                        items.push(
                            div()
                                .h(px(SESSION_PANEL_LOGICAL_ROW_HEIGHT_PX))
                                .px_2()
                                .pb_1()
                                .flex_none()
                                .child(this.active_session_row(row.session, row.display_name, cx)),
                        );
                    }
                    items
                }),
            )
            .flex_1()
            .min_h_0()
            .track_scroll(&list_scroll)
            .into_any_element()
        };

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
                    .child(div().min_w_0().flex_1().child(search_input)),
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
                    .child(list)
                    .vertical_scrollbar(&list_scroll),
            )
    }

    pub(in crate::features) fn left_workspace_summary(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let sessions = self.session.ordered_sessions();
        let session_count = sessions.len();
        let query = self
            .session
            .active_search_draft()
            .trim()
            .to_ascii_lowercase();
        // Built before the panel, which reads `self` throughout: creating the
        // box needs it mutably.
        let search_draft = self.session.active_search_draft().to_string();
        let sessions_search_input = self
            .search_input_box(
                "sessions.filter",
                &search_draft,
                TextInputSetup::placeholder("Search sessions"),
                cx,
            )
            .into_any_element();
        let mut active_session_rows = div().mt_3().flex().flex_col().gap_2();
        let mut visible_count = 0usize;
        if sessions.is_empty() {
            active_session_rows = active_session_rows.child(empty_panel(
                "No active runtime sessions.",
                self.theme_palette(),
            ));
        } else {
            for session in sessions {
                let display_name = self.session.display_name_by_info(&session);
                let haystack = format!(
                    "{} {} {} {}",
                    display_name,
                    session.name,
                    session_kind_label(session.kind),
                    session.id
                )
                .to_ascii_lowercase();
                if !query.is_empty() && !haystack.contains(&query) {
                    continue;
                }
                visible_count += 1;
                active_session_rows =
                    active_session_rows.child(self.active_session_row(session, display_name, cx));
            }
            if visible_count == 0 {
                active_session_rows = active_session_rows.child(empty_panel(
                    "No matching active sessions.",
                    self.theme_palette(),
                ));
            }
        }

        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(palette.border))
                    .bg(rgb(palette.input))
                    .p_3()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight(700.))
                            .text_color(rgb(palette.text_muted))
                            .child("WORKSPACE"),
                    )
                    .child(capability_line(
                        palette,
                        "Active Sessions",
                        session_count.to_string(),
                    ))
                    .child(capability_line(
                        palette,
                        "Profiles",
                        self.connection_state.connections().len().to_string(),
                    ))
                    .child(capability_line(
                        palette,
                        "Quick Commands",
                        self.commands.quick_commands().len().to_string(),
                    ))
                    .child(capability_line(
                        palette,
                        "Tunnels",
                        self.tunnel_state.tunnels().len().to_string(),
                    )),
            )
            .child(
                div()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(palette.border))
                    .bg(rgb(palette.input))
                    .p_3()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight(800.))
                                    .text_color(rgb(palette.text_muted))
                                    .child("ACTIVE SESSIONS"),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(if query.is_empty() {
                                        rgb(palette.text_muted)
                                    } else {
                                        rgb(palette.success)
                                    })
                                    .child(if query.is_empty() {
                                        session_count.to_string()
                                    } else {
                                        format!("{visible_count}/{session_count}")
                                    }),
                            ),
                    )
                    .child(div().mt_3().child(sessions_search_input))
                    .child(active_session_rows),
            )
            .child(
                div()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(palette.border))
                    .bg(rgb(palette.input))
                    .p_3()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight(700.))
                                    .text_color(rgb(palette.text_muted))
                                    .child("START"),
                            )
                            .child(status_pill(
                                status_label(self.shell.status()),
                                rgb(palette.link),
                                rgb(palette.hover),
                            )),
                    )
                    .child(
                        div()
                            .mt_3()
                            .flex()
                            .gap_2()
                            .child(small_button(
                                palette,
                                "left-start-local",
                                "Local",
                                cx.listener(|this, _, window, cx| {
                                    this.start_local_session(window, cx);
                                }),
                            ))
                            .child(small_button(
                                palette,
                                "left-probe",
                                "Probe",
                                cx.listener(|this, _, _, cx| {
                                    this.send_probe_command(cx);
                                }),
                            )),
                    ),
            )
            .child(
                div()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(palette.border))
                    .bg(rgb(palette.input))
                    .p_3()
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(palette.text_muted))
                            .child("Runtime"),
                    )
                    .child(div().mt_1().text_sm().child(match self.runtime.mode() {
                        RuntimeMode::Portable => "Portable",
                        RuntimeMode::Installed => "Installed",
                    }))
                    .child(
                        div()
                            .mt_2()
                            .text_xs()
                            .text_color(rgb(palette.text_muted))
                            .child(self.runtime.config_dir().display().to_string()),
                    ),
            )
            .child(
                div()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(palette.hover))
                    .bg(rgb(palette.input))
                    .p_3()
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(palette.text_muted))
                            .child("Config Store"),
                    )
                    .child(
                        div()
                            .mt_1()
                            .text_sm()
                            .text_color(if self.settings.store_status().ready {
                                rgb(palette.success)
                            } else {
                                rgb(palette.danger)
                            })
                            .child(self.settings.store_status().message.to_string()),
                    )
                    .child(
                        div()
                            .mt_2()
                            .text_xs()
                            .text_color(rgb(palette.text_muted))
                            .child(self.settings.store_status().path.to_string()),
                    ),
            )
    }

    pub(in crate::features) fn active_session_row(
        &mut self,
        session: SessionInfo,
        display_name: String,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let session_id = session.id.clone();
        let row_group = SharedString::from(format!("active-session-group-{}", session.id));
        let rename_session_id = session.id.clone();
        let reconnect_session_id = session.id.clone();
        let disconnect_session_id = session.id.clone();
        let custom_color = self.session.tab_color(&session.id);
        let is_active = self.session.active_id() == Some(session.id.as_str());
        let is_disconnected = self.session.is_disconnected(&session.id);
        let has_unread = self.terminal.session_has_unread(&session.id);
        let busy_action = self.session.busy_action(&session.id).map(str::to_string);
        let is_busy = busy_action.is_some();
        let can_reconnect = !is_busy
            && !self
                .session
                .start_reconnect_is_pending(reconnect_session_id.as_str());
        let can_disconnect = !is_busy && !is_disconnected;
        let reconnect_label = if busy_action.as_deref() == Some("reconnect") {
            t!("tabCtx.reconnecting").to_string()
        } else {
            t!("tabCtx.reconnect").to_string()
        };
        let disconnect_label = if busy_action.as_deref() == Some("disconnect") {
            t!("tabCtx.disconnecting").to_string()
        } else {
            t!("tabCtx.disconnect").to_string()
        };
        let menu = NyaDropdownMenu::new(format!("active-session-more-{}", session.id))
            .icon("icons/session/more.svg")
            .icon_size(px(14.))
            .tooltip(t!("common.more"))
            .disabled(is_busy)
            .min_width(px(160.))
            .items([
                NyaMenuItem::action(reconnect_label)
                    .icon("icons/session/reconnect.svg")
                    .disabled(!can_reconnect)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if this.session.session_is_busy(&reconnect_session_id)
                            || this
                                .session
                                .start_reconnect_is_pending(&reconnect_session_id)
                        {
                            cx.notify();
                            return;
                        }
                        this.select_session(reconnect_session_id.clone(), cx);
                        this.reconnect_session(reconnect_session_id.clone(), window, cx);
                    })),
                NyaMenuItem::action(disconnect_label)
                    .icon("icons/session/disconnect.svg")
                    .disabled(!can_disconnect)
                    .danger()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.session.session_is_busy(&disconnect_session_id)
                            || this.session.is_disconnected(&disconnect_session_id)
                        {
                            cx.notify();
                            return;
                        }
                        this.disconnect_session(disconnect_session_id.clone(), cx);
                    })),
            ])
            .on_trigger(|_, _, cx| cx.stop_propagation());
        let accent = if let Some(custom_color) = custom_color {
            rgb(custom_color)
        } else if is_disconnected {
            rgb(palette.text_dimmed)
        } else if is_active {
            rgb(palette.success)
        } else if has_unread {
            rgb(palette.warning)
        } else {
            rgb(0x22c55e)
        };
        let row_bg = if let Some(custom_color) = custom_color {
            rgba((custom_color << 8) | if is_active { 0x22 } else { 0x12 })
        } else {
            rgba(0x00000000)
        };
        let hover_bg = if let Some(custom_color) = custom_color {
            rgba((custom_color << 8) | if is_active { 0x30 } else { 0x20 })
        } else {
            rgb(palette.hover)
        };
        // Tauri ActiveSessions: full display name + type badge + full mono session id.
        let kind = session_kind_label(session.kind).to_ascii_uppercase();
        let full_id = session.id.clone();
        let id_preview = truncate_preview(&full_id, 42);
        // The row already shows the endpoint type badge; a trailing `:port` only
        // adds noise (OxideTerm-style rows show host, not host:port).
        let title = truncate_preview(&nyaterm_core::strip_display_port(&display_name), 32);

        div()
            .id(SharedString::from(format!(
                "active-session-row-{session_id}"
            )))
            .group(row_group.clone())
            .relative()
            .h(px(48.))
            .rounded_md()
            .px_2()
            .bg(row_bg)
            .when(is_disconnected, |this| this.opacity(0.5))
            .when(is_busy, |this| this.opacity(0.72))
            .cursor_pointer()
            .hover(move |this| this.bg(hover_bg))
            .child(
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .min_w_0()
                                            .flex_1()
                                            .text_xs()
                                            .font_weight(FontWeight(600.))
                                            .text_color(rgb(palette.text))
                                            .overflow_hidden()
                                            .child(title),
                                    )
                                    .child(
                                        div()
                                            .px_1()
                                            .py(px(1.))
                                            .rounded_sm()
                                            .bg(rgb(palette.hover))
                                            .text_size(px(10.))
                                            .font_weight(FontWeight(700.))
                                            .text_color(rgb(palette.text_dimmed))
                                            .child(kind),
                                    ),
                            )
                            .child(
                                div()
                                    .font_family(crate::features::shell::gpui_code_font_family())
                                    .text_size(px(10.))
                                    .text_color(rgb(palette.text_dimmed))
                                    .overflow_hidden()
                                    .child(id_preview),
                            ),
                    )
                    .child(
                        div()
                            .size(px(8.))
                            .rounded_full()
                            .bg(accent)
                            .flex_none()
                            .mr_1(),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_0()
                            .flex_none()
                            .opacity(0.)
                            .group_hover(row_group, |style| style.opacity(1.))
                            .child(session_action_svg_button(
                                palette,
                                format!("active-session-rename-{rename_session_id}"),
                                "icons/session/rename.svg",
                                t!("tabCtx.rename").to_string(),
                                !is_busy,
                                cx.listener(move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    if this.session.session_is_busy(&rename_session_id) {
                                        return;
                                    }
                                    this.open_rename_session(rename_session_id.clone(), window, cx);
                                }),
                            ))
                            .child(menu),
                    ),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.select_session(session_id.clone(), cx);
            }))
    }
}

fn sort_active_sessions(sessions: &mut [SessionInfo]) {
    sessions.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| session_kind_label(left.kind).cmp(session_kind_label(right.kind)))
    });
}

fn active_session_matches_query(session: &SessionInfo, display_name: &str, query: &str) -> bool {
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

#[cfg(test)]
mod tests {
    use nyaterm_transport::{SessionInfo, SessionKind};

    use super::{active_session_matches_query, session_panel_count_label, sort_active_sessions};

    fn session(id: &str, name: &str, kind: SessionKind) -> SessionInfo {
        SessionInfo {
            id: id.to_string(),
            name: name.to_string(),
            kind,
            working_dir: None,
            cols: 80,
            rows: 24,
        }
    }

    #[test]
    fn active_session_model_sort_is_case_insensitive_and_stable_by_kind() {
        let mut sessions = vec![
            session("serial", "alpha", SessionKind::Serial),
            session("ssh", "Alpha", SessionKind::Ssh),
            session("beta", "beta", SessionKind::LocalPty),
        ];

        sort_active_sessions(&mut sessions);

        assert_eq!(
            sessions
                .iter()
                .map(|session| session.id.as_str())
                .collect::<Vec<_>>(),
            vec!["serial", "ssh", "beta"]
        );
    }

    #[test]
    fn active_session_query_matches_dynamic_display_name_case_insensitively() {
        let session = session("session-1", "original", SessionKind::Ssh);

        assert!(active_session_matches_query(
            &session,
            "Production Shell",
            "production"
        ));
        assert!(active_session_matches_query(
            &session,
            "Production Shell",
            "SESSION-1".to_lowercase().as_str()
        ));
        assert!(!active_session_matches_query(
            &session,
            "Production Shell",
            "missing"
        ));
    }

    #[test]
    fn active_session_count_label_tracks_filtered_and_unfiltered_counts() {
        assert_eq!(session_panel_count_label(100, 12, false), "100");
        assert_eq!(session_panel_count_label(100, 12, true), "12/100");
    }
}
