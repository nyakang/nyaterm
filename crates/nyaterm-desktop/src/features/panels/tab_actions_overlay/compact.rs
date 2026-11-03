use rust_i18n::t;

use std::borrow::Cow;

use super::{
    TAB_ACTIONS_MENU_WIDTH, TabActionMenuGroup, TabActionPolicy, clamp_tab_actions_position,
    tab_actions_menu_visible_height,
};
use crate::features::NyaTermApp;
use crate::features::view_widgets::{
    tab_menu_item, tab_menu_item_enabled, tab_menu_item_with_icon, tab_menu_separator,
};
use crate::models::{StartupCommandAction, TabActionsSubmenu, WorkspaceSplitDirection};
use crate::theme::ThemePalette;
use gpui::{
    AnyElement, App, ClickEvent, Context, IntoElement, KeyDownEvent, MouseButton, SharedString,
    Window, div, prelude::*, px, rgb, rgba, svg,
};
use nyaterm_core::WorkspaceId;
use nyaterm_ui::{NyaPopover, NyaPopoverAlign, NyaPopoverPlacement, NyaScrollable};

use super::super::TAB_PRESET_COLORS;

pub(super) struct CompactTabActionsMenuState {
    pub session_id: String,
    pub tab_root_id: String,
    pub active_color: Option<u32>,
    pub locked: bool,
    pub policy: TabActionPolicy,
    pub visible_for_ai: String,
    pub buffer_for_ai: String,
}

struct TabActionsSubmenuItem {
    id: &'static str,
    icon_path: &'static str,
    label: Cow<'static, str>,
    enabled: bool,
    active: bool,
}

struct TabActionsSubmenuHandlers<OnHover, OnClick> {
    on_hover: OnHover,
    on_click: OnClick,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MoveWindowMenuDestination {
    NewWindow,
    Workspace {
        workspace_id: WorkspaceId,
        ordinal: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MoveWindowMenuItemSpec {
    id: String,
    icon_path: &'static str,
    separator_before: bool,
    destination: MoveWindowMenuDestination,
}

fn move_window_menu_items(
    targets: impl IntoIterator<Item = (WorkspaceId, usize)>,
) -> Vec<MoveWindowMenuItemSpec> {
    let mut items = vec![MoveWindowMenuItemSpec {
        id: "tab-ctx-move-new-window".to_string(),
        icon_path: "icons/menu/external.svg",
        separator_before: false,
        destination: MoveWindowMenuDestination::NewWindow,
    }];
    items.extend(
        targets
            .into_iter()
            .map(|(workspace_id, ordinal)| MoveWindowMenuItemSpec {
                id: format!("tab-ctx-move-window-{workspace_id:?}"),
                icon_path: "icons/workspace.svg",
                separator_before: false,
                destination: MoveWindowMenuDestination::Workspace {
                    workspace_id,
                    ordinal,
                },
            }),
    );
    if items.len() > 1 {
        items[1].separator_before = true;
    }
    items
}

impl NyaTermApp {
    pub(super) fn compact_tab_actions_menu(
        &mut self,
        palette: ThemePalette,
        state: CompactTabActionsMenuState,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let CompactTabActionsMenuState {
            session_id,
            tab_root_id,
            active_color,
            locked,
            policy,
            visible_for_ai,
            buffer_for_ai,
        } = state;
        let support = policy.support;
        let availability = policy.availability;
        let menu_groups = policy.menu_groups();
        let show_session_group = menu_groups.contains(&TabActionMenuGroup::Session);
        let show_split_group = menu_groups.contains(&TabActionMenuGroup::Split);
        let (viewport_w, viewport_h) = self.shell.viewport_size();
        let targets = self
            .desktop_controller
            .as_ref()
            .and_then(gpui::WeakEntity::upgrade)
            .map(|controller| controller.read(cx).workspace_targets(self.workspace_id))
            .unwrap_or_default();
        let move_window_items = move_window_menu_items(
            targets
                .iter()
                .map(|target| (target.workspace_id, target.ordinal)),
        );
        let menu_visible_height = tab_actions_menu_visible_height(policy, viewport_h);
        let (menu_x, menu_y) = if let Some((x, y)) = self.session.dialog_tab_actions_anchor() {
            clamp_tab_actions_position(
                x,
                y,
                TAB_ACTIONS_MENU_WIDTH,
                menu_visible_height,
                viewport_w,
                viewport_h,
            )
        } else {
            (
                ((viewport_w - TAB_ACTIONS_MENU_WIDTH).max(16.) * 0.5).max(8.),
                74.0,
            )
        };
        let active_submenu = self.session.dialog_tab_actions_submenu();

        let mut color_row = div().flex_none().p_2().grid().grid_cols(6).gap_1();
        for (name, color) in TAB_PRESET_COLORS {
            let selected = active_color == Some(color);
            let color_session_id = tab_root_id.clone();
            let color_group = SharedString::from(format!("tab-ctx-color-group-{name}"));
            color_row = color_row.child(
                div()
                    .id(SharedString::from(format!("tab-ctx-color-{name}")))
                    .group(color_group.clone())
                    .relative()
                    .size(px(20.))
                    .rounded_full()
                    .cursor_pointer()
                    .when(selected, |this| {
                        this.child(
                            div()
                                .absolute()
                                .top(px(-4.))
                                .left(px(-4.))
                                .size(px(28.))
                                .rounded_full()
                                .border_2()
                                .border_color(rgb(color)),
                        )
                        .child(
                            div()
                                .absolute()
                                .top(px(-2.))
                                .left(px(-2.))
                                .size(px(24.))
                                .rounded_full()
                                .border_2()
                                .border_color(rgb(palette.surface)),
                        )
                    })
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .size(px(20.))
                            .rounded_full()
                            .bg(rgb(color))
                            .when(!selected, |this| {
                                this.group_hover(color_group, |style| {
                                    style.top(px(-2.)).left(px(-2.)).size(px(24.))
                                })
                            }),
                    )
                    .tooltip({
                        let label = name.to_string();
                        move |window, cx| {
                            nyaterm_ui::NyaTooltip::new(label.clone()).build(window, cx)
                        }
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.select_session(color_session_id.clone(), cx);
                        this.close_tab_actions(cx);
                        this.set_active_session_tab_color(Some(color), cx);
                    })),
            );
        }
        let rename_session_id = tab_root_id.clone();
        let copy_name_session_id = tab_root_id.clone();
        let copy_host_session_id = session_id.clone();
        let duplicate_session_id = session_id.clone();
        let multiplex_session_id = session_id.clone();
        let startup_session_id = session_id.clone();
        let multiplex_startup_session_id = session_id.clone();
        let split_horizontal_session_id = session_id.clone();
        let split_vertical_session_id = session_id.clone();
        let reconnect_session_id = session_id.clone();
        let disconnect_session_id = session_id.clone();
        let info_session_id = session_id.clone();
        let inactive_anchor = tab_root_id.clone();
        let right_anchor = tab_root_id.clone();
        let lock_session_id = tab_root_id.clone();
        let explain_session_id = session_id.clone();
        let analyze_session_id = session_id.clone();
        let secure_attention_session_id = session_id.clone();
        let submenu_panel = active_submenu
            .filter(|submenu| policy.supports_submenu(*submenu))
            .map(|submenu| {
            let submenu_width = if submenu == TabActionsSubmenu::Color { 160. } else { 240. };
            let mut panel = div()
                .max_h(px((viewport_h - 16.).max(80.)))
                .overflow_y_scrollbar()
                .p_1()
                .flex()
                .flex_col();

            match submenu {
                TabActionsSubmenu::Color => {
                    panel = panel.child(color_row);
                    if active_color.is_some() {
                        let reset_color_session_id = tab_root_id.clone();
                        panel = panel.child(tab_menu_separator(palette)).child(tab_menu_item(
                            palette,
                            "tab-ctx-color-reset",
                            t!("tabCtx.resetColor"),
                            cx.listener(move |this, _, _, cx| {
                                this.select_session(reset_color_session_id.clone(), cx);
                                this.close_tab_actions(cx);
                                this.set_active_session_tab_color(None, cx);
                            }),
                        ));
                    }
                }
                TabActionsSubmenu::MoveWindow => {
                    for item in &move_window_items {
                        if item.separator_before {
                            panel = panel.child(tab_menu_separator(palette));
                        }
                        let moved_tab_id = tab_root_id.clone();
                        let label = match item.destination {
                            MoveWindowMenuDestination::NewWindow => {
                                t!("tabCtx.moveToNewWindow").to_string()
                            }
                            MoveWindowMenuDestination::Workspace { ordinal, .. } => {
                                t!("tabCtx.windowNumber", number = ordinal).to_string()
                            }
                        };
                        let destination = item.destination;
                        panel = panel.child(tab_menu_item_with_icon(
                            palette,
                            item.id.clone(),
                            label,
                            item.icon_path,
                            cx.listener(move |this, _, _, cx| {
                                this.close_tab_actions(cx);
                                match destination {
                                    MoveWindowMenuDestination::NewWindow => {
                                        this.move_tab_tree_to_new_window(moved_tab_id.clone(), cx);
                                    }
                                    MoveWindowMenuDestination::Workspace { workspace_id, .. } => {
                                        this.move_tab_tree_to_workspace(
                                            this.workspace_id,
                                            moved_tab_id.clone(),
                                            this.workspace_revision(),
                                            workspace_id,
                                            nyaterm_core::MoveTabPlacement::Append,
                                            cx,
                                        );
                                    }
                                }
                            }),
                        ));
                    }
                }
                TabActionsSubmenu::SshAdvanced => {
                    panel = panel
                        .child(tab_menu_item_enabled(
                            palette,
                            "tab-ctx-multiplex",
                            t!("tabCtx.multiplexSsh"),
                            availability.multiplex,
                            cx.listener(move |this, _, window, cx| {
                                this.select_session(multiplex_session_id.clone(), cx);
                                this.close_tab_actions(cx);
                                if this.session.session_is_busy(&multiplex_session_id)
                                    || this.session.is_disconnected(&multiplex_session_id)
                                {
                                    this.shell.set_status("SSH multiplex is unavailable for this session".to_string());
                                    cx.notify();
                                    return;
                                }
                                this.multiplex_active_ssh_session(window, cx);
                            }),
                        ))
                        .child(tab_menu_item_enabled(
                            palette,
                            "tab-ctx-multiplex-run",
                            t!("tabCtx.multiplexSshWithCommand"),
                            availability.multiplex,
                            cx.listener(move |this, _, window, cx| {
                                this.select_session(multiplex_startup_session_id.clone(), cx);
                                this.close_tab_actions(cx);
                                if this
                                    .session
                                    .session_is_busy(&multiplex_startup_session_id)
                                    || this.session.is_disconnected(&multiplex_startup_session_id)
                                {
                                    this.shell.set_status("SSH multiplex is unavailable for this session".to_string());
                                    cx.notify();
                                    return;
                                }
                                this.open_startup_command_dialog_for(
                                    StartupCommandAction::Multiplex,
                                    window,
                                    cx,
                                );
                            }),
                        ));
                }
                TabActionsSubmenu::Ai => {
                    panel = panel
                        .child(tab_menu_item_enabled(
                            palette,
                            "tab-ctx-ai-explain",
                            t!("ai.explainRecent"),
                            availability.use_ai,
                            cx.listener(move |this, _, window, cx| {
                                this.select_session(explain_session_id.clone(), cx);
                                this.close_tab_actions(cx);
                                if this.session.session_is_busy(&explain_session_id)
                                    || this.session.is_disconnected(&explain_session_id)
                                {
                                    this.ai
                                        .set_panel_status("terminal session unavailable for AI");
                                    cx.notify();
                                    return;
                                }
                                if visible_for_ai.trim().is_empty() {
                                    this.ai
                                        .set_panel_status("terminal visible screen is empty");
                                } else {
                                    this.set_ai_prompt_draft(format!(
                                        "Explain this terminal output:\n\n{}",
                                        visible_for_ai
                                    ), cx);
                                    this.ai
                                        .set_panel_status("terminal output loaded into AI prompt");
                                    window.focus(this.ai.chat_focus(), cx);
                                }
                                cx.notify();
                            }),
                        ))
                        .child(tab_menu_item_enabled(
                            palette,
                            "tab-ctx-ai-analyze",
                            t!("ai.analyzeError"),
                            availability.use_ai,
                            cx.listener(move |this, _, window, cx| {
                                this.select_session(analyze_session_id.clone(), cx);
                                this.close_tab_actions(cx);
                                if this.session.session_is_busy(&analyze_session_id)
                                    || this.session.is_disconnected(&analyze_session_id)
                                {
                                    this.ai
                                        .set_panel_status("terminal session unavailable for AI");
                                    cx.notify();
                                    return;
                                }
                                if buffer_for_ai.trim().is_empty() {
                                    this.ai.set_panel_status("terminal buffer is empty");
                                } else {
                                    this.set_ai_prompt_draft(format!(
                                        "Analyze this terminal buffer for errors, risks, and next actions:\n\n{}",
                                        buffer_for_ai
                                    ), cx);
                                    this.ai
                                        .set_panel_status("terminal buffer loaded into AI prompt");
                                    window.focus(this.ai.chat_focus(), cx);
                                }
                                cx.notify();
                            }),
                        ));
                }
            }
            div()
                .id(SharedString::from("tab-actions-submenu"))
                .w(px(submenu_width))
                .rounded_md()
                .border_1()
                .border_color(rgb(palette.border))
                .bg(self.shell_surface_color(palette.surface))
                .shadow_lg()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_mouse_down(MouseButton::Middle, |_, _, cx| cx.stop_propagation())
                .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                .on_click(|_, _, cx| cx.stop_propagation())
                .child(panel)
                .into_any_element()
        });
        let (color_submenu, move_window_submenu, ssh_submenu, ai_submenu) =
            match (active_submenu, submenu_panel) {
                (Some(TabActionsSubmenu::Color), panel) => (panel, None, None, None),
                (Some(TabActionsSubmenu::MoveWindow), panel) => (None, panel, None, None),
                (Some(TabActionsSubmenu::SshAdvanced), panel) => (None, None, panel, None),
                (Some(TabActionsSubmenu::Ai), panel) => (None, None, None, panel),
                (None, _) => (None, None, None, None),
            };

        div()
            .id(SharedString::from("tab-actions-overlay"))
            .absolute()
            .top_0()
            .bottom_0()
            .left_0()
            .right_0()
            .bg(rgba(0x00000000))
            .track_focus(self.session.dialog_tab_actions_focus())
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.close_tab_actions(cx);
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.close_tab_actions(cx);
                }),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.close_tab_actions(cx);
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                cx.stop_propagation();
                if event.keystroke.key == "escape" {
                    this.close_tab_actions(cx);
                }
            }))
            .child(
                div()
                    .id(SharedString::from("tab-actions-menu"))
                    .absolute()
                    .left(px(menu_x))
                    .top(px(menu_y))
                    .w(px(TAB_ACTIONS_MENU_WIDTH))
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(palette.border))
                    .bg(self.shell_surface_color(palette.surface))
                    .shadow_lg()
                    .on_mouse_down(MouseButton::Middle, |_, _, cx| cx.stop_propagation())
                    .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .max_h(px(menu_visible_height))
                            .overflow_y_scrollbar()
                            .p_1()
                            .flex()
                            .flex_col()
                            .child(tab_actions_submenu_host(
                                "tab-actions-color-submenu",
                                tab_actions_submenu_item(
                                    palette,
                                    TabActionsSubmenuItem {
                                        id: "tab-ctx-set-color",
                                        icon_path: "icons/menu/palette.svg",
                                        label: t!("tabCtx.setColor"),
                                        enabled: true,
                                        active: active_submenu == Some(TabActionsSubmenu::Color),
                                    },
                                    TabActionsSubmenuHandlers {
                                        on_hover: cx.listener(|this, hovered: &bool, _, cx| {
                                            if *hovered {
                                                this.open_tab_actions_submenu(
                                                    TabActionsSubmenu::Color,
                                                    cx,
                                                );
                                            }
                                        }),
                                        on_click: cx.listener(|this, _, _, cx| {
                                            this.open_tab_actions_submenu(
                                                TabActionsSubmenu::Color,
                                                cx,
                                            );
                                        }),
                                    },
                                ),
                                color_submenu,
                            ))
                            .child(tab_menu_item(
                                palette,
                                "tab-ctx-rename",
                                t!("tabCtx.rename"),
                                cx.listener(move |this, _, window, cx| {
                                    this.close_tab_actions(cx);
                                    this.open_rename_session(rename_session_id.clone(), window, cx);
                                }),
                            ))
                            .child(tab_menu_item(
                                palette,
                                if locked {
                                    "tab-ctx-unlock"
                                } else {
                                    "tab-ctx-lock"
                                },
                                if locked {
                                    t!("tabCtx.unlockTab")
                                } else {
                                    t!("tabCtx.lockTab")
                                },
                                cx.listener(move |this, _, _, cx| {
                                    this.close_tab_actions(cx);
                                    this.toggle_tab_tree_locked(&lock_session_id, cx);
                                }),
                            ))
                            .child(tab_menu_item(
                                palette,
                                "tab-ctx-copy-name",
                                t!("tabCtx.copyName"),
                                cx.listener(move |this, _, _, cx| {
                                    this.close_tab_actions(cx);
                                    this.copy_session_name(&copy_name_session_id, cx);
                                }),
                            ))
                            .child(tab_menu_separator(palette))
                            .child(tab_actions_submenu_host(
                                "tab-actions-move-window-submenu",
                                tab_actions_submenu_item(
                                    palette,
                                    TabActionsSubmenuItem {
                                        id: "tab-ctx-move-window",
                                        icon_path: "icons/net/move.svg",
                                        label: t!("tabCtx.moveToWindow"),
                                        enabled: true,
                                        active: active_submenu
                                            == Some(TabActionsSubmenu::MoveWindow),
                                    },
                                    TabActionsSubmenuHandlers {
                                        on_hover: cx.listener(|this, hovered: &bool, _, cx| {
                                            if *hovered {
                                                this.open_tab_actions_submenu(
                                                    TabActionsSubmenu::MoveWindow,
                                                    cx,
                                                );
                                            }
                                        }),
                                        on_click: cx.listener(|this, _, _, cx| {
                                            this.open_tab_actions_submenu(
                                                TabActionsSubmenu::MoveWindow,
                                                cx,
                                            );
                                        }),
                                    },
                                ),
                                move_window_submenu,
                            ))
                            .when(support.copy_ssh_host, |this| {
                                this.child(tab_menu_item(
                                    palette,
                                    "tab-ctx-copy-ip",
                                    t!("tabCtx.copyIp"),
                                    cx.listener(move |this, _, _, cx| {
                                        this.select_session(copy_host_session_id.clone(), cx);
                                        this.close_tab_actions(cx);
                                        this.copy_active_session_ssh_host(cx);
                                    }),
                                ))
                            })
                            .when(show_session_group, |this| {
                                this.child(tab_menu_separator(palette))
                            })
                            .when(support.session_spawn, |this| {
                                this.child(tab_menu_item_enabled(
                                    palette,
                                    "tab-ctx-duplicate",
                                    t!("tabCtx.duplicate"),
                                    availability.spawn_session,
                                    cx.listener(move |this, _, window, cx| {
                                        this.select_session(duplicate_session_id.clone(), cx);
                                        this.close_tab_actions(cx);
                                        if !this.tab_action_can_spawn_session(&duplicate_session_id)
                                        {
                                            this.shell.set_status(
                                                "active session cannot be duplicated".to_string(),
                                            );
                                            cx.notify();
                                            return;
                                        }
                                        this.duplicate_active_session(window, cx);
                                    }),
                                ))
                            })
                            .when(support.session_spawn, |this| {
                                this.child(tab_menu_item_enabled(
                                    palette,
                                    "tab-ctx-duplicate-run",
                                    t!("tabCtx.duplicateWithCommand"),
                                    availability.spawn_session,
                                    cx.listener(move |this, _, window, cx| {
                                        this.select_session(startup_session_id.clone(), cx);
                                        this.close_tab_actions(cx);
                                        if !this.tab_action_can_spawn_session(&startup_session_id) {
                                            this.shell.set_status(
                                                "active session cannot be duplicated".to_string(),
                                            );
                                            cx.notify();
                                            return;
                                        }
                                        this.open_startup_command_dialog(window, cx);
                                    }),
                                ))
                            })
                            .when(support.ssh_multiplex, |this| {
                                this.child(tab_actions_submenu_host(
                                    "tab-actions-ssh-submenu",
                                    tab_actions_submenu_item(
                                        palette,
                                        TabActionsSubmenuItem {
                                            id: "tab-ctx-ssh-advanced",
                                            icon_path: "icons/menu/call-split.svg",
                                            label: t!("tabCtx.sshAdvanced"),
                                            enabled: availability.multiplex,
                                            active: active_submenu
                                                == Some(TabActionsSubmenu::SshAdvanced),
                                        },
                                        TabActionsSubmenuHandlers {
                                            on_hover: cx.listener(
                                                move |this, hovered: &bool, _, cx| {
                                                    if *hovered && availability.multiplex {
                                                        this.open_tab_actions_submenu(
                                                            TabActionsSubmenu::SshAdvanced,
                                                            cx,
                                                        );
                                                    }
                                                },
                                            ),
                                            on_click: cx.listener(move |this, _, _, cx| {
                                                if availability.multiplex {
                                                    this.open_tab_actions_submenu(
                                                        TabActionsSubmenu::SshAdvanced,
                                                        cx,
                                                    );
                                                }
                                            }),
                                        },
                                    ),
                                    ssh_submenu,
                                ))
                            })
                            .when(support.reconnect, |this| {
                                this.child(tab_menu_item_enabled(
                                    palette,
                                    "tab-ctx-reconnect",
                                    t!("tabCtx.reconnect"),
                                    availability.reconnect,
                                    cx.listener(move |this, _, window, cx| {
                                        this.select_session(reconnect_session_id.clone(), cx);
                                        this.close_tab_actions(cx);
                                        if this.session.session_is_busy(&reconnect_session_id)
                                            || this
                                                .session
                                                .start_reconnect_is_pending(&reconnect_session_id)
                                        {
                                            cx.notify();
                                            return;
                                        }
                                        this.reconnect_active_session(window, cx);
                                    }),
                                ))
                            })
                            .when(support.disconnect, |this| {
                                this.child(tab_menu_item_enabled(
                                    palette,
                                    "tab-ctx-disconnect",
                                    t!("tabCtx.disconnect"),
                                    availability.disconnect,
                                    cx.listener(move |this, _, _, cx| {
                                        this.close_tab_actions(cx);
                                        if this.session.session_is_busy(&disconnect_session_id)
                                            || this.session.is_disconnected(&disconnect_session_id)
                                        {
                                            cx.notify();
                                            return;
                                        }
                                        this.disconnect_session(disconnect_session_id.clone(), cx);
                                    }),
                                ))
                            })
                            .when(support.ai, |this| {
                                this.child(tab_actions_submenu_host(
                                    "tab-actions-ai-submenu",
                                    tab_actions_submenu_item(
                                        palette,
                                        TabActionsSubmenuItem {
                                            id: "tab-ctx-ai",
                                            icon_path: "icons/ai.svg",
                                            label: t!("ai.title"),
                                            enabled: true,
                                            active: active_submenu == Some(TabActionsSubmenu::Ai),
                                        },
                                        TabActionsSubmenuHandlers {
                                            on_hover: cx.listener(|this, hovered: &bool, _, cx| {
                                                if *hovered {
                                                    this.open_tab_actions_submenu(
                                                        TabActionsSubmenu::Ai,
                                                        cx,
                                                    );
                                                }
                                            }),
                                            on_click: cx.listener(|this, _, _, cx| {
                                                this.open_tab_actions_submenu(
                                                    TabActionsSubmenu::Ai,
                                                    cx,
                                                );
                                            }),
                                        },
                                    ),
                                    ai_submenu,
                                ))
                            })
                            .when(support.rdp_secure_attention, |this| {
                                this.child(tab_menu_item_enabled(
                                    palette,
                                    "tab-ctx-rdp-secure-attention",
                                    t!("remoteDesktop.secureAttention"),
                                    availability.rdp_secure_attention,
                                    cx.listener(move |this, _, _, cx| {
                                        this.select_session(
                                            secure_attention_session_id.clone(),
                                            cx,
                                        );
                                        this.close_tab_actions(cx);
                                        if this
                                            .send_rdp_secure_attention(&secure_attention_session_id)
                                        {
                                            this.mark_user_activity();
                                        }
                                        cx.notify();
                                    }),
                                ))
                            })
                            .when(show_split_group, |this| {
                                this.child(tab_menu_separator(palette))
                            })
                            .when(support.split, |this| {
                                this.child(tab_menu_item_enabled(
                                    palette,
                                    "tab-ctx-split-h",
                                    t!("tabCtx.splitHorizontal"),
                                    availability.split,
                                    cx.listener(move |this, _, window, cx| {
                                        this.select_session(
                                            split_horizontal_session_id.clone(),
                                            cx,
                                        );
                                        this.close_tab_actions(cx);
                                        if !this.tab_action_can_spawn_session(
                                            &split_horizontal_session_id,
                                        ) {
                                            this.shell.set_status(
                                                "active session cannot be duplicated for split"
                                                    .to_string(),
                                            );
                                            cx.notify();
                                            return;
                                        }
                                        this.split_workspace_with_duplicate(
                                            WorkspaceSplitDirection::Horizontal,
                                            window,
                                            cx,
                                        );
                                    }),
                                ))
                            })
                            .when(support.split, |this| {
                                this.child(tab_menu_item_enabled(
                                    palette,
                                    "tab-ctx-split-v",
                                    t!("tabCtx.splitVertical"),
                                    availability.split,
                                    cx.listener(move |this, _, window, cx| {
                                        this.select_session(split_vertical_session_id.clone(), cx);
                                        this.close_tab_actions(cx);
                                        if !this.tab_action_can_spawn_session(
                                            &split_vertical_session_id,
                                        ) {
                                            this.shell.set_status(
                                                "active session cannot be duplicated for split"
                                                    .to_string(),
                                            );
                                            cx.notify();
                                            return;
                                        }
                                        this.split_workspace_with_duplicate(
                                            WorkspaceSplitDirection::Vertical,
                                            window,
                                            cx,
                                        );
                                    }),
                                ))
                            })
                            .when(support.split, |this| {
                                this.child(tab_menu_item(
                                    palette,
                                    "tab-ctx-unsplit",
                                    t!("tabCtx.unsplit"),
                                    cx.listener(|this, _, _, cx| {
                                        this.close_tab_actions(cx);
                                        this.unsplit_workspace(cx);
                                    }),
                                ))
                            })
                            .child(tab_menu_separator(palette))
                            .child(tab_menu_item_enabled(
                                palette,
                                "tab-ctx-close",
                                t!("tabCtx.close"),
                                availability.close_tab,
                                cx.listener(move |this, _, _, cx| {
                                    this.close_tab_actions(cx);
                                    this.close_tab_active_pane(&tab_root_id, cx);
                                }),
                            ))
                            .child(tab_menu_item(
                                palette,
                                "tab-ctx-close-all",
                                t!("tabCtx.closeAll"),
                                cx.listener(|this, _, window, cx| {
                                    this.close_tab_actions(cx);
                                    this.open_close_all_sessions_confirm(window, cx);
                                }),
                            ))
                            .child(tab_menu_item_enabled(
                                palette,
                                "tab-ctx-close-others",
                                t!("tabCtx.closeInactive"),
                                availability.close_inactive,
                                cx.listener(move |this, _, _, cx| {
                                    this.close_tab_actions(cx);
                                    this.close_inactive_sessions(inactive_anchor.clone(), cx);
                                }),
                            ))
                            .child(tab_menu_item_enabled(
                                palette,
                                "tab-ctx-close-right",
                                t!("tabCtx.closeRight"),
                                availability.close_right,
                                cx.listener(move |this, _, _, cx| {
                                    this.close_tab_actions(cx);
                                    this.close_sessions_to_right(right_anchor.clone(), cx);
                                }),
                            ))
                            .when(support.session_info, |this| {
                                this.child(tab_menu_item(
                                    palette,
                                    "tab-ctx-info",
                                    t!("tabCtx.sessionInfo"),
                                    cx.listener(move |this, _, window, cx| {
                                        this.close_tab_actions(cx);
                                        let Some(connection_id) = this
                                            .tab_action_source_connection_id(&info_session_id)
                                            .map(ToOwned::to_owned)
                                        else {
                                            this.shell.set_status(
                                                "session has no saved connection to edit"
                                                    .to_string(),
                                            );
                                            cx.notify();
                                            return;
                                        };
                                        this.open_connection_editor(
                                            Some(connection_id),
                                            None,
                                            false,
                                            window,
                                            cx,
                                        );
                                    }),
                                ))
                            }),
                    ),
            )
            .into_any_element()
    }

    fn open_tab_actions_submenu(&mut self, submenu: TabActionsSubmenu, cx: &mut Context<Self>) {
        if self.session.dialog_select_tab_actions_submenu(submenu) {
            cx.notify();
        }
    }
}

fn tab_actions_submenu_host(
    id: &'static str,
    trigger: impl IntoElement,
    content: Option<AnyElement>,
) -> AnyElement {
    let trigger = trigger.into_any_element();
    let hosted = if let Some(content) = content {
        NyaPopover::new(id, trigger, content)
            .placement(NyaPopoverPlacement::Right)
            .align(NyaPopoverAlign::Start)
            .offset(px(4.))
            .appearance(false)
            .overlay_closable(false)
            .trigger_click(false)
            .open(true)
            .into_any_element()
    } else {
        trigger
    };
    div()
        .h(px(28.))
        .w_full()
        .flex_none()
        .child(hosted)
        .into_any_element()
}

fn tab_actions_submenu_item<OnHover, OnClick>(
    palette: ThemePalette,
    item: TabActionsSubmenuItem,
    handlers: TabActionsSubmenuHandlers<OnHover, OnClick>,
) -> impl IntoElement
where
    OnHover: Fn(&bool, &mut Window, &mut App) + 'static,
    OnClick: Fn(&ClickEvent, &mut Window, &mut App) + 'static,
{
    let TabActionsSubmenuItem {
        id,
        icon_path,
        label,
        enabled,
        active,
    } = item;
    let TabActionsSubmenuHandlers { on_hover, on_click } = handlers;
    let text_color = rgb(palette.text);
    let icon_color = rgb(palette.text_muted);
    div()
        .id(SharedString::from(id))
        .h(px(28.))
        .flex_none()
        .px_2()
        .flex()
        .items_center()
        .gap_2()
        .rounded_sm()
        .text_size(px(12.))
        .text_color(text_color)
        .opacity(if enabled { 1. } else { 0.5 })
        .when(active, |this| this.bg(rgb(palette.hover)))
        .when(enabled, |this| {
            this.cursor_pointer()
                .hover(|this| this.bg(rgb(palette.hover)))
                .on_hover(on_hover)
                .on_click(on_click)
        })
        .child(
            div()
                .w(px(24.))
                .h_full()
                .flex_none()
                .flex()
                .items_center()
                .child(svg().size(px(16.)).path(icon_path).text_color(icon_color)),
        )
        .child(div().min_w_0().flex_1().child(label))
        .child(
            svg()
                .size(px(16.))
                .flex_none()
                .path("icons/fe/forward.svg")
                .text_color(icon_color),
        )
}

#[cfg(test)]
mod tests {
    use super::{MoveWindowMenuDestination, move_window_menu_items};
    use nyaterm_core::WorkspaceId;

    #[test]
    fn tab_actions_menu_keeps_new_window_when_no_existing_target_is_available() {
        let items = move_window_menu_items([]);

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].icon_path, "icons/menu/external.svg");
        assert!(!items[0].separator_before);
        assert_eq!(items[0].destination, MoveWindowMenuDestination::NewWindow);
    }

    #[test]
    fn tab_actions_menu_orders_and_icons_move_window_targets() {
        let second = WorkspaceId::new();
        let fourth = WorkspaceId::new();
        let items = move_window_menu_items([(second, 2), (fourth, 4)]);

        assert_eq!(items.len(), 3);
        assert_eq!(items[0].destination, MoveWindowMenuDestination::NewWindow);
        assert_eq!(items[0].icon_path, "icons/menu/external.svg");
        assert!(items[1].separator_before);
        assert_eq!(items[1].icon_path, "icons/workspace.svg");
        assert_eq!(
            items[1].destination,
            MoveWindowMenuDestination::Workspace {
                workspace_id: second,
                ordinal: 2,
            }
        );
        assert!(!items[2].separator_before);
        assert_eq!(items[2].icon_path, "icons/workspace.svg");
        assert_eq!(
            items[2].destination,
            MoveWindowMenuDestination::Workspace {
                workspace_id: fourth,
                ordinal: 4,
            }
        );
    }
}
