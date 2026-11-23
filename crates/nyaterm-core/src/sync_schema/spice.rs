//! Only reusable SPICE identities may enter a cloud endpoint register.

use serde_json::Value;

use crate::spice::settings::SpiceEndpoint;
use crate::sync_schema::pick;

pub(super) const ENDPOINT_FIELDS: &[&str] = &["host", "port", "tls_port", "proxy"];

pub(super) fn project_endpoint(source: &Value) -> Result<Value, String> {
    let endpoint: SpiceEndpoint =
        serde_json::from_value(source.clone()).map_err(|_| "Invalid saved SPICE endpoint")?;
    endpoint
        .validate()
        .map_err(|_| "Invalid saved SPICE endpoint")?;
    endpoint
        .validate_persistent_identity()
        .map_err(|_| "Temporary SPICE identity cannot be synchronized")?;
    Ok(pick(source, ENDPOINT_FIELDS))
}
