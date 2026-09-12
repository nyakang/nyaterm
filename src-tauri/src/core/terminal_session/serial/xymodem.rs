use crate::config::SerialModemUploadProtocol;
use serde::Serialize;
use std::fs::File;
use std::io::Read;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const SOH: u8 = 0x01;
const EOT: u8 = 0x04;
const ACK: u8 = 0x06;
const NAK: u8 = 0x15;
const CAN: u8 = 0x18;
const CRC_REQUEST: u8 = b'C';
const CPM_EOF: u8 = 0x1a;
const BLOCK_SIZE: usize = 128;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_RETRIES: u8 = 10;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum SerialModemEvent {
    Progress {
        protocol: SerialModemUploadProtocol,
        #[serde(rename = "fileName")]
        file_name: String,
        #[serde(rename = "fileIndex")]
        file_index: u32,
        #[serde(rename = "bytesTransferred")]
        bytes_transferred: u64,
        #[serde(rename = "totalSize")]
        total_size: u64,
    },
    FileComplete {
        protocol: SerialModemUploadProtocol,
        #[serde(rename = "fileIndex")]
        file_index: u32,
        #[serde(rename = "fileName")]
        file_name: String,
    },
    Complete {
        protocol: SerialModemUploadProtocol,
        #[serde(rename = "fileCount")]
        file_count: u32,
    },
    Failed {
        protocol: SerialModemUploadProtocol,
        reason: String,
        #[serde(rename = "fileIndex")]
        file_index: u32,
        #[serde(rename = "fileName", skip_serializing_if = "Option::is_none")]
        file_name: Option<String>,
    },
}

pub enum XyModemAction {
    SendToRemote(Vec<u8>),
    EmitEvent(SerialModemEvent),
}

pub struct XyModemFeedResult {
    pub actions: Vec<XyModemAction>,
    pub consumed: usize,
}

struct UploadFile {
    path: PathBuf,
    name: String,
    size: u64,
}

#[derive(Clone, Copy)]
enum CheckMode {
    Checksum,
    Crc,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TransferState {
    WaitingHandshake,
    WaitingHeaderAck,
    WaitingDataStart,
    WaitingDataAck,
    WaitingEotAck,
    Done,
}

pub struct XyModemTransfer {
    protocol: SerialModemUploadProtocol,
    files: Vec<UploadFile>,
    file_index: usize,
    current_file: Option<File>,
    bytes_acked: u64,
    sequence: u8,
    check_mode: CheckMode,
    state: TransferState,
    last_packet: Vec<u8>,
    last_payload_len: usize,
    retries: u8,
    remote_cancel_count: u8,
    deadline: Instant,
}

impl XyModemTransfer {
    pub fn new(
        protocol: SerialModemUploadProtocol,
        paths: Vec<PathBuf>,
    ) -> Result<(Self, Vec<XyModemAction>), String> {
        Self::new_at(protocol, paths, Instant::now())
    }

    fn new_at(
        protocol: SerialModemUploadProtocol,
        paths: Vec<PathBuf>,
        now: Instant,
    ) -> Result<(Self, Vec<XyModemAction>), String> {
        if protocol == SerialModemUploadProtocol::Zmodem {
            return Err("ZMODEM is handled by the existing ZMODEM engine".to_string());
        }
        if paths.is_empty() {
            return Err("No files selected for modem upload".to_string());
        }
        if protocol == SerialModemUploadProtocol::Xmodem && paths.len() != 1 {
            return Err("XMODEM supports exactly one file per transfer".to_string());
        }

        let mut files = Vec::with_capacity(paths.len());
        for path in paths {
            let metadata = std::fs::metadata(&path)
                .map_err(|error| format!("Failed to inspect '{}': {error}", path.display()))?;
            if !metadata.is_file() {
                return Err(format!(
                    "Modem upload only supports files: {}",
                    path.display()
                ));
            }
            let name = path
                .file_name()
                .filter(|name| !name.is_empty())
                .map(|name| name.to_string_lossy().into_owned())
                .ok_or_else(|| format!("File has no basename: {}", path.display()))?;
            if protocol == SerialModemUploadProtocol::Ymodem {
                validate_ymodem_header(&name, metadata.len())?;
            }
            files.push(UploadFile {
                path,
                name,
                size: metadata.len(),
            });
        }

        let transfer = Self {
            protocol,
            files,
            file_index: 0,
            current_file: None,
            bytes_acked: 0,
            sequence: 1,
            check_mode: CheckMode::Crc,
            state: TransferState::WaitingHandshake,
            last_packet: Vec::new(),
            last_payload_len: 0,
            retries: 0,
            remote_cancel_count: 0,
            deadline: now + HANDSHAKE_TIMEOUT,
        };
        let progress = transfer.progress_event();
        Ok((transfer, vec![XyModemAction::EmitEvent(progress)]))
    }

    pub fn is_done(&self) -> bool {
        self.state == TransferState::Done
    }

    #[cfg(test)]
    pub fn feed_incoming(&mut self, data: &[u8]) -> Vec<XyModemAction> {
        self.feed_incoming_result(data).actions
    }

    pub fn feed_incoming_result(&mut self, data: &[u8]) -> XyModemFeedResult {
        self.feed_incoming_result_at(data, Instant::now())
    }

    #[cfg(test)]
    fn feed_incoming_at(&mut self, data: &[u8], now: Instant) -> Vec<XyModemAction> {
        self.feed_incoming_result_at(data, now).actions
    }

    fn feed_incoming_result_at(&mut self, data: &[u8], now: Instant) -> XyModemFeedResult {
        if self.is_done() {
            return XyModemFeedResult {
                actions: Vec::new(),
                consumed: 0,
            };
        }

        let mut actions = Vec::new();
        if now >= self.deadline && !data.iter().copied().any(|byte| self.accepts_response(byte)) {
            actions.extend(self.tick_at(now));
            return XyModemFeedResult {
                actions,
                consumed: 0,
            };
        }

        let mut consumed = 0;
        for &byte in data {
            if self.is_done() {
                break;
            }
            consumed += 1;

            let action_start = actions.len();

            if byte == CAN {
                self.remote_cancel_count = self.remote_cancel_count.saturating_add(1);
                if self.remote_cancel_count >= 2 {
                    actions.extend(self.fail("Receiver cancelled the modem transfer"));
                }
                continue;
            }
            self.remote_cancel_count = 0;

            match self.protocol {
                SerialModemUploadProtocol::Xmodem => {
                    self.handle_xmodem_byte(byte, now, &mut actions)
                }
                SerialModemUploadProtocol::Ymodem => {
                    self.handle_ymodem_byte(byte, now, &mut actions)
                }
                SerialModemUploadProtocol::Zmodem => unreachable!(),
            }

            if actions[action_start..]
                .iter()
                .any(|action| matches!(action, XyModemAction::SendToRemote(_)))
            {
                break;
            }
        }

        if !self.is_done() {
            actions.extend(self.tick_at(now));
        }
        XyModemFeedResult { actions, consumed }
    }

    fn accepts_response(&self, byte: u8) -> bool {
        if byte == CAN {
            return true;
        }
        match self.state {
            TransferState::WaitingHandshake => match self.protocol {
                SerialModemUploadProtocol::Xmodem => matches!(byte, NAK | CRC_REQUEST),
                SerialModemUploadProtocol::Ymodem => byte == CRC_REQUEST,
                SerialModemUploadProtocol::Zmodem => false,
            },
            TransferState::WaitingHeaderAck
            | TransferState::WaitingDataAck
            | TransferState::WaitingEotAck => matches!(byte, ACK | NAK),
            TransferState::WaitingDataStart => {
                self.protocol == SerialModemUploadProtocol::Ymodem && byte == CRC_REQUEST
            }
            TransferState::Done => false,
        }
    }

    pub fn tick(&mut self) -> Vec<XyModemAction> {
        self.tick_at(Instant::now())
    }

    fn tick_at(&mut self, now: Instant) -> Vec<XyModemAction> {
        if self.is_done() || now < self.deadline {
            return Vec::new();
        }
        match self.state {
            TransferState::WaitingHandshake | TransferState::WaitingDataStart => {
                self.fail("Timed out waiting for modem receiver handshake")
            }
            TransferState::WaitingHeaderAck
            | TransferState::WaitingDataAck
            | TransferState::WaitingEotAck => self.retry_last(now),
            TransferState::Done => Vec::new(),
        }
    }

    fn handle_xmodem_byte(&mut self, byte: u8, now: Instant, actions: &mut Vec<XyModemAction>) {
        match self.state {
            TransferState::WaitingHandshake => {
                let check_mode = match byte {
                    NAK => CheckMode::Checksum,
                    CRC_REQUEST => CheckMode::Crc,
                    _ => return,
                };
                self.check_mode = check_mode;
                self.sequence = 1;
                if let Err(reason) = self.send_next_data(now, actions) {
                    actions.extend(self.fail(reason));
                }
            }
            TransferState::WaitingDataAck if byte == ACK => {
                self.note_data_acked(actions);
                self.sequence = self.sequence.wrapping_add(1);
                self.retries = 0;
                if let Err(reason) = self.send_next_data(now, actions) {
                    actions.extend(self.fail(reason));
                }
            }
            TransferState::WaitingDataAck if byte == NAK => actions.extend(self.retry_last(now)),
            TransferState::WaitingEotAck if byte == ACK => {
                self.state = TransferState::Done;
                actions.push(XyModemAction::EmitEvent(SerialModemEvent::Complete {
                    protocol: self.protocol,
                    file_count: 1,
                }));
            }
            TransferState::WaitingEotAck if byte == NAK => actions.extend(self.retry_last(now)),
            _ => {}
        }
    }

    fn handle_ymodem_byte(&mut self, byte: u8, now: Instant, actions: &mut Vec<XyModemAction>) {
        match self.state {
            TransferState::WaitingHandshake if byte == CRC_REQUEST => {
                if let Err(reason) = self.send_ymodem_header(now, actions) {
                    actions.extend(self.fail(reason));
                }
            }
            TransferState::WaitingHeaderAck if byte == ACK => {
                self.retries = 0;
                if self.file_index >= self.files.len() {
                    self.state = TransferState::Done;
                    actions.push(XyModemAction::EmitEvent(SerialModemEvent::Complete {
                        protocol: self.protocol,
                        file_count: self.files.len() as u32,
                    }));
                } else {
                    self.state = TransferState::WaitingDataStart;
                    self.deadline = now + HANDSHAKE_TIMEOUT;
                }
            }
            TransferState::WaitingHeaderAck if byte == NAK => actions.extend(self.retry_last(now)),
            TransferState::WaitingDataStart if byte == CRC_REQUEST => {
                self.sequence = 1;
                if let Err(reason) = self.send_next_data(now, actions) {
                    actions.extend(self.fail(reason));
                }
            }
            TransferState::WaitingDataAck if byte == ACK => {
                self.note_data_acked(actions);
                self.sequence = self.sequence.wrapping_add(1);
                self.retries = 0;
                if let Err(reason) = self.send_next_data(now, actions) {
                    actions.extend(self.fail(reason));
                }
            }
            TransferState::WaitingDataAck if byte == NAK => actions.extend(self.retry_last(now)),
            TransferState::WaitingEotAck if byte == NAK => actions.extend(self.retry_last(now)),
            TransferState::WaitingEotAck if byte == ACK => {
                self.retries = 0;
                if let Some(file) = self.files.get(self.file_index) {
                    actions.push(XyModemAction::EmitEvent(SerialModemEvent::FileComplete {
                        protocol: self.protocol,
                        file_index: self.file_index as u32,
                        file_name: file.name.clone(),
                    }));
                }
                self.current_file = None;
                self.file_index += 1;
                self.bytes_acked = 0;
                self.last_payload_len = 0;
                self.state = TransferState::WaitingHandshake;
                self.deadline = now + HANDSHAKE_TIMEOUT;
                if self.file_index < self.files.len() {
                    actions.push(XyModemAction::EmitEvent(self.progress_event()));
                }
            }
            _ => {}
        }
    }

    fn send_ymodem_header(
        &mut self,
        now: Instant,
        actions: &mut Vec<XyModemAction>,
    ) -> Result<(), String> {
        let mut data = [0u8; BLOCK_SIZE];
        if let Some(file) = self.files.get(self.file_index) {
            let size = file.size.to_string();
            let name_bytes = file.name.as_bytes();
            data[..name_bytes.len()].copy_from_slice(name_bytes);
            let size_start = name_bytes.len() + 1;
            data[size_start..size_start + size.len()].copy_from_slice(size.as_bytes());
        }
        let packet = build_packet(0, &data, CheckMode::Crc);
        self.set_waiting_packet(packet, 0, TransferState::WaitingHeaderAck, now, actions);
        Ok(())
    }

    fn send_next_data(
        &mut self,
        now: Instant,
        actions: &mut Vec<XyModemAction>,
    ) -> Result<(), String> {
        if self.current_file.is_none() {
            let file = self
                .files
                .get(self.file_index)
                .ok_or_else(|| "Modem transfer file index is out of range".to_string())?;
            self.current_file =
                Some(File::open(&file.path).map_err(|error| {
                    format!("Failed to open '{}': {error}", file.path.display())
                })?);
        }

        let mut data = [CPM_EOF; BLOCK_SIZE];
        let read = self
            .current_file
            .as_mut()
            .expect("current file initialized")
            .read(&mut data)
            .map_err(|error| format!("Failed to read modem upload file: {error}"))?;
        if read == 0 {
            self.set_waiting_packet(vec![EOT], 0, TransferState::WaitingEotAck, now, actions);
            return Ok(());
        }

        let packet = build_packet(self.sequence, &data, self.check_mode);
        self.set_waiting_packet(packet, read, TransferState::WaitingDataAck, now, actions);
        Ok(())
    }

    fn set_waiting_packet(
        &mut self,
        packet: Vec<u8>,
        payload_len: usize,
        state: TransferState,
        now: Instant,
        actions: &mut Vec<XyModemAction>,
    ) {
        self.last_packet = packet.clone();
        self.last_payload_len = payload_len;
        self.state = state;
        self.retries = 0;
        self.deadline = now + RESPONSE_TIMEOUT;
        actions.push(XyModemAction::SendToRemote(packet));
    }

    fn retry_last(&mut self, now: Instant) -> Vec<XyModemAction> {
        self.retries = self.retries.saturating_add(1);
        if self.retries > MAX_RETRIES {
            return self.fail("Modem receiver did not acknowledge the transfer");
        }
        self.deadline = now + RESPONSE_TIMEOUT;
        vec![XyModemAction::SendToRemote(self.last_packet.clone())]
    }

    fn note_data_acked(&mut self, actions: &mut Vec<XyModemAction>) {
        self.bytes_acked = self
            .bytes_acked
            .saturating_add(self.last_payload_len as u64)
            .min(self.files[self.file_index].size);
        actions.push(XyModemAction::EmitEvent(self.progress_event()));
    }

    fn progress_event(&self) -> SerialModemEvent {
        let file = &self.files[self.file_index];
        SerialModemEvent::Progress {
            protocol: self.protocol,
            file_name: file.name.clone(),
            file_index: self.file_index as u32,
            bytes_transferred: self.bytes_acked,
            total_size: file.size,
        }
    }

    fn fail(&mut self, reason: impl Into<String>) -> Vec<XyModemAction> {
        let file_name = self
            .files
            .get(self.file_index)
            .map(|file| file.name.clone());
        self.state = TransferState::Done;
        vec![
            XyModemAction::SendToRemote(vec![CAN; 8]),
            XyModemAction::EmitEvent(SerialModemEvent::Failed {
                protocol: self.protocol,
                reason: reason.into(),
                file_index: self.file_index.min(self.files.len().saturating_sub(1)) as u32,
                file_name,
            }),
        ]
    }
}

fn validate_ymodem_header(name: &str, size: u64) -> Result<(), String> {
    let metadata_len = name.len() + 1 + size.to_string().len();
    if metadata_len > BLOCK_SIZE {
        return Err(format!("YMODEM file name is too long: {name}"));
    }
    Ok(())
}

fn build_packet(sequence: u8, data: &[u8; BLOCK_SIZE], check_mode: CheckMode) -> Vec<u8> {
    let trailer_len = match check_mode {
        CheckMode::Checksum => 1,
        CheckMode::Crc => 2,
    };
    let mut packet = Vec::with_capacity(3 + BLOCK_SIZE + trailer_len);
    packet.extend_from_slice(&[SOH, sequence, !sequence]);
    packet.extend_from_slice(data);
    match check_mode {
        CheckMode::Checksum => {
            packet.push(data.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)))
        }
        CheckMode::Crc => packet.extend_from_slice(&crc16_xmodem(data).to_be_bytes()),
    }
    packet
}

fn crc16_xmodem(data: &[u8]) -> u16 {
    let mut crc = 0u16;
    for &byte in data {
        crc ^= u16::from(byte) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST_FILE: AtomicU64 = AtomicU64::new(1);

    fn test_file(name: &str, data: &[u8]) -> PathBuf {
        let id = NEXT_TEST_FILE.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("nyaterm-xymodem-{}-{id}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        fs::write(&path, data).unwrap();
        path
    }

    fn sent(actions: &[XyModemAction]) -> Vec<&[u8]> {
        actions
            .iter()
            .filter_map(|action| match action {
                XyModemAction::SendToRemote(data) => Some(data.as_slice()),
                XyModemAction::EmitEvent(_) => None,
            })
            .collect()
    }

    fn has_complete(actions: &[XyModemAction]) -> bool {
        actions.iter().any(|action| {
            matches!(
                action,
                XyModemAction::EmitEvent(SerialModemEvent::Complete { .. })
            )
        })
    }

    #[test]
    fn serial_modem_events_serialize_for_frontend_listener() {
        let value = serde_json::to_value(SerialModemEvent::Progress {
            protocol: SerialModemUploadProtocol::Ymodem,
            file_name: "firmware.bin".to_string(),
            file_index: 2,
            bytes_transferred: 128,
            total_size: 1024,
        })
        .unwrap();

        assert_eq!(
            value,
            serde_json::json!({
                "type": "progress",
                "protocol": "ymodem",
                "fileName": "firmware.bin",
                "fileIndex": 2,
                "bytesTransferred": 128,
                "totalSize": 1024,
            })
        );

        let file_complete = serde_json::to_value(SerialModemEvent::FileComplete {
            protocol: SerialModemUploadProtocol::Ymodem,
            file_index: 2,
            file_name: "firmware.bin".to_string(),
        })
        .unwrap();
        assert_eq!(
            file_complete,
            serde_json::json!({
                "type": "fileComplete",
                "protocol": "ymodem",
                "fileIndex": 2,
                "fileName": "firmware.bin",
            })
        );
    }

    #[test]
    fn xymodem_xmodem_supports_checksum_crc_retry_and_eot() {
        for handshake in [NAK, CRC_REQUEST] {
            let path = test_file("firmware.bin", b"hello");
            let (mut transfer, _) = XyModemTransfer::new_at(
                SerialModemUploadProtocol::Xmodem,
                vec![path],
                Instant::now(),
            )
            .unwrap();
            let first = transfer.feed_incoming(&[handshake]);
            let first_packet = sent(&first)[0].to_vec();
            assert_eq!(first_packet[0], SOH);
            assert_eq!(first_packet[1], 1);
            assert_eq!(first_packet[2], !1u8);
            assert_eq!(first_packet.len(), if handshake == NAK { 132 } else { 133 });
            let payload: &[u8; BLOCK_SIZE] = first_packet[3..3 + BLOCK_SIZE].try_into().unwrap();
            if handshake == NAK {
                assert_eq!(
                    first_packet[3 + BLOCK_SIZE],
                    payload
                        .iter()
                        .fold(0u8, |sum, byte| sum.wrapping_add(*byte))
                );
            } else {
                assert_eq!(
                    &first_packet[3 + BLOCK_SIZE..],
                    &crc16_xmodem(payload).to_be_bytes()
                );
            }

            let retry = transfer.feed_incoming(&[NAK]);
            assert_eq!(sent(&retry)[0], first_packet);
            let next = transfer.feed_incoming(&[ACK]);
            assert_eq!(sent(&next)[0], &[EOT]);
            let complete = transfer.feed_incoming(&[ACK]);
            assert!(has_complete(&complete));
            assert!(transfer.is_done());
        }
    }

    #[test]
    fn xymodem_repeated_ack_does_not_confirm_unsent_next_block() {
        let data = vec![0x5a; BLOCK_SIZE * 3];
        let path = test_file("repeated-ack.bin", &data);
        let now = Instant::now();
        let (mut transfer, _) =
            XyModemTransfer::new_at(SerialModemUploadProtocol::Xmodem, vec![path], now).unwrap();

        let first = transfer.feed_incoming_at(&[CRC_REQUEST], now);
        assert_eq!(sent(&first)[0][1], 1);

        let result = transfer.feed_incoming_result_at(&[ACK, ACK], now + Duration::from_millis(1));
        let queued = sent(&result.actions);
        assert_eq!(result.consumed, 1);
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0][1], 2);
        assert!(result.actions.iter().any(|action| matches!(
            action,
            XyModemAction::EmitEvent(SerialModemEvent::Progress {
                bytes_transferred,
                ..
            }) if *bytes_transferred == BLOCK_SIZE as u64
        )));

        let next = transfer.feed_incoming_at(&[ACK], now + Duration::from_millis(2));
        assert_eq!(sent(&next)[0][1], 3);
    }

    #[test]
    fn xymodem_ack_at_response_deadline_does_not_queue_old_retry() {
        let data = vec![0x33; BLOCK_SIZE * 2];
        let path = test_file("deadline-ack.bin", &data);
        let now = Instant::now();
        let (mut transfer, _) =
            XyModemTransfer::new_at(SerialModemUploadProtocol::Xmodem, vec![path], now).unwrap();

        let first = transfer.feed_incoming_at(&[CRC_REQUEST], now);
        let first_packet = sent(&first)[0].to_vec();
        let result = transfer
            .feed_incoming_result_at(&[ACK], now + RESPONSE_TIMEOUT + Duration::from_millis(1));
        let queued = sent(&result.actions);

        assert_eq!(queued.len(), 1);
        assert_ne!(queued[0], first_packet);
        assert_eq!(queued[0][1], 2);
    }

    #[test]
    fn serial_modem_xmodem_rejects_batches_and_remote_cancel() {
        let first = test_file("first.bin", b"1");
        let second = test_file("second.bin", b"2");
        assert!(
            XyModemTransfer::new(
                SerialModemUploadProtocol::Xmodem,
                vec![first.clone(), second]
            )
            .is_err()
        );

        let (mut transfer, _) =
            XyModemTransfer::new(SerialModemUploadProtocol::Xmodem, vec![first]).unwrap();
        let actions = transfer.feed_incoming(&[CAN, CAN]);
        assert!(actions.iter().any(|action| matches!(
            action,
            XyModemAction::EmitEvent(SerialModemEvent::Failed { reason, .. })
                if reason.contains("cancelled")
        )));
        assert!(transfer.is_done());
    }

    #[test]
    fn xymodem_preserves_terminal_suffix_after_final_ack() {
        let path = test_file("final-ack.bin", b"data");
        let (mut transfer, _) =
            XyModemTransfer::new(SerialModemUploadProtocol::Xmodem, vec![path]).unwrap();
        let _ = transfer.feed_incoming(&[CRC_REQUEST]);
        let eot = transfer.feed_incoming(&[ACK]);
        assert_eq!(sent(&eot)[0], &[EOT]);

        let input = b"\x06device> ";
        let result = transfer.feed_incoming_result(input);
        assert_eq!(result.consumed, 1);
        assert!(has_complete(&result.actions));
        assert_eq!(&input[result.consumed..], b"device> ");
    }

    #[test]
    fn xymodem_preserves_terminal_suffix_after_remote_cancel() {
        let path = test_file("cancel-suffix.bin", b"data");
        let (mut transfer, _) =
            XyModemTransfer::new(SerialModemUploadProtocol::Xmodem, vec![path]).unwrap();

        let input = b"\x18\x18cancelled\r\n";
        let result = transfer.feed_incoming_result(input);
        assert_eq!(result.consumed, 2);
        assert!(result.actions.iter().any(|action| matches!(
            action,
            XyModemAction::EmitEvent(SerialModemEvent::Failed { reason, .. })
                if reason.contains("cancelled")
        )));
        assert_eq!(&input[result.consumed..], b"cancelled\r\n");
    }

    #[test]
    fn xymodem_handshake_timeout_aborts_transfer() {
        let path = test_file("timeout.bin", b"data");
        let now = Instant::now();
        let (mut transfer, _) =
            XyModemTransfer::new_at(SerialModemUploadProtocol::Xmodem, vec![path], now).unwrap();
        let actions = transfer.tick_at(now + HANDSHAKE_TIMEOUT + Duration::from_millis(1));
        assert!(actions.iter().any(|action| matches!(
            action,
            XyModemAction::EmitEvent(SerialModemEvent::Failed { reason, .. })
                if reason.contains("Timed out")
        )));
        assert!(transfer.is_done());
    }

    #[test]
    fn xymodem_handshake_timeout_preserves_current_read() {
        let path = test_file("timeout-suffix.bin", b"data");
        let now = Instant::now();
        let (mut transfer, _) =
            XyModemTransfer::new_at(SerialModemUploadProtocol::Xmodem, vec![path], now).unwrap();
        let input = b"device ready\r\n";
        let result = transfer
            .feed_incoming_result_at(input, now + HANDSHAKE_TIMEOUT + Duration::from_millis(1));

        assert_eq!(result.consumed, 0);
        assert!(transfer.is_done());
        assert_eq!(&input[result.consumed..], input);
    }

    #[test]
    fn xymodem_response_timeout_retries_then_aborts_transfer() {
        let path = test_file("retry-timeout.bin", b"data");
        let mut now = Instant::now();
        let (mut transfer, _) =
            XyModemTransfer::new_at(SerialModemUploadProtocol::Xmodem, vec![path], now).unwrap();
        let first = transfer.feed_incoming_at(&[CRC_REQUEST], now);
        let first_packet = sent(&first)[0].to_vec();

        for _ in 0..MAX_RETRIES {
            now += RESPONSE_TIMEOUT + Duration::from_millis(1);
            let retry = transfer.tick_at(now);
            assert_eq!(sent(&retry)[0], first_packet);
            assert!(!transfer.is_done());
        }

        now += RESPONSE_TIMEOUT + Duration::from_millis(1);
        let failed = transfer.tick_at(now);
        assert!(failed.iter().any(|action| matches!(
            action,
            XyModemAction::EmitEvent(SerialModemEvent::Failed { reason, .. })
                if reason.contains("did not acknowledge")
        )));
        assert!(transfer.is_done());
    }

    #[test]
    fn xymodem_ymodem_sends_basename_size_batches_and_final_empty_header() {
        let first = test_file("first.bin", b"abc");
        let second = test_file("second.bin", b"defg");
        let (mut transfer, _) =
            XyModemTransfer::new(SerialModemUploadProtocol::Ymodem, vec![first, second]).unwrap();

        let header = sent(&transfer.feed_incoming(&[CRC_REQUEST]))[0].to_vec();
        assert_eq!(&header[3..13], b"first.bin\0");
        assert_eq!(header[13], b'3');
        assert!(!String::from_utf8_lossy(&header).contains("nyaterm-xymodem"));
        let data = sent(&transfer.feed_incoming(&[ACK, CRC_REQUEST]))[0].to_vec();
        assert_eq!(data[1], 1);
        assert_eq!(sent(&transfer.feed_incoming(&[ACK]))[0], &[EOT]);
        assert_eq!(sent(&transfer.feed_incoming(&[NAK]))[0], &[EOT]);

        let second_header_actions = transfer.feed_incoming(&[ACK, CRC_REQUEST]);
        assert!(second_header_actions.iter().any(|action| matches!(
            action,
            XyModemAction::EmitEvent(SerialModemEvent::FileComplete {
                file_index: 0,
                file_name,
                ..
            }) if file_name == "first.bin"
        )));
        let second_header = sent(&second_header_actions)[0].to_vec();
        assert!(String::from_utf8_lossy(&second_header[3..]).starts_with("second.bin\0"));
        let _ = transfer.feed_incoming(&[ACK, CRC_REQUEST]);
        assert_eq!(sent(&transfer.feed_incoming(&[ACK]))[0], &[EOT]);

        let final_header = sent(&transfer.feed_incoming(&[ACK, CRC_REQUEST]))[0].to_vec();
        assert!(
            final_header[3..3 + BLOCK_SIZE]
                .iter()
                .all(|byte| *byte == 0)
        );
        let complete = transfer.feed_incoming(&[ACK]);
        assert!(has_complete(&complete));
        assert!(transfer.is_done());
    }
}
