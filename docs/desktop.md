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

The search field always has the focus; navigation keys are taken by the
grid, everything else edits the query.

| Key | Action |
|---|---|
| typing | search (after a 120 ms pause); empty query shows recently used items |
| ← → ↑ ↓ | move in the grid |
| Home / End | first / last item |
| Page Up / Page Down | one screen up / down |
| Enter | copy the selected item for the current target, hide |
| Shift+Enter | copy with sticker mode flipped for this copy (Telegram: send as a sticker) |
| Ctrl+T / Ctrl+Shift+T | next / previous target app |
| Ctrl+S | toggle sticker mode for this popup session |
| Escape | hide |
| Ctrl+←/→, Shift+Home/End | edit the query (word jump, select) |

Ctrl shortcuts also work while a Cyrillic (or other non-Latin) layout is
active, as long as the first configured layout is Latin.

Mouse: hovering selects, a click copies, and dragging an item drops the
file itself into the target (useful for Telegram stickers and when an app
ignores pasted file references). The search syntax of the server works in
the query: `e:😂`, `set:cats`, `type:gif`, `vibe:tired`, `q>70`.

The footer shows the target app and how it was chosen (`auto: <window
class>`, `default`, `chosen`), the sticker indicator (only for apps with
stickers), and the status: searching, offline, "Copied cat for Telegram",
warnings such as "Discord limit is 10 MiB", or errors. A ⚠ on a cell means
the item cannot be sent to the current target as is (the tooltip says
why); copying still uses the best available variant.

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
