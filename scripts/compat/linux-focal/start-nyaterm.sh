#!/usr/bin/env bash
# Private, CPU-rendered OpenGL path for the offline focal desktop.
set -euo pipefail
app_dir="$(cd -- "$(dirname -- "$0")" && pwd)"
runtime="$app_dir/opengl"
export LD_LIBRARY_PATH="$runtime/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
export __EGL_VENDOR_LIBRARY_FILENAMES="$runtime/share/glvnd/egl_vendor.d/50_mesa.json"
export LIBGL_DRIVERS_PATH="$runtime/lib/dri"
export GALLIUM_DRIVER=llvmpipe LIBGL_ALWAYS_SOFTWARE=1
# Do not rely on Vulkan or NVIDIA proprietary drivers for this entry point.
export VK_DRIVER_FILES=/dev/null VK_ICD_FILENAMES=/dev/null
exec "$app_dir/nyaterm" "$@"
