//! Own the Agent CLI process tree so inherited pipes cannot outlive cleanup.

use std::io;
use std::process::{Child, Command};

pub(crate) struct AgentChild {
    child: Child,
    #[cfg(windows)]
    job: windows::Job,
}

impl AgentChild {
    pub(crate) fn spawn(command: &mut Command) -> io::Result<Self> {
        #[cfg(windows)]
        let job = {
            use std::os::windows::process::CommandExt as _;
            use windows_sys::Win32::System::Threading::{CREATE_NO_WINDOW, CREATE_SUSPENDED};

            // Assign the suspended process before .cmd/Node can spawn children.
            command.creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
            windows::Job::new()?
        };
        let owner = Self {
            child: command.spawn()?,
            #[cfg(windows)]
            job,
        };
        #[cfg(windows)]
        if let Err(error) = owner.job.assign_and_resume(&owner.child) {
            drop(owner);
            return Err(error);
        }
        Ok(owner)
    }

    pub(crate) fn child_mut(&mut self) -> &mut Child {
        &mut self.child
    }

    pub(crate) fn terminate(&mut self) {
        #[cfg(windows)]
        self.job.terminate();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for AgentChild {
    fn drop(&mut self) {
        self.terminate();
    }
}

#[cfg(all(test, windows))]
mod tests {
    use std::io::{BufRead, BufReader, Read};
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::time::Duration;

    use nyaterm_core::test_support::TestTempDir;
    use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };

    use super::AgentChild;

    #[test]
    fn cmd_cleanup_reaps_descendants_before_waiting_for_pipe_eof() {
        let directory = TestTempDir::new("nyaterm-agent-settings-cmd-cleanup");
        std::fs::create_dir_all(directory.path()).unwrap();
        let wrapper = directory.path().join("mock-codex.cmd");
        let executable = std::env::current_exe().unwrap();
        std::fs::write(
            &wrapper,
            format!(
                "@echo off\r\n\"{}\" --exact account::tests::mock_codex_app_server --nocapture\r\n",
                executable.display(),
            ),
        )
        .unwrap();
        let mut command = Command::new(wrapper);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .env("NYATERM_MOCK_CODEX_ACCOUNT_SERVER", "1")
            .env("NYATERM_MOCK_CODEX_KEEP_OPEN", "1");
        let mut child = AgentChild::spawn(&mut command).unwrap();
        let mut stdout = BufReader::new(child.child_mut().stdout.take().unwrap());
        let pid = loop {
            let mut line = String::new();
            assert_ne!(stdout.read_line(&mut line).unwrap(), 0, "mock must start");
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&line)
                && let Some(pid) = value["fixturePid"].as_u64()
            {
                break pid as u32;
            }
        };
        assert_ne!(
            pid,
            child.child_mut().id(),
            "fixture must be a .cmd descendant"
        );
        // SAFETY: hold a handle to the fixture before cleanup to avoid PID reuse.
        let raw = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        assert!(!raw.is_null());
        let descendant = unsafe { OwnedHandle::from_raw_handle(raw) };
        let (done_tx, done_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            child.terminate();
            let mut output = Vec::new();
            stdout.read_to_end(&mut output).unwrap();
            done_tx.send(()).unwrap();
        });
        done_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("cleanup must not wait for the descendant's 30 second sleep");
        assert_eq!(
            unsafe { WaitForSingleObject(descendant.as_raw_handle(), 2_000) },
            WAIT_OBJECT_0
        );
        worker.join().unwrap();
    }
}

#[cfg(windows)]
mod windows {
    use std::io;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::process::Child;

    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject,
    };
    use windows_sys::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};

    pub(super) struct Job(OwnedHandle);

    impl Job {
        pub(super) fn new() -> io::Result<Self> {
            // SAFETY: unnamed, non-inheritable job; the returned handle is owned here.
            let raw = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if raw.is_null() {
                return Err(io::Error::last_os_error());
            }
            let job = Self(unsafe { OwnedHandle::from_raw_handle(raw) });
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            // SAFETY: limits has the layout/size required by this information class.
            if unsafe {
                SetInformationJobObject(
                    job.0.as_raw_handle(),
                    JobObjectExtendedLimitInformation,
                    (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                    std::mem::size_of_val(&limits) as u32,
                )
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(job)
        }

        pub(super) fn assign_and_resume(&self, child: &Child) -> io::Result<()> {
            // SAFETY: both handles are live; the child was created suspended.
            if unsafe { AssignProcessToJobObject(self.0.as_raw_handle(), child.as_raw_handle()) }
                == 0
            {
                return Err(io::Error::last_os_error());
            }
            // Rust Child does not expose its primary thread handle. Before resume
            // the new process has only that thread, so find it by its owned PID.
            let raw = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
            if raw == INVALID_HANDLE_VALUE {
                return Err(io::Error::last_os_error());
            }
            let snapshot = unsafe { OwnedHandle::from_raw_handle(raw) };
            let mut entry = THREADENTRY32 {
                dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
                ..Default::default()
            };
            let mut found = unsafe { Thread32First(snapshot.as_raw_handle(), &mut entry) };
            while found != 0 {
                if entry.th32OwnerProcessID == child.id() {
                    let raw = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) };
                    if raw.is_null() {
                        return Err(io::Error::last_os_error());
                    }
                    let thread = unsafe { OwnedHandle::from_raw_handle(raw) };
                    if unsafe { ResumeThread(thread.as_raw_handle()) } == u32::MAX {
                        return Err(io::Error::last_os_error());
                    }
                    return Ok(());
                }
                entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
                found = unsafe { Thread32Next(snapshot.as_raw_handle(), &mut entry) };
            }
            Err(io::Error::other("Agent CLI primary thread not found"))
        }

        pub(super) fn terminate(&self) {
            // SAFETY: terminate only the private job owning this CLI and descendants.
            unsafe { TerminateJobObject(self.0.as_raw_handle(), 1) };
        }
    }
}
