pub(in crate::features) mod model;

use std::collections::HashMap;
use std::time::Duration;

use gpui::{Context, Task};
use nyaterm_store::StoreDomain;

use self::model::{ActivityFilter, ActivityOutcome, ActivityPage, project_activity};
use crate::features::NyaTermApp;
use crate::features::runtime_jobs::await_blocking_result;
use crate::models::{SettingsTab, WorkspaceTabId};

#[derive(Clone, Default, PartialEq, Eq)]
pub(in crate::features) struct McpActivityView {
    pub open: bool,
    pub loading: bool,
    pub failed: bool,
    pub filter: ActivityFilter,
    pub data: ActivityPage,
}

#[derive(Default)]
pub(in crate::features) struct McpActivityState {
    pub view: McpActivityView,
    generation: u64,
    visible: bool,
    requested_page: usize,
    timer: Option<Task<()>>,
    query: Option<Task<()>>,
}

impl McpActivityState {
    fn set_filter(&mut self, field: &str, value: &str) -> bool {
        let filter = &mut self.view.filter;
        match field {
            "client" => filter.client = value.strip_prefix("client:").map(str::to_owned),
            "target" => filter.target = (value != "__all__").then(|| value.to_owned()),
            "outcome" => filter.outcome = ActivityOutcome::from_key(value),
            _ => return false,
        }
        self.requested_page = 0;
        true
    }
    fn change_page(&mut self, forward: bool) {
        self.requested_page = if forward {
            self.view.data.page.saturating_add(1)
        } else {
            self.view.data.page.saturating_sub(1)
        };
    }

    fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.query = None;
        self.view.loading = false;
    }
    fn finish(&mut self, generation: u64, result: Result<ActivityPage, String>) {
        if generation != self.generation || !self.visible {
            return;
        }
        self.view.loading = false;
        self.view.failed = result.is_err();
        if let Ok(data) = result {
            self.requested_page = data.page;
            self.view.data = data;
        }
    }
}

impl NyaTermApp {
    fn mcp_activity_visible(&self) -> bool {
        if !self.mcp.activity.view.open
            || self.security.screen_locked()
            || self.shell.settings_active_tab() != SettingsTab::AiAgents
        {
            return false;
        }
        let key = WorkspaceTabId::settings_key();
        if self.shell.pane_focus_mode()
            && self.active_workspace_tab().as_deref() != Some(key.as_str())
        {
            return false;
        }

        self.shell.terminal_window_tree().is_some_and(|tree| {
            tree.leaf_for_tab(&key)
                .and_then(|leaf| tree.leaf_tabs(leaf))
                .is_some_and(|(_, active)| active == Some(key.as_str()))
        })
    }

    pub(in crate::features) fn reconcile_mcp_activity_visibility(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        let visible = self.mcp_activity_visible();
        if visible == self.mcp.activity.visible {
            return;
        }
        self.mcp.activity.visible = visible;
        self.mcp.activity.invalidate();
        self.mcp.activity.timer = None;
        if !visible {
            return;
        }
        self.refresh_mcp_activity(cx);
        self.mcp.activity.timer = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(5)).await;
                let keep = this
                    .update(cx, |app, cx| {
                        if !app.mcp_activity_visible() {
                            app.mcp.activity.visible = false;
                            app.mcp.activity.invalidate();
                            return false;
                        }
                        if !app.mcp.activity.view.loading {
                            app.refresh_mcp_activity(cx);
                        }
                        true
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
        }));
    }

    pub(in crate::features) fn toggle_mcp_activity(&mut self, cx: &mut Context<Self>) {
        self.mcp.activity.view.open = !self.mcp.activity.view.open;
        self.reconcile_mcp_activity_visibility(cx);
        self.request_settings_panel_refresh(cx);
    }

    pub(in crate::features) fn set_mcp_activity_filter(
        &mut self,
        field: &str,
        value: &str,
        cx: &mut Context<Self>,
    ) {
        if self.mcp.activity.set_filter(field, value) {
            self.refresh_mcp_activity(cx);
        }
    }

    pub(in crate::features) fn change_mcp_activity_page(
        &mut self,
        forward: bool,
        cx: &mut Context<Self>,
    ) {
        self.mcp.activity.change_page(forward);
        self.refresh_mcp_activity(cx);
    }

    pub(in crate::features) fn refresh_mcp_activity(&mut self, cx: &mut Context<Self>) {
        if !self.mcp_activity_visible() {
            return;
        }
        self.mcp.activity.invalidate();
        self.mcp.activity.visible = true;
        self.mcp.activity.view.loading = true;
        let generation = self.mcp.activity.generation;
        let filter = self.mcp.activity.view.filter.clone();
        let page = self.mcp.activity.requested_page;
        let mut names: HashMap<String, String> = self
            .connection_state
            .connections()
            .iter()
            .map(|connection| {
                (
                    format!("connection:{}", connection.id),
                    connection.name.clone(),
                )
            })
            .collect();
        for session in self.session.ordered_sessions() {
            names.insert(
                format!("session:{}", session.id),
                self.session
                    .display_name(&session.id)
                    .unwrap_or(session.name),
            );
        }
        let store = self.store_blocking_client();
        let task = self
            .blocking_jobs
            .submit_task("mcp-activity-read", move |_| {
                let logs = store
                    .request_fn(StoreDomain::Ai, |database| {
                        database.list_ai_audit_logs(None)
                    })
                    .map_err(|_| "MCP activity is unavailable".to_string())?;
                Ok::<_, String>(project_activity(logs, &names, &filter, page))
            });
        self.mcp.activity.query = Some(cx.spawn(async move |this, cx| {
            let result = await_blocking_result(task).await;
            let _ = this.update(cx, |app, cx| {
                if !app.mcp_activity_visible() {
                    app.reconcile_mcp_activity_visibility(cx);
                    return;
                }
                app.mcp.activity.finish(generation, result);
                app.request_settings_panel_refresh(cx);
            });
        }));
        self.request_settings_panel_refresh(cx);
    }
}

#[cfg(test)]
mod tests {
    use super::McpActivityState;
    use super::model::ActivityPage;

    #[test]
    fn stale_results_and_hidden_queries_do_not_replace_current_page() {
        let mut state = McpActivityState {
            visible: true,
            ..Default::default()
        };
        state.invalidate();
        let old = state.generation;
        state.invalidate();
        state.finish(
            old,
            Ok(ActivityPage {
                matched_count: 99,
                ..Default::default()
            }),
        );
        assert_eq!(state.view.data.matched_count, 0);
        state.finish(
            state.generation,
            Ok(ActivityPage {
                matched_count: 12,
                ..Default::default()
            }),
        );
        state.finish(state.generation, Err("failure".into()));
        assert!(state.view.failed);
        assert_eq!(state.view.data.matched_count, 12);
        state.visible = false;
        state.finish(state.generation, Ok(ActivityPage::default()));
        assert_eq!(state.view.data.matched_count, 12);
    }
}

#[cfg(test)]
mod visibility_tests {
    use crate::features::test_support::app_with_visible_local_session;
    use crate::models::{SettingsTab, WorkspaceTabId};
    use crate::test_support::{TestAppCleanup, TestConfigDir};
    use gpui::TestAppContext;
    use nyaterm_core::workspace_layout::tabs::TerminalWindowNode;

    #[test]
    fn mcp_activity_requires_an_expanded_visible_settings_page() {
        let root = TestConfigDir::new("nyaterm-mcp-activity-visibility");
        let mut cx = TestAppContext::single();
        let _cleanup = TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, root.path(), "local");
        app.update(&mut cx, |app, _| {
            let key = WorkspaceTabId::settings_key();
            app.shell.set_settings_active_tab(SettingsTab::AiAgents);
            app.shell
                .restore_terminal_window_tree(Some(TerminalWindowNode::leaf(
                    vec![key.clone(), "local".into()],
                    Some(key.clone()),
                )));
            assert!(!app.mcp_activity_visible());
            app.mcp.activity.view.open = true;
            assert!(app.mcp_activity_visible());
            app.shell.set_settings_active_tab(SettingsTab::AiGeneral);
            assert!(!app.mcp_activity_visible());
            app.shell.set_settings_active_tab(SettingsTab::AiAgents);
            app.shell
                .restore_terminal_window_tree(Some(TerminalWindowNode::leaf(
                    vec![key, "local".into()],
                    Some("local".into()),
                )));
            assert!(!app.mcp_activity_visible());
            app.mcp.activity.view.open = false;
        });
        let cleanup = app.update(&mut cx, |app, cx| app.shutdown_blocking_jobs(cx));
        cx.foreground_executor().block_test(cleanup);
    }
}
