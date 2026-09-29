{ lib
, stdenv
, rustPlatform
, fetchPnpmDeps
, pnpmConfigHook
, # Node and pnpm versions:
  # nixpkgs' pnpm_9 has known CVEs; pnpm_10 is secure and reads lockfile v9 seamlessly.
  # nodejs defaults to the current supported LTS in nixpkgs.
  nodejs
, pnpm_10
, pkg-config
, wrapGAppsHook3
, gtk3
, webkitgtk_4_1
, libsoup_3
, libappindicator-gtk3
, libayatana-appindicator
, librsvg
, openssl
, udev
, xdg-utils
, desktop-file-utils
, cacert
, copyDesktopItems
, makeDesktopItem
, githubGistClientId ? null
,
}:

let
  packageJson = builtins.fromJSON (builtins.readFile ../package.json);
  version = packageJson.version;
  src = lib.cleanSourceWith {
    src = ../.;
    filter = path: type:
      let
        baseName = baseNameOf path;
      in
        !(
          baseName == "target" ||
          baseName == "node_modules" ||
          baseName == "dist" ||
          baseName == ".git" ||
          lib.hasPrefix "result" baseName
        );
  };

  mcp-sidecar = rustPlatform.buildRustPackage {
    pname = "nyaterm-mcp";
    inherit version src;

    cargoLock = {
      lockFile = ../src-tauri/crates/nyaterm-mcp/Cargo.lock;
    };
    cargoRoot = "src-tauri/crates/nyaterm-mcp";
    buildAndTestSubdir = "src-tauri/crates/nyaterm-mcp";

    doCheck = false;
  };

  frontend = stdenv.mkDerivation {
    pname = "nyaterm-frontend";
    inherit version src;

    pnpmDeps = fetchPnpmDeps {
      pname = "nyaterm";
      inherit version src;
      pnpm = pnpm_10;
      fetcherVersion = 4;
      hash = "sha256-2Qtar7sVKGGhWR2vQsKH4UyUN7WiOoaqKCZcw6IDD1A=";
      # Uses npmmirror registry to avoid network timeouts during dependency fetch;
      # integrity is strictly verified by the hash above.
      prePnpmInstall = ''
        echo registry=https://registry.npmmirror.com >> ~/.npmrc
        echo registry=https://registry.npmmirror.com >> .npmrc
      '';
    };

    nativeBuildInputs = [
      nodejs
      pnpm_10
      pnpmConfigHook
    ];

    preBuild = ''
      # pnpmConfigHook skips root postinstall in Nix sandbox; run it explicitly
      pnpm run postinstall
    '';

    buildPhase = ''
      runHook preBuild
      pnpm run build:frontend
      runHook postBuild
    '';

    installPhase = ''
      runHook preInstall
      mkdir -p $out
      cp -r dist/. $out
      runHook postInstall
    '';
  };
in
rustPlatform.buildRustPackage {
  pname = "nyaterm";
  inherit version src;

  cargoLock = {
    lockFile = ../src-tauri/Cargo.lock;
  };
  cargoRoot = "src-tauri";
  # Tauri's context macro otherwise selects devUrl and omits embedded frontend assets.
  cargoBuildFlags = [ "--features=tauri/custom-protocol" ];
  buildAndTestSubdir = "src-tauri";

  nativeBuildInputs = [
    pkg-config
    wrapGAppsHook3
    copyDesktopItems
  ];

  buildInputs = [
    gtk3
    webkitgtk_4_1
    libsoup_3
    libappindicator-gtk3
    libayatana-appindicator
    librsvg
    openssl
    udev
  ];

  NYATERM_PACKAGE_MANAGER = "nix";
  NYATERM_GITHUB_GIST_CLIENT_ID = lib.optionalString (githubGistClientId != null) githubGistClientId;

  preBuild = ''
    # Provide built frontend for tauri-build
    cp -r ${frontend} dist

    # Provide externalBin sidecar for tauri-build
    mkdir -p src-tauri/binaries
    cp ${mcp-sidecar}/bin/nyaterm-mcp src-tauri/binaries/nyaterm-mcp
    cp ${mcp-sidecar}/bin/nyaterm-mcp src-tauri/binaries/nyaterm-mcp-${stdenv.hostPlatform.rust.rustcTarget}
  '';

  desktopItems = [
    (makeDesktopItem {
      name = "nyaterm";
      desktopName = "NyaTerm";
      comment = "A modern, high-performance SSH client and terminal workspace";
      exec = "nyaterm %U";
      icon = "nyaterm";
      categories = [
        "System"
        "TerminalEmulator"
      ];
      mimeTypes = [
        "x-scheme-handler/nyaterm"
        "x-scheme-handler/ssh"
        "x-scheme-handler/telnet"
      ];
      startupWMClass = "nyaterm";
    })
  ];

  postInstall = ''
    # Ensure sidecar is installed in same directory as nyaterm binary
    install -m 755 ${mcp-sidecar}/bin/nyaterm-mcp $out/bin/nyaterm-mcp

    # Install desktop icons
    install -Dm 644 src-tauri/icons/32x32.png $out/share/icons/hicolor/32x32/apps/nyaterm.png
    install -Dm 644 src-tauri/icons/128x128.png $out/share/icons/hicolor/128x128/apps/nyaterm.png
    install -Dm 644 src-tauri/icons/128x128@2x.png $out/share/icons/hicolor/256x256/apps/nyaterm.png
  '';

  preFixup = ''
    gappsWrapperArgs+=(
      --prefix PATH : "${lib.makeBinPath [ xdg-utils desktop-file-utils ]}"
      --prefix LD_LIBRARY_PATH : "${lib.makeLibraryPath [ libayatana-appindicator libappindicator-gtk3 ]}"
      --set-default SSL_CERT_FILE "${cacert}/etc/ssl/certs/ca-bundle.crt"
    )
  '';

  doCheck = false;

  passthru = {
    inherit mcp-sidecar frontend;
  };

  meta = with lib; {
    description = "A modern, high-performance SSH client built with Tauri and React";
    homepage = "https://nyaterm.app";
    license = licenses.mit;
    platforms = [ "x86_64-linux" "aarch64-linux" ];
    mainProgram = "nyaterm";
  };
}
