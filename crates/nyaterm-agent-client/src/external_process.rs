//! Shared child-process and pipe-reader ownership for external Agent CLIs.

use std::io::{BufRead, BufReader};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};
use std::sync::mpsc::{self, Receiver};
use std::thread::JoinHandle;

pub(crate) mod child_process;

use child_process::AgentChild;

pub(crate) struct ExternalAgentChild {
    child: AgentChild,
    readers: Vec<JoinHandle<()>>,
}

impl ExternalAgentChild {
    pub fn spawn(command: &mut Command) -> std::io::Result<Self> {
        Ok(Self {
            child: AgentChild::spawn(command)?,
            readers: Vec::with_capacity(2),
        })
    }

    pub fn child_mut(&mut self) -> &mut Child {
        self.child.child_mut()
    }

    pub fn take_stdin(&mut self) -> Option<ChildStdin> {
        self.child_mut().stdin.take()
    }

    pub fn take_stdout(&mut self) -> Option<ChildStdout> {
        self.child_mut().stdout.take()
    }

    pub fn take_stderr(&mut self) -> Option<ChildStderr> {
        self.child_mut().stderr.take()
    }

    pub fn capture_output(
        &mut self,
        worker_name: &'static str,
        stdout: ChildStdout,
        stderr: ChildStderr,
        log_stderr: impl Fn(String) + Send + 'static,
    ) -> std::io::Result<Receiver<Result<String, String>>> {
        let (line_tx, line_rx) = mpsc::channel();
        self.readers
            .push(spawn_reader(format!("{worker_name}-stdout"), move || {
                for line in BufReader::new(stdout).lines() {
                    if line_tx
                        .send(line.map_err(|error| error.to_string()))
                        .is_err()
                    {
                        break;
                    }
                }
            })?);
        self.readers
            .push(spawn_reader(format!("{worker_name}-stderr"), move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    log_stderr(line);
                }
            })?);
        Ok(line_rx)
    }
}

impl Drop for ExternalAgentChild {
    fn drop(&mut self) {
        // Descendants can inherit both pipes, even after the CLI itself exits.
        // Terminate the whole tree before waiting for the readers to see EOF.
        self.child.terminate();
        for reader in self.readers.drain(..) {
            let _ = reader.join();
        }
    }
}

/// Named reader threads stay owned by the process guard that joins them.
pub(crate) fn spawn_reader<T: Send + 'static>(
    name: impl Into<String>,
    task: impl FnOnce() -> T + Send + 'static,
) -> std::io::Result<JoinHandle<T>> {
    std::thread::Builder::new().name(name.into()).spawn(task)
}

#[cfg(all(test, windows))]
mod tests {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use nyaterm_core::test_support::TestTempDir;
    use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };

    use super::ExternalAgentChild;

    #[test]
    fn cancelled_agent_reaps_descendants_before_joining_output_readers() {
        assert_descendant_cleanup(false);
    }

    #[test]
    fn exited_agent_reaps_descendants_that_still_hold_output_pipes() {
        assert_descendant_cleanup(true);
    }

    fn assert_descendant_cleanup(wrapper_exits: bool) {
        let directory = TestTempDir::new("nyaterm-external-agent-descendants");
        std::fs::create_dir_all(directory.path()).unwrap();
        let wrapper = directory.join("agent.cmd");
        let executable = std::env::current_exe().unwrap();
        let prefix = if wrapper_exits { "start \"\" /b " } else { "" };
        std::fs::write(
            &wrapper,
            format!(
                "@echo off\r\n{prefix}\"{}\" --exact account::tests::mock_codex_app_server --nocapture\r\n",
                executable.display(),
            ),
        )
        .unwrap();
        let mut command = Command::new(wrapper);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("NYATERM_MOCK_CODEX_ACCOUNT_SERVER", "1")
            .env("NYATERM_MOCK_CODEX_KEEP_OPEN", "1");
        let mut child = ExternalAgentChild::spawn(&mut command).unwrap();
        let stdout = child.take_stdout().unwrap();
        let stderr = child.take_stderr().unwrap();
        let lines = child
            .capture_output("nyaterm-agent-cleanup-test", stdout, stderr, |_| {})
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let pid = loop {
            let line = lines
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("descendant must start")
                .unwrap();
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&line)
                && let Some(pid) = value["fixturePid"].as_u64()
            {
                break pid as u32;
            }
        };
        assert_ne!(pid, child.child_mut().id());
        // SAFETY: hold the descendant's process handle across cleanup to avoid PID reuse.
        let raw = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        assert!(!raw.is_null());
        let descendant = unsafe { OwnedHandle::from_raw_handle(raw) };
        if wrapper_exits {
            while child.child_mut().try_wait().unwrap().is_none() {
                assert!(
                    Instant::now() < deadline,
                    "wrapper must exit before cleanup"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
        } else {
            // Match cancellation in the Claude runtime, which kills the direct child first.
            child.child_mut().kill().unwrap();
        }
        let (done_tx, done_rx) = mpsc::channel();
        let cleanup = std::thread::spawn(move || {
            drop(child);
            let _ = done_tx.send(());
        });
        done_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("cleanup must not wait for the descendant's 30 second sleep");
        assert_eq!(
            unsafe { WaitForSingleObject(descendant.as_raw_handle(), 2_000) },
            WAIT_OBJECT_0,
        );
        cleanup.join().unwrap();
    }
}
