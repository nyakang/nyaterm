//! Live control acceptance. Uses Settings; persists no screen or clipboard data.
use nyaterm_android::adb::client::AdbClient;
use nyaterm_android::adb::shell::execute;
use nyaterm_android::ipc::{Control, Frame};
use nyaterm_android::screen::ScreenHandle;
use nyaterm_core::models::android::{AdbDeviceSelector, AndroidScreenSettings};
use std::path::Path;
use std::time::{Duration, Instant};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn next_frame(screen: &ScreenHandle, generation: u64) -> Result<Frame> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        for event in screen.events() {
            if let Control::Fatal { code } = event {
                return Err(code.into());
            }
        }
        if let Some(frame) = screen.take_frame() {
            if frame.generation >= generation {
                return Ok(frame);
            }
            screen.acknowledge(frame.sequence);
        }
        if screen.is_closed() {
            return Err("screen exited".into());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    Err("frame deadline exceeded".into())
}

fn main() -> Result<()> {
    let endpoint = std::env::args().nth(1).ok_or("host:port required")?;
    let (host, port) = endpoint.rsplit_once(':').ok_or("host:port required")?;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug");
    let client = AdbClient::local(Some(
        &root.join(format!("adb/adb{}", std::env::consts::EXE_SUFFIX)),
    ))?;
    let selector = AdbDeviceSelector::Tcp {
        host: host.into(),
        port: port.parse()?,
    };
    let binding = client.bind(&selector, 1)?;
    execute(
        &binding,
        b"am start -a android.settings.SETTINGS >/dev/null",
        4096,
    )?;
    let screen = ScreenHandle::start_with_helper(
        client.clone(),
        binding,
        AndroidScreenSettings::default(),
        &root.join(format!(
            "nyaterm-android-helper{}",
            std::env::consts::EXE_SUFFIX
        )),
    )?;
    let initial = next_frame(&screen, 1)?;
    let touch = |action, x, y| Control::Touch {
        action,
        x,
        y,
        width: initial.capture_width,
        height: initial.capture_height,
        generation: initial.generation,
    };
    // Tap the Settings title, then drag its list. No preferences are toggled.
    screen.send(touch(
        0,
        initial.capture_width / 2,
        initial.capture_height / 12,
    ))?;
    screen.send(touch(
        1,
        initial.capture_width / 2,
        initial.capture_height / 12,
    ))?;
    screen.send(touch(
        0,
        initial.capture_width / 2,
        initial.capture_height * 3 / 4,
    ))?;
    for step in 0..20 {
        screen.send(touch(
            2,
            initial.capture_width / 2,
            initial.capture_height * (60 - step) / 80,
        ))?;
        std::thread::sleep(Duration::from_millis(20));
    }
    screen.send(touch(
        1,
        initial.capture_width / 2,
        initial.capture_height / 2,
    ))?;
    screen.send(Control::Scroll {
        x: initial.capture_width / 2,
        y: initial.capture_height / 2,
        width: initial.capture_width,
        height: initial.capture_height,
        generation: initial.generation,
        horizontal: 0.0,
        vertical: 1.0,
    })?;
    std::thread::sleep(Duration::from_secs(3));
    if screen.take_frame().is_some() {
        return Err("more than one frame was sent without credit".into());
    }
    screen.acknowledge(initial.sequence);
    let latest = next_frame(&screen, initial.generation)?;
    if latest.sequence <= initial.sequence || latest.pts < initial.pts {
        return Err("slow consumer did not receive the latest decoded frame".into());
    }
    screen.acknowledge(latest.sequence);
    screen.send(Control::Rotate)?;
    let rotated = next_frame(&screen, initial.generation + 1)?;
    screen.acknowledge(rotated.sequence);
    // Old geometry is deliberately rejected by the helper.
    screen.send(touch(0, 1, 1))?;
    screen.send(Control::ReleaseInputs)?;
    screen.send(Control::Rotate)?;
    let restored = next_frame(&screen, rotated.generation + 1)?;
    screen.acknowledge(restored.sequence);
    screen.send(Control::Paste {
        text: "NyaTerm 中文粘贴验证".into(),
        sequence: 17,
    })?;
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut acknowledged = false;
    while Instant::now() < deadline && !acknowledged {
        for event in screen.events() {
            match event {
                Control::ClipboardAck { sequence: 17 } => acknowledged = true,
                Control::Fatal { code } => return Err(code.into()),
                _ => {}
            }
        }
        if let Some(frame) = screen.take_frame() {
            screen.acknowledge(frame.sequence);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    if !acknowledged {
        return Err("clipboard request acknowledgement missing".into());
    }
    screen.send(Control::ReleaseInputs)?;
    screen.close();
    if !screen.wait_closed(Duration::from_secs(20)) {
        return Err("cleanup deadline exceeded".into());
    }
    let pending = screen
        .cleanup_ticket
        .lock()
        .map_err(|_| "cleanup lock")?
        .take();
    if let Some(mut cleanup) = pending {
        // Test the same deferred cleanup route used by manual rebind.
        cleanup.binding = client.bind(&selector, 2)?;
        cleanup.retry()?;
        println!("deferred cleanup completed after rebind");
    }
    println!(
        "click, drag, scroll, rotation generations, Unicode request acknowledgement and slow-consumer credit passed"
    );
    Ok(())
}
