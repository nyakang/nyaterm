use std::io::Read;

use minisign_verify::{PublicKey, Signature};
use nyaterm_core::plugins::native::catalog::{MAX_INDEX_BYTES, NativeIndex, NativeRelease};
use nyaterm_core::plugins::native::{NativeComponentId, PUBLIC_KEY, RUNTIME, TARGET};
use nyaterm_core::plugins::{ErrorCode, PluginError, PluginResult};

use crate::native::client;
use crate::package::io_error;

const INDEX: &str = "https://downloads.nyaterm.app/native/v1/index.json";

pub(super) fn network_error() -> PluginError {
    PluginError::new(
        ErrorCode::NativeNetwork,
        "Cannot download native components; check the network and retry",
    )
}

fn check_status(status: reqwest::StatusCode) -> PluginResult<()> {
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err(PluginError::new(
            ErrorCode::NativeCatalogUnavailable,
            "Native component releases are not published yet; retry later or import an official signed package",
        ));
    }
    if !status.is_success() {
        return Err(network_error());
    }
    Ok(())
}

fn download(
    url: &str,
    limit: u64,
    cancel: &impl Fn() -> PluginResult<()>,
) -> PluginResult<Vec<u8>> {
    cancel()?;
    let mut response = client()?.get(url).send().map_err(|_| network_error())?;
    check_status(response.status())?;
    if response.content_length().is_some_and(|size| size > limit) {
        return Err(io_error());
    }
    let mut bytes = Vec::new();
    let mut buffer = [0; 16384];
    loop {
        cancel()?;
        let count = response.read(&mut buffer).map_err(|_| network_error())?;
        if count == 0 {
            return Ok(bytes);
        }
        if bytes.len() as u64 + count as u64 > limit {
            return Err(io_error());
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
}

pub(super) fn select(
    id: NativeComponentId,
    cancel: &impl Fn() -> PluginResult<()>,
) -> PluginResult<NativeRelease> {
    // Components are installed in the user registry, independently of ownership
    // of the application executable (including distribution-managed installs).
    // One object binds index bytes and signature, avoiding a two-object publish race.
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Envelope {
        index: String,
        signature: String,
    }
    let bytes = download(INDEX, MAX_INDEX_BYTES, cancel)?;
    let envelope: Envelope = serde_json::from_slice(&bytes).map_err(|_| io_error())?;
    let index = parse(
        envelope.index.as_bytes(),
        envelope.signature.as_bytes(),
        PUBLIC_KEY,
    )?;
    index
        .select(id, env!("CARGO_PKG_VERSION"), TARGET, RUNTIME)
        .cloned()
        .ok_or_else(|| {
            PluginError::new(
                ErrorCode::Incompatible,
                "No compatible native component release is available for this host and platform",
            )
        })
}

fn parse(bytes: &[u8], signature: &[u8], key: &str) -> PluginResult<NativeIndex> {
    let invalid = || {
        PluginError::new(
            ErrorCode::InvalidManifest,
            "Native component index signature is invalid",
        )
    };
    if bytes.len() as u64 > MAX_INDEX_BYTES || signature.len() > 8192 {
        return Err(invalid());
    }
    let signature = Signature::decode(std::str::from_utf8(signature).map_err(|_| invalid())?)
        .map_err(|_| invalid())?;
    PublicKey::from_base64(key)
        .map_err(|_| invalid())?
        .verify(bytes, &signature, false)
        .map_err(|_| invalid())?;
    let index: NativeIndex = serde_json::from_slice(bytes).map_err(|_| invalid())?;
    index.validate()?;
    Ok(index)
}

#[cfg(test)]
mod tests {
    use crate::native::catalog::{check_status, parse};
    use nyaterm_core::plugins::ErrorCode;

    #[test]
    fn unpublished_catalog_is_distinct_from_network_failure() {
        assert_eq!(
            check_status(reqwest::StatusCode::NOT_FOUND)
                .unwrap_err()
                .code,
            ErrorCode::NativeCatalogUnavailable
        );
        for status in [
            reqwest::StatusCode::FORBIDDEN,
            reqwest::StatusCode::SERVICE_UNAVAILABLE,
            reqwest::StatusCode::FOUND,
        ] {
            assert_eq!(
                check_status(status).unwrap_err().code,
                ErrorCode::NativeNetwork
            );
        }
        assert!(check_status(reqwest::StatusCode::OK).is_ok());
    }

    #[test]
    fn index_authentication_precedes_release_selection() {
        let raw = include_bytes!("../../tests/fixtures/native/index.json");
        let signature = include_bytes!("../../tests/fixtures/native/index.minisig");
        let key = include_str!("../../tests/fixtures/native/public-key.txt");
        assert_eq!(parse(raw, signature, key).unwrap().releases.len(), 1);
        let mut changed = raw.to_vec();
        changed.push(b' ');
        assert!(parse(&changed, signature, key).is_err());
        assert!(parse(raw, signature, nyaterm_core::plugins::native::PUBLIC_KEY).is_err());
    }
}
