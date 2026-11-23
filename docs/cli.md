# `stickpile` command-line client

`stickpile` talks to a stickpile server over its REST API. It handles login,
sets, import and export of set directories, search, copying to the clipboard,
usage statistics, git packs, Telegram packs, rechecks of metadata, keeping
stickers offline and admin tasks. It shares its
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
                                        # (or: stickpile login --code <code from Settings, Add a device>)
stickpile import ~/stickers/my-cats     # a set directory with set.toml + media files
stickpile search cat type:gif           # search your library and public sets
stickpile copy naahh --target discord   # best match onto the clipboard
stickpile export my-cats ./my-cats      # write the set back out as a directory
stickpile sync                          # fill the offline cache
stickpile offline keep --set my-cats    # keep a pack on this device (shows the estimate, asks)
stickpile offline sync                  # download what is kept; then it copies with the server down
```

## Configuration (`config.toml`)

Default location: `$XDG_CONFIG_HOME/stickpile/config.toml`
(`~/.config/stickpile/config.toml`). A missing file means all defaults.
A key this version does not know (a typo, or a newer version's setting)
is skipped with a warning, so a key a newer popup saved never stops the
CLI; `stickpile config check` validates a file without contacting the
server and fails on such keys
([configuration.md](configuration.md#older-clients-and-newer-keys)).

The desktop client reads the same file. The `ui`, `focus` and `paste`
sections only affect the desktop popup. When the file is read-only (home-manager), the
popup's Settings tab saves to `settings.toml` next to it, and both clients
layer that file under `config.toml`; see
[configuration.md](configuration.md#settings-saved-from-the-popup).

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
columns = 6             # per row at the default window size; the grid reflows on resize
thumb_px = 128          # >= 32
hide_on_copy = true
start_hidden = false
recent_count = 24       # items shown before typing, most recently copied first
max_results = 60
modal = true            # vim-style Insert/Normal modes; false keeps the plain keys
start_tab = "last"      # last | search | recent | most_used | packs | favorites | me | settings
start_mode = "normal"   # normal | insert: the mode the popup opens in
mark_favorites = true   # a heart on the items you liked
show_emojis = true      # an item's emojis on its tile
show_tags = true        # an item's tags on its tile
show_vibe = true        # an item's vibe on its tile
show_text = false       # the words in an item's picture on its tile, one line
show_details = true     # the selected item's stats above the footer
safe_mode = false       # hide the items tagged nsfw (S in the popup)
counts = false          # digits start a count (3j, 5G); tab digits then wait
language = "auto"       # auto | en | uk: the popup's language (auto: the account's, else the system's)
theme = "system"        # system | dark | oled | white: the popup's colours

[copy]                  # `copy` and the popup
square = "off"          # off | transparent | white: pad non-square stills and stickers to a square
background = "keep"     # keep | white: fill the transparent pixels of stills copied as images

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

[paste]                 # desktop popup and macOS app only
auto = true             # after a pick, press the paste shortcut in the window the popup was opened over
wait_ms = 1000          # longest wait for that window to get the focus back (0..5000)
delay_ms = 50           # pause before the key press (0..1000)
shortcut = "ctrl+v"     # for windows no rule matches; none: no paste
# First match wins; substrings matched case-insensitively against the window class only.
# Setting `rules` replaces the default list, which is:
[[paste.rules]]
contains = ["kitty", "foot", "alacritty", "wezterm", "ghostty", "konsole", "terminal",
            "org.gnome.console", "kgx", "ptyxis", "xterm", "rxvt", "terminator", "tilix",
            "terminology", "blackbox", "st-256color", "contour"]
shortcut = "ctrl+shift+v"

# MIME types put on the clipboard, in offer order, per media kind.
# Supported: image/png image/gif image/webp image/jpeg text/uri-list
#            x-special/gnome-copied-files text/plain
#            application/x-sticker application/x-no-dialog
[clipboard.default]
still = ["image/png", "text/uri-list", "x-special/gnome-copied-files"]
animation = ["image/gif", "text/uri-list", "x-special/gnome-copied-files"]
video = ["text/uri-list", "x-special/gnome-copied-files"]
sticker = ["text/uri-list", "x-special/gnome-copied-files", "application/x-sticker", "application/x-no-dialog"]

# Per-target overrides replace [clipboard.default] for that target. Kinds left
# out of a target section take the built-in defaults shown above (not the
# values of your [clipboard.default]).
[clipboard.targets.telegram]
still = ["image/png", "text/uri-list"]
animation = ["text/uri-list"]
video = ["text/uri-list"]
sticker = ["text/uri-list"]

[offline]               # what `offline keep` keeps (see "Keeping stickers offline")
targets = []            # chat apps whose renditions are kept; [] = default_target + the apps in the copy history
sticker_renditions = true  # also keep the rendition a copy as a sticker uses (Telegram)
stills = true           # kinds downloaded for packs, the library and automatic keeping
animations = true
videos = false
max_file_mb = 0         # skip a single file larger than this (packs, library, automatic); 0: no limit
max_mb = 1024           # the most kept files may take; 0: no limit

[offline.auto]
thumbnails = true       # keep every item's thumbnail (fetched by `offline sync`)
used = false            # keep the most recently copied stickers...
used_count = 200        # ...this many (1-2000)
liked = false           # keep the stickers you liked
own_packs = false       # keep the packs you own
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
| `ui.columns` | `6` | popup grid columns at the default window size (> 0); a resized window reflows |
| `ui.thumb_px` | `128` | popup thumbnail size (>= 32) |
| `ui.hide_on_copy` | `true` | hide the popup after copying |
| `ui.start_hidden` | `false` | a bare `stickpile-desktop` that starts the resident instance does not show the popup (autostart uses `--hidden` instead) |
| `ui.recent_count` | `24` | recently used items shown before typing |
| `ui.max_results` | `60` | popup search results |
| `ui.modal` | `true` | vim-style Insert/Normal modes in the popup |
| `ui.start_tab` | `"last"` | popup tab shown on open: `last` (the tab it was closed on), `search`, `recent`, `most_used`, `packs`, `favorites`, `me` or `settings` |
| `ui.start_mode` | `"normal"` | mode the popup opens in (with `modal`): `normal`, or `insert` to type right away on Search and Packs |
| `ui.mark_favorites` | `true` | a heart on the tiles of items you liked |
| `ui.show_emojis` | `true` | an item's emojis on its tile |
| `ui.show_tags` | `true` | an item's tags on its tile |
| `ui.show_vibe` | `true` | an item's vibe on its tile |
| `ui.show_text` | `false` | the words in an item's picture on its tile, one line (off: the line would cover the picture whose words it repeats) |
| `ui.show_details` | `true` | the selected item's stats in a row above the popup's footer |
| `ui.safe_mode` | `false` | safe mode: the popup hides the items tagged `nsfw` (`S` switches it) |
| `ui.counts` | `false` | Normal-mode digits start a count (`3j`, `5G`); a lone 1-7 then switches tabs after a short wait instead of at once |
| `ui.language` | `"auto"` | the popup's language: `auto` (the account's, else the system's), `en` or `uk`; see [languages.md](languages.md) |
| `copy.square` | `"off"` | pad non-square stills and sticker renditions to a square: `off`, `transparent` or `white` margins |
| `copy.background` | `"keep"` | `white` fills the transparent pixels of stills copied as images |
| `focus.detect` | `true` | detect the focused window |
| `focus.rules` | see above | window → target rules |
| `paste.auto` | `true` | paste after a pick ([desktop.md](desktop.md#paste-after-a-pick)) |
| `paste.wait_ms` | `1000` | longest wait for the window to get the focus back, 0..5000 |
| `paste.delay_ms` | `50` | pause before the key press, 0..1000 |
| `paste.shortcut` | `"ctrl+v"` | shortcut for windows no rule matches (`none`: no paste); not used on macOS |
| `paste.rules` | see above | window class → shortcut rules (terminals: `ctrl+shift+v`) |
| `clipboard.default.{still,animation,video,sticker}` | see above | offers per kind |
| `clipboard.targets.<target>.*` | none | per-target override of the four lists |
| `offline.targets` | `[]` | chat apps whose renditions a kept sticker keeps; empty: `default_target` plus the targets in the copy history |
| `offline.sticker_renditions` | `true` | also keep the rendition a copy with `--sticker` uses |
| `offline.stills`, `.animations`, `.videos` | `true`, `true`, `false` | kinds downloaded for kept packs, the library and the automatic rules (a sticker kept by itself is always downloaded) |
| `offline.max_file_mb` | `0` | skip a single rendition larger than this for packs, the library and the automatic rules (0: no limit) |
| `offline.max_mb` | `1024` | budget for kept files; `offline sync` stops there and deletes nothing (0: no limit) |
| `offline.auto.thumbnails` | `true` | keep every item's thumbnail |
| `offline.auto.used`, `.used_count` | `false`, `200` | keep the most recently copied stickers of this device, this many (1-2000) |
| `offline.auto.liked` | `false` | keep the stickers the account liked |
| `offline.auto.own_packs` | `false` | keep the packs the account owns |
| `offline.mobile_data` | `false` | Android only |

These clipboard defaults are **provisional**. The phase 0 clipboard spike and
the manual paste matrix in [docs/clipboard.md](clipboard.md) decide what each
chat app actually accepts. The lists are config, so they can be tuned without
a rebuild.

## Logging in

`stickpile login` stores an API token in `token_file`. Later commands send it
as `Authorization: Bearer ...`. Four ways to get one:

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
3. **A pairing code, no password.** In the web UI, signed in on another
   device, Settings, **Add a device** shows a one-time code such as
   `7KQ2-M9XA-4FTR` (and a QR code for the phone app). Then:

   ```sh
   stickpile login --code 7KQ2-M9XA-4FTR
   ```

   The code is exchanged for a token (`POST /api/v1/auth/pair`) on the
   configured `server_url` (or `--server`; the page shows the server's
   address). Case, spaces and dashes do not matter, and `O`, `I`, `L` read
   as `0`, `1`, `1`. A code works once, for five minutes; a wrong, expired
   or used one is refused and the stored token, if any, stays.
   `--token-name` works as above; `--code` cannot be combined with
   `--username`, `--password-file` or `--totp`.
4. **A token made elsewhere.** Create one in the web UI or with
   `stickpile tokens create <name>`, then write it to the token file yourself
   with mode 0600.

The token file is created with mode 0600 in one atomic step (temp file and
rename), in a 0700 directory. The client **refuses to read a token file that
group or others can read** and says how to fix it (`chmod 600 <file>`).

`stickpile logout` uploads queued usage events, revokes this client's token on
the server (found by its prefix), deletes the token file and clears the
account data in the offline cache, the files kept offline and the list of
what is kept included.

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
unreachable, a sticker not kept offline). `offline sync` also exits 2 when
it ended before everything kept was on this device (`incomplete: ...` on
stderr; see [`offline`](#offline)).

Set arguments (`<set>`) take a set id or the slug of one of your own sets.
Item arguments take an item id (shown in `search` and `sets show`).

## Commands

### `login`, `logout`, `whoami`

```sh
stickpile login [--username U] [--password-file F] [--token-name N] [--totp CODE]
stickpile login --code PAIRING-CODE [--token-name N]
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
file, a TOML error, a non-http(s) `server_url`, an unknown `ui.start_tab`
or `[copy]` value, unsupported clipboard MIME types, or keys this version
does not know. Those keys only get a warning from the other commands and
the popup, which skip them; the check prints the same warnings and then
fails, naming every such key:

```
warning: /home/me/.config/stickpile/config.toml: ui.show_nonsense is not a setting this stickpile knows (a typo, or a newer version's setting); ignoring it
error: /home/me/.config/stickpile/config.toml: 1 key this stickpile does not know: ui.show_nonsense
```

When the file is read-only, the popup's `settings.toml` next to it is
layered in as the clients do it: its skipped entries are printed as
`warning:` lines, and `--json` shows the result. It never contacts the
server, reads the token or opens the cache.

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
stickpile upload cats ~/Downloads/stare.png --text 'NO THOUGHTS\nhead empty'
```

Uploads in batches of 16 files. For each file it prints the new item, "already
in this set", or the error. It also notes near-duplicates ("you already have
the same file in set memes"). It exits 1 if any file failed.

`--text` gives the picture its text, the words written in it, as yours (one
file only; a literal `\n` is a line break). Without it the server reads the
words itself when it has a text reader ([ai.md](ai.md#ocr-and-the-text-field)).
A server older than item texts (0.7.0) drops the text and keeps the file:
the command then fails with "uploaded as ... without its text: the server
is too old for item texts". A file already in the set keeps its own
metadata, text included, on every server.

### `item`

```sh
stickpile item show 6f3k...                       # name, keywords, emojis, vibe, tags, text
stickpile item edit 6f3k... --text 'no thoughts'  # the text becomes yours
stickpile item edit 6f3k... --text ''             # clear it; the server no longer fills it
stickpile item edit 6f3k... --name 'staring cat'
```

`item show` prints the item's metadata; a value the server filled and nobody
confirmed is marked `(AI)`, and the text keeps its lines. `item edit` changes
the name or the text (at least one) and prints the item; what you give is
yours ([api.md](api.md#items)). With `--json` both print the `Item`. On a
server older than item texts (0.7.0) `--text` fails with "the server is
too old for item texts" (a `--name` given with it is saved).

### `import`, `export`, `backup`

```sh
stickpile import ~/stickers/my-cats              # creates a set from set.toml (slug, name, visibility, ...)
stickpile import ~/stickers/my-cats --into cats  # re-import into an existing set
stickpile export cats ./cats                     # writes ./cats/set.toml + 001-….png …
stickpile export cats --format zip -o cats.zip   # one file: zip, tar or tar.gz
stickpile export cats --format whatsapp -o cats-whatsapp.zip   # the WhatsApp sticker packs
stickpile backup ~/backups/stickpile             # every own set into ~/backups/stickpile/<slug>/
```

`import` reads the directory with the set format reader
([docs/set-format.md](set-format.md)). Hidden files, subdirectories, symlinks
and non-media files are skipped with a warning. It uploads the set as one tar
(`archive` part), so the server sees exactly what is on disk. `export`
downloads the set's tar and writes the directory. Media files that are no
longer in the set are removed, so the directory matches the export.

`export --format FORMAT -o FILE` downloads one file instead of writing a
directory (give either a directory or `--format`, not both):

- `zip`, `tar`, `tar.gz` (also `tgz`): the set directory as an archive; the
  three hold the same files. There is no RAR: making RAR archives needs the
  proprietary `rar` program.
- `whatsapp`: the WhatsApp sticker packs. The server builds them first (a
  job; an unchanged set reuses the last build), then the zip of `.wastickers`
  files and `contents.json` is downloaded. The packs, their kind and size,
  and the items that were left out are printed; with `--json`, the build
  (`WhatsappBuild`) is in the output. It exits 1 if the build failed, for
  example when fewer than 3 items can become stickers. See
  [docs/whatsapp.md](whatsapp.md).

Sets that are not yours work by id, as long as you can read them. `backup`
only covers your sets as directories. The full server backup with the database
dump is `stickpile-server backup`.

### `search`

```sh
stickpile search cat
stickpile search 'e:😂 set:cats vibe:tired q>70 type:gif -tag:nsfw' --target telegram -n 10
stickpile search e:skull                     # stickers with a skull emoji (💀, ☠️, ...)
stickpile search x:lo                        # "LOL", "love", ... written in the picture
stickpile search 'x:?'                       # "?", "???", "what?!" written in the picture
stickpile --json search naahh
```

The query uses the search syntax from the plan: bare words, `"phrases"`,
`e:` (an emoji, `e:😂`, or its English name, `e:skull`, typos forgiven:
any emoji whose name or keyword starts so), `set:`, `vibe:`, `tag:`, `type:`, `by:`, `is:mine|liked|public`,
`text:` or `x:` (the words in the picture, one after another; typos and
words inside longer words count, and the last may be just its start:
`x:l` finds "lol"; punctuation too: `x:?`, `x:...`, `x:"what?"`;
[search.md](search.md#matching)),
`q>70`, `likes>=2`, `copies>5`, `sort:recent|likes|copies|quality`, and `-` to
negate. A server up to 0.8.1 takes `text:` words whole, so `x:lo` sent to
it finds the word "lo" alone; one up to 0.8.7 finds nothing for `x:?`.
Quote `?` and `*` for the shell. Words starting with a single dash after `search`/`copy` are query
negations, not options (`-tag:nsfw`, `-naahh`, `-?`). `-t`, `-n` and `-v` keep their
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
With `[copy]` padding or a white background, a still is copied as the
adjusted PNG (`<cache_dir>/clip/<media>.<variant>.sq-white/<name>.png`) and a sticker
as the padded WebP.

### `sync`, `flush`

```sh
stickpile sync     # refresh the offline cache (incremental) and upload queued usage
stickpile flush    # upload queued usage events now
```

### `offline`

Keeps stickers on this device, so that `copy` works with the server
unreachable: one sticker, a pack (its items now and what a sync adds
later) or the whole library (every item the offline cache lists). What is
kept is stored in the cache, on this device only (not on the server, not
synced); the files are ordinary cache files, protected from eviction
([offline.md](offline.md)).

```sh
stickpile offline keep <item>              # one sticker: kept at once
stickpile offline keep --set cats          # a pack: the estimate, then [y/N]
stickpile offline keep --all --yes         # the library, without asking
stickpile offline keep --set cats --dry-run   # the estimate only
stickpile offline sync                     # sync, then download what is kept (exit 0, 2 or 1)
stickpile offline status                   # what is kept, sizes, to download, skipped, failed
stickpile offline status --dry-run --set cats # what keeping it would take, without keeping it
stickpile offline release --set cats       # stop keeping it; its files stay until evicted
stickpile offline remove-all [--yes]       # delete the kept files and forget what is kept
stickpile cache clear                      # delete the other cached files; kept ones stay
```

A scope is an item id, `--set` with a set id or slug (looked up in the
offline cache; when two packs share a slug, yours wins, else give the id)
or `--all`. `keep` looks a scope the cache does not know up after a sync
(an own pack with no items yet: on the server).

**`keep`** stores the choice; `offline sync` downloads the files. For a
pack or the library it first prints the estimate and asks:

```
Keeping pack "Cats" (cats) offline takes:
  items:        4
  files:        12 (291.2 KiB)
  on device:    0 (0 B)
  to download:  12 (291.2 KiB)
  kept files afterwards: 301.7 KiB of the 1.0 GiB budget (offline.max_mb)
Keep pack "Cats" (cats) offline? [y/N] y
Kept pack "Cats" (cats) offline: 12 files (291.2 KiB), 0 of them on this device; `stickpile offline sync` downloads the rest.
```

What the options leave out gets a line per reason, such as
`skipped:      1 item: videos are not kept with packs, the library or automatic rules`.
"Kept files afterwards" counts every kept file (here the automatic
thumbnails of the other packs too), against `offline.max_mb`; over it,
the line says so and keeping goes on only when confirmed, with a warning.

The files are the thumbnail and, for each chat app of
`offline.targets`, the rendition `copy` would use (as a sticker too with
`offline.sticker_renditions`). A pack or the library leaves out what the
per-kind options (`offline.stills`, `.animations`, `.videos`) and
`offline.max_file_mb` exclude, reported as skipped with the reason; a
sticker kept by itself is always downloaded whole. `--yes` does not ask,
and is required when stdin is not a terminal (`refusing to keep ...
without confirmation; pass --yes`, exit 1). A sticker is kept without
asking. Keeping what is kept already says so and changes nothing.

**`release`** stops keeping a scope. Its files go back to the ordinary
cache: evicted least recently used beyond `cache_max_mb`, not deleted now.
A file another kept scope or an automatic rule wants stays kept.

**`status`** prints a table of what is kept (each kept sticker, pack or
the library, and the automatic rules that are on, with their items and
how many of their files and bytes are on this device), the kept files
against the budget, the other cached files, what is still to download,
the skipped items with their reasons and the downloads that failed with
theirs (the attempts and when). `--json`: `keeps` (each with `scope`, `id`,
`name`, a pack's `slug`), `usage` (`kept_files`, `kept_bytes`,
`other_files`, `other_bytes`, `budget`), `totals`, `missing_files`,
`missing_bytes`, `complete`, `over_budget`, `scopes`, `skipped`,
`failures`. With `--dry-run` and a scope it prints the estimate instead
(`--json`: `scope`, `estimate`, `missing_files`, `missing_bytes`, `fits`),
like `keep --dry-run`.

**`sync`** syncs the cache, then runs the downloader until it ends: four
files at a time, each written to a part file and renamed into place once
its size and checksum match. The progress goes to stderr (in place on a
terminal: `offline: downloading: 8/18 files, 278.5 KiB/676.3 KiB`), then a
line on stdout:

```
Synced: 0 updated, 0 removed, 6 items cached.
offline: downloading: 0/14 files, 0 B/301.7 KiB
offline: done: 14/14 files, 301.7 KiB/301.7 KiB
Downloaded 14 files (301.7 KiB); 14 of 14 kept files are on this device (301.7 KiB of 301.7 KiB).
```

(the two `offline:` lines are the progress on stderr; on a terminal it is
one line, updated in place).

Ctrl-C cancels the run cleanly (the downloads stop and their part files
go; a second Ctrl-C stops at once); the next `offline sync` goes on.
Exit status:

| status | when | stderr |
|---|---|---|
| 0 | everything kept is on this device | |
| 2 | the budget is reached: no download starts that would pass `offline.max_mb`, and nothing is deleted | `incomplete: the offline budget is reached: kept files take 600.1 KiB and offline.max_mb is 1.0 MiB; 3 files (1.2 MiB) not on this device; nothing was deleted` |
| 2 | the server is unreachable (the sync failed, or it went away during the run), also when everything kept as of the last sync is here | `incomplete: server unreachable: error sending request ... Connection refused (os error 111); 2 files (70.1 KiB) not on this device` |
| 2 | cancelled (Ctrl-C) | `incomplete: cancelled; ...` |
| 2 | the disk is full; nothing is deleted | `incomplete: the disk is full (...); ...` |
| 2 | files failed after the downloader's retries ([offline.md](offline.md#the-downloader)) | `incomplete: 1 file failed; ...`; `status` lists the reasons |
| 1 | an error: not logged in, the token rejected, a broken config | `error: ...` |

Each `incomplete:` line is followed by a `hint:`. `--json` prints `sync`
(the sync report, `null` when the server was unreachable), `end`
(`complete`, `limit_reached`, `waiting_for_network`, `cancelled`,
`disk_full`, `unauthorized`), `complete`, `downloaded_files`,
`downloaded_bytes`, `budget` and the downloader's last `status` (`state`,
`files_done`, `files_total`, `bytes_done`, `bytes_total`, `skipped`,
`failed`, `scopes`, `message`), and exits as above. `stickpile sync` does
not download kept files: the CLI is not resident, so `offline sync` is its
run of the downloader ([offline.md](offline.md#clients) for the other
clients).

**`remove-all`** deletes every kept file and forgets every kept scope, the
failures included. It asks first (`--yes` does not; required when stdin is
not a terminal). The automatic rules of `[offline.auto]` are options, not
choices: the ones that are on keep their files again at the next
`offline sync` (the command says which) until they are turned off.

### `cache clear`

```sh
stickpile cache clear
```

Deletes the cached thumbnails and copied files that are not kept offline,
and says how many kept files stay:

```
Deleted 1 cached file (235.4 KiB). 14 files (301.7 KiB) kept offline stay: `stickpile offline remove-all` deletes them.
```

`--json`: `removed_files`, `freed_bytes`, `other_bytes` (what is left of
the other files), `kept_files`, `kept_bytes`. Item metadata, the copy
history and queued usage stay.

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

### `publish`

```sh
stickpile publish cats                     # where the set is published, per platform
stickpile publish cats --to telegram --wait
stickpile publish cats --to telegram -O telegram.pack_name=cats -O telegram.title="My cats"
stickpile publish cats --to discord -O discord.guild_id=100000000000000001 -O discord.kind=emojis
stickpile publish cats --to telegram,discord,signal --wait   # Discord: the server used last
```

Without `--to`, a table of the platforms the server knows: whether each is
configured, what the set is published as, when it was last published, the
last outcome and the link (`--json`: the `Publication` list). With `--to`
(comma-separated platform ids: `telegram`, `discord`, `signal`), one
publish job per platform is queued through `POST /api/v1/sets/{id}/publish`;
the request fails as a whole for an unknown or unconfigured platform or a
bad option. `-O PLATFORM.KEY=VALUE` (repeatable) sets one of a platform's
options: Telegram's `pack_name` and `title`; Discord's `guild_id` (the
server id, required the first time, afterwards the server used last;
`discord.guild=...` works too) and `kind` (`stickers` or `emojis`); Signal
has none, and every publish makes a new Signal pack with a new link.
`--wait` waits for every job; the command exits 1 if any failed. Only the
set's owner may publish. See [discord.md](discord.md#in-the-server) and
[signal.md](signal.md#in-the-server).

There is no `stickpile discord` or `stickpile signal` command for imports
yet: use the web pages (`/discord/import`, `/signal/import`) or the API
(`POST /api/v1/discord/import`, `POST /api/v1/signal/import`).

### `job`

```sh
stickpile job <id>          # status, subject, result, error
stickpile job <id> --wait
```

### `recheck`

```sh
stickpile recheck --parts keywords,emojis,vibe,text [--mode missing|refresh] [--set SET] [--wait]
stickpile recheck --all-parts [--mode refresh] [--wait]
stickpile recheck --all-parts --all [--set SET-ID] [--wait]   # every item on the server (admin role)
stickpile recheck list                  # yours (an admin's include the server-wide ones), newest first
stickpile recheck show <id> [--wait]    # progress and the pictures that failed
stickpile recheck cancel <id>
```

Recomputes metadata of your items in the background (the items of the
sets you own; with `--set`, one of them). `--parts` takes `keywords`,
`emojis`, `vibe`, `text` (the words in the picture), `embedding` (for
search), `quality` and `renditions`; `--all-parts` asks for everything
the server can recompute now (without AI only `quality` and
`renditions`). One of the two is required. `--mode missing` (the default)
computes only what is not there yet; `refresh` computes everything again
and replaces what the computer gave before. A value you gave is never
changed. What each part does: [ai.md](ai.md#recheck).

The server answers with the running recheck when the same one runs
(`Recheck ... is already running`), and limits how many you start
(`limits.recheck_per_hour`). `--wait` shows the progress on stderr
(`3/10 pictures, 0 failed, 2 changed`) and prints the result; it fails
when every picture failed. `--json` prints the `Recheck` (`show`: with
`failures`). A part the server cannot recompute is refused with its
reason (for example "AI is turned off on this server"). Against a server
older than rechecks (0.7.0) every `recheck` command fails with "the server
is too old for rechecks".

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
| `application/x-sticker` | `1`. Paste hint: the file is a sticker and should be sent as one, not as a photo or a file. Apps that do not know it ignore it (`docs/clipboard.md`, section 5) |
| `application/x-no-dialog` | `1`. Paste hint: complete the paste without the app's confirmation dialog (a send box, for example). Apps that do not know it ignore it (`docs/clipboard.md`, section 5) |

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
  beyond `cache_max_mb`; and the files kept offline
  ([`offline`](#offline)), which are never evicted and count against
  `offline.max_mb` instead.
- **What is kept offline**: the stickers, packs and library you keep.
- **Copy history**: the "recently used" list.
- **Usage queue**: copy events not yet uploaded.

When the server is unreachable (connection refused, timeout after
`timeout_secs`, or a 502/503/504 from the proxy):

| command | offline |
|---|---|
| `search` | answered from the cache with the same query syntax and ranking code. Metadata only: no image-content matches, and `is:liked` matches nothing. A note says so on stderr; `--json` has `"offline": true`. |
| `copy` | works for items whose variant file is on this device: kept offline, or copied before and not evicted yet. When the file the target would get is not here but another one the target takes is (a kept still copied to Telegram as a sticker whose sticker WebP is not kept: its PNG), that one is copied, with `warning: The file for this app is not on this device; another one it takes was copied`. The usage event is queued. Otherwise it fails (exit 1) with `<name>: the <rendition> file is not on this device and the server is unreachable (...)` and a `hint:` that depends on the sticker: not kept, the `stickpile offline keep <item>` to run while online; kept but not downloaded yet, `stickpile offline sync`; kept without that file, the chat app to list in `offline.targets` (and `offline.sticker_renditions` for `--sticker`); kept with a pack, the library or a rule that leave the file out, the per-kind options and `offline.max_file_mb`. |
| `copy --id` | uses the cached metadata. |
| `sync`, `flush` | fail. Queued events stay and go out on the next `copy`, `sync`, `flush` or desktop sync. |
| `offline sync` | exits 2: `incomplete: server unreachable: ...`, with how much is on this device. |
| `offline keep`, `release`, `status`, `remove-all`, `cache clear` | work (`keep` of something the cache does not list needs the server). |
| everything else | needs the server |

Usage events carry a client-generated id, so uploads can be repeated safely.
The server ignores duplicates. Events the server rejects (item deleted or no
longer visible) are dropped.

## Files

| path | content |
|---|---|
| `$XDG_CONFIG_HOME/stickpile/config.toml` | configuration (shared with the desktop client) |
| `$XDG_CONFIG_HOME/stickpile/settings.toml` | popup settings, used only when `config.toml` is read-only |
| `$XDG_STATE_HOME/stickpile/token` | API token, mode 0600 |
| `$XDG_CACHE_HOME/stickpile/cache.sqlite3` | metadata, sync cursor, history, usage queue, what is kept offline and which files are (SQLite, WAL; safe to share between the CLI and the desktop client) |
| `$XDG_CACHE_HOME/stickpile/files/thumb/` | thumbnails |
| `$XDG_CACHE_HOME/stickpile/files/cover/` | the cover images of kept packs |
| `$XDG_CACHE_HOME/stickpile/clip/<media>.<variant>/<name>.<ext>` | files put on the clipboard, and the renditions kept offline |

Deleting the cache directory is always safe, except that queued usage events
would be lost (run `stickpile flush` first), and so would what is kept
offline (`stickpile cache clear` leaves it).
