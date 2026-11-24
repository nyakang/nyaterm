#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" == Darwin ]]; then
  # Runtime bottles may require a newer macOS than the app's 12.0 baseline.
  # Build the private runtime from pinned sources; Homebrew supplies tools only.
  brew install meson ninja pkg-config cmake nasm bison flex
else
  # Packaging modules (including zstandard) are installed into setup-python's
  # interpreter by the caller; Ubuntu 22.04 has no python3-zstandard package.
  sudo apt-get update
  sudo apt-get install -y --no-install-recommends \
    build-essential meson ninja-build pkg-config python3-pyparsing \
    libglib2.0-dev libpixman-1-dev libssl-dev libjpeg-dev zlib1g-dev libjson-glib-dev \
    libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev liblz4-dev libsasl2-dev \
    libopus-dev libsasl2-modules gstreamer1.0-plugins-base gstreamer1.0-plugins-good \
    gstreamer1.0-plugins-bad ca-certificates patchelf
fi
