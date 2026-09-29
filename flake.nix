{
  description = "A modern, high-performance SSH client and remote terminal workspace";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs = { self, nixpkgs }:
    let
      supportedSystems = [ "x86_64-linux" "aarch64-linux" ];
      forAllSystems = nixpkgs.lib.genAttrs supportedSystems;
    in
    {
      overlays = {
        default = final: prev: {
          nyaterm = final.callPackage ./nix/package.nix { };
        };
        nyaterm = self.overlays.default;
      };

      packages = forAllSystems (system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ self.overlays.default ];
          };
        in
        {
          nyaterm = pkgs.nyaterm;
          default = self.packages.${system}.nyaterm;
        }
      );

      apps = forAllSystems (system: {
        nyaterm = {
          type = "app";
          program = "${self.packages.${system}.nyaterm}/bin/nyaterm";
          meta = self.packages.${system}.nyaterm.meta;
        };
        default = self.apps.${system}.nyaterm;
      });

      checks = forAllSystems (system: {
        nyaterm = self.packages.${system}.nyaterm;
      });

      formatter = forAllSystems (system: nixpkgs.legacyPackages.${system}.nixpkgs-fmt);

      devShells = forAllSystems (system:
        let
          pkgs = import nixpkgs { inherit system; };
        in
        {
          default = pkgs.mkShell {
            inputsFrom = [ self.packages.${system}.nyaterm ];
            packages = with pkgs; [
              cargo-tauri
              pnpm_10 # Note: pnpm_10 used as nixpkgs' pnpm_9 has known CVEs
              nodejs
              rustc
              cargo
              rustfmt
              clippy
              pkg-config
            ];
            RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
            # Derived dynamically from package buildInputs to avoid dependency drift
            LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath self.packages.${system}.nyaterm.buildInputs;
          };
        }
      );
    };
}
