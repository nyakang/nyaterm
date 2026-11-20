//! Local network interface addresses for the machine running the application.
//!
//! This is the same `sysinfo` snapshot used by [`crate::local_stats`], exposed
//! as a read-only address list instead of traffic counters. Enumerating here
//! keeps every local system probe in one crate; callers snapshot the list once
//! and never probe from a render path.

use std::net::IpAddr;

use sysinfo::{InterfaceOperationalState, Networks};

/// One address assigned to one local interface.
///
/// An interface with several addresses produces several entries, so callers
/// can present one selectable row per address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalInterfaceAddress {
    pub interface: String,
    pub address: IpAddr,
    pub prefix: u8,
    /// `false` only for states that cannot carry traffic. `Unknown` and
    /// `Testing` stay `true` because virtual and tunnel devices report them
    /// while still being usable.
    pub up: bool,
}

/// Snapshot every local interface address once.
///
/// The result is sorted by interface name and then address so repeated calls
/// produce a stable order even though `sysinfo` stores interfaces in a hash
/// map.
pub fn local_interface_addresses() -> Vec<LocalInterfaceAddress> {
    let networks = Networks::new_with_refreshed_list();
    let mut addresses = Vec::new();
    for (interface, data) in networks.list() {
        let up = !matches!(
            data.operational_state(),
            InterfaceOperationalState::Down
                | InterfaceOperationalState::NotPresent
                | InterfaceOperationalState::LowerLayerDown
                | InterfaceOperationalState::Dormant
        );
        for network in data.ip_networks() {
            addresses.push(LocalInterfaceAddress {
                interface: interface.clone(),
                address: network.addr,
                prefix: network.prefix,
                up,
            });
        }
    }
    addresses.sort_by(|left, right| {
        left.interface
            .cmp(&right.interface)
            .then_with(|| left.address.cmp(&right.address))
    });
    addresses
}
