//! One TFTP transfer session, served on its own ephemeral UDP socket (the TID).

use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, Read, Write};
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nyaterm_core::models::networktools::TftpSettings;

use super::packet::{self, Mode, Request};
use super::path::{self, TftpPathError};
use crate::networktools::event::{NetworkToolEvent, TransferDirection, TransferOutcome};
use crate::networktools::registry::NetworkToolKind;
use crate::networktools::status::TransferSnapshot;

const RETRIES: usize = 5;

pub(super) struct SessionContext {
    pub kind: NetworkToolKind,
    pub generation: u64,
    pub root: PathBuf,
    pub config: TftpSettings,
    pub bind_ip: IpAddr,
    pub events: SyncSender<NetworkToolEvent>,
    pub transfers: Arc<Mutex<Vec<TransferSnapshot>>>,
    pub stop: Arc<AtomicBool>,
}

pub(super) fn run(request: Request, peer: SocketAddr, id: u64, ctx: SessionContext) {
    let direction = if request.write {
        TransferDirection::Upload
    } else {
        TransferDirection::Download
    };
    let total = if request.mode != Mode::Octet {
        None
    } else if request.write {
        request
            .options
            .iter()
            .find(|(name, _)| name == "tsize")
            .and_then(|(_, value)| value.parse::<u64>().ok())
    } else {
        path::read_path(&ctx.root, &request.file)
            .ok()
            .and_then(|path| std::fs::metadata(path).ok())
            .map(|metadata| metadata.len())
    };
    register_transfer(&ctx, id, direction, &request.file, total, peer);
    emit(
        &ctx,
        NetworkToolEvent::TransferStarted {
            kind: ctx.kind,
            generation: ctx.generation,
            id,
            direction,
            file: request.file.clone(),
            total,
            remote: peer.to_string(),
        },
    );

    let outcome = match UdpSocket::bind(SocketAddr::new(ctx.bind_ip, 0)) {
        Err(error) => failed(format!("session socket: {error}")),
        Ok(socket) if request.mode == Mode::Mail => {
            let _ = socket.send_to(
                &packet::write_error(packet::ERR_ILLEGAL_OPERATION, "mail mode is not supported"),
                peer,
            );
            failed("mail mode is not supported")
        }
        Ok(socket) if request.write => receive_file(&socket, peer, &request, &ctx, id),
        Ok(socket) => send_file(&socket, peer, &request, &ctx, id),
    };
    finish_transfer(&ctx, id, direction, &request.file, peer, outcome);
}

fn send_file(
    socket: &UdpSocket,
    peer: SocketAddr,
    request: &Request,
    ctx: &SessionContext,
    id: u64,
) -> TransferOutcome {
    let path = match path::read_path(&ctx.root, &request.file) {
        Ok(path) => path,
        Err(error) => {
            send_path_error(socket, peer, &error);
            return failed(error.to_string());
        }
    };
    let file = match File::open(&path) {
        Ok(file) => file,
        Err(error) => {
            let message = error.to_string();
            send_path_error(socket, peer, &TftpPathError::from(error));
            return failed(message);
        }
    };
    let size = file.metadata().ok().map(|metadata| metadata.len());
    let negotiated = packet::negotiate(request, size, &ctx.config);
    let block_size = usize::from(negotiated.block_size);
    let timeout = Duration::from_secs(u64::from(negotiated.timeout_secs));
    if negotiated.has_options()
        && let Err(error) = send_until_ack(
            socket,
            peer,
            &packet::write_oack(&negotiated.accepted),
            0,
            timeout,
            &ctx.stop,
        )
    {
        return failed(error.to_string());
    }
    let mut source = Source::new(file, request.mode);
    let mut block: u16 = 1;
    let mut done = 0u64;
    loop {
        if ctx.stop.load(Ordering::Relaxed) {
            return failed("service stopped");
        }
        let chunk = match source.next_block(block_size) {
            Ok(chunk) => chunk,
            Err(error) => {
                let _ = socket.send_to(
                    &packet::write_error(packet::ERR_NOT_DEFINED, "file read failed"),
                    peer,
                );
                return failed(error.to_string());
            }
        };
        if let Err(error) = send_until_ack(
            socket,
            peer,
            &packet::write_data(block, &chunk),
            block,
            timeout,
            &ctx.stop,
        ) {
            return failed(error.to_string());
        }
        done += chunk.len() as u64;
        update_progress(ctx, id, done);
        if chunk.len() < block_size {
            return TransferOutcome::Completed { bytes: done };
        }
        block = block.wrapping_add(1);
    }
}

fn receive_file(
    socket: &UdpSocket,
    peer: SocketAddr,
    request: &Request,
    ctx: &SessionContext,
    id: u64,
) -> TransferOutcome {
    if !ctx.config.allow_upload {
        let _ = socket.send_to(
            &packet::write_error(packet::ERR_ACCESS_VIOLATION, "upload is disabled"),
            peer,
        );
        return failed("upload is disabled");
    }
    let mut target = match path::write_target(&ctx.root, &request.file, ctx.config.allow_overwrite)
    {
        Ok(target) => target,
        Err(error) => {
            send_path_error(socket, peer, &error);
            return failed(error.to_string());
        }
    };
    let negotiated = packet::negotiate(request, None, &ctx.config);
    let block_size = usize::from(negotiated.block_size);
    let timeout = Duration::from_secs(u64::from(negotiated.timeout_secs));
    let mut response = if negotiated.has_options() {
        packet::write_oack(&negotiated.accepted)
    } else {
        packet::write_ack(0).to_vec()
    };
    if let Err(error) = socket.send_to(&response, peer) {
        return failed(error.to_string());
    }
    let mut expected: u16 = 1;
    let mut done = 0u64;
    let mut retries = 0usize;
    let mut deadline = Instant::now() + timeout;
    let mut netascii = NetAsciiDecoder::default();
    // One full UDP datagram, including packets larger than the negotiated size.
    let mut buffer = vec![0u8; 65536];
    loop {
        match recv_data(socket, peer, deadline, &ctx.stop, &mut buffer) {
            Ok(Received::Data(block, payload)) => {
                if payload.len() > block_size {
                    let _ = socket.send_to(
                        &packet::write_error(
                            packet::ERR_ILLEGAL_OPERATION,
                            "DATA exceeds negotiated block size",
                        ),
                        peer,
                    );
                    return failed("DATA exceeds negotiated block size");
                }
                if block == expected {
                    let received_len = payload.len();
                    let final_block = received_len < block_size;
                    let bytes = if request.mode == Mode::NetAscii {
                        netascii.push(payload, final_block)
                    } else {
                        payload
                    };
                    if let Err(error) = target.temp.write_all(&bytes) {
                        let _ = socket.send_to(
                            &packet::write_error(packet::ERR_DISK_FULL, "file write failed"),
                            peer,
                        );
                        return failed(error.to_string());
                    }
                    done += received_len as u64;
                    update_progress(ctx, id, done);
                    response = packet::write_ack(block).to_vec();
                    if final_block {
                        if let Err(error) = target.temp.flush() {
                            let _ = socket.send_to(
                                &packet::write_error(packet::ERR_DISK_FULL, "file flush failed"),
                                peer,
                            );
                            return failed(error.to_string());
                        }
                        if ctx.stop.load(Ordering::Relaxed) {
                            return failed("service stopped");
                        }
                        if let Err(error) = target.commit() {
                            let error = TftpPathError::from(error);
                            send_path_error(socket, peer, &error);
                            return failed(error.to_string());
                        }
                        // The upload is committed even if the ACK is lost. Keep
                        // the TID alive briefly to acknowledge retransmissions.
                        let _ = socket.send_to(&response, peer);
                        dally(socket, peer, block, timeout, &ctx.stop, &mut buffer);
                        return TransferOutcome::Completed { bytes: done };
                    }
                    if let Err(error) = socket.send_to(&response, peer) {
                        return failed(error.to_string());
                    }
                    expected = expected.wrapping_add(1);
                    retries = 0;
                    deadline = Instant::now() + timeout;
                } else if block == expected.wrapping_sub(1) {
                    let _ = socket.send_to(&response, peer);
                }
            }
            Ok(Received::Ignored) => {}
            Ok(Received::ClientError(message)) => return failed(message),
            Ok(Received::Invalid) => {
                let _ = socket.send_to(
                    &packet::write_error(packet::ERR_ILLEGAL_OPERATION, "invalid data packet"),
                    peer,
                );
                return failed("invalid data packet");
            }
            Err(error) if error.kind() == io::ErrorKind::TimedOut => {
                retries += 1;
                if retries >= RETRIES {
                    return failed("transfer timed out");
                }
                if let Err(error) = socket.send_to(&response, peer) {
                    return failed(error.to_string());
                }
                deadline = Instant::now() + timeout;
            }
            Err(error) => return failed(error.to_string()),
        }
    }
}

fn dally(
    socket: &UdpSocket,
    peer: SocketAddr,
    final_block: u16,
    timeout: Duration,
    stop: &AtomicBool,
    buffer: &mut [u8],
) {
    let deadline = Instant::now() + timeout;
    while let Ok(received) = recv_data(socket, peer, deadline, stop, buffer) {
        if let Received::Data(block, _) = received
            && block == final_block
        {
            let _ = socket.send_to(&packet::write_ack(block), peer);
        }
    }
}

fn send_until_ack(
    socket: &UdpSocket,
    peer: SocketAddr,
    response: &[u8],
    expected: u16,
    timeout: Duration,
    stop: &AtomicBool,
) -> io::Result<()> {
    let mut buffer = [0u8; 65536];
    for _ in 0..RETRIES {
        if stop.load(Ordering::Relaxed) {
            return Err(io::Error::other("service stopped"));
        }
        socket.send_to(response, peer)?;
        let deadline = Instant::now() + timeout;
        loop {
            match recv_packet(socket, peer, deadline, stop, &mut buffer) {
                Ok(Some(size)) => {
                    if packet::parse_ack(&buffer[..size]) == Ok(expected) {
                        return Ok(());
                    }
                    if let Ok((code, message)) = packet::parse_error(&buffer[..size]) {
                        return Err(io::Error::other(tftp_error_reason(code, &message)));
                    }
                }
                Ok(None) => {}
                Err(error) if error.kind() == io::ErrorKind::TimedOut => break,
                Err(error) => return Err(error),
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        "acknowledgement timed out",
    ))
}

/// Poll cancellation independently from the negotiated protocol deadline. An
/// unrelated datagram never resets that deadline or consumes a retry.
fn recv_packet(
    socket: &UdpSocket,
    peer: SocketAddr,
    deadline: Instant,
    stop: &AtomicBool,
    buffer: &mut [u8],
) -> io::Result<Option<usize>> {
    loop {
        if stop.load(Ordering::Relaxed) {
            return Err(io::Error::other("service stopped"));
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::ErrorKind::TimedOut.into());
        }
        socket.set_read_timeout(Some(remaining.min(Duration::from_millis(200))))?;
        match socket.recv_from(buffer) {
            Ok((size, from)) if from == peer => return Ok(Some(size)),
            Ok((_, from)) => {
                let _ = socket.send_to(
                    &packet::write_error(packet::ERR_UNKNOWN_TID, "unknown transfer ID"),
                    from,
                );
                return Ok(None);
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) => {}
            Err(error) => return Err(error),
        }
    }
}

enum Received {
    Data(u16, Vec<u8>),
    Ignored,
    ClientError(String),
    Invalid,
}

fn recv_data(
    socket: &UdpSocket,
    peer: SocketAddr,
    deadline: Instant,
    stop: &AtomicBool,
    buffer: &mut [u8],
) -> io::Result<Received> {
    let Some(size) = recv_packet(socket, peer, deadline, stop, buffer)? else {
        return Ok(Received::Ignored);
    };
    Ok(match packet::parse_data(&buffer[..size]) {
        Ok((block, payload)) => Received::Data(block, payload.to_vec()),
        Err(_) => match packet::parse_error(&buffer[..size]) {
            Ok((code, message)) => Received::ClientError(tftp_error_reason(code, &message)),
            Err(_) => Received::Invalid,
        },
    })
}

/// TFTP ERROR packets are allowed to carry an empty text field; some clients
/// send only a code when they abort. Never hand an empty reason to the UI or
/// log: fall back to the code's meaning.
fn tftp_error_reason(code: u16, message: &str) -> String {
    let message = message.trim();
    if !message.is_empty() {
        return message.to_string();
    }
    match code {
        0 => "client aborted the transfer".to_string(),
        packet::ERR_FILE_NOT_FOUND => "file not found".to_string(),
        packet::ERR_ACCESS_VIOLATION => "access denied".to_string(),
        packet::ERR_DISK_FULL => "disk full or allocation exceeded".to_string(),
        packet::ERR_ILLEGAL_OPERATION => "illegal TFTP operation".to_string(),
        packet::ERR_UNKNOWN_TID => "unknown transfer ID".to_string(),
        packet::ERR_FILE_EXISTS => "file already exists".to_string(),
        7 => "no such user".to_string(),
        8 => "option negotiation failed".to_string(),
        _ => format!("TFTP error {code}"),
    }
}

fn send_path_error(socket: &UdpSocket, peer: SocketAddr, error: &TftpPathError) {
    let (code, message) = match error {
        TftpPathError::NotFound => (packet::ERR_FILE_NOT_FOUND, "file not found"),
        TftpPathError::AlreadyExists => (packet::ERR_FILE_EXISTS, "file already exists"),
        TftpPathError::IsDirectory => (packet::ERR_ACCESS_VIOLATION, "is a directory"),
        TftpPathError::InvalidName | TftpPathError::OutsideRoot => {
            (packet::ERR_ACCESS_VIOLATION, "access violation")
        }
        TftpPathError::Io(_) => (packet::ERR_NOT_DEFINED, "local error"),
    };
    let _ = socket.send_to(&packet::write_error(code, message), peer);
}

fn register_transfer(
    ctx: &SessionContext,
    id: u64,
    direction: TransferDirection,
    file: &str,
    total: Option<u64>,
    peer: SocketAddr,
) {
    let mut transfers = ctx
        .transfers
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    transfers.push(TransferSnapshot::new(
        ctx.generation,
        id,
        direction,
        file.to_string(),
        peer.to_string(),
        total,
    ));
}

fn update_progress(ctx: &SessionContext, id: u64, done: u64) {
    let mut transfers = ctx
        .transfers
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(snapshot) = transfers.iter_mut().find(|snapshot| snapshot.id == id) {
        snapshot.advance(done);
    }
}

fn finish_transfer(
    ctx: &SessionContext,
    id: u64,
    direction: TransferDirection,
    file: &str,
    peer: SocketAddr,
    outcome: TransferOutcome,
) {
    {
        let mut transfers = ctx
            .transfers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        transfers.retain(|snapshot| snapshot.id != id);
    }
    emit(
        ctx,
        NetworkToolEvent::TransferFinished {
            kind: ctx.kind,
            generation: ctx.generation,
            id,
            direction,
            file: file.to_string(),
            remote: peer.to_string(),
            outcome,
        },
    );
}

fn emit(ctx: &SessionContext, event: NetworkToolEvent) {
    let _ = ctx.events.send(event);
}

fn failed(message: impl Into<String>) -> TransferOutcome {
    TransferOutcome::Failed {
        message: message.into(),
    }
}

struct Source {
    file: File,
    mode: Mode,
    pending: VecDeque<u8>,
    eof: bool,
}

impl Source {
    fn new(file: File, mode: Mode) -> Self {
        Self {
            file,
            mode,
            pending: VecDeque::new(),
            eof: false,
        }
    }

    fn next_block(&mut self, block_size: usize) -> io::Result<Vec<u8>> {
        let mut out = Vec::with_capacity(block_size);
        while out.len() < block_size {
            if let Some(byte) = self.pending.pop_front() {
                out.push(byte);
                continue;
            }
            if self.eof {
                break;
            }
            let mut raw = vec![0u8; block_size.max(4096)];
            let read = self.file.read(&mut raw)?;
            if read == 0 {
                self.eof = true;
                break;
            }
            raw.truncate(read);
            match self.mode {
                Mode::Octet => self.pending.extend(raw),
                Mode::NetAscii => {
                    for byte in raw {
                        match byte {
                            b'\n' => {
                                self.pending.push_back(b'\r');
                                self.pending.push_back(b'\n');
                            }
                            b'\r' => {
                                self.pending.push_back(b'\r');
                                self.pending.push_back(0);
                            }
                            other => self.pending.push_back(other),
                        }
                    }
                }
                Mode::Mail => {}
            }
        }
        Ok(out)
    }
}

/// Streaming netascii decoder for uploads: `CRLF` becomes `LF`, `CR NUL`
/// becomes `CR`, and a trailing `CR` is carried across block boundaries.
#[derive(Default)]
struct NetAsciiDecoder {
    pending_cr: bool,
}

impl NetAsciiDecoder {
    fn push(&mut self, chunk: Vec<u8>, final_block: bool) -> Vec<u8> {
        let mut out = Vec::with_capacity(chunk.len());
        for byte in chunk {
            if self.pending_cr {
                self.pending_cr = false;
                match byte {
                    b'\n' => out.push(b'\n'),
                    0 => out.push(b'\r'),
                    other => {
                        out.push(b'\r');
                        out.push(other);
                    }
                }
            } else if byte == b'\r' {
                self.pending_cr = true;
            } else {
                out.push(byte);
            }
        }
        if final_block {
            out.extend_from_slice(&self.finish());
        }
        out
    }

    fn finish(&mut self) -> Vec<u8> {
        if self.pending_cr {
            self.pending_cr = false;
            return vec![b'\r'];
        }
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::NetAsciiDecoder;

    #[test]
    fn netascii_decoding_handles_crlf_nul_and_boundaries() {
        let mut decoder = NetAsciiDecoder::default();
        assert_eq!(decoder.push(vec![b'a', b'\r'], false), b"a".to_vec());
        assert_eq!(
            decoder.push(vec![b'\n', b'b', b'\r'], false),
            b"\nb".to_vec()
        );
        assert_eq!(decoder.push(vec![0, b'c'], true), b"\rc".to_vec());
    }

    #[test]
    fn trailing_carriage_return_is_emitted_on_finish() {
        let mut decoder = NetAsciiDecoder::default();
        assert_eq!(decoder.push(vec![b'\r'], true), b"\r".to_vec());
    }
}
