use super::TrzszUploadEvent;
use super::{
    TrzszDownloadDataState, TrzszDownloadEngine, TrzszDownloadError, TrzszDownloadEvent,
    TrzszDownloadPhase, TrzszDownloadStep, TrzszProtocolFrame, TrzszProtocolPayload,
    TrzszUploadEngine, TrzszUploadError, TrzszUploadPhase, TrzszUploadReader, TrzszUploadStep,
    build_trzsz_integer_frame, build_trzsz_string_frame, codec, expect_upload_ack, resume,
    upload_string_payload,
};

impl TrzszDownloadEngine {
    pub(super) fn observe_modern_data(
        &mut self,
        frame: TrzszProtocolFrame,
        state: TrzszDownloadDataState,
    ) -> Result<TrzszDownloadStep, TrzszDownloadError> {
        let mut events = Vec::new();
        let mut step = self.observe_modern_data_into(frame, state, &mut |event| {
            events.push(event);
            Ok(())
        })?;
        step.events = events;
        Ok(step)
    }

    pub(super) fn observe_modern_data_into(
        &mut self,
        frame: TrzszProtocolFrame,
        mut state: TrzszDownloadDataState,
        sink: &mut impl FnMut(TrzszDownloadEvent) -> Result<(), String>,
    ) -> Result<TrzszDownloadStep, TrzszDownloadError> {
        let invalid = || TrzszDownloadError::InvalidPayload {
            frame_type: frame.frame_type.clone(),
        };
        if frame.payload == TrzszProtocolPayload::Raw("=".into()) && self.protocol_version >= 3 {
            self.phase = state.into_phase();
            return Ok(TrzszDownloadStep::default());
        }
        if self.awaiting_compression {
            let compress = match (&*frame.frame_type, &frame.payload) {
                ("COMP", TrzszProtocolPayload::Raw(flag)) if flag == "true" => true,
                ("COMP", TrzszProtocolPayload::Raw(flag)) if flag == "false" => false,
                _ => return Err(invalid()),
            };
            self.decoder =
                Some(codec::Decoder::new(&self.config, compress).map_err(|_| invalid())?);
            self.awaiting_compression = false;
            self.phase = state.into_phase();
            return Ok(TrzszDownloadStep::default());
        }
        if frame.frame_type != "DATA" {
            return Err(invalid());
        }
        if let TrzszProtocolPayload::Integer(length) = frame.payload {
            if length < 0 || state.expected_binary_chunk.is_some() {
                return Err(invalid());
            }
            if length > 0 {
                state.expected_binary_chunk = Some(length);
                self.phase = state.into_phase();
                return Ok(TrzszDownloadStep::default());
            }
        }
        let (bytes, end) = match frame.payload {
            TrzszProtocolPayload::Raw(text) => (text.into_bytes(), true),
            TrzszProtocolPayload::EncodedBytes(bytes) => (bytes, true),
            TrzszProtocolPayload::Integer(0) => (Vec::new(), true),
            TrzszProtocolPayload::DataChunk { bytes, end } => (bytes, end),
            _ => return Err(invalid()),
        };
        self.wire_received = self
            .wire_received
            .checked_add(bytes.len())
            .ok_or_else(invalid)?;
        if let Some(expected) = state.expected_binary_chunk {
            if self.wire_received as i64 > expected
                || (end && self.wire_received as i64 != expected)
            {
                return Err(invalid());
            }
            if end {
                state.expected_binary_chunk = None;
            }
        }
        let finish = end && self.wire_received == 0;
        let archive = &mut self.archive;
        self.decoder
            .as_mut()
            .ok_or_else(invalid)?
            .decode(&bytes, finish, |data| {
                if data.is_empty() {
                    return Ok(());
                }
                if state.received + data.len() as i64 > state.size {
                    return Err("trzsz data exceeds declared size".into());
                }
                state.md5.consume(data);
                state.received += data.len() as i64;
                if let Some(archive) = archive {
                    archive.push(data, sink)?;
                } else {
                    sink(TrzszDownloadEvent::Data {
                        name: state.name.clone(),
                        bytes: data.to_vec(),
                        received: state.received,
                        size: state.size,
                    })?;
                }
                Ok(())
            })
            .map_err(|message| TrzszDownloadError::DataDecode { message })?;
        if !end {
            self.phase = state.into_phase();
            return Ok(TrzszDownloadStep::default());
        }
        let wire_length = std::mem::take(&mut self.wire_received);
        let mut responses = vec![
            format!("#SUCC:{}/{}{}", wire_length, state.received, self.newline()).into_bytes(),
        ];
        if finish {
            if state.received != state.size {
                return Err(invalid());
            }
            if let Some(archive) = self.archive.take() {
                archive.finish().map_err(|_| invalid())?;
            }
            responses.push(build_trzsz_integer_frame(
                "SUCC",
                state.received,
                self.newline(),
            ));
            self.phase = TrzszDownloadPhase::AwaitMd5 {
                remaining: state.remaining,
                names: state.names,
                name: state.name,
                digest: state.md5.finalize().0.to_vec(),
            };
        } else {
            self.phase = state.into_phase();
        }
        Ok(TrzszDownloadStep {
            events: Vec::new(),
            responses,
        })
    }
}

impl TrzszUploadEngine {
    pub(super) fn encode_modern_data(
        &mut self,
        bytes: &[u8],
        finish: bool,
    ) -> Result<Vec<Vec<u8>>, TrzszUploadError> {
        let encoded = self
            .encoder
            .as_mut()
            .expect("configured DATA encoder")
            .encode(bytes, finish)
            .map_err(|message| TrzszUploadError::PayloadRead {
                name: "trzsz stream".into(),
                message,
            })?;
        let cap = self
            .config
            .max_buf_size
            .unwrap_or(256 * 1024)
            .clamp(1, 256 * 1024) as usize;
        let mut responses = Vec::new();
        for data in encoded.chunks(cap) {
            let mut wire = if self.config.binary {
                format!("#DATA:{}{}", data.len(), self.newline()).into_bytes()
            } else {
                b"#DATA:".to_vec()
            };
            wire.extend(data);
            if !self.config.binary {
                wire.extend(self.newline().as_bytes());
            }
            self.wire_acks.push_back(data.len() as i64);
            responses.push(wire);
        }
        if finish {
            self.wire_acks.push_back(0);
            responses.push(if self.config.binary {
                format!("#DATA:0{}", self.newline()).into_bytes()
            } else {
                format!("#DATA:{}", self.newline()).into_bytes()
            });
        }
        Ok(responses)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn observe_modern_ack(
        &mut self,
        frame: TrzszProtocolFrame,
        index: usize,
        remote_names: Vec<String>,
        remote_name: String,
        sent: i64,
        chunk: i64,
        md5: md5::Context,
        reader: TrzszUploadReader,
    ) -> Result<TrzszUploadStep, TrzszUploadError> {
        let invalid = || TrzszUploadError::InvalidPayload {
            frame_type: frame.frame_type.clone(),
        };
        let TrzszProtocolPayload::Raw(text) = &frame.payload else {
            return Err(invalid());
        };
        let (length, saved) = text.split_once('/').ok_or_else(invalid)?;
        let length: i64 = length.parse().map_err(|_| invalid())?;
        let saved: i64 = saved.parse().map_err(|_| invalid())?;
        if frame.frame_type != "SUCC"
            || self.wire_acks.pop_front() != Some(length)
            || !(self.acknowledged..=sent).contains(&saved)
        {
            return Err(invalid());
        }
        self.acknowledged = saved;
        let events = vec![TrzszUploadEvent::Data {
            name: self.entries[index].name.clone(),
            sent: saved + self.offset,
            size: self.entries[index].size,
        }];
        if !self.wire_acks.is_empty() {
            if sent < self.entries[index].size - self.offset && self.wire_acks.len() < 5 {
                let mut step = self.send_upload_data_chunk(
                    index,
                    remote_names,
                    remote_name,
                    sent,
                    md5,
                    reader,
                )?;
                step.events.splice(0..0, events);
                return Ok(step);
            }
            self.phase = TrzszUploadPhase::AwaitDataAck {
                index,
                remote_names,
                remote_name,
                sent,
                chunk,
                md5,
                reader,
            };
            return Ok(TrzszUploadStep {
                events,
                responses: Vec::new(),
            });
        }
        if sent < self.entries[index].size - self.offset {
            let mut step =
                self.send_upload_data_chunk(index, remote_names, remote_name, sent, md5, reader)?;
            step.events.splice(0..0, events);
            return Ok(step);
        }
        self.phase = TrzszUploadPhase::AwaitFinalAck {
            index,
            remote_names,
            digest: md5.finalize().0.to_vec(),
        };
        Ok(TrzszUploadStep {
            events,
            responses: Vec::new(),
        })
    }

    pub(super) fn observe_final_ack(
        &mut self,
        frame: TrzszProtocolFrame,
        index: usize,
        remote_names: Vec<String>,
        digest: Vec<u8>,
    ) -> Result<TrzszUploadStep, TrzszUploadError> {
        let size = self.entries[index].size - self.offset;
        if let TrzszProtocolPayload::Integer(saved) = frame.payload
            && frame.frame_type == "SUCC"
            && (self.acknowledged..size).contains(&saved)
        {
            self.acknowledged = saved;
            self.phase = TrzszUploadPhase::AwaitFinalAck {
                index,
                remote_names,
                digest,
            };
            return Ok(TrzszUploadStep::default());
        }
        expect_upload_ack(&frame, &TrzszProtocolPayload::Integer(size))?;
        let response = build_trzsz_string_frame("MD5", &digest, self.newline());
        self.phase = TrzszUploadPhase::AwaitMd5Ack {
            index,
            remote_names,
            digest,
        };
        Ok(TrzszUploadStep {
            events: Vec::new(),
            responses: vec![response],
        })
    }

    pub(super) fn observe_prefix_ack(
        &mut self,
        frame: TrzszProtocolFrame,
        index: usize,
        remote_names: Vec<String>,
        remote_name: String,
        mut prefix: resume::PrefixSender,
    ) -> Result<TrzszUploadStep, TrzszUploadError> {
        let invalid = || TrzszUploadError::InvalidPayload {
            frame_type: "SUCC".into(),
        };
        let ack: resume::PrefixAck =
            serde_json::from_str(&upload_string_payload(&frame)?).map_err(|_| invalid())?;
        if frame.frame_type != "SUCC" || ack.step != prefix.step {
            return Err(invalid());
        }
        if ack.r#match {
            prefix.matched = ack.step;
        }
        if ack.r#match && prefix.step < prefix.limit {
            let hash = prefix.next(&self.entries[index])?;
            let response = build_trzsz_string_frame(
                "HASH",
                &serde_json::to_vec(&hash).expect("prefix"),
                self.newline(),
            );
            self.phase = TrzszUploadPhase::AwaitPrefix {
                index,
                remote_names,
                remote_name,
                prefix,
            };
            return Ok(TrzszUploadStep {
                events: Vec::new(),
                responses: vec![response],
            });
        }
        self.offset = prefix.matched;
        let over = resume::PrefixHash {
            step: 0,
            hash: String::new(),
            over: true,
        };
        let size = self.entries[index].size - self.offset;
        let responses = vec![
            build_trzsz_string_frame(
                "HASH",
                &serde_json::to_vec(&over).expect("prefix"),
                self.newline(),
            ),
            build_trzsz_integer_frame("SIZE", size, self.newline()),
        ];
        self.phase = TrzszUploadPhase::AwaitSizeAck {
            index,
            remote_names,
            remote_name,
        };
        Ok(TrzszUploadStep {
            events: Vec::new(),
            responses,
        })
    }
}
