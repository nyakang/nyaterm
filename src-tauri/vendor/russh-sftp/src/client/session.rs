use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use super::{
    error::Error,
    fs::{File, Metadata, ReadDir},
    rawsession::{Limits, SftpResult},
    RawSftpSession,
};
use crate::{
    client::Config,
    extensions::{self, Statvfs},
    protocol::{FileAttributes, OpenFlags, StatusCode},
};

#[derive(Debug, Clone, Copy)]
pub(crate) struct Features {
    pub hardlink: bool,
    pub fsync: bool,
    pub statvfs: bool,
    pub limits: Option<Limits>,
    pub max_concurrent_writes: usize,
    pub max_packet_len: u32,
}

/// High-level SFTP implementation for easy interaction with a remote file system.
/// Contains most methods similar to the native [filesystem](std::fs)
pub struct SftpSession {
    session: Arc<RawSftpSession>,
    features: Features,
}

impl SftpSession {
    /// Creates a new session by initializing the protocol and extensions
    pub async fn new<S>(stream: S) -> SftpResult<Self>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        Self::new_with_config(stream, Config::default()).await
    }

    /// Creates a new session with custom configuration
    pub async fn new_with_config<S>(stream: S, cfg: Config) -> SftpResult<Self>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let max_concurrent_writes = cfg.max_concurrent_writes;
        let max_packet_len = cfg.max_packet_len;
        let mut session = RawSftpSession::new_with_config(stream, cfg);

        let version = session.init().await?;
        let has_extension = |name, ver| version.extensions.get(name).is_some_and(|v| v == ver);

        let mut features = Features {
            hardlink: has_extension(extensions::HARDLINK, "1"),
            fsync: has_extension(extensions::FSYNC, "1"),
            statvfs: has_extension(extensions::STATVFS, "2"),
            limits: None,
            max_concurrent_writes,
            max_packet_len,
        };

        if has_extension(extensions::LIMITS, "1") {
            let limits = Limits::from(session.limits().await?);
            session.set_limits(limits);
            features.limits = Some(limits);
            if let Some(plen) = limits.packet_len {
                features.max_packet_len = (plen as u32).min(max_packet_len);
            }
        }

        Ok(Self {
            session: Arc::new(session),
            features,
        })
    }

    /// Set the maximum response time in seconds.
    /// Default: 10 seconds
    pub fn set_timeout(&self, secs: u64) {
        self.session.set_timeout(secs);
    }

    /// Returns limits advertised by the server via the `limits@openssh.com`
    /// extension, when available.
    pub fn limits(&self) -> Option<Limits> {
        self.features.limits
    }

    /// Returns the effective maximum packet length after applying server limits.
    pub fn effective_max_packet_len(&self) -> u32 {
        self.features.max_packet_len
    }

    /// Returns the maximum number of open handles advertised by the server.
    pub fn max_open_handles(&self) -> Option<u64> {
        self.features.limits.and_then(|limits| limits.open_handles)
    }

    /// Closes the inner channel stream.
    pub async fn close(&self) -> SftpResult<()> {
        self.session.close_session()
    }

    /// Attempts to open a file in read-only mode.
    pub async fn open<T: Into<String>>(&self, filename: T) -> SftpResult<File> {
        self.open_with_flags(filename, OpenFlags::READ).await
    }

    /// Opens a file using raw bytes for the filename (preserves original encoding).
    pub async fn open_bytes(&self, filename_bytes: Vec<u8>) -> SftpResult<File> {
        self.open_with_flags_bytes(filename_bytes, OpenFlags::READ)
            .await
    }

    /// Opens a file in write-only mode.
    ///
    /// This function will create a file if it does not exist, and will truncate it if it does.
    pub async fn create<T: Into<String>>(&self, filename: T) -> SftpResult<File> {
        self.open_with_flags(
            filename,
            OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE,
        )
        .await
    }

    /// Attempts to open or create the file in the specified mode
    pub async fn open_with_flags<T: Into<String>>(
        &self,
        filename: T,
        flags: OpenFlags,
    ) -> SftpResult<File> {
        self.open_with_flags_and_attributes(filename, flags, FileAttributes::empty())
            .await
    }

    /// Attempts to open or create the file using raw bytes (preserves original encoding).
    pub async fn open_with_flags_bytes(
        &self,
        filename_bytes: Vec<u8>,
        flags: OpenFlags,
    ) -> SftpResult<File> {
        self.open_with_flags_and_attributes_bytes(filename_bytes, flags, FileAttributes::empty())
            .await
    }

    /// Attempts to open or create the file in the specified mode and with specified file attributes
    pub async fn open_with_flags_and_attributes<T: Into<String>>(
        &self,
        filename: T,
        flags: OpenFlags,
        attributes: FileAttributes,
    ) -> SftpResult<File> {
        let handle = self.session.open(filename, flags, attributes).await?.handle;
        Ok(File::new(self.session.clone(), handle, self.features))
    }

    /// Attempts to open or create the file using raw bytes (preserves original encoding).
    pub async fn open_with_flags_and_attributes_bytes(
        &self,
        filename_bytes: Vec<u8>,
        flags: OpenFlags,
        attributes: FileAttributes,
    ) -> SftpResult<File> {
        let handle = self
            .session
            .open_bytes(filename_bytes, flags, attributes)
            .await?
            .handle;
        Ok(File::new(self.session.clone(), handle, self.features))
    }

    /// Requests the remote party for the absolute from the relative path.
    pub async fn canonicalize<T: Into<String>>(&self, path: T) -> SftpResult<String> {
        let name = self.session.realpath(path).await?;
        match name.files.first() {
            Some(file) => Ok(file.filename.to_owned()),
            None => Err(Error::UnexpectedBehavior("no file".to_owned())),
        }
    }

    /// Creates a new empty directory.
    pub async fn create_dir<T: Into<String>>(&self, path: T) -> SftpResult<()> {
        self.session
            .mkdir(path, FileAttributes::empty())
            .await
            .map(|_| ())
    }

    /// Creates a new empty directory using raw bytes for the path.
    pub async fn create_dir_bytes(&self, path_bytes: Vec<u8>) -> SftpResult<()> {
        self.session
            .mkdir_bytes(path_bytes, FileAttributes::empty())
            .await
            .map(|_| ())
    }

    /// Reads the contents of a file located at the specified path to the end.
    pub async fn read<P: Into<String>>(&self, path: P) -> SftpResult<Vec<u8>> {
        let mut file = self.open(path).await?;
        let mut buffer = Vec::new();

        file.read_to_end(&mut buffer).await?;
        file.shutdown().await?;

        Ok(buffer)
    }

    /// Writes the contents to a file whose path is specified.
    pub async fn write<P: Into<String>>(&self, path: P, data: &[u8]) -> SftpResult<()> {
        let mut file = self.open_with_flags(path, OpenFlags::WRITE).await?;
        file.write_all(data).await?;
        file.flush().await?;
        file.shutdown().await?;
        Ok(())
    }

    /// Checks a file or folder exists at the specified path
    pub async fn try_exists<P: Into<String>>(&self, path: P) -> SftpResult<bool> {
        match self.metadata(path).await {
            Ok(_) => Ok(true),
            Err(Error::Status(status)) if status.status_code == StatusCode::NoSuchFile => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// Returns an iterator over the entries within a directory.
    pub async fn read_dir<P: Into<String>>(&self, path: P) -> SftpResult<ReadDir> {
        let path: String = path.into();
        let parent = Arc::from(path.as_str());

        let handle = self.session.opendir(path).await?.handle;
        let mut files = vec![];

        loop {
            match self.session.readdir(handle.as_str()).await {
                Ok(name) => {
                    files = name
                        .files
                        .into_iter()
                        .map(|f| {
                            // Use filename_bytes if available, otherwise fall back to encoding filename as bytes
                            let bytes = if f.filename_bytes.is_empty() {
                                f.filename.as_bytes().to_vec()
                            } else {
                                f.filename_bytes
                            };
                            (bytes, f.filename, f.attrs)
                        })
                        .chain(files)
                        .collect();
                }
                Err(Error::Status(status)) if status.status_code == StatusCode::Eof => break,
                Err(err) => return Err(err),
            }
        }

        self.session.close(handle).await?;

        Ok(ReadDir {
            parent,
            entries: files.into(),
        })
    }

    /// Returns an iterator over the entries within a directory using raw bytes for the path.
    pub async fn read_dir_bytes(&self, path_bytes: Vec<u8>) -> SftpResult<ReadDir> {
        let path_str = String::from_utf8_lossy(&path_bytes).into_owned();
        let parent = Arc::from(path_str.as_str());

        let handle = self.session.opendir_bytes(path_bytes).await?.handle;
        let mut files = vec![];

        loop {
            match self.session.readdir(handle.as_str()).await {
                Ok(name) => {
                    files = name
                        .files
                        .into_iter()
                        .map(|f| {
                            // Use filename_bytes if available, otherwise fall back to encoding filename as bytes
                            let bytes = if f.filename_bytes.is_empty() {
                                f.filename.as_bytes().to_vec()
                            } else {
                                f.filename_bytes
                            };
                            (bytes, f.filename, f.attrs)
                        })
                        .chain(files)
                        .collect();
                }
                Err(Error::Status(status)) if status.status_code == StatusCode::Eof => break,
                Err(err) => return Err(err),
            }
        }

        self.session.close(handle).await?;

        Ok(ReadDir {
            parent,
            entries: files.into(),
        })
    }

    /// Reads a symbolic link, returning the file that the link points to.
    pub async fn read_link<P: Into<String>>(&self, path: P) -> SftpResult<String> {
        let name = self.session.readlink(path).await?;
        match name.files.first() {
            Some(file) => Ok(file.filename.to_owned()),
            None => Err(Error::UnexpectedBehavior("no file".to_owned())),
        }
    }

    /// Reads a symbolic link using raw bytes for the link path and target.
    pub async fn read_link_bytes(&self, path_bytes: Vec<u8>) -> SftpResult<Vec<u8>> {
        let name = self.session.readlink_bytes(path_bytes).await?;
        match name.files.first() {
            Some(file) => Ok(file.filename_bytes.clone()),
            None => Err(Error::UnexpectedBehavior("no file".to_owned())),
        }
    }

    /// Removes the specified folder.
    pub async fn remove_dir<P: Into<String>>(&self, path: P) -> SftpResult<()> {
        self.session.rmdir(path).await.map(|_| ())
    }

    /// Removes the specified folder using raw bytes for the path.
    pub async fn remove_dir_bytes(&self, path_bytes: Vec<u8>) -> SftpResult<()> {
        self.session.rmdir_bytes(path_bytes).await.map(|_| ())
    }

    /// Removes the specified file.
    pub async fn remove_file<T: Into<String>>(&self, filename: T) -> SftpResult<()> {
        self.session.remove(filename).await.map(|_| ())
    }

    /// Removes a file using raw bytes for the filename (preserves original encoding).
    pub async fn remove_file_bytes(&self, filename_bytes: Vec<u8>) -> SftpResult<()> {
        self.session.remove_bytes(filename_bytes).await.map(|_| ())
    }

    /// Rename a file or directory to a new name.
    pub async fn rename<O, N>(&self, oldpath: O, newpath: N) -> SftpResult<()>
    where
        O: Into<String>,
        N: Into<String>,
    {
        self.session.rename(oldpath, newpath).await.map(|_| ())
    }

    /// Rename a file or directory using raw bytes for both paths.
    pub async fn rename_bytes(
        &self,
        oldpath_bytes: Vec<u8>,
        newpath_bytes: Vec<u8>,
    ) -> SftpResult<()> {
        self.session
            .rename_bytes(oldpath_bytes, newpath_bytes)
            .await
            .map(|_| ())
    }

    /// Creates a symlink of the specified target.
    pub async fn symlink<P, T>(&self, path: P, target: T) -> SftpResult<()>
    where
        P: Into<String>,
        T: Into<String>,
    {
        self.session.symlink(path, target).await.map(|_| ())
    }

    /// Creates a symlink using OpenSSH SFTP argument ordering.
    ///
    /// OpenSSH's `SSH_FXP_SYMLINK` implementation expects `(target, link)`,
    /// which is the reverse of the order documented by the old draft protocol.
    pub async fn symlink_openssh<T, L>(&self, target: T, link: L) -> SftpResult<()>
    where
        T: Into<String>,
        L: Into<String>,
    {
        self.session.symlink_openssh(target, link).await.map(|_| ())
    }

    /// Creates an OpenSSH-compatible symlink using raw bytes for both paths.
    pub async fn symlink_openssh_bytes(
        &self,
        target_bytes: Vec<u8>,
        link_bytes: Vec<u8>,
    ) -> SftpResult<()> {
        self.session
            .symlink_openssh_bytes(target_bytes, link_bytes)
            .await
            .map(|_| ())
    }

    /// Queries metadata about the remote file.
    pub async fn metadata<P: Into<String>>(&self, path: P) -> SftpResult<Metadata> {
        Ok(self.session.stat(path).await?.attrs)
    }

    /// Queries metadata about the remote file using raw bytes (preserves original encoding).
    pub async fn metadata_bytes(&self, path_bytes: Vec<u8>) -> SftpResult<Metadata> {
        Ok(self.session.stat_bytes(path_bytes).await?.attrs)
    }

    /// Sets metadata for a remote file.
    pub async fn set_metadata<P: Into<String>>(
        &self,
        path: P,
        metadata: Metadata,
    ) -> Result<(), Error> {
        self.session.setstat(path, metadata).await.map(|_| ())
    }

    /// Sets metadata for a remote file using raw bytes for the path.
    pub async fn set_metadata_bytes(
        &self,
        path_bytes: Vec<u8>,
        metadata: Metadata,
    ) -> Result<(), Error> {
        self.session
            .setstat_bytes(path_bytes, metadata)
            .await
            .map(|_| ())
    }

    pub async fn symlink_metadata<P: Into<String>>(&self, path: P) -> SftpResult<Metadata> {
        Ok(self.session.lstat(path).await?.attrs)
    }

    pub async fn symlink_metadata_bytes(&self, path_bytes: Vec<u8>) -> SftpResult<Metadata> {
        Ok(self.session.lstat_bytes(path_bytes).await?.attrs)
    }

    pub async fn hardlink<O, N>(&self, oldpath: O, newpath: N) -> SftpResult<bool>
    where
        O: Into<String>,
        N: Into<String>,
    {
        if !self.features.hardlink {
            return Ok(false);
        }

        self.session.hardlink(oldpath, newpath).await.map(|_| true)
    }

    /// Performs a statvfs on the remote file system path.
    /// Returns [`Ok(None)`] if the remote SFTP server does not support `statvfs@openssh.com` extension v2.
    pub async fn fs_info<P: Into<String>>(&self, path: P) -> SftpResult<Option<Statvfs>> {
        if !self.features.statvfs {
            return Ok(None);
        }

        self.session.statvfs(path).await.map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    impl SftpSession {
        fn for_test_with_limits(limits: Option<Limits>, max_packet_len: u32) -> Self {
            let stream = tokio::io::duplex(64).0;
            Self {
                session: Arc::new(RawSftpSession::new(stream)),
                features: Features {
                    hardlink: false,
                    fsync: false,
                    statvfs: false,
                    limits,
                    max_concurrent_writes: 8,
                    max_packet_len,
                },
            }
        }
    }

    #[tokio::test]
    async fn exposes_server_limits_and_effective_packet_len() {
        let limits = Limits {
            packet_len: Some(65_536),
            read_len: Some(32_768),
            write_len: Some(32_768),
            open_handles: Some(128),
        };
        let session = SftpSession::for_test_with_limits(Some(limits), 65_536);

        assert_eq!(session.limits(), Some(limits));
        assert_eq!(session.effective_max_packet_len(), 65_536);
        assert_eq!(session.max_open_handles(), Some(128));
    }

    #[tokio::test]
    async fn max_open_handles_is_none_without_limits_extension() {
        let session = SftpSession::for_test_with_limits(None, 262_144);

        assert_eq!(session.limits(), None);
        assert_eq!(session.effective_max_packet_len(), 262_144);
        assert_eq!(session.max_open_handles(), None);
    }
}
