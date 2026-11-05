#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../../.."
work="$PWD/target/linux-focal"
tools="$work/opengl-tools"
mkdir -p "$tools"
runtime_lock=scripts/compat/linux-focal/opengl-lock.txt
tools_lock=scripts/compat/linux-focal/opengl-tools-lock.txt
runtime_hash="$(sha256sum "$runtime_lock")"
runtime_hash="${runtime_hash%% *}"
tools_hash="$(sha256sum "$tools_lock")"
tools_hash="${tools_hash%% *}"
# A lock change selects a fresh prefix; old build caches are never used for a
# new lock and are left intact for other checkouts/builds that may use them.
runtime="${NYATERM_OPENGL_RUNTIME:-$work/opengl-env-$runtime_hash}"
elf_tools="$tools/elf-$tools_hash"
patchelf_bin="${NYATERM_PATCHELF:-$elf_tools/bin/patchelf}"
stamp="$runtime/.nyaterm-lock.sha256"
runtime_ready=false
if [[ -d "$runtime/conda-meta" && -f "$stamp" && "$(<"$stamp")" == "$runtime_hash" ]]; then
  runtime_ready=true
elif [[ -e "$runtime" ]]; then
  # In particular, never delete or silently trust an externally supplied prefix.
  printf 'NyaTerm: incomplete or mismatched OpenGL cache: %s; select a fresh NYATERM_OPENGL_RUNTIME or remove this build cache.\n' "$runtime" >&2
  exit 1
fi
if [[ "$runtime_ready" != true || ! -x "$patchelf_bin" ]]; then
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
  if [[ "$runtime_ready" != true ]]; then
    "$mamba" create -y -p "$runtime" --ssl-verify "$ca" \
      --file "$runtime_lock"
    printf '%s\n' "$runtime_hash" > "$stamp"
  fi
  if [[ ! -x "$patchelf_bin" ]]; then
    if [[ -n "${NYATERM_PATCHELF:-}" ]]; then
      printf 'NyaTerm: NYATERM_PATCHELF is not executable: %s\n' "$patchelf_bin" >&2
      exit 1
    fi
    "$mamba" create -y -p "$elf_tools" --ssl-verify "$ca" \
      --file "$tools_lock"
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
