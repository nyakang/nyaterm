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
    let selector = if let Some((host, port)) = serial
        .rsplit_once(':')
        .filter(|(_, port)| port.parse::<u16>().is_ok())
    {
        AdbDeviceSelector::Tcp {
            host: host.into(),
            port: port.parse()?,
        }
    } else {
        AdbDeviceSelector::Usb {
            serial: serial.clone(),
        }
    };
    let seconds: u64 = args.get(2).map(|v| v.parse()).transpose()?.unwrap_or(10);
    let cycles: u32 = args.get(3).map(|v| v.parse()).transpose()?.unwrap_or(1);
    let fps = args.get(4).map(|v| v.parse()).transpose()?.unwrap_or(30);
    let visibility_probe = args.get(5).is_some_and(|value| value == "visibility");
    if visibility_probe && seconds < 3 {
        return Err("visibility probe needs at least three seconds".into());
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug");
    let adb = std::env::var_os("NYATERM_PROBE_ADB")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| root.join(format!("adb/adb{}", std::env::consts::EXE_SUFFIX)));
    let client = AdbClient::local(Some(&adb))?;
    for cycle in 0..cycles {
        let binding = client.bind(&selector, u64::from(cycle) + 1)?;
        let screen = ScreenHandle::start_with_helper(
            client.clone(),
            binding,
            AndroidScreenSettings {
                max_fps: fps,
                ..Default::default()
            },
            &root.join(format!(
                "nyaterm-android-helper{}",
                std::env::consts::EXE_SUFFIX
            )),
        )?;
        let started = Instant::now();
        let mut frames = 0u64;
        let mut first = None;
        let mut hidden_at = None;
        let mut resumed_at = None;
        let mut resume_frame = false;
        while started.elapsed() < Duration::from_secs(seconds) {
            if visibility_probe {
                if hidden_at.is_none() && started.elapsed() >= Duration::from_secs(seconds / 3) {
                    screen.send(Control::Presentation { visible: false })?;
                    hidden_at = Some(Instant::now());
                }
                if resumed_at.is_none() && started.elapsed() >= Duration::from_secs(seconds * 2 / 3)
                {
                    screen.send(Control::Presentation { visible: true })?;
                    resumed_at = Some(Instant::now());
                }
            }
            for event in screen.events() {
                match event {
                    Control::Fatal { code } => return Err(code.into()),
                    Control::Ready { .. } => println!("ready"),
                    _ => {}
                }
            }
            if let Some(frame) = screen.take_frame() {
                if hidden_at.is_some_and(|at| at.elapsed() > Duration::from_millis(500))
                    && resumed_at.is_none()
                {
                    return Err("hidden screen continued frame IPC after draining".into());
                }
                if resumed_at.is_some() {
                    resume_frame = true;
                }
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
                screen.release_frame(frame);
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
        if visibility_probe && !resume_frame {
            return Err("cached frame did not resume".into());
        }
        if visibility_probe {
            println!("hidden frame IPC stopped; cached frame resumed");
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
