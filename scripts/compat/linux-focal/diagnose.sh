#!/usr/bin/env bash
# Read-only preflight, also usable offline on the customized focal desktop.
set -uo pipefail
cd "$(dirname "$0")"
failed=0
echo '== System =='
uname -smr
getconf GNU_LIBC_VERSION
if [[ "$(uname -m)" != x86_64 ]]; then
  echo 'ERROR: this package requires x86_64.'; failed=1
fi
echo '== Executables and system libraries =='
private_libs="$PWD/opengl/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
for binary in nyaterm nyaterm-mcp nyaterm-rdp-helper nyaterm-vnc-helper; do
  if [[ ! -x "$binary" ]]; then echo "ERROR: missing executable: $binary"; failed=1; continue; fi
  output="$(LD_LIBRARY_PATH="$private_libs" LC_ALL=C ldd -r "./$binary" 2>&1)"
  status=$?
  printf '%s\n%s\n' "-- $binary --" "$output"
  if [[ $status != 0 ]] || [[ "$output" == *'not found'* || "$output" == *'undefined symbol'* ]]; then
    failed=1
  fi
done
echo '== Bundled OpenGL =='
if [[ ! -f opengl/lib/libEGL.so.1 || ! -f opengl/lib/libEGL_nyaterm.so.1 ]]; then
  echo 'ERROR: incomplete private OpenGL runtime.'; failed=1
else
  output="$(LD_LIBRARY_PATH="$private_libs" LC_ALL=C ldd -r opengl/lib/libEGL.so.1 2>&1)"
  printf '%s\n' "$output"
  if [[ "$output" == *'not found'* || "$output" == *'undefined symbol'* ]]; then failed=1; fi
fi
echo '== Desktop and graphics =='
printf 'DISPLAY=%s\n' "${DISPLAY:-<unset>}"
if [[ -z "${DISPLAY:-}" ]]; then echo 'ERROR: run from the target X11 desktop session.'; failed=1; fi
for tool in nvidia-smi glxinfo vulkaninfo; do
  if command -v "$tool" >/dev/null; then
    case "$tool" in
      nvidia-smi) nvidia-smi --query-gpu=name,driver_version --format=csv,noheader ;;
      glxinfo) glxinfo -B ;;
      vulkaninfo) vulkaninfo --summary ;;
    esac
  else
    echo "$tool: unavailable (optional diagnostic tool)"
  fi
done
echo 'Start with ./start-nyaterm.sh for bundled software OpenGL (no Vulkan/NVIDIA required).'
echo 'ABI checks do not certify every application feature on the target machine.'
exit "$failed"
