pub mod reaper;
pub mod worker;

use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::app_identity::AppFlavor;
use crate::data_directory;

/// Handoff marker for the process that a restart is replacing.
///
/// A successor process is spawned before the old process exits. The variable
/// carries a marker file inside the old instance directory and exists only in
/// the successor's environment. It must wait for that instance lock to be
/// released before `AppRuntime::resolve` applies a pending data-directory
/// switch: the migration copies the redb file, which has to be closed first,
/// and acquiring the new instance lock early would make the successor forward
/// its activation to the old process and exit.
pub const RELAUNCH_HANDOFF_ENV: &str = "NYATERM_RELAUNCH_INSTANCE_DIR";

// Fixed at build time so direct launches and MCP agree with the application.
// Ordinary builds retain their existing directories and portable mode wins.
fn linux_compatibility_build() -> bool {
    option_env!("NYATERM_LINUX_COMPAT") == Some("glibc231")
}

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("unable to resolve home directory for installed configuration")]
    MissingConfigDir,
    #[error("failed to prepare runtime directory {path}: {source}")]
    CreateDir {
        path: PathBuf,
        source: std::io::Error,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeMode {
    Portable,
    Installed,
}

#[derive(Debug, Clone)]
pub struct AppRuntime {
    mode: RuntimeMode,
    /// Fixed installation directory. Logs, crash reports, instance locks and
    /// other machine-local state always stay here, even when `data_dir` points
    /// at a user-selected or synchronization-managed directory.
    local_dir: PathBuf,
    data_dir: PathBuf,
    config_dir: PathBuf,
    log_dir: PathBuf,
    cache_dir: PathBuf,
    portable_key_path: Option<PathBuf>,
    data_dir_custom: bool,
    data_dir_notice: Option<String>,
}

impl AppRuntime {
    pub fn resolve() -> Result<Self, RuntimeError> {
        if let Some(runtime) = Self::portable_from_current_exe() {
            return Ok(runtime);
        }

        let home = dirs::home_dir().ok_or(RuntimeError::MissingConfigDir)?;
        Ok(Self::installed_from(
            &home,
            dirs::cache_dir().as_deref(),
            AppFlavor::current(),
        ))
    }

    pub fn installed_config_dir(home: &Path, flavor: AppFlavor) -> PathBuf {
        home.join(Self::installed_directory_names(flavor, linux_compatibility_build()).0)
    }

    fn installed_from(home: &Path, cache: Option<&Path>, flavor: AppFlavor) -> Self {
        Self::installed_resolved(home, cache, flavor, linux_compatibility_build())
    }

    fn installed_directory_names(
        flavor: AppFlavor,
        compatibility: bool,
    ) -> (&'static str, &'static str) {
        match (flavor, compatibility) {
            (AppFlavor::Stable, true) => (".nyaterm-compat", "nyaterm-compat"),
            (AppFlavor::Preview, true) => (".nyaterm-compat-preview", "nyaterm-compat-preview"),
            (_, false) => (flavor.config_directory_name(), flavor.desktop_id()),
        }
    }

    /// Builds the fixed installation layout without consulting the bootstrap
    /// pointer. Used by tests and by callers that must remain pointer-agnostic.
    #[cfg(test)]
    fn installed_for_package(
        home: &Path,
        cache: Option<&Path>,
        flavor: AppFlavor,
        compatibility: bool,
    ) -> Self {
        let (config_name, cache_name) = Self::installed_directory_names(flavor, compatibility);
        let local_dir = home.join(config_name);
        let data_dir = local_dir.clone();
        Self {
            mode: RuntimeMode::Installed,
            log_dir: local_dir.join("logs"),
            cache_dir: cache
                .map(Path::to_path_buf)
                .unwrap_or_else(|| local_dir.join("cache"))
                .join(cache_name),
            local_dir: local_dir.clone(),
            data_dir: data_dir.clone(),
            config_dir: data_dir,
            portable_key_path: None,
            data_dir_custom: false,
            data_dir_notice: None,
        }
    }

    fn installed_resolved(
        home: &Path,
        cache: Option<&Path>,
        flavor: AppFlavor,
        compatibility: bool,
    ) -> Self {
        let (config_name, cache_name) = Self::installed_directory_names(flavor, compatibility);
        let local_dir = home.join(config_name);
        let default_dir = local_dir.clone();

        let override_dir = std::env::var_os(data_directory::DATA_DIR_ENV)
            .map(PathBuf::from)
            .filter(|path| path.is_absolute());
        let (data_dir, data_dir_custom, data_dir_notice) = match override_dir {
            Some(dir) => match data_directory::validate_usable_directory(&dir, flavor) {
                Ok(()) => (dir, true, None),
                Err(error) => (default_dir.clone(), false, Some(error.to_string())),
            },
            None => {
                let startup =
                    data_directory::resolve_startup_data_dir(&local_dir, &default_dir, flavor);
                (startup.data_dir, startup.custom, startup.notice)
            }
        };

        Self {
            mode: RuntimeMode::Installed,
            log_dir: local_dir.join("logs"),
            cache_dir: cache
                .map(Path::to_path_buf)
                .unwrap_or_else(|| local_dir.join("cache"))
                .join(cache_name),
            local_dir,
            config_dir: data_dir.clone(),
            data_dir,
            portable_key_path: None,
            data_dir_custom,
            data_dir_notice,
        }
    }

    pub fn ensure_directories(&self) -> Result<(), RuntimeError> {
        for path in [
            self.local_dir(),
            self.data_dir(),
            self.config_dir(),
            self.log_dir(),
            self.cache_dir(),
        ] {
            std::fs::create_dir_all(path).map_err(|source| RuntimeError::CreateDir {
                path: path.to_path_buf(),
                source,
            })?;
        }
        Ok(())
    }

    pub fn mode(&self) -> RuntimeMode {
        self.mode
    }

    /// Fixed installation directory used for machine-local runtime state.
    pub fn local_dir(&self) -> &Path {
        &self.local_dir
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// Whether `data_dir` was explicitly selected instead of the default.
    pub fn data_dir_custom(&self) -> bool {
        self.data_dir_custom
    }

    /// Non-fatal problem detected while resolving the selected data directory.
    pub fn data_dir_notice(&self) -> Option<&str> {
        self.data_dir_notice.as_deref()
    }

    pub fn config_dir(&self) -> &Path {
        &self.config_dir
    }

    pub fn log_dir(&self) -> &Path {
        &self.log_dir
    }

    pub fn crash_dir(&self) -> PathBuf {
        self.local_dir.join("crashes")
    }

    pub fn instance_dir(&self) -> PathBuf {
        if self.data_dir_custom {
            data_directory::instance_lock_dir(&self.local_dir, &self.data_dir)
        } else {
            self.data_dir.join(data_directory::INSTANCE_DIR_NAME)
        }
    }

    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    pub fn portable_key_path(&self) -> Option<&Path> {
        self.portable_key_path.as_deref()
    }

    fn portable_from_current_exe() -> Option<Self> {
        let exe_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
        Self::portable_from_directory(&exe_dir)
    }

    fn portable_from_directory(exe_dir: &Path) -> Option<Self> {
        let marker = exe_dir.join("nyaterm-portable");
        if !marker.exists() {
            return None;
        }

        let data_dir = exe_dir.join("data");
        Some(Self {
            mode: RuntimeMode::Portable,
            config_dir: data_dir.join("config"),
            log_dir: data_dir.join("logs"),
            cache_dir: data_dir.join("cache"),
            portable_key_path: Some(data_dir.join("config").join("portable.key")),
            local_dir: data_dir.clone(),
            data_dir,
            data_dir_custom: false,
            data_dir_notice: None,
        })
    }

    #[doc(hidden)]
    pub fn from_parts_for_test(
        mode: RuntimeMode,
        data_dir: PathBuf,
        config_dir: PathBuf,
        log_dir: PathBuf,
        cache_dir: PathBuf,
        portable_key_path: Option<PathBuf>,
    ) -> Self {
        Self {
            mode,
            local_dir: data_dir.clone(),
            data_dir,
            config_dir,
            log_dir,
            cache_dir,
            portable_key_path,
            data_dir_custom: false,
            data_dir_notice: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::app_identity::AppFlavor;
    use crate::runtime::{AppRuntime, RuntimeMode};
    use std::path::Path;

    #[test]
    fn installed_flavors_isolate_all_runtime_and_instance_paths() {
        let home = Path::new("home");
        let stable = AppRuntime::installed_for_package(
            home,
            Some(Path::new("cache")),
            AppFlavor::Stable,
            false,
        );
        let preview = AppRuntime::installed_for_package(
            home,
            Some(Path::new("cache")),
            AppFlavor::Preview,
            false,
        );
        assert_eq!(stable.config_dir(), home.join(".nyaterm"));
        assert_eq!(preview.config_dir(), home.join(".nyaterm-preview"));
        assert_eq!(stable.cache_dir(), Path::new("cache/nyaterm"));
        assert_eq!(preview.cache_dir(), Path::new("cache/nyaterm-preview"));
        assert_ne!(stable.data_dir(), preview.data_dir());
        assert_ne!(stable.log_dir(), preview.log_dir());
        assert_eq!(stable.crash_dir(), home.join(".nyaterm/crashes"));
        assert_eq!(preview.crash_dir(), home.join(".nyaterm-preview/crashes"));
        assert_ne!(
            stable.config_dir().join(".instance/owner.lock"),
            preview.config_dir().join(".instance/owner.lock")
        );
        let stable = AppRuntime::installed_for_package(home, None, AppFlavor::Stable, false);
        let preview = AppRuntime::installed_for_package(home, None, AppFlavor::Preview, false);
        assert_ne!(stable.cache_dir(), preview.cache_dir());
    }

    #[test]
    fn compatibility_packages_isolate_data_cache_and_instance_locks_without_migrating_data() {
        let directory = crate::test_support::TestTempDir::new("nyaterm-compat-runtime");
        let home = directory.path();
        let cache_root = home.join("cache-root");
        let mut paths = std::collections::HashSet::new();
        for flavor in [AppFlavor::Stable, AppFlavor::Preview] {
            let standard = AppRuntime::installed_for_package(home, None, flavor, false);
            standard.ensure_directories().unwrap();
            let existing = standard.config_dir().join("existing-data");
            std::fs::write(&existing, b"unchanged").unwrap();
            for cache in [None, Some(cache_root.as_path())] {
                let compat = AppRuntime::installed_for_package(home, cache, flavor, true);
                let standard_cache = AppRuntime::installed_for_package(home, cache, flavor, false);
                let expected = match flavor {
                    AppFlavor::Stable => ".nyaterm-compat",
                    AppFlavor::Preview => ".nyaterm-compat-preview",
                };
                assert_eq!(compat.config_dir(), home.join(expected));
                assert_ne!(compat.config_dir(), standard.config_dir());
                assert_ne!(compat.cache_dir(), standard_cache.cache_dir());
                assert_ne!(compat.log_dir(), standard.log_dir());
                assert_ne!(compat.crash_dir(), standard.crash_dir());
                assert_ne!(
                    compat.config_dir().join(".instance/owner.lock"),
                    standard.config_dir().join(".instance/owner.lock")
                );
                compat.ensure_directories().unwrap();
                assert!(!compat.config_dir().join("existing-data").exists());
                assert_eq!(std::fs::read(&existing).unwrap(), b"unchanged");
            }
            assert!(
                paths.insert(
                    AppRuntime::installed_for_package(home, None, flavor, true)
                        .config_dir()
                        .to_path_buf()
                )
            );
        }
    }

    #[test]
    fn installed_discovery_path_uses_the_compiled_package_namespace() {
        let expected = if option_env!("NYATERM_LINUX_COMPAT") == Some("glibc231") {
            [".nyaterm-compat", ".nyaterm-compat-preview"]
        } else {
            [".nyaterm", ".nyaterm-preview"]
        };
        for (flavor, name) in [AppFlavor::Stable, AppFlavor::Preview]
            .into_iter()
            .zip(expected)
        {
            let runtime = AppRuntime::installed_from(Path::new("home"), None, flavor);
            assert_eq!(runtime.config_dir(), Path::new("home").join(name));
            assert_eq!(
                runtime.config_dir(),
                AppRuntime::installed_config_dir(Path::new("home"), flavor)
            );
        }
    }

    #[test]
    fn portable_runtime_remains_directory_local() {
        let directory = crate::test_support::TestTempDir::new("nyaterm-runtime");
        std::fs::create_dir_all(directory.path()).unwrap();
        assert!(AppRuntime::portable_from_directory(&directory).is_none());
        std::fs::write(directory.join("nyaterm-portable"), b"").unwrap();
        let runtime = AppRuntime::portable_from_directory(&directory).unwrap();
        assert_eq!(runtime.mode(), RuntimeMode::Portable);
        assert_eq!(runtime.config_dir(), directory.join("data/config"));
        assert_eq!(runtime.log_dir(), directory.join("data/logs"));
        assert_eq!(runtime.crash_dir(), directory.join("data/crashes"));
        assert_eq!(runtime.cache_dir(), directory.join("data/cache"));
        assert_eq!(
            runtime.portable_key_path(),
            Some(directory.join("data/config/portable.key").as_path())
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_selected_data_directory_keeps_local_state_in_the_installation_directory() {
        let directory = crate::test_support::TestTempDir::new("nyaterm-custom-data");
        let home = directory.path().join("home");
        let custom = directory.path().join("synced");
        std::fs::create_dir_all(&custom).unwrap();
        crate::data_directory::set_active_data_dir(&home.join(".nyaterm"), Some(&custom)).unwrap();

        let runtime = AppRuntime::installed_resolved(&home, None, AppFlavor::Stable, false);
        assert_eq!(runtime.data_dir(), custom);
        assert!(runtime.data_dir_custom());
        assert_eq!(runtime.config_dir(), custom);
        assert_eq!(runtime.log_dir(), home.join(".nyaterm/logs"));
        assert_eq!(runtime.crash_dir(), home.join(".nyaterm/crashes"));
        assert_eq!(
            runtime.instance_dir(),
            crate::data_directory::instance_lock_dir(&home.join(".nyaterm"), &custom)
        );
        assert!(!runtime.instance_dir().starts_with(&custom));
    }

    #[test]
    fn the_default_directory_keeps_the_existing_instance_layout() {
        let runtime =
            AppRuntime::installed_for_package(Path::new("home"), None, AppFlavor::Stable, false);
        assert!(!runtime.data_dir_custom());
        assert_eq!(
            runtime.instance_dir(),
            Path::new("home/.nyaterm").join(".instance")
        );
        assert_eq!(runtime.log_dir(), Path::new("home/.nyaterm/logs"));
    }

    #[test]
    fn an_unavailable_crash_directory_does_not_block_runtime_preparation() {
        let directory = crate::test_support::TestTempDir::new("nyaterm-runtime-crashes");
        std::fs::create_dir_all(directory.path()).unwrap();
        std::fs::write(directory.join("nyaterm-portable"), b"").unwrap();
        let runtime = AppRuntime::portable_from_directory(&directory).unwrap();
        std::fs::create_dir_all(runtime.data_dir()).unwrap();
        std::fs::write(runtime.crash_dir(), b"blocked").unwrap();
        runtime.ensure_directories().unwrap();
        assert!(runtime.crash_dir().is_file());
    }
}
