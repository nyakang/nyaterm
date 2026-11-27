//! Private, bounded parent/sidecar control channel, never an HTTP endpoint.
use serde::{Deserialize, Serialize};
pub const CONTROL_VERSION: u32 = 1;
pub const MAX_CONTROL_BYTES: usize = 64 * 1024;
#[derive(Serialize, Deserialize)]
pub struct HttpBootstrap {
    pub version: u32,
    pub port: u16,
    pub allowed_origins: Vec<String>,
    pub bridge_port: u16,
    pub bridge_token: String,
    pub bridge_generation: String,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HttpControlEvent {
    Ready {
        version: u32,
        port: u16,
        token: String,
    },
    Failed {
        version: u32,
        code: String,
    },
}
impl std::fmt::Debug for HttpBootstrap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HttpBootstrap([REDACTED])")
    }
}
impl std::fmt::Debug for HttpControlEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HttpControlEvent([REDACTED])")
    }
}
