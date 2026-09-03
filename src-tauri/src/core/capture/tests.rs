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
        assert!(visible.is_empty());

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
        assert!(proc.process("__DF_CMD_END_m2_0__\n").is_empty());

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
        assert!(proc.process("D_m3_9__\n").is_empty());

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
        assert!(proc.process("__DF_CMD_END_large_0__\n").is_empty());

        let captured = rx.await.unwrap();
        assert!(captured.output.len() <= MAX_CAPTURE_BYTES);
        assert!(captured.source_truncated);
        assert!(captured.output.is_char_boundary(captured.output.len()));
    }
}
