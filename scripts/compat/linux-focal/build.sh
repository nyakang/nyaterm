#!/usr/bin/env bash
# Run inside the focal image with the checkout mounted at /src (see README).
set -euo pipefail
cd "$(dirname "$0")/../../.."
if [[ "$(getconf GNU_LIBC_VERSION)" != 'glibc 2.31' || "$(uname -m)" != x86_64 ]]; then
  echo 'This build must run on x86_64 Ubuntu 20.04 / glibc 2.31.' >&2
  exit 1
fi
# Keep host compiler flags, modern pkg-config paths and build outputs isolated.
unset RUSTFLAGS CARGO_ENCODED_RUSTFLAGS PKG_CONFIG_PATH PKG_CONFIG_SYSROOT_DIR LD_LIBRARY_PATH NYATERM_SPICE_RUNTIME
export CARGO_TARGET_DIR="$PWD/target/linux-focal"
export NYATERM_ANDROID_DEPS_DIR="$CARGO_TARGET_DIR/android-deps"
export NYATERM_SPICE_DEPS_DIR="$CARGO_TARGET_DIR/spice-deps"
export CARGO_HOME="${NYATERM_FOCAL_CARGO_HOME:-$CARGO_TARGET_DIR/cargo}"
export CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}"
export CARGO_PROFILE_RELEASE_STRIP=debuginfo
# Compatibility artifacts are upgraded manually from this same build channel.
# Bake both policy and installed-data isolation into every executable, including
# direct binary launches and MCP discovery, rather than relying on the wrapper.
export NYATERM_PACKAGE_MANAGER='NyaTerm glibc 2.31 compatibility packages'
export NYATERM_LINUX_COMPAT=glibc231
python3 scripts/release/prepare_android.py --target x86_64-unknown-linux-gnu
export FFMPEG_DIR="$NYATERM_ANDROID_DEPS_DIR/ffmpeg/x86_64-unknown-linux-gnu"
export PKG_CONFIG_PATH="$FFMPEG_DIR/lib/pkgconfig"
# Default members include the MCP bridge; the Windows-only RDP integration is
# not shipped in the focal package.
cargo build --release --locked --target x86_64-unknown-linux-gnu
rustc -Vv > "$CARGO_TARGET_DIR/compiler.txt"
binary_dir="$CARGO_TARGET_DIR/x86_64-unknown-linux-gnu/release"
export NYATERM_FOCAL_BINARY_DIR="$binary_dir"
python3 - <<'PY'
import os, shutil, sys
from pathlib import Path
sys.path.insert(0, 'scripts/release')
import prepare_spice
runtime = prepare_spice.prepare('x86_64-unknown-linux-gnu')
prepare_spice.verify_runtime(runtime, 'x86_64-unknown-linux-gnu')
destination = Path(os.environ['NYATERM_FOCAL_BINARY_DIR']) / 'spice'
if destination.exists():
    # Owned build output, never an installed runtime. Avoid stale decoder files.
    shutil.rmtree(destination)
shutil.copytree(runtime, destination)
PY
export NYATERM_TEST_SPICE_RUNTIME="$binary_dir/spice"
export NYATERM_SPICE_RUNTIME="$NYATERM_TEST_SPICE_RUNTIME"
python3 scripts/ci/probe_spice_runtime.py "$NYATERM_TEST_SPICE_RUNTIME"
bash scripts/compat/linux-focal/prepare-opengl.sh
package_options=()
if [[ "${NYATERM_FOCAL_DEB:-0}" == 1 ]]; then
  package_options+=(--deb)
fi
if [[ -n "${NYATERM_FOCAL_VERSION:-}" ]]; then
  package_options+=(--version "$NYATERM_FOCAL_VERSION")
fi
python3 scripts/compat/linux-focal/package.py --binary-dir "$binary_dir" \
  --opengl "$CARGO_TARGET_DIR/opengl" --output "${NYATERM_FOCAL_DIST:-dist}" "${package_options[@]}"
if [[ "${NYATERM_FOCAL_TESTS:-1}" == 1 ]]; then
  LD_LIBRARY_PATH="$FFMPEG_DIR/lib" cargo test --release --locked --target x86_64-unknown-linux-gnu \
    -p nyaterm-rdp-helper -p nyaterm-vnc-helper -p nyaterm-android-helper --test lifecycle
  cargo test --release --locked --target x86_64-unknown-linux-gnu \
    -p nyaterm-spice-helper --features test-support --test lifecycle -- --include-ignored --test-threads=1
  cargo test --release --locked --target x86_64-unknown-linux-gnu \
    -p nyaterm-spice-helper --test native_runtime -- --ignored --test-threads=1
fi
