use std::collections::BTreeMap;
use std::fs::{self, File};
use std::path::PathBuf;
use std::sync::{Arc, Weak, mpsc};

use nyaterm_core::plugins::native::catalog::{InstalledComponent, NativeSelection};
use nyaterm_core::plugins::native::{
    NativeComponentId, NativeLaunch, NativePreferences, NativeStatus, RUNTIME, TARGET,
};
use nyaterm_core::plugins::{ErrorCode, PluginError, PluginResult};
use nyaterm_store::native_component_preferences::NativePreferenceStore;

use crate::native::{Lease, Request, package};
use crate::package::{io_error, reject_path_links};

pub(super) struct Registry {
    pub root: PathBuf,
    pub preferences: NativePreferences,
    store: NativePreferenceStore,
    leases: BTreeMap<PathBuf, Weak<Lease>>,
}

impl Registry {
    pub fn open(root: PathBuf) -> PluginResult<Self> {
        fs::create_dir_all(&root).map_err(|_| io_error())?;
        crate::package::reject_link(&root)?;
        reject_path_links(&root, &root.join("preferences.redb"))?;
        let store = NativePreferenceStore::open(&root.join("preferences.redb"))?;
        let preferences = store.load()?;
        fs::create_dir_all(root.join("staging")).map_err(|_| io_error())?;
        reject_path_links(&root, &root.join("staging"))?;
        let registry = Self {
            root,
            preferences,
            store,
            leases: BTreeMap::new(),
        };
        registry.recover()?;
        Ok(registry)
    }
    fn recover(&self) -> PluginResult<()> {
        // Opening the redb store owns the process-level registry lock. Only UUID
        // staging directories created by this registry are eligible for cleanup.
        for entry in fs::read_dir(self.root.join("staging")).map_err(|_| io_error())? {
            let path = entry.map_err(|_| io_error())?.path();
            reject_path_links(&self.root, &path)?;
            if !path.is_dir()
                || path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .and_then(|n| uuid::Uuid::parse_str(n).ok())
                    .is_none()
            {
                continue;
            }
            let journal = path.join("transaction.json");
            if journal.exists() {
                let raw = crate::package::read_bounded(&journal, 4096)?;
                let (id, installed): (NativeComponentId, InstalledComponent) =
                    serde_json::from_slice(&raw).map_err(|_| io_error())?;
                installed.validate()?;
                let target = self.version_directory(id, &installed);
                reject_path_links(&self.root, &target)?;
                let old = path.join("old");
                reject_path_links(&self.root, &old)?;
                if !target.exists() && old.exists() {
                    fs::rename(old, target).map_err(|_| io_error())?;
                }
            }
            fs::remove_dir_all(path).map_err(|_| io_error())?;
        }
        Ok(())
    }
    fn identity(id: NativeComponentId) -> String {
        format!("{}/{TARGET}/{RUNTIME}", id.slug())
    }
    pub fn selection(&self, id: NativeComponentId) -> PluginResult<NativeSelection> {
        self.store.selection(&Self::identity(id))
    }
    pub fn version_directory(
        &self,
        id: NativeComponentId,
        version: &InstalledComponent,
    ) -> PathBuf {
        self.root
            .join(id.slug())
            .join(format!("{TARGET}-{RUNTIME}"))
            .join(version.directory())
    }
    pub fn save(&mut self, preferences: NativePreferences) -> PluginResult<()> {
        self.store.save(&preferences)?;
        self.preferences = preferences;
        Ok(())
    }
    pub fn activate(
        &self,
        id: NativeComponentId,
        installed: InstalledComponent,
    ) -> PluginResult<()> {
        installed.validate()?;
        let mut selection = self.selection(id)?;
        if selection.active.as_ref() == Some(&installed) {
            return Ok(());
        }
        selection.previous = selection.active.replace(installed);
        self.store.save_selection(&Self::identity(id), &selection)
    }
    pub fn rollback(&self, id: NativeComponentId) -> PluginResult<()> {
        let mut selection = self.selection(id)?;
        let previous = selection.previous.as_ref().ok_or_else(|| {
            PluginError::new(ErrorCode::NotFound, "No previous component version")
        })?;
        let directory = self.version_directory(id, previous);
        let manifest = package::verify(&directory, id, &|| Ok(()))?;
        if manifest.component_version != previous.version
            || package::manifest_digest(&directory)? != previous.digest
        {
            return Err(PluginError::new(
                ErrorCode::InvalidManifest,
                "Previous component identity changed",
            ));
        }
        std::mem::swap(&mut selection.active, &mut selection.previous);
        self.store.save_selection(&Self::identity(id), &selection)
    }
    pub fn lock_version(
        &self,
        id: NativeComponentId,
        installed: &InstalledComponent,
        shared: bool,
    ) -> PluginResult<File> {
        installed.validate()?;
        self.lock_name(
            &format!("{}-{TARGET}-{RUNTIME}-{}", id.slug(), installed.directory()),
            shared,
        )
    }
    fn lock_name(&self, name: &str, shared: bool) -> PluginResult<File> {
        let path = self.root.join(format!("{name}.lock"));
        reject_path_links(&self.root, &path)?;
        let file = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(|_| io_error())?;
        let result = if shared {
            file.try_lock_shared()
        } else {
            file.try_lock()
        };
        result.map_err(|_| {
            PluginError::new(ErrorCode::Conflict, "Native component version is in use")
        })?;
        Ok(file)
    }
    pub fn install_lock(&self, id: NativeComponentId) -> PluginResult<File> {
        self.lock_name(&format!("{}-install", id.slug()), false)
    }
    pub fn status(&self, id: NativeComponentId) -> NativeStatus {
        let preference = self
            .preferences
            .components
            .get(&id)
            .cloned()
            .unwrap_or_default();
        if preference.uninstall_pending {
            return NativeStatus::PendingUninstall;
        }
        if preference.disabled {
            return NativeStatus::Disabled;
        }
        match self.selection(id) {
            Ok(selection) => match selection.active {
                Some(version) => {
                    match package::verify(&self.version_directory(id, &version), id, &|| Ok(())) {
                        Ok(_) => NativeStatus::Ready,
                        Err(error) if error.code == ErrorCode::Incompatible => {
                            NativeStatus::NeedsUpdate
                        }
                        Err(_) => NativeStatus::Damaged,
                    }
                }
                None => NativeStatus::Missing,
            },
            Err(_) => NativeStatus::Damaged,
        }
    }
    pub fn acquire(
        &mut self,
        id: NativeComponentId,
        sender: mpsc::SyncSender<Request>,
    ) -> PluginResult<NativeLaunch> {
        if self
            .preferences
            .components
            .get(&id)
            .is_some_and(|p| p.disabled || p.uninstall_pending)
        {
            return Err(PluginError::new(
                ErrorCode::Disabled,
                "Native component is disabled or pending uninstall",
            ));
        }
        if let Some(path) = std::env::var_os(id.override_variable()).filter(|p| !p.is_empty()) {
            let executable = PathBuf::from(path);
            if !executable.is_file() {
                return Err(PluginError::new(
                    ErrorCode::NotFound,
                    "Explicit helper override does not name a file",
                ));
            }
            return Ok(NativeLaunch {
                root: executable.parent().ok_or_else(io_error)?.into(),
                executable,
                lease: Arc::new(()),
            });
        }
        if let Some(version) = self.selection(id)?.active {
            let root = self.version_directory(id, &version);
            let lease = match self.leases.get(&root).and_then(Weak::upgrade) {
                Some(lease) => lease,
                None => {
                    let lease = Arc::new(Lease {
                        _file: Some(self.lock_version(id, &version, true)?),
                        sender,
                    });
                    self.leases.insert(root.clone(), Arc::downgrade(&lease));
                    lease
                }
            };
            reject_path_links(&self.root, &root)?;
            let manifest = package::verify(&root, id, &|| Ok(()))?;
            if manifest.component_version != version.version
                || package::manifest_digest(&root)? != version.digest
            {
                return Err(PluginError::new(
                    ErrorCode::InvalidManifest,
                    "Installed component identity changed",
                ));
            }
            return Ok(NativeLaunch {
                executable: root.join(manifest.entry),
                root,
                lease,
            });
        }
        if cfg!(debug_assertions) {
            let executable = std::env::current_exe().map_err(|_| io_error())?;
            let root = executable.parent().ok_or_else(io_error)?;
            let helper = root.join(id.helper(TARGET));
            if helper.is_file() {
                return Ok(NativeLaunch {
                    executable: helper,
                    root: root.into(),
                    lease: Arc::new(()),
                });
            }
        }
        Err(PluginError::new(
            ErrorCode::NotFound,
            "Install a compatible official native component in Plugins",
        ))
    }
    pub fn reap(&mut self) {
        for id in NativeComponentId::ALL {
            let _ = self.collect(id);
        }
    }
    fn collect(&mut self, id: NativeComponentId) -> PluginResult<()> {
        let _install = self.install_lock(id)?;
        let pending = self
            .preferences
            .components
            .get(&id)
            .is_some_and(|p| p.uninstall_pending);
        let selection = self.selection(id)?;
        let parent = self
            .root
            .join(id.slug())
            .join(format!("{TARGET}-{RUNTIME}"));
        if !parent.is_dir() && !pending {
            return Ok(());
        }
        reject_path_links(&self.root, &parent)?;
        let mut in_use = false;
        for entry in if parent.is_dir() {
            Some(fs::read_dir(&parent).map_err(|_| io_error())?)
        } else {
            None
        }
        .into_iter()
        .flatten()
        {
            let path = entry.map_err(|_| io_error())?.path();
            reject_path_links(&self.root, &path)?;
            if !path.is_dir() {
                continue;
            }
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(io_error)?;
            if !pending
                && [&selection.active, &selection.previous]
                    .into_iter()
                    .flatten()
                    .any(|v| v.directory() == name)
            {
                continue;
            }
            let Some((version, digest)) = name.rsplit_once('-') else {
                continue;
            };
            let installed = InstalledComponent {
                version: version.into(),
                digest: digest.into(),
            };
            match self.lock_version(id, &installed, false) {
                Ok(_lock) => fs::remove_dir_all(&path).map_err(|_| io_error())?,
                Err(error) if error.code == ErrorCode::Conflict => in_use = true,
                Err(error) => return Err(error),
            }
        }
        if pending && !in_use {
            self.store
                .save_selection(&Self::identity(id), &NativeSelection::default())?;
            let mut preferences = self.preferences.clone();
            let preference = preferences.components.entry(id).or_default();
            preference.disabled = true;
            preference.uninstall_pending = false;
            self.save(preferences)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::native::registry::Registry;
    use nyaterm_core::plugins::native::catalog::InstalledComponent;
    use nyaterm_core::plugins::native::{NativeComponentId, NativePreference};
    use nyaterm_core::test_support::TestTempDir;
    use std::fs;

    fn installed(version: &str) -> InstalledComponent {
        InstalledComponent {
            version: version.into(),
            digest: "a".repeat(64),
        }
    }

    #[test]
    fn activation_preserves_running_and_previous_versions_and_collects_after_release() {
        let root = TestTempDir::new("nyaterm-native-independent-versions");
        let mut registry = Registry::open(root.path().into()).unwrap();
        let id = NativeComponentId::Vnc;
        let versions = [installed("1.0.0"), installed("1.1.0"), installed("1.2.0")];
        for version in &versions {
            fs::create_dir_all(registry.version_directory(id, version)).unwrap();
        }
        registry.activate(id, versions[0].clone()).unwrap();
        let lease = registry.lock_version(id, &versions[0], true).unwrap();
        registry.activate(id, versions[1].clone()).unwrap();
        // Updating a component does not contend with a session using the old version.
        let new_lock = registry.lock_version(id, &versions[1], false).unwrap();
        drop(new_lock);
        registry.activate(id, versions[2].clone()).unwrap();
        registry.reap();
        assert!(registry.version_directory(id, &versions[0]).exists());
        assert!(registry.version_directory(id, &versions[1]).exists());
        drop(lease);
        registry.reap();
        assert!(!registry.version_directory(id, &versions[0]).exists());
        assert_eq!(
            registry.selection(id).unwrap().previous,
            Some(versions[1].clone())
        );
        drop(registry);
        let reopened = Registry::open(root.path().into()).unwrap();
        assert_eq!(
            reopened.selection(id).unwrap().active,
            Some(versions[2].clone())
        );
    }

    #[test]
    fn uninstall_waits_for_each_version_lease_and_preserves_disabled_preference() {
        let root = TestTempDir::new("nyaterm-native-independent-uninstall");
        let mut registry = Registry::open(root.path().into()).unwrap();
        let id = NativeComponentId::Android;
        let version = installed("1.0.0");
        let directory = registry.version_directory(id, &version);
        fs::create_dir_all(&directory).unwrap();
        registry.activate(id, version.clone()).unwrap();
        let lease = registry.lock_version(id, &version, true).unwrap();
        let mut preferences = registry.preferences.clone();
        preferences.components.insert(
            id,
            NativePreference {
                disabled: true,
                uninstall_pending: true,
            },
        );
        registry.save(preferences).unwrap();
        registry.reap();
        assert!(directory.exists());
        assert!(registry.preferences.components[&id].uninstall_pending);
        drop(lease);
        registry.reap();
        assert!(!directory.exists());
        assert!(registry.selection(id).unwrap().active.is_none());
        assert!(registry.preferences.components[&id].disabled);
        assert!(!registry.preferences.components[&id].uninstall_pending);
    }

    #[test]
    fn interrupted_repair_restores_old_directory_before_reaping() {
        let root = TestTempDir::new("nyaterm-native-repair-recovery");
        let registry = Registry::open(root.path().into()).unwrap();
        let id = NativeComponentId::Vnc;
        let version = installed("1.0.0");
        let target = registry.version_directory(id, &version);
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("sentinel"), b"old").unwrap();
        registry.activate(id, version.clone()).unwrap();
        let stage = root
            .path()
            .join("staging")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir(&stage).unwrap();
        fs::write(
            stage.join("transaction.json"),
            serde_json::to_vec(&(id, &version)).unwrap(),
        )
        .unwrap();
        fs::rename(&target, stage.join("old")).unwrap();
        drop(registry);
        let recovered = Registry::open(root.path().into()).unwrap();
        assert_eq!(fs::read(target.join("sentinel")).unwrap(), b"old");
        assert_eq!(recovered.selection(id).unwrap().active, Some(version));
        assert!(!stage.exists());
    }

    #[test]
    fn uninstall_missing_directory_clears_pending_state() {
        let root = TestTempDir::new("nyaterm-native-missing-uninstall");
        let mut registry = Registry::open(root.path().into()).unwrap();
        let id = NativeComponentId::Vnc;
        let mut preferences = registry.preferences.clone();
        preferences.components.insert(
            id,
            NativePreference {
                disabled: true,
                uninstall_pending: true,
            },
        );
        registry.save(preferences).unwrap();
        registry.reap();
        assert!(!registry.preferences.components[&id].uninstall_pending);
        assert!(registry.preferences.components[&id].disabled);
    }

    #[test]
    fn incomplete_install_does_not_change_active_selection() {
        let root = TestTempDir::new("nyaterm-native-independent-recovery");
        let registry = Registry::open(root.path().into()).unwrap();
        let id = NativeComponentId::Rdp;
        let active = installed("1.0.0");
        let unfinished = installed("1.1.0");
        fs::create_dir_all(registry.version_directory(id, &active)).unwrap();
        registry.activate(id, active.clone()).unwrap();
        fs::create_dir_all(registry.version_directory(id, &unfinished)).unwrap();
        drop(registry);
        let mut recovered = Registry::open(root.path().into()).unwrap();
        recovered.reap();
        assert_eq!(recovered.selection(id).unwrap().active, Some(active));
        assert!(!recovered.version_directory(id, &unfinished).exists());
    }
}
