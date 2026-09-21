{ lib
, stdenv
, rustPlatform
, pkg-config
, cmake
, wrapGAppsHook3
, fontconfig
, freetype
, libGL
, openssl
, zstd
, dbus
, systemd
, glib
, gtk3
, libayatana-appindicator
, libxkbcommon
, wayland
, vulkan-loader
, libx11
, libxcursor
, libxi
, libxrandr
, libxcb
, libxcb-cursor
, libxcb-image
, libxcb-keysyms
, libxcb-render-util
, libxcb-wm
, libxcb-util
, apple-sdk_14 ? null
}:

let
  linuxRuntimeLibs = [
    # Desktop & Tray
    gtk3
    glib
    libayatana-appindicator
    dbus
    systemd

    # Graphics & Display
    libGL
    vulkan-loader
    wayland
    libxkbcommon

    # X11
    libx11
    libxcursor
    libxi
    libxrandr
    libxcb
    libxcb-cursor
    libxcb-image
    libxcb-keysyms
    libxcb-render-util
    libxcb-wm
    libxcb-util

    # Font, Text & Core
    fontconfig
    freetype
    openssl
    zstd
  ];
in
rustPlatform.buildRustPackage rec {
  pname = "nyaterm";
  version = (builtins.fromTOML (builtins.readFile ../Cargo.toml)).workspace.package.version;

  src = lib.cleanSourceWith {
    src = lib.cleanSource ../.;
    filter = path: type:
      let
        base = baseNameOf path;
      in
        !(base == "target" || base == "dist" || base == "temp" || base == "result" || base == ".git");
  };

  cargoLock = {
    lockFile = ../Cargo.lock;
    outputHashes = {
      "ironrdp-async-0.10.0" = "sha256-z0DXhRlsN9XuyxX4qN3A6scAHEhDHZnAh3E/RprG/Oc=";
      "alacritty_terminal-0.26.1-dev" = "sha256-ylRyCMNIQRLhrTD9L65viAzgd5RQ7H0TvdG9jwJ+YQE=";
      "gpui-base-0.6.0" = "sha256-+gcxnGAUcR7jUd07aGFCGk4d/wIl3Fa9aGdVBLlQaI8=";
      "pageant-0.2.2" = "sha256-RcJp/h+PLx5zqBJBhuZ1JfiNVIoE0YxZqmXnQN5aBkc=";
      "russh-sftp-2.4.0" = "sha256-HJbN4dIaMvSqL4XgJrfHGu0LUOTsFAJJN6utc09oC2U=";
      "sspi-0.21.4" = "sha256-GkUW3bJ7W87ieYpywgrg5v/I1FCWYIv0E7bJb47wb+w=";
      "vnc-rs-0.5.3" = "sha256-XqMKY1tLqleeHItzSI6k3nKKgKx3fvv9OreNWiWvJio=";
      "collections-0.1.0" = "sha256-un3oM4yQHuMGiwCfqvzzbDkEHHCC4m0/alfsAYzsRQ4=";
      "zmodem2-0.7.2" = "sha256-Qh4I3VR2it4t6rs7/WYr6SlsC7On3DEw4z4fkVQGYHo=";
      "proptest-1.10.0" = "sha256-p5NTcHhruI8QQvANACg8AMRVNmuvGxs2NLit+/8PaWo=";
      "zed-font-kit-0.14.1-zed" = "sha256-KXygi0olNQi5yM8eaJVykNDtbPMDjT+cWPBF8UrtXR4=";
      "zed-scap-0.0.8-zed" = "sha256-BihiQHlal/eRsktyf0GI3aSWsUCW7WcICMsC2Xvb7kw=";
      "wasm_thread-0.3.3" = "sha256-+lRLCIk0S6Y5ORYjDKsYYHia2FtoSoh+rWkQh7mnPBE=";
      "xim-ctext-0.3.0" = "sha256-pRT4Sz1JU9ros47/7pmIW9kosWOGMOItcnNd+VrvnpE=";
    };
  };

  nativeBuildInputs = [
    pkg-config
    cmake
  ]
  ++ lib.optionals stdenv.hostPlatform.isLinux [
    wrapGAppsHook3
  ];

  # aws-lc-sys and other C libraries use cmake, but the crate build should not invoke cmake configure
  dontUseCmakeConfigure = true;

  buildInputs = [
    openssl
    zstd
  ]
  ++ lib.optionals stdenv.hostPlatform.isLinux linuxRuntimeLibs
  ++ lib.optionals (stdenv.hostPlatform.isDarwin && apple-sdk_14 != null) [
    apple-sdk_14
  ];

  cargoBuildFlags = [
    "--package=nyaterm-app"
    "--package=nyaterm-mcp"
    "--package=nyaterm-rdp-helper"
    "--package=nyaterm-vnc-helper"
  ];

  # GUI tests require display server and GPU in sandbox
  doCheck = false;

  postInstall = ''
    # Install desktop entry
    install -Dm644 -T /dev/stdin $out/share/applications/nyaterm.desktop <<'EOF'
[Desktop Entry]
Type=Application
Name=NyaTerm
Comment=Native GPUI terminal workspace with SSH, SFTP, RDP and VNC
Exec=nyaterm %U
Icon=nyaterm
StartupWMClass=nyaterm
Terminal=false
Categories=Development;TerminalEmulator;Network;
MimeType=x-scheme-handler/nyaterm;
StartupNotify=true
EOF

    # Install application icons
    for size in 32 64 128 256 512; do
      icon_file="crates/nyaterm-app/resources/icons/''${size}x''${size}.png"
      if [ -f "$icon_file" ]; then
        install -Dm644 "$icon_file" "$out/share/icons/hicolor/''${size}x''${size}/apps/nyaterm.png"
      fi
    done
  '';

  preFixup = lib.optionalString stdenv.hostPlatform.isLinux ''
    patchelf --add-rpath "${lib.makeLibraryPath linuxRuntimeLibs}" $out/bin/nyaterm

    gappsWrapperArgs+=(
      --prefix LD_LIBRARY_PATH : "${lib.makeLibraryPath linuxRuntimeLibs}"
      --set NYATERM_RDP_HELPER "$out/bin/nyaterm-rdp-helper"
      --set NYATERM_VNC_HELPER "$out/bin/nyaterm-vnc-helper"
      --set NYATERM_MCP_HELPER "$out/bin/nyaterm-mcp"
    )
  '';

  meta = with lib; {
    description = "Native GPUI terminal workspace with SSH, SFTP, RDP and VNC";
    homepage = "https://github.com/nyakang/nyaterm";
    license = licenses.asl20;
    mainProgram = "nyaterm";
    platforms = platforms.linux ++ platforms.darwin;
  };
}
