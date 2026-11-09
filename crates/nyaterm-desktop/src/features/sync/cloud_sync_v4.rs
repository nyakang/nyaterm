//! Blocking V4 orchestration. Prepare is read-only; the Store owns atomic Apply.
mod legacy;
mod local;
mod rotation;
pub(in crate::features) use rotation::rotate_master_password;
use std::collections::{BTreeMap, BTreeSet};

use nyaterm_core::cloud_sync::{CloudSyncRemote, control::CloudSyncOperationControl};
use nyaterm_core::sync_schema::{FIXED_CLOUD_SYNC_SCOPE, encryption};
use nyaterm_core::{CloudSyncError, CloudSyncSettings, LocalCloudSyncOptions};
use nyaterm_store::sync_v4::{LocalReplica, PreparedApply};
use nyaterm_store::{StoreBlockingClient, StoreClientError, StoreDomain};
use nyaterm_sync::engine::{SyncReplica, digest, prepare};
use nyaterm_sync::protocol::{
    MAX_PUBLICATION_BYTES, PendingPublication, ProviderCapabilities, PublicationId,
    PublishedReplica,
};
use uuid::Uuid;
use zeroize::Zeroizing;

const REGISTRY: &str = "sync/v5/registry";
const MAX_OBJECTS: usize = 20_000;

pub(super) struct Preview {
    pub apply: PreparedApply,
    pub migration: bool,
    pub resource_count: usize,
    presentation: PreviewSummary,
    metadata_changed: bool,
}

#[derive(Clone, Default, PartialEq)]
pub(in crate::features) struct PreviewSummary {
    pub resources: usize,
    pub migration: bool,
    pub conflicts: Vec<ConflictSummary>,
    pub deferred: Vec<String>,
}

#[derive(Clone, PartialEq)]
pub(in crate::features) struct ConflictSummary {
    pub index: usize,
    pub field: String,
    pub choices: Vec<String>,
}

impl Preview {
    pub fn summary(&self) -> PreviewSummary {
        self.presentation.clone()
    }

    fn compute_summary(&self) -> PreviewSummary {
        let replica = SyncReplica::load(&self.apply.plan.replica, Uuid::new_v4());
        PreviewSummary {
            resources: self.resource_count,
            migration: self.migration,
            deferred: self.presentation.deferred.clone(),
            conflicts: self
                .apply
                .plan
                .conflicts
                .iter()
                .enumerate()
                .filter(|(_, conflict)| {
                    self.apply
                        .plan
                        .scope
                        .includes(&conflict.field.resource.kind)
                        && nyaterm_core::sync_schema::supports_field(
                            &conflict.field.resource.kind,
                            &conflict.field.field,
                        )
                })
                .map(|(index, conflict)| ConflictSummary {
                    index,
                    field: format!(
                        "{} / {} / {}",
                        conflict.field.resource.kind,
                        conflict.field.resource.id,
                        conflict.field.field
                    ),
                    choices: conflict
                        .candidates
                        .iter()
                        .enumerate()
                        .map(|(index, candidate)| {
                            if candidate.value == "deleted" {
                                return "∅".into();
                            }
                            if nyaterm_core::sync_schema::SECRET_KINDS
                                .contains(&conflict.field.resource.kind.as_str())
                            {
                                return format!("•••• ({})", index + 1);
                            }
                            replica
                                .as_ref()
                                .ok()
                                .and_then(|replica| replica.candidate_value(candidate).ok())
                                .and_then(|value| value.decode().ok())
                                .map(|value| value.to_string().chars().take(160).collect())
                                .unwrap_or_else(|| "?".into())
                        })
                        .collect(),
                })
                .collect(),
        }
    }

    pub fn resolve(
        &mut self,
        store: &StoreBlockingClient,
        password: &str,
        conflict_index: usize,
        candidate_index: usize,
    ) -> Result<(), CloudSyncError> {
        let conflict = self
            .apply
            .plan
            .conflicts
            .get(conflict_index)
            .ok_or_else(invalid)?;
        let candidate = conflict
            .candidates
            .get(candidate_index)
            .ok_or_else(invalid)?;
        let mut replica = SyncReplica::load(&self.apply.plan.replica, self.apply.local.writer)
            .map_err(|_| invalid())?;
        replica
            .resolve(conflict, candidate, &self.apply.plan.scope)
            .map_err(|_| invalid())?;
        self.apply.plan.effective = replica
            .materialize(
                &self.apply.plan.expected_local.values,
                &self.apply.plan.scope,
            )
            .map_err(|_| invalid())?;
        self.apply.plan.conflicts = replica.conflicts().map_err(|_| invalid())?;
        self.apply.plan.replica = replica.snapshot();
        self.refresh_summary(store, password)?;
        Ok(())
    }

    fn refresh_summary(
        &mut self,
        store: &StoreBlockingClient,
        password: &str,
    ) -> Result<(), CloudSyncError> {
        let plan = self.apply.plan.clone();
        let objects = self.apply.local.secret_objects.clone();
        let trust_approvals = self.apply.trust_approvals.clone();
        let password = nyaterm_core::SecretString::from(password);
        let application = store
            .request_fn(StoreDomain::CloudSync, move |store| {
                store.inspect_v4_application_with_secrets(
                    &plan,
                    &objects,
                    password.expose_secret(),
                    &trust_approvals,
                )
            })
            .map_err(store_error)?;
        self.presentation.deferred = application
            .deferred
            .iter()
            .map(|field| {
                format!(
                    "{} / {} / {}",
                    field.resource.kind, field.resource.id, field.field
                )
            })
            .collect();
        self.presentation = self.compute_summary();
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub(crate) struct SyncReport {
    pub conflicts: usize,
    pub deferred: usize,
    pub verified: bool,
    pub publication_error: Option<String>,
}

fn invalid() -> CloudSyncError {
    CloudSyncError::Remote(
        "Invalid or incompatible Cloud Sync V3 data; no data was overwritten".into(),
    )
}
fn store_error(error: StoreClientError) -> CloudSyncError {
    if let StoreClientError::Operation(error) = &error
        && let Some(limit_bytes) = error.local_replica_payload_limit()
    {
        return CloudSyncError::PortableSnapshot(
            nyaterm_core::PortableSnapshotError::LocalReplicaPayloadTooLarge { limit_bytes },
        );
    }
    CloudSyncError::LocalChangedDuringSync
}

fn destination(settings: &CloudSyncSettings, options: &LocalCloudSyncOptions) -> String {
    digest(
        format!(
            "fixed-cloud-sync-v5:{}",
            legacy_destination(settings, options)
        )
        .as_bytes(),
    )
}

fn legacy_destination(settings: &CloudSyncSettings, options: &LocalCloudSyncOptions) -> String {
    // Passwords and rotating access tokens are not destination identities.
    if settings.provider == "s3" {
        return digest(
            serde_json::json!([
                settings.provider,
                options.remote_root,
                settings.s3.endpoint,
                settings.s3.bucket,
                settings.s3.region,
                settings.s3.root,
                settings.s3.virtual_host_style
            ])
            .to_string()
            .as_bytes(),
        );
    }
    let local_directory = (settings.provider == "local_directory").then_some(&options.remote_dir);
    let value = serde_json::json!([
        settings.provider,
        options.remote_root,
        local_directory,
        settings.webdav.endpoint,
        settings.webdav.root,
        settings.webdav.username
    ]);
    digest(value.to_string().as_bytes())
}

fn path(options: &LocalCloudSyncOptions, suffix: &str) -> String {
    nyaterm_core::cloud_sync::remote_path(&options.remote_root, suffix)
}

pub(super) fn preview(
    store: &StoreBlockingClient,
    settings: &CloudSyncSettings,
    options: &LocalCloudSyncOptions,
    control: &CloudSyncOperationControl,
) -> Result<Preview, CloudSyncError> {
    preview_with_import(store, settings, options, control, false)
}

pub(super) fn preview_on_change(
    store: &StoreBlockingClient,
    settings: &CloudSyncSettings,
    options: &LocalCloudSyncOptions,
    remote_available: bool,
    control: &CloudSyncOperationControl,
) -> Result<Option<Preview>, CloudSyncError> {
    control.checkpoint()?;
    let view = local::load(store, settings, options)?;
    let caches =
        super::io_runtime::CloudSyncIoRuntime::shared().caches(store, settings, Some(options))?;
    if !remote_available
        && !caches
            .pending_remote
            .load(std::sync::atomic::Ordering::Acquire)
        && !local::needs_remote(&view, &FIXED_CLOUD_SYNC_SCOPE)?
    {
        control.checkpoint()?;
        return Ok(None);
    }
    validate_provider(settings)?;
    control.checkpoint()?;
    super::cloud_sync_provider::with_remote(store, settings, options, "v4_prepare", |remote, _| {
        let remote = control.remote(remote);
        prepare_remote_with_view(store, settings, options, &remote, false, view)
    })
    .map(Some)
}

pub(super) fn preview_manual(
    store: &StoreBlockingClient,
    settings: &CloudSyncSettings,
    options: &LocalCloudSyncOptions,
    control: &CloudSyncOperationControl,
) -> Result<Preview, CloudSyncError> {
    preview_with_import(store, settings, options, control, true)
}

fn preview_with_import(
    store: &StoreBlockingClient,
    settings: &CloudSyncSettings,
    options: &LocalCloudSyncOptions,
    control: &CloudSyncOperationControl,
    allow_legacy_import: bool,
) -> Result<Preview, CloudSyncError> {
    validate_provider(settings)?;
    super::cloud_sync_provider::with_remote(store, settings, options, "v4_prepare", |remote, _| {
        let remote = control.remote(remote);
        prepare_remote(store, settings, options, &remote, allow_legacy_import)
    })
}

fn validate_provider(settings: &CloudSyncSettings) -> Result<(), CloudSyncError> {
    if !ProviderCapabilities::for_provider(&settings.provider)
        .is_some_and(|capabilities| capabilities.supports_v4())
    {
        return Err(CloudSyncError::Remote(
            "This provider has not yet passed V4 capability validation".into(),
        ));
    }
    Ok(())
}

fn prepare_remote(
    store: &StoreBlockingClient,
    settings: &CloudSyncSettings,
    options: &LocalCloudSyncOptions,
    remote: &dyn CloudSyncRemote,
    allow_legacy_import: bool,
) -> Result<Preview, CloudSyncError> {
    let view = local::load(store, settings, options)?;
    prepare_remote_with_view(store, settings, options, remote, allow_legacy_import, view)
}

fn prepare_remote_with_view(
    store: &StoreBlockingClient,
    settings: &CloudSyncSettings,
    options: &LocalCloudSyncOptions,
    remote: &dyn CloudSyncRemote,
    allow_legacy_import: bool,
    view: nyaterm_store::sync_v4::LocalView,
) -> Result<Preview, CloudSyncError> {
    let destination = destination(settings, options);
    let mut local = view.record;
    let previous_observations = local.observed.clone();
    // All legacy branches are decoded before any mutation; no absent resource
    // becomes a tombstone without an effective local baseline.
    let legacy = nyaterm_core::cloud_sync::read_legacy_sync_branches(store, remote, options)?;
    let mut fingerprints = legacy
        .iter()
        .map(|snapshot| {
            Ok::<_, nyaterm_core::PortableSnapshotError>(snapshot.source_payload_hash().to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let legacy_fields = legacy::read(remote, options)?;
    fingerprints.extend(
        legacy_fields
            .iter()
            .map(|(fingerprint, _)| fingerprint.clone()),
    );
    fingerprints.sort();
    let fingerprint = digest(
        serde_json::to_string(&fingerprints)
            .map_err(|_| invalid())?
            .as_bytes(),
    );
    let legacy_changed = local
        .legacy_fingerprint
        .as_ref()
        .is_some_and(|previous| previous != &fingerprint);
    if legacy_changed && !allow_legacy_import {
        return Err(CloudSyncError::Remote("A legacy client changed this sync space. Automatic V4 sync is paused; an explicit import preview is required".into()));
    }
    let migration = !local.migration_approved || legacy_changed;
    let mut snapshots = Vec::new();
    if migration {
        for snapshot in legacy {
            let projection =
                nyaterm_core::portable_snapshot::project_fixed_sync_entities(&snapshot)?;
            let mut replica = SyncReplica::new(Uuid::new_v4());
            replica
                .capture(&BTreeMap::new(), &projection, &FIXED_CLOUD_SYNC_SCOPE)
                .map_err(|_| invalid())?;
            snapshots.push(replica.snapshot());
        }
    }
    if migration {
        for (_, snapshot) in legacy_fields {
            let mut replica =
                SyncReplica::load(&snapshot, Uuid::new_v4()).map_err(|_| invalid())?;
            snapshots.push(replica.fixed_snapshot().map_err(|_| invalid())?);
        }
    }
    local.legacy_fingerprint = Some(fingerprint);
    let registry = path(options, REGISTRY);
    let mut writers = BTreeSet::new();
    let entries = remote.list_sync_files(&registry)?;
    if entries.len() > MAX_OBJECTS {
        return Err(invalid());
    }
    for entry in entries {
        let name = entry
            .strip_prefix(&format!("{registry}/"))
            .ok_or_else(invalid)?;
        let writer = Uuid::parse_str(name.strip_suffix(".bin").ok_or_else(invalid)?)
            .map_err(|_| invalid())?;
        if name != format!("{writer}.bin") || writer.is_nil() {
            return Err(invalid());
        }
        let registration = remote
            .read_if_exists(&entry)?
            .ok_or(CloudSyncError::StaleRemoteRevision)?;
        if registration != writer.as_bytes() {
            return Err(invalid());
        }
        writers.insert(writer);
    }
    for id in local.observed.values() {
        writers.insert(id.writer);
    }
    if let Some(pending) = &local.pending {
        writers.insert(pending.id.writer);
    }
    let mut identities = BTreeMap::new();
    for id in local.observed.values() {
        identities.insert((id.writer, id.sequence), id.hash.clone());
    }
    let mut objects = 0;
    for writer in writers {
        let prefix = path(options, &format!("sync/v5/writers/{writer}"));
        for entry in remote.list_sync_files(&prefix)? {
            objects += 1;
            if objects > MAX_OBJECTS {
                return Err(invalid());
            }
            let relative = entry
                .strip_prefix(&format!("{}/", options.remote_root.trim_matches('/')))
                .unwrap_or(&entry);
            let id = PublicationId::parse(relative).ok_or_else(invalid)?;
            if id.writer != writer {
                return Err(invalid());
            }
            if identities
                .insert((writer, id.sequence), id.hash.clone())
                .is_some_and(|old| old != id.hash)
            {
                return Err(CloudSyncError::Remote("A V4 writer identity fork was detected. Both branches were retained and publication is stopped".into()));
            }
            if local.observed.contains_key(relative) {
                continue;
            }
            let bytes = remote
                .read_if_exists(&entry)?
                .ok_or(CloudSyncError::StaleRemoteRevision)?;
            if !id.verifies(&bytes) {
                // A failed streaming PUT can leave our reserved name visible
                // with a partial body. The encrypted local pending record is
                // authoritative for this one exact publication; Publish will
                // repair it. Other damaged publications still fail closed.
                if local
                    .pending
                    .as_ref()
                    .is_some_and(|pending| pending.id == id && pending.id.verifies(&pending.bytes))
                {
                    continue;
                }
                return Err(invalid());
            }
            let binding = format!("{}/{}", id.writer, id.sequence);
            let plaintext = encryption::open(
                options.master_password.expose_secret(),
                "publication",
                &binding,
                &bytes,
            )?;
            let published: PublishedReplica =
                serde_json::from_slice(&plaintext).map_err(|_| invalid())?;
            if published.schema != 5
                || published.writer != id.writer
                || published.sequence != id.sequence
            {
                return Err(invalid());
            }
            published.validate_fixed().map_err(|_| invalid())?;
            snapshots.push(published.replica);
            local.observed.insert(relative.into(), id);
        }
    }
    let plan = prepare(
        local.writer,
        local.replica.as_ref(),
        &local.effective,
        view.projection,
        &snapshots,
        FIXED_CLOUD_SYNC_SCOPE,
    )
    .map_err(|_| invalid())?;
    let resource_count = plan
        .effective
        .keys()
        .filter_map(|key| nyaterm_core::sync_schema::SyncFieldId::from_key(key))
        .filter(|field| field.field == "$present")
        .count();
    let mut preview = Preview {
        migration,
        resource_count,
        presentation: PreviewSummary::default(),
        metadata_changed: previous_observations != local.observed,
        apply: PreparedApply {
            destination,
            expected_record: view.record_hash,
            generation: view.generation,
            local,
            plan,
            trust_approvals: BTreeMap::new(),
        },
    };
    preview.refresh_summary(store, options.master_password.expose_secret())?;
    Ok(preview)
}

pub(super) fn apply_and_publish(
    store: &StoreBlockingClient,
    settings: &CloudSyncSettings,
    options: &LocalCloudSyncOptions,
    preview: Preview,
    control: &CloudSyncOperationControl,
) -> Result<SyncReport, CloudSyncError> {
    if preview.apply.destination != destination(settings, options) {
        return Err(CloudSyncError::LocalChangedDuringSync);
    }
    control.checkpoint()?;
    // Confirmation never applies a preview that already missed a publication
    // or a legacy write observed since Prepare. Later concurrent publications
    // remain concurrent and will be merged on the next pass.
    let fresh = preview_with_import(store, settings, options, control, preview.migration)?;
    if fresh.apply.local.observed != preview.apply.local.observed
        || fresh.apply.local.legacy_fingerprint != preview.apply.local.legacy_fingerprint
        || fresh.apply.expected_record != preview.apply.expected_record
        || fresh.apply.generation != preview.apply.generation
    {
        return Err(CloudSyncError::LocalChangedDuringSync);
    }
    let unchanged = !preview.migration
        && !fresh.metadata_changed
        && fresh.apply.local.pending.is_none()
        && fresh.apply.local.scope == fresh.apply.plan.scope
        && fresh.apply.local.effective == fresh.apply.plan.expected_local.values
        && fresh.apply.plan.expected_local.edited.is_empty()
        && fresh.apply.plan.effective == fresh.apply.plan.expected_local.values
        && preview.apply.plan.replica.document == fresh.apply.plan.replica.document
        && preview.apply.plan.replica.values == fresh.apply.plan.replica.values
        && fresh.apply.local.conflicts == fresh.apply.plan.conflicts
        && fresh.apply.local.deferred.is_empty()
        && fresh.apply.local.replica.as_ref().is_some_and(|replica| {
            replica.document == fresh.apply.plan.replica.document
                && replica.values == fresh.apply.plan.replica.values
                && fresh.apply.local.published_document.as_ref() == Some(&digest(&replica.document))
        });
    if unchanged {
        // Both sides have been checked again. No business or causal operation
        // needs committing, so retain the verified encrypted record verbatim.
        control.checkpoint()?;
        return Ok(SyncReport {
            conflicts: fresh.apply.local.conflicts.len(),
            deferred: 0,
            verified: true,
            publication_error: None,
        });
    }
    if preview.migration {
        // Verify the independent .nya safety backup before applying migration.
        let file = options
            .config_dir
            .join(format!("cloud-sync-v3-safety-{}.nya", Uuid::new_v4()));
        let output = file.clone();
        let password = options.master_password.clone();
        store
            .request_fn(StoreDomain::CloudSync, move |store| {
                store.export_v4_safety_backup(&output, password.expose_secret())
            })
            .map_err(store_error)?;
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&file)
            .and_then(|file| file.sync_all())
            .map_err(|_| invalid())?;
        let bytes = std::fs::read(&file).map_err(|_| invalid())?;
        nyaterm_store::decode_encrypted_raw_portable_snapshot(
            &bytes,
            options.master_password.expose_secret(),
        )?;
    }
    control.begin_commit()?;
    let destination = preview.apply.destination.clone();
    let password = options.master_password.clone();
    let record = store
        .request_fn(StoreDomain::CloudSync, move |store| {
            store.apply_v4_plan(preview.apply, password.expose_secret())
        })
        .map_err(store_error)?;
    let conflicts = record.conflicts.len();
    let deferred = record.deferred.len();
    match super::cloud_sync_provider::with_remote(
        store,
        settings,
        options,
        "v4_publish",
        |remote, _| publish(store, options, &destination, record, remote),
    ) {
        Ok(report) => Ok(report),
        Err(error) => Ok(SyncReport {
            conflicts,
            deferred,
            verified: false,
            publication_error: Some(error.to_string()),
        }),
    }
}

fn persist(
    store: &StoreBlockingClient,
    destination: &str,
    password: &str,
    expected: Option<String>,
    record: &LocalReplica,
) -> Result<String, CloudSyncError> {
    let destination = destination.to_owned();
    let password = nyaterm_core::SecretString::from(password);
    let record = record.clone();
    store
        .request_fn(StoreDomain::CloudSync, move |store| {
            store.persist_v4_record(
                &destination,
                password.expose_secret(),
                expected.as_deref(),
                &record,
            )
        })
        .map_err(store_error)
}

fn publish(
    store: &StoreBlockingClient,
    options: &LocalCloudSyncOptions,
    destination: &str,
    mut record: LocalReplica,
    remote: &dyn CloudSyncRemote,
) -> Result<SyncReport, CloudSyncError> {
    let dest = destination.to_owned();
    let password = options.master_password.clone();
    let (durable, receipt) = store
        .request_fn(StoreDomain::CloudSync, move |store| {
            store.load_v4_replica(&dest, password.expose_secret())
        })
        .map_err(store_error)?
        .ok_or(CloudSyncError::LocalChangedDuringSync)?;
    // A later Apply may have committed before this publisher acquired its
    // receipt. Never use that newer CAS token to acknowledge an older replica.
    // Ordinary saved edits remain in the separate journal and are not cleared
    // by a publication acknowledgement.
    if durable != record {
        return Err(CloudSyncError::LocalChangedDuringSync);
    }
    let mut receipt = Some(receipt);
    // The exact old bytes must be retried before reserving a new publication.
    if let Some(pending) = record.pending.clone() {
        publish_bytes(remote, options, &pending)?;
        record.published_document = Some(pending.document_hash);
        record.observed.insert(pending.id.path(), pending.id);
        record.pending = None;
        receipt = Some(persist(
            store,
            destination,
            options.master_password.expose_secret(),
            receipt,
            &record,
        )?);
    }
    let replica = record.replica.clone().ok_or_else(invalid)?;
    let document_hash = digest(&replica.document);
    if record.published_document.as_ref() != Some(&document_hash) {
        let sequence = record.next_sequence;
        record.next_sequence = sequence.checked_add(1).ok_or_else(invalid)?;
        let publication =
            PublishedReplica::fixed(record.writer, sequence, &replica).map_err(|_| invalid())?;
        let plaintext = Zeroizing::new(serde_json::to_vec(&publication).map_err(|_| invalid())?);
        if plaintext.len() > MAX_PUBLICATION_BYTES / 2 {
            return Err(invalid());
        }
        let bytes = encryption::seal(
            options.master_password.expose_secret(),
            "publication",
            &format!("{}/{}", record.writer, sequence),
            &plaintext,
        )?;
        record.pending = Some(
            PendingPublication::new(record.writer, sequence, bytes, document_hash)
                .ok_or_else(invalid)?,
        );
        receipt = Some(persist(
            store,
            destination,
            options.master_password.expose_secret(),
            receipt,
            &record,
        )?);
        let pending = record.pending.as_ref().ok_or_else(invalid)?;
        publish_bytes(remote, options, pending)?;
        record.published_document = Some(pending.document_hash.clone());
        record
            .observed
            .insert(pending.id.path(), pending.id.clone());
        record.pending = None;
    }
    record.migration_approved = true;
    persist(
        store,
        destination,
        options.master_password.expose_secret(),
        receipt,
        &record,
    )?;
    Ok(SyncReport {
        conflicts: record.conflicts.len(),
        deferred: record.deferred.len(),
        verified: true,
        publication_error: None,
    })
}

fn publish_bytes(
    remote: &dyn CloudSyncRemote,
    options: &LocalCloudSyncOptions,
    pending: &PendingPublication,
) -> Result<(), CloudSyncError> {
    if !pending.id.verifies(&pending.bytes) {
        return Err(invalid());
    }
    let plaintext = encryption::open(
        options.master_password.expose_secret(),
        "publication",
        &format!("{}/{}", pending.id.writer, pending.id.sequence),
        &pending.bytes,
    )?;
    let publication: PublishedReplica =
        serde_json::from_slice(&plaintext).map_err(|_| invalid())?;
    publication.validate_fixed().map_err(|_| invalid())?;
    if publication.writer != pending.id.writer || publication.sequence != pending.id.sequence {
        return Err(invalid());
    }
    let writer_dir = path(options, &format!("sync/v5/writers/{}", pending.id.writer));
    for suffix in ["sync", "sync/v5", REGISTRY, "sync/v5/writers"] {
        remote.create_dir(&path(options, suffix))?;
    }
    remote.create_dir(&writer_dir)?;
    let object = path(options, &pending.id.path());
    if let Some(existing) = remote.read_if_exists(&object)? {
        if existing != pending.bytes {
            if pending.id.verifies(&existing) {
                return Err(invalid());
            }
            // This path names the digest of the durably reserved bytes. A
            // different body cannot be a valid object at this identity. Repair
            // only our pending object, then perform the ordinary full readback.
            remote.write(&object, &pending.bytes)?;
        }
    } else {
        remote.write_sync_file(&object, &pending.bytes)?;
    }
    if remote.read_if_exists(&object)?.as_deref() != Some(pending.bytes.as_slice()) {
        return Err(CloudSyncError::StaleRemoteRevision);
    }
    let registry = path(options, &format!("{REGISTRY}/{}.bin", pending.id.writer));
    remote.write_sync_file(&registry, pending.id.writer.as_bytes())?;
    if remote.read_if_exists(&registry)?.as_deref() != Some(pending.id.writer.as_bytes().as_slice())
    {
        return Err(CloudSyncError::StaleRemoteRevision);
    }
    Ok(())
}

#[cfg(test)]
mod change_tests;
#[cfg(test)]
mod tests;
