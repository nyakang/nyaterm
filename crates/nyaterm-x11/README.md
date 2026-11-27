# X11 local runtime

The application prepares a local display before requesting SSH X11 forwarding.
This crate owns blocking discovery, readiness probes and managed process trees.
It depends on core contracts and the independently pinned native control protocol;
desktop owns scheduling, prompts and settings, and transport owns X11 channels.

## Platform policy

| Platform | Automatic mode | Ownership |
| --- | --- | --- |
| Windows x64 | Install/acquire the signed X11 component, start one VcXsrv per SSH session | Private instance, clipboard off by default |
| macOS | Discover and start an already installed XQuartz | Shared system server, never stopped |
| Linux | Use the current DISPLAY and scoped xauth entry for Xorg/Xwayland | Shared system server, never stopped |
| Windows ARM64 | External server only | No official managed component |

An explicit saved DISPLAY remains authoritative in Automatic mode. External
mode never starts a managed server. X11 preparation failure requires the user
to choose ordinary SSH or cancel; it never silently removes forwarding. An
X11-enabled tab receives its own SSH connection so its cookie and receiver are
not overwritten by another multiplexed tab. Auxiliary SFTP/monitoring channels
can still share that tab's SSH connection.

## Authentication and cleanup

Readiness requires an actual successful X11 setup exchange. External xauth
queries are display-scoped and bounded; another display's cookie is never used
as fallback. The SSH fake cookie is validated before rewriting even when an
external server accepts connections without local authentication. Malformed,
oversized or timed-out setup packets close the forwarding channel.

The managed helper launches the pinned loopback-only VcXsrv fork with a private
Xauthority file. It reports the display and ephemeral cookie through a bounded
control pipe. Each process tree has a Windows Job Object. Final runtime release,
SSH X11 rejection, cancellation and session shutdown trigger cleanup outside
the UI thread. The component directory lease survives until child termination
and pipe-reader reaping. User-owned servers are never terminated.

## Compatibility and release

`terminal.x11_display` retains its meaning. `terminal.x11_mode` defaults to
`auto`, and `terminal.x11_clipboard` defaults to false. Unknown mode strings
round-trip unchanged. These machine-local settings are preserved during sync;
ordinary backup/settings documents retain them.

X11 uses `native/v1/x11/index.json` and `preferences.x11.v1`, leaving the legacy
four-component index and preference document readable by older clients. Staging
also uses a separate directory, outside old clients' recovery traversal.

The helper/build/release implementation is in `nyakang/nyaterm-plugins`, branch
`feat/x11-runtime`. VcXsrv patches live in `nyakang/vcxsrv`, branch `nyaterm`, with
individual patch rationale and validation in `NYATERM.md`. Runtime packages
include licenses and an exact corresponding-source reference. This application
change does not itself publish a signed component.

## Verification

Run `cargo test -p nyaterm-x11` for process cleanup and readiness tests. On Linux
with Xvfb installed, run:

```sh
cargo test -p nyaterm-x11 --test xorg -- --ignored
```

The independent Windows CI builds the actual VcXsrv source and tests staged
helper authentication, loopback listeners, per-session isolation and process
reaping using `native/scripts/release/verify_x11.py`.

[Windows runtime validation run 45980001210](https://github.com/nyakang/nyaterm-plugins/actions/runs/45980001210)
passed on 2026-11-27 using plugin commit `1ebe2ed` and VcXsrv source revision
`d31ed5c652303c2be21d5b7db4a50e055a021edc`. Its unsigned Windows x64 component
archive is available as the workflow artifact. Correct cookies were accepted;
missing, wrong and cross-instance cookies were rejected. Both independent
instances listened only on `127.0.0.1`, and normal stop and forced helper
termination left no server listener behind. This artifact has not been signed
or published to the official component index.

Implementation validation on 2026-11-27 covered workspace check, all-target
Clippy, formatting, architecture rules, documentation translations and workspace
doc tests. Workspace tests were completed across multiple runs with
`RUST_MIN_STACK=33554432` and `RUST_TEST_THREADS=8`: the existing file-permission
test was rerun successfully as an unprivileged user, and the existing shell
environment timeout test passed in isolation. This is aggregate coverage, not a
claim that the entire suite passed in one root-run invocation. The X11 crate also
passed Windows GNU and macOS ARM64 all-target compilation, and its real Xvfb
authentication integration test passed on Linux.

Interactive Windows acceptance must also cover remote application windows,
clipboard opt-in, DPI and multiple monitors. macOS acceptance requires installed
XQuartz and a desktop login session. Linux Wayland acceptance requires a real
compositor with Xwayland; Xvfb only verifies the X11 protocol/authentication path.
