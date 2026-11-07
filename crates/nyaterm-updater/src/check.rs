use std::io::Read as _;
use std::time::Duration;

use nyaterm_core::NativeUpdateInfo;
use nyaterm_core::updater::{UpdateChannel, UpdateManifest, parse_current_version};

const UPDATE_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
const USER_AGENT: &str = concat!("nyaterm-app/", env!("CARGO_PKG_VERSION"));

pub fn check_native_update() -> Result<NativeUpdateInfo, String> {
    let current_version =
        parse_current_version(env!("CARGO_PKG_VERSION")).map_err(|error| error.to_string())?;
    let channel = UpdateChannel::for_version(&current_version);
    let client = zed_reqwest::blocking::Client::builder()
        .timeout(UPDATE_TIMEOUT)
        .user_agent(USER_AGENT)
        .build()
        .map_err(|error| format!("build updater HTTP client failed: {error}"))?;
    let response = client
        .get(channel.manifest_url())
        .header("Accept", "application/json")
        .send()
        .map_err(|error| format!("update check request failed: {error}"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("update manifest endpoint returned {status}"));
    }
    let mut body = String::new();
    response
        .take(MAX_MANIFEST_BYTES + 1)
        .read_to_string(&mut body)
        .map_err(|error| format!("read update manifest failed: {error}"))?;
    if body.len() as u64 > MAX_MANIFEST_BYTES {
        return Err("update manifest is too large".to_string());
    }
    UpdateManifest::parse(&body)
        .map(|manifest| manifest.update_info(&current_version))
        .map_err(|error| error.to_string())
}
