# Compatibility validation (2026-10-29)

Target from the supplied environment guide: customized Ubuntu 20.04, x86-64,
glibc 2.31, GTK 3.24.23, GLib 2.64.6, Linux 5.4, isolated network; GT 1010
(GP108), proprietary driver not confirmed. The offline default uses CPU OpenGL
and does not depend on Vulkan or that proprietary driver.

## Verified

| Check | Result |
| --- | --- |
| Final builder | Ubuntu 20.04, glibc 2.31, GTK 3.24.20, GLib 2.64.6, Rust 1.99.1, ordinary UID 1000 |
| Frozen release build | All four default-member executables built successfully |
| Main executable + three helpers | Highest required GLIBC version **2.30**; system interpreter; no RPATH; focal `ldd -r` resolved |
| Private OpenGL runtime | **83 ELF libraries** audited; highest required GLIBC version **2.17**; required C++ libraries included; focal `ldd -r` resolved |
| Runtime reproducibility | Full checksum-pinned preparation script executed from an empty project-local download cache |
| Actual archive relocation | Extracted into a new temporary directory and launched using its own `start-nyaterm.sh` |
| Graphics on focal | **Passed**: OpenGL/llvmpipe, Vulkan ICDs disabled, no NVIDIA device/driver; 1280×800 NyaTerm window and workbench screenshot inspected |
| RDP/VNC lifecycle tests | **22 passed**, no skips or failures, release-profile binaries on focal |
| ABI, EGL adapter and packaging/release Python tests | **65 passed**, **1 skipped** (Windows NSIS unavailable), no failures |
| Shell/workflow validation | `bash -n`, `git diff --check`, actionlint 1.7.7 passed |
| Archive integrity | SHA-256 verification passed |

Evidence produced by the current run:

- `target/linux-focal/smoke/application.log` explicitly records `backend=Gl`,
  `type=Cpu` and `Selected GPU ... llvmpipe (LLVM 23.1.4, 256 bits) (Gl)`.
- `target/linux-focal/smoke/workbench.xwd` and `window-tree.txt` record the actual
  final archive's rendered window.
- The archive contains `abi-report.json`, individual library hashes,
  `opengl/packages.json`, the SHA-256 package lock, the EGL adapter source and
  third-party notices/recipes.

## Diagnosed failures that led to the default OpenGL path

The ABI-only build initially failed graphics startup with stock focal Mesa 21:
lavapipe exposed two color attachments while GPUI requested four; the GL path
fell back to surfaceless EGL because the old driver lacked the XCB platform
extension. These were not libc failures. They are resolved for the default
launcher by the private Mesa/llvmpipe runtime.

The newer conda-forge Mesa runtime enables desktop OpenGL, not GLES. The pinned
wgpu config selector requests an ES2 config before creating a desktop context.
The application-private EGL adapter corrects that filter only for a desktop-only
driver. Unit tests verify GLES-capable drivers retain the original filter, and
the final archive's real OpenGL surface/device and rendered frame were checked.
No glibc symbol tags, GPU limits, third-party Cargo sources or system files were
modified to make the checks pass.

## Remaining target-machine acceptance

The test containers share host kernel **6.18.55**. They do not emulate Linux 5.4
or the downstream security patches. Target-machine testing is still required
for its kernel/JIT policy, Chinese input, clipboard, local shell, actual internal
SSH/SFTP/RDP/VNC services and sustained software-renderer performance. No claim
is made that every feature is fully certified on that unavailable machine.

Network-dependent AI, external OAuth, public cloud sync, online translation and
online updates cannot work on an isolated network without reachable internal
services. Use the same offline compatibility build to upgrade, retaining `data/`.
Hardware-accelerated system-driver mode is optional and remains unverified on
the target GT 1010; the default software OpenGL mode avoids that dependency.

## Upstream sync and icon packaging revalidation (2026-11-02)

Merged upstream `main` through `4671cc964de45063a5c5541178cf284c7e251d12`
without rewriting the compatibility branch. The merged 2.0.1 code built in the
pinned focal/Rust 1.99.1 image. New validation artifacts were written under
`target/validation-20261102/artifacts`; the existing 2.0.0 tarball, deb and their
checksum files in `dist` were checked against their original SHA-256 values and
were unchanged. No Release, tag or attachment was published or modified.

| Check | Result |
| --- | --- |
| Merged-source frozen optimized build | Passed on Ubuntu 20.04 / glibc 2.31, x86-64 |
| Application, MCP, RDP and VNC executables | Highest required GLIBC **2.30** for each; focal ABI/SONAME/interpreter/RPATH and `ldd -r` gates passed |
| Private OpenGL runtime | **83 ELF libraries** audited, highest required GLIBC **2.17**; used the existing checksum-pinned runtime cache |
| Portable archive | Extracted into a new directory; rendered a 1280×800 workbench under Xvfb using llvmpipe, `backend=Gl`, `type=Cpu`; Vulkan ICDs disabled |
| Real compatibility deb install | `dpkg -i` succeeded in a separate disposable focal container; `/usr/bin/nyaterm-compat` rendered a workbench as ordinary UID 1000 |
| Installed icon lookup and decoding | Ordinary-user GTK icon-theme lookup and GdkPixbuf decoding passed at **128, 256 and 512 pixels**; root:root / 0644 |
| Install repair | Intentionally changed an installed icon to 0600; repeated `postinst configure` restored 0644 successfully |
| X11 identity | Real application window reported `("nyaterm" "NyaTerm")`; native adapter regression tested stable and preview class/instance pairs against an Xvfb server |
| Helper lifecycle regression | **22 passed**, release-profile RDP/VNC helpers on focal |
| Python scripts | **122 tests**, 114 passed and 8 platform/tool skips; focal Python 3.8 ABI/package suite **9 passed** |
| Packaging regressions | Real dpkg archives checked for root ownership and readable icons, including restrictive source permissions, repeated configure, missing sizes, symlinks and unrelated-file preservation |
| Source checks | Formatting, architecture boundaries, shell syntax, diff whitespace and actionlint 1.7.7 passed |

Evidence remains in `target/validation-20261102/`: `build-final.log`,
`abi-summary.json`, `package.log`, `native-x11-test.log`, `lifecycle.log`,
`deb-install.log`, `smoke/` and `deb-smoke/`. Each new artifact also has a SHA-256
file and embeds the full ABI report and third-party notices.

These checks validate the packaged files and X11/OpenGL behavior in focal
containers. They do not certify icon presentation in the unavailable internal
machine's actual desktop shell, its Linux 5.4 kernel/custom security patches,
real driver or internal services. The target-machine acceptance requirements
above still apply.
