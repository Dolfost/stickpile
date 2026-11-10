{
  description = "stickpile — self-hosted sticker and meme library";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";
    crane.url = "github:ipetkov/crane";
    # Only for the home-manager module check (a real home-manager evaluation).
    home-manager = {
      url = "github:nix-community/home-manager/release-26.05";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      crane,
      home-manager,
      ...
    }:
    let
      inherit (nixpkgs) lib;
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = f: lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});

      # nix/packages.nix: crane build of the workspace (packages + cargo checks).
      rustFor = forAllSystems (pkgs: import ./nix/packages.nix { inherit pkgs crane; });
      # nix/checks: module evaluation, config drift guard, VM test.
      moduleChecksFor = forAllSystems (
        pkgs:
        import ./nix/checks {
          inherit
            pkgs
            self
            nixpkgs
            home-manager
            ;
          rust = rustFor.${pkgs.stdenv.hostPlatform.system};
        }
      );
    in
    {
      packages = forAllSystems (pkgs: rustFor.${pkgs.stdenv.hostPlatform.system}.packages);

      overlays.default = final: _prev: {
        inherit (self.packages.${final.stdenv.hostPlatform.system})
          stickpile-server
          stickpile-cli
          stickpile-desktop
          ;
      };

      nixosModules.default =
        { lib, pkgs, ... }:
        {
          imports = [ ./nix/modules/nixos.nix ];
          services.stickpile.package =
            lib.mkDefault
              self.packages.${pkgs.stdenv.hostPlatform.system}.stickpile-server;
        };

      homeManagerModules.default =
        { lib, pkgs, ... }:
        {
          imports = [ ./nix/modules/home-manager.nix ];
          programs.stickpile = {
            package = lib.mkDefault self.packages.${pkgs.stdenv.hostPlatform.system}.stickpile-cli;
            desktop.package = lib.mkDefault self.packages.${pkgs.stdenv.hostPlatform.system}.stickpile-desktop;
          };
        };

      # `nix flake check`: every package (models excluded: large downloads),
      # tests, doctests, clippy -D warnings, rustfmt, module checks.
      checks = forAllSystems (
        pkgs:
        let
          system = pkgs.stdenv.hostPlatform.system;
          rust = rustFor.${system};
        in
        lib.filterAttrs (
          name: _:
          !(lib.elem name [
            "default"
            "models"
            "models-english"
            "models-multilingual"
          ])
        ) rust.packages
        // rust.checks
        // moduleChecksFor.${system}.checks
      );

      # `nix fmt`: nixfmt over the repository's Nix files, except the aorus
      # snippet, which follows the owner's dotfiles style (tabs).
      formatter = forAllSystems (
        pkgs:
        pkgs.nixfmt-tree.override {
          settings.global.excludes = [ "deploy/aorus/*" ];
        }
      );

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          packages = with pkgs; [
            cargo
            rustc
            clippy
            rustfmt
            rust-analyzer
            cargo-llvm-cov
            sccache
            pkg-config
            openssl
            sqlite
            gtk4
            libadwaita
            glib
            graphene
            onnxruntime
            ffmpeg-headless
            git
            libgit2
            wl-clipboard
            cargo-nextest
          ];
          ORT_DYLIB_PATH = "${pkgs.onnxruntime}/lib/libonnxruntime.so";
          RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
          # cargo-llvm-cov needs the llvm-cov/llvm-profdata of rustc's own LLVM.
          LLVM_COV = "${pkgs.rustc.llvmPackages.llvm}/bin/llvm-cov";
          LLVM_PROFDATA = "${pkgs.rustc.llvmPackages.llvm}/bin/llvm-profdata";
          # A global LD_LIBRARY_PATH from the host profile breaks ffmpeg and GTK
          # binaries built against this nixpkgs' glibc.
          # Compiler cache and temp files live under the checkout's target/
          # (git-ignored, next to the build artefacts) instead of ~/.cache and
          # /tmp, which may sit on a small root filesystem.
          shellHook = ''
            unset LD_LIBRARY_PATH
            stickpile_target="$(git rev-parse --show-toplevel 2>/dev/null || pwd)/target"
            mkdir -p "$stickpile_target/sccache" "$stickpile_target/tmp"
            export RUSTC_WRAPPER=sccache
            export SCCACHE_DIR=''${SCCACHE_DIR:-$stickpile_target/sccache}
            export SCCACHE_CACHE_SIZE=20G
            export TMPDIR="$stickpile_target/tmp"
            unset stickpile_target
          '';
        };
      });
    };
}
