# Server configuration

`stickpile-server` reads one TOML file: `--config <path>`, the
`STICKPILE_CONFIG` environment variable, or `/etc/stickpile/config.toml`.
Every key is optional; unknown keys are an error, so typos do not pass
silently. The NixOS module (`services.stickpile.settings`, [nix.md](nix.md))
mirrors every key one-to-one and checks its output with `check-config`.

An annotated example is in
[`crates/stickpile-server/config.example.toml`](../crates/stickpile-server/config.example.toml).

## Secrets are files

Secrets are never written into the configuration. Keys ending in `_file` hold
a path; the server reads the file at startup and trims one trailing newline.
Use systemd credentials, agenix/sops-nix or podman secrets to provide them.

## Command line

| Command | Does |
|---|---|
| `stickpile-server [serve]` | Run the server until SIGINT/SIGTERM (graceful shutdown). |
| `stickpile-server check-config` | Parse and validate the file, then exit. Invalid configuration fails. Problems that only matter at runtime are printed as `warning:` lines on stderr without failing: missing CLIP model files when `ai.enabled`, an unreadable or empty Telegram token or git token file, a missing `git.ssh_key_file`. |
| `stickpile-server create-user <username> [--admin] [--password-file F] [--tailscale-login L]` | Create a user directly in the database (at least one of `--password-file` / `--tailscale-login`). |
| `stickpile-server reset-password <username> --password-file F [--revoke-tokens] [--reset-totp]` | Set a new password for a user who forgot theirs; signs them out of every web session (see [deployment](deployment.md#forgotten-passwords)). |
| `stickpile-server backup <dir>` | Full backup: every set as a set directory plus a database snapshot; safe while the server runs. See [backup.md](backup.md). |
| `stickpile-server restore <dir>` | Rebuild a fresh `data_dir` from such a backup. |
| `stickpile-server healthcheck [--url URL]` | `GET http://<server.listen>/healthz` (a wildcard address is checked on loopback) or `URL` (plain `http://` only); exit 0 on a 2xx answer within 3 s, else 1. For container health checks: no shell or curl needed. |

## `[server]`

| Key | Type | Default | Meaning |
|---|---|---|---|
| `listen` | socket address | `"127.0.0.1:8080"` | TCP listener. Tailnet identity headers are never trusted here (see `[trusted_proxy]`). |
| `public_url` | string | `"http://localhost:8080"` | Absolute external base URL (`http://` or `https://`). Used in invitation and webhook links and for the same-origin check; `https://` enables HSTS. |
| `data_dir` | absolute path | `"/var/lib/stickpile"` | All state, see [Data directory](#data-directory). Created if missing. |
| `log` | string | `"info"` | `tracing` filter, e.g. `stickpile_server=debug,info`. `RUST_LOG` overrides it. |
| `secure_cookies` | bool | `true` | Mark the session cookie `Secure`. Only disable for plain-http local development. |

## `[trusted_proxy]`

How `tailscale serve` reaches the server. See
[security.md](security.md#tailscale-trust-model).

| Key | Type | Default | Meaning |
|---|---|---|---|
| `socket` | path | unset | Extra unix-socket listener for `tailscale serve ... unix:<socket>`. Identity headers are trusted on it. A stale socket file is replaced at startup. |
| `socket_mode` | octal string | `"0660"` | Permission bits of `socket`. |
| `secret_file` | path | unset | For a TCP reverse proxy on loopback: requests from a loopback address carrying `secret_header: <file contents>` are trusted. Startup fails if the file cannot be read. |
| `secret_header` | string | `"X-Stickpile-Proxy-Secret"` | Header name for `secret_file`. |

## `[auth]`

| Key | Type | Default | Meaning |
|---|---|---|---|
| `registration` | `"closed"` / `"invite"` / `"open"` | `"closed"` | Self-service registration: only admins (and tailnet auto-create) create accounts / anyone with an invitation link / anyone who can reach the server. |
| `tailscale_autocreate` | bool | `true` | Create a user the first time an unknown tailnet login arrives through the trusted proxy. |
| `tailscale_admins` | list of strings | `[]` | Tailnet logins (e.g. `alice@github`) that get the admin role when created or on their next request. |
| `session_ttl_days` | integer > 0 | `30` | Web session lifetime; sliding with use. |
| `min_password_length` | integer 4..=128 | `8` | Shortest password accepted when one is set (registration, settings, admin, `create-user`, the initial admin). Existing passwords keep working. Below 8, rely on the login rate limits and keep the server off the public internet. |
| `totp_issuer` | string | `"stickpile"` | Issuer shown in authenticator apps. |
| `initial_admin_username` | string | unset | If the database has no users at startup, create this admin ... |
| `initial_admin_password_file` | path | unset | ... with the password in this file. Both keys or neither. |

## `[limits]`

| Key | Type | Default | Meaning |
|---|---|---|---|
| `max_upload_mb` | integer > 0 | `50` | Per uploaded file (MiB). |
| `max_request_mb` | integer >= `max_upload_mb` | `500` | Per upload or import request, including tar archives (MiB). |
| `max_pixels` | integer | `40000000` | Largest decoded frame (width x height). |
| `max_animation_frames` | integer | `3000` | Frames per animation. |
| `max_video_seconds` | integer | `120` | Video length. |
| `login_per_minute` | integer > 0 | `10` | Password logins, token requests, registrations, password and TOTP changes, per client IP and per username. |
| `upload_per_minute` | integer > 0 | `120` | Upload and import requests per user. |
| `ai_suggest_per_minute` | integer > 0 | `120` | AI suggestion requests (`POST /api/v1/ai/suggest`) per user. The upload tray asks once per staged file. |

## `[media]`

See [media.md](media.md) and [quality.md](quality.md).

| Key | Type | Default | Meaning |
|---|---|---|---|
| `ffmpeg` | path | `"ffmpeg"` | ffmpeg binary (videos and animated variants; fixed arguments only). If it is missing the server still starts with a warning; video uploads and animated variants then fail. |
| `ffprobe` | path | `"ffprobe"` | ffprobe binary. |
| `ffmpeg_timeout_secs` | integer | `120` | Kill ffmpeg/ffprobe after this long. |
| `thumb_px` | integer | `256` | Longest side of grid thumbnails (also the frame AI enrichment embeds). |
| `discord_gif_max_mb` | integer | `10` | Size budget of the `discord_gif` variant. |
| `workers` | integer > 0 | `2` | Concurrent background jobs (variants, enrichment, git, Telegram). |
| `default_recommended_px` | integer | `512` | Recommended longest side for sets that set none (quality score). |
| `near_duplicate_distance` | integer 0-32 | `8` | Largest pHash Hamming distance reported as a near-duplicate and used by "find a better version". |

## `[ai]`

See [ai.md](ai.md). The model loads in the background after startup: the
server serves requests meanwhile and search reports `semantic: false` until it
is ready. If loading fails, the error is logged and the server runs without
AI. When the model becomes ready, media without an embedding from it are queued
for enrichment.

| Key | Type | Default | Meaning |
|---|---|---|---|
| `enabled` | bool | `false` | Load the CLIP model. |
| `model_dir` | path | `"/var/lib/stickpile/models"` | ONNX models and tokenizers (layout in [ai.md](ai.md)). |
| `text_encoder` | `"clip"` / `"multilingual"` | `"clip"` | English CLIP text tower, or the multilingual encoder aligned to the same image space. Switching needs no image re-embedding. |
| `onnxruntime_library` | path | unset | `libonnxruntime.so`; default `ORT_DYLIB_PATH` or the system library. |
| `threads` | integer | `2` | Inference threads. |
| `vocabulary_file` | path | unset | TOML replacing (parts of) the built-in zero-shot vocabulary. Also used for vibe-synonym query expansion, even without a model. Startup fails if it is invalid. |
| `max_keywords` | integer | `5` | AI keywords suggested per item. |
| `max_emojis` | integer | `3` | AI emojis suggested per item. |

## `[telegram]`

Used by Telegram import/export (integrations, [telegram.md](telegram.md)).

| Key | Type | Default | Meaning |
|---|---|---|---|
| `enabled` | bool | `false` | Enable the Telegram endpoints. Requires `bot_token_file`. |
| `bot_token_file` | path | unset | File with the bot token. |
| `api_url` | string | `"https://api.telegram.org"` | Bot API base URL (local Bot API server or tests). |
| `default_owner_user_id` | integer | unset | Telegram user id that owns packs created on export, unless the exporting user set their own. |

## `[git]`

Used by git packs (integrations, [git-packs.md](git-packs.md)).

| Key | Type | Default | Meaning |
|---|---|---|---|
| `enabled` | bool | `true` | Allow binding sets to git repositories. |
| `pull_interval_minutes` | integer | `60` | Scheduled re-pull interval (`0` = only webhooks and manual pulls). |
| `allowed_schemes` | list of `https`, `http`, `ssh`, `file` | `["https"]` | URL schemes users may bind (`file` for tests only). |
| `ssh_key_file` | path | unset | Key for `ssh` remotes and pushing. |
| `https_token_file` | path | unset | HTTPS token used as the password when pushing (`push` mode). |
| `https_username` | string | `"x-access-token"` | HTTPS username for pushing. |
| `author_name` | string | `"stickpile"` | Commit author for pushed edits. |
| `author_email` | string | `"stickpile@localhost"` | Commit author email. |
| `max_repo_mb` | integer | `1024` | Refuse repositories whose checkout is larger. |

## Data directory

Everything the server writes lives under `server.data_dir`:

| Path | Contents |
|---|---|
| `stickpile.db` (+ `-wal`, `-shm`) | SQLite database (WAL mode); migrations run at startup. |
| `media/<sha[0..2]>/<sha>/<variant>.<ext>` | Stored originals and variants, keyed by the sha256 of the uploaded bytes and shared by every item with that content. Unreferenced media is removed by a periodic cleanup. |
| `tmp/` | Upload spool and ffmpeg scratch files; leftovers older than a day are removed. |
| `gitpacks/` | Git pack checkouts (integrations). |

Back up the whole directory; the database is the source of truth.

## Client configuration

The `stickpile` CLI and the desktop popup read one file:
`$XDG_CONFIG_HOME/stickpile/config.toml`, or `--config FILE` /
`STICKPILE_CONFIG`. Every key is listed in
[cli.md](cli.md#configuration-configtoml), and `stickpile config check`
validates the file. The home-manager module
(`programs.stickpile.settings`, [nix.md](nix.md#home-manager)) can write it
for you.

### Popup tabs and modes

```toml
[ui]
modal = true          # vim-style Insert/Normal modes in the popup; false keeps the plain keys
start_tab = "last"    # last | search | recent | most_used | packs | favorites | me | settings
start_mode = "normal" # normal | insert
```

`last` reopens the tab the popup was closed on, also after a restart (it
is kept in the cache directory). `start_mode = "insert"` puts the cursor
in the text field on Search and Packs; the other tabs open in Normal
either way. Any other `start_tab` or `start_mode` value is an error.

### Popup tiles

```toml
[ui]
mark_favorites = true # a heart on the items you liked
show_emojis = true    # an item's emojis on its tile
show_tags = true      # an item's tags on its tile
```

`zf`, `ze` and `zt` in Normal mode turn them on and off and save the
change like the Settings tab does.

### Copy-time image adjustments

```toml
[copy]
square = "off"        # off | transparent | white: pad non-square stills and stickers to a square
background = "keep"   # keep | white: fill the transparent pixels of stills copied as images
```

- `square` centres non-square stills and Telegram sticker renditions on a
  square of their longest side. `transparent` or `white` is the colour of
  the added margins; the image's own pixels are not changed. Nothing is
  scaled up or cropped.
- `background = "white"` composites stills copied as images onto white.
  Sticker renditions keep their transparency.
- Both apply in the popup and in `stickpile copy`, to the image data on the
  clipboard and to the file that the file reference (`text/uri-list`, drag
  and drop) points at, so every paste target gets the same pixels. An
  adjusted still is a PNG. An adjusted sticker is still a valid Telegram
  sticker: WebP, longest side 512, under the size limit. Animations and
  videos are never changed, and nothing is rewritten when there is nothing
  to do (a square, opaque image).
- Adjusted files are cached next to the downloaded variants, one directory
  per option combination, so changing an option never serves an old file:
  `clip/<media>.<variant>.sq-white.bg-white/<name>.png`.

### Settings saved from the popup

The popup's Settings tab edits these keys:

| Group | Keys |
|---|---|
| Popup | `ui.language`, `ui.start_tab`, `ui.start_mode`, `ui.modal`, `ui.columns`, `ui.thumb_px`, `ui.mark_favorites`, `ui.show_emojis`, `ui.show_tags`, `ui.recent_count`, `ui.max_results`, `ui.hide_on_copy`, `ui.start_hidden` |
| Copying | `default_target`, `focus.detect`, `prefer_stickers`, `copy.square`, `copy.background` |
| Server | `server_url`, `timeout_secs`, `cache_max_mb` |

Clipboard offer lists, focus rules, `token_file` and `cache_dir` are set in
`config.toml` only.

Where the changes go depends on whether `config.toml` can be written:

| `config.toml` | The Settings tab writes | Locked in the popup |
|---|---|---|
| writable, or missing | `config.toml` itself, keeping its comments, key order and other keys | nothing |
| read-only: it resolves into `/nix/store` (home-manager links it there), it is a dangling symlink, or you have no write permission | `settings.toml` in the same directory | every key `config.toml` sets, shown as "set by home-manager" |

With a read-only `config.toml` the client layers the two files:
`config.toml` wins over `settings.toml`, which wins over the defaults.

- `config.toml` keeps its strict errors: unknown keys and invalid values
  fail as always.
- In `settings.toml`, unknown keys, keys the Settings tab does not manage
  and invalid values are skipped with a warning (logged by the popup,
  printed by the CLI), so a stale value never stops the client from
  starting.
- A `settings.toml` next to a writable `config.toml` is ignored, with a
  warning. Move its values into `config.toml` or delete it.
- Both clients load the same way and see the same configuration.
  `stickpile --json config check` prints the effective one, `settings.toml`
  included.
