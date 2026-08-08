pub struct ZmodemTransfer {
    #[allow(dead_code)]
    direction: ZmodemDirection,
    state: TransferState,
    /// Count of consecutive CAN bytes seen — 5 in a row means abort.
    cancel_count: usize,
    file_count: u32,
    progress_throttle: ProgressThrottle,
    send_buf: Vec<u8>,
}

enum TransferState {
    /// Waiting for the frontend to provide save path / file paths.
    WaitingForUser {
        /// Raw bytes buffered while waiting for the user to pick files.
        buffered: Vec<u8>,
    },
    /// Actively receiving files (download / remote `sz`).
    Receiving {
        receiver: zmodem2::Receiver,
        save_dir: PathBuf,
        current_file: Option<ReceiveFile>,
        stats: TransferStats,
    },
    /// Actively sending files (upload / remote `rz`).
    Sending {
        sender: zmodem2::Sender,
        files: Vec<PathBuf>,
        file_index: usize,
        current_file: Option<SendFile>,
        preserve_timestamps: bool,
        stats: TransferStats,
    },
    /// Transfer finished or aborted.
    Done,
}

struct ReceiveFile {
    name: String,
    path: PathBuf,
    size: u64,
    file: BufWriter<std::fs::File>,
    written: u64,
}

struct SendFile {
    name: String,
    size: u64,
    file: std::fs::File,
    sent: u64,
    position: u64,
}

struct ProgressThrottle {
    last_emit_at: Option<Instant>,
    last_emit_bytes: u64,
}

impl ProgressThrottle {
    fn new() -> Self {
        Self {
            last_emit_at: None,
            last_emit_bytes: 0,
        }
    }

    fn reset(&mut self) {
        self.last_emit_at = None;
        self.last_emit_bytes = 0;
    }

    fn should_emit(&mut self, bytes_transferred: u64, force: bool) -> bool {
        self.should_emit_at(bytes_transferred, force, Instant::now())
    }

    fn should_emit_at(&mut self, bytes_transferred: u64, force: bool, now: Instant) -> bool {
        if force
            || self.last_emit_at.is_none()
            || bytes_transferred.saturating_sub(self.last_emit_bytes) >= ZMODEM_PROGRESS_BYTES
            || self
                .last_emit_at
                .is_some_and(|last| now.duration_since(last) >= ZMODEM_PROGRESS_INTERVAL)
        {
            self.last_emit_at = Some(now);
            self.last_emit_bytes = bytes_transferred;
            return true;
        }

        false
    }
}

struct TransferStats {
    started_at: Instant,
    bytes: u64,
    ack_count: u64,
    file_write_count: u64,
}

impl TransferStats {
    fn new() -> Self {
        Self {
            started_at: Instant::now(),
            bytes: 0,
            ack_count: 0,
            file_write_count: 0,
        }
    }

    fn elapsed_secs(&self) -> f64 {
        self.started_at.elapsed().as_secs_f64()
    }

    fn mbps(&self) -> f64 {
        let elapsed = self.elapsed_secs();
        if elapsed <= f64::EPSILON {
            0.0
        } else {
            self.bytes as f64 / 1024.0 / 1024.0 / elapsed
        }
    }
}

#[derive(Default)]
pub struct ZmodemUploadDrain {
    suppress_until: Option<Instant>,
}

impl ZmodemUploadDrain {
    pub fn new() -> Self {
        Self {
            suppress_until: None,
        }
    }

    pub fn start(&mut self, now: Instant) {
        self.suppress_until = now.checked_add(ZMODEM_FINISH_DRAIN_IDLE);
    }

    pub fn is_active(&self) -> bool {
        self.suppress_until.is_some()
    }

    pub fn should_suppress(&mut self, now: Instant) -> bool {
        let Some(until) = self.suppress_until else {
            return false;
        };

        if now <= until {
            self.start(now);
            return true;
        }

        self.suppress_until = None;
        false
    }

    pub fn filter<'a>(&mut self, data: &'a [u8], now: Instant) -> &'a [u8] {
        let Some(until) = self.suppress_until else {
            return data;
        };

        if now > until {
            self.suppress_until = None;
            return data;
        }

        if looks_like_terminal_text(data) {
            self.suppress_until = None;
            return data;
        }

        self.start(now);
        &data[data.len()..]
    }
}

#[derive(Default)]
pub struct ZmodemDownloadOoDrain {
    suppress_until: Option<Instant>,
    remaining_o: usize,
}

impl ZmodemDownloadOoDrain {
    pub fn new() -> Self {
        Self {
            suppress_until: None,
            remaining_o: 0,
        }
    }

    pub fn start(&mut self, now: Instant) {
        self.suppress_until = now.checked_add(ZMODEM_FINISH_DRAIN_IDLE);
        self.remaining_o = 2;
    }

    pub fn is_active(&self) -> bool {
        self.suppress_until.is_some()
    }

    pub fn filter<'a>(&mut self, data: &'a [u8], now: Instant) -> &'a [u8] {
        let Some(until) = self.suppress_until else {
            return data;
        };

        if now > until {
            self.suppress_until = None;
            self.remaining_o = 0;
            return data;
        }

        let mut offset = 0;
        while self.remaining_o > 0 && data.get(offset) == Some(&b'O') {
            offset += 1;
            self.remaining_o -= 1;
        }

        if self.remaining_o == 0 || offset < data.len() {
            self.suppress_until = None;
            self.remaining_o = 0;
        }

        &data[offset..]
    }
}

impl ZmodemTransfer {
    pub fn new(direction: ZmodemDirection, initial_bytes: &[u8]) -> Self {
        Self {
            direction,
            state: TransferState::WaitingForUser {
                buffered: initial_bytes.to_vec(),
            },
            cancel_count: 0,
            file_count: 0,
            progress_throttle: ProgressThrottle::new(),
            send_buf: Vec::new(),
        }
    }

    #[allow(dead_code)]
    pub fn direction(&self) -> ZmodemDirection {
        self.direction
    }

    pub fn is_done(&self) -> bool {
        matches!(self.state, TransferState::Done)
    }

    /// Called when the user cancels the transfer from the frontend.
    pub fn cancel(&mut self) -> Vec<ZmodemAction> {
        self.state = TransferState::Done;
        vec![
            ZmodemAction::SendToRemote(cancel_sequence()),
            ZmodemAction::EmitEvent(ZmodemEvent::Failed {
                reason: "cancelled".to_string(),
            }),
        ]
    }

    /// Called when the user accepts a **download** and provides a save directory.
    pub fn accept_download(&mut self, save_dir: PathBuf) -> Vec<ZmodemAction> {
        let buffered = match &mut self.state {
            TransferState::WaitingForUser { buffered } => std::mem::take(buffered),
            _ => return vec![],
        };

        let receiver = match zmodem2::Receiver::new() {
            Ok(r) => r,
            Err(e) => {
                self.state = TransferState::Done;
                return vec![ZmodemAction::EmitEvent(ZmodemEvent::Failed {
                    reason: format!("Failed to create ZMODEM receiver: {e}"),
                })];
            }
        };

        self.state = TransferState::Receiving {
            receiver,
            save_dir,
            current_file: None,
            stats: TransferStats::new(),
        };

        let mut actions = self.drain_outgoing();
        // Process the buffered bytes that arrived before the user accepted.
        actions.extend(self.feed_incoming(&buffered));
        actions
    }

    /// Called when the user accepts an **upload** and provides file paths.
    pub fn accept_upload(
        &mut self,
        files: Vec<PathBuf>,
        conflict_mode: ZmodemUploadConflictMode,
        preserve_timestamps: bool,
    ) -> Vec<ZmodemAction> {
        let buffered = match &mut self.state {
            TransferState::WaitingForUser { buffered } => std::mem::take(buffered),
            _ => return vec![],
        };

        let mut sender = match zmodem2::Sender::new() {
            Ok(s) => s,
            Err(e) => {
                self.state = TransferState::Done;
                return vec![ZmodemAction::EmitEvent(ZmodemEvent::Failed {
                    reason: format!("Failed to create ZMODEM sender: {e}"),
                })];
            }
        };
        sender.set_file_options(conflict_mode.file_options());

        // Discard the auto-queued ZRQINIT: the remote `rz` already sent
        // ZRINIT proactively, so sending ZRQINIT back would confuse it.
        // We skip directly to feeding the buffered ZRINIT and starting the
        // first file.
        {
            let n = sender.drain_outgoing().len();
            sender.advance_outgoing(n);
        }

        self.state = TransferState::Sending {
            sender,
            files,
            file_index: 0,
            current_file: None,
            preserve_timestamps,
            stats: TransferStats::new(),
        };

        let mut actions = Vec::new();
        // 1. Feed the buffered remote ZRINIT so the Sender knows the
        //    receiver's capabilities.
        actions.extend(self.feed_incoming(&buffered));
        // 2. Start the first file (prepares ZFILE frame).
        actions.extend(self.start_next_send_file());
        // 3. Drain the ZFILE frame.
        actions.extend(self.drain_outgoing());
        actions
    }

    /// Feed raw bytes from the remote into the transfer state machine.
    pub fn feed_incoming(&mut self, data: &[u8]) -> Vec<ZmodemAction> {
        // Check for cancel sequence (5+ consecutive CAN/ZDLE bytes).
        for &b in data {
            if b == ZDLE {
                self.cancel_count += 1;
                if self.cancel_count >= CANCEL_SEQ_LEN {
                    self.state = TransferState::Done;
                    return vec![ZmodemAction::EmitEvent(ZmodemEvent::Failed {
                        reason: "Remote cancelled transfer".to_string(),
                    })];
                }
            } else {
                self.cancel_count = 0;
            }
        }

        match &mut self.state {
            TransferState::WaitingForUser { buffered } => {
                buffered.extend_from_slice(data);
                vec![]
            }
            TransferState::Receiving { .. } => self.feed_receiver(data),
            TransferState::Sending { .. } => self.feed_sender(data),
            TransferState::Done => vec![],
        }
    }

    fn feed_receiver(&mut self, data: &[u8]) -> Vec<ZmodemAction> {
        let mut actions = Vec::new();

        let TransferState::Receiving {
            receiver,
            save_dir,
            current_file,
            stats,
        } = &mut self.state
        else {
            return actions;
        };

        let mut offset = 0;
        while offset < data.len() {
            match receiver.feed_incoming(&data[offset..]) {
                Ok(consumed) => {
                    if consumed == 0 {
                        break;
                    }
                    offset += consumed;
                }
                Err(e) => {
                    tracing::warn!("ZMODEM receive error: {e}");
                    if matches!(
                        e,
                        zmodem2::Error::UnexpectedCrc16 | zmodem2::Error::UnexpectedCrc32
                    ) {
                        offset += 1;
                        continue;
                    }
                    self.state = TransferState::Done;
                    actions.push(ZmodemAction::EmitEvent(ZmodemEvent::Failed {
                        reason: format!("ZMODEM protocol error: {e}"),
                    }));
                    return actions;
                }
            }

            // Drain outgoing protocol bytes first.
            let out = receiver.drain_outgoing();
            if !out.is_empty() {
                let out = out.to_vec();
                let n = out.len();
                actions.push(ZmodemAction::SendToRemote(out));
                stats.ack_count += 1;
                receiver.advance_outgoing(n);
            }

            // Poll events — handle FileStart to create the output file.
            while let Some(event) = receiver.poll_event() {
                match event {
                    zmodem2::ReceiverEvent::FileStart => {
                        let name_raw = receiver.file_name();
                        let name = String::from_utf8_lossy(name_raw).to_string();
                        let name = sanitize_filename(&name);
                        let size = u64::from(receiver.file_size());

                        let file_path = save_dir.join(&name);
                        tracing::info!(
                            file = %file_path.display(),
                            size,
                            "ZMODEM receiving file"
                        );
                        match std::fs::File::create(&file_path) {
                            Ok(file) => {
                                self.progress_throttle.reset();
                                *current_file = Some(ReceiveFile {
                                    name: name.clone(),
                                    path: file_path.clone(),
                                    size,
                                    file: BufWriter::with_capacity(
                                        ZMODEM_FILE_WRITE_BUFFER_SIZE,
                                        file,
                                    ),
                                    written: 0,
                                });
                                if self.progress_throttle.should_emit(0, true) {
                                    actions.push(ZmodemAction::EmitEvent(ZmodemEvent::Progress {
                                        file_name: name,
                                        bytes_transferred: 0,
                                        total_size: size,
                                        local_path: Some(file_path.to_string_lossy().to_string()),
                                        direction: ZmodemDirection::Download,
                                    }));
                                }
                            }
                            Err(e) => {
                                tracing::error!(
                                    "Failed to create file {}: {e}",
                                    file_path.display()
                                );
                                self.state = TransferState::Done;
                                actions.push(ZmodemAction::SendToRemote(cancel_sequence()));
                                actions.push(ZmodemAction::EmitEvent(ZmodemEvent::Failed {
                                    reason: format!("Failed to create file: {e}"),
                                }));
                                return actions;
                            }
                        }
                    }
                    zmodem2::ReceiverEvent::FileComplete => {
                        if let Some(rf) = current_file {
                            if self.progress_throttle.should_emit(rf.written, true) {
                                actions.push(ZmodemAction::EmitEvent(ZmodemEvent::Progress {
                                    file_name: rf.name.clone(),
                                    bytes_transferred: rf.written,
                                    total_size: rf.size,
                                    local_path: Some(rf.path.to_string_lossy().to_string()),
                                    direction: ZmodemDirection::Download,
                                }));
                            }
                            let _ = rf.file.flush();
                        }
                        self.file_count += 1;
                        *current_file = None;
                    }
                    zmodem2::ReceiverEvent::SessionComplete => {
                        log_transfer_complete(ZmodemDirection::Download, self.file_count, stats);
                        self.state = TransferState::Done;
                        actions.push(ZmodemAction::EmitEvent(ZmodemEvent::Complete {
                            direction: ZmodemDirection::Download,
                            file_count: self.file_count,
                        }));
                        return actions;
                    }
                }
            }

            // Drain file data and write to the output file.
            let file_data = receiver.drain_file();
            if !file_data.is_empty() {
                let len = file_data.len();

                if let Some(rf) = current_file {
                    if let Err(e) = rf.file.write_all(file_data) {
                        tracing::error!("Failed to write file data: {e}");
                        self.state = TransferState::Done;
                        actions.push(ZmodemAction::SendToRemote(cancel_sequence()));
                        actions.push(ZmodemAction::EmitEvent(ZmodemEvent::Failed {
                            reason: format!("File write error: {e}"),
                        }));
                        return actions;
                    }
                    rf.written += len as u64;
                    stats.bytes += len as u64;
                    stats.file_write_count += 1;
                    if self.progress_throttle.should_emit(rf.written, false) {
                        actions.push(ZmodemAction::EmitEvent(ZmodemEvent::Progress {
                            file_name: rf.name.clone(),
                            bytes_transferred: rf.written,
                            total_size: rf.size,
                            local_path: Some(rf.path.to_string_lossy().to_string()),
                            direction: ZmodemDirection::Download,
                        }));
                    }
                }

                if let Err(e) = receiver.advance_file(len) {
                    tracing::warn!("advance_file error: {e}");
                }
            }
        }

        actions
    }

    fn feed_sender(&mut self, data: &[u8]) -> Vec<ZmodemAction> {
        let mut actions = Vec::new();

        let TransferState::Sending {
            sender,
            files,
            file_index,
            current_file,
            preserve_timestamps,
            stats,
        } = &mut self.state
        else {
            return actions;
        };

        let mut offset = 0;
        while offset < data.len() {
            match sender.feed_incoming(&data[offset..]) {
                Ok(consumed) => {
                    if consumed == 0 {
                        break;
                    }
                    offset += consumed;
                }
                Err(e) => {
                    tracing::warn!("ZMODEM send error: {e}");
                    if matches!(
                        e,
                        zmodem2::Error::UnexpectedCrc16 | zmodem2::Error::UnexpectedCrc32
                    ) {
                        offset += 1;
                        continue;
                    }
                    self.state = TransferState::Done;
                    actions.push(ZmodemAction::EmitEvent(ZmodemEvent::Failed {
                        reason: format!("ZMODEM send error: {e}"),
                    }));
                    return actions;
                }
            }
        }

        // Drain any outgoing protocol responses before fulfilling file requests.
        drain_sender_outgoing(sender, &mut actions);

        // Fulfill file data requests from the sender state machine.
        // Drain outgoing after each feed_file() to prevent buffer overflow
        // in the no_std fixed-capacity internal buffer.
        while let Some(req) = sender.poll_file() {
            if let Some(sf) = current_file {
                let requested_offset = u64::from(req.offset);
                if sf.position != requested_offset {
                    if let Err(e) = sf.file.seek(SeekFrom::Start(requested_offset)) {
                        tracing::warn!("File seek error: {e}");
                        break;
                    }
                    sf.position = requested_offset;
                }
                if self.send_buf.len() < req.len {
                    self.send_buf.resize(req.len, 0);
                }
                match sf.file.read(&mut self.send_buf[..req.len]) {
                    Ok(n) => {
                        if let Err(e) = sender.feed_file(&self.send_buf[..n]) {
                            tracing::warn!("feed_file error: {e}");
                            break;
                        }
                        stats.bytes += n as u64;
                        sf.sent = sf.sent.max(requested_offset + n as u64);
                        sf.position += n as u64;
                        if self.progress_throttle.should_emit(sf.sent, false) {
                            actions.push(ZmodemAction::EmitEvent(ZmodemEvent::Progress {
                                file_name: sf.name.clone(),
                                bytes_transferred: sf.sent,
                                total_size: sf.size,
                                local_path: None,
                                direction: ZmodemDirection::Upload,
                            }));
                        }
                        drain_sender_outgoing(sender, &mut actions);
                    }
                    Err(e) => {
                        tracing::warn!("File read error: {e}");
                        break;
                    }
                }
            }
        }

        // Poll events.
        while let Some(event) = sender.poll_event() {
            match event {
                zmodem2::SenderEvent::FileComplete => {
                    if let Some(sf) = current_file {
                        if self.progress_throttle.should_emit(sf.sent, true) {
                            actions.push(ZmodemAction::EmitEvent(ZmodemEvent::Progress {
                                file_name: sf.name.clone(),
                                bytes_transferred: sf.sent,
                                total_size: sf.size,
                                local_path: None,
                                direction: ZmodemDirection::Upload,
                            }));
                        }
                    }
                    self.file_count += 1;
                    *current_file = None;
                    *file_index += 1;
                    if *file_index < files.len() {
                        self.progress_throttle.reset();
                        actions.extend(Self::start_file_for_sender(
                            sender,
                            files,
                            *file_index,
                            current_file,
                            *preserve_timestamps,
                        ));
                        if let Some(sf) = current_file {
                            if self.progress_throttle.should_emit(0, true) {
                                actions.push(ZmodemAction::EmitEvent(ZmodemEvent::Progress {
                                    file_name: sf.name.clone(),
                                    bytes_transferred: 0,
                                    total_size: sf.size,
                                    local_path: None,
                                    direction: ZmodemDirection::Upload,
                                }));
                            }
                        }
                    } else if let Err(e) = sender.finish_session() {
                        tracing::warn!("finish_session error: {e}");
                    }
                }
                zmodem2::SenderEvent::SessionComplete => {
                    log_transfer_complete(ZmodemDirection::Upload, self.file_count, stats);
                    self.state = TransferState::Done;
                    actions.push(ZmodemAction::EmitEvent(ZmodemEvent::Complete {
                        direction: ZmodemDirection::Upload,
                        file_count: self.file_count,
                    }));
                    return actions;
                }
            }
        }

        // Final drain.
        drain_sender_outgoing(sender, &mut actions);

        actions
    }

    fn start_next_send_file(&mut self) -> Vec<ZmodemAction> {
        let TransferState::Sending {
            sender,
            files,
            file_index,
            current_file,
            preserve_timestamps,
            ..
        } = &mut self.state
        else {
            return vec![];
        };

        if *file_index >= files.len() {
            return vec![];
        }

        self.progress_throttle.reset();
        let mut actions = Self::start_file_for_sender(
            sender,
            files,
            *file_index,
            current_file,
            *preserve_timestamps,
        );
        if let Some(sf) = current_file {
            if self.progress_throttle.should_emit(0, true) {
                actions.push(ZmodemAction::EmitEvent(ZmodemEvent::Progress {
                    file_name: sf.name.clone(),
                    bytes_transferred: 0,
                    total_size: sf.size,
                    local_path: None,
                    direction: ZmodemDirection::Upload,
                }));
            }
        }
        actions
    }

    fn start_file_for_sender(
        sender: &mut zmodem2::Sender,
        files: &[PathBuf],
        index: usize,
        current_file: &mut Option<SendFile>,
        preserve_timestamps: bool,
    ) -> Vec<ZmodemAction> {
        let path = &files[index];
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file")
            .to_string();

        let file = match std::fs::File::open(path) {
            Ok(f) => f,
            Err(e) => {
                return vec![ZmodemAction::EmitEvent(ZmodemEvent::Failed {
                    reason: format!("Failed to open {file_name}: {e}"),
                })];
            }
        };

        let metadata = match file.metadata() {
            Ok(m) => m,
            Err(e) => {
                return vec![ZmodemAction::EmitEvent(ZmodemEvent::Failed {
                    reason: format!("Failed to read metadata for {file_name}: {e}"),
                })];
            }
        };

        let size = metadata.len();
        // zmodem2 uses u32 for file size
        let size_u32 = u32::try_from(size).unwrap_or(u32::MAX);
        let mtime = zmodem_mtime_from_metadata(&metadata, preserve_timestamps);

        if let Err(e) =
            sender.start_file_with_metadata(file_name.as_bytes(), size_u32, mtime, 0o100644)
        {
            return vec![ZmodemAction::EmitEvent(ZmodemEvent::Failed {
                reason: format!("start_file error: {e}"),
            })];
        }

        *current_file = Some(SendFile {
            name: file_name,
            size,
            file,
            sent: 0,
            position: 0,
        });

        vec![]
    }

    fn drain_outgoing(&mut self) -> Vec<ZmodemAction> {
        let mut actions = Vec::new();
        match &mut self.state {
            TransferState::Receiving { receiver, .. } => {
                let out = receiver.drain_outgoing();
                if !out.is_empty() {
                    let out = out.to_vec();
                    let n = out.len();
                    actions.push(ZmodemAction::SendToRemote(out));
                    receiver.advance_outgoing(n);
                }
            }
            TransferState::Sending { sender, .. } => {
                let out = sender.drain_outgoing();
                if !out.is_empty() {
                    let out = out.to_vec();
                    let n = out.len();
                    actions.push(ZmodemAction::SendToRemote(out));
                    sender.advance_outgoing(n);
                }
            }
            _ => {}
        }
        actions
    }
}

/// Drain the Sender's outgoing buffer into actions, advancing the cursor.
fn drain_sender_outgoing(sender: &mut zmodem2::Sender, actions: &mut Vec<ZmodemAction>) {
    let out = sender.drain_outgoing();
    if !out.is_empty() {
        let out = out.to_vec();
        let n = out.len();
        actions.push(ZmodemAction::SendToRemote(out));
        sender.advance_outgoing(n);
    }
}

fn zmodem_mtime_from_metadata(metadata: &std::fs::Metadata, preserve_timestamps: bool) -> u32 {
    if !preserve_timestamps {
        return 0;
    }

    zmodem_mtime_from_system_time(metadata.modified())
}

fn zmodem_mtime_from_system_time(modified: std::io::Result<SystemTime>) -> u32 {
    modified
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .and_then(|duration| u32::try_from(duration.as_secs()).ok())
        .unwrap_or(0)
}

fn log_transfer_complete(direction: ZmodemDirection, file_count: u32, stats: &TransferStats) {
    tracing::info!(
        ?direction,
        file_count,
        bytes = stats.bytes,
        elapsed_secs = stats.elapsed_secs(),
        mb_per_sec = stats.mbps(),
        ack_count = stats.ack_count,
        file_write_count = stats.file_write_count,
        "ZMODEM transfer complete"
    );
}

fn looks_like_terminal_text(data: &[u8]) -> bool {
    if data.is_empty() || std::str::from_utf8(data).is_err() {
        return false;
    }

    data.iter().all(|&byte| {
        matches!(
            byte,
            b'\t' | b'\n' | b'\r' | 0x07 | 0x08 | 0x1b | 0x20..=0x7e | 0x80..=0xff
        )
    })
}

