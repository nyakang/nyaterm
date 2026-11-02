use crate::engine::{ReplicaSnapshot, digest};
use nyaterm_core::sync_schema::secrets::EncryptedSecretValue;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const WRITERS_PREFIX: &str = "sync/v4/writers/";
pub const MAX_PUBLICATION_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderCapabilities {
    pub complete_listing: bool,
    pub max_files: Option<usize>,
    pub max_object_bytes: usize,
}

impl ProviderCapabilities {
    pub fn for_provider(provider: &str) -> Option<Self> {
        matches!(provider, "local_directory" | "webdav" | "s3").then_some(Self {
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
        let tail = path.strip_prefix(WRITERS_PREFIX)?;
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
            && id.path() == path)
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
