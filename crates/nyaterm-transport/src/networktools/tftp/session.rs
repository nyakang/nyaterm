//! One TFTP transfer session, served on its own ephemeral UDP socket (the TID).

use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, Read, Write};
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nyaterm_core::TftpSettings;

use super::packet::{self, Mode, Request};
use super::path::{self, TftpPathError};
use crate::networktools::event::{NetworkToolEvent, TransferDirection, TransferOutcome};
use crate::networktools::registry::NetworkToolKind;
use crate::networktools::status::TransferSnapshot;

const RETRIES: usize = 5;

pub(super) struct SessionContext {
    pub kind: NetworkToolKind,
    pub root: PathBuf,
    pub config: TftpSettings,
    pub bind_ip: IpAddr,
    pub events: SyncSender<NetworkToolEvent>,
    pub transfers: Arc<Mutex<Vec<TransferSnapshot>>>,
    pub stop: Arc<AtomicBool>,
}

pub(super) fn run(request: Request, peer: SocketAddr, id: u64, ctx: SessionContext) {
    let socket = match UdpSocket::bind(SocketAddr::new(ctx.bind_ip, 0)) {
        Ok(socket) => socket,
        Err(error) => {
            emit(
                &ctx,
                NetworkToolEvent::Failed {
                    kind: ctx.kind,
                    message: format!("session socket: {error}"),
                },
            );
            return;
        }
    };
    let timeout = Duration::from_secs(u64::from(ctx.config.timeout_secs.max(1)));
    let _ = socket.set_read_timeout(Some(timeout));

    if request.mode == Mode::Mail {
        let _ = socket.send_to(
            &packet::write_error(packet::ERR_ILLEGAL_OPERATION, "mail mode is not supported"),
            peer,
        );
        return;
    }

    let direction = if request.write {
        TransferDirection::Upload
    } else {
        TransferDirection::Download
    };
    let total = if request.write {
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
            id,
            direction,
            file: request.file.clone(),
            total,
            remote: peer.to_string(),
        },
    );

    let outcome = if request.write {
        receive_file(&socket, peer, &request, &ctx, id, total, timeout)
    } else {
        send_file(&socket, peer, &request, &ctx, id, total, timeout)
    };

    finish_transfer(&ctx, id, direction, &request.file, peer, outcome);
}

fn send_file(
    socket: &UdpSocket,
    peer: SocketAddr,
    request: &Request,
    ctx: &SessionContext,
    id: u64,
    total: Option<u64>,
    timeout: Duration,
) -> TransferOutcome {
    let path = match path::read_path(&ctx.root, &request.file) {
        Ok(path) => path,
        Err(error) => {
            send_path_error(socket, peer, &error);
            return failed(format!("{}", error));
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
    let negotiated = packet::negotiate(request, size);
    let block_size = usize::from(negotiated.block_size);

    if negotiated.has_options() {
        let _ = socket.send_to(&packet::write_oack(&negotiated.accepted), peer);
        if !matches!(wait_ack(socket, peer, 0, timeout), Ack::Matched) {
            return failed("client did not acknowledge transfer options");
        }
    }

    let mut source = Source::new(file, request.mode);
    let mut block: u16 = 1;
    let mut done = 0u64;
    let mut last_percent = None;

    loop {
        if ctx.stop.load(Ordering::Relaxed) {
            return failed("service stopped");
        }
        let chunk = match source.next_block(block_size) {
            Ok(chunk) => chunk,
            Err(error) => {
                let _ = socket.send_to(
                    &packet::write_error(packet::ERR_NOT_DEFINED, &error.to_string()),
                    peer,
                );
                return failed(error.to_string());
            }
        };

        let mut acknowledged = false;
        for _ in 0..RETRIES {
            let _ = socket.send_to(&packet::write_data(block, &chunk), peer);
            match wait_ack(socket, peer, block, timeout) {
                Ack::Matched => {
                    acknowledged = true;
                    break;
                }
                Ack::Error(message) => return failed(message),
                Ack::Other => continue,
                Ack::Timeout => continue,
            }
        }
        if !acknowledged {
            return failed("acknowledgement timed out");
        }

        done += chunk.len() as u64;
        update_progress(ctx, id, done, total, &mut last_percent);
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
    total: Option<u64>,
    timeout: Duration,
) -> TransferOutcome {
    if !ctx.config.allow_upload {
        let _ = socket.send_to(
            &packet::write_error(packet::ERR_ACCESS_VIOLATION, "upload is disabled"),
            peer,
        );
        return failed("upload is disabled");
    }

    let target = match path::write_target(&ctx.root, &request.file, ctx.config.allow_overwrite) {
        Ok(target) => target,
        Err(error) => {
            send_path_error(socket, peer, &error);
            return failed(format!("{error}"));
        }
    };
    let mut file = match File::create(&target.temp) {
        Ok(file) => file,
        Err(error) => {
            let _ = socket.send_to(
                &packet::write_error(packet::ERR_NOT_DEFINED, &error.to_string()),
                peer,
            );
            return failed(error.to_string());
        }
    };

    let negotiated = packet::negotiate(request, None);
    let block_size = usize::from(negotiated.block_size);

    if negotiated.has_options() {
        let _ = socket.send_to(&packet::write_oack(&negotiated.accepted), peer);
    } else {
        let _ = socket.send_to(&packet::write_ack(0), peer);
    }

    let mut expected: u16 = 1;
    let mut done = 0u64;
    let mut last_percent = None;
    let mut retries = 0usize;
    let mut netascii = NetAsciiDecoder::default();
    let mut buffer = vec![0u8; usize::from(packet::MAX_BLOCK_SIZE) + 4];

    loop {
        if ctx.stop.load(Ordering::Relaxed) {
            target.cleanup();
            return failed("service stopped");
        }
        match recv_data(socket, peer, timeout, &mut buffer) {
            Ok(Some((block, payload))) => {
                if block == expected {
                    let received_len = payload.len() as u64;
                    let bytes = if request.mode == Mode::NetAscii {
                        netascii.push(payload, false)
                    } else {
                        payload
                    };
                    if let Err(error) = file.write_all(&bytes) {
                        target.cleanup();
                        let _ = socket.send_to(
                            &packet::write_error(packet::ERR_DISK_FULL, &error.to_string()),
                            peer,
                        );
                        return failed(error.to_string());
                    }
                    let _ = socket.send_to(&packet::write_ack(expected), peer);
                    done += received_len;
                    update_progress(ctx, id, done, total, &mut last_percent);
                    retries = 0;
                    if received_len < block_size as u64 {
                        let tail = if request.mode == Mode::NetAscii {
                            netascii.finish()
                        } else {
                            Vec::new()
                        };
                        if let Err(error) = file.write_all(&tail) {
                            target.cleanup();
                            return failed(error.to_string());
                        }
                        return match target.commit() {
                            Ok(()) => TransferOutcome::Completed { bytes: done },
                            Err(error) => {
                                target.cleanup();
                                failed(error.to_string())
                            }
                        };
                    }
                    expected = expected.wrapping_add(1);
                } else if block == expected.wrapping_sub(1) {
                    // Duplicate of the last accepted block: re-acknowledge it.
                    let _ = socket.send_to(&packet::write_ack(block), peer);
                }
            }
            Ok(None) => return failed("invalid data packet"),
            Err(error) if error.kind() == io::ErrorKind::TimedOut => {
                retries += 1;
                if retries > RETRIES {
                    target.cleanup();
                    return failed("transfer timed out");
                }
                let _ = socket.send_to(&packet::write_ack(expected.wrapping_sub(1)), peer);
            }
            Err(error) => {
                target.cleanup();
                return failed(error.to_string());
            }
        }
    }
}

fn wait_ack(socket: &UdpSocket, peer: SocketAddr, expected: u16, timeout: Duration) -> Ack {
    let mut buffer = [0u8; 64];
    let _ = socket.set_read_timeout(Some(timeout));
    match socket.recv_from(&mut buffer) {
        Ok((size, from)) if from == peer => match packet::parse_ack(&buffer[..size]) {
            Ok(block) if block == expected => Ack::Matched,
            Ok(_) => Ack::Other,
            Err(_) => match packet::parse_error(&buffer[..size]) {
                Ok((code, message)) => Ack::Error(tftp_error_reason(code, &message)),
                Err(_) => Ack::Other,
            },
        },
        Ok((_, from)) => {
            let _ = socket.send_to(
                &packet::write_error(packet::ERR_UNKNOWN_TID, "unknown transfer ID"),
                from,
            );
            Ack::Other
        }
        Err(error)
            if error.kind() == io::ErrorKind::TimedOut
                || error.kind() == io::ErrorKind::WouldBlock =>
        {
            Ack::Timeout
        }
        Err(_) => Ack::Timeout,
    }
}

fn recv_data(
    socket: &UdpSocket,
    peer: SocketAddr,
    timeout: Duration,
    buffer: &mut [u8],
) -> io::Result<Option<(u16, Vec<u8>)>> {
    let _ = socket.set_read_timeout(Some(timeout));
    let (size, from) = socket.recv_from(buffer)?;
    if from != peer {
        let _ = socket.send_to(
            &packet::write_error(packet::ERR_UNKNOWN_TID, "unknown transfer ID"),
            from,
        );
        return Ok(Some((u16::MAX, Vec::new())));
    }
    match packet::parse_data(&buffer[..size]) {
        Ok((block, payload)) => Ok(Some((block, payload.to_vec()))),
        // A client can abort an upload by sending ERROR instead of DATA. Keep
        // its reason instead of collapsing it into an "invalid data packet".
        Err(_) => match packet::parse_error(&buffer[..size]) {
            Ok((code, message)) => Err(io::Error::other(tftp_error_reason(code, &message))),
            Err(_) => Ok(None),
        },
    }
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
        id,
        direction,
        file.to_string(),
        peer.to_string(),
        total,
    ));
}

fn update_progress(
    ctx: &SessionContext,
    id: u64,
    done: u64,
    total: Option<u64>,
    last_percent: &mut Option<u64>,
) {
    let mut transfers = ctx
        .transfers
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(snapshot) = transfers.iter_mut().find(|snapshot| snapshot.id == id) {
        snapshot.advance(done);
    }
    drop(transfers);

    // Coalesce progress at 1% boundaries (or every block when the size is
    // unknown) so the bounded event channel is not saturated by ACK-sized steps.
    let percent = total.map(|total| {
        done.saturating_mul(100)
            .checked_div(total)
            .unwrap_or(100)
            .min(100)
    });
    let should_emit = match percent {
        Some(percent) => *last_percent != Some(percent),
        None => last_percent.is_none(),
    };
    if should_emit {
        *last_percent = percent;
        emit(
            ctx,
            NetworkToolEvent::TransferProgress {
                kind: ctx.kind,
                id,
                done,
                total,
            },
        );
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
            id,
            direction,
            file: file.to_string(),
            remote: peer.to_string(),
            outcome,
        },
    );
}

fn emit(ctx: &SessionContext, event: NetworkToolEvent) {
    let _ = ctx.events.try_send(event);
}

fn failed(message: impl Into<String>) -> TransferOutcome {
    TransferOutcome::Failed {
        message: message.into(),
    }
}

enum Ack {
    Matched,
    Other,
    Error(String),
    Timeout,
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
    use super::*;

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
