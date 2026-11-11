#!/usr/bin/env bash
# Run stickpile on this machine from the checkout: no NixOS module, no podman.
# Builds the packages with Nix, keeps all server state in one directory, and
# runs the server and the resident desktop popup as transient systemd user
# units (gone after logout or reboot; run `up` again).
#
#   scripts/run-local.sh up      build, write config + admin password if missing,
#                                start server and popup, log the CLI in
#   scripts/run-local.sh down    stop both units (data is kept)
#   scripts/run-local.sh status  unit state and server health
#   scripts/run-local.sh logs    follow the server journal
#
# State: $STICKPILE_LOCAL_DIR (default ~/.local/share/stickpile-local)
#   config.toml              server config, written once, edit freely
#   secrets/admin-password   password of the initial admin ($USER)
#   stickpile.db, media/ ... everything the server stores
# Binaries: <repo>/target/local/stickpile-{server,cli,desktop}/bin
# Web UI: http://localhost:8080 (plain HTTP on loopback only)
#
# For a permanent setup use the NixOS / home-manager modules or the quadlet
# (docs/deployment.md) instead.
set -euo pipefail

repo=$(cd "$(dirname "$0")/.." && pwd)
dir=${STICKPILE_LOCAL_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/stickpile-local}
out=$repo/target/local
listen=127.0.0.1:8080
url=http://localhost:8080
server_unit=stickpile-local-server
desktop_unit=stickpile-local-desktop
# A login shell on this host may export LD_LIBRARY_PATH entries built against
# a newer glibc than the pinned nixpkgs; never pass it on.
unset LD_LIBRARY_PATH

server=$out/stickpile-server/bin/stickpile-server
cli=$out/stickpile-cli/bin/stickpile
desktop=$out/stickpile-desktop/bin/stickpile-desktop

build() {
  for p in stickpile-server stickpile-cli stickpile-desktop; do
    echo "building $p"
    nix build "$repo#$p" -o "$out/$p"
  done
}

write_config() {
  mkdir -p "$dir/secrets"
  chmod 700 "$dir" "$dir/secrets"
  if [ ! -s "$dir/secrets/admin-password" ]; then
    (umask 077 && head -c 18 /dev/urandom | base64 > "$dir/secrets/admin-password")
    echo "generated $dir/secrets/admin-password"
  fi
  [ -e "$dir/config.toml" ] && return
  local ai=false
  [ -s "$repo/models/clip-vit-b32/vision.onnx" ] && ai=true
  cat > "$dir/config.toml" <<EOF
# Local stickpile server, written by scripts/run-local.sh. Every option:
# docs/configuration.md. Restart after editing: scripts/run-local.sh down && up

[server]
listen = "$listen"
public_url = "$url"
data_dir = "$dir"
log = "info"
# Plain HTTP on loopback; never disable this behind a real proxy.
secure_cookies = false

[auth]
registration = "closed"
initial_admin_username = "$USER"
initial_admin_password_file = "$dir/secrets/admin-password"

[ai]
# Models from scripts/fetch-models.sh --all in the checkout.
enabled = $ai
model_dir = "$repo/models"
text_encoder = "multilingual"
threads = 2

[git]
enabled = true

# [telegram]
# enabled = true
# bot_token_file = "$dir/secrets/telegram-bot-token"
EOF
  echo "wrote $dir/config.toml"
}

wait_healthy() {
  for _ in $(seq 1 60); do
    if "$server" --config "$dir/config.toml" healthcheck >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
  done
  echo "server did not become healthy; see: journalctl --user -u $server_unit" >&2
  return 1
}

up() {
  build
  write_config
  "$server" --config "$dir/config.toml" check-config

  if ! systemctl --user is-active --quiet "$server_unit"; then
    systemctl --user reset-failed "$server_unit" 2>/dev/null || true
    systemd-run --user --unit="$server_unit" --description="stickpile server (local)" \
      -p Restart=on-failure -p RestartSec=2 \
      "$server" --config "$dir/config.toml" serve
  fi
  wait_healthy
  echo "server up: $url"

  if ! "$cli" whoami >/dev/null 2>&1; then
    "$cli" --server "$url" login --username "$USER" \
      --password-file "$dir/secrets/admin-password" --token-name "stickpile-cli@$(hostname)"
  fi

  if systemctl --user show-environment | grep -qE '^(WAYLAND_DISPLAY|DISPLAY)='; then
    if ! systemctl --user is-active --quiet "$desktop_unit"; then
      systemctl --user reset-failed "$desktop_unit" 2>/dev/null || true
      systemd-run --user --unit="$desktop_unit" --description="stickpile popup (local)" \
        -p Restart=on-failure -p RestartSec=2 \
        "$desktop" --hidden
    fi
    echo "popup resident; toggle it with: $desktop"
  else
    echo "no graphical session in the systemd user manager; popup not started"
  fi
}

down() {
  systemctl --user stop "$desktop_unit" "$server_unit" 2>/dev/null || true
  echo "stopped (state kept in $dir)"
}

status() {
  systemctl --user --no-pager status "$server_unit" "$desktop_unit" 2>/dev/null | grep -E '●|Active:' || true
  "$server" --config "$dir/config.toml" healthcheck || true
}

case "${1:-}" in
  up) up ;;
  down) down ;;
  status) status ;;
  logs) exec journalctl --user -fu "$server_unit" ;;
  *) sed -n '2,21p' "$0" | sed 's/^# \{0,1\}//'; exit 2 ;;
esac
