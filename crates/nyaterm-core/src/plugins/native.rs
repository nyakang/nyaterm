//! Signed, platform-specific helper packages. These never execute in the WASM host.
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::plugins::{ErrorCode, PluginError, PluginResult, validate_resource_path};

pub mod catalog;
pub const SCHEMA: u32 = 1;
pub const PACKAGE_SCHEMA: u32 = 2;
pub const RUNTIME: &str = env!("NYATERM_COMPONENT_RUNTIME");
pub const MAX_ARCHIVE_BYTES: u64 = 256 * 1024 * 1024;
pub const MAX_PAYLOAD_BYTES: u64 = 1024 * 1024 * 1024;
pub const MAX_FILES: usize = 4096;
pub const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
pub const TARGET: &str = env!("NYATERM_COMPONENT_TARGET");
pub const PUBLIC_KEY: &str = "RWSZPSpWBhqvgoiKJDtMwSvT1VUNZFVd4bU6qiNFGMYMAcM91M5b1bSb";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeComponentId {
    Spice,
    Rdp,
    Vnc,
    Android,
    X11,
}

impl NativeComponentId {
    pub const ALL: [Self; 5] = [Self::Spice, Self::Rdp, Self::Vnc, Self::Android, Self::X11];
    pub fn supported(self) -> bool {
        self != Self::X11 || cfg!(all(windows, target_arch = "x86_64"))
    }
    pub fn ipc_version(self) -> u32 {
        match self {
            Self::Vnc => nyaterm_native_protocol::protocol::VNC_PROTOCOL_VERSION,
            Self::Rdp => nyaterm_native_protocol::protocol::PROTOCOL_VERSION,
            Self::Spice => nyaterm_native_protocol::spice::protocol::SPICE_PROTOCOL_VERSION.into(),
            Self::Android => nyaterm_native_protocol::android::ipc::PROTOCOL_VERSION,
            Self::X11 => nyaterm_native_protocol::x11::X11_PROTOCOL_VERSION,
        }
    }
    pub fn slug(self) -> &'static str {
        match self {
            Self::Spice => "spice",
            Self::Rdp => "rdp",
            Self::Vnc => "vnc",
            Self::Android => "android",
            Self::X11 => "x11",
        }
    }
    pub fn plugin_id(self) -> String {
        format!("nyaterm-native-{}", self.slug())
    }
    pub fn title(self) -> &'static str {
        match self {
            Self::Spice => "SPICE",
            Self::Rdp => "RDP Builtin",
            Self::Vnc => "VNC",
            Self::Android => "Android",
            Self::X11 => "X11 Server",
        }
    }
    pub fn helper(self, target: &str) -> String {
        format!(
            "nyaterm-{}-helper{}",
            self.slug(),
            if target.contains("windows") {
                ".exe"
            } else {
                ""
            }
        )
    }
    pub fn override_variable(self) -> &'static str {
        match self {
            Self::Spice => "NYATERM_SPICE_HELPER",
            Self::Rdp => "NYATERM_RDP_HELPER",
            Self::Vnc => "NYATERM_VNC_HELPER",
            Self::Android => "NYATERM_ANDROID_HELPER",
            Self::X11 => "NYATERM_X11_HELPER",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeFile {
    pub size: u64,
    pub sha256: String,
    pub executable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeManifest {
    pub schema_version: u32,
    pub component: NativeComponentId,
    pub component_version: String,
    pub host_version: String,
    pub ipc_version: u32,
    pub runtime: String,
    pub build_id: String,
    pub target: String,
    /// Relative executable path inside the independently signed component.
    pub entry: String,
    pub files: BTreeMap<String, NativeFile>,
}

impl NativeManifest {
    pub fn validate(
        &self,
        id: NativeComponentId,
        version: &str,
        runtime: &str,
        target: &str,
    ) -> PluginResult<()> {
        if self.schema_version != PACKAGE_SCHEMA
            || self.component != id
            || !catalog::compatible(&self.host_version, version)
            || semver::Version::parse(&self.component_version).is_err()
            || self.ipc_version != id.ipc_version()
            || self.runtime != runtime
            || self.build_id.len() != 40
            || !self.build_id.bytes().all(|b| b.is_ascii_hexdigit())
            || self.target != target
            || self.entry != id.helper(target)
        {
            return Err(PluginError::new(
                ErrorCode::Incompatible,
                "Native component does not match this host, IPC protocol and platform",
            ));
        }
        if self.files.is_empty()
            || self.files.len() > MAX_FILES
            || !self.files.contains_key(&self.entry)
        {
            return Err(PluginError::new(
                ErrorCode::InvalidManifest,
                "Invalid native component inventory",
            ));
        }
        let mut size = 0u64;
        let mut names = BTreeSet::new();
        for (name, file) in &self.files {
            validate_resource_path(name)?;
            if name == "component.json"
                || name == "component.minisig"
                || !names.insert(name.to_ascii_lowercase())
                || file.sha256.len() != 64
                || !file
                    .sha256
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                return Err(PluginError::new(
                    ErrorCode::InvalidManifest,
                    "Invalid native component file entry",
                ));
            }
            size = size
                .checked_add(file.size)
                .filter(|n| *n <= MAX_PAYLOAD_BYTES)
                .ok_or_else(|| {
                    PluginError::new(
                        ErrorCode::PackageLimit,
                        "Native component payload is too large",
                    )
                })?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativePreference {
    pub disabled: bool,
    pub uninstall_pending: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativePreferences {
    pub schema_version: u32,
    pub components: BTreeMap<NativeComponentId, NativePreference>,
}
impl Default for NativePreferences {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA,
            components: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeStatus {
    Missing,
    Installing,
    Ready,
    Disabled,
    NeedsUpdate,
    UpdateAvailable,
    Damaged,
    PendingUninstall,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeSnapshot {
    pub installed_version: Option<String>,
    pub available_version: Option<String>,
    pub rollback_available: bool,
    pub id: NativeComponentId,
    pub status: NativeStatus,
    pub downloaded: u64,
    pub download_size: Option<u64>,
    pub error: Option<PluginError>,
}

/// Lifetime proof retained until all children using this immutable directory have exited.
#[derive(Clone)]
pub struct NativeLaunch {
    pub executable: PathBuf,
    pub root: PathBuf,
    pub lease: Arc<dyn Send + Sync>,
}
impl std::fmt::Debug for NativeLaunch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeLaunch").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use crate::plugins::native::{NativeComponentId, NativeFile, NativeManifest};
    use std::collections::BTreeMap;
    fn manifest() -> NativeManifest {
        let id = NativeComponentId::Spice;
        NativeManifest {
            schema_version: 2,
            component: id,
            component_version: "1.0.0".into(),
            host_version: ">=2.0.8, <3".into(),
            ipc_version: id.ipc_version(),
            runtime: "windows-msvc".into(),
            build_id: "a".repeat(40),
            target: "x86_64-pc-windows-msvc".into(),
            entry: id.helper("windows"),
            files: BTreeMap::from([(
                id.helper("windows"),
                NativeFile {
                    size: 1,
                    sha256: "a".repeat(64),
                    executable: true,
                },
            )]),
        }
    }
    #[test]
    fn component_version_is_independent_but_host_range_is_enforced() {
        let m = manifest();
        assert!(
            m.validate(m.component, "2.0.8", "windows-msvc", &m.target)
                .is_ok()
        );
        assert!(
            m.validate(m.component, "2.1.0", "windows-msvc", &m.target)
                .is_ok()
        );
        assert!(
            m.validate(m.component, "3.0.0", "windows-msvc", &m.target)
                .is_err()
        );
    }
    #[test]
    fn inventory_rejects_path_aliases_and_wrong_platform() {
        let mut m = manifest();
        assert!(
            m.validate(
                m.component,
                "2.0.8",
                "windows-msvc",
                "aarch64-pc-windows-msvc"
            )
            .is_err()
        );
        m.files
            .insert("../escape".into(), m.files.values().next().unwrap().clone());
        assert!(
            m.validate(m.component, "2.0.8", "windows-msvc", &m.target)
                .is_err()
        );
    }
}
