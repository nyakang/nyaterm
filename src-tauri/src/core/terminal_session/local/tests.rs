#[cfg(test)]
mod tests {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    use super::{configure_local_pty_environment, local_shell_line_editor_ready};
    #[cfg(unix)]
    use super::prepare_local_startup_injection;
    #[cfg(target_os = "macos")]
    use super::is_utf8_locale;
    use super::{
        InputOrigin, LocalSessionConfig, LocalStartupPipeline, OscStripper, ShellResolutionSource,
        StartupGateOutput, StartupInjectionAttempt, StartupInjectionDecision, StartupInjectionSchedule,
        StartupInputBarrier, StartupOutputGate, StdMutex, TerminalQueryDetector, WorkingDirectoryOutcome, ZmodemDetectResult, ZmodemDetector,
        ZmodemDirection, apply_working_dir_to_command, build_cmd_prompt,
        build_local_startup_script, cancel_startup_for_pending_zmodem, detect_local_zmodem,
        drain_startup_pipeline_on_cancel, input_cancels_startup_injection,
        is_unresolved_shell_wrapper, lock_or_recover, managed_cwd_runtime_ready,
        parse_shell_args, reconcile_managed_cwd_payloads, resolve_shell_command,
        should_allow_local_injection,
        validate_working_dir_before_spawn,
    };
    use crate::core::ssh::osc::build_ready_marker;
    use portable_pty::CommandBuilder;
    use std::sync::Arc;

    fn ready_marker() -> String {
        build_ready_marker("session-1")
    }

    fn argv_strings(cmd: &CommandBuilder) -> Vec<String> {
        cmd.get_argv()
            .iter()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect()
    }

    #[test]
    fn shell_path_with_spaces_stays_single_program() {
        let spec =
            resolve_shell_command(r"D:\Soft wares\Git\bin\bash.exe", "").expect("command spec");

        assert_eq!(spec.program, r"D:\Soft wares\Git\bin\bash.exe");
        assert!(spec.args.is_empty());
    }

    #[test]
    fn shell_args_are_split_separately_from_program() {
        let spec = resolve_shell_command("pwsh.exe", "-NoLogo -NoExit").expect("command spec");

        assert!(
            spec.program.ends_with("pwsh.exe"),
            "program was {}",
            spec.program
        );
        assert_eq!(spec.args, vec!["-NoLogo", "-NoExit"]);
    }

    #[test]
    fn unix_bash_defaults_to_login_interactive_when_args_are_empty() {
        let spec = resolve_shell_command("/bin/bash", "").expect("command spec");

        assert_eq!(spec.program, "/bin/bash");
        if cfg!(windows) {
            assert!(spec.args.is_empty());
        } else {
            assert_eq!(spec.args, vec!["--login", "-i"]);
        }
    }

    #[test]
    fn explicit_shell_args_override_unix_interactive_defaults() {
        let spec = resolve_shell_command("/bin/bash", "--noprofile --norc").expect("command spec");

        assert_eq!(spec.program, "/bin/bash");
        assert_eq!(spec.args, vec!["--noprofile", "--norc"]);
    }

    #[test]
    fn legacy_shell_path_command_with_args_is_still_supported() {
        let spec = resolve_shell_command("pwsh.exe -NoLogo", "").expect("command spec");

        assert!(
            spec.program.ends_with("pwsh.exe"),
            "program was {}",
            spec.program
        );
        assert_eq!(spec.args, vec!["-NoLogo"]);
    }

    #[test]
    fn windows_builtin_shell_names_resolve_to_spawnable_programs() {
        if !cfg!(windows) {
            return;
        }

        for shell in ["cmd.exe", "powershell.exe"] {
            let spec = resolve_shell_command(shell, "").expect("command spec");

            assert!(
                spec.program.contains('\\') || spec.program.contains('/'),
                "{shell} should resolve to an absolute executable path, got {}",
                spec.program
            );
            assert!(
                spec.program.to_ascii_lowercase().ends_with(shell),
                "{shell} resolved to unexpected program {}",
                spec.program
            );
        }
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn windows_terminal_alias_does_not_spawn_wt_host() {
        let spec = resolve_shell_command("wt.exe", "").expect("command spec");

        assert!(
            !spec.program.to_ascii_lowercase().ends_with("wt.exe"),
            "wt.exe should resolve to an embeddable shell, got {}",
            spec.program
        );
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn windows_terminal_profile_commandline_is_parsed_as_shell_spec() {
        let spec = super::shell_spec_from_windows_commandline(
            r#"%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe -NoLogo"#,
            vec!["-NoExit".to_string()],
        )
        .expect("command spec");

        assert!(
            spec.program
                .to_ascii_lowercase()
                .ends_with("powershell.exe"),
            "program was {}",
            spec.program
        );
        assert_eq!(spec.args, vec!["-NoLogo", "-NoExit"]);
        assert_eq!(
            spec.resolution_source,
            ShellResolutionSource::WindowsTerminalProfile
        );
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn windows_terminal_fallback_is_marked_as_passive() {
        let spec = super::fallback_windows_terminal_shell(Vec::new());

        assert_eq!(
            spec.resolution_source,
            ShellResolutionSource::WindowsTerminalFallback
        );
        assert!(!should_allow_local_injection(
            "wt.exe",
            &spec.program,
            "",
            spec.args.is_empty(),
            spec.resolution_source,
        ));
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn windows_terminal_dynamic_powershell_core_profile_resolves_pwsh() {
        let profile = serde_json::json!({
            "guid": "{574e775e-4f2a-5b96-ac1e-a2962a402336}",
            "name": "PowerShell",
            "source": "Windows.Terminal.PowershellCore"
        });

        let commandline = super::windows_terminal_profile_commandline(&profile)
            .expect("PowerShell Core commandline");
        assert_eq!(commandline, "pwsh.exe");
        let spec = super::shell_spec_from_windows_commandline(&commandline, Vec::new())
            .expect("PowerShell Core shell spec");
        assert!(
            spec.program.to_ascii_lowercase().ends_with("pwsh.exe"),
            "PowerShell Core profile resolved to {}",
            spec.program
        );
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn windows_terminal_dynamic_wsl_source_stays_passive() {
        let profile = serde_json::json!({
            "name": "PowerShell-looking Linux",
            "source": "Windows.Terminal.Wsl"
        });

        assert_eq!(
            super::windows_terminal_profile_commandline(&profile).as_deref(),
            Some("wsl.exe")
        );
    }

    #[test]
    fn shell_args_support_quotes_and_windows_paths() {
        let args = parse_shell_args(r#"-Command "echo hi" "C:\Program Files\Tool""#).expect("args");

        assert_eq!(args, vec!["-Command", "echo hi", r"C:\Program Files\Tool"]);
    }

    #[test]
    fn wsl_working_dir_uses_cd_arg_before_user_args() {
        let mut cmd = CommandBuilder::new("wsl.exe");
        cmd.args(["-d", "Ubuntu"]);

        let outcome = apply_working_dir_to_command(&mut cmd, "/home/nya");

        assert_eq!(outcome, WorkingDirectoryOutcome::AppliedWslCd);
        assert_eq!(
            argv_strings(&cmd),
            vec!["wsl.exe", "--cd", "/home/nya", "-d", "Ubuntu"]
        );
        assert!(cmd.get_cwd().is_none());
    }

    #[test]
    fn wsl_working_dir_accepts_home_marker() {
        let mut cmd = CommandBuilder::new("wsl.exe");

        let outcome = apply_working_dir_to_command(&mut cmd, "~");

        assert_eq!(outcome, WorkingDirectoryOutcome::AppliedWslCd);
        assert_eq!(argv_strings(&cmd), vec!["wsl.exe", "--cd", "~"]);
        assert!(cmd.get_cwd().is_none());
    }

    #[test]
    fn wsl_working_dir_accepts_windows_path_without_local_validation() {
        let mut cmd = CommandBuilder::new(r"C:\Windows\System32\wsl.exe");

        let outcome = apply_working_dir_to_command(&mut cmd, r"C:\Projects");

        assert_eq!(outcome, WorkingDirectoryOutcome::AppliedWslCd);
        assert_eq!(
            argv_strings(&cmd),
            vec![r"C:\Windows\System32\wsl.exe", "--cd", r"C:\Projects"]
        );
        assert!(cmd.get_cwd().is_none());
    }

    #[test]
    fn wsl_working_dir_does_not_duplicate_explicit_cd_arg() {
        let mut cmd = CommandBuilder::new("wsl.exe");
        cmd.args(["--cd", "/explicit"]);

        let outcome = apply_working_dir_to_command(&mut cmd, "/home/nya");

        assert_eq!(outcome, WorkingDirectoryOutcome::WslCdAlreadyConfigured);
        assert_eq!(argv_strings(&cmd), vec!["wsl.exe", "--cd", "/explicit"]);
        assert!(cmd.get_cwd().is_none());
    }

    #[test]
    fn local_shell_valid_working_dir_uses_process_cwd() {
        let current_dir = std::env::current_dir().expect("current dir");
        let current_dir_string = current_dir.to_string_lossy().to_string();
        let mut cmd = CommandBuilder::new("pwsh.exe");

        let outcome = apply_working_dir_to_command(&mut cmd, &current_dir_string);

        assert_eq!(outcome, WorkingDirectoryOutcome::AppliedLocalCwd);
        assert_eq!(
            cmd.get_cwd().and_then(|cwd| cwd.to_str()),
            Some(current_dir_string.as_str())
        );
    }

    #[test]
    fn local_shell_missing_working_dir_falls_back() {
        let missing_dir = std::env::current_dir()
            .expect("current dir")
            .join(format!("__nyaterm_missing_work_dir_{}__", std::process::id()));
        assert!(!missing_dir.exists());
        let missing_dir_string = missing_dir.to_string_lossy().to_string();
        let mut cmd = CommandBuilder::new("pwsh.exe");

        let outcome = apply_working_dir_to_command(&mut cmd, &missing_dir_string);

        assert_eq!(outcome, WorkingDirectoryOutcome::MissingLocalDirectory);
        assert!(cmd.get_cwd().is_none());
    }

    #[test]
    fn explicit_missing_working_dir_is_rejected_before_spawn() {
        let missing_dir = std::env::current_dir()
            .expect("current dir")
            .join(format!(
                "__nyaterm_missing_explicit_work_dir_{}__",
                std::process::id()
            ));
        assert!(!missing_dir.exists());
        let config = LocalSessionConfig {
            connection_id: None,
            shell_path: "pwsh.exe".to_string(),
            shell_args: String::new(),
            working_dir: Some(missing_dir.to_string_lossy().to_string()),
            fail_on_missing_working_dir: true,
            name: "Local Terminal".to_string(),
            encoding: "UTF-8".to_string(),
            dynamic_tab_title: false,
        };

        let shell_spec = resolve_shell_command(&config.shell_path, &config.shell_args)
            .expect("shell command spec");
        assert!(
            validate_working_dir_before_spawn(Some(&config), &shell_spec).is_err()
        );
    }

    #[test]
    fn saved_missing_working_dir_preserves_fallback_behavior() {
        let missing_dir = std::env::current_dir()
            .expect("current dir")
            .join(format!(
                "__nyaterm_missing_saved_work_dir_{}__",
                std::process::id()
            ));
        assert!(!missing_dir.exists());
        let config = LocalSessionConfig {
            connection_id: Some("saved-local".to_string()),
            shell_path: "pwsh.exe".to_string(),
            shell_args: String::new(),
            working_dir: Some(missing_dir.to_string_lossy().to_string()),
            fail_on_missing_working_dir: false,
            name: "Saved Local".to_string(),
            encoding: "UTF-8".to_string(),
            dynamic_tab_title: false,
        };

        let shell_spec = resolve_shell_command(&config.shell_path, &config.shell_args)
            .expect("shell command spec");
        assert!(
            validate_working_dir_before_spawn(Some(&config), &shell_spec).is_ok()
        );
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn utf8_locale_detection_accepts_common_spellings() {
        assert!(is_utf8_locale("en_US.UTF-8"));
        assert!(is_utf8_locale("zh_CN.utf8"));
        assert!(!is_utf8_locale("C"));
        assert!(!is_utf8_locale("zh_CN.GBK"));
    }

    #[test]
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn local_pty_environment_sets_terminal() {
        let shell = if cfg!(target_os = "macos") {
            "/bin/zsh"
        } else {
            "/bin/bash"
        };
        let mut cmd = CommandBuilder::new(shell);
        configure_local_pty_environment(&mut cmd);

        assert_eq!(
            cmd.get_env("TERM").and_then(|value| value.to_str()),
            Some("xterm-256color")
        );
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn local_pty_environment_sets_utf8_locale() {
        let mut cmd = CommandBuilder::new("/bin/zsh");
        configure_local_pty_environment(&mut cmd);

        assert!(
            cmd.get_env("LANG")
                .and_then(|value| value.to_str())
                .is_some_and(is_utf8_locale)
        );
        assert!(
            cmd.get_env("LC_CTYPE")
                .and_then(|value| value.to_str())
                .is_some_and(is_utf8_locale)
        );
    }

    #[test]
    fn unix_injection_waits_for_initial_output_to_become_idle() {
        let start = std::time::Instant::now();
        let mut schedule = StartupInjectionSchedule::new(
            start,
            std::time::Duration::from_millis(25),
            std::time::Duration::from_millis(250),
        );
        assert_eq!(schedule.decision(start), StartupInjectionDecision::Wait);
        schedule.note_activity(start + std::time::Duration::from_millis(10));
        assert_eq!(
            schedule.decision(start + std::time::Duration::from_millis(34)),
            StartupInjectionDecision::Wait
        );
        assert_eq!(
            schedule.decision(start + std::time::Duration::from_millis(35)),
            StartupInjectionDecision::Inject
        );
    }

    #[test]
    fn terminal_response_requires_newer_shell_output_before_injection() {
        let start = std::time::Instant::now();
        let mut schedule = StartupInjectionSchedule::new(
            start,
            std::time::Duration::from_millis(25),
            std::time::Duration::from_millis(250),
        );
        schedule.note_activity(start + std::time::Duration::from_millis(10));
        schedule.require_output_after_terminal_response(
            start + std::time::Duration::from_millis(20),
        );
        schedule.note_activity(start + std::time::Duration::from_millis(19));
        assert_eq!(
            schedule.decision(start + std::time::Duration::from_millis(100)),
            StartupInjectionDecision::Wait
        );
        schedule.note_activity(start + std::time::Duration::from_millis(21));
        assert_eq!(
            schedule.decision(start + std::time::Duration::from_millis(46)),
            StartupInjectionDecision::Inject
        );
    }

    #[test]
    fn unix_injection_timeout_fails_passive_without_output() {
        let start = std::time::Instant::now();
        let schedule = StartupInjectionSchedule::new(
            start,
            std::time::Duration::from_millis(10),
            std::time::Duration::from_millis(20),
        );
        assert_eq!(
            schedule.decision(start + std::time::Duration::from_millis(20)),
            StartupInjectionDecision::Timeout
        );
    }

    #[test]
    fn terminal_query_detector_handles_split_csi_osc_and_dcs() {
        let mut detector = TerminalQueryDetector::new();
        assert_eq!(detector.push("banner\x1b["), 0);
        assert_eq!(detector.push("c\x1b]11;?\x1b"), 1);
        assert_eq!(detector.push("\\\x1bP+q544e\x1b\\"), 2);
        assert_eq!(detector.push("\x1b]2;?\x07\x1b[8;24;80t"), 0);
    }

    #[test]
    fn disabled_title_queries_do_not_hold_startup_barrier() {
        let mut detector = TerminalQueryDetector::new();
        assert_eq!(detector.push("\x1b[20t\x1b[21t"), 0);
        assert!(!detector.has_pending_sequence());
    }

    #[test]
    fn inactive_startup_gate_passes_terminal_queries_until_injection() {
        let gate = StartupOutputGate::new_inactive();
        let query = "\u{1b}[c\u{1b}]11;?\u{1b}\\".to_string();
        assert_eq!(
            gate.consume(query.clone(), String::new(), false),
            StartupGateOutput::Pass(query)
        );
        gate.activate();
        assert_eq!(
            gate.consume("injected echo\x1b[".to_string(), String::new(), false),
            StartupGateOutput::Buffered(String::new())
        );
        assert_eq!(
            gate.consume(
                "c\x1b]11;?\x1b\\\x1bP+q544e\x1b\\hidden".to_string(),
                String::new(),
                false,
            ),
            StartupGateOutput::Buffered(
                "\x1b[c\x1b]11;?\x1b\\\x1bP+q544e\x1b\\".to_string()
            )
        );
    }

    #[test]
    fn startup_gate_passes_zero_padded_osc_title_selectors() {
        let gate = StartupOutputGate::new();
        let title = "\x1b]02;zero padded\x07";
        assert_eq!(
            gate.consume(title.to_string(), String::new(), false),
            StartupGateOutput::Buffered(title.to_string())
        );
    }

    #[test]
    fn startup_gate_passes_c1_osc_titles_without_interpreting_payload() {
        let gate = StartupOutputGate::new();
        let title = "\u{009d}2;C1 title\u{009c}";
        assert_eq!(
            gate.consume(title.to_string(), String::new(), false),
            StartupGateOutput::Buffered(title.to_string())
        );
    }

    #[test]
    fn only_terminal_responses_may_cross_the_pre_injection_barrier() {
        assert!(!input_cancels_startup_injection(
            InputOrigin::TerminalResponse
        ));
        for origin in [
            InputOrigin::Keyboard,
            InputOrigin::QuickCommand,
            InputOrigin::StartupCommand,
            InputOrigin::PostLogin,
            InputOrigin::AiAgent,
            InputOrigin::CredentialAutofill,
            InputOrigin::OtpAutofill,
            InputOrigin::SyncInput,
        ] {
            assert!(input_cancels_startup_injection(origin), "{origin:?}");
        }
    }

    #[test]
    fn cancelling_before_gate_activation_releases_partial_terminal_query() {
        let marker = ready_marker();
        let pipeline: Arc<StdMutex<LocalStartupPipeline>> = Arc::new(StdMutex::new((
            OscStripper::new(&marker),
            StartupOutputGate::new_inactive(),
        )));
        {
            let mut locked = pipeline.lock().unwrap();
            let parsed = locked.0.push("\x1b]11;?");
            assert!(parsed.visible.is_empty());
        }
        assert_eq!(
            drain_startup_pipeline_on_cancel(&pipeline),
            "\x1b]11;?"
        );
    }

    #[test]
    fn managed_cwd_split_before_ready_is_released_with_the_marker() {
        let initial = "file://localhost/Users/alice/project".to_string();
        let mut payloads = vec![initial.clone()];
        let mut pending = None;

        reconcile_managed_cwd_payloads(
            &mut payloads,
            &mut pending,
            true,
            true,
            false,
            false,
            true,
            false,
        );
        assert!(payloads.is_empty());
        assert_eq!(pending.as_deref(), Some(initial.as_str()));

        reconcile_managed_cwd_payloads(
            &mut payloads,
            &mut pending,
            true,
            true,
            false,
            true,
            true,
            false,
        );
        assert_eq!(payloads, vec![initial]);
        assert!(pending.is_none());
    }

    #[test]
    fn unmanaged_or_pre_injection_cwd_is_never_staged_as_managed() {
        let mut payloads = vec!["file://localhost/untrusted".to_string()];
        let mut pending = None;
        reconcile_managed_cwd_payloads(
            &mut payloads,
            &mut pending,
            true,
            true,
            false,
            false,
            false,
            false,
        );
        assert!(payloads.is_empty());
        assert!(pending.is_none());
    }

    #[test]
    fn managed_cwd_waits_for_ready_and_stops_after_runtime_failure() {
        assert!(!managed_cwd_runtime_ready(true, true, false, false));
        assert!(managed_cwd_runtime_ready(true, true, false, true));
        assert!(managed_cwd_runtime_ready(true, true, true, false));
        assert!(!managed_cwd_runtime_ready(true, false, true, false));
        assert!(managed_cwd_runtime_ready(false, false, false, false));
    }

    #[test]
    fn single_zpad_does_not_cancel_an_injection_that_already_won() {
        let barrier = Arc::new(StartupInputBarrier::new());
        assert_eq!(
            barrier.try_inject(|| Ok::<_, ()>(())).unwrap(),
            StartupInjectionAttempt::Injected(())
        );
        assert!(!cancel_startup_for_pending_zmodem(Some(&barrier)));
    }

    #[test]
    fn zmodem_detection_runs_when_ready_and_header_share_a_chunk() {
        let mut detector = ZmodemDetector::new();
        let marker = ready_marker();
        let mut payload = marker.as_bytes().to_vec();
        payload.extend_from_slice(b"**\x18B00payload");
        match detect_local_zmodem(&mut detector, &payload) {
            ZmodemDetectResult::Detected {
                direction,
                passthrough,
                initial_bytes,
            } => {
                assert_eq!(direction, ZmodemDirection::Download);
                assert_eq!(passthrough, marker.as_bytes());
                assert_eq!(initial_bytes, b"**\x18B00payload");
            }
            ZmodemDetectResult::NoMatch { .. } => panic!("expected ZMODEM detection"),
        }
    }

    #[test]
    fn startup_mutex_recovers_from_poison_without_a_second_panic() {
        let mutex = StdMutex::new(1_u8);
        let _ = std::panic::catch_unwind(|| {
            let _guard = mutex.lock().expect("initial lock");
            panic!("poison test mutex");
        });
        *lock_or_recover(&mutex, "test_startup_mutex") = 2;
        assert_eq!(*lock_or_recover(&mutex, "test_startup_mutex"), 2);
    }

    #[test]
    fn startup_gate_passes_banners_before_activation_but_never_republishes_redraws() {
        let gate = StartupOutputGate::new_inactive();
        assert_eq!(
            gate.consume(
                "startup banner\r\n[~]> ".to_string(),
                String::new(),
                false,
            ),
            StartupGateOutput::Pass("startup banner\r\n[~]> ".to_string())
        );

        gate.activate();
        assert_eq!(
            gate.consume(
                concat!(
                    "NYATERM_PRUNE_HISTORY=nt_test; . /tmp/.nt-test",
                    "\x1b[?2004l\r\r\n",
                    "\x1b[1m\x1b[7m%\x1b[27m\x1b[0m",
                    " . /tmp/.nt-test\r ",
                    "\x1b]2;\x1b\\[~]> ",
                )
                .to_string(),
                String::new(),
                true,
            ),
            StartupGateOutput::Ready("\x1b]2;\x1b\\".to_string())
        );
    }

    #[test]
    fn startup_gate_keeps_original_prompt_when_no_late_banner_exists() {
        let gate = StartupOutputGate::new_inactive();
        gate.activate();
        assert_eq!(
            gate.consume(
                ". /tmp/.nt-test\r\n\x1b]2;\x1b\\line 1\r\nline 2$ ".to_string(),
                String::new(),
                true,
            ),
            StartupGateOutput::Ready("\x1b]2;\x1b\\".to_string())
        );
    }

    #[cfg(unix)]
    #[test]
    fn unix_startup_source_file_is_private_and_removed() {
        use std::os::unix::fs::PermissionsExt;

        let (command, file) =
            prepare_local_startup_injection("printf hook".to_string()).expect("temp hook");
        let file = file.expect("Unix temp file");
        assert!(command.starts_with("NYATERM_PRUNE_HISTORY=nt_"));
        assert!(command.contains("; . /tmp/.nt-"));
        assert_eq!(std::fs::read_to_string(&file.path).unwrap(), "printf hook");
        assert_eq!(
            std::fs::metadata(&file.path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let path = file.path.clone();
        file.remove();
        assert!(!path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn native_bash_source_injection_hides_echo_and_multiline_prompt() {
        use std::io::{ErrorKind, Read, Write};

        fn read_available(reader: &mut dyn Read) -> Vec<u8> {
            let mut output = Vec::new();
            let mut chunk = [0_u8; 4096];
            loop {
                match reader.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(count) => output.extend_from_slice(&chunk[..count]),
                    Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                    Err(error) => panic!("PTY read failed: {error}"),
                }
            }
            output
        }

        let system = super::native_pty_system();
        let pair = system
            .openpty(super::PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("open PTY");
        let mut command = super::CommandBuilder::new("/bin/bash");
        command.args(["--noprofile", "--norc", "-i"]);
        command.env("TERM", "xterm-256color");
        let mut child = pair.slave.spawn_command(command).expect("spawn Bash");
        let shell_pid = child.process_id();
        drop(pair.slave);

        let fd = pair.master.as_raw_fd().expect("Unix PTY fd");
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        assert!(flags >= 0);
        assert_eq!(
            unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) },
            0
        );
        let mut reader = pair.master.try_clone_reader().expect("PTY reader");
        let mut writer = pair.master.take_writer().expect("PTY writer");

        let prompt_deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while !local_shell_line_editor_ready(&*pair.master, shell_pid) {
            assert!(std::time::Instant::now() < prompt_deadline, "Bash prompt timeout");
            let _ = read_available(&mut *reader);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let _ = read_available(&mut *reader);

        writer
            .write_all(
                b"__nyaterm_test_theme(){ __nyaterm_test_theme_status=$?; printf '\\033]2;BASH THEME\\007'; PS1=$'line 1\\nline 2$ '; PROMPT_COMMAND=(__nyaterm_test_theme); }; PROMPT_COMMAND=(__nyaterm_test_theme)\n",
            )
            .expect("set theme-rewritten multiline prompt");
        writer.flush().expect("flush prompt command");
        std::thread::sleep(std::time::Duration::from_millis(100));
        let _ = read_available(&mut *reader);
        assert!(local_shell_line_editor_ready(&*pair.master, shell_pid));

        let marker = ready_marker();
        let source = build_local_startup_script("/bin/bash", &marker, true, true)
            .script
            .expect("Bash source");
        let (source_command, source_file) =
            prepare_local_startup_injection(source).expect("prepare source file");
        let source_file = source_file.expect("Unix source file");
        let mut stripper = OscStripper::with_cwd_payloads(&marker, true);
        let gate = StartupOutputGate::new_inactive();
        gate.activate();
        writer
            .write_all(source_command.as_bytes())
            .expect("inject source command");
        writer.flush().expect("flush source command");

        let mut published = String::new();
        let mut cwd_payloads = Vec::new();
        let mut ready = false;
        let ready_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !ready {
            assert!(std::time::Instant::now() < ready_deadline, "ready marker timeout");
            let raw = read_available(&mut *reader);
            if raw.is_empty() {
                std::thread::sleep(std::time::Duration::from_millis(10));
                continue;
            }
            let parsed = stripper.push(&String::from_utf8_lossy(&raw));
            ready = parsed.ready;
            cwd_payloads.extend(parsed.cwd_payloads.iter().cloned());
            let gated = gate.consume(
                parsed.visible,
                parsed.visible_after_ready,
                parsed.ready || parsed.ready_failed,
            );
            match gated {
                StartupGateOutput::Buffered(value)
                | StartupGateOutput::Ready(value)
                | StartupGateOutput::SuppressedOverflow(value)
                | StartupGateOutput::Pass(value) => published.push_str(&value),
            }
        }

        source_file.remove();
        assert!(
            published.contains("\x1b]2;BASH THEME\x07"),
            "Bash theme title did not cross the startup gate: {published:?}"
        );
        assert!(!published.contains("/tmp/.nt-"));
        assert!(!published.contains("line 1") && !published.contains("line 2$"));
        assert!(
            cwd_payloads
                .iter()
                .any(|payload| payload.starts_with("file://localhost/")),
            "Bash hook did not publish its initial managed cwd: {cwd_payloads:?}"
        );

        writer
            .write_all(b"false\n")
            .expect("set failing command status");
        writer.flush().expect("flush failing command");
        let false_deadline =
            std::time::Instant::now() + std::time::Duration::from_secs(3);
        while !local_shell_line_editor_ready(&*pair.master, shell_pid) {
            assert!(
                std::time::Instant::now() < false_deadline,
                "Bash false prompt timeout"
            );
            let _ = read_available(&mut *reader);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let _ = read_available(&mut *reader);

        writer
            .write_all(
                b"printf '__NYATERM_THEME_STATUS=%s__\\n' \"$__nyaterm_test_theme_status\"\n",
            )
            .expect("query theme status");
        writer.flush().expect("flush theme status query");
        let status_deadline =
            std::time::Instant::now() + std::time::Duration::from_secs(3);
        let mut status_output = Vec::new();
        while !status_output
            .windows(b"__NYATERM_THEME_STATUS=1__".len())
            .any(|window| window == b"__NYATERM_THEME_STATUS=1__")
        {
            assert!(
                std::time::Instant::now() < status_deadline,
                "Bash theme did not observe the incoming failure status: {}",
                String::from_utf8_lossy(&status_output)
            );
            status_output.extend(read_available(&mut *reader));
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        writer.write_all(b"history 5\n").expect("request Bash history");
        writer.flush().expect("flush history command");
        let history_deadline =
            std::time::Instant::now() + std::time::Duration::from_secs(3);
        let mut history_output = Vec::new();
        while std::time::Instant::now() < history_deadline {
            history_output.extend(read_available(&mut *reader));
            if history_output.windows(b"history 5".len()).any(|window| window == b"history 5")
                && local_shell_line_editor_ready(&*pair.master, shell_pid)
            {
                break;
    }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let source_history_line = source_command.trim();
        assert!(
            !String::from_utf8_lossy(&history_output).contains(source_history_line),
            "NyaTerm startup source command leaked into Bash history: {}",
            String::from_utf8_lossy(&history_output)
        );

        child.kill().expect("kill Bash");
        let _ = child.wait();
    }

    #[cfg(unix)]
    #[test]
    fn native_bash_string_prompt_command_edge_cases_still_reach_ready() {
        use std::io::Write;
        use std::process::{Command, Stdio};

        for existing in [
            "history -a;",
            "jobs &",
        ] {
        let marker = ready_marker();
            let source = build_local_startup_script("/bin/bash", &marker, true, true)
                .script
                .expect("Bash source");
            let (source_command, source_file) =
                prepare_local_startup_injection(source).expect("prepare source file");
            let source_file = source_file.expect("Unix source file");
            let mut child = Command::new("/bin/bash")
                .args(["--noprofile", "--norc", "-i"])
                .env("TERM", "dumb")
                .env("HISTFILE", "/dev/null")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("spawn Bash");
            child
                .stdin
                .as_mut()
                .expect("Bash stdin")
                .write_all(
                    format!(
                        "PS1='term> '; PROMPT_COMMAND='{}'\n{}FUNCNEST=20\nPROMPT_COMMAND+='; printf __NYATERM_MUTATED__'\nPROMPT_COMMAND+='& printf __NYATERM_BACKGROUND__'\nprintf '__PROMPT_VALUE=%s__\\n' \"$PROMPT_COMMAND\"\nfalse\nprintf '__AFTER_STATUS=%s__\\n' \"$?\"\nexit\n",
                        existing.replace('\'', "'\\''"),
                        source_command
                    )
                    .as_bytes(),
                )
                .expect("write Bash smoke commands");
            let output = child.wait_with_output().expect("wait for Bash");
            source_file.remove();

            let mut combined = output.stdout;
            combined.extend_from_slice(&output.stderr);
            assert!(
                combined
                    .windows(b"NyaTermReady:session-1".len())
                    .any(|window| window == b"NyaTermReady:session-1"),
                "Bash string PROMPT_COMMAND did not reach ready for {existing:?}: {}",
                String::from_utf8_lossy(&combined)
            );
            assert!(
                !combined.windows(b"syntax error".len()).any(|window| window == b"syntax error"),
                "Bash string PROMPT_COMMAND became invalid for {existing:?}: {}",
                String::from_utf8_lossy(&combined)
            );
            assert!(
                !combined
                    .windows(b"maximum function nesting".len())
                    .any(|window| window == b"maximum function nesting"),
                "Bash wrapper recursively captured itself for {existing:?}: {}",
                String::from_utf8_lossy(&combined)
            );
            assert!(
                combined
                    .windows(b"__PROMPT_VALUE=__nyaterm_local_dynamic_prompt; __nyaterm_local_run_saved_prompt_command; __nyaterm_local_ready_prompt; __nyaterm_local_prompt_guard__".len())
                    .any(|window| window == b"__PROMPT_VALUE=__nyaterm_local_dynamic_prompt; __nyaterm_local_run_saved_prompt_command; __nyaterm_local_ready_prompt; __nyaterm_local_prompt_guard__"),
                "Bash background mutation was not repaired in the parent shell for {existing:?}: {}",
                String::from_utf8_lossy(&combined)
            );
            assert!(
                combined
                    .windows(b"__AFTER_STATUS=1__".len())
                    .any(|window| window == b"__AFTER_STATUS=1__"),
                "Bash prompt mutation lost incoming status for {existing:?}: {}",
                String::from_utf8_lossy(&combined)
            );
            assert!(
                combined
                    .windows(b"__NYATERM_MUTATED__".len())
                    .filter(|window| *window == b"__NYATERM_MUTATED__")
                    .count()
                    >= 2,
                "Bash prompt mutation was not retained for {existing:?}: {}",
                String::from_utf8_lossy(&combined)
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn native_bash_full_prompt_command_replacement_emits_failure() {
        use std::io::Write;
        use std::process::{Command, Stdio};

        let source = build_local_startup_script(
            "/bin/bash",
            &ready_marker(),
            true,
            true,
        )
        .script
        .expect("Bash source");
        let mut child = Command::new("/bin/bash")
            .args(["--noprofile", "--norc", "-i"])
            .env("TERM", "dumb")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn Bash");
        child
            .stdin
            .as_mut()
            .expect("Bash stdin")
            .write_all(
                format!(
                    "PS1='term> '\n{source}\nPROMPT_COMMAND=:\nprintf 'TRIGGER\\n'\nexit\n"
                )
                .as_bytes(),
            )
            .expect("write Bash replacement smoke");
        let output = child.wait_with_output().expect("wait for Bash");
        let mut combined = output.stdout;
        combined.extend_from_slice(&output.stderr);

        assert!(
            combined
                .windows(b"NyaTermReady:session-1".len())
                .any(|window| window == b"NyaTermReady:session-1"),
            "{}",
            String::from_utf8_lossy(&combined)
        );
        assert!(
            combined
                .windows(b"NyaTermReadyFailed:session-1".len())
                .any(|window| window == b"NyaTermReadyFailed:session-1"),
            "{}",
            String::from_utf8_lossy(&combined)
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_macos_bash_accepts_generated_script_without_extglob() {
        use std::io::Write;
        use std::process::{Command, Stdio};

        let script = build_local_startup_script("/bin/bash", &ready_marker(), true, true)
            .script
            .expect("Bash source");
        let mut child = Command::new("/bin/bash")
            .arg("-n")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn macOS Bash syntax check");
        child
            .stdin
            .as_mut()
            .expect("Bash stdin")
            .write_all(script.as_bytes())
            .expect("write generated Bash script");
        let output = child.wait_with_output().expect("wait for Bash syntax check");

            assert!(
            output.status.success(),
            "macOS Bash rejected generated integration: {}",
            String::from_utf8_lossy(&output.stderr)
            );
        }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_zsh_source_command_and_prompt_redraw_remain_hidden() {
        use std::io::{ErrorKind, Read, Write};

        fn read_available(reader: &mut dyn Read) -> Vec<u8> {
            let mut output = Vec::new();
            let mut chunk = [0_u8; 4096];
            loop {
                match reader.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(count) => output.extend_from_slice(&chunk[..count]),
                    Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                    Err(error) => panic!("PTY read failed: {error}"),
                }
    }
            output
        }

        let system = super::native_pty_system();
        let pair = system
            .openpty(super::PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("open PTY");
        let mut command = super::CommandBuilder::new("/bin/zsh");
        command.args(["-f", "-i"]);
        command.env("TERM", "xterm-256color");
        command.env("HISTFILE", "/dev/null");
        let mut child = pair.slave.spawn_command(command).expect("spawn Zsh");
        let shell_pid = child.process_id();
        drop(pair.slave);

        let fd = pair.master.as_raw_fd().expect("Unix PTY fd");
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        assert!(flags >= 0);
        assert_eq!(
            unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) },
            0
        );
        let mut reader = pair.master.try_clone_reader().expect("PTY reader");
        let mut writer = pair.master.take_writer().expect("PTY writer");

        let initial_deadline =
            std::time::Instant::now() + std::time::Duration::from_secs(3);
        while !local_shell_line_editor_ready(&*pair.master, shell_pid) {
            assert!(
                std::time::Instant::now() < initial_deadline,
                "initial Zsh prompt timeout"
            );
            let _ = read_available(&mut *reader);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let _ = read_available(&mut *reader);

        writer
            .write_all(
                b"__nyaterm_test_theme(){ __nyaterm_test_theme_status=$?; printf '\\033]2;ZSH THEME\\007'; PS1='[~]> '; RPROMPT='RIGHT'; precmd_functions=(__nyaterm_test_theme); }; precmd_functions=(__nyaterm_test_theme)\n",
            )
            .expect("set theme-rewritten Zsh prompts");
        writer.flush().expect("flush prompt setup");
        std::thread::sleep(std::time::Duration::from_millis(100));
        let _ = read_available(&mut *reader);

        let marker = ready_marker();
        let source = build_local_startup_script("/bin/zsh", &marker, true, true)
            .script
            .expect("Zsh source");
        let (source_command, source_file) =
            prepare_local_startup_injection(source).expect("prepare source file");
        let source_file = source_file.expect("Unix source file");
        let mut stripper = OscStripper::with_cwd_payloads(&marker, true);
        let gate = StartupOutputGate::new_inactive();
        gate.activate();
        writer
            .write_all(source_command.as_bytes())
            .expect("inject source command");
        writer.flush().expect("flush source command");

        let mut raw_output = Vec::new();
        let mut published = String::new();
        let mut ready = false;
        let ready_deadline =
            std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !ready {
            assert!(
                std::time::Instant::now() < ready_deadline,
                "Zsh ready marker timeout"
            );
            let raw = read_available(&mut *reader);
            if raw.is_empty() {
                std::thread::sleep(std::time::Duration::from_millis(10));
                continue;
            }
            raw_output.extend_from_slice(&raw);
            let parsed = stripper.push(&String::from_utf8_lossy(&raw));
            ready = parsed.ready;
            match gate.consume(
                parsed.visible,
                parsed.visible_after_ready,
                parsed.ready || parsed.ready_failed,
            ) {
                StartupGateOutput::Buffered(value)
                | StartupGateOutput::Ready(value)
                | StartupGateOutput::SuppressedOverflow(value)
                | StartupGateOutput::Pass(value) => published.push_str(&value),
            }
        }

        assert!(
            String::from_utf8_lossy(&raw_output).contains(source_command.trim()),
            "test did not observe Zsh's source-command redraw"
        );
        assert!(
            published.contains("\x1b]2;ZSH THEME\x07"),
            "Zsh theme title did not cross the startup gate: {published:?}"
        );
        assert!(
            !published.contains("/tmp/.nt-"),
            "Zsh source command leaked through the startup gate: {published:?}"
        );
        assert!(
            !published.contains("[~]>") && !published.contains("RIGHT"),
            "Zsh replacement prompt leaked through the startup gate: {published:?}"
        );

        writer
            .write_all(b"false\n")
            .expect("set failing Zsh command status");
        writer.flush().expect("flush failing Zsh command");
        std::thread::sleep(std::time::Duration::from_millis(100));
        let _ = read_available(&mut *reader);
        writer
            .write_all(
                b"print -r -- '__NYATERM_ZSH_THEME_STATUS='${__nyaterm_test_theme_status}'__'\n",
            )
            .expect("query Zsh theme status");
        writer.flush().expect("flush Zsh theme status query");
        let status_deadline =
            std::time::Instant::now() + std::time::Duration::from_secs(3);
        let mut status_output = Vec::new();
        while !status_output
            .windows(b"__NYATERM_ZSH_THEME_STATUS=1__".len())
            .any(|window| window == b"__NYATERM_ZSH_THEME_STATUS=1__")
        {
            assert!(
                std::time::Instant::now() < status_deadline,
                "Zsh theme did not observe the incoming failure status: {}",
                String::from_utf8_lossy(&status_output)
            );
            status_output.extend(read_available(&mut *reader));
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        source_file.remove();
        child.kill().expect("kill Zsh");
        let _ = child.wait();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_zsh_readonly_prompt_or_hook_array_emits_failure_immediately() {
        use std::io::Write;
        use std::process::{Command, Stdio};

        fn run_readonly_failure(setup: &str) {
        let marker = ready_marker();
            let source = build_local_startup_script("/bin/zsh", &marker, true, true)
                .script
                .expect("Zsh source");
            let (source_command, source_file) =
                prepare_local_startup_injection(source).expect("prepare source file");
            let source_file = source_file.expect("Unix source file");
            let mut child = Command::new("/bin/zsh")
                .args(["-f", "-i"])
                .env("TERM", "dumb")
                .env("HISTFILE", "/dev/null")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("spawn Zsh");
            child
                .stdin
                .as_mut()
                .expect("Zsh stdin")
                .write_all(format!("{setup}\n{source_command}exit\n").as_bytes())
                .expect("write Zsh smoke commands");
            let output = child.wait_with_output().expect("wait for Zsh");
            source_file.remove();

            let mut combined = output.stdout;
            combined.extend_from_slice(&output.stderr);
            assert!(
                combined
                    .windows(b"NyaTermReadyFailed:session-1".len())
                    .any(|window| window == b"NyaTermReadyFailed:session-1"),
                "readonly Zsh hook omitted its immediate failure marker: {}",
                String::from_utf8_lossy(&combined)
            );
            assert!(
                !combined
                    .windows(b"\x1b]7777;NyaTermReady:session-1\x07".len())
                    .any(|window| window == b"\x1b]7777;NyaTermReady:session-1\x07"),
                "readonly Zsh hook emitted a success marker"
            );
        }

        run_readonly_failure("PS1='readonly> '; readonly PS1");
        run_readonly_failure(
            "typeset -ga precmd_functions=(); readonly precmd_functions; PS1='readonly> '",
        );
    }

    #[cfg(unix)]
    #[test]
    fn native_bash_failed_hook_install_still_prunes_source_history() {
        use std::io::{ErrorKind, Read, Write};

        fn read_available(reader: &mut dyn Read) -> Vec<u8> {
            let mut output = Vec::new();
            let mut chunk = [0_u8; 4096];
            loop {
                match reader.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(count) => output.extend_from_slice(&chunk[..count]),
                    Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                    Err(error) => panic!("PTY read failed: {error}"),
                }
            }
            output
        }

        fn run_readonly_failure(setup: &[u8]) {
            const HISTORY_DONE: &[u8] = b"__NYATERM_HISTORY_DONE__";
            let system = super::native_pty_system();
            let pair = system
                .openpty(super::PtySize {
                    rows: 24,
                    cols: 80,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .expect("open PTY");
            let mut command = super::CommandBuilder::new("/bin/bash");
            command.args(["--noprofile", "--norc", "-i"]);
            command.env("TERM", "dumb");
            command.env("HISTFILE", "/dev/null");
            command.env("HISTCONTROL", "");
            let mut child = pair.slave.spawn_command(command).expect("spawn Bash");
            let shell_pid = child.process_id();
            drop(pair.slave);

            let fd = pair.master.as_raw_fd().expect("Unix PTY fd");
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
            assert!(flags >= 0);
            assert_eq!(
                unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) },
                0
            );
            let mut reader = pair.master.try_clone_reader().expect("PTY reader");
            let mut writer = pair.master.take_writer().expect("PTY writer");

            let initial_deadline =
                std::time::Instant::now() + std::time::Duration::from_secs(3);
            while !local_shell_line_editor_ready(&*pair.master, shell_pid) {
                assert!(
                    std::time::Instant::now() < initial_deadline,
                    "initial Bash prompt timeout"
                );
                let _ = read_available(&mut *reader);
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            let _ = read_available(&mut *reader);

            writer.write_all(setup).expect("configure readonly prompt");
            writer.flush().expect("flush readonly setup");
            std::thread::sleep(std::time::Duration::from_millis(100));
            let _ = read_available(&mut *reader);

            let marker = ready_marker();
            let source = build_local_startup_script("/bin/bash", &marker, true, true)
                .script
                .expect("Bash source");
            let (source_command, source_file) =
                prepare_local_startup_injection(source).expect("prepare source file");
            let source_file = source_file.expect("Unix source file");
            writer
                .write_all(source_command.as_bytes())
                .expect("inject source command");
            writer.flush().expect("flush source command");

            let source_deadline =
                std::time::Instant::now() + std::time::Duration::from_secs(3);
            let mut source_output = Vec::new();
            while !(source_output
                .windows(b"NyaTermReadyFailed:session-1".len())
                .any(|window| window == b"NyaTermReadyFailed:session-1")
                && local_shell_line_editor_ready(&*pair.master, shell_pid))
            {
                assert!(
                    std::time::Instant::now() < source_deadline,
                    "failed Bash hook did not emit its failure marker and return to its prompt"
                );
                source_output.extend(read_available(&mut *reader));
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            source_output.extend(read_available(&mut *reader));
            assert!(
                source_output
                    .windows(b"NyaTermReadyFailed:session-1".len())
                    .any(|window| window == b"NyaTermReadyFailed:session-1"),
                "failed Bash hook omitted its immediate session marker: {}",
                String::from_utf8_lossy(&source_output)
            );

            writer
                .write_all(b"history 8; printf '__NYATERM_HISTORY_DONE__\\n'\n")
                .expect("request history");
            writer.flush().expect("flush history command");
            let history_deadline =
                std::time::Instant::now() + std::time::Duration::from_secs(3);
            let mut history_output = Vec::new();
            while !(history_output
                .windows(HISTORY_DONE.len())
                .filter(|window| *window == HISTORY_DONE)
                .count()
                >= 2
                && local_shell_line_editor_ready(&*pair.master, shell_pid))
            {
                assert!(
                    std::time::Instant::now() < history_deadline,
                    "Bash history listing did not complete"
                );
                history_output.extend(read_available(&mut *reader));
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            let history = String::from_utf8_lossy(&history_output);
            let setup_history_line = String::from_utf8_lossy(setup);
            assert!(
                history.contains(setup_history_line.trim()),
                "known setup entry was absent from the completed history listing: {history}"
            );
            assert!(
                !history.contains(source_command.trim()),
                "failed Bash hook leaked startup command into history: {history}"
            );

            source_file.remove();
            child.kill().expect("kill Bash");
            let _ = child.wait();
        }

        run_readonly_failure(
            b"PROMPT_COMMAND=''; readonly PROMPT_COMMAND; PS1='failure$ '\n",
        );
        run_readonly_failure(b"PROMPT_COMMAND=''; PS1='failure$ '; readonly PS1\n");
    }

    #[test]
    fn startup_gate_transports_pre_ready_application_titles_but_not_prompt_echo() {
        let marker = ready_marker();
        let mut stripper = OscStripper::new(&marker);
        let gate = StartupOutputGate::new();
        let parsed = stripper.push(&format!(
            "source echo\x1b[c\x1b]2;theme title\x07{marker}replacement prompt"
        ));
        let mut published = String::new();
        let gated = gate.consume(
            parsed.visible,
            parsed.visible_after_ready,
            parsed.ready || parsed.ready_failed,
        );
        match gated {
            StartupGateOutput::Buffered(value)
            | StartupGateOutput::Ready(value)
            | StartupGateOutput::SuppressedOverflow(value)
            | StartupGateOutput::Pass(value) => published.push_str(&value),
        }

        assert_eq!(published, "\x1b[c\x1b]2;theme title\x07");
    }

    #[test]
    fn startup_gate_discards_echo_and_replacement_prompt() {
        let gate = StartupOutputGate::new();
        assert_eq!(
            gate.consume("echoed integration".to_string(), String::new(), false),
            StartupGateOutput::Buffered(String::new())
        );
        assert_eq!(
            gate.consume(
                "before markerprompt".to_string(),
                "prompt".to_string(),
                true,
            ),
            StartupGateOutput::Ready(String::new())
        );
        assert_eq!(
            gate.consume("next".to_string(), String::new(), false),
            StartupGateOutput::Pass("next".to_string())
        );
    }

    #[test]
    fn startup_gate_discards_hidden_injection_once() {
        let gate = StartupOutputGate::new();
        assert_eq!(
            gate.consume("echoed hook source".to_string(), String::new(), false),
            StartupGateOutput::Buffered(String::new())
        );
        assert!(gate.discard());
        assert!(!gate.discard());
        assert_eq!(
            gate.consume("next".to_string(), String::new(), false),
            StartupGateOutput::Pass("next".to_string())
        );
    }

    #[test]
    fn startup_gate_buffer_limit_discards_all_injection_bytes() {
        let gate = StartupOutputGate::new();
        let oversized = "x".repeat(64 * 1024 + 1);
        assert_eq!(
            gate.consume(oversized, String::new(), false),
            StartupGateOutput::SuppressedOverflow(String::new())
        );
        assert!(!gate.is_active());
        assert_eq!(
            gate.consume("normal passive output".to_string(), String::new(), false),
            StartupGateOutput::Pass("normal passive output".to_string())
        );
    }

    #[test]
    fn local_startup_injects_title_only_hooks_for_supported_unix_shells() {
        let marker = ready_marker();

        for shell in ["/bin/bash", "/bin/zsh"] {
            let startup = build_local_startup_script(shell, &marker, true, true);
            assert!(startup.dynamic_title_integration_requested);
            let script = startup.script.expect("unix startup script");
            assert!(script.contains("NyaTermReady:session-1"));
            assert!(script.contains("NyaTermReadyFailed:session-1"));
            assert!(script.contains("]2;"));
            assert!(script.contains("]7;file://localhost"));
            assert!(script.contains("uri_encode"));
            assert!(script.contains("PS1="));
            assert!(!script.contains("NyaTermCommand"));
            assert!(!script.contains("base64"));
            assert!(startup.shell_init_args.is_none());
            assert!(startup.pwsh_init_args.is_none());
            assert!(startup.cmd_prompt.is_none());
            assert!(script.contains("return 1"));
            if shell.ends_with("bash") {
                assert!(script.contains("code & 255"));
                assert!(script.contains("NYATERM_PRUNE_HISTORY=$history_marker"));
                assert!(script.contains("__nyaterm_local_array_prompt_supported"));
                assert!(script.contains("__nyaterm_local_capture_prompt_string"));
                assert!(!script.contains("BASH_REMATCH"));
                assert!(!script.contains(" in ("));
                assert!(!script.contains("|*__nyaterm_local_"));
                assert!(!script.contains("|*NyaTermReady"));
                assert!(script.contains("__nyaterm_local_prompt_container_valid"));
                assert!(script.contains("__nyaterm_local_prompt_liveness"));
                assert!(script.contains("\\[$(__nyaterm_local_prompt_liveness)\\]"));
                assert!(script.contains(
                    "PROMPT_COMMAND=(__nyaterm_local_dynamic_prompt \"${retained[@]}\" __nyaterm_local_ready_prompt)"
                ));
            } else {
                assert!(script.contains("%{\\e]7777;NyaTermReady"));
                assert!(script.contains(
                    "precmd_functions=(__nyaterm_local_dynamic_emit \"${retained[@]}\" __nyaterm_local_ready_prompt)"
                ));
            }
        }

        let fish = build_local_startup_script("/opt/homebrew/bin/fish", &marker, true, true);
        assert!(fish.dynamic_title_integration_requested);
        assert!(fish.script.is_none());
        let fish_args = fish.shell_init_args.expect("Fish init args");
        assert_eq!(fish_args.first().map(String::as_str), Some("--init-command"));
        let fish_script = fish_args.last().expect("Fish init script");
        assert!(fish_script.contains("NyaTermReady:session-1"));
        assert!(fish_script.contains("--on-event fish_postexec"));
        assert!(!fish_script.contains("--on-event fish_prompt"));
        assert!(!fish_script.contains("fish_private_mode"));

        for shell in ["/bin/sh", "nu", "/opt/bin/my-bash-wrapper"] {
            let startup = build_local_startup_script(shell, &marker, true, true);
            assert!(!startup.dynamic_title_integration_requested);
            assert!(startup.script.is_none());
            assert!(startup.shell_init_args.is_none());
        }

        let custom = build_local_startup_script("/bin/bash", &marker, true, false);
        assert!(!custom.dynamic_title_integration_requested);
        assert!(custom.script.is_none());
        assert!(custom.shell_init_args.is_none());
    }

    #[test]
    fn local_startup_injects_pwsh_via_spawn_args_only() {
        let marker = ready_marker();
        let pwsh = build_local_startup_script("powershell.exe", &marker, true, true);
        assert!(pwsh.dynamic_title_integration_requested);
        assert!(pwsh.script.is_none());
        assert!(pwsh.cmd_prompt.is_none());
        let args = pwsh.pwsh_init_args.expect("pwsh init args");
        assert!(!args.contains(&"-NoLogo".to_string()));
        assert!(args.contains(&"-NoExit".to_string()));
        assert!(args.contains(&"-Command".to_string()));
        let init = args.last().expect("init script");
        assert!(init.contains("global:prompt"));
        assert!(init.contains("Provider.Name -eq 'FileSystem'"));
        assert!(init.contains("ProviderPath"));
        assert!(init.contains("nyaterm-clear://session-1"));
        assert!(init.contains("NyaTermReady:session-1"));
        assert!(init.contains("$__nt_success = $?"));
        assert!(init.contains("$__nt_had_last_exit = Test-Path"));
        assert!(init.contains("$global:LASTEXITCODE = $__nt_last_exit"));
        assert!(init.contains("Remove-Variable LASTEXITCODE"));
        assert!(init.contains("if ($__nt_success) { & {} } else { Write-Error"));
        assert!(
            init.find("NyaTermReady:session-1").unwrap()
                < init.find("']7;' + $__nt_uri").unwrap()
        );
        assert!(
            init.find("]2;").unwrap()
                < init
                    .find("& $global:__nyaterm_local_prev_prompt")
                    .unwrap()
        );
        assert!(init.contains("ReferenceEquals"));
        assert!(init.contains("Microsoft.PowerShell.Utility\\Register-EngineEvent"));
        assert!(init.contains("Microsoft.PowerShell.Utility\\Get-EventSubscriber -SourceIdentifier PowerShell.OnIdle"));
        assert!(init.contains("Microsoft.PowerShell.Utility\\Unregister-Event -SubscriptionId"));
        assert!(init.contains("Microsoft.PowerShell.Core\\Remove-Job -Id"));
        assert!(init.contains("PowerShell.OnIdle"));
        assert!(init.contains("__nyaterm_local_report"));
        assert!(init.contains("__nyaterm_local_failure_reported"));
        assert!(init.contains("NyaTermReadyFailed:session-1"));

        let no_inject = build_local_startup_script("powershell.exe", &marker, true, false);
        assert!(!no_inject.dynamic_title_integration_requested);
        assert!(no_inject.pwsh_init_args.is_none());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn native_powershell_repeated_init_remains_single_and_non_recursive() {
        use std::process::Command;

        let marker = ready_marker();
        let init = build_local_startup_script("powershell.exe", &marker, true, true)
            .pwsh_init_args
            .expect("PowerShell init args")
            .pop()
            .expect("PowerShell init script");
        let harness = format!(
            "$ErrorActionPreference='Stop'; function global:prompt {{ 'USER> ' }}; Start-Job {{ 1 }} | Out-Null; $user = Register-EngineEvent -SourceIdentifier PowerShell.OnIdle -Action {{ 'USER-IDLE' }}; {init}; {init}; $value = & prompt; Write-Output ('VALUE=' + $value); Write-Output ('SUBSCRIBERS=' + @($ExecutionContext.Events.Subscribers | Where-Object SourceIdentifier -eq 'PowerShell.OnIdle').Count); if (@(Get-EventSubscriber -SourceIdentifier PowerShell.OnIdle | Where-Object {{ [object]::ReferenceEquals($_.Action, $user) }}).Count -ne 1) {{ throw 'user subscriber removed' }}"
        );
        let output = Command::new("powershell.exe")
            .args(["-NoLogo", "-NoProfile", "-Command", &harness])
            .output()
            .expect("run repeated PowerShell init smoke");
        let mut combined = output.stdout;
        combined.extend_from_slice(&output.stderr);

        assert!(output.status.success(), "{}", String::from_utf8_lossy(&combined));
        assert!(
            combined
                .windows(b"VALUE=USER> ".len())
                .any(|window| window == b"VALUE=USER> "),
            "{}",
            String::from_utf8_lossy(&combined)
        );
        assert!(
            combined
                .windows(b"SUBSCRIBERS=2".len())
                .any(|window| window == b"SUBSCRIBERS=2"),
            "{}",
            String::from_utf8_lossy(&combined)
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn native_powershell_prompt_wrapper_repairs_self_replacement() {
        use std::process::Command;

        let marker = ready_marker();
        let init = build_local_startup_script("powershell.exe", &marker, true, true)
            .pwsh_init_args
            .expect("PowerShell init args")
            .pop()
            .expect("PowerShell init script");
        let harness = format!(
            "function global:prompt {{ function global:prompt {{ 'replacement> ' }}; 'first> ' }}; {init}; $null = prompt; $null = prompt"
        );
        let output = Command::new("powershell.exe")
            .args(["-NoLogo", "-NoProfile", "-Command", &harness])
            .output()
            .expect("run Windows PowerShell prompt smoke");

        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert_eq!(
            output
                .stdout
                .windows(b"NyaTermReady:session-1".len())
                .filter(|window| *window == b"NyaTermReady:session-1")
                .count(),
            2,
            "PowerShell wrapper was not restored: {}",
            String::from_utf8_lossy(&output.stdout)
        );
    }

    #[test]
    fn cmd_prompt_preserves_existing_prompt_and_uses_session_bound_cwd() {
        let prompt = build_cmd_prompt(Some("$T $P$G"), "session-1");
        assert_eq!(
            prompt,
            "$E]2;$E\\$E]7777;NyaTermReady:session-1$E\\$E]7;nyaterm-cmd://session-1/$P$E\\$T $P$G"
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn local_startup_configures_cmd_prompt_without_mutating_argv() {
        let marker = ready_marker();
        let cmd = build_local_startup_script(
            r"C:\Windows\System32\cmd.exe",
            &marker,
            true,
            true,
        );
        assert!(cmd.dynamic_title_integration_requested);
        assert!(cmd.script.is_none());
        assert!(cmd.pwsh_init_args.is_none());
        let prompt = cmd.cmd_prompt.expect("cmd prompt integration");
        assert!(prompt.starts_with(
            "$E]2;$E\\$E]7777;NyaTermReady:session-1$E\\$E]7;nyaterm-cmd://session-1/$P$E\\"
        ));

        let custom = build_local_startup_script("cmd.exe", &marker, true, false);
        assert!(!custom.dynamic_title_integration_requested);
        assert!(custom.cmd_prompt.is_none());
    }

    #[test]
    fn wrappers_fallbacks_and_disabled_policy_remain_passive() {
        assert!(is_unresolved_shell_wrapper("wsl.exe"));
        assert!(is_unresolved_shell_wrapper(r"C:\Windows\System32\wsl.exe"));
        assert!(is_unresolved_shell_wrapper("wt"));
        assert!(!is_unresolved_shell_wrapper("bash.exe"));
        assert!(should_allow_local_injection(
            "bash.exe",
            "/bin/bash",
            "",
            false,
            ShellResolutionSource::Direct,
        ));
        assert!(!should_allow_local_injection(
            "bash.exe",
            "/bin/bash",
            " ",
            true,
            ShellResolutionSource::Direct,
        ));
        assert!(!should_allow_local_injection(
            "pwsh.exe -NoLogo",
            "pwsh.exe",
            "",
            false,
            ShellResolutionSource::Direct,
        ));
        assert!(!should_allow_local_injection(
            "wsl.exe",
            "wsl.exe",
            "",
            true,
            ShellResolutionSource::Direct,
        ));

        let wt_resolved_shell = r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe";
        assert!(should_allow_local_injection(
            "wt.exe",
            wt_resolved_shell,
            "",
            true,
            ShellResolutionSource::WindowsTerminalProfile,
        ));
        assert!(should_allow_local_injection(
            "wt",
            r"C:\Windows\System32\cmd.exe",
            "",
            true,
            ShellResolutionSource::WindowsTerminalProfile,
        ));
        assert!(!should_allow_local_injection(
            "wt.exe",
            wt_resolved_shell,
            "",
            true,
            ShellResolutionSource::WindowsTerminalFallback,
        ));
        assert!(!should_allow_local_injection(
            "wt.exe",
            "wsl.exe",
            "",
            true,
            ShellResolutionSource::WindowsTerminalProfile,
        ));
        assert!(!should_allow_local_injection(
            "wt.exe",
            r"C:\Program Files\PowerShell\7\pwsh.exe",
            "",
            false,
            ShellResolutionSource::WindowsTerminalProfile,
        ));

        let wt_startup = build_local_startup_script(
            wt_resolved_shell,
            &ready_marker(),
            true,
            should_allow_local_injection(
                "wt.exe",
                wt_resolved_shell,
                "",
                true,
                ShellResolutionSource::WindowsTerminalProfile,
            ),
        );
        assert!(wt_startup.dynamic_title_integration_requested);
        assert!(wt_startup.pwsh_init_args.is_some());

        let startup = build_local_startup_script("/bin/zsh", &ready_marker(), false, true);
        assert!(!startup.dynamic_title_integration_requested);
        assert!(startup.script.is_none());
        assert!(startup.pwsh_init_args.is_none());
        assert!(startup.cmd_prompt.is_none());
    }
}
