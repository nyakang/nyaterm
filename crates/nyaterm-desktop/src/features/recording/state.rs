use std::collections::HashMap;
use std::time::Instant;

use nyaterm_terminal::recording_sanitizer::RecordingSanitizer;
use nyaterm_transport::RecordingStatus;

use crate::models::{
    RecordingEventReceiver, RecordingHistorySearchKey, RecordingWriteHandle, RecordingWritePipeline,
};

pub(in crate::features) struct RecordingFeatureState {
    active_count: usize,
    pipeline: RecordingWritePipeline,
    macro_memory_limit: usize,
    macros: HashMap<String, MacroRecording>,
    search_draft: String,
    busy_actions: HashMap<String, String>,
    rekeyed_sessions: HashMap<String, String>,
    statuses: HashMap<String, RecordingStatus>,
    prompts: HashMap<String, RecordingPrompt>,
}

/// One successful user input write. Pauses are measured before this write.
pub(in crate::features) struct MacroInputChunk {
    pub(in crate::features) delay_seconds: u64,
    pub(in crate::features) bytes: Vec<u8>,
}

#[derive(Default)]
struct MacroRecording {
    chunks: Vec<MacroInputChunk>,
    size_bytes: usize,
    last_input_at: Option<Instant>,
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
            macro_memory_limit: memory_limit_bytes,
            macros: HashMap::new(),
            search_draft: String::new(),
            busy_actions: HashMap::new(),
            rekeyed_sessions: HashMap::new(),
            statuses: HashMap::new(),
            prompts: HashMap::new(),
        }
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

    pub(in crate::features) fn set_memory_limit(&mut self, memory_limit_bytes: usize) {
        self.macro_memory_limit = memory_limit_bytes;
        self.pipeline.set_memory_limit(memory_limit_bytes);
    }

    pub(in crate::features) fn start_macro(&mut self, session_id: &str) -> bool {
        if self.macros.contains_key(session_id) {
            return false;
        }
        self.macros
            .insert(session_id.to_string(), MacroRecording::default());
        true
    }

    pub(in crate::features) fn is_macro_recording(&self, session_id: &str) -> bool {
        self.macros.contains_key(session_id)
    }

    pub(in crate::features) fn macro_bytes(&self, session_id: &str) -> Option<usize> {
        self.macros.get(session_id).map(|draft| draft.size_bytes)
    }

    /// Returns true if the memory limit was exceeded and the draft was discarded.
    pub(in crate::features) fn append_macro_input(
        &mut self,
        session_id: &str,
        bytes: &[u8],
        at: Instant,
    ) -> bool {
        let Some(draft) = self.macros.get_mut(session_id) else {
            return false;
        };
        if bytes.is_empty() {
            return false;
        }
        if bytes.len() > self.macro_memory_limit.saturating_sub(draft.size_bytes) {
            self.macros.remove(session_id);
            return true;
        }
        let pause = draft.last_input_at.map_or(0, |previous| {
            at.saturating_duration_since(previous).as_secs()
        });
        // Quick Commands only support one-second pauses; bound long idle periods.
        let pause = pause.min(60);
        if pause == 0
            && let Some(last) = draft.chunks.last_mut()
        {
            last.bytes.extend_from_slice(bytes);
        } else {
            draft.chunks.push(MacroInputChunk {
                delay_seconds: pause,
                bytes: bytes.to_vec(),
            });
        }
        draft.size_bytes += bytes.len();
        draft.last_input_at = Some(at);
        false
    }

    pub(in crate::features) fn finish_macro(
        &mut self,
        session_id: &str,
    ) -> Option<Vec<MacroInputChunk>> {
        self.macros.remove(session_id).map(|draft| draft.chunks)
    }

    pub(in crate::features) fn discard_macro(&mut self, session_id: &str) {
        self.macros.remove(session_id);
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
        self.discard_macro(session_id);
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
        self.discard_macro(old_id);
        self.discard_macro(new_id);
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
    use std::time::{Duration, Instant};

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

    #[test]
    fn macro_records_only_successfully_appended_session_bytes_and_whole_second_pauses() {
        let mut recording = RecordingFeatureState::new(1024);
        let start = Instant::now();
        assert!(recording.start_macro("first"));
        assert!(!recording.start_macro("first"));
        assert!(!recording.append_macro_input("other", b"ignored", start));
        assert!(!recording.append_macro_input("first", b"hello", start));
        assert!(!recording.append_macro_input("first", b"\r", start + Duration::from_millis(500)));
        assert!(!recording.append_macro_input(
            "first",
            b"world",
            start + Duration::from_millis(1700)
        ));
        assert_eq!(recording.macro_bytes("first"), Some(11));
        let chunks = recording.finish_macro("first").unwrap();
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].bytes, b"hello\r");
        assert_eq!(chunks[0].delay_seconds, 0);
        assert_eq!(chunks[1].bytes, b"world");
        assert_eq!(chunks[1].delay_seconds, 1);
        assert!(!recording.is_macro_recording("first"));
        assert!(recording.finish_macro("first").is_none());
    }

    #[test]
    fn macro_discards_on_limit_disconnect_cleanup_and_rekey() {
        let mut recording = RecordingFeatureState::new(4);
        let now = Instant::now();
        recording.start_macro("first");
        recording.append_macro_input("first", b"abcd", now);
        assert!(recording.append_macro_input("first", b"x", now));
        assert!(recording.finish_macro("first").is_none());
        recording.start_macro("first");
        recording.cleanup_session("first");
        assert!(!recording.is_macro_recording("first"));
        recording.start_macro("first");
        recording.start_macro("second");
        recording.rekey_session("first", "second");
        assert!(!recording.is_macro_recording("first"));
        assert!(!recording.is_macro_recording("second"));
        recording.start_macro("third");
        assert!(recording.finish_macro("third").unwrap().is_empty());
    }
}
