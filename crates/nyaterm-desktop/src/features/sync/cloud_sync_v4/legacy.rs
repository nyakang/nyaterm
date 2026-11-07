//! Retained V4 publications are migration inputs, never publication targets.
use std::collections::BTreeMap;

use nyaterm_core::cloud_sync::CloudSyncRemote;
use nyaterm_core::sync_schema::encryption;
use nyaterm_core::{CloudSyncError, LocalCloudSyncOptions};
use nyaterm_sync::engine::ReplicaSnapshot;
use nyaterm_sync::protocol::{PublicationId, PublishedReplica};
use uuid::Uuid;

use super::{MAX_OBJECTS, invalid, path};

pub(super) fn read(
    remote: &dyn CloudSyncRemote,
    options: &LocalCloudSyncOptions,
) -> Result<Vec<(String, ReplicaSnapshot)>, CloudSyncError> {
    let registry = path(options, "sync/v4/registry");
    let entries = remote.list_sync_files(&registry)?;
    if entries.len() > MAX_OBJECTS {
        return Err(invalid());
    }
    let mut result = Vec::new();
    let mut identities = BTreeMap::new();
    for entry in entries {
        let name = entry
            .strip_prefix(&format!("{registry}/"))
            .ok_or_else(invalid)?;
        let writer = Uuid::parse_str(name.strip_suffix(".bin").ok_or_else(invalid)?)
            .map_err(|_| invalid())?;
        if writer.is_nil()
            || name != format!("{writer}.bin")
            || remote.read_if_exists(&entry)?.as_deref() != Some(writer.as_bytes())
        {
            return Err(invalid());
        }
        let prefix = path(options, &format!("sync/v4/writers/{writer}"));
        for entry in remote.list_sync_files(&prefix)? {
            if result.len() >= MAX_OBJECTS {
                return Err(invalid());
            }
            let relative = entry
                .strip_prefix(&format!("{}/", options.remote_root.trim_matches('/')))
                .unwrap_or(&entry);
            let id = PublicationId::parse_legacy(relative).ok_or_else(invalid)?;
            if id.writer != writer
                || identities
                    .insert((writer, id.sequence), id.hash.clone())
                    .is_some_and(|previous| previous != id.hash)
            {
                return Err(invalid());
            }
            let bytes = remote
                .read_if_exists(&entry)?
                .ok_or(CloudSyncError::StaleRemoteRevision)?;
            if !id.verifies(&bytes) {
                return Err(invalid());
            }
            let plaintext = encryption::open(
                options.master_password.expose_secret(),
                "publication",
                &format!("{writer}/{}", id.sequence),
                &bytes,
            )?;
            let publication: PublishedReplica =
                serde_json::from_slice(&plaintext).map_err(|_| invalid())?;
            if publication.schema != 4
                || publication.writer != writer
                || publication.sequence != id.sequence
            {
                return Err(invalid());
            }
            nyaterm_sync::engine::SyncReplica::load(&publication.replica, Uuid::nil())
                .map_err(|_| invalid())?;
            result.push((id.hash, publication.replica));
        }
    }
    Ok(result)
}
