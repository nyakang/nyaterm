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
verify all six native targets and the glibc 2.31 tar.gz/deb variant, sign updater
payloads, and generate/verify the manifests without publishing. The `release-candidate` Actions artifact retains
packages, manifests, hashes, release notes and per-target source records for 30
days. This is distinct from OS code signing/notarization.

`source_ref` is resolved once to a full SHA. Reusing `source_run_id` requires a
verified signed candidate from a completed run of this repository's Release workflow, complete v2
source/version/target/variant/compiler/lockfile/build-configuration records and
package hashes. The existing signed candidate is verified and reused byte for
byte, without resigning or regenerating manifests. Incomplete historical runs
must rebuild all packages, including glibc 2.31. Supported v1 records remain
readable with `release_provenance.py verify --legacy`, but cannot authorize a
new publication. Reuse is
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
cargo test -p nyaterm-updater --lib --locked portable_release_archive_round_trip -- --ignored
```

The test operates on isolated temporary directories and does not run the ZIP's
executables. Before release, also verify helper launch, file-lock recovery and
application restart on Windows 10 and Windows 11. If rollback fails, retain the
reported `portable-backup` directory for recovery; it must not be automatically
cleaned or followed by an automatic application restart. Include
[the prepared upgrade notice](notes/portable-update-recovery.md) in the next
version's release notes.

## CI and publication topology

Rust and Docs select jobs for PRs, branch pushes and Merge Queue revisions.
Manual runs and the Sunday 03:00 Asia/Shanghai schedule (`0 19 * * 6` UTC) run
full regular CI. `Rust gate` and `Docs gate` always report. Rust changes retain
Linux x64, macOS arm64 and Windows x64 tests. Rust 1.99.1 is pinned in the root
`rust-toolchain.toml`; frozen diagnostic builds keep their requested compiler.

`python scripts/ci/test_scripts.py` discovers script tests. Real Cargo/WiX
integration classes are excluded from this common suite and run explicitly on
Windows using `--suite windows-resources`, `--suite windows-msi` and
`--suite diagnostic`. Selected native suites fail on missing tools, and every
suite records its run/skip/failure counts in Job Summary.

Release and Main Snapshot call the same native packaging workflow. Release also
calls Linux glibc 2.31, whose ABI audit, helper lifecycle and CPU fallback smoke
must pass. All compatibility packages and their sidecars enter `SHA256SUMS` and
versioned GitHub/R2 assets. Existing `latest.json` and `downloads.json` platform
keys are unchanged; compatibility users continue to upgrade manually.

Publication first validates the candidate, stages matching GitHub assets and
conditionally creates immutable R2 version assets. Existing bytes must match;
changed bytes require a new version. Stable and preview promotion jobs have
separate non-cancelling locks. Within the lock, both old manifests (including
recorded absence) are backed up before any writes. SemVer downgrades and
same-version/different-byte changes are refused. Origin and public manifests
are verified before making the GitHub release public. GitHub and R2 do not
provide a shared atomic transaction; the journal records completed stages.
GitHub concurrency does not guarantee FIFO and can replace a pending job; rerun
an affected candidate explicitly. The version guard still prevents regression.

A successful run uploads `published-release/release-receipt.json` with schema,
tag, frozen source SHA, channel, run ID and `SHA256SUMS` digest. Homebrew, AUR and
Gitee use the shared resolver and verify the actual published release. During
migration the resolver also accepts the old `published-release-tag` artifact.
Signing rehearsals and build-only runs have no publication receipt and cannot
start distribution. Homebrew/AUR accept stable versions; Gitee also accepts
previews. Each channel retains its manual retry input; no package rebuild is
needed when just one distributor fails. Gitee's `upload_small_assets=false`
remains false on manual runs.

## Channel interruption and recovery

Prefer **Re-run failed jobs** on the original Release run. A partial channel
write can resume with the same verified candidate; a published GitHub release
can finish a failed receipt upload without changing its bytes. For a new
publication run, use `source_run_id` from a complete signed rehearsal or an
interrupted publication with a saved verified candidate, and the same frozen
`source_ref`. The source run must have completed; it need not have succeeded if
publication failed after candidate verification. Keep repository build variables unchanged;
build configuration identity is checked during reuse.

The `channel-preimage-<attempt>` artifact is saved before writes, and
`channel-recovery-<attempt>` records progress even on failure (90-day retention).
To restore a failed channel, first stop competing publishing runs. Download the
chosen artifact into a fresh recovery directory, configure the same existing
R2 credentials used by CI, and run from the matching source checkout:

```sh
python scripts/release/release_channel.py restore --version 2.0.2 --recovery /path/to/channel-recovery
```

Restore checks every backup before writing, preserves original absence, refuses
to overwrite a newer channel, and verifies both origin bytes and public
responses. It does not delete immutable versioned packages or change installed
applications. GitHub Latest/publication state and downstream package-manager
metadata must be recovered separately following the existing cutover runbook.

## Fixed Snapshot recovery

Main Snapshot remains manual, globally serialized and not automatically
cancelled. Before overwriting any fixed-name asset, the workflow saves the old
release metadata, tag object and all assets in `snapshot-preimage-<attempt>`.
It uploads and reads back every new asset before changing the tag/notes, then
removes obsolete assets. Fixed attachment replacement is not atomic; partial
uploads retain the old tag and can be restored. A first-ever snapshot is created
as a draft until uploads pass. Recovery artifacts are retained for seven days.

After stopping competing Snapshot runs, download the matching preimage or
recovery artifact into a fresh directory and use an existing repository token
with the same release permissions as the workflow:

```sh
python scripts/release/snapshot_release.py restore --recovery /path/to/snapshot-recovery
```

Restore verifies backup hashes and refuses a tag moved by a later snapshot. It
restores old attachments, metadata and the exact previous tag object, or removes
only the initial snapshot created by the interrupted first run. The progress
journal is retained as `snapshot-recovery-<attempt>` even when publication fails.
