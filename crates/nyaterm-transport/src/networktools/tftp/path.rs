//! Filesystem boundary for the TFTP root.
//!
//! Every request name is reduced to normal components, joined under the
//! canonical root, and re-canonicalized before use. Anything that resolves
//! outside the root -- including symlinks and Windows prefixes -- is rejected.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use tempfile::NamedTempFile;

#[derive(Debug)]
pub enum TftpPathError {
    InvalidName,
    OutsideRoot,
    NotFound,
    AlreadyExists,
    IsDirectory,
    Io(io::Error),
}

impl fmt::Display for TftpPathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidName => f.write_str("invalid file name"),
            Self::OutsideRoot => f.write_str("path escapes the shared directory"),
            Self::NotFound => f.write_str("file not found"),
            Self::AlreadyExists => f.write_str("file already exists"),
            Self::IsDirectory => f.write_str("target is a directory"),
            Self::Io(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for TftpPathError {}

impl From<io::Error> for TftpPathError {
    fn from(error: io::Error) -> Self {
        match error.kind() {
            io::ErrorKind::NotFound => Self::NotFound,
            io::ErrorKind::AlreadyExists => Self::AlreadyExists,
            _ => Self::Io(error),
        }
    }
}

/// Canonicalize the configured root once when the server starts.
pub(crate) fn canonical_root(root: &Path) -> io::Result<PathBuf> {
    let canonical = fs::canonicalize(root)?;
    if !canonical.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotADirectory,
            "TFTP root is not a directory",
        ));
    }
    Ok(canonical)
}

fn relative_path(name: &str) -> Result<PathBuf, TftpPathError> {
    // Apply the same wire-name policy on every platform, including Windows
    // drive-relative names, UNC paths and alternate data streams.
    if name.is_empty() || name.contains(['\0', '\\', ':']) {
        return Err(TftpPathError::InvalidName);
    }
    let mut relative = PathBuf::new();
    for component in Path::new(name).components() {
        match component {
            Component::Normal(part) => relative.push(part),
            _ => return Err(TftpPathError::InvalidName),
        }
    }
    if relative.as_os_str().is_empty() {
        return Err(TftpPathError::InvalidName);
    }
    Ok(relative)
}

/// Resolve an RRQ name to an existing file under `root`.
pub(crate) fn read_path(root: &Path, name: &str) -> Result<PathBuf, TftpPathError> {
    let candidate = root.join(relative_path(name)?);
    let canonical = fs::canonicalize(&candidate).map_err(TftpPathError::from)?;
    if !canonical.starts_with(root) {
        return Err(TftpPathError::OutsideRoot);
    }
    if !canonical.is_file() {
        return Err(TftpPathError::IsDirectory);
    }
    Ok(canonical)
}

/// A pending upload: bytes go to a temporary file next to the target and are
/// committed before the final ACK. The temporary file is removed on every
/// uncommitted exit, including Windows where open handles prevent deletion.
pub(crate) struct WriteTarget {
    pub path: PathBuf,
    pub temp: NamedTempFile,
    allow_overwrite: bool,
}

impl WriteTarget {
    pub fn commit(self) -> io::Result<()> {
        // Close the writer before renaming on Windows. TempPath retains RAII
        // cleanup if persistence fails. Noclobber checks at commit, not WRQ.
        let temp = self.temp.into_temp_path();
        let result = if self.allow_overwrite {
            temp.persist(&self.path)
        } else {
            temp.persist_noclobber(&self.path)
        };
        result.map_err(|error| error.error)
    }
}

/// Resolve a WRQ name, refusing overwrites unless explicitly allowed.
pub(crate) fn write_target(
    root: &Path,
    name: &str,
    allow_overwrite: bool,
) -> Result<WriteTarget, TftpPathError> {
    let relative = relative_path(name)?;
    let candidate = root.join(&relative);
    let parent = candidate.parent().ok_or(TftpPathError::InvalidName)?;
    let canonical_parent = fs::canonicalize(parent).map_err(TftpPathError::from)?;
    if !canonical_parent.starts_with(root) {
        return Err(TftpPathError::OutsideRoot);
    }
    let file_name = candidate
        .file_name()
        .ok_or(TftpPathError::InvalidName)?
        .to_owned();
    let path = canonical_parent.join(&file_name);
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_dir() => return Err(TftpPathError::IsDirectory),
        Ok(_) if !allow_overwrite => return Err(TftpPathError::AlreadyExists),
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let temp = tempfile::Builder::new()
        .prefix(".nyaterm-tftp-")
        .tempfile_in(&canonical_parent)?;
    Ok(WriteTarget {
        path,
        temp,
        allow_overwrite,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::{TftpPathError, read_path, write_target};

    fn root() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        (dir, root)
    }

    #[test]
    fn reads_nested_files_inside_the_root() {
        let (_dir, root) = root();
        fs::create_dir(root.join("firmware")).unwrap();
        fs::write(root.join("firmware/boot.bin"), b"data").unwrap();
        let path = read_path(&root, "firmware/boot.bin").unwrap();
        assert_eq!(path, root.join("firmware/boot.bin"));
    }

    #[test]
    fn rejects_traversal_absolute_and_windows_prefixed_names() {
        let (_dir, root) = root();
        for name in [
            "..",
            "../secret",
            "a/../../b",
            "/etc/passwd",
            ".",
            "",
            "C:\\Windows\\win.ini",
            "C:relative.txt",
            "C:/Windows/win.ini",
            "file.txt:stream",
            "\\\\server\\share\\file",
        ] {
            assert!(
                matches!(read_path(&root, name), Err(TftpPathError::InvalidName)),
                "{name} should be rejected"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_escape() {
        let (_dir, root) = root();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("secret.txt"), b"secret").unwrap();
        std::os::unix::fs::symlink(outside.path(), root.join("link")).unwrap();

        assert!(matches!(
            read_path(&root, "link/secret.txt"),
            Err(TftpPathError::OutsideRoot)
        ));
    }

    #[test]
    fn write_refuses_overwrite_by_default() {
        let (_dir, root) = root();
        fs::write(root.join("config.bak"), b"old").unwrap();
        assert!(matches!(
            write_target(&root, "config.bak", false),
            Err(TftpPathError::AlreadyExists)
        ));
        let target = write_target(&root, "config.bak", true).unwrap();
        fs::write(target.temp.path(), b"new").unwrap();
        target.commit().unwrap();
        assert_eq!(fs::read(root.join("config.bak")).unwrap(), b"new");
    }

    #[test]
    fn write_target_stays_inside_nested_directory() {
        let (_dir, root) = root();
        fs::create_dir(root.join("backups")).unwrap();
        let target = write_target(&root, "backups/router.cfg", false).unwrap();
        assert!(target.path.starts_with(&root));
        assert_eq!(target.path.file_name().unwrap(), "router.cfg");
    }

    #[test]
    fn dropping_an_uncommitted_upload_removes_its_temporary_file() {
        let (_dir, root) = root();
        let target = write_target(&root, "cancel.bin", false).unwrap();
        let temporary = target.temp.path().to_path_buf();
        assert!(temporary.exists());
        drop(target);
        assert!(!temporary.exists());
        assert!(!root.join("cancel.bin").exists());
    }

    #[test]
    fn commit_refuses_a_file_created_after_the_initial_check() {
        let (_dir, root) = root();
        let target = write_target(&root, "race.bin", false).unwrap();
        let temporary = target.temp.path().to_path_buf();
        fs::write(target.temp.path(), b"new").unwrap();
        fs::write(&target.path, b"existing").unwrap();
        assert_eq!(
            target.commit().unwrap_err().kind(),
            std::io::ErrorKind::AlreadyExists
        );
        assert_eq!(fs::read(root.join("race.bin")).unwrap(), b"existing");
        assert!(!temporary.exists());
    }
}
