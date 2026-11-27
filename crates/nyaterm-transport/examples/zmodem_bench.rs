//! Repeatable protocol/file-I/O and localhost runtime benchmarks using temporary files.
use std::collections::VecDeque;
use std::time::Instant;

use nyaterm_transport::zmodem::{
    ZmodemAction, ZmodemDirection, ZmodemEvent, ZmodemTransfer, ZmodemTransferOptions,
};
use sha2::{Digest, Sha256};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let value = |name: &str, default: &str| {
        args.windows(2)
            .find(|pair| pair[0] == name)
            .map_or_else(|| default.to_string(), |pair| pair[1].clone())
    };
    let size: usize = value("--bytes", "8388608").parse()?;
    let rtt_ms: u64 = value("--rtt-ms", "50").parse()?;
    let label = value("--label", "optimized");
    let default_window = ZmodemTransferOptions::reliable_transport()
        .streaming_window
        .to_string();
    let window: usize = value("--window", &default_window).parse()?;
    let nonstop = value("--receive", "nonstop") == "nonstop";
    let options = ZmodemTransferOptions {
        nonstop_receive: nonstop,
        streaming_window: window,
        buffered_files: value("--buffered", "1") == "1",
    };
    let root = tempfile::tempdir()?;
    let destination = root.path().join("received");
    std::fs::create_dir(&destination)?;
    let source = root.path().join("payload.bin");
    let mut seed = 0x1234_5678u32;
    let payload: Vec<u8> = (0..size)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as u8
        })
        .collect();
    std::fs::write(&source, &payload)?;
    if value("--layer", "protocol") == "runtime" {
        return runtime_bench(
            source,
            destination,
            payload,
            options,
            &label,
            PeerOptions {
                download: value("--direction", "upload") == "download",
                delay_ms: value("--delay-ms", "0").parse()?,
                rate_kib: value("--rate-kib", "0").parse()?,
            },
        );
    }
    let mut upload = ZmodemTransfer::with_options(ZmodemDirection::Upload, &[], options);
    let mut download = ZmodemTransfer::with_options(ZmodemDirection::Download, &[], options);
    let start = Instant::now();
    let mut queue = VecDeque::new();
    let mut now = 0u64;
    let mut waits = 0u64;
    let mut wire_bytes = 0usize;
    let mut queue_peak = 0usize;
    let mut complete = [false; 2];
    let mut enqueue = |actions: Vec<ZmodemAction>,
                       from: usize,
                       now: u64,
                       queue: &mut VecDeque<(u64, usize, Vec<u8>)>|
     -> anyhow::Result<()> {
        let mut batch = Vec::new();
        for action in actions {
            match action {
                ZmodemAction::SendToRemote(bytes) => {
                    if from == 0 {
                        waits += bytes.windows(2).filter(|p| *p == [0x18, b'k']).count() as u64;
                    }
                    wire_bytes += bytes.len();
                    batch.extend(bytes);
                }
                ZmodemAction::EmitEvent(ZmodemEvent::Failed { reason }) => {
                    anyhow::bail!("{reason}")
                }
                ZmodemAction::EmitEvent(ZmodemEvent::Complete { .. }) => complete[from] = true,
                _ => {}
            }
        }
        if !batch.is_empty() {
            queue.push_back((now + rtt_ms * 500, 1 - from, batch));
        }
        queue_peak = queue_peak.max(queue.iter().map(|(_, _, bytes)| bytes.len()).sum());
        Ok(())
    };
    enqueue(
        download.accept_download(destination.clone()),
        1,
        0,
        &mut queue,
    )?;
    enqueue(upload.accept_upload(vec![source]), 0, 0, &mut queue)?;
    while let Some((at, to, bytes)) = queue.pop_front() {
        now = now.max(at);
        let actions = if to == 0 {
            upload.feed_incoming(&bytes)
        } else {
            download.feed_incoming(&bytes)
        };
        enqueue(actions, to, now, &mut queue)?;
    }
    let elapsed = start.elapsed().as_secs_f64();
    let received = std::fs::read(destination.join("payload.bin"))?;
    anyhow::ensure!(
        received == payload && complete == [true, true],
        "incomplete transfer"
    );
    let simulated = now as f64 / 1_000_000.0;
    println!(
        "{}",
        serde_json::json!({
            "layer": "protocol_file_io", "label": label, "bytes": size, "window": window, "nonstop_receive": nonstop, "buffered_files": options.buffered_files,
            "elapsed_seconds": elapsed, "mib_per_second": size as f64 / 1048576.0 / elapsed,
            "simulated_rtt_ms": rtt_ms, "simulated_wire_seconds": simulated,
            "projected_mib_per_second": size as f64 / 1048576.0 / (elapsed + simulated),
            "sender_ack_waits": waits, "wire_bytes": wire_bytes, "protocol_queue_peak_bytes": queue_peak, "capacity_wait_nanoseconds": 0,
            "sha256": Sha256::digest(&received).iter().map(|b| format!("{b:02x}")).collect::<String>(), "success": true
        })
    );
    Ok(())
}

struct PeerOptions {
    download: bool,
    delay_ms: u64,
    rate_kib: u64,
}

fn runtime_bench(
    source: std::path::PathBuf,
    destination: std::path::PathBuf,
    payload: Vec<u8>,
    options: ZmodemTransferOptions,
    label: &str,
    peer_options: PeerOptions,
) -> anyhow::Result<()> {
    let PeerOptions {
        download,
        delay_ms,
        rate_kib,
    } = peer_options;
    use nyaterm_transport::zmodem::runtime::ZmodemRuntime;
    use nyaterm_transport::{SessionManager, TelnetSessionConfig};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::time::Duration;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let peer_destination = destination.clone();
    let peer_source = source.clone();
    let peer = std::thread::spawn(move || -> anyhow::Result<u64> {
        let (mut stream, _) = listener.accept()?;
        stream.set_nodelay(true)?;
        stream.set_read_timeout(Some(Duration::from_secs(30)))?;
        let direction = if download {
            ZmodemDirection::Upload
        } else {
            ZmodemDirection::Download
        };
        let mut transfer = ZmodemTransfer::with_options(direction, &[], options);
        let mut actions = if download {
            transfer.accept_upload(vec![peer_source])
        } else {
            transfer.accept_download(peer_destination)
        };
        let mut input = vec![0; 64 * 1024];
        let mut waits = 0;

        loop {
            let mut complete = false;
            for action in actions {
                match action {
                    ZmodemAction::SendToRemote(bytes) => {
                        waits += bytes
                            .windows(2)
                            .filter(|pair| *pair == [0x18, b'k'])
                            .count() as u64;
                        if download && rate_kib > 0 {
                            std::thread::sleep(Duration::from_secs_f64(
                                bytes.len() as f64 / (rate_kib * 1024) as f64,
                            ));
                        }
                        stream.write_all(&bytes)?;
                    }
                    ZmodemAction::EmitEvent(ZmodemEvent::Failed { reason }) => {
                        anyhow::bail!("peer: {reason}")
                    }
                    ZmodemAction::EmitEvent(ZmodemEvent::Complete { .. }) => complete = true,
                    _ => {}
                }
            }
            if complete {
                if !download {
                    // Receiver completion precedes the sender's final OO.
                    let mut finish = Vec::new();
                    while !finish.windows(2).any(|pair| pair == b"OO") {
                        let count = stream.read(&mut input)?;
                        anyhow::ensure!(count != 0, "missing final OO");
                        finish.extend_from_slice(&input[..count]);
                        anyhow::ensure!(finish.len() <= 32, "unexpected final bytes");
                    }
                }
                break;
            }
            let read = stream.read(&mut input)?;
            anyhow::ensure!(read != 0, "peer saw premature EOF");
            if delay_ms > 0 {
                std::thread::sleep(Duration::from_millis(delay_ms));
            }
            if !download && rate_kib > 0 {
                std::thread::sleep(Duration::from_secs_f64(
                    read as f64 / (rate_kib * 1024) as f64,
                ));
            }
            actions = transfer.feed_incoming(&input[..read]);
        }
        Ok(waits)
    });
    let manager = Arc::new(SessionManager::new());
    let info = manager.create_telnet_session(TelnetSessionConfig {
        host: "127.0.0.1".into(),
        port,
        raw_tcp: true,
        keep_alive_interval_secs: 0,
        ..Default::default()
    })?;
    let direction = if download {
        ZmodemDirection::Download
    } else {
        ZmodemDirection::Upload
    };
    let runtime = ZmodemRuntime::spawn(
        manager.clone(),
        info.id.clone(),
        ZmodemTransfer::with_options(direction, &[], options),
    )?;
    manager.assign_session_event_consumer(&info.id, runtime.consumer_id());
    runtime.activate(VecDeque::new());
    let start = Instant::now();
    if download {
        runtime.accept_download(destination.clone());
    } else {
        runtime.accept_upload(vec![source]);
    }
    let mut completed = false;
    let mut failure = None;
    while start.elapsed() < Duration::from_secs(120) && !completed && failure.is_none() {
        while let Some(event) = runtime.try_recv_event() {
            for action in event.actions {
                match action {
                    ZmodemAction::EmitEvent(ZmodemEvent::Complete { .. }) => completed = true,
                    ZmodemAction::EmitEvent(ZmodemEvent::Failed { reason }) => {
                        failure = Some(reason)
                    }
                    _ => {}
                }
            }
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    let elapsed = start.elapsed().as_secs_f64();
    let stats = runtime.stats();
    if !completed {
        runtime.cancel("benchmark stopped");
    }
    if !completed {
        manager.close(&info.id)?;
    }
    let peer_waits = peer
        .join()
        .map_err(|_| anyhow::anyhow!("peer panicked"))??;
    if completed {
        manager.close(&info.id)?;
    }
    anyhow::ensure!(
        failure.is_none() && completed,
        "runtime failed: {failure:?}"
    );
    let received = std::fs::read(destination.join("payload.bin"))?;
    anyhow::ensure!(received == payload, "runtime payload mismatch");
    println!(
        "{}",
        serde_json::json!({
            "layer": "runtime_tcp", "label": label, "direction": if download { "download" } else { "upload" },
            "bytes": payload.len(), "elapsed_seconds": elapsed, "mib_per_second": payload.len() as f64 / 1048576.0 / elapsed,
            "window": options.streaming_window, "nonstop_receive": options.nonstop_receive,
            "buffered_files": options.buffered_files,
            "peer_delivery_delay_ms": delay_ms, "peer_rate_kib": rate_kib,
            "sender_ack_waits": stats.sender_ack_waits + peer_waits,
            "stats": stats, "sha256": Sha256::digest(&received).iter().map(|b| format!("{b:02x}")).collect::<String>(), "success": true,
        })
    );
    Ok(())
}
