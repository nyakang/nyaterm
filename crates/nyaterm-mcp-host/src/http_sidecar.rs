//! Owned HTTP forwarder process. All child I/O and reaping run off the UI thread.
use futures::channel::{mpsc, oneshot};
use nyaterm_core::SecretString;
use nyaterm_mcp_protocol::http_control::{
    CONTROL_VERSION, HttpBootstrap, HttpControlEvent, MAX_CONTROL_BYTES,
};
use std::{path::PathBuf, process::Stdio, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SidecarFailure {
    PortInUse,
    Unavailable,
}
pub enum SidecarEvent {
    Ready { port: u16, token: SecretString },
    Failed(SidecarFailure),
    Stopped,
}
pub struct HttpSidecar {
    stop: CancellationToken,
    finished: Option<oneshot::Receiver<()>>,
}
impl HttpSidecar {
    pub fn spawn(
        helper: PathBuf,
        bootstrap: HttpBootstrap,
        previous: Option<oneshot::Receiver<()>>,
    ) -> std::io::Result<(Self, mpsc::UnboundedReceiver<SidecarEvent>)> {
        let stop = CancellationToken::new();
        let worker_stop = stop.clone();
        let (events, receiver) = mpsc::unbounded();
        let (done, finished) = oneshot::channel();
        std::thread::Builder::new()
            .name("nyaterm-mcp-http".into())
            .spawn(move || {
                if let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    runtime.block_on(async {
                        // A replacement never races the previous process for the same port.
                        if let Some(previous) = previous {
                            let _ = previous.await;
                        }
                        if !worker_stop.is_cancelled()
                            && supervise(helper, bootstrap, worker_stop.clone(), &events)
                                .await
                                .is_err()
                            && !worker_stop.is_cancelled()
                        {
                            let _ = events
                                .unbounded_send(SidecarEvent::Failed(SidecarFailure::Unavailable));
                        }
                        let _ = events.unbounded_send(SidecarEvent::Stopped);
                    });
                } else {
                    let _ =
                        events.unbounded_send(SidecarEvent::Failed(SidecarFailure::Unavailable));
                }
                let _ = done.send(());
            })?;
        Ok((
            Self {
                stop,
                finished: Some(finished),
            },
            receiver,
        ))
    }
    pub fn shutdown(mut self) -> Option<oneshot::Receiver<()>> {
        self.stop.cancel();
        self.finished.take()
    }
}
impl Drop for HttpSidecar {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

async fn supervise(
    helper: PathBuf,
    bootstrap: HttpBootstrap,
    stop: CancellationToken,
    events: &mpsc::UnboundedSender<SidecarEvent>,
) -> Result<(), ()> {
    let expected_port = bootstrap.port;
    let mut command = tokio::process::Command::new(helper);
    command
        .args(["--transport", "streamable-http", "--managed"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    let mut child = command.spawn().map_err(|_| ())?;
    let result = async {
        let mut input = child.stdin.take().ok_or(())?;
        let output = child.stdout.take().ok_or(())?;
        let mut bytes = serde_json::to_vec(&bootstrap).map_err(|_| ())?;
        bytes.push(b'\n');
        if bytes.len() > MAX_CONTROL_BYTES {
            return Err(());
        }
        let ready = async {
            input.write_all(&bytes).await.map_err(|_| ())?;
            input.flush().await.map_err(|_| ())?;
            let mut output = BufReader::new(output).take((MAX_CONTROL_BYTES + 1) as u64);
            let mut line = Vec::new();
            output.read_until(b'\n', &mut line).await.map_err(|_| ())?;
            if line.len() > MAX_CONTROL_BYTES || line.last() != Some(&b'\n') {
                return Err(());
            }
            let event: HttpControlEvent = serde_json::from_slice(&line).map_err(|_| ())?;
            match event {
                HttpControlEvent::Ready {
                    version,
                    port,
                    token,
                } if version == CONTROL_VERSION
                    && port == expected_port
                    && token.len() == 64
                    && token.bytes().all(|b| b.is_ascii_hexdigit()) =>
                {
                    events
                        .unbounded_send(SidecarEvent::Ready {
                            port,
                            token: SecretString::new(token),
                        })
                        .map_err(|_| ())?;
                    Ok(())
                }
                HttpControlEvent::Failed { version, code } if version == CONTROL_VERSION => {
                    let failure = if code == "port_in_use" {
                        SidecarFailure::PortInUse
                    } else {
                        SidecarFailure::Unavailable
                    };
                    let _ = events.unbounded_send(SidecarEvent::Failed(failure));
                    Err(())
                }
                _ => Err(()),
            }
        };
        tokio::select! {
            _=stop.cancelled()=>return Ok(()),
            result=tokio::time::timeout(Duration::from_secs(10),ready)=>result.map_err(|_|())??,
        }
        tokio::select! {
            _=stop.cancelled()=>{drop(input); Ok(())},
            _=child.wait()=>Err(()),
        }
    }
    .await;
    // EOF requests graceful shutdown; kill and reap if the helper is hung.
    if tokio::time::timeout(Duration::from_secs(2), child.wait())
        .await
        .is_err()
    {
        let _ = child.kill().await;
        let _ = child.wait().await;
    }
    result
}

#[cfg(all(test, unix))]
mod tests {
    use super::{HttpSidecar, SidecarEvent, SidecarFailure};
    use futures::StreamExt;
    use nyaterm_mcp_protocol::http_control::{CONTROL_VERSION, HttpBootstrap};
    use std::{os::unix::fs::PermissionsExt, time::Duration};
    fn bootstrap() -> HttpBootstrap {
        HttpBootstrap {
            version: CONTROL_VERSION,
            port: 8002,
            allowed_origins: vec![],
            bridge_port: 1,
            bridge_token: "fixture".into(),
            bridge_generation: "fixture".into(),
        }
    }
    #[tokio::test]
    async fn supervisor_reaps_hung_child_and_rejects_bad_ready() {
        for valid in [true, false] {
            let temp = tempfile::tempdir().unwrap();
            let helper = temp.path().join("helper");
            let port = if valid { 8002 } else { 8003 };
            std::fs::write(&helper,format!("#!/bin/sh\nread -r bootstrap\nprintf '%s\\n' '{{\"type\":\"ready\",\"version\":1,\"port\":{port},\"token\":\"{}\"}}'\nexec sleep 60\n","a".repeat(64))).unwrap();
            std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o700)).unwrap();
            let (runtime, mut events) = HttpSidecar::spawn(helper, bootstrap(), None).unwrap();
            let event = tokio::time::timeout(Duration::from_secs(5), events.next())
                .await
                .unwrap()
                .unwrap();
            assert!(matches!(
                (valid, event),
                (true, SidecarEvent::Ready { .. })
                    | (false, SidecarEvent::Failed(SidecarFailure::Unavailable))
            ));
            tokio::time::timeout(Duration::from_secs(5), runtime.shutdown().unwrap())
                .await
                .unwrap()
                .unwrap();
        }
    }
    #[tokio::test]
    async fn supervisor_reports_missing_executable_without_exposing_paths() {
        let (runtime, mut events) =
            HttpSidecar::spawn("/nonexistent/private-location".into(), bootstrap(), None).unwrap();
        assert!(matches!(
            events.next().await,
            Some(SidecarEvent::Failed(SidecarFailure::Unavailable))
        ));
        runtime.shutdown().unwrap().await.unwrap();
    }
}
