//! One generator for every runtime identifier owned by a network tool.
//!
//! Panel element ids, configuration input ids, the text-input registry prefix
//! and notification keys all follow `networktools.<service>.<surface>`, so a
//! new service never collides with an existing one and the prefix used by
//! `forget_text_inputs` can never drift from the ids the form registers.

use nyaterm_transport::networktools::NetworkToolKind;

/// Base identifier for one service surface.
fn networktools_id(kind: NetworkToolKind, surface: &str) -> String {
    format!("networktools.{}.{surface}", kind.log_name())
}

/// Configuration form input id, e.g. `networktools.tftp.config.port`.
pub(in crate::features) fn config_input(kind: NetworkToolKind, field: &str) -> String {
    networktools_id(kind, &format!("config.{field}"))
}

/// Prefix that covers every configuration input of one service, handed to
/// `forget_text_inputs` when the window closes.
pub(in crate::features) fn input_prefix(kind: NetworkToolKind) -> String {
    format!("networktools.{}.config.", kind.log_name())
}

/// Non-input element inside the configuration window, such as its scroll area
/// or an info tooltip.
pub(in crate::features) fn config_element(kind: NetworkToolKind, element: &str) -> String {
    networktools_id(kind, &format!("config.{element}"))
}

/// Element id inside the service's panel row.
pub(in crate::features) fn panel_element(kind: NetworkToolKind, element: &str) -> String {
    networktools_id(kind, &format!("panel.{element}"))
}

/// Service status notification. Per service so two failures do not replace
/// each other's toast.
pub(in crate::features) fn status_notification(kind: NetworkToolKind) -> String {
    networktools_id(kind, "status")
}

/// Per-transfer notification. A transfer's completion reuses its start key, so
/// the lifecycle replaces that transfer's toast instead of stacking a new one.
pub(in crate::features) fn transfer_notification(kind: NetworkToolKind, id: u64) -> String {
    networktools_id(kind, &format!("transfer.{id}"))
}
