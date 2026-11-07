//! A ProxyCommand's pipes and process have the same lifetime, including cancellation.
use std::pin::Pin;
use std::process::Stdio;
use std::thread::JoinHandle;

use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite};
use tokio::sync::oneshot;

use crate::ssh_connection::{expand_proxy_command, system_shell_command};

pub(crate) struct ProxyCommandStream {
    stdout: tokio::process::ChildStdout,
    stdin: tokio::process::ChildStdin,
    cancel: Option<oneshot::Sender<()>>,
    supervisor: Option<JoinHandle<()>>,
}

impl Drop for ProxyCommandStream {
    fn drop(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            let _ = cancel.send(());
        }
        // These streams only live on transport workers. A dedicated supervisor
        // keeps its reactor alive to kill AND reap the child even if the caller's
        // runtime is already shutting down.
        if let Some(supervisor) = self.supervisor.take() {
            let _ = supervisor.join();
        }
    }
}

pub(crate) async fn open_proxy_command_stream(
    template: Option<&str>,
    host: &str,
    port: u16,
    username: &str,
) -> anyhow::Result<ProxyCommandStream> {
    let command = expand_proxy_command(template, host, port, username)?;
    let (started, startup) = oneshot::channel();
    let (cancel, mut cancelled) = oneshot::channel();
    let supervisor = std::thread::Builder::new()
        .name("nyaterm-proxy-command".into())
        .spawn(move || {
            let result = (|| -> anyhow::Result<_> {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()?;
                Ok(runtime)
            })();
            let runtime = match result {
                Ok(runtime) => runtime,
                Err(error) => {
                    let _ = started.send(Err(error));
                    return;
                }
            };
            runtime.block_on(async move {
                let mut builder = system_shell_command(&command);
                builder
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .kill_on_drop(true);
                #[cfg(unix)]
                {
                    use std::os::unix::process::CommandExt as _;
                    builder.as_std_mut().process_group(0);
                }
                let mut process = match builder.spawn() {
                    Ok(process) => process,
                    Err(error) => {
                        let _ = started.send(Err(anyhow::anyhow!(
                            "ProxyCommand failed to start: {error}"
                        )));
                        return;
                    }
                };
                let group = match ProcessGroup::new(&process) {
                    Ok(group) => group,
                    Err(error) => {
                        let _ = process.kill().await;
                        let _ = started.send(Err(error.into()));
                        return;
                    }
                };
                let stderr_task = process.stderr.take().map(|mut stderr| {
                    tokio::spawn(async move {
                        let mut buffer = [0; 1024];
                        // ProxyCommand stderr can contain credentials or user data.
                        while matches!(stderr.read(&mut buffer).await, Ok(n) if n != 0) {}
                    })
                });
                let pipes = process.stdin.take().zip(process.stdout.take());
                let delivered = match pipes {
                    Some(pipes) => started.send(Ok(pipes)).is_ok(),
                    None => {
                        let _ =
                            started.send(Err(anyhow::anyhow!("ProxyCommand pipes unavailable")));
                        false
                    }
                };
                let exited = if delivered {
                    tokio::select! {
                        _ = process.wait() => true,
                        _ = &mut cancelled => false,
                    }
                } else {
                    false
                };
                // Kill descendants too: a shell may have spawned a command which
                // still holds the pipes after the shell itself exits.
                drop(group);
                let _ = process.kill().await;
                let _ = process.wait().await;
                // Keep the pipes' reactor alive until the stream is dropped,
                // so a short-lived command's final stdout can still be drained.
                if exited {
                    let _ = cancelled.await;
                }
                if let Some(task) = stderr_task {
                    task.abort();
                    let _ = task.await;
                }
            });
        })?;
    match startup.await {
        Ok(Ok((stdin, stdout))) => Ok(ProxyCommandStream {
            stdin,
            stdout,
            cancel: Some(cancel),
            supervisor: Some(supervisor),
        }),
        result => {
            drop(cancel);
            let _ = supervisor.join();
            match result {
                Ok(Err(error)) => Err(error),
                _ => anyhow::bail!("ProxyCommand supervisor unavailable"),
            }
        }
    }
}

#[cfg(unix)]
struct ProcessGroup(libc::pid_t);

#[cfg(unix)]
impl ProcessGroup {
    fn new(process: &tokio::process::Child) -> std::io::Result<Self> {
        process
            .id()
            .map(|id| Self(id as libc::pid_t))
            .ok_or_else(|| std::io::Error::other("ProxyCommand already exited"))
    }
}

#[cfg(unix)]
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        // The child was placed in its own group before exec; a negative pid
        // addresses only that group's processes, never NyaTerm's group.
        unsafe {
            libc::kill(-self.0, libc::SIGKILL);
        }
    }
}

#[cfg(windows)]
struct ProcessGroup(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl ProcessGroup {
    fn new(process: &tokio::process::Child) -> std::io::Result<Self> {
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject,
        };
        // This handle stays on the supervisor thread. Closing it terminates
        // the command and descendants, without affecting other transports.
        unsafe {
            let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if handle.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            let group = Self(handle);
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                std::mem::size_of_val(&info) as u32,
            ) == 0
            {
                return Err(std::io::Error::last_os_error());
            }
            let child = process
                .raw_handle()
                .ok_or_else(|| std::io::Error::other("ProxyCommand already exited"))?;
            if AssignProcessToJobObject(handle, child as _) == 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(group)
        }
    }
}

#[cfg(windows)]
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

impl AsyncRead for ProxyCommandStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        Pin::new(&mut self.stdout).poll_read(cx, buf)
    }
}

impl AsyncWrite for ProxyCommandStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        Pin::new(&mut self.stdin).poll_write(cx, buf)
    }
    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        Pin::new(&mut self.stdin).poll_flush(cx)
    }
    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        Pin::new(&mut self.stdin).poll_shutdown(cx)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::open_proxy_command_stream;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    #[tokio::test]
    async fn a_short_lived_commands_final_stdout_is_drained_before_reaping() {
        let mut stream = open_proxy_command_stream(Some("printf final-output"), "unused", 23, "")
            .await
            .unwrap();
        let mut output = Vec::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(3),
            stream.read_to_end(&mut output),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(output, b"final-output");
    }

    #[test]
    fn command_is_reaped_even_after_the_callers_runtime_has_stopped() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (stream, pid) = runtime.block_on(async {
            let mut stream = open_proxy_command_stream(
                Some("printf '%s\\n' \"$$\"; exec cat"),
                "unused",
                23,
                "",
            )
            .await
            .unwrap();
            let mut line = Vec::new();
            loop {
                let byte = stream.read_u8().await.unwrap();
                if byte == b'\n' {
                    break;
                }
                line.push(byte);
            }
            let pid: libc::pid_t = String::from_utf8(line).unwrap().parse().unwrap();
            stream.write_all(b"ping").await.unwrap();
            let mut reply = [0; 4];
            stream.read_exact(&mut reply).await.unwrap();
            assert_eq!(&reply, b"ping");
            (stream, pid)
        });
        drop(runtime);
        drop(stream);
        // The supervisor joins after wait(), so this is stronger than checking EOF.
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
    }
}
