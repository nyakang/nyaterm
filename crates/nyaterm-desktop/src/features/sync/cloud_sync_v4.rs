//! Blocking V4 orchestration. Prepare is read-only; the Store owns atomic Apply.
mod legacy;
mod local;
mod rotation;
pub(in crate::features) use rotation::rotate_master_password;
use std::collections::{BTreeMap, BTreeSet};

use nyaterm_core::cloud_sync::{CloudSyncRemote, control::CloudSyncOperationControl};
use nyaterm_core::sync_schema::{FIXED_CLOUD_SYNC_SCOPE, SyncFieldId, encryption};
use nyaterm_core::{CloudSyncError, CloudSyncSettings, LocalCloudSyncOptions};
use nyaterm_store::sync_v4::{LocalReplica, PreparedApply};
use nyaterm_store::{StoreBlockingClient, StoreClientError, StoreDomain};
use nyaterm_sync::engine::{SyncReplica, digest, prepare};
use nyaterm_sync::protocol::{
    MAX_PUBLICATION_BYTES, PendingPublication, ProviderCapabilities, PublicationId,
    PublishedReplica,
};
use rust_i18n::t;
use uuid::Uuid;
use zeroize::Zeroizing;

const REGISTRY: &str = "sync/v5/registry";
const MAX_OBJECTS: usize = 20_000;

pub(super) struct Preview {
    pub apply: PreparedApply,
    pub migration: bool,
    pub resource_count: usize,
    /// Set once the user staged a field decision. Such a preview cannot be
    /// re-based onto a newer local snapshot, because the decisions name
    /// positions in the conflict list the user reviewed.
    resolved: bool,
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
    pub candidate_ids: Vec<String>,
    pub operation_ids: Vec<String>,
    /// Candidate position matching the local baseline, if one can be proven.
    /// Candidates are unordered, so "keep local" must be computed, not guessed.
    pub local_candidate: Option<usize>,
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
                .map(|(index, conflict)| {
                    let expected = self
                        .apply
                        .plan
                        .expected_local
                        .values
                        .get(&conflict.field.key());
                    let local_candidate = conflict.candidates.iter().position(|candidate| {
                        if candidate.value == "deleted" {
                            return expected.is_none();
                        }
                        let Some(expected) = expected else {
                            return false;
                        };
                        let Ok(expected) = expected.decode() else {
                            return false;
                        };
                        replica
                            .as_ref()
                            .ok()
                            .and_then(|replica| replica.candidate_value(candidate).ok())
                            .and_then(|value| value.decode().ok())
                            .is_some_and(|value| value == expected)
                    });
                    ConflictSummary {
                        index,
                        field: format!(
                            "{} / {} / {}",
                            conflict.field.resource.kind,
                            conflict.field.resource.id,
                            conflict.field.field
                        ),
                        candidate_ids: conflict
                            .candidates
                            .iter()
                            .map(|candidate| candidate.value.clone())
                            .collect(),
                        operation_ids: conflict
                            .candidates
                            .iter()
                            .map(|candidate| candidate.operation.clone())
                            .collect(),
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
                                    .map(|value| value.to_string())
                                    .unwrap_or_else(|| "?".into())
                            })
                            .collect(),
                        local_candidate,
                    }
                })
                .collect(),
        }
    }

    /// Resolve several conflicts against the pre-batch conflict list in one
    /// pass, then materialize and refresh the summary exactly once. Decisions
    /// reference the current `plan.conflicts` indices; duplicates of the same
    /// conflict are dropped because the first write settles that field.
    pub fn resolve_batch(
        &mut self,
        store: &StoreBlockingClient,
        password: &str,
        decisions: &[(usize, usize)],
    ) -> Result<usize, CloudSyncError> {
        let conflicts = self.apply.plan.conflicts.clone();
        let mut replica = SyncReplica::load(&self.apply.plan.replica, self.apply.local.writer)
            .map_err(|_| invalid())?;
        let mut seen = BTreeSet::new();
        let mut resolved = 0usize;
        for (conflict_index, candidate_index) in decisions {
            if !seen.insert(*conflict_index) {
                continue;
            }
            let conflict = conflicts.get(*conflict_index).ok_or_else(invalid)?;
            let candidate = conflict
                .candidates
                .get(*candidate_index)
                .ok_or_else(invalid)?;
            replica
                .resolve(conflict, candidate, &self.apply.plan.scope)
                .map_err(|_| invalid())?;
            resolved += 1;
        }
        if resolved == 0 {
            return Ok(0);
        }
        self.apply.plan.effective = replica
            .materialize(
                &self.apply.plan.expected_local.values,
                &self.apply.plan.scope,
            )
            .map_err(|_| invalid())?;
        self.apply.plan.conflicts = replica.conflicts().map_err(|_| invalid())?;
        self.apply.plan.replica = replica.snapshot();
        self.refresh_summary(store, password)?;
        self.resolved = true;
        Ok(resolved)
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
    pub deferred: Vec<String>,
    /// Records whose remote address change was not adopted because a password
    /// of this device is bound to the local address.
    pub kept: Vec<String>,
    pub verified: bool,
    /// A publication actually reached the remote in this pass. False for the
    /// verified no-op fast path, whose only news is that both sides match.
    pub published: bool,
    pub publication_error: Option<String>,
}

impl SyncReport {
    /// The single user-visible sentence for a finished apply. Deferred fields
    /// are named instead of only counted: the count alone leaves the user with
    /// nothing to repair, which is how a permanently deferred field looks
    /// indistinguishable from a healthy sync.
    pub(crate) fn status_message(&self) -> String {
        let mut message = if self.verified && (self.conflicts > 0 || !self.deferred.is_empty()) {
            t!(
                "settings.syncV4NeedsAttention",
                conflicts = self.conflicts,
                deferred = self.deferred.len()
            )
            .to_string()
        } else if self.verified {
            t!("settings.syncV3Verified", count = self.conflicts).to_string()
        } else {
            t!(
                "settings.syncV3Pending",
                detail = self.publication_error.clone().unwrap_or_default()
            )
            .to_string()
        };
        if !self.deferred.is_empty() {
            message.push_str(&format!(
                " · {}",
                t!("settings.syncV3Deferred", count = self.deferred.len())
            ));
            message.push_str(&format!(
                " {}",
                t!(
                    "settings.syncV3DeferredFields",
                    fields = summarize_fields(&self.deferred, STATUS_FIELD_LIMIT)
                )
            ));
        }
        if !self.kept.is_empty() {
            message.push_str(&format!(
                " · {}",
                t!(
                    "settings.syncV3KeptEndpoints",
                    entries = summarize_fields(&self.kept, STATUS_FIELD_LIMIT)
                )
            ));
        }
        message
    }

    /// The history-list counterpart of `status_message`. Every manual apply the
    /// user finishes must appear here: a finished sync whose only trace is the
    /// status bar looks, from the panel, like "no conflicts and no success".
    pub(crate) fn history_entry(
        &self,
        trigger: &str,
        provider: Option<String>,
        duration_ms: Option<u64>,
    ) -> nyaterm_core::CloudSyncHistoryEntry {
        let (status, message) =
            if self.verified && (self.conflicts > 0 || !self.deferred.is_empty()) {
                (
                    "conflict",
                    format!(
                        "Cloud synchronization verified with {} conflicts and {} unapplied fields",
                        self.conflicts,
                        self.deferred.len()
                    ),
                )
            } else if self.verified {
                (
                    "success",
                    if self.published {
                        "Cloud synchronization applied and verified".to_string()
                    } else {
                        "Cloud synchronization is already up to date".to_string()
                    },
                )
            } else {
                (
                    "failed",
                    format!(
                        "Cloud synchronization could not be verified: {}",
                        self.publication_error.as_deref().unwrap_or("unknown")
                    ),
                )
            };
        let mut entry =
            nyaterm_core::CloudSyncHistoryEntry::sync(status, trigger, provider, None, message);
        entry.duration_ms = duration_ms;
        entry
    }
}

/// Enough identities to find the record, without turning the status bar into a
/// wall of text when a whole catalog is deferred.
const STATUS_FIELD_LIMIT: usize = 3;

fn summarize_fields(fields: &[String], limit: usize) -> String {
    let mut shown = fields
        .iter()
        .take(limit)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if fields.len() > limit {
        shown.push_str(&format!(" … (+{})", fields.len() - limit));
    }
    shown
}

fn deferred_fields(fields: &[SyncFieldId]) -> Vec<String> {
    fields
        .iter()
        .map(|field| {
            format!(
                "{} / {} / {}",
                field.resource.kind, field.resource.id, field.field
            )
        })
        .collect()
}

fn invalid() -> CloudSyncError {
    CloudSyncError::Remote(
        "Invalid or incompatible Cloud Sync V3 data; no data was overwritten".into(),
    )
}
fn store_error(error: StoreClientError) -> CloudSyncError {
    let StoreClientError::Operation(error) = &error else {
        return CloudSyncError::LocalChangedDuringSync;
    };
    if let Some(limit_bytes) = error.local_replica_payload_limit() {
        return CloudSyncError::PortableSnapshot(
            nyaterm_core::PortableSnapshotError::LocalReplicaPayloadTooLarge { limit_bytes },
        );
    }
    // Only a real compare-and-swap loss is "the preview went stale". Anything
    // else (unreadable or unsupported local data, a missing key, a transient
    // storage failure) used to be reported as `local_changed`, which hid the
    // actual cause and made an unrecoverable condition look like a retry race.
    if error.category() == "sync_local_changed" {
        return CloudSyncError::LocalChangedDuringSync;
    }
    CloudSyncError::LocalStore(error.category().to_string())
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
    .map(|preview| {
        record_check(store);
        Some(preview)
    })
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
    .inspect(|_| record_check(store))
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

/// Persists that this device has just talked to the remote replica. The V3
/// flows update these timestamps inside the engine; the V4 flows own Apply
/// themselves, so the settings panel would otherwise show "上次同步: 从未"
/// forever even after verified publications.
fn record_check(store: &StoreBlockingClient) {
    touch_state(store, false);
}

/// A publication reached the remote and was verified: counts as a sync, not
/// just a check.
fn record_sync(store: &StoreBlockingClient) {
    touch_state(store, true);
}

fn touch_state(store: &StoreBlockingClient, synced: bool) {
    let _ = store.request_fn(StoreDomain::CloudSync, move |store| {
        let mut state = store.load_cloud_sync_state()?;
        let now = current_time_ms();
        state.last_checked_at_ms = Some(now);
        if synced {
            state.last_synced_at_ms = Some(now);
        }
        store.save_cloud_sync_state(&state)
    });
}

fn current_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
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
        resolved: false,
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
    mut preview: Preview,
    control: &CloudSyncOperationControl,
) -> Result<SyncReport, CloudSyncError> {
    if preview.apply.destination != destination(settings, options) {
        return Err(CloudSyncError::LocalChangedDuringSync);
    }
    control.checkpoint()?;
    // Confirmation never applies a preview that already missed a publication
    // or a legacy write observed since Prepare. Later concurrent publications
    // remain concurrent and will be merged on the next pass.
    let verification = preview_with_import(store, settings, options, control, preview.migration)?;
    if verification.apply.local.observed != preview.apply.local.observed
        || verification.apply.local.legacy_fingerprint != preview.apply.local.legacy_fingerprint
    {
        return Err(CloudSyncError::LocalChangedDuringSync);
    }
    if verification.apply.expected_record != preview.apply.expected_record
        || verification.apply.generation != preview.apply.generation
    {
        if preview.resolved {
            // The staged decisions name positions in the conflict list the user
            // reviewed; that list belongs to the older snapshot.
            return Err(CloudSyncError::LocalChangedDuringSync);
        }
        // Only the local side moved, and no user decision depends on the old
        // snapshot. Re-base onto the newer merge instead of failing: a routine
        // edit that races the sync used to surface as a failure that backed off
        // for minutes and re-downloaded the whole replica on every attempt.
        preview = verification;
    }
    let unchanged = !preview.migration
        && !preview.metadata_changed
        && !preview.resolved
        && preview.apply.local.pending.is_none()
        && preview.apply.local.scope == preview.apply.plan.scope
        && preview.apply.local.effective == preview.apply.plan.expected_local.values
        && preview.apply.plan.expected_local.edited.is_empty()
        && preview.apply.plan.effective == preview.apply.plan.expected_local.values
        && preview.apply.local.conflicts == preview.apply.plan.conflicts
        && preview.apply.local.deferred.is_empty()
        && preview.apply.local.replica.as_ref().is_some_and(|replica| {
            replica.document == preview.apply.plan.replica.document
                && replica.values == preview.apply.plan.replica.values
                && preview.apply.local.published_document.as_ref()
                    == Some(&digest(&replica.document))
        });
    if unchanged {
        // Both sides have been checked again. No business or causal operation
        // needs committing, so retain the verified encrypted record verbatim.
        control.checkpoint()?;
        // This pass read the remote replica and proved both sides identical:
        // from the settings panel's point of view that is a completed sync,
        // not merely a check, so "上次同步" must move off "从未".
        record_sync(store);
        return Ok(SyncReport {
            conflicts: preview.apply.local.conflicts.len(),
            deferred: deferred_fields(&preview.apply.local.deferred),
            // A retained address was already re-published by the apply that
            // recorded it: replaying the note here would make it permanent.
            kept: Vec::new(),
            verified: true,
            published: false,
            publication_error: None,
        });
    }
    if preview.migration {
        // Verify the independent .nya safety backup before applying migration.
        let file = options
            .config_dir
            .join(format!("cloud-sync-v3-safety-{}.nya", Uuid::new_v4()));
        store
            .export_verified_v4_safety_backup(file, options.master_password.clone())
            .map_err(|error| match error {
                nyaterm_store::safety_backup::VerifiedBackupError::Export(error) => {
                    store_error(error)
                }
                nyaterm_store::safety_backup::VerifiedBackupError::Decode(error) => {
                    CloudSyncError::PortableSnapshot(error)
                }
                nyaterm_store::safety_backup::VerifiedBackupError::Sync(_)
                | nyaterm_store::safety_backup::VerifiedBackupError::Read(_) => invalid(),
            })?;
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
    let deferred = deferred_fields(&record.deferred);
    let kept = record.kept.clone();
    if !deferred.is_empty() {
        // Field identities only: no values ever reach the log.
        tracing::warn!(
            count = deferred.len(),
            "Cloud sync kept local values for fields that cannot be applied"
        );
    }
    if !kept.is_empty() {
        tracing::warn!(
            count = kept.len(),
            "Cloud sync kept this device's address because a saved password is bound to it"
        );
    }
    match super::cloud_sync_provider::with_remote(
        store,
        settings,
        options,
        "v4_publish",
        |remote, _| publish(store, options, &destination, record, remote),
    ) {
        Ok(report) => {
            // The verification above already recorded the check; a successful
            // publication also counts as a sync.
            record_sync(store);
            Ok(report)
        }
        Err(error) => Ok(SyncReport {
            conflicts,
            deferred,
            kept,
            verified: false,
            published: false,
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
        deferred: deferred_fields(&record.deferred),
        kept: record.kept.clone(),
        verified: true,
        published: true,
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
