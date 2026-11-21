# Contributing to NyaTerm

## Before changing code

Read `AGENTS.md` and the relevant crate-level architecture notes. Keep one
authoritative owner for each piece of state. Do not introduce WebView/Tauri
layers, `#[path]` module aliases, `use super::*`, snapshot-only stores, or direct
database access from GPUI views.

Choose the crate that owns the behavior:

- Pure models, parsing, compatibility formats, and policies belong in
  `nyaterm-core`.
- Database execution and compatibility readers belong in `nyaterm-store`.
- PTY, SSH, Telnet, Serial, SFTP, transfer, and tunnel runtime code belongs in
  `nyaterm-transport`.
- Terminal parsing and snapshots belong in `nyaterm-terminal`; GPUI layout and
  painting belong in `nyaterm-terminal-gpui`.
- GPUI state, views, and background coordination belong in
  `nyaterm-desktop`.
- Shared GPUI controls and theme integration belong in `nyaterm-ui`.

For a change that crosses crates, keep the boundary adapter small and document
which crate owns the resulting state. Do not put filesystem, database, network,
SSH, SFTP, subprocess, or image-decoding work in a render path or a long-running
GPUI update callback.

### Connection editor parity

The connection editor and the asset batch editor must keep the same surface.
When adding, removing, or renaming a connection setting:

- add the field to the connection editor section that owns it;
- give the batch editor a row with the same label, the same section, and the
  same default value used for a new connection;
- keep `BATCH_FIELDS_BY_SECTION` exhaustive; a batch field that no protocol
  section renders is a parity bug;
- run the `editor_surface_parity` tests in `nyaterm-desktop`, which check field
  coverage, section reachability, and that `Restore default` writes the same
  value a new connection starts with.

Protocol-specific editors compose shared `editor_surface` primitives instead of
painting their own control shells. Do not copy a control layout into a batch
section to make a field fit.

## Local checks

Run focused checks while iterating:

```bash
cargo check -p <crate-name> --locked
cargo test -p <crate-name> --locked
```

Before review, reproduce the exact Rust CI commands. Formatting and clippy run
on Linux; the workspace test runs independently on Linux x64, macOS arm64, and
Windows x64:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked
cargo test --workspace --locked --no-fail-fast
```

The Linux packaging-helper job, which needs neither Rust nor native GUI
libraries, runs this exact command:

```bash
python -m unittest scripts.tests.test_check_release_assets scripts.tests.test_package_native scripts.tests.test_verify_native_package scripts.tests.test_generate_release_metadata
```

Documentation CI runs these exact commands on Linux with Python 3.12, Node.js
22, and the repository-pinned pnpm version:

```bash
python3 scripts/ci/check_docs_translations.py
pnpm --dir docs-site install --frozen-lockfile
pnpm --dir docs-site build
```

`cargo test --workspace` builds every test target, so a separate `cargo check
--workspace` before it is redundant. `--no-fail-fast` matters: without it the
first failing crate hides every crate after it. The non-ignored RDP and VNC
helper lifecycle integration tests are included automatically in the workspace
test; they can also be reproduced directly:

```bash
cargo test -p nyaterm-rdp-helper --test lifecycle --locked
cargo test -p nyaterm-vnc-helper --test lifecycle --locked
```

Use `cargo run -p nyaterm-app --bin nyaterm` for a local graphical smoke test.
That command builds only the application, not the RDP/VNC helpers. Build the
helpers first when the smoke test includes those protocols:

```bash
cargo build -p nyaterm-rdp-helper -p nyaterm-vnc-helper --locked
```

The full workspace and application checks may need platform-native GPUI, PTY,
serial, clipboard, and window-system dependencies.

### Ignored benchmarks and SFTP integration test

Ignored benchmarks are manual diagnostics, not CI gates. Run them by exact test
name in a release build on a recorded, otherwise idle machine; do not run a
blanket workspace `--ignored`, which would also select credentialed integration
tests and unrelated ignored fixtures:

```bash
cargo test -p nyaterm-desktop --release --locked dense_action_link_selection_drag_benchmark -- --ignored --nocapture --test-threads=1
cargo test -p nyaterm-desktop --release --locked overview_marker_fast_scroll_benchmark -- --ignored --nocapture --test-threads=1
cargo test -p nyaterm-desktop --release --locked selected_occurrence_search_large_scrollback_benchmark -- --ignored --nocapture --test-threads=1
cargo test -p nyaterm-desktop --release --locked root_render_hundred_sessions_eight_terminal_leaves_benchmark -- --ignored --nocapture --test-threads=1
cargo test -p nyaterm-terminal-gpui --release --locked keyword_highlight_benchmark -- --ignored --nocapture --test-threads=1
cargo test -p nyaterm-core --release --locked sustained_in_place_input_and_deletion -- --ignored --nocapture --test-threads=1
```

A benchmark exiting successfully means only that its assertions did not fail;
elapsed output is not a portable performance threshold. Record the commit,
OS/architecture, CPU/GPU, display scale, font, build profile, workload, sample
count, and raw output before comparing runs.

The ignored SFTP end-to-end test requires
`NYATERM_TEST_SFTP_HOST`, `NYATERM_TEST_SFTP_PORT`,
`NYATERM_TEST_SFTP_USERNAME`, `NYATERM_TEST_SFTP_PASSWORD`, and
`NYATERM_TEST_SFTP_ROOT`. The root must already exist, be writable, and be safe
for the test to create and remove children. Use only an isolated test server and
disposable root, keep credentials out of command lines and logs, then run:

```bash
cargo test -p nyaterm-transport --test sftp_service_e2e --locked sftp_service_round_trips_file_manager_operations -- --ignored --nocapture --test-threads=1
```

### Release packaging matrix

Release CI packages and verifies six native targets:

| Runner | Rust target | Artifacts |
| --- | --- | --- |
| macOS arm64 | `aarch64-apple-darwin` | `.dmg`, `.app.tar.gz` |
| macOS x64 | `x86_64-apple-darwin` | `.dmg`, `.app.tar.gz` |
| Linux x64 | `x86_64-unknown-linux-gnu` | `.AppImage`, `.deb`, `.rpm` |
| Linux arm64 | `aarch64-unknown-linux-gnu` | `.AppImage`, `.deb`, `.rpm` |
| Windows x64 | `x86_64-pc-windows-msvc` | `_portable.zip`, `-setup.exe` |
| Windows arm64 | `aarch64-pc-windows-msvc` | `_portable.zip`, `-setup.exe` |

The release preflight uses the same locked workspace test and Python packaging
unit tests shown above. Each matrix leg runs `package_native.py` and
`verify_native_package.py`; publishing additionally checks that the combined
asset set is exact. Those automated checks validate package structure,
metadata, helper presence, and binary architecture. They do not replace manual
installation, launch, upgrade/uninstall, URL-handler, platform trust/signing,
GUI, PTY, GPU/IME, or real RDP/VNC acceptance on the target OS.

`NYATERM_ARTIFACT_VERSION` changes only the version segment in artifact names;
package metadata still uses the workspace SemVer. This is reserved for the
manual snapshot workflow and must be passed to both packaging and verification:

```bash
NYATERM_ARTIFACT_VERSION=main-snapshot \
  python scripts/release/package_native.py "${TARGET}"
python scripts/release/verify_native_package.py \
  --target "${TARGET}" --version "${VERSION}" \
  --artifact-version main-snapshot --dist dist
```

Tag releases publish the verified assets to GitHub and Cloudflare R2, then
trigger the Gitee, AUR, and Homebrew workflows. `downloads.json` is the website
download catalog; the signed `latest.json` exists only to migrate installed
Tauri releases. A manual `Main Snapshot` run overwrites the `main-snapshot`
prerelease and does not publish to downstream channels. Release builds require
the repository variables and secrets named by those workflows; missing release
configuration is a hard failure.

Tests should live beside the behavior they cover. Storage, credentials,
encryption, backup, cloud-sync, known-host, and session changes must include
new-data round trips and representative legacy-data tests. Never log passwords,
private keys, OTP secrets, API keys, or unredacted terminal context.

## Pull requests

Use a Conventional Commit-style subject, for example
`fix(transport): handle closed SSH channels`. Describe behavior, architectural
ownership, commands and platforms tested, and any persistence, credential,
migration, or forked-dependency impact. Keep unrelated formatting or
generated metadata out of the change.

Keep pull requests focused and explain compatibility-sensitive decisions in the
description. Patched third-party dependencies are not vendored: each is a patch
series on a fork under <https://github.com/nyakang> on branch `nyaterm`, pinned
by revision in the root `Cargo.toml`. Change one by committing to its fork
branch and bumping that revision, and identify the upstream project/version or
commit, the reason for the modification, and the validation performed. `temp/vendor/`
holds untracked read-only copies for reading only; nothing there is compiled.

## Compatibility

Preserve existing table names, keys, serialized field names, document keys,
encryption prefixes, master-key wrapping, backup formats, and fallback
decryption behavior unless the change includes a tested migration path. Do not
overwrite user data until validation succeeds, and keep secret-bearing values
masked when returning settings to the UI.
