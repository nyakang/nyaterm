use crate::package::{StagingDirectory, load, snapshot};
use minisign_verify::{PublicKey, Signature};
use nyaterm_core::plugins::catalog::{Catalog, Listing, Release};
use nyaterm_core::plugins::manifest::PluginManifest;
use nyaterm_core::plugins::{ErrorCode, PluginError, PluginResult};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::Path;

pub const OFFICIAL_CATALOG: &str =
    "https://raw.githubusercontent.com/nyakang/nyaterm-plugins/main/public/v2/plugins.json";
pub const OFFICIAL_KEY: &str = match option_env!("NYATERM_EXTENSION_PUBLIC_KEY") {
    Some(key) => key,
    None => "",
};

fn invalid() -> PluginError {
    PluginError::new(
        ErrorCode::InvalidManifest,
        "Extension download or signature validation failed",
    )
}

fn download(url: &str, limit: u64, cancelled: &impl Fn() -> bool) -> PluginResult<Vec<u8>> {
    let check = || {
        if cancelled() {
            Err(PluginError::new(
                ErrorCode::Cancelled,
                "Extension download cancelled",
            ))
        } else {
            Ok(())
        }
    };
    check()?;
    let url = reqwest::Url::parse(url).map_err(|_| invalid())?;
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
        return Err(invalid());
    }
    let client = reqwest::blocking::Client::builder()
        .https_only(true)
        .timeout(std::time::Duration::from_secs(30))
        .connect_timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|_| invalid())?;
    let mut response = client
        .get(url)
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|_| invalid())?;
    if response.content_length().is_some_and(|size| size > limit) {
        return Err(invalid());
    }
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 32 * 1024];
    loop {
        check()?;
        let count = response.read(&mut chunk).map_err(|_| invalid())?;
        if count == 0 {
            break;
        }
        if bytes.len() as u64 + count as u64 > limit {
            return Err(invalid());
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
    check()?;
    Ok(bytes)
}

pub fn fetch(cancelled: impl Fn() -> bool) -> PluginResult<Catalog> {
    Catalog::parse(&download(OFFICIAL_CATALOG, 4 * 1024 * 1024, &cancelled)?)
}

pub fn verify_archive(bytes: &[u8], release: &Release, key: &str) -> PluginResult<()> {
    if bytes.len() as u64 != release.artifact.size
        || hex::encode(Sha256::digest(bytes)) != release.artifact.sha256
    {
        return Err(invalid());
    }
    // A separate key keeps third-party releases out of the official helper trust domain.
    PublicKey::from_base64(key)
        .map_err(|_| invalid())?
        .verify(
            bytes,
            &Signature::decode(&release.artifact.signature).map_err(|_| invalid())?,
            false,
        )
        .map_err(|_| invalid())
}

pub fn acquire(
    listing: &Listing,
    release: &Release,
    staging: &Path,
    host_version: &str,
    cancelled: impl Fn() -> bool,
) -> PluginResult<StagingDirectory> {
    if OFFICIAL_KEY.is_empty() {
        return Err(PluginError::new(
            ErrorCode::PermissionDenied,
            "The extension store signing key is not configured",
        ));
    }
    let bytes = download(&release.artifact.url, release.artifact.size, &cancelled)?;
    verify_archive(&bytes, release, OFFICIAL_KEY)?;
    let stage = StagingDirectory::new(staging)?;
    let archive = stage.path.join("extension.zip");
    std::fs::write(&archive, bytes).map_err(|_| invalid())?;
    let payload = stage.path.join("payload");
    std::fs::create_dir(&payload).map_err(|_| invalid())?;
    snapshot(&archive, &payload)?;
    let package = load(&payload, host_version)?;
    let capabilities = package
        .manifest
        .capabilities
        .iter()
        .map(|value| {
            nyaterm_core::plugins::extension::Capability::parse(value).map_err(|_| invalid())
        })
        .collect::<PluginResult<Vec<_>>>()?;
    if package.manifest.id != listing.id
        || package.manifest.publisher != listing.publisher
        || package.manifest.version != release.version
        || package.manifest.api_version != release.api_version
        || package.manifest.host_version != release.host_version
        || capabilities != release.capabilities
    {
        return Err(invalid());
    }
    Ok(stage)
}

/// A local receipt is only an index into signed bytes. It never establishes trust
/// by itself; every activation verifies the ZIP again against the compiled key.
pub fn remember_release(
    trust_root: &Path,
    stage: &StagingDirectory,
    release: &Release,
    host_version: &str,
) -> PluginResult<()> {
    let package = load(&stage.path.join("payload"), host_version)?;
    let digest = crate::environment::ExtensionEnvironment::identity(
        &package.manifest,
        package.component.as_deref().unwrap_or_default(),
    )?;
    std::fs::create_dir_all(trust_root).map_err(|_| invalid())?;
    crate::package::reject_link(trust_root)?;
    let temporary = StagingDirectory::new(trust_root)?;
    std::fs::copy(
        stage.path.join("extension.zip"),
        temporary.path.join("archive.zip"),
    )
    .map_err(|_| invalid())?;
    std::fs::write(
        temporary.path.join("release.json"),
        serde_json::to_vec(release).map_err(|_| invalid())?,
    )
    .map_err(|_| invalid())?;
    let destination = trust_root.join(digest);
    if destination.exists() {
        // An existing receipt must pass the same verification before reuse.
        trusted_source(
            trust_root,
            &package.manifest,
            package.component.as_deref().unwrap_or_default(),
        )?;
        return Ok(());
    }
    std::fs::rename(&temporary.path, destination).map_err(|_| invalid())?;
    Ok(())
}

pub fn trusted_source(
    trust_root: &Path,
    manifest: &PluginManifest,
    component: &[u8],
) -> PluginResult<Option<String>> {
    trusted_source_with_key(trust_root, manifest, component, OFFICIAL_KEY)
}

fn trusted_source_with_key(
    trust_root: &Path,
    manifest: &PluginManifest,
    component: &[u8],
    key: &str,
) -> PluginResult<Option<String>> {
    let digest = crate::environment::ExtensionEnvironment::identity(manifest, component)?;
    let receipt = trust_root.join(digest);
    if !receipt.exists() {
        return Ok(None);
    }
    crate::package::reject_path_links(trust_root, &receipt.join("release.json"))?;
    crate::package::reject_path_links(trust_root, &receipt.join("archive.zip"))?;
    if std::fs::metadata(receipt.join("release.json"))
        .map_err(|_| invalid())?
        .len()
        > 32 * 1024
        || std::fs::metadata(receipt.join("archive.zip"))
            .map_err(|_| invalid())?
            .len()
            > crate::package::MAX_ARCHIVE_BYTES
    {
        return Err(invalid());
    }
    let release: Release = serde_json::from_slice(
        &std::fs::read(receipt.join("release.json")).map_err(|_| invalid())?,
    )
    .map_err(|_| invalid())?;
    let bytes = std::fs::read(receipt.join("archive.zip")).map_err(|_| invalid())?;
    verify_archive(&bytes, &release, key)?;
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|_| invalid())?;
    let mut raw = String::new();
    archive
        .by_name("plugin.toml")
        .map_err(|_| invalid())?
        .take(nyaterm_core::plugins::MAX_MANIFEST_BYTES as u64 + 1)
        .read_to_string(&mut raw)
        .map_err(|_| invalid())?;
    let signed = PluginManifest::parse(&raw, env!("CARGO_PKG_VERSION"))?;
    if &signed != manifest {
        return Err(invalid());
    }
    if let Some(name) = &signed.component {
        let mut signed_component = Vec::new();
        archive
            .by_name(name)
            .map_err(|_| invalid())?
            .take(crate::package::MAX_FILE_BYTES + 1)
            .read_to_end(&mut signed_component)
            .map_err(|_| invalid())?;
        if signed_component != component {
            return Err(invalid());
        }
    } else if !component.is_empty() {
        return Err(invalid());
    }
    Ok(Some(format!(
        "store:{}:{}:{}",
        hex::encode(Sha256::digest(key.as_bytes())),
        manifest.publisher,
        manifest.id
    )))
}

#[cfg(test)]
mod tests {
    use crate::catalog::trusted_source_with_key;
    use crate::environment::ExtensionEnvironment;
    use crate::package::{load, snapshot};
    use nyaterm_core::test_support::TestTempDir;
    use std::fs;

    #[test]
    fn verified_updates_share_identity_but_changed_code_and_untrusted_receipts_do_not() {
        let root = TestTempDir::new("nyaterm-extension-receipts");
        let trust = root.join("trusted");
        fs::create_dir_all(&trust).unwrap();
        let fixtures =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/store");
        let key = fs::read_to_string(fixtures.join("public-key.txt")).unwrap();
        let mut identities = Vec::new();
        for version in ["1.0.0", "1.1.0"] {
            let unpacked = root.join(version);
            fs::create_dir(&unpacked).unwrap();
            snapshot(&fixtures.join(format!("{version}.zip")), &unpacked).unwrap();
            let package = load(&unpacked, env!("CARGO_PKG_VERSION")).unwrap();
            assert!(
                trusted_source_with_key(&trust, &package.manifest, &[], key.trim())
                    .unwrap()
                    .is_none()
            );
            let digest = ExtensionEnvironment::identity(&package.manifest, &[]).unwrap();
            let receipt = trust.join(digest);
            fs::create_dir(&receipt).unwrap();
            fs::copy(
                fixtures.join(format!("{version}.zip")),
                receipt.join("archive.zip"),
            )
            .unwrap();
            fs::copy(
                fixtures.join(format!("{version}.json")),
                receipt.join("release.json"),
            )
            .unwrap();
            identities.push(
                trusted_source_with_key(&trust, &package.manifest, &[], key.trim())
                    .unwrap()
                    .unwrap(),
            );
            let mut changed = package.manifest.clone();
            changed.publisher = "impostor".into();
            assert!(
                trusted_source_with_key(&trust, &changed, &[], key.trim())
                    .unwrap()
                    .is_none()
            );
            assert!(
                trusted_source_with_key(&trust, &package.manifest, &[], "untrusted-key").is_err()
            );
            fs::write(receipt.join("archive.zip"), b"tampered").unwrap();
            assert!(trusted_source_with_key(&trust, &package.manifest, &[], key.trim()).is_err());
        }
        assert_eq!(identities[0], identities[1]);
    }
}
