//! Headless live-device acceptance probe; never persists pixels or clipboard contents.
use nyaterm_android::adb::client::AdbClient;
use nyaterm_android::ipc::Control;
use nyaterm_android::screen::ScreenHandle;
use nyaterm_core::models::android::{AdbDeviceSelector, AndroidScreenSettings};
use std::path::Path;
use std::time::{Duration, Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let serial = args.get(1).ok_or("provide host:port")?;
    let (host, port) = serial.rsplit_once(':').ok_or("provide host:port")?;
    let seconds: u64 = args.get(2).map(|v| v.parse()).transpose()?.unwrap_or(10);
    let cycles: u32 = args.get(3).map(|v| v.parse()).transpose()?.unwrap_or(1);
    let fps = args.get(4).map(|v| v.parse()).transpose()?.unwrap_or(30);
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug");
    let client = AdbClient::local(Some(&root.join("adb/adb")))?;
    for cycle in 0..cycles {
        let binding = client.bind(
            &AdbDeviceSelector::Tcp {
                host: host.into(),
                port: port.parse()?,
            },
            u64::from(cycle) + 1,
        )?;
        let screen = ScreenHandle::start_with_helper(
            client.clone(),
            binding,
            AndroidScreenSettings {
                max_fps: fps,
                ..Default::default()
            },
            &root.join("nyaterm-android-helper"),
        )?;
        let started = Instant::now();
        let mut frames = 0u64;
        let mut first = None;
        while started.elapsed() < Duration::from_secs(seconds) {
            for event in screen.events() {
                match event {
                    Control::Fatal { code } => return Err(code.into()),
                    Control::Ready { .. } => println!("ready"),
                    _ => {}
                }
            }
            if let Some(frame) = screen.take_frame() {
                first.get_or_insert_with(|| {
                    println!(
                        "first frame {}x{} after {:?}",
                        frame.width,
                        frame.height,
                        started.elapsed()
                    );
                    started.elapsed()
                });
                frames += 1;
                screen.acknowledge(frame.sequence);
            }
            if screen.is_closed() {
                return Err("screen closed unexpectedly".into());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        screen.close();
        if !screen.wait_closed(Duration::from_secs(20)) {
            return Err("screen cleanup timed out".into());
        }
        if let Some(cleanup) = screen
            .cleanup_ticket
            .lock()
            .map_err(|_| "cleanup lock")?
            .as_ref()
        {
            return Err(format!(
                "remote cleanup deferred: {}",
                cleanup.last_error.as_deref().unwrap_or("unknown failure")
            )
            .into());
        }
        if first.is_none() {
            return Err("no decoded frames".into());
        }
        println!(
            "cycle={} frames={} elapsed={:?}",
            cycle + 1,
            frames,
            started.elapsed()
        );
    }
    Ok(())
}
