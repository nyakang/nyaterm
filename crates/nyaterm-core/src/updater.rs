use std::collections::BTreeMap;

use semver::Version;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

use crate::app_identity::AppFlavor;

pub const STABLE_MANIFEST_URL: &str = "https://downloads.nyaterm.app/latest.json";
pub const PREVIEW_MANIFEST_URL: &str = "https://downloads.nyaterm.app/channels/preview/latest.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateChannel {
    Stable,
    Preview,
}

impl UpdateChannel {
    pub fn for_version(version: &Version) -> Self {
        AppFlavor::for_version(version).into()
    }

    pub fn manifest_url(self) -> &'static str {
        match self {
            Self::Stable => STABLE_MANIFEST_URL,
            Self::Preview => PREVIEW_MANIFEST_URL,
        }
    }
}

impl From<AppFlavor> for UpdateChannel {
    fn from(flavor: AppFlavor) -> Self {
        match flavor {
            AppFlavor::Stable => Self::Stable,
            AppFlavor::Preview => Self::Preview,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpdateManifest {
    pub version: Version,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub pub_date: Option<String>,
    pub platforms: BTreeMap<String, UpdateArtifact>,
}

impl UpdateManifest {
    pub fn parse(body: &str) -> Result<Self, UpdaterError> {
        let manifest: Self = serde_json::from_str(body).map_err(UpdaterError::ManifestParse)?;
        if manifest.platforms.is_empty() {
            return Err(UpdaterError::MissingPlatforms);
        }
        Ok(manifest)
    }

    pub fn parse_for_version(body: &str, expected: &Version) -> Result<Self, UpdaterError> {
        let manifest = Self::parse(body)?;
        if &manifest.version != expected {
            return Err(UpdaterError::ManifestVersionMismatch {
                expected: expected.clone(),
                actual: manifest.version,
            });
        }
        Ok(manifest)
    }

    pub fn update_info(&self, current: &Version) -> NativeUpdateInfo {
        NativeUpdateInfo {
            current_version: current.to_string(),
            latest_version: self.version.to_string(),
            release_date: non_empty(self.pub_date.clone()),
            release_notes: non_empty(self.notes.clone()),
            html_url: Some(format!(
                "https://github.com/nyakang/nyaterm/releases/tag/v{}",
                self.version
            )),
            available: self.version > *current,
        }
    }

    pub fn select_artifact(
        &self,
        expected_version: &Version,
        target: UpdateTarget,
        package: UpdatePackageKind,
    ) -> Result<SelectedUpdateArtifact, UpdaterError> {
        if &self.version != expected_version {
            return Err(UpdaterError::ManifestVersionMismatch {
                expected: expected_version.clone(),
                actual: self.version.clone(),
            });
        }

        let key = target.manifest_key(package)?;
        let artifact = self
            .platforms
            .get(&key)
            .ok_or_else(|| UpdaterError::MissingArtifact(key))?;
        if artifact.signature.trim().is_empty() {
            return Err(UpdaterError::MissingSignature);
        }

        let filename = target.artifact_filename(expected_version, package)?;
        let immutable_url =
            format!("https://downloads.nyaterm.app/releases/v{expected_version}/{filename}");
        let github_url = format!(
            "https://github.com/nyakang/nyaterm/releases/download/v{expected_version}/{filename}"
        );
        if !artifact_url_matches(&artifact.url, &immutable_url, &github_url) {
            return Err(UpdaterError::InvalidArtifactUrl);
        }

        Ok(SelectedUpdateArtifact {
            url: artifact.url.clone(),
            fallback_url: (artifact.url != github_url).then_some(github_url),
            signature: artifact.signature.clone(),
            filename,
        })
    }
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.trim().is_empty())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpdateArtifact {
    pub url: String,
    #[serde(default)]
    pub signature: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedUpdateArtifact {
    pub url: String,
    pub fallback_url: Option<String>,
    pub signature: String,
    pub filename: String,
}

fn artifact_url_matches(candidate: &str, immutable_url: &str, github_url: &str) -> bool {
    let Ok(candidate) = Url::parse(candidate) else {
        return false;
    };
    if candidate.username() != ""
        || candidate.password().is_some()
        || candidate.port().is_some()
        || candidate.query().is_some()
        || candidate.fragment().is_some()
    {
        return false;
    }
    candidate.as_str() == immutable_url || candidate.as_str() == github_url
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdatePlatform {
    Windows,
    MacOs,
    Linux,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateArchitecture {
    X86_64,
    Aarch64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdatePackageKind {
    Installed,
    WindowsPortable,
    LinuxDeb,
    LinuxDebGlibc231,
    LinuxRpm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpdateTarget {
    pub platform: UpdatePlatform,
    pub architecture: UpdateArchitecture,
}

impl UpdateTarget {
    pub fn from_rust_target(platform: &str, architecture: &str) -> Result<Self, UpdaterError> {
        let platform = match platform {
            "windows" => UpdatePlatform::Windows,
            "macos" | "darwin" => UpdatePlatform::MacOs,
            "linux" => UpdatePlatform::Linux,
            other => return Err(UpdaterError::UnsupportedPlatform(other.to_string())),
        };
        let architecture = match architecture {
            "x86_64" => UpdateArchitecture::X86_64,
            "aarch64" => UpdateArchitecture::Aarch64,
            other => return Err(UpdaterError::UnsupportedArchitecture(other.to_string())),
        };
        Ok(Self {
            platform,
            architecture,
        })
    }

    fn manifest_key(self, package: UpdatePackageKind) -> Result<String, UpdaterError> {
        let platform = match self.platform {
            UpdatePlatform::Windows => "windows",
            UpdatePlatform::MacOs => "darwin",
            UpdatePlatform::Linux => "linux",
        };
        let architecture = match self.architecture {
            UpdateArchitecture::X86_64 => "x86_64",
            UpdateArchitecture::Aarch64 => "aarch64",
        };
        let suffix = match package {
            UpdatePackageKind::Installed => "",
            UpdatePackageKind::LinuxDeb if self.platform == UpdatePlatform::Linux => "-deb",
            UpdatePackageKind::LinuxRpm if self.platform == UpdatePlatform::Linux => "-rpm",
            UpdatePackageKind::LinuxDebGlibc231
                if self.platform == UpdatePlatform::Linux
                    && self.architecture == UpdateArchitecture::X86_64 =>
            {
                "-deb-glibc231"
            }
            UpdatePackageKind::WindowsPortable if self.platform == UpdatePlatform::Windows => {
                "-portable"
            }
            _ => {
                return Err(UpdaterError::UnsupportedPackageKind);
            }
        };
        Ok(format!("{platform}-{architecture}{suffix}"))
    }

    fn artifact_filename(
        self,
        version: &Version,
        package: UpdatePackageKind,
    ) -> Result<String, UpdaterError> {
        let architecture = match self.architecture {
            UpdateArchitecture::X86_64 => "x64",
            UpdateArchitecture::Aarch64 => "arm64",
        };
        let suffix = match (self.platform, package) {
            (UpdatePlatform::Linux, UpdatePackageKind::LinuxDeb) => {
                format!("linux_{architecture}.deb")
            }
            (UpdatePlatform::Linux, UpdatePackageKind::LinuxRpm) => {
                format!("linux_{architecture}.rpm")
            }
            (UpdatePlatform::Linux, UpdatePackageKind::LinuxDebGlibc231)
                if self.architecture == UpdateArchitecture::X86_64 =>
            {
                "linux_x64_glibc231.deb".to_string()
            }
            (UpdatePlatform::Windows, UpdatePackageKind::Installed) => {
                format!("windows_{architecture}-setup.exe")
            }
            (UpdatePlatform::Windows, UpdatePackageKind::WindowsPortable) => {
                format!("windows_{architecture}_portable.zip")
            }
            (_, UpdatePackageKind::WindowsPortable) => {
                return Err(UpdaterError::UnsupportedPackageKind);
            }
            (UpdatePlatform::MacOs, UpdatePackageKind::Installed) => {
                format!("macos_{architecture}.app.tar.gz")
            }
            (UpdatePlatform::Linux, UpdatePackageKind::Installed) => {
                format!("linux_{architecture}.AppImage")
            }
            _ => return Err(UpdaterError::UnsupportedPackageKind),
        };
        Ok(format!("NyaTerm_{version}_{suffix}"))
    }
}

#[derive(Debug, Error)]
pub enum UpdaterError {
    #[error("invalid application version `{value}`: {source}")]
    InvalidVersion {
        value: String,
        source: semver::Error,
    },
    #[error("update manifest is invalid: {0}")]
    ManifestParse(serde_json::Error),
    #[error("update manifest does not contain any platforms")]
    MissingPlatforms,
    #[error("update manifest version mismatch: expected {expected}, got {actual}")]
    ManifestVersionMismatch { expected: Version, actual: Version },
    #[error("unsupported update platform `{0}`")]
    UnsupportedPlatform(String),
    #[error("unsupported update architecture `{0}`")]
    UnsupportedArchitecture(String),
    #[error("the selected update package is not supported on this platform")]
    UnsupportedPackageKind,
    #[error("update manifest does not contain artifact `{0}`")]
    MissingArtifact(String),
    #[error("update artifact URL does not match the selected platform and version")]
    InvalidArtifactUrl,
    #[error("update artifact signature is missing")]
    MissingSignature,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NativeUpdateInfo {
    pub current_version: String,
    pub latest_version: String,
    #[serde(default)]
    pub release_date: Option<String>,
    #[serde(default)]
    pub release_notes: Option<String>,
    #[serde(default)]
    pub html_url: Option<String>,
    pub available: bool,
}

pub fn parse_current_version(value: &str) -> Result<Version, UpdaterError> {
    let normalized = value.trim().trim_start_matches(['v', 'V']);
    Version::parse(normalized).map_err(|source| UpdaterError::InvalidVersion {
        value: value.to_string(),
        source,
    })
}

pub fn parse_update_manifest(
    body: &str,
    current_version: &str,
) -> Result<NativeUpdateInfo, UpdaterError> {
    let current = parse_current_version(current_version)?;
    Ok(UpdateManifest::parse(body)?.update_info(&current))
}

#[cfg(test)]
mod tests {
    use semver::Version;

    use super::{
        PREVIEW_MANIFEST_URL, STABLE_MANIFEST_URL, UpdateChannel, UpdateManifest,
        UpdatePackageKind, UpdateTarget, UpdaterError, parse_update_manifest,
    };

    fn manifest(version: &str, platform: &str, url: &str, signature: &str) -> String {
        serde_json::json!({
            "version": version,
            "notes": "release notes",
            "pub_date": "2026-09-18T00:00:00Z",
            "platforms": {
                platform: {
                    "url": url,
                    "signature": signature,
                }
            }
        })
        .to_string()
    }

    #[test]
    fn release_versions_choose_their_expected_channels() {
        let stable = Version::parse("2.0.0").unwrap();
        let preview = Version::parse("2.0.0-preview.1").unwrap();

        assert_eq!(UpdateChannel::for_version(&stable), UpdateChannel::Stable);
        assert_eq!(UpdateChannel::for_version(&preview), UpdateChannel::Preview);
        assert_eq!(UpdateChannel::Stable.manifest_url(), STABLE_MANIFEST_URL);
        assert_eq!(UpdateChannel::Preview.manifest_url(), PREVIEW_MANIFEST_URL);
    }

    #[test]
    fn semver_prerelease_ordering_is_used_for_update_checks() {
        for (current, latest) in [
            ("2.0.0-preview.1", "2.0.0-preview.2"),
            ("2.0.0-preview.2", "2.0.0-preview.10"),
            ("2.0.0-preview.10", "2.0.0-rc.1"),
            ("2.0.0-preview.1", "2.0.0"),
            ("2.0.0-rc.1", "2.0.0"),
        ] {
            let body = manifest(latest, "linux-x86_64", "unused", "signed");
            let info = parse_update_manifest(&body, current).unwrap();
            assert!(
                info.available,
                "expected {latest} to be newer than {current}"
            );
        }
    }

    #[test]
    fn same_or_older_manifest_version_is_not_an_update() {
        let same = manifest("2.0.0-preview.2", "linux-x86_64", "unused", "signed");
        let older = manifest("2.0.0-preview.1", "linux-x86_64", "unused", "signed");

        assert!(
            !parse_update_manifest(&same, "2.0.0-preview.2")
                .unwrap()
                .available
        );
        assert!(
            !parse_update_manifest(&older, "2.0.0-preview.2")
                .unwrap()
                .available
        );
    }

    #[test]
    fn manifest_version_mismatch_is_rejected() {
        let body = manifest("2.0.1", "linux-x86_64", "unused", "signed");
        let expected = Version::parse("2.0.0").unwrap();

        assert!(matches!(
            UpdateManifest::parse_for_version(&body, &expected),
            Err(UpdaterError::ManifestVersionMismatch { .. })
        ));
    }

    #[test]
    fn artifact_selection_rejects_wrong_target_url_and_missing_signature() {
        let version = Version::parse("2.1.0-preview.1").unwrap();
        let version_text = version.to_string();
        let expected_url = "https://downloads.nyaterm.app/releases/v2.1.0-preview.1/NyaTerm_2.1.0-preview.1_windows_x64-setup.exe";
        let target = UpdateTarget::from_rust_target("windows", "x86_64").unwrap();
        let valid = UpdateManifest::parse(&manifest(
            &version_text,
            "windows-x86_64",
            expected_url,
            "signed",
        ))
        .unwrap();
        assert_eq!(
            valid
                .select_artifact(&version, target, UpdatePackageKind::Installed)
                .unwrap()
                .url,
            expected_url
        );

        assert!(
            valid
                .select_artifact(
                    &version,
                    UpdateTarget::from_rust_target("windows", "aarch64").unwrap(),
                    UpdatePackageKind::Installed,
                )
                .is_err()
        );
        assert!(
            valid
                .select_artifact(
                    &version,
                    UpdateTarget::from_rust_target("linux", "x86_64").unwrap(),
                    UpdatePackageKind::Installed,
                )
                .is_err()
        );
        let wrong_url = UpdateManifest::parse(&manifest(
            &version_text,
            "windows-x86_64",
            "https://example.com/update.exe",
            "signed",
        ))
        .unwrap();
        assert!(matches!(
            wrong_url.select_artifact(&version, target, UpdatePackageKind::Installed),
            Err(UpdaterError::InvalidArtifactUrl)
        ));
        let unsigned = UpdateManifest::parse(
            &serde_json::json!({
                "version": version_text,
                "platforms": {
                    "windows-x86_64": {
                        "url": expected_url,
                    }
                }
            })
            .to_string(),
        )
        .unwrap();
        assert!(matches!(
            unsigned.select_artifact(&version, target, UpdatePackageKind::Installed),
            Err(UpdaterError::MissingSignature)
        ));

        let portable_url = "https://downloads.nyaterm.app/releases/v2.1.0-preview.1/NyaTerm_2.1.0-preview.1_windows_x64_portable.zip";
        let portable = UpdateManifest::parse(&manifest(
            &version_text,
            "windows-x86_64-portable",
            portable_url,
            "signed",
        ))
        .unwrap();
        let selected = portable
            .select_artifact(&version, target, UpdatePackageKind::WindowsPortable)
            .unwrap();
        assert_eq!(selected.url, portable_url);
        assert_eq!(
            selected.fallback_url.as_deref(),
            Some(
                "https://github.com/nyakang/nyaterm/releases/download/v2.1.0-preview.1/NyaTerm_2.1.0-preview.1_windows_x64_portable.zip"
            )
        );
    }

    #[test]
    fn artifact_selection_rejects_noncanonical_url_components() {
        let version = Version::parse("2.1.0-preview.1").unwrap();
        let target = UpdateTarget::from_rust_target("windows", "x86_64").unwrap();
        for url in [
            "https://download.nyaterm.app/releases/v2.1.0-preview.1/NyaTerm_2.1.0-preview.1_windows_x64-setup.exe",
            "https://downloads.nyaterm.app/releases/releases/v2.1.0-preview.1/NyaTerm_2.1.0-preview.1_windows_x64-setup.exe",
            "https://downloads.nyaterm.app/releases/v2.1.0-preview.1/NyaTerm_2.1.0-preview.1_windows_x64-setup.exe?mirror=1",
        ] {
            let manifest = UpdateManifest::parse(&manifest(
                version.to_string().as_str(),
                "windows-x86_64",
                url,
                "signed",
            ))
            .unwrap();
            assert!(matches!(
                manifest.select_artifact(&version, target, UpdatePackageKind::Installed),
                Err(UpdaterError::InvalidArtifactUrl)
            ));
        }
    }

    #[test]
    fn unsupported_platform_and_architecture_are_rejected() {
        assert!(matches!(
            UpdateTarget::from_rust_target("freebsd", "x86_64"),
            Err(UpdaterError::UnsupportedPlatform(_))
        ));
        assert!(matches!(
            UpdateTarget::from_rust_target("linux", "riscv64"),
            Err(UpdaterError::UnsupportedArchitecture(_))
        ));
    }
}

#[cfg(test)]
mod package_update_tests {
    use super::{
        UpdateManifest, UpdatePackageKind, UpdateTarget, UpdaterError, parse_current_version,
    };

    #[test]
    fn linux_package_formats_select_distinct_signed_artifacts_without_changing_appimage() {
        let version = parse_current_version("2.0.9").unwrap();
        let target = UpdateTarget::from_rust_target("linux", "x86_64").unwrap();
        for (kind, key, filename) in [
            (
                UpdatePackageKind::Installed,
                "linux-x86_64",
                "NyaTerm_2.0.9_linux_x64.AppImage",
            ),
            (
                UpdatePackageKind::LinuxDeb,
                "linux-x86_64-deb",
                "NyaTerm_2.0.9_linux_x64.deb",
            ),
            (
                UpdatePackageKind::LinuxRpm,
                "linux-x86_64-rpm",
                "NyaTerm_2.0.9_linux_x64.rpm",
            ),
            (
                UpdatePackageKind::LinuxDebGlibc231,
                "linux-x86_64-deb-glibc231",
                "NyaTerm_2.0.9_linux_x64_glibc231.deb",
            ),
        ] {
            let json = serde_json::json!({ "version": "2.0.9", "platforms": { key: {
                "url": format!("https://downloads.nyaterm.app/releases/v2.0.9/{filename}"), "signature": "signed"
            }}});
            let manifest = UpdateManifest::parse(&json.to_string()).unwrap();
            assert_eq!(
                manifest
                    .select_artifact(&version, target, kind)
                    .unwrap()
                    .filename,
                filename
            );
            if kind != UpdatePackageKind::Installed {
                assert!(matches!(
                    manifest.select_artifact(&version, target, UpdatePackageKind::Installed),
                    Err(UpdaterError::MissingArtifact(_))
                ));
            }
        }
        let arm = UpdateTarget::from_rust_target("linux", "aarch64").unwrap();
        assert!(matches!(
            arm.manifest_key(UpdatePackageKind::LinuxDebGlibc231),
            Err(UpdaterError::UnsupportedPackageKind)
        ));
        let windows = UpdateTarget::from_rust_target("windows", "x86_64").unwrap();
        assert!(matches!(
            windows.manifest_key(UpdatePackageKind::LinuxDeb),
            Err(UpdaterError::UnsupportedPackageKind)
        ));
    }
}
