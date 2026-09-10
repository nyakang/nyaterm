use rust_i18n::t;

use nyaterm_ui::{NyaPopover, NyaPopoverAlign, NyaPopoverPlacement, NyaScrollArea, NyaScrollable};
use std::collections::{HashMap, HashSet};

use gpui::{
    AnyElement, Context, FontWeight, IntoElement, MouseButton, SharedString, div, prelude::*, px,
    rgb, rgba, svg,
};
use nyaterm_core::{ConnectionType, Group, SavedConnection, build_group_path, truncate_preview};

use crate::features::NyaTermApp;
use crate::features::icons::resolve_connection_icon;
use crate::features::shell::SessionTabTooltip;
use crate::features::view_widgets::themed_icon;
use crate::theme::ThemePalette;

const NEW_SESSION_MENU_WIDTH: f32 = 300.;
const NEW_SESSION_SUBMENU_WIDTH: f32 = 260.;
const NEW_SESSION_MENU_ROW_HEIGHT: f32 = 32.;

fn new_session_shell_connections(connections: &[SavedConnection]) -> Vec<SavedConnection> {
    let mut shell = connections
        .iter()
        .filter(|connection| matches!(connection.config, ConnectionType::LocalTerminal { .. }))
        .cloned()
        .collect::<Vec<_>>();
    shell.sort_by(|left, right| {
        left.sort_order
            .cmp(&right.sort_order)
            .then_with(|| {
                left.name
                    .to_ascii_lowercase()
                    .cmp(&right.name.to_ascii_lowercase())
            })
            .then_with(|| left.id.cmp(&right.id))
    });
    shell
}

fn new_session_recent_connections(connections: &[SavedConnection]) -> Vec<SavedConnection> {
    let mut recent = connections
        .iter()
        .filter(|connection| connection.last_used_at_ms.unwrap_or(0) > 0)
        .cloned()
        .collect::<Vec<_>>();
    recent.sort_by(|left, right| {
        right
            .last_used_at_ms
            .unwrap_or(0)
            .cmp(&left.last_used_at_ms.unwrap_or(0))
            .then_with(|| left.sort_order.cmp(&right.sort_order))
            .then_with(|| {
                left.name
                    .to_ascii_lowercase()
                    .cmp(&right.name.to_ascii_lowercase())
            })
            .then_with(|| left.id.cmp(&right.id))
    });
    recent.truncate(10);
    recent
}

fn new_session_connection_protocol(connection: &SavedConnection) -> &'static str {
    match connection.config {
        ConnectionType::LocalTerminal { .. } => "shell",
        ConnectionType::Telnet { .. } => "telnet",
        ConnectionType::Serial { .. } => "serial",
        ConnectionType::Rdp { .. } => "rdp",
        ConnectionType::Ssh { .. } => "ssh",
        ConnectionType::Vnc { .. } => "vnc",
    }
}

fn new_session_recent_label(connection: &SavedConnection, groups: &[Group]) -> String {
    let mut path = build_group_path(groups, connection.group_id.as_deref())
        .into_iter()
        .skip(1)
        .map(|segment| segment.name)
        .collect::<Vec<_>>();
    path.push(connection.name.clone());
    format!(
        "{}://{}",
        new_session_connection_protocol(connection),
        path.join("/")
    )
}

fn new_session_visible_group_ids(
    connections: &[SavedConnection],
    groups: &[Group],
) -> HashSet<String> {
    let parents = groups
        .iter()
        .map(|group| (group.id.as_str(), group.parent_id.as_deref()))
        .collect::<HashMap<_, _>>();
    let mut visible = HashSet::new();

    for connection in connections {
        let mut current = connection
            .group_id
            .as_deref()
            .filter(|group_id| parents.contains_key(group_id));
        let mut visited = HashSet::new();
        while let Some(group_id) = current {
            if !visited.insert(group_id) {
                break;
            }
            visible.insert(group_id.to_string());
            current = parents
                .get(group_id)
                .copied()
                .flatten()
                .filter(|parent_id| parents.contains_key(parent_id));
        }
    }

    visible
}

fn new_session_groups_for_parent(
    groups: &[Group],
    visible_group_ids: &HashSet<String>,
    parent_id: Option<&str>,
) -> Vec<Group> {
    let group_ids: HashSet<&str> = groups.iter().map(|group| group.id.as_str()).collect();
    let mut children = groups
        .iter()
        .filter(|group| {
            if !visible_group_ids.contains(&group.id) {
                return false;
            }
            let normalized_parent = group
                .parent_id
                .as_deref()
                .filter(|candidate| group_ids.contains(*candidate));
            normalized_parent == parent_id
        })
        .cloned()
        .collect::<Vec<_>>();
    children.sort_by(|left, right| {
        left.sort_order.cmp(&right.sort_order).then_with(|| {
            left.name
                .to_ascii_lowercase()
                .cmp(&right.name.to_ascii_lowercase())
        })
    });
    children
}

fn new_session_connections_for_group(
    connections: &[SavedConnection],
    groups: &[Group],
    group_id: Option<&str>,
) -> Vec<SavedConnection> {
    let group_ids: HashSet<&str> = groups.iter().map(|group| group.id.as_str()).collect();
    let mut matches = connections
        .iter()
        .filter(|connection| {
            let normalized_group = connection
                .group_id
                .as_deref()
                .filter(|candidate| group_ids.contains(*candidate));
            normalized_group == group_id
        })
        .cloned()
        .collect::<Vec<_>>();
    matches.sort_by(|left, right| {
        left.sort_order.cmp(&right.sort_order).then_with(|| {
            left.name
                .to_ascii_lowercase()
                .cmp(&right.name.to_ascii_lowercase())
        })
    });
    matches
}

impl NyaTermApp {
    pub(in crate::features) fn render_open_tabs_menu(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let hover_bg = self.shell_surface_color(palette.hover);
        // Tauri openTabsMenuItems is reversed (rightmost first) but keeps global ordinals.
        let ordered = self.ordered_tab_sessions();
        let ordinals: std::collections::HashMap<String, usize> = ordered
            .iter()
            .enumerate()
            .map(|(index, session)| (session.id.clone(), index + 1))
            .collect();
        let mut sessions = ordered;
        sessions.reverse();
        let active_id = self.session.active_id_owned();
        let mut rows = div().max_h(px(320.)).overflow_y_scrollbar().py_1();

        if sessions.is_empty() {
            rows = rows.child(
                div()
                    .px_3()
                    .py_2()
                    .text_size(px(12.))
                    .text_color(rgb(palette.text_muted))
                    .child(t!("sessionQuickSwitcher.noSessions")),
            );
        } else {
            for (index, session) in sessions.into_iter().enumerate() {
                let session_id = session.id.clone();
                let is_active = active_id
                    .as_deref()
                    .is_some_and(|id| self.tab_root_for_session(id) == session_id);
                let leaf_ids = self
                    .shell
                    .workspace_pane_root(&session_id)
                    .map(|root| root.session_ids())
                    .unwrap_or_else(|| vec![session_id.clone()]);
                let is_disconnected = leaf_ids.iter().any(|id| self.session.is_disconnected(id));
                let title = self.session.display_name_by_info(&session);
                let is_locked = self.tab_tree_is_locked(&session_id);
                let active_pane = self.active_pane_for_tab_root(&session_id);
                let connection = self
                    .session
                    .metadata(&active_pane)
                    .and_then(|metadata| metadata.source_connection_id.as_deref())
                    .and_then(|connection_id| {
                        self.connection_state
                            .connections()
                            .iter()
                            .find(|connection| connection.id == connection_id)
                    });
                let icon_kind = connection.map_or_else(
                    || match session.kind {
                        nyaterm_transport::SessionKind::LocalPty => "Local",
                        nyaterm_transport::SessionKind::Ssh => "SSH",
                        nyaterm_transport::SessionKind::Telnet => "Telnet",
                        nyaterm_transport::SessionKind::Serial => "Serial",
                        nyaterm_transport::SessionKind::RawTcp => "SSH",
                        nyaterm_transport::SessionKind::Rdp => "RDP",
                        nyaterm_transport::SessionKind::Vnc => "VNC",
                    },
                    SavedConnection::kind_label,
                );
                let icon = resolve_connection_icon(
                    connection
                        .and_then(|connection| connection.icon.as_deref())
                        .filter(|icon| !icon.trim().is_empty()),
                    icon_kind,
                );
                let mut tooltip_lines = self.session.tab_tooltip_lines(&active_pane);
                if is_locked {
                    tooltip_lines.push(t!("tabCtx.locked").to_string());
                }
                let tooltip_title = title.clone();
                rows = rows.child(
                    div()
                        .id(SharedString::from(format!("open-tabs-menu-{session_id}")))
                        .h(px(32.))
                        .px_3()
                        .flex()
                        .items_center()
                        .cursor_pointer()
                        .tooltip(move |_, cx| {
                            cx.new(|_| {
                                SessionTabTooltip::new(tooltip_title.clone(), tooltip_lines.clone())
                            })
                            .into()
                        })
                        .hover(move |this| this.bg(hover_bg))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.close_open_tabs_menu(cx);
                            this.select_session(session_id.clone(), cx);
                            window.focus(this.terminal.input_focus(), cx);
                        }))
                        .child(
                            div()
                                .w(px(20.))
                                .h_full()
                                .flex_none()
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(if is_active {
                                    svg()
                                        .size(px(13.))
                                        .path("icons/check.svg")
                                        .text_color(rgb(palette.primary))
                                        .into_any_element()
                                } else {
                                    div().size(px(13.)).into_any_element()
                                }),
                        )
                        .child(
                            div()
                                .size(px(16.))
                                .ml(px(2.))
                                .flex_none()
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(themed_icon(palette, icon, false, 14.)),
                        )
                        .child(
                            div()
                                .ml(px(8.))
                                .min_w_0()
                                .flex_1()
                                .text_size(px(12.))
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .child(
                                    div()
                                        .flex_none()
                                        .text_color(rgb(palette.text_dimmed))
                                        .child(
                                            ordinals
                                                .get(&session.id)
                                                .copied()
                                                .unwrap_or(index + 1)
                                                .to_string(),
                                        ),
                                )
                                .child(
                                    div()
                                        .min_w_0()
                                        .flex_1()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .text_color(if is_disconnected {
                                            rgb(palette.text_dimmed)
                                        } else {
                                            rgb(palette.text)
                                        })
                                        .child(truncate_preview(&title, 40)),
                                ),
                        ),
                );
            }
        }
        div()
            .id("workspace-open-tabs-dropdown")
            .w(px(256.))
            .rounded_md()
            .border_1()
            .border_color(rgb(palette.border))
            .bg(self.shell_surface_color(palette.surface))
            .shadow_lg()
            .overflow_hidden()
            .child(
                div()
                    .h(px(40.))
                    .px_3()
                    .flex()
                    .items_center()
                    .text_size(px(12.))
                    .font_weight(FontWeight(600.))
                    .text_color(rgb(palette.text_muted))
                    .child(t!("terminal.openTabs")),
            )
            .child(div().h(px(1.)).bg(rgb(palette.border)))
            .child(rows)
    }

    pub(in crate::features) fn render_new_session_menu(
        &mut self,
        menu_id_suffix: &str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.theme_palette();
        let hover_bg = self.shell_surface_color(palette.hover);
        let new_session_label = t!("terminal.newSession");
        let all_sessions_label = t!("terminal.allSessions");
        let shell_sessions_label = t!("terminal.shellSessions");
        let no_shell_sessions_label = t!("terminal.noShellSessions");
        let recent_sessions_label = t!("terminal.recentSessions");
        let no_recent_sessions_label = t!("terminal.noRecentSessions");
        let all_sessions_open = self.shell.new_session_all_sessions_is_open();
        let shell = new_session_shell_connections(self.connection_state.connections());
        let recent = new_session_recent_connections(self.connection_state.connections());
        let menu_max_height = (self.shell.viewport_size().1 - 96.).clamp(240., 640.);
        let menu_id_suffix = menu_id_suffix.to_string();
        let visible_group_ids = new_session_visible_group_ids(
            self.connection_state.connections(),
            self.connection_state.groups(),
        );
        let all_sessions_content = self.render_new_session_all_sessions_level(
            None,
            &visible_group_ids,
            0,
            &menu_id_suffix,
            cx,
        );
        let group_menu_path_empty = self.shell.new_session_group_menu_path().is_empty();
        let all_sessions_trigger = div()
            .id(SharedString::from(format!(
                "new-session-connections-{menu_id_suffix}"
            )))
            .h(px(32.))
            .px_3()
            .flex()
            .items_center()
            .gap_2()
            .cursor_pointer()
            .bg(if all_sessions_open {
                self.shell_surface_color(palette.hover)
            } else {
                rgba(0x00000000)
            })
            .hover(move |this| this.bg(hover_bg))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if *hovered {
                    this.open_new_session_all_sessions_menu(cx);
                }
            }))
            .child(
                svg()
                    .size(px(12.))
                    .path("icons/connections.svg")
                    .text_color(rgb(palette.text_muted)),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .text_size(px(12.))
                    .text_color(rgb(palette.text))
                    .child(all_sessions_label),
            )
            .child(
                svg()
                    .size(px(12.))
                    .path("icons/fe/forward.svg")
                    .text_color(rgb(palette.text_dimmed)),
            );
        let app = cx.weak_entity();
        let all_sessions_popover = NyaPopover::new(
            SharedString::from(format!("new-session-all-sessions-popover-{menu_id_suffix}")),
            all_sessions_trigger,
            all_sessions_content,
        )
        .placement(NyaPopoverPlacement::Right)
        .align(NyaPopoverAlign::Start)
        .offset(px(4.))
        .appearance(false)
        .overlay_closable(group_menu_path_empty)
        .open(all_sessions_open)
        .on_open_change(cx.listener(|this, open, _, cx| {
            if *open {
                this.open_new_session_all_sessions_menu(cx);
            } else {
                this.close_new_session_all_sessions_menu(cx);
            }
        }))
        .on_click_outside(move |_, cx| {
            _ = app.update(cx, |this, cx| this.close_new_session_menu(cx));
        });

        let mut menu = div()
            .py_1()
            .flex()
            .flex_col()
            .child(
                div()
                    .id(SharedString::from(format!(
                        "new-session-new-{menu_id_suffix}"
                    )))
                    .h(px(32.))
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .hover(move |this| this.bg(hover_bg))
                    .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                        if *hovered {
                            this.close_new_session_all_sessions_menu(cx);
                        }
                    }))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.close_new_session_menu(cx);
                        this.open_connection_editor(None, None, false, window, cx);
                    }))
                    .child(
                        svg()
                            .size(px(12.))
                            .path("icons/conn/add.svg")
                            .text_color(rgb(palette.link)),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(palette.text))
                            .child(new_session_label),
                    ),
            )
            .child(all_sessions_popover);

        menu = menu
            .child(div().mx_2().my_1().h(px(1.)).bg(rgb(palette.border)))
            .child(
                div()
                    .px_3()
                    .py_1()
                    .text_size(px(10.))
                    .font_weight(FontWeight(700.))
                    .text_color(rgb(palette.text_dimmed))
                    .child(shell_sessions_label),
            );
        if shell.is_empty() {
            menu = menu.child(
                div()
                    .px_3()
                    .py_1()
                    .text_size(px(11.))
                    .text_color(rgb(palette.text_dimmed))
                    .child(no_shell_sessions_label),
            );
        } else {
            for connection in shell {
                let label = connection.name.clone();
                menu = menu.child(self.new_session_connection_row(
                    palette,
                    connection,
                    label,
                    &menu_id_suffix,
                    cx,
                ));
            }
        }

        menu = menu
            .child(div().mx_2().my_1().h(px(1.)).bg(rgb(palette.border)))
            .child(
                div()
                    .px_3()
                    .py_1()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(10.))
                    .font_weight(FontWeight(700.))
                    .text_color(rgb(palette.text_dimmed))
                    .child(
                        svg()
                            .size(px(12.))
                            .path("icons/history.svg")
                            .text_color(rgb(palette.text_dimmed)),
                    )
                    .child(recent_sessions_label),
            );
        if recent.is_empty() {
            menu = menu.child(
                div()
                    .px_3()
                    .py_1()
                    .text_size(px(11.))
                    .text_color(rgb(palette.text_dimmed))
                    .child(no_recent_sessions_label),
            );
        } else {
            for connection in recent {
                let label = new_session_recent_label(&connection, self.connection_state.groups());
                menu = menu.child(self.new_session_connection_row(
                    palette,
                    connection,
                    label,
                    &menu_id_suffix,
                    cx,
                ));
            }
        }
        div()
            .id(SharedString::from(format!(
                "workspace-new-session-dropdown-{menu_id_suffix}"
            )))
            .w(px(NEW_SESSION_MENU_WIDTH))
            .rounded_md()
            .border_1()
            .border_color(rgb(palette.border))
            .bg(self.shell_surface_color(palette.surface))
            .shadow_lg()
            .overflow_hidden()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.stop_propagation()),
            )
            .child(
                NyaScrollArea::new(SharedString::from(format!(
                    "workspace-new-session-dropdown-scroll-{menu_id_suffix}"
                )))
                .max_h(px(menu_max_height))
                .child(menu),
            )
    }

    fn render_new_session_all_sessions_level(
        &mut self,
        parent_group_id: Option<String>,
        visible_group_ids: &HashSet<String>,
        depth: usize,
        menu_id_suffix: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = self.theme_palette();
        let groups = new_session_groups_for_parent(
            self.connection_state.groups(),
            visible_group_ids,
            parent_group_id.as_deref(),
        );
        let connections = new_session_connections_for_group(
            self.connection_state.connections(),
            self.connection_state.groups(),
            parent_group_id.as_deref(),
        );
        let has_groups = !groups.is_empty();
        let no_saved_sessions_label = t!("terminal.noSavedSessions");
        let selected_group_id = self.shell.new_session_group_menu_path().get(depth).cloned();
        let has_open_descendant = self.shell.new_session_group_menu_path().len() > depth + 1;
        let submenu_max_height = (self.shell.viewport_size().1 - 32.).clamp(160., 420.);
        let mut menu = div().py_1().flex().flex_col();

        if groups.is_empty() && connections.is_empty() {
            menu = menu.child(
                div()
                    .h(px(NEW_SESSION_MENU_ROW_HEIGHT))
                    .px_3()
                    .flex()
                    .items_center()
                    .text_size(px(11.))
                    .text_color(rgb(palette.text_dimmed))
                    .child(no_saved_sessions_label),
            );
        } else {
            for group in groups {
                let group_id = group.id.clone();
                let selected = selected_group_id.as_deref() == Some(group_id.as_str());
                let child_menu = if selected {
                    self.render_new_session_all_sessions_level(
                        Some(group_id.clone()),
                        visible_group_ids,
                        depth + 1,
                        menu_id_suffix,
                        cx,
                    )
                } else {
                    div().into_any_element()
                };
                let row = self.new_session_group_menu_row(
                    palette,
                    group,
                    selected,
                    depth,
                    menu_id_suffix,
                    cx,
                );
                let popover_group_id = group_id.clone();
                let app = cx.weak_entity();
                menu = menu.child(
                    NyaPopover::new(
                        SharedString::from(format!(
                            "new-session-group-popover-{menu_id_suffix}-{depth}-{group_id}"
                        )),
                        row,
                        child_menu,
                    )
                    .placement(NyaPopoverPlacement::Right)
                    .align(NyaPopoverAlign::Start)
                    .offset(px(4.))
                    .appearance(false)
                    .overlay_closable(!has_open_descendant)
                    .open(selected)
                    .on_open_change(cx.listener(move |this, open, _, cx| {
                        if *open {
                            this.open_new_session_group_menu(popover_group_id.clone(), depth, cx);
                        } else {
                            this.truncate_new_session_group_menu(depth, cx);
                        }
                    }))
                    .on_click_outside(move |_, cx| {
                        _ = app.update(cx, |this, cx| this.close_new_session_menu(cx));
                    }),
                );
            }
            if has_groups && !connections.is_empty() {
                menu = menu.child(div().mx_2().my_1().h(px(1.)).bg(rgb(palette.border)));
            }
            for connection in connections {
                menu = menu.child(self.new_session_all_sessions_connection_row(
                    palette,
                    connection,
                    depth,
                    menu_id_suffix,
                    cx,
                ));
            }
        }

        div()
            .id(SharedString::from(format!(
                "new-session-all-sessions-level-{menu_id_suffix}-{depth}"
            )))
            .w(px(NEW_SESSION_SUBMENU_WIDTH))
            .rounded_md()
            .border_1()
            .border_color(rgb(palette.border))
            .bg(self.shell_surface_color(palette.surface))
            .shadow_lg()
            .overflow_hidden()
            .child(
                NyaScrollArea::new(SharedString::from(format!(
                    "new-session-all-sessions-scroll-{menu_id_suffix}-{depth}"
                )))
                .max_h(px(submenu_max_height))
                .child(menu),
            )
            .into_any_element()
    }

    fn new_session_group_menu_row(
        &mut self,
        palette: ThemePalette,
        group: Group,
        selected: bool,
        depth: usize,
        menu_id_suffix: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let hover_bg = self.shell_surface_color(palette.hover);
        let group_id = group.id.clone();
        let hover_group_id = group_id.clone();
        div()
            .id(SharedString::from(format!(
                "new-session-group-{menu_id_suffix}-{depth}-{group_id}"
            )))
            .h(px(NEW_SESSION_MENU_ROW_HEIGHT))
            .px_3()
            .flex()
            .items_center()
            .gap_2()
            .bg(if selected {
                self.shell_surface_color(palette.hover)
            } else {
                rgba(0x00000000)
            })
            .cursor_pointer()
            .hover(move |this| this.bg(hover_bg))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered {
                    this.open_new_session_group_menu(hover_group_id.clone(), depth, cx);
                }
            }))
            .child(
                svg()
                    .size(px(13.))
                    .path("icons/conn/folder.svg")
                    .text_color(rgb(palette.warning)),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .overflow_hidden()
                    .text_size(px(12.))
                    .text_color(rgb(palette.text))
                    .child(truncate_preview(&group.name, 30)),
            )
            .child(
                svg()
                    .size(px(12.))
                    .path("icons/fe/forward.svg")
                    .text_color(rgb(palette.text_dimmed)),
            )
            .into_any_element()
    }

    fn new_session_all_sessions_connection_row(
        &mut self,
        palette: ThemePalette,
        connection: SavedConnection,
        depth: usize,
        menu_id_suffix: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let hover_bg = self.shell_surface_color(palette.hover);
        let connection_id = connection.id.clone();
        let icon = resolve_connection_icon(
            connection
                .icon
                .as_deref()
                .filter(|icon| !icon.trim().is_empty()),
            connection.kind_label(),
        );
        let label = connection.name.clone();
        div()
            .id(SharedString::from(format!(
                "new-session-all-connection-{menu_id_suffix}-{connection_id}"
            )))
            .h(px(NEW_SESSION_MENU_ROW_HEIGHT))
            .px_3()
            .flex()
            .items_center()
            .gap_2()
            .cursor_pointer()
            .hover(move |this| this.bg(hover_bg))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered {
                    this.truncate_new_session_group_menu(depth, cx);
                }
            }))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.close_new_session_menu(cx);
                if let Some(connection) = this
                    .connection_state
                    .connections()
                    .iter()
                    .find(|item| item.id == connection_id)
                    .cloned()
                {
                    this.start_saved_connection(connection, window, cx);
                }
            }))
            .child(
                div()
                    .size(px(14.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(themed_icon(palette, icon, false, 14.)),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_size(px(12.))
                    .text_color(rgb(palette.text))
                    .child(label),
            )
            .into_any_element()
    }

    fn open_new_session_all_sessions_menu(&mut self, cx: &mut Context<Self>) {
        if self.shell.open_new_session_all_sessions() {
            cx.notify();
        }
    }

    fn close_new_session_all_sessions_menu(&mut self, cx: &mut Context<Self>) {
        if self.shell.close_new_session_all_sessions() {
            cx.notify();
        }
    }

    fn open_new_session_group_menu(
        &mut self,
        group_id: String,
        depth: usize,
        cx: &mut Context<Self>,
    ) {
        if self.shell.open_new_session_group(group_id, depth) {
            cx.notify();
        }
    }

    fn truncate_new_session_group_menu(&mut self, depth: usize, cx: &mut Context<Self>) {
        if self.shell.truncate_new_session_group_path(depth) {
            cx.notify();
        }
    }

    pub(in crate::features) fn new_session_connection_row(
        &mut self,
        palette: ThemePalette,
        connection: SavedConnection,
        label: String,
        menu_id_suffix: &str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let hover_bg = self.shell_surface_color(palette.hover);
        let connection_id = connection.id.clone();
        let icon = resolve_connection_icon(
            connection
                .icon
                .as_deref()
                .filter(|icon| !icon.trim().is_empty()),
            connection.kind_label(),
        );
        div()
            .id(SharedString::from(format!(
                "new-session-conn-{menu_id_suffix}-{connection_id}"
            )))
            .h(px(32.))
            .px_3()
            .flex()
            .items_center()
            .gap_2()
            .cursor_pointer()
            .hover(move |this| this.bg(hover_bg))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if *hovered {
                    this.close_new_session_all_sessions_menu(cx);
                }
            }))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.close_new_session_menu(cx);
                if let Some(connection) = this
                    .connection_state
                    .connections()
                    .iter()
                    .find(|item| item.id == connection_id)
                    .cloned()
                {
                    this.start_saved_connection(connection, window, cx);
                }
            }))
            .child(
                div()
                    .size(px(14.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(themed_icon(palette, icon, false, 14.)),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_size(px(12.))
                    .text_color(rgb(palette.text))
                    .child(label),
            )
    }
}

#[cfg(test)]
mod tests {
    use nyaterm_core::{ConnectionType, Group, SavedConnection};

    use super::{
        new_session_connection_protocol, new_session_connections_for_group,
        new_session_groups_for_parent, new_session_recent_connections, new_session_recent_label,
        new_session_shell_connections, new_session_visible_group_ids,
    };

    fn group(id: &str, name: &str, parent_id: Option<&str>, sort_order: i32) -> Group {
        Group {
            id: id.to_string(),
            name: name.to_string(),
            parent_id: parent_id.map(str::to_string),
            sort_order,
            created_at_ms: None,
            updated_at_ms: None,
        }
    }

    fn connection(
        id: &str,
        name: &str,
        group_id: Option<&str>,
        sort_order: i32,
    ) -> SavedConnection {
        SavedConnection {
            extensions: Default::default(),
            id: id.to_string(),
            name: name.to_string(),
            config: ConnectionType::LocalTerminal {
                shell_path: String::new(),
                shell_args: String::new(),
                working_dir: None,
                ai_execution_profile: Default::default(),
                encoding: String::new(),
                dynamic_tab_title: false,
            },
            group_id: group_id.map(str::to_string),
            description: None,
            sort_order,
            icon: None,
            icon_auto_detect: None,
            auth: None,
            recording: None,
            ssh_algorithms: None,
            ssh_profile: Default::default(),
            terminal_type: None,
            sftp: Default::default(),
            network: None,
            post_login: None,
            asset: None,
            created_at_ms: None,
            updated_at_ms: None,
            last_used_at_ms: None,
        }
    }

    fn ssh_connection(id: &str, name: &str, group_id: Option<&str>) -> SavedConnection {
        let mut connection = connection(id, name, group_id, 0);
        connection.config = ConnectionType::Ssh {
            host: "example.test".to_string(),
            port: 22,
            username: "root".to_string(),
            backspace_mode: "auto".to_string(),
            ai_execution_profile: Default::default(),
            x11_forwarding: false,
            auth_agent_endpoint: None,
            agent_forwarding_config: None,
            legacy_agent_forwarding: None,
            encoding: String::new(),
            dynamic_tab_title: false,
        };
        connection
    }

    #[test]
    fn new_session_groups_follow_parent_and_sort_order() {
        let groups = vec![
            group("child-b", "Child B", Some("root-a"), 3),
            group("root-b", "Root B", None, 4),
            group("root-a", "Root A", None, 1),
            group("child-a", "Child A", Some("root-a"), 2),
            group("orphan", "Orphan", Some("missing"), 0),
        ];
        let connections = vec![
            connection("root-a-connection", "Root A", Some("root-a"), 0),
            connection("child-a-connection", "Child A", Some("child-a"), 0),
            connection("child-b-connection", "Child B", Some("child-b"), 0),
            connection("orphan-connection", "Orphan", Some("orphan"), 0),
        ];
        let visible = new_session_visible_group_ids(&connections, &groups);

        let roots = new_session_groups_for_parent(&groups, &visible, None);
        assert_eq!(
            roots
                .iter()
                .map(|group| group.id.as_str())
                .collect::<Vec<_>>(),
            vec!["orphan", "root-a"]
        );
        let children = new_session_groups_for_parent(&groups, &visible, Some("root-a"));
        assert_eq!(
            children
                .iter()
                .map(|group| group.id.as_str())
                .collect::<Vec<_>>(),
            vec!["child-a", "child-b"]
        );
    }

    #[test]
    fn new_session_groups_hide_empty_branches() {
        let groups = vec![
            group("parent", "Parent", None, 0),
            group("populated", "Populated", Some("parent"), 0),
            group("empty", "Empty", Some("parent"), 1),
            group("empty-root", "Empty Root", None, 1),
        ];
        let connections = vec![connection(
            "nested-connection",
            "Nested",
            Some("populated"),
            0,
        )];
        let visible = new_session_visible_group_ids(&connections, &groups);

        let roots = new_session_groups_for_parent(&groups, &visible, None);
        assert_eq!(
            roots
                .iter()
                .map(|group| group.id.as_str())
                .collect::<Vec<_>>(),
            vec!["parent"]
        );
        let children = new_session_groups_for_parent(&groups, &visible, Some("parent"));
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].id, "populated");
    }

    #[test]
    fn new_session_connections_include_invalid_groups_at_root() {
        let groups = vec![group("group-a", "Group A", None, 0)];
        let connections = vec![
            connection("grouped", "Grouped", Some("group-a"), 0),
            connection("root-b", "Root B", None, 4),
            connection("orphan", "Orphan", Some("missing"), 1),
            connection("root-a", "Root A", None, 1),
        ];

        let root = new_session_connections_for_group(&connections, &groups, None);
        assert_eq!(
            root.iter()
                .map(|connection| connection.id.as_str())
                .collect::<Vec<_>>(),
            vec!["orphan", "root-a", "root-b"]
        );
        let grouped = new_session_connections_for_group(&connections, &groups, Some("group-a"));
        assert_eq!(grouped.len(), 1);
        assert_eq!(grouped[0].id, "grouped");
    }

    #[test]
    fn shell_connections_follow_configured_order_with_stable_ties() {
        let mut remote = ssh_connection("remote", "Remote", None);
        remote.sort_order = -1;
        let connections = vec![
            connection("z", "Zulu", None, 2),
            remote,
            connection("b", "Beta", None, 1),
            connection("a", "Alpha", None, 1),
        ];

        let shell = new_session_shell_connections(&connections);
        assert_eq!(
            shell
                .iter()
                .map(|connection| connection.id.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "b", "z"]
        );
    }

    #[test]
    fn recent_connections_use_last_used_order_limit_and_no_fallback() {
        let unused = vec![connection("unused", "Unused", None, 0)];
        assert!(new_session_recent_connections(&unused).is_empty());

        let mut connections = (0..12)
            .map(|index| {
                let mut item = connection(
                    &format!("id-{index}"),
                    &format!("Connection {index}"),
                    None,
                    index,
                );
                item.last_used_at_ms = Some(index as u64 + 1);
                item
            })
            .collect::<Vec<_>>();
        connections.reverse();

        let recent = new_session_recent_connections(&connections);
        assert_eq!(recent.len(), 10);
        assert_eq!(recent.first().map(|item| item.id.as_str()), Some("id-11"));
        assert_eq!(recent.last().map(|item| item.id.as_str()), Some("id-2"));
    }

    #[test]
    fn recent_labels_include_protocol_and_cycle_safe_group_path() {
        let groups = vec![
            group("a", "A", Some("b"), 0),
            group("b", "B", Some("a"), 0),
            group("orphan", "Orphan", Some("missing"), 0),
        ];
        let local = connection("local", "PowerShell", Some("a"), 0);
        let ssh = ssh_connection("ssh", "Server", Some("orphan"));

        assert_eq!(new_session_connection_protocol(&local), "shell");
        assert_eq!(new_session_connection_protocol(&ssh), "ssh");
        assert_eq!(
            new_session_recent_label(&local, &groups),
            "shell://B/A/PowerShell"
        );
        assert_eq!(
            new_session_recent_label(&ssh, &groups),
            "ssh://Orphan/Server"
        );
    }
}
