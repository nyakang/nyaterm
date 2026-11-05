use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_REMOTE_TEXT_GENERATION: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RemoteTextGeneration(u64);

impl RemoteTextGeneration {
    pub fn next() -> Self {
        Self(NEXT_REMOTE_TEXT_GENERATION.fetch_add(1, Ordering::Relaxed))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteTextMetadata {
    pub size: u64,
    pub modified_at: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteTextRevision {
    pub content_sha256: [u8; 32],
    pub metadata: RemoteTextMetadata,
}

impl RemoteTextRevision {
    pub fn from_bytes(content: &[u8], metadata: RemoteTextMetadata) -> Self {
        Self {
            content_sha256: Sha256::digest(content).into(),
            metadata,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteTextDocument {
    pub path: String,
    pub content: String,
    pub revision: RemoteTextRevision,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoteTextWriteResult {
    Saved {
        revision: RemoteTextRevision,
    },
    SavedWithBackup {
        revision: RemoteTextRevision,
        backup_path: String,
    },
    Conflict,
}

pub(crate) fn metadata_is_stable(before: &RemoteTextMetadata, after: &RemoteTextMetadata) -> bool {
    before.size == after.size && before.modified_at == after.modified_at
}

#[cfg(test)]
mod tests {
    use super::{RemoteTextMetadata, RemoteTextRevision, metadata_is_stable};

    #[test]
    fn revision_hash_detects_same_size_same_metadata_content_changes() {
        let metadata = RemoteTextMetadata {
            size: 4,
            modified_at: Some(42),
        };
        let first = RemoteTextRevision::from_bytes(b"same", metadata.clone());
        let second = RemoteTextRevision::from_bytes(b"diff", metadata);

        assert_ne!(first, second);
        assert_ne!(first.content_sha256, second.content_sha256);
    }

    #[test]
    fn metadata_stability_requires_all_available_fields_to_match() {
        let baseline = RemoteTextMetadata {
            size: 4,
            modified_at: Some(42),
        };
        assert!(metadata_is_stable(&baseline, &baseline));
        assert!(!metadata_is_stable(
            &baseline,
            &RemoteTextMetadata {
                size: 4,
                modified_at: Some(43),
            }
        ));
        assert!(!metadata_is_stable(
            &baseline,
            &RemoteTextMetadata {
                size: 5,
                modified_at: Some(42),
            }
        ));
    }
}
