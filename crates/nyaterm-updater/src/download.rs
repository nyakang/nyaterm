use crate::error::{UpdateError, UpdatePackageProblem};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use nyaterm_core::updater::{
    UpdateManifest, UpdatePackageKind, UpdateTarget, parse_current_version,
};
use std::io::{Read as _, Write as _};
use std::path::Path;

const PUBLIC_KEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDgyQUYxQTA2NTYyQTNEOTkKUldTWlBTcFdCaHF2Z29pS0pEdE13U3ZUMVZVTlpGVmQ0YlU2cWlORkdNWU1BY005MU01YjFiU2IK";

/// Resolved off the UI thread, then retained for download and restart.
#[derive(Clone, Debug)]
pub struct InstallationTarget {
    pub package: UpdatePackageKind,
    pub executable: std::path::PathBuf,
}

pub fn detect_installation(portable: bool) -> Option<InstallationTarget> {
    if cfg!(debug_assertions) || nyaterm_core::packaging::updates_managed_externally() {
        return None;
    }
    let executable = std::env::current_exe().ok()?.canonicalize().ok()?;
    if supports_file_install(portable) {
        return Some(InstallationTarget {
            package: if portable {
                UpdatePackageKind::WindowsPortable
            } else {
                UpdatePackageKind::Installed
            },
            executable,
        });
    }
    #[cfg(target_os = "linux")]
    if !portable {
        return crate::linux::detect_package(&executable).map(|package| InstallationTarget {
            package,
            executable,
        });
    }
    None
}

pub fn supports_native_install(portable: bool) -> bool {
    detect_installation(portable).is_some()
}

fn supports_file_install(portable: bool) -> bool {
    if cfg!(debug_assertions) {
        return false;
    }
    let Ok(executable) = std::env::current_exe() else {
        return false;
    };
    #[cfg(windows)]
    {
        let Some(directory) = executable.parent() else {
            return false;
        };
        return if portable {
            directory.join("nyaterm-portable").is_file()
        } else {
            directory.join("Uninstall.exe").is_file()
        };
    }
    #[cfg(target_os = "macos")]
    {
        if portable {
            return false;
        }
        let Some(bundle) = executable.ancestors().nth(3) else {
            return false;
        };
        return bundle.extension().and_then(|value| value.to_str()) == Some("app")
            && !bundle
                .components()
                .any(|part| part.as_os_str() == "Caskroom" || part.as_os_str() == "Cellar");
    }
    #[cfg(target_os = "linux")]
    {
        let _ = executable;
        return !portable && std::env::var_os("APPIMAGE").is_some();
    }
    #[allow(unreachable_code)]
    false
}

fn decode_update_signature(value: &str) -> Result<minisign_verify::Signature, UpdateError> {
    let signature = STANDARD.decode(value).map_err(|_| {
        UpdateError::package(
            UpdatePackageProblem::Signature,
            "invalid update signature encoding",
        )
    })?;
    minisign_verify::Signature::decode(std::str::from_utf8(&signature).map_err(|_| {
        UpdateError::package(UpdatePackageProblem::Signature, "invalid update signature")
    })?)
    .map_err(|_| UpdateError::package(UpdatePackageProblem::Signature, "invalid update signature"))
}

/// Download and verify an artifact on a blocking worker before preparing installation.
/// The caller supplies cancellation checks and receives byte progress without UI types.
pub fn download_signed_update(
    version: &str,
    directory: &Path,
    installation: &InstallationTarget,
    check_cancel: impl Fn() -> Result<(), String>,
    mut progress: impl FnMut(u64, Option<u64>),
) -> Result<super::install::PreparedUpdate, UpdateError> {
    let version = parse_current_version(version)?;
    if !nyaterm_core::app_identity::AppFlavor::current().accepts_update(&version) {
        return Err(UpdateError::package(
            UpdatePackageProblem::Rejected,
            "update belongs to a different application flavor",
        ));
    }
    let target = UpdateTarget::from_rust_target(std::env::consts::OS, std::env::consts::ARCH)?;
    let package = installation.package;
    let client = zed_reqwest::blocking::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(20))
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .map_err(UpdateError::download)?;
    let manifest_url = format!("https://downloads.nyaterm.app/releases/v{version}/latest.json");
    let mut manifest_body = String::new();
    client
        .get(&manifest_url)
        .send()
        .map_err(UpdateError::download)?
        .error_for_status()
        .map_err(UpdateError::download)?
        .take(1024 * 1024 + 1)
        .read_to_string(&mut manifest_body)
        .map_err(UpdateError::download)?;
    if manifest_body.len() > 1024 * 1024 {
        return Err(UpdateError::package(
            UpdatePackageProblem::Invalid,
            "update manifest too large",
        ));
    }
    let manifest = UpdateManifest::parse_for_version(&manifest_body, &version)?;
    let selected = manifest.select_artifact(&version, target, package)?;
    let signature_text = selected.signature.clone();
    let signature = decode_update_signature(&signature_text)?;
    let public_key = STANDARD
        .decode(PUBLIC_KEY)
        .map_err(|_| UpdateError::package(UpdatePackageProblem::Signature, "invalid public key"))?;
    let public_key =
        minisign_verify::PublicKey::decode(std::str::from_utf8(&public_key).map_err(|_| {
            UpdateError::package(UpdatePackageProblem::Signature, "invalid public key")
        })?)
        .map_err(|_| UpdateError::package(UpdatePackageProblem::Signature, "invalid public key"))?;
    let mut verifier = public_key.verify_stream(&signature).map_err(|_| {
        UpdateError::package(
            UpdatePackageProblem::Signature,
            "unsupported update signature",
        )
    })?;
    check_cancel().map_err(UpdateError::download)?;
    std::fs::create_dir_all(directory).map_err(UpdateError::download)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))
            .map_err(UpdateError::download)?;
    }
    let name = selected.filename;
    let partial = directory.join(format!("{name}.partial"));
    let artifact = directory.join(&name);
    let result = (|| {
        let mut response = None;
        let mut last_error = None;
        for url in std::iter::once(selected.url.as_str()).chain(selected.fallback_url.as_deref()) {
            match client
                .get(url)
                .send()
                .and_then(|response| response.error_for_status())
            {
                Ok(candidate) => {
                    response = Some(candidate);
                    break;
                }
                Err(error) => last_error = Some(error.to_string()),
            }
        }
        let mut response = response.ok_or_else(|| {
            UpdateError::download(
                last_error.unwrap_or_else(|| "no update download URL is available".to_string()),
            )
        })?;
        let total = response.content_length();
        if total.is_some_and(|total| total > 1024 * 1024 * 1024) {
            return Err(UpdateError::package(
                UpdatePackageProblem::Invalid,
                "update too large",
            ));
        }
        let mut output = std::fs::File::create(&partial).map_err(UpdateError::download)?;
        let mut buffer = vec![0; 256 * 1024];
        let mut received = 0;
        loop {
            check_cancel().map_err(UpdateError::download)?;
            let count = response.read(&mut buffer).map_err(UpdateError::download)?;
            if count == 0 {
                break;
            }
            received += count as u64;
            if received > 1024 * 1024 * 1024 {
                return Err(UpdateError::package(
                    UpdatePackageProblem::Invalid,
                    "update too large",
                ));
            }
            verifier.update(&buffer[..count]);
            output
                .write_all(&buffer[..count])
                .map_err(UpdateError::download)?;
            progress(received, total);
        }
        if total.is_some_and(|total| received != total) {
            return Err(UpdateError::download("update download truncated"));
        }
        verifier.finalize().map_err(|_| {
            UpdateError::package(
                UpdatePackageProblem::Signature,
                "update signature verification failed",
            )
        })?;
        output.sync_all().map_err(UpdateError::download)?;
        drop(output);
        check_cancel().map_err(UpdateError::download)?;
        std::fs::rename(&partial, &artifact).map_err(UpdateError::download)?;
        #[cfg(target_os = "linux")]
        if crate::linux::is_system_package(package) {
            return crate::linux::PreparedPackage::prepare(
                artifact,
                installation.clone(),
                version.to_string(),
                signature_text,
            )
            .map(super::install::PreparedUpdate::LinuxPackage);
        }
        super::install::prepare_artifact(
            artifact,
            installation.executable.clone(),
            package == UpdatePackageKind::WindowsPortable,
        )
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(partial);
    }
    result
}

#[cfg(target_os = "linux")]
pub(crate) fn verify_package_file(path: &Path, signature: &str) -> Result<(), UpdateError> {
    let signature = decode_update_signature(signature)?;
    let key = STANDARD.decode(PUBLIC_KEY).map_err(UpdateError::download)?;
    let key = minisign_verify::PublicKey::decode(
        std::str::from_utf8(&key).map_err(UpdateError::download)?,
    )
    .map_err(UpdateError::download)?;
    let mut verifier = key
        .verify_stream(&signature)
        .map_err(UpdateError::download)?;
    let mut file = std::fs::File::open(path).map_err(UpdateError::download)?;
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(UpdateError::download)?;
        if count == 0 {
            break;
        }
        verifier.update(&buffer[..count]);
    }
    verifier.finalize().map_err(|_| {
        UpdateError::package(
            UpdatePackageProblem::Signature,
            "update signature verification failed",
        )
    })
}

#[cfg(test)]
mod tests {
    use super::{PUBLIC_KEY, decode_update_signature, supports_native_install};
    use crate::error::{UpdateError, UpdatePackageProblem};
    use base64::{Engine as _, engine::general_purpose::STANDARD};

    #[test]
    fn published_signing_key_loads_and_debug_builds_do_not_self_update() {
        let key = STANDARD.decode(PUBLIC_KEY).unwrap();
        assert!(minisign_verify::PublicKey::decode(std::str::from_utf8(&key).unwrap()).is_ok());
        if cfg!(debug_assertions) {
            assert!(!supports_native_install(true));
            assert!(!supports_native_install(false));
        }
    }

    #[test]
    fn malformed_update_signatures_are_rejected_before_download() {
        for signature in [
            "not-base64".to_string(),
            STANDARD.encode("not a minisign signature"),
        ] {
            assert!(matches!(
                decode_update_signature(&signature),
                Err(UpdateError::Package {
                    problem: UpdatePackageProblem::Signature,
                    ..
                })
            ));
        }
    }
}
