{
  description = "Native GPUI terminal workspace with SSH, SFTP, RDP and VNC";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
  };

  outputs = { self, nixpkgs }:
    let
      supportedSystems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
      ];
      forAllSystems = nixpkgs.lib.genAttrs supportedSystems;
      pkgsFor = system: import nixpkgs { inherit system; };
    in
    {
      packages = forAllSystems (system:
        let
          pkgs = pkgsFor system;
        in
        {
          default = pkgs.callPackage ./nix/package.nix { };
          nyaterm = self.packages.${system}.default;
        }
      );

      apps = forAllSystems (system: {
        default = {
          type = "app";
          program = "${self.packages.${system}.default}/bin/nyaterm";
          meta.description = "Launch NyaTerm";
        };
      });

      devShells = forAllSystems (system:
        let
          pkgs = pkgsFor system;
          nyaterm = self.packages.${system}.default;
        in
        {
          default = pkgs.mkShell (
            {
              inputsFrom = [ nyaterm ];
              nativeBuildInputs = with pkgs; [
                cargo
                rustc
                rustfmt
                clippy
              ];
              RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
            }
            // pkgs.lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
              LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath (
                nyaterm.buildInputs
                ++ [
                  pkgs.vulkan-loader
                  pkgs.libGL
                ]
              );
            }
          );
        }
      );
    };
}
