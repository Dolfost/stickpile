# The Rust workspace, built with crane.
#
# One dependency build (`cargoArtifacts`) is shared by every package and every
# cargo-based check, so dependencies compile once per lockfile change.
# Packages are built per crate (`-p <crate>`) so that one crate that does not
# compile yet does not take the others down with it.
{
  pkgs,
  crane,
}:
let
  inherit (pkgs) lib;
  craneLib = crane.mkLib pkgs;

  # Rust sources, manifests and every data file next to them (migrations,
  # vendored htmx, config examples, test fixtures, .desktop files). nix/,
  # deploy/, docs/ and .github/ stay out, so editing them does not rebuild Rust.
  srcFileset = lib.fileset.unions [
    ../Cargo.toml
    ../Cargo.lock
    ../rustfmt.toml
    (lib.fileset.maybeMissing ../.cargo/config.toml)
    ../crates
    ../spike
  ];
  src = lib.fileset.toSource {
    root = ../.;
    fileset = srcFileset;
  };

  version = (lib.importTOML ../Cargo.toml).workspace.package.version;

  ortLibrary = "${pkgs.onnxruntime}/lib/libonnxruntime.so";

  commonArgs = {
    inherit src version;
    pname = "stickpile";
    strictDeps = true;

    nativeBuildInputs = [ pkgs.pkg-config ];
    buildInputs = [
      pkgs.openssl
      pkgs.libgit2
      pkgs.libssh2
      pkgs.zlib
      # Only stickpile-desktop and the clipboard spike link these, but the
      # shared dependency build covers the whole workspace.
      pkgs.gtk4
      pkgs.libadwaita
    ];

    # git2 0.20 / libgit2-sys 0.18.8+1.9.7 accepts the system libgit2 1.9.x
    # from nixpkgs instead of compiling its vendored copy.
    LIBGIT2_NO_VENDOR = 1;
    # libssh2-sys (git2's `ssh` feature) builds a static libssh2 unless told
    # to use the system one.
    LIBSSH2_SYS_USE_PKG_CONFIG = 1;
  };

  cargoArtifacts = craneLib.buildDepsOnly (
    commonArgs
    // {
      # Default `--locked` plus every member, so dev-dependencies and all
      # targets are cached for clippy and the test suite too.
      cargoExtraArgs = "--locked --workspace";
    }
  );

  mkCrate =
    crate: extra:
    craneLib.buildPackage (
      commonArgs
      // {
        inherit cargoArtifacts;
        pname = crate;
        cargoExtraArgs = "--locked -p ${crate}";
        # The test suite runs once, workspace-wide, in checks.tests.
        doCheck = false;
      }
      // extra
    );

  meta = {
    homepage = "https://github.com/Dolfost/stickpile";
    license = lib.licenses.agpl3Plus;
    platforms = lib.platforms.linux;
  };

  stickpile-server = mkCrate "stickpile-server" {
    nativeBuildInputs = commonArgs.nativeBuildInputs ++ [ pkgs.makeBinaryWrapper ];
    # ffmpeg/ffprobe are looked up on PATH (config `media.ffmpeg = "ffmpeg"`);
    # ORT_DYLIB_PATH is only a default, `ai.onnxruntime_library` overrides it.
    # A binary wrapper keeps bash out of the runtime closure (container image).
    postInstall = ''
      install -Dm644 crates/stickpile-server/config.example.toml \
        $out/share/doc/stickpile/config.example.toml
      wrapProgram $out/bin/stickpile-server \
        --prefix PATH : ${lib.makeBinPath [ pkgs.ffmpeg-headless ]} \
        --set-default ORT_DYLIB_PATH ${ortLibrary}
    '';
    passthru = {
      inherit ortLibrary;
      ffmpeg = pkgs.ffmpeg-headless;
    };
    meta = meta // {
      description = "stickpile server: REST API, web UI, media pipeline, search";
      mainProgram = "stickpile-server";
    };
  };

  stickpile-cli = mkCrate "stickpile-cli" {
    meta = meta // {
      description = "stickpile command-line client";
      mainProgram = "stickpile";
    };
  };

  stickpile-desktop = mkCrate "stickpile-desktop" {
    nativeBuildInputs = commonArgs.nativeBuildInputs ++ [ pkgs.wrapGAppsHook4 ];
    # Desktop entry (io.github.dolfost.Stickpile.desktop), the icon theme tree
    # under data/icons/ and AppStream metadata, whichever of them the crate
    # ships. data/stickpile-desktop.service is only an example for non-Nix
    # installs; home-manager generates the real user unit.
    postInstall = ''
      data=crates/stickpile-desktop/data
      if [ -d "$data" ]; then
        for f in "$data"/*.desktop; do
          [ -e "$f" ] && install -Dm644 "$f" -t $out/share/applications
        done
        if [ -d "$data/icons" ]; then
          mkdir -p $out/share/icons
          cp -r "$data/icons/." $out/share/icons/
        fi
        for f in "$data"/*.metainfo.xml; do
          [ -e "$f" ] && install -Dm644 "$f" -t $out/share/metainfo
        done
      fi
    '';
    meta = meta // {
      description = "stickpile quick-search popup for Linux (GTK4 + libadwaita)";
      mainProgram = "stickpile-desktop";
    };
  };

  container = import ./container.nix { inherit pkgs stickpile-server version; };

  # CLIP model directories (fixed-output downloads), maintained next to the AI
  # code. Optional: absent until nix/models.nix exists and is git-tracked.
  models = lib.optionalAttrs (builtins.pathExists ./models.nix) (
    import ./models.nix { inherit pkgs; }
  );

  # Environment shared by the cargo-based checks.
  checkArgs = commonArgs // {
    inherit cargoArtifacts;
    nativeBuildInputs = commonArgs.nativeBuildInputs ++ [
      pkgs.ffmpeg-headless
      pkgs.git
    ];
    ORT_DYLIB_PATH = ortLibrary;
    # Tests that shell out to git need a writable HOME and an identity.
    GIT_AUTHOR_NAME = "stickpile tests";
    GIT_AUTHOR_EMAIL = "tests@stickpile.invalid";
    GIT_COMMITTER_NAME = "stickpile tests";
    GIT_COMMITTER_EMAIL = "tests@stickpile.invalid";
    preCheck = ''
      export HOME=$(mktemp -d)
    '';
  };
in
{
  inherit
    craneLib
    src
    srcFileset
    version
    commonArgs
    cargoArtifacts
    ;

  packages = {
    inherit
      stickpile-server
      stickpile-cli
      stickpile-desktop
      container
      ;
    default = stickpile-server;
  }
  // lib.optionalAttrs (models != { }) {
    models = models.all;
    models-english = models.english;
    models-multilingual = models.multilingual;
  };

  checks = {
    # Whole-workspace test suite (nextest, one process per test). No test
    # may need a display: GTK-free logic lives in stickpile-quick.
    tests = craneLib.cargoNextest (
      checkArgs
      // {
        cargoNextestExtraArgs = "--workspace --no-tests=warn --no-fail-fast";
      }
    );

    # nextest does not run doctests.
    doctests = craneLib.cargoDocTest (
      checkArgs
      // {
        cargoTestExtraArgs = "--workspace";
      }
    );

    clippy = craneLib.cargoClippy (
      commonArgs
      // {
        inherit cargoArtifacts;
        cargoClippyExtraArgs = "--workspace --all-targets -- --deny warnings";
      }
    );

    fmt = craneLib.cargoFmt {
      inherit src version;
      pname = "stickpile";
    };
  };
}
