# Contributing

Bug reports, fixes and features are welcome. For anything bigger than a small
fix, open an issue first so the approach can be agreed on before you write the
code. Security problems go through [SECURITY.md](SECURITY.md), not issues.

## Setup

You need [Nix](https://nixos.org/download/) with flakes enabled. The dev shell
has everything else: the Rust toolchain, GTK4/libadwaita, ffmpeg,
onnxruntime, libgit2, cargo-nextest and cargo-llvm-cov
([docs/nix.md](docs/nix.md#dev-shell)).

```sh
nix develop
cargo build --workspace
```

The AI models are optional. Tests that need them are skipped unless
`STICKPILE_TEST_MODEL_DIR` is set ([docs/ai.md](docs/ai.md),
`scripts/fetch-models.sh`). To try the server and the desktop popup from the
checkout, run `scripts/run-local.sh up`.

## Before you open a pull request

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
nix flake check -L    # what CI runs
```

`nix flake check` also builds the packages and the container image, checks
the NixOS and home-manager modules, and runs a NixOS VM test on x86_64 (needs
KVM). Its checks are listed in [docs/nix.md](docs/nix.md#checks).

- Tests must not need a display or the network.
- A changed config default must change in the NixOS/home-manager modules too.
  The `config-drift` check compares them
  ([docs/nix.md](docs/nix.md#the-drift-guard)).
- Update the page in [docs/](docs/) that describes what you changed.
- Keep commits focused, with an imperative summary line ("Add …", "Fix …").

## Licence

stickpile is licensed under the [GNU AGPL v3 or later](LICENSE). By
contributing, you agree that your contributions are licensed under the same
terms (AGPL-3.0-or-later).
