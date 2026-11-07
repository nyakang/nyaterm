//! Desktop facade for the UI-independent recording writer.

pub(crate) use nyaterm_recording::pipeline::{
    ExecutionRecordingEvent, RecordingEventReceiver, RecordingHistorySearchEvent,
    RecordingHistorySearchKey, RecordingWriteEvent, RecordingWriteHandle, RecordingWritePipeline,
};

#[cfg(test)]
mod tests {
    use super::RecordingWritePipeline;
    use nyaterm_recording::recording::{
        ExistingFileBehavior, RecordingContext, RecordingMode, RecordingProfile,
        RecordingRotationPolicy,
    };
    use std::fs;
    use std::path::{Path, PathBuf};
    use time::OffsetDateTime;

    fn recording_context(session_id: &str) -> RecordingContext {
        RecordingContext {
            session_id: session_id.to_string(),
            session_name: session_id.to_string(),
            connection_id: None,
            connection_name: None,
            group_path: None,
            protocol: "local".to_string(),
            host: None,
            port: None,
            username: None,
            started_at: OffsetDateTime::now_utc(),
        }
    }

    fn recording_profile(path: &Path) -> RecordingProfile {
        RecordingProfile {
            mode: RecordingMode::Transcript,
            base_path: path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from(".")),
            path_template: path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("recording.log")
                .to_string(),
            include_timestamps: false,
            include_io_labels: true,
            include_session_metadata: false,
            rotation: RecordingRotationPolicy::Session,
            existing_file_behavior: ExistingFileBehavior::Overwrite,
            include_binary_transfer_payloads: false,
            include_input: true,
        }
    }

    #[test]
    fn raw_capture_marker_preserves_split_wire_bytes_without_duplicate_decoding_output() {
        use crate::models::{TerminalFrameOutputSubmission, TerminalFramePipeline};
        let root = nyaterm_core::test_support::TestTempDir::new("nyaterm-raw-frame-marker");
        let path = root.join("raw.log");
        let mut recording = RecordingWritePipeline::spawn(4096);
        let writer = recording.writer();
        let mut profile = recording_profile(&path);
        profile.mode = nyaterm_recording::recording::RecordingMode::Raw;
        writer
            .start(
                "raw".into(),
                recording_context("raw"),
                profile,
                Some(path.clone()),
                4096,
            )
            .unwrap();
        let mut frames = TerminalFramePipeline::spawn(writer.clone());
        let first = b"\xb2\xe2\xca";
        writer.write_raw_output("raw", first);
        frames.submit_outputs(vec![TerminalFrameOutputSubmission {
            session_id: "raw".into(),
            data: first.to_vec(),
            encoding: "GBK".into(),
            scrollback_limit: 100,
            raw_already_captured: true,
        }]);
        for part in [
            b"\xd4\xff\x00\x1b[".as_slice(),
            b"31m\xe4",
            b"\xb8",
            b"\xad",
        ] {
            frames.submit_output("raw", part.to_vec(), "GBK", 100);
        }
        frames.append_local_text("raw", "local status");
        frames.submit_decoded_output("raw".into(), "AI-visible text".into());
        frames.shutdown();
        recording.shutdown();
        assert_eq!(
            fs::read(path).unwrap(),
            b"\xb2\xe2\xca\xd4\xff\x00\x1b[31m\xe4\xb8\xad"
        );
    }

    #[test]
    fn frame_system_diagnostics_preserve_recording_order_charset_and_raw_stream() {
        use crate::models::TerminalFramePipeline;
        for mode in [RecordingMode::Transcript, RecordingMode::Raw] {
            let root =
                nyaterm_core::test_support::TestTempDir::new("nyaterm-system-frame-recording");
            let path = root.join("session.log");
            let mut recording = RecordingWritePipeline::spawn(4096);
            let writer = recording.writer();
            let mut profile = recording_profile(&path);
            profile.mode = mode;
            writer
                .start(
                    "s1".into(),
                    recording_context("s1"),
                    profile,
                    Some(path.clone()),
                    4096,
                )
                .unwrap();
            let mut frames = TerminalFramePipeline::spawn(writer.clone());
            frames.submit_output("s1", b"before\r\n\xe4".to_vec(), "UTF-8", 100);
            frames.submit_system_recording("s1", "fixture diagnostic".into());
            frames.submit_output("s1", b"\xb8\xad\r\nafter\r\n".to_vec(), "UTF-8", 100);
            let text = futures::executor::block_on(frames.request_all_text("s1".into()))
                .unwrap()
                .unwrap();
            assert!(text.contains("中"));
            assert!(!text.contains("fixture diagnostic"));
            frames.shutdown();
            recording.shutdown();
            let bytes = fs::read(&path).unwrap();
            if mode == RecordingMode::Raw {
                assert_eq!(bytes, b"before\r\n\xe4\xb8\xad\r\nafter\r\n");
            } else {
                let transcript = String::from_utf8(bytes).unwrap();
                assert!(transcript.contains("[SYSTEM] fixture diagnostic"));
                assert!(
                    transcript.find("before").unwrap()
                        < transcript.find("fixture diagnostic").unwrap()
                );
                assert!(
                    transcript.find("fixture diagnostic").unwrap()
                        < transcript.find("after").unwrap()
                );
            }
        }
    }
}
