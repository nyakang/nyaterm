use std::collections::HashMap;

use gpui::{ListAlignment, ListState, px};
use nyaterm_terminal::recording_sanitizer::RecordingSanitizer;
use nyaterm_transport::RecordingStatus;

use crate::models::{
    RecordingEventReceiver, RecordingHistorySearchKey, RecordingWriteHandle, RecordingWritePipeline,
};

pub(in crate::features) struct RecordingFeatureState {
    active_count: usize,
    pipeline: RecordingWritePipeline,
    search_draft: String,
    panel_list: ListState,
    /// Invalidates cached measurements when optional lines appear or disappear.
    panel_row_layout: Vec<RecordingPanelRowLayout>,
    busy_actions: HashMap<String, String>,
    rekeyed_sessions: HashMap<String, String>,
    statuses: HashMap<String, RecordingStatus>,
    prompts: HashMap<String, RecordingPrompt>,
}

#[derive(PartialEq, Eq)]
pub(in crate::features) struct RecordingPanelRowLayout {
    pub session_id: String,
    pub has_path: bool,
    pub has_error: bool,
}

#[derive(Default)]
struct RecordingPrompt {
    tail: String,
    sensitive: bool,
    sanitizer: RecordingSanitizer,
}

impl RecordingFeatureState {
    pub(in crate::features) fn new(memory_limit_bytes: usize) -> Self {
        let pipeline = RecordingWritePipeline::spawn(memory_limit_bytes);
        Self {
            active_count: 0,
            pipeline,
            search_draft: String::new(),
            panel_list: ListState::new(0, ListAlignment::Top, px(100.)),
            panel_row_layout: Vec::new(),
            busy_actions: HashMap::new(),
            rekeyed_sessions: HashMap::new(),
            statuses: HashMap::new(),
            prompts: HashMap::new(),
        }
    }

    pub(in crate::features) fn panel_list(
        &mut self,
        rows: Vec<RecordingPanelRowLayout>,
    ) -> ListState {
        if !self
            .panel_row_layout
            .iter()
            .map(|row| &row.session_id)
            .eq(rows.iter().map(|row| &row.session_id))
        {
            self.panel_list.reset(rows.len());
        } else {
            // Recording start/stop and failures can change even an off-screen row's height.
            for (index, (previous, next)) in self.panel_row_layout.iter().zip(&rows).enumerate() {
                if previous != next {
                    self.panel_list.remeasure_items(index..index + 1);
                }
            }
        }
        self.panel_row_layout = rows;
        self.panel_list.clone()
    }

    pub(in crate::features) fn observe_prompt(
        &mut self,
        session_id: &str,
        text: &str,
        classify: impl FnOnce(&str) -> bool,
    ) {
        let prompt = self.prompts.entry(session_id.to_string()).or_default();
        let text = prompt.sanitizer.feed(text);
        if let Some(newline) = text.rfind('\n') {
            prompt.tail.clear();
            prompt.tail.push_str(&text[newline + 1..]);
            prompt.sensitive = false;
        } else {
            prompt.tail.push_str(&text);
        }
        if prompt.tail.len() > 2048 {
            let mut start = prompt.tail.len() - 2048;
            while !prompt.tail.is_char_boundary(start) {
                start += 1;
            }
            prompt.tail.drain(..start);
        }
        prompt.sensitive |= classify(&prompt.tail);
    }

    pub(in crate::features) fn has_observed_prompt(&self, session_id: &str) -> bool {
        self.prompts.contains_key(session_id)
    }

    pub(in crate::features) fn input_is_sensitive(&self, session_id: &str) -> bool {
        self.prompts
            .get(session_id)
            .is_some_and(|prompt| prompt.sensitive)
    }

    pub(in crate::features) fn writer(&self) -> RecordingWriteHandle {
        self.pipeline.writer()
    }

    pub(in crate::features) fn set_memory_limit(&self, memory_limit_bytes: usize) {
        self.pipeline.set_memory_limit(memory_limit_bytes);
    }

    pub(in crate::features) fn active_count(&self) -> usize {
        self.active_count
    }

    pub(in crate::features) fn is_recording(&self, session_id: &str) -> bool {
        self.statuses.contains_key(session_id)
    }

    pub(in crate::features) fn status(&self, session_id: &str) -> Option<RecordingStatus> {
        self.statuses.get(session_id).cloned()
    }

    pub(in crate::features) fn apply_status(&mut self, status: RecordingStatus) {
        if matches!(
            status.state,
            nyaterm_transport::RecordingStatusState::Recording
                | nyaterm_transport::RecordingStatusState::Degraded
                | nyaterm_transport::RecordingStatusState::Failed
        ) {
            self.statuses.insert(status.session_id.clone(), status);
        } else {
            self.statuses.remove(&status.session_id);
        }
        self.active_count = self.statuses.len();
    }

    pub(in crate::features) fn remove_status(&mut self, session_id: &str) {
        self.statuses.remove(session_id);
        self.active_count = self.statuses.len();
    }

    pub(in crate::features) fn busy_action(&self, session_id: &str) -> Option<&str> {
        self.busy_actions.get(session_id).map(String::as_str)
    }

    pub(in crate::features) fn begin_action(&mut self, session_id: &str, action: &str) -> bool {
        if self.busy_actions.contains_key(session_id) {
            return false;
        }
        self.busy_actions
            .insert(session_id.to_string(), action.to_string());
        true
    }

    pub(in crate::features) fn finish_action(&mut self, session_id: &str) {
        let current_id = self.current_session_id(session_id);
        self.busy_actions.remove(&current_id);
    }

    pub(in crate::features) fn current_session_id(&self, session_id: &str) -> String {
        let mut current = session_id;
        while let Some(next) = self.rekeyed_sessions.get(current) {
            current = next;
        }
        current.to_string()
    }

    pub(in crate::features) fn search_draft(&self) -> &str {
        &self.search_draft
    }

    pub(in crate::features) fn set_search_draft(&mut self, text: String) {
        self.search_draft = text;
    }

    pub(in crate::features) fn clear_search_draft(&mut self) {
        self.search_draft.clear();
    }

    pub(in crate::features) fn set_history_include_input(&self, enabled: bool) {
        self.pipeline.set_history_include_input(enabled);
    }

    pub(in crate::features) fn take_shutdown(
        &mut self,
    ) -> Option<impl FnOnce() -> Vec<String> + Send + 'static> {
        self.pipeline.take_shutdown()
    }

    pub(in crate::features) fn cleanup_writer_session(&self, session_id: &str) {
        self.pipeline.cleanup_session(session_id.to_string());
    }

    pub(in crate::features) fn write_local_message(
        &self,
        session_id: impl Into<String>,
        text: impl Into<String>,
    ) {
        self.pipeline.writer().write_local_message(session_id, text);
    }

    #[cfg(test)]
    pub(in crate::features) fn write_output(
        &self,
        session_id: impl Into<String>,
        text: impl Into<String>,
    ) {
        self.pipeline.write_output(session_id, text);
    }

    pub(in crate::features) fn write_input(
        &self,
        session_id: impl Into<String>,
        data: impl Into<Vec<u8>>,
    ) {
        self.pipeline.write_input(session_id, data);
    }

    pub(in crate::features) fn resync_input_line(
        &self,
        session_id: impl Into<String>,
        line: String,
    ) {
        self.pipeline.resync_input_line(session_id, line);
    }

    pub(in crate::features) fn write_raw_input(
        &self,
        session_id: impl Into<String>,
        data: impl Into<Vec<u8>>,
    ) {
        self.pipeline.write_raw_input(session_id, data);
    }

    pub(in crate::features) fn request_history_search(&self, key: RecordingHistorySearchKey) {
        self.pipeline.request_history_search(key);
    }

    pub(in crate::features) fn take_event_receiver(&mut self) -> Option<RecordingEventReceiver> {
        self.pipeline.take_event_receiver()
    }

    pub(in crate::features) fn cleanup_session(&mut self, session_id: &str) {
        self.prompts.remove(session_id);
        self.busy_actions.remove(session_id);
        self.statuses.remove(session_id);
        self.active_count = self.statuses.len();
        self.pipeline.cleanup_session(session_id.to_string());
    }

    pub(in crate::features) fn disconnect_session(&self, session_id: &str) {
        self.pipeline.disconnect_session(session_id.to_string());
    }

    pub(in crate::features) fn rekey_session(&mut self, old_id: &str, new_id: &str) {
        if old_id == new_id {
            return;
        }
        self.prompts.remove(old_id);
        self.prompts.remove(new_id);
        if let Some(action) = self.busy_actions.remove(old_id) {
            self.busy_actions.insert(new_id.to_string(), action);
        }
        self.rekeyed_sessions
            .insert(old_id.to_string(), new_id.to_string());
        self.pipeline
            .rekey_session(old_id.to_string(), new_id.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::RecordingFeatureState;

    #[test]
    fn recording_state_owns_runtime_and_session_cleanup_state() {
        let mut recording = RecordingFeatureState::new(1024);
        assert!(recording.begin_action("session-1", "record"));
        assert!(!recording.begin_action("session-1", "save"));

        let _writer = recording.writer();
        recording.cleanup_session("session-1");

        assert_eq!(recording.active_count(), 0);
        assert!(recording.busy_action("session-1").is_none());
    }

    #[test]
    fn sensitive_prompts_are_incremental_and_owned_by_each_session() {
        let mut recording = RecordingFeatureState::new(1024);
        let classify = |text: &str| text.trim() == "Password:" || text.trim() == "OTP:";
        recording.observe_prompt("first", "Pass", classify);
        recording.observe_prompt("first", "word:", classify);
        recording.observe_prompt("second", "shell$ ", classify);
        assert!(recording.input_is_sensitive("first"));
        assert!(!recording.input_is_sensitive("second"));
        recording.observe_prompt("first", "*", classify);
        assert!(recording.input_is_sensitive("first"));
        recording.observe_prompt("first", "\nready$ ", classify);
        assert!(!recording.input_is_sensitive("first"));
        recording.observe_prompt("second", "\nOTP:", classify);
        assert!(recording.input_is_sensitive("second"));
        recording.cleanup_session("second");
        assert!(!recording.input_is_sensitive("second"));
    }

    #[test]
    fn reconnect_moves_busy_action_once() {
        let mut recording = RecordingFeatureState::new(1024);
        assert!(recording.begin_action("s1", "record"));
        recording.rekey_session("s1", "s2");
        assert_eq!(recording.busy_action("s2"), Some("record"));
        assert!(recording.busy_action("s1").is_none());
    }
}
