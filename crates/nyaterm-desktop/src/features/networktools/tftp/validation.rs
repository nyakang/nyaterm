//! TFTP field rules: shared field checks plus the protocol-specific ones.

use std::net::IpAddr;

use nyaterm_core::{DEFAULT_TFTP_BIND_ADDRESS, TFTP_BLOCK_SIZE_MAX, TFTP_BLOCK_SIZE_MIN};
use rust_i18n::t;

use crate::features::networktools::common::validation::{
    FieldErrors, field_errors as common_field_errors,
};

/// TFTP form errors: the shared fields plus the protocol-only block size.
#[derive(Default, Clone)]
pub(in crate::features) struct TftpFieldErrors {
    pub common: FieldErrors,
    pub block: Option<String>,
}

impl TftpFieldErrors {
    pub(in crate::features) fn any(&self) -> bool {
        self.common.any() || self.block.is_some()
    }
}

/// Pure field validation for the TFTP form. Directory existence/readability is
/// passed in from a probe because it is IO.
pub(in crate::features) fn field_errors(
    bind: &str,
    port: &str,
    root: &str,
    block: &str,
    timeout: &str,
    concurrency: &str,
    root_probe: Option<bool>,
) -> TftpFieldErrors {
    let mut common = common_field_errors(bind, port, root, timeout, concurrency, root_probe);

    // The shared check only rejects unparsable input; TFTP additionally refuses
    // IPv6 because its session sockets rebind to `0.0.0.0`.
    if common.bind.is_none() {
        let value = if bind.trim().is_empty() {
            DEFAULT_TFTP_BIND_ADDRESS
        } else {
            bind.trim()
        };
        if matches!(value.parse::<IpAddr>(), Ok(IpAddr::V6(_))) {
            common.bind = Some(t!("networktools.tftp.error.bindIpv6Unsupported").to_string());
        }
    }

    let block = if (TFTP_BLOCK_SIZE_MIN..=TFTP_BLOCK_SIZE_MAX)
        .contains(&block.trim().parse::<u16>().unwrap_or(0))
    {
        None
    } else {
        Some(t!("networktools.tftp.error.blockSize").to_string())
    };

    TftpFieldErrors { common, block }
}

#[cfg(test)]
mod tests {
    use super::field_errors;

    #[test]
    fn ipv6_bind_addresses_are_rejected_while_ipv4_passes() {
        let ipv6 = field_errors("::1", "69", "srv", "512", "3", "8", None);
        assert!(ipv6.common.bind.is_some());
        assert!(ipv6.common.port.is_none());
        assert!(ipv6.common.root.is_none());
        assert!(ipv6.block.is_none());
        assert!(ipv6.common.timeout.is_none());
        assert!(ipv6.common.concurrency.is_none());

        let ipv4 = field_errors("192.168.1.10", "69", "srv", "512", "3", "8", None);
        assert!(!ipv4.any());

        let wildcard = field_errors("0.0.0.0", "69", "srv", "512", "3", "8", None);
        assert!(!wildcard.any());
    }

    #[test]
    fn unparsable_bind_addresses_are_rejected() {
        let errors = field_errors("not-an-ip", "69", "srv", "512", "3", "8", None);
        assert!(errors.common.bind.is_some());
    }

    #[test]
    fn block_size_stays_inside_the_protocol_range() {
        assert!(
            field_errors("0.0.0.0", "69", "srv", "7", "3", "8", None)
                .block
                .is_some()
        );
        assert!(
            field_errors("0.0.0.0", "69", "srv", "65465", "3", "8", None)
                .block
                .is_some()
        );
        assert!(
            field_errors("0.0.0.0", "69", "srv", "8", "3", "8", None)
                .block
                .is_none()
        );
    }
}
