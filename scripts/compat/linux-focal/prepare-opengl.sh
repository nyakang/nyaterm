#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../../.."
work="$PWD/target/linux-focal"
tools="$work/opengl-tools"
mkdir -p "$tools"
runtime="${NYATERM_OPENGL_RUNTIME:-$work/opengl-env}"
patchelf_bin="${NYATERM_PATCHELF:-$tools/elf/bin/patchelf}"
if [[ ! -d "$runtime/conda-meta" || ! -x "$patchelf_bin" ]]; then
  mamba="$tools/micromamba"
  if [[ ! -x "$mamba" ]]; then
    curl --proto '=https' --tlsv1.2 --fail --location --retry 3 \
      https://github.com/mamba-org/micromamba-releases/releases/download/2.10.0-0/micromamba-linux-64 \
      -o "$mamba"
    echo "701f39c06f8e6a670a0e186c8479b9e5c0502a2f0cda6cef65d83bdd2875e8bd  $mamba" | sha256sum -c -
    chmod +x "$mamba"
  fi
  export MAMBA_ROOT_PREFIX="$tools/mamba"
  export CONDA_OVERRIDE_GLIBC=2.31 CONDA_OVERRIDE_ARCHSPEC=x86_64
  ca="${SSL_CERT_FILE:-/etc/ssl/certs/ca-certificates.crt}"
  if [[ ! -d "$runtime/conda-meta" ]]; then
    "$mamba" create -y -p "$runtime" --ssl-verify "$ca" \
      --file scripts/compat/linux-focal/opengl-lock.txt
  fi
  if [[ ! -x "$patchelf_bin" ]]; then
    "$mamba" create -y -p "$tools/elf" --ssl-verify "$ca" \
      --file scripts/compat/linux-focal/opengl-tools-lock.txt
  fi
fi
python3 scripts/compat/linux-focal/prepare_opengl.py "$runtime" "$work/opengl"
lib="$work/opengl/lib"
# Keep the unmodified vendor dispatch library separate from our EGL adapter.
cp -L "$runtime/lib/libEGL.so.1" "$lib/libEGL_nyaterm.so.1"
"$patchelf_bin" --set-soname libEGL_nyaterm.so.1 "$lib/libEGL_nyaterm.so.1"
rm -f "$lib/libEGL.so.1"
cc -shared -fPIC -O2 -Wall -Wextra -Werror scripts/compat/linux-focal/egl_desktop.c \
  -Wl,--no-as-needed "$lib/libEGL_nyaterm.so.1" -Wl,--as-needed -ldl -lpthread \
  '-Wl,-rpath,$ORIGIN' -o "$lib/libEGL.so.1"
