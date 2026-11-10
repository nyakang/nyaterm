use std::io::{self, Read, Write};
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::process::{Child, ChildStdin, Command};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::JoinHandle;
use windows_sys::Win32::Foundation::ERROR_BROKEN_PIPE;
use windows_sys::Win32::System::IO::CancelSynchronousIo;
use windows_sys::Win32::System::Pipes::PeekNamedPipe;
use windows_sys::Win32::System::Threading::{CREATE_NO_WINDOW, CREATE_SUSPENDED};
use zeroize::Zeroizing;

pub(super) trait PipeHandle: AsRawHandle {}
impl<T: AsRawHandle> PipeHandle for T {}
pub(super) fn prepare_pipe(_: &impl PipeHandle) -> io::Result<()> {
    Ok(())
}
pub(super) fn read_pipe(
    pipe: &mut (impl Read + PipeHandle),
    buffer: &mut [u8],
) -> io::Result<Option<usize>> {
    let mut available = 0;
    // The supervisor is the only reader, so available bytes cannot be consumed
    // elsewhere between PeekNamedPipe and ReadFile.
    if unsafe {
        PeekNamedPipe(
            pipe.as_raw_handle(),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            &mut available,
            std::ptr::null_mut(),
        )
    } == 0
    {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(ERROR_BROKEN_PIPE as i32) {
            return Ok(Some(0));
        }
        return Err(error);
    }
    if available == 0 {
        return Ok(None);
    }
    let count = buffer.len().min(available as usize);
    pipe.read(&mut buffer[..count]).map(Some)
}

pub(super) struct Process {
    pub(super) child: Child,
    job: job::Job,
    armed: bool,
}
impl Process {
    pub(super) fn spawn(command: &mut Command) -> io::Result<Self> {
        command.creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
        let job = job::Job::new()?;
        let owner = Self {
            child: command.spawn()?,
            job,
            armed: true,
        };
        owner.job.assign_and_resume(&owner.child)?;
        Ok(owner)
    }
    pub(super) fn terminate(&mut self) {
        if !self.armed {
            return;
        }
        self.job.terminate();
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.armed = false;
    }
    pub(super) fn complete(&mut self) -> io::Result<()> {
        self.job.release()?;
        self.armed = false;
        Ok(())
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        self.terminate();
    }
}

pub(super) struct Input {
    writer: Option<JoinHandle<io::Result<()>>>,
    cancelled: Arc<AtomicBool>,
}
impl Input {
    pub(super) fn new(pipe: Option<ChildStdin>, bytes: Zeroizing<Vec<u8>>) -> io::Result<Self> {
        let cancelled = Arc::new(AtomicBool::new(false));
        let stop = cancelled.clone();
        let writer = match pipe {
            None => None,
            Some(mut pipe) => Some(
                std::thread::Builder::new()
                    .name("host-command-stdin".into())
                    .spawn(move || {
                        for chunk in bytes.chunks(super::CHUNK) {
                            if stop.load(Ordering::Acquire) {
                                break;
                            }
                            if let Err(error) = pipe.write_all(chunk) {
                                if stop.load(Ordering::Acquire)
                                    || error.kind() == io::ErrorKind::BrokenPipe
                                {
                                    break;
                                }
                                return Err(error);
                            }
                        }
                        Ok(())
                    })?,
            ),
        };
        Ok(Self { writer, cancelled })
    }
    pub(super) fn finished(&self) -> bool {
        self.writer.is_none()
    }
    pub(super) fn pump(&mut self) -> io::Result<bool> {
        if self.writer.as_ref().is_some_and(JoinHandle::is_finished) {
            self.writer
                .take()
                .unwrap()
                .join()
                .map_err(|_| io::Error::other("stdin writer failed"))??;
            return Ok(true);
        }
        Ok(false)
    }
}
impl Drop for Input {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        if let Some(writer) = self.writer.take() {
            // Repeat to cover cancellation racing with the start of WriteFile.
            while !writer.is_finished() {
                unsafe {
                    CancelSynchronousIo(writer.as_raw_handle());
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            let _ = writer.join();
        }
    }
}

mod job {
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
            Err(io::Error::other("local command primary thread not found"))
        }

        pub(super) fn terminate(&self) {
            // SAFETY: terminate only the private job owning this CLI and descendants.
            unsafe { TerminateJobObject(self.0.as_raw_handle(), 1) };
        }

        pub(super) fn release(&self) -> io::Result<()> {
            // Clearing the only configured limit prevents closing our last
            // job handle from killing services with no inherited pipes.
            let limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            // SAFETY: the handle is owned and limits has the required layout.
            if unsafe {
                SetInformationJobObject(
                    self.0.as_raw_handle(),
                    JobObjectExtendedLimitInformation,
                    (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                    std::mem::size_of_val(&limits) as u32,
                )
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }
    }
}
