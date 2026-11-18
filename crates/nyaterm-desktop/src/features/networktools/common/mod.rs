//! Shared building blocks for the built-in network tools configuration windows.
//!
//! Everything here is service-neutral: form widgets, identifier generation,
//! and field rules that all services share. Service-specific layout, rules and
//! lifecycle live under `networktools::<service>`.

pub(in crate::features) mod form;
pub(in crate::features) mod ids;
pub(in crate::features) mod validation;
