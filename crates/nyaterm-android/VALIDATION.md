# Android / ADB validation

## Windows planar screen rendering validation — 2026-11-15

The screen path now uses IPC **4**, with validated BGRA/I420 layout and explicit
matrix/range metadata. Mixed helper versions are rejected. H.264 decoding stays
inside the isolated helper. I420 is requested only when the renderer supports it;
upload/paint failures request a cached BGRA frame. Hardware decoding and audio
are outside this change.

Saved settings retain their existing names and bitrate units (bits/s); the UI
edits Mbps. New wake/orientation fields have compatibility defaults. Quality
presets and per-session drafts do not overwrite saved connection defaults.
Applying a session draft restarts only that screen channel. Tests cover legacy
settings, round trips, presets, plane bounds, odd sizes, color metadata, format
switches, cached-frame refresh and session ownership.

Published fork revisions:

- Zed/GPUI `nyakang/zed`, branch `nyaterm`,
  `2302414991d9495c2b0c6a1b5534689a58da06c4` (common API, Windows, Metal, wgpu
  commits remain separate).
- GPUI kit `nyakang/gpui-kit`, branch `nyaterm`,
  `7119d8193f559bd5a692ee6c8ee01f6365e4e499` (aligned GPUI dependency).

Windows DirectX native GPU readback passed (limited/full-range black and white,
BT.709 red, opacity, clipping and texture removal). Both HLSL entrypoints compile.
Four wgpu shader/layout tests passed, including storage and WebGL shader variants.
Metal and Linux/wgpu native rendering still require platform acceptance. Full
interactive application toolbar/dialog/resize acceptance and end-to-end display
latency have not been measured by these renderer and headless tests.

### Windows live-device probes

An authorized device completed two six-second 30 FPS I420 visibility cycles and
one six-second 120 FPS cycle. Each delivered 12 I420 frames at 576×1280; first
frame times were 148–162 ms after screen startup. Hidden frame IPC stopped and
cached frames resumed on visibility restoration. No device key input or clipboard
changes were sent. Device identity and pixel contents are intentionally omitted.
The mostly static display verifies negotiation and lifecycle, not sustained
120 FPS presentation.

### Automated checks

- `cargo check --workspace`: passed with the published fork revisions.
- `cargo clippy --workspace --all-targets`: passed after resolving warnings in
  this change. Existing unrelated unused-mut (store backup) and unused-import
  (desktop transfer editor) warnings remain.
- `cargo fmt --all -- --check`: passed.
- `python scripts/ci/check_architecture.py`: passed.
- `cargo test --workspace --no-fail-fast -- --test-threads=2`: 6730 passed,
  8 failed, 60 ignored. The run is **not green**: seven untouched
  store migration tests compare canonical/verbatim or long/8.3 paths as strings.
  A focused rerun of `complete_migration_preserves_credentials_unknown_fields_plugins_and_recordings`
  reproduces the verbatim-prefix mismatch. The cross-process migration-lock test
  blocked in its child database open; that identified test child was terminated
  after several minutes so the remaining workspace tests could continue. This
  produces an eighth store failure. No storage production code was changed.
- Android desktop regression filter: 37 passed, including draft cancellation,
  input validation, focus restoration and session settings ownership.
- Android client/helper tests: 32 client + 9 helper + 4 lifecycle tests passed.
- Native DirectX readback: 1 passed; wgpu shader/layout validation: 4 passed.

### Synthetic helper comparison

Windows x64 MSVC release, FFmpeg 8.0.1 helper, FFmpeg 7.0.2 synthetic H.264
producer. Each condition ran for 10 seconds, three times, alternating order.
There are 48 samples in
[`android_screen_yuv_results.json`](../../scripts/bench/android_screen_yuv_results.json).
The table reports medians across the three runs, BGRA baseline → I420 candidate.

| Size / FPS | Helper p95 (ms) | CPU (% of one core) | Peak RSS (MiB) | Delivered FPS |
| --- | --- | --- | --- | --- |
| 1280×720 / 30 | 4.67 → 2.49 | 10.31 → 6.72 | 20.63 → 14.60 | 30.00 → 30.00 |
| 1280×720 / 60 | 4.78 → 2.47 | 13.75 → 9.53 | 20.55 → 14.61 | 60.00 → 60.00 |
| 1280×720 / 120 | 7.03 → 3.35 | 30.16 → 21.25 | 21.85 → 14.64 | 119.50 → 119.99 |
| 1920×1080 / 30 | 9.11 → 4.40 | 16.25 → 8.44 | 34.55 → 21.51 | 30.00 → 30.00 |
| 1920×1080 / 60 | 9.02 → 4.31 | 37.03 → 30.15 | 34.54 → 21.52 | 60.00 → 60.00 |
| 1920×1080 / 120 | 15.42 → 6.00 | 94.69 → 59.22 | 37.57 → 21.68 | 119.49 → 119.90 |

Pixel payload per equal-size even-dimension frame falls by 62.5%. All hidden
three-screen samples emitted zero frames. Decode still runs while hidden; this is
not a promise of zero background CPU. CPU samples are noisy. These measurements
exclude ADB capture, GPU upload, display and user-input latency. The candidate
binary precedes the final unsupported-color-space fallback adjustment, which does
not affect the normal I420 stream measured here.

## Windows IPC 3 validation — 2026-11-11

The application and helper use IPC **3**, including capture dimensions and
`RefreshPresentation`. Mixed IPC versions are rejected. No stored data format,
credential, dependency revision, database, backup or sync format changed.

Environment: Windows x64, MSVC dev/test profile, pinned FFmpeg 8.0.1 from
`prepare_android.py`. `FFMPEG_DIR` points to the prepared dependency directory;
`LIBCLANG_PATH` points to the local LLVM installation. No release was published.

Regression coverage includes:

- Pending first Shell ownership across repeated collection; cancellation during
  the native handshake; discarded and undeliverable successful channel cleanup;
  unchanged-binding reconnect with sibling views; selector changes; independent
  screen focus; transfer lease conflict and rollback; runtime invalidation even
  when the tracker revision does not change.
- Conservative mount classification and component boundaries; default new-file
  mount-assigned permissions; strict explicit permissions and overwrite
  attributes; unknown mounts; cancellation before commit and temporary cleanup.
  Accepted Windows mock sockets restore blocking mode. Android protocol filenames
  and Windows host filename rejection are tested separately.
- Actual FFmpeg conversion of 2560×1920, 2560×2560 and rotated captures inside
  the 16 MiB BGRA limit; capture-coordinate preservation; malformed dimensions
  and zero generations; recyclable reads; bounded buffers; reliable controls
  under queue overflow; static-frame presentation without new media; native SPS
  allocation edge limits; separate presentation/writer wakes and no per-frame
  presentation wake when hidden.
- Pointer-anchored zoom, pan boundaries and fit reset. The GPUI fixture renders
  two screen input surfaces and tests blur isolation and host paste modifier
  release. These headless tests do not constitute native desktop acceptance.

Automated checks (Windows):

- `cargo test --workspace --no-fail-fast -- --test-threads=2`: **6490 passed,
  0 failed, 57 ignored**, including unit, integration and documentation tests.
- Follow-up package runs after the last focused changes: Android **29 passed**;
  helper **7 unit + 4 lifecycle passed**; desktop Android selection **31 passed**.
  The broader run preceded the final maintenance/ACK regression additions and the
  helper's separate wake channels; these follow-up runs verify those changes.
- Files **241 unit + 2 integration passed** (7 ignored across targets), transport
  **519 unit + 5 integration passed** (21 ignored across targets), and core
  **853 passed / 2 ignored** in the broader run. Ignored native/integration probes
  were not treated as passes.
- Latest `cargo check --workspace`, `cargo clippy --workspace --all-targets`,
  `cargo fmt --all -- --check` and `python scripts/ci/check_architecture.py` passed.
  Clippy emitted no warnings. `git diff --check` passed.
- `cargo build -p nyaterm-android-helper` and benchmark runner compilation passed.

The first broad run exposed the new GPUI fixture's missing theme initialization;
subsequent focused runs also corrected activation and explicit key-up delivery in
that fixture. The corrected focus/paste test and final broad run pass.

### Synthetic performance comparison

The reproducible runner is [`android_screen.py`](../../scripts/bench/android_screen.py).
It feeds identical synthetic 1280×720 H.264 into baseline and candidate helpers,
uses the same dev profile and dynamic FFmpeg libraries, warms up for 0.5 seconds,
then samples for 4 seconds. It records process CPU, peak working set, pipe frame
rate, timestamp-to-pipe latency and five static-frame resume samples. CPU is a
percentage of one logical core; hidden-screen memory and CPU are summed across
three helpers. It stores counters and times only.

Raw results: [`android_screen_results.json`](../../scripts/bench/android_screen_results.json).
Baseline: `2157c2c9f68277ec5f58f0fc8fa66630423959a1`; candidate: this working tree.
The final run took place after Cargo and workspace tests exited. Columns show
baseline → candidate; latency is milliseconds and memory is MiB.

| Scenario / FPS limit | CPU (% of one core) | Peak RSS | Presented FPS | Frame latency p95 | Static resume p50 |
|---|---:|---:|---:|---:|---:|
| 1 visible / 30 | 10.15 → 14.45 | 20.86 → 20.94 | 30.0 → 30.0 | 4.61 → 4.73 | 21.81 → 2.94 |
| 1 visible / 120 | 30.47 → 20.7 | 20.82 → 22.25 | 90.49 → 119.49 | 19.25 → 7.06 | 23.09 → 2.94 |
| 3 hidden / 120 | 26.95 → 61.33 | 40.73 → 40.88 | 0.0 → 0.0 | — | 22.87 → 3.18 |

Both builds received 120 frames in the 30 FPS sample, 479 in the 120 FPS sample,
and 1441 summed across the hidden streams. Three hidden screens emitted no
frames in either build. The visible high-FPS path and static resume improved;
30 FPS and the three-hidden CPU sample increased, while working sets stayed
similar. This is a short dev-build comparison, not a universal CPU improvement.
Two additional hidden samples per build, with reversed order, ranged from
69.13–86.33% for the baseline and 42.57–66.40% for the candidate (same nominal
input rate). The raw artifact retains these repeats. Their large variation means
these short CPU samples cannot establish a general CPU reduction or regression.
The CPU cost of continuously decoding multiple hidden streams remains a
performance acceptance item; no real-device CPU gain is claimed.

These are helper-only measurements: ADB capture, USB/network transport, GPUI,
texture upload and display latency are excluded. Synthetic hidden-frame counts
and mocks cannot establish real-device end-to-end performance.

### Outstanding acceptance

Device discovery found **0 online devices**. The following have **not** been
executed for IPC 3 and remain required acceptance work:

- Real uploads, strict overwrites and cancellation under `/sdcard/` and an
  independent `/data/local/tmp` test directory.
- Windows GPUI split-screen typing, held-key focus switches, window deactivation,
  migration of a static image, reconnect with active transfers, zoom/pan/touch,
  rotation, clipboard insertion and native drag/drop.
- Real-device CPU, memory, frame rate, latency and restore latency at default
  1280/30, high FPS and multiple hidden screens; macOS/Linux/ARM64 behavior.

## Historical Linux IPC 2 validation

The following is the previous revision's record. Its device probes and test
counts **do not validate the current IPC 3 implementation**.

This change preserves stored configuration and data formats and does not change
fork revisions. Android helper IPC is version 2; build and package the application
and helper from the same revision. The existing native packaging workflow builds
both and stages the pinned FFmpeg libraries beside the helper.

### Behavior covered

- Ordinary upload/download and Windows export staging use one SEND/RECV per
  successful attempt. Incremental Sync progress is accumulated once; copies
  through local staging count both network legs while their summary counts the
  content once. Scanning, transferring and committing are distinct task phases.
- Temporary upload and safe edit share the commit policy. Overwrites preserve
  permissions and ownership; unavailable attributes fail before replacement.
  New files explicitly apply their requested permissions because adbd may broaden
  the SEND mode. Timestamp preservation is verified. Edits require a revision
  unless forced and recheck the content revision after staging.
- Ordinary metadata checks cannot detect same-size changes with unchanged mtime.
  Editor revisions still detect those changes. The final shell rename has the
  same best-effort concurrency boundary as SSH; it is not atomic compare-and-swap.
- Tracker generations invalidate old bindings even after a quick offline/online
  transition. Native selection verifies serial and transport on the same channel;
  older servers fall back to serial selection with mapping checks. A binding owns
  at most two exclusive reusable Sync channels; failures discard their channels.
  Waiting for a capability probe checks cancellation and deadlines even when the
  task that owns the probe is paused.
- Network idle budgets exclude pauses. Paused transfers and virtual-file reads
  exit on binding invalidation. Inventory limits are 65,536 entries and 128 levels.
  Multiple window leases release after all views close; submitted tasks and native
  descriptors retain the service until their work finishes.
- Screen startup has a cancellable 30-second deadline. CLI processes and inherited
  output pipes share a bounded deadline. Invalid text is rejected without ending
  the screen session. Visibility messages coalesce and survive an input backlog.
- Hidden screens continue H.264 decoding while skipping BGRA conversion and frame
  IPC. Restoring visibility presents the latest cached frame, including static
  content. GPUI wakes from events and uploads on the next frame.
- Failure messages are localized. Stable log fields contain operation, backend,
  request ID and error code; paths, device IDs, commands, output and input text are
  excluded. User cancellation does not emit failure warnings.

### Linux device probes

The probes ran against one connected device with the pinned ADB/scrcpy/FFmpeg
dependencies. Device identifiers and test contents are intentionally omitted.

- `android_files_probe`: six special filenames, 1 MiB each, byte comparisons,
  exclusive commit, cancellation protection and temporary cleanup passed. The
  probe used a unique test directory under `/data/local/tmp` and removed it.
- `screen_probe` visibility mode: requested 30 FPS and 120 FPS separately for six
  seconds at the default maximum size of 1280. Both produced 576 x 1280 frames,
  with the first frame around 188–206 ms on the probe's post-start timer across
  the initial and final runs. Hidden frame IPC stopped; restoring
  visibility returned a cached frame; cleanup passed.
- The mostly static screen yielded 12 frames in each run. This demonstrates that
  the 120 FPS setting is accepted, not that 120 FPS presentation was achieved.

### Remaining platform and performance acceptance

This Linux environment has no X11/Wayland display. Actual GPUI system drag-in,
dual-pane dragging and the Linux drag-out notice have not been exercised through
a desktop file manager. Windows virtual-file drag-out and macOS file promises
have automated coverage but no native device/file-manager acceptance here.

The before/after CPU comparison at 1280/30 FPS, presentation FPS and end-to-end
latency, and 120 FPS on a high-refresh display remain manual acceptance items.
Mock tests and Linux device probes do not substitute for those measurements or
for Windows/macOS execution.

### Automated checks

The final builds and device probes use the dependencies reported by
`scripts/release/prepare_android.py` through `FFMPEG_DIR`.

- `cargo check --workspace`: passed.
- Workspace unit, integration and documentation tests: 6315 passed / 40 ignored.
  Core reported 839 passed / 2 ignored; desktop reported 2860 passed / 15 ignored.
  The final Android capability-wait regression and ordinary-user file tests were
  also rerun after the last runtime change.
- `cargo clippy --workspace --all-targets`: passed without warnings.
- `cargo fmt --all -- --check`: passed.
- `python3 scripts/ci/check_architecture.py`: passed.
- `cargo test -p nyaterm-android`: 27 passed, including inherited-pipe deadlines,
  incomplete handshake cancellation, input limits, cleanup before PID receipt and
  cancellation while waiting for a paused capability probe.
- Android desktop state/error tests: 11 passed, including shared-window resource
  release and typed, localized and redacted failure reporting.
- The complete file-service unit test binary under the ordinary `nobody` account:
  239 passed, 1 ignored, including existing-target permissions and read-only safety.
- `python3 -m unittest scripts.tests.test_package_native scripts.tests.test_verify_native_package`:
  53 passed, 1 skipped.
- The application, Android/RDP/VNC helpers and MCP executable were built together.
  A local debug bundle stages these executables and the pinned Android resources;
  the bundled Android helper successfully reports IPC 2 / version 2.0.4. No
  distribution installer was produced and nothing was published.

The workspace test run uses `SHELL=/bin/sh` to avoid this host's Bash profile
initialization (about nine seconds in an isolated shell-cache test). It runs with
two test threads and skips only
`download_path::tests::staged_write_preserves_existing_permissions` as root; the
ordinary-user file-service run above exercises that test.

Final workspace command:

```sh
SHELL=/bin/sh cargo test --workspace --no-fail-fast -- \
  --test-threads=2 \
  --skip download_path::tests::staged_write_preserves_existing_permissions
```
