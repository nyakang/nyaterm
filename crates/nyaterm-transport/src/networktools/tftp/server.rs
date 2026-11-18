//! Listener for one TFTP instance and the thread that accepts requests.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use nyaterm_core::TftpSettings;

use super::packet;
use super::path;
use super::session::{self, SessionContext};
use crate::networktools::event::NetworkToolEvent;
use crate::networktools::registry::{NetworkToolError, NetworkToolKind};
use crate::networktools::status::TransferSnapshot;

const ACCEPT_POLL: Duration = Duration::from_millis(200);

pub struct TftpServer {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    local_addr: SocketAddr,
    kind: NetworkToolKind,
    events: SyncSender<NetworkToolEvent>,
    stopped_announced: bool,
}

impl TftpServer {
    pub(crate) fn start(
        config: TftpSettings,
        events: SyncSender<NetworkToolEvent>,
        transfers: Arc<Mutex<Vec<TransferSnapshot>>>,
    ) -> Result<Self, NetworkToolError> {
        let kind = NetworkToolKind::Tftp;
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
        let bind_ip = local_addr.ip();
        let _ = events.try_send(NetworkToolEvent::Started {
            kind,
            addr: local_addr,
        });

        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let stop = Arc::clone(&stop);
            let events = events.clone();
            let config = config.clone();
            let transfers = Arc::clone(&transfers);
            thread::Builder::new()
                .name("nyaterm-networktools".to_string())
                .spawn(move || {
                    run_loop(socket, config, root, bind_ip, events, stop, transfers);
                })
                .map_err(NetworkToolError::Io)?
        };

        Ok(Self {
            stop,
            thread: Some(thread),
            local_addr,
            kind,
            events,
            stopped_announced: false,
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub fn stop(&mut self) {
        self.stop_inner();
    }

    fn stop_inner(&mut self) {
        if let Some(thread) = self.thread.take() {
            self.stop.store(true, Ordering::Relaxed);
            let _ = thread.join();
        }
        if !self.stopped_announced {
            self.stopped_announced = true;
            let _ = self
                .events
                .try_send(NetworkToolEvent::Stopped { kind: self.kind });
        }
    }
}

impl Drop for TftpServer {
    fn drop(&mut self) {
        self.stop_inner();
    }
}

fn run_loop(
    socket: UdpSocket,
    config: TftpSettings,
    root: PathBuf,
    bind_ip: IpAddr,
    events: SyncSender<NetworkToolEvent>,
    stop: Arc<AtomicBool>,
    transfers: Arc<Mutex<Vec<TransferSnapshot>>>,
) {
    let _ = bind_ip;
    let active = Arc::new(AtomicUsize::new(0));
    let mut next_id: u64 = 1;
    let mut buffer = vec![0u8; 2048];

    while !stop.load(Ordering::Relaxed) {
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
            Err(_) => break,
        };

        let request = match packet::parse_request(&buffer[..size]) {
            Ok(request) => request,
            Err(error) => {
                let _ = socket.send_to(
                    &packet::write_error(packet::ERR_ILLEGAL_OPERATION, &error.to_string()),
                    peer,
                );
                continue;
            }
        };

        if active.load(Ordering::Relaxed) >= usize::from(config.max_concurrency) {
            let _ = socket.send_to(
                &packet::write_error(packet::ERR_NOT_DEFINED, "server busy"),
                peer,
            );
            continue;
        }

        let id = next_id;
        next_id = next_id.wrapping_add(1);
        active.fetch_add(1, Ordering::Relaxed);
        let session_stop = Arc::clone(&stop);
        let active_after = Arc::clone(&active);
        let context = SessionContext {
            kind: NetworkToolKind::Tftp,
            root: root.clone(),
            config: config.clone(),
            bind_ip: session_bind_ip(&socket),
            events: events.clone(),
            transfers: Arc::clone(&transfers),
            stop: session_stop,
        };
        let spawned = thread::Builder::new()
            .name(format!("nyaterm-networktools-{id}"))
            .spawn(move || {
                session::run(request, peer, id, context);
                active_after.fetch_sub(1, Ordering::Relaxed);
            });
        if spawned.is_err() {
            active.fetch_sub(1, Ordering::Relaxed);
        }
    }
}

/// Sessions bind a fresh TID socket. A wildcard listener keeps the wildcard so
/// replies use the address the client reached us on; a specific listener binds
/// that same address.
fn session_bind_ip(socket: &UdpSocket) -> IpAddr {
    match socket.local_addr() {
        Ok(addr) if !addr.ip().is_unspecified() => addr.ip(),
        _ => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
    }
}
