//! Blocking document I/O. Paths, source contents and tickets never enter logs.
use std::fs::{File, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use nyaterm_core::spice::virt_viewer::{MAX_VIRT_VIEWER_BYTES, VirtViewerFile, parse_virt_viewer};
use zeroize::Zeroizing;

pub(super) struct ImportedSpiceFile {
    pub document: VirtViewerFile,
    path: PathBuf,
    identity: same_file::Handle,
    contents: Zeroizing<Vec<u8>>,
}

impl ImportedSpiceFile {
    pub const fn source_deletion_supported() -> bool {
        cfg!(windows)
    }

    pub fn read(path: PathBuf) -> Result<Self, &'static str> {
        let file = open_regular(&path, false).map_err(|_| "read")?;
        let contents = read_bounded(&file).map_err(|_| "read")?;
        let document = parse_virt_viewer(&contents).map_err(|_| "parse")?;
        let identity = same_file::Handle::from_file(file).map_err(|_| "read")?;
        Ok(Self {
            document,
            path,
            identity,
            contents,
        })
    }

    /// Only called after the user chooses deletion in the import dialog. A
    /// replaced or modified document is retained, even if it has the same name.
    pub fn delete_unchanged(&self) -> io::Result<()> {
        let file = open_regular(&self.path, true)?;
        let identity = same_file::Handle::from_file(file.try_clone()?)?;
        if identity != self.identity || read_bounded(&file)?.as_slice() != self.contents.as_slice()
        {
            return Err(io::Error::other("connection document changed"));
        }
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle as _;
            use windows_sys::Win32::Storage::FileSystem::{
                FILE_DISPOSITION_INFO, FileDispositionInfo, SetFileInformationByHandle,
            };
            let info = FILE_DISPOSITION_INFO { DeleteFile: true };
            // The verified handle denies concurrent writes/rename. Deleting by
            // handle cannot target a replacement at the original pathname.
            if unsafe {
                SetFileInformationByHandle(
                    file.as_raw_handle(),
                    FileDispositionInfo,
                    &info as *const _ as *const _,
                    std::mem::size_of_val(&info) as u32,
                )
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }
        #[cfg(not(windows))]
        {
            // POSIX unlink is pathname-based: a second identity check cannot
            // stop an editor replacing that name before unlink. Preserve the
            // source instead of ever deleting a replacement document.
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "remove the connection document manually after import",
            ))
        }
    }
}

fn open_regular(path: &Path, deleting: bool) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt as _;
        use windows_sys::Win32::Storage::FileSystem::{
            DELETE, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
        };
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        if deleting {
            options
                .access_mode(0x8000_0000 | DELETE)
                .share_mode(FILE_SHARE_READ);
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        let _ = deleting;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() || std::fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(io::Error::other(
            "connection document is not a regular file",
        ));
    }
    Ok(file)
}

fn read_bounded(file: &File) -> io::Result<Zeroizing<Vec<u8>>> {
    if file.metadata()?.len() > MAX_VIRT_VIEWER_BYTES as u64 {
        return Err(io::Error::other("connection document too large"));
    }
    let mut bytes = Zeroizing::new(Vec::new());
    file.take(MAX_VIRT_VIEWER_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_VIRT_VIEWER_BYTES {
        return Err(io::Error::other("connection document too large"));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::ImportedSpiceFile;
    const DOCUMENT: &str = "[virt-viewer]\ntype=spice\nhost=localhost\nport=5900\npassword=private-ticket\ndelete-this-file=1\n";

    #[test]
    fn spice_file_deletion_requires_explicit_call_and_preserves_modified_source() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("console.vv");
        std::fs::write(&path, DOCUMENT).unwrap();
        let imported = ImportedSpiceFile::read(path.clone()).unwrap();
        assert!(path.exists());
        std::fs::write(&path, "replacement").unwrap();
        assert!(imported.delete_unchanged().is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "replacement");
    }

    #[test]
    #[cfg(windows)]
    fn spice_file_explicit_deletion_removes_only_the_opened_document() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("console.vv");
        std::fs::write(&path, DOCUMENT).unwrap();
        let imported = ImportedSpiceFile::read(path.clone()).unwrap();
        imported.delete_unchanged().unwrap();
        drop(imported);
        assert!(!path.exists());
    }

    #[test]
    #[cfg(unix)]
    fn spice_file_unix_import_retains_source_for_manual_removal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("console.vv");
        std::fs::write(&path, DOCUMENT).unwrap();
        let imported = ImportedSpiceFile::read(path.clone()).unwrap();
        assert!(!ImportedSpiceFile::source_deletion_supported());
        assert_eq!(
            imported.delete_unchanged().unwrap_err().kind(),
            std::io::ErrorKind::Unsupported
        );
        assert_eq!(std::fs::read_to_string(path).unwrap(), DOCUMENT);
    }

    #[test]
    fn spice_file_deletion_rejects_an_identical_replacement_with_a_new_identity() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("console.vv");
        std::fs::write(&path, DOCUMENT).unwrap();
        let imported = ImportedSpiceFile::read(path.clone()).unwrap();
        std::fs::rename(&path, dir.path().join("original.vv")).unwrap();
        std::fs::write(&path, DOCUMENT).unwrap();
        assert!(imported.delete_unchanged().is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), DOCUMENT);
    }
}
