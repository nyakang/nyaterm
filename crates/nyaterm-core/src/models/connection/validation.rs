//! Edit-time domain validation. Loading old configurations does not invoke it.
use crate::models::{ConnectionUsernameSource, RdpDisplaySettings, SavedPassword};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionValidationError {
    PortInvalid,
    MissingAccount,
    UsernameRequired,
    BaudRateInvalid,
    RdpDisplayWidthInvalid,
    RdpDisplayHeightInvalid,
    RdpReconnectAttemptsInvalid,
    RdpRelayUnsupported,
    RdpWindowsAuthenticationRequired,
    VncUsernameTooLong,
    VncReconnectAttemptsInvalid,
    VncPasswordTooLong,
    PostLoginCommandRequired,
    PostLoginDelayInvalid,
    SftpShellDetectionTimeoutInvalid,
}

pub struct AccountSelection<'a> {
    pub reference: Option<&'a str>,
    pub require_saved_password: bool,
    pub username: &'a str,
    pub username_source: ConnectionUsernameSource,
    pub uses_account_identity: bool,
    pub ssh: bool,
}
pub fn validate_account_selection(
    selection: AccountSelection<'_>,
    accounts: &[SavedPassword],
) -> Result<(), ConnectionValidationError> {
    let account = selection
        .reference
        .and_then(|id| accounts.iter().find(|entry| entry.id == id));
    if (selection.reference.is_some() || selection.require_saved_password) && account.is_none() {
        return Err(ConnectionValidationError::MissingAccount);
    }
    let account_username = account
        .map(|entry| entry.username.trim())
        .filter(|value| !value.is_empty());
    let username = if selection.uses_account_identity
        && selection.username_source == ConnectionUsernameSource::Account
    {
        account_username.unwrap_or(selection.username.trim())
    } else {
        selection.username.trim()
    };
    if selection.ssh && username.is_empty() {
        return Err(ConnectionValidationError::UsernameRequired);
    }
    Ok(())
}
pub fn parse_port(value: &str) -> Result<u16, ConnectionValidationError> {
    value
        .trim()
        .parse::<u16>()
        .ok()
        .filter(|port| *port != 0)
        .ok_or(ConnectionValidationError::PortInvalid)
}
pub fn validate_baud_rate(value: u32) -> Result<(), ConnectionValidationError> {
    if (1..=4_000_000).contains(&value) {
        Ok(())
    } else {
        Err(ConnectionValidationError::BaudRateInvalid)
    }
}
pub fn validate_rdp_display(display: &RdpDisplaySettings) -> Result<(), ConnectionValidationError> {
    if !(640..=7680).contains(&display.width) {
        return Err(ConnectionValidationError::RdpDisplayWidthInvalid);
    }
    if !(480..=4320).contains(&display.height) {
        return Err(ConnectionValidationError::RdpDisplayHeightInvalid);
    }
    Ok(())
}
pub fn validate_rdp_reconnect(attempts: u32) -> Result<(), ConnectionValidationError> {
    if attempts > 20 {
        Err(ConnectionValidationError::RdpReconnectAttemptsInvalid)
    } else {
        Ok(())
    }
}
pub fn validate_native_rdp(
    relayed: bool,
    authentication: Option<&str>,
) -> Result<(), ConnectionValidationError> {
    if relayed {
        return Err(ConnectionValidationError::RdpRelayUnsupported);
    }
    if authentication != Some("system") {
        return Err(ConnectionValidationError::RdpWindowsAuthenticationRequired);
    }
    Ok(())
}
pub fn validate_vnc_username(username: &str) -> Result<(), ConnectionValidationError> {
    if username.trim().len() > 255 {
        Err(ConnectionValidationError::VncUsernameTooLong)
    } else {
        Ok(())
    }
}
pub fn validate_vnc_reconnect(attempts: u32) -> Result<(), ConnectionValidationError> {
    if attempts > 20 {
        Err(ConnectionValidationError::VncReconnectAttemptsInvalid)
    } else {
        Ok(())
    }
}
/// Only called for an active, explicitly replaced password; length is in bytes.
pub fn validate_vnc_password_length(
    mode: &str,
    bytes: usize,
) -> Result<(), ConnectionValidationError> {
    if bytes > if mode == "vnc_auth" { 8 } else { 255 } {
        Err(ConnectionValidationError::VncPasswordTooLong)
    } else {
        Ok(())
    }
}
pub fn validate_post_login_command(
    enabled: bool,
    command: &str,
) -> Result<(), ConnectionValidationError> {
    if enabled && command.trim().is_empty() {
        Err(ConnectionValidationError::PostLoginCommandRequired)
    } else {
        Ok(())
    }
}
pub fn validate_post_login_delay(delay: u64) -> Result<(), ConnectionValidationError> {
    if delay > 60_000 {
        Err(ConnectionValidationError::PostLoginDelayInvalid)
    } else {
        Ok(())
    }
}
pub fn validate_sftp_detection_timeout(timeout: u64) -> Result<(), ConnectionValidationError> {
    if (100..=60_000).contains(&timeout) {
        Ok(())
    } else {
        Err(ConnectionValidationError::SftpShellDetectionTimeoutInvalid)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AccountSelection, ConnectionValidationError, parse_port, validate_account_selection,
        validate_baud_rate, validate_native_rdp, validate_post_login_command,
        validate_post_login_delay, validate_rdp_display, validate_rdp_reconnect,
        validate_sftp_detection_timeout, validate_vnc_password_length, validate_vnc_reconnect,
        validate_vnc_username,
    };
    use crate::models::{ConnectionUsernameSource, RdpDisplaySettings, SavedPassword};

    #[test]
    fn account_identity_falls_back_without_requiring_a_secret_to_be_loaded() {
        let mut accounts = vec![SavedPassword {
            id: "account".into(),
            sort_order: 0,
            name: "fixture".into(),
            username: "  operator  ".into(),
            password: None,
            has_password: true,
        }];
        let selection = |reference, username| AccountSelection {
            reference,
            username,
            require_saved_password: true,
            username_source: ConnectionUsernameSource::Account,
            uses_account_identity: true,
            ssh: true,
        };
        assert_eq!(
            validate_account_selection(selection(Some("account"), ""), &accounts),
            Ok(())
        );
        assert_eq!(
            validate_account_selection(selection(Some("missing"), "fallback"), &accounts),
            Err(ConnectionValidationError::MissingAccount)
        );
        accounts[0].username = " ".into();
        assert_eq!(
            validate_account_selection(selection(Some("account"), "fallback"), &accounts),
            Ok(())
        );
        assert_eq!(
            validate_account_selection(selection(Some("account"), " "), &accounts),
            Err(ConnectionValidationError::UsernameRequired)
        );
        assert_eq!(
            validate_account_selection(selection(None, "fallback"), &accounts),
            Err(ConnectionValidationError::MissingAccount)
        );
    }

    #[test]
    fn numeric_edit_limits_preserve_inclusive_boundaries() {
        assert_eq!(parse_port(" 65535 "), Ok(65535));
        for value in ["", "0", "65536", "-1"] {
            assert!(parse_port(value).is_err());
        }
        for value in [1, 4_000_000] {
            assert!(validate_baud_rate(value).is_ok());
        }
        for value in [0, 4_000_001] {
            assert!(validate_baud_rate(value).is_err());
        }
        for (width, height) in [(640, 480), (7680, 4320)] {
            assert!(
                validate_rdp_display(&RdpDisplaySettings {
                    width,
                    height,
                    ..Default::default()
                })
                .is_ok()
            );
        }
        assert_eq!(
            validate_rdp_display(&RdpDisplaySettings {
                width: 639,
                height: 479,
                ..Default::default()
            }),
            Err(ConnectionValidationError::RdpDisplayWidthInvalid)
        );
        assert_eq!(
            validate_rdp_display(&RdpDisplaySettings {
                height: 4321,
                ..Default::default()
            }),
            Err(ConnectionValidationError::RdpDisplayHeightInvalid)
        );
        for attempts in [0, 20] {
            assert!(validate_rdp_reconnect(attempts).is_ok());
            assert!(validate_vnc_reconnect(attempts).is_ok());
        }
        assert!(validate_rdp_reconnect(21).is_err());
        assert!(validate_vnc_reconnect(21).is_err());
        for delay in [0, 60_000] {
            assert!(validate_post_login_delay(delay).is_ok());
        }
        assert!(validate_post_login_delay(60_001).is_err());
        for timeout in [100, 60_000] {
            assert!(validate_sftp_detection_timeout(timeout).is_ok());
        }
        for timeout in [99, 60_001] {
            assert!(validate_sftp_detection_timeout(timeout).is_err());
        }
    }

    #[test]
    fn conditional_rules_keep_native_rdp_error_order_and_vnc_byte_limits() {
        assert_eq!(
            validate_native_rdp(true, None),
            Err(ConnectionValidationError::RdpRelayUnsupported)
        );
        assert_eq!(
            validate_native_rdp(false, None),
            Err(ConnectionValidationError::RdpWindowsAuthenticationRequired)
        );
        assert_eq!(validate_native_rdp(false, Some("system")), Ok(()));
        assert!(validate_vnc_username(&"界".repeat(85)).is_ok());
        assert!(validate_vnc_username(&"界".repeat(86)).is_err());
        assert!(validate_vnc_password_length("vnc_auth", 8).is_ok());
        assert!(validate_vnc_password_length("vnc_auth", 9).is_err());
        assert!(validate_vnc_password_length("auto", 255).is_ok());
        assert!(validate_vnc_password_length("auto", 256).is_err());
        assert!(validate_post_login_command(false, " ").is_ok());
        assert!(validate_post_login_command(true, " ").is_err());
    }
}
