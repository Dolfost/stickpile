# Deployment

stickpile is one process with one state directory (SQLite database, media,
git clones, models). This page covers three ways to run it:

- [NixOS](#nixos): the `services.stickpile` module.
- [Podman quadlet](#podman-quadlet-rootless): the container image as a
  rootless quadlet, as on the aorus host.
- [Release binaries](#release-binaries).

The server has two listeners:

| Listener | Config | Trusts `Tailscale-User-Login`? |
|---|---|---|
| TCP (`server.listen`, default `127.0.0.1:8080`) | always on | Only with `trusted_proxy.secret_file`, and only from a loopback peer that sends the secret header. |
| Unix socket (`trusted_proxy.socket`) | optional | Yes. Only root, the service user and its group can connect. |

On a tailnet, put `tailscale serve` in front of the **unix socket**. Tailnet
users are then logged in by their tailnet identity, with no password. Local
accounts and API tokens work on both listeners.

> **Open question** ([plan.md](plan.md#open-questions-and-risks), question 2):
> nobody has yet checked whether `tailscale serve --service=svc:<name>`
> (Tailscale Services) forwards the identity headers the way a node-level
> `tailscale serve` does. Verify this after the first deployment (see
> [Check the tailnet login](#7-check-the-tailnet-login)). Without the headers,
> users log in with local accounts and tokens, and nothing else changes.

## NixOS

```nix
# flake.nix
{
  inputs.stickpile.url = "github:Dolfost/stickpile";
  # inputs.stickpile.inputs.nixpkgs.follows = "nixpkgs";  # optional; the flake is tested on nixos-26.05

  outputs = { nixpkgs, stickpile, ... }: {
    nixosConfigurations.host = nixpkgs.lib.nixosSystem {
      modules = [ stickpile.nixosModules.default ./stickpile.nix ];
    };
  };
}
```

```nix
# stickpile.nix: a complete example
{ config, pkgs, inputs, ... }:
{
  services.stickpile = {
    enable = true;
    # openFirewall = false;  # default; tailscale serve needs no open port
    settings = {
      server = {
        listen = "127.0.0.1:8080";
        public_url = "https://stickpile.example.ts.net";
        # data_dir = "/var/lib/stickpile";  # default (StateDirectory=stickpile)
      };
      # Target of `tailscale serve`. /run/stickpile is the unit's RuntimeDirectory.
      trusted_proxy.socket = "/run/stickpile/tailscale.sock";
      auth = {
        registration = "invite";
        tailscale_admins = [ "you@github" ];
        # Optional local admin, for logins from outside the tailnet.
        initial_admin_username = "admin";
        initial_admin_password_file = config.age.secrets.stickpile-admin.path;  # agenix, sops-nix, or any root-only file
      };
      ai = {
        enabled = true;
        model_dir = inputs.stickpile.packages.${pkgs.system}.models;
        text_encoder = "multilingual";
      };
      telegram = {
        enabled = true;
        bot_token_file = config.age.secrets.stickpile-telegram.path;
      };
      git.allowed_schemes = [ "https" "ssh" ];
      git.ssh_key_file = "/var/lib/secrets/stickpile-deploy-key";
    };
  };

  # tailscale serve -> the trusted socket. Same shape as the oneshots in
  # deploy/aorus/stickpile.nix.
  systemd.services.tailscale-serve-stickpile = {
    description = "tailscale serve: svc:stickpile -> stickpile socket";
    after = [ "tailscaled.service" "stickpile.service" ];
    wants = [ "tailscaled.service" ];
    wantedBy = [ "multi-user.target" ];
    serviceConfig = {
      Type = "oneshot";
      RemainAfterExit = true;
      Restart = "on-failure";
      RestartSec = 15;
      ExecStart = "${config.services.tailscale.package}/bin/tailscale serve --yes "
        + "--service=svc:stickpile --https=443 unix:/run/stickpile/tailscale.sock";
    };
    unitConfig.StartLimitIntervalSec = 0;
  };
}
```

What the module does:

- **`settings`** mirrors the server's `Config` one-to-one: every key of
  [`config.example.toml`](../crates/stickpile-server/config.example.toml) is a
  typed, documented option with the same name and default
  (`nixos-option services.stickpile.settings` lists them). The module renders
  it to TOML and checks the result with `stickpile-server check-config` at
  build time (`validateConfig`). See [nix.md](nix.md#the-drift-guard) for why
  the two cannot drift.
- **Secrets are paths only.** There are five: `trusted_proxy.secret_file`,
  `auth.initial_admin_password_file`, `telegram.bot_token_file`,
  `git.ssh_key_file` and `git.https_token_file`. Each must be an absolute path
  outside the Nix store (the option type rejects store paths). systemd reads
  the file as root (`LoadCredential=`), and the server sees it as
  `/run/credentials/stickpile.service/<name>`. The file can therefore stay
  `root:root 0400`, and its contents never enter the store.
- **User and directories**: the server runs as the system user `stickpile`
  (`user` and `group` options). State goes to `StateDirectory=stickpile`
  (`/var/lib/stickpile`, mode 0750), and the runtime directory is
  `/run/stickpile`. If `data_dir` is outside `/var/lib`, a tmpfiles rule
  creates it and it is added to `ReadWritePaths=`.
- **Hardening**: `ProtectSystem=strict`, `ProtectHome`, `PrivateTmp`,
  `PrivateDevices`, `NoNewPrivileges`, an empty capability set,
  `RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6`,
  `SystemCallFilter=@system-service ~@privileged`, `MemoryDenyWriteExecute`,
  `ProtectProc=invisible`, `RestrictNamespaces`, `LockPersonality` and the
  other `Protect*` settings. `MemoryDenyWriteExecute` was tested with real
  CLIP inference through ONNX Runtime and with ffmpeg (x264, VP9, GIF palette),
  and both work under it. `ProcSubset=pid` is deliberately not set, because
  ONNX Runtime reads `/proc/cpuinfo`.
- `openFirewall` (default `false`) opens the TCP port of `server.listen`.
- `/etc/stickpile/config.toml` points at the generated file, so admin
  subcommands work without `--config`:
  `sudo -u stickpile stickpile-server create-user alice --admin --password-file /root/alice.pw`.

## Podman quadlet (rootless)

Files: [`deploy/quadlet/`](../deploy/quadlet/). The quadlet follows the aorus
conventions:

- nothing host-specific is tracked: every host path is `$STICKPILE_DIR`, set
  by a systemd drop-in;
- SQLite keeps it to one container, so there is no `.pod`;
- it is exposed with `tailscale serve`.

### 1. State directory

```sh
export STICKPILE_DIR=/storage/2.5/stickpile        # anywhere you like
mkdir -p "$STICKPILE_DIR/secrets"
chmod 700 "$STICKPILE_DIR/secrets"
```

podman does not create missing bind sources, so `secrets/` must exist even
when it is empty.

Layout once the server has run: `stickpile.db`, `media/`, `gitpacks/`,
`models/`, `tmp/`, `tailscale.sock`, plus your `config.toml` and `secrets/`.
All of it is owned by **your** user on the host (see `UserNS=keep-id` below).

### 2. Configuration

```sh
cp deploy/quadlet/config.toml "$STICKPILE_DIR/config.toml"
$EDITOR "$STICKPILE_DIR/config.toml"    # public_url, tailscale_admins, ...
```

Paths in it are **container** paths:

- the state directory is `/var/lib/stickpile`;
- secrets are `/run/secrets/stickpile/<file>`;
- the trusted socket is `/var/lib/stickpile/tailscale.sock`.

Leave `server.listen = "0.0.0.0:8080"`. That address is inside the
container; the quadlet publishes it only on the host's loopback. To check the
file:

```sh
podman run --rm -v "$STICKPILE_DIR/config.toml:/etc/stickpile/config.toml:ro" \
  ghcr.io/dolfost/stickpile:latest check-config
```

### 3. Secrets

Each secret is one file in `$STICKPILE_DIR/secrets/` (mounted read-only),
referenced by a `*_file` key:

```sh
openssl rand -base64 24 > "$STICKPILE_DIR/secrets/admin-password"      # auth.initial_admin_password_file
printf %s "$BOT_TOKEN"  > "$STICKPILE_DIR/secrets/telegram-bot-token"  # telegram.bot_token_file
chmod 600 "$STICKPILE_DIR"/secrets/*
```

### 4. Quadlet and drop-in

```sh
cp deploy/quadlet/stickpile.container ~/.config/containers/systemd/
mkdir -p ~/.config/systemd/user/stickpile.service.d
cp deploy/quadlet/stickpile.service.d/env.conf.example \
   ~/.config/systemd/user/stickpile.service.d/env.conf
$EDITOR ~/.config/systemd/user/stickpile.service.d/env.conf   # STICKPILE_DIR=...
systemctl --user daemon-reload
systemctl --user start stickpile
loginctl enable-linger "$USER"   # start at boot without a login
```

On aorus, the drop-in and the tailscale unit come from Nix: copy
[`deploy/aorus/stickpile.nix`](../deploy/aorus/stickpile.nix) to
`nix/hosts/aorus/services/` and import it there. The file explains its two
options for the serve unit, and `deploy/aorus/mkServe-unix.diff` is the
`unix` escape hatch for `mkServe`, which is the recommended option.

### 5. Container hardening

What `stickpile.container` sets, and why:

| Setting | Effect |
|---|---|
| image `User=65532:65532` | The server never runs as root. The image has no shell or package manager, only the server's closure (ffmpeg-headless, onnxruntime), CA certificates and tzdata. |
| `UserNS=keep-id:uid=65532,gid=65532` | Your host uid becomes 65532 inside the container. Files in `$STICKPILE_DIR` (and the socket) are owned by you on the host, so plain backups work without `podman unshare`. Without it, they would belong to subuid 165531. |
| `ReadOnly=true` | Read-only root filesystem. Every write goes to the volume, including `TMPDIR=/var/lib/stickpile/tmp`, which shares a filesystem with `media/` so finished uploads are renamed, not copied. |
| `NoNewPrivileges=true`, `DropCapability=all` | No setuid escalation and no capabilities. |
| config and secrets `:ro` | The server cannot change its own configuration or secrets. |
| `PublishPort=127.0.0.1:8080:8080` | Plain HTTP on the host's loopback only. With rootless podman every local user can reach it, which is why identity headers are never trusted there. |

### 6. tailscale serve

tailscaled runs as root, so it can connect to the socket inside your
directory:

```sh
sudo tailscale serve --yes --service=svc:stickpile --https=443 unix:$STICKPILE_DIR/tailscale.sock
```

On aorus, a oneshot unit runs this (`deploy/aorus/stickpile.nix`). As with
the other services, `svc:stickpile` has to be defined in the admin console.
The server recreates the socket on every start, and `tailscale serve` only
stores the path, so restarts need no re-run.

If identity headers turn out not to reach the server (next step), the
simpler fallback is the TCP port: `dotfiles.serve.stickpile.port = 8080;`.
Users then log in with local accounts.

### 7. Check the tailnet login

Open `https://stickpile.<tailnet>.ts.net`. If Services forward the headers,
you are logged in as your tailnet login (and are an admin if it is listed in
`tailscale_admins`); the login appears in the admin panel's audit log. If you
get the password login page instead, the headers did not arrive. In that
case, record the result in docs/plan.md (open question 2) and use local
accounts.

### 8. Updates

`AutoUpdate=registry` makes `podman auto-update` pull `:latest` and restart
the container when the image changed:

```sh
systemctl --user enable --now podman-auto-update.timer
```

Database migrations run on start. To pin a version instead, use
`Image=ghcr.io/dolfost/stickpile:<version>` and drop `AutoUpdate`.

## Git remotes: certificates and host keys

Git packs use libgit2 and need no `git` binary.

- **HTTPS**: libgit2 verifies certificates with OpenSSL against the system
  CA bundle. On NixOS that is `/etc/ssl/certs`, readable under
  `ProtectSystem=strict`. In the image, `SSL_CERT_FILE` points at the bundled
  cacert. Telegram and other HTTP clients use rustls with built-in webpki
  roots.
- **SSH** (`git.allowed_schemes = [ "ssh" ]`, `git.ssh_key_file`): libgit2
  checks the server's host key against the service user's
  `~/.ssh/known_hosts`, and an unknown host fails the clone. Add the hosts
  before binding a set:
  - NixOS: the `stickpile` user's home is `data_dir`, so run
    `sudo -u stickpile sh -c 'mkdir -p ~/.ssh && ssh-keyscan github.com >> ~/.ssh/known_hosts'`
    (this writes `/var/lib/stickpile/.ssh/known_hosts`).
  - Container: `HOME=/var/lib/stickpile`, so on the host run
    `mkdir -p $STICKPILE_DIR/.ssh && ssh-keyscan github.com >> $STICKPILE_DIR/.ssh/known_hosts`.
  - Check the fingerprints `ssh-keyscan` prints against the ones the forge
    publishes.

## First admin

Pick one:

- **Tailnet**: list your login in `auth.tailscale_admins`. Your first visit
  through `tailscale serve` creates the account with the admin role
  (`tailscale_autocreate`, default on).
- **Bootstrap account**: set `auth.initial_admin_username` and
  `auth.initial_admin_password_file`. On first start, while the database has
  no users, the server creates this admin. Later starts ignore it; once you
  have logged in, remove both keys (and then the secret file).
- **CLI**, at any time:

  ```sh
  # NixOS
  sudo -u stickpile stickpile-server create-user alice --admin --password-file /path/to/pw
  # container: the secret must be readable inside it
  podman exec stickpile stickpile-server create-user alice --admin \
    --password-file /run/secrets/stickpile/alice-password
  # or bind a tailnet login instead of a password
  podman exec stickpile stickpile-server create-user alice --admin --tailscale-login alice@github
  ```

## Backups and upgrades

Everything is in the state directory: `/var/lib/stickpile` on NixOS,
`$STICKPILE_DIR` with the container.

- **Database**: `stickpile.db` is SQLite in WAL mode. Copying it while the
  server runs can give an inconsistent copy. Either stop the service for a
  file-level backup, or take an online snapshot with
  `sqlite3 stickpile.db ".backup /backup/stickpile.db"`.
- **Media**: `media/` is content-addressed and append-mostly, so incremental
  backups (restic, borg, rsync) are cheap. `models/` and `gitpacks/` can be
  re-downloaded.
- **`stickpile-server backup <dir>`** (recommended): every set as a set
  directory plus a consistent database snapshot, safe while the server runs;
  `stickpile-server restore <dir>` rebuilds a fresh data directory from it.
  See [backup.md](backup.md).
- **Upgrades**: NixOS: `nix flake update stickpile && nixos-rebuild switch`.
  Container: `podman auto-update` or a pinned tag. Migrations run at startup.
  Take a backup before upgrading across a minor version.

## Container image

`nix build .#container` produces `ghcr.io/dolfost/stickpile:<version>` as a
docker-archive (`podman load < result`). Releases push
`ghcr.io/dolfost/stickpile:<version>` and `:latest` for linux/amd64 and
linux/arm64.

- Entrypoint `stickpile-server`, default command `serve`. Any subcommand
  works as arguments (`check-config`, `create-user …`).
- `STICKPILE_CONFIG=/etc/stickpile/config.toml`. A default config is baked in
  (listens on `0.0.0.0:8080`, socket at `/var/lib/stickpile/tailscale.sock`,
  everything else at server defaults); mount your own over it.
- Volume `/var/lib/stickpile`, `TMPDIR` inside it, `HOME` set to it.
- Runs as `65532:65532`. A read-only root filesystem works.
- AI models are not in the image. Put them in `$STICKPILE_DIR/models` (see
  docs/ai.md for `stickpile-server fetch-models` and the layout) and set
  `ai.enabled = true`.

## Release binaries

Each `v*` tag publishes the following on GitHub Releases, with a `SHA256SUMS`
file:

- **Linux** (`x86_64`, `aarch64`): `stickpile-server`, `stickpile` and
  `stickpile-desktop`, built by Nix and rewritten to use the system loader and
  libraries. Each tarball's `README.txt` lists the shared libraries every
  binary needs and the minimum glibc. Runtime needs: ffmpeg on `PATH` for
  videos and animations; libonnxruntime (`ORT_DYLIB_PATH`) for AI; GTK ≥ 4.16
  and libadwaita ≥ 1.6 for the desktop client. On NixOS or with Nix, use the
  flake instead.
- **macOS** (`arm64`, `x86_64`): `stickpile` and `stickpile-server`, built
  by cargo. Install `ffmpeg` and `onnxruntime` from Homebrew to get the same
  features.
- **Windows**: the `stickpile` CLI, best effort.
- Android (APK and F-Droid metadata) comes after v1.
