{
  description = "stickpile — self-hosted sticker and meme library";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";
    crane.url = "github:ipetkov/crane";
  };

  outputs = { self, nixpkgs, crane, ... }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          packages = with pkgs; [
            cargo rustc clippy rustfmt rust-analyzer cargo-llvm-cov sccache
            pkg-config openssl sqlite
            gtk4 libadwaita glib graphene
            onnxruntime ffmpeg-headless git libgit2
            wl-clipboard
          ];
          ORT_DYLIB_PATH = "${pkgs.onnxruntime}/lib/libonnxruntime.so";
          RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
          # A global LD_LIBRARY_PATH from the host profile breaks ffmpeg and GTK
          # binaries built against this nixpkgs' glibc.
          shellHook = ''
            unset LD_LIBRARY_PATH
            export RUSTC_WRAPPER=sccache
            export SCCACHE_DIR=''${SCCACHE_DIR:-$HOME/.cache/sccache-stickpile}
          '';
        };
      });
    };
}
