# GPUI cutover and release readiness

This is the execution record and runbook for the **draft** cutover PR. Version
`2.0.0` in Cargo is a stable-identity candidate, not evidence of publication.
Do not merge main, create `v2.0.0`, or publish channels as part of preparation.

## Frozen sources and history

| Reference | Commit / disposition |
| --- | --- |
| Former main / `archive/tauri-v1` | `18e6f2dafee9a70b4cdb55cad7b32108460ed4f3` |
| GPUI / `migration/gpui` | `25ec4166af2ec53761fcf67406d559a6d7079385` |
| History-only ours merge | `8f76a60ceba9a6c177963aec17754637e4f3bcbe` |
| Candidate C | The full `cutover/gpui` PR head SHA recorded with its CI/rehearsal run |

The ours merge was checked for an identical tree to the GPUI baseline and for
ancestry of both frozen sources. Preparation commits intentionally change that
tree; compare the eventual main tree with **C**, not with the original baseline.
Do not use squash or rebase merging for this PR.

Before merging, fetch both branches again. If either source advanced, review the
new work, reconcile the cutover without force-pushing shared history, and repeat
CI, rehearsal, review and tree checks for the new candidate. Never silently mark
new legacy changes as integrated by adding an unreviewed ours merge.

## Legacy change classification

`git log --no-merges <GPUI>..<old-main>` contains 883 commits. This is a history
difference, not a list of 883 missing GPUI fixes. The application was rewritten;
do not cherry-pick the legacy implementation wholesale.

| Class / examples | Disposition and evidence |
| --- | --- |
| Tauri/WebView and Windows 7 UI (`30f6463fa`, `df6448dc7`) | Legacy platform-specific implementation; GPUI has no WebView and requires Windows 10+. Validate remote-desktop Ctrl+R using GPUI input handling. |
| Version/release (`6af18f59b`) | Superseded by workspace version 2.0.0 and the native package workflow. Preserve updater signature identity and data paths. |
| tmux control mode (`42d45c35c`, `f612a904b`, `34dc74768`) | Legacy protocol behavior; no assumption of equivalent GPUI support. Keep special-key, bracketed-paste and protocol-state cases in the feature-gap review before release. |
| AI command capture/custom instructions (`7d9ce7d24`, `43ab3c2d7`) | Do not claim parity from the presence of an AI feature. Explicit feature-gap review and heredoc command-capture regression remain required before release. |
| Cloud sync (`cbc57fc93`, `898a27a5c`) | GPUI has `features/sync/auto_sync.rs` and core cloud-sync regression tests. Startup/retry, pending local edits and partial merges still require isolated end-to-end validation. |
| Terminal echo/history and external MCP approvals (`69c9e89d3`, `327c59bea`) | GPUI owns different state and input paths. Keep wrapped history and repeated approval cases in the release regression matrix. |
| Other old UI, translations, documentation and domain work | Retain archive for comparison; the classification above is a preparation review, not a claim of exhaustive behavioral equivalence. Remaining feature-gap review is a release gate. |

No pending PR is included in the frozen candidate:

| PR | Existing base | Follow-up |
| --- | --- | --- |
| #991 WSL UNC working directories | migration/gpui | Re-evaluate/rebase for GPUI main after cutover |
| #974 VS16 emoji width | migration/gpui | Re-evaluate/rebase; retain terminal rendering regression |
| #972 SQL prompts / highlighting / save errors | migration/gpui | Re-evaluate/rebase by concern |
| #990 MCP notification audit | main | Port relevant behavior to GPUI, not Tauri source files |
| #854 NyaScript | main | Separate GPUI feature proposal |
| #853 Windows Mosh | main | Separate GPUI feature proposal |
| #718 terminal glyph painting | main | Compare with GPUI terminal renderer before porting |

Leave these PRs open and unchanged during preparation.

## Reproducible signed rehearsal

Run from PowerShell after committing and pushing C. Dispatch the workflow on the
same candidate branch, with the actual full checkout SHA as `source_ref`:

```powershell
$candidateSha = git rev-parse HEAD
gh workflow run release.yml --repo nyakang/nyaterm --ref cutover/gpui -f version=2.0.0 -f source_ref=$candidateSha -f upload_release=false -f verify_release=true
```

`verify_release=false` remains the default build-only behavior. Verification
uses the existing updater signing secret and public key, but no GitHub release,
tag, R2 object or downstream package is written. The existing downstream
workflows only publish after a successful Release run with a validated public
publication receipt; the channel commit and receipt jobs must be skipped in a
rehearsal. Before cutover the old main workflows listen to
`Build and Release`, not this `Release` workflow.

`release-candidate` retains `dist-release/`, `provenance/`, and
`release-notes.md` for 30 days. Seven v2 target/variant provenance records bind
actual checkout SHA, Cargo version, producing run ID, compiler, lockfile, build
configuration and package SHA-256 values. A stable candidate contains 20 package
files (including focal sidecars); a prerelease contains 18. Both include global
checksums and manifests. Signing is
verified with minisign before `.sig` sidecars are removed; signatures remain in
`latest.json`. Keep the full artifact and its run link in the PR evidence.

An optional `source_run_id` must identify a completed run of this repository's
Release workflow with a saved verified signed candidate. A publication failure
after candidate verification does not invalidate that candidate. Reuse is
accepted only for verification or publication. Source SHA and version must match
the requested checkout, all seven v2 records must exist, and every package hash
and manifest signature must match. The run's `head_sha` is
not used as a substitute for the actual checkout SHA. Old runs without these
records must be rebuilt. A later merge commit has a different SHA: build and
validate that final source again rather than reusing C's packages under it.

The rehearsal validates manifest structure, package hashes and signatures
offline. Its canonical release URLs are not expected to exist yet. Network
asset probes and downstream distribution remain post-publication checks.

## Validation evidence and outstanding acceptance

Record command, OS/architecture, full source SHA, run URL, artifact hashes,
result and any limitation in the PR. Re-run checks after candidate changes.

Required automated checks:

```powershell
cargo check --workspace --locked
cargo test --workspace --locked --no-fail-fast
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
python scripts/ci/test_scripts.py
python scripts/ci/check_docs_translations.py
pnpm --dir docs-site install --frozen-lockfile
pnpm --dir docs-site build
```

Run actionlint 1.7.7 as well. Signature integration tests require minisign;
the signed rehearsal installs it and runs them without skipping. The former
baseline Rust run `40272634203` failed fetching vnc-rs over SSL and is not a pass.

| Acceptance scenario | Required evidence / current disposition |
| --- | --- |
| Windows x64/ARM64 NSIS upgrade from 1.3.1 | Isolated OS installation, data/key preservation, shortcuts/uninstaller; pending real-machine validation |
| Windows old MSI → NSIS | Backup, uninstall without data removal, same-architecture install, no duplicate entries; pending |
| Windows portable manual overwrite | Full payload including RDP/VNC/MCP/ConPTY, preserved data and keys, helper launch; pending |
| macOS Intel/Apple Silicon | Same-identity bundle replacement and Homebrew path; pending |
| Linux x64/ARM64 | AppImage replacement and deb/rpm package-manager upgrades; pending |
| Legacy data and backups | With/without master password, wrong password, corrupt backup, credentials, known hosts, OTP, unknown-field retention, old `.nya`, restart persistence and full-copy restoration; pending end-to-end fixtures |
| Cloud sync | Dedicated remote namespace, no production credentials; startup/retry/merge and restored backups; pending |
| Sessions/input | SSH/SFTP, local shell, IME, RDP/VNC startup and disconnect; pending interactive validation |

Use synthetic or explicitly supplied sanitized 1.3.1 data in isolated accounts
or VMs. Do not install over the operator's application or open their real data
for acceptance. Cross-compilation and unit tests do not satisfy these rows.
Missing machines or fixtures remain visible blockers for stable publication,
not blockers to creating the draft cutover PR.

## Merge and publication gates (outside preparation)

1. Complete outstanding migration, feature-gap and platform acceptance. Freeze
   both source branches for the merge window. Record the exact reviewed C.
2. Require one approval and successful `Rust gate` / `Docs gate` for C. Configure
   those two required checks before merge; do not weaken the existing ruleset.
3. Merge with a Merge Commit, fetch main, then verify in PowerShell:

   ```powershell
   git diff --exit-code $candidateSha origin/main
   git merge-base --is-ancestor 18e6f2dafee9a70b4cdb55cad7b32108460ed4f3 origin/main
   git merge-base --is-ancestor 25ec4166af2ec53761fcf67406d559a6d7079385 origin/main
   ```

   Every command must exit zero. Verify the main CI and signed rehearsal again
   for its final SHA; do not publish a tag against a different tree.
4. Save the current root `latest.json` and `downloads.json` from the R2 bucket,
   their public responses and SHA-256 values, current GitHub Latest tag,
   Homebrew/AUR revisions and Gitee release state in a dated operator backup.
   Record object existence at the R2 origin as well: a public 404 alone does not
   prove the object is absent. At preparation time the public `latest.json` was
   1.3.1, while root `downloads.json` returned 404. If an origin object is absent,
   record that fact rather than inventing a backup. Keep URLs/configuration only
   in the record, never credentials.
5. Only after all gates pass, create the matching formal tag on final main.
   Tag push automatically starts public publishing; it is not a dry run.
6. Verify GitHub Latest, all signed update/download URLs, public manifests,
   website downloads, Homebrew, AUR and Gitee. Do not call downstream workflows
   during preparation. Keep archive and migration branches through stabilization.

## Stop distribution and recover

The current CI/publication topology, v2 provenance requirements, immutable
artifact checks and channel/Snapshot journal recovery commands are documented in
[Release tooling](README.md#channel-interruption-and-recovery). Preserve these
records alongside the operator backups described below.


Stop/cancel the affected Release and downstream publishing runs first so that
they cannot reapply the faulty channel. Preserve the failed run and evidence.
Restore **both** root R2 manifests from the saved pre-release copies, with JSON
content type and `public, max-age=60, must-revalidate`. If an origin manifest
was absent before publication, remove only that newly created root manifest to
restore its recorded absence; do not delete versioned assets. Verify origin bytes and
public responses, allowing for cache expiration or explicitly purge those URLs.
Recheck the website's download redirects and manifest-driven links.

Restore the prior GitHub release as Latest; mark the bad release as prerelease
or withdraw it as appropriate. Handle Gitee and package-manager updates
separately: restore the recorded downstream metadata or ship a forward fix,
and verify their actual public state. Hiding a GitHub release does not restore
R2 manifests or roll back already installed packages.

Do not overwrite immutable versioned artifacts with different bytes. Prefer
`2.0.1` for a correction. Users recover from their complete pre-migration copy;
never instruct 1.x to open data that 2.0 has already modified.
