//! Selection and persistent identity of independently released native components.
use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::plugins::native::{MAX_ARCHIVE_BYTES, NativeComponentId};
use crate::plugins::{ErrorCode, PluginError, PluginResult, validate_resource_path};

pub const INDEX_SCHEMA: u32 = 1;
pub const MAX_INDEX_BYTES: u64 = 4 * 1024 * 1024;

pub fn compatible(requirement: &str, host: &str) -> bool {
    match (
        semver::VersionReq::parse(requirement),
        semver::Version::parse(host),
    ) {
        (Ok(requirement), Ok(host)) => requirement.matches(&host),
        _ => false,
    }
}

pub fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstalledComponent {
    pub version: String,
    /// Digest of the signed manifest, independent of ZIP compression.
    pub digest: String,
}

impl InstalledComponent {
    pub fn validate(&self) -> PluginResult<()> {
        if semver::Version::parse(&self.version).is_err() || !valid_digest(&self.digest) {
            return Err(invalid());
        }
        validate_resource_path(&self.directory())
    }
    pub fn directory(&self) -> String {
        format!("{}-{}", self.version, self.digest)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeSelection {
    pub active: Option<InstalledComponent>,
    pub previous: Option<InstalledComponent>,
}

impl NativeSelection {
    pub fn validate(&self) -> PluginResult<()> {
        for installed in [&self.active, &self.previous].into_iter().flatten() {
            installed.validate()?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeRelease {
    pub component: NativeComponentId,
    pub version: String,
    pub host_version: String,
    pub ipc_version: u32,
    pub target: String,
    pub runtime: String,
    pub url: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeIndex {
    pub schema_version: u32,
    pub releases: Vec<NativeRelease>,
}

fn invalid() -> PluginError {
    PluginError::new(
        ErrorCode::InvalidManifest,
        "Invalid native component index or identity",
    )
}

impl NativeIndex {
    pub fn validate(&self) -> PluginResult<()> {
        if self.schema_version != INDEX_SCHEMA || self.releases.len() > 10_000 {
            return Err(invalid());
        }
        let mut identities = BTreeSet::new();
        for release in &self.releases {
            let url = url::Url::parse(&release.url).map_err(|_| invalid())?;
            if url.scheme() != "https"
                || url.host_str() != Some("downloads.nyaterm.app")
                || !url.username().is_empty()
                || url.password().is_some()
                || url.port().is_some()
                || url.fragment().is_some()
                || !url.path().starts_with("/native/v1/")
                || release.size == 0
                || release.size > MAX_ARCHIVE_BYTES
                || !valid_digest(&release.sha256)
                || semver::Version::parse(&release.version).is_err()
                || semver::VersionReq::parse(&release.host_version).is_err()
                || !identities.insert((
                    release.component,
                    &release.version,
                    &release.target,
                    &release.runtime,
                ))
            {
                return Err(invalid());
            }
        }
        Ok(())
    }

    pub fn select(
        &self,
        component: NativeComponentId,
        host: &str,
        target: &str,
        runtime: &str,
    ) -> Option<&NativeRelease> {
        self.releases
            .iter()
            .filter_map(|release| {
                let version = semver::Version::parse(&release.version).ok()?;
                (release.component == component
                    && release.target == target
                    && release.runtime == runtime
                    && release.ipc_version == component.ipc_version()
                    && version.pre.is_empty()
                    && compatible(&release.host_version, host))
                .then_some((version, release))
            })
            .max_by(|a, b| a.0.cmp(&b.0))
            .map(|(_, release)| release)
    }
}

#[cfg(test)]
mod tests {
    use crate::plugins::native::NativeComponentId;
    use crate::plugins::native::catalog::{NativeIndex, NativeRelease};

    fn release(version: &str) -> NativeRelease {
        NativeRelease {
            component: NativeComponentId::Vnc,
            version: version.into(),
            host_version: ">=2.0.8, <3".into(),
            ipc_version: NativeComponentId::Vnc.ipc_version(),
            target: "x86_64-pc-windows-msvc".into(),
            runtime: "windows-msvc".into(),
            url: format!("https://downloads.nyaterm.app/native/v1/vnc/{version}/package.zip"),
            size: 123,
            sha256: "a".repeat(64),
        }
    }

    #[test]
    fn highest_compatible_stable_version_is_independent_of_host_version() {
        let mut incompatible = release("4.0.0");
        incompatible.ipc_version += 1;
        let index = NativeIndex {
            schema_version: 1,
            releases: vec![
                release("1.0.0"),
                release("1.2.0"),
                release("2.0.0-beta.1"),
                incompatible,
            ],
        };
        index.validate().unwrap();
        let chosen = index
            .select(
                NativeComponentId::Vnc,
                "2.1.0",
                "x86_64-pc-windows-msvc",
                "windows-msvc",
            )
            .unwrap();
        assert_eq!(chosen.version, "1.2.0");
        assert!(
            index
                .select(
                    NativeComponentId::Vnc,
                    "3.0.0",
                    &chosen.target,
                    &chosen.runtime
                )
                .is_none()
        );
        assert!(
            index
                .select(
                    NativeComponentId::Vnc,
                    "2.1.0",
                    &chosen.target,
                    "glibc-2.31"
                )
                .is_none()
        );
    }

    #[test]
    fn conflicting_identity_and_untrusted_download_origin_are_rejected() {
        let mut index = NativeIndex {
            schema_version: 1,
            releases: vec![release("1.0.0"), release("1.0.0")],
        };
        assert!(index.validate().is_err());
        index.releases.pop();
        index.releases[0].url = "https://untrusted.invalid/native/v1/package.zip".into();
        assert!(index.validate().is_err());
    }
}
