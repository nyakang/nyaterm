//! Dedicated, default-off process fixture; never included in release packages.

use nyaterm_remote_desktop::spice::ipc::{decode_control, encode_control};
use nyaterm_remote_desktop::spice::protocol::{SPICE_PROTOCOL_VERSION, SpiceControl};
use nyaterm_remote_desktop::{read_packet, write_packet};

fn main() {
    let executable = std::env::current_exe().unwrap();
    let mode = executable.file_stem().unwrap().to_string_lossy();
    let mut input = std::io::stdin();
    let packet = read_packet(&mut input).unwrap().unwrap();
    assert_eq!(
        decode_control(&packet).unwrap(),
        SpiceControl::Hello {
            version: SPICE_PROTOCOL_VERSION
        }
    );
    if mode.contains("crash") {
        std::process::exit(91);
    }
    write_packet(
        &mut std::io::stdout(),
        &encode_control(&SpiceControl::HelloAck {
            version: SPICE_PROTOCOL_VERSION,
        })
        .unwrap(),
    )
    .unwrap();
    // Simulate a native call that never returns to its command/heartbeat loop.
    std::thread::sleep(std::time::Duration::from_secs(60));
}
