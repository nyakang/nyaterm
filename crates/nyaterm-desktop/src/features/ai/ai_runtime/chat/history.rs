use gpui::{Context, Window};
use nyaterm_core::AiSessionScopeType;
use nyaterm_store::StoreDomain;
use std::sync::atomic::Ordering;

use crate::features::{NyaTermApp, formatting::compact_id, runtime_jobs::await_blocking_job};

use super::super::super::ai_jobs::{ai_active_profile_drafts, ai_usage_counts};

impl NyaTermApp {
    pub(in crate::features) fn toggle_ai_history(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.ai.history_is_open() {
            self.close_ai_history(window, cx);
        } else {
            self.ai.remember_history_focus(window.focused(cx));
            self.ai.toggle_history();
            self.refresh_ai_session_list(cx);
            let query = self.ai.history_query().to_string();
            self.reset_text_input("ai.history-search", &query, cx);
            let field = self.text_input(
                "ai.history-search",
                &query,
                crate::features::text_inputs::TextInputSetup::placeholder(rust_i18n::t!(
                    "ai.historySearchPlaceholder"
                )),
                cx,
            );
            window.focus(&field.read(cx).focus_handle(), cx);
        }
        self.defer_ai_panel_snapshot_flush(cx);
    }

    pub(in crate::features) fn close_ai_history(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ai.close_history();
        let focus = self.ai.take_history_focus();
        if let Some(focus) = focus {
            window.focus(&focus, cx);
        }
        self.forget_text_inputs("ai.history-search");
        self.defer_ai_panel_snapshot_flush(cx);
    }
    pub(in crate::features) fn sync_ai_drafts_from_active_profile(&mut self) {
        let (model, base_url) = ai_active_profile_drafts(self.ai.settings_config());
        self.ai.sync_settings_active_profile_drafts(model, base_url);
    }

    pub(in crate::features) fn refresh_ai_session_list(&mut self, cx: &mut Context<Self>) {
        let Some(job_id) = self.begin_ai_history_operation("loading AI history", cx) else {
            return;
        };
        let store = self.store_blocking_client();
        let task = self.blocking_jobs.submit_task("ai-history-list", move |_| {
            store
                .request_fn(StoreDomain::Ai, |store| store.list_ai_sessions())
                .map_err(|error| error.to_string())
        });
        cx.spawn(async move |this, cx| {
            let result = await_blocking_job(task).await.and_then(|result| result);
            let _ = this.update(cx, |this, cx| {
                if this.ai.finish_history_session_list(job_id, result) {
                    this.defer_ai_panel_snapshot_flush(cx);
                }
            });
        })
        .detach();
    }

    pub(in crate::features) fn start_new_ai_chat(&mut self, cx: &mut Context<Self>) {
        self.sync_ai_active_scope(cx);
        if self.ai.chat_or_agent_is_running() {
            return;
        }
        let before = self.ai_header_presentation();
        self.ai.start_new_chat();
        // The composer keeps its own buffer, so clearing the draft is not enough.
        self.reset_text_input("ai.chat.prompt", "", cx);
        self.defer_ai_panel_snapshot_flush(cx);
        self.notify_root_if_ai_header_changed(before, cx);
    }

    pub(in crate::features) fn load_ai_session_messages(
        &mut self,
        session_id: String,
        cx: &mut Context<Self>,
    ) {
        if self.ai.chat_session_id() == session_id {
            self.ai.close_history();
            self.defer_ai_panel_snapshot_flush(cx);
            return;
        }
        if self.ai.ai_session_is_running(&session_id) || self.ai.chat_or_agent_is_running() {
            self.ai.set_panel_status("AI session is in use".to_string());
            self.defer_ai_panel_snapshot_flush(cx);
            return;
        }
        let Some(job_id) = self.begin_ai_history_operation("loading AI session", cx) else {
            return;
        };
        let source_session_id = self.ai.chat_session_id().to_string();
        let source_scope = self.ai.active_scope_key().to_string();
        let (scope_epoch, expected_epoch) = self.ai.visible_scope_guard();
        let loaded_kind = self
            .ai
            .history_session(&session_id)
            .map(|session| session.agent_kind.clone());
        let owner_scope = self.ai_current_owner_scope();
        let should_rebind = owner_scope.r#type == AiSessionScopeType::Terminal
            && self.ai.history_session(&session_id).is_some_and(|session| {
                session.scope.r#type != owner_scope.r#type
                    || session.scope.target_id != owner_scope.target_id
            });
        let store = self.store_blocking_client();
        let job_session_id = session_id.clone();
        let read_store = store.clone();
        let task = self
            .blocking_jobs
            .submit_task("ai-history-messages", move |_| {
                read_store
                    .request_fn(StoreDomain::Ai, move |store| {
                        let messages = store.list_ai_messages(&job_session_id)?;
                        Ok(messages)
                    })
                    .map_err(|error| error.to_string())
            });
        cx.spawn(async move |this, cx| {
            let mut result = await_blocking_job(task).await.and_then(|result| result);
            let still_current = this
                .update(cx, |this, _| {
                    this.ai
                        .history_load_is_current(job_id, &source_scope, &source_session_id)
                        && scope_epoch.load(Ordering::Acquire) == expected_epoch
                })
                .unwrap_or(false);
            if !still_current {
                result = Err("AI session load cancelled after switching terminal".to_string());
            }
            let rebound = if result.is_ok() && should_rebind {
                let target_session_id = session_id.clone();
                let binding_store = store.clone();
                let binding_epoch = std::sync::Arc::clone(&scope_epoch);
                let binding_task = this.update(cx, |this, _| {
                    this.blocking_jobs
                        .submit_task("ai-history-rebind", move |_| {
                            binding_store
                                .request_fn(StoreDomain::Ai, move |store| {
                                    if binding_epoch.load(Ordering::Acquire) != expected_epoch {
                                        return Err(nyaterm_store::StorageError::InvalidData(
                                            "AI session load cancelled after switching terminal"
                                                .to_string(),
                                        ));
                                    }
                                    store.rebind_ai_session(&target_session_id, owner_scope)
                                })
                                .map_err(|error| error.to_string())
                        })
                });
                match binding_task {
                    Ok(task) => match await_blocking_job(task).await.and_then(|result| result) {
                        Ok(session) => Some(session),
                        Err(error) => {
                            result = Err(error);
                            None
                        }
                    },
                    Err(_) => {
                        result = Err("AI session load was interrupted".to_string());
                        None
                    }
                }
            } else {
                None
            };
            let _ = this.update(cx, |this, cx| {
                let valid =
                    this.ai
                        .history_load_is_current(job_id, &source_scope, &source_session_id)
                        && scope_epoch.load(Ordering::Acquire) == expected_epoch;
                let loaded = valid && result.is_ok();
                let loaded_status = format!("loaded AI session {}", compact_id(&session_id));
                if loaded {
                    this.ai.release_ai_session(&session_id);
                }
                let target_session_id = session_id.clone();
                let before = this.ai_header_presentation();
                if this.ai.finish_history_message_load(
                    job_id,
                    &source_session_id,
                    session_id,
                    if valid {
                        result
                    } else {
                        Err("AI session load cancelled after switching terminal".to_string())
                    },
                    loaded_status,
                ) {
                    if loaded && let Some(rebound) = rebound {
                        this.ai.replace_history_session(rebound);
                    }
                    if loaded
                        && this.ai.chat_session_id() == target_session_id
                        && let Some(kind) = loaded_kind
                    {
                        this.ai.apply_loaded_session_kind(kind);
                    }
                    if loaded {
                        let draft = this.ai.chat_prompt_draft().to_string();
                        this.reset_text_input("ai.chat.prompt", &draft, cx);
                    }
                    this.defer_ai_panel_snapshot_flush(cx);
                    this.notify_root_if_ai_header_changed(before, cx);
                }
            });
        })
        .detach();
    }

    pub(in crate::features) fn delete_ai_session(
        &mut self,
        session_id: String,
        cx: &mut Context<Self>,
    ) {
        if self.ai.ai_session_is_running(&session_id) {
            self.ai.set_panel_status("AI session is in use".to_string());
            self.defer_ai_panel_snapshot_flush(cx);
            return;
        }
        let Some(job_id) = self.begin_ai_history_operation("deleting AI session", cx) else {
            return;
        };
        let store = self.store_blocking_client();
        let job_session_id = session_id.clone();
        let task = self
            .blocking_jobs
            .submit_task("ai-history-delete", move |_| {
                store
                    .request_fn(StoreDomain::Ai, move |store| {
                        store.delete_ai_session(&job_session_id)
                    })
                    .map_err(|error| error.to_string())
            });
        cx.spawn(async move |this, cx| {
            let result = await_blocking_job(task).await.and_then(|result| result);
            let _ = this.update(cx, |this, cx| {
                let before = this.ai_header_presentation();
                if let Some(succeeded) =
                    this.ai
                        .finish_history_session_delete(job_id, &session_id, result)
                {
                    if succeeded {
                        this.refresh_ai_usage_counts(cx);
                    }
                    this.defer_ai_panel_snapshot_flush(cx);
                    this.notify_root_if_ai_header_changed(before, cx);
                }
            });
        })
        .detach();
    }

    pub(in crate::features) fn clear_all_ai_history(&mut self, cx: &mut Context<Self>) {
        if self.ai.any_chat_or_agent_running() {
            self.ai
                .set_panel_status("Stop AI requests before clearing history".to_string());
            self.defer_ai_panel_snapshot_flush(cx);
            return;
        }
        let Some(job_id) = self.begin_ai_history_operation("clearing AI history", cx) else {
            return;
        };
        let source_session_id = self.ai.chat_session_id().to_string();
        let store = self.store_blocking_client();
        let task = self
            .blocking_jobs
            .submit_task("ai-history-clear", move |_| {
                store
                    .request_fn(StoreDomain::Ai, |store| store.clear_ai_history())
                    .map_err(|error| error.to_string())
            });
        cx.spawn(async move |this, cx| {
            let result = await_blocking_job(task).await.and_then(|result| result);
            let _ = this.update(cx, |this, cx| {
                let before = this.ai_header_presentation();
                if let Some(succeeded) =
                    this.ai
                        .finish_history_clear(job_id, &source_session_id, result)
                {
                    if succeeded {
                        this.refresh_ai_usage_counts(cx);
                    }
                    this.defer_ai_panel_snapshot_flush(cx);
                    this.notify_root_if_ai_header_changed(before, cx);
                }
            });
        })
        .detach();
    }

    pub(in crate::features) fn apply_ai_history_search(
        &mut self,
        text: String,
        cx: &mut Context<Self>,
    ) {
        self.ai.set_history_query(text);
        self.defer_ai_panel_snapshot_flush(cx);
    }

    pub(in crate::features) fn refresh_ai_usage_counts(&mut self, cx: &mut Context<Self>) {
        let job_id = self.ai.begin_history_usage_count_job();
        let store = self.store_blocking_client();
        let task = self.blocking_jobs.submit_task("ai-usage-counts", move |_| {
            store
                .request_fn(StoreDomain::Ai, |store| Ok(ai_usage_counts(store)))
                .map_err(|error| error.to_string())
        });
        cx.spawn(async move |this, cx| {
            let result = await_blocking_job(task).await.and_then(|result| result);
            let _ = this.update(cx, |this, cx| {
                if this.ai.finish_history_usage_counts(job_id, result) {
                    this.defer_ai_panel_snapshot_flush(cx);
                }
            });
        })
        .detach();
    }

    fn begin_ai_history_operation(
        &mut self,
        status: &'static str,
        cx: &mut Context<Self>,
    ) -> Option<u64> {
        let job_id = self.ai.begin_history_operation(status);
        self.defer_ai_panel_snapshot_flush(cx);
        job_id
    }
}
