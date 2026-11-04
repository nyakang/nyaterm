//! Launch the system RDP client without owning its window or credentials.

#[derive(Debug)]
pub(crate) enum MstscError {
    InvalidHost,
    InvalidPort,
    UnsupportedPlatform,
    RelayUnsupported,
    Spawn(std::io::Error),
}

pub(crate) struct MstscTarget(String);

impl MstscTarget {
    pub(crate) fn new(host: &str, port: u16) -> Result<Self, MstscError> {
        let host = host.trim();
        if host.is_empty() || host.chars().any(|c| c.is_control() || c.is_whitespace()) {
            return Err(MstscError::InvalidHost);
        }
        if port == 0 {
            return Err(MstscError::InvalidPort);
        }
        let server = if host.contains(':') {
            let address = host
                .strip_prefix('[')
                .and_then(|host| host.strip_suffix(']'))
                .unwrap_or(host);
            // Brackets are meaningful only around an IPv6 literal. Reject an
            // embedded port rather than sending an ambiguous target to mstsc.
            let ip = address.split_once('%').map_or(address, |(ip, _)| ip);
            let valid_scope = address.split_once('%').is_none_or(|(_, scope)| {
                !scope.is_empty()
                    && scope
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
            });
            if ip.parse::<std::net::Ipv6Addr>().is_err() || !valid_scope {
                return Err(MstscError::InvalidHost);
            }
            format!("[{address}]")
        } else {
            if host.contains(['[', ']', '/', '\\', '"']) {
                return Err(MstscError::InvalidHost);
            }
            host.to_string()
        };
        Ok(Self(if port == 3389 {
            server
        } else {
            format!("{server}:{port}")
        }))
    }

    pub(crate) fn validate_route(windows: bool, has_relay: bool) -> Result<(), MstscError> {
        if !windows {
            return Err(MstscError::UnsupportedPlatform);
        }
        if has_relay {
            return Err(MstscError::RelayUnsupported);
        }
        Ok(())
    }

    fn argument(&self) -> String {
        format!("/v:{}", self.0)
    }
}

pub(crate) fn launch(target: MstscTarget) -> Result<(), MstscError> {
    MstscTarget::validate_route(cfg!(windows), false)?;
    // Do not wait for, kill, or attach the system client's independent window.
    std::process::Command::new("mstsc.exe")
        .arg(target.argument())
        .spawn()
        .map_err(MstscError::Spawn)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{MstscError, MstscTarget};

    #[test]
    fn formats_one_target_argument_for_domain_ipv4_and_ipv6() {
        for (host, port, expected) in [
            (" server.example.com ", 3389, "/v:server.example.com"),
            ("192.0.2.10", 3390, "/v:192.0.2.10:3390"),
            ("2001:db8::1", 3389, "/v:[2001:db8::1]"),
            ("[2001:db8::1]", 3390, "/v:[2001:db8::1]:3390"),
            ("fe80::1%12", 3389, "/v:[fe80::1%12]"),
        ] {
            assert_eq!(MstscTarget::new(host, port).unwrap().argument(), expected);
        }
    }

    #[test]
    fn rejects_empty_malformed_and_control_character_targets_and_zero_ports() {
        for host in [
            "",
            "  ",
            "server\0",
            "server\n/v:other",
            "server /admin",
            "[::1",
            "::1]",
            "host:3390",
            "/admin",
            "[host]",
        ] {
            assert!(matches!(
                MstscTarget::new(host, 3389),
                Err(MstscError::InvalidHost)
            ));
        }
        assert!(matches!(
            MstscTarget::new("host", 0),
            Err(MstscError::InvalidPort)
        ));
    }

    #[test]
    fn external_client_requires_windows_and_a_direct_route() {
        assert!(MstscTarget::validate_route(true, false).is_ok());
        assert!(matches!(
            MstscTarget::validate_route(false, false),
            Err(MstscError::UnsupportedPlatform)
        ));
        assert!(matches!(
            MstscTarget::validate_route(true, true),
            Err(MstscError::RelayUnsupported)
        ));
    }
}
