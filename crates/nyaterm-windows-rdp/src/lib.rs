//! Thread-affine system RDP host. No GPUI dependency or RDP wire decoder.

pub mod cleanup;
#[cfg(windows)]
mod controller;
pub mod display;
pub mod lifecycle;
#[cfg(windows)]
mod native;
#[cfg(windows)]
pub use controller::{NativeRdpCompleted, NativeRdpControl, NativeRdpOperation};
#[cfg(windows)]
pub use native::{
    HostCreateError, HostStatus, NativeRdpCapabilities, NativeRdpHost, Viewport, maintain_retired,
    retired_count,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeRdpError {
    pub operation: &'static str,
    pub hresult: i32,
}

impl std::fmt::Display for NativeRdpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Windows RDP {} failed (0x{:08X})",
            self.operation, self.hresult as u32
        )
    }
}

impl std::error::Error for NativeRdpError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeRdpEvent {
    pub generation: u64,
    pub kind: NativeRdpEventKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeRdpEventKind {
    Connecting,
    Connected,
    LoginComplete,
    Disconnected { basic: u32, extended: u32 },
    Fatal { code: i32 },
    Reconnecting,
    CloseConfirmed,
    FocusReleased,
    Reconnected,
    AuthenticationWarningDisplayed,
    AuthenticationWarningDismissed,
    LogonError { code: i32 },
}
