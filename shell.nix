{ pkgs ? import <nixpkgs> { } }:
let
  pkg = pkgs.callPackage ./nix/package.nix { };
in
pkgs.mkShell {
  inputsFrom = [ pkg ];
  nativeBuildInputs = with pkgs; [
    cargo
    rustc
    rustfmt
    clippy
  ];
  RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
  LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath (pkg.buildInputs ++ [
    pkgs.vulkan-loader
    pkgs.libGL
  ]);
}
