use crate::plugins::extension::Capability;
use crate::plugins::{API_VERSION, ErrorCode, PluginError, PluginResult, validate_id};
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    pub catalog_version: u32,
    pub extensions: Vec<Listing>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Listing {
    pub id: String,
    pub name: String,
    pub description: String,
    pub publisher: String,
    pub releases: Vec<Release>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Release {
    pub version: String,
    pub host_version: String,
    pub api_version: String,
    pub capabilities: Vec<Capability>,
    pub artifact: Artifact,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    pub url: String,
    pub sha256: String,
    pub size: u64,
    pub signature: String,
}

impl Catalog {
    pub fn parse(bytes: &[u8]) -> PluginResult<Self> {
        let invalid = || PluginError::new(ErrorCode::InvalidManifest, "Invalid extension catalog");
        if bytes.len() > 4 * 1024 * 1024 {
            return Err(invalid());
        }
        let catalog: Self = serde_json::from_slice(bytes).map_err(|_| invalid())?;
        if catalog.catalog_version != 2 || catalog.extensions.len() > 4096 {
            return Err(invalid());
        }
        let mut ids = BTreeSet::new();
        for listing in &catalog.extensions {
            validate_id(&listing.id)?;
            validate_id(&listing.publisher)?;
            if listing.id.starts_with("nyaterm-native-")
                || !ids.insert(&listing.id)
                || listing.name.is_empty()
                || listing.name.len() > 160
                || listing.description.len() > 4096
                || listing.publisher.is_empty()
                || listing.publisher.len() > 160
                || listing.releases.len() > 128
            {
                return Err(invalid());
            }
            let mut versions = BTreeSet::new();
            for release in &listing.releases {
                Version::parse(&release.version).map_err(|_| invalid())?;
                Version::parse(&release.api_version).map_err(|_| invalid())?;
                VersionReq::parse(&release.host_version).map_err(|_| invalid())?;
                let artifact = &release.artifact;
                let url = url::Url::parse(&artifact.url).map_err(|_| invalid())?;
                let unique_capabilities: BTreeSet<_> = release.capabilities.iter().collect();
                if !versions.insert(&release.version)
                    || release.api_version != API_VERSION
                    || unique_capabilities.len() != release.capabilities.len()
                    || url.scheme() != "https"
                    || url.host_str().is_none()
                    || !url.username().is_empty()
                    || url.password().is_some()
                    || url.fragment().is_some()
                    || artifact.url.len() > 4096
                    || artifact.size == 0
                    || artifact.size > 64 * 1024 * 1024
                    || artifact.sha256.len() != 64
                    || !artifact
                        .sha256
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                    || artifact.signature.len() > 8192
                    || artifact.signature.is_empty()
                {
                    return Err(invalid());
                }
            }
        }
        Ok(catalog)
    }
}

impl Listing {
    pub fn compatible_release(&self, host_version: &str) -> Option<&Release> {
        let host = Version::parse(host_version).ok()?;
        self.releases
            .iter()
            .filter(|release| {
                release.api_version == API_VERSION
                    && VersionReq::parse(&release.host_version)
                        .is_ok_and(|requirement| requirement.matches(&host))
            })
            .filter_map(|release| {
                Version::parse(&release.version)
                    .ok()
                    .map(|version| (version, release))
            })
            .max_by(|a, b| a.0.cmp(&b.0))
            .map(|(_, release)| release)
    }
}

#[cfg(test)]
mod tests {
    use crate::plugins::catalog::{Artifact, Listing, Release};
    #[test]
    fn chooses_highest_compatible_release_instead_of_latest() {
        let release = |version: &str, host: &str| Release {
            version: version.into(),
            host_version: host.into(),
            api_version: "2.0.0".into(),
            capabilities: vec![],
            artifact: Artifact {
                url: "https://example.com/plugin.zip".into(),
                sha256: "a".repeat(64),
                size: 1,
                signature: "test".into(),
            },
        };
        let listing = Listing {
            id: "test".into(),
            name: "Test".into(),
            description: String::new(),
            publisher: "test".into(),
            releases: vec![
                release("1.0.0", ">=2.0.0"),
                release("3.0.0", ">=3.0.0"),
                release("2.0.0", ">=2.0.0"),
            ],
        };
        assert_eq!(
            listing.compatible_release("2.0.8").unwrap().version,
            "2.0.0"
        );
    }
}
