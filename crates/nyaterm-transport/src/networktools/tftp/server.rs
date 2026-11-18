//! Listener and owned transfer threads for one TFTP service generation.

use std::net::{SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use nyaterm_core::models::networktools::TftpSettings;

use super::session::{self, SessionContext};
use super::{packet, path};
use crate::networktools::event::NetworkToolEvent;
use crate::networktools::registry::{NetworkToolError, NetworkToolKind};
use crate::networktools::status::TransferSnapshot;

const ACCEPT_POLL: Duration = Duration::from_millis(200);
static NEXT_TRANSFER_ID: AtomicU64 = AtomicU64::new(1);

pub struct TftpServer {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    local_addr: SocketAddr,
    generation: u64,
    events: SyncSender<NetworkToolEvent>,
    stopped_announced: bool,
}

impl TftpServer {
    pub(crate) fn start(
        config: TftpSettings,
        events: SyncSender<NetworkToolEvent>,
        transfers: Arc<Mutex<Vec<TransferSnapshot>>>,
        generation: u64,
    ) -> Result<Self, NetworkToolError> {
        let ip = config
            .parsed_bind_address()
            .map_err(|error| NetworkToolError::InvalidSettings(error.to_string()))?;
        if config.root_dir.trim().is_empty() {
            return Err(NetworkToolError::RootUnavailable(
                "root directory is not configured".to_string(),
            ));
        }
        let root = path::canonical_root(&PathBuf::from(&config.root_dir))
            .map_err(|error| NetworkToolError::RootUnavailable(error.to_string()))?;
        let socket = UdpSocket::bind(SocketAddr::new(ip, config.port))
            .map_err(|error| NetworkToolError::Bind(error.to_string()))?;
        socket
            .set_read_timeout(Some(ACCEPT_POLL))
            .map_err(NetworkToolError::Io)?;
        let local_addr = socket.local_addr().map_err(NetworkToolError::Io)?;
        let stop = Arc::new(AtomicBool::new(false));
        let context = SessionContext {
            kind: NetworkToolKind::Tftp,
            generation,
            root,
            config,
            bind_ip: local_addr.ip(),
            events: events.clone(),
            transfers,
            stop: Arc::clone(&stop),
        };
        let thread = thread::Builder::new()
            .name("nyaterm-networktools".to_string())
            .spawn(move || run_loop(socket, context))
            .map_err(NetworkToolError::Io)?;
        Ok(Self {
            stop,
            thread: Some(thread),
            local_addr,
            generation,
            events,
            stopped_announced: false,
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub(crate) fn is_running(&self) -> bool {
        !self.stop.load(Ordering::Relaxed)
            && self
                .thread
                .as_ref()
                .is_some_and(|thread| !thread.is_finished())
    }

    pub fn stop(&mut self) {
        self.stop_inner();
    }

    fn stop_inner(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        if !self.stopped_announced {
            self.stopped_announced = true;
            let _ = self.events.send(NetworkToolEvent::Stopped {
                kind: NetworkToolKind::Tftp,
                generation: self.generation,
            });
        }
    }
}

impl Drop for TftpServer {
    fn drop(&mut self) {
        self.stop_inner();
    }
}

fn run_loop(socket: UdpSocket, context: SessionContext) {
    if context
        .events
        .send(NetworkToolEvent::Started {
            kind: context.kind,
            generation: context.generation,
            addr: socket.local_addr().expect("bound UDP socket"),
        })
        .is_err()
    {
        return;
    }
    let mut sessions: Vec<JoinHandle<()>> = Vec::new();
    let mut buffer = [0u8; 65536];
    let mut failure = None;
    while !context.stop.load(Ordering::Relaxed) {
        // Reap completed sessions even while the listener is idle. Keeping all
        // handles makes Stopped a barrier after every transfer has terminated.
        let mut index = 0;
        while index < sessions.len() {
            if sessions[index].is_finished() {
                let _ = sessions.swap_remove(index).join();
            } else {
                index += 1;
            }
        }
        let (size, peer) = match socket.recv_from(&mut buffer) {
            Ok(received) => received,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                continue;
            }
            Err(error) => {
                failure = Some(error.to_string());
                break;
            }
        };
        if context.stop.load(Ordering::Relaxed) {
            break;
        }
        let request = match packet::parse_request(&buffer[..size]) {
            Ok(request) => request,
            Err(_) => {
                let _ = socket.send_to(
                    &packet::write_error(packet::ERR_ILLEGAL_OPERATION, "invalid request"),
                    peer,
                );
                continue;
            }
        };
        if sessions.len() >= usize::from(context.config.max_concurrency) {
            let _ = socket.send_to(
                &packet::write_error(packet::ERR_NOT_DEFINED, "server busy"),
                peer,
            );
            continue;
        }
        let id = NEXT_TRANSFER_ID.fetch_add(1, Ordering::Relaxed);
        let session_context = SessionContext {
            kind: context.kind,
            generation: context.generation,
            root: context.root.clone(),
            config: context.config.clone(),
            bind_ip: context.bind_ip,
            events: context.events.clone(),
            transfers: Arc::clone(&context.transfers),
            stop: Arc::clone(&context.stop),
        };
        match thread::Builder::new()
            .name(format!("nyaterm-networktools-{id}"))
            .spawn(move || session::run(request, peer, id, session_context))
        {
            Ok(thread) => sessions.push(thread),
            Err(_) => {
                let _ = socket.send_to(
                    &packet::write_error(packet::ERR_NOT_DEFINED, "cannot start transfer"),
                    peer,
                );
            }
        }
    }
    context.stop.store(true, Ordering::Relaxed);
    for session in sessions {
        let _ = session.join();
    }
    if let Some(message) = failure {
        let _ = context.events.send(NetworkToolEvent::Failed {
            kind: context.kind,
            generation: context.generation,
            message,
        });
    }
}
