use crate::engine::{ReplicaSnapshot, SyncError, SyncReplica, digest};
use nyaterm_core::sync_schema::secrets::EncryptedSecretValue;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const WRITERS_PREFIX: &str = "sync/v5/writers/";
pub const LEGACY_WRITERS_PREFIX: &str = "sync/v4/writers/";
pub const PUBLICATION_SCHEMA_VERSION: u32 = 5;
pub const MAX_PUBLICATION_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderCapabilities {
    pub complete_listing: bool,
    pub max_files: Option<usize>,
    pub max_object_bytes: usize,
}

impl ProviderCapabilities {
    pub fn for_provider(provider: &str) -> Option<Self> {
        matches!(
            provider,
            "local_directory" | "webdav" | "s3" | "google_drive" | "onedrive" | "aliyun_drive"
        )
        .then_some(Self {
            complete_listing: true,
            max_files: None,
            max_object_bytes: MAX_PUBLICATION_BYTES,
        })
    }
    pub fn supports_v4(&self) -> bool {
        self.complete_listing
            && self.max_files.is_none()
            && self.max_object_bytes >= MAX_PUBLICATION_BYTES
    }

    /// The protocol that can actually run for `provider`.
    ///
    /// A stored V4 preference only applies to providers that passed capability
    /// validation. Switching to a legacy provider must keep synchronizing
    /// instead of failing every attempt against an unsupported protocol.
    pub fn effective_protocol_version(provider: &str, configured: u32) -> u32 {
        if configured == 4
            && !Self::for_provider(provider).is_some_and(|capabilities| capabilities.supports_v4())
        {
            3
        } else {
            configured
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PublicationId {
    pub writer: Uuid,
    pub sequence: u64,
    pub hash: String,
}

impl PublicationId {
    pub fn path(&self) -> String {
        format!(
            "{WRITERS_PREFIX}{}/{}-{}.bin",
            self.writer, self.sequence, self.hash
        )
    }
    pub fn parse(path: &str) -> Option<Self> {
        Self::parse_with_prefix(path, WRITERS_PREFIX)
    }
    pub fn parse_legacy(path: &str) -> Option<Self> {
        Self::parse_with_prefix(path, LEGACY_WRITERS_PREFIX)
    }
    fn parse_with_prefix(path: &str, prefix: &str) -> Option<Self> {
        let tail = path.strip_prefix(prefix)?;
        let (writer, file) = tail.split_once('/')?;
        let (sequence, hash) = file.strip_suffix(".bin")?.split_once('-')?;
        let id = Self {
            writer: Uuid::parse_str(writer).ok()?,
            sequence: sequence.parse().ok()?,
            hash: hash.into(),
        };
        (!id.writer.is_nil()
            && id.sequence > 0
            && id.hash.len() == 64
            && id
                .hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            && format!("{prefix}{}/{}-{}.bin", id.writer, id.sequence, id.hash) == path)
            .then_some(id)
    }
    pub fn verifies(&self, bytes: &[u8]) -> bool {
        bytes.len() <= MAX_PUBLICATION_BYTES && digest(bytes) == self.hash
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublishedReplica {
    pub schema: u32,
    pub writer: Uuid,
    pub sequence: u64,
    pub replica: ReplicaSnapshot,
    #[serde(default)]
    pub secret_objects: std::collections::BTreeMap<String, EncryptedSecretValue>,
}

impl PublishedReplica {
    pub fn fixed(
        writer: Uuid,
        sequence: u64,
        snapshot: &ReplicaSnapshot,
    ) -> Result<Self, SyncError> {
        let mut replica = SyncReplica::load(snapshot, writer)?;
        Ok(Self {
            schema: PUBLICATION_SCHEMA_VERSION,
            writer,
            sequence,
            replica: replica.fixed_snapshot()?,
            secret_objects: Default::default(),
        })
    }

    pub fn validate_fixed(&self) -> Result<(), SyncError> {
        if self.schema != PUBLICATION_SCHEMA_VERSION
            || self.replica.schema != 5
            || !self.secret_objects.is_empty()
        {
            return Err(SyncError::InvalidReplica);
        }
        let mut replica = SyncReplica::load(&self.replica, self.writer)?;
        if replica.fixed_snapshot()?.values != self.replica.values {
            return Err(SyncError::InvalidReplica);
        }
        Ok(())
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingPublication {
    pub id: PublicationId,
    pub bytes: Vec<u8>,
    pub document_hash: String,
}

impl PendingPublication {
    pub fn new(writer: Uuid, sequence: u64, bytes: Vec<u8>, document_hash: String) -> Option<Self> {
        if writer.is_nil() || sequence == 0 || bytes.len() > MAX_PUBLICATION_BYTES {
            return None;
        }
        Some(Self {
            id: PublicationId {
                writer,
                sequence,
                hash: digest(&bytes),
            },
            bytes,
            document_hash,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::ProviderCapabilities;

    #[test]
    fn a_stored_v4_preference_falls_back_to_the_legacy_protocol_for_snippet_providers() {
        for provider in ["github_gist", "gitee_snippet", "unknown_provider"] {
            assert_eq!(
                ProviderCapabilities::effective_protocol_version(provider, 4),
                3,
                "{provider} has not passed V4 capability validation"
            );
        }
        for provider in [
            "local_directory",
            "webdav",
            "s3",
            "google_drive",
            "onedrive",
            "aliyun_drive",
        ] {
            assert_eq!(
                ProviderCapabilities::effective_protocol_version(provider, 4),
                4,
                "{provider} supports V4"
            );
        }
    }

    #[test]
    fn a_configured_legacy_protocol_is_never_promoted() {
        for provider in ["github_gist", "webdav"] {
            for configured in [1, 2, 3, 5] {
                assert_eq!(
                    ProviderCapabilities::effective_protocol_version(provider, configured),
                    configured
                );
            }
        }
    }
}
