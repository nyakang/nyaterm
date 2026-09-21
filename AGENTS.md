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
  contracts, and shared pure logic. Do not add GPUI dependencies here.
* `crates/nyaterm-desktop`: GPUI application composition, `AppShell`,
  `NyaTermApp`, feature state, views, platform adapters, background-job
  coordination, native HTTP adapters, and GPUI Entity stores.
* `crates/nyaterm-terminal`: terminal state machine, snapshots,
  control-sequence handling, encoding, and graphics protocols. It must remain
  independent of UI frameworks.
* `crates/nyaterm-terminal-gpui`: GPUI-specific terminal layout, input,
  highlighting, images, and painting.
* `crates/nyaterm-transport`: local PTY, SSH, Telnet, Serial, SFTP, tunnels,
  remote operations, and transfer-protocol runtime. It must remain independent
  of GPUI and desktop presentation types.
* `crates/nyaterm-ui`: shared GPUI theme tokens, the `gpui-component`
  integration boundary, and reusable NyaTerm presentation and interaction
  widgets.
* `crates/nyaterm-store`: persistence implementation, transactions, redb
  schema, encryption adapters, and database compatibility readers. Keep pure
  data models and serialization policies in `nyaterm-core`.
* `crates/nyaterm-remote-desktop`: UI-independent RDP/VNC session management,
  framebuffer and input models, certificate policy, clipboard state, and the
  RDP/VNC helper IPC contracts. It owns no protocol decoder; both live in the
  helper crates below.
* `crates/nyaterm-rdp-helper`: isolated IronRDP helper process that communicates
  with the application through the typed IPC protocol in
  `nyaterm-remote-desktop`.
* `crates/nyaterm-vnc-helper`: isolated VNC helper process using the same IPC
  protocol. It owns the forked `vnc-rs` decoders, the VNC reconnect ladder, and
  the server-facing policy gates (`view_only`, `shared`, clipboard enablement).
  Those gates must stay enforced here, not only in the application.
* `crates/nyaterm-otp`: bundled HOTP/TOTP implementation.
* `crates/nyaterm-app/assets`: bundled icons and images. Assets under
  `icons/**` are normally tintable and rendered through `svg()` or
  `mono_icon()`; full-color assets use `img()` or `color_icon()`. Preserve the
  rendering distinction when adding assets.
* Third-party dependencies that NyaTerm patches are not vendored. Each is a
  patch series on a fork under <https://github.com/nyakang> on branch
  `nyaterm`, consumed from a revision pinned in the root `Cargo.toml`:
  `alacritty` (`alacritty_terminal`), `gpui-component`, `IronRDP`
  (`ironrdp-client`, `ironrdp-connector`), `russh`, `russh-sftp`, `sspi-rs`,
  `vnc-rs`, `zed` (`gpui`, `gpui_platform`), and `zmodem2`. Each branch carries
  a `NYATERM.md` recording its base revision, its patches, and how they were
  validated.
* `temp/vendor/`: untracked, read-only copies of those sources, kept only so
  they can be read locally. Nothing under `temp/` is compiled. **Editing
  anything there has no effect on the build and produces no error** — change
  the fork branch instead.
* `nix/package.nix`, `flake.nix`, `default.nix`, `shell.nix`: declarative Nix
  packaging, development shells, and Flake outputs. When adding or bumping Git
  dependencies in `Cargo.lock`, keep `outputHashes` in `nix/package.nix` in step.

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
* Keep transport code independent of GPUI and desktop presentation types.
* Keep remote-desktop protocol and session logic in
  `nyaterm-remote-desktop`; keep GPUI presentation in `nyaterm-desktop`.
* Keep protocol decoders that parse server-controlled bytes in the helper
  crates, never in a crate the application links. Both helpers must translate a
  decoder panic into a fatal IPC error rather than dying silently.
* Keep persistence logic out of GPUI views.

Use normal Rust module trees with `mod.rs` files or sibling module
declarations. Do not add `#[path = "..."]` declarations or `use super::*`
imports. Name dependencies explicitly, including in tests.

Avoid broad exports from crate roots and shared feature preludes. Import
models, services, GPUI types, and helpers from their authoritative modules.

## UI and Input Rules

Ordinary forms, prompts, searches, menus, selects, switches, and dialogs should
use the wrappers exposed by `nyaterm-ui`. That crate owns the
`gpui-component` integration, theme mapping, and stable NyaTerm component API;
desktop feature modules must not depend directly on `gpui-component`.

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

RDP and VNC each run in a helper process that the application resolves beside its
own executable. `cargo run -p nyaterm-app --bin nyaterm` builds only the
application, so build the helpers into the same target directory first or both
protocols fail with `HelperMissing`:

* `cargo build -p nyaterm-rdp-helper -p nyaterm-vnc-helper`

A bare `cargo build` or `cargo check` covers all three: they are the workspace
`default-members`. `NYATERM_RDP_HELPER` and `NYATERM_VNC_HELPER` override the
lookup with an explicit path. `scripts/release/package_native.py` is what puts the
helpers next to the application in release packages; its `HELPER_BINS` list must
name every helper.

Before review, run the relevant broader checks:

* `cargo check --workspace`
* `cargo test --workspace`
* `cargo fmt --all -- --check`
* `cargo clippy --workspace --all-targets`

Nix builds and development environments:

* `nix build` (builds `nyaterm`, helpers, desktop entry, and icons into `./result`)
* `nix run` (executes the packaged application)
* `nix develop` or `nix-shell` (interactive shell with toolchain and runtime libraries)
* `nix flake check --no-build` (evaluates and checks flake outputs)

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

* Terminal parsing, snapshots, graphics, selection, input, and rendering tests
  belong in `nyaterm-terminal` or `nyaterm-terminal-gpui`.
* SSH, SFTP, Telnet, Serial, tunnel, transfer, and terminal-session lifecycle
  tests belong in `nyaterm-transport`.
* RDP/VNC protocol, framebuffer, input mapping, IPC, certificate, clipboard,
  and reconnect tests belong in `nyaterm-remote-desktop`, `nyaterm-rdp-helper`,
  or `nyaterm-vnc-helper`. Helper crates carry a `tests/lifecycle.rs` covering
  the handshake, an ordinary disconnect, and crash/hang reaping; keep both in
  step when the IPC contract changes.
* Storage changes require round-trip and supported-format compatibility tests.
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
