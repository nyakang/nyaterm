//! Pure presentation state for the Ports and Services panels: query matching,
//! filter chips and sorting. No GPUI types, so every rule is unit-tested.
//!
//! Filtering runs on the client over the last snapshot, so typing, chips and
//! sort changes are instant and never touch the host.

use nyaterm_remote_ops::ports::{PortEntry, PortExposure, PortProtocol, classify_address};
use nyaterm_remote_ops::services::{ServiceEntry, ServiceStartup, ServiceState};
use std::cmp::Ordering;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SortDirection {
    Ascending,
    Descending,
}

impl SortDirection {
    pub(super) fn flipped(self) -> Self {
        match self {
            Self::Ascending => Self::Descending,
            Self::Descending => Self::Ascending,
        }
    }

    pub(super) fn marker(self) -> &'static str {
        match self {
            Self::Ascending => "↑",
            Self::Descending => "↓",
        }
    }

    fn apply(self, ordering: Ordering) -> Ordering {
        match self {
            Self::Ascending => ordering,
            Self::Descending => ordering.reverse(),
        }
    }
}

// ---------------------------------------------------------------- ports --

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PortSortKey {
    Port,
    Process,
    Protocol,
    Pid,
    User,
}

impl PortSortKey {
    pub(super) const ALL: [Self; 5] = [
        Self::Port,
        Self::Process,
        Self::Protocol,
        Self::Pid,
        Self::User,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PortProtocolFilter {
    All,
    Tcp,
    Udp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PortScopeFilter {
    All,
    /// Reachable beyond this machine: wildcard or interface-bound.
    Exposed,
    Loopback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PortsView {
    pub sort: PortSortKey,
    pub direction: SortDirection,
    pub protocol: PortProtocolFilter,
    pub scope: PortScopeFilter,
}

impl Default for PortsView {
    fn default() -> Self {
        Self {
            sort: PortSortKey::Port,
            direction: SortDirection::Ascending,
            protocol: PortProtocolFilter::All,
            scope: PortScopeFilter::All,
        }
    }
}

/// Every whitespace-separated term must match somewhere in the row.
fn terms(query: &str) -> Vec<String> {
    query
        .split_whitespace()
        .map(str::to_ascii_lowercase)
        .collect()
}

fn port_matches(entry: &PortEntry, terms: &[String]) -> bool {
    if terms.is_empty() {
        return true;
    }
    let family = if entry.address.contains(':') {
        "ipv6"
    } else {
        "ipv4"
    };
    let haystack = format!(
        "{} {} {} {} {} {} {}",
        entry.port,
        entry.protocol.label(),
        family,
        entry.address,
        entry.process.as_deref().unwrap_or(""),
        entry.user.as_deref().unwrap_or(""),
        entry.pid.map(|pid| pid.to_string()).unwrap_or_default(),
    )
    .to_ascii_lowercase();
    terms.iter().all(|term| haystack.contains(term))
}

/// One listening port as the user thinks of it: a protocol, a port and its
/// owner, however many addresses (IPv4, IPv6, several interfaces) it is bound to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PortRow {
    pub protocol: PortProtocol,
    pub port: u16,
    pub pid: Option<u32>,
    pub process: Option<String>,
    pub user: Option<String>,
    pub addresses: Vec<String>,
}

fn is_wildcard(address: &str) -> bool {
    matches!(address, "*" | "0.0.0.0" | "::" | "")
}

fn is_loopback(address: &str) -> bool {
    classify_address(address) == PortExposure::Loopback
}

fn exposure_rank(exposure: PortExposure) -> u8 {
    match exposure {
        PortExposure::AllInterfaces => 2,
        PortExposure::Interface => 1,
        PortExposure::Loopback => 0,
    }
}

impl PortRow {
    /// Stable identity for selection across refreshes.
    pub(super) fn key(&self) -> String {
        format!(
            "{}|{}|{}",
            self.protocol.label(),
            self.port,
            self.pid.unwrap_or(0)
        )
    }

    /// The widest reach of any bind: one public address makes the port public.
    pub(super) fn exposure(&self) -> PortExposure {
        self.addresses
            .iter()
            .map(|address| classify_address(address))
            .max_by_key(|exposure| exposure_rank(*exposure))
            .unwrap_or(PortExposure::Loopback)
    }

    /// `*` is a dual-stack wildcard, so it covers both families.
    pub(super) fn has_ipv4(&self) -> bool {
        self.addresses
            .iter()
            .any(|a| a == "*" || a.is_empty() || !a.contains(':'))
    }

    pub(super) fn has_ipv6(&self) -> bool {
        self.addresses
            .iter()
            .any(|a| a == "*" || a.is_empty() || a.contains(':'))
    }

    fn family_label(&self) -> &'static str {
        match (self.has_ipv4(), self.has_ipv6()) {
            (true, true) => "IPv4 + IPv6",
            (true, false) => "IPv4",
            (false, true) => "IPv6",
            (false, false) => "",
        }
    }

    /// What to show for "where is it bound". Wildcards and loopbacks are
    /// described by family (the badge already says public/local); a specific
    /// interface address is shown because it is the information.
    pub(super) fn bind_summary(&self) -> String {
        let specific: Vec<&str> = self
            .addresses
            .iter()
            .map(String::as_str)
            .filter(|a| !is_wildcard(a) && !is_loopback(a))
            .collect();
        if specific.is_empty() {
            return self.family_label().to_string();
        }
        let mut shown = specific
            .iter()
            .take(2)
            .copied()
            .collect::<Vec<_>>()
            .join(", ");
        if specific.len() > 2 {
            shown.push_str(&format!(" +{}", specific.len() - 2));
        }
        shown
    }
}

/// Merge entries of the same protocol, port and owner into one row.
fn group_ports(entries: Vec<&PortEntry>) -> Vec<PortRow> {
    let mut rows: Vec<PortRow> = Vec::new();
    let mut index: HashMap<(PortProtocol, u16, Option<u32>), usize> = HashMap::new();
    for entry in entries {
        let key = (entry.protocol, entry.port, entry.pid);
        match index.get(&key) {
            Some(&at) => {
                let row = &mut rows[at];
                if !row.addresses.contains(&entry.address) {
                    row.addresses.push(entry.address.clone());
                }
                row.process = row.process.take().or_else(|| entry.process.clone());
                row.user = row.user.take().or_else(|| entry.user.clone());
            }
            None => {
                index.insert(key, rows.len());
                rows.push(PortRow {
                    protocol: entry.protocol,
                    port: entry.port,
                    pid: entry.pid,
                    process: entry.process.clone(),
                    user: entry.user.clone(),
                    addresses: vec![entry.address.clone()],
                });
            }
        }
    }
    rows
}

pub(super) fn derive_ports(entries: &[PortEntry], query: &str, view: PortsView) -> Vec<PortRow> {
    let terms = terms(query);
    let kept: Vec<&PortEntry> = entries
        .iter()
        .filter(|entry| match view.protocol {
            PortProtocolFilter::All => true,
            PortProtocolFilter::Tcp => entry.protocol == PortProtocol::Tcp,
            PortProtocolFilter::Udp => entry.protocol == PortProtocol::Udp,
        })
        .filter(|entry| match view.scope {
            PortScopeFilter::All => true,
            PortScopeFilter::Exposed => entry.exposure() != PortExposure::Loopback,
            PortScopeFilter::Loopback => entry.exposure() == PortExposure::Loopback,
        })
        .filter(|entry| port_matches(entry, &terms))
        .collect();
    let mut rows = group_ports(kept);
    rows.sort_by(|a, b| {
        let primary = match view.sort {
            PortSortKey::Port => a.port.cmp(&b.port),
            PortSortKey::Process => lower(&a.process).cmp(&lower(&b.process)),
            PortSortKey::Protocol => a.protocol.cmp(&b.protocol),
            PortSortKey::Pid => a.pid.cmp(&b.pid),
            PortSortKey::User => lower(&a.user).cmp(&lower(&b.user)),
        };
        // Ties fall back to port so the order never jitters between refreshes.
        view.direction
            .apply(primary)
            .then(a.port.cmp(&b.port))
            .then(a.protocol.cmp(&b.protocol))
            .then(a.pid.cmp(&b.pid))
    });
    rows
}

/// What a bind address means, for the details panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BindKind {
    AllV4,
    AllV6,
    AllDual,
    LoopV4,
    LoopV6,
    LinkLocal,
    Mapped,
    IfaceV4,
    IfaceV6,
}

pub(super) fn bind_kind(address: &str) -> BindKind {
    let base = address.split('%').next().unwrap_or(address);
    let lower = base.to_ascii_lowercase();
    match lower.as_str() {
        "*" | "" => BindKind::AllDual,
        "0.0.0.0" => BindKind::AllV4,
        "::" => BindKind::AllV6,
        "::1" => BindKind::LoopV6,
        a if a.starts_with("127.") => BindKind::LoopV4,
        a if a.starts_with("::ffff:") => BindKind::Mapped,
        a if a.starts_with("fe80") => BindKind::LinkLocal,
        a if a.contains(':') => BindKind::IfaceV6,
        _ => BindKind::IfaceV4,
    }
}

/// Well-known service for a port, shown dimmed when the owner is hidden so the
/// row still says something useful.
pub(super) fn well_known_service(port: u16) -> Option<&'static str> {
    Some(match port {
        20 | 21 => "ftp",
        22 => "ssh",
        23 => "telnet",
        25 | 587 => "smtp",
        53 => "dns",
        67 | 68 => "dhcp",
        80 => "http",
        110 => "pop3",
        111 => "rpcbind",
        123 => "ntp",
        137..=139 => "netbios",
        143 => "imap",
        161 => "snmp",
        389 => "ldap",
        443 => "https",
        445 => "smb",
        514 => "syslog",
        631 => "ipp",
        993 => "imaps",
        1433 => "mssql",
        2049 => "nfs",
        3000 => "dev server",
        3306 => "mysql",
        3389 => "rdp",
        5000 => "http",
        5353 => "mdns",
        5432 => "postgresql",
        5900 => "vnc",
        6379 => "redis",
        8080 | 8000 | 8888 => "http-alt",
        8443 => "https-alt",
        9090 => "prometheus",
        9200 => "elasticsearch",
        11211 => "memcached",
        27017 => "mongodb",
        _ => return None,
    })
}

fn lower(value: &Option<String>) -> String {
    value.as_deref().unwrap_or("").to_ascii_lowercase()
}

// ------------------------------------------------------------- services --

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ServiceSortKey {
    Name,
    State,
    Startup,
    Pid,
}

impl ServiceSortKey {
    pub(super) const ALL: [Self; 4] = [Self::Name, Self::State, Self::Startup, Self::Pid];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ServiceStateFilter {
    All,
    Running,
    Stopped,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ServicesView {
    pub sort: ServiceSortKey,
    pub direction: SortDirection,
    pub state: ServiceStateFilter,
}

impl Default for ServicesView {
    fn default() -> Self {
        Self {
            // Problems first is the useful default for a service list.
            sort: ServiceSortKey::State,
            direction: SortDirection::Ascending,
            state: ServiceStateFilter::All,
        }
    }
}

fn startup_rank(startup: ServiceStartup) -> u8 {
    match startup {
        ServiceStartup::Enabled => 0,
        ServiceStartup::Static => 1,
        ServiceStartup::Disabled => 2,
        ServiceStartup::Masked => 3,
        ServiceStartup::Unknown => 4,
    }
}

fn service_matches(entry: &ServiceEntry, terms: &[String]) -> bool {
    if terms.is_empty() {
        return true;
    }
    let haystack = format!(
        "{} {} {} {}",
        entry.name,
        entry.description,
        entry.detail,
        entry.pid.map(|pid| pid.to_string()).unwrap_or_default()
    )
    .to_ascii_lowercase();
    terms.iter().all(|term| haystack.contains(term))
}

pub(super) fn derive_services(
    entries: &[ServiceEntry],
    query: &str,
    view: ServicesView,
) -> Vec<ServiceEntry> {
    let terms = terms(query);
    let mut rows: Vec<ServiceEntry> = entries
        .iter()
        .filter(|entry| match view.state {
            ServiceStateFilter::All => true,
            ServiceStateFilter::Running => entry.state == ServiceState::Running,
            ServiceStateFilter::Stopped => entry.state == ServiceState::Stopped,
            ServiceStateFilter::Failed => entry.state == ServiceState::Failed,
        })
        .filter(|entry| service_matches(entry, &terms))
        .cloned()
        .collect();
    rows.sort_by(|a, b| {
        let primary = match view.sort {
            ServiceSortKey::Name => a
                .name
                .to_ascii_lowercase()
                .cmp(&b.name.to_ascii_lowercase()),
            ServiceSortKey::State => a.state.cmp(&b.state),
            ServiceSortKey::Startup => startup_rank(a.startup).cmp(&startup_rank(b.startup)),
            ServiceSortKey::Pid => a.pid.cmp(&b.pid),
        };
        view.direction.apply(primary).then(
            a.name
                .to_ascii_lowercase()
                .cmp(&b.name.to_ascii_lowercase()),
        )
    });
    rows
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct ServiceCounts {
    pub total: usize,
    pub running: usize,
    pub stopped: usize,
    pub failed: usize,
}

pub(super) fn count_services(entries: &[ServiceEntry]) -> ServiceCounts {
    let mut counts = ServiceCounts {
        total: entries.len(),
        ..Default::default()
    };
    for entry in entries {
        match entry.state {
            ServiceState::Running => counts.running += 1,
            ServiceState::Stopped => counts.stopped += 1,
            ServiceState::Failed => counts.failed += 1,
            _ => {}
        }
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::{
        BindKind, PortProtocolFilter, PortScopeFilter, PortSortKey, PortsView, ServiceSortKey,
        ServiceStateFilter, ServicesView, SortDirection, bind_kind, count_services, derive_ports,
        derive_services, well_known_service,
    };
    use nyaterm_remote_ops::ports::{PortEntry, PortExposure, PortProtocol};
    use nyaterm_remote_ops::services::{ServiceEntry, ServiceStartup, ServiceState};

    fn port(proto: PortProtocol, address: &str, port: u16, process: &str, pid: u32) -> PortEntry {
        PortEntry {
            protocol: proto,
            address: address.to_string(),
            port,
            pid: Some(pid),
            process: Some(process.to_string()),
            user: Some("root".to_string()),
        }
    }

    fn service(name: &str, state: ServiceState, startup: ServiceStartup) -> ServiceEntry {
        ServiceEntry {
            name: name.to_string(),
            description: format!("{name} daemon"),
            state,
            detail: String::new(),
            startup,
            pid: (state == ServiceState::Running).then_some(10),
            scope: String::new(),
        }
    }

    fn sample_ports() -> Vec<PortEntry> {
        vec![
            port(PortProtocol::Tcp, "0.0.0.0", 22, "sshd", 800),
            port(PortProtocol::Tcp, "127.0.0.1", 5432, "postgres", 900),
            port(PortProtocol::Udp, "0.0.0.0", 53, "dnsmasq", 700),
            port(PortProtocol::Tcp, "::1", 631, "cupsd", 650),
        ]
    }

    #[test]
    fn port_terms_are_anded_across_every_column() {
        let rows = derive_ports(&sample_ports(), "tcp root 5432", PortsView::default());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].process.as_deref(), Some("postgres"));
        assert_eq!(
            derive_ports(&sample_ports(), "SSHD", PortsView::default()).len(),
            1
        );
        assert!(derive_ports(&sample_ports(), "nothing here", PortsView::default()).is_empty());
    }

    #[test]
    fn ipv4_and_ipv6_binds_of_one_service_become_one_row() {
        let entries = vec![
            port(PortProtocol::Tcp, "0.0.0.0", 22, "sshd", 800),
            port(PortProtocol::Tcp, "::", 22, "sshd", 800),
            port(PortProtocol::Tcp, "0.0.0.0", 80, "nginx", 900),
            port(PortProtocol::Tcp, "10.0.0.5", 8443, "app", 901),
            port(PortProtocol::Tcp, "127.0.0.1", 8443, "app", 901),
        ];
        let rows = derive_ports(&entries, "", PortsView::default());
        assert_eq!(rows.len(), 3, "22 v4+v6 collapse into one row");
        assert_eq!(rows[0].bind_summary(), "IPv4 + IPv6");
        assert!(rows[0].has_ipv4() && rows[0].has_ipv6());
        assert_eq!(rows[1].bind_summary(), "IPv4");
        // A specific interface address is information, so it is shown.
        assert_eq!(rows[2].bind_summary(), "10.0.0.5");
        assert_eq!(rows[2].exposure(), PortExposure::Interface);
        // Searching a family keeps only rows that have it.
        let v6 = derive_ports(&entries, "ipv6", PortsView::default());
        assert_eq!(v6.len(), 1);
        assert_eq!(v6[0].port, 22);
    }

    #[test]
    fn bind_addresses_are_classified_for_the_details_panel() {
        assert_eq!(bind_kind("0.0.0.0"), BindKind::AllV4);
        assert_eq!(bind_kind("::"), BindKind::AllV6);
        assert_eq!(bind_kind("*"), BindKind::AllDual);
        assert_eq!(bind_kind("127.0.0.53%lo"), BindKind::LoopV4);
        assert_eq!(bind_kind("::1"), BindKind::LoopV6);
        assert_eq!(bind_kind("::ffff:10.0.0.1"), BindKind::Mapped);
        assert_eq!(bind_kind("fe80::1%en0"), BindKind::LinkLocal);
        assert_eq!(bind_kind("2001:db8::5"), BindKind::IfaceV6);
        assert_eq!(bind_kind("192.168.1.9"), BindKind::IfaceV4);
    }

    #[test]
    fn a_dual_stack_wildcard_counts_as_both_families_and_hidden_owners_group() {
        let mut star = port(PortProtocol::Udp, "*", 5353, "mdns", 5);
        star.pid = None;
        star.process = None;
        let rows = derive_ports(&[star.clone(), star], "", PortsView::default());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].bind_summary(), "IPv4 + IPv6");
        assert_eq!(well_known_service(5353), Some("mdns"));
        assert_eq!(well_known_service(40001), None);
    }

    #[test]
    fn port_scope_and_protocol_chips_narrow_the_list() {
        let exposed = PortsView {
            scope: PortScopeFilter::Exposed,
            ..Default::default()
        };
        let ports: Vec<u16> = derive_ports(&sample_ports(), "", exposed)
            .iter()
            .map(|e| e.port)
            .collect();
        assert_eq!(ports, vec![22, 53]);
        let udp = PortsView {
            protocol: PortProtocolFilter::Udp,
            ..Default::default()
        };
        assert_eq!(derive_ports(&sample_ports(), "", udp).len(), 1);
        let local = PortsView {
            scope: PortScopeFilter::Loopback,
            ..Default::default()
        };
        assert_eq!(derive_ports(&sample_ports(), "", local).len(), 2);
    }

    #[test]
    fn port_sort_is_stable_with_port_tiebreak_and_reversible() {
        let by_process = PortsView {
            sort: PortSortKey::Process,
            ..Default::default()
        };
        let names: Vec<_> = derive_ports(&sample_ports(), "", by_process)
            .iter()
            .map(|e| e.process.clone().unwrap())
            .collect();
        assert_eq!(names, ["cupsd", "dnsmasq", "postgres", "sshd"]);
        let desc = PortsView {
            direction: SortDirection::Descending,
            ..Default::default()
        };
        assert_eq!(derive_ports(&sample_ports(), "", desc)[0].port, 5432);
    }

    #[test]
    fn services_default_order_puts_failures_first_then_running() {
        let entries = vec![
            service("zeta", ServiceState::Stopped, ServiceStartup::Disabled),
            service("alpha", ServiceState::Running, ServiceStartup::Enabled),
            service("beta", ServiceState::Failed, ServiceStartup::Enabled),
            service("able", ServiceState::Running, ServiceStartup::Enabled),
        ];
        let names: Vec<_> = derive_services(&entries, "", ServicesView::default())
            .iter()
            .map(|e| e.name.clone())
            .collect();
        assert_eq!(names, ["beta", "able", "alpha", "zeta"]);
    }

    #[test]
    fn service_state_chip_and_query_combine() {
        let entries = vec![
            service("nginx", ServiceState::Running, ServiceStartup::Enabled),
            service(
                "nginx-debug",
                ServiceState::Stopped,
                ServiceStartup::Disabled,
            ),
            service("sshd", ServiceState::Running, ServiceStartup::Enabled),
        ];
        let view = ServicesView {
            state: ServiceStateFilter::Running,
            ..Default::default()
        };
        let rows = derive_services(&entries, "nginx", view);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "nginx");
        let counts = count_services(&entries);
        assert_eq!((counts.total, counts.running, counts.stopped), (3, 2, 1));
    }

    #[test]
    fn startup_sort_groups_enabled_before_disabled() {
        let entries = vec![
            service("b", ServiceState::Stopped, ServiceStartup::Disabled),
            service("a", ServiceState::Stopped, ServiceStartup::Enabled),
            service("c", ServiceState::Stopped, ServiceStartup::Masked),
        ];
        let view = ServicesView {
            sort: ServiceSortKey::Startup,
            ..Default::default()
        };
        let names: Vec<_> = derive_services(&entries, "", view)
            .iter()
            .map(|e| e.name.clone())
            .collect();
        assert_eq!(names, ["a", "b", "c"]);
    }
}
