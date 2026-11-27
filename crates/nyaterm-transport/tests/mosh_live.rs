//! Disposable fixture: scripts/test/mosh. No endpoint keys or terminal data are logged.
use nyaterm_core::mosh::MoshSettings;
use nyaterm_terminal::TerminalScreen;
use nyaterm_transport::mosh::MoshSessionConfig;
use nyaterm_transport::{
    SessionEvent, SessionManager, SshHostKey, SshHostKeyDecision, SshHostKeyVerifier,
    SshKeyAuthConfig, SshSessionConfig, open_ssh_multiplex_handle,
};
use std::sync::Arc;
use std::time::{Duration, Instant};

struct FixtureKey;
impl SshHostKeyVerifier for FixtureKey {
    fn verify(&self, _: &SshHostKey) -> Result<SshHostKeyDecision, String> {
        Ok(SshHostKeyDecision::Accept)
    }
}
fn wait_for(manager: &SessionManager, screen: &mut TerminalScreen, text: &str) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        for event in manager.drain_events(128).unwrap().events {
            match event {
                SessionEvent::MoshFrame { frame, .. } => {
                    screen.resize(frame.cols, frame.rows);
                    screen.advance(&frame.data);
                }
                SessionEvent::Exited { .. } | SessionEvent::Error { .. } => {
                    panic!("Mosh session ended prematurely")
                }
                _ => {}
            }
        }
        if screen.all_text().is_some_and(|value| value.contains(text)) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("Expected fixture output was not confirmed before deadline");
}

#[test]
#[ignore = "requires disposable Docker SSH/Mosh fixture"]
fn bootstrap_resize_unicode_and_auxiliary_disconnect() {
    let host = std::env::var("NYATERM_MOSH_TEST_HOST").expect("fixture host");
    let key_path = std::env::var("NYATERM_MOSH_TEST_KEY").expect("fixture private key path");
    let ssh = SshSessionConfig {
        host,
        username: "root".into(),
        name: "Mosh fixture".into(),
        key_auth: Some(SshKeyAuthConfig {
            key_data: std::fs::read_to_string(key_path).unwrap().into(),
            cert_data: None,
            passphrase: None,
        }),
        host_key_verifier: Some(Arc::new(FixtureKey)),
        encoding: "UTF-8".into(),
        cols: 80,
        rows: 24,
        ..Default::default()
    };
    let connection = open_ssh_multiplex_handle(ssh.clone()).expect("fixture SSH authentication");
    let manager = SessionManager::new();
    let info = manager
        .create_mosh_session(
            MoshSessionConfig {
                ssh: ssh.clone(),
                settings: MoshSettings {
                    udp_port_start: 60000,
                    udp_port_end: 60000,
                    ..Default::default()
                },
            },
            connection.clone(),
        )
        .expect("Mosh bootstrap");
    let mut screen = TerminalScreen::default();
    // The expected token is assembled remotely so shell echo cannot satisfy the assertion.
    manager
        .write(&info.id, b"printf '\\116\\131\\101-READY\\n'\r")
        .unwrap();
    wait_for(&manager, &mut screen, "NYA-READY");
    assert!(manager.write_raw(&info.id, b"raw").is_err());
    manager.resize(&info.id, 100, 30).unwrap();
    manager.write(&info.id, b"stty size\r").unwrap();
    wait_for(&manager, &mut screen, "30 100");
    manager
        .write(
            &info.id,
            "printf '\\344\\270\\255\\346\\226\\207\\n'\r".as_bytes(),
        )
        .unwrap();
    wait_for(&manager, &mut screen, "中文");
    connection.disconnect().unwrap();
    manager
        .write(&info.id, b"printf '\\116\\131\\101-UDP-ALIVE\\n'\r")
        .unwrap();
    wait_for(&manager, &mut screen, "NYA-UDP-ALIVE");
    let auxiliary =
        open_ssh_multiplex_handle(ssh).expect("auxiliary SSH can reconnect independently");
    auxiliary.disconnect().unwrap();
    if let Ok(container) = std::env::var("NYATERM_MOSH_TEST_CONTAINER") {
        struct RestoreNetwork(String);
        impl Drop for RestoreNetwork {
            fn drop(&mut self) {
                let _ = std::process::Command::new("docker")
                    .args(["exec", &self.0, "tc", "qdisc", "del", "dev", "eth0", "root"])
                    .output();
            }
        }
        let status = std::process::Command::new("docker")
            .args([
                "exec", &container, "tc", "qdisc", "add", "dev", "eth0", "root", "netem", "loss",
                "100%",
            ])
            .status()
            .unwrap();
        assert!(status.success());
        let restore = RestoreNetwork(container);
        manager
            .write(&info.id, b"printf '\\116\\131\\101-RECOVERED\\n'\r")
            .unwrap();
        std::thread::sleep(Duration::from_secs(3));
        drop(restore);
        wait_for(&manager, &mut screen, "NYA-RECOVERED");
    }
    let started = Instant::now();
    manager.close(&info.id).unwrap();
    assert!(started.elapsed() < Duration::from_secs(5));
}
