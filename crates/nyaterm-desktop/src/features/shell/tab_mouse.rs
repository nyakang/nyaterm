use nyaterm_ui::notification::{NyaNotificationKind, NyaNotificationWindowExt as _};
use rust_i18n::t;

use gpui::{
    ClickEvent, ClipboardItem, Context, FontWeight, InteractiveElement as _, IntoElement,
    MouseButton, MouseDownEvent, ParentElement as _, Render, RenderOnce, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, prelude::FluentBuilder as _, px,
    rgb, rgba, svg,
};
use nyaterm_core::{assets::build_group_path, models::Group, truncate_preview};
use nyaterm_ui::NyaIconButton;

use crate::features::{NyaTermApp, formatting::short_id, view_widgets::mono_icon};
use crate::theme::ThemePalette;

/// A start keeps its identity across retries and registration as a formal tab.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::features) enum TabActionTarget {
    Session(String),
    Start {
        sequence: u64,
        allocated_at: std::time::Instant,
    },
}

#[derive(Clone, Debug)]
pub(in crate::features) struct WorkspaceTabDragPayload {
    pub source_workspace_id: nyaterm_core::WorkspaceId,
    pub root_tab_id: String,
    pub source_revision: u64,
    pub tab_id: super::session_tabs::SessionStripTabId,
    pub order_index: usize,
    pub source_group_id: Option<String>,
    pub display_name: String,
    pub kind_icon: &'static str,
    pub preview_background: u32,
    pub preview_border: u32,
    pub preview_text: u32,
    pub preview_text_muted: u32,
    pub preview_accent: u32,
}

pub(in crate::features) struct SessionTabDragPreview {
    payload: WorkspaceTabDragPayload,
    width: f32,
    icon: Option<Box<dyn Fn() -> gpui::AnyElement>>,
}

impl SessionTabDragPreview {
    pub(in crate::features) fn new(
        payload: WorkspaceTabDragPayload,
        _position: gpui::Point<gpui::Pixels>,
    ) -> Self {
        Self {
            payload,
            width: 188.,
            icon: None,
        }
    }

    pub(in crate::features) fn with_width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    pub(in crate::features) fn with_icon(
        mut self,
        icon: impl Fn() -> gpui::AnyElement + 'static,
    ) -> Self {
        self.icon = Some(Box::new(icon));
        self
    }
}

impl Render for SessionTabDragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        // GPUI already subtracts the actual grab offset from the pointer.
        div()
            .w(px(self.width))
            .h(px(36.))
            .pl_3()
            .pr_2()
            .flex()
            .items_center()
            .gap_2()
            .border_r_1()
            .border_color(rgb(self.payload.preview_border))
            .bg(rgb(self.payload.preview_background))
            .shadow_lg()
            .relative()
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .h(px(2.))
                    .bg(rgb(self.payload.preview_accent)),
            )
            .child(
                div()
                    .size(px(14.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(self.icon.as_ref().map_or_else(
                        || {
                            svg()
                                .size(px(14.))
                                .path(self.payload.kind_icon)
                                .text_color(rgb(self.payload.preview_accent))
                                .into_any_element()
                        },
                        |icon| icon(),
                    )),
            )
            .child(
                div()
                    .min_w(px(12.))
                    .text_size(px(11.))
                    .font_weight(FontWeight(700.))
                    .text_color(rgb(self.payload.preview_text_muted))
                    .child(format!("{}", self.payload.order_index + 1)),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .text_size(px(12.))
                    .font_weight(FontWeight(600.))
                    .text_color(rgb(self.payload.preview_text))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(truncate_preview(&self.payload.display_name, 28)),
            )
            .child(
                svg()
                    .size(px(13.))
                    .path("icons/window/close.svg")
                    .text_color(rgb(self.payload.preview_text_muted)),
            )
    }
}

/// Interactive connection summary shown when hovering a live session tab.
#[derive(IntoElement)]
pub(in crate::features) struct SessionTabHoverCard {
    session_id: String,
    group: String,
    ip: Option<String>,
    ssh_command: Option<String>,
    palette: ThemePalette,
}

impl RenderOnce for SessionTabHoverCard {
    fn render(self, _window: &mut Window, _cx: &mut gpui::App) -> impl IntoElement {
        let palette = self.palette;
        // Match Tauri's 260px tooltip content cap. The HoverCard appearance adds
        // its own horizontal padding outside this content width.
        let mut body = div().w(px(260.)).flex().flex_col().gap_2();

        body = body.child(session_tab_hover_card_row(
            palette,
            t!("tabCtx.connectionGroup").to_string(),
            self.group,
            None,
        ));
        if let Some(ip) = self.ip {
            body = body.child(session_tab_hover_card_row(
                palette,
                t!("tabCtx.ipAddress").to_string(),
                ip.clone(),
                Some((
                    format!("session-tab-card-copy-ip-{}", self.session_id),
                    t!("tabCtx.copyIp").to_string(),
                    ip,
                )),
            ));
        }
        if let Some(ssh_command) = self.ssh_command {
            body = body.child(session_tab_hover_card_row(
                palette,
                t!("tabCtx.sshCommand").to_string(),
                ssh_command.clone(),
                Some((
                    format!("session-tab-card-copy-ssh-{}", self.session_id),
                    t!("tabCtx.copySshAddress").to_string(),
                    ssh_command,
                )),
            ));
        }
        body
    }
}

fn session_tab_hover_card_row(
    palette: ThemePalette,
    label: String,
    value: String,
    copy: Option<(String, String, String)>,
) -> impl IntoElement {
    div()
        .min_w_0()
        .flex()
        .items_center()
        .gap_2()
        .child(
            div()
                .w(px(76.))
                .flex_none()
                .text_size(px(12.))
                .text_color(rgb(palette.text_muted))
                .child(label),
        )
        .child(
            div()
                .min_w_0()
                .flex_1()
                .text_size(px(12.))
                .text_color(rgb(palette.text))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(value),
        )
        .when_some(copy, |this, (id, tooltip, value)| {
            this.child(
                NyaIconButton::new(id, "icons/copy.svg")
                    .icon_size(px(13.))
                    .tooltip(tooltip)
                    .on_click(move |_, window, cx| {
                        cx.stop_propagation();
                        cx.write_to_clipboard(ClipboardItem::new_string(value.clone()));
                        window.notify_operation(
                            "copy-tab-detail",
                            NyaNotificationKind::Success,
                            t!("common.copied").to_string(),
                            cx,
                        );
                    }),
            )
        })
}

/// Hover tooltip for session tabs (Tauri TabBar host / SSH address).
pub(in crate::features) struct SessionTabTooltip {
    pub title: String,
    pub lines: Vec<String>,
}

impl SessionTabTooltip {
    pub(in crate::features) fn new(title: String, lines: Vec<String>) -> Self {
        Self { title, lines }
    }
}

impl Render for SessionTabTooltip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut body = div().flex().flex_col().gap_1().child(
            div()
                .text_size(px(12.))
                .font_weight(FontWeight(700.))
                .text_color(rgb(0xe5edf7))
                .child(self.title.clone()),
        );
        for line in &self.lines {
            let copyable = line_looks_copyable(line);
            let copy_value = line.clone();
            let row = div()
                .flex()
                .items_center()
                .gap_2()
                .min_w_0()
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .text_size(px(11.))
                        .text_color(rgb(0x8f98aa))
                        .overflow_hidden()
                        .child(line.clone()),
                )
                .when(copyable, |this| {
                    this.child(
                        div()
                            .id(SharedString::from(format!(
                                "tab-tooltip-copy-{}",
                                copy_value.chars().take(24).collect::<String>()
                            )))
                            .size(px(18.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_sm()
                            .text_size(px(10.))
                            .text_color(rgb(0x8f98aa))
                            .hover(|style| style.bg(rgb(0x334155)).text_color(rgb(0xe5edf7)))
                            .cursor_pointer()
                            .child(mono_icon("icons/copy.svg", rgb(0x8f98aa).into(), 11.))
                            .on_click(cx.listener(move |_, _, window, cx| {
                                cx.stop_propagation();
                                cx.write_to_clipboard(ClipboardItem::new_string(
                                    copy_value.clone(),
                                ));
                                window.notify_operation(
                                    "copy-tab-detail",
                                    NyaNotificationKind::Success,
                                    t!("common.copied").to_string(),
                                    cx,
                                );
                            })),
                    )
                });
            body = body.child(row);
        }
        div()
            .px_3()
            .py_2()
            .max_w(px(280.))
            .rounded_md()
            .border_1()
            .border_color(rgb(0x334155))
            .bg(rgba(0x151b24f2))
            .shadow_lg()
            .child(body)
    }
}

fn line_looks_copyable(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }
    if trimmed.starts_with("cwd ") || trimmed.starts_with("Disconnected") {
        return false;
    }
    // host, endpoint labels, ssh -p user@host
    true
}

fn session_tab_group_label(groups: &[Group], group_id: Option<&str>, ungrouped: &str) -> String {
    let path = build_group_path(groups, group_id)
        .into_iter()
        .skip(1)
        .map(|segment| segment.name)
        .collect::<Vec<_>>()
        .join(" / ");
    if path.is_empty() {
        ungrouped.to_string()
    } else {
        path
    }
}

#[derive(Debug, Clone, Copy)]
pub(in crate::features) enum TabMouseActionTarget {
    Double,
    Middle,
    Right,
}

impl NyaTermApp {
    pub(in crate::features) fn session_tab_hover_card(
        &self,
        session_id: &str,
        palette: ThemePalette,
    ) -> SessionTabHoverCard {
        let connection = self
            .session
            .metadata(session_id)
            .and_then(|metadata| metadata.source_connection_id.as_deref())
            .and_then(|connection_id| {
                self.connection_state
                    .connections()
                    .iter()
                    .find(|connection| connection.id == connection_id)
            });
        let ungrouped = t!("savedConnections.ungroupedConnections").to_string();
        let group = connection.map_or_else(
            || ungrouped.clone(),
            |connection| {
                session_tab_group_label(
                    self.connection_state.groups(),
                    connection.group_id.as_deref(),
                    &ungrouped,
                )
            },
        );
        SessionTabHoverCard {
            session_id: session_id.to_string(),
            group,
            ip: self.session.ssh_host(session_id),
            ssh_command: self.session.ssh_address(session_id),
            palette,
        }
    }

    pub(in crate::features) fn set_tab_mouse_action(
        &mut self,
        target: TabMouseActionTarget,
        action: &str,
        cx: &mut Context<Self>,
    ) {
        let action = normalize_tab_mouse_action(action);
        match target {
            TabMouseActionTarget::Double => {
                if self.settings.summary().interaction_tab_double_click_action == action {
                    return;
                }
                self.settings
                    .set_tab_double_click_action(action.to_string());
            }
            TabMouseActionTarget::Middle => {
                if self.settings.summary().interaction_tab_middle_click_action == action {
                    return;
                }
                self.settings
                    .set_tab_middle_click_action(action.to_string());
            }
            TabMouseActionTarget::Right => {
                if self.settings.summary().interaction_tab_right_click_action == action {
                    return;
                }
                self.settings.set_tab_right_click_action(action.to_string());
            }
        }
        self.save_interaction_settings(cx);
    }

    pub(in crate::features) fn handle_session_tab_click(
        &mut self,
        session_id: String,
        event: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.handle_tab_target_click(TabActionTarget::Session(session_id), event, window, cx);
    }

    pub(in crate::features) fn handle_tab_target_click(
        &mut self,
        target: TabActionTarget,
        event: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let ClickEvent::Mouse(mouse) = event
            && mouse.down.button == MouseButton::Left
            && mouse.up.button == MouseButton::Left
            && event.click_count() >= 2
        {
            let action = self
                .settings
                .summary()
                .interaction_tab_double_click_action
                .clone();
            if self.tab_target_action_enabled(&target, &action) {
                cx.stop_propagation();
                self.run_tab_target_action(target, &action, window, cx);
                return;
            }
        }
        self.select_tab_action_target(&target, window, cx);
    }

    pub(in crate::features) fn select_tab_action_target(
        &mut self,
        target: &TabActionTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some((id, _, ended)) = self.session.start_action_request(target) {
            let id = id.to_string();
            if ended.is_some() {
                self.select_failed_session_start(id, cx);
            } else {
                self.select_pending_session_start(id, cx);
            }
        } else if let Some(id) = self.session.resolved_tab_action_session(target) {
            self.select_session(id.clone(), cx);
            self.focus_terminal_session(&id, window, cx);
        }
    }

    pub(in crate::features) fn handle_session_tab_mouse_down(
        &mut self,
        session_id: String,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.handle_tab_target_mouse_down(TabActionTarget::Session(session_id), event, window, cx);
    }

    pub(in crate::features) fn handle_tab_target_mouse_down(
        &mut self,
        target: TabActionTarget,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let action = match event.button {
            MouseButton::Middle => self
                .settings
                .summary()
                .interaction_tab_middle_click_action
                .clone(),
            MouseButton::Right => self
                .settings
                .summary()
                .interaction_tab_right_click_action
                .clone(),
            _ => return,
        };
        let enabled = self.tab_target_action_enabled(&target, &action);
        if event.button == MouseButton::Right && !enabled {
            cx.stop_propagation();
            let anchor = (f32::from(event.position.x), f32::from(event.position.y));
            if self.session.start_action_request(&target).is_some() {
                self.open_start_tab_actions_at(target, anchor, window, cx);
            } else if let Some(id) = self.session.resolved_tab_action_session(&target) {
                self.open_tab_actions_at(id, Some(anchor), window, cx);
            }
        } else if enabled {
            cx.stop_propagation();
            self.run_tab_target_action(target, &action, window, cx);
        }
    }

    pub(in crate::features) fn tab_target_action_enabled(
        &self,
        target: &TabActionTarget,
        action: &str,
    ) -> bool {
        if let Some(context) = self.start_tab_action_context(target) {
            context.enabled(action)
        } else {
            self.session
                .resolved_tab_action_session(target)
                .is_some_and(|id| self.tab_mouse_action_enabled(&id, action))
        }
    }

    pub(in crate::features) fn run_tab_target_action(
        &mut self,
        target: TabActionTarget,
        action: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.tab_target_action_enabled(&target, action) {
            return;
        }
        if self.session.start_action_request(&target).is_some() {
            self.run_start_tab_mouse_action(target, action, window, cx);
        } else if let Some(id) = self.session.resolved_tab_action_session(&target) {
            self.run_tab_mouse_action(id, action.to_string(), window, cx);
        }
    }

    pub(in crate::features) fn reorder_session_relative(
        &mut self,
        dragged_session_id: String,
        target_session_id: String,
        insert_after: bool,
        cx: &mut Context<Self>,
    ) {
        if dragged_session_id == target_session_id {
            self.clear_session_tab_drag(cx);
            return;
        }
        if let Some(mut root) = self.shell.terminal_window_tree() {
            let before = if insert_after {
                root.leaf_tab_ids_for_tab(&target_session_id)
                    .and_then(|tabs| {
                        tabs.iter()
                            .position(|id| id == &target_session_id)
                            .and_then(|index| tabs.get(index + 1).cloned())
                    })
            } else {
                Some(target_session_id.clone())
            };
            let changed = if let Some(before) = before {
                root.place_tab_before(&dragged_session_id, &before)
            } else if let Some(group) = root.leaf_for_tab(&target_session_id).map(str::to_string) {
                root.move_tab_to_leaf(&dragged_session_id, &group)
            } else {
                false
            };
            if changed {
                self.shell.restore_terminal_window_tree(Some(root));
                self.select_session(dragged_session_id, cx);
                self.persist_open_tabs();
            }
            self.clear_session_tab_drag(cx);
            return;
        }
        let tabs = self.ordered_tab_sessions();
        if !tabs.iter().any(|session| session.id == target_session_id) {
            self.shell
                .set_status("drop target session no longer exists".to_string());
            self.clear_session_tab_drag(cx);
            return;
        }
        if !tabs.iter().any(|session| session.id == dragged_session_id) {
            self.shell
                .set_status("dragged session no longer exists".to_string());
            self.clear_session_tab_drag(cx);
            return;
        }
        let source_ids = self.tab_tree_session_ids(&dragged_session_id);
        let target_ids = self.tab_tree_session_ids(&target_session_id);
        if !self
            .session
            .move_session_group_relative(&source_ids, &target_ids, insert_after)
        {
            self.clear_session_tab_drag(cx);
            return;
        }
        self.session.clear_tab_drag();
        self.shell.request_session_tab_scroll_into_view();
        self.shell.set_status(format!(
            "moved tab {} {}",
            if insert_after { "after" } else { "before" },
            short_id(&target_session_id)
        ));
        if self.session.restore_is_complete() {
            self.persist_open_tabs();
        } else if self.terminal_windows_is_multi_leaf() {
            self.persist_terminal_window_layout();
        }
        cx.notify();
    }

    pub(in crate::features) fn reorder_session_to_end(
        &mut self,
        dragged_session_id: String,
        cx: &mut Context<Self>,
    ) {
        if self.shell.terminal_window_tree_is_some() {
            if let Some(last) = self.workspace_tab_ids().last().cloned() {
                self.reorder_session_relative(dragged_session_id, last, true, cx);
            } else {
                self.clear_session_tab_drag(cx);
            }
            return;
        }
        let sessions = self.ordered_tab_sessions();
        let Some(source_index) = sessions
            .iter()
            .position(|session| session.id == dragged_session_id)
        else {
            self.shell
                .set_status("dragged session no longer exists".to_string());
            self.session.clear_tab_drag();
            cx.notify();
            return;
        };
        if source_index + 1 == sessions.len() {
            self.clear_session_tab_drag(cx);
            return;
        }
        let source_ids = self.tab_tree_session_ids(&dragged_session_id);
        if !self.session.move_session_group_to_end(&source_ids) {
            self.clear_session_tab_drag(cx);
            return;
        }
        self.session.clear_tab_drag();
        self.shell.request_session_tab_scroll_into_view();
        self.shell.set_status("moved tab to end".to_string());
        if self.session.restore_is_complete() {
            self.persist_open_tabs();
        } else if self.terminal_windows_is_multi_leaf() {
            self.persist_terminal_window_layout();
        }
        cx.notify();
    }

    pub(in crate::features) fn update_session_tab_drag(
        &mut self,
        source_id: String,
        cx: &mut Context<Self>,
    ) {
        if self.session.set_tab_drag_source(source_id) {
            cx.notify();
        }
        self.ensure_drop_hover_clock(cx);
    }

    pub(in crate::features) fn clear_session_tab_drag(&mut self, cx: &mut Context<Self>) {
        self.shell.finish_tab_strip_motion();
        if self.session.clear_tab_drag() {
            cx.notify();
        }
    }

    fn tab_mouse_action_enabled(&self, session_id: &str, action: &str) -> bool {
        let tab_root = self.tab_root_for_session(session_id);
        let pane_id = self.active_pane_for_tab_root(&tab_root);
        let policy = self.tab_action_policy_for(&pane_id, &tab_root);
        match action {
            "android_screen" => policy.is_some_and(|policy| policy.availability.android_screen),
            "none" => false,
            "rename_tab" | "copy_tab_name" => true,
            "copy_server_ip" => policy.is_some_and(|policy| policy.support.copy_ssh_host),
            "duplicate_session" => policy.is_some_and(|policy| policy.availability.spawn_session),
            "multiplex_ssh" => policy.is_some_and(|policy| policy.availability.multiplex),
            "reconnect_session" => policy.is_some_and(|policy| policy.availability.reconnect),
            "disconnect_session" => policy.is_some_and(|policy| policy.availability.disconnect),
            "close_tab" => !self.tab_tree_is_locked(&tab_root),
            _ => false,
        }
    }

    fn run_tab_mouse_action(
        &mut self,
        session_id: String,
        action: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tab_root = self.tab_root_for_session(&session_id);
        if action == "close_tab" {
            self.close_session(tab_root, cx);
            return;
        }

        self.select_session(tab_root.clone(), cx);
        let Some(active_pane_id) = self.session.active_id_owned() else {
            return;
        };
        if self.tab_root_for_session(&active_pane_id) != tab_root {
            return;
        }

        match action.as_str() {
            "android_screen" => {
                let tab_root = self.tab_root_for_session(&session_id);
                let pane_id = self.active_pane_for_tab_root(&tab_root);
                self.android_open_from_session(
                    &pane_id,
                    nyaterm_core::models::android::AndroidViewKind::Screen,
                    cx,
                );
            }

            "none" => {}
            "rename_tab" => self.open_rename_session(tab_root, window, cx),
            "copy_tab_name" => self.copy_session_name(&tab_root, cx),
            "copy_server_ip" => self.copy_active_session_ssh_host(cx),
            "duplicate_session" => self.duplicate_active_session(window, cx),
            "multiplex_ssh" => self.multiplex_active_ssh_session(window, cx),
            "reconnect_session" => self.reconnect_active_session(window, cx),
            "disconnect_session" => self.disconnect_session(active_pane_id, cx),
            _ => {
                self.shell
                    .set_status(format!("unknown tab action '{action}'"));
                cx.notify();
            }
        }
    }
}

pub(in crate::features) const TAB_MOUSE_ACTIONS: [&str; 9] = [
    "none",
    "rename_tab",
    "copy_tab_name",
    "copy_server_ip",
    "duplicate_session",
    "multiplex_ssh",
    "reconnect_session",
    "disconnect_session",
    "close_tab",
];

fn normalize_tab_mouse_action(action: &str) -> &'static str {
    TAB_MOUSE_ACTIONS
        .iter()
        .copied()
        .find(|item| *item == action)
        .unwrap_or("none")
}

#[cfg(test)]
mod tests {
    use nyaterm_core::models::Group;

    use super::session_tab_group_label;

    fn group(id: &str, name: &str, parent_id: Option<&str>) -> Group {
        Group {
            id: id.to_string(),
            name: name.to_string(),
            parent_id: parent_id.map(ToOwned::to_owned),
            ..Group::default()
        }
    }

    #[test]
    fn session_tab_group_label_uses_ungrouped_for_no_group() {
        let groups = [group("production", "Production", None)];
        assert_eq!(
            session_tab_group_label(&groups, None, "Ungrouped"),
            "Ungrouped"
        );
    }

    #[test]
    fn session_tab_group_label_omits_root_for_top_level_group() {
        let groups = [group("production", "Production", None)];
        assert_eq!(
            session_tab_group_label(&groups, Some("production"), "Ungrouped"),
            "Production"
        );
    }

    #[test]
    fn session_tab_group_label_joins_only_real_ancestors() {
        let groups = [
            group("production", "Production", None),
            group("web", "Web Servers", Some("production")),
        ];
        assert_eq!(
            session_tab_group_label(&groups, Some("web"), "Ungrouped"),
            "Production / Web Servers"
        );
    }

    #[test]
    fn session_tab_group_label_uses_ungrouped_for_missing_group() {
        let groups = [group("production", "Production", None)];
        assert_eq!(
            session_tab_group_label(&groups, Some("deleted"), "Ungrouped"),
            "Ungrouped"
        );
    }
}
