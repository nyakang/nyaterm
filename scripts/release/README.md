# Release tooling

## Windows crash diagnostic builds (#1065)

Run from an x64 MSVC developer shell on Windows, with Python 3.11+, Git, and
the original compiler installed:

```powershell
rustup toolchain install 1.99.1 --profile minimal --component rustfmt
python scripts/release/package_diagnostic.py --source-ref v2.0.1 --toolchain 1.99.1 --id issue1065-test1
```

The manual `Windows Diagnostic` workflow runs the same command and retains its
outputs as an Actions artifact; it does not publish a release or post messages.
`--prepare-only` prepares the source and patch without compiling; `--resume`
continues the same ID/ref/toolchain after preparation or a failed compilation,
provided the recorded sources, compiler and lockfile still match.

The builder checks out the exact application revision in
`target/diagnostic-build/<id>/source`. It adds a narrowly scoped diagnostic
overlay there, leaving ordinary builds and release packaging untouched. Unknown
source layouts fail before applying any edits. The original dependency revisions,
lockfile and version remain frozen. In particular, v2.0.1's packaging includes
its ActiveX helper, even though current releases use a different architecture.
Rust 1.99.1's `58cf8f9c3` compiler matches the reports in #1065.

Release optimization is retained, with full debuginfo and no symbol stripping for
the application and dependencies. Every Rust executable ships with its matching
PDB. The builder runs an intentional, caught panic in a relocated copy of the
actual GUI executable while hiding the build-directory PDB. Packaging succeeds
only if its panic report resolves the named probe function and a source line.
This self-test exits before opening a database, acquiring the app instance or
starting GPUI. It does not reproduce or claim to fix the reported crash.

Diagnostic breadcrumbs track asset saving, editor closing, session startup,
PTY creation, shell spawning, layout, text shaping and painting. Callbacks only
update bounded process-wide atomic counters, first/last sequence numbers and
numeric font/cell/scale values. They do not capture names, paths, terminal text,
commands, credentials or keys, and do not perform render-path disk I/O. The
existing synchronous panic hook writes these values before resolving symbols.
Concurrent snapshots are best effort; sequence numbers establish instrumentation
order, not wall-clock timestamps or per-session attribution.

Outputs under `dist/diagnostic-<id>/` include the portable ZIP and SHA-256,
`build-info.json`, the exact `diagnostic.patch`, and `symbol-self-test.log`.
The manifest records the compiler, source SHA, patch SHA-256, lockfile SHA-256,
and packaged file hashes. Send the ZIP to the reporter, who should extract it
into a fresh writable directory, retain all PDB/ConPTY/helper files, and return
new `data/crashes/panic-*.log`, corresponding `data/logs/*.jsonl`, the build
manifest and reproduction steps. Panic reports are not part of the ordinary
diagnostic export. If clean settings do not reproduce, compare font/display
settings through the UI. Do not copy an installed database or keys into this
portable build, and do not request the entire `data` directory.

Validation:

```powershell
python -m unittest scripts.tests.test_package_diagnostic -v
```

## Release version

Linux packages stage the original 32/64/128/256/512 pixel icons with 0644
permissions and traversable 0755 directories, independent of checkout modes or
the builder's umask. Debian packages use `dpkg-deb --root-owner-group`; their
`postinst configure` repairs only the installed package's regular icon files.
`StartupWMClass` matches the X11 class (`NyaTerm` or `NyaTerm Preview`), while
Wayland retains the lowercase desktop ID. See the separate
[focal compatibility builder](../compat/linux-focal/README.md) for glibc 2.31 and
private software OpenGL artifacts.

Use Python 3.11 or newer to preview and apply a release version from the repository root:

```sh
python scripts/release/bump_version.py 2.0.0-preview.5 --dry-run
python scripts/release/bump_version.py 2.0.0-preview.5
git diff -- Cargo.toml Cargo.lock

# Update and commit the release version in one step:
python scripts/release/bump_version.py 2.0.0-preview.5 --commit

# Preview both the edits and the commit message:
python scripts/release/bump_version.py 2.0.0-preview.5 --commit --dry-run
```

Stable versions such as `2.0.0` and a leading `v` are also accepted. The script
validates SemVer and both files before writing. It preserves comments and line
endings and works without invoking Cargo, accessing the network, or updating
dependency resolutions. Repeating the same version is a no-op.

`--commit` creates `chore(release): bump version to <version>` containing only
`Cargo.toml` and `Cargo.lock`. Unrelated staged and unstaged changes are preserved.
Both version files must be tracked and clean before running this option, so
existing manifest or dependency edits cannot accidentally enter the release
commit. A branch, valid Git identity, and no conflicts or unfinished Git
operations are required. These checks run before editing files. If a commit
hook or Git commit fails, the version edits remain for review and manual commit.
An unchanged version creates no commit.

The script updates `[workspace.package].version` in the root `Cargo.toml` and the
local packages in `Cargo.lock` whose manifests use `version.workspace = true`.
Those member manifests need no edits. Application version strings use
`CARGO_PKG_VERSION`; native packaging reads the workspace version and generates
platform version metadata and release manifests automatically.

Windows application and installer numeric versions use four 16-bit components:
`major.minor.patch.revision`. The revision is the final prerelease identifier if
it is numeric, otherwise zero: `2.0.0-preview.9` becomes `2.0.0.9`, while `2.0.0`
and `2.0.0-beta` become `2.0.0.0`. Build metadata is ignored for numeric versions.
Components above 65535 fail the Windows build or packaging rather than truncate.
The application's product version retains the full Cargo SemVer string.

Other version values have separate meanings and remain unchanged:

- `nyaterm-otp`, `nyaterm-plugin-api`, and plugin guest packages have independent
  package versions.
- Plugin `host_version` constraints record the minimum compatible host release;
  example manifests, documentation, and compatibility fixtures retain that floor.
- The docs site's `package.json` and the pinned ConPTY package have independent
  versions.

Commit the version changes before creating the matching `v<version>` release
tag. The release workflow requires the tag/input version to match the Cargo
workspace version. The script can create a local commit with `--commit`; tags,
pushes, and release publishing remain separate steps.

## Signed rehearsal and GPUI cutover

See [GPUI_CUTOVER.md](GPUI_CUTOVER.md) for frozen sources, migration acceptance,
PowerShell commands, release gates and distribution recovery.

`Release` accepts `verify_release` (default `false`) in both dispatch and reusable
workflow calls. Set it to `true` with `upload_release=false` to build all targets,
verify package provenance, sign updater payloads, and generate/verify the
manifests without publishing. The `release-candidate` Actions artifact retains
packages, manifests, hashes, release notes and per-target source records for 30
days. This is distinct from OS code signing/notarization.

`source_ref` is resolved once to a full SHA. Reusing `source_run_id` requires a
successful run from this repository's Release workflow, matching source/version
records and package hashes. Runs without provenance must be rebuilt. Reuse is
allowed for signed rehearsals as well as publication; build-only mode does not
accept it. Keep the dispatch ref and source checkout at the same reviewed
candidate when recording evidence.

Release notes use `scripts/release/notes/<version>.md` when present, otherwise a
local Git history summary. The 2.0.0 notes include the required manual portable
and MSI migration instructions. Check in version-specific notes before the
candidate is frozen.

## Windows portable update recovery (#1089)

The v2.0.0 portable updater rejects the ConPTY files in normal release ZIPs.
Updating the published ZIP cannot repair that already installed updater.
For affected users, close NyaTerm and back up the entire `data/` directory, then
download the matching architecture's portable ZIP and replace the complete
program file set, including helpers and `conpty/`. Preserve the existing `data/`
directory; do not replace it with the ZIP's empty data directory.

Portable packaging and update validation share
`crates/nyaterm-core/src/packaging/windows-portable.json`. Change this contract
when adding a packaged component. Release and snapshot jobs test their actual
ZIPs through the Rust extraction and replacement code before uploading them.
To repeat this check locally in PowerShell:

```powershell
$env:NYATERM_TEST_PORTABLE_ARCHIVE = (Resolve-Path 'dist/NyaTerm_2.0.1_windows_x64_portable.zip').Path
$env:NYATERM_TEST_PORTABLE_ARCH = 'x86_64' # 'aarch64' for the ARM64 ZIP
cargo test -p nyaterm-desktop --lib --locked portable_release_archive_round_trip -- --ignored
```

The test operates on isolated temporary directories and does not run the ZIP's
executables. Before release, also verify helper launch, file-lock recovery and
application restart on Windows 10 and Windows 11. If rollback fails, retain the
reported `portable-backup` directory for recovery; it must not be automatically
cleaned or followed by an automatic application restart. Include
[the prepared upgrade notice](notes/portable-update-recovery.md) in the next
version's release notes.
