# glibc 2.31 compatibility build

This is a separate **x86-64 Ubuntu 20.04 offline portable artifact**, for the
customized focal/X11 environment described in the adaptation guide. Standard
release artifacts remain unchanged. Build on focal rather than rewriting ELF
symbol versions or replacing the destination's patched system libc.

## Build on a connected Linux machine

From the checkout root, using Docker with BuildKit and host Python 3.11+:

```sh
docker build --build-arg BUILDER_UID="$(id -u)" --build-arg BUILDER_GID="$(id -g)" \
  --build-arg RUST_VERSION="$(python3 -c 'import tomllib; print(tomllib.load(open("rust-toolchain.toml", "rb"))["toolchain"]["channel"])')" \
  -t nyaterm-focal scripts/compat/linux-focal
docker run --rm --user "$(id -u):$(id -g)" \
  -v "$PWD:/src" -w /src nyaterm-focal
```

The image pins Ubuntu 20.04 and reads Rust 1.99.1 from the root toolchain contract through the required build argument. Cargo uses the unchanged lockfile.
The baseline is x86-64, without `target-cpu=native`. Outputs and Cargo downloads
are isolated under `target/linux-focal/`; the host's newer sysroot, compiler
flags and target cache are not used. Four build jobs are the default; set
`CARGO_BUILD_JOBS` on the container for smaller builders. The build needs several
GB of disk; at least 16 GiB RAM is recommended for the optimized desktop build.
The default command also runs the RDP/VNC lifecycle tests.

`build.sh` sets `NYATERM_LINUX_COMPAT=glibc231` and
`NYATERM_PACKAGE_MANAGER="NyaTerm glibc 2.31 compatibility packages"` at compile
time for all executables. This isolates installed data and disables standard
release update checks/downloads, including when a binary is launched directly.
Upgrade manually using artifacts from this compatibility build process.

On managed cloud builders, preserve their Docker proxy configuration. Supply a
combined trusted CA bundle with `docker build --secret id=proxy_ca,src=<bundle>`.
For Cargo downloads at runtime, mount that bundle read-only and set
`CARGO_HTTP_CAINFO`, `SSL_CERT_FILE` and `GIT_SSL_CAINFO` to its container path.
Never disable TLS or package-signature verification. Do not pass secret values
as image build arguments.

The build writes `dist/NyaTerm_<version>_linux_x64_glibc231.tar.gz` plus a SHA-256
file. `NYATERM_FOCAL_VERSION` optionally labels a compatibility packaging revision
(for example, `2.0.1+compat.2` for tag `compat-glibc231-2.0.1-r2`) while the
application keeps its upstream version. Pass this variable into the container;
use an independent `NYATERM_FOCAL_DIST` so existing release assets remain intact.
The application, four helpers and ADB must pass the following **before** archive creation:

- ELF64 x86-64 with the system `/lib64/ld-linux-x86-64.so.2` interpreter;
- GLIBC ≤ 2.31, GLIBCXX ≤ 3.4.28, CXXABI ≤ 1.3.12, no private libc ABI;
- reviewed focal SONAMEs, no RPATH/RUNPATH into a build machine;
- `ldd -r` on focal, checking transitive dependencies and unversioned GLib APIs.

FFmpeg is built inside focal using a separate dependency cache. Its shared libraries
are audited against the same ABI limits; only the Android helper and FFmpeg may use
their explicitly allowed `$ORIGIN` library paths. Android resource hashes are written
after relocation, and the extracted tarball and deb run ADB and helper IPC smoke probes.

The archive includes `abi-report.json`, all helpers, ADB, FFmpeg, Android resource
hashes, a portable-mode marker, license, target-side read-only diagnosis and Chinese offline instructions. It
also includes a **private CPU OpenGL fallback runtime**. `start-nyaterm.sh` now
defaults to the system graphics stack, where GPUI prefers available hardware GPUs;
`start-nyaterm.sh --software` selects the bundled fallback without requiring
Vulkan or a proprietary NVIDIA driver. No Rust, Python,
package manager, AppImage/FUSE or libc installation is needed on the destination.
The tarball works without package-manager installation, including customized
downstream package databases that cannot satisfy ordinary Ubuntu dependencies.

## Optional Debian package

Set `NYATERM_FOCAL_DEB=1` on the build container to also generate an installable
`NyaTerm_<version>_linux_x64_glibc231.deb` and checksum. To package already built
executables, run the following **inside the focal builder**:

```sh
python3 scripts/compat/linux-focal/package.py \
  --binary-dir target/linux-focal/x86_64-unknown-linux-gnu/release \
  --opengl target/linux-focal/opengl --output target/focal-review --deb
```

Both formats pass the same ABI/runtime audits. The `nyaterm-compat` package
installs the renderer-selecting launcher under `/opt/nyaterm-compat` and exposes
`/usr/bin/nyaterm-compat`. The installed build stores data in
`~/.nyaterm-compat/` (`~/.nyaterm-compat-preview/` for prerelease binaries), so
`/opt` stays root-owned. Its cache, logs, crash reports, single-instance lock and
MCP discovery also use the compatibility namespace. Standard stable/preview
installs retain their existing paths; the portable tarball retains local `data/`.

Earlier compatibility debs shared the standard data directory. On upgrading,
the new namespace initially has no connections or settings. Close both builds,
export a backup from the old build and restore it through the application's
supported backup UI. Nothing automatically copies or rewrites the old directory.
Do not symlink the two directories, copy a live database, or point different
versions at the same data. Separate namespaces allow standard and compatibility
builds to run concurrently; multiple launches within one namespace still use the
existing single-instance lock. Restore only backups supported by the destination
version, and keep the original backup before testing an older compatibility build.

Debian `Depends` is generated with focal's `dpkg-shlibdeps` over the application
and all four helpers, ADB and FFmpeg, plus `bash` for the launcher. Missing package ownership or
dependency metadata fails packaging. The SONAME audit remains a separate ABI gate.
Review the resulting dependencies against the target's package database before
choosing the deb; continue using the tarball on customized systems that reject them.

The generated desktop entry uses `StartupWMClass=NyaTerm`, matching the X11
window class; Wayland keeps its lowercase desktop ID. Packages include the
repository's original 32, 64, 128, 256 and 512 pixel hicolor PNGs. Package staging
normalizes directories to 0755 and data/icons to 0644 even with a restrictive
checkout or umask. `dpkg-deb --root-owner-group` sets archive ownership; it is a
build argument, not a Debian control field. The generated `postinst configure`
repairs only the package's explicit regular icon files and refreshes the icon
cache when the tool is available. It tolerates missing sizes and repeated calls.

`NYATERM_FOCAL_DIST` and `NYATERM_FOCAL_SMOKE_OUTPUT` select independent build and
smoke-evidence directories, so verification need not overwrite an earlier
artifact. Nothing here publishes or rewrites an existing Release.

## Renderer selection

New packages built from this revision prefer the **system GPU driver** by default.
The launcher leaves existing driver variables intact, adds no private library path,
and does not force llvmpipe or disable Vulkan in this mode. GPUI selects a usable
hardware adapter ahead of CPU adapters; actual acceleration depends on the driver
and any environment overrides supplied by the user.

| Selection | Portable tar | Installed deb |
| --- | --- | --- |
| Default system/GPU path | `./start-nyaterm.sh` | `nyaterm-compat` or the normal desktop entry |
| Explicit CPU fallback | `./start-nyaterm.sh --software` | `nyaterm-compat --software` or the desktop entry's **Start with CPU software rendering** action |
| Environment selection | `NYATERM_RENDERER=software ./start-nyaterm.sh` | `NYATERM_RENDERER=software nyaterm-compat` |

`NYATERM_RENDERER=gpu` selects the system path. A leading `--gpu` or `--software`
overrides that variable; use a leading `--` to pass a literal launcher option to
the application. Other application arguments and the exit status are preserved.
There is no application restart on an error: if graphics initialization fails,
choose the explicit CPU fallback. Software mode uses only the application-private
Mesa/EGL libraries, forces llvmpipe, and disables Vulkan as in the original build.
Its `LD_LIBRARY_PATH` prepends the entire private runtime, including C++, X11 and
XCB libraries, and is inherited by local shells, helpers and other subprocesses.
It can therefore affect non-graphics code in those processes. This broad scope
is retained because Mesa/LLVM requires the matching private dependency set; the
default system/GPU path does not add these overrides. See the acceptance matrix
below before deploying software mode to a new desktop environment.

The already published `compat-glibc231-2.0.1` assets retain their original CPU
launcher and SHA-256 values. For that existing portable package, `./nyaterm`
already uses system drivers. To apply this change in place, close NyaTerm, back up
and replace only `start-nyaterm.sh` from this revision (mode 0755), retaining all
binaries, `opengl/` and `data/`. For an installed package, the corresponding file
is `/opt/nyaterm-compat/start-nyaterm.sh`; a newly built deb additionally provides
the desktop CPU action.

## Private OpenGL runtime

`prepare-opengl.sh` downloads a checksum-pinned Micromamba build tool, then uses
the SHA-256 explicit locks `opengl-lock.txt` and `opengl-tools-lock.txt`. No solver
refresh or unpinned packages are used in the reusable build. The runtime comes
from conda-forge's Mesa/llvmpipe and supporting libraries built for older glibc.
Only runtime libraries/data, notices and source recipes are shipped, not Conda,
Micromamba, compilers or Python. These libraries are not installed system-wide.

Runtime and patchelf caches use separate prefixes containing the SHA-256 of their
respective lock files. Changing either lock creates a new environment for that
component. A successful runtime creation writes `.nyaterm-lock.sha256`; an
incomplete or mismatched prefix is rejected. `NYATERM_OPENGL_RUNTIME` may select
a fresh prefix; an existing custom prefix must have the matching stamp. Old
unmarked caches must be removed or replaced explicitly. `NYATERM_PATCHELF` is
an explicit external-tool override and must name an executable.

The target's GCC 10 C++ runtime is insufficient for modern Mesa/LLVM, so the
private directory also supplies its required C++ runtime. Every bundled ELF's
**required glibc versions** are checked against 2.31, and `ldd -r` is checked with
the private directory. New GLIBCXX requirements are resolved by the included
libstdc++, rather than by upgrading the target's libstdc++.

There are two separate old-graphics problems:

1. Stock focal Mesa lacks EGL's XCB platform extension and its software Vulkan
   adapter exposes too few color attachments for GPUI.
2. The pinned wgpu code initially selects EGL configs with the ES2 bit even when
   it subsequently creates a desktop OpenGL context. The packaged Mesa exposes
   desktop OpenGL only.

The private `egl_desktop.c` adapter changes that config filter **only when EGL
reports OpenGL without OpenGL_ES**, and forwards the real EGL ABI. The original
GLVND library is renamed privately with patchelf; the adapter and vendor library
are loaded only by the launcher's explicit software mode. It does not change glibc versions, fake
GPU limits, edit third-party Cargo sources, or replace the system EGL driver.
Its source is included in the archive. Software mode explicitly disables Vulkan
ICDs and selects llvmpipe; software rendering trades GPU independence for CPU
usage and lower graphics performance.

## Validation

```sh
python3 -m unittest scripts.tests.test_linux_focal scripts.tests.test_linux_focal_launcher
docker run --rm --user "$(id -u):$(id -g)" -v "$PWD:/src" -w /src \
  nyaterm-focal bash scripts/compat/linux-focal/smoke.sh
```

The smoke test extracts the actual archive into a new temporary directory,
starts its launcher with **--software** under focal's Xvfb, checks a mapped NyaTerm
window and captures its pixels. This explicitly validates the CPU fallback using
the **packaged OpenGL runtime**; it does not certify default hardware rendering.
Launcher tests separately exercise default environment/argument forwarding and
explicit renderer selection using a recording executable.
Evidence is retained in `target/linux-focal/smoke/`. See
[validation evidence](VALIDATION.md).

The dedicated `Linux glibc 2.31` Actions workflow is callable by Release and
manually through `workflow_dispatch` (Actions → Linux glibc 2.31 → Run workflow).
It does not run automatically for PRs or main pushes. Every stable/preview Release
and full signing rehearsal requires this variant. It builds,
audits, tests, exercises the CPU fallback without Vulkan/NVIDIA, and uploads the
archive and deb with checksums and v2 source records after all gates pass. The
standalone workflow does not publish a release or change the updater channel;
Release includes its outputs in the versioned GitHub/R2 assets and global
`SHA256SUMS`. The existing update/download manifest platform keys stay unchanged.

### Target desktop acceptance (pending)

Run both GPU and software modes with disposable accounts/data. The launcher
environment tests and Xvfb rendering smoke do not establish these results:

| Area | Acceptance check |
| --- | --- |
| Local terminal and children | Start the configured shell, run system commands and a C++ program; compare library resolution with the system path |
| SSH / SFTP | Authenticate, open/close sessions, upload/download files and compare their hashes |
| Clipboard / input method | Copy/paste Unicode in both directions and compose/commit Chinese input on the actual desktop |
| RDP / VNC | Connect through each helper, exercise input/clipboard and verify disconnect/reaping |
| Installed data | Alternate standard and compatibility launches, then run both together; verify each catalog and MCP discovery remain separate |
| Clean focal deb | Install the generated deb using only declared dependencies, then launch both rendering modes; do not rely on the builder's development packages |

These checks remain pending until performed on the target environment; historical
validation records below do not cover changes made after their recorded revision.

Containers share the host kernel. This validates the glibc 2.31 / GTK / GLib
userspace, **not** the destination's Linux 5.4 kernel, custom security patches,
real NVIDIA driver or real network services. Finish acceptance on that machine
using `OFFLINE-README.zh-CN.md`. No private glibc or system driver replacement is
shipped; default system-driver mode still needs a working graphics driver.
