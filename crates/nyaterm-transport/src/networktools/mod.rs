//! Built-in, on-demand network tools.
//!
//! The module owns listening sockets and file IO for tools such as TFTP. It is
//! deliberately separate from session transports: nothing here registers a
//! `SessionKind`, appears in a connection list, or participates in session
//! restore. The desktop injects a bounded event channel and reads status
//! snapshots; it never calls into a session thread.

pub mod event;
pub mod registry;
pub mod status;
mod tftp;

pub use event::{NetworkToolEvent, TransferDirection, TransferOutcome};
pub use registry::{
    NetworkToolCapabilities, NetworkToolDescriptor, NetworkToolError, NetworkToolKind,
    NetworkToolRegistry, NetworkToolTransport,
};
pub use status::{NetworkToolState, NetworkToolStatus, TransferSnapshot};
