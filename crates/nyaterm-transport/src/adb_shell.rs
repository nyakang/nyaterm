//! Android-owned PTY transported through native ADB shell-v2 channels.
use std::io::Read;
use std::net::{Shutdown, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::session_event_queue::SessionEventQueue;
use crate::session_types::{SessionEvent, SessionInfo, TerminalTransport};
use nyaterm_android::adb::client::DeviceBinding;
use nyaterm_android::adb::shell::{
    CLOSE_STDIN, EXIT, MAX_SHELL_PACKET, STDERR, STDIN, STDOUT, ShellDecoder, WINDOW_SIZE,
    open_shell_with_cancel, window_size, write_packet,
};

const INPUT_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct AdbShellConfig {
    pub name: String,
    pub binding: DeviceBinding,
    pub cols: u16,
    pub rows: u16,
}

struct WriteRequest {
    id: u8,
    data: Vec<u8>,
}

pub(crate) struct AdbTerminalTransport {
    pub(crate) info: SessionInfo,
    stream: TcpStream,
    tx: mpsc::SyncSender<WriteRequest>,
    stopped: Arc<AtomicBool>,
    queued: Arc<AtomicUsize>,
    workers: Vec<JoinHandle<()>>,
}

impl AdbTerminalTransport {
    pub(crate) fn open(
        info: SessionInfo,
        binding: &DeviceBinding,
        events: SessionEventQueue,
        mut check: impl FnMut() -> nyaterm_android::error::AdbResult<()>,
    ) -> anyhow::Result<Self> {
        let mut stream = open_shell_with_cancel(binding, b"", true, &mut check)?;
        check()?;
        write_packet(
            &mut stream,
            WINDOW_SIZE,
            &window_size(info.cols, info.rows, 0, 0),
        )?;
        stream.set_read_timeout(Some(Duration::from_millis(100)))?;
        stream.set_write_timeout(Some(Duration::from_millis(500)))?;
        let mut reader = stream.try_clone()?;
        let mut writer = stream.try_clone()?;
        let stopped = Arc::new(AtomicBool::new(false));
        let queued = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = mpsc::sync_channel::<WriteRequest>(256);
        let writer_stop = Arc::clone(&stopped);
        let writer_queued = Arc::clone(&queued);
        let writer_events = events.clone();
        let writer_id = info.id.clone();
        let writer_worker = nyaterm_core::runtime::worker::spawn(
            format!("adb-shell-writer-{}", info.id),
            move || {
                while !writer_stop.load(Ordering::Acquire) {
                    let request = match rx.recv_timeout(Duration::from_millis(100)) {
                        Ok(request) => request,
                        Err(mpsc::RecvTimeoutError::Timeout) => continue,
                        Err(_) => break,
                    };
                    let result = write_packet(&mut writer, request.id, &request.data);
                    writer_queued.fetch_sub(request.data.len(), Ordering::AcqRel);
                    if result.is_err() {
                        writer_events.push(SessionEvent::Error {
                            session_id: writer_id.clone(),
                            message: "AndroidShellInputFailed".into(),
                        });
                        writer_stop.store(true, Ordering::Release);
                        let _ = writer.shutdown(Shutdown::Both);
                        break;
                    }
                }
            },
        )?;
        let reader_stop = Arc::clone(&stopped);
        let reader_id = info.id.clone();
        let reader_worker = match nyaterm_core::runtime::worker::spawn(
            format!("adb-shell-reader-{}", info.id),
            move || {
                let mut decoder = ShellDecoder::default();
                let mut bytes = [0; 8192];
                let mut reason = "AndroidDeviceDisconnected".to_owned();
                'read: while !reader_stop.load(Ordering::Acquire) {
                    match reader.read(&mut bytes) {
                        Ok(0) => {
                            if decoder.finish().is_err() {
                                reason = "AndroidShellTruncated".into();
                            }
                            break;
                        }
                        Ok(count) => match decoder.push(&bytes[..count]) {
                            Ok(packets) => {
                                for packet in packets {
                                    match packet.id {
                                        STDOUT | STDERR => events.push(SessionEvent::Output {
                                            session_id: reader_id.clone(),
                                            data: packet.data,
                                        }),
                                        EXIT => {
                                            reason = format!("AndroidShellExit:{}", packet.data[0]);
                                            break 'read;
                                        }
                                        _ => {
                                            reason = "AndroidShellProtocolFailed".into();
                                            break 'read;
                                        }
                                    }
                                }
                            }
                            Err(_) => {
                                reason = "AndroidShellProtocolFailed".into();
                                break;
                            }
                        },
                        Err(error)
                            if matches!(
                                error.kind(),
                                std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                            ) => {}
                        Err(_) => break,
                    }
                }
                reader_stop.store(true, Ordering::Release);
                let _ = reader.shutdown(Shutdown::Both);
                events.push(SessionEvent::Exited {
                    session_id: reader_id,
                    reason,
                });
            },
        ) {
            Ok(worker) => worker,
            Err(error) => {
                stopped.store(true, Ordering::Release);
                let _ = stream.shutdown(Shutdown::Both);
                let _ = writer_worker.join();
                return Err(error.into());
            }
        };
        Ok(Self {
            info,
            stream,
            tx,
            stopped,
            queued,
            workers: vec![writer_worker, reader_worker],
        })
    }

    pub(crate) fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
    }

    fn enqueue(&self, id: u8, data: &[u8]) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.stopped.load(Ordering::Acquire),
            "Android shell is closed"
        );
        anyhow::ensure!(
            data.len() <= MAX_SHELL_PACKET,
            "Android shell input packet is too large"
        );
        self.queued
            .try_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                (used + data.len() <= INPUT_BYTES).then_some(used + data.len())
            })
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::WouldBlock))?;
        if let Err(error) = self.tx.try_send(WriteRequest {
            id,
            data: data.to_vec(),
        }) {
            self.queued.fetch_sub(data.len(), Ordering::AcqRel);
            return Err(std::io::Error::from(match error {
                mpsc::TrySendError::Full(_) => std::io::ErrorKind::WouldBlock,
                mpsc::TrySendError::Disconnected(_) => std::io::ErrorKind::BrokenPipe,
            })
            .into());
        }
        Ok(())
    }
}

impl TerminalTransport for AdbTerminalTransport {
    fn write(&mut self, data: &[u8]) -> anyhow::Result<()> {
        self.enqueue(STDIN, data)
    }
    fn resize(&mut self, cols: u16, rows: u16, width: u16, height: u16) -> anyhow::Result<()> {
        self.enqueue(WINDOW_SIZE, &window_size(cols, rows, width, height))?;
        self.info.cols = cols;
        self.info.rows = rows;
        Ok(())
    }
    fn close(&mut self) -> anyhow::Result<()> {
        let _ = self.enqueue(CLOSE_STDIN, b"");
        self.stop();
        let _ = self.stream.shutdown(Shutdown::Both);
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
        Ok(())
    }
}

impl Drop for AdbTerminalTransport {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

#[cfg(test)]
mod tests {
    use crate::adb_shell::AdbTerminalTransport;
    use crate::session_event_queue::SessionEventQueue;
    use crate::session_types::{SessionEvent, SessionInfo, SessionKind, TerminalTransport};
    use nyaterm_android::adb::client::DeviceBinding;
    use nyaterm_android::adb::shell::{
        EXIT, STDERR, STDIN, STDOUT, ShellDecoder, WINDOW_SIZE, write_packet,
    };
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::time::{Duration, Instant};
    fn request(socket: &mut TcpStream) -> Vec<u8> {
        let mut header = [0; 4];
        socket.read_exact(&mut header).unwrap();
        let length = usize::from_str_radix(std::str::from_utf8(&header).unwrap(), 16).unwrap();
        let mut bytes = vec![0; length];
        socket.read_exact(&mut bytes).unwrap();
        socket.write_all(b"OKAY").unwrap();
        bytes
    }
    #[test]
    fn cancelled_adb_handshake_does_not_register_or_leak_a_session() {
        use crate::SessionManager;
        use crate::adb_shell::AdbShellConfig;
        use nyaterm_android::error::AdbError;
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let server = listener.local_addr().unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancel = Arc::clone(&cancelled);
        let worker = std::thread::spawn(move || {
            let (mut peer, _) = listener.accept().unwrap();
            peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let mut prefix = [0; 4];
            peer.read_exact(&mut prefix).unwrap();
            cancel.store(true, Ordering::Release);
            let mut bytes = [0; 128];
            while peer.read(&mut bytes).unwrap() != 0 {}
        });
        let manager = SessionManager::new();
        let started = Instant::now();
        let result = manager.create_adb_session_with_cancel(
            AdbShellConfig {
                name: "fixture".into(),
                binding: DeviceBinding {
                    server,
                    serial: "fixture".into(),
                    transport_id: None,
                    generation: 1,
                },
                cols: 80,
                rows: 24,
            },
            || {
                if cancelled.load(Ordering::Acquire) {
                    Err(AdbError::Cancelled)
                } else {
                    Ok(())
                }
            },
        );
        assert!(result.is_err());
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(manager.list_sessions().unwrap().is_empty());
        worker.join().unwrap();
    }
    #[test]
    fn native_pty_routes_input_resize_utf8_stderr_and_exit_without_local_scripts() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let server = listener.local_addr().unwrap();
        let worker = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            assert_eq!(request(&mut socket), b"host:tport:serial:fixture");
            socket.write_all(&7u64.to_le_bytes()).unwrap();
            assert_eq!(request(&mut socket), b"shell,v2,pty,TERM=xterm-256color:");
            let mut decoder = ShellDecoder::default();
            let mut packets = Vec::new();
            while packets.len() < 3 {
                let mut bytes = [0; 4096];
                let n = socket.read(&mut bytes).unwrap();
                assert!(n > 0);
                packets.extend(decoder.push(&bytes[..n]).unwrap());
            }
            assert_eq!(packets[0].id, WINDOW_SIZE);
            assert_eq!(packets[0].data, b"24x80,0x0\0");
            assert_eq!(packets[1].id, STDIN);
            assert_eq!(packets[1].data, b"test\x03");
            assert_eq!(packets[2].data, b"40x120,600x400\0");
            let mut output = Vec::new();
            write_packet(&mut output, STDOUT, "中文".as_bytes()).unwrap();
            write_packet(&mut output, STDERR, b"stderr").unwrap();
            write_packet(&mut output, EXIT, &[7]).unwrap();
            for chunk in output.chunks(2) {
                socket.write_all(chunk).unwrap();
            }
        });
        let events = SessionEventQueue::new();
        let info = SessionInfo {
            id: "fixture".into(),
            name: "Fixture".into(),
            kind: SessionKind::AdbShell,
            working_dir: None,
            cols: 80,
            rows: 24,
        };
        let binding = DeviceBinding {
            server,
            serial: "fixture".into(),
            transport_id: Some(7),
            generation: 1,
        };
        let mut transport =
            AdbTerminalTransport::open(info, &binding, events.clone(), || Ok(())).unwrap();
        transport.write(b"test\x03").unwrap();
        transport.resize(120, 40, 600, 400).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut received = Vec::new();
        let mut exited = false;
        while Instant::now() < deadline && !exited {
            for event in events.drain(100).events {
                match event {
                    SessionEvent::Output { data, .. } => received.extend(data),
                    SessionEvent::Exited { reason, .. } => {
                        assert_eq!(reason, "AndroidShellExit:7");
                        exited = true;
                    }
                    _ => {}
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(exited);
        assert_eq!(received, "中文stderr".as_bytes());
        transport.close().unwrap();
        worker.join().unwrap();
        assert!(transport.workers.is_empty());
        assert!(transport.write(b"after-close").is_err());
    }
}
