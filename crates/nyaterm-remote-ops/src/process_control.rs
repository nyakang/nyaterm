//! Host-specific process controls; unsupported signals never become a kill.
use crate::host::HostOs;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessControlCapabilities {
    Posix,
    TerminateOnly,
}

impl ProcessControlCapabilities {
    pub const fn for_os(os: HostOs) -> Self {
        match os {
            HostOs::Windows => Self::TerminateOnly,
            _ => Self::Posix,
        }
    }

    pub const fn supports_priority(self) -> bool {
        matches!(self, Self::Posix)
    }

    pub fn ensure_signal(self, signal: &str) -> Result<(), UnsupportedProcessControl> {
        if signal == "KILL"
            || (self == Self::Posix && matches!(signal, "TERM" | "HUP" | "STOP" | "CONT"))
        {
            Ok(())
        } else {
            Err(UnsupportedProcessControl)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnsupportedProcessControl;

impl fmt::Display for UnsupportedProcessControl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("this host does not support this process control; Windows only supports explicit forced termination")
    }
}
impl std::error::Error for UnsupportedProcessControl {}

#[cfg(test)]
mod tests {
    use super::ProcessControlCapabilities;
    use crate::host::HostOs;

    #[test]
    fn windows_rejects_every_non_kill_signal_and_priority() {
        let caps = ProcessControlCapabilities::for_os(HostOs::Windows);
        for signal in ["TERM", "HUP", "STOP", "CONT", "INT", ""] {
            assert!(caps.ensure_signal(signal).is_err());
        }
        assert!(caps.ensure_signal("KILL").is_ok());
        assert!(!caps.supports_priority());
    }

    #[test]
    fn posix_retains_the_supported_signal_allowlist() {
        let caps = ProcessControlCapabilities::for_os(HostOs::OtherUnix);
        for signal in ["TERM", "HUP", "STOP", "CONT", "KILL"] {
            assert!(caps.ensure_signal(signal).is_ok());
        }
        assert!(caps.ensure_signal("injected").is_err());
        assert!(caps.supports_priority());
    }
}
