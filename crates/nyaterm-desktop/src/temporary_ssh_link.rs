//! Compatibility facade for UI-independent command/connection policy.
pub(crate) use nyaterm_core::temporary_connection::{
    TemporaryLinkProtocol, TemporarySerialLinkConfig, TemporarySshLinkConfig,
    TemporaryTelnetLinkConfig, build_temporary_serial_link, parse_temporary_ssh_link,
    parse_temporary_telnet_link,
};
