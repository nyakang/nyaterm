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
