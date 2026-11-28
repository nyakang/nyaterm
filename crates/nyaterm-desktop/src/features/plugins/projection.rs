use std::path::PathBuf;

use nyaterm_core::plugins::management::{InstallProgress, PluginKey};
use nyaterm_core::plugins::native::{NativeComponentId, NativeStatus};
use nyaterm_core::plugins::{PluginError, PluginStatus};
use nyaterm_plugin_host::manager::PluginSource;
use rust_i18n::t;

use crate::features::plugins::native::{native_description, native_display_name};
use crate::features::plugins::process::PluginProcess;

#[derive(Clone)]
pub(super) struct PluginEntry {
    pub key: PluginKey,
    pub name: String,
    pub description: String,
    pub publisher: String,
    pub installed: bool,
    pub enabled: bool,
    pub compatible: bool,
    pub damaged: bool,
    pub pending_uninstall: bool,
    pub in_use: bool,
    pub installed_version: Option<String>,
    pub available_version: Option<String>,
    pub update_available: bool,
    pub progress: Option<InstallProgress>,
    pub busy: bool,
    pub error: Option<PluginError>,
    pub install_path: Option<PathBuf>,
    pub download_size: Option<u64>,
    pub development: bool,
    pub rollback: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum InstalledGroup {
    Processing,
    Attention,
    Updates,
    Enabled,
    Disabled,
}

impl InstalledGroup {
    pub const ALL: [Self; 5] = [
        Self::Processing,
        Self::Attention,
        Self::Updates,
        Self::Enabled,
        Self::Disabled,
    ];
    pub fn label(self) -> String {
        t!(match self {
            Self::Processing => "plugins.processing",
            Self::Attention => "plugins.attention",
            Self::Updates => "plugins.availableUpdates",
            Self::Enabled => "plugins.enabled",
            Self::Disabled => "plugins.nativeDisabled",
        })
        .to_string()
    }
}

impl PluginEntry {
    pub fn group(&self) -> InstalledGroup {
        if self.busy || self.pending_uninstall {
            InstalledGroup::Processing
        } else if self.error.is_some() || self.damaged || !self.compatible {
            InstalledGroup::Attention
        } else if self.update_available {
            InstalledGroup::Updates
        } else if self.enabled {
            InstalledGroup::Enabled
        } else {
            InstalledGroup::Disabled
        }
    }
    pub fn matches(&self, query: &str) -> bool {
        [
            &self.name,
            &self.description,
            &self.publisher,
            &self.key.id(),
        ]
        .into_iter()
        .any(|field| field.to_lowercase().contains(query))
    }
    pub fn icon(&self) -> &'static str {
        match self.key {
            PluginKey::Native(id) => native_icon(id),
            PluginKey::Wasm(_) => "icons/extensions.svg",
        }
    }
}

pub(super) fn native_icon(id: NativeComponentId) -> &'static str {
    match id {
        NativeComponentId::Spice => "icons/plugins/spice.svg",
        NativeComponentId::Rdp => "icons/plugins/rdp.svg",
        NativeComponentId::Vnc => "icons/plugins/vnc.svg",
        NativeComponentId::Android => "color/os/android.svg",
        NativeComponentId::X11 => "icons/plugins/x11.svg",
    }
}

fn newer(available: Option<&str>, installed: Option<&str>) -> bool {
    available
        .and_then(|v| semver::Version::parse(v).ok())
        .zip(installed.and_then(|v| semver::Version::parse(v).ok()))
        .is_some_and(|(available, installed)| available > installed)
}

pub(super) fn entries(process: &PluginProcess) -> Vec<PluginEntry> {
    let mut entries = Vec::new();
    for native in &process.native {
        let id = native.id;
        let job = process.jobs.get(&id.plugin_id());
        entries.push(PluginEntry {
            key: PluginKey::Native(id),
            name: native_display_name(id),
            description: native_description(id),
            publisher: "NyaTerm".into(),
            installed: native.installed_version.is_some(),
            enabled: native.enabled,
            compatible: id.supported(),
            damaged: matches!(
                native.status,
                NativeStatus::Damaged | NativeStatus::NeedsUpdate
            ),
            pending_uninstall: native.status == NativeStatus::PendingUninstall,
            in_use: native.in_use,
            installed_version: native.installed_version.clone(),
            available_version: native.available_version.clone(),
            update_available: newer(
                native.available_version.as_deref(),
                native.installed_version.as_deref(),
            ),
            progress: native.phase.map(|phase| InstallProgress {
                phase,
                downloaded: native.downloaded,
                total: native.download_size,
            }),
            busy: native.phase.is_some() || job.is_some_and(|j| j.task.is_some()),
            error: job
                .and_then(|j| j.error.clone())
                .or_else(|| native.error.clone()),
            install_path: native.install_path.clone(),
            download_size: native.download_size,
            development: false,
            rollback: native.rollback_available,
        });
    }
    let mut ids: std::collections::BTreeSet<String> = process
        .snapshot
        .plugins
        .iter()
        .map(|p| p.id.clone())
        .collect();
    if let Some(catalog) = &process.store {
        ids.extend(catalog.extensions.iter().map(|p| p.id.clone()));
    }
    for id in ids {
        let installed = process.snapshot.plugins.iter().find(|p| p.id == id);
        let manifest = installed.and_then(|p| p.manifest.as_ref());
        let listing = process
            .store
            .as_ref()
            .and_then(|c| c.extensions.iter().find(|p| p.id == id));
        let release = listing.and_then(|p| p.compatible_release(env!("CARGO_PKG_VERSION")));
        let version = manifest.map(|m| m.version.clone());
        let available = release.map(|r| r.version.clone());
        let development =
            installed.is_some_and(|p| matches!(p.source, PluginSource::Development(_)));
        let job = process.jobs.get(&id);
        entries.push(PluginEntry {
            key: PluginKey::Wasm(id.clone()),
            name: manifest
                .map(|m| m.name.clone())
                .or_else(|| listing.map(|l| l.name.clone()))
                .unwrap_or_else(|| id.clone()),
            description: manifest
                .map(|m| m.description.clone())
                .or_else(|| listing.map(|l| l.description.clone()))
                .unwrap_or_default(),
            publisher: manifest
                .map(|m| m.publisher.clone())
                .filter(|s| !s.is_empty())
                .or_else(|| listing.map(|l| l.publisher.clone()))
                .unwrap_or_default(),
            installed: installed.is_some(),
            enabled: installed.is_some_and(|p| p.enabled),
            compatible: installed.map_or(release.is_some(), |p| {
                p.status != PluginStatus::Incompatible
            }),
            damaged: installed.is_some_and(|p| {
                matches!(p.status, PluginStatus::LoadFailed | PluginStatus::Faulted)
            }),
            pending_uninstall: false,
            in_use: false,
            update_available: !development && newer(available.as_deref(), version.as_deref()),
            installed_version: version,
            available_version: available,
            progress: process.progress.get(&id).cloned(),
            busy: process.progress.contains_key(&id) || job.is_some_and(|j| j.task.is_some()),
            error: job
                .and_then(|j| j.error.clone())
                .or_else(|| installed.and_then(|p| p.error.clone())),
            install_path: installed.map(|p| p.install_path.clone()),
            download_size: release.map(|r| r.artifact.size),
            development,
            rollback: false,
        });
    }
    entries
}

#[cfg(test)]
mod tests {
    use crate::features::plugins::projection::newer;

    #[test]
    fn updates_require_a_strictly_newer_valid_installed_version() {
        assert!(newer(Some("1.10.0"), Some("1.2.0")));
        assert!(!newer(Some("1.0.0"), Some("1.0.0")));
        assert!(!newer(Some("1.0.0"), None));
        assert!(!newer(Some("bad"), Some("1.0.0")));
    }
}

#[cfg(test)]
mod state_tests {
    use crate::features::plugins::process::PluginProcess;
    use crate::features::plugins::projection::{InstalledGroup, entries};
    use nyaterm_core::plugins::management::OperationPhase;
    use nyaterm_core::plugins::native::{NativeComponentId, NativeSnapshot, NativeStatus};
    use nyaterm_core::plugins::{ErrorCode, PluginError, PluginStatus};
    use nyaterm_plugin_host::manager::{PluginSnapshot, PluginSource};

    fn component(id: NativeComponentId) -> NativeSnapshot {
        NativeSnapshot {
            id,
            installed_version: None,
            available_version: Some("2.0.0".into()),
            enabled: false,
            in_use: false,
            install_path: None,
            phase: None,
            rollback_available: false,
            status: NativeStatus::Disabled,
            downloaded: 0,
            download_size: Some(100),
            error: None,
        }
    }

    #[test]
    fn disabled_preference_does_not_count_as_installed_and_disabled_updates_are_visible() {
        let mut process = PluginProcess::empty();
        process.native = vec![
            component(NativeComponentId::Rdp),
            component(NativeComponentId::Vnc),
        ];
        process.native[1].installed_version = Some("1.0.0".into());
        process.snapshot.plugins.push(PluginSnapshot {
            install_path: "/plugins/local".into(),
            session_events_authorized: false,
            id: "local".into(),
            manifest: None,
            enabled: false,
            source: PluginSource::Managed,
            status: PluginStatus::LoadFailed,
            revision: 1,
            error: Some(PluginError::new(ErrorCode::Io, "Unavailable")),
        });
        let rows = entries(&process);
        assert_eq!(rows.iter().filter(|row| row.installed).count(), 2);
        assert!(!rows[0].installed);
        assert_eq!(rows[1].group(), InstalledGroup::Updates);
        assert!(!rows[1].enabled);
        assert_eq!(rows[2].group(), InstalledGroup::Attention);
        assert!(rows[2].installed);
    }

    #[test]
    fn install_progress_and_errors_do_not_create_duplicate_installed_groups() {
        let mut process = PluginProcess::empty();
        process.native = vec![component(NativeComponentId::Rdp)];
        process.native[0].phase = Some(OperationPhase::Downloading);
        let row = entries(&process).remove(0);
        assert!(!row.installed);
        assert!(row.busy);
        assert_eq!(row.group(), InstalledGroup::Processing);
        process.native[0].phase = None;
        process.native[0].error = Some(PluginError::new(ErrorCode::Io, "Unavailable"));
        let row = entries(&process).remove(0);
        assert_eq!(row.group(), InstalledGroup::Attention);
        assert_eq!(
            InstalledGroup::ALL
                .into_iter()
                .filter(|group| *group == row.group())
                .count(),
            1
        );
        assert!(!row.installed);
    }
}
