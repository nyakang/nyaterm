//! Metadata-only directory enumeration for latency-sensitive callers.
use crate::file_browser::FileBrowserService;
use crate::sftp::{SftpFileType, SftpTransferControl};
use std::time::Instant;

/// A completion request may follow only this many selected targets.
pub const MAX_DIRECTORY_TARGETS: usize = 12;

#[derive(Clone, Copy)]
pub struct DirectoryLimits {
    pub entries: usize,
    pub name_bytes: usize,
}

#[derive(Clone)]
pub struct DirectoryEntry {
    pub name: String,
    pub file_type: SftpFileType,
    pub size: Option<u64>,
}

pub struct DirectoryListing {
    pub entries: Vec<DirectoryEntry>,
    /// Only EOF proves completeness. A budget-limited result is still useful.
    pub complete: bool,
}

impl FileBrowserService {
    /// Follows only the caller-selected targets, never recursively enumerates them.
    pub fn probe_directory_targets(
        &self,
        paths: &[String],
        control: SftpTransferControl,
        deadline: Instant,
    ) -> anyhow::Result<Vec<Option<bool>>> {
        anyhow::ensure!(
            paths.len() <= MAX_DIRECTORY_TARGETS,
            "too many directory targets"
        );
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        match self {
            Self::Local(_) => {
                let mut result = Vec::with_capacity(paths.len());
                for path in paths {
                    if control.is_cancelled() || Instant::now() >= deadline {
                        break;
                    }
                    result.push(
                        std::fs::metadata(path)
                            .ok()
                            .map(|metadata| metadata.is_dir()),
                    );
                }
                result.resize(paths.len(), None);
                Ok(result)
            }
            Self::Remote(service) => service.probe_directory_targets(paths, control, deadline),
            Self::Adb(_) => anyhow::bail!("directory target probing is unavailable"),
        }
    }

    pub fn list_directory_lightweight(
        &self,
        path: &str,
        limits: DirectoryLimits,
        control: SftpTransferControl,
        deadline: Instant,
    ) -> anyhow::Result<DirectoryListing> {
        match self {
            Self::Local(_) => list_local(path, limits, &control, deadline),
            Self::Remote(service) => {
                service.list_directory_lightweight(path, limits, control, deadline)
            }
            Self::Adb(_) => anyhow::bail!("lightweight listing is unavailable"),
        }
    }
}

fn list_local(
    path: &str,
    limits: DirectoryLimits,
    control: &SftpTransferControl,
    deadline: Instant,
) -> anyhow::Result<DirectoryListing> {
    let mut result = DirectoryListing {
        entries: Vec::new(),
        complete: false,
    };
    if limits.entries == 0 || limits.name_bytes == 0 {
        return Ok(result);
    }
    control.check_cancelled()?;
    anyhow::ensure!(Instant::now() < deadline, "directory listing timed out");
    let mut entries = std::fs::read_dir(path)?;
    let mut bytes = 0;
    for _ in 0..limits.entries {
        control.check_cancelled()?;
        anyhow::ensure!(Instant::now() < deadline, "directory listing timed out");
        let Some(entry) = entries.next() else {
            result.complete = true;
            break;
        };
        let entry = entry?;
        // Lossy display names must never become shell input for another file.
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.len() > limits.name_bytes - bytes {
            break;
        }
        bytes += name.len();
        let kind = entry.file_type()?;
        result.entries.push(DirectoryEntry {
            name: name.into(),
            file_type: if kind.is_dir() {
                SftpFileType::Directory
            } else if kind.is_symlink() {
                SftpFileType::Symlink
            } else if kind.is_file() {
                SftpFileType::File
            } else {
                SftpFileType::Other
            },
            size: None,
        });
        if bytes == limits.name_bytes {
            break;
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::{DirectoryLimits, list_local};
    use crate::sftp::SftpTransferControl;
    use std::time::{Duration, Instant};
    #[test]
    fn enumeration_respects_entry_and_byte_budgets() {
        let dir = tempfile::tempdir().unwrap();
        for n in 0..100 {
            std::fs::write(dir.path().join(format!("{n:04}")), []).unwrap();
        }
        for (entries, name_bytes, expected) in [(3, 1000, 3), (100, 8, 2), (100, 3, 0)] {
            let listing = list_local(
                dir.path().to_str().unwrap(),
                DirectoryLimits {
                    entries,
                    name_bytes,
                },
                &SftpTransferControl::default(),
                Instant::now() + Duration::from_secs(2),
            )
            .unwrap();
            assert_eq!(listing.entries.len(), expected);
            assert!(!listing.complete);
        }
    }
    #[cfg(unix)]
    #[test]
    fn links_are_not_followed() {
        use crate::sftp::SftpFileType;

        let dir = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink("missing", dir.path().join("link")).unwrap();
        let listing = list_local(
            dir.path().to_str().unwrap(),
            DirectoryLimits {
                entries: 100,
                name_bytes: 1000,
            },
            &SftpTransferControl::default(),
            Instant::now() + Duration::from_secs(2),
        )
        .unwrap();
        assert!(listing.complete);
        assert_eq!(listing.entries.len(), 1);
        assert_eq!(listing.entries[0].file_type, SftpFileType::Symlink);
    }

    // APFS rejects these filenames before enumeration can inspect them.
    #[cfg(target_os = "linux")]
    #[test]
    fn non_utf8_names_are_not_inserted() {
        use std::os::unix::ffi::OsStrExt;
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(std::ffi::OsStr::from_bytes(b"bad\xff")), []).unwrap();
        std::fs::write(dir.path().join("valid"), []).unwrap();
        let listing = list_local(
            dir.path().to_str().unwrap(),
            DirectoryLimits {
                entries: 100,
                name_bytes: 1000,
            },
            &SftpTransferControl::default(),
            Instant::now() + Duration::from_secs(2),
        )
        .unwrap();
        assert!(listing.complete);
        assert_eq!(listing.entries.len(), 1);
        assert_eq!(listing.entries[0].name, "valid");
    }
    #[cfg(unix)]
    #[test]
    fn selected_link_targets_distinguish_directories_files_and_broken_loops() {
        use crate::file_browser::FileBrowserService;
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("directory")).unwrap();
        std::fs::write(dir.path().join("file"), []).unwrap();
        for (name, target) in [
            ("dir-link", "directory"),
            ("file-link", "file"),
            ("broken", "missing"),
            ("loop", "loop"),
        ] {
            std::os::unix::fs::symlink(target, dir.path().join(name)).unwrap();
        }
        let paths: Vec<_> = ["dir-link", "file-link", "broken", "loop"]
            .iter()
            .map(|name| dir.path().join(name).to_str().unwrap().to_string())
            .collect();
        let service = FileBrowserService::local();
        let result = service
            .probe_directory_targets(
                &paths,
                SftpTransferControl::default(),
                Instant::now() + Duration::from_secs(2),
            )
            .unwrap();
        assert_eq!(result, vec![Some(true), Some(false), None, None]);
        let control = SftpTransferControl::default();
        control.cancel();
        assert_eq!(
            service
                .probe_directory_targets(&paths, control, Instant::now() + Duration::from_secs(2))
                .unwrap(),
            vec![None; 4]
        );
        assert_eq!(
            service
                .probe_directory_targets(&paths, SftpTransferControl::default(), Instant::now())
                .unwrap(),
            vec![None; 4]
        );
        assert!(
            service
                .probe_directory_targets(
                    &vec![paths[0].clone(); 13],
                    SftpTransferControl::default(),
                    Instant::now() + Duration::from_secs(2)
                )
                .is_err()
        );
    }
}
