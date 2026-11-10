# Native Android connections

Saved Android connections open an equal Files/Shell horizontal split and focus
Shell. Screen opens from the toolbar in a separate tab. Views have independent
channels; closing Files retains submitted transfers, while closing the device
cancels its transfers and closes its views. Disconnect retains directories,
terminal history and layout. Reconnect is manual and never replays commands.

`nyaterm-android` owns ADB server channels, shared discovery, basic Sync and
Android helper IPC/process supervision. Transport owns terminal runtime, files
owns file operations, and desktop owns GPUI presentation and view orchestration.
Only `nyaterm-android-helper` links FFmpeg and parses scrcpy video/control bytes.

The connection editor keeps the TCP host and numeric port on one row when space
allows. Screen settings have their own category: maximum edge (128–2560 pixels),
maximum FPS (1–120), and bit rate (100000–100000000 bits/s). Defaults are
1280 pixels, 30 FPS and 8000000 bits/s. Screen sessions always enable the scrcpy
control channel; there is no read-only mode. Desktop localizes Android labels,
status codes and typed backend errors in all six supported languages.

## Bundled dependencies

Release packages include ADB, its companions, the Android helper and LGPL dynamic
FFmpeg libraries. `android.adb_path` is a device-local setting that overrides the
bundled executable; an invalid explicit path is an error. Debug builds can use
PATH or SDK discovery when the bundle is absent. Existing compatible ADB servers
are reused; NyaTerm does not kill shared servers or delete ADB keys. Device
authorization and operating-system USB drivers/permissions remain necessary.

Pinned sources and SHA-256 values are in
[`android-package.json`](../../scripts/release/android-package.json):
Platform-Tools 37.0.1, Linux ARM64 android-tools 37.0.0p1, scrcpy Server 5.0.1 and
FFmpeg 8.0.1 commit `894da5ca7d742e4429ffb2af534fcda0103ef593`.
Official Windows ADB is x86 and uses Windows ARM64 compatibility support;
official macOS ADB is universal.

Run `python3 scripts/release/prepare_android.py` from the repository root and
export its reported `FFMPEG_DIR` before Cargo commands. Build
`nyaterm-android-helper` beside `nyaterm`. `NYATERM_ANDROID_HELPER` overrides the
helper path. `package_native.py` prepares and packages the same dependencies.
FFmpeg builds require a C compiler, make, pkg-config and libclang; Windows uses
MSVC and sh/make. Linux ARM64 additionally uses cmake, Ninja and the libraries
listed in the release workflow.

FFmpeg disables GPL, nonfree, autodetection, network and programs, and enables
H.264 decoding/parsing and BGRA scaling. The pinned ffmpeg-next 8.1.0 needs its
format feature because of unconditional imports; avformat contains no demuxers
or protocols. Packages include dependency notices, licenses, source URLs,
checksums and configure parameters. Dynamic libraries can be replaced/relinked.
See [scrcpy notices](assets/NOTICE.md) and [license](assets/LICENSE.scrcpy).

## Input and compatibility

Defaults are H.264, maximum edge 1280, 30 FPS and 8 Mbps. BGRA frames are capped
at 16 MiB, media packets at 8 MiB, and native decoding limits pixel allocation
before parsing SPS dimensions. The helper retains one in-flight frame and one
latest pending frame. Rotation invalidates old coordinates. View-only input is
enforced in both the application and helper. Blur, close and input overflow
release held input.

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
cargo run -p nyaterm-files --example adb_probe -- HOST:PORT
cargo run -p nyaterm-transport --example adb_probe -- HOST:PORT
cargo run -p nyaterm-android --example screen_probe -- HOST:PORT SECONDS CYCLES FPS
cargo run -p nyaterm-android --example screen_control_probe -- HOST:PORT
```

The file probe cleans a unique `/sdcard` directory and verifies special names,
content, overwrite protection and cancellation. The PTY probe checks resize,
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
