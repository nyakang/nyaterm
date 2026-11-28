use gpui::{AppContext, Context, IntoElement, SharedString, div, prelude::*, px, rgb};
use nyaterm_core::plugins::management::PluginKey;
use nyaterm_ui::tab_close::NyaTabCloseButton;
use rust_i18n::t;

use crate::features::NyaTermApp;
use crate::features::plugins::view::PluginPanel;
use crate::features::shell::session_tabs::SessionStripTabId;
use crate::features::shell::{SessionTabDragPreview, WorkspaceTabDragPayload};
use crate::models::{NavItem, WorkspaceTabId};

impl NyaTermApp {
    pub(in crate::features) fn plugin_title(&self, key: &PluginKey, cx: &gpui::App) -> String {
        let process = self.plugins.panel.read(cx).process.read(cx);
        super::projection::entries(process)
            .into_iter()
            .find(|entry| &entry.key == key)
            .map(|entry| entry.name)
            .unwrap_or_else(|| key.id())
    }

    pub(in crate::features) fn open_plugin_details(
        &mut self,
        key: PluginKey,
        tab: usize,
        cx: &mut Context<Self>,
    ) -> gpui::Entity<PluginPanel> {
        let detail = if let Some(detail) = self.plugins.details.get(&key) {
            detail.clone()
        } else {
            let app = cx.entity().downgrade();
            let process = self.plugins.panel.read(cx).process.clone();
            let detail_key = key.clone();
            let detail = cx.new(|cx| {
                let mut panel = PluginPanel::new(app, process, cx);
                panel.detail_key = Some(detail_key);
                panel
            });
            self.plugins.details.insert(key.clone(), detail.clone());
            detail
        };
        detail.update(cx, |detail, cx| {
            detail.detail_tab = tab;
            cx.notify();
        });
        self.ensure_terminal_windows_root();
        let layout_key = WorkspaceTabId::Plugin(key.clone()).layout_key();
        if let Some(mut tree) = self.shell.terminal_window_tree() {
            if !tree.contains_tab(&layout_key) {
                let group = self
                    .shell
                    .workspace
                    .focused_group
                    .clone()
                    .or_else(|| tree.first_leaf_id());
                tree.ensure_tab(&layout_key, group.as_deref());
            }
            self.shell.restore_terminal_window_tree(Some(tree));
        }
        self.activate_plugin_tab(&key, cx);
        if let Some(window) = cx.active_window() {
            let app = cx.weak_entity();
            cx.defer(move |cx| {
                let _ = cx.update_window(window, |_, window, cx| {
                    let _ =
                        app.update(cx, |app, cx| app.focus_active_workspace_surface(window, cx));
                });
            });
        }
        detail
    }

    pub(in crate::features) fn activate_plugin_tab(
        &mut self,
        key: &PluginKey,
        cx: &mut Context<Self>,
    ) {
        let layout_key = WorkspaceTabId::Plugin(key.clone()).layout_key();
        self.session.clear_active_start_selection();
        self.shell.workspace.focused_group =
            self.shell.sync_terminal_windows_active_tab(&layout_key);
        self.shell.set_pane_focus_mode(false);
        self.shell.navigation.selected_nav = NavItem::Workspace;
        self.shell.request_session_tab_scroll_into_view();
        self.close_action_link_menu(cx);
        cx.notify();
    }

    pub(in crate::features) fn close_plugin_details(
        &mut self,
        key: &PluginKey,
        cx: &mut Context<Self>,
    ) {
        let layout_key = WorkspaceTabId::Plugin(key.clone()).layout_key();
        let active = self.active_workspace_tab();
        if let Some(detail) = self.plugins.details.remove(key) {
            detail.update(cx, |detail, cx| detail.cancel(cx));
        }
        if let Some(mut tree) = self.shell.terminal_window_tree() {
            self.shell
                .restore_terminal_window_tree(tree.remove_tab(&layout_key));
        }
        if active.as_deref() == Some(&layout_key) {
            if let Some(next) = self.workspace_tab_ids().first().cloned() {
                self.activate_workspace_tab(&next, cx);
            } else {
                self.shell.workspace.focused_group = None;
            }
        }
        cx.notify();
    }

    pub(in crate::features) fn plugin_details_content(&self, key: &PluginKey) -> gpui::AnyElement {
        self.plugins
            .details
            .get(key)
            .map(|detail| detail.clone().into_any_element())
            .unwrap_or_else(|| div().into_any_element())
    }

    pub(in crate::features) fn plugin_strip_tab(
        &mut self,
        key: PluginKey,
        index: usize,
        active: Option<&str>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let palette = self.theme_palette();
        let layout_key = WorkspaceTabId::Plugin(key.clone()).layout_key();
        let selected = active == Some(layout_key.as_str());
        let title = format!("{}: {}", t!("plugins.title"), self.plugin_title(&key, cx));
        let group = SharedString::from(format!("plugin-tab-{}", key.id()));
        let click_key = layout_key.clone();
        let close_key = key.clone();
        let payload = WorkspaceTabDragPayload {
            source_workspace_id: self.workspace_id,
            root_tab_id: layout_key.clone(),
            source_revision: self.workspace_revision(),
            tab_id: SessionStripTabId::Workspace(WorkspaceTabId::Plugin(key.clone())),
            order_index: index,
            source_group_id: self.shell.terminal_group_for_tab(&layout_key),
            display_name: title.clone(),
            kind_icon: "icons/extensions.svg",
            preview_background: palette.surface,
            preview_border: palette.border,
            preview_text: palette.text,
            preview_text_muted: palette.text_muted,
            preview_accent: palette.primary,
        };
        let app = cx.weak_entity();
        div()
            .id(group.clone())
            .group(group.clone())
            .h(px(36.))
            .min_w(px(118.))
            .max_w(px(236.))
            .px_2()
            .flex_none()
            .flex()
            .items_center()
            .gap_2()
            .border_r_1()
            .border_color(rgb(palette.border))
            .bg(rgb(if selected {
                palette.surface
            } else {
                palette.bg
            }))
            .hover(move |style| style.bg(rgb(palette.hover)))
            .on_click(cx.listener(move |app, _, window, cx| {
                app.activate_workspace_tab(&click_key, cx);
                app.focus_active_workspace_surface(window, cx);
            }))
            .on_mouse_down(
                gpui::MouseButton::Middle,
                cx.listener(move |app, _, _, cx| {
                    cx.stop_propagation();
                    app.close_plugin_details(&close_key, cx);
                }),
            )
            .on_drag(payload, move |payload, position, window, cx| {
                let _ = app.update(cx, |app, cx| {
                    app.begin_session_tab_drag(payload, position, window, cx)
                });
                cx.new(|_| SessionTabDragPreview::new(payload.clone(), position))
            })
            .on_drop(
                cx.listener(move |app, payload: &WorkspaceTabDragPayload, _, cx| {
                    if payload.source_workspace_id != app.workspace_id {
                        app.clear_session_tab_drag(cx);
                        return;
                    }
                    app.reorder_session_relative(
                        payload.tab_id.layout_key(),
                        layout_key.clone(),
                        false,
                        cx,
                    );
                }),
            )
            .child(nyaterm_ui::icons::mono_icon(
                "icons/extensions.svg",
                rgb(palette.text_muted).into(),
                16.,
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(12.))
                    .child(title),
            )
            .child(
                NyaTabCloseButton::new("close", group, palette, t!("common.close"))
                    .active(selected)
                    .on_click(cx.listener(move |app, _, _, cx| {
                        cx.stop_propagation();
                        app.close_plugin_details(&key, cx);
                    })),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use crate::features::test_support::app_with_visible_local_session;
    use crate::models::WorkspaceTabId;
    use crate::test_support::{TestAppCleanup, TestConfigDir};
    use gpui::{AppContext, TestAppContext};
    use nyaterm_core::plugins::management::PluginKey;
    use nyaterm_core::plugins::native::NativeComponentId;

    #[test]
    fn detail_tabs_are_unique_transient_and_never_become_sessions() {
        let dir = TestConfigDir::new("nyaterm-plugin-detail-tabs");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, dir.path(), "session");
        cx.update_entity(&app, |app, cx| {
            let rdp = PluginKey::Native(NativeComponentId::Rdp);
            let wasm = PluginKey::Wasm("test-extension".into());
            let first = app.open_plugin_details(rdp.clone(), 0, cx);
            let second = app.open_plugin_details(wasm.clone(), 3, cx);
            let reopened = app.open_plugin_details(rdp.clone(), 1, cx);
            assert_eq!(first.entity_id(), reopened.entity_id());
            assert_ne!(first.entity_id(), second.entity_id());
            assert_eq!(app.plugins.details.len(), 2);
            assert_eq!(first.read(cx).detail_tab, 1);
            assert_eq!(second.read(cx).detail_tab, 3);
            let key = WorkspaceTabId::Plugin(rdp.clone()).layout_key();
            assert_eq!(app.active_workspace_tab(), Some(key.clone()));
            assert_eq!(app.workspace_tab_session(&key), None);
            assert!(!WorkspaceTabId::key_is_session(&key));
            app.reconcile_terminal_windows();
            assert!(app.workspace_tab_ids().contains(&key));
            let session_ids: Vec<_> = app
                .ordered_tab_sessions()
                .into_iter()
                .map(|session| session.id)
                .collect();
            let saved = app
                .shell
                .serialize_terminal_window_layout(&session_ids)
                .unwrap();
            let restored =
                nyaterm_core::workspace_layout::tabs::TerminalWindowNode::restore_layout(
                    &saved,
                    &session_ids,
                )
                .unwrap();
            assert!(
                restored
                    .collect_tab_ids()
                    .iter()
                    .all(|key| WorkspaceTabId::key_is_session(key))
            );
            app.close_plugin_details(&rdp, cx);
            assert!(!app.workspace_tab_ids().contains(&key));
            assert!(app.plugins.details.contains_key(&wasm));
            assert!(app.session.has_session("session"));
        });
    }
}
