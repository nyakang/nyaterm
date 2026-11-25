//! Compatibility facade for the independently versioned native contract.
pub use nyaterm_native_protocol::spice::protocol::{
    MAX_SPICE_CLIPBOARD_BYTES, MAX_SPICE_INPUT_BATCH, MAX_SPICE_TRANSFER_FILES,
    SPICE_PROTOCOL_VERSION, SpiceCapabilities, SpiceControl, SpiceCredentialLifetime,
    SpiceErrorKind, SpiceInput, SpiceMouseMode, SpiceSessionConfig, SpiceSessionState,
    SpiceTransferState,
};
