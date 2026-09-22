# Development Setup

The NyaTerm application is a Cargo workspace. Node.js and pnpm are only required for the Docusaurus documentation site in this repository.

## Application prerequisites

### Rust and Git

Install the latest stable Rust toolchain:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Windows users can install from [rustup.rs](https://rustup.rs/). Every platform also needs Git and its native compiler toolchain.

### Platform dependencies

#### Windows

Install [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) with the **Desktop development with C++** workload.

#### macOS

Install Xcode and its command-line tools:

```bash
xcode-select --install
```

GPUI uses Metal on macOS, so a working macOS SDK is required.

#### Linux (Ubuntu / Debian)

Install Rust crate build tools, font/window-system development libraries, and the Vulkan loader:

```bash
sudo apt update
sudo apt install build-essential clang pkg-config cmake \
  libfontconfig1-dev libfreetype6-dev libssl-dev libudev-dev \
  libwayland-dev libx11-dev libx11-xcb-dev \
  libxcb-cursor-dev libxcb-icccm4-dev libxcb-image0-dev \
  libxcb-keysyms1-dev libxcb-randr0-dev libxcb-render0-dev \
  libxcb-shape0-dev libxcb-xfixes0-dev libxcb-xinerama0-dev \
  libxkbcommon-dev libxkbcommon-x11-dev libzstd-dev \
  libvulkan1 mesa-vulkan-drivers
```

Running the desktop application also requires a working Vulkan driver and an X11 or Wayland session.

## Clone the repository

```bash
git clone https://github.com/nyakang/nyaterm.git
cd nyaterm
```

Cargo manages all application dependencies. The Node.js dependencies in this repository belong to `docs-site` only.

## Run the application

```bash
cargo run -p nyaterm-app --bin nyaterm
```

The first build compiles GPUI and every dependency, so it takes noticeably longer than subsequent incremental builds.

### RDP / VNC need their helpers built first

RDP and VNC each run in a separate helper process that the application resolves beside its own executable. The command above **only builds the application**, so both protocols fail with `HelperMissing`. Build the helpers first:

```bash
cargo build -p nyaterm-rdp-helper -p nyaterm-vnc-helper
```

A bare `cargo build` builds the application and both helpers because they are the workspace `default-members`. `cargo check` checks all three too, but does not produce helper executables that the application can launch. If you use a custom `CARGO_TARGET_DIR`, `--target`, or profile, keep the helpers in the same directory as the application.

`NYATERM_RDP_HELPER` and `NYATERM_VNC_HELPER` override the lookup with an explicit path, which is handy for pointing at binaries in another target directory.

## Common checks

Prefer locked checks scoped to the affected crate while iterating:

```bash
cargo check -p <crate-name> --locked
cargo test -p <crate-name> --locked
```

Rust CI runs fmt and clippy on Linux, then runs workspace tests independently on Linux x64, macOS arm64, and Windows x64. The exact commands are:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked
cargo test --workspace --locked --no-fail-fast
```

The separate Linux Python packaging-test job runs:

```bash
python -m unittest scripts.tests.test_check_release_assets scripts.tests.test_package_native scripts.tests.test_verify_native_package scripts.tests.test_generate_release_metadata
```

The non-ignored RDP/VNC helper lifecycle integration tests are covered automatically by workspace tests, including handshake, normal exit, and crash/hang reaping. They can also be run directly:

```bash
cargo test -p nyaterm-rdp-helper --test lifecycle --locked
cargo test -p nyaterm-vnc-helper --test lifecycle --locked
```

These lifecycle tests launch real helper executables but do not connect to real RDP/VNC servers, so they do not replace manual protocol interoperability, framebuffer, clipboard, or input-path acceptance. `cargo fmt --all` writes formatting changes; use it only when you intend to apply them.

## Release-profile build

```bash
cargo build -p nyaterm-app --bin nyaterm --release --locked
```

The native binary is written to `target/release/nyaterm`, or `target/release/nyaterm.exe` on Windows. This command builds only the application binary — neither the helpers nor any installer.

Release packages come from `scripts/release/package_native.py`. It builds the application and both helpers with locked dependencies, puts the helpers next to the application, and produces native installers and portable packages. When you add a helper, its `HELPER_BINS` list must be updated too.

### Six release targets

| Platform | Rust target | Artifacts |
| --- | --- | --- |
| macOS arm64 | `aarch64-apple-darwin` | `.dmg`, `.app.tar.gz` |
| macOS x64 | `x86_64-apple-darwin` | `.dmg`, `.app.tar.gz` |
| Linux x64 | `x86_64-unknown-linux-gnu` | `.AppImage`, `.deb`, `.rpm` |
| Linux arm64 | `aarch64-unknown-linux-gnu` | `.AppImage`, `.deb`, `.rpm` |
| Windows x64 | `x86_64-pc-windows-msvc` | `_portable.zip`, `-setup.exe` |
| Windows arm64 | `aarch64-pc-windows-msvc` | `_portable.zip`, `-setup.exe` |

Every release CI matrix leg runs:

```bash
python scripts/release/package_native.py "${TARGET}"
python scripts/release/verify_native_package.py --target "${TARGET}" --version "${VERSION}" --dist dist
```

Before publication, `scripts/ci/check_release_assets.py` also rejects missing or extra artifacts in the combined six-target asset set.

`NYATERM_ARTIFACT_VERSION` changes only the version segment in artifact names;
package metadata still uses the workspace SemVer. This interface is reserved
for manual snapshot builds, and packaging and verification must receive the
same value:

```bash
NYATERM_ARTIFACT_VERSION=main-snapshot \
  python scripts/release/package_native.py "${TARGET}"
python scripts/release/verify_native_package.py \
  --target "${TARGET}" --version "${VERSION}" \
  --artifact-version main-snapshot --dist dist
```

After validation, a version tag publishes GitHub Release and versioned R2 assets, then triggers Gitee, AUR, and Homebrew. The website reads `downloads.json`; signed `latest.json` exists only to migrate installed Tauri releases to GPUI. Only stable releases replace the root R2 manifests, while prereleases retain versioned manifests. A manual `Main Snapshot` run overwrites the `main-snapshot` prerelease without publishing to downstream channels.

The Release workflow requires `NYATERM_GITHUB_GIST_CLIENT_ID`, the Gitee/R2 repository variables, and the updater, R2, Gitee, AUR, and Homebrew secrets named in the workflows. Missing configuration fails the relevant release step instead of producing an incomplete official release.

### Native tools and the manual-acceptance boundary

Native packaging depends on target-platform tools: Windows uses NSIS and package verification also needs 7-Zip; macOS uses `codesign` and `hdiutil`; Linux uses tools such as `appimagetool`, `dpkg-shlibdeps`, `dpkg-deb`, `rpmbuild`, and `rpm`/`rpm2cpio`. Running only the Python packaging unit tests on a machine without those tools is therefore not a native package build.

Automated verification checks the artifact set, archive paths, application and helper presence, binary architecture, version, and package metadata. It does not prove that the GUI launches, and does not cover real install/upgrade/uninstall flows, shortcuts or `nyaterm:` URL-handler invocation, signing/notarization and Gatekeeper/SmartScreen trust, real RDP/VNC sessions, or GPU, IME, PTY, clipboard, and window lifecycle behavior. Release candidates must be accepted manually on the corresponding target OS, with the actual platform and results recorded truthfully.

## Documentation development

When editing `docs-site`, also install Node.js 22.13+ and [pnpm](https://pnpm.io/). Docusaurus itself only needs Node 18+, but the pnpm version this repository pins imports `node:sqlite` and crashes outright on anything older. The `packageManager` field in `docs-site/package.json` pins that pnpm version, and corepack picks it up automatically.

```bash
pnpm --dir docs-site install --frozen-lockfile
pnpm --dir docs-site start:zh
```

Start the English documentation server with:

```bash
pnpm --dir docs-site start:en
```

The exact documentation CI commands are:

```bash
python3 scripts/ci/check_docs_translations.py
pnpm --dir docs-site install --frozen-lockfile
pnpm --dir docs-site build
```

The build checks pages and sidebars for every locale and reports Markdown-link problems according to the site configuration. The translation script verifies that every page exists in both locales, that both copies have the same heading count (which catches a whole section going untranslated), that every page is referenced from `sidebars.ts`, and that no authoring notes were left in the published text. The `Documentation site` CI job runs the script before installing locked dependencies and building every locale.

Note that the script compares heading counts, not translated wording.

## Changing third-party dependencies

The third-party dependencies NyaTerm patches are **not vendored into this repository**. Each is a patch series on a fork under [github.com/nyakang](https://github.com/nyakang) on branch `nyaterm`, consumed from a revision pinned in the root `Cargo.toml`: `alacritty`, `gpui-kit`, `IronRDP`, `russh`, `russh-sftp`, `sspi-rs`, `vnc-rs`, `zed` (`gpui`), and `zmodem2`.

The workflow is: commit to the fork branch, push, then bump the pinned revision in the root `Cargo.toml`. Keep the patch series split by concern rather than squashed, and record the reason and the validation performed on the patch commit and in that branch's `NYATERM.md`. Prefer rebasing an existing series onto a newer upstream revision over accumulating snapshots.

`temp/vendor/` holds read-only local copies of those sources, kept only so they can be read locally. **Nothing there is compiled — editing it has no effect on the build and produces no error.**

## Development conventions

- Read the root `AGENTS.md` and `CONTRIBUTING.md` first.
- UI state and views live in `nyaterm-desktop`; shared controls live in `nyaterm-ui`.
- Transport, terminal, and core crates stay independent of GPUI.
- New UI text updates both locale files under `crates/nyaterm-desktop/src/i18n/locales/`.
- Never use real credentials in tests, logs, or diagnostic data.
