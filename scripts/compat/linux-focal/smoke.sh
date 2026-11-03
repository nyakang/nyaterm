#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../../.."
[[ "$(getconf GNU_LIBC_VERSION)" == 'glibc 2.31' ]]
output="${NYATERM_FOCAL_SMOKE_OUTPUT:-$PWD/target/linux-focal/smoke}"
mkdir -p "$output"
export XDG_RUNTIME_DIR
XDG_RUNTIME_DIR="$(mktemp -d)"
trap 'rm -rf "$XDG_RUNTIME_DIR"' EXIT
xvfb-run -a -s '-screen 0 1280x800x24 -nolisten tcp' \
  python3 scripts/compat/linux-focal/smoke.py "$output" "${NYATERM_FOCAL_DIST:-dist}"
