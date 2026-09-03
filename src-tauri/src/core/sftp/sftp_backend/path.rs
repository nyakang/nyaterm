//! Internal pieces of the SFTP backend moved out of `sftp_backend.rs`.

use super::*;

impl SftpBackend {
    pub(crate) fn new(
        ssh_handle: Arc<SshConnectionHandles>,
        encoding: &str,
        pipeline_depth_override: Option<u32>,
    ) -> Self {
        Self {
            ssh_handle,
            identity_cache: Arc::new(RwLock::new(RemoteIdentityCache::default())),
            path_cache: Arc::new(RwLock::new(HashMap::new())),
            encoding: encoding.to_string(),
            pipeline_depth_override,
        }
    }

    /// Get the encoding setting for this connection
    pub(crate) fn encoding(&self) -> &str {
        &self.encoding
    }

    /// Convert UTF-8 path to raw bytes for SFTP operations.
    /// Uses the connection's encoding setting.
    pub(super) fn encode_path_for_sftp(&self, path: &str) -> Vec<u8> {
        let encoding = Encoding::for_label(self.encoding.trim().as_bytes()).unwrap_or(UTF_8);
        if encoding == UTF_8 || path.bytes().all(|b| b < 128) {
            return path.as_bytes().to_vec();
        }

        let (encoded, _, _) = encoding.encode(path);
        encoded.into_owned()
    }

    /// Decode raw bytes to string using the connection's encoding.
    pub(super) fn decode_path_from_sftp(&self, bytes: &[u8]) -> String {
        let encoding = Encoding::for_label(self.encoding.trim().as_bytes()).unwrap_or(UTF_8);
        if encoding == UTF_8 {
            return String::from_utf8_lossy(bytes).into_owned();
        }
        let (decoded, _, had_errors) = encoding.decode(bytes);
        if had_errors {
            String::from_utf8_lossy(bytes).into_owned()
        } else {
            decoded.into_owned()
        }
    }

    pub(super) fn remote_path_bytes(&self, path: &RemotePathRef) -> Vec<u8> {
        path.raw_path()
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| self.encode_path_for_sftp(path.display_path()))
    }
}

pub(super) fn normalize_remote_dir_path_bytes(path: &[u8]) -> Vec<u8> {
    if path == b"/" {
        return b"/".to_vec();
    }

    let trimmed = path
        .iter()
        .rposition(|byte| *byte != b'/')
        .map(|index| &path[..=index])
        .unwrap_or(path);
    if trimmed.is_empty() {
        path.to_vec()
    } else {
        trimmed.to_vec()
    }
}

pub(super) fn join_remote_child(parent: &str, name: &str) -> String {
    if parent == "/" {
        format!("/{name}")
    } else {
        format!("{parent}/{name}")
    }
}

pub(super) fn join_remote_child_bytes(parent: &[u8], name: &[u8]) -> Vec<u8> {
    let mut path = Vec::with_capacity(parent.len() + name.len() + 1);
    if parent == b"/" {
        path.push(b'/');
        path.extend_from_slice(name);
    } else {
        path.extend_from_slice(parent);
        path.push(b'/');
        path.extend_from_slice(name);
    }
    path
}

#[allow(dead_code)]
pub(super) fn is_safe_recursive_remove_target(path: &str) -> bool {
    let trimmed = path.trim();
    if trimmed.is_empty() || matches!(trimmed, "/" | "." | "..") {
        return false;
    }

    let normalized = normalize_remote_dir_path(trimmed);
    !normalized.is_empty()
        && !matches!(normalized, "/" | "." | "..")
        && !normalized.split('/').any(|part| part == "..")
}

pub(super) fn is_safe_recursive_remove_target_bytes(path: &[u8]) -> bool {
    let normalized = normalize_remote_dir_path_bytes(path);
    if normalized.is_empty() || normalized == b"/" || normalized == b"." || normalized == b".." {
        return false;
    }

    !normalized
        .split(|byte| *byte == b'/')
        .any(|part| part == b"..")
}
