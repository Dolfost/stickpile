# Linux desktop client (`stickpile-desktop`)

A keyboard-first quick-search popup: press a shortcut in any chat app, type,
press Enter, and the popup pastes the item into the chat
([paste after a pick](#paste-after-a-pick); on Hyprland, Sway and X11).
GTK4 + libadwaita, resident in the background.

## Install and run

1. Configure the server and log in once (the token is stored in
   `$XDG_STATE_HOME/stickpile/token`, mode 0600), either on the popup's
   Me tab (6) or in a terminal:

   ```sh
   stickpile login            # see docs/cli.md; writes the token
   ```

   On a tailnet behind `tailscale serve` requests work without a token
   (the server knows you from your tailnet identity), so this step is
   optional there; "Log in with tailnet" on the Me tab, or `stickpile
   login`, still turns that identity into a token without a password.

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
  still has focus*, then the popup hides and, once the chat window has
  the focus back, presses the paste shortcut there
  ([below](#paste-after-a-pick)). The process keeps owning the
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

The popup sizes itself from `ui.columns` x 4 rows of `ui.thumb_px`, at
least 700 px wide so the tab row fits (888x688 with the defaults); add a
`size` rule to override.

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

## Tabs

The popup has seven tabs, numbered in this order. It opens on the tab it
was closed on, also after a restart; `ui.start_tab` names a fixed tab
instead. When the server has rejected the client (signed out), it opens on
Me, with the login form.

| Tab | Shows |
|---|---|
| 1 Search | search results as you type; an empty query lists the recently used items, the ones used in the focused app first |
| 2 Recent | the items recently copied on this device |
| 3 Most used | your most copied items (on every device; offline, this device's history) |
| 4 Packs | your packs, then other users' public packs: cover, name, item count. Enter opens one |
| 5 Favorites | the items you liked; `f` (Ctrl+D in Insert) likes or unlikes the selected item anywhere |
| 6 Me | who you are signed in as, with login and logout ([below](#me)) |
| 7 Settings | the everyday settings, saved and applied at once ([below](#settings)) |

From top to bottom the window has the tab row (click a tab, or press its
number; on the right the client's version, which opens the project page on
GitHub, and `?`, the key help), a header with the title ("Packs / Cats"
inside a pack, with a back arrow), the text field on Search and Packs
(an item count on the other grids) with a Syntax button next to it where
it searches (Search, and inside a pack), the line of
[suggestions](#search-suggestions) while a filter is typed, the grid or
page, and the footer. Its first line has the selected item's stats: name,
pack, format and size, how often it was copied, likes and quality ("cat ·
Cats · GIF 320×240 · copied 12 times · ♥ 3 · quality 87"; on the pack list
a pack's name, item count and owner); `zi` in Normal mode hides and shows
it (`ui.show_details`). Under it: the mode (INSERT / NORMAL), keys of a command still
being typed, the target app, the sticker and safe mode toggles and the
status. Every tab fetches its list again each time it is shown, so it is
never older than the last look.

The grid fills the window: `ui.columns` sets how many thumbnails a row has
at the default window size, and a wider or narrower window shows more or
fewer per row. Each tile shows, over the thumbnail, the item's emojis
(top left), its vibe and a heart if you liked it (top right) and its tags
(bottom). `zf`, `ze`, `zt` and `zv` in Normal mode turn the heart, the
emojis, the tags and the vibe on and off, and the Settings tab has a switch
for each (`ui.mark_favorites`, `ui.show_emojis`, `ui.show_tags`,
`ui.show_vibe`). The heart marks
the items the Favorites tab lists (fetched each time the popup opens) and
the ones you like or unlike since. `zx` (`ui.show_text`) adds the words in
the picture (the item's text, read by the server or written in the web
UI) as one line above the tags, its lines joined with " / "; it is off by
default, because the line covers the picture whose words it repeats.
Hovering a tile shows its name, pack, emojis and keywords and, after a
blank line, its text with its line breaks (the first 8 lines or 300
characters).

**Safe mode.** `S` in Normal mode (Ctrl+Shift+S in any mode, or a click on
"safe" in the footer) hides every item tagged `nsfw`: on every tab, inside
packs, and in search, where the query gets `-tag:nsfw` so the results
still fill the grid. Pressing it again shows them. The footer says
"safe: on" while it is on, and the status how many items it hid. The choice
is saved as `ui.safe_mode`, so it holds after a restart (unless a read-only
`config.toml` sets it). Tag the items in the web UI (Tags field: `nsfw`).
Pack covers on the pack list are not filtered.

### Search suggestions

While a filter is typed in the search field (`vibe:`, `tag:`, `-tag:`,
`set:`, `by:`, `type:`, `is:`, `sort:`, `e:`, or the short `v:`, `t:`, `s:`,
`b:`), a line under the field offers its
values: the vibes, tags, packs and owners of the items you can see (from
the offline cache), most used first, and the fixed words of `type:`, `is:`
and `sort:`. Inside a pack the pack's own values come first. They match
what is typed so far: its start first, then
anywhere in the value, then with a typo or two forgiven, so `vibe:sarcstic`
and `vibe:sacr` still offer `sarcastic`. The line pushes the grid down
instead of covering it, and goes away with the space after the value.
The values are read from the cache when the popup opens and again after
every sync that changed the cache, so the first opening after a start
offers them too once its sync is over (before, the cache is still
empty): a line already shown takes them in place, with the text as
typed.

`e:` (or `emoji:`) offers emojis, each followed by its English name,
smaller and dimmer (💀 skull): `e:` alone offers the emojis of your own
items (not those of other people's public items), most used first, and
letters narrow it to the emojis whose name or keyword starts so, those
your items use first, then others from the table, best match first
(`e:sk` offers 💀 skull, ☠️ skull and crossbones, 👲 person with
skullcap, ...; the order is [search.md](search.md#emoji-names)'s). Taking
one puts the emoji into the query in place of the name (`e:💀`). While a
name is typed nothing is highlighted: the search matches the name itself,
typos included, so `e:skull` alone finds the stickers with any skull
emoji. The line sets no font of its own, so fontconfig draws the emoji
with a colour emoji font (Noto Color Emoji, say) when one is installed.

The search does not wait for the value to be finished: a half-typed or
misspelled value is searched as its best match (the highlighted one), so
`vibe:sar` already shows the sarcastic stickers. The short keys go out
long (`v:sar` as `vibe:sarcastic`), so an older server understands them
too. Tab puts the highlighted
value into the field, and pressing it again goes on to the next one
(Shift+Tab: the previous one); a click on a value puts it in and finishes
the filter. Without a filter being typed, Tab switches tabs as before.

F2 (or `g/` in Normal mode, or the Syntax button next to the search
field) shows the whole search syntax over the popup, as `?` shows the
keys. It lists the short keys: `v:` is `vibe:`, `t:` is `tag:`, `s:` is
`set:`, `b:` is `by:` and `e:` is `emoji:` ([search.md](search.md#query-syntax)).

## Keys

Keys work vim-style, in two modes:

- **Insert**: the text field has the keys.
- **Normal**: single keys act on the grid. Escape leaves Insert for Normal;
  `i` or `/` go back to typing.

Switching tabs never starts typing: from Normal mode every tab is shown in
Normal, and Insert mode stays on only when the tab switched to has a text
field too (Search and Packs). The popup opens in Normal (`i` or `/` to
type). With `ui.start_mode = "insert"` it opens in Insert on Search and
Packs.

The footer shows the mode and, like vim's showcmd, the keys of a command
still being typed (`3`, `g`, `12g`, `z`). `?` (or F1) shows all keys over the
popup.

### Insert mode

| Key | Action |
|---|---|
| typing | search (after a 120 ms pause; an empty query shows recently used items). On Packs: filter the packs by name, or inside a pack search its items with the whole search syntax |
| Tab / Shift+Tab | while a filter is typed (`vibe:sa`): put in the next / previous [suggested value](#search-suggestions); otherwise the next / previous tab |
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
| 3j, 2l | move with a count (with `ui.counts` on, see below) |
| gg / G | first / last item (with counts, `5G` or `5gg`: the fifth) |
| 0 / $ | start / end of the row |
| Ctrl+d / Ctrl+u | half a screen down / up |
| y, Enter | copy the selected item for the current target, hide |
| Y, Shift+Enter | copy with sticker mode flipped for this copy |
| f | add the selected item to favorites, or remove it |
| gx | open the selected item in the browser |
| t / T | next / previous target app |
| s | toggle sticker mode |
| S | safe mode: hide or show the items tagged `nsfw` (saved like a setting) |
| zf / ze / zt / zv / zx | show or hide the favorite hearts / emojis / tags / vibe / text on the tiles (saved like a setting) |
| zi | show or hide the selected item's stats row above the footer (saved like a setting) |
| / | clear the text field and type (from Recent, Most used, Favorites, Me and Settings: a new search) |
| i, a | back to the text field, text kept (from Recent, Most used and Favorites: to Search) |
| 1 to 7 | go to that tab |
| gt / gT | next / previous tab (`3gt`: tab 3) |
| ? | show or hide the key help |
| g/ | show or hide the search syntax (as F2) |
| q, Escape | hide; Escape first cancels a half-typed command, closes the help, or leaves an opened pack |

A digit switches to its tab at once. With `ui.counts = true` digits also
start a count: `3j` moves three rows down and `5G` goes to the fifth item,
but then a lone `3` can only switch to tab 3 after a short pause (400 ms),
or at once when the next key is not a motion. On Me and Settings, which
have nothing to count, digits always switch at once. Unknown keys do
nothing.

**Packs.** The Packs tab lists your packs. Enter, or `o` in Normal mode,
opens the selected one: its items fill the grid, the header reads
"Packs / <name>", and the text field searches the pack's items with the
whole search syntax: `v:tired`, `type:gif`, `-t:nsfw`, `is:liked`, `q>70`,
`sort:copies`, and words as in the search (typos forgiven; a word also
finds the items with it anywhere in the name, a keyword, an emoji or a
tag). Filters alone keep the pack's order. This runs in the popup, over
the items the pack showed, so it works offline too
([search.md](search.md#inside-a-pack)); clearing the field shows the
whole pack again. `-`, Backspace or
Escape in Normal mode go back to the list, with the pack you came from
selected and the list filter as it was.

**Me and Settings.** In Normal mode `j` / `k` (or the arrow keys) move a
highlight between the rows and buttons, `h` / `l` change a choice or a
number by one step (and turn a switch off / on), Enter or Space presses a
button, flips a switch or opens a choice list (arrows and Enter pick, Escape
closes it), and `i` edits the row's text or number field. Editing a field
(or clicking into one) is Insert mode; Escape leaves it.

### In every mode

| Key | Action |
|---|---|
| Tab / Shift+Tab | next / previous tab, wrapping |
| Alt+1 to Alt+7 | go to that tab |
| F1 | show or hide the key help |
| F2 | show or hide the search syntax |
| Ctrl+T / Ctrl+Shift+T | next / previous target app |
| Ctrl+S | toggle sticker mode; kept until stickpile-desktop restarts (the footer toggle does the same) |
| Ctrl+Shift+S | safe mode on / off (as `S`) |

In a text field on Me or Settings only Escape, the tab keys, F1 and F2 are
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
the query: `e:😂`, `set:cats`, `type:gif`, `vibe:tired`, `q>70`,
`text:"no thoughts"` (the words in the picture), and the short keys
`v:tired`, `t:reaction`, `s:cats`, `b:alice`, `x:кіт`
([search.md](search.md#query-syntax)).

The footer shows the selected item's stats (see above), the target app and
how it was chosen (`auto: <window class>`, `default`, `chosen`), the
sticker toggle (dimmed for apps without stickers; clicking it is Ctrl+S),
the safe mode toggle (clicking it is `S`), "paste: by hand" where the
session cannot [paste for you](#paste-after-a-pick), and the status: searching, offline,
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

## Paste after a pick

After a pick (Enter, `y`, a click) the item is copied and the popup hides.
With `paste.auto` on, which is the default, the popup then pastes it too:
it waits until the window it was opened over has the keyboard focus again
(at most `paste.wait_ms`, 1 s), pauses `paste.delay_ms` (50 ms), checks the
focus once more and presses the paste shortcut in that window. If another
window has the focus by then (you clicked elsewhere, or the compositor
gave the focus to the window under the mouse), nothing is pressed: the
popup never pastes into a different window. The item stays on the
clipboard either way, so Ctrl+V by hand still works.

To know which window to wait for, it needs the same window identity as
[target detection](#target-detection), so it works in the same sessions:

| Session | How the shortcut is pressed |
|---|---|
| Hyprland | the `send_shortcut` dispatcher over Hyprland's IPC socket, aimed at the window's address; Lua (0.55+) and hyprlang configs |
| Sway | a virtual keyboard (`zwp_virtual_keyboard_v1`) with a small keymap of its own, so the real keyboard's layout does not matter |
| X11 | XTest; the window manager must set `_NET_ACTIVE_WINDOW` (most do) |
| GNOME, KDE Wayland, other compositors | not at all: the popup only copies, and the footer says "paste: by hand" |

Other wlroots compositors (river, labwc, Wayfire) have the virtual
keyboard but no way for the popup to read the focused window, so they
count as "other" here.

Nothing is pasted when the popup stays open after a copy
(`ui.hide_on_copy = false`), for drag and drop, or when no window had the
focus when the popup opened. Copies from `stickpile copy` never paste.

**The shortcut.** Ctrl+V, and Ctrl+Shift+V in terminals (kitty, foot,
Alacritty, WezTerm, Ghostty, Konsole, GNOME Terminal and Console, Ptyxis,
xterm, urxvt, Terminator, Tilix and others). `paste.rules` picks it by the
window class; unlike `focus.rules` the title is not looked at, since a
chat called "Kitty" must not get the terminal shortcut:

```toml
[paste]
auto = true            # paste after a pick
wait_ms = 1000         # longest wait for the window to get the focus back (0..5000)
delay_ms = 50          # pause before the key press (0..1000)
shortcut = "ctrl+v"    # for windows no rule matches

# First match wins: substrings of the window class, case-insensitive.
# Setting rules replaces the default list, which is:
[[paste.rules]]
contains = ["kitty", "foot", "alacritty", "wezterm", "ghostty", "konsole", "terminal",
            "org.gnome.console", "kgx", "ptyxis", "xterm", "rxvt", "terminator", "tilix",
            "terminology", "blackbox", "st-256color", "contour"]
shortcut = "ctrl+shift+v"
```

A shortcut is modifiers (`ctrl`, `shift`, `alt`, `super`) and one key (a
letter, a digit or `insert`) joined with `+`: `ctrl+v`, `ctrl+shift+v`,
`shift+insert`. `none` means no paste: a rule with `shortcut = "none"`
turns pasting off for those apps only.

Keyboard layouts: on Sway the virtual keyboard brings its own keymap, so
the key is a V whatever layout is active. On Hyprland the key is looked up
in the active layout and, when that has none (a Cyrillic layout is
active), sent by its keycode on a US keyboard. On X11 the key that has it
in the first layout is used.

**Turning it off.** Settings tab, Copying, "Paste after a pick", or
`paste.auto = false` in `config.toml`. The Settings tab also edits the
shortcut, the delay and the wait; `paste.rules` stays in `config.toml`.

**Status (2026-11-16).** Observed in a headless Sway 1.12 (foot got
Ctrl+Shift+V and pasted the copied file's path after a pick in the demo
popup; a GTK 4 entry got Ctrl+V) and in an X11 session (Xvfb with i3, a GTK
4 entry got Ctrl+V through XTest). Hyprland is checked against a fake IPC
socket only, with requests read from Hyprland 0.55's source; it has not
been run against a real Hyprland yet.

## Me

The Me tab shows who the popup is signed in as: name, server (the button
next to it opens the web UI), how it is signed in (a token stored on this
device, or the tailnet identity without a token), two-factor login, and
counts (copies in all and this week, likes given and received, packs,
items, storage, copies not yet uploaded). It is fetched again each time
the tab is shown.

Signed out, it shows a login form: username, password, then Enter (Enter
in the username field moves on to the password). An account with
two-factor login asks for the code in a third field. "Log in with tailnet"
appears when the server knows this device by its tailnet identity; it
stores a token without a password. Signed in with a token, "Log out"
revokes the token on the server and deletes it; signed in by tailnet
identity without a token, "Store a token" turns the identity into one. A
login or logout applies at once: no restart, and every tab fetches its
list again for the new account.

## Settings

The Settings tab edits the keys below plus the everyday ones from the top
of the file, grouped as Popup, Copying and Server: a switch per boolean,
a list per choice, a number field (with its range) per number, and a text
field for the server URL (saved on Enter or when the field loses the
focus). Every change is saved at once and applied to the running popup:
the grid and window size, modes and start tab, copy adjustments, server
URL and timeout (the connection is opened again; a new server URL also
drops the lists fetched from the old one). Only "Start hidden" waits for
the next start, as it only matters then. A value that cannot be saved (a
server URL that is not http or https) is rejected with a message and the
file stays as it was.

The arrow next to a changed value resets it: the key is removed from the
file, so the default (or the value `config.toml` sets) applies again.

A key in `config.toml` that this version does not know (a typo, or a key
a newer popup saved) is skipped with a warning, listed at the top of the
page; `stickpile config check` fails on it. Clients up to 0.7.0 refuse
such a key instead, with "unknown field": before going back to 0.7.0, or
when a 0.7.0 CLI shares the file, reset or remove the keys it does not
know (`ui.show_text`, which `zx` also saves, is newer than 0.7.0;
[configuration.md](configuration.md#older-clients-and-newer-keys)).

Where changes go depends on `config.toml`:

- writable (or missing): the Settings tab edits `config.toml` itself;
  comments, order and the other keys are kept, and nothing is locked;
- read-only, as the home-manager module makes it (a link into
  `/nix/store`): changes go to `settings.toml` next to it, which
  `config.toml` overrides. The keys `config.toml` sets are shown read-only
  with "Set by home-manager" ("Set in config.toml" when it is read-only
  for another reason). Problems in `settings.toml` (an unknown key, a bad
  value) are listed at the top of the page and the default is used.

The page says at the top which file it saves to.

## Popup settings

```toml
[ui]
columns = 6          # grid columns at the default window size; resizing reflows
thumb_px = 128       # thumbnail cell size (logical px)
hide_on_copy = true  # hide after copying / dropping
start_hidden = false # true: a bare first `stickpile-desktop` starts resident without showing
recent_count = 24    # recently used items shown before typing
max_results = 60
modal = true         # vim-style Insert/Normal modes; false keeps the plain keys
start_tab = "last"   # last | search | recent | most_used | packs | favorites | me | settings
start_mode = "normal" # normal | insert: the mode the popup opens in
mark_favorites = true # a heart on the items you liked (zf)
show_emojis = true   # an item's emojis on its tile (ze)
show_tags = true     # an item's tags on its tile (zt)
show_vibe = true     # an item's vibe on its tile (zv)
show_text = false    # the words in an item's picture on its tile, one line (zx)
show_details = true  # the selected item's stats above the footer (zi)
safe_mode = false    # hide items tagged nsfw (S)
counts = false       # digits start a count (3j, 5G); tab digits then wait
language = "auto"    # auto | en | uk: the popup's language (docs/languages.md)
theme = "system"     # system | dark | oled | white: the popup's colours; oled is dark on pure black

[copy]
square = "off"       # off | transparent | white: pad non-square stills and stickers to a square
background = "keep"  # keep | white: fill the transparent pixels of stills copied as images
```

`[copy]` changes what is copied, in the popup and in `stickpile copy`
alike: the image data and the file reference carry the same padded or
flattened image, and padded stickers stay 512 px WebP. Animations and
videos are left alone.

The [Settings tab](#settings) edits these keys and the everyday ones from
the top of the file (server URL, timeout, cache size, default target,
sticker preference, focus detection, [pasting](#paste-after-a-pick)).
Clipboard offer lists, focus rules and paste rules stay in `config.toml`. The layering rules are in
[configuration.md](configuration.md#settings-saved-from-the-popup).

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
512 px WebP counts as a Telegram sticker. Each subdirectory of `DIR` is a
pack (the output of `stickpile backup` works) and the loose files are one
more; favorites and most used start from a fixed choice, and the account is
a demo user that logs in and out without a server (any username works).
`[copy]` padding and background apply as in the real client; the adjusted
files are written to `demo/` in the cache directory. `gx` opens the file
itself, since there is no web UI. Useful to try the popup
and the paste matrix without a server. Quit a running instance first, since
the running instance ignores `--demo`.

## Troubleshooting

- **Nothing happens on the shortcut.** Run `stickpile-desktop show` in a
  terminal and read the output. Logs: `RUST_LOG=stickpile_desktop=debug`,
  or `journalctl --user -u stickpile-desktop` with the unit. Without a
  D-Bus session bus every invocation is its own instance and the popup
  cannot be toggled.
- **The popup opens tiled / full-size.** Add the float + center window
  rules above.
- **"Not logged in; log in on the Me tab (6)".** The server rejected a
  request (no token and no tailnet identity, or a revoked token). Once
  that happened, the popup opens on the Me tab with the login form. Log in
  there, or run `stickpile login`: the next time the popup opens it
  re-reads the token, no restart needed.
- **"stickpile cannot start" page.** The config file failed to parse (the
  error says where), or the cache could not be opened.
- **Wrong target app.** Check the window class with `hyprctl -j
  activewindow` and add a `[[focus.rules]]` entry. Ctrl+T overrides it for
  one popup session.
- **Nothing is pasted after a pick.** Check the footer: "paste: by hand"
  means this session cannot paste (GNOME, KDE Wayland). Otherwise run with
  `RUST_LOG=stickpile_desktop=debug`: the log says "pasted with ...", or
  why not ("did not get the focus back", "not pasting: ..."). If your
  compositor focuses another window when the popup closes, nothing is
  pasted by design; raise `paste.wait_ms` if the focus comes back slowly.
- **The wrong shortcut is pressed** (a terminal that wants Ctrl+Shift+V,
  an app that wants Shift+Insert). Add a `[[paste.rules]]` entry for its
  window class (`hyprctl -j activewindow`).
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
