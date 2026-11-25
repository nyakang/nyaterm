use crate::package::{io_error, read_bounded, reject_path_links};
use minisign_verify::{PublicKey, Signature};
use nyaterm_core::plugins::native::{
    MAX_ARCHIVE_BYTES, MAX_FILES, MAX_MANIFEST_BYTES, MAX_PAYLOAD_BYTES, NativeComponentId,
    NativeManifest, PUBLIC_KEY, RUNTIME, TARGET,
};
use nyaterm_core::plugins::{ErrorCode, PluginError, PluginResult, validate_resource_path};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::Path;

fn invalid() -> PluginError {
    PluginError::new(
        ErrorCode::InvalidManifest,
        "Native component signature or file inventory is invalid",
    )
}

struct Trust<'a> {
    version: &'a str,
    runtime: &'a str,
    target: &'a str,
    key: &'a str,
}
const TRUST: Trust<'static> = Trust {
    version: env!("CARGO_PKG_VERSION"),
    runtime: RUNTIME,
    target: TARGET,
    key: PUBLIC_KEY,
};

fn manifest(root: &Path, id: NativeComponentId, trust: &Trust<'_>) -> PluginResult<NativeManifest> {
    let raw = read_bounded(&root.join("component.json"), MAX_MANIFEST_BYTES)?;
    let signature = read_bounded(&root.join("component.minisig"), 8192)?;
    let signature = Signature::decode(std::str::from_utf8(&signature).map_err(|_| invalid())?)
        .map_err(|_| invalid())?;
    PublicKey::from_base64(trust.key)
        .map_err(|_| invalid())?
        .verify(&raw, &signature, false)
        .map_err(|_| invalid())?;
    let manifest: NativeManifest = serde_json::from_slice(&raw).map_err(|_| invalid())?;
    manifest.validate(id, trust.version, trust.runtime, trust.target)?;
    Ok(manifest)
}

pub(super) fn verify(
    root: &Path,
    id: NativeComponentId,
    cancel: &impl Fn() -> PluginResult<()>,
) -> PluginResult<NativeManifest> {
    verify_with_trust(root, id, cancel, &TRUST)
}

fn verify_with_trust(
    root: &Path,
    id: NativeComponentId,
    cancel: &impl Fn() -> PluginResult<()>,
    trust: &Trust<'_>,
) -> PluginResult<NativeManifest> {
    let manifest = manifest(root, id, trust)?;
    let mut expected = BTreeSet::from([
        "component.json".to_string(),
        "component.minisig".to_string(),
    ]);
    for (name, entry) in &manifest.files {
        cancel()?;
        expected.insert(name.clone());
        let path = root.join(name);
        reject_path_links(root, &path)?;
        let mut file = File::open(&path).map_err(|_| io_error())?;
        if file.metadata().map_err(|_| io_error())?.len() != entry.size {
            return Err(invalid());
        }
        let mut hash = Sha256::new();
        let mut buffer = [0; 65536];
        let mut count = 0u64;
        loop {
            cancel()?;
            let n = file.read(&mut buffer).map_err(|_| io_error())?;
            if n == 0 {
                break;
            }
            count += n as u64;
            if count > entry.size {
                return Err(invalid());
            }
            hash.update(&buffer[..n]);
        }
        if count != entry.size || hex::encode(hash.finalize()) != entry.sha256 {
            return Err(invalid());
        }
    }
    let mut actual = BTreeSet::new();
    inventory(root, root, &mut actual)?;
    if actual != expected {
        return Err(invalid());
    }
    Ok(manifest)
}

fn inventory(root: &Path, directory: &Path, files: &mut BTreeSet<String>) -> PluginResult<()> {
    if directory
        .components()
        .count()
        .saturating_sub(root.components().count())
        > 32
    {
        return Err(invalid());
    }
    for item in fs::read_dir(directory).map_err(|_| io_error())? {
        let path = item.map_err(|_| io_error())?.path();
        reject_path_links(root, &path)?;
        if path.is_dir() {
            inventory(root, &path, files)?;
        } else {
            if files.len() >= MAX_FILES + 2 {
                return Err(invalid());
            }
            files.insert(
                path.strip_prefix(root)
                    .map_err(|_| invalid())?
                    .to_str()
                    .ok_or_else(invalid)?
                    .replace('\\', "/"),
            );
        }
    }
    Ok(())
}

pub(super) fn extract(
    source: &Path,
    root: &Path,
    id: NativeComponentId,
    cancel: &impl Fn() -> PluginResult<()>,
) -> PluginResult<()> {
    let file = File::open(source).map_err(|_| io_error())?;
    if file.metadata().map_err(|_| io_error())?.len() > MAX_ARCHIVE_BYTES {
        return Err(invalid());
    }
    let mut archive = zip::ZipArchive::new(file).map_err(|_| invalid())?;
    if archive.len() > MAX_FILES + 2 {
        return Err(invalid());
    }
    let mut seen = BTreeSet::new();
    let mut total = 0u64;
    for index in 0..archive.len() {
        cancel()?;
        let mut entry = archive.by_index(index).map_err(|_| invalid())?;
        let name = entry.name().to_string();
        validate_resource_path(&name)?;
        if entry.is_dir()
            || entry.is_symlink()
            || !seen.insert(name.to_ascii_lowercase())
            || entry
                .unix_mode()
                .is_some_and(|m| !matches!(m & 0o170000, 0 | 0o100000))
        {
            return Err(invalid());
        }
        total = total
            .checked_add(entry.size())
            .filter(|n| *n <= MAX_PAYLOAD_BYTES + MAX_MANIFEST_BYTES + 8192)
            .ok_or_else(invalid)?;
        let destination = root.join(&name);
        fs::create_dir_all(destination.parent().ok_or_else(invalid)?).map_err(|_| io_error())?;
        let mut output = File::options()
            .write(true)
            .create_new(true)
            .open(&destination)
            .map_err(|_| io_error())?;
        let mut remaining = entry.size();
        let mut buffer = [0; 65536];
        loop {
            cancel()?;
            let n = entry.read(&mut buffer).map_err(|_| invalid())?;
            if n == 0 {
                break;
            }
            remaining = remaining.checked_sub(n as u64).ok_or_else(invalid)?;
            output.write_all(&buffer[..n]).map_err(|_| io_error())?;
        }
        if remaining != 0 {
            return Err(invalid());
        }
        output.sync_all().map_err(|_| io_error())?;
    }
    let manifest = verify(root, id, cancel)?;
    #[cfg(unix)]
    for (name, entry) in manifest.files {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            root.join(name),
            fs::Permissions::from_mode(if entry.executable { 0o755 } else { 0o644 }),
        )
        .map_err(|_| io_error())?;
    }
    #[cfg(not(unix))]
    let _ = manifest;
    Ok(())
}

pub(super) fn manifest_digest(root: &Path) -> PluginResult<String> {
    Ok(hex::encode(Sha256::digest(read_bounded(
        &root.join("component.json"),
        MAX_MANIFEST_BYTES,
    )?)))
}

#[cfg(test)]
mod tests {
    use crate::native::package::{Trust, verify_with_trust};
    use nyaterm_core::plugins::native::NativeComponentId;
    use nyaterm_core::test_support::TestTempDir;
    use std::fs;
    fn fixture() -> TestTempDir {
        let root = TestTempDir::new("nyaterm-native-signed-package");
        fs::create_dir_all(root.path()).unwrap();
        fs::write(
            root.join("component.json"),
            include_bytes!("../../tests/fixtures/native/component.json"),
        )
        .unwrap();
        fs::write(
            root.join("component.minisig"),
            include_bytes!("../../tests/fixtures/native/component.minisig"),
        )
        .unwrap();
        fs::write(
            root.join("nyaterm-vnc-helper.exe"),
            include_bytes!("../../tests/fixtures/native/helper.txt"),
        )
        .unwrap();
        root
    }
    fn trust() -> Trust<'static> {
        Trust {
            version: "2.0.8",
            runtime: "windows-msvc",
            target: "x86_64-pc-windows-msvc",
            key: include_str!("../../tests/fixtures/native/public-key.txt"),
        }
    }
    #[test]
    fn signed_inventory_accepts_original_and_rejects_changed_payload_or_extra_dll() {
        let root = fixture();
        let verify =
            || verify_with_trust(root.path(), NativeComponentId::Vnc, &|| Ok(()), &trust());
        assert!(verify().is_ok());
        fs::write(root.join("injected.dll"), b"extra").unwrap();
        assert!(verify().is_err());
        fs::remove_file(root.join("injected.dll")).unwrap();
        fs::write(root.join("nyaterm-vnc-helper.exe"), b"changed").unwrap();
        assert!(verify().is_err());
    }
    #[test]
    fn signature_and_runtime_compatibility_are_both_required() {
        let root = fixture();
        let mut wrong = trust();
        wrong.runtime = "glibc-2.31";
        assert!(
            verify_with_trust(root.path(), NativeComponentId::Vnc, &|| Ok(()), &wrong).is_err()
        );
        assert!(
            verify_with_trust(root.path(), NativeComponentId::Rdp, &|| Ok(()), &trust()).is_err()
        );
        let mut raw = fs::read(root.join("component.json")).unwrap();
        raw.push(b' ');
        fs::write(root.join("component.json"), raw).unwrap();
        assert!(
            verify_with_trust(root.path(), NativeComponentId::Vnc, &|| Ok(()), &trust()).is_err()
        );
    }
    #[test]
    fn signature_from_another_key_is_not_an_official_component() {
        let root = fixture();
        assert!(super::verify(root.path(), NativeComponentId::Vnc, &|| Ok(())).is_err());
    }
    #[test]
    fn extraction_rejects_traversal_before_writing_outside_staging() {
        use std::io::Write;
        let temp = TestTempDir::new("nyaterm-native-traversal");
        fs::create_dir_all(temp.join("stage")).unwrap();
        let source = temp.join("unsafe.zip");
        let mut zip = zip::ZipWriter::new(fs::File::create(&source).unwrap());
        zip.start_file("../escaped", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"unexpected").unwrap();
        zip.finish().unwrap();
        assert!(
            super::extract(
                &source,
                &temp.join("stage"),
                NativeComponentId::Vnc,
                &|| Ok(())
            )
            .is_err()
        );
        assert!(!temp.join("escaped").exists());
    }

    #[test]
    fn verification_observes_cancellation_before_reading_payload() {
        use nyaterm_core::plugins::{ErrorCode, PluginError};
        let root = fixture();
        let result = verify_with_trust(
            root.path(),
            NativeComponentId::Vnc,
            &|| Err(PluginError::new(ErrorCode::Cancelled, "cancelled")),
            &trust(),
        );
        assert_eq!(result.unwrap_err().code, ErrorCode::Cancelled);
    }
}
