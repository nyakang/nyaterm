#!/usr/bin/env bash
set -euo pipefail

echo "::group::Install Linux system dependencies"

apt_options=(
  -o Acquire::ForceIPv4=true
  -o Acquire::Retries=3
)

# GitHub ARM runners occasionally advertise an unreachable HTTP mirror.
for source_file in /etc/apt/sources.list /etc/apt/sources.list.d/ubuntu.sources; do
  if [[ -f "${source_file}" ]]; then
    sudo sed -i \
      's|http://ports.ubuntu.com/ubuntu-ports|https://ports.ubuntu.com/ubuntu-ports|g' \
      "${source_file}"
  fi
done

sudo apt-get "${apt_options[@]}" -o APT::Update::Error-Mode=any update

packages=(
  build-essential
  clang
  libgtk-3-dev
  libayatana-appindicator3-dev
  libdbus-1-dev
  libfontconfig1-dev
  libfreetype6-dev
  libssl-dev
  libudev-dev
  libwayland-dev
  libx11-dev
  libx11-xcb-dev
  libxcb-cursor-dev
  libxcb-icccm4-dev
  libxcb-image0-dev
  libxcb-keysyms1-dev
  libxcb-randr0-dev
  libxcb-render0-dev
  libxcb-shape0-dev
  libxcb-xfixes0-dev
  libxcb-xinerama0-dev
  libxkbcommon-dev
  libxkbcommon-x11-dev
  libzstd-dev
  libvulkan1
  mesa-vulkan-drivers
  pkg-config
)

if [[ -n "${EXTRA_PACKAGES:-}" ]]; then
  # shellcheck disable=SC2206
  packages+=(${EXTRA_PACKAGES})
fi

sudo apt-get "${apt_options[@]}" install -y "${packages[@]}"
echo "::endgroup::"
