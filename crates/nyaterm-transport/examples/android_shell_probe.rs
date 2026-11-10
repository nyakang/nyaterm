//! Headless Android PTY acceptance using only synthetic command output.
use nyaterm_android::adb::client::AdbClient;
use nyaterm_core::models::android::AdbDeviceSelector;
use nyaterm_transport::adb_shell::AdbShellConfig;
use nyaterm_transport::{SessionEvent, SessionManager};
use std::time::{Duration, Instant};
fn until(manager: &SessionManager, id: &str, needle: &[u8]) -> anyhow::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut bytes = Vec::new();
    while Instant::now() < deadline {
        for event in manager.drain_events(100)?.events {
            match event {
                SessionEvent::Output { session_id, data } if session_id == id => {
                    bytes.extend(data);
                    anyhow::ensure!(bytes.len() < 65536, "synthetic output limit");
                }
                SessionEvent::Exited { session_id, .. } if session_id == id => {
                    anyhow::bail!("Android shell exited unexpectedly")
                }
                _ => {}
            }
        }
        if bytes.windows(needle.len()).any(|value| value == needle) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    anyhow::bail!("Android PTY response timed out")
}
fn main() -> anyhow::Result<()> {
    let endpoint = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("host:port required"))?;
    let (host, port) = endpoint
        .rsplit_once(':')
        .ok_or_else(|| anyhow::anyhow!("host:port required"))?;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug");
    let client = AdbClient::local(Some(
        &root.join(format!("adb/adb{}", std::env::consts::EXE_SUFFIX)),
    ))?;
    let binding = client.bind(
        &AdbDeviceSelector::Tcp {
            host: host.into(),
            port: port.parse()?,
        },
        1,
    )?;
    let manager = SessionManager::new();
    let config = AdbShellConfig {
        name: "Synthetic PTY probe".into(),
        binding,
        cols: 80,
        rows: 24,
    };
    let first = manager.create_adb_session(config.clone())?;
    let second = manager.create_adb_session(config)?;
    manager.resize_with_pixels(&first.id, 111, 37, 800, 600)?;
    manager.write(&first.id, b"stty size\n")?;
    until(&manager, &first.id, b"37 111")?;
    manager.write(&first.id, b"sleep 10\n")?;
    std::thread::sleep(Duration::from_millis(300));
    manager.write(&first.id, b"\x03")?;
    manager.write(&first.id, b"printf 'nya'; printf 'ctrlc\\n'\n")?;
    until(&manager, &first.id, b"nyactrlc")?;
    manager.close_detached(&first.id)?;
    manager.write(&second.id, b"printf 'nya'; printf 'independent\\n'\n")?;
    until(&manager, &second.id, b"nyaindependent")?;
    manager.write(&second.id, b"exit 7\n")?;
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut exited = false;
    while Instant::now() < deadline && !exited {
        for event in manager.drain_events(100)?.events {
            if let SessionEvent::Exited { session_id, reason } = event
                && session_id == second.id
            {
                anyhow::ensure!(reason == "AndroidShellExit:7", "wrong Android exit status");
                exited = true;
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    anyhow::ensure!(exited, "Android exit event missing");
    manager.close_detached(&second.id)?;
    println!("Native PTY resize, Ctrl+C, exit status and independent Shell channels passed");
    Ok(())
}
