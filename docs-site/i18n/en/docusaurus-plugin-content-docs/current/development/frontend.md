# GPUI Desktop Development

NyaTerm's native interface lives in `crates/nyaterm-desktop`, while shared controls live in `crates/nyaterm-ui`. The desktop layer includes GPUI state, views, window interaction, platform adapters, and integration points for background-job results.

## Entry point and windows

`nyaterm-app` creates the root window and `AppShell`. After storage bootstrap completes, `AppShell` creates the `NyaTermApp` Entity and starts workspace restoration.

Independent settings, connection editor, quick-command, and remote-text-editor windows are created with GPUI `open_window`. Windows exchange Entities, typed state, or explicit callbacks rather than URL routing or message bridges.

## Module structure

```text
crates/nyaterm-desktop/src/
├── app_shell/       # Root shell, startup/recovery/quit lifecycle, native menus
├── entities/        # Authoritative window runtime, startup restore, quick switch Entities
├── features/        # Focused feature state, runtime adapters, and views
├── i18n/            # Locale loading and translation
├── models/          # Desktop presentation models
├── http/            # Native HTTP adapters
└── terminal.rs      # Terminal presentation entry point
```

`features/` groups connections, sessions, terminal behavior, settings, security, transfers, tunnels, sync, AI, remote operations, layout, and panels by domain. Add behavior to the domain that owns it instead of creating a general migration bucket or shared prelude.

## State management

`NyaTermApp` composes focused feature-state structs. A method that only changes one domain should generally live on that state. Keep a thin `NyaTermApp` adapter when GPUI notification, window access, or cross-domain coordination is required.

Follow these ownership rules:

- Each mutable value has one authoritative owner.
- Do not keep writable mirrors in both `NyaTermApp`/feature state and an Entity store.
- Views read current state directly; they do not publish and read back snapshots during render.
- Cross-thread work returns through typed events or typed task results in a GPUI update.

## Views and controls

Helpers that build GPUI elements remain with views or desktop features. Do not move view construction onto pure state merely to reduce the number of `impl NyaTermApp` blocks.

Ordinary inputs, selects, menus, switches, and dialogs use the stable component API exposed by `nyaterm-ui`. Desktop features do not depend directly on `gpui-kit`. Ordinary text fields use `NyaInput`/`NyaInputState` or the id registry in `features/text_inputs.rs`, with definite dimensions around the input.

Terminal input, paste review, and `RemoteTextEditor` are full editing surfaces and must not be replaced by ordinary single-line inputs.

## Input and native window interaction

Global shortcuts and pointer events route from the root view into the active feature. Event handlers should deliberately choose when to call `cx.stop_propagation()` and must avoid parent click handlers that steal focus from text fields.

Platform-specific window, clipboard, drag-and-drop, PTY, and IME behavior requires validation on the affected operating system. New child windows should reuse the established window lifecycle and modal coordination patterns.

## Terminal presentation

Terminal responsibilities are split across two layers:

- `nyaterm-terminal` owns the grid, scrollback, control-sequence state, search, and graphics protocols.
- `nyaterm-terminal-gpui` owns pixel layout, key-event conversion, selection, highlighting, images, and painting.

The desktop feeds session output into terminal state, then provides snapshots and interaction state to the GPUI terminal element. Do not reimplement control-sequence parsing or wire protocols in views.

## Internationalization

Application locale files are stored at:

- `crates/nyaterm-desktop/src/i18n/locales/zh-CN.json`
- `crates/nyaterm-desktop/src/i18n/locales/en.json`

Update both languages for new or changed user-facing text and follow the existing translation-key conventions.

## Background work and tests

Render paths and long GPUI update callbacks must not perform database, filesystem, network, SSH, SFTP, subprocess, or image-decoding work. Use GPUI executors, dedicated runtimes, or an existing job coordinator, then update authoritative state when the result returns.

Test state transitions through pure methods where possible. GPUI interaction tests use adjacent `#[gpui::test]` modules or the existing test context. Window, clipboard, drag-and-drop, and IME changes also require a platform smoke test.
