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
, version ? (builtins.fromTOML (builtins.readFile ../Cargo.toml)).workspace.package.version
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

  isPrerelease = lib.hasInfix "-" version;
  identity = if isPrerelease then {
    displayName = "NyaTerm Preview";
    desktopId = "nyaterm-preview";
  } else {
    displayName = "NyaTerm";
    desktopId = "nyaterm";
  };
in
rustPlatform.buildRustPackage rec {
  pname = identity.desktopId;
  inherit version;

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
      "alacritty_terminal-0.26.1-dev" = "sha256-ylRyCMNIQRLhrTD9L65viAzgd5RQ7H0TvdG9jwJ+YQE=";
      "collections-0.1.0" = "sha256-7/EnmEmIEf5jnGOq4QVyMuZ9gkUrTHwU7oGaOKlX1iA=";
      "derive_refineable-0.1.0" = "sha256-7/EnmEmIEf5jnGOq4QVyMuZ9gkUrTHwU7oGaOKlX1iA=";
      "gpui-0.2.2" = "sha256-7/EnmEmIEf5jnGOq4QVyMuZ9gkUrTHwU7oGaOKlX1iA=";
      "gpui-base-0.6.5" = "sha256-dcU/83z88+7X7Y97V9MCpYQGQbm9Wnqzt0P9VVrI22o=";
      "gpui-component-0.6.5" = "sha256-dcU/83z88+7X7Y97V9MCpYQGQbm9Wnqzt0P9VVrI22o=";
      "gpui-component-macros-0.6.5" = "sha256-dcU/83z88+7X7Y97V9MCpYQGQbm9Wnqzt0P9VVrI22o=";
      "gpui-kit-0.6.5" = "sha256-dcU/83z88+7X7Y97V9MCpYQGQbm9Wnqzt0P9VVrI22o=";
      "gpui-kit-assets-0.6.5" = "sha256-dcU/83z88+7X7Y97V9MCpYQGQbm9Wnqzt0P9VVrI22o=";
      "gpui_apple-0.1.0" = "sha256-7/EnmEmIEf5jnGOq4QVyMuZ9gkUrTHwU7oGaOKlX1iA=";
      "gpui_linux-0.1.0" = "sha256-7/EnmEmIEf5jnGOq4QVyMuZ9gkUrTHwU7oGaOKlX1iA=";
      "gpui_macos-0.1.0" = "sha256-7/EnmEmIEf5jnGOq4QVyMuZ9gkUrTHwU7oGaOKlX1iA=";
      "gpui_macros-0.1.0" = "sha256-7/EnmEmIEf5jnGOq4QVyMuZ9gkUrTHwU7oGaOKlX1iA=";
      "gpui_platform-0.1.0" = "sha256-7/EnmEmIEf5jnGOq4QVyMuZ9gkUrTHwU7oGaOKlX1iA=";
      "gpui_shared_string-0.1.0" = "sha256-7/EnmEmIEf5jnGOq4QVyMuZ9gkUrTHwU7oGaOKlX1iA=";
      "gpui_util-0.1.0" = "sha256-7/EnmEmIEf5jnGOq4QVyMuZ9gkUrTHwU7oGaOKlX1iA=";
      "gpui_web-0.1.0" = "sha256-7/EnmEmIEf5jnGOq4QVyMuZ9gkUrTHwU7oGaOKlX1iA=";
      "gpui_wgpu-0.1.0" = "sha256-7/EnmEmIEf5jnGOq4QVyMuZ9gkUrTHwU7oGaOKlX1iA=";
      "gpui_windows-0.1.0" = "sha256-7/EnmEmIEf5jnGOq4QVyMuZ9gkUrTHwU7oGaOKlX1iA=";
      "http_client-0.1.0" = "sha256-7/EnmEmIEf5jnGOq4QVyMuZ9gkUrTHwU7oGaOKlX1iA=";
      "ironrdp-async-0.10.0" = "sha256-z0DXhRlsN9XuyxX4qN3A6scAHEhDHZnAh3E/RprG/Oc=";
      "ironrdp-bulk-0.1.1" = "sha256-z0DXhRlsN9XuyxX4qN3A6scAHEhDHZnAh3E/RprG/Oc=";
      "ironrdp-cfg-0.1.0" = "sha256-z0DXhRlsN9XuyxX4qN3A6scAHEhDHZnAh3E/RprG/Oc=";
      "ironrdp-client-0.1.0" = "sha256-z0DXhRlsN9XuyxX4qN3A6scAHEhDHZnAh3E/RprG/Oc=";
      "ironrdp-cliprdr-0.7.0" = "sha256-z0DXhRlsN9XuyxX4qN3A6scAHEhDHZnAh3E/RprG/Oc=";
      "ironrdp-cliprdr-native-0.7.0" = "sha256-z0DXhRlsN9XuyxX4qN3A6scAHEhDHZnAh3E/RprG/Oc=";
      "ironrdp-connector-0.10.0" = "sha256-z0DXhRlsN9XuyxX4qN3A6scAHEhDHZnAh3E/RprG/Oc=";
      "ironrdp-core-0.2.1" = "sha256-z0DXhRlsN9XuyxX4qN3A6scAHEhDHZnAh3E/RprG/Oc=";
      "ironrdp-displaycontrol-0.8.0" = "sha256-z0DXhRlsN9XuyxX4qN3A6scAHEhDHZnAh3E/RprG/Oc=";
      "ironrdp-dvc-0.8.0" = "sha256-z0DXhRlsN9XuyxX4qN3A6scAHEhDHZnAh3E/RprG/Oc=";
      "ironrdp-echo-0.4.0" = "sha256-z0DXhRlsN9XuyxX4qN3A6scAHEhDHZnAh3E/RprG/Oc=";
      "ironrdp-error-0.2.0" = "sha256-z0DXhRlsN9XuyxX4qN3A6scAHEhDHZnAh3E/RprG/Oc=";
      "ironrdp-graphics-0.9.0" = "sha256-z0DXhRlsN9XuyxX4qN3A6scAHEhDHZnAh3E/RprG/Oc=";
      "ironrdp-input-0.7.0" = "sha256-z0DXhRlsN9XuyxX4qN3A6scAHEhDHZnAh3E/RprG/Oc=";
      "ironrdp-mstsgu-0.0.1" = "sha256-z0DXhRlsN9XuyxX4qN3A6scAHEhDHZnAh3E/RprG/Oc=";
      "ironrdp-pdu-0.9.0" = "sha256-z0DXhRlsN9XuyxX4qN3A6scAHEhDHZnAh3E/RprG/Oc=";
      "ironrdp-propertyset-0.1.0" = "sha256-z0DXhRlsN9XuyxX4qN3A6scAHEhDHZnAh3E/RprG/Oc=";
      "ironrdp-rail-0.1.0" = "sha256-z0DXhRlsN9XuyxX4qN3A6scAHEhDHZnAh3E/RprG/Oc=";
      "ironrdp-rdcleanpath-0.2.2" = "sha256-z0DXhRlsN9XuyxX4qN3A6scAHEhDHZnAh3E/RprG/Oc=";
      "ironrdp-rdpei-0.1.0" = "sha256-z0DXhRlsN9XuyxX4qN3A6scAHEhDHZnAh3E/RprG/Oc=";
      "ironrdp-session-0.11.0" = "sha256-z0DXhRlsN9XuyxX4qN3A6scAHEhDHZnAh3E/RprG/Oc=";
      "ironrdp-svc-0.8.0" = "sha256-z0DXhRlsN9XuyxX4qN3A6scAHEhDHZnAh3E/RprG/Oc=";
      "ironrdp-tls-0.2.2" = "sha256-z0DXhRlsN9XuyxX4qN3A6scAHEhDHZnAh3E/RprG/Oc=";
      "ironrdp-tokio-0.10.0" = "sha256-z0DXhRlsN9XuyxX4qN3A6scAHEhDHZnAh3E/RprG/Oc=";
      "pageant-0.2.2" = "sha256-RcJp/h+PLx5zqBJBhuZ1JfiNVIoE0YxZqmXnQN5aBkc=";
      "perf-0.1.0" = "sha256-7/EnmEmIEf5jnGOq4QVyMuZ9gkUrTHwU7oGaOKlX1iA=";
      "proptest-1.10.0" = "sha256-p5NTcHhruI8QQvANACg8AMRVNmuvGxs2NLit+/8PaWo=";
      "proptest-macro-0.5.0" = "sha256-p5NTcHhruI8QQvANACg8AMRVNmuvGxs2NLit+/8PaWo=";
      "refineable-0.1.0" = "sha256-7/EnmEmIEf5jnGOq4QVyMuZ9gkUrTHwU7oGaOKlX1iA=";
      "russh-0.63.1" = "sha256-RcJp/h+PLx5zqBJBhuZ1JfiNVIoE0YxZqmXnQN5aBkc=";
      "russh-cryptovec-0.62.0" = "sha256-RcJp/h+PLx5zqBJBhuZ1JfiNVIoE0YxZqmXnQN5aBkc=";
      "russh-sftp-2.4.0" = "sha256-HJbN4dIaMvSqL4XgJrfHGu0LUOTsFAJJN6utc09oC2U=";
      "russh-util-0.52.0" = "sha256-RcJp/h+PLx5zqBJBhuZ1JfiNVIoE0YxZqmXnQN5aBkc=";
      "scheduler-0.1.0" = "sha256-7/EnmEmIEf5jnGOq4QVyMuZ9gkUrTHwU7oGaOKlX1iA=";
      "sspi-0.21.4" = "sha256-GkUW3bJ7W87ieYpywgrg5v/I1FCWYIv0E7bJb47wb+w=";
      "sum_tree-0.1.0" = "sha256-7/EnmEmIEf5jnGOq4QVyMuZ9gkUrTHwU7oGaOKlX1iA=";
      "util_macros-0.1.0" = "sha256-7/EnmEmIEf5jnGOq4QVyMuZ9gkUrTHwU7oGaOKlX1iA=";
      "vnc-rs-0.5.3" = "sha256-XqMKY1tLqleeHItzSI6k3nKKgKx3fvv9OreNWiWvJio=";
      "wasm_thread-0.3.3" = "sha256-+lRLCIk0S6Y5ORYjDKsYYHia2FtoSoh+rWkQh7mnPBE=";
      "xim-ctext-0.3.0" = "sha256-pRT4Sz1JU9ros47/7pmIW9kosWOGMOItcnNd+VrvnpE=";
      "xim-parser-0.2.1" = "sha256-pRT4Sz1JU9ros47/7pmIW9kosWOGMOItcnNd+VrvnpE=";
      "zed-font-kit-0.14.1-zed" = "sha256-KXygi0olNQi5yM8eaJVykNDtbPMDjT+cWPBF8UrtXR4=";
      "zed-scap-0.0.8-zed" = "sha256-BihiQHlal/eRsktyf0GI3aSWsUCW7WcICMsC2Xvb7kw=";
      "zed-xim-0.4.0-zed" = "sha256-pRT4Sz1JU9ros47/7pmIW9kosWOGMOItcnNd+VrvnpE=";
      "zlog-0.1.0" = "sha256-7/EnmEmIEf5jnGOq4QVyMuZ9gkUrTHwU7oGaOKlX1iA=";
      "zmodem2-0.7.2" = "sha256-Qh4I3VR2it4t6rs7/WYr6SlsC7On3DEw4z4fkVQGYHo=";
      "ztracing-0.1.0" = "sha256-7/EnmEmIEf5jnGOq4QVyMuZ9gkUrTHwU7oGaOKlX1iA=";
      "ztracing_macro-0.1.0" = "sha256-7/EnmEmIEf5jnGOq4QVyMuZ9gkUrTHwU7oGaOKlX1iA=";
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
    install -Dm644 -T /dev/stdin $out/share/applications/${identity.desktopId}.desktop <<EOF
[Desktop Entry]
Type=Application
Name=${identity.displayName}
Comment=Native GPUI terminal workspace with SSH, SFTP, RDP and VNC
Exec=nyaterm %U
Icon=${identity.desktopId}
StartupWMClass=${identity.desktopId}
Terminal=false
Categories=Development;TerminalEmulator;Network;
MimeType=x-scheme-handler/${identity.desktopId};
StartupNotify=true
EOF

    # Install application icons
    for size in 32 64 128 256 512; do
      icon_file="crates/nyaterm-app/resources/icons/''${size}x''${size}.png"
      if [ -f "$icon_file" ]; then
        install -Dm644 "$icon_file" "$out/share/icons/hicolor/''${size}x''${size}/apps/${identity.desktopId}.png"
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

  postFixup = lib.optionalString (identity.desktopId != "nyaterm") ''
    ln -s nyaterm $out/bin/${identity.desktopId}
  '';

  passthru = {
    inherit identity;
  };

  meta = with lib; {
    description = "Native GPUI terminal workspace with SSH, SFTP, RDP and VNC";
    homepage = "https://github.com/nyakang/nyaterm";
    license = licenses.asl20;
    mainProgram = "nyaterm";
    platforms = platforms.linux ++ platforms.darwin;
  };
}
