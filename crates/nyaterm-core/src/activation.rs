mod dedup;
mod deep_link;
pub use dedup::RecentConnectionRequests;

pub use deep_link::{
    ActivationAction, ActivationParseError, ExternalConnectionRequest, MAX_ACTIVATION_ACTIONS,
    MAX_DEEP_LINK_BYTES, normalize_external_host, parse_activation_batch, parse_activation_request,
    parse_deep_link,
};

use std::ffi::OsString;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

use futures::StreamExt as _;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use thiserror::Error;

pub const ACTIVATION_MAGIC: [u8; 4] = *b"NTSI";
pub const ACTIVATION_PROTOCOL_VERSION: u16 = 1;
pub const MAX_ACTIVATION_FRAME_BYTES: usize = 256 * 1024;
pub const MAX_ACTIVATION_ARGS: usize = 256;
pub const MAX_ACTIVATION_ARG_BYTES: usize = 32 * 1024;
pub const ACTIVATION_QUEUE_CAPACITY: usize = 64;

const HEADER_LEN: usize = 12;
const REQUEST_KIND: u8 = 1;
const ACK_KIND: u8 = 2;

#[derive(Clone, PartialEq, Eq)]
pub enum RawActivationArg {
    Bytes(Vec<u8>),
    Wide(Vec<u16>),
}

impl std::fmt::Debug for RawActivationArg {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("RawActivationArg(<redacted>)")
    }
}

impl Drop for RawActivationArg {
    fn drop(&mut self) {
        use zeroize::Zeroize as _;
        match self {
            Self::Bytes(bytes) => bytes.zeroize(),
            Self::Wide(units) => units.zeroize(),
        }
    }
}

impl RawActivationArg {
    pub fn from_os_string(value: OsString) -> Self {
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt as _;
            Self::Wide(value.as_os_str().encode_wide().collect())
        }
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt as _;
            Self::Bytes(value.into_vec())
        }
        #[cfg(not(any(unix, windows)))]
        {
            Self::Bytes(value.to_string_lossy().into_owned().into_bytes())
        }
    }

    fn encoded_len(&self) -> usize {
        match self {
            Self::Bytes(bytes) => bytes.len(),
            Self::Wide(units) => units.len().saturating_mul(2),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActivationRequest {
    pub request_id: [u8; 16],
    pub args: Vec<RawActivationArg>,
}

impl ActivationRequest {
    pub fn from_os_args(request_id: [u8; 16], args: impl IntoIterator<Item = OsString>) -> Self {
        Self {
            request_id,
            args: args
                .into_iter()
                .map(RawActivationArg::from_os_string)
                .collect(),
        }
    }

    pub fn parse(&self) -> ParsedActivationRequest {
        let actions = parse_activation_batch(self);
        let reuse = self.args.iter().any(|arg| arg.matches("--reuse-window"))
            || actions
                .iter()
                .any(|action| matches!(action, Ok(ActivationAction::Connect(_))));
        ParsedActivationRequest {
            request_id: self.request_id,
            actions,
            open_behavior: if reuse {
                crate::ActivationOpenBehavior::ReuseMostRecent
            } else {
                crate::ActivationOpenBehavior::NewWindow
            },
        }
    }

    pub fn open_behavior(&self) -> crate::ActivationOpenBehavior {
        self.parse().open_behavior
    }
}

/// Parsed once at the process boundary and shared by routing and execution.
/// Secret-bearing actions deliberately have no serialization implementation.
#[derive(Clone, Debug)]
pub struct ParsedActivationRequest {
    pub request_id: [u8; 16],
    pub actions: Vec<Result<ActivationAction, ActivationParseError>>,
    pub open_behavior: crate::ActivationOpenBehavior,
}

impl RawActivationArg {
    fn matches(&self, expected: &str) -> bool {
        match self {
            Self::Bytes(bytes) => String::from_utf8_lossy(bytes) == expected,
            Self::Wide(units) => String::from_utf16_lossy(units) == expected,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ActivationAckStatus {
    Accepted = 0,
    QueueFull = 1,
    ShuttingDown = 2,
    InvalidRequest = 3,
}

impl ActivationAckStatus {
    fn from_u8(value: u8) -> Result<Self, ActivationProtocolError> {
        match value {
            0 => Ok(Self::Accepted),
            1 => Ok(Self::QueueFull),
            2 => Ok(Self::ShuttingDown),
            3 => Ok(Self::InvalidRequest),
            _ => Err(ActivationProtocolError::InvalidAckStatus(value)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActivationAck {
    pub request_id: [u8; 16],
    pub status: ActivationAckStatus,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ActivationProtocolError {
    #[error("activation frame is truncated")]
    Truncated,
    #[error("activation frame has an invalid magic value")]
    InvalidMagic,
    #[error("unsupported activation protocol version {0}")]
    UnsupportedVersion(u16),
    #[error("unexpected activation message kind {0}")]
    UnexpectedKind(u8),
    #[error("activation frame exceeds the size limit")]
    FrameTooLarge,
    #[error("activation request has too many arguments")]
    TooManyArguments,
    #[error("activation argument exceeds the size limit")]
    ArgumentTooLarge,
    #[error("activation argument has an invalid encoding tag {0}")]
    InvalidArgumentEncoding(u8),
    #[error("activation wide argument has an odd byte length")]
    InvalidWideArgument,
    #[error("activation frame contains trailing data")]
    TrailingData,
    #[error("activation ACK has an invalid status {0}")]
    InvalidAckStatus(u8),
}

pub fn encode_activation_request(
    auth_token: [u8; 16],
    request: &ActivationRequest,
) -> Result<Vec<u8>, ActivationProtocolError> {
    if request.args.len() > MAX_ACTIVATION_ARGS {
        return Err(ActivationProtocolError::TooManyArguments);
    }
    let mut payload = Vec::new();
    payload.extend_from_slice(&auth_token);
    payload.extend_from_slice(&request.request_id);
    payload.extend_from_slice(&(request.args.len() as u16).to_le_bytes());
    for arg in &request.args {
        let encoded_len = arg.encoded_len();
        if encoded_len > MAX_ACTIVATION_ARG_BYTES {
            return Err(ActivationProtocolError::ArgumentTooLarge);
        }
        match arg {
            RawActivationArg::Bytes(bytes) => {
                payload.push(0);
                payload.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
                payload.extend_from_slice(bytes);
            }
            RawActivationArg::Wide(units) => {
                payload.push(1);
                payload.extend_from_slice(&(encoded_len as u32).to_le_bytes());
                for unit in units {
                    payload.extend_from_slice(&unit.to_le_bytes());
                }
            }
        }
    }
    encode_frame(REQUEST_KIND, &payload)
}

pub fn decode_activation_request(
    frame: &[u8],
) -> Result<([u8; 16], ActivationRequest), ActivationProtocolError> {
    let payload = decode_frame(frame, REQUEST_KIND)?;
    let mut cursor = Cursor::new(payload);
    let auth_token = cursor.read_array()?;
    let request_id = cursor.read_array()?;
    let arg_count = cursor.read_u16()? as usize;
    if arg_count > MAX_ACTIVATION_ARGS {
        return Err(ActivationProtocolError::TooManyArguments);
    }
    let mut args = Vec::with_capacity(arg_count);
    for _ in 0..arg_count {
        let encoding = cursor.read_u8()?;
        let byte_len = cursor.read_u32()? as usize;
        if byte_len > MAX_ACTIVATION_ARG_BYTES {
            return Err(ActivationProtocolError::ArgumentTooLarge);
        }
        let bytes = cursor.read_bytes(byte_len)?;
        let arg = match encoding {
            0 => RawActivationArg::Bytes(bytes.to_vec()),
            1 => {
                if !byte_len.is_multiple_of(2) {
                    return Err(ActivationProtocolError::InvalidWideArgument);
                }
                RawActivationArg::Wide(
                    bytes
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|chunk| u16::from_le_bytes(*chunk))
                        .collect(),
                )
            }
            value => return Err(ActivationProtocolError::InvalidArgumentEncoding(value)),
        };
        args.push(arg);
    }
    cursor.finish()?;
    Ok((auth_token, ActivationRequest { request_id, args }))
}

pub fn encode_activation_ack(ack: ActivationAck) -> Result<Vec<u8>, ActivationProtocolError> {
    let mut payload = Vec::with_capacity(17);
    payload.extend_from_slice(&ack.request_id);
    payload.push(ack.status as u8);
    encode_frame(ACK_KIND, &payload)
}

pub fn decode_activation_ack(frame: &[u8]) -> Result<ActivationAck, ActivationProtocolError> {
    let payload = decode_frame(frame, ACK_KIND)?;
    let mut cursor = Cursor::new(payload);
    let request_id = cursor.read_array()?;
    let status = ActivationAckStatus::from_u8(cursor.read_u8()?)?;
    cursor.finish()?;
    Ok(ActivationAck { request_id, status })
}

fn encode_frame(kind: u8, payload: &[u8]) -> Result<Vec<u8>, ActivationProtocolError> {
    if payload.len() > MAX_ACTIVATION_FRAME_BYTES.saturating_sub(HEADER_LEN) {
        return Err(ActivationProtocolError::FrameTooLarge);
    }
    let mut frame = Vec::with_capacity(HEADER_LEN + payload.len());
    frame.extend_from_slice(&ACTIVATION_MAGIC);
    frame.extend_from_slice(&ACTIVATION_PROTOCOL_VERSION.to_le_bytes());
    frame.push(kind);
    frame.push(0);
    frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    frame.extend_from_slice(payload);
    Ok(frame)
}

fn decode_frame(frame: &[u8], expected_kind: u8) -> Result<&[u8], ActivationProtocolError> {
    if frame.len() < HEADER_LEN {
        return Err(ActivationProtocolError::Truncated);
    }
    if frame[..4] != ACTIVATION_MAGIC {
        return Err(ActivationProtocolError::InvalidMagic);
    }
    let version = u16::from_le_bytes([frame[4], frame[5]]);
    if version != ACTIVATION_PROTOCOL_VERSION {
        return Err(ActivationProtocolError::UnsupportedVersion(version));
    }
    let kind = frame[6];
    if kind != expected_kind {
        return Err(ActivationProtocolError::UnexpectedKind(kind));
    }
    let payload_len = u32::from_le_bytes([frame[8], frame[9], frame[10], frame[11]]) as usize;
    if payload_len > MAX_ACTIVATION_FRAME_BYTES.saturating_sub(HEADER_LEN) {
        return Err(ActivationProtocolError::FrameTooLarge);
    }
    let expected_len = HEADER_LEN
        .checked_add(payload_len)
        .ok_or(ActivationProtocolError::FrameTooLarge)?;
    if frame.len() < expected_len {
        return Err(ActivationProtocolError::Truncated);
    }
    if frame.len() != expected_len {
        return Err(ActivationProtocolError::TrailingData);
    }
    Ok(&frame[HEADER_LEN..])
}

struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn read_u8(&mut self) -> Result<u8, ActivationProtocolError> {
        Ok(self.read_bytes(1)?[0])
    }

    fn read_u16(&mut self) -> Result<u16, ActivationProtocolError> {
        let bytes = self.read_bytes(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    fn read_u32(&mut self) -> Result<u32, ActivationProtocolError> {
        let bytes = self.read_bytes(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn read_array<const N: usize>(&mut self) -> Result<[u8; N], ActivationProtocolError> {
        self.read_bytes(N)?
            .try_into()
            .map_err(|_| ActivationProtocolError::Truncated)
    }

    fn read_bytes(&mut self, len: usize) -> Result<&'a [u8], ActivationProtocolError> {
        let end = self
            .offset
            .checked_add(len)
            .ok_or(ActivationProtocolError::FrameTooLarge)?;
        let bytes = self
            .bytes
            .get(self.offset..end)
            .ok_or(ActivationProtocolError::Truncated)?;
        self.offset = end;
        Ok(bytes)
    }

    fn finish(self) -> Result<(), ActivationProtocolError> {
        if self.offset == self.bytes.len() {
            Ok(())
        } else {
            Err(ActivationProtocolError::TrailingData)
        }
    }
}

#[derive(Clone)]
pub struct ActivationSender {
    tx: UnboundedSender<ActivationRequest>,
    queued: Arc<AtomicUsize>,
    closed: Arc<AtomicBool>,
    capacity: usize,
}

pub struct ActivationReceiver {
    rx: UnboundedReceiver<ActivationRequest>,
    queued: Arc<AtomicUsize>,
    closed: Arc<AtomicBool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivationQueueError {
    Full,
    Closed,
}

pub fn activation_channel(capacity: usize) -> (ActivationSender, ActivationReceiver) {
    let (tx, rx) = unbounded();
    let queued = Arc::new(AtomicUsize::new(0));
    let closed = Arc::new(AtomicBool::new(false));
    (
        ActivationSender {
            tx,
            queued: Arc::clone(&queued),
            closed: Arc::clone(&closed),
            capacity: capacity.max(1),
        },
        ActivationReceiver { rx, queued, closed },
    )
}

impl ActivationSender {
    pub fn try_send(&self, request: ActivationRequest) -> Result<(), ActivationQueueError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(ActivationQueueError::Closed);
        }
        let mut queued = self.queued.load(Ordering::Acquire);
        loop {
            if queued >= self.capacity {
                return Err(ActivationQueueError::Full);
            }
            match self.queued.compare_exchange_weak(
                queued,
                queued + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(actual) => queued = actual,
            }
        }
        if self.tx.unbounded_send(request).is_err() {
            self.queued.fetch_sub(1, Ordering::AcqRel);
            return Err(ActivationQueueError::Closed);
        }
        Ok(())
    }

    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
    }
}

impl ActivationReceiver {
    pub async fn recv(&mut self) -> Option<ActivationRequest> {
        let request = self.rx.next().await?;
        self.queued.fetch_sub(1, Ordering::AcqRel);
        Some(request)
    }

    pub fn close(&mut self) {
        self.closed.store(true, Ordering::Release);
        self.rx.close();
    }
}

impl Drop for ActivationReceiver {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ActivationAck, ActivationAckStatus, ActivationProtocolError, ActivationQueueError,
        ActivationRequest, RawActivationArg, activation_channel, decode_activation_ack,
        decode_activation_request, encode_activation_ack, encode_activation_request,
    };

    #[test]
    fn activation_request_round_trips_raw_unix_and_windows_arguments() {
        let request = ActivationRequest {
            request_id: [7; 16],
            args: vec![
                RawActivationArg::Bytes(vec![0xff, b'a', 0]),
                RawActivationArg::Wide(vec![0xd800, 0x0061]),
            ],
        };
        let frame = encode_activation_request([9; 16], &request).unwrap();
        let (token, decoded) = decode_activation_request(&frame).unwrap();
        assert_eq!(token, [9; 16]);
        assert_eq!(decoded, request);
    }

    #[test]
    fn activation_codec_rejects_truncated_and_trailing_frames() {
        let request = ActivationRequest {
            request_id: [1; 16],
            args: Vec::new(),
        };
        let frame = encode_activation_request([2; 16], &request).unwrap();
        assert_eq!(
            decode_activation_request(&frame[..frame.len() - 1]),
            Err(ActivationProtocolError::Truncated)
        );
        let mut trailing = frame;
        trailing.push(0);
        assert_eq!(
            decode_activation_request(&trailing),
            Err(ActivationProtocolError::TrailingData)
        );
    }

    #[test]
    fn activation_ack_round_trips_request_identity_and_status() {
        let ack = ActivationAck {
            request_id: [3; 16],
            status: ActivationAckStatus::QueueFull,
        };
        assert_eq!(
            decode_activation_ack(&encode_activation_ack(ack).unwrap()),
            Ok(ack)
        );
    }

    #[test]
    fn activation_queue_is_bounded_and_reopens_capacity_after_receive() {
        let (sender, mut receiver) = activation_channel(1);
        let request = ActivationRequest {
            request_id: [4; 16],
            args: Vec::new(),
        };
        sender.try_send(request.clone()).unwrap();
        assert_eq!(
            sender.try_send(request.clone()),
            Err(ActivationQueueError::Full)
        );
        assert_eq!(
            futures::executor::block_on(receiver.recv()),
            Some(request.clone())
        );
        sender.try_send(request).unwrap();
        receiver.close();
        assert_eq!(
            sender.try_send(ActivationRequest {
                request_id: [5; 16],
                args: Vec::new(),
            }),
            Err(ActivationQueueError::Closed)
        );
    }
}
