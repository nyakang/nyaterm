# Repository Guidelines

## Project Overview

NyaTerm is a native GPUI desktop application written in Rust. It provides SSH,
local shell, Telnet, Serial, RDP, VNC, SFTP, tunnels, OTP, AI assistance, and
encrypted sync and backup in one workspace.

This repository is a Rust 2024 Cargo workspace using resolver `3`. Preserve
compatibility with existing NyaTerm configuration, credentials, backups,
cloud-sync data, known hosts, and sessions. Any incompatible data-format change
must include explicit conversion logic and tests.

## Workspace Structure

* `crates/nyaterm-app`: executable entry point, bundled assets, logging setup,
  and root-window creation.
* `crates/nyaterm-core`: UI-independent domain models, parsing, policies, AI
  settings/risk/provider logic, schema-neutral serialization and encryption
  contracts, command-completion fill planning, and shared pure logic. Do not add GPUI
  dependencies here. Desktop authorizes and executes completion plans.
* `crates/nyaterm-desktop`: GPUI application composition, `AppShell`,
  `NyaTermApp`, feature state, views, platform adapters, background-job
  coordination, service adapters, and GPUI Entity stores.
* `crates/nyaterm-mcp-host`: UI-independent loopback MCP service, authentication,
  client identities, cancellation routing, and atomic discovery-file I/O. It may
  depend on core and mcp-protocol only among workspace crates. Desktop injects
  discovery permissions and owns approval, capability execution, configuration
  paths, generation registration, and ephemeral credential lifetime.
* `crates/nyaterm-agent-client`: UI-independent Codex and Claude Code CLI clients,
  executable discovery, account/version/authentication probes, and owned child
  process trees and pipe readers. Only core is allowed as a workspace dependency,
  including tests. Desktop injects MCP helper paths/environment and callbacks,
  retaining task scheduling, UI state, persistence, and credential guards.
* `crates/nyaterm-ai-client`: UI-independent native AI provider HTTP clients,
  request and streaming-response adapters, model discovery, and proxy handling.
  Runtime/task ownership and AI feature orchestration remain in desktop.
* `crates/nyaterm-preview`: UI-independent image/PDF decoding, text preview
  parsing, delimited table ordering, pixel transformations and decoded-content contracts. Desktop owns
  fetching, job lifecycle, GPUI image conversion and preview caches. Its `icons`,
  `wallpaper` and `qr` modules own decoding; OTP URI interpretation stays in desktop.
* `crates/nyaterm-translation-client`: UI-independent native translation HTTP
  clients and provider request/response adapters. Desktop owns translation
  tasks, settings persistence, credential state and presentation.
* `crates/nyaterm-cloud-storage`: UI-independent native and OpenDAL cloud-storage
  adapters, OAuth device authorization, token-refresh sinks, and HTTP I/O counters.
  Sync orchestration, caches, and credential persistence remain in desktop/store.
  Its default-off `test-support` feature exposes local fixtures only for tests.
* `crates/nyaterm-updater`: UI-independent native update HTTP checks, signed
  downloads, installation preparation, helper execution, and portable recovery.
  Desktop owns update state, scheduling, close guards, and native progress UI;
  cancellation and helper progress execution are injected through callbacks.
  These adapter crates may depend on `nyaterm-core`, but not GPUI, desktop,
  store, other business crates, or each other, including in development dependencies.
* `crates/nyaterm-terminal`: terminal state machine, snapshots,
  control-sequence handling, encoding, and graphics protocols. It must remain
  independent of UI frameworks.
* `crates/nyaterm-terminal-runtime`: UI-independent frame commands, parsing worker,
  coalesced events, chunked search and transport session event bridge. Only core,
  terminal, transport and recording are allowed as workspace dependencies,
  including tests. Desktop owns visibility, event application, session/window
  ownership and shutdown scheduling. Transferred frame sessions are opaque;
  its default-off `test-support` feature exposes only integration fixtures.
* `crates/nyaterm-terminal-gpui`: GPUI-specific terminal layout, input,
  highlighting, images, and painting. Its `scrollbar`, `decorations` and `ime`
  modules own reusable presentation; desktop retains drag/session routing,
  action-link cache selection and composition editing state.
* `crates/nyaterm-transport`: local PTY, SSH authentication and channels, Telnet,
  Serial, tunnels, session lifecycle, tmux, and transfer-protocol runtime. It
  exposes file-session probe injection and must not depend on the file,
  recording, or remote-operations crates.
* `crates/nyaterm-files`: local and remote file services, SFTP and shell file
  backends, file revisions, path safety, listing/sort and navigation policies, and
  file-transfer policies. Desktop owns listing caches and navigation state. Its
  `external_edit` module owns private workspaces and blocking file watching;
  desktop injects platform permissions and owns editor/upload orchestration.
* `crates/nyaterm-remote-ops`: system metrics, process management, Docker, and
  GPU/NPU operations over transport execution APIs. Docker state is shared by
  the desktop SSH connection lease, not owned by transport.
* `crates/nyaterm-recording`: recording, transcripts, rotation, and terminal
  history search over the core and terminal contracts. Its `pipeline` module
  owns the recording writer, bounded queues, and coalesced events; desktop owns
  lifecycle coordination. Its default-off `test-support` feature exposes only
  cross-crate test helpers.
* These four crates remain independent of GPUI and desktop presentation types;
  the three business crates must not depend on each other.
* `crates/nyaterm-ui`: shared GPUI theme tokens, the `gpui-kit`
  integration boundary, static icon catalogs/resolvers/rendering, and reusable
  NyaTerm presentation and interaction widgets. `markdown::lightweight` retains the lightweight Markdown renderer
  separately from `NyaMarkdown`; `presentation` owns stateless menu rows.
* `crates/nyaterm-sync`: UI-independent sync protocol and policies. Its
  `auto_sync` schedule receives time from callers; desktop owns operations,
  task cancellation, safe-window checks, result mapping and history persistence.
* `crates/nyaterm-store`: persistence implementation, transactions, redb
  schema, encryption adapters, and database compatibility readers. Keep pure
  data models and serialization policies in `nyaterm-core`. The blocking store
  client exports and verifies migration safety backups off the UI thread.
* `crates/nyaterm-remote-desktop`: UI-independent RDP/VNC session management,
  framebuffer and input models, certificate policy, clipboard state, and the
  RDP/VNC helper IPC contracts. It owns no protocol decoder; both live in the
  helper crates below.
* `crates/nyaterm-android`: UI-independent ADB server client, shared device
  tracker, native Shell v2 and Sync channels, bounded Android helper IPC and
  screen process supervision. It may depend on core, but not GPUI, desktop,
  store, transport, files, or decoder libraries.
* `crates/nyaterm-android-helper`: isolated scrcpy protocol, H.264 decoding,
  BGRA conversion and Android input enforcement. Only this helper links FFmpeg.
* `crates/nyaterm-rdp-helper`: isolated IronRDP helper process that communicates
  with the application through the typed IPC protocol in
  `nyaterm-remote-desktop`.
* `crates/nyaterm-vnc-helper`: isolated VNC helper process using the same IPC
  protocol. It owns the forked `vnc-rs` decoders, the VNC reconnect ladder, and
  the server-facing policy gates (`view_only`, `shared`, clipboard enablement).
  Those gates must stay enforced here, not only in the application.
* `crates/nyaterm-windows-rdp`: explicitly selected Windows Native RDP system
  component exception. Thread-bound ActiveX/ATL host and staged controller;
  COM runs on the creating UI/STA thread outside all GPUI borrows. No GPUI
  production dependency, no protocol decoder or exposed COM interfaces.
* `crates/nyaterm-otp`: bundled HOTP/TOTP implementation.
* `crates/nyaterm-app/assets`: bundled icons and images. Assets under
  `icons/**` are normally tintable and rendered through `svg()` or
  `mono_icon()`; full-color assets use `img()` or `color_icon()`. Preserve the
  rendering distinction when adding assets.
* Third-party dependencies that NyaTerm patches are not vendored. Each is a
  patch series on a fork under <https://github.com/nyakang> on branch
  `nyaterm`, consumed from a revision pinned in the root `Cargo.toml`:
  `alacritty` (`alacritty_terminal`), `gpui-kit`, `IronRDP`
  (`ironrdp-client`, `ironrdp-connector`), `russh`, `russh-sftp`, `sspi-rs`,
  `picky-rs` (Picky and its ASN.1 crates),
  `vnc-rs`, `zed` (`gpui`, `gpui_platform`), and `zmodem2`. Each branch carries
  a `NYATERM.md` recording its base revision, its patches, and how they were
  validated.
* `temp/vendor/`: untracked, read-only copies of those sources, kept only so
  they can be read locally. Nothing under `temp/` is compiled. **Editing
  anything there has no effect on the build and produces no error** — change
  the fork branch instead.

## Application Architecture

`NyaTermApp` is the central GPUI composition owner. Feature-specific state
lives in focused structs for connections, commands, remote operations, remote
desktop, security, settings, AI, terminal presentation, sessions, transfers,
sync, translation, updates, tunnels, recording, and shell behavior.

Persisted collections and compatibility-sensitive catalogs belong to their
feature owners. Schema-neutral persistence formats and parsing contracts belong
in `nyaterm-core`; database execution and compatibility readers belong in
`nyaterm-store`.

The GPUI Entity stores each own state that `NyaTermApp` does not own:

* `StartupRestoreStore`: startup-restore queue.
* `OverlayStore`: authoritative quick-switch overlay state.

For new or substantially changed features:

* Prefer a focused feature-state struct or a deliberately authoritative GPUI
  Entity over new top-level `NyaTermApp` fields.
* Keep exactly one authoritative mutable owner for each piece of state. Do not
  mirror independently mutable state between `NyaTermApp`, feature state, and
  Entity stores.
* If a method only reads and writes one feature-state struct, place that logic
  on the state and keep the `NyaTermApp` method as a notifier or adapter when
  needed.
* Keep GPUI element construction in views. Pure state types should not render
  UI.
* Use typed events to communicate from background jobs to GPUI state.
* Never perform filesystem, database, network, SSH, SFTP, subprocess, image
  decoding, or other blocking work in a render path or a long-running GPUI
  update callback.
* Keep terminal parsing, snapshots, graphics protocols, and wire handling in
  `nyaterm-terminal`.
* Keep terminal layout, GPUI input adapters, highlighting, image presentation,
  and painting in `nyaterm-terminal-gpui`.
* Keep transport, files, remote operations, and recording code independent of
  GPUI and desktop presentation types.
* Keep remote-desktop protocol and session logic in
  `nyaterm-remote-desktop`; keep GPUI presentation in `nyaterm-desktop`.
* Keep protocol decoders that parse server-controlled bytes in the helper
  crates, never in a crate the application links. Both helpers must translate a
  decoder panic into a fatal IPC error rather than dying silently. Windows Native,
  selected explicitly by the user, hosts the installed Windows system component
  in process; the IronRDP/VNC decoder isolation requirement continues to apply.
* Keep persistence logic out of GPUI views.

Use normal Rust module trees with `mod.rs` files or sibling module
declarations. Do not add `#[path = "..."]` declarations or `use super::*`
imports. Name dependencies explicitly, including in tests.

Avoid broad exports from crate roots and shared feature preludes. Import
models, services, GPUI types, and helpers from their authoritative modules.

## UI and Input Rules

Ordinary forms, prompts, searches, menus, selects, switches, and dialogs should
use the wrappers exposed by `nyaterm-ui`. That crate owns the
`gpui-kit` integration, theme mapping, and stable NyaTerm component API;
desktop feature modules must not depend directly on `gpui-kit`.

Ordinary text inputs should use `nyaterm-ui::NyaInput` and `NyaInputState`,
either owned by a focused feature or through the id-keyed registry in
`features/text_inputs.rs`. Do not add hand-painted ordinary text inputs.

Terminal input, paste review, and `RemoteTextEditor` are full editing surfaces
with dedicated selection, undo, IME, and command handling. Do not replace them
with single-line registry fields.

Text-field wrappers must have a definite height and width. Parent click
handlers must not immediately steal focus from a field. Keep controls usable
with keyboard navigation and ensure overlays and child windows restore focus
predictably.

GPUI's `overflow_y_scroll()` enables scrolling but paints nothing, and `scrollbar_width` only reserves gutter space, so use `NyaScrollable` (the `ScrollableElement` trait re-exported by `nyaterm-ui`):

* In-flow containers: use `overflow_*_scrollbar()` instead of
  `overflow_*_scroll()`, and do not also reserve a `scrollbar_width` gutter -
  the bar overlays the content.
* Virtualized lists (`uniform_list`) and any caller-owned scroll handle: keep
  `track_scroll` on the list and attach `.vertical_scrollbar(&handle)` to the
  non-scrolling parent. An absolutely positioned overlay inside a scrolling
  element is translated by the scroll offset and would scroll away.
* Absolutely positioned popups must not use `overflow_*_scrollbar()`. The
  wrapper inherits only size and flex styles, so `position`/`top`/`left` would
  move onto the inner content node and break the popup's placement. Give the
  popup a non-scrolling shell and scroll an in-flow child instead.
* A `Scrollbar` inside a caller-positioned overlay needs
  `viewport_from_layout()`; otherwise it paints at the scroll handle's own
  bounds and ignores the overlay.

Scrollbars auto-hide (`ScrollbarMode::Hover`), installed once in
`theme_bridge.rs`. A bar fades in while the pointer is anywhere over its scroll
viewport or that axis is scrolling, and out after idle, so a horizontal bar is
discoverable without pinning it open. Two further consequences:

* A hand-positioned `Scrollbar` overlay must span the whole scroll viewport,
  not just the strip the track occupies. Reveal-on-hover watches the bar's own
  hitbox, so a strip-sized overlay only reacts within a track width of the edge.
  `overflow_*_scrollbar()` and `vertical_scrollbar(&handle)` already do this.
* Where the two axes ride different scroll handles they are separate
  `Scrollbar` elements with separate reveal state, and the upstream
  component's corner-avoidance does not apply, so inset one overlay by a track
  width.

Anything that writes `Theme::scrollbar_mode` or the `scrollbar*` colors directly
must call `Theme::sync_base(cx)` afterwards. `Scrollbar` reads the
`gpui_base::Theme` projection, which those assignments do not touch.

The terminal scrollback scrollbar is hand-rolled as a reserved flex column with
its own drag and overview ruler, and is deliberately exempt from all of the
above.

## Persistence and Compatibility

Changes involving redb, credentials, known hosts, OTP, cloud sync, portable
snapshots, `.nya` backups, AI or translation secrets, and application settings
are compatibility-sensitive.

Before changing these areas:

* Preserve table names, keys, serialized field names, document keys,
  encryption prefixes, master-key wrapping, backup formats, and fallback
  decryption behavior unless the change explicitly updates the data contract.
* Test both new-data round trips and loading representative data written by
  supported NyaTerm versions.
* Do not silently discard unknown or unsupported fields.
* Validate fully before overwriting existing user data.
* Keep secret-bearing values masked when returning settings to the UI.

`nyaterm-store/src/storage/mod.rs` owns the database implementation, with
domain-specific modules under `nyaterm-store/src/storage/`. Treat existing
redb data, `.nya` backups, master-key wrapping, encrypted payload formats, and
text-document fallbacks as public compatibility contracts.

## Security Rules

Never log or commit:

* passwords or decrypted saved credentials;
* private-key contents or passphrases;
* OTP secrets or generated codes;
* AI, cloud-sync, OAuth, snippet, or translation API secrets;
* unredacted diagnostics containing terminal output, command context, or user
  data.

Use custom redacted `Debug` implementations for secret-bearing structs when
debug output is required. Prefer typed secret wrappers and zeroize sensitive
buffers where practical.

Patching a third-party dependency means committing to its fork branch, pushing,
and bumping the pinned revision in the root `Cargo.toml`. Do not edit `temp/`.
Record the reason for the change and the validation performed on the patch
commit and in that branch's `NYATERM.md`, and keep the patch series split by
concern rather than squashed. Prefer rebasing the series onto a newer upstream
revision over accumulating snapshots.

## Build and Development Commands

Use package-specific checks while iterating:

* `cargo check -p nyaterm-app`
* `cargo test -p <crate-name>`
* `cargo run -p nyaterm-app --bin nyaterm`

Builtin/IronRDP, VNC and Android screen run in helper processes that the application resolves
beside its own executable. Windows Native RDP is the explicitly selected
in-process system-component exception and requires MSVC with ATL when building.
`cargo run -p nyaterm-app --bin nyaterm` builds only the application, so build the
helpers into the same target directory first or the helper backends fail with
`HelperMissing`:

* `python3 scripts/release/prepare_android.py` (export its reported `FFMPEG_DIR`)
* `cargo build -p nyaterm-rdp-helper -p nyaterm-vnc-helper -p nyaterm-android-helper`

A bare `cargo build` or `cargo check` covers the app and helpers: they are the workspace
`default-members`. `NYATERM_RDP_HELPER` and `NYATERM_VNC_HELPER` override the
lookup with an explicit path; `NYATERM_ANDROID_HELPER` does the same for Android.
See `crates/nyaterm-android/README.md` for pinned dependencies and acceptance probes.
`scripts/release/package_native.py` is what puts the
helpers next to the application in release packages; its `HELPER_BINS` list must
name every helper.

Before review, run the relevant broader checks:

* `cargo check --workspace`
* `cargo test --workspace`
* `cargo fmt --all -- --check`
* `cargo clippy --workspace --all-targets`

Use `cargo fmt --all` only when intentionally applying formatting changes.

Platform-specific GPUI, PTY, Serial, SSH, clipboard, window, path, RDP/VNC,
and icon-rendering behavior must be verified on the affected operating system.

## Coding Style

Use standard `rustfmt` formatting and Rust 2024 idioms.

* Use `snake_case` for modules, functions, fields, and variables.
* Use `PascalCase` for structs, enums, and traits.
* Use `SCREAMING_SNAKE_CASE` for constants.
* Prefer explicit imports and narrow public interfaces.
* Prefer typed models and errors over loosely structured strings.
* Use small adapters at crate boundaries instead of importing
  application-wide models into low-level crates.
* Comments should explain invariants, compatibility constraints, ownership, or
  non-obvious performance decisions rather than restating code.

When splitting a large file, preserve its public facade where practical so
structural changes do not cause unrelated call-site churn. Prefer domain cuts
that move constants, records, helpers, tests, and dependencies together over
type-only splits that leave coupling behind.

## Testing Guidelines

Add tests beside the behavior being changed.

* Frame queue, search worker, ingress routing, recording order and shutdown tests
  belong in `nyaterm-terminal-runtime`; GPUI event-application tests stay in desktop.
* Terminal parsing, snapshots, graphics, selection, input, and rendering tests
  belong in `nyaterm-terminal` or `nyaterm-terminal-gpui`.
* SSH, Telnet, Serial, tunnel, protocol transfer, and terminal-session lifecycle
  tests belong in `nyaterm-transport`. File and SFTP tests belong in
  `nyaterm-files`; operation and recording tests belong in their owning crates.
* RDP/VNC protocol, framebuffer, input mapping, IPC, certificate, clipboard,
  and reconnect tests belong in `nyaterm-remote-desktop`, `nyaterm-rdp-helper`,
  or `nyaterm-vnc-helper`. Helper crates carry a `tests/lifecycle.rs` covering
  the handshake, an ordinary disconnect, and crash/hang reaping; keep both in
  step when the IPC contract changes.
* Storage changes require round-trip and supported-format compatibility tests.
* Update HTTP, signature, package validation, replacement, and recovery tests
  belong in `nyaterm-updater`. GPUI update state and native progress presentation
  tests remain in desktop. Release jobs run `portable_release_archive_round_trip`
  from the updater crate against the actual packaged ZIP.
* Credential and encryption changes require success, invalid-password,
  corrupted-data, and compatibility-format tests.
* GPUI state changes should test state transitions separately from visual
  rendering where possible.

Use descriptive, behavior-oriented test names.

## Commits and Pull Requests

Use Conventional Commit-style subjects:

`type(scope): imperative summary`

Common scopes include `terminal`, `transport`, `desktop`, `storage`, `ui`,
`remote-desktop`, `ai`, and `sync`.

Pull requests should include:

* a concise description of behavior and architectural impact;
* linked issues where applicable;
* commands and platforms tested;
* explicit notes for persistence, credentials, data compatibility, or forked
  dependency changes, including the fork branch and revision a bump moves to.
