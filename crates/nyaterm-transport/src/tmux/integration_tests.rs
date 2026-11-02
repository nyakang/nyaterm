use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use russh::ChannelMsg;

use super::handshake::{Failure, Verification, verify};
use super::test_peer::connect;

#[tokio::test]
async fn rejected_nonce_and_closed_channel_are_distinct_and_never_send_detach() {
    for closed in [false, true] {
        let mut connection = connect().await;
        let mut rx = connection.peer_rx.take().unwrap();
        let tx = connection.peer_tx.clone();
        let peer = tokio::spawn(async move {
            let Some(ChannelMsg::Data { data }) = rx.wait().await else {
                panic!("missing probe")
            };
            assert!(data.starts_with(b"display-message -p 'nyaterm_"));
            if closed {
                tx.eof().await.unwrap();
            } else {
                tx.data_bytes(b"%begin 1 2 1\nwrong\n%end 1 2 1\n".to_vec())
                    .await
                    .unwrap();
            }
            assert!(
                tokio::time::timeout(Duration::from_millis(50), rx.wait())
                    .await
                    .is_err(),
                "verification must not inject a detach"
            );
        });
        let result = verify(&connection.tx, &mut connection.rx, b"", b"").await;
        assert!(
            matches!(result, Verification::Failed(reason) if reason == if closed { Failure::ChannelClosed } else { Failure::Rejected })
        );
        peer.await.unwrap();
    }
}

#[tokio::test]
async fn nonce_verification_accepts_fragmented_replies_and_preserves_following_bytes() {
    let mut connection = connect().await;
    let mut rx = connection.peer_rx.take().unwrap();
    let tx = connection.peer_tx.clone();
    let peer = tokio::spawn(async move {
        let Some(ChannelMsg::Data { data }) = rx.wait().await else {
            panic!("missing probe")
        };
        let command = std::str::from_utf8(&data).unwrap();
        let nonce = command
            .trim()
            .strip_prefix("display-message -p '")
            .unwrap()
            .strip_suffix('\'')
            .unwrap();
        let response = format!("%begin 1 2 1\n{nonce}\n%end 1 2 1\n%window-add @2\n");
        for chunk in response.as_bytes().chunks(3) {
            tx.data_bytes(chunk.to_vec()).await.unwrap();
        }
    });
    let result = verify(&connection.tx, &mut connection.rx, b"", b"").await;
    let Verification::Confirmed(mut rest) = result else {
        panic!("nonce not confirmed")
    };
    while !rest.ends_with(b"%window-add @2\n") {
        let Some(ChannelMsg::Data { data }) = connection.rx.wait().await else {
            panic!("missing tail")
        };
        rest.extend_from_slice(&data);
    }
    assert_eq!(rest, b"%window-add @2\n");
    peer.await.unwrap();
}

#[tokio::test]
async fn exit_during_verification_returns_only_shell_tail() {
    let mut connection = connect().await;
    assert!(
        matches!(verify(&connection.tx, &mut connection.rx, b"startup", b"%exit detached\nprompt> ").await,
        Verification::Exited(rest) if rest == b"prompt> ")
    );
}

#[tokio::test]
async fn continuous_notifications_do_not_extend_verification_deadline() {
    let mut connection = connect().await;
    let tx = connection.peer_tx.clone();
    let peer = tokio::spawn(async move {
        loop {
            if tx.data_bytes(b"%window-add @1\n".to_vec()).await.is_err() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });
    let started = tokio::time::Instant::now();
    assert!(matches!(
        verify(&connection.tx, &mut connection.rx, b"", b"").await,
        Verification::Failed(Failure::Timeout)
    ));
    assert!(started.elapsed() < Duration::from_secs(7));
    peer.abort();
}

#[tokio::test]
async fn channel_exit_status_during_verification_is_not_shell_handback() {
    let mut connection = connect().await;
    connection.peer_tx.exit_status(42).await.unwrap();
    assert!(matches!(
        verify(&connection.tx, &mut connection.rx, b"", b"").await,
        Verification::Failed(Failure::ExitStatus(42))
    ));
}

#[tokio::test]
async fn probe_write_failure_is_not_reported_as_protocol_rejection() {
    let mut connection = connect().await;
    connection.disconnect().await;
    tokio::time::timeout(Duration::from_secs(2), async {
        while connection.rx.wait().await.is_some() {}
    })
    .await
    .unwrap();
    assert!(matches!(
        verify(&connection.tx, &mut connection.rx, b"", b"").await,
        Verification::Failed(Failure::WriteFailed)
    ));
}

#[tokio::test]
async fn unsupported_control_capability_detaches_and_waits_for_protocol_exit() {
    use crate::session_event_queue::SessionEventQueue;
    use crate::ssh_mailbox;
    use crate::tmux::session::{ControlExit, run_control_session};
    let mut connection = connect().await;
    let mut incoming = connection.peer_rx.take().unwrap();
    let outgoing = connection.peer_tx.clone();
    let peer = tokio::spawn(async move {
        let mut bytes = Vec::new();
        let mut rejected = false;
        loop {
            let Some(ChannelMsg::Data { data }) = incoming.wait().await else {
                panic!("missing command")
            };
            bytes.extend_from_slice(&data);
            while let Some(end) = bytes.iter().position(|byte| *byte == b'\n') {
                let line: Vec<u8> = bytes.drain(..=end).collect();
                if !rejected {
                    assert_eq!(line, b"refresh-client -f pause-after=2\n");
                    outgoing
                        .data_bytes(b"%begin 1 1 1\nunknown flag\n%error 1 1 1\n".to_vec())
                        .await
                        .unwrap();
                    rejected = true;
                }
                if line == b"detach-client\n" {
                    outgoing
                        .data_bytes(b"%exit detached\nprompt> ".to_vec())
                        .await
                        .unwrap();
                    return;
                }
            }
        }
    });
    let queue = SessionEventQueue::new();
    let sessions = Arc::new(Mutex::new(HashMap::new()));
    let (tx, mut rx) = super::mailbox::channel();
    let (_host, mut host_rx) = ssh_mailbox::channel();
    let channel = connection.tx.clone();
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        run_control_session(
            &queue,
            &sessions,
            "host",
            &channel,
            &mut connection.rx,
            &mut host_rx,
            &mut rx,
            tx,
            b"",
        ),
    )
    .await
    .unwrap();
    assert!(matches!(result, ControlExit::ReturnToShell { leftover } if leftover == b"prompt> "));
    peer.await.unwrap();
}

#[tokio::test]
async fn closing_a_host_without_command_responses_reaps_panes_and_releases_input() {
    use crate::session_event_queue::SessionEventQueue;
    use crate::tmux::session::{ControlExit, run_control_session};
    use crate::{SshCommand, ssh_mailbox};

    let mut connection = connect().await;
    let queue = SessionEventQueue::new();
    let sessions = Arc::new(Mutex::new(HashMap::new()));
    let (tx, mut rx) = super::mailbox::channel();
    let (host, mut host_rx) = ssh_mailbox::channel();
    // The peer never acknowledges bootstrap/seed/input commands.
    tx.write("%0", &vec![b'x'; crate::writer_mailbox::BYTE_LIMIT])
        .unwrap();
    let channel = connection.tx.clone();
    let driver = run_control_session(
        &queue,
        &sessions,
        "host",
        &channel,
        &mut connection.rx,
        &mut host_rx,
        &mut rx,
        tx.clone(),
        b"%layout-change @0 b25f,80x24,0,0,0 b25f,80x24,0,0,0 *\n",
    );
    let closer = async {
        tokio::time::sleep(Duration::from_millis(50)).await;
        host.close();
    };
    let (exit, ()) = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(driver, closer)
    })
    .await
    .unwrap();
    assert!(matches!(exit, ControlExit::Closed));
    assert!(sessions.lock().unwrap().is_empty());
    assert_eq!(tx.peak_bytes(), crate::writer_mailbox::BYTE_LIMIT);
    // The receiver can be reused only after discarding the old attachment.
    assert!(tx.write("%0", b"stale input").is_err());
    rx.resume();
    tx.write("%0", b"next generation").unwrap();
    assert!(
        matches!(rx.recv().await.unwrap().command, SshCommand::TmuxPaneInput { data, .. } if data == b"next generation")
    );
}

// Opt-in because a real tmux binary is required. CI runs this group explicitly
// on Linux and macOS. Windows can exercise the same SSH path against WSL tmux.
mod real {
    use super::{Arc, ChannelMsg, Duration, HashMap, Mutex, Verification, connect, verify};
    use crate::session_event_queue::SessionEventQueue;
    use crate::tmux::detect::{TmuxControlDetector, TmuxFeed};
    use crate::tmux::mailbox;
    use crate::tmux::session::{ControlExit, TmuxStatePayload, run_control_session};
    use crate::{ManagedSession, SessionEvent, SshCommand, ssh_mailbox};
    use nyaterm_terminal::TerminalCore;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn binary(name: &str) -> tokio::process::Command {
        #[cfg(windows)]
        {
            let mut command = tokio::process::Command::new("wsl.exe");
            command.args([
                "-d",
                &std::env::var("NYATERM_TEST_TMUX_WSL")
                    .expect("set NYATERM_TEST_TMUX_WSL to a WSL distribution"),
                "--exec",
                name,
            ]);
            command
        }
        #[cfg(not(windows))]
        {
            tokio::process::Command::new(name)
        }
    }

    fn command() -> tokio::process::Command {
        binary(&std::env::var("NYATERM_TEST_TMUX").unwrap_or_else(|_| "tmux".into()))
    }

    fn control_command(socket: &str, attach: bool) -> tokio::process::Command {
        let tmux = std::env::var("NYATERM_TEST_TMUX").unwrap_or_else(|_| "tmux".into());
        let mut command = binary("script");
        // -CC needs a real remote tty. script supplies one on both Unix hosts
        // and WSL, while its pipes are carried over an actual russh connection.
        #[cfg(target_os = "macos")]
        {
            command.args([
                "-q",
                "/dev/null",
                &tmux,
                "-L",
                socket,
                "-f",
                "/dev/null",
                "-CC",
            ]);
            if attach {
                command.args(["attach-session", "-t", "test"]);
            } else {
                command.args(["new-session", "-s", "test", "-x", "80", "-y", "24", "sh"]);
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let action = if attach {
                "attach-session -t test"
            } else {
                "new-session -s test -x 80 -y 24 sh"
            };
            command.args([
                "-q",
                "-e",
                "-c",
                &format!(
                    "'{}' -L {socket} -f /dev/null -CC {action}",
                    tmux.replace('\'', "'\"'\"'")
                ),
                "/dev/null",
            ]);
        }
        command
    }

    struct Socket(String);
    impl Drop for Socket {
        fn drop(&mut self) {
            let _ = command()
                .args(["-L", &self.0, "kill-server"])
                .as_std_mut()
                .output();
        }
    }

    struct Tmux {
        socket: Arc<Socket>,
        child: tokio::process::Child,
        input: tokio::task::JoinHandle<()>,
        output: tokio::task::JoinHandle<()>,
    }

    impl Drop for Tmux {
        fn drop(&mut self) {
            self.input.abort();
            self.output.abort();
            let _ = self.child.start_kill();
        }
    }

    struct Running {
        tmux: Tmux,
        driver: tokio::task::JoinHandle<ControlExit>,
        queue: SessionEventQueue,
        tx: mailbox::Sender,
        _host: ssh_mailbox::Sender,
        sessions: Arc<Mutex<HashMap<String, ManagedSession>>>,
        terminals: HashMap<String, TerminalCore>,
        state: Option<TmuxStatePayload>,
    }

    impl Drop for Running {
        fn drop(&mut self) {
            self.driver.abort();
        }
    }

    impl Running {
        async fn start() -> Self {
            Self::start_with(None).await
        }

        async fn start_with(socket: Option<Arc<Socket>>) -> Self {
            let mut connection = connect().await;
            let attach = socket.is_some();
            let socket = socket.unwrap_or_else(|| {
                Arc::new(Socket(format!(
                    "nyaterm-test-{}",
                    uuid::Uuid::new_v4().simple()
                )))
            });
            let mut process = control_command(&socket.0, attach);
            process
                .env("TERM", "xterm-256color")
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true);
            let mut child = process.spawn().expect("real tmux required");
            let mut stdin = child.stdin.take().unwrap();
            let mut stdout = child.stdout.take().unwrap();
            let mut incoming = connection.peer_rx.take().unwrap();
            let outgoing = connection.peer_tx.clone();
            let input = tokio::spawn(async move {
                while let Some(message) = incoming.wait().await {
                    if let ChannelMsg::Data { data } = message
                        && stdin.write_all(&data).await.is_err()
                    {
                        break;
                    }
                }
            });
            let output = tokio::spawn(async move {
                let mut buffer = vec![0; 16 * 1024];
                while let Ok(size) = stdout.read(&mut buffer).await {
                    if size == 0 {
                        break;
                    }
                    if outgoing.data_bytes(buffer[..size].to_vec()).await.is_err() {
                        return;
                    }
                }
                let _ = outgoing.eof().await;
            });
            let tmux = Tmux {
                socket,
                child,
                input,
                output,
            };
            let mut detector = TmuxControlDetector::new();
            detector.feed_input(b"tmux -CC\r");
            let initial = tokio::time::timeout(Duration::from_secs(15), async {
                loop {
                    let Some(ChannelMsg::Data { data }) = connection.rx.wait().await else {
                        panic!("tmux startup ended")
                    };
                    if let TmuxFeed::Detected { startup, rest, .. } = detector.feed(&data) {
                        let Verification::Confirmed(initial) =
                            verify(&connection.tx, &mut connection.rx, &startup, &rest).await
                        else {
                            panic!("tmux handshake failed")
                        };
                        break initial;
                    }
                }
            })
            .await
            .expect("tmux startup timeout");
            let queue = SessionEventQueue::new();
            let sessions = Arc::new(Mutex::new(HashMap::new()));
            let (tx, mut rx) = mailbox::channel();
            let (host, mut host_rx) = ssh_mailbox::channel();
            let (driver_queue, driver_sessions, driver_tx) =
                (queue.clone(), sessions.clone(), tx.clone());
            let driver = tokio::spawn(async move {
                let channel = connection.tx.clone();
                let exit = run_control_session(
                    &driver_queue,
                    &driver_sessions,
                    "host",
                    &channel,
                    &mut connection.rx,
                    &mut host_rx,
                    &mut rx,
                    driver_tx,
                    &initial,
                )
                .await;
                // Keep the SSH connection alive until the control driver exits.
                drop(connection);
                exit
            });
            Self {
                tmux,
                driver,
                queue,
                tx,
                _host: host,
                sessions,
                terminals: HashMap::new(),
                state: None,
            }
        }

        async fn remote(&self, args: &[&str]) -> String {
            let output = command()
                .args(["-L", &self.tmux.socket.0])
                .args(args)
                .output()
                .await
                .unwrap();
            assert!(
                output.status.success(),
                "tmux fixture command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).unwrap()
        }

        fn drain(&mut self) {
            for event in self.queue.drain(100_000).events {
                match event {
                    SessionEvent::Output { session_id, data } => {
                        self.terminals
                            .entry(session_id)
                            .or_insert_with(|| TerminalCore::new(80, 24))
                            .advance(&data);
                    }
                    SessionEvent::TmuxState { state, .. } => self.state = Some(*state),
                    _ => {}
                }
            }
        }

        async fn wait(&mut self, predicate: impl Fn(&Self) -> bool) {
            tokio::time::timeout(Duration::from_secs(20), async {
                loop {
                    self.drain();
                    if predicate(self) {
                        break;
                    }
                    assert!(
                        !self.driver.is_finished(),
                        "control driver exited before expected state"
                    );
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("tmux state timeout");
        }

        fn screen(&self, pane: &str) -> String {
            let Some(terminal) = self
                .terminals
                .get(&format!("host::pane{}", pane.trim_start_matches('%')))
            else {
                return String::new();
            };
            terminal
                .snapshot()
                .row_data
                .iter()
                .map(|row| row.text.trim_end())
                .collect::<Vec<_>>()
                .join("\n")
                .trim_end()
                .into()
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "requires real tmux; run explicitly in compatibility CI"]
    async fn real_tmux_pause_recaptures_high_volume_output_and_preserves_live_tail() {
        let mut run = Running::start().await;
        run.wait(|run| run.terminals.contains_key("host::pane0"))
            .await;
        let client = run
            .remote(&["list-clients", "-F", "#{client_name}"])
            .await
            .trim()
            .to_string();
        run.remote(&["refresh-client", "-t", &client, "-A", "%0:pause"])
            .await;
        run.remote(&["send-keys", "-t", "%0", "i=0; while [ $i -lt 6000 ]; do printf 'row-%06d\\n' $i; i=$((i+1)); done; printf 'RECOVERED\\n'", "Enter"]).await;
        // Intentionally leave the UI event queue undrained during the burst.
        tokio::time::sleep(Duration::from_millis(600)).await;
        run.wait(|run| run.screen("%0").contains("RECOVERED")).await;
        let expected = run.remote(&["capture-pane", "-p", "-t", "%0"]).await;
        assert_eq!(run.screen("%0"), expected.trim_end());
        run.tx.write("%0", b"printf 'LIVE-TAIL\\n'\r").unwrap();
        run.wait(|run| run.screen("%0").contains("LIVE-TAIL\n"))
            .await;
        let expected = run.remote(&["capture-pane", "-p", "-t", "%0"]).await;
        assert_eq!(run.screen("%0"), expected.trim_end());
        // Reconstruct retained history as well as the current screen.
        let expected_history = run
            .remote(&["capture-pane", "-p", "-S", "-", "-t", "%0"])
            .await;
        let history = run.terminals["host::pane0"].viewport_snapshot_with_window(0, 10000, 0);
        let actual_history = history
            .row_data
            .iter()
            .map(|row| row.text.trim_end())
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(actual_history.trim_end(), expected_history.trim_end());
        run.tx.send(SshCommand::TmuxDetach).unwrap();
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(10), &mut run.driver)
                .await
                .unwrap()
                .unwrap(),
            ControlExit::ReturnToShell { .. }
        ));
        assert!(run.sessions.lock().unwrap().is_empty());
        assert_eq!(
            run.remote(&["list-sessions", "-F", "#{session_name}"])
                .await
                .trim(),
            "test"
        );
        let mut reattached = Running::start_with(Some(run.tmux.socket.clone())).await;
        reattached
            .wait(|run| run.screen("%0").contains("LIVE-TAIL"))
            .await;
        let expected = reattached.remote(&["capture-pane", "-p", "-t", "%0"]).await;
        assert_eq!(reattached.screen("%0"), expected.trim_end());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "requires real tmux; run explicitly in compatibility CI"]
    async fn real_tmux_multiple_windows_pane_recreation_and_server_exit_reap_virtual_sessions() {
        let mut run = Running::start().await;
        run.wait(|run| run.terminals.contains_key("host::pane0"))
            .await;
        run.remote(&["new-window", "-n", "second", "sh"]).await;
        run.remote(&["split-window", "-h", "sh"]).await;
        run.wait(|run| {
            run.state
                .as_ref()
                .is_some_and(|state| state.windows.len() == 2)
                && run.sessions.lock().unwrap().len() == 3
        })
        .await;
        run.remote(&["kill-pane", "-t", "%2"]).await;
        run.wait(|run| run.sessions.lock().unwrap().len() == 2)
            .await;
        run.remote(&["split-window", "-h", "sh"]).await;
        run.wait(|run| run.sessions.lock().unwrap().contains_key("host::pane3"))
            .await;
        run.remote(&["kill-server"]).await;
        let _ = tokio::time::timeout(Duration::from_secs(10), &mut run.driver)
            .await
            .unwrap()
            .unwrap();
        assert!(run.sessions.lock().unwrap().is_empty());
        run.drain();
        assert!(run.state.as_ref().unwrap().exited);
    }
}
