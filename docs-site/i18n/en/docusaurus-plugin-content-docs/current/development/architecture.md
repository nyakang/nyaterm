# Architecture

NyaTerm is a native Rust desktop application built with **GPUI**. Its interface, terminal emulator, connection transports, and persistence implementation live in one Cargo workspace without a browser runtime or an IPC bridge.

## Layers

```text
nyaterm-app
  └─ starts GPUI, registers assets, creates the root window
       └─ nyaterm-desktop
            ├─ AppShell / NyaTermApp / feature state / views
            ├─ nyaterm-ui                shared GPUI controls and theme
            ├─ nyaterm-terminal-gpui     terminal layout, input, and painting
            ├─ nyaterm-terminal          terminal state machine and snapshots
            ├─ nyaterm-transport         PTY, SSH, SFTP, and other protocols
            ├─ nyaterm-recording         recording, history search, and writer pipeline
            ├─ nyaterm-updater           update checks, verification, installation, and recovery
            ├─ nyaterm-remote-desktop    RDP/VNC session management and helper IPC
            ├─ nyaterm-store             redb, transactions, compatibility readers
            └─ nyaterm-core              pure models, formats, and policies

  separate processes (IPC only; never linked into the application)
       ├─ nyaterm-rdp-helper            IronRDP decoding
       └─ nyaterm-vnc-helper            vnc-rs decoding
```

The main responsibilities are:

| Crate | Responsibility |
|------|----------------|
| `nyaterm-app` | Executable entry point, logging, embedded assets, and root-window creation |
| `nyaterm-desktop` | GPUI composition, state, views, platform adapters, and background coordination |
| `nyaterm-ui` | Shared controls, theme tokens, and the `gpui-kit` integration boundary |
| `nyaterm-terminal` | UI-independent terminal state, control sequences, encodings, and graphics protocols |
| `nyaterm-terminal-gpui` | GPUI terminal input, layout, selection, highlighting, images, and painting |
| `nyaterm-transport` | PTY, SSH, Telnet, Serial, SFTP, tunnels, remote operations, and transfer protocols |
| `nyaterm-recording` | UI-independent recording, transcripts, rotation, and history search; `pipeline` owns the writer thread, bounded queues, and coalesced events |
| `nyaterm-updater` | UI-independent update HTTP checks, signed downloads, installation preparation, helper execution, and portable recovery |
| `nyaterm-store` | redb persistence, transactions, encryption adapters, and compatibility readers |
| `nyaterm-core` | Domain models, compatibility formats, parsing, policies, and pure logic |
| `nyaterm-remote-desktop` | UI-independent RDP/VNC session management, framebuffer and input models, certificate policy, clipboard state, and the helper IPC contracts |
| `nyaterm-rdp-helper` | Isolated IronRDP helper process |
| `nyaterm-vnc-helper` | Isolated VNC helper process; owns the forked `vnc-rs` decoders, the reconnect ladder, and the server-facing policy gates |
| `nyaterm-otp` | HOTP/TOTP compatibility implementation |

## Startup

`crates/nyaterm-app/src/main.rs` is the application entry point:

1. Resolve runtime directories and initialize logging.
2. Register embedded assets and shared components with GPUI.
3. Create the native root window and an `AppShell` Entity.
4. Let `AppShell` start `StoreRuntime` and load the bootstrap snapshot asynchronously.
5. After validation succeeds, create `NyaTermApp`, then restore the window layout and sessions.

`AppShell` also owns application-level loading, recovery, and pre-exit flushing. A storage startup failure enters a recovery view instead of constructing the main application from unvalidated data.

## State ownership

`NyaTermApp` is the GPUI composition center, while focused feature-state structs own major UI domains such as connections, sessions, terminal presentation, transfers, settings, security, AI, sync, and remote operations.

Each value has one writable owner. The remaining independent Entity stores own only state that `NyaTermApp` does not own:

- `StartupRestoreStore` for the startup restore queue
- `OverlayStore` for the quick-switch overlay

Views read authoritative state directly when building GPUI elements. Do not introduce same-frame publish/read-back projections or keep independently mutable copies in both a feature state and an Entity.

## Background work and events

Filesystem, database, network, SSH, SFTP, subprocess, and image-decoding work does not run in render paths.

Background jobs return typed results or events to GPUI state. For example, session runtimes use `nyaterm_transport::SessionEvent` for output, working-directory changes, accepted commands, exits, and errors. The desktop window-runtime pump consumes those events, updates feature state, and notifies GPUI when a repaint is needed.

`nyaterm-recording::pipeline` owns the recording writer thread, queue limits, and coalescing of status and search results. Desktop coordinates its lifecycle and executes the shutdown task returned by `take_shutdown()` on a background worker. Synchronous helpers for cross-crate tests use the default-off `test-support` feature, which desktop enables only in development dependencies.

`nyaterm-updater` executes update checks, signed downloads, installation, and recovery. Desktop retains update state, generation checks, scheduling, close guards, and the Windows native progress window; cancellation checks, byte progress, and helper progress execution are injected through callbacks. Helper arguments, the cleanup environment variable, and package formats retain their existing contracts.

## Terminal data flow

```text
PTY / SSH / Telnet / Serial
        │
        ▼
nyaterm-transport typed events
        │
        ▼
nyaterm-desktop event drain and session state
        │
        ▼
nyaterm-terminal state machine and snapshots
        │
        ▼
nyaterm-terminal-gpui layout, input, and painting
```

`nyaterm-terminal` uses Alacritty terminal components for its grid and control-sequence state, while also owning UI-independent search, encoding, Kitty graphics, and Sixel behavior. GPUI sizing, keyboard adaptation, selection, highlighting, images, and per-frame painting remain in `nyaterm-terminal-gpui`.

## Persistence and compatibility

`nyaterm-store` executes database work through a dedicated `StoreRuntime`; the desktop submits typed requests through its UI or blocking clients. GPUI views never access redb directly.

Schema-neutral contracts such as configuration models, backup formats, cloud-sync documents, and encryption policies live in `nyaterm-core`. Database implementation and legacy-data readers live in `nyaterm-store`. Existing table names, keys, field names, encryption prefixes, `.nya` backups, and Dragonfly fallbacks are compatibility boundaries.

## RDP / VNC process isolation

The RDP and VNC protocol decoders parse **server-controlled bytes**, so they live in no crate the application links. Each runs in its own helper process and talks to the application through the typed IPC protocol in `nyaterm-remote-desktop`.

What that boundary means:

- A decoder crash cannot take the application down. Both helpers must translate a decoder panic into a fatal IPC error rather than dying silently
- `nyaterm-remote-desktop` owns no decoder. It handles session management, framebuffer and input models, certificate policy, and clipboard state
- The VNC server-facing policy gates (`view_only`, `shared`, clipboard enablement) must stay enforced in the helper, not only in the application
- The application resolves helper paths beside its own executable; `NYATERM_RDP_HELPER` / `NYATERM_VNC_HELPER` override that

Both helper crates carry a `tests/lifecycle.rs` covering the handshake, an ordinary disconnect, and crash/hang reaping. Keep both in step when the IPC contract changes.

## Dependency rules

- `nyaterm-core`, `nyaterm-terminal`, `nyaterm-transport`, and `nyaterm-remote-desktop` stay independent of GPUI.
- `nyaterm-updater` may depend only on `nyaterm-core` within the project; ordinary, development, build, and platform dependencies must not introduce GPUI, desktop, store, or other business crates. `nyaterm-recording` also remains independent of GPUI, desktop, transport, and other business crates.
- Protocol decoders that parse server-controlled bytes live only in the helper crates, never in a crate the application links.
- Desktop features use `nyaterm-ui` for ordinary inputs, selects, menus, switches, and dialogs.
- Modules use normal Rust module trees and explicit imports.
- New features prefer an existing focused feature state; add an authoritative Entity only when an independent lifecycle requires one.

See [GPUI Desktop Development](./frontend) for presentation rules and [Runtime, Transport, and Storage Development](./backend) for runtime and persistence guidance.
