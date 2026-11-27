//! Own the Android screen process tree so inherited pipes cannot outlive cleanup.

use std::io;
use std::process::{Child, Command};

pub(crate) struct ScreenChild {
    child: Child,
    #[cfg(windows)]
    job: windows::Job,
}

impl ScreenChild {
    pub(crate) fn spawn(command: &mut Command) -> io::Result<Self> {
        #[cfg(windows)]
        let job = {
            use std::os::windows::process::CommandExt as _;
            use windows_sys::Win32::System::Threading::{CREATE_NO_WINDOW, CREATE_SUSPENDED};

            // Assign the suspended helper before it can spawn ADB children.
            command.creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
            windows::Job::new()?
        };
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
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
        #[cfg(unix)]
        // SAFETY: this child was made the leader of its own process group.
        unsafe {
            libc::kill(-(self.child.id() as i32), libc::SIGKILL);
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for ScreenChild {
    fn drop(&mut self) {
        self.terminate();
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
            Err(io::Error::other("Android screen primary thread not found"))
        }

        pub(super) fn terminate(&self) {
            // SAFETY: terminate only the private job owning this helper and descendants.
            unsafe { TerminateJobObject(self.0.as_raw_handle(), 1) };
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use crate::screen_process::ScreenChild;
    use std::io::{BufRead, BufReader};
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };

    #[test]
    #[ignore = "child-process fixture"]
    fn screen_descendant_fixture() {
        if std::env::var_os("NYATERM_SCREEN_PROCESS_FIXTURE").is_none() {
            return;
        }
        let mut child = Command::new("powershell.exe")
            .args(["-NoProfile", "-Command", "Start-Sleep -Seconds 60"])
            .creation_flags(0x08000000)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        println!("descendant:{}", child.id());
        let _ = child.wait();
    }

    #[test]
    fn screen_cleanup_terminates_adb_descendants() {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "screen_process::tests::screen_descendant_fixture",
                "--ignored",
                "--nocapture",
            ])
            .env("NYATERM_SCREEN_PROCESS_FIXTURE", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = ScreenChild::spawn(&mut command).unwrap();
        let mut reader = BufReader::new(child.child_mut().stdout.take().unwrap());
        let pid = loop {
            let mut line = String::new();
            assert_ne!(reader.read_line(&mut line).unwrap(), 0);
            if let Some(pid) = line.trim().strip_prefix("descendant:") {
                break pid.parse::<u32>().unwrap();
            }
        };
        // SAFETY: keep a process handle before termination to avoid PID-reuse races.
        let raw = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        assert!(!raw.is_null());
        let descendant = unsafe { OwnedHandle::from_raw_handle(raw) };
        child.terminate();
        assert_eq!(
            unsafe { WaitForSingleObject(descendant.as_raw_handle(), 2_000) },
            WAIT_OBJECT_0
        );
    }
}
