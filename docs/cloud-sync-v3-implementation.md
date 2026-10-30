# Cloud Sync V3 implementation record

Product V3 uses wire namespace V4. This change introduces the schema, Automerge engine, transactional application and a Local/WebDAV/S3 preview. It is not the completion of every stage in the approved seven-stage rollout.

## Ownership and durable boundaries

- Core owns field allowlists and V4 purpose-separated encryption. `sync_schema/fields.json` classifies persisted connection, network, command and note fields; the architecture check rejects new unclassified fields. Proxy and tunnel endpoints are atomic groups. Backup codecs remain unchanged.
- `nyaterm-sync` owns pure causal operations, candidate validation, materialization and publication identities. It depends on no UI, database or HTTP implementation.
- Store journals portable field identities in the same transaction as domain writes, including edits later reverted to the baseline value. It separately persists an encrypted causal document, actual typed business baseline, applied generation, pending publication bytes and remote publication receipts.
- Desktop performs blocking discovery, migration backup verification, plan preparation and provider I/O. GPUI receives immutable presentation summaries; replica decoding occurs on the worker.

Tables `sync_v4_field_edits` and `sync_v4_replicas` are additive. Existing table names, keys, portable formats, credential wrappers and fallback readers are retained. No external key-file mutation occurs during V4 Apply; all business changes and replica state commit in one Redb transaction. Backup output is verified before migration Apply.

Prepare uses raw read-only catalogs, avoiding the legacy session reader's icon/credential migrations. The migration safety exporter preserves encrypted credential slots in the existing `.nya` format without migrating source data; the existing importer restores them under a different local master password. Apply overlays portable connection/proxy/tunnel fields onto transaction-local records and retains excluded credential slots byte for byte. Its baseline is projected from actual persisted session records, including untouched legacy records that omit serde defaults.

## Publication and discovery

Publication objects are `sync/v4/writers/<uuid>/<sequence>-<ciphertext-sha256>.bin`. The complete causal document is encrypted using the existing Argon2id/AES-GCM envelope under the V4 domain and authenticated publication identity. The pending sequence, path and exact bytes are persisted before network writes. An interrupted streaming upload may leave a partial body at the reserved digest path. Prepare retains that exact locally owned pending publication without treating its damaged remote body as applied; Publish repairs it using the reserved bytes and performs full readback. Damaged peer publications still fail closed. Verification precedes acknowledgement. Upload acknowledgement never advances the applied business generation. Before I/O, Publish compares its complete record with the actual persisted replica; an older task cannot acquire a newer Apply's CAS token and overwrite that newer replica. This receipt read does not bind a writer or observe newly edited credential values.

Because existing provider APIs enumerate immediate files, writers are discovered through immutable `sync/v4/registry/<uuid>.bin` registrations containing their UUID bytes. These records are discovery hints, not an authoritative mutable pointer. A registration is written only after publication readback. Previously observed publications remain in the local replica when a listing is delayed. Different hashes at one writer/sequence stop publication. No V4 publication, tombstone or causal history is automatically deleted in this preview. Listings and object sizes have explicit bounds.

Each process/database binding gets a new writer on restart or copied configuration. An old pending publication retains its original identity and ciphertext for retry. Portable backups omit all replica tables and identities.

Local V4 writes flush a staging file before atomically installing a hard link without replacement. Interrupted staging files are outside publication listings; retrying an existing different object fails. WebDAV requires a complete multistatus document and rejects failed child statuses. S3 traverses all pages, rejects missing/repeated continuation tokens, malformed/error documents and paths outside the requested prefix. Drive parsers reject missing/partial lists, malformed entries and invalid continuation types. Their listing loops bound pages/entries and reject duplicate objects or cursor cycles. OneDrive continuation requests must retain the original HTTPS origin and path; a later-page 404 is an error, not an empty space. S3 capability validation currently uses a local HTTP fixture; real service validation is still pending.

## Current delivery boundary

| Stage | Delivered | Remaining acceptance work |
| --- | --- | --- |
| 1 | Explicit core-resource projection and field classification guard; preference/shortcut/AI allowlists; local usage/path/monitoring/discovery exclusion; unknown register preservation; preference, AI, vault and trust field classifications; independent encrypted credential objects | Additional nested security classification |
| 2 | Automerge 0.12.0; independent-field merge; conflicts; field and resource tombstones; unique presence writes; explicit candidate resolution | Long-offline scale coverage; eight deterministic partition/reordering/duplicate schedules are covered |
| 3 | Same-transaction field journal; encrypted CAS replica; actual application baseline; atomic business Apply; real process-exit recovery at eight Apply boundaries; competing-preview and stale-publisher guards | More cross-window/cancellation schedules; encrypted journals if external key operations are introduced |
| 4 | Local/WebDAV coordinator; atomic Local publication; immutable ciphertext retry; readback; identity fork rejection; two-device and delayed-listing fixtures; WsgiDAV server interoperability | Authenticated/TLS WebDAV service matrix and more fault schedules |
| 5 | All readable legacy branches imported without inferred deletion; read-only encrypted verified safety backup; legacy write detection and manual import preview; unchanged `.nya` codec; credential-object migration; verified password-space rotation | Additional historical provider fixtures and native rotation interaction |
| 6 | Preview confirmation; per-field candidate choice; appearance, shortcut, AI, sensitive-data and individually confirmed trust scopes; partial application of valid groups; deferred-field presentation; applied/pending/verified status; safe automatic routing | Native interaction verification |
| 7 | Local/WebDAV/S3 capability gate; bounded pagination; no unsafe cleanup; synthetic and persisted 1,000/5,000/10,000-resource measurements | Gist/drive contracts and activation; verified successor cleanup; production latency acceptance |

Unsupported fields and providers fail closed. Gitee Snippet retains the legacy entry point. V4 never downgrades to legacy publication after an error. Typed network records, group/category parents, proxy jumps, tunnel references and authentication identities are validated before committing. Invalid groups retain local values, while other valid changes apply. Creation and deletion remain atomic for the resource. Deferred dependencies are retried against the accepted changes; the causal document is never rewritten to discard deferred candidates. An already unreadable local catalog fails closed. Credential identity validation reads keys without decoding or decrypting excluded values. Ordinary vault deletions invalidate outstanding previews in the same transaction.

The optional AI scope preserves local secret ciphertext and unknown fields. API keys and headers use the independent sensitive-data scope. The nonsecret scope excludes proxy settings, execution permissions, external-tool paths, discovered models and discovery metadata. Nonsecret deletion or transport changes cannot remove or redirect a local key/header; affected groups wait for a local edit. Initial default AI resources do not conflict with remote customizations on first join. AI model and default-model references are validated before Apply.

Future envelope metadata is rejected explicitly; unknown causal business fields remain preserved and cannot be applied by an unsupported client.

## Reproduction

Run `cargo test -p nyaterm-sync`, `cargo test -p nyaterm-core --lib sync_schema`, `cargo test -p nyaterm-store`, and `cargo test -p nyaterm-desktop cloud_sync_v4 --lib` for the focused checks. `cargo run -p nyaterm-sync --example scale` measures synthetic 1,000/5,000/10,000-resource creation and unchanged checks and asserts that unchanged checks produce identical causal documents.

Before review run workspace check/test/fmt/clippy and `python scripts/ci/check_architecture.py`. Windows native interaction and real-provider verification must be recorded separately; passing unit fixtures is not evidence of those checks.

## Synthetic performance record

Windows release build, engine-only fixture, after eliminating repeated register queries and unchanged capture work, measured alongside other checks:

| Resources | Initial prepare (ms) | Unchanged prepare (ms) | Compressed document bytes |
| --- | ---: | ---: | ---: |
| 1,000 | 266 | 116 | 187,707 |
| 5,000 | 1,504 | 493 | 939,749 |
| 10,000 | 2,587 | 993 | 1,879,142 |

Every unchanged check retained identical causal document bytes. This measures neither persisted Store throughput nor Provider transfer time. Unchanged-check latency still needs optimization before claiming production scale acceptance.

The explicit persisted workload is `cargo test -p nyaterm-desktop persisted_local_provider_scale_and_unchanged_checks --lib -- --ignored --nocapture`. It uses synthetic databases and the actual Local Provider coordinator for 1,000/5,000/10,000 resources, verifies second-device application and asserts that unchanged checks create no publication objects. It is ignored in the ordinary suite because of its runtime.

`process_exit_at_each_apply_boundary_recovers_all_business_data_and_replica_or_neither` starts a fresh test subprocess for each transaction boundary and exits without running destructors. On reopening the database it checks both business collections and the causal record. This is a process-termination fixture on Windows, not a physical power-loss test.

Netcatty and OxideTerm were studied for comparison. Their GPL implementation code is not copied into this change.

## Sensitive data, trust and password rotation

The sensitive scope is off by default. Password/key/credential/OTP values, inline connection and proxy passwords, and AI keys/headers use independent authenticated value objects. CRDT registers contain keyed version references. Store Prepare is read-only; Apply re-encrypts received values under the local master key and commits vault, wrapped key, business records and causal baseline in one transaction. Same-version local ciphertext and unknown local fields survive unchanged. Legacy cloud credentials, including pre-2.0 plaintext AI fields, are converted read-only. Excluded embedded secrets prevent remote parent deletion or credential redirection. Ordinary proxy passwords now have an explicit encrypted storage marker; unmarked legacy values remain readable and released `.nya` exports preserve their password field contract.

SSH/VNC identities are atomic values. Each received addition/change/deletion requires transient local approval bound to both exact identities. A changed candidate or local record invalidates approval. Automatic synchronization defers trust changes while applying other valid fields. Trust approvals are neither backed up nor published.

Master-password edits with V4 replicas use a durable encrypted staging table, `sync_v4_password_rotations`. The coordinator publishes a full replica under a fresh `rotation-<uuid>` subspace and reads back both publication and registration before a CAS transaction switches password, wrapped vault key, root and encrypted local replicas. Old cloud objects remain untouched. Staging retains exact ciphertext across restart; a concurrent saved edit invalidates the switch and retains the old password/root. Historical secret references survive rewrapping through a separately authenticated encrypted reference key. No external key file changes are needed. A staged rotation does not change business settings and therefore requires no startup rollback; the next matching request resumes it. Direct password removal/change is blocked while V4 replicas exist. Account/scope edits must be saved separately. Other devices join the new namespace explicitly.

## Persisted performance record

Windows Debug build, actual Store plus Local Provider; measurements began before batch connection-index cleanup was optimized:

| Resources | Initial application (ms) | Unchanged check (ms) | Second-device application (ms) |
| --- | ---: | ---: | ---: |
| 1,000 | 39,601 | 12,989 | 44,961 |
| 5,000 | 180,131 | 49,171 | 207,332 |
| 10,000 | 292,293 | 76,399 | 452,492 |

All consistency and no-new-publication assertions passed. These timings do not meet production latency acceptance. The batch index cleanup change is covered by a legacy-index compatibility test. Engine-only release timings above must not be compared directly with these Debug end-to-end measurements.

The persisted workload was also rerun after batching index cleanup. It used a separate copy of the test executable, captured before the final publication receipt-reader change. Workspace compilation and tests ran concurrently, so these are measurements under contention, not an isolated before/after performance comparison:

| Resources | Initial application (ms) | Unchanged check (ms) | Second-device application (ms) |
| --- | ---: | ---: | ---: |
| 1,000 | 37,020 | 9,222 | 31,727 |
| 5,000 | 158,588 | 49,580 | 204,847 |
| 10,000 | 408,295 | 107,109 | 281,986 |

The complete workload passed in 1,299.27 seconds. Every resource reached the second device, and unchanged checks created no additional publication objects. The final receipt-reader change is covered by the stale-publisher regression and the complete workspace suite; the scale workload has not been repeated for that later change. Persisted no-change latency remains an explicit release limitation.

## Native verification boundary

On Windows, the current application was built and launched as the uniquely named `nyaterm-sync-v3-fixture.exe` using a portable synthetic configuration under `target/cloud-sync-v3-ui`. The returned window belonged to that executable. The input API failed with `GetCursorPos failed: Access denied (0x80070005)` after an activation failure. No settings interaction, password change or scope confirmation was completed through the native UI. The isolated process was stopped; the installed application and real user data were not modified. Native interaction acceptance remains outstanding.

## WebDAV interoperability record

On Windows, `webdav_server_merges_two_devices_and_keeps_unchanged_publications` passed against a separate WsgiDAV 4.4.0 / Cheroot 11.1.2 server on loopback HTTP, using anonymous authentication and a fresh random provider root. It exercises the actual native HTTP adapter and coordinator, two separate databases, verified first publication, second-device join, independent offline edits, stale-preview rejection, field merge and unchanged checks. The initial passing run took 32.17 seconds; the final run after the publication CAS change also passed in 37.82 seconds. The fixture server was then stopped.

This run found that MKCOL started below an absent configured provider root and incorrectly accepted HTTP 409. Directory creation now starts at that root and proceeds one segment at a time; 409 propagates as failure. Three ordinary HTTP regression tests cover a missing root, existing parents and the failure case. This is server interoperability evidence, not TLS, authenticated service or hosted-provider acceptance. No real cloud account or user configuration was used.

## Final automated verification

On Windows, after the publication CAS fix, all of the following completed successfully:

- `cargo check --workspace`
- `cargo test --workspace`: 4,727 passed, 0 failed, 35 ignored (including manual/native/scale fixtures)
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets`: no warnings
- `python scripts/ci/check_architecture.py`

The ignored WebDAV interoperability and persisted scale fixtures were run explicitly as recorded above. No commits or pushes were created. The native interaction and remaining provider/cleanup/latency acceptance items in the delivery table remain open.
