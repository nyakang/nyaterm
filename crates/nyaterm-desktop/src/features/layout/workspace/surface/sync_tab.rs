use gpui::{
    AnyElement, Context, IntoElement, MouseButton, SharedString, div, prelude::*, px, rgb, svg,
};
use nyaterm_ui::{NyaContextMenu, NyaIconButton, NyaMenuItem};
use rust_i18n::t;

use crate::features::NyaTermApp;
use crate::features::sync_input::tab_state::{TabSyncPane, TabSyncState};

const SYNC_ICON: &str = "icons/session/sync-input.svg";

#[derive(Clone)]
enum SyncTabAction {
    Receive(bool),
    Pause { group: String, was_paused: bool },
    Leave { group: String },
}

struct SyncTabTarget {
    root: String,
    ids: Vec<String>,
    broadcast: bool,
    revision: u64,
    selected_group: Option<String>,
}

fn enabled_label(enabled: bool) -> String {
    if enabled {
        t!("syncTab.enabled")
    } else {
        t!("syncTab.disabled")
    }
    .to_string()
}

impl NyaTermApp {
    fn sync_tab_terminal_ids(&self, root: &str) -> Vec<String> {
        self.shell
            .workspace_pane_root(root)
            .map(|tree| tree.session_ids())
            .unwrap_or_else(|| vec![root.to_string()])
            .into_iter()
            .filter(|id| self.terminal_sync_session(id))
            .collect()
    }

    fn sync_tab_tooltip(&self, state: &TabSyncState) -> String {
        let mut lines = Vec::new();
        if state.broadcast {
            lines.push(t!("menu.broadcastToAll").to_string());
            lines.push(t!("syncTab.broadcastNote").to_string());
        } else if state.ungrouped() {
            lines.push(t!("syncTab.ungrouped").to_string());
        } else if state.mixed_membership() {
            lines.push(t!("syncTab.mixedGroups").to_string());
        } else if let Some(group) = state.common_group() {
            lines.push(
                t!(
                    "syncTab.group",
                    name = &group.name,
                    state = enabled_label(true)
                )
                .to_string(),
            );
        }
        let receive = if state.mixed_receive() {
            t!("syncTab.partial").to_string()
        } else {
            enabled_label(state.all_receive())
        };
        lines.push(t!("syncTab.receive", state = receive).to_string());
        if state.panes.len() > 1 {
            lines.push(
                t!(
                    "syncTab.count",
                    count = state.receive_count(),
                    total = state.panes.len()
                )
                .to_string(),
            );
        }
        for (index, pane) in state.panes.iter().enumerate() {
            if state.panes.len() > 1 {
                lines.push(String::new());
                lines.push(
                    t!(
                        "syncTab.pane",
                        index = index + 1,
                        name = self.session.display_name(&pane.id).unwrap_or_default()
                    )
                    .to_string(),
                );
                lines.push(t!("syncTab.receive", state = enabled_label(pane.receives)).to_string());
            }
            lines.push(
                t!(
                    "syncTab.send",
                    state = enabled_label(pane.sends && !self.session.is_disconnected(&pane.id))
                )
                .to_string(),
            );
            if self.session.is_disconnected(&pane.id) {
                lines.push(t!("syncTab.disconnected").to_string());
            }
            if pane.groups.is_empty() && !state.broadcast && state.panes.len() > 1 {
                lines.push(t!("syncTab.ungrouped").to_string());
            }
            for group in &pane.groups {
                if state.broadcast
                    || state.panes.len() > 1
                    || state
                        .common_group()
                        .is_none_or(|common| common.id != group.id)
                {
                    lines.push(
                        t!(
                            "syncTab.group",
                            name = &group.name,
                            state = enabled_label(group.enabled)
                        )
                        .to_string(),
                    );
                }
                if group.paused {
                    lines.push(t!("syncTab.pausedIn", name = &group.name).to_string());
                }
            }
        }
        lines.push(String::new());
        lines.push(
            if state.all_receive() {
                t!("syncTab.clickDisable")
            } else if state.ungrouped() {
                t!("syncTab.clickJoin")
            } else {
                t!("syncTab.clickEnable")
            }
            .to_string(),
        );
        lines.join("\n")
    }

    pub(super) fn sync_input_tab_button(
        &self,
        root: String,
        state: TabSyncState,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = self.theme_palette();
        let color = if state.receive_count() == 0 {
            palette.text_dimmed
        } else if state.broadcast {
            palette.link
        } else {
            state
                .common_group()
                .map_or(palette.text_muted, |group| group.color)
        };
        let icon = if state.ungrouped() {
            "icons/plus.svg"
        } else if state.receive_count() == 0 {
            "icons/session/sync-input-off.svg"
        } else {
            SYNC_ICON
        };
        let glyph = div()
            .size(px(20.))
            .relative()
            .flex()
            .items_center()
            .justify_center()
            .child(svg().size(px(14.)).path(icon).text_color(rgb(color)))
            .when(state.mixed_receive(), |this| {
                this.child(
                    svg()
                        .absolute()
                        .top_0()
                        .right_0()
                        .size(px(6.))
                        .path("icons/minus.svg")
                        .text_color(rgb(palette.text)),
                )
            })
            .when(state.mixed_membership(), |this| {
                this.child(
                    svg()
                        .absolute()
                        .bottom_0()
                        .left_0()
                        .size(px(6.))
                        .path("icons/session/sync-input-groups.svg")
                        .text_color(rgb(palette.text_muted)),
                )
            })
            .when(!state.broadcast && state.has_group_pause(), |this| {
                this.child(
                    svg()
                        .absolute()
                        .bottom_0()
                        .right_0()
                        .size(px(6.))
                        .path("icons/transfer/pause.svg")
                        .text_color(rgb(palette.warning)),
                )
            });
        let click_root = root.clone();
        let button = div()
            .id(SharedString::from(format!(
                "session-tab-sync-receive-{root}"
            )))
            .debug_selector(|| "session-tab-sync-receive".to_string())
            .size(px(24.))
            .flex_none()
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Middle, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .on_click(|_, _, cx| cx.stop_propagation())
            .child(
                NyaIconButton::new(format!("session-tab-sync-receive-box-{root}"), icon)
                    .size(px(24.))
                    .radius(px(4.))
                    .content(glyph)
                    .when(!state.mixed_receive(), |button| {
                        button.toggled(state.all_receive())
                    })
                    .tooltip(self.sync_tab_tooltip(&state))
                    .on_click(cx.listener(move |app, event: &gpui::ClickEvent, _, cx| {
                        cx.stop_propagation();
                        if let gpui::ClickEvent::Mouse(mouse) = event
                            && (mouse.down.button != MouseButton::Left
                                || mouse.up.button != MouseButton::Left)
                        {
                            return;
                        }
                        let ids = app.sync_tab_terminal_ids(&click_root);
                        if !app.sync_input.input_selection_active() {
                            return;
                        }
                        let (all, _) = app.sync_input.tab_receive_state(&ids);
                        app.set_sessions_receive_sync_input(&ids, !all, cx);
                    })),
            );
        let app = cx.weak_entity();
        NyaContextMenu::new_dynamic(button, move |_, cx| {
            app.update(cx, |app, cx| app.sync_tab_menu(&root, cx))
                .unwrap_or_default()
        })
        .into_any_element()
    }

    fn sync_tab_action_item(
        &self,
        scope: (&str, Vec<String>),
        broadcast: bool,
        action: SyncTabAction,
        label: String,
        cx: &Context<Self>,
    ) -> NyaMenuItem {
        let target = SyncTabTarget {
            root: scope.0.to_string(),
            ids: scope.1,
            broadcast,
            revision: self.sync_input.revision(),
            selected_group: self
                .sync_input
                .selected_group()
                .map(|group| group.id.clone()),
        };
        let app = cx.weak_entity();
        NyaMenuItem::action(label).on_click(move |_, _, cx| {
            let _ = app.update(cx, |app, cx| {
                app.apply_sync_tab_action(&target, &action, cx)
            });
        })
    }

    fn apply_sync_tab_action(
        &mut self,
        target: &SyncTabTarget,
        action: &SyncTabAction,
        cx: &mut Context<Self>,
    ) {
        // Freeze scope at menu construction. A routing change invalidates the menu,
        // including a changed join destination or a group that was removed/recreated.
        let current_ids = self.sync_tab_terminal_ids(&target.root);
        if !self.sync_input.input_selection_active()
            || self.sync_input.revision() != target.revision
            || self.sync_input.broadcast_to_all() != target.broadcast
            || self.sync_input.selected_group().map(|group| &group.id)
                != target.selected_group.as_ref()
            || self
                .shell
                .workspace_tab_owner(&target.root)
                .is_some_and(|owner| owner != target.root)
            || target.ids.iter().any(|id| !current_ids.contains(id))
        {
            return;
        }
        match action {
            SyncTabAction::Receive(receive) => {
                self.set_sessions_receive_sync_input(&target.ids, *receive, cx);
            }
            SyncTabAction::Pause { group, was_paused } => {
                if let Some(id) = target.ids.first()
                    && self
                        .sync_input
                        .active_group_for_session(id)
                        .is_some_and(|active| &active.id == group)
                    && self.sync_input.session_is_paused_in_active_group(id) == *was_paused
                {
                    self.toggle_session_paused_in_active_sync_group(id.clone(), cx);
                }
            }
            SyncTabAction::Leave { group } => {
                if let Some(id) = target.ids.first()
                    && self
                        .sync_input
                        .active_group_for_session(id)
                        .is_some_and(|active| &active.id == group)
                {
                    self.leave_active_sync_group(id.clone(), cx);
                }
            }
        }
    }

    fn sync_tab_pane_menu(
        &self,
        root: &str,
        pane: &TabSyncPane,
        cx: &Context<Self>,
    ) -> Vec<NyaMenuItem> {
        let Some(group) = pane.groups.iter().find(|group| group.enabled) else {
            return vec![self.sync_tab_action_item(
                (root, vec![pane.id.clone()]),
                false,
                SyncTabAction::Receive(true),
                t!("syncTab.join").to_string(),
                cx,
            )];
        };
        vec![
            NyaMenuItem::label(group.name.clone())
                .icon(SYNC_ICON)
                .icon_color(group.color),
            self.sync_tab_action_item(
                (root, vec![pane.id.clone()]),
                false,
                SyncTabAction::Pause {
                    group: group.id.clone(),
                    was_paused: group.paused,
                },
                if group.paused {
                    t!("syncTab.resumeGroup", name = &group.name)
                } else {
                    t!("syncTab.pauseGroup", name = &group.name)
                }
                .to_string(),
                cx,
            ),
            self.sync_tab_action_item(
                (root, vec![pane.id.clone()]),
                false,
                SyncTabAction::Leave {
                    group: group.id.clone(),
                },
                t!("syncTab.leaveGroup", name = &group.name).to_string(),
                cx,
            ),
        ]
    }

    fn sync_tab_menu(&self, root: &str, cx: &Context<Self>) -> Vec<NyaMenuItem> {
        let ids = self.sync_tab_terminal_ids(root);
        let Some(state) = self.sync_input.tab_sync_state(&ids) else {
            return Vec::new();
        };
        let mut items = vec![
            self.sync_tab_action_item(
                (root, ids),
                state.broadcast,
                SyncTabAction::Receive(!state.all_receive()),
                if state.all_receive() {
                    t!("syncTab.disableReceive")
                } else {
                    t!("syncTab.enableReceive")
                }
                .to_string(),
                cx,
            )
            .checked(state.all_receive()),
        ];
        if state.broadcast {
            items.insert(
                0,
                NyaMenuItem::label(t!("menu.broadcastToAll").to_string())
                    .icon(SYNC_ICON)
                    .icon_color(self.theme_palette().link),
            );
        } else if state.panes.len() == 1 {
            items.extend(self.sync_tab_pane_menu(root, &state.panes[0], cx));
        } else {
            for (index, pane) in state.panes.iter().enumerate() {
                items.push(NyaMenuItem::submenu(
                    t!(
                        "syncTab.pane",
                        index = index + 1,
                        name = self.session.display_name(&pane.id).unwrap_or_default()
                    )
                    .to_string(),
                    self.sync_tab_pane_menu(root, pane, cx),
                ));
            }
        }
        items.push(NyaMenuItem::separator());
        let app = cx.weak_entity();
        items.push(
            NyaMenuItem::action(t!("syncTab.manage").to_string()).on_click(move |_, window, cx| {
                let _ = app.update(cx, |app, cx| app.open_sync_groups(window, cx));
            }),
        );
        items
    }
}

#[cfg(test)]
mod tests {
    use super::{SyncTabAction, SyncTabTarget};
    use crate::features::NyaTermApp;
    use crate::features::test_support::app_with_visible_local_session;
    use crate::models::{WorkspacePaneNode, WorkspaceSplitDirection};
    use crate::test_support::{TestAppCleanup, TestConfigDir};
    use gpui::{AppContext as _, Context, TestAppContext};
    use rust_i18n::t;

    fn with_split_app(test: impl FnOnce(&mut NyaTermApp, &mut Context<NyaTermApp>)) {
        let root = TestConfigDir::new("nyaterm-sync-tab-menu");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, root.path(), "root");
        cx.update_entity(&app, |app, cx| {
            let metadata = app.session.metadata("root").unwrap().clone();
            app.session.register_session_metadata("other", metadata);
            app.shell.insert_workspace_pane_root(
                "root".into(),
                WorkspacePaneNode::Split {
                    id: "split".into(),
                    direction: WorkspaceSplitDirection::Vertical,
                    ratio_percent: 50,
                    first: Box::new(WorkspacePaneNode::leaf("root")),
                    second: Box::new(WorkspacePaneNode::leaf("other")),
                },
            );
            app.sync_input.create_group();
            app.sync_input
                .add_selected_sessions(vec!["root".into(), "other".into()]);
            test(app, cx);
        });
    }

    fn target(app: &NyaTermApp, id: &str) -> SyncTabTarget {
        SyncTabTarget {
            root: "root".into(),
            ids: vec![id.into()],
            broadcast: app.sync_input.broadcast_to_all(),
            revision: app.sync_input.revision(),
            selected_group: app
                .sync_input
                .selected_group()
                .map(|group| group.id.clone()),
        }
    }

    #[test]
    fn pane_menu_pause_and_leave_never_expand_to_siblings() {
        with_split_app(|app, cx| {
            let group = app
                .sync_input
                .active_group_for_session("other")
                .unwrap()
                .id
                .clone();
            let menu = app.sync_tab_menu("root", cx);
            let submenus = menu
                .iter()
                .filter_map(|item| item.children())
                .collect::<Vec<_>>();
            assert_eq!(submenus.len(), 2);
            assert_eq!(submenus[0].len(), 3);
            assert_eq!(submenus[1].len(), 3);
            let scope = target(app, "other");
            app.apply_sync_tab_action(
                &scope,
                &SyncTabAction::Pause {
                    group: group.clone(),
                    was_paused: false,
                },
                cx,
            );
            assert!(app.sync_input.session_is_paused_in_active_group("other"));
            assert!(!app.sync_input.session_is_paused_in_active_group("root"));
            assert!(app.sync_input.session_receives_input("other"));
            let scope = target(app, "other");
            app.apply_sync_tab_action(&scope, &SyncTabAction::Leave { group }, cx);
            assert!(app.sync_input.active_group_for_session("other").is_none());
            assert!(app.sync_input.active_group_for_session("root").is_some());
        });
    }

    #[test]
    fn stale_menu_does_not_mutate_changed_routes_or_moved_panes() {
        with_split_app(|app, cx| {
            let scope = target(app, "other");
            app.sync_input.toggle_broadcast_to_all();
            app.apply_sync_tab_action(&scope, &SyncTabAction::Receive(false), cx);
            assert!(app.sync_input.session_receives_input("other"));
            let scope = target(app, "other");
            app.shell
                .insert_workspace_pane_root("root".into(), WorkspacePaneNode::leaf("root"));
            app.apply_sync_tab_action(&scope, &SyncTabAction::Receive(false), cx);
            assert!(app.sync_input.session_receives_input("other"));
        });
    }

    #[test]
    fn changing_selected_join_group_invalidates_existing_menu() {
        with_split_app(|app, cx| {
            let first = app.sync_input.selected_group().unwrap().id.clone();
            app.sync_input.leave_active_group("other");
            app.sync_input.create_group();
            let scope = target(app, "other");
            app.sync_input.select(first);
            app.apply_sync_tab_action(&scope, &SyncTabAction::Receive(true), cx);
            assert!(app.sync_input.active_group_for_session("other").is_none());
        });
    }

    #[test]
    fn broadcast_menu_omits_group_actions_and_tooltip_keeps_pauses_and_disconnects() {
        with_split_app(|app, cx| {
            app.sync_input.toggle_active_session_paused("other".into());
            app.sync_input.set_session_receives_input("root", false);
            let mut metadata = app.session.metadata("other").unwrap().clone();
            metadata.disconnected = true;
            app.session.register_session_metadata("other", metadata);
            app.sync_input.toggle_broadcast_to_all();
            let menu = app.sync_tab_menu("root", cx);
            assert_eq!(menu.len(), 4);
            assert!(menu.iter().all(|item| item.children().is_none()));
            let state = app
                .sync_input
                .tab_sync_state(&["root".into(), "other".into()])
                .unwrap();
            let tooltip = app.sync_tab_tooltip(&state);
            assert!(tooltip.contains(t!("syncTab.broadcastNote").as_ref()));
            assert!(tooltip.contains(t!("syncTab.disconnected").as_ref()));
            assert!(tooltip.contains(t!("syncTab.partial").as_ref()));
            let group = app.sync_input.active_group_for_session("other").unwrap();
            assert!(tooltip.contains(t!("syncTab.pausedIn", name = &group.name).as_ref()));
        });
    }
}
