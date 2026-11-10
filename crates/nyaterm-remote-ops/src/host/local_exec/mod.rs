//! A single deadline covers the process and all inherited pipes.
#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;
#[cfg(unix)]
use unix as platform;
#[cfg(windows)]
use windows as platform;

use super::HostCommandError;
use nyaterm_transport::ssh_execution::RemoteCommandOutput;
use std::io::{self, Read};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

const OUTPUT_LIMIT: usize = 16 * 1024 * 1024;
const CHUNK: usize = 64 * 1024;

struct Supervisor {
    process: platform::Process,
    input: Option<platform::Input>,
}
impl Drop for Supervisor {
    fn drop(&mut self) {
        // This ordering also applies during setup failures and panic unwinding.
        // A blocked Windows writer must lose its readers before we join it.
        self.process.terminate();
        drop(self.input.take());
    }
}

struct Output<R> {
    pipe: R,
    bytes: Vec<u8>,
    eof: bool,
}
impl<R: Read + platform::PipeHandle> Output<R> {
    fn new(pipe: R) -> io::Result<Self> {
        platform::prepare_pipe(&pipe)?;
        Ok(Self {
            pipe,
            bytes: Vec::new(),
            eof: false,
        })
    }
    fn pump(&mut self) -> anyhow::Result<bool> {
        if self.eof {
            return Ok(false);
        }
        let mut buffer = [0; CHUNK];
        match platform::read_pipe(&mut self.pipe, &mut buffer)? {
            None => Ok(false),
            Some(0) => {
                self.eof = true;
                Ok(true)
            }
            Some(count) => {
                if self.bytes.len().saturating_add(count) > OUTPUT_LIMIT {
                    return Err(HostCommandError::OutputLimitExceeded.into());
                }
                self.bytes.extend_from_slice(&buffer[..count]);
                Ok(true)
            }
        }
    }
}

pub(super) fn run_local(
    command: &mut Command,
    input: Option<String>,
    timeout: Duration,
) -> anyhow::Result<RemoteCommandOutput> {
    let has_input = input.is_some();
    let input_bytes = Zeroizing::new(input.unwrap_or_default().into_bytes());
    let deadline = Instant::now() + timeout;
    command
        .stdin(if has_input {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut owner = Supervisor {
        process: platform::Process::spawn(command)?,
        input: None,
    };
    owner.input = Some(platform::Input::new(
        owner.process.child.stdin.take(),
        input_bytes,
    )?);
    let result = (|| {
        let mut stdout = Output::new(
            owner
                .process
                .child
                .stdout
                .take()
                .ok_or_else(|| io::Error::other("stdout unavailable"))?,
        )?;
        let mut stderr = Output::new(
            owner
                .process
                .child
                .stderr
                .take()
                .ok_or_else(|| io::Error::other("stderr unavailable"))?,
        )?;
        let mut status = None;
        let mut delay = Duration::from_millis(1);
        let input = owner.input.as_mut().expect("supervisor input initialized");
        loop {
            if Instant::now() >= deadline {
                return Err(HostCommandError::Timeout(timeout).into());
            }
            let progress = stdout.pump()? | stderr.pump()? | input.pump()?;
            if status.is_none() {
                status = owner.process.child.try_wait()?;
            }
            if let Some(status) = status
                && stdout.eof
                && stderr.eof
                && input.finished()
            {
                return Ok(RemoteCommandOutput {
                    stdout: String::from_utf8_lossy(&stdout.bytes).into_owned(),
                    stderr: String::from_utf8_lossy(&stderr.bytes).into_owned(),
                    exit_status: status.code().and_then(|code| u32::try_from(code).ok()),
                });
            }
            if progress {
                delay = Duration::from_millis(1);
            } else {
                std::thread::sleep(delay.min(deadline.saturating_duration_since(Instant::now())));
                delay = (delay * 2).min(Duration::from_millis(20));
            }
        }
    })();
    if result.is_ok() {
        // Detached-pipe descendants may be services intentionally started by
        // this command. Only supervision failures warrant killing the tree.
        owner.process.complete()?;
    }
    result
}

#[cfg(all(test, unix))]
mod tests {
    use super::{Supervisor, platform, run_local};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    fn run(
        script: &str,
        input: Option<String>,
        timeout: Duration,
    ) -> anyhow::Result<nyaterm_transport::ssh_execution::RemoteCommandOutput> {
        run_local(Command::new("sh").args(["-c", script]), input, timeout)
    }

    #[test]
    fn inherited_pipes_remain_subject_to_deadline_after_parent_exits() {
        let start = Instant::now();
        let error = run("sleep 10 & printf done", None, Duration::from_millis(150)).unwrap_err();
        assert!(error.to_string().contains("timed out"));
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn completed_command_preserves_background_work_with_redirected_pipes() {
        let directory = tempfile::tempdir().unwrap();
        for code in [0, 7] {
            let marker = directory.path().join(format!("completed-{code}"));
            let release = directory.path().join(format!("release-{code}"));
            let script = format!(
                "(attempt=0; while [ ! -e {} ]; do [ $attempt -lt 300 ] || exit 1; attempt=$((attempt+1)); sleep 0.01; done; touch {}) </dev/null >/dev/null 2>&1 & exit {code}",
                crate::host::sh_quote(release.to_str().unwrap()),
                crate::host::sh_quote(marker.to_str().unwrap())
            );
            let output = run(&script, None, Duration::from_secs(2)).unwrap();
            assert_eq!(output.exit_status, Some(code));
            // Let the descendant finish only after the supervisor has returned.
            std::fs::write(release, b"released").unwrap();
            let deadline = Instant::now() + Duration::from_secs(3);
            while !marker.exists() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(marker.exists(), "normal completion killed background work");
        }
    }

    #[test]
    fn panic_unwinding_reaps_the_tree_before_dropping_input() {
        let directory = tempfile::tempdir().unwrap();
        let marker = directory.path().join("escaped");
        let ready = directory.path().join("ready");
        let script = format!(
            "(sleep 0.6; touch {}) & printf ready > {}; wait",
            crate::host::sh_quote(marker.to_str().unwrap()),
            crate::host::sh_quote(ready.to_str().unwrap())
        );
        let mut command = Command::new("sh");
        command
            .args(["-c", &script])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut owner = Supervisor {
            process: platform::Process::spawn(&mut command).unwrap(),
            input: None,
        };
        owner.input = Some(
            platform::Input::new(
                owner.process.child.stdin.take(),
                zeroize::Zeroizing::new(vec![b'x'; 4_000_000]),
            )
            .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(2);
        while !ready.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(ready.exists());
        let result = std::panic::catch_unwind(move || {
            let _owner = owner;
            panic!("controlled supervisor panic");
        });
        assert!(result.is_err());
        std::thread::sleep(Duration::from_millis(700));
        assert!(!marker.exists());
        assert!(run("exit 0", None, Duration::from_secs(2)).is_ok());
    }

    #[test]
    fn timeout_reaps_the_group_and_the_next_command_runs() {
        let directory = tempfile::tempdir().unwrap();
        let marker = directory.path().join("escaped");
        let script = format!(
            "(sleep 0.6; touch {}) & wait",
            crate::host::sh_quote(marker.to_str().unwrap())
        );
        assert!(run(&script, None, Duration::from_millis(100)).is_err());
        std::thread::sleep(Duration::from_millis(700));
        assert!(!marker.exists(), "the descendant survived cleanup");
        assert_eq!(
            run("printf recovered", None, Duration::from_secs(2))
                .unwrap()
                .stdout,
            "recovered"
        );
    }

    #[test]
    fn large_input_and_both_output_streams_do_not_deadlock() {
        let data = "x".repeat(2_000_000);
        let output = run(
            "tee /dev/stderr; exit 7",
            Some(data.clone()),
            Duration::from_secs(10),
        )
        .unwrap();
        assert_eq!(output.stdout, data);
        assert_eq!(output.stderr, data);
        assert_eq!(output.exit_status, Some(7));
    }

    #[test]
    fn stalled_stdin_and_continuous_dual_output_obey_the_same_deadline() {
        for (script, input) in [
            ("sleep 10", Some("x".repeat(4_000_000))),
            ("while :; do printf x; printf x >&2; done", None),
        ] {
            let start = Instant::now();
            let error = run(script, input, Duration::from_millis(120)).unwrap_err();
            assert!(matches!(
                error.downcast_ref::<crate::host::HostCommandError>(),
                Some(crate::host::HostCommandError::Timeout(_))
            ));
            assert!(start.elapsed() < Duration::from_secs(2));
        }
    }

    #[test]
    fn stderr_output_has_its_own_limit_and_cleanup_allows_more_work() {
        let error = run("yes x >&2", None, Duration::from_secs(10)).unwrap_err();
        assert!(matches!(
            error.downcast_ref::<crate::host::HostCommandError>(),
            Some(crate::host::HostCommandError::OutputLimitExceeded)
        ));
        assert!(run("exit 0", None, Duration::from_secs(2)).is_ok());
    }

    #[test]
    fn continuous_output_is_bounded_without_returning_partial_success() {
        let error = run("yes x", None, Duration::from_secs(10)).unwrap_err();
        assert!(matches!(
            error.downcast_ref::<crate::host::HostCommandError>(),
            Some(crate::host::HostCommandError::OutputLimitExceeded)
        ));
    }
}

#[cfg(all(test, windows))]
mod windows_tests;
