use std::time::Duration;

use base64::Engine;
use russh::ChannelMsg;
use zeroize::Zeroizing;

use crate::SshMultiplexHandle;
use crate::mosh::MoshSessionConfig;

pub(super) struct Bootstrap {
    pub port: u16,
    pub key: Zeroizing<String>,
    pub address: Option<String>,
}

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub(super) fn command(config: &MoshSessionConfig) -> String {
    // Prefer an installed UTF-8 locale instead of assuming C.UTF-8 exists on every host.
    format!(
        "printf 'NYATERM_MOSH_ADDRESS %s\\n' \"$SSH_CONNECTION\"; \
         if [ \"$(locale charmap 2>/dev/null)\" != UTF-8 ]; then \
         nya_locale=$(locale -a 2>/dev/null | LC_ALL=C grep -i -m1 -E 'utf-?8$'); \
         [ -n \"$nya_locale\" ] || {{ printf 'NYATERM_MOSH_NO_UTF8\\n'; exit 78; }}; \
         export LC_ALL=\"$nya_locale\"; fi; exec {} new -s -p {}:{}",
        quote(&config.settings.server_path),
        config.settings.udp_port_start,
        config.settings.udp_port_end,
    )
}

fn parse(output: &[u8]) -> Result<Option<Bootstrap>, String> {
    let mut address = None;
    for line in output.split(|byte| *byte == b'\n') {
        // Login banners may use a legacy encoding; only our ASCII protocol
        // lines participate in the bootstrap handshake.
        let Ok(line) = std::str::from_utf8(line) else {
            continue;
        };
        let line = line.trim();
        if let Some(value) = line.strip_prefix("NYATERM_MOSH_ADDRESS ") {
            address = value
                .split_whitespace()
                .nth(2)
                .filter(|host| host.parse::<std::net::IpAddr>().is_ok())
                .map(str::to_owned);
        }
        if line == "NYATERM_MOSH_NO_UTF8" {
            return Err("Mosh requires a UTF-8 locale on the server".into());
        }
        if let Some(value) = line.strip_prefix("MOSH CONNECT ") {
            let mut parts = value.split_whitespace();
            let port = parts
                .next()
                .and_then(|p| p.parse::<u16>().ok())
                .filter(|p| *p != 0)
                .ok_or("Invalid Mosh bootstrap port")?;
            let key = parts.next().ok_or("Missing Mosh session key")?;
            if parts.next().is_some() || key.len() > 24 {
                return Err("Invalid Mosh session key".into());
            }
            let decoded = Zeroizing::new(
                base64::engine::general_purpose::STANDARD_NO_PAD
                    .decode(key.trim_end_matches('='))
                    .map_err(|_| "Invalid Mosh session key")?,
            );
            if decoded.len() != 16 {
                return Err("Invalid Mosh session key".into());
            }
            return Ok(Some(Bootstrap {
                port,
                key: Zeroizing::new(key.to_owned()),
                address,
            }));
        }
    }
    Ok(None)
}

pub(super) fn start(
    config: &MoshSessionConfig,
    connection: &SshMultiplexHandle,
) -> Result<Bootstrap, String> {
    let config = config.clone();
    let connection = connection.clone();
    let runtime = connection.clone();
    runtime
        .block_on(async move { run(config, connection).await.map_err(anyhow::Error::msg) })
        .map_err(|error| error.to_string())
}

async fn run(
    config: MoshSessionConfig,
    connection: SshMultiplexHandle,
) -> Result<Bootstrap, String> {
    let attempt = config.ssh.attempt.clone();
    attempt
        .until_cancelled(async {
            tokio::time::timeout(Duration::from_secs(30), async {
                let claim = connection.claim_primary_session();
                let target = connection.target_handle();
                let mut channel = target
                    .lock()
                    .await
                    .channel_open_session()
                    .await
                    .map_err(|_| "Unable to open Mosh bootstrap channel")?;
                channel
                    .request_pty(
                        false,
                        "xterm-256color",
                        config.ssh.cols as u32,
                        config.ssh.rows as u32,
                        0,
                        0,
                        &[],
                    )
                    .await
                    .map_err(|_| "Unable to allocate Mosh bootstrap PTY")?;
                channel
                    .exec(true, command(&config))
                    .await
                    .map_err(|_| "Mosh bootstrap command was rejected")?;
                drop(claim);
                let mut output = Zeroizing::new(Vec::new());
                let mut exit_status = None;
                while let Some(message) = channel.wait().await {
                    match message {
                        ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. } => {
                            if output.len() + data.len() > 64 * 1024 {
                                let _ = channel.close().await;
                                return Err("Mosh bootstrap response exceeded its limit".into());
                            }
                            output.extend_from_slice(&data);
                            // Parse only complete lines; banners may split UTF-8 between messages.
                            if let Some(end) = output.iter().rposition(|b| *b == b'\n')
                                && let Some(result) = parse(&output[..=end])?
                            {
                                let _ = channel.close().await;
                                return Ok(result);
                            }
                        }
                        ChannelMsg::ExitStatus {
                            exit_status: status,
                        } => exit_status = Some(status),
                        _ => {}
                    }
                }
                if let Some(result) = parse(&output)? {
                    return Ok(result);
                }
                Err(match exit_status {
                    Some(127) => "mosh-server was not found on the remote host",
                    Some(78) => "Mosh requires a UTF-8 locale on the server",
                    _ => "mosh-server did not return a connection endpoint",
                }
                .into())
            })
            .await
            .map_err(|_| "Mosh bootstrap timed out".to_string())?
        })
        .await?
}

#[cfg(test)]
mod tests {
    use super::{parse, quote};

    #[test]
    fn response_accepts_banner_and_extracts_authenticated_ssh_target() {
        let response = b"banner\r\nNYATERM_MOSH_ADDRESS 192.0.2.1 4567 192.0.2.2 22\r\nMOSH CONNECT 60001 AAAAAAAAAAAAAAAAAAAAAA\r\n";
        let result = parse(response).unwrap().unwrap();
        assert_eq!(result.port, 60001);
        assert_eq!(result.address.as_deref(), Some("192.0.2.2"));
    }

    #[test]
    fn invalid_keys_and_ports_are_rejected_without_echoing_response() {
        for line in ["MOSH CONNECT 0 private", "MOSH CONNECT 60001 private"] {
            assert!(!parse(line.as_bytes()).err().unwrap().contains("private"));
        }
        assert_eq!(quote("/path with space/a'b"), "'/path with space/a'\\''b'");
    }
}
