use std::time::Duration;

use crate::{CloudLocalStore, PortableSnapshotError, RawPortableSnapshot};

use super::{
    CloudSyncError, CloudSyncRemote, LocalCloudSyncOptions, REMOTE_SYNC_POINTER_SCHEMA_VERSION,
    RemoteSyncPointer, SYNC_CURRENT_FILE, SnapshotUnreadableCause, legacy_sync_snapshot_file,
    load_sync_pointer_from_remote, remote_path, write_sync_pointer,
};

const MIN_ENCRYPTED_SNAPSHOT_BYTES: usize = 13;
const MAX_ENCRYPTED_SNAPSHOT_BYTES: usize = 64 * 1024 * 1024;

/// Read-back verification attempts after writing a snapshot. Object stores with
/// eventual consistency (WebDAV gateways, drives) may not serve the bytes we
/// just wrote on the first read, so the first attempt is retried with backoff.
pub(super) const READBACK_VERIFY_ATTEMPTS: usize = 3;
/// Backoff between read-back verification attempts. Kept short: the caller is
/// holding the cloud-sync operation lock while this runs.
pub(super) const READBACK_VERIFY_BACKOFF: Duration = Duration::from_millis(400);

/// Sleep before the next read-back attempt of a capacity-less store.
pub(super) fn readback_backoff(attempt: usize) {
    std::thread::sleep(READBACK_VERIFY_BACKOFF * (attempt as u32 + 1));
}

/// Details about a remote head that exists but cannot be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RemoteSnapshotUnreadableHead {
    /// Human-readable reason, surfaced verbatim in the conflict banner.
    pub reason: String,
    /// Whether the local snapshot must not be pushed over this remote head.
    pub overwrite_blocked: bool,
}

#[derive(Debug)]
pub(super) enum RemoteSnapshotResolution {
    Current(RawPortableSnapshot),
    LegacyMigrated(RawPortableSnapshot),
    Inconsistent {
        pointer: RemoteSyncPointer,
        recovery_candidate: Option<RawPortableSnapshot>,
        unreadable: Option<RemoteSnapshotUnreadableHead>,
    },
}

pub(super) fn pointer_from_snapshot(snapshot: &RawPortableSnapshot) -> RemoteSyncPointer {
    RemoteSyncPointer {
        schema_version: REMOTE_SYNC_POINTER_SCHEMA_VERSION,
        revision_id: snapshot.meta.revision_id.clone(),
        created_at_ms: snapshot.meta.created_at_ms,
        payload_hash: snapshot.source_payload_hash().to_string(),
        device_id: snapshot.meta.device_id.clone(),
        app_version: snapshot.meta.app_version.clone(),
        sync_clock: snapshot.meta.sync_clock.clone(),
    }
}

pub(super) fn upload_sync_snapshot(
    local_store: &dyn CloudLocalStore,
    remote: &dyn CloudSyncRemote,
    options: &LocalCloudSyncOptions,
    snapshot: &RawPortableSnapshot,
    expected: Option<&RemoteSyncPointer>,
) -> Result<Vec<u8>, CloudSyncError> {
    if snapshot.meta.schema_version != crate::portable_snapshot::FIXED_SYNC_SNAPSHOT_SCHEMA_VERSION
        || crate::portable_snapshot::project_fixed_sync_snapshot(snapshot)?.entities
            != snapshot.entities
    {
        return Err(CloudSyncError::Remote(
            "Only fixed-scope snapshots can be published".into(),
        ));
    }
    let bytes =
        local_store.encode_sync_snapshot(snapshot, options.master_password.expose_secret())?;
    upload_verified_snapshot(
        local_store,
        remote,
        options,
        &pointer_from_snapshot(snapshot),
        expected,
        &bytes,
        true,
    )?;
    Ok(bytes)
}

fn upload_verified_snapshot(
    local_store: &dyn CloudLocalStore,
    remote: &dyn CloudSyncRemote,
    options: &LocalCloudSyncOptions,
    pointer: &RemoteSyncPointer,
    expected: Option<&RemoteSyncPointer>,
    bytes: &[u8],
    stage_device: bool,
) -> Result<(), CloudSyncError> {
    let path = remote_path(
        &options.remote_root,
        &legacy_sync_snapshot_file(&pointer.revision_id),
    );
    // File-slot stores (GitHub Gist, Gitee Snippet) only ever need a second
    // attempt, and only for capacity reasons. Object stores with eventual
    // consistency need read-back retries after a single write: repeating PUT
    // can restart propagation and keep the new content invisible indefinitely.
    let capacity_limited = remote.file_capacity()?.is_some();
    let attempts = if capacity_limited {
        2
    } else {
        READBACK_VERIFY_ATTEMPTS
    };
    for attempt in 0..attempts {
        if capacity_limited || attempt == 0 {
            super::gc::reserve_snapshot_capacity(
                local_store,
                options,
                remote,
                expected,
                &pointer.revision_id,
            )?;
            if stage_device {
                super::heads::stage(
                    remote,
                    &options.remote_root,
                    &options.device_id,
                    &pointer.revision_id,
                )?;
            }
        }
        let last_attempt = attempt + 1 == attempts;
        // Capacity-limited stores may retry the write after reserving slots.
        // Other stores only retry GET once PUT has been accepted.
        if (capacity_limited || attempt == 0)
            && let Err(error) = remote.write(&path, bytes)
        {
            if capacity_limited && !last_attempt {
                continue;
            }
            return Err(
                if capacity_limited && matches!(error, CloudSyncError::SnapshotMissing { .. }) {
                    CloudSyncError::GistCapacity
                } else {
                    error
                },
            );
        }
        match verify_written_snapshot(local_store, remote, options, pointer, bytes) {
            Ok(()) => return Ok(()),
            Err(error) if last_attempt || !readback_retry_is_useful(&error) => {
                return Err(
                    if capacity_limited && matches!(error, CloudSyncError::SnapshotMissing { .. }) {
                        CloudSyncError::GistCapacity
                    } else {
                        error
                    },
                );
            }
            Err(_) => {}
        }
        if !capacity_limited {
            readback_backoff(attempt);
        }
    }
    Err(CloudSyncError::Remote(format!(
        "remote snapshot {path} could not be verified after {attempts} attempts"
    )))
}

/// Whether another read-back attempt could plausibly succeed.
fn readback_retry_is_useful(error: &CloudSyncError) -> bool {
    !matches!(
        error,
        CloudSyncError::Cancelled | CloudSyncError::Disabled | CloudSyncError::GistCapacity
    )
}

/// Verify that the remote kept the snapshot that was just uploaded.
///
/// The read-back bytes are compared with the bytes that were written *before*
/// they are decrypted. Identical ciphertext is a precondition for a successful
/// decrypt, so a remote that serves anything else — a stale copy behind an
/// eventually-consistent gateway, a proxy that rewrites the body, or an upload
/// that never persisted — is reported as a divergent write instead of as a
/// decryption failure, which is what it otherwise looks like.
fn verify_written_snapshot(
    local_store: &dyn CloudLocalStore,
    remote: &dyn CloudSyncRemote,
    options: &LocalCloudSyncOptions,
    pointer: &RemoteSyncPointer,
    written: &[u8],
) -> Result<(), CloudSyncError> {
    let path = remote_path(
        &options.remote_root,
        &legacy_sync_snapshot_file(&pointer.revision_id),
    );
    let Some(read_back) = remote.read_if_exists(&path)? else {
        return Err(CloudSyncError::SnapshotMissing {
            revision: pointer.revision_id.clone(),
        });
    };
    if read_back != written {
        return Err(CloudSyncError::RemoteWriteDiverged {
            revision: pointer.revision_id.clone(),
            path,
            expected_bytes: written.len(),
            expected_digest: sha256_hex(written),
            actual_bytes: read_back.len(),
            actual_digest: sha256_hex(&read_back),
        });
    }
    remote.observe(super::io::CloudSyncIoEvent::FullValidation);
    decode_and_validate_remote_snapshot(local_store, &read_back, options, &path, pointer)
        .map(|_| ())
}

pub(super) fn read_snapshot_for_pointer(
    local_store: &dyn CloudLocalStore,
    remote: &dyn CloudSyncRemote,
    options: &LocalCloudSyncOptions,
    pointer: &RemoteSyncPointer,
) -> Result<RawPortableSnapshot, CloudSyncError> {
    let path = remote_path(
        &options.remote_root,
        &legacy_sync_snapshot_file(&pointer.revision_id),
    );
    let Some(bytes) = remote.read_if_exists(&path)? else {
        return Err(CloudSyncError::SnapshotMissing {
            revision: pointer.revision_id.clone(),
        });
    };
    remote.observe(super::io::CloudSyncIoEvent::FullValidation);
    decode_and_validate_remote_snapshot(local_store, &bytes, options, &path, pointer)
}

fn decode_and_validate_remote_snapshot(
    local_store: &dyn CloudLocalStore,
    bytes: &[u8],
    options: &LocalCloudSyncOptions,
    path: &str,
    pointer: &RemoteSyncPointer,
) -> Result<RawPortableSnapshot, CloudSyncError> {
    validate_remote_snapshot_size(bytes, path, &pointer.revision_id)?;
    let snapshot = decode_remote_snapshot(
        local_store,
        bytes,
        options.master_password.expose_secret(),
        path,
        &pointer.revision_id,
    )?;
    validate_snapshot_against_pointer(pointer, &snapshot)?;
    Ok(snapshot)
}

fn read_current_sync_snapshot_compat(
    local_store: &dyn CloudLocalStore,
    remote: &dyn CloudSyncRemote,
    options: &LocalCloudSyncOptions,
) -> Result<Option<RawPortableSnapshot>, CloudSyncError> {
    let path = remote_path(&options.remote_root, SYNC_CURRENT_FILE);
    let Some(bytes) = remote.read_if_exists(&path)? else {
        return Ok(None);
    };
    validate_remote_snapshot_size(&bytes, &path, "current")?;
    remote.observe(super::io::CloudSyncIoEvent::FullValidation);
    decode_remote_snapshot(
        local_store,
        &bytes,
        options.master_password.expose_secret(),
        &path,
        "current",
    )
    .map(Some)
}

pub(super) fn ensure_remote_head_unchanged(
    local_store: &dyn CloudLocalStore,
    remote: &dyn CloudSyncRemote,
    remote_root: &str,
    expected: Option<&RemoteSyncPointer>,
) -> Result<(), CloudSyncError> {
    let actual = load_sync_pointer_from_remote(local_store, remote, remote_root)?;
    let expected_revision = expected.map(|pointer| pointer.revision_id.clone());
    let actual_revision = actual.as_ref().map(|pointer| pointer.revision_id.clone());
    if expected_revision != actual_revision {
        return Err(CloudSyncError::ConcurrentUpdate {
            expected_revision,
            actual_revision,
        });
    }
    Ok(())
}

pub(super) fn resolve_remote_snapshot(
    local_store: &dyn CloudLocalStore,
    remote: &dyn CloudSyncRemote,
    options: &LocalCloudSyncOptions,
    pointer: &RemoteSyncPointer,
) -> Result<RemoteSnapshotResolution, CloudSyncError> {
    // An unreadable head is not a hard failure: the compatible `current.redb.enc`
    // copy may still repair it, and otherwise the caller turns this into an
    // explicit, recoverable conflict instead of a dead end.
    let mut unreadable = match read_snapshot_for_pointer(local_store, remote, options, pointer) {
        Ok(snapshot) => return Ok(RemoteSnapshotResolution::Current(snapshot)),
        // Anything that makes the head unusable is recoverable state, not a dead
        // end: `current.redb.enc` may still repair it, and otherwise the caller
        // turns this into an explicit conflict. The reason travels with it so the
        // banner can say what actually happened.
        Err(
            error @ (CloudSyncError::SnapshotMissing { .. }
            | CloudSyncError::CorruptedSnapshot { .. }
            | CloudSyncError::RevisionMismatch { .. }
            | CloudSyncError::HashMismatch { .. }
            | CloudSyncError::RemoteSnapshotUnreadable { .. }),
        ) => unreadable_head(&error),
        Err(error) => return Err(error),
    };

    let current = match read_current_sync_snapshot_compat(local_store, remote, options) {
        Ok(Some(current)) => current,
        Ok(None) | Err(CloudSyncError::CorruptedSnapshot { .. }) => {
            return Ok(RemoteSnapshotResolution::Inconsistent {
                pointer: pointer.clone(),
                recovery_candidate: None,
                unreadable: Some(unreadable),
            });
        }
        Err(error @ CloudSyncError::RemoteSnapshotUnreadable { .. }) => {
            // Preserve both diagnoses. A missing or corrupted head must never
            // hide evidence that the configured password cannot read current.
            let current_unreadable = unreadable_head(&error);
            unreadable.reason.push(' ');
            unreadable.reason.push_str(&current_unreadable.reason);
            unreadable.overwrite_blocked |= current_unreadable.overwrite_blocked;
            return Ok(RemoteSnapshotResolution::Inconsistent {
                pointer: pointer.clone(),
                recovery_candidate: None,
                unreadable: Some(unreadable),
            });
        }
        Err(error) => return Err(error),
    };
    if validate_snapshot_against_pointer(pointer, &current).is_ok()
        && current.meta.schema_version
            == crate::portable_snapshot::FIXED_SYNC_SNAPSHOT_SCHEMA_VERSION
        && crate::portable_snapshot::project_fixed_sync_snapshot(&current)?.entities
            == current.entities
    {
        let current_bytes = remote
            .read_if_exists(&remote_path(&options.remote_root, SYNC_CURRENT_FILE))?
            .ok_or(CloudSyncError::SnapshotMissing {
                revision: "current".to_string(),
            })?;
        upload_verified_snapshot(
            local_store,
            remote,
            options,
            pointer,
            Some(pointer),
            &current_bytes,
            false,
        )?;
        read_snapshot_for_pointer(local_store, remote, options, pointer)?;
        return Ok(RemoteSnapshotResolution::LegacyMigrated(current));
    }

    Ok(RemoteSnapshotResolution::Inconsistent {
        pointer: pointer.clone(),
        recovery_candidate: Some(current),
        unreadable: Some(unreadable),
    })
}

/// Build the banner-facing summary for a payload that could not be decrypted.
fn unreadable_head(error: &CloudSyncError) -> RemoteSnapshotUnreadableHead {
    let CloudSyncError::RemoteSnapshotUnreadable {
        revision,
        path,
        bytes,
        digest,
        cause,
        ..
    } = error
    else {
        return RemoteSnapshotUnreadableHead {
            reason: format!("The remote snapshot could not be read: {error}"),
            overwrite_blocked: false,
        };
    };
    RemoteSnapshotUnreadableHead {
        reason: format!(
            "The remote snapshot {revision} at '{path}' ({bytes} bytes, sha256 {digest}) could not \
             be decrypted: {cause}."
        ),
        overwrite_blocked: cause.blocks_remote_overwrite(),
    }
}

pub(super) fn recover_current_remote_snapshot(
    local_store: &dyn CloudLocalStore,
    remote: &dyn CloudSyncRemote,
    options: &LocalCloudSyncOptions,
    control: &super::control::CloudSyncOperationControl,
) -> Result<RawPortableSnapshot, CloudSyncError> {
    let Some(snapshot) = read_current_sync_snapshot_compat(local_store, remote, options)? else {
        return Err(CloudSyncError::SnapshotMissing {
            revision: "current".to_string(),
        });
    };
    let heads = super::heads::read(local_store, remote, &options.remote_root)?;
    let mut snapshot = crate::portable_snapshot::project_fixed_sync_snapshot(&snapshot)?;
    snapshot.meta.revision_id = uuid::Uuid::new_v4().to_string();
    snapshot.meta.device_id = options.device_id.clone();
    snapshot.meta.created_at_ms = super::current_time_ms();
    snapshot.meta.sync_clock = Some(heads.next_clock(None, &options.device_id)?);
    snapshot.recalculate_hash()?;
    let bytes = upload_sync_snapshot(
        local_store,
        remote,
        options,
        &snapshot,
        heads.compatibility.as_ref(),
    )?;
    let pointer = pointer_from_snapshot(&snapshot);
    ensure_remote_head_unchanged(
        local_store,
        remote,
        &options.remote_root,
        heads.compatibility.as_ref(),
    )?;
    control.begin_commit()?;
    super::heads::publish(remote, &options.remote_root, &pointer)?;
    let _ = write_sync_pointer(local_store, remote, &options.remote_root, &pointer);
    let _ = remote.write(
        &remote_path(&options.remote_root, SYNC_CURRENT_FILE),
        &bytes,
    );
    Ok(snapshot)
}

pub(super) fn validate_snapshot_against_pointer(
    pointer: &RemoteSyncPointer,
    snapshot: &RawPortableSnapshot,
) -> Result<(), CloudSyncError> {
    if snapshot.meta.revision_id != pointer.revision_id {
        return Err(CloudSyncError::RevisionMismatch {
            pointer_revision: pointer.revision_id.clone(),
            snapshot_revision: snapshot.meta.revision_id.clone(),
        });
    }
    if pointer.sync_clock != snapshot.meta.sync_clock {
        return Err(CloudSyncError::Remote(
            "snapshot causal clock does not match its head".into(),
        ));
    }
    if snapshot.source_payload_hash() != pointer.payload_hash {
        return Err(CloudSyncError::HashMismatch {
            expected: pointer.payload_hash.clone(),
            actual: snapshot.source_payload_hash().to_string(),
        });
    }
    Ok(())
}

fn decode_remote_snapshot(
    local_store: &dyn CloudLocalStore,
    bytes: &[u8],
    master_password: &str,
    path: &str,
    revision: &str,
) -> Result<RawPortableSnapshot, CloudSyncError> {
    local_store
        .decode_sync_snapshot(bytes, master_password)
        .map_err(|error| match error {
            CloudSyncError::PortableSnapshot(source @ PortableSnapshotError::Decrypt { .. }) => {
                CloudSyncError::RemoteSnapshotUnreadable {
                    revision: revision.to_string(),
                    path: path.to_string(),
                    bytes: bytes.len(),
                    digest: sha256_hex(bytes),
                    cause: decrypt_cause(local_store, master_password),
                    source: Box::new(source),
                }
            }
            // A missing master password is a local misconfiguration, not a
            // property of the remote bytes.
            error @ CloudSyncError::PortableSnapshot(
                PortableSnapshotError::MissingMasterPassword
                | PortableSnapshotError::UnsupportedEncryptionVersion(_)
                | PortableSnapshotError::InvalidEncryptionEnvelope
                | PortableSnapshotError::UnsupportedVersion(_),
            ) => error,
            error => CloudSyncError::CorruptedSnapshot {
                revision: revision.to_string(),
                detail: format!(
                    "{} could not be decoded: {error}",
                    describe_payload(path, bytes)
                ),
            },
        })
}

/// Decide whether a decryption failure means "wrong local password" or
/// "the remote payload is not ours".
fn decrypt_cause(
    local_store: &dyn CloudLocalStore,
    master_password: &str,
) -> SnapshotUnreadableCause {
    match local_store.verify_master_password(master_password) {
        Ok(Some(true)) => SnapshotUnreadableCause::PayloadNotAuthentic,
        Ok(Some(false)) => SnapshotUnreadableCause::MasterPasswordMismatch,
        _ => SnapshotUnreadableCause::Unknown,
    }
}

fn validate_remote_snapshot_size(
    bytes: &[u8],
    path: &str,
    revision: &str,
) -> Result<(), CloudSyncError> {
    if (MIN_ENCRYPTED_SNAPSHOT_BYTES..=MAX_ENCRYPTED_SNAPSHOT_BYTES).contains(&bytes.len()) {
        Ok(())
    } else {
        Err(CloudSyncError::CorruptedSnapshot {
            revision: revision.to_string(),
            detail: format!(
                "{} is outside the accepted {MIN_ENCRYPTED_SNAPSHOT_BYTES}..={MAX_ENCRYPTED_SNAPSHOT_BYTES} byte range",
                describe_payload(path, bytes)
            ),
        })
    }
}

/// Identify a remote payload in an error message: path, size and content digest.
fn describe_payload(path: &str, bytes: &[u8]) -> String {
    format!(
        "'{path}' ({} bytes, sha256 {})",
        bytes.len(),
        sha256_hex(bytes)
    )
}

pub(super) fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};

    hex::encode(Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_ENCRYPTED_SNAPSHOT_BYTES, describe_payload, readback_retry_is_useful, sha256_hex,
        validate_remote_snapshot_size,
    };

    #[test]
    fn remote_ciphertext_rejects_oversize_before_decode() {
        assert!(validate_remote_snapshot_size(&[0; 12], "path", "revision").is_err());
        assert!(
            validate_remote_snapshot_size(
                &vec![0; MAX_ENCRYPTED_SNAPSHOT_BYTES + 1],
                "path",
                "revision"
            )
            .is_err()
        );
    }

    #[test]
    fn size_failure_reports_path_size_and_digest() {
        let error = validate_remote_snapshot_size(&[0; 12], "sync/snapshots/r.redb.enc", "r")
            .expect_err("too short");
        let message = error.to_string();
        assert!(message.contains("sync/snapshots/r.redb.enc"), "{message}");
        assert!(message.contains("12 bytes"), "{message}");
        assert!(message.contains(&sha256_hex(&[0; 12])), "{message}");
    }

    #[test]
    fn payload_description_is_stable() {
        assert_eq!(
            describe_payload("a/b", b""),
            format!("'a/b' (0 bytes, sha256 {})", sha256_hex(b""))
        );
    }

    #[test]
    fn cancelled_and_disabled_readbacks_are_not_retried() {
        assert!(!readback_retry_is_useful(&crate::CloudSyncError::Cancelled));
        assert!(!readback_retry_is_useful(&crate::CloudSyncError::Disabled));
        assert!(readback_retry_is_useful(&crate::CloudSyncError::Remote(
            "WebDAV PUT failed".into()
        )));
        assert!(readback_retry_is_useful(
            &crate::CloudSyncError::SnapshotMissing {
                revision: "r".into()
            }
        ));
        assert!(readback_retry_is_useful(
            &crate::CloudSyncError::CorruptedSnapshot {
                revision: "r".into(),
                detail: "truncated".into(),
            }
        ));
    }
}
