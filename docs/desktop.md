# Linux desktop client (`stickpile-desktop`)

A keyboard-first quick-search popup: press a shortcut in any chat app, type,
press Enter, paste. GTK4 + libadwaita, resident in the background.

## Install and run

1. Configure the server and log in once (the token is stored in
   `$XDG_STATE_HOME/stickpile/token`, mode 0600):

   ```sh
   stickpile login            # see docs/cli.md; writes the token
   ```

   On a tailnet behind `tailscale serve` requests work without a token
   (the server knows you from your tailnet identity), so this step is
   optional there; `stickpile login` still turns that identity into a token
   without a password.

   The client reads `~/.config/stickpile/config.toml` (`--config FILE` or
   `STICKPILE_CONFIG` to override); with home-manager it is generated from
   `programs.stickpile.settings`.

2. Start the resident instance once per session, hidden:

   ```sh
   stickpile-desktop --hidden
   ```

   The example systemd user unit
   `crates/stickpile-desktop/data/stickpile-desktop.service` does this
   (`systemctl --user enable --now stickpile-desktop`); the home-manager
   module installs an equivalent unit. Alternatively start it from the
   compositor's autostart (below).

3. Bind a key to `stickpile-desktop` (below). Each press toggles the popup.

From a checkout: `nix develop -c cargo run -p stickpile-desktop -- --hidden`.
The desktop entry and icon live in `crates/stickpile-desktop/data/`.

### Command line

| Invocation | First instance (nothing running) | Later invocation (forwarded to the running instance) |
|---|---|---|
| `stickpile-desktop` | start; show unless `ui.start_hidden` | toggle the popup |
| `stickpile-desktop toggle` | start and show | toggle the popup |
| `stickpile-desktop show` | start and show | show (re-detect the focused window) |
| `stickpile-desktop hide` | start hidden | hide |
| `stickpile-desktop --hidden` | start hidden | nothing |
| `stickpile-desktop quit` | nothing | quit (the clipboard content it owns goes away) |
| `--config FILE`, `--demo DIR` | used | ignored (the running instance keeps its own) |

"Toggle" hides the popup only if it is visible *and focused*; if it is
visible behind another window, it is brought back for the newly focused
app. With the default `ui.start_hidden = false`, the very first press of a
key bound to the bare command already shows the popup, whether or not the
resident instance was autostarted (autostart passes `--hidden`).

## Why it stays resident

On Wayland the clipboard has no storage of its own: the application that
copied serves the data to whoever pastes, for as long as it is running.
When it exits, the content is gone (unless a clipboard manager such as
`wl-clip-persist` grabbed a copy). Setting the selection also requires the
keyboard focus: the compositor only accepts it from the focused window,
with the serial of a recent input event.

So the client is a single resident process:

- It is a unique `GApplication` on D-Bus (`io.github.dolfost.Stickpile`).
  Running `stickpile-desktop` again does not start a second client: it
  forwards its arguments (and environment) to the running one and exits.
  That is how the global shortcut toggles the popup, on every compositor.
- When Enter is pressed, the item is prepared (variant chosen and
  downloaded into the cache) and put on the clipboard *while the popup
  still has focus*, then the popup hides. The process keeps owning the
  clipboard until something else is copied.
- If the popup loses focus before the download finishes (you clicked
  elsewhere), the copy is refused with "the popup lost focus … press Enter
  again" instead of silently failing.

The same holds for X11 (the selection owner must stay alive), so the
design is identical there. See `docs/clipboard.md` for the measurements.

## Binding the shortcut

The popup must be started from a key binding of the compositor or desktop.
The focused window is read when the command runs, *before* the popup
appears, so it sees the chat app.

### Hyprland

Classic `hyprland.conf` syntax (the plan's suggestion):

```ini
exec-once = stickpile-desktop --hidden          # or the systemd unit
bind = SUPER, period, exec, stickpile-desktop
windowrule = float, class:^(io\.github\.dolfost\.Stickpile)$
windowrule = center, class:^(io\.github\.dolfost\.Stickpile)$
```

(The window-rule syntax changed across Hyprland releases; on newer
versions write the equivalent `match:class` form.)

Lua config (`hyprland.lua`, in the style of the owner's dotfiles). Note
that in those dotfiles `SUPER + period` is already the brightness key, so
pick a free chord such as `SUPER + semicolon`:

```lua
-- hypr/autostart.lua (or enable the systemd user unit instead)
exec_cmd_uwsm('stickpile-desktop --hidden')

-- hypr/input.lua
hl.bind(l..'semicolon', hl.dsp.exec_cmd('stickpile-desktop'))

-- hypr/rules.lua: the popup floats centred at its own size
hl.window_rule {
	name = 'stickpile-popup',
	match = { class = '^io\\.github\\.dolfost\\.Stickpile$' },
	float = true,
	center = true,
}
```

The popup sizes itself from `ui.columns` × 4 rows of `ui.thumb_px`
(888×676 with the defaults); add a `size` rule to override.

### Sway

```
exec stickpile-desktop --hidden
bindsym $mod+period exec stickpile-desktop
for_window [app_id="io.github.dolfost.Stickpile"] floating enable, move position center
```

### GNOME

Settings → Keyboard → View and Customise Shortcuts → Custom Shortcuts →
add `stickpile-desktop` as the command. Start the resident instance with
the systemd unit or an autostart entry (`~/.config/autostart/`, with
`Exec=stickpile-desktop --hidden`). GNOME has no API to read the focused
window, so the target falls back to `default_target` (switch with Ctrl+T).

### KDE Plasma

System Settings → Keyboard → Shortcuts → Add New → Command or Script →
`stickpile-desktop`. Autostart with System Settings → Autostart (add
`stickpile-desktop --hidden`) or the systemd unit. Focus detection is not
available on KDE Wayland (same fallback as GNOME); on KDE X11 it works.

## Keys

The popup has seven tabs, numbered in this order: 1 Search, 2 Recent,
3 Most used, 4 Packs, 5 Favorites, 6 Me, 7 Settings. Every opening starts
on `ui.start_tab` (Search by default).

Keys work vim-style, in two modes:

- **Insert**: the text field has the keys. Search and Packs open in
  Insert, so typing starts right away.
- **Normal**: single keys act on the grid. Recent, Most used, Favorites,
  Me and Settings open in Normal; Escape leaves Insert for Normal.

The footer shows the mode and, like vim's showcmd, the keys of a command
still being typed (`3`, `g`, `12g`). `?` (or F1) shows all keys over the
popup.

### Insert mode

| Key | Action |
|---|---|
| typing | search (after a 120 ms pause; an empty query shows recently used items). On Packs: filter the packs by name, or inside a pack its items by name, keyword or emoji |
| arrow keys | move in the grid |
| Home / End | first / last item |
| Page Up / Page Down | one screen up / down |
| Enter | copy the selected item for the current target, hide; in the pack list, open the pack |
| Shift+Enter | copy with sticker mode flipped for this copy (Telegram: send as a sticker) |
| Ctrl+D | add the selected item to favorites, or remove it |
| Escape | Normal mode |
| Ctrl+Left/Right, Shift+Home/End | edit the query (word jump, select) |

### Normal mode

| Key | Action |
|---|---|
| h j k l, arrow keys | move left, down, up, right |
| 3j, 2l | move with a count |
| gg / G | first / last item (`5G` or `5gg`: the fifth) |
| 0 / $ | start / end of the row |
| Ctrl+d / Ctrl+u | half a screen down / up |
| y, Enter | copy the selected item for the current target, hide |
| Y, Shift+Enter | copy with sticker mode flipped for this copy |
| f | add the selected item to favorites, or remove it |
| gx | open the selected item in the browser |
| t / T | next / previous target app |
| s | toggle sticker mode |
| / | clear the text field and type (from Recent, Most used, Favorites, Me and Settings: a new search) |
| i, a | back to the text field, text kept (from Recent, Most used and Favorites: to Search) |
| 1 to 7 | go to that tab |
| gt / gT | next / previous tab (`3gt`: tab 3) |
| ? | show or hide the key help |
| q, Escape | hide; Escape first cancels a half-typed command, closes the help, or leaves an opened pack |

A digit is both a tab number and the start of a count: `3j` moves three
rows down, while a lone `3` switches to tab 3 after a short pause
(400 ms), or at once when the next key is not a motion. On Me and
Settings, which have nothing to count, digits switch at once. Unknown
keys do nothing.

**Packs.** The Packs tab lists your packs. Enter, or `o` in Normal mode,
opens the selected one: its items fill the grid, the header reads
"Packs / <name>", and the text field filters the items. `-`, Backspace or
Escape in Normal mode go back to the list, with the pack you came from
selected and the list filter as it was.

**Me and Settings.** In Normal mode `j` / `k` (or the arrow keys) move
between rows, Enter or Space presses a button or flips a switch, and `i`
edits the row's text field. Editing a field (or clicking into one) is
Insert mode; Escape leaves it.

### In every mode

| Key | Action |
|---|---|
| Tab / Shift+Tab | next / previous tab, wrapping |
| Alt+1 to Alt+7 | go to that tab |
| F1 | show or hide the key help |
| Ctrl+T / Ctrl+Shift+T | next / previous target app |
| Ctrl+S | toggle sticker mode; kept until stickpile-desktop restarts (the footer toggle does the same) |

In a text field on Me or Settings only Escape, the tab keys and F1 are
taken by the popup; every other key goes to the field.

The Normal-mode keys and the Ctrl shortcuts are read by physical key, so
they also work while a Cyrillic (or other non-Latin) layout is active, as
long as the first configured layout is Latin.

**Without modes.** With `ui.modal = false` the text field always has the
keys and there is no Normal mode: the Insert-mode table above applies on
every tab, Escape hides (inside an opened pack it first goes back to the
list), and Tab / Shift+Tab, Alt+1 to Alt+7, F1 and the Ctrl shortcuts
work as listed.

Mouse: hovering selects, a click copies (in the pack list: opens the
pack), and dragging an item drops the file itself into the target (useful
for Telegram stickers and when an app ignores pasted file references).
The search syntax of the server works in
the query: `e:😂`, `set:cats`, `type:gif`, `vibe:tired`, `q>70`.

The footer shows the target app and how it was chosen (`auto: <window
class>`, `default`, `chosen`), the sticker toggle (only for apps with
stickers; clicking it is Ctrl+S), and the status: searching, offline,
"Copied cat for Telegram", warnings such as "Discord limit is 10 MiB", or
errors. A ⚠ on a cell means the item cannot be sent to the current target
as is (the tooltip says why); copying still uses the best available
variant.

**Sticker mode (Telegram).** Telegram sends a pasted still as a
compressed photo, a JPEG, which fills transparent areas with white. In
sticker mode the popup copies the item's 512 px WebP as a file instead.
Telegram makes a sticker of it, transparency kept, only when it is sent as
a document: tick "Send as a document" in Telegram's send box once
(Telegram remembers it; other pasted images then go as files too), or drag
the item from the popup onto Telegram's "send without compression" area.
Sent compressed, the WebP becomes a photo on white as well. This is read
from Telegram Desktop's source and not yet observed (row T8 in
[clipboard.md](clipboard.md#21-telegram-desktop--ayugram-qt-6)). Sticker
mode is off by default; `prefer_stickers = true` turns it on when
stickpile-desktop starts, and Ctrl+S or the footer toggle changes it until
the process restarts.

## Target detection

When the popup opens, the client reads the focused window:

| Session | Source |
|---|---|
| Hyprland | `$XDG_RUNTIME_DIR/hypr/$HYPRLAND_INSTANCE_SIGNATURE/.socket.sock`, `j/activewindow` (class, title) |
| Sway / i3-ipc | `$SWAYSOCK`, `GET_TREE`, focused node's `app_id` or X11 class |
| X11 | `_NET_ACTIVE_WINDOW` → `WM_CLASS`, `_NET_WM_NAME` |
| GNOME, KDE Wayland, others | not available: `default_target` |

The environment of the invoking command is used, so a long-running
instance follows a restarted Hyprland session. The class and title are
matched against `focus.rules` (first match wins, case-insensitive
substring of class *or* title). The defaults map Telegram Desktop and its
forks (AyuGram, `com.ayugram.desktop`, 64Gram, Kotatogram, Materialgram) to
Telegram, Discord clients (Vesktop, WebCord, Legcord) to Discord, plus
Signal, WhatsApp clients and Slack. Browser chats match by window title
(`Telegram Web — Mozilla Firefox` → Telegram).

To find a window's class on Hyprland, focus it and run
`hyprctl -j activewindow` (look at `class`). Rules in `config.toml`
replace the default list, so repeat the defaults you want to keep:

```toml
default_target = "generic"   # when nothing matches
prefer_stickers = false      # Telegram: copy the sticker rendition by default

[focus]
detect = true

[[focus.rules]]
contains = ["telegram", "ayugram"]
target = "telegram"

[[focus.rules]]
contains = ["vesktop", "discord"]
target = "discord"

[[focus.rules]]
contains = ["element"]       # treat Element like a browser chat
target = "generic"
```

Targets: `telegram`, `discord`, `signal`, `whatsapp`, `slack`, `generic`.
The target decides which variant is copied (e.g. the size-limited GIF for
Discord, the 512 px WebP for Telegram stickers), the "cannot be sent"
warnings, and the clipboard offers (`[clipboard.targets.<name>]`, see
`docs/clipboard.md`).

**Per-window history.** With an empty query the popup lists the recently
used items, and the ones last used in the focused window class come
first: AyuGram opens on what you send in Telegram, Vesktop on what you send
in Discord. This is stored in `$XDG_CACHE_HOME/stickpile/desktop-window-history.json`
(delete it to reset).

## Popup settings

```toml
[ui]
columns = 6          # grid columns
thumb_px = 128       # thumbnail cell size (logical px)
hide_on_copy = true  # hide after copying / dropping
start_hidden = false # true: a bare first `stickpile-desktop` starts resident without showing
recent_count = 24    # recently used items shown before typing
max_results = 60
```

## Offline behaviour

- Search falls back to the local cache when the server does not answer
  within `timeout_secs`: metadata-only ranking (no image-content matches),
  and the footer says "Offline — showing cached results".
- Thumbnails and recently used files are cached under
  `$XDG_CACHE_HOME/stickpile` up to `cache_max_mb`. An item whose file is
  cached copies offline; others fail with "Server unreachable and the item
  is not cached".
- Copies are recorded locally and uploaded as usage statistics when the
  server is reachable again. The client syncs in the background every 10
  minutes and when the popup opens after a longer pause (failures retry
  after a minute).

## Demo mode

`stickpile-desktop --demo DIR` serves the media files of `DIR` (png, jpg,
webp, gif, mp4, webm) instead of a server: substring search over file
names, the same clipboard offers as the real client, drag-and-drop, and a
512 px WebP counts as a Telegram sticker. Useful to try the popup and the
paste matrix without a server. Quit a running instance first, since the
running instance ignores `--demo`.

## Troubleshooting

- **Nothing happens on the shortcut.** Run `stickpile-desktop show` in a
  terminal and read the output. Logs: `RUST_LOG=stickpile_desktop=debug`,
  or `journalctl --user -u stickpile-desktop` with the unit. Without a
  D-Bus session bus every invocation is its own instance and the popup
  cannot be toggled.
- **The popup opens tiled / full-size.** Add the float + center window
  rules above.
- **"Not logged in" page.** Shown when the server rejected a request
  (no token and no tailnet identity, or a revoked token). Run
  `stickpile login`; the next time the popup opens it re-reads the token,
  no restart needed. The page shows the server URL and config path in use.
- **"stickpile cannot start" page.** The config file failed to parse (the
  error says where), or the cache could not be opened.
- **Wrong target app.** Check the window class with `hyprctl -j
  activewindow` and add a `[[focus.rules]]` entry. Ctrl+T overrides it for
  one popup session.
- **Paste inserts a file path or nothing.** The app ignores the offered
  types: see `docs/clipboard.md` and adjust `[clipboard.targets.<app>]`,
  or drag the item from the popup instead.
- **"the popup lost focus before the item was ready".** Wayland only
  accepts the clipboard from the focused window; keep the popup focused
  until the copy finishes (large uncached items take a moment).
- **The clipboard empties after `stickpile-desktop quit` or a crash.**
  Expected on Wayland: the resident process owns the content. Use the
  systemd unit (it restarts on failure) or a clipboard manager.
- **Thumbnails missing.** They are downloaded on demand; offline, only
  cached ones show. They are retried each time the popup opens.
