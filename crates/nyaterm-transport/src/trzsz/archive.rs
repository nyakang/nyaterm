//! Protocol 4 archives are a sequence of encoded source headers and file bytes.
//! Paths are validated before any event can reach a filesystem adapter.
use std::collections::{BTreeMap, VecDeque};
use std::io::{self, Read};

use super::{
    TrzszDownloadEvent, TrzszSourceFile, TrzszUploadEntry, TrzszUploadPayload, decode_trzsz_string,
    encode_trzsz_string, parse_trzsz_source_file,
};

pub(super) struct Receiver {
    root: TrzszSourceFile,
    header: Vec<u8>,
    current: Option<(TrzszSourceFile, i64)>,
}

impl Receiver {
    pub fn new(root: TrzszSourceFile) -> Self {
        Self {
            root,
            header: Vec::new(),
            current: None,
        }
    }

    pub fn push(
        &mut self,
        mut bytes: &[u8],
        sink: &mut impl FnMut(TrzszDownloadEvent) -> Result<(), String>,
    ) -> Result<(), String> {
        while !bytes.is_empty() {
            if let Some((file, received)) = &mut self.current {
                let len = bytes.len().min((file.size - *received) as usize);
                *received += len as i64;
                sink(TrzszDownloadEvent::Data {
                    name: file.file_name().into(),
                    bytes: bytes[..len].to_vec(),
                    received: *received,
                    size: file.size,
                })?;
                bytes = &bytes[len..];
                if *received == file.size {
                    sink(TrzszDownloadEvent::FileFinished {
                        name: file.file_name().into(),
                        digest: Vec::new(),
                    })?;
                    self.current = None;
                }
                continue;
            }
            let len = bytes
                .iter()
                .position(|byte| *byte == b'\n')
                .map(|index| index + 1)
                .unwrap_or(bytes.len());
            if self.header.len() + len > super::TRZSZ_MAX_PROTOCOL_LINE_LEN {
                return Err("trzsz archive header too large".into());
            }
            self.header.extend(&bytes[..len]);
            bytes = &bytes[len..];
            if self.header.last() != Some(&b'\n') {
                continue;
            }
            self.header.pop();
            let json = decode_trzsz_string(&self.header).ok_or("invalid trzsz archive header")?;
            self.header.clear();
            let file = parse_trzsz_source_file(
                std::str::from_utf8(&json).map_err(|_| "invalid trzsz archive name")?,
            )
            .ok_or("invalid trzsz archive entry")?;
            if file.archive
                || file.path_id != self.root.path_id
                || file.path_name.len() <= self.root.path_name.len()
                || !file.path_name.starts_with(&self.root.path_name)
                || file.size < 0
            {
                return Err("trzsz archive entry escapes its root".into());
            }
            if file.is_dir {
                sink(TrzszDownloadEvent::Directory {
                    name: file.file_name().into(),
                    path_id: file.path_id,
                    components: file.path_name,
                })?;
            } else {
                sink(TrzszDownloadEvent::FilePath {
                    name: file.file_name().into(),
                    path_id: file.path_id,
                    components: file.path_name.clone(),
                })?;
                sink(TrzszDownloadEvent::FileSize {
                    name: file.file_name().into(),
                    size: file.size,
                })?;
                if file.size == 0 {
                    sink(TrzszDownloadEvent::FileFinished {
                        name: file.file_name().into(),
                        digest: Vec::new(),
                    })?;
                } else {
                    self.current = Some((file, 0));
                }
            }
        }
        Ok(())
    }

    pub fn finish(&self) -> Result<(), String> {
        if self.current.is_some() || !self.header.is_empty() {
            Err("truncated trzsz archive".into())
        } else {
            Ok(())
        }
    }
}

pub(super) fn pack(entries: Vec<TrzszUploadEntry>) -> Result<Vec<TrzszUploadEntry>, String> {
    let mut groups: BTreeMap<i64, Vec<TrzszUploadEntry>> = BTreeMap::new();
    let mut plain = Vec::new();
    for entry in entries {
        if let Some(source) = &entry.source {
            groups.entry(source.path_id).or_default().push(entry);
        } else {
            plain.push(entry);
        }
    }
    for mut entries in groups.into_values() {
        let mut root = entries.remove(0);
        if root.is_dir() && !entries.is_empty() {
            let mut size = 0i64;
            for entry in &entries {
                let header = header(entry)?;
                size = size
                    .checked_add(header.len() as i64)
                    .and_then(|size| size.checked_add(if entry.is_dir() { 0 } else { entry.size }))
                    .ok_or("trzsz archive too large")?;
            }
            root.size = size;
            root.source.as_mut().expect("root").archive = true;
            root.source.as_mut().expect("root").size = size;
            root.payload = TrzszUploadPayload::Archive(entries);
        } else {
            plain.extend(entries);
        }
        plain.push(root);
    }
    Ok(plain)
}

fn header(entry: &TrzszUploadEntry) -> Result<Vec<u8>, String> {
    let json = serde_json::to_vec(
        entry
            .source
            .as_ref()
            .ok_or("archive entry missing metadata")?,
    )
    .map_err(|error| error.to_string())?;
    let mut header = encode_trzsz_string(&json).into_bytes();
    header.push(b'\n');
    Ok(header)
}

pub(super) struct Reader {
    entries: VecDeque<TrzszUploadEntry>,
    header: io::Cursor<Vec<u8>>,
    payload: Option<(Box<dyn Read + Send>, i64)>,
}

impl Reader {
    pub fn new(entries: Vec<TrzszUploadEntry>) -> Self {
        Self {
            entries: entries.into(),
            header: io::Cursor::new(Vec::new()),
            payload: None,
        }
    }
}

impl Read for Reader {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        loop {
            let count = self.header.read(bytes)?;
            if count > 0 {
                return Ok(count);
            }
            if let Some((reader, remaining)) = &mut self.payload {
                if *remaining > 0 {
                    let cap = bytes.len().min(*remaining as usize);
                    let count = reader.read(&mut bytes[..cap])?;
                    if count == 0 {
                        return Err(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            "truncated trzsz upload",
                        ));
                    }
                    *remaining -= count as i64;
                    return Ok(count);
                }
                self.payload = None;
            }
            let Some(entry) = self.entries.pop_front() else {
                return Ok(0);
            };
            self.header = io::Cursor::new(header(&entry).map_err(io::Error::other)?);
            if !entry.is_dir() {
                let reader: Box<dyn Read + Send> = match entry.payload {
                    TrzszUploadPayload::Memory(data) => Box::new(io::Cursor::new(data)),
                    TrzszUploadPayload::File(path) => Box::new(std::fs::File::open(path)?),
                    TrzszUploadPayload::Archive(_) => {
                        return Err(io::Error::other("nested trzsz archive"));
                    }
                };
                self.payload = Some((reader, entry.size));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Receiver;
    use crate::trzsz::{encode_trzsz_string, parse_trzsz_source_file};

    #[test]
    fn archive_rejects_escape_wrong_root_negative_lengths_and_truncated_payloads() {
        let root = parse_trzsz_source_file(
            r#"{"path_id":1,"path_name":["root"],"is_dir":true,"archive":true}"#,
        )
        .unwrap();
        for entry in [
            r#"{"path_id":1,"path_name":["root","..","escape"],"size":0}"#,
            r#"{"path_id":1,"path_name":["other","file"],"size":0}"#,
            r#"{"path_id":2,"path_name":["root","file"],"size":0}"#,
            r#"{"path_id":1,"path_name":["root","file"],"size":-1}"#,
            r#"{"path_id":1,"path_name":["root","file"],"archive":true}"#,
        ] {
            let mut receiver = Receiver::new(root.clone());
            let header = format!("{}\n", encode_trzsz_string(entry.as_bytes()));
            assert!(
                receiver
                    .push(header.as_bytes(), &mut |_| panic!(
                        "unsafe entry reached filesystem sink"
                    ))
                    .is_err()
            );
        }
        let mut receiver = Receiver::new(root);
        let header = format!(
            "{}\n",
            encode_trzsz_string(br#"{"path_id":1,"path_name":["root","file"],"size":5}"#)
        );
        for byte in header.bytes().chain(b"abc".iter().copied()) {
            receiver.push(&[byte], &mut |_| Ok(())).unwrap();
        }
        assert!(receiver.finish().is_err());
    }
}
