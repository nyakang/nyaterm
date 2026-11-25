# SPICE integration — implementation and acceptance record

This isolated native SPICE backend is connected to the desktop editor, session
lifecycle, GPUI presentation and release packaging. Automated validation is
recorded below; real-guest and remaining native-platform acceptance are tracked
in `MANUAL_TESTS.zh-CN.md`. Native probes, CPU benchmarks and contract tests do
not establish real-guest or cross-platform acceptance.

## Delivery contract

Deliver SPICE in the GPUI workspace on Windows x64/ARM64, macOS x64/ARM64 and
Linux x64/ARM64. The existing release targets are the platform baseline.

- Saved manual connections and transient `.vv` activation/open-file sessions.
- TCP, validated TLS/CA/subject, HTTP CONNECT (including Proxmox opaque host
  tickets), and existing authenticated SSH/proxy routes for every channel.
- Independent helper process, bounded IPC, heartbeat, crash/hang reaping,
  startup cancellation, reconnect policy and orderly shutdown.
- Native GPUI rendering with partial updates, bounded pixel memory, stale
  epoch rejection, hidden-session throttling and fair multi-display scheduling.
- Multi-monitor selection/presentation, dynamic resolution, local scaling,
  relative/absolute mouse modes, capture/release, keyboard mapping and focus
  loss recovery. No-agent boot/install consoles remain usable.
- Directional text clipboard, agent capability changes and useful disabled
  feature states; no leakage between tabs or through logs.
- Playback audio with per-session mute, no microphone capture by default.
- Explicit file sending with progress, cancellation and typed failures.
- Connection editor, icons, tab/quick-switch/session routing, shortcuts and
  internationalized controls using nyaterm-ui.
- Saved data round trips and old-format compatibility; unknown fields survive;
  transient tickets never enter backups, sync or startup restoration.
- Reproducible runtime preparation, dependency/notice collection, package
  integrity checks, updater integration, CI and a manual acceptance guide.
- Automated correctness and lifecycle tests, meaningful performance baselines,
  and explicit records of what requires real QEMU/Proxmox/platform validation.

USB, smartcard, microphone capture, persistent directory sharing and a Proxmox
inventory/API integration were optional in the agreed technical analysis.
Importing a `.vv` containing their flags does not authorize local device access.
Unhandled file entries are retained, so UI can disclose unsupported requests.

## Current architecture and evidence

- `nyaterm-core::spice`: serializable preferences (unknown fields retained),
  masked endpoint/authentication contracts and side-effect-free bounded `.vv`
  parsing. The document itself intentionally does not implement Serialize.
- `nyaterm-remote-desktop::spice`: typed control IPC, monitor/surface identities,
  input validation, temporary credential policy and existing binary frame/cursor
  envelopes. Binary pixels retain shared ownership; they never go through JSON.
  Its session manager owns bounded ordered queues, the shared heartbeat supervisor,
  asynchronous process reaping and received-surface epoch/dimension validation.
- `nyaterm-spice-helper::native`: explicit-path C ABI loading, thread-affine
  runtime/session objects, native channel/display/cursor/input callbacks.
  All library handles stay pinned because GType retains module vtables.
  Agent callbacks implement directional text clipboard, atomic monitor changes
  and bounded cancellable file transfers. Playback uses an output-only CPAL
  worker, bounded PCM buffering, rate conversion and local/guest mute and volume.
- `nyaterm-spice-helper::frame`: stride-aware 16/32-bit conversion, damage
  merging, bounded region count and 4 MiB frame chunks. XRGB conversion uses
  SSE2 on x86-64 and NEON on little-endian AArch64, with a scalar tail/fallback.
- The executable driver validates handshake and command direction, rejects pixel
  commands before allocating their bodies, and keeps controls/native sources
  running under frame backpressure. Its output queue is limited to 16 MiB and
  two frame chunks, including the write currently blocked in the OS pipe.
  On Linux it requests at most 1 MiB of kernel frame-pipe buffering to reduce
  full-frame wakeups; denied growth retains the existing capacity. Application
  queue bounds, blocking mode, framing and cancellation remain unchanged.
- `relay`: private authenticated CONNECT adapter for application routes. Every
  connection sends the existing loopback relay token; missing routes fail closed.
  If the endpoint already has a proxy, the application route must terminate at
  that proxy, and the adapter preserves its CONNECT authentication.

The implementation loads the public spice-gtk 0.43 C ABI without linking GTK
into NyaTerm. Windows MSVC loaded the clang64 C DLL successfully. No C++ or
allocator-owned object crosses that boundary.

spice-gtk's coroutine sockets and video timers attach to the process-default
GLib context. Merely pushing a new thread-default context is insufficient.
Each isolated helper must acquire and pump the process-default context.
The coroutine leader is process-global. In particular, the Windows Fiber cannot
be reused from another OS thread after a runtime is dropped. Runtime loading
enforces a permanent process owner thread; native integration tests use separate
processes, matching the actual one-session-per-helper architecture.

The upstream `open-fd` path jumps past TLS initialization. Relays therefore use
the authenticated CONNECT adapter instead, retaining the original host, ports,
CA and subject verification in spice-gtk. Native tests verify that the TLS route
actually emits a TLS ClientHello instead of the plaintext SPICE link header.

The public `spice_session_new` eagerly initializes the USB manager. The backend
constructs the public GObject type with device redirection disabled instead.
Native diagnostics are redacted because library errors can contain tickets.

The desktop now provides saved connection editing, independent TCP/TLS routes,
password loading/prompting, manual settings refresh on retry, bounded automatic
reconnection, background multi-surface composition and selected-monitor GPU
uploads. Monitor switches retain channel cursors; resizing preserves nonoverlapping
layouts and reflows overlapping displays horizontally within coordinate limits.
Clipboard permissions, playback mute and explicit file selection/progress/cancellation
are integrated. Windows system-key capture routes to SPICE. All platforms also
provide a Send keys menu for Ctrl+Alt+Delete, Alt+Tab/Shift+Tab/F4, Super/Windows,
Super+D/L and Ctrl+Alt+F1–F12. Each shortcut is one ordered input batch with balanced
key releases, available only for a connected writable session. This provides an
explicit guest route when the host desktop reserves a chord; non-Windows global
system-key interception is not claimed.

The new-session menu opens `.vv` files. Local file URLs and native filename
activation use the existing locked-screen/startup/dialog queue. Import shows TLS
policy and unsupported option names, lets the user select full screen and explicit
source-file deletion on Windows, and never saves temporary tickets. Deletion checks
both the opened file identity and original contents; Windows deletes using the
verified handle. Linux/macOS retain the source and display manual cleanup guidance:
POSIX pathname deletion cannot atomically match an already verified file identity.
This avoids deleting an editor's replacement in the check/unlink race.
Temporary sessions and their custom names are excluded from restore
serialization, and their failure view asks for a fresh file instead of retrying.

Saved SPICE connections participate in batch editing and cloud endpoint projection.
Batch changes address individual leaves and validate the final TCP/TLS combination
before any database write. Endpoint extensions survive local round trips and editor
changes; only reusable host/port/proxy fields enter sync. Display/security preferences
remain local, and proxy credentials and temporary ticket hosts are rejected by sync.

Installers register `.vv` as an alternate document handler. Windows NSIS/MSI use
flavor-specific OpenWithProgids and ownership-checked cleanup without changing the
existing default viewer. macOS declares an imported connection type with Alternate
rank. Linux packages include a shared MIME declaration and desktop entry; DEB/RPM
refresh MIME/desktop caches on installation and removal. Portable archives register
nothing in the host OS. Actual file-manager activation remains a manual acceptance item.

Relative mouse mode now uses the GPUI native pointer lock: click the display to
capture, and press **Shift+F12** to release. A modifier-only Ctrl+Alt chord is not
used because host AltGr layouts can produce that chord. Fractional motion is
accumulated without viewport scaling. Focus loss, disconnects, mouse-mode changes,
session changes and overlays revoke ownership and release held inputs. Windows
uses raw mouse input, X11 uses XI2 raw motion and a device grab, Wayland uses
one-shot pointer constraints and relative pointers, and macOS detaches the cursor
through Quartz. Unsupported Wayland compositors report capture as unavailable.

Native X11 probe (2026-11-23, Xvfb/Openbox): an 800x600 desktop delivered 50 motion
events totaling X=5000/Y=2000, with successful shortcut release, reacquisition and
focus-loss release. An additional 8,000-event synthetic burst preserved all 8,000
unit deltas on both axes and both releases; this is an input-integrity stress
check, not an end-to-end latency or physical polling-rate measurement. Windows
raw-input parsing and application lifecycle tests pass. The native Windows
backend also passes an ARM64 cross-target compile check.
macOS passes a cross-target Rust type check only; its host-gated Metal build script
was bypassed with a check-only shader-source include. This is not a native macOS
build or validation of rendering/cursor behavior. Physical-device and compositor
acceptance on all supported desktops remains necessary.

The native pointer API is published on `nyakang/zed`, branch `nyaterm`, revision
`5591fbd2d0ef56b9469e22216b9a91d833eb8282`. The aligned component fork is
`nyakang/gpui-kit`, branch `nyaterm`, revision
`c3e573ae50640dffff9f9778efa1f0829e814f94`. Both pins and Cargo.lock use one GPUI
type graph; no local source override is needed. Each fork records its patch and
validation in `NYATERM.md`.

The Ubuntu 20.04 compatibility recipe now stages the SPICE helper and private
runtime, audits the complete ELF closure against glibc 2.31 and retains the strict
application SONAME gate. It includes `.vv` DEB associations and extracted-package
video-element and helper IPC smoke probes. Python 3.8 source extraction validates
the whole archive before writing: reviewed GLib/json-glib license aliases and
Pixman's internal aliases become ordinary files; other links, escaping paths
and alias cycles are rejected. On an Ubuntu 20.04/glibc 2.31 chroot under WSL,
the prepared 28 MiB runtime passed all 10 video-element probes, the 44-object ELF
closure audit, 24 helper unit tests, nine lifecycle tests and five native/TLS tests.
The runtime builds private pinned GLib 2.88.4, Cyrus SASL 2.1.28, OpenH264 2.6.0 and
the matching GStreamer 1.16.3 plugin to fill focal's missing/older components.
This is helper/runtime evidence; the complete compatibility application package
and a real focal guest/desktop still require acceptance.

The macOS runtime now builds its dependencies from pinned source archives with
deployment target 12.0. Preparation and final package verification inspect each
Mach-O library/plugin and reject newer deployment floors. The complete portable
source recipe was exercised on focal: its 36 MiB runtime passed the 44-object ELF
audit, all ten video factories, relocated package-layout IPC, nine lifecycle and
five native/TLS tests. The subsequent gettext-runtime subdirectory configuration
and install were also executed successfully in a fresh focal build directory.
This validates the portable recipe on Linux, not macOS compilation or execution.

**Pending manual/native acceptance:** macOS and ARM64 execution, full Ubuntu
20.04 application distribution, real guest clipboard/file/audio/display behavior
and physical input on each desktop. See the manual checklist for exact cases.
SPICE input uses physical scan codes. Host IME committed text is rejected with
guidance to use the guest input method or clipboard; Unicode keystroke injection
is not provided by this integration.

## Validation performed on Windows x64

2026-11-23, AMD Ryzen 7 5700G, Rust 1.99.1:

After the native-pointer fork pins were published, `cargo check --workspace`,
`cargo clippy --workspace --all-targets` and the complete Windows workspace suite
passed (7,593 passed, 74 ignored). Later shortcut/source-cleanup and compatibility
packaging changes have separate focused validation; that count predates them.

```powershell
cargo test -p nyaterm-core spice:: --lib
# 22 passed
cargo test -p nyaterm-remote-desktop spice:: --lib
# Full remote-desktop library: 103 passed, plus an ignored performance baseline
cargo test -p nyaterm-spice-helper --lib
# 19 passed
cargo check --release -p nyaterm-spice-helper --all-targets
 cargo test --release -p nyaterm-spice-helper --features test-support --test lifecycle
# 8 passed, 1 requires native runtime

$env:NYATERM_SPICE_RUNTIME = (Resolve-Path target/spice-reference/runtime).Path
cargo run -p nyaterm-spice-helper --example probe
cargo test --release -p nyaterm-spice-helper --test native_runtime -- --ignored --test-threads=1
# 5 passed against real spice-gtk 0.43 DLLs
cargo test --release -p nyaterm-spice-helper --features test-support --test lifecycle -- --include-ignored --test-threads=1
# Native stalled-handshake cancellation is also exercised with this runtime.
```

The native tests allocate actual GObjects, exercise direct and authenticated
relay handshakes, verify TLS startup, and reject native-thread migration. Process
tests exercise cancellation during a stalled handshake and all worker-start
rollback points. A separate default-off fixture binary exercises crash reporting
and heartbeat timeout/reaping; it is never a release helper mode.
The TLS fixtures exercise both trusted and rejected CA/hostname cases. These tests
do not prove real-guest authentication, guest-agent interoperability, decoded
framebuffer presentation or actual audio-device output.

The helper also passed a Rust cross-target check for `x86_64-apple-darwin` from
Windows. This is not a macOS native-library load, linked package, or device test.

The original exploratory runtime in `target/spice-reference` is superseded by
the preparation scripts below. Do not copy the development helper into a release
without its verified runtime.

## Native runtime preparation

```powershell
python -m pip install pefile==2024.8.26 zstandard==0.25.0
python scripts/release/prepare_spice.py --target x86_64-pc-windows-msvc
# Set NYATERM_SPICE_RUNTIME to the absolute directory printed by this command.
cargo run --release -p nyaterm-spice-helper --example probe -- --require-video
```

`scripts/release/spice-windows-lock.json` pins both Windows architectures to
archive and extracted-file SHA-256 values. Normal preparation never queries the
rolling repository. `--refresh-windows-lock` is a deliberate maintainer operation;
its output and corresponding `spice-sources.json` version changes need review.
Each target currently selects a 40-package import closure, nine GStreamer plugin
modules, five SASL plugin modules, a CA snapshot and upstream license notices.
The manifest rejects modified package members and unexpected files. A cached
archive with a mismatched digest fails without being used. No unrelated GTK
widgets, audio-device plugins or USB tools are selected for the runtime.

On Linux/macOS, install `scripts/ci/install-spice-deps.sh` prerequisites and Python
`zstandard`/`pyparsing`, then run `prepare_spice.py --target <native-target>`.
Unix preparation builds the unmodified upstream revision
`02377912fe173e4af7714aef036d0555ee7531bc` with locked submodule revisions, GTK,
USB, smartcard, WebDAV and EGL disabled. That revision includes the macOS DRM
header fix absent from the 0.43 tarball. Its version string is `0.43.20-0237`.
The `ucontext` backend keeps coroutine callbacks on the GLib owner thread;
`gthread` must not be substituted without redesigning the Rust thread contract.
ELF/Mach-O dependencies are copied, architecture-checked and their load paths
rewritten. Mac libraries receive ad-hoc signatures after rewriting. On macOS,
Homebrew supplies build tools only; `spice_source_runtime.py` builds the private
runtime dependencies from `spice-sources.json`, uses the Apple SDK with deployment
target 12.0, and rejects runtime libraries outside that prefix. The same source
recipe can be exercised on Linux with `NYATERM_SPICE_SOURCE_RUNTIME=1`.
Source hashes, Linux distribution versions and copyright notices are recorded;
outputs are **not** claimed to be bit-reproducible between runner toolchains.
Mach-O deployment metadata is checked during preparation, runtime reuse and final
artifact verification. This gate does not replace execution on macOS 12; a probe
on a newer runner alone cannot prove the minimum-system compatibility claim.

The isolated helper configures private GStreamer, GIO, OpenSSL and SASL paths
before starting any threads. Windows DLL resolution excludes CWD and PATH.
GStreamer loads selected bundled plugins by exact filename and keeps its registry
only in memory. Automatic discovery and disk registry caching are disabled, so
obsolete plugins left by earlier installers cannot enter the video registry.
`GST_REGISTRY_UPDATE=no` alone is insufficient: a Linux probe demonstrated that
initial cache creation still writes a file. Both native probes now verify that
loading leaves the runtime inventory unchanged. The default CA is an explicitly packaged Mozilla snapshot,
not the operating-system certificate store. It is read through Rust's Unicode
path API and passed as bytes; an imported inline CA takes precedence exclusively.
Update the CA snapshot with native dependency maintenance.

2026-11-23 validation:

- Both Windows runtime bundles were prepared and their target PE imports checked.
- Windows x64 Rust probe loaded all ten required video factories; five actual
  library/TLS integration tests passed. Windows ARM64 was not executed.
- Ubuntu 24.04 x64 under WSL built the pinned Unix source. In a fresh process,
  empty CWD and with external library search variables cleared,
  `scripts/ci/probe_spice_runtime.py <directory>` created a SPICE session and
  instantiated `appsrc`, `appsink`, `decodebin`, `videoconvert`, `jpegdec`,
  `vp8dec`, `vp9dec`, `openh264dec`, `h264parse` and `queue`.
- Linux Rust 1.99.1 also passed 24 helper unit tests, nine process lifecycle tests
  and five actual library/TLS integration tests, with all ignored native tests
  enabled. This verifies the Rust helper on Linux, including stalled-handshake
  cancellation, crash/hang reaping and TLS authority/hostname checks. Guest
  interoperability, GPUI and audio-device output remain untested. Its system
  dependencies came from Ubuntu 24.04; the Ubuntu 22.04 ABI floor remains a CI check.
- `.github/workflows/spice-runtime.yml` now describes six-target preparation,
  native probes and helper/lifecycle tests. It has not been run remotely yet;
  Windows ARM64 is explicitly limited to preparation and compilation on x64.

Application installers and archives now stage these runtimes beside the helper.
ZIP/NSIS/MSI, macOS tar archives and AppImage/DEB/RPM verifiers check their
manifest, target architecture, source provenance and file hashes. Unexpected
runtime files are rejected. Windows manifests also match the exact shared
portable contract and locked package-member hashes. RPM verification decompresses
the payload once, independently of the native file count.

Windows portable updates validate every file before modifying the installation,
then replace the private `spice` directory as one transaction unit. Failure
restores the complete old directory, including obsolete plugins; success removes
them. User data stays outside this transaction. Old **already shipped** updaters
with the previous immutable ZIP allowlist cannot install this larger archive.
The first SPICE release uses manual portable migration as documented in
`FIRST_UPGRADE.zh-CN.md`: back up, close the old app, extract the complete matching
architecture ZIP into a new directory, copy the closed portable data directory,
and keep the original installation/data for rollback. Release notes must include
that procedure; new updater tests do not retroactively fix old executables.

macOS libraries are signed before their manifest hashes are refreshed; the outer
bundle is then signed without recursively rewriting its nested code and verified
with `codesign --verify --deep --strict`. This ordering is regression-tested,
but still needs execution on macOS. Linux RPM metadata excludes the private
SPICE closure from global library provides/requires. DEB and RPM declare the
actual highest glibc requirement found in that closure. Local Ubuntu 24.04
artifacts therefore do not establish the Ubuntu 22.04 release floor.

2026-11-23 release integration validation:

- Final focused preparation/package/compatibility suite: 94 tests, ten platform
  skips. Includes rejection of self-consistent runtime hashes for macOS dylibs
  and SASL `.so` plugins requiring a newer deployment target. This supersedes
  the smaller historical package-suite counts below.
- Portable updater: 13 passed, one actual-release-archive test not run. Covers
  both architectures, every commit failure point, legacy installations and
  obsolete SPICE plugin removal/restoration.
- Windows x64 and Ubuntu 24.04 x64 actual runtime staging verified exact bytes.
  Linux also built real RPM and DEB containers with the actual runtime and tiny
  fixture executables; final package verification and private dependency metadata
  checks passed. These were packaging tests, not complete application releases.
- Windows native ABI/TLS tests: five passed. Both platforms created all ten
  video elements and retained an unchanged runtime inventory.
- All seven locales now include SPICE labels, explanations and errors; all 15
  catalog tests passed, including key parity and interpolation placeholders.
- Windows full-workspace tests passed: 7,588 tests, 74 ignored, across 102 test
  binaries/doc-test groups. The subsequent file-association change passed both
  Windows registry tests; package tests passed 63 with three platform-specific
  skips. Linux actual-runtime/package tests passed all eight after adding MIME
  declarations and installation/removal scriptlets. These packaging fixtures use
  tiny executables and do not prove real application activation.
- The complete Python suite passed 249 tests with 22 platform-specific skips
  after loading the MSVC developer environment. This includes real WiX x64/ARM64
  MSI compilation/extraction, with the complete SPICE inventory required. All
  13 MSI tests passed; missing plugin/resource rows are explicitly rejected.
- Windows `cargo check --workspace`, `cargo fmt --all -- --check` and
  `cargo clippy --workspace --all-targets` passed during integration. Native
  platform/device coverage is still required.

## CPU baseline

```powershell
cargo run --release -p nyaterm-spice-helper --example pipeline_bench
```

Each JSON line describes one scenario, five warm-up updates and 60 measured
updates. The pipeline covers XRGB-to-BGRA conversion, binary packet writing to
memory, packet reading and framebuffer application. It excludes native decode,
networking, actual OS pipes, audio and GPUI/GPU uploads.

Initial baseline (other builds were running; this is not an idle-machine or
end-to-end FPS claim):

| Scenario | Mean update | P95 update |
| --- | ---: | ---: |
| 1920 × 1080 full | 8.96 ms | 12.30 ms |
| 3840 × 2160 full | 40.49 ms | 54.11 ms |
| 3840 × 2160, 256 × 128 dirty area | 0.036 ms | 0.069 ms |
| Two 3840 × 2160 full displays | 82.25 ms | 98.59 ms |

Full-screen results need improvement before claiming the requested performance
goal. Benchmark allocation/reuse, IPC copies and compositor integration; keep
correctness, memory bounds and partial-update behavior intact. Re-measure after
the final pipeline and on each actual release platform.

## Actual pipe baseline

```powershell
cargo run --release -p nyaterm-spice-helper --example pipe_bench
```

This starts a second process, transfers each update over actual OS pipes, applies
it to a framebuffer, and waits for the consumer's acknowledgement. Five warm-up
updates and 60 measured updates per scenario, Windows x64 on the machine above:

| Scenario | Mean update | P95 update | Mean source conversion |
| --- | ---: | ---: | ---: |
| 1920 × 1080 full | 4.05 ms | 4.50 ms | 1.53 ms |
| 3840 × 2160 full | 18.26 ms | 23.46 ms | 7.46 ms |
| 3840 × 2160, 256 × 128 dirty area | 0.064 ms | 0.094 ms | 0.0073 ms |
| Two 3840 × 2160 full displays | 32.47 ms | 35.59 ms | 14.43 ms |

This measures conversion/pipe/consumer latency; it excludes the native decoder,
GLib scheduling, the application's event drain/compositor, GPU uploads, network
and audio. The different scope and overlapping producer/consumer make these
numbers unsuitable for a direct speedup comparison with the in-memory baseline.

Initial Linux x64 baseline before pipe tuning, Ubuntu 24.04 under WSL on the same host,
2026-11-23, Rust 1.99.1 release build, five warm-up and 60 measured updates:

| Scenario | Mean update | P95 update | Mean source conversion |
| --- | ---: | ---: | ---: |
| 1920 × 1080 full | 10.249 ms | 12.259 ms | 1.315 ms |
| 3840 × 2160 full | 36.501 ms | 44.909 ms | 5.394 ms |
| 3840 × 2160, 256 × 128 dirty area | 0.147 ms | 0.193 ms | 0.006 ms |
| Two 3840 × 2160 full displays | 67.926 ms | 72.099 ms | 9.188 ms |

This is a WSL virtualized baseline while other validation work was active, not
a controlled Windows-versus-Linux comparison. The full-frame pipe cost remains
an optimization target; source conversion alone does not account for the latency.
Do not infer native Linux desktop FPS or native hardware latency from these results.

The Linux helper now requests 1 MiB of output pipe capacity, with a safe fallback
when kernel/user limits reject growth. A subsequent paired comparison alternated
the default and tuned pipe over three runs each (60 measured samples per run).
The same release binary and code path were used; only the pipe capacity changed.
The table reports the median of the three per-run means and P95 values:

| Scenario | Default mean / P95 | Tuned mean / P95 |
| --- | ---: | ---: |
| 1080p full | 9.458 / 11.286 ms | 4.917 / 5.878 ms |
| 4K full | 34.274 / 41.458 ms | 18.973 / 20.234 ms |
| 4K sparse | 0.141 / 0.235 ms | 0.099 / 0.156 ms |
| Dual 4K full | 69.572 / 70.789 ms | 36.305 / 39.168 ms |

All tuned runs received the requested 1,048,576-byte capacity. Repeat with
`NYATERM_BENCH_DEFAULT_PIPE=1` for the default-capacity comparison and without that
variable for the production setting; JSON reports the obtained tuned capacity.
This improves this WSL CPU/pipe benchmark, not a measured real-guest frame rate.
The Linux helper's 24 unit, nine lifecycle and five native/TLS tests passed after
tuning, including unchanged data/EOF/blocking flags and non-pipe rejection.

## Background compositor baseline

```powershell
cargo test --release -p nyaterm-remote-desktop --lib spice_compositor_upload_preparation_baseline -- --ignored --nocapture
```

2026-11-23, same Windows x64 machine, five warm-up and 60 measured updates:

| Scenario | Mean | P95 | Packed upload bytes |
| --- | ---: | ---: | ---: |
| 1920 × 1080 full | 2.987 ms | 3.484 ms | 8,294,400 |
| 3840 × 2160 full | 12.594 ms | 13.357 ms | 33,177,600 |
| 4K, 256 × 128 dirty area | 0.017 ms | 0.021 ms | 196,608 |
| Two 4K surfaces, one visible | 20.295 ms | 25.367 ms | 33,177,600 |

Scope: incoming BGRA chunk allocation, framebuffer damage application, tile
coalescing, immutable upload packing and acknowledgement. The dirty upload is
rounded to tile boundaries. This excludes native decode/conversion, IPC pipes,
worker frame pacing, network, audio and GPUI/GPU. It is neither an end-to-end
latency measurement nor a frame-rate promise. Full-frame video still needs
measurement and optimization on real release platforms.

## Upstream references

- spice-gtk public headers and source, version 0.43:
  `https://www.spice-space.org/download/gtk/spice-gtk-0.43.tar.xz`
- remote-viewer key-file semantics:
  `https://gitlab.com/virt-viewer/virt-viewer/-/raw/master/man/remote-viewer.pod`
- Windows runtime package metadata:
  `https://packages.msys2.org/api/search?query=spice-gtk`

All downloaded reference sources live under ignored `target/spice-reference`.
They are unmodified upstream references, not vendored build inputs.
