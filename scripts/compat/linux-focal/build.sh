#!/usr/bin/env bash
# Run inside the focal image with the checkout mounted at /src (see README).
set -euo pipefail
cd "$(dirname "$0")/../../.."
if [[ "$(getconf GNU_LIBC_VERSION)" != 'glibc 2.31' || "$(uname -m)" != x86_64 ]]; then
  echo 'This build must run on x86_64 Ubuntu 20.04 / glibc 2.31.' >&2
  exit 1
fi
# Keep host compiler flags, modern pkg-config paths and build outputs isolated.
unset RUSTFLAGS CARGO_ENCODED_RUSTFLAGS PKG_CONFIG_PATH PKG_CONFIG_SYSROOT_DIR LD_LIBRARY_PATH
export CARGO_TARGET_DIR="$PWD/target/linux-focal"
export CARGO_HOME="${NYATERM_FOCAL_CARGO_HOME:-$CARGO_TARGET_DIR/cargo}"
export CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}"
export CARGO_PROFILE_RELEASE_STRIP=debuginfo
# All four default members are required at runtime, including the MCP bridge.
cargo build --release --locked --target x86_64-unknown-linux-gnu
binary_dir="$CARGO_TARGET_DIR/x86_64-unknown-linux-gnu/release"
bash scripts/compat/linux-focal/prepare-opengl.sh
python3 scripts/compat/linux-focal/package.py --binary-dir "$binary_dir" \
  --opengl "$CARGO_TARGET_DIR/opengl" --output dist
if [[ "${NYATERM_FOCAL_TESTS:-1}" == 1 ]]; then
  cargo test --release --locked --target x86_64-unknown-linux-gnu \
    -p nyaterm-rdp-helper -p nyaterm-vnc-helper --test lifecycle
fi
