use crate::process_tree::ExtensionChild;
use crate::tasks::{MAX_TASK_BYTES, TaskContext};
use nyaterm_core::plugins::extension::{HostRequest, TaskEvent};
use nyaterm_core::plugins::{ErrorCode, PluginError, PluginResult};
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::mpsc;

pub(crate) fn execute(request: &HostRequest, context: &TaskContext) -> PluginResult<TaskEvent> {
    let HostRequest::ProcessExecute {
        executable,
        arguments,
    } = request
    else {
        unreachable!()
    };
    let failure = || PluginError::new(ErrorCode::Io, "Extension process failed");
    context.check(request)?;
    if !std::path::Path::new(executable).is_absolute() {
        return Err(failure());
    }
    crate::package::reject_link(std::path::Path::new(executable))?;
    let mut command = Command::new(executable);
    command
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = ExtensionChild::spawn(&mut command).map_err(|_| failure())?;
    let stdout = child.child_mut().stdout.take().ok_or_else(failure)?;
    let stderr = child.child_mut().stderr.take().ok_or_else(failure)?;
    let (sender, receiver) = mpsc::sync_channel::<Vec<u8>>(16);
    let mut readers = Vec::new();
    for pipe in [Box::new(stdout) as Box<dyn Read + Send>, Box::new(stderr)] {
        let sender = sender.clone();
        readers.push(std::thread::spawn(move || {
            let mut pipe = pipe;
            let mut buffer = [0u8; 8192];
            while let Ok(count) = pipe.read(&mut buffer) {
                if count == 0 || sender.send(buffer[..count].to_vec()).is_err() {
                    break;
                }
            }
        }));
    }
    drop(sender);
    let result = (|| {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
        let mut bytes = Vec::new();
        let mut exit_code = None;
        loop {
            context.check(request)?;
            if std::time::Instant::now() >= deadline {
                return Err(PluginError::new(
                    ErrorCode::Timeout,
                    "Extension process timed out",
                ));
            }
            match receiver.recv_timeout(std::time::Duration::from_millis(25)) {
                Ok(chunk) => {
                    if bytes.len() + chunk.len() > MAX_TASK_BYTES {
                        return Err(PluginError::new(
                            ErrorCode::PackageLimit,
                            "Extension process output exceeds its limit",
                        ));
                    }
                    context.output(request, &chunk)?;
                    bytes.extend(chunk);
                }
                Err(mpsc::RecvTimeoutError::Disconnected) if exit_code.is_some() => break,
                Err(_) => {}
            }
            if exit_code.is_none()
                && let Some(status) = child.child_mut().try_wait().map_err(|_| failure())?
            {
                exit_code = Some(status.code().unwrap_or(-1));
                child.terminate();
            }
        }
        Ok(TaskEvent::Completed { bytes, exit_code })
    })();
    child.terminate();
    drop(receiver);
    for reader in readers {
        let _ = reader.join();
    }
    result
}

#[cfg(all(test, windows))]
mod tests {
    use crate::authority::Authority;
    use crate::tasks::{HostAdapter, TaskBroker, TaskContext};
    use nyaterm_core::plugins::PluginResult;
    use nyaterm_core::plugins::extension::{
        Authorization, Capability, Grant, HostRequest, Scope, TaskEvent,
    };
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use std::time::{Duration, Instant};

    struct ProcessAdapter(Arc<AtomicBool>);
    impl HostAdapter for ProcessAdapter {
        fn execute(&self, request: HostRequest, context: &TaskContext) -> PluginResult<TaskEvent> {
            let result = super::execute(&request, context);
            self.0.store(true, Ordering::Release);
            result
        }
    }

    #[test]
    fn output_arrives_before_exit_and_cancel_reaps_the_shell_and_pipe_readers() {
        let executable = std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap())
            .join("System32/WindowsPowerShell/v1.0/powershell.exe")
            .to_string_lossy()
            .into_owned();
        let authority = Authority::new(
            vec![Capability::ProcessExecute],
            Authorization {
                plugin_id: "process-test".into(),
                source: "fixture".into(),
                grants: vec![Grant {
                    capability: Capability::ProcessExecute,
                    scope: Scope::Executable {
                        path: executable.clone(),
                    },
                }],
            },
        );
        let reaped = Arc::new(AtomicBool::new(false));
        let mut broker = TaskBroker::new(authority, Arc::new(ProcessAdapter(reaped.clone())));
        let id = broker
            .submit(HostRequest::ProcessExecute {
                executable,
                arguments: vec![
                    "-NoProfile".into(),
                    "-NonInteractive".into(),
                    "-Command".into(),
                    "Write-Output ready; Start-Sleep -Seconds 30".into(),
                ],
            })
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match broker.poll(id).unwrap() {
                TaskEvent::Output { bytes } => {
                    assert!(String::from_utf8_lossy(&bytes).contains("ready"));
                    break;
                }
                TaskEvent::Pending => {}
                _ => panic!("process ended before streaming output"),
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        broker.cancel(id).unwrap();
        assert!(matches!(broker.poll(id).unwrap(), TaskEvent::Cancelled));
        while !reaped.load(Ordering::Acquire) {
            assert!(
                Instant::now() < deadline,
                "process tree or pipe readers outlived cancellation"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
