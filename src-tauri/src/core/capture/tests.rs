#[cfg(test)]
mod tests {
    use super::*;

    fn register_capture(
        proc: &mut OutputCaptureProcessor,
        marker_id: &str,
    ) -> oneshot::Receiver<CapturedOutput> {
        let (tx, rx) = oneshot::channel();
        proc.register(marker_id.to_string(), tx);
        rx
    }

    #[test]
    fn builders_do_not_embed_matchable_markers_in_input_text() {
        for profile in [
            AiExecutionProfile::Posix,
            AiExecutionProfile::Powershell,
            AiExecutionProfile::Cmd,
        ] {
            let command = build_capture_command(profile, "marker-1", "echo ok").unwrap();
            assert!(!command.contains("__DF_CMD_START_marker-1__"));
            assert!(!command.contains("__DF_CMD_END_marker-1_0__"));
            assert!(!command.contains("__NT_S_marker-1__"));
            assert!(!command.contains("__NT_E_marker-1_0__"));
        }
    }

    #[test]
    fn powershell_builder_is_single_logical_input_line() {
        let command = build_capture_command(
            AiExecutionProfile::Powershell,
            "marker-1",
            "Write-Output 'ok'\r\n# comment",
        )
        .unwrap();
        let command = command.strip_suffix("\r\n").unwrap();

        assert!(!command.contains('\r'));
        assert!(!command.contains('\n'));
        assert!(command.contains("[scriptblock]::Create($nyaiScript)"));
        assert!(!command.contains("Write-Output 'ok'"));
    }

    #[test]
    fn cmd_builder_is_single_logical_input_line() {
        let command =
            build_capture_command(AiExecutionProfile::Cmd, "marker-1", "echo one\r\necho two")
                .unwrap();
        let command = command.strip_suffix("\r\n").unwrap();

        assert!(!command.contains('\r'));
        assert!(!command.contains('\n'));
        assert!(command.contains("echo one & echo two"));
        assert!(command.contains("call echo"));
        assert!(command.contains("^%ERRORLEVEL^%"));
    }

    #[test]
    fn unsupported_profiles_do_not_build_capture_commands() {
        for profile in [
            AiExecutionProfile::Auto,
            AiExecutionProfile::SendOnly,
            AiExecutionProfile::Disabled,
        ] {
            assert!(build_capture_command(profile, "marker-1", "echo ok").is_none());
        }
    }

    #[tokio::test]
    async fn captures_crlf_output_with_prompt_before_markers() {
        let mut proc = OutputCaptureProcessor::new();
        let rx = register_capture(&mut proc, "m1");

        let visible = proc.process(
            "C:\\>echo marker\r\n__DF_CMD_START_m1__\r\nok\r\n__DF_CMD_END_m1_7__\r\nC:\\>",
        );
        assert_eq!(visible, "\r\nC:\\>");

        let captured = rx.await.unwrap();
        assert_eq!(captured.output, "ok");
        assert_eq!(captured.exit_code, Some(7));
    }

    #[tokio::test]
    async fn captures_start_marker_split_across_chunks() {
        let mut proc = OutputCaptureProcessor::new();
        let rx = register_capture(&mut proc, "m2");

        assert!(proc.process("__DF_CMD_STA").is_empty());
        assert!(proc.process("RT_m2__\nhello\n").is_empty());
        assert_eq!(proc.process("__DF_CMD_END_m2_0__\n"), "\n");

        let captured = rx.await.unwrap();
        assert_eq!(captured.output, "hello");
        assert_eq!(captured.exit_code, Some(0));
    }

    #[tokio::test]
    async fn captures_end_marker_split_across_chunks() {
        let mut proc = OutputCaptureProcessor::new();
        let rx = register_capture(&mut proc, "m3");

        assert!(
            proc.process("__DF_CMD_START_m3__\nhello\n__DF_CMD_EN")
                .is_empty()
        );
        assert_eq!(proc.process("D_m3_9__\n"), "\n");

        let captured = rx.await.unwrap();
        assert_eq!(captured.output, "hello");
        assert_eq!(captured.exit_code, Some(9));
    }

    #[tokio::test]
    async fn caps_large_capture_output_at_the_source() {
        let mut proc = OutputCaptureProcessor::new();
        let rx = register_capture(&mut proc, "large");

        assert!(proc.process("__DF_CMD_START_large__\n").is_empty());
        assert!(proc.process(&"猫".repeat(MAX_CAPTURE_BYTES)).is_empty());
        assert_eq!(proc.process("__DF_CMD_END_large_0__\n"), "\n");

        let captured = rx.await.unwrap();
        assert!(captured.output.len() <= MAX_CAPTURE_BYTES);
        assert!(captured.source_truncated);
        assert!(captured.output.is_char_boundary(captured.output.len()));
    }

    #[tokio::test]
    async fn every_marker_split_preserves_output_after_end() {
        let stream = "echo\r\n__DF_CMD_START_split__\r\n猫🙂\r\n__DF_CMD_END_split_-12__\r\nprompt> notification";
        for split in stream.char_indices().map(|(idx, _)| idx) {
            let mut proc = OutputCaptureProcessor::new();
            let rx = register_capture(&mut proc, "split");
            let visible = proc.process(&stream[..split]) + &proc.process(&stream[split..]);
            let result = rx.await.unwrap();
            assert_eq!(result.output, "猫🙂", "split at {split}");
            assert_eq!(result.exit_code, Some(-12));
            assert_eq!(visible, "\r\nprompt> notification", "split at {split}");
            assert!(!proc.has_active());
            assert_eq!(proc.process("later output"), "later output");
        }
        let mut proc = OutputCaptureProcessor::new();
        let rx = register_capture(&mut proc, "split");
        let visible = stream
            .chars()
            .map(|ch| proc.process(&ch.to_string()))
            .collect::<String>();
        assert_eq!(rx.await.unwrap().output, "猫🙂");
        assert_eq!(visible, "\r\nprompt> notification");
    }

    #[tokio::test]
    async fn powershell_short_markers_survive_every_stream_boundary() {
        let stream = "echo__NT_S_other__echo__NT_S_osc__\r\n猫\r\n__NT_E_osc_0__\r\nprompt";
        for split in stream.char_indices().map(|(idx, _)| idx) {
            let mut proc = OutputCaptureProcessor::new();
            let rx = register_capture(&mut proc, "osc");
            let visible = proc.process(&stream[..split]) + &proc.process(&stream[split..]);
            let result = rx.await.unwrap();
            assert_eq!(result.output, "猫", "split at {split}");
            assert_eq!(visible, "\r\nprompt", "split at {split}");
        }
        let mut proc = OutputCaptureProcessor::new();
        let rx = register_capture(&mut proc, "osc");
        let visible = stream
            .chars()
            .map(|ch| proc.process(&ch.to_string()))
            .collect::<String>();
        assert_eq!(rx.await.unwrap().output, "猫");
        assert_eq!(visible, "\r\nprompt");
    }

    #[tokio::test]
    async fn single_chunk_output_is_bounded_and_invalid_markers_are_preserved() {
        let mut proc = OutputCaptureProcessor::new();
        let rx = register_capture(&mut proc, "bounded");
        let text = format!(
            "__DF_CMD_START_bounded__\n{}__DF_CMD_END_bounded_0__\nnotice",
            "猫".repeat(MAX_CAPTURE_BYTES)
        );
        assert_eq!(proc.process(&text), "\nnotice");
        let result = rx.await.unwrap();
        assert!(result.output.len() <= MAX_CAPTURE_BYTES);
        assert!(result.source_truncated);

        let rx = register_capture(&mut proc, "valid");
        proc.process("__DF_CMD_START_valid__\n__DF_CMD_END_valid_nope__\n__DF_CMD_END_other_0__\n__DF_CMD_START_valid__\n__DF_CMD_END_valid_0__");
        assert_eq!(
            rx.await.unwrap().output,
            "__DF_CMD_END_valid_nope__\n__DF_CMD_END_other_0__\n__DF_CMD_START_valid__"
        );
    }

    #[tokio::test]
    async fn cancellation_keeps_ownership_until_late_end_and_strips_partial_markers() {
        use crate::core::session::{SessionManager, TerminalExecutionState};
        let manager = Arc::new(SessionManager::new());
        let mut proc = OutputCaptureProcessor::new();
        let (tx, rx) = oneshot::channel();
        let guard = manager
            .begin_terminal_execution("s1", "cancelled")
            .await
            .unwrap();
        assert!(proc.register_execution("cancelled".into(), tx, Some(guard)));
        assert!(
            manager
                .begin_terminal_execution("s1", "other")
                .await
                .is_err()
        );
        let parallel = manager
            .begin_terminal_execution("s2", "other")
            .await
            .unwrap();
        drop(parallel);
        proc.process("__DF_CMD_START_cancelled__\nsecret\n__DF_CMD_E");
        proc.cancel("cancelled");
        assert!(rx.await.is_err());
        assert!(matches!(
            manager.terminal_execution_state("s1"),
            TerminalExecutionState::AwaitingEnd
        ));
        assert_eq!(proc.process("ND_cancelled_0__\nprompt"), "\nprompt");
        assert!(!proc.has_active());
        assert!(matches!(
            manager.terminal_execution_state("s1"),
            TerminalExecutionState::Idle
        ));
        assert!(manager.begin_terminal_execution("s1", "next").await.is_ok());
    }

    #[tokio::test]
    async fn abandoned_capture_streams_output_and_releases_on_disconnect_or_trusted_prompt() {
        let manager = Arc::new(crate::core::session::SessionManager::new());
        let mut proc = OutputCaptureProcessor::new();
        let (tx, _rx) = oneshot::channel();
        proc.register_execution(
            "abandoned".into(),
            tx,
            Some(
                manager
                    .begin_terminal_execution("s", "abandoned")
                    .await
                    .unwrap(),
            ),
        );
        proc.cancel("abandoned");
        assert_eq!(
            proc.process("__DF_CMD_START_abandoned__\nnormal output"),
            "normal output"
        );
        let (tx, mut rx) = oneshot::channel();
        assert!(!proc.register_execution("second".into(), tx, None));
        assert!(rx.try_recv().is_err());
        proc.finish_abandoned_at_prompt();
        assert!(manager.begin_terminal_execution("s", "next").await.is_ok());
    }

    #[test]
    fn forwards_terminal_queries_even_when_echo_is_hidden() {
        let mut proc = OutputCaptureProcessor::new();
        let _rx = register_capture(&mut proc, "query");
        assert_eq!(proc.process("echo\x1b["), "");
        assert_eq!(
            proc.process("6n__DF_CMD_START_query__\noutput\x1b[>0c"),
            "\x1b[6n\x1b[>0c"
        );
        assert_eq!(proc.process("__DF_CMD_END_query_0__\nprompt"), "\nprompt");
    }
}
