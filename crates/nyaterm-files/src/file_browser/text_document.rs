use crate::file_browser::{FileBrowserService, resolve_local_path, system_time_secs};
use crate::remote_file::{RemoteTextMetadata, RemoteTextRevision, RemoteTextWriteResult};
use crate::sftp::RemoteFilePath;
use crate::text_document::{ByteDocument, EncodedTextDocument};
use nyaterm_core::file_encoding::{FileReadEncoding, FileTextFormat};

impl FileBrowserService {
    pub fn read_encoded_document_path(
        &self,
        path: &RemoteFilePath,
        max_bytes: u64,
        selection: FileReadEncoding,
    ) -> anyhow::Result<EncodedTextDocument> {
        self.read_document_bytes(path, max_bytes)?.decode(selection)
    }

    fn read_document_bytes(
        &self,
        path: &RemoteFilePath,
        max_bytes: u64,
    ) -> anyhow::Result<ByteDocument> {
        match self {
            Self::Remote(service) => service.read_document_bytes(path, max_bytes),
            Self::Adb(service) => service.read_document_bytes(path, max_bytes),
            Self::Local(service) => {
                let local_path = resolve_local_path(&path.display_path);
                let before = std::fs::metadata(&local_path)?;
                let file = service.read_file_bytes(&local_path, max_bytes)?;
                let after = std::fs::metadata(&local_path)?;
                anyhow::ensure!(
                    before.len() == after.len()
                        && before.modified()? == after.modified()?
                        && file.content_bytes.len() as u64 == after.len(),
                    "File changed while being read"
                );
                let revision = RemoteTextRevision::from_bytes(
                    &file.content_bytes,
                    RemoteTextMetadata {
                        size: file.size,
                        modified_at: Some(file.modified_at),
                    },
                );
                Ok(ByteDocument {
                    path: file.path,
                    bytes: file.content_bytes,
                    revision,
                })
            }
            Self::Ftp(service) => {
                let before = service.stat(&path.display_path)?;
                anyhow::ensure!(
                    !before.is_directory(),
                    "Directories cannot be opened as text"
                );
                let bytes = service.read_file_bytes_limited(&path.display_path, max_bytes)?;
                let after = service.stat(&path.display_path)?;
                anyhow::ensure!(
                    before == after
                        && (after.kind != crate::ftp::FtpEntryKind::File
                            || after.size == bytes.len() as u64),
                    "File changed while being read"
                );
                anyhow::ensure!(bytes.len() as u64 <= max_bytes, "File is too large to open");
                let revision = RemoteTextRevision::from_bytes(
                    &bytes,
                    RemoteTextMetadata {
                        size: bytes.len() as u64,
                        modified_at: after.modified.and_then(system_time_secs),
                    },
                );
                Ok(ByteDocument {
                    path: path.display_path.clone(),
                    bytes,
                    revision,
                })
            }
        }
    }

    pub fn write_encoded_document_path(
        &self,
        path: &RemoteFilePath,
        request: EncodedDocumentWrite<'_>,
    ) -> anyhow::Result<RemoteTextWriteResult> {
        // Validate the entire output before any backend can create or truncate a file.
        let bytes = request.format.encode(request.content, request.max_bytes)?;
        match self {
            Self::Local(service) => service.write_document_bytes(
                resolve_local_path(&path.display_path),
                &bytes,
                request.expected_revision,
                request.force,
            ),
            Self::Remote(service) => {
                service.write_document_bytes(path, &bytes, request.expected_revision, request.force)
            }
            Self::Adb(service) => {
                service.write_document_bytes(path, &bytes, request.expected_revision, request.force)
            }
            Self::Ftp(service) => service
                .write_document_bytes(
                    &path.display_path,
                    &bytes,
                    request.expected_revision,
                    request.force,
                )
                .map_err(anyhow::Error::from),
        }
    }
}

pub struct EncodedDocumentWrite<'a> {
    pub content: &'a str,
    pub format: FileTextFormat,
    pub expected_revision: Option<&'a RemoteTextRevision>,
    pub force: bool,
    pub max_bytes: u64,
}

#[cfg(test)]
mod tests;
