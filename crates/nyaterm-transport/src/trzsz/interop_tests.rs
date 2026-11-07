//! Opt-in interoperability tests against the unmodified trzsz-go v1.2.0 tools.
//! Set TRZSZ_GO_BIN to their directory and run `cargo test -p nyaterm-transport
//! trzsz::interop_tests -- --ignored --nocapture`.
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use super::{
    TrzszAction, TrzszDownloadEngine, TrzszDownloadEvent, TrzszProtocolStream, TrzszUploadEngine,
    TrzszUploadEntry, TrzszUploadEvent, TrzszUploadPayload, build_trzsz_action_frame,
    build_trzsz_string_frame, parse_trzsz_config_frame,
};

struct Peer {
    child: Child,
    stdin: Box<dyn Write>,
    tunnel: bool,
    rx: mpsc::Receiver<Vec<u8>>,
    stream: TrzszProtocolStream,
    deadline: Instant,
    read_delay: Duration,
}

impl Peer {
    fn start(tool: &str, args: &[String], protocol: i64, tunnel: bool) -> Self {
        let bin = PathBuf::from(std::env::var_os("TRZSZ_GO_BIN").expect("set TRZSZ_GO_BIN"));
        let executable = bin.join(format!("{tool}{}", std::env::consts::EXE_SUFFIX));
        let version = Command::new(&executable).arg("--version").output().unwrap();
        assert!(
            String::from_utf8_lossy(&version.stdout).contains("1.2.0")
                || String::from_utf8_lossy(&version.stderr).contains("1.2.0")
        );
        let mut child = Command::new(executable)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let mut stdout = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::sync_channel(4);
        std::thread::spawn(move || {
            let mut bytes = [0; 8192];
            while let Ok(n) = stdout.read(&mut bytes) {
                if n == 0 || tx.send(bytes[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
        let mut peer = Self {
            child,
            stdin: Box::new(stdin),
            rx,
            tunnel,
            stream: TrzszProtocolStream::new(),
            deadline: Instant::now() + Duration::from_secs(30),
            read_delay: Duration::ZERO,
        };
        // Wait for initialization: the tools discard input queued before their trigger.
        let mut trigger = Vec::new();
        while !String::from_utf8_lossy(&trigger)
            .split("::TRZSZ:TRANSFER:")
            .nth(1)
            .is_some_and(|tail| tail.contains('\n'))
        {
            trigger.extend(
                peer.rx
                    .recv_timeout(Duration::from_secs(5))
                    .expect("missing trigger"),
            );
        }
        let mut action = TrzszAction::local_default(cfg!(windows));
        action.protocol = Some(protocol);
        if tunnel {
            let text = String::from_utf8_lossy(&trigger);
            let marker = text
                .split("::TRZSZ:TRANSFER:")
                .nth(1)
                .unwrap()
                .lines()
                .next()
                .unwrap()
                .trim();
            let parts: Vec<_> = marker.split(':').collect();
            let uid = parts[2];
            let port = parts[3];
            let uid = &uid[..uid.len() - 2];
            let mut conn = std::net::TcpStream::connect(format!("127.0.0.1:{port}")).unwrap();
            conn.set_nodelay(true).unwrap();
            conn.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            conn.write_all(format!("::TRZSZ::CLIENT::HELLO::{uid}:{port}").as_bytes())
                .unwrap();
            let hello = format!("::TRZSZ::SERVER::HELLO::{uid}:{port}");
            let mut bytes = vec![0; hello.len()];
            conn.read_exact(&mut bytes).unwrap();
            assert_eq!(bytes, hello.as_bytes());
            let mut reader = conn.try_clone().unwrap();
            let (tx, rx) = mpsc::sync_channel(4);
            std::thread::spawn(move || {
                let mut bytes = [0; 8192];
                while let Ok(n) = reader.read(&mut bytes) {
                    if n == 0 || tx.send(bytes[..n].to_vec()).is_err() {
                        break;
                    }
                }
            });
            peer.rx = rx;
            peer.stdin = Box::new(conn);
            action.tunnel_connected = true;
            action.newline = None;
        }
        peer.send(vec![build_trzsz_action_frame(&action, cfg!(windows))]);
        peer
    }

    fn read(&mut self) -> Vec<super::TrzszProtocolFrame> {
        if !self.read_delay.is_zero() {
            std::thread::sleep(self.read_delay);
        }
        let bytes = self
            .rx
            .recv_timeout(self.deadline.saturating_duration_since(Instant::now()))
            .expect("peer exited or transfer timed out");
        {
            let frames = self.stream.filter_terminal_output(&bytes).frames;
            if std::env::var_os("TRZSZ_TRACE").is_some() {
                for f in &frames {
                    if f.frame_type != "DATA" {
                        eprintln!("recv {f:?}");
                    }
                }
            }
            frames
        }
    }

    fn send(&mut self, responses: Vec<Vec<u8>>) {
        for response in responses {
            if std::env::var_os("TRZSZ_TRACE").is_some() && !response.starts_with(b"#DATA:") {
                eprintln!("send {}", String::from_utf8_lossy(&response));
            }
            self.stdin.write_all(&response).unwrap();
        }
        self.stdin.flush().unwrap();
    }

    fn finish(&mut self) {
        self.send(vec![build_trzsz_string_frame(
            "EXIT",
            b"interop verified",
            if cfg!(windows) && !self.tunnel {
                "!\n"
            } else {
                "\n"
            },
        )]);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            assert!(Instant::now() < self.deadline, "peer did not exit");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
#[ignore = "requires pinned trzsz-go v1.2.0 executables (TRZSZ_GO_BIN)"]
fn real_go_large_files_with_slow_sink_delayed_acks_and_cancel() {
    // File-backed data and a streaming sink deliberately avoid retaining the
    // payload in this harness. Run alone to measure the test process RSS.
    for size in [32 * 1024 * 1024, 128 * 1024 * 1024] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large.bin");
        let block = payload(64 * 1024);
        let mut file = std::fs::File::create(&path).unwrap();
        let mut expected = md5::Context::new();
        for _ in 0..size / block.len() {
            file.write_all(&block).unwrap();
            expected.consume(&block);
        }
        drop(file);
        let digest = expected.finalize();
        let args = vec![
            "-q".into(),
            "-b".into(),
            "-c".into(),
            "no".into(),
            "-B".into(),
            "2M".into(),
            path.to_string_lossy().into_owned(),
        ];
        let mut peer = Peer::start("tsz", &args, 4, true);
        peer.deadline = Instant::now() + Duration::from_secs(180);
        let mut engine = TrzszDownloadEngine::new(false);
        let mut received = 0;
        let mut actual = md5::Context::new();
        let start = Instant::now();
        'download: loop {
            for frame in peer.read() {
                if let Some(config) = parse_trzsz_config_frame(&frame) {
                    engine.configure(&config).unwrap();
                    continue;
                }
                let mut done = false;
                let step = engine
                    .observe_frame_into(frame, |event| {
                        match event {
                            TrzszDownloadEvent::Data { bytes, .. } => {
                                assert!(bytes.len() <= 256 * 1024);
                                received += bytes.len();
                                actual.consume(bytes);
                                std::thread::sleep(Duration::from_millis(1));
                            }
                            TrzszDownloadEvent::Completed { .. } => done = true,
                            _ => {}
                        }
                        Ok(())
                    })
                    .unwrap();
                assert!(peer.stream.pending_line.len() < 300 * 1024);
                if !step.responses.is_empty() {
                    std::thread::sleep(Duration::from_millis(3));
                }
                peer.send(step.responses);
                if done {
                    peer.finish();
                    break 'download;
                }
            }
        }
        assert_eq!(received, size);
        assert_eq!(actual.finalize(), digest);
        eprintln!("slow download: {size} bytes in {:?}", start.elapsed());

        let target = tempfile::tempdir().unwrap();
        let mut peer = Peer::start(
            "trz",
            &[
                "-q".into(),
                "-b".into(),
                "-c".into(),
                "no".into(),
                target.path().to_string_lossy().into_owned(),
            ],
            4,
            true,
        );
        peer.deadline = Instant::now() + Duration::from_secs(180);
        let mut engine = TrzszUploadEngine::new(
            false,
            vec![TrzszUploadEntry {
                name: "large.bin".into(),
                size: size as i64,
                payload: TrzszUploadPayload::File(path.clone()),
                source: None,
            }],
        );
        let start = Instant::now();
        'upload: loop {
            for frame in peer.read() {
                let step = if let Some(config) = parse_trzsz_config_frame(&frame) {
                    engine.configure(&config).unwrap();
                    engine.begin().unwrap()
                } else {
                    engine.observe_frame(frame).unwrap()
                };
                assert!(engine.wire_acks.len() <= 10);
                assert!(step.responses.iter().map(Vec::len).sum::<usize>() < 3 * 1024 * 1024);
                let done = step
                    .events
                    .iter()
                    .any(|event| matches!(event, TrzszUploadEvent::Completed { .. }));
                std::thread::sleep(Duration::from_millis(3));
                peer.send(step.responses);
                if done {
                    peer.finish();
                    break 'upload;
                }
            }
        }
        let mut file = std::fs::File::open(target.path().join("large.bin")).unwrap();
        let mut actual = md5::Context::new();
        let mut buffer = [0; 64 * 1024];
        loop {
            let len = file.read(&mut buffer).unwrap();
            if len == 0 {
                break;
            }
            actual.consume(&buffer[..len]);
        }
        assert_eq!(actual.finalize(), digest);
        eprintln!("delayed upload: {size} bytes in {:?}", start.elapsed());

        // Abort a real sender while its bounded output pipe is saturated.
        let mut peer = Peer::start("tsz", &args, 4, true);
        let mut engine = TrzszDownloadEngine::new(false);
        'cancel: loop {
            for frame in peer.read() {
                if let Some(config) = parse_trzsz_config_frame(&frame) {
                    engine.configure(&config).unwrap();
                    continue;
                }
                if frame.frame_type == "DATA" {
                    break 'cancel;
                }
                peer.send(engine.observe_frame(frame).unwrap().responses);
            }
        }
        std::thread::sleep(Duration::from_millis(100));
        let start = Instant::now();
        peer.send(vec![super::trzsz_fail_response("Cancelled", false)]);
        while peer.child.try_wait().unwrap().is_none() {
            let _ = peer.rx.recv_timeout(Duration::from_millis(10));
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "cancel did not stop the sender"
            );
        }
        eprintln!("cancel response: {:?}", start.elapsed());
    }
}

fn payload(size: usize) -> Vec<u8> {
    let mut seed = 7_u32;
    (0..size)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as u8
        })
        .collect()
}

#[test]
#[ignore = "requires pinned trzsz-go v1.2.0 executables (TRZSZ_GO_BIN)"]
fn real_go_truncated_data_cannot_complete_or_acknowledge_an_incomplete_frame() {
    for protocol in 2..=4 {
        let source = tempfile::tempdir().unwrap();
        let path = source.path().join("truncated.bin");
        std::fs::write(&path, payload(600_001)).unwrap();
        let mut peer = Peer::start(
            "tsz",
            &[
                "-q".into(),
                "-b".into(),
                "-c".into(),
                "yes".into(),
                path.to_string_lossy().into_owned(),
            ],
            protocol,
            true,
        );
        let mut engine = TrzszDownloadEngine::new(false);
        'transfer: loop {
            for mut frame in peer.read() {
                if let Some(config) = parse_trzsz_config_frame(&frame) {
                    engine.configure(&config).unwrap();
                    continue;
                }
                let truncated = match &mut frame.payload {
                    super::TrzszProtocolPayload::EncodedBytes(bytes)
                        if frame.frame_type == "DATA" && bytes.len() > 1 =>
                    {
                        bytes.truncate(bytes.len() / 2);
                        true
                    }
                    _ => false,
                };
                let step = engine.observe_frame(frame);
                if truncated {
                    assert!(
                        step.is_err(),
                        "truncated DATA must not produce an ACK or completion"
                    );
                    break 'transfer;
                }
                let step = step.unwrap();
                assert!(
                    !step
                        .events
                        .iter()
                        .any(|event| matches!(event, TrzszDownloadEvent::Completed { .. }))
                );
                peer.send(step.responses);
            }
        }
    }
}

#[test]
#[ignore = "requires pinned trzsz-go v1.2.0 executables (TRZSZ_GO_BIN)"]
fn real_go_upload_with_one_hundred_millisecond_ack_latency() {
    let source = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let path = source.path().join("latency.bin");
    let block = payload(64 * 1024);
    let mut file = std::fs::File::create(&path).unwrap();
    let mut expected = md5::Context::new();
    for _ in 0..256 {
        file.write_all(&block).unwrap();
        expected.consume(&block);
    }
    drop(file);
    let mut peer = Peer::start(
        "trz",
        &[
            "-q".into(),
            "-b".into(),
            "-c".into(),
            "no".into(),
            target.path().to_string_lossy().into_owned(),
        ],
        4,
        true,
    );
    peer.read_delay = Duration::from_millis(100);
    peer.deadline = Instant::now() + Duration::from_secs(120);
    let start = Instant::now();
    upload(
        &mut peer,
        vec![TrzszUploadEntry::from_file(
            "latency.bin",
            path,
            16 * 1024 * 1024,
        )],
    );
    let mut file = std::fs::File::open(target.path().join("latency.bin")).unwrap();
    let mut actual = md5::Context::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).unwrap();
        if count == 0 {
            break;
        }
        actual.consume(&buffer[..count]);
    }
    assert_eq!(actual.finalize(), expected.finalize());
    eprintln!(
        "16 MiB upload with 100 ms ACK latency: {:?}",
        start.elapsed()
    );
}

fn download(peer: &mut Peer) -> BTreeMap<String, Vec<u8>> {
    let mut engine = TrzszDownloadEngine::new(cfg!(windows) && !peer.tunnel);
    let mut received = BTreeMap::<String, Vec<u8>>::new();
    let mut current = None;
    loop {
        for frame in peer.read() {
            if let Some(config) = parse_trzsz_config_frame(&frame) {
                engine.configure(&config).unwrap();
                continue;
            }
            let step = engine
                .observe_frame(frame.clone())
                .unwrap_or_else(|error| panic!("{frame:?}: {error:?}"));
            let mut done = false;
            for event in step.events {
                match event {
                    TrzszDownloadEvent::FilePath { components, .. } => {
                        current = Some(components.join("/"))
                    }
                    TrzszDownloadEvent::FileName { name } => {
                        current.get_or_insert(name);
                    }
                    TrzszDownloadEvent::FileSize { name, .. } => {
                        received.entry(current.clone().unwrap_or(name)).or_default();
                    }
                    TrzszDownloadEvent::Data { name, bytes, .. } => received
                        .entry(current.clone().unwrap_or(name))
                        .or_default()
                        .extend(bytes),
                    TrzszDownloadEvent::FileFinished { .. } => current = None,
                    TrzszDownloadEvent::Completed { .. } => done = true,
                    _ => {}
                }
            }
            peer.send(step.responses);
            if done {
                peer.finish();
                return received;
            }
        }
    }
}

fn upload(peer: &mut Peer, entries: Vec<TrzszUploadEntry>) {
    let mut engine = TrzszUploadEngine::new(cfg!(windows) && !peer.tunnel, entries);
    loop {
        for frame in peer.read() {
            let step = if let Some(config) = parse_trzsz_config_frame(&frame) {
                engine.configure(&config).unwrap();
                engine.begin().unwrap()
            } else {
                engine
                    .observe_frame(frame.clone())
                    .unwrap_or_else(|error| panic!("{frame:?}: {error:?}"))
            };
            let done = step
                .events
                .iter()
                .any(|e| matches!(e, TrzszUploadEvent::Completed { .. }));
            peer.send(step.responses);
            if done {
                peer.finish();
                return;
            }
        }
    }
}

#[test]
#[ignore = "requires pinned trzsz-go v1.2.0 executables (TRZSZ_GO_BIN)"]
fn real_go_directories_duplicate_roots_and_prefix_resume() {
    use super::TrzszUploadSource;
    for protocol in 1..=4 {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let files = BTreeMap::from([
            ("project/sub/empty".to_string(), Vec::new()),
            ("project/sub/data".to_string(), payload(700_001)),
            ("project/other/data".to_string(), payload(513)),
        ]);
        for (name, bytes) in &files {
            let path = source.path().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }
        std::fs::create_dir_all(source.path().join("project/empty-dir")).unwrap();
        let args = vec![
            "-q".into(),
            "-d".into(),
            source.path().join("project").to_string_lossy().into_owned(),
        ];
        let mut peer = Peer::start("tsz", &args, protocol, true);
        assert_eq!(download(&mut peer), files);
        let mut entries = Vec::new();
        for name in [
            "project",
            "project/sub",
            "project/other",
            "project/empty-dir",
        ] {
            entries.push(TrzszUploadEntry {
                name: name.into(),
                size: 0,
                payload: TrzszUploadPayload::Memory(Vec::new()),
                source: Some(TrzszUploadSource {
                    path_id: 0,
                    path_name: name.split('/').map(String::from).collect(),
                    is_dir: true,
                    size: 0,
                    perm: None,
                    archive: false,
                }),
            });
        }
        for (name, bytes) in &files {
            entries.push(TrzszUploadEntry {
                name: name.clone(),
                size: bytes.len() as i64,
                payload: TrzszUploadPayload::File(source.path().join(name)),
                source: Some(TrzszUploadSource {
                    path_id: 0,
                    path_name: name.split('/').map(String::from).collect(),
                    is_dir: false,
                    size: bytes.len() as i64,
                    perm: None,
                    archive: false,
                }),
            });
        }
        let mut peer = Peer::start(
            "trz",
            &[
                "-q".into(),
                "-d".into(),
                target.path().to_string_lossy().into_owned(),
            ],
            protocol,
            true,
        );
        upload(&mut peer, entries);
        for (name, bytes) in &files {
            assert_eq!(&std::fs::read(target.path().join(name)).unwrap(), bytes);
        }
        assert!(target.path().join("project/empty-dir").is_dir());
        if protocol < 3 {
            continue;
        }
        for mismatch in [false, true] {
            let target = tempfile::tempdir().unwrap();
            let bytes = payload(12 * 1024 * 1024 + 17);
            let mut prefix = bytes[..11 * 1024 * 1024].to_vec();
            if mismatch {
                prefix[10 * 1024 * 1024 + 1] ^= 0xff;
            }
            std::fs::write(target.path().join("resume"), prefix).unwrap();
            std::fs::write(target.path().join("empty"), b"old contents").unwrap();
            let mut peer = Peer::start(
                "trz",
                &[
                    "-q".into(),
                    "-y".into(),
                    target.path().to_string_lossy().into_owned(),
                ],
                protocol,
                true,
            );
            upload(
                &mut peer,
                vec![
                    TrzszUploadEntry {
                        name: "resume".into(),
                        size: bytes.len() as i64,
                        payload: TrzszUploadPayload::Memory(bytes.clone()),
                        source: None,
                    },
                    TrzszUploadEntry {
                        name: "empty".into(),
                        size: 0,
                        payload: TrzszUploadPayload::Memory(Vec::new()),
                        source: None,
                    },
                ],
            );
            assert_eq!(std::fs::read(target.path().join("resume")).unwrap(), bytes);
            assert_eq!(
                std::fs::metadata(target.path().join("empty"))
                    .unwrap()
                    .len(),
                0
            );
        }
    }
}

#[test]
#[ignore = "requires pinned trzsz-go v1.2.0 executables (TRZSZ_GO_BIN)"]
fn real_go_bidirectional_protocol_matrix() {
    for protocol in 1..=4 {
        if std::env::var("TRZSZ_PROTOCOL")
            .ok()
            .and_then(|s| s.parse::<i64>().ok())
            .is_some_and(|p| p != protocol)
        {
            continue;
        }
        for binary in [false, true] {
            for compress in ["yes", "no", "auto"] {
                eprintln!("protocol={protocol} binary={binary} compress={compress}");
                let input = tempfile::tempdir().unwrap();
                let output = tempfile::tempdir().unwrap();
                let files: Vec<_> = [0, 1, 511, 512, 131_072, 600_001]
                    .into_iter()
                    .enumerate()
                    .map(|(i, n)| (format!("file{i}.bin"), payload(n)))
                    .collect();
                for (name, bytes) in &files {
                    std::fs::write(input.path().join(name), bytes).unwrap();
                }
                let mut args = vec![
                    "-q".into(),
                    "-c".into(),
                    compress.into(),
                    "-B".into(),
                    "64K".into(),
                ];
                if binary {
                    args.extend(["-b".into(), "-e".into()]);
                }
                let mut download_args = args.clone();
                download_args.extend(
                    files
                        .iter()
                        .map(|(name, _)| input.path().join(name).to_string_lossy().into_owned()),
                );
                let mut peer = Peer::start("tsz", &download_args, protocol, binary);
                let mut engine = TrzszDownloadEngine::new(cfg!(windows) && !binary);
                let mut received = BTreeMap::<String, Vec<u8>>::new();
                'download: loop {
                    for frame in peer.read() {
                        if let Some(config) = parse_trzsz_config_frame(&frame) {
                            engine.configure(&config).unwrap();
                            continue;
                        }
                        let step = engine
                            .observe_frame(frame.clone())
                            .unwrap_or_else(|e| panic!("download {frame:?}: {e:?}"));
                        let mut done = false;
                        for event in step.events {
                            match event {
                                TrzszDownloadEvent::FileName { name } => {
                                    received.entry(name).or_default();
                                }
                                TrzszDownloadEvent::Data { name, bytes, .. } => {
                                    received.entry(name).or_default().extend(bytes)
                                }
                                TrzszDownloadEvent::Completed { .. } => done = true,
                                _ => {}
                            }
                        }
                        peer.send(step.responses);
                        if done {
                            peer.finish();
                            break 'download;
                        }
                    }
                }
                assert_eq!(received, files.iter().cloned().collect());
                args.push(output.path().to_string_lossy().into_owned());
                let mut peer = Peer::start("trz", &args, protocol, binary);
                let mut engine = TrzszUploadEngine::new(
                    cfg!(windows) && !binary,
                    files
                        .iter()
                        .map(|(name, bytes)| TrzszUploadEntry {
                            name: name.clone(),
                            size: bytes.len() as i64,
                            payload: TrzszUploadPayload::Memory(bytes.clone()),
                            source: None,
                        })
                        .collect(),
                );
                'upload: loop {
                    for frame in peer.read() {
                        let step = if let Some(config) = parse_trzsz_config_frame(&frame) {
                            engine.configure(&config).unwrap();
                            engine.begin().unwrap()
                        } else {
                            engine
                                .observe_frame(frame.clone())
                                .unwrap_or_else(|e| panic!("upload {frame:?}: {e:?}"))
                        };
                        let done = step
                            .events
                            .iter()
                            .any(|e| matches!(e, TrzszUploadEvent::Completed { .. }));
                        peer.send(step.responses);
                        if done {
                            peer.finish();
                            break 'upload;
                        }
                    }
                }
                for (name, bytes) in &files {
                    assert_eq!(&std::fs::read(output.path().join(name)).unwrap(), bytes);
                }
            }
        }
    }
}
