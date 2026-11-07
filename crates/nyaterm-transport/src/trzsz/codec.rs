//! Streaming DATA encoding used by trzsz-go v1.2.0 (protocols 2–4).
use std::io::{Read, Seek, SeekFrom, Write};

use base64::Engine as _;
use zstd::stream::raw::Operation;

use super::TrzszConfig;

pub(super) fn compression(config: &TrzszConfig, size: i64) -> Option<bool> {
    if config.protocol.unwrap_or(0) < 3 {
        Some(!config.binary)
    } else {
        match config.compress.unwrap_or(0) {
            1 => Some(true),
            2 => Some(false),
            _ if size < 512 => Some(false),
            _ if size < 128 * 1024 => Some(true),
            _ => None,
        }
    }
}

/// Sample a bounded amount at the start, end and middle, restoring the resume
/// offset afterwards. Already compressed files should avoid Zstd CPU overhead.
pub(super) fn compression_profitable(
    reader: &mut (impl Read + Seek),
    size: i64,
) -> Result<bool, String> {
    const SAMPLE: i64 = 128 * 1024;
    let position = reader
        .stream_position()
        .map_err(|error| error.to_string())?;
    let result = (|| {
        let mut incompressible = 0;
        let mut sample = vec![0; SAMPLE as usize];
        let offsets = [0, size - SAMPLE, size / 2 - SAMPLE / 2];
        for (index, offset) in offsets.into_iter().enumerate() {
            if size < (index as i64 + 1) * SAMPLE {
                continue;
            }
            reader
                .seek(SeekFrom::Start(position + offset as u64))
                .map_err(|error| error.to_string())?;
            reader
                .read_exact(&mut sample)
                .map_err(|error| error.to_string())?;
            let encoded = zstd::bulk::compress(&sample, 1).map_err(|error| error.to_string())?;
            if encoded.len() > sample.len() * 98 / 100 {
                incompressible += 1;
            }
        }
        Ok(incompressible < 2)
    })();
    reader
        .seek(SeekFrom::Start(position))
        .map_err(|error| error.to_string())?;
    result
}

type EscapeTable = [Option<u8>; 256];

fn escape_table(config: &TrzszConfig) -> Result<(EscapeTable, EscapeTable), String> {
    let mut encode = [None; 256];
    let mut decode = [None; 256];
    for [from, to] in &config.escape_chars {
        let from: Vec<_> = from.chars().map(u32::from).collect();
        let to: Vec<_> = to.chars().map(u32::from).collect();
        if from.len() != 1 || from[0] > 255 || to.len() != 2 || to[0] != 238 || to[1] > 255 {
            return Err("invalid trzsz escape table".into());
        }
        if encode[from[0] as usize].is_some() || decode[to[1] as usize].is_some() {
            return Err("duplicate trzsz escape mapping".into());
        }
        encode[from[0] as usize] = Some(to[1] as u8);
        decode[to[1] as usize] = Some(from[0] as u8);
    }
    Ok((encode, decode))
}

pub(super) fn validate(config: &TrzszConfig) -> Result<(), String> {
    if !(0..=4).contains(&config.protocol.unwrap_or(0))
        || !(0..=2).contains(&config.compress.unwrap_or(0))
        || config
            .max_buf_size
            .is_some_and(|size| !(1024..=1024 * 1024 * 1024).contains(&size))
    {
        return Err("unsupported trzsz negotiation".into());
    }
    escape_table(config)?;
    Ok(())
}

pub(super) struct Encoder {
    zstd: Option<zstd::stream::write::Encoder<'static, Vec<u8>>>,
    base64_tail: Vec<u8>,
    escape: [Option<u8>; 256],
    binary: bool,
}

impl Encoder {
    pub fn new(config: &TrzszConfig, compress: bool) -> Result<Self, String> {
        Ok(Self {
            zstd: compress
                .then(|| zstd::stream::write::Encoder::new(Vec::new(), 1))
                .transpose()
                .map_err(|e| e.to_string())?,
            base64_tail: Vec::new(),
            escape: escape_table(config)?.0,
            binary: config.binary,
        })
    }

    pub fn encode(&mut self, input: &[u8], finish: bool) -> Result<Vec<u8>, String> {
        let bytes = if let Some(encoder) = &mut self.zstd {
            encoder.write_all(input).map_err(|e| e.to_string())?;
            encoder.flush().map_err(|e| e.to_string())?;
            let mut bytes = std::mem::take(encoder.get_mut());
            if finish {
                bytes.extend(
                    self.zstd
                        .take()
                        .expect("encoder")
                        .finish()
                        .map_err(|e| e.to_string())?,
                );
            }
            bytes
        } else {
            input.to_vec()
        };
        if self.binary {
            let mut output = Vec::with_capacity(bytes.len());
            for byte in bytes {
                if let Some(code) = self.escape[byte as usize] {
                    output.extend([238, code]);
                } else {
                    output.push(byte);
                }
            }
            Ok(output)
        } else {
            self.base64_tail.extend(bytes);
            let len = if finish {
                self.base64_tail.len()
            } else {
                self.base64_tail.len() / 3 * 3
            };
            let encoded =
                base64::engine::general_purpose::STANDARD.encode(&self.base64_tail[..len]);
            self.base64_tail.drain(..len);
            Ok(encoded.into_bytes())
        }
    }
}

pub(super) struct Decoder {
    zstd: Option<zstd::stream::raw::Decoder<'static>>,
    tail: Vec<u8>,
    unescape: [Option<u8>; 256],
    binary: bool,
    escaped: bool,
    has_escape: bool,
    frame_complete: bool,
    padded: bool,
    seen_input: bool,
}

impl Decoder {
    pub fn new(config: &TrzszConfig, compress: bool) -> Result<Self, String> {
        let mut zstd = compress
            .then(zstd::stream::raw::Decoder::new)
            .transpose()
            .map_err(|e| e.to_string())?;
        if let Some(decoder) = &mut zstd {
            decoder
                .set_parameter(zstd::zstd_safe::DParameter::WindowLogMax(27))
                .map_err(|e| e.to_string())?;
        }
        Ok(Self {
            zstd,
            tail: Vec::new(),
            unescape: escape_table(config)?.1,
            binary: config.binary,
            escaped: false,
            has_escape: !config.escape_chars.is_empty(),
            frame_complete: !compress,
            padded: false,
            seen_input: false,
        })
    }

    pub fn decode(
        &mut self,
        input: &[u8],
        finish: bool,
        mut sink: impl FnMut(&[u8]) -> Result<(), String>,
    ) -> Result<(), String> {
        if finish && input.is_empty() && !self.seen_input {
            return Ok(());
        }
        self.seen_input |= !input.is_empty();
        let decoded = if self.binary {
            let mut bytes = Vec::with_capacity(input.len());
            for &byte in input {
                if self.escaped {
                    bytes.push(self.unescape[byte as usize].ok_or("unknown trzsz escape code")?);
                    self.escaped = false;
                } else if byte == 238 && self.has_escape {
                    self.escaped = true;
                } else {
                    bytes.push(byte);
                }
            }
            bytes
        } else {
            if self.padded && !input.is_empty() {
                return Err("data after base64 padding".into());
            }
            self.tail.extend(input);
            let len = self.tail.len() / 4 * 4;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(&self.tail[..len])
                .map_err(|_| "invalid trzsz base64 stream")?;
            self.padded |= self.tail[..len].contains(&b'=');
            self.tail.drain(..len);
            bytes
        };
        if let Some(decoder) = &mut self.zstd {
            if self.frame_complete && !decoded.is_empty() {
                return Err("data after zstd frame".into());
            }
            let mut consumed = 0;
            while !decoded.is_empty() || !self.frame_complete {
                let mut output = [0; 32 * 1024];
                let status = decoder
                    .run_on_buffers(&decoded[consumed..], &mut output)
                    .map_err(|_| "invalid trzsz zstd stream")?;
                consumed += status.bytes_read;
                self.frame_complete = status.remaining == 0;
                sink(&output[..status.bytes_written])?;
                if consumed == decoded.len()
                    && (status.bytes_written < output.len() || self.frame_complete)
                {
                    break;
                }
                if status.bytes_read == 0 && status.bytes_written == 0 {
                    return Err("stalled trzsz decoder".into());
                }
            }
        } else {
            sink(&decoded)?;
        }
        if finish && (!self.tail.is_empty() || self.escaped || !self.frame_complete) {
            return Err("truncated trzsz data stream".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{Decoder, Encoder, compression_profitable};
    use crate::trzsz::TrzszConfig;

    #[test]
    fn automatic_compression_samples_content_and_restores_the_resume_position() {
        let mut bytes = vec![0; 512 * 1024];
        let mut reader = std::io::Cursor::new(&bytes);
        reader.set_position(1024);
        assert!(compression_profitable(&mut reader, bytes.len() as i64 - 1024).unwrap());
        assert_eq!(reader.position(), 1024);
        let mut seed = 7_u32;
        for byte in &mut bytes {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            *byte = seed as u8;
        }
        let mut reader = std::io::Cursor::new(&bytes);
        reader.set_position(1024);
        assert!(!compression_profitable(&mut reader, bytes.len() as i64 - 1024).unwrap());
        assert_eq!(reader.position(), 1024);
        assert!(compression_profitable(&mut reader, bytes.len() as i64).is_err());
        assert_eq!(reader.position(), 1024);
    }

    fn config(binary: bool) -> TrzszConfig {
        let mut config = TrzszConfig::local_default(None, false);
        config.protocol = Some(4);
        config.binary = binary;
        config.escape_chars = vec![
            ['\u{ee}'.to_string(), "\u{ee}\u{ee}".into()],
            ["\0".into(), "\u{ee}A".into()],
        ];
        config
    }

    #[test]
    fn continuous_codecs_accept_arbitrary_splits_and_reject_truncation() {
        let data: Vec<_> = (0..200_003).map(|i| ((i * 31) % 256) as u8).collect();
        for binary in [false, true] {
            for compress in [false, true] {
                let config = config(binary);
                let mut encoder = Encoder::new(&config, compress).unwrap();
                let mut wire = Vec::new();
                for (i, chunk) in data.chunks(997).enumerate() {
                    wire.extend(encoder.encode(chunk, (i + 1) * 997 >= data.len()).unwrap());
                }
                for split in [1, 2, 3, 4, 511, 32_768] {
                    let mut decoder = Decoder::new(&config, compress).unwrap();
                    let mut plain = Vec::new();
                    for chunk in wire.chunks(split) {
                        decoder
                            .decode(chunk, false, |bytes| {
                                plain.extend_from_slice(bytes);
                                Ok(())
                            })
                            .unwrap();
                    }
                    decoder.decode(&[], true, |_| Ok(())).unwrap();
                    assert_eq!(plain, data);
                }
                if compress {
                    let mut decoder = Decoder::new(&config, true).unwrap();
                    let result = decoder.decode(&wire[..wire.len() / 2], true, |_| Ok(()));
                    assert!(result.is_err(), "truncated compression stream");
                }
            }
        }
    }

    #[test]
    fn large_stream_uses_bounded_codec_buffers_and_stops_on_sink_failure() {
        let config = config(false);
        let mut encoder = Encoder::new(&config, true).unwrap();
        let mut decoder = Decoder::new(&config, true).unwrap();
        let chunk = vec![42; 256 * 1024];
        let mut total = 0;
        for i in 0..256 {
            let encoded = encoder.encode(&chunk, i == 255).unwrap();
            assert!(encoded.len() < 512 * 1024);
            assert!(encoder.base64_tail.len() <= 2);
            decoder
                .decode(&encoded, false, |bytes| {
                    assert!(bytes.len() <= 32 * 1024);
                    total += bytes.len();
                    Ok(())
                })
                .unwrap();
            assert!(decoder.tail.len() <= 3);
        }
        decoder.decode(&[], true, |_| Ok(())).unwrap();
        assert_eq!(total, 64 * 1024 * 1024);
        let mut encoder = Encoder::new(&config, true).unwrap();
        let wire = encoder.encode(&chunk, true).unwrap();
        let mut decoder = Decoder::new(&config, true).unwrap();
        assert_eq!(
            decoder
                .decode(&wire, true, |_| Err("disk full".into()))
                .unwrap_err(),
            "disk full"
        );
    }
}
