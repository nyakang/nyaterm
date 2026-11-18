//! Field rules shared by every network tool form.
//!
//! Services add protocol-specific checks on top (TFTP block size, IPv6
//! rejection) and call this for the fields they all expose.

use nyaterm_core::TFTP_MAX_CONCURRENCY_LIMIT;
use rust_i18n::t;

/// Per-field validation messages. `None` means the field is valid.
#[derive(Default, Clone)]
pub(in crate::features) struct FieldErrors {
    pub bind: Option<String>,
    pub port: Option<String>,
    pub root: Option<String>,
    pub timeout: Option<String>,
    pub concurrency: Option<String>,
}

impl FieldErrors {
    pub(in crate::features) fn any(&self) -> bool {
        self.bind.is_some()
            || self.port.is_some()
            || self.root.is_some()
            || self.timeout.is_some()
            || self.concurrency.is_some()
    }
}

/// Pure field validation. Directory existence/readability is passed in from a
/// probe because it is IO; everything else is derived from the current values.
pub(in crate::features) fn field_errors(
    bind: &str,
    port: &str,
    root: &str,
    timeout: &str,
    concurrency: &str,
    root_probe: Option<bool>,
) -> FieldErrors {
    let mut errors = FieldErrors::default();
    let bind = bind.trim();
    if !bind.is_empty() && bind.parse::<std::net::IpAddr>().is_err() {
        errors.bind = Some(t!("networktools.common.error.bind").to_string());
    }
    if !matches!(port.trim().parse::<u16>(), Ok(value) if value != 0) {
        errors.port = Some(t!("networktools.common.error.port").to_string());
    }
    if !matches!(timeout.trim().parse::<u8>(), Ok(value) if value != 0) {
        errors.timeout = Some(t!("networktools.common.error.timeout").to_string());
    }
    if !matches!(
        concurrency.trim().parse::<u16>(),
        Ok(value) if (1..=TFTP_MAX_CONCURRENCY_LIMIT).contains(&value)
    ) {
        errors.concurrency = Some(
            t!(
                "networktools.common.error.concurrency",
                max = TFTP_MAX_CONCURRENCY_LIMIT
            )
            .to_string(),
        );
    }
    if root.trim().is_empty() {
        errors.root = Some(t!("networktools.common.error.rootDirRequired").to_string());
    } else if root_probe == Some(false) {
        errors.root = Some(t!("networktools.common.error.rootDirUnreadable").to_string());
    }
    errors
}
