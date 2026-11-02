# glibc 2.31 compatibility build

This is a separate **x86-64 Ubuntu 20.04 offline portable artifact**, for the
customized focal/X11 environment described in the adaptation guide. Standard
release artifacts remain unchanged. Build on focal rather than rewriting ELF
symbol versions or replacing the destination's patched system libc.

## Build on a connected Linux machine

From the checkout root, using Docker with BuildKit:

```sh
docker build --build-arg BUILDER_UID="$(id -u)" --build-arg BUILDER_GID="$(id -g)" \
  -t nyaterm-focal scripts/compat/linux-focal
docker run --rm --user "$(id -u):$(id -g)" \
  -v "$PWD:/src" -w /src nyaterm-focal
```

The image pins Ubuntu 20.04 and Rust 1.99.1. Cargo uses the unchanged lockfile.
The baseline is x86-64, without `target-cpu=native`. Outputs and Cargo downloads
are isolated under `target/linux-focal/`; the host's newer sysroot, compiler
flags and target cache are not used. Four build jobs are the default; set
`CARGO_BUILD_JOBS` on the container for smaller builders. The build needs several
GB of disk; at least 16 GiB RAM is recommended for the optimized desktop build.
The default command also runs the RDP/VNC lifecycle tests.

On managed cloud builders, preserve their Docker proxy configuration. Supply a
combined trusted CA bundle with `docker build --secret id=proxy_ca,src=<bundle>`.
For Cargo downloads at runtime, mount that bundle read-only and set
`CARGO_HTTP_CAINFO`, `SSL_CERT_FILE` and `GIT_SSL_CAINFO` to its container path.
Never disable TLS or package-signature verification. Do not pass secret values
as image build arguments.

The build writes `dist/NyaTerm_<version>_linux_x64_glibc231.tar.gz` plus a SHA-256
file. All four executables must pass the following **before** archive creation:

- ELF64 x86-64 with the system `/lib64/ld-linux-x86-64.so.2` interpreter;
- GLIBC ≤ 2.31, GLIBCXX ≤ 3.4.28, CXXABI ≤ 1.3.12, no private libc ABI;
- reviewed focal SONAMEs, no RPATH/RUNPATH into a build machine;
- `ldd -r` on focal, checking transitive dependencies and unversioned GLib APIs.

The archive includes `abi-report.json`, all helpers, a portable-mode marker,
license, target-side read-only diagnosis and Chinese offline instructions. It
also includes a **private CPU OpenGL runtime**, selected by `start-nyaterm.sh`, so
Vulkan and a proprietary NVIDIA driver are not prerequisites. No Rust, Python,
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
installs the private OpenGL launcher under `/opt/nyaterm-compat` and exposes
`/usr/bin/nyaterm-compat`. Unlike the portable tarball, it stores data in the
normal per-user NyaTerm directory, so `/opt` can stay root-owned. Review the
declared dependencies against the target's package database before choosing the
deb; continue using the tarball on customized systems that reject them.

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

## Private OpenGL runtime

`prepare-opengl.sh` downloads a checksum-pinned Micromamba build tool, then uses
the SHA-256 explicit locks `opengl-lock.txt` and `opengl-tools-lock.txt`. No solver
refresh or unpinned packages are used in the reusable build. The runtime comes
from conda-forge's Mesa/llvmpipe and supporting libraries built for older glibc.
Only runtime libraries/data, notices and source recipes are shipped, not Conda,
Micromamba, compilers or Python. These libraries are not installed system-wide.

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
are loaded only by the launch script. It does not change glibc versions, fake
GPU limits, edit third-party Cargo sources, or replace the system EGL driver.
Its source is included in the archive. The launcher explicitly disables Vulkan
ICDs and selects llvmpipe; software rendering trades GPU independence for CPU
usage and lower graphics performance.

## Validation

```sh
python3 -m unittest scripts.tests.test_linux_focal
docker run --rm --user "$(id -u):$(id -g)" -v "$PWD:/src" -w /src \
  nyaterm-focal bash scripts/compat/linux-focal/smoke.sh
```

The smoke test extracts the actual archive into a new temporary directory,
starts its launcher under focal's Xvfb, checks a mapped NyaTerm window and captures
its pixels. It uses the **packaged OpenGL runtime**, not newer host Mesa libraries.
Evidence is retained in `target/linux-focal/smoke/`. See
[validation evidence](VALIDATION.md).

The dedicated `Linux glibc 2.31` Actions workflow builds, audits, tests, exercises
OpenGL without Vulkan/NVIDIA, and uploads the archive. It does not publish a
release or change the updater channel.

Containers share the host kernel. This validates the glibc 2.31 / GTK / GLib
userspace, **not** the destination's Linux 5.4 kernel, custom security patches,
real NVIDIA driver or real network services. Finish acceptance on that machine
using `OFFLINE-README.zh-CN.md`. No private glibc or system driver replacement is
shipped; system-driver mode (`./nyaterm`) still needs a working graphics driver.
