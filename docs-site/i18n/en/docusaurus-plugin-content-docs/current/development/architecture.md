---
sidebar_position: 1
---

# Architecture

Dragonfly is a **Tauri 2** desktop application. The frontend lives in `src/`, the backend lives in `src-tauri/src/`, and they communicate through Tauri commands and events.

## Overall architecture

```text
┌─────────────────────────────────────────────────────────┐
│ Frontend (React / TypeScript)                          │
│  ├─ Main window: AppProvider + App.tsx                 │
│  ├─ Child windows: ChildAppProvider + ChildWindowRouter│
│  ├─ Terminal workspace, side panels, dialogs           │
│  └─ invoke wrapper + Tauri event listeners             │
├─────────────────────────────────────────────────────────┤
│ Tauri IPC bridge                                       │
├─────────────────────────────────────────────────────────┤
│ Backend (Rust)                                         │
│  ├─ SessionManager / TunnelManager / RecordingManager  │
│  ├─ PendingAuthManager / CloudSyncManager              │
│  ├─ SSH / SFTP / watcher / importer / stats            │
│  └─ JSON config + encrypted credential storage         │
└─────────────────────────────────────────────────────────┘
```

## Frontend window model

The frontend entry path is selected in `src/main.tsx`:

- **Main window** — loads `AppProvider` and `App.tsx`
- **Child windows** — load `ChildAppProvider` and `ChildWindowRouter`

Current child-window flows include:

- Settings
- New session
- Quick command editing
- Auto-upload prompts

Relevant files:

- `src/main.tsx`
- `src/ChildWindowRouter.tsx`
- `src/lib/windowManager.ts`

`windowManager.ts` also coordinates focus and interactivity between modal child windows and the main window.

## Frontend state model

### AppContext

`src/context/AppContext.tsx` is the main state container for the primary window. It owns:

- Workspace tabs and pane trees
- Application settings and UI settings
- Saved connections and group refreshes
- Startup restoration for `ui.open_tabs`

### ChildAppProvider

`src/context/ChildAppProvider.tsx` is a lightweight provider used by child windows:

- Loads and saves settings only
- Syncs with the main window through events
- Does not manage the full workspace or active session state

### TransferContext

`src/context/TransferContext.tsx` manages the file transfer queue separately. It consumes backend `transfer-event` notifications and drives pause, resume, cancel, and retry behavior in the UI.

## Workspace model

Dragonfly's terminal workspace has two layers that are easy to confuse but serve different purposes.

### Logical tabs and pane trees

`src/lib/workspaceTabs.ts` is responsible for:

- Creating tabs and session panes
- Horizontal and vertical splits inside a tab
- Persisting `ui.open_tabs`
- Restoring the serializable workspace structure on startup

### Runtime window layout

`src/lib/tabWindows.ts` is responsible for:

- Which tabs are currently attached to which window leaf
- The active tab inside each leaf
- Runtime window split ratios

A practical shorthand is:

- `workspaceTabs.ts` = the logical workspace that gets persisted
- `tabWindows.ts` = the live runtime arrangement of terminal areas

## Terminal integration

`src/components/terminal/XTerminal.tsx` is the xterm.js integration center. It is responsible for:

- Fit / Search / WebLinks and related addons
- Shell integration and command-history suggestions
- Line-number / timestamp gutter
- Action links and keyword highlighting
- Large-output protection and recovery messaging
- Session event subscriptions and reconnect behavior

## Backend runtime model

`src-tauri/src/lib.rs` is the backend entry point. It constructs and stores shared runtime state such as:

- `SessionManager`
- `TunnelManager`
- `RecordingManager`
- `PendingAuthManager`
- `CloudSyncManager`

It also registers Tauri commands centrally, including commands for:

- Session creation / close / write / recording / OTP flows
- SFTP file and transfer operations
- Connections, keys, passwords, OTP, and settings persistence
- Cloud sync / backup status, push, pull, restore, and conflict handling
- Watcher, translation, importer, stats, tunnel, and proxy flows

## SessionManager and event flow

`src-tauri/src/core/session.rs` contains `SessionManager`, the central registry for active sessions. It is responsible for:

- Tracking all active sessions
- Routing commands into per-session I/O loops
- Maintaining command history and fuzzy search storage
- Emitting `sessions-changed`, `command-history-changed`, and related events

The backend also emits these common events to the frontend:

| Event | Description |
|------|------|
| `terminal-output-{id}` | Terminal output |
| `cwd-changed-{id}` | Working directory updates |
| `session-closed-{id}` | Session closed |
| `sessions-changed` | Session list changed |
| `connections-changed` | Saved connections changed |
| `transfer-event` | Transfer queue progress changed |
| `otp-request` | OTP / keyboard-interactive authentication requested |
| `cloud-sync-status-changed` | Cloud sync / backup status changed |
| `cloud-sync-history-changed` | Sync / backup history changed |
| `cloud-sync-conflict` | Cloud sync conflict preview and handling entry point |

## SSH, SFTP, watcher, and import flows

Core backend capabilities are mainly organized under these modules:

- `src-tauri/src/core/ssh/` — SSH connection setup, authentication, OSC/CWD tracking, SFTP, tunnels
- `src-tauri/src/core/pty.rs` — local terminal sessions
- `src-tauri/src/core/telnet.rs` — Telnet sessions
- `src-tauri/src/core/serial.rs` — serial sessions
- `src-tauri/src/core/watcher.rs` — local file watching and auto-upload workflows
- `src-tauri/src/core/importer.rs` — Xshell / MobaXterm / WindTerm session import
- `src-tauri/src/core/recording.rs` — session recording
- `src-tauri/src/core/cloud_sync.rs` — cloud sync, remote backups, status events, and conflict handling
- `src-tauri/src/core/portable_snapshot.rs` — portable snapshot build/apply logic and sync scope control

## Configuration and persistence

Application data is stored under `~/.dragonfly/`. Typical files include:

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

Sensitive values are encrypted before being written, so the app manages reusable credential records rather than plain-text secrets.

Cloud sync adds two more important layers:

- `src-tauri/src/config/cloud_sync.rs` manages provider settings, runtime state, and sensitive-field encrypt / mask / merge behavior
- `src-tauri/src/core/portable_snapshot.rs` defines which data belongs in portable snapshots and which device-local UI state stays local
