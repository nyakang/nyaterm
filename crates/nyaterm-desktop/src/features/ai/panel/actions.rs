use crate::features::NyaTermApp;
use crate::models::AiDetectedErrorState;
use crate::models::AiPreparedRequest;
use gpui::Context;
use gpui::Window;
use nyaterm_core::AiAction;
use rust_i18n::t;

impl NyaTermApp {
    pub(in crate::features) fn dismiss_ai_detected_error(&mut self, cx: &mut Context<Self>) {
        self.ai.dismiss_detected_error();
        self.defer_ai_panel_snapshot_flush(cx);
    }

    pub(in crate::features) fn quote_ai_message_text(
        &mut self,
        text: String,
        cx: &mut Context<Self>,
    ) {
        self.ai.quote_message(text);
        self.defer_ai_panel_snapshot_flush(cx);
    }

    pub(in crate::features) fn clear_ai_quote(&mut self, cx: &mut Context<Self>) {
        self.ai.clear_quote();
        self.defer_ai_panel_snapshot_flush(cx);
    }

    pub(in crate::features) fn analyze_ai_detected_error(
        &mut self,
        detected: AiDetectedErrorState,
        cx: &mut Context<Self>,
    ) {
        if self.ai.chat_or_agent_is_running() {
            self.ai
                .set_chat_response_preview("AI request already running");
            self.ai.set_panel_status("AI request already running");
            self.defer_ai_panel_snapshot_flush(cx);
            return;
        }
        let mut context = self.ai_terminal_context_for_session(Some(&detected.session_id));
        context.selected_text = detected.output.clone();
        let request = AiPreparedRequest {
            action: AiAction::AnalyzeError,
            context,
            source_label: "Detected terminal error".to_string(),
        };
        self.ai
            .prepare_detected_error_request(request, detected.session_id.clone());
        self.set_ai_prompt_draft("Analyze detected error", cx);
        self.start_ai_ask(cx);
    }

    pub(super) fn open_ai_delete_history_confirm(
        &mut self,
        session_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.ai.history_is_pending() || self.ai.ai_session_is_running(&session_id) {
            return;
        }
        self.open_confirm_dialog(
            (
                t!("ai.deleteHistoryTitle").to_string(),
                t!("ai.deleteHistoryDesc").to_string(),
                t!("ai.deleteSession").to_string(),
                true,
                move |app, _, cx| {
                    if app.ai.history_is_pending() || app.ai.ai_session_is_running(&session_id) {
                        return false;
                    }
                    app.delete_ai_session(session_id.clone(), cx);
                    true
                },
            ),
            window,
            cx,
        );
        cx.notify();
    }

    pub(in crate::features) fn open_ai_clear_history_confirm(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.ai.request_history_clear_confirm() {
            return;
        }
        self.open_confirm_dialog(
            (
                t!("ai.clearHistoryTitle").to_string(),
                t!("ai.clearHistoryDesc").to_string(),
                t!("ai.clearHistory").to_string(),
                true,
                |app, _, cx| app.confirm_ai_clear_history(cx),
            ),
            window,
            cx,
        );
        self.defer_ai_panel_snapshot_flush(cx);
        cx.notify();
    }

    pub(in crate::features) fn confirm_ai_clear_history(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.ai.confirm_history_clear() {
            return false;
        }
        self.clear_all_ai_history(cx);
        true
    }

    pub(in crate::features) fn open_ai_auto_execution_confirm(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ai.request_agent_auto_confirm();
        self.open_confirm_dialog(
            (
                t!("ai.autoExecutionConfirmTitle").to_string(),
                t!("ai.autoExecutionConfirmDesc").to_string(),
                t!("ai.enableAutoExecution").to_string(),
                true,
                |app, _, cx| app.confirm_ai_auto_execution(cx),
            ),
            window,
            cx,
        );
        self.defer_ai_panel_snapshot_flush(cx);
        cx.notify();
    }

    pub(in crate::features) fn confirm_ai_auto_execution(
        &mut self,
        cx: &mut Context<Self>,
    ) -> bool {
        let before = self.ai_header_presentation();
        if !self.ai.confirm_agent_auto_execution() {
            return false;
        }
        self.persist_ai_settings_now(cx);
        self.defer_ai_panel_snapshot_flush(cx);
        self.notify_root_if_ai_header_changed(before, cx);
        true
    }
}
