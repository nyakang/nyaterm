//! Registry of built-in network tools.
//!
//! The desktop owns user interaction and persistence; this registry owns each
//! running listener and exposes only non-blocking status snapshots. Services are
//! independent: one failing to bind, stopping, or being reconfigured never
//! touches another service's runtime, status or transfer list.
//!
//! Adding a tool means adding a `NetworkToolKind`, its runtime, and one branch
//! in `start`/`stop`; the per-service bookkeeping below is shared.

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex};

use nyaterm_core::models::networktools::NetworkToolsSettings;

use super::event::NetworkToolEvent;
use super::status::{NetworkToolStatus, TransferSnapshot};
use super::tftp::TftpServer;

static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NetworkToolKind {
    Tftp,
}

/// Transport a service listens on. Panel and form copy comes from the i18n
/// catalog; this only tells the UI that a service is UDP or TCP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkToolTransport {
    Udp,
    Tcp,
}

/// What a service needs from the shared panel and configuration window.
///
/// These flags let both surfaces render every service from one code path
/// instead of matching on the service kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetworkToolCapabilities {
    pub transport: NetworkToolTransport,
    pub uses_bind_address: bool,
    pub uses_port: bool,
    pub needs_root_dir: bool,
    pub supports_upload: bool,
}

impl NetworkToolKind {
    pub const ALL: [Self; 1] = [Self::Tftp];

    /// i18n keys only; the transport crate never loads a catalog.
    pub const fn name_key(self) -> &'static str {
        match self {
            Self::Tftp => "networktools.tftp.title",
        }
    }

    /// Short label that attributes transfer notifications ("TFTP · …").
    pub const fn short_name_key(self) -> &'static str {
        match self {
            Self::Tftp => "networktools.tftp.shortName",
        }
    }

    pub const fn description_key(self) -> &'static str {
        match self {
            Self::Tftp => "networktools.tftp.description",
        }
    }

    pub const fn icon(self) -> &'static str {
        "icons/networktools.svg"
    }

    /// Stable identifier for structured log fields and per-service input ids.
    pub const fn log_name(self) -> &'static str {
        match self {
            Self::Tftp => "tftp",
        }
    }

    /// Tracing target for this service's lifecycle events. It stays inside the
    /// `nyaterm` namespace and ends with the service's log name, which lets the
    /// application route each service into its own `nyaterm-<service>` file.
    pub const fn log_target(self) -> &'static str {
        match self {
            Self::Tftp => "nyaterm::networktools::tftp",
        }
    }

    pub const fn capabilities(self) -> NetworkToolCapabilities {
        match self {
            Self::Tftp => NetworkToolCapabilities {
                transport: NetworkToolTransport::Udp,
                uses_bind_address: true,
                uses_port: true,
                needs_root_dir: true,
                supports_upload: true,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkToolDescriptor {
    pub kind: NetworkToolKind,
    pub name_key: &'static str,
    pub short_name_key: &'static str,
    pub description_key: &'static str,
    pub icon: &'static str,
    pub capabilities: NetworkToolCapabilities,
}

impl From<NetworkToolKind> for NetworkToolDescriptor {
    fn from(kind: NetworkToolKind) -> Self {
        Self {
            kind,
            name_key: kind.name_key(),
            short_name_key: kind.short_name_key(),
            description_key: kind.description_key(),
            icon: kind.icon(),
            capabilities: kind.capabilities(),
        }
    }
}

#[derive(Debug)]
pub enum NetworkToolError {
    AlreadyRunning,
    InvalidSettings(String),
    RootUnavailable(String),
    Bind(String),
    Io(std::io::Error),
}

impl fmt::Display for NetworkToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyRunning => f.write_str("service is already running"),
            Self::InvalidSettings(message) => write!(f, "invalid settings: {message}"),
            Self::RootUnavailable(message) => write!(f, "shared directory unavailable: {message}"),
            Self::Bind(message) => write!(f, "cannot listen: {message}"),
            Self::Io(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for NetworkToolError {}

impl From<std::io::Error> for NetworkToolError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// One running service and the transfers it reported. Each service owns its
/// own transfer list so a restart or stop cannot drop another service's rows.
struct RunningService {
    runtime: ToolRuntime,
    transfers: Arc<Mutex<Vec<TransferSnapshot>>>,
}

enum ToolRuntime {
    Tftp(TftpServer),
}

impl ToolRuntime {
    fn is_running(&self) -> bool {
        match self {
            Self::Tftp(server) => server.is_running(),
        }
    }

    fn stop(&mut self) {
        match self {
            Self::Tftp(server) => server.stop(),
        }
    }
}

pub struct NetworkToolRegistry {
    events: SyncSender<NetworkToolEvent>,
    services: HashMap<NetworkToolKind, RunningService>,
    generations: HashMap<NetworkToolKind, u64>,
    /// Last status including a failed start. Running entries mirror the live
    /// socket; stopped entries are removed.
    statuses: HashMap<NetworkToolKind, NetworkToolStatus>,
}

impl NetworkToolRegistry {
    /// Lifecycle sends can wait for channel capacity. Keep the receiver draining
    /// during operations, or disconnect it before shutdown. Run registry
    /// operations and destruction off the UI thread.
    pub fn new(events: SyncSender<NetworkToolEvent>) -> Self {
        Self {
            events,
            services: HashMap::new(),
            generations: HashMap::new(),
            statuses: HashMap::new(),
        }
    }

    /// Start a tool. Binding happens on the calling thread, so port conflicts
    /// and unreadable roots surface as an error immediately; the listener then
    /// runs on its own thread. A failure records that service's error and
    /// leaves every other service untouched.
    pub fn start(
        &mut self,
        kind: NetworkToolKind,
        settings: &NetworkToolsSettings,
    ) -> Result<(), NetworkToolError> {
        if self.is_running(kind) {
            return Err(NetworkToolError::AlreadyRunning);
        }
        self.stop(kind);
        let generation = NEXT_GENERATION.fetch_add(1, Ordering::Relaxed);
        self.generations.insert(kind, generation);
        settings
            .validate()
            .map_err(|error| NetworkToolError::InvalidSettings(error.to_string()))?;
        // A fresh list per start keeps stale transfers from a previous run out
        // of the new one, independently for each service.
        let transfers = Arc::new(Mutex::new(Vec::new()));
        let started = match kind {
            NetworkToolKind::Tftp => TftpServer::start(
                settings.tftp.clone(),
                self.events.clone(),
                Arc::clone(&transfers),
                generation,
            )
            .map(|server| {
                let addr = server.local_addr();
                (ToolRuntime::Tftp(server), addr)
            }),
        };
        match started {
            Ok((runtime, addr)) => {
                self.statuses.insert(kind, NetworkToolStatus::running(addr));
                self.services
                    .insert(kind, RunningService { runtime, transfers });
                Ok(())
            }
            Err(error) => {
                self.statuses
                    .insert(kind, NetworkToolStatus::failed(error.to_string()));
                let _ = self.events.send(NetworkToolEvent::Failed {
                    kind,
                    generation,
                    message: error.to_string(),
                });
                Err(error)
            }
        }
    }

    pub fn stop(&mut self, kind: NetworkToolKind) {
        if let Some(mut service) = self.services.remove(&kind) {
            service.runtime.stop();
        }
        self.statuses.remove(&kind);
    }

    pub fn stop_all(&mut self) {
        for kind in NetworkToolKind::ALL {
            self.stop(kind);
        }
    }

    pub fn is_running(&self, kind: NetworkToolKind) -> bool {
        self.services
            .get(&kind)
            .is_some_and(|service| service.runtime.is_running())
    }

    pub fn generation(&self, kind: NetworkToolKind) -> Option<u64> {
        self.generations.get(&kind).copied()
    }

    pub fn status(&self, kind: NetworkToolKind) -> NetworkToolStatus {
        if self.services.contains_key(&kind) && !self.is_running(kind) {
            return NetworkToolStatus::failed("listener stopped unexpectedly");
        }
        self.statuses.get(&kind).cloned().unwrap_or_default()
    }

    /// Live transfers, ordered by id. The desktop renders this snapshot and
    /// never touches session sockets.
    pub fn transfers(&self, kind: NetworkToolKind) -> Vec<TransferSnapshot> {
        let Some(service) = self.services.get(&kind) else {
            return Vec::new();
        };
        let mut transfers = service
            .transfers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        transfers.sort_by_key(|snapshot| snapshot.id);
        transfers
    }
}

impl Drop for NetworkToolRegistry {
    fn drop(&mut self) {
        self.stop_all();
    }
}

#[cfg(test)]
mod tests {
    use super::{NetworkToolKind, NetworkToolRegistry, NetworkToolTransport};

    #[test]
    fn tftp_capabilities_describe_a_udp_file_service() {
        let capabilities = NetworkToolKind::Tftp.capabilities();
        assert_eq!(capabilities.transport, NetworkToolTransport::Udp);
        assert!(capabilities.uses_bind_address);
        assert!(capabilities.uses_port);
        assert!(capabilities.needs_root_dir);
        assert!(capabilities.supports_upload);
    }

    #[test]
    fn every_kind_carries_distinct_label_and_log_identifiers() {
        for kind in NetworkToolKind::ALL {
            assert!(!kind.name_key().is_empty());
            assert!(!kind.short_name_key().is_empty());
            assert!(!kind.log_name().is_empty());
            assert_ne!(kind.name_key(), kind.short_name_key());
            assert_ne!(kind.name_key(), kind.log_name());
        }
    }

    #[test]
    fn unknown_service_status_is_stopped_without_touching_others() {
        let (sender, _receiver) = std::sync::mpsc::sync_channel(8);
        let registry = NetworkToolRegistry::new(sender);
        assert!(!registry.is_running(NetworkToolKind::Tftp));
        assert!(!registry.status(NetworkToolKind::Tftp).is_running());
        assert!(registry.transfers(NetworkToolKind::Tftp).is_empty());
    }
}
