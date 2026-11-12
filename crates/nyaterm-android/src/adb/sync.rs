use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream};
use std::time::{Duration, Instant};

use crate::adb::client::{DeviceBinding, device_features};
use crate::error::{AdbError, AdbResult};

const CHUNK: usize = 64 * 1024;
const PATH_LIMIT: usize = 4096;
const LIST_LIMIT: usize = 100_000;

#[derive(Clone, Debug)]
pub struct FileStat {
    pub identity: Option<(u64, u64)>,
    pub mode: u32,
    pub size: u64,
    pub modified: u64,
    pub accessed: u64,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
}

#[derive(Clone)]
pub struct DirectoryEntry {
    pub name: Vec<u8>,
    pub stat: FileStat,
}

/// A data notification is an increment, never a running counter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SyncByteDelta(pub u64);

#[derive(Clone, Copy)]
pub struct SyncSendOptions {
    pub mode: u32,
    pub modified: u64,
    pub buffer_size: usize,
}

pub struct SyncStream {
    stream: TcpStream,
    stat_v2: bool,
    list_v2: bool,
    idle_timeout: Duration,
}

impl SyncStream {
    pub fn open(binding: &DeviceBinding) -> AdbResult<Self> {
        let features = device_features(binding)?;
        Self::open_with_features(binding, &features)
    }
    pub(crate) fn open_with_features(
        binding: &DeviceBinding,
        features: &[String],
    ) -> AdbResult<Self> {
        Self::open_with_features_controlled(binding, features, || Ok(()))
    }
    pub(crate) fn open_with_features_controlled(
        binding: &DeviceBinding,
        features: &[String],
        check: impl FnMut() -> AdbResult<()>,
    ) -> AdbResult<Self> {
        let stream = crate::adb::client::open_service_with_cancel(binding, b"sync:", check)?;
        stream.set_read_timeout(Some(Duration::from_millis(200)))?;
        stream.set_write_timeout(Some(Duration::from_millis(200)))?;
        Ok(Self {
            stream,
            stat_v2: features.iter().any(|f| f == "stat_v2"),
            list_v2: features.iter().any(|f| f == "ls_v2"),
            idle_timeout: Duration::from_secs(15),
        })
    }

    fn request(
        &mut self,
        id: &[u8; 4],
        path: &[u8],
        progress: &mut impl FnMut(u64) -> AdbResult<()>,
    ) -> AdbResult<()> {
        if path.is_empty() || path.len() > PATH_LIMIT || path.contains(&0) {
            return Err(AdbError::Protocol("sync path"));
        }
        self.write(id, progress)?;
        self.write(&(path.len() as u32).to_le_bytes(), progress)?;
        self.write(path, progress)
    }

    fn read(
        &mut self,
        bytes: &mut [u8],
        progress: &mut impl FnMut(u64) -> AdbResult<()>,
    ) -> AdbResult<()> {
        let mut stalled = Duration::ZERO;
        let mut offset = 0;
        while offset < bytes.len() {
            progress(0)?;
            let started = Instant::now();
            match self.stream.read(&mut bytes[offset..]) {
                Ok(0) => return Err(AdbError::Protocol("sync EOF")),
                Ok(count) => {
                    offset += count;
                    stalled = Duration::ZERO;
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                    ) =>
                {
                    stalled += started.elapsed();
                    if stalled >= self.idle_timeout {
                        return Err(AdbError::Timeout);
                    }
                }
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }

    fn write(
        &mut self,
        bytes: &[u8],
        progress: &mut impl FnMut(u64) -> AdbResult<()>,
    ) -> AdbResult<()> {
        let mut stalled = Duration::ZERO;
        let mut offset = 0;
        while offset < bytes.len() {
            progress(0)?;
            let started = Instant::now();
            match self.stream.write(&bytes[offset..]) {
                Ok(0) => return Err(AdbError::Protocol("sync write EOF")),
                Ok(count) => {
                    offset += count;
                    stalled = Duration::ZERO;
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                    ) =>
                {
                    stalled += started.elapsed();
                    if stalled >= self.idle_timeout {
                        return Err(AdbError::Timeout);
                    }
                }
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }

    fn tag(&mut self, progress: &mut impl FnMut(u64) -> AdbResult<()>) -> AdbResult<[u8; 4]> {
        let mut tag = [0; 4];
        self.read(&mut tag, progress)?;
        if &tag == b"FAIL" {
            let mut length = [0; 4];
            self.read(&mut length, progress)?;
            let length = u32::from_le_bytes(length) as usize;
            if length > CHUNK {
                return Err(AdbError::Protocol("sync error length"));
            }
            let mut ignored = vec![0; length];
            self.read(&mut ignored, progress)?;
            return Err(AdbError::Rejected);
        }
        Ok(tag)
    }

    /// No-follow metadata is required for mutation and recursive-copy safety.
    pub fn stat(&mut self, path: &[u8]) -> AdbResult<FileStat> {
        self.stat_with_cancel(path, || Ok(()))
    }
    pub fn stat_with_cancel(
        &mut self,
        path: &[u8],
        mut check: impl FnMut() -> AdbResult<()>,
    ) -> AdbResult<FileStat> {
        let progress = &mut |_| check();
        let id = if self.stat_v2 { b"LST2" } else { b"STAT" };
        self.request(id, path, progress)?;
        if &self.tag(progress)? != id {
            return Err(AdbError::Protocol("stat response"));
        }
        if self.stat_v2 {
            let mut data = [0; 68];
            self.read(&mut data, progress)?;
            decode_stat_v2(&data)
        } else {
            let mut data = [0; 12];
            self.read(&mut data, progress)?;
            let mode = u32::from_le_bytes(data[0..4].try_into().unwrap());
            if mode == 0 {
                return Err(AdbError::FileMissing);
            }
            Ok(FileStat {
                identity: None,
                mode,
                size: u32::from_le_bytes(data[4..8].try_into().unwrap()).into(),
                modified: u32::from_le_bytes(data[8..12].try_into().unwrap()).into(),
                accessed: 0,
                uid: None,
                gid: None,
            })
        }
    }
    pub fn list(&mut self, path: &[u8]) -> AdbResult<Vec<DirectoryEntry>> {
        self.list_with_cancel(path, || Ok(()))
    }
    pub fn list_with_cancel(
        &mut self,
        path: &[u8],
        mut check: impl FnMut() -> AdbResult<()>,
    ) -> AdbResult<Vec<DirectoryEntry>> {
        let progress = &mut |_| check();
        self.request(if self.list_v2 { b"LIS2" } else { b"LIST" }, path, progress)?;
        let mut entries = Vec::new();
        loop {
            let tag = self.tag(progress)?;
            let mut data = vec![0; if self.list_v2 { 72 } else { 16 }];
            self.read(&mut data, progress)?;
            if &tag == b"DONE" {
                break;
            }
            let (stat, length) = if self.list_v2 {
                if &tag != b"DNT2" {
                    return Err(AdbError::Protocol("list-v2 response"));
                }
                (
                    decode_stat_v2(&data[..68])?,
                    u32::from_le_bytes(data[68..72].try_into().unwrap()) as usize,
                )
            } else {
                if &tag != b"DENT" {
                    return Err(AdbError::Protocol("list response"));
                }
                (
                    FileStat {
                        identity: None,
                        mode: u32::from_le_bytes(data[0..4].try_into().unwrap()),
                        size: u32::from_le_bytes(data[4..8].try_into().unwrap()).into(),
                        modified: u32::from_le_bytes(data[8..12].try_into().unwrap()).into(),
                        accessed: 0,
                        uid: None,
                        gid: None,
                    },
                    u32::from_le_bytes(data[12..16].try_into().unwrap()) as usize,
                )
            };
            if length > PATH_LIMIT || entries.len() >= LIST_LIMIT {
                return Err(AdbError::Protocol("directory limit"));
            }
            let mut name = vec![0; length];
            self.read(&mut name, progress)?;
            if name != b"." && name != b".." {
                entries.push(DirectoryEntry { name, stat });
            }
        }
        Ok(entries)
    }

    pub fn receive_controlled(
        &mut self,
        path: &[u8],
        writer: &mut impl Write,
        mut check: impl FnMut() -> AdbResult<()>,
        mut progress: impl FnMut(SyncByteDelta) -> AdbResult<()>,
    ) -> AdbResult<u64> {
        self.receive(path, writer, |bytes| {
            check()?;
            if bytes != 0 {
                progress(SyncByteDelta(bytes))?;
            }
            Ok(())
        })
    }
    pub fn send_controlled(
        &mut self,
        path: &[u8],
        reader: &mut impl Read,
        options: SyncSendOptions,
        mut check: impl FnMut() -> AdbResult<()>,
        mut progress: impl FnMut(SyncByteDelta) -> AdbResult<()>,
    ) -> AdbResult<u64> {
        self.send_with_buffer(
            path,
            reader,
            options.mode,
            options.modified,
            options.buffer_size,
            |bytes| {
                check()?;
                if bytes != 0 {
                    progress(SyncByteDelta(bytes))?;
                }
                Ok(())
            },
        )
    }

    pub fn receive(
        &mut self,
        path: &[u8],
        writer: &mut impl Write,
        mut progress: impl FnMut(u64) -> AdbResult<()>,
    ) -> AdbResult<u64> {
        self.request(b"RECV", path, &mut progress)?;
        let mut total = 0;
        let mut buffer = vec![0; CHUNK];
        loop {
            let tag = self.tag(&mut progress)?;
            let mut length = [0; 4];
            self.read(&mut length, &mut progress)?;
            let length = u32::from_le_bytes(length) as usize;
            if &tag == b"DONE" {
                break;
            }
            if &tag != b"DATA" || length > CHUNK {
                return Err(AdbError::Protocol("sync data length"));
            }
            self.read(&mut buffer[..length], &mut progress)?;
            writer.write_all(&buffer[..length])?;
            total += length as u64;
            progress(length as u64)?;
        }
        Ok(total)
    }

    pub fn send(
        &mut self,
        path: &[u8],
        reader: &mut impl Read,
        mode: u32,
        modified: u64,
        progress: impl FnMut(u64) -> AdbResult<()>,
    ) -> AdbResult<u64> {
        self.send_with_buffer(path, reader, mode, modified, CHUNK, progress)
    }

    pub fn send_with_buffer(
        &mut self,
        path: &[u8],
        reader: &mut impl Read,
        mode: u32,
        modified: u64,
        buffer_size: usize,
        mut progress: impl FnMut(u64) -> AdbResult<()>,
    ) -> AdbResult<u64> {
        let mut target = path.to_vec();
        target.extend_from_slice(format!(",{}", mode & 0o777).as_bytes());
        self.request(b"SEND", &target, &mut progress)?;
        let mut total = 0;
        let mut buffer = vec![0; buffer_size.clamp(8 * 1024, CHUNK)];
        loop {
            progress(0)?;
            let length = reader.read(&mut buffer)?;
            if length == 0 {
                break;
            }
            self.write(b"DATA", &mut progress)?;
            self.write(&(length as u32).to_le_bytes(), &mut progress)?;
            self.write(&buffer[..length], &mut progress)?;
            total += length as u64;
            progress(length as u64)?;
        }
        self.write(b"DONE", &mut progress)?;
        self.write(
            &(u32::try_from(modified).map_err(|_| AdbError::Protocol("sync timestamp range"))?)
                .to_le_bytes(),
            &mut progress,
        )?;
        if &self.tag(&mut progress)? != b"OKAY" {
            return Err(AdbError::Protocol("sync send response"));
        }
        let mut ignored = [0; 4];
        self.read(&mut ignored, &mut progress)?;
        Ok(total)
    }
}

impl Drop for SyncStream {
    fn drop(&mut self) {
        let _ = self.stream.shutdown(Shutdown::Both);
    }
}

fn decode_stat_v2(data: &[u8]) -> AdbResult<FileStat> {
    let get32 = |offset| u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
    let get64 = |offset| u64::from_le_bytes(data[offset..offset + 8].try_into().unwrap());
    if get32(0) == 2 {
        return Err(AdbError::FileMissing);
    }
    if get32(0) != 0 {
        return Err(AdbError::Rejected);
    }
    Ok(FileStat {
        identity: Some((get64(4), get64(12))),
        mode: get32(20),
        uid: Some(get32(28)),
        gid: Some(get32(32)),
        size: get64(36),
        accessed: get64(44),
        modified: get64(52),
    })
}

#[cfg(test)]
mod tests {
    use crate::adb::sync::SyncStream;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::time::Duration;
    fn fixture(bytes: Vec<u8>, v2: bool) -> SyncStream {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut header = [0; 8];
            if socket.read_exact(&mut header).is_err() {
                return;
            }
            let len = u32::from_le_bytes(header[4..8].try_into().unwrap()) as usize;
            let mut path = vec![0; len];
            if socket.read_exact(&mut path).is_err() {
                return;
            }
            for byte in bytes {
                socket.write_all(&[byte]).unwrap();
            }
        });
        SyncStream {
            stream: client,
            stat_v2: v2,
            list_v2: v2,
            idle_timeout: Duration::from_secs(15),
        }
    }
    #[test]
    fn fragmented_legacy_listing_and_no_follow_stat_are_supported() {
        let mut bytes = b"DENT".to_vec();
        for value in [0o120777u32, 3, 7, 3] {
            bytes.extend(value.to_le_bytes());
        }
        bytes.extend(b"a'b");
        bytes.extend(b"DONE");
        bytes.extend([0; 16]);
        let entry = fixture(bytes, false).list(b"/fixture").unwrap().remove(0);
        assert_eq!(entry.name, b"a'b");
        assert_eq!(entry.stat.mode, 0o120777);
        let mut bytes = b"STAT".to_vec();
        for value in [0o100644u32, 12, 7] {
            bytes.extend(value.to_le_bytes());
        }
        assert_eq!(fixture(bytes, false).stat(b"/fixture").unwrap().size, 12);
    }
    #[test]
    fn v2_lstat_keeps_64bit_metadata_and_rejects_missing_files() {
        let mut bytes = b"LST2".to_vec();
        let mut stat = [0; 68];
        stat[20..24].copy_from_slice(&0o120777u32.to_le_bytes());
        stat[28..32].copy_from_slice(&2000u32.to_le_bytes());
        stat[36..44].copy_from_slice(&(1u64 << 40).to_le_bytes());
        bytes.extend(stat);
        let result = fixture(bytes, true).stat(b"/fixture").unwrap();
        assert_eq!(result.mode, 0o120777);
        assert_eq!(result.size, 1u64 << 40);
        assert_eq!(result.uid, Some(2000));
        let mut bytes = b"LST2".to_vec();
        let mut stat = [0; 68];
        stat[..4].copy_from_slice(&2u32.to_le_bytes());
        bytes.extend(stat);
        assert!(matches!(
            fixture(bytes, true).stat(b"/missing"),
            Err(crate::error::AdbError::FileMissing)
        ));
    }
    #[test]
    fn malformed_sync_chunk_and_cancellation_are_rejected() {
        let mut bytes = b"DATA".to_vec();
        bytes.extend(&65537u32.to_le_bytes());
        assert!(
            fixture(bytes, false)
                .receive(b"/fixture", &mut Vec::new(), |_| Ok(()))
                .is_err()
        );
        assert!(matches!(
            fixture(Vec::new(), false)
                .stat_with_cancel(b"/fixture", || Err(crate::error::AdbError::Cancelled)),
            Err(crate::error::AdbError::Cancelled)
        ));
    }
    #[test]
    fn paused_callbacks_do_not_consume_network_idle_budget() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        client
            .set_read_timeout(Some(Duration::from_millis(10)))
            .unwrap();
        let (resume_tx, resume_rx) = std::sync::mpsc::channel();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            resume_rx.recv().unwrap();
            socket.write_all(b"okay").unwrap();
            ready_tx.send(()).unwrap();
        });
        let mut stream = SyncStream {
            stream: client,
            stat_v2: false,
            list_v2: false,
            idle_timeout: Duration::from_millis(100),
        };
        let mut paused = false;
        let mut bytes = [0; 4];
        let result = stream.read(&mut bytes, &mut |_| {
            if !paused {
                paused = true;
                std::thread::sleep(Duration::from_millis(150));
                resume_tx.send(()).unwrap();
                // Server scheduling is part of the pause, not network idle time.
                ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            }
            Ok(())
        });
        server.join().unwrap();
        result.unwrap();
        assert_eq!(&bytes, b"okay");
    }

    #[test]
    fn network_stalls_exhaust_the_idle_budget() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        client
            .set_read_timeout(Some(Duration::from_millis(10)))
            .unwrap();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let (_socket, _) = listener.accept().unwrap();
            // Keep the connection open and silent until the client times out.
            let _ = done_rx.recv();
        });
        let mut stream = SyncStream {
            stream: client,
            stat_v2: false,
            list_v2: false,
            idle_timeout: Duration::from_millis(100),
        };
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let result = stream.read(&mut [0; 4], &mut |_| {
            if std::time::Instant::now() >= deadline {
                return Err(crate::error::AdbError::Cancelled);
            }
            Ok(())
        });
        done_tx.send(()).unwrap();
        server.join().unwrap();
        assert!(matches!(result, Err(crate::error::AdbError::Timeout)));
    }
}
