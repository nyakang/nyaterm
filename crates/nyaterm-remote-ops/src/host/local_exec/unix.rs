use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::process::{Child, ChildStdin, Command};
use zeroize::Zeroizing;

pub(super) trait PipeHandle: AsRawFd {}
impl<T: AsRawFd> PipeHandle for T {}

pub(super) fn prepare_pipe(pipe: &impl PipeHandle) -> io::Result<()> {
    // These pipe endpoints have one owner; nonblocking I/O lets the supervisor
    // service stdin and both outputs without auxiliary threads or a runtime.
    let flags = unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_GETFL) };
    if flags < 0
        || unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub(super) fn read_pipe(
    pipe: &mut (impl Read + PipeHandle),
    buffer: &mut [u8],
) -> io::Result<Option<usize>> {
    match pipe.read(buffer) {
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
            ) =>
        {
            Ok(None)
        }
        result => result.map(Some),
    }
}

pub(super) struct Process {
    pub(super) child: Child,
    group: libc::pid_t,
}
impl Process {
    pub(super) fn spawn(command: &mut Command) -> io::Result<Self> {
        command.process_group(0);
        let child = command.spawn()?;
        Ok(Self {
            group: child.id() as libc::pid_t,
            child,
        })
    }
    pub(super) fn terminate(&mut self) {
        if self.group == 0 {
            return;
        }
        // The child entered its private group before exec, never our group.
        unsafe {
            libc::kill(-self.group, libc::SIGKILL);
        }
        self.group = 0;
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
    pub(super) fn complete(&mut self) -> io::Result<()> {
        self.group = 0;
        Ok(())
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        self.terminate();
    }
}

pub(super) struct Input {
    pipe: Option<ChildStdin>,
    bytes: Zeroizing<Vec<u8>>,
    offset: usize,
}
impl Input {
    pub(super) fn new(pipe: Option<ChildStdin>, bytes: Zeroizing<Vec<u8>>) -> io::Result<Self> {
        if let Some(pipe) = &pipe {
            prepare_pipe(pipe)?;
        }
        Ok(Self {
            pipe,
            bytes,
            offset: 0,
        })
    }
    pub(super) fn finished(&self) -> bool {
        self.pipe.is_none()
    }
    pub(super) fn pump(&mut self) -> io::Result<bool> {
        let Some(pipe) = self.pipe.as_mut() else {
            return Ok(false);
        };
        if self.offset == self.bytes.len() {
            self.pipe = None;
            return Ok(true);
        }
        let end = (self.offset + super::CHUNK).min(self.bytes.len());
        match pipe.write(&self.bytes[self.offset..end]) {
            Ok(0) => {
                self.pipe = None;
                Ok(true)
            }
            Ok(count) => {
                self.offset += count;
                Ok(true)
            }
            Err(error) if error.kind() == io::ErrorKind::BrokenPipe => {
                self.pipe = None;
                Ok(true)
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) =>
            {
                Ok(false)
            }
            Err(error) => Err(error),
        }
    }
}
