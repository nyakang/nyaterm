#[cfg(all(test, windows))]
mod conpty_tests {
    use super::*;
    use portable_pty::{CommandBuilder, PtySize, native_pty_system};
    use std::io::{BufRead, BufReader, Read, Write};
    use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
    use std::sync::{Mutex, mpsc};
    use std::time::Duration;

    struct Renderer {
        child: Child,
        input: ChildStdin,
        output: BufReader<ChildStdout>,
    }

    impl Renderer {
        fn new() -> Self {
            let mut child = Command::new("node")
                .arg("scripts/conpty-terminal-test.mjs")
                .current_dir(
                    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                        .parent()
                        .unwrap(),
                )
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .expect("start xterm parser bridge (pnpm install required)");
            Self {
                input: child.stdin.take().unwrap(),
                output: BufReader::new(child.stdout.take().unwrap()),
                child,
            }
        }

        fn write(&mut self, data: &str, cols: Option<u16>) -> serde_json::Value {
            writeln!(
                self.input,
                "{}",
                serde_json::json!({ "data": data, "cols": cols })
            )
            .unwrap();
            self.input.flush().unwrap();
            let mut response = String::new();
            self.output.read_line(&mut response).unwrap();
            serde_json::from_str(&response).expect("xterm parser response")
        }
    }

    impl Drop for Renderer {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    struct ShellGuard(Box<dyn portable_pty::Child + Send + Sync>);

    impl Drop for ShellGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn pump(
        chunks: &mpsc::Receiver<Vec<u8>>,
        decoder: &mut crate::core::terminal_session::TerminalOutputDecoder,
        proc: &mut OutputCaptureProcessor,
        renderer: &Arc<Mutex<Renderer>>,
        writer: &mut dyn Write,
        mut done: impl FnMut(&serde_json::Value) -> bool,
    ) -> serde_json::Value {
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut screen = serde_json::Value::Null;
        let mut completed = false;
        let mut recent_raw = String::new();
        loop {
            assert!(
                Instant::now() < deadline,
                "ConPTY did not return to an interactive prompt: {screen}; raw: {recent_raw:?}"
            );
            match chunks.recv_timeout(Duration::from_millis(150)) {
                Ok(chunk) => {
                    let decoded = decoder.decode(&chunk);
                    recent_raw.push_str(&decoded);
                    if recent_raw.len() > 8192 {
                        recent_raw.clear();
                    }
                    let visible = proc.process(&decoded);
                    screen = renderer.lock().unwrap().write(&visible, None);
                    for response in screen["responses"].as_array().unwrap() {
                        writer
                            .write_all(response.as_str().unwrap().as_bytes())
                            .unwrap();
                        writer.flush().unwrap();
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) if completed => {
                    screen["raw"] = recent_raw.into();
                    return screen;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(error) => panic!("ConPTY output closed: {error}"),
            }
            completed |= done(&screen);
        }
    }

    fn at_prompt(screen: &serde_json::Value) -> bool {
        let row = screen["baseY"].as_u64().unwrap_or(0) + screen["y"].as_u64().unwrap_or(0);
        screen["lines"][row as usize]
            .as_str()
            .is_some_and(|line| line.trim() == "PS>")
            && screen["x"].as_u64() == Some(4)
    }

    /// Real Windows console + PSReadLine + the production marker processor +
    /// xterm parsing, including actual cursor-query responses.
    #[tokio::test]
    async fn native_powershell_conpty_capture_preserves_interactive_input() {
        let dispatch = PowershellDispatch::new().unwrap();
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let mut command = CommandBuilder::new("powershell.exe");
        let cwd = tempfile::Builder::new()
            .prefix("nyaterm-conpty-中文-")
            .tempdir()
            .unwrap();
        command.cwd(cwd.path());
        command.args(["-NoLogo", "-NoExit", "-Command"]);
        command.arg(format!(
            "{} Import-Module PSReadLine; function global:prompt {{ 'PS> ' }}",
            dispatch.init_script()
        ));
        let _child = ShellGuard(pair.slave.spawn_command(command).unwrap());
        drop(pair.slave);
        let mut writer = pair.master.take_writer().unwrap();
        let mut reader = pair.master.try_clone_reader().unwrap();
        let (tx, chunks) = mpsc::channel();
        std::thread::spawn(move || {
            let mut chunk = [0; 4096];
            while let Ok(count) = reader.read(&mut chunk) {
                if count == 0 || tx.send(chunk[..count].to_vec()).is_err() {
                    break;
                }
            }
        });
        let renderer = Arc::new(Mutex::new(Renderer::new()));
        let mut proc = OutputCaptureProcessor::new();
        let mut decoder = crate::core::terminal_session::TerminalOutputDecoder::new("UTF-8");
        // The initial prompt can require a cursor report before PSReadLine starts.
        pump(
            &chunks,
            &mut decoder,
            &mut proc,
            &renderer,
            &mut *writer,
            at_prompt,
        );

        for step in 0..20 {
            if step == 10 {
                pair.master
                    .resize(PtySize {
                        rows: 24,
                        cols: 32,
                        pixel_width: 0,
                        pixel_height: 0,
                    })
                    .unwrap();
                renderer.lock().unwrap().write("", Some(32));
            }
            let id = uuid::Uuid::new_v4().simple().to_string()[..12].to_string();
            let suffix = match step {
                2 => "cmd.exe /d /c exit 7",
                3 => "Write-Error 'expected non-terminating error'",
                4 => "throw 'expected terminating error'",
                6 => "[pscustomobject]@{ Label = 'OBJECT_OK' }",
                _ => "",
            };
            let real_command = format!(
                "if ({step} -gt 0 -and $ntTestValue -ne '中文路径') {{ throw 'session variable was lost' }}; $ntTestValue = '中文路径';\r\nWrite-Output ('hello-{step}-' + $ntTestValue + ' & ''quoted''');\r\n# {}\r\n{suffix}",
                "long comment ".repeat(100)
            );
            let (input, file) = dispatch.prepare(&id, &real_command).unwrap();
            assert!(input.len() + 4 < 32);
            let manager = Arc::new(crate::core::session::SessionManager::new());
            let mut execution = manager.begin_terminal_execution("test", &id).await.unwrap();
            execution.command_file = Some(file);
            let render_end = renderer.clone();
            execution.on_complete(move |result| {
                render_end.lock().unwrap().write(
                    &format!(
                        "\r\n{}\r\nexit {:?}\r\n",
                        strip_ansi_escapes::strip_str(&result.output),
                        result.exit_code
                    ),
                    None,
                );
            });
            renderer
                .lock()
                .unwrap()
                .write(&format!("\r\nMCP #{}\r\n", step + 1), None);
            let (tx, mut rx) = oneshot::channel();
            proc.register_execution(id, tx, Some(execution));
            writer.write_all(input.as_bytes()).unwrap();
            writer.flush().unwrap();
            let mut result = None;
            let screen = pump(
                &chunks,
                &mut decoder,
                &mut proc,
                &renderer,
                &mut *writer,
                |screen| {
                    if let Ok(value) = rx.try_recv() {
                        result = Some(value);
                    }
                    result.is_some() && at_prompt(screen)
                },
            );
            let result = result.unwrap();
            let expected_exit = match step {
                2 => 7,
                3 | 4 => 1,
                _ => 0,
            };
            assert_eq!(
                result.exit_code,
                Some(expected_exit),
                "step {step}: {}",
                result.output
            );
            if step == 6 {
                assert!(result.output.contains("OBJECT_OK"), "{}", result.output);
            }
            assert!(
                strip_ansi_escapes::strip_str(&result.output)
                    .contains(&format!("hello-{step}-中文路径")),
                "step {step}: {}; screen: {screen}",
                result.output
            );
            assert!(!proc.has_active());
            assert!(
                !screen["lines"].as_array().unwrap().iter().any(|line| line
                    .as_str()
                    .unwrap()
                    .trim_start()
                    .starts_with(">>")),
                "unexpected continuation prompt: {screen}"
            );
        }
        // Input and history must work immediately after the repeated captures.
        writer.write_all(b"Write-Output 'AFTER_TYPED'\r").unwrap();
        writer.flush().unwrap();
        let screen = pump(
            &chunks,
            &mut decoder,
            &mut proc,
            &renderer,
            &mut *writer,
            |screen| {
                at_prompt(screen)
                    && screen["lines"].as_array().is_some_and(|lines| {
                        lines
                            .iter()
                            .filter(|line| line.as_str().unwrap().contains("AFTER_TYPED"))
                            .count()
                            >= 2
                    })
            },
        );
        assert!(
            screen["lines"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|line| line.as_str().unwrap().contains("AFTER_TYPED"))
                .count()
                >= 2,
            "typed command did not execute: {screen}"
        );
        writer.write_all(b"\x1b[A\r").unwrap();
        writer.flush().unwrap();
        let screen = pump(
            &chunks,
            &mut decoder,
            &mut proc,
            &renderer,
            &mut *writer,
            |screen| {
                at_prompt(screen)
                    && screen["lines"].as_array().is_some_and(|lines| {
                        lines
                            .iter()
                            .filter(|line| line.as_str().unwrap().contains("AFTER_TYPED"))
                            .count()
                            >= 4
                    })
            },
        );
        assert!(
            screen["lines"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|line| line.as_str().unwrap().contains("AFTER_TYPED"))
                .count()
                >= 4,
            "history recall did not execute: {screen}"
        );
        writer
            .write_all(b"\x1b[A\x1b[BWrite-Output 'AFTER_DOWN'\r")
            .unwrap();
        writer.flush().unwrap();
        let screen = pump(
            &chunks,
            &mut decoder,
            &mut proc,
            &renderer,
            &mut *writer,
            |screen| {
                at_prompt(screen)
                    && screen["lines"].as_array().is_some_and(|lines| {
                        lines
                            .iter()
                            .filter(|line| line.as_str().unwrap().contains("AFTER_DOWN"))
                            .count()
                            >= 2
                    })
            },
        );
        assert!(
            at_prompt(&screen),
            "history down left the cursor displaced: {screen}"
        );

        let manager = Arc::new(crate::core::session::SessionManager::new());
        let id = uuid::Uuid::new_v4().simple().to_string()[..12].to_string();
        let (input, file) = dispatch
            .prepare(&id, "Start-Sleep -Seconds 30; Write-Output 'LATE'")
            .unwrap();
        let mut execution = manager
            .begin_terminal_execution("interrupt", &id)
            .await
            .unwrap();
        execution.command_file = Some(file);
        let (tx, rx) = oneshot::channel();
        proc.register_execution(id.clone(), tx, Some(execution));
        writer.write_all(input.as_bytes()).unwrap();
        writer.flush().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while proc
            .active
            .get(&id)
            .is_some_and(|cap| cap.phase == CapturePhase::WaitingForStart)
        {
            assert!(
                Instant::now() < deadline,
                "capture did not start before cancellation"
            );
            let chunk = chunks.recv_timeout(Duration::from_secs(2)).unwrap();
            let visible = proc.process(&decoder.decode(&chunk));
            let screen = renderer.lock().unwrap().write(&visible, None);
            for response in screen["responses"].as_array().unwrap() {
                writer
                    .write_all(response.as_str().unwrap().as_bytes())
                    .unwrap();
                writer.flush().unwrap();
            }
        }
        proc.cancel(&id);
        assert!(rx.await.is_err());
        assert!(
            manager
                .begin_terminal_execution("interrupt", "competing")
                .await
                .is_err()
        );
        writer.write_all(&[3]).unwrap();
        writer.flush().unwrap();
        pump(
            &chunks,
            &mut decoder,
            &mut proc,
            &renderer,
            &mut *writer,
            |screen| {
                at_prompt(screen)
                    && matches!(
                        manager.terminal_execution_state("interrupt"),
                        crate::core::session::TerminalExecutionState::Idle
                    )
            },
        );
        assert!(!proc.has_active());
    }
}
