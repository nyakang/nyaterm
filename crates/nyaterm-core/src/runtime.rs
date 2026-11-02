pub mod reaper;
pub mod worker;

use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::app_identity::AppFlavor;

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
    data_dir: PathBuf,
    config_dir: PathBuf,
    log_dir: PathBuf,
    cache_dir: PathBuf,
    portable_key_path: Option<PathBuf>,
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
        home.join(flavor.config_directory_name())
    }

    fn installed_from(home: &Path, cache: Option<&Path>, flavor: AppFlavor) -> Self {
        let config_dir = Self::installed_config_dir(home, flavor);
        let data_dir = config_dir.clone();
        let log_dir = data_dir.join("logs");
        let cache_dir = cache
            .map(Path::to_path_buf)
            .unwrap_or_else(|| data_dir.join("cache"))
            .join(flavor.desktop_id());

        Self {
            mode: RuntimeMode::Installed,
            data_dir,
            config_dir,
            log_dir,
            cache_dir,
            portable_key_path: None,
        }
    }

    pub fn ensure_directories(&self) -> Result<(), RuntimeError> {
        for path in [
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

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn config_dir(&self) -> &Path {
        &self.config_dir
    }

    pub fn log_dir(&self) -> &Path {
        &self.log_dir
    }

    pub fn crash_dir(&self) -> PathBuf {
        self.data_dir.join("crashes")
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
            data_dir,
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
            data_dir,
            config_dir,
            log_dir,
            cache_dir,
            portable_key_path,
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
        let stable = AppRuntime::installed_from(home, Some(Path::new("cache")), AppFlavor::Stable);
        let preview =
            AppRuntime::installed_from(home, Some(Path::new("cache")), AppFlavor::Preview);
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
        let stable = AppRuntime::installed_from(home, None, AppFlavor::Stable);
        let preview = AppRuntime::installed_from(home, None, AppFlavor::Preview);
        assert_ne!(stable.cache_dir(), preview.cache_dir());
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
