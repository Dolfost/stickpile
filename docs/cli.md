# `stickpile` command-line client

`stickpile` talks to a stickpile server over its REST API. It handles login,
sets, import and export of set directories, search, copying to the clipboard,
usage statistics, git packs, Telegram packs and admin tasks. It shares its
configuration file, token and offline cache with the desktop client
(`stickpile-desktop`). Both are built on the `stickpile-core` crate.

- [Install](#install)
- [Quick start](#quick-start)
- [Configuration (`config.toml`)](#configuration-configtoml)
- [Logging in](#logging-in)
- [Global options](#global-options)
- [Commands](#commands)
- [Clipboard behaviour](#clipboard-behaviour)
- [Offline behaviour](#offline-behaviour)
- [Files](#files)

## Install

With Nix:

```sh
nix profile install github:Dolfost/stickpile#stickpile-cli
```

With home-manager, `programs.stickpile` installs the CLI and writes
`config.toml` from `programs.stickpile.settings` (the keys below, one to one).
At build time it runs `stickpile config check` on the generated file (see
[docs/nix.md](nix.md)).

From source (inside the flake dev shell):

```sh
nix develop -c cargo build --release -p stickpile-cli   # target/release/stickpile
```

The Wayland and X11 clipboard backends are compiled in by default.

## Quick start

```sh
stickpile login                         # passwordless on the tailnet, else asks for username/password
stickpile import ~/stickers/my-cats     # a set directory with set.toml + media files
stickpile search cat type:gif           # search your library and public sets
stickpile copy naahh --target discord   # best match onto the clipboard
stickpile export my-cats ./my-cats      # write the set back out as a directory
stickpile sync                          # fill the offline cache
```

## Configuration (`config.toml`)

Default location: `$XDG_CONFIG_HOME/stickpile/config.toml`
(`~/.config/stickpile/config.toml`). A missing file means all defaults.
Unknown keys are rejected, so typos fail loudly. `stickpile config check`
validates a file without contacting the server.

The desktop client reads the same file. The `ui` and `focus` sections only
affect the desktop popup.

```toml
# Base URL of the server (a path prefix is kept).
server_url = "http://localhost:8080"
# API token file written by `stickpile login` (mode 0600).
# Default: $XDG_STATE_HOME/stickpile/token (~/.local/state/stickpile/token)
# token_file = "/home/me/.local/state/stickpile/token"
# Offline cache directory. Default: $XDG_CACHE_HOME/stickpile (~/.cache/stickpile)
# cache_dir = "/home/me/.cache/stickpile"
# Thumbnails and copied files beyond this many MiB are evicted, least recently used first.
cache_max_mb = 512
# Target app when none is given (CLI) or the focused window is not recognised (desktop):
# telegram | discord | signal | whatsapp | slack | generic
default_target = "generic"
# For targets with real stickers (Telegram), copy the sticker rendition by default.
# In the desktop popup this is the starting value: Ctrl+S or the footer toggle
# change it until stickpile-desktop restarts.
prefer_stickers = false
# Request timeout before falling back to the offline cache.
timeout_secs = 5

[ui]                    # desktop popup only
columns = 6
thumb_px = 128          # >= 32
hide_on_copy = true
start_hidden = false
recent_count = 24       # items shown before typing, most recently copied first
max_results = 60

[focus]                 # desktop popup only
detect = true           # read the focused window (Hyprland, Sway, X11) to pick the target
# First match wins; substrings matched case-insensitively against window class/app id and title.
# Setting `rules` replaces the default list, which is:
[[focus.rules]]
contains = ["telegram", "ayugram", "materialgram", "64gram", "kotatogram"]
target = "telegram"
[[focus.rules]]
contains = ["discord", "vesktop", "webcord", "armcord", "legcord"]
target = "discord"
[[focus.rules]]
contains = ["signal"]
target = "signal"
[[focus.rules]]
contains = ["whatsapp", "zapzap", "wasistlos"]
target = "whatsapp"
[[focus.rules]]
contains = ["slack"]
target = "slack"

# MIME types put on the clipboard, in offer order, per media kind.
# Supported: image/png image/gif image/webp image/jpeg text/uri-list
#            x-special/gnome-copied-files text/plain
[clipboard.default]
still = ["image/png", "text/uri-list", "x-special/gnome-copied-files"]
animation = ["image/gif", "text/uri-list", "x-special/gnome-copied-files"]
video = ["text/uri-list", "x-special/gnome-copied-files"]
sticker = ["text/uri-list", "x-special/gnome-copied-files"]

# Per-target overrides replace [clipboard.default] for that target. Kinds left
# out of a target section take the built-in defaults shown above (not the
# values of your [clipboard.default]).
[clipboard.targets.telegram]
still = ["image/png", "text/uri-list"]
animation = ["text/uri-list"]
video = ["text/uri-list"]
sticker = ["text/uri-list"]
```

| key | default | meaning |
|---|---|---|
| `server_url` | `"http://localhost:8080"` | server base URL, `http` or `https` |
| `token_file` | `$XDG_STATE_HOME/stickpile/token` | API token file (0600) |
| `cache_dir` | `$XDG_CACHE_HOME/stickpile` | offline cache (SQLite + files) |
| `cache_max_mb` | `512` | LRU cap for cached thumbnails and copied files |
| `default_target` | `"generic"` | target app when none is given |
| `prefer_stickers` | `false` | copy Telegram sticker renditions by default |
| `timeout_secs` | `5` | request timeout before the offline fallback (transfers get 10 min) |
| `ui.columns` | `6` | popup grid columns (> 0) |
| `ui.thumb_px` | `128` | popup thumbnail size (>= 32) |
| `ui.hide_on_copy` | `true` | hide the popup after copying |
| `ui.start_hidden` | `false` | a bare `stickpile-desktop` that starts the resident instance does not show the popup (autostart uses `--hidden` instead) |
| `ui.recent_count` | `24` | recently used items shown before typing |
| `ui.max_results` | `60` | popup search results |
| `focus.detect` | `true` | detect the focused window |
| `focus.rules` | see above | window → target rules |
| `clipboard.default.{still,animation,video,sticker}` | see above | offers per kind |
| `clipboard.targets.<target>.*` | none | per-target override of the four lists |

These clipboard defaults are **provisional**. The phase 0 clipboard spike and
the manual paste matrix in [docs/clipboard.md](clipboard.md) decide what each
chat app actually accepts. The lists are config, so they can be tuned without
a rebuild.

## Logging in

`stickpile login` stores an API token in `token_file`. Later commands send it
as `Authorization: Bearer …`. Three ways to get one:

1. **Tailnet, no password.** If the server sits behind `tailscale serve`, the
   proxy already identifies you. `stickpile login` with no options first asks
   the server who you are without credentials. If it gets an answer, it
   creates a token for that identity (`POST /tokens`) and saves it. No
   password is involved. Without a token, the CLI also works on the tailnet
   request by request. The desktop client needs the stored token.
2. **Password (+TOTP).** Without a tailnet identity, `login` asks for the
   username and password (hidden input). If the account has TOTP, the server
   answers `totp_required` and `login` asks for the code. For scripts:

   ```sh
   stickpile login --username alice --password-file /run/secrets/stickpile-pw --totp 123456
   ```

   `--password-file` reads the first line. `--token-name` sets the name shown
   in the token list (default `stickpile-cli@<hostname>`).
3. **A token made elsewhere.** Create one in the web UI or with
   `stickpile tokens create <name>`, then write it to the token file yourself
   with mode 0600.

The token file is created with mode 0600 in one atomic step (temp file and
rename), in a 0700 directory. The client **refuses to read a token file that
group or others can read** and says how to fix it (`chmod 600 <file>`).

`stickpile logout` uploads queued usage events, revokes this client's token on
the server (found by its prefix), deletes the token file and clears the
account data in the offline cache.

## Global options

| option | env | meaning |
|---|---|---|
| `--config <path>` | `STICKPILE_CONFIG` | config file (default `$XDG_CONFIG_HOME/stickpile/config.toml`) |
| `--server <url>` | `STICKPILE_SERVER` | override `server_url` for this run |
| `--json` | | print the API response or a JSON result object instead of tables |
| `-v`, `-vv` | `STICKPILE_LOG` (tracing filter) | log to stderr |

`--server` does not change which token is used: the token file belongs to
whichever server you last logged in to. The offline cache is bound to one
server URL and one account. Switching either resets the cached items, history
and queued usage.

Exit status: 0 on success, 1 on any error. Errors go to stderr as
`error: …`, with a `hint:` for common causes (expired token, server
unreachable).

Set arguments (`<set>`) take a set id or the slug of one of your own sets.
Item arguments take an item id (shown in `search` and `sets show`).

## Commands

### `login`, `logout`, `whoami`

```sh
stickpile login [--username U] [--password-file F] [--token-name N] [--totp CODE]
stickpile logout
stickpile whoami            # username, role, TOTP, tailnet binding
```

### `config check`, `config path`

```sh
stickpile config check                     # checks --config (or the default path)
stickpile config check --file ./config.toml
stickpile --json config check --file x.toml   # prints the effective config with defaults
stickpile config path
```

`config check` runs the real loader and validation. It fails on a missing
file, a TOML error, unknown keys, a non-http(s) `server_url` or unsupported
clipboard MIME types. It never contacts the server, reads the token or opens
the cache.

### `sets`

```sh
stickpile sets list [--scope mine|public|all]
stickpile sets create cats --name "Cats" --visibility unlisted --aspect 1:1 --px 512
stickpile sets show cats
stickpile sets rm cats [--yes]       # asks for confirmation unless --yes (required when not interactive)
```

### `upload`

```sh
stickpile upload cats ~/Downloads/cat.png ~/Downloads/naahh.gif
```

Uploads in batches of 16 files. For each file it prints the new item, "already
in this set", or the error. It also notes near-duplicates ("you already have
the same file in set memes"). It exits 1 if any file failed.

### `import`, `export`, `backup`

```sh
stickpile import ~/stickers/my-cats              # creates a set from set.toml (slug, name, visibility, ...)
stickpile import ~/stickers/my-cats --into cats  # re-import into an existing set
stickpile export cats ./cats                     # writes ./cats/set.toml + 001-….png …
stickpile backup ~/backups/stickpile             # every own set into ~/backups/stickpile/<slug>/
```

`import` reads the directory with the set format reader
([docs/set-format.md](set-format.md)). Hidden files, subdirectories, symlinks
and non-media files are skipped with a warning. It uploads the set as one tar
(`archive` part), so the server sees exactly what is on disk. `export`
downloads the set's tar and writes the directory. Media files that are no
longer in the set are removed, so the directory matches the export. `backup`
only covers your sets as directories. The full server backup with the database
dump is `stickpile-server backup`.

### `search`

```sh
stickpile search cat
stickpile search 'e:😂 set:cats vibe:tired q>70 type:gif -tag:nsfw' --target telegram -n 10
stickpile --json search naahh
```

The query uses the search syntax from the plan: bare words, `"phrases"`,
`e:`, `set:`, `vibe:`, `tag:`, `type:`, `by:`, `is:mine|liked|public`,
`q>70`, `likes>=2`, `copies>5`, `sort:recent|likes|copies|quality`, and `-` to
negate. Words starting with a single dash after `search`/`copy` are query
negations, not options (`-tag:nsfw`, `-naahh`). `-t`, `-n` and `-v` keep their
meaning, and anything after `--` is query. With `--target`, items that cannot
be sent to that app are marked `(!)` with the reason.

### `copy`

```sh
stickpile copy naahh                         # best hit for the default target
stickpile copy grumpy cat --target telegram --sticker
stickpile copy --id 6f3k…                    # a specific item
stickpile copy naahh --print-path            # print the file path, do not touch the clipboard
```

`copy` picks the best match, chooses the variant for the target (see the
target-app profiles), downloads it into the cache, puts it on the clipboard,
records a usage event and uploads it. When no variant fits the target (for
example a video over Discord's 10 MiB limit, or no Telegram sticker rendition
yet), the best available variant is copied anyway and a `warning:` explains
why. `--print-path` prints the path of the cached file, named after the item
(`…/clip/<media>.<variant>/naahh.gif`), for drag-and-drop tools or scripts.

### `sync`, `flush`

```sh
stickpile sync     # refresh the offline cache (incremental) and upload queued usage
stickpile flush    # upload queued usage events now
```

### `like`, `unlike`, `popular`

```sh
stickpile like 6f3k…
stickpile popular                                    # server-wide, last week
stickpile popular --scope mine --window month -n 20
stickpile popular --scope set --set cats --window all
```

`popular` prints the three signals side by side: most copied, most liked, and
trending (copies in the last 7 days). They are never blended.

### `tokens`

```sh
stickpile tokens list                 # marks the token this client uses
stickpile tokens create phone --expires-days 90   # prints the secret once
stickpile tokens revoke <id>
```

### `gitpack`, `pull`

```sh
stickpile gitpack add https://github.com/someone/frog-pack.git --branch main --subdir pack --mode read-only --slug frogs
stickpile pull frogs --wait
```

`gitpack add` prints the new set, the webhook URL and its secret (shown once),
and the clone job. See [docs/git-packs.md](git-packs.md).

### `telegram`

```sh
stickpile telegram import Animals --slug animals --visibility private --wait
stickpile telegram export cats --pack cats --title "Cats" --wait   # pack cats_by_<bot>
```

Both run as server jobs. `--wait` polls until the job is done and exits 1 if
it failed. See [docs/telegram.md](telegram.md).

### `job`

```sh
stickpile job <id>          # status, subject, result, error
stickpile job <id> --wait
```

### `admin` (admin role)

```sh
stickpile admin users
stickpile admin invite --role user --expires-days 7    # prints the registration link
stickpile admin audit -n 100 [--before <id>]
stickpile admin storage
stickpile admin jobs --status failed
```

## Clipboard behaviour

`copy` (and the desktop popup) puts several representations of one file on
the clipboard. Which ones depends on `[clipboard]` for the target and the
media kind:

| offer | payload |
|---|---|
| `image/png`, `image/gif`, `image/webp`, `image/jpeg` | the file bytes if it already has that type, else converted (a WebP still becomes PNG data; JPEG is flattened onto white). Animations convert only to `image/gif`; other image offers are skipped for animations. Videos never become image data. |
| `text/uri-list` | `file:///…\r\n`, percent-encoded (spaces, `#`, non-ASCII) |
| `x-special/gnome-copied-files` | `copy\nfile:///…` (GTK/Nautilus "copied file") |
| `text/plain` | the absolute path |

The file sits in the cache and is named after the item. Telegram and file
managers show that name.

On **Wayland** the clipboard contents disappear when the process that set
them exits. On **X11** someone has to answer paste requests. So `copy`
re-executes itself as a hidden helper (`stickpile __serve-clipboard`),
waits until the helper owns the clipboard, and exits. The helper is
detached (new session, stdio on `/dev/null`) and serves paste requests until
another application takes the clipboard. Then it exits. Only one helper is
alive at a time in practice, because each new copy replaces the previous
selection.

- **Wayland** uses the `wlr`/`ext` data-control protocol, which works on
  Hyprland, Sway, KDE and other wlroots compositors. It is chosen when
  `WAYLAND_DISPLAY` is set. The offers are announced exactly as configured
  (no implicit `text/plain` aliases), but the announcement order is not
  guaranteed.
- **X11** owns `CLIPBOARD` from an unmapped window and answers `TARGETS`,
  `TIMESTAMP` and every offer. `text/plain` is also served as `UTF8_STRING`,
  `STRING` and `TEXT`. Large payloads use the `INCR` protocol. It is chosen
  when only `DISPLAY` is set. It is also the fallback on compositors without
  data-control (GNOME) through XWayland.
- Without either display, `copy` fails with "no clipboard available". Use
  `--print-path`.

What each chat app does with these offers (photo vs. file vs. sticker) is
recorded in the manual matrix in [docs/clipboard.md](clipboard.md). The
defaults above are provisional until that matrix is filled in. To check the
helper on a headless display, run the gated test:
`STICKPILE_TEST_X11_DISPLAY=:99 cargo test -p stickpile-cli copy_to_a_real_clipboard`
(Xvfb), or `STICKPILE_TEST_WAYLAND_DISPLAY=wayland-1` with a headless sway.

## Offline behaviour

The cache (`cache_dir`) keeps:

- **Item metadata**: every item you may see in search (your own and public
  sets, not unlisted ones). `sync` fills it incrementally
  (`GET /items?updated_since=…`, paged), and it applies deletions and items
  that became private. The first sync is a full listing that also drops stale
  entries. The desktop client syncs periodically.
- **Files**: thumbnails and the files you copied, evicted least recently used
  beyond `cache_max_mb`.
- **Copy history**: the "recently used" list.
- **Usage queue**: copy events not yet uploaded.

When the server is unreachable (connection refused, timeout after
`timeout_secs`, or a 502/503/504 from the proxy):

| command | offline |
|---|---|
| `search` | answered from the cache with the same query syntax and ranking code. Metadata only: no image-content matches, and `is:liked` matches nothing. A note says so on stderr; `--json` has `"offline": true`. |
| `copy` | works for items whose variant file is cached (copied before). The usage event is queued. |
| `copy --id` | uses the cached metadata. |
| `sync`, `flush` | fail. Queued events stay and go out on the next `copy`, `sync`, `flush` or desktop sync. |
| everything else | needs the server |

Usage events carry a client-generated id, so uploads can be repeated safely.
The server ignores duplicates. Events the server rejects (item deleted or no
longer visible) are dropped.

## Files

| path | content |
|---|---|
| `$XDG_CONFIG_HOME/stickpile/config.toml` | configuration (shared with the desktop client) |
| `$XDG_STATE_HOME/stickpile/token` | API token, mode 0600 |
| `$XDG_CACHE_HOME/stickpile/cache.sqlite3` | metadata, sync cursor, history, usage queue (SQLite, WAL; safe to share between the CLI and the desktop client) |
| `$XDG_CACHE_HOME/stickpile/files/thumb/` | thumbnails |
| `$XDG_CACHE_HOME/stickpile/clip/<media>.<variant>/<name>.<ext>` | files put on the clipboard |

Deleting the cache directory is always safe, except that queued usage events
would be lost. Run `stickpile flush` first.
