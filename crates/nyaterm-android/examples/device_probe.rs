//! Native bundled-ADB resolution probe. No device output is logged.
use nyaterm_android::adb::client::AdbClient;
use nyaterm_android::adb::shell::execute;
use nyaterm_core::models::android::AdbDeviceSelector;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let endpoint = std::env::args().nth(1).ok_or("host:port required")?;
    let (host, port) = endpoint.rsplit_once(':').ok_or("host:port required")?;
    let mut client = AdbClient::local(None)?;
    // An optional private server port lets acceptance test startup without
    // disturbing the shared server used by other applications.
    if let Some(port) = std::env::args().nth(2) {
        client.server.set_port(port.parse()?);
    }
    let binding = client.bind(
        &AdbDeviceSelector::Tcp {
            host: host.into(),
            port: port.parse()?,
        },
        1,
    )?;
    let bytes = execute(&binding, b"printf nyaterm-native-adb-probe", 4096)?;
    if bytes != b"nyaterm-native-adb-probe" {
        return Err("native shell verification failed".into());
    }
    if AdbClient::local(Some(std::path::Path::new("/missing/explicit-adb"))).is_ok() {
        return Err("invalid explicit path unexpectedly accepted".into());
    }
    println!("Default bundled ADB, native shell channel and invalid explicit override passed");
    Ok(())
}
