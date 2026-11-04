# OpenDAL cloud sync migration

## Source audit (2026-11-04)

Base: origin/main, 9fed0698c. Worktree: ../nyaterm-opendal; branch:
feat/opendal-cloud-sync. The original index and working files are untouched.

The current source has a separate nyaterm-sync crate for V4 merge/publication
contracts. The synchronous CloudSyncRemote remains in nyaterm-core. Desktop
blocking jobs construct remotes per operation, inside the existing operation
registration/lock. GPUI only receives job results. CloudSyncOperationControl
arbitrates Preparing -> Cancelled/Committing; committing is not interrupted.

The desktop provider facade reloads credentials and checks their identity before
constructing a remote. Token sinks persist rotated credentials through the Store
actor and reject superseded accounts. ObservedRemote and CachedMetadataRemote
wrap the backend, with process-owned caches keyed by database/account/password/
root. V3 publishes verified encrypted snapshots, causal heads and pointers. V4
publishes immutable writer objects, verifies read-back, then registers the writer;
Store retains pending publications for recovery. None of these contracts should
move into OpenDAL.

The Tauri reference used OpenDAL 0.58, timeout/retry/tracing, a custom Digest
transport, and gist adapters. Its generic retries and error formatting are not
suitable for the stronger GPUI commit, credential and diagnostic contracts.
The published version selected here is 0.59.5 (Rust >=1.91). Its blocking API
captures a runtime handle and calls block_on: it cannot safely be called from an
arbitrary existing runtime. Use a joinable, operation-owned worker with one
current-thread runtime instead. A bounded mailbox prevents detached requests;
dropping the remote closes the mailbox and joins the worker. The worker also
owns, constructs and drops the retained blocking directory clients outside
block_on, avoiding nested-runtime creation/destruction. Its compatibility HTTP
requests bind to the same statistics scope as OpenDAL requests. Cancellation
remains at the existing protocol checkpoints, including the non-interruptible
commit.

## Risk matrix

| Area | Risk | Migration gate |
| --- | --- | --- |
| WebDAV | Digest, NAS collection slashes, redirects losing methods/bodies or credentials | Reuse the native redirect policy and Digest signer in an explicit HTTP transport; local HTTP fixtures |
| TLS | Losing Windows/system roots or disabling certificate validation | Reuse zed-reqwest with rustls native roots; never accept invalid certificates |
| S3 | Lost ETag/304 optimization, bad pagination interpreted as deletion | Explicit conditional reads; retain strict list-page validation, prefix, token and capacity guards |
| Publication | Retry of an ambiguous mutation, overwriting immutable bytes | No automatic mutation retry; explicit immutable checks and read-back; keep protocol commit/recovery |
| OAuth | Rotated refresh token retained only inside OpenDAL | Keep each Native backend until its Store token sink can be guaranteed |
| Diagnostics | OpenDAL errors/spans expose URL, root, body or credentials | Controlled classifications and fixed operation labels; no raw OpenDAL Debug/Display in user diagnostics |
| Compatibility | Provider root applied twice, snapshot/path/encryption drift | Preserve provider-root + remote-root composition and cross-backend tests |

Implementation and validation results are recorded below as each gate is checked.


## Implemented architecture and provider status

Production call chain:

`GPUI background job -> existing operation control/credential identity checks
-> provider facade -> ObservedRemote -> CachedMetadataRemote -> OpenDalRemote
-> bounded joined worker -> OpenDAL Operator / fixed compatibility adapter
-> zed-reqwest HTTP transport -> remote service`.

The synchronous trait and every explicit method remain available. No serialized
settings, Store schema, encryption, master-password wrapping, account identity,
V3/V4 wire document or published object name changes. Provider root is applied
once, before the unchanged protocol remote root. There is no user-visible backend
switch and no error-triggered fallback to a second writer.

| Provider/capability | Production implementation | Reason for retained code |
| --- | --- | --- |
| WebDAV object GET/conditional GET/PUT/DELETE | OpenDAL WebDAV 0.59.5 | Custom HTTP transport preserves Digest, safe redirects, native TLS roots, byte statistics and sanitized failures |
| WebDAV create_dir | Native recursive MKCOL on the joined worker | Preserve NAS collection slash and existing-directory semantics without a mandatory timestamp |
| WebDAV list_files/list_sync_files/list_metadata | Native strict PROPFIND parser on the joined worker | OpenDAL requires getlastmodified even when a server returns only resourcetype; absent optional metadata must remain acceptable |
| WebDAV write_sync_file | OpenDAL staging PUT, direct MOVE with Overwrite: F on the same HTTP transport, read-back verification | OpenDAL rename first PROPFINDs the source and requires optional metadata; no unsafe metadata fabrication |
| S3 GET/conditional GET/PUT/DELETE | OpenDAL S3 0.59.5 | Explicit credentials/session token, region, endpoint, root and virtual-host settings; no environment/instance credential discovery |
| S3 write_sync_file | OpenDAL conditional PUT (If-None-Match: *), idempotent byte check and read-back verification | An explicit create race is accepted only when the existing bytes match |
| S3 create_dir | Explicit no-op | S3 has object prefixes rather than directories |
| S3 list_files/list_sync_files/list_metadata | Native strict paginated ListObjectsV2 adapter on the joined worker | Preserve optional metadata, prefix filtering, duplicate/token/capacity guards and malformed-page fail-closed behavior |
| Google Drive | Native retained | No reliable external Store token sink in the audited OpenDAL signer; its refresh URL also includes refresh/client credentials in the query |
| OneDrive | Native retained | OpenDAL rotates refresh tokens in private signer state without a persistence callback |
| AliyunDrive | Native retained | OpenDAL replaces refresh tokens in private signer state without a persistence callback |
| GitHub Gist / Gitee snippet / local directory | Existing implementation | Explicitly outside the storage-backend migration |

Thus WebDAV/S3 object access has migrated; directory/listing operations remain
fixed compatibility adapters. The three OAuth providers have **not** migrated.
The existing reliable token sink persists credentials before installing refreshed
tokens and still rejects superseded accounts. Their OpenDAL service features are
not enabled. Safely moving those providers requires a persistence-capable signer
API or a separately validated external token manager; interception that persists
only after refresh has already installed tokens is insufficient.

## Execution, publication and diagnostics

Each operation owns one remote, one bounded mailbox (capacity one), and one
current-thread Tokio runtime. Requests await their reply; no detached sync work
is introduced. Blocking compatibility requests execute on the same worker but
outside the runtime. Destruction joins the worker and destroys its blocking
clients there. GPUI render/update callbacks do not perform storage I/O.
Cancellation remains cooperative at existing protocol checkpoints; an in-flight
request is bounded by HTTP timeouts rather than force-aborted. Once committing,
existing operation control continues to reject cancellation. Pending publication
and recovery remain in the protocol/Store owners.

TimeoutLayer uses 30-second operation/I/O limits. HTTP transport also has a
30-second request timeout and a 10-second connect timeout. RetryLayer applies
only to object reads (up to two retries, jitter). Rate limits/Retry-After are
returned to the existing scheduler; mutations are never automatically replayed
on network failure. WebDAV Digest challenges and approved redirects retain the
original method/body. Cross-origin redirects, HTTPS downgrades and mutation 303
responses cannot replay credentials or upload bodies. S3 redirects are rejected, including by the retained Native listing client, so
custom x-amz-security-token headers cannot travel to another origin.

For V4, existing identical bytes are idempotent and differing bytes conflict.
WebDAV stages under a hidden child directory, outside direct-child protocol
enumeration; MOVE forbids overwrite. Staging is deliberately retained after an
ambiguous failure, without deleting user objects. S3 uses a single conditional
PUT. Successful publication is read back and compared. Network failures remain
errors even if the server might have accepted the object, allowing existing
pending-publication recovery to make the next decision.

HTTP status and Retry-After are sanitized; authentication, permission, DNS,
connect, TLS, timeout and connection-reset categories reach CloudSyncError and
the automatic sync classifier. The retained Native request helpers use the same
transport classification, eliminating generic HTTP 0 for unclassified connection
failures in directory requests too. OpenDAL error messages/source bodies and raw
URLs never enter user diagnostics. Fixed provider/operation tracing spans replace
stock TracingLayer, which records object paths. Retry notifications contain only
attempt counters. Operator construction suppresses upstream builder debug logs
that otherwise include endpoint/root configuration in the app log bridge. HTTP body statistics are shared across worker boundaries and
count partial received bodies and authentication/redirect attempts.

## Changed file responsibilities

| Files | Responsibility |
| --- | --- |
| Cargo.toml, Cargo.lock, desktop/Cargo.toml | Pin OpenDAL, select minimal services/layers, reuse HTTP types; test-only TLS fixture dependencies |
| core/src/cloud_sync/error.rs and mod.rs | UI-independent sanitized storage categories; no persisted format changes |
| desktop/src/http/cloud_sync/opendal/mod.rs | Full CloudSyncRemote facade, configuration, conditional reads and immutable publication |
| opendal/worker.rs | Bounded synchronous/async bridge and joined ownership of runtime/compatibility clients |
| opendal/transport.rs | Existing zed-reqwest TLS stack, safe redirects/Digest, request/body accounting |
| opendal/error.rs | Controlled OpenDAL-to-domain conversion, no backend diagnostic leakage |
| http/cloud_sync/helpers.rs, metrics.rs, webdav.rs, s3.rs, mod.rs | Share Digest/header parsing, transport classification, worker statistics scope, narrow redirect adapter and S3 redirect isolation |
| features/sync/cloud_sync_provider.rs | Select OpenDAL for WebDAV/S3 while preserving credential, observation and cache wrappers |
| app_shell/controller/auto_sync/coordinator.rs and tests.rs | Permanent versus retryable typed storage errors |
| opendal/tests.rs, tests/fixture.rs | Stateful native/OpenDAL HTTP compatibility, TLS, errors and publication fault tests |
| features/sync/cloud_sync_v4/tests.rs | Production-factory V3 migration, two-device stale preview/conflicts, cancellation and historical bytes |

## Dependencies and platform scope

OpenDAL is pinned to **=0.59.5**, default features disabled. Enabled features:
`executors-tokio`, `services-webdav`, `services-s3`, `layers-timeout`,
`layers-retry`. No stock reqwest transport, tracing layer, OAuth services or new
fork patches are enabled. `http = 1` bridges its HTTP API to the existing
zed-reqwest client. TLS continues to use the workspace's
`rustls-tls-native-roots` feature, including Windows system trust.
`rcgen = 0.14` and `rustls = 0.23` are test-only dependencies for a generated
localhost certificate; no private-key fixture is committed. OpenDAL's MSRV is
Rust 1.91; this run uses Rust/Cargo 1.99 on Windows, and CI selects stable.
Linux/macOS runtime behavior still needs their existing CI jobs.

## Validation results

Executed on Windows x86_64/MSVC, Rust/Cargo 1.99.0. Logs are untracked under
`target/opendal-*.log`.

| Command | Result |
| --- | --- |
| `python scripts/ci/check_architecture.py` | Passed |
| `cargo fmt --all -- --check` | Passed |
| `git diff --check` | Passed |
| `cargo check --workspace` | Passed, including application and both helpers |
| `cargo test -p nyaterm-core --lib` | 582 passed, 1 ignored |
| `cargo test -p nyaterm-desktop --lib` | 2619 passed, 12 ignored |
| `cargo test --workspace` | 5173 passed, 43 ignored across 51 suites (including doctests) |
| `cargo clippy --workspace --all-targets` | Passed without warnings |
| `cargo check -p nyaterm-desktop` | Passed |

The workspace total is from the broader run; the final desktop rerun includes
the subsequently added S3 listing redirect regression. Clippy and the desktop
check were repeated after final source cleanup.

The desktop run includes 18 OpenDAL storage tests and one production-factory
V3-to-V4 integration test that runs against both WebDAV and S3. Coverage:

- Native -> OpenDAL -> Native encrypted V3 round trips, old revision bytes
  retained, provider-root/remote-root paths and direct-child V4 listings.
- Basic/Digest, 301/302/307/308 mutation body preservation, rejected 303
  mutation/cross-origin replay, generated HTTPS certificate rejection and
  explicitly trusted test CA success.
- NAS responses without timestamps, recursive MKCOL, strict malformed XML and
  repeated pagination-token rejection, S3 multi-page Prefix/metadata/session
  credentials, default/custom/virtual-host endpoint and Unicode/percent paths.
- Conditional GET ETag/304 across adapter reconstruction, changed/deleted
  objects, no HEAD race, compatible 400/405/501 fallback, HTTP status and
  Retry-After, timeout/reset/partial-body counters, redacted error content.
- Acknowledgement loss after an accepted upload returns an error without
  replaying mutations or deleting legacy objects. WebDAV retains staging;
  subsequent explicit S3 recovery recognizes the exact complete bytes.
- Joined worker construction, directory requests, statistics and destruction
  from within an existing Tokio runtime, without nested block_on.
- Production-factory V3 migration, two-device V4 synchronization, stale preview
  rejection/conflicts, cancellation before publication and old bytes retained.
  Existing core tests exercise cancellation versus commit races; existing V4
  tests exercise durable pending publication, response loss, new edits during
  recovery and rotated-password compatibility.
- Existing Drive parser/token-sink tests run in the full desktop suite, including
  Store-write failure before token installation and refresh-token retention.
  This is not a claim of live OAuth refresh/pagination/throttling validation for
  all three remote services; those remain Native and need the external checks.

The initial local fixture inherited nonblocking sockets on Windows and produced
spurious connection resets. Accepted sockets are now explicitly blocking with
bounded reads; this fixture correction was necessary to make fault tests
represent actual HTTP failures. No production TLS checks were relaxed.

## External integration checks and remaining limits

Use disposable accounts/buckets/NAS directories and a separate application
configuration. Never use production snapshots for fault injection.

1. Flying Bull NAS 1.2.0800 (Issue #1067): on Windows, compare 1.3.2/2.0.0 and
   this branch with the same certificate/network; test Basic/Digest, HTTP/HTTPS,
   collection slash redirects, sleep/reconnect and transient connection resets.
   Confirm the error category and I/O counters without collecting terminal data
   or credentials. Local equivalents do **not** establish that #1067 is fixed.
2. AWS S3 plus a compatible service (e.g. MinIO): test endpoint/path/virtual host,
   region/session credentials, trusted TLS, >1 listing page, ETag/304, limits,
   offline interruption and recovery. Compare old/new object names and verify
   both clients can read their encrypted snapshots.
3. Google Drive/OneDrive/AliyunDrive remain Native: test access expiry, refresh
   success/failure, token rotation and Store-write failure, superseded accounts,
   pagination, throttling, upload failure and directory operations. Do not switch
   them until reliable credential persistence is demonstrated.
4. For each migrated service, import existing V3 data, join two V4 devices,
   create offline edits/conflicts, cancel before commit, attempt cancel during
   commit, lose an upload acknowledgement and restart with pending recovery.
   Verify old revisions remain readable and the master password is unchanged.

Snapshots remain buffered as required by the existing byte-vector trait; this
change does not add streaming or resumable multipart uploads. Interrupted WebDAV
staging objects can accumulate and are intentionally not auto-cleaned. S3
compatible services must implement conditional PUT correctly; unsupported
publication fails closed. MOVE atomicity/Overwrite enforcement and correct TLS
root installation must be verified on real services. No real external account or
NAS was accessed, and no issue is claimed fixed solely from adopting OpenDAL.
