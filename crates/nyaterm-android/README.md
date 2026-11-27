> Android helper implementation, scrcpy assets and release tooling now belong to
> `nyaterm-plugins/native`. See [independent component maintenance](../../NATIVE_COMPONENTS.md).
> Build/preparation commands below describe the helper workspace, not this app workspace.

# Native Android connections

Saved Android connections open a terminal. Files use the existing sidebar and
its dual-pane file-manager entry; an existing manager retains its browsing state.
Android Screen opens from an Android session tab's context menu in a separate tab,
or focuses that device's existing screen. The action is disabled while the session
is disconnected or connecting. Views have independent channels, and closing a view
retains submitted transfers. Disconnect retains directories, terminal history and
layout. Reconnect is manual and never replays commands. Reconnecting a channel
reuses a valid unchanged binding and leaves sibling Shell, screen and file channels running.
Discovery restarts a stopped ADB server using the configured executable. Only
TCP connection failures permit startup; an existing server's protocol timeout
does not. A server restart invalidates device leases, so reconnect renews the
binding and generation before opening channels instead of retrying an old transport ID.
Pending Shell requests retain their device until accepted, failed or cancelled;
undeliverable successful results close their channel on the background reaper.
Previously saved Android Files views and layouts still restore, with both file-manager panes initialized.

The sidebar starts at the connection's device directory (default `/sdcard/`),
never the host computer's home directory. File-browser failures emit structured
operation/backend/request/error-code diagnostics without paths, device identities
or device output; expected cancellation is not logged as a failure.

`nyaterm-android` owns ADB server channels, shared discovery, basic Sync and
Android helper IPC/process supervision. Transport owns terminal runtime, files
owns file operations, and desktop owns GPUI presentation and view orchestration.
Only `nyaterm-android-helper` links FFmpeg and parses scrcpy video/control bytes.

The connection editor keeps the TCP host and numeric port on one row when space
allows. Screen settings have their own category: maximum edge (128–2560 pixels),
maximum FPS (1–120), and bit rate (0.1–100 Mbps in the UI, still stored as
100000–100000000 bits/s). Defaults are
1280 pixels, 30 FPS and 8000000 bits/s. Screen sessions always enable the scrcpy
control channel; there is no read-only mode. Desktop localizes Android labels,
status codes and typed backend errors in all seven supported languages.

## Transfer and runtime guarantees

Ordinary ADB upload/download and native drag export read each source once per
attempt. Protocol completion, actual byte counts and source type/size/mtime before
and after transfer protect temporary commits; content hashes and complete remote
readbacks are reserved for editor revisions. A same-size content change with an
unchanged modification time is outside ordinary-transfer detection. Editor saves
require a revision unless forced, upload into a private remote temporary, then
recheck the current revision immediately before replacement. This is the same
best-effort concurrency boundary as SSH shell saves, not a remote compare-and-swap.

Transfers expose scanning, data transfer and committing phases. ADB pause/resume
continues an open transfer; retry after a failed transfer starts from the beginning.
Only unfinished temporary data can retry automatically. Replacement, deletion,
editor saves and terminal commands are never automatically replayed. Overwrites
preserve remote mode/owner/group or fail safely; timestamp preservation is verified.
New uploads with no explicit mode allow mount-assigned ordinary-file permissions
only on confirmed Android emulated FUSE, sdcardfs, FAT or exFAT mounts. Filesystem
type, canonical parent and longest matching mount identity are probed in the
background. Other FUSE mounts and unrecognized probes keep strict mode checks.
Existing targets always preserve exact mode/uid/gid; explicit new-file modes must
also match exactly. Cancellation and binding validity are checked through metadata
and mutation handshakes and once more before the non-retried commit.
Each immutable binding caches Sync capabilities and leases at most two exclusive
channels. Cancelled, failed or desynchronized channels are destroyed.

Tracker transitions invalidate old bindings immediately, including offline/online
transitions between UI updates. Native service selection uses the serial and
verifies the transport ID on the same channel; older servers use serial selection
with mapping checks. Closed views leave submitted transfers alive; weak runtime
registration releases device resources after the last owner disappears.

Screen startup is cancellable with a 30-second deadline. Helper IPC version 4 adds
bounded I420/BGRA frame formats, matrix/range metadata and presentation-format
selection. Capture geometry, cached-frame refresh, visibility and recoverable
input rejection remain supported. Older helpers are rejected during the handshake with
a specific version mismatch error. Text must fit UTF-8 and serialized IPC limits and contain no NUL. Input overflow releases held input and
shows a rate-limited notice. Screen events wake GPUI and schedule presentation on
the next frame. Hidden screens continue H.264 decoding, retain the latest decoded
frame and skip pixel conversion/frame IPC/GPU uploads. A separate presentation
worker wakes directly on frames, credit, visibility or refresh; restoring a static
screen or moving it to another window does not require new media. Decoded frames
are shared immutably without deep copies. Both sides retain at most two idle pixel
buffers and return frame credit with recycled storage after upload or discard.
App and helper are independently versioned and packaged; IPC compatibility is required. A transferred tab recreates focus, event wakes and GPU textures
in its destination window. Different valid leases for the same saved connection reject
the move before attachment so the source can roll back atomically.

See [VALIDATION.md](VALIDATION.md) for regression coverage, device probe results
and outstanding platform/performance acceptance.

## Bundled dependencies

The independently installed Android component includes ADB, its companions, the
Android helper and LGPL dynamic FFmpeg libraries. The application always uses
that component's ADB executable. Existing compatible ADB servers are reused.
When no server is reachable, NyaTerm starts the component's ADB in
foreground mode on the existing default port (5037), retaining its process handle.
After all workspace users stop, application exit cancels discovery and terminates
only the foreground processes it owns, with a bounded background cleanup. A server
started by another tool, including one replacing an earlier NyaTerm server at the
same port, is left running. Shell and batch wrappers, and executables that exit
instead of supporting foreground mode, retain the `start-server` compatibility
path; those detached servers are left running because NyaTerm cannot establish
process ownership. NyaTerm does not delete ADB keys. Device authorization and
operating-system USB drivers/permissions remain necessary.

Pinned sources and SHA-256 values are in
[`android-package.json`](../../scripts/release/android-package.json):
Platform-Tools 37.0.1, Linux ARM64 android-tools 37.0.0p1, scrcpy Server 5.0.1 and
FFmpeg 8.0.1 commit `894da5ca7d742e4429ffb2af534fcda0103ef593`.
Official Windows ADB is x86 and uses Windows ARM64 compatibility support;
official macOS ADB is universal.

Run `python3 scripts/release/prepare_android.py` from the repository root and
export its reported `FFMPEG_DIR` before Cargo commands. Build
`nyaterm-android-helper` beside `nyaterm`. `NYATERM_ANDROID_HELPER` overrides the
helper path. The independent repository native/scripts/release/components.py prepares and packages these dependencies.
FFmpeg builds require a C compiler, make, pkg-config and libclang; Windows uses
MSVC and MSYS2 sh/make. Linux ARM64 additionally uses cmake, Ninja and the libraries
listed in the release workflow.
On Windows the preparation script supplies all four MSVC import libraries in
`FFMPEG_DIR/lib`, reuses FFmpeg's installed libraries or generates missing ones
with the selected MSVC librarian, and repairs incomplete cached installations.

FFmpeg disables GPL, nonfree, autodetection, network and programs, and enables
H.264 decoding/parsing and BGRA scaling. The pinned ffmpeg-next 8.1.0 needs its
format feature because of unconditional imports; avformat contains no demuxers
or protocols. Packages include dependency notices, licenses, source URLs,
checksums and configure parameters. Dynamic libraries can be replaced/relinked.
scrcpy notices and license are maintained alongside the helper assets in nyaterm-plugins/native/crates/nyaterm-android-helper/assets.

## Input and compatibility

Defaults are H.264, maximum edge 1280, 30 FPS and 8 Mbps. BGRA frames are capped
at 16 MiB and media packets at 8 MiB. Native decoding limits allocation to
2560² pixels with each edge capped at 2560 before decoded-surface allocation,
including when SPS dimensions disagree with the scrcpy header. Large
captures (including 2560×1920 and 2560×2560) scale proportionally into the 16 MiB
presentation limit. IPC carries capture and presentation dimensions separately;
touch and scroll always use the full capture geometry. The helper retains one
in-flight frame and one latest pending frame. Rotation invalidates old coordinates. Android screen sessions
always enable control. Blur, close and input overflow release held input.

A focused screen uses Ctrl+V / Cmd+V to paste the computer clipboard, sharing the
toolbar paste path; an empty clipboard sends nothing. Forwarded Ctrl/Meta keys are
released before paste. Zoom spans 1×–4× with zoom-in, zoom-out and fit controls.
Fit clears pan; middle-button drag pans and Ctrl/Cmd + wheel zooms around the
pointer. Ordinary wheel and left-button gestures control the device. Image and
input coordinates use the same clipped transform; pan is constrained to the image.
Each split screen owns its focus and releases held input independently.

Hold a modifier before pressing the left mouse button to simulate two fingers:

| Modifier + left drag | Device gesture |
| --- | --- |
| Ctrl | Pinch/rotate, with the second finger mirrored through the capture center |
| Shift | Vertical two-finger slide, mirrored across the vertical center line |
| Ctrl + Shift | Horizontal two-finger slide, mirrored across the horizontal center line |

The gesture is latched on press until release, even if modifiers change mid-drag.
These gestures change content inside apps that support them; Ctrl/Cmd + wheel
still changes only NyaTerm's local image scale. Cmd alone does not simulate Ctrl.
Moving outside the visible image, losing focus, hiding the screen, changing its
geometry or local transform, or an input failure ends the gesture. Press again
to start a new one. All active fingers are released together on interruption.
The application and Android helper must both use IPC version 6; older helpers
are rejected by the existing version handshake. Saved settings are unchanged.

Unicode uses explicit paste or the device input method. Host IME does not
automatically replace the device clipboard. A clipboard acknowledgement proves
request handling, not insertion into an input field.

ADB adds `type: adb`, `device`, `directory` and `screen` to saved connections and
`adb_files`, `adb_shell`, `adb_screen` restore types. Existing database tables,
keys, encryption, backups and pane layouts retain their contracts; unknown
properties survive saves. Clients opening ADB data must support the new type.

## Acceptance probes

Run from the repository root with an explicitly authorized test device:

```sh
cargo run -p nyaterm-files --example android_files_probe -- HOST:PORT /data/local/tmp
cargo run -p nyaterm-transport --example android_shell_probe -- HOST:PORT
cargo run -p nyaterm-android --example screen_probe -- HOST:PORT SECONDS CYCLES FPS
cargo run -p nyaterm-android --example screen_control_probe -- HOST:PORT
```

The file probe lists `/sdcard/`, then cleans a unique directory under its optional
second argument (default `/sdcard`) and verifies special names, content, overwrite
protection and cancellation. Use `/data/local/tmp` for the full filename cases when
emulated shared storage rejects quotes or control characters. The PTY probe checks resize,
Ctrl+C, exit status and independent channels. The control probe opens Android
Settings, tests tap/drag/scroll, rotation and frame credit, and sends synthetic
Unicode clipboard text. Probes retain no device frames or clipboard contents.

Place the `device_probe` executable beside packaged `adb/` and
`android-resources.json` to verify bundled discovery with PATH/SDK lookup
disabled. Its optional second argument selects a private server port for
startup testing without stopping the shared server.

Headless protocol acceptance does not verify real GPUI rendering, focus,
drag/drop, IME or GPU upload latency. Windows/macOS/ARM64 loading and signing,
actual Windows portable update round trips and animated-screen performance
require platform acceptance.

## Screen controls and planar presentation

The screen toolbar uses tintable icons, localized tooltips and a compact overflow
menu. It includes Android navigation, rotation, fit/zoom, paste and session
settings. The overflow menu also sends volume, power-key and notification-panel
controls. A power key is not a screen-off-mirroring mode. Existing touch, middle
button pan and Ctrl/Cmd-wheel local zoom gestures are retained.

Connection settings persist defaults. Smooth (1280/60/8 Mbps), balanced
(1280/30/8 Mbps) and sharp (1920/30/12 Mbps) presets fill the three numeric values;
custom combinations remain supported. New `power_on` defaults to false and
`capture_orientation` defaults to `follow_device`; other values lock the initial
orientation or 0/90/180/270 degrees. Old connection documents retain their behavior
and unknown fields. No database, encryption or backup migration is needed.

The toolbar settings dialog edits a separate draft. Apply restarts only that
screen channel; cancel leaves it unchanged. Session overrides survive manual
reconnect and window transfer, but disappear when the screen session closes.
Neither saved connection defaults nor sibling shell/files channels are modified.

On supported GPUI backends the helper packs decoded 8-bit I420 planes directly,
without CPU BGRA conversion. Scaling remains in the helper when required by the
existing presentation size limit. IPC carries tight Y/U/V planes with ceil-divided
chroma sizes, and BT.601/709/2020-NCL matrix plus full/limited range. Unspecified
source metadata defaults to BT.601 limited; YUVJ420 is full range. No HDR tone
mapping or hardware decoder is introduced. Other decoded pixel formats use the
BGRA path. The GPUI fork samples three independently clamped R8 plane regions in
one atlas allocation, preserving clipping, opacity and scene order on DirectX,
Metal and wgpu. Unsupported backends or failed YUV uploads fall back to BGRA and
request the cached static frame. A window transfer recreates GPU resources and
renegotiates the format. Both formats retain bounded frame credit and recycling.

See VALIDATION.md for actual platform coverage; implementing a backend is not
native platform acceptance. `screen_probe ... visibility yuv` exercises I420
visibility/restore without retaining device frames.
