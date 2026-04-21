---
sidebar_position: 4
---

# Backend Development

Backend code lives in `src-tauri/src/` and is written in Rust. It is the runtime core of Dragonfly: session management, SSH/SFTP, recording, translation, tunnels, authentication, and config persistence all land here.

## Command entry points and module organization

Backend command registration is centralized in `src-tauri/src/lib.rs`:

- Shared manager state is created there
- Tauri plugins are mounted there
- Commands are registered there through `tauri::generate_handler![]`

Command modules live in:

```text
src-tauri/src/cmd/
├── session.rs
├── sftp.rs
├── connection.rs
├── settings.rs
├── watcher.rs
├── translate.rs
├── stats.rs
├── tunnel.rs
├── proxy.rs
├── otp.rs
├── importer.rs
├── cloud_sync.rs
├── clipboard.rs
└── log.rs
```

When adding a new command, the usual flow is:

1. Define a `#[tauri::command]` in the appropriate `cmd/*.rs` file
2. Reuse existing logic in `core/` or `config/` where possible
3. Register the command in `src-tauri/src/lib.rs`
4. Call it from the frontend through `src/lib/invoke.ts`

## Shared runtime state

`src-tauri/src/lib.rs` injects these shared objects into Tauri state:

- `SessionManager`
- `TunnelManager`
- `RecordingManager`
- `PendingAuthManager`
- `CloudSyncManager`

Their roles are:

- Active session lifecycle and command history
- SSH tunnel state
- Recording state
- Pending keyboard-interactive / OTP authentication requests
- Cloud sync / backup state, remote snapshot operations, and conflict handling

## SessionManager

`src-tauri/src/core/session.rs` contains `SessionManager`, the center of session runtime behavior. It is responsible for:

- Registering and removing active sessions
- Sending `Write`, `Resize`, `Close`, and `Attach` commands into session I/O loops
- Managing command history and fuzzy search storage
- Emitting events such as `sessions-changed` and `command-history-changed`

The session metadata exposed to the frontend also includes `injection_active`, which indicates whether the current session supports shell-integration features such as terminal path tracking.

## Session implementations

Concrete session types are implemented under `src-tauri/src/core/`:

- `ssh/` — SSH connections, authentication, OSC/CWD tracking, SFTP, tunnels
- `pty.rs` — local terminal sessions
- `telnet.rs` — Telnet sessions
- `serial.rs` — serial sessions
- `recording.rs` — session recording
- `watcher.rs` — local file watching and auto-upload flows
- `importer.rs` — external client session import

## SSH modules

`src-tauri/src/core/ssh/` is the most central backend area:

- `client.rs` — russh client setup, known-host verification, proxy-aware connection setup
- `auth.rs` — loading saved authentication data and handling keyboard-interactive / OTP flows
- `io.rs` — terminal I/O and cwd update events
- `sftp.rs` — remote file operations and transfer queue handling
- `tunnel.rs` — local / remote / dynamic tunnels
- `session.rs` — SSH session lifecycle coordination

A typical SSH flow is:

1. Read the connection configuration
2. Decrypt passwords, private keys, or other credentials
3. Establish the TCP or proxy connection
4. Apply host-key policy verification
5. Complete authentication, possibly entering OTP / interactive flow
6. Open the PTY channel and enter the async I/O loop
7. Inject OSC/CWD tracking when supported

## SFTP and the transfer queue

`src-tauri/src/core/ssh/sftp.rs` is responsible for:

- Listing directories
- Uploading / downloading files and directories
- Delete / rename / mkdir / symlink / stat operations
- Transfer queue control such as pause / resume / cancel
- Emitting `transfer-event` for the frontend

The frontend transfer panel and `TransferContext` are built on top of these events.

## Watcher and auto-upload

`src-tauri/src/core/watcher.rs` handles local file watching.

A typical flow is:

1. The frontend chooses **Open** on a remote file from the file explorer
2. The backend downloads it into a local temp directory and starts watching it
3. After the local file is saved, a `file-modified` event is emitted
4. The frontend decides whether to open the auto-upload window or upload immediately when the user previously chose an always-upload behavior

This flow involves:

- `cmd/watcher.rs`
- `core/watcher.rs`
- Frontend `FileUploadPage.tsx`

## Cloud sync / portable snapshot

`src-tauri/src/core/cloud_sync.rs` owns the runtime behavior for this capability, including:

- startup sync checks
- manual push / pull
- debounced auto-push
- scheduled backups
- remote backup listing and restore
- conflict detection and event emission

The frontend reaches it through `src-tauri/src/cmd/cloud_sync.rs`, which exposes commands such as:

- `test_cloud_sync_connection`
- `get_cloud_sync_status`
- `sync_push_now`
- `sync_pull_now`
- `run_cloud_backup_now`
- `list_cloud_sync_history`
- `list_remote_backups`
- `restore_remote_backup`
- `resolve_cloud_sync_conflict`

`src-tauri/src/core/portable_snapshot.rs` defines which data belongs in a portable snapshot. That matters because it controls:

- which settings are suitable for cross-device sync
- which data should remain local to the device
- how backup snapshots differ in scope from sync snapshots

In the current implementation, portable snapshots cover connections, credential config, OTP, proxies, tunnels, quick commands, and most application settings; they do not blindly roam all live UI state.

## Configuration and encryption

Configuration files are stored under `~/.dragonfly/` and are mainly managed by `src-tauri/src/config/`.

Common files include:

- `settings.json`
- `sessions.json`
- `keys.json`
- `passwords.json`
- `otp.json`
- `quick-command.json`
- `tunnels.json`
- `proxies.json`
- `history.json`
- `known_hosts`
- `cloud_sync_state.json`

Sensitive fields are encrypted before being written, so when adding new configuration you should verify whether it crosses a sensitive-data boundary.

For cloud sync specifically, also pay attention to:

- provider credential fields going through encrypt / mask / merge handling
- runtime state being stored separately in `cloud_sync_state.json`
- history mainly being reconstructed from structured logs and then exposed through `list_cloud_sync_history`

## Event model

The backend relies heavily on Tauri events to notify the frontend. Typical events include:

| Event | Description |
|------|------|
| `terminal-output-{id}` | Terminal output |
| `cwd-changed-{id}` | Working directory changed |
| `session-closed-{id}` | Session closed |
| `sessions-changed` | Session list changed |
| `connections-changed` | Saved connections changed |
| `transfer-event` | Transfer progress |
| `otp-request` | OTP / keyboard-interactive authentication requested |
| `cloud-sync-status-changed` | Cloud sync / backup status changed |
| `cloud-sync-history-changed` | Sync / backup history changed |
| `cloud-sync-conflict` | Cloud sync conflict preview |

When designing new backend features, prefer exposing them through the existing event flow where appropriate rather than introducing extra polling APIs.
