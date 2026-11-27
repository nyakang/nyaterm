# FTP runtime and validation

Related request: [nyakang/nyaterm#1376](https://github.com/nyakang/nyaterm/issues/1376).

## Delivered scope

- External opening downloads a binary copy into the existing private editor
  workspace. The watcher uploads a stable local snapshot to its original session
  and path. Repeated saves coalesce while an upload is running. Remote metadata
  and a SHA-256 digest detect conflicts; automatic upload never approves a conflict.
- A session reserves one connection for metadata and lazily opens a bounded pool
  for transfers. The pool uses the larger upload/download concurrency setting at
  connection time, capped at ten transfer connections. Desktop owns the credential
  factory and the maintenance task; files owns blocking operations and leases.
- Idle connections receive NOOP probes after 30 seconds. Safe metadata reads and
  transfer attempts can reconnect with bounded exponential backoff, using the
  existing retry preference. Authentication and TLS failures are not retried; they also stop background
  reconnect attempts until the session is configured again.
- Binary transfers use task-owned checkpoints when the existing resume preference
  is enabled. Manual retry shares that task's checkpoint; another task never
  adopts it. Downloads verify metadata, staged-file identity and the preceding
  64 KiB before REST. Missing source timestamps or rejected REST fall back to a
  full transfer. ASCII transfers restart rather than splice byte offsets.
- Uploads use unique remote temporary files and validate the local file handle
  and path before committing. Existing destinations remain intact until commit.
  Uncertain rename outcomes are never automatically retried or deleted. Retained
  upload paths are reported for recovery, including exhausted retries whose
  cleanup may later fail after disconnection.
- FTP plugs into the existing OS export and clipboard routes. Deferred virtual
  readers share a private binary snapshot and support random reads. Directory
  inventory includes empty folders, validates names and rejects links. Session
  shutdown invalidates exported sources. macOS promised downloads force binary.

Server-side rename/move already uses RNFR/RNTO. This work does not change saved
connections, credentials, backups or sync formats, and requires no fork revision
bump. Checkpoints are in memory only: restarting NyaTerm does not resume them.
FTP has no portable compare-and-swap primitive; metadata and boundary checks cannot
exclude all concurrent changes by another client between checking and committing.

## Automated coverage

`cargo test -p nyaterm-files --lib` includes local FTP server fixtures for:

- browsing during a stalled transfer, pool bounds and shutdown cancellation;
- idle probing and metadata reconnection;
- interrupted uploads/downloads, REST rejection and manual retry ownership;
- pause releasing a transfer slot and changed sources refusing commit;
- binary external edits and explicit conflict confirmation;
- lazy shared virtual streams, session expiry, directory inventory and links.

Desktop transfer state tests cover coalescing saves. Existing replacement tests
cover ambiguous rename replies, rollback and retained backups. Existing ADB
snapshot tests continue to cover the shared snapshot reader.

## Platform acceptance

Native desktop behavior still requires testing on the corresponding OS:

- Windows: drag and clipboard paste into Explorer and Office; random reads,
  duplicate opens, cancellation, folder/empty-folder export, disconnect and
  private staging ACLs.
- macOS: Finder file/folder promises, Unicode names, binary data, cancellation
  and destination conflicts.
- Linux: existing supported export/clipboard destinations. This change does not
  add a new desktop file-promise protocol.

Use both FTP and explicit/implicit FTPS, including a server without REST support.
No live-server or native Windows/macOS acceptance is implied by Linux fixture tests.
