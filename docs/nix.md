# Nix

The flake (`flake.nix`, with everything else in `nix/`) builds every package,
provides the dev shell, the NixOS and home-manager modules and the container
image, and runs the full test suite in `nix flake check`. Linux only
(`x86_64-linux`, `aarch64-linux`). Inputs: nixpkgs `nixos-26.05`, crane, and
home-manager `release-26.05` (used only by the home-manager module check).

Nix sees only files that git knows about. A new file stays invisible to
`nix build` and `nix flake check` until it is added to git (`git add -N` is
enough).

## Outputs

| Output | What it is |
|---|---|
| `packages.<system>.stickpile-server` (also `default`) | The server, wrapped so that `ffmpeg`/`ffprobe` (ffmpeg-headless) are on its PATH and `ORT_DYLIB_PATH` defaults to nixpkgs' onnxruntime. `config.example.toml` is in `share/doc/stickpile/`. |
| `packages.<system>.stickpile-cli` | The `stickpile` CLI. |
| `packages.<system>.stickpile-desktop` | The GTK4/libadwaita popup (wrapGAppsHook4), plus the `.desktop` file, icons and metainfo from `crates/stickpile-desktop/data/` when they exist. |
| `packages.<system>.container` | OCI image `ghcr.io/dolfost/stickpile:<version>` (`podman load < result`). See [deployment.md](deployment.md#container-image). |
| `packages.<system>.models`, `models-english`, `models-multilingual` | CLIP model directories (fixed-output downloads, about 750, 610 and 490 MB) from `nix/models.nix`, to point `ai.model_dir` at. Present once that file is tracked. Not built by `nix flake check`. |
| `nixosModules.default` | `services.stickpile` ([deployment.md](deployment.md#nixos)) |
| `homeManagerModules.default` | `programs.stickpile`, see [below](#home-manager) |
| `overlays.default` | `stickpile-server`, `stickpile-cli`, `stickpile-desktop` in `pkgs` |
| `devShells.<system>.default` | The development shell |
| `checks.<system>.*` | See [Checks](#checks) |
| `formatter.<system>` | `nix fmt`: nixfmt over the Nix files. `deploy/aorus/` is left alone because it follows the owner's dotfiles style (tabs). |

### How the Rust build works

`nix/packages.nix` uses crane:

- **Source**: `Cargo.toml`, `Cargo.lock`, `rustfmt.toml`, and the `crates/`
  and `spike/` trees with every file in them (migrations, vendored htmx, config
  examples, fixtures, `.desktop` files). `nix/`, `deploy/`, `docs/` and
  `.github/` are left out, so editing them does not trigger a Rust rebuild. If
  a crate starts to `include_str!` something outside `crates/` (for example a
  file in `docs/`), add that path to `srcFileset`.
- **One dependency build** (`cargoArtifacts`, `cargo build/check/test
  --no-run --workspace` on a dummy source) is shared by every package and
  every cargo check. It is rebuilt only when `Cargo.lock` or a manifest
  changes.
- **Packages are built per crate** (`cargo build -p <crate>`), so a crate that
  does not compile does not break the others. Their tests do not run there,
  because they run once, workspace-wide, in `checks.tests`.
- **Native inputs**: pkg-config, openssl, zlib, gtk4, libadwaita, libgit2 and
  libssh2. libgit2 is the system 1.9 (`LIBGIT2_NO_VENDOR=1`; libgit2-sys
  0.18.8+1.9.7 accepts it), and libssh2 is the system one
  (`LIBSSH2_SYS_USE_PKG_CONFIG=1`). SQLite (sqlx, rusqlite), libwebp,
  oniguruma and ring's assembly are compiled from bundled sources. reqwest
  uses rustls with webpki roots. ort uses `load-dynamic`, so nothing links
  ONNX Runtime at build time. No git binary is needed at runtime: tests use
  the `git` from the check environment.
- **Wrappers** are binary wrappers (`makeBinaryWrapper`), so the server's
  closure needs no shell of its own.

## Dev shell

`nix develop` gives cargo, rustc, clippy, rustfmt, rust-analyzer,
cargo-nextest, cargo-llvm-cov, sccache, pkg-config, openssl, sqlite, gtk4,
libadwaita, glib, graphene, onnxruntime, ffmpeg-headless, git, libgit2 and
wl-clipboard. It also sets:

- `ORT_DYLIB_PATH`: nixpkgs' `libonnxruntime.so`.
- `LLVM_COV` and `LLVM_PROFDATA`: the tools of rustc's own LLVM. With these,
  `cargo llvm-cov` works with nixpkgs' rustc (which ships the profiler
  runtime).
- `RUSTC_WRAPPER=sccache`, with `SCCACHE_DIR` and `TMPDIR` under the checkout's
  `target/` (`target/sccache`, `target/tmp`) instead of `~/.cache` and `/tmp`.
- It unsets `LD_LIBRARY_PATH`, because a host-wide one breaks binaries built
  against this nixpkgs' glibc.

Coverage, as CI runs it:

```sh
nix develop -c cargo llvm-cov --workspace --exclude stickpile-desktop --exclude clipspike --html
```

## Checks

`nix flake check -L` builds and runs all of these. To run a single one, use
`nix build -L .#checks.x86_64-linux.<name>`.

| Check | What it proves |
|---|---|
| `stickpile-server`, `stickpile-cli`, `stickpile-desktop`, `container` | Every package builds. The image check builds the full layered image tarball. |
| `tests` | `cargo nextest run --workspace` (release profile) with ffmpeg, git and `ORT_DYLIB_PATH` available and a temporary `HOME`. Tests must not need a display or the network. |
| `doctests` | `cargo test --doc --workspace` (nextest skips doctests). |
| `clippy` | `cargo clippy --workspace --all-targets -- --deny warnings` |
| `fmt` | `cargo fmt --check` with the repo's `rustfmt.toml` |
| `config-drift` | The NixOS and home-manager module defaults equal the Rust defaults, key by key. See [The drift guard](#the-drift-guard). |
| `nixos-module` | `nix/examples/nixos.nix` (every option set) evaluates as a whole NixOS system with its assertions. The real `stickpile-server --config <generated> check-config` accepts the generated TOML. Secret paths are rewritten to the service's credentials, and the unit has its `LoadCredential=` lines and hardening. |
| `home-manager-module` | `nix/examples/home-manager.nix` (every option set) evaluates in a real home-manager configuration. The real `stickpile --config <generated> config check` accepts the TOML, and the `stickpile-desktop` user unit runs `stickpile-desktop --hidden` and is `PartOf=graphical-session.target`. |
| `deploy-configs` | `deploy/quadlet/config.toml` and the config baked into the image both pass `check-config`. |
| `nixos-vm` (x86_64 only) | A NixOS VM runs the module with its full hardening (including `MemoryDenyWriteExecute`). `/healthz` must answer over TCP and over the trusted unix socket (mode 0660), the database must belong to the service user. The initial-admin password is a root-only (0400) file, passed in through `LoadCredential`. The test prints the `systemd-analyze security` score. Needs KVM. |

## The drift guard

The server and client configs are mirrored one-to-one in Nix modules, and
the two must never drift. Three layers enforce this, and each fails loudly.

1. **`checks.config-drift`, from both sides.** `nix/drift-guard/` is a small
   Rust program that compiles the **real** config modules with `#[path]`
   includes: `crates/stickpile-server/src/config.rs` and
   `crates/stickpile-core/src/config.rs`. It serialises `Config::default()`
   and `ClientConfig::default()` to JSON. `None` becomes `null`, so optional
   fields count too. It then compares that JSON with the module's evaluated
   `settings` with nothing set (`builtins.toJSON`, nulls kept), recursively.
   A key that exists on only one side fails the check, and so does a default
   that differs, for example:

   ```
   CONFIG DRIFT between the Nix module (services.stickpile.settings) and crates/stickpile-server/src/config.rs (Config):
     - ai.max_emojis: field exists in Rust (default 3) but has no Nix option
     - media.thumb_px: default differs: Nix 255, Rust 256
   ```

   It also feeds the module's rendered default TOML through the real
   `from_toml` loader (`deny_unknown_fields`) and requires the result to
   equal the default. This catches type mismatches between the module and
   Rust, and keys that the TOML rendering would mangle.

   The program builds against the workspace `Cargo.lock` and its vendored
   crates. The included config modules must stay self-contained: they may use
   std, serde, toml, thiserror, directories and `stickpile_api`. If one of
   them grows another dependency, add it to `nix/drift-guard/Cargo.toml`;
   otherwise this check fails to compile.

2. **`checks.nixos-module` / `checks.home-manager-module`**: the example
   configurations set *every* option to a non-null value. The real binaries'
   `check-config` / `config check` must accept the generated TOML. This shows
   that every value the module can produce has the type Rust expects.

3. **Build-time validation for users**: `services.stickpile.validateConfig`
   and `programs.stickpile.validateConfig` (both on by default unless
   cross-compiling) run the same loader on the user's own generated file while
   building. An invalid configuration fails `nixos-rebuild` or
   `home-manager switch` instead of the service.

When a config field is added in Rust, add an option with the same name,
type, default and doc comment to `nix/modules/nixos.nix` (server) or
`nix/modules/home-manager.nix` (client). Also set it in the matching
`nix/examples/*.nix` (so that layer 2 sees a non-null value) and, for the
server, in `crates/stickpile-server/config.example.toml`.

## Home-manager

```nix
{
  inputs.stickpile.url = "github:Dolfost/stickpile";

  # in your home-manager configuration
  imports = [ inputs.stickpile.homeManagerModules.default ];

  programs.stickpile = {
    enable = true;          # CLI + ~/.config/stickpile/config.toml
    desktop.enable = true;  # stickpile-desktop + systemd user service
    settings = {
      server_url = "https://stickpile.example.ts.net";
      prefer_stickers = true;
      ui.columns = 8;
      focus.rules = [
        { contains = [ "ayugram" "telegram" ]; target = "telegram"; }
        { contains = [ "vesktop" "discord" ]; target = "discord"; }
      ];
      clipboard.targets.telegram.still = [ "text/uri-list" ];
    };
  };
}
```

- `settings` mirrors `ClientConfig` (`crates/stickpile-core/src/config.rs`)
  one-to-one. Options left at `null` (`token_file`, `cache_dir`) are omitted
  from the file, so the client uses its XDG defaults
  (`$XDG_STATE_HOME/stickpile/token`, `$XDG_CACHE_HOME/stickpile`). Setting
  `focus.rules` replaces the whole default list. Each entry under
  `clipboard.targets.<target>` falls back to the built-in offers for the lists
  it does not set (not to `clipboard.default`). This is the Rust behaviour.
- The token stays out of Nix: run `stickpile login` once and it writes
  `token_file`.
- `desktop.enable` adds `systemd.user.services.stickpile-desktop`, which runs
  `stickpile-desktop --hidden`. It is `WantedBy`, `After` and `PartOf`
  `graphical-session.target`, and it restarts when the config file changes.
  The process stays resident because, on Wayland, the clipboard content
  belongs to it after a copy. To toggle the popup, bind a compositor shortcut
  to `stickpile-desktop` (e.g. Hyprland: `bind = SUPER, period, exec,
  stickpile-desktop`).

## Models

`nix/models.nix` (kept in sync with the AI code) describes the CLIP files as
fixed-output derivations, in the layout of `docs/ai.md`:

```nix
services.stickpile.settings.ai = {
  enabled = true;
  model_dir = inputs.stickpile.packages.${pkgs.system}.models;  # or models-english / models-multilingual
  text_encoder = "multilingual";
};
```

These are store paths, so they are read-only, shared, and garbage-collected
together with the system generation. The container and non-Nix installs use
`stickpile-server fetch-models` or `scripts/fetch-models.sh` instead (see
docs/ai.md).
