# ADB runtime and transfer validation

This change preserves stored configuration and data formats and does not change
fork revisions. Android helper IPC is version 2; build and package the application
and helper from the same revision. The existing native packaging workflow builds
both and stages the pinned FFmpeg libraries beside the helper.

## Behavior covered

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

## Linux device probes

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

## Remaining platform and performance acceptance

This Linux environment has no X11/Wayland display. Actual GPUI system drag-in,
dual-pane dragging and the Linux drag-out notice have not been exercised through
a desktop file manager. Windows virtual-file drag-out and macOS file promises
have automated coverage but no native device/file-manager acceptance here.

The before/after CPU comparison at 1280/30 FPS, presentation FPS and end-to-end
latency, and 120 FPS on a high-refresh display remain manual acceptance items.
Mock tests and Linux device probes do not substitute for those measurements or
for Windows/macOS execution.

## Automated checks

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
