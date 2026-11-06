#!/usr/bin/env bash
# Prefer system GPU drivers; keep the private CPU OpenGL path as an opt-in.
set -euo pipefail
app_dir="$(cd -- "$(dirname -- "$0")" && pwd)"
renderer="${NYATERM_RENDERER:-gpu}"
case "${1:-}" in
  --gpu) renderer=gpu; shift ;;
  --software) renderer=software; shift ;;
  --) shift ;;
esac
export NYATERM_RENDERER="$renderer"
case "$renderer" in
  gpu)
    # GPUI prefers available hardware. Preserve explicit system-driver settings;
    # do not inject private Mesa, force llvmpipe, or disable Vulkan here.
    ;;
  software)
    runtime="$app_dir/opengl"
    for required in lib/libEGL.so.1 lib/libEGL_nyaterm.so.1 share/glvnd/egl_vendor.d/50_mesa.json; do
      if [[ ! -r "$runtime/$required" ]]; then
        printf 'NyaTerm: software renderer is incomplete: %s\n' "$runtime/$required" >&2
        exit 78
      fi
    done
    export LD_LIBRARY_PATH="$runtime/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
    export __EGL_VENDOR_LIBRARY_FILENAMES="$runtime/share/glvnd/egl_vendor.d/50_mesa.json"
    export LIBGL_DRIVERS_PATH="$runtime/lib/dri"
    export GALLIUM_DRIVER=llvmpipe LIBGL_ALWAYS_SOFTWARE=1
    export VK_DRIVER_FILES=/dev/null VK_ICD_FILENAMES=/dev/null
    ;;
  *)
    printf 'NyaTerm: NYATERM_RENDERER must be gpu or software (got %s).\n' "$renderer" >&2
    exit 64
    ;;
esac
exec "$app_dir/nyaterm" "$@"
