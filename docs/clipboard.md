# Clipboard matrix (phase 0)

Pasting into Telegram and Discord is the main feature, and it cannot be
automated. This document holds three things:

1. how to run the phase 0 spike (`spike/clipspike`);
2. the matrix of offer combinations × target apps × sessions, with an
   **expected (UNVERIFIED)** column to be replaced by what the spike shows;
3. the provisional clipboard defaults the clients ship with, how to change
   them without rebuilding, and the checklist to re-run before every release.

> **Status:** the matrix is not filled in yet. Everything in the "expected"
> columns is reasoning from how the apps are built (Qt for Telegram Desktop,
> Chromium/Electron for Discord, Gecko for Firefox), not observation. Treat
> it as a hypothesis until a row has a result.

## 1. The spike

`clipspike` is a small GTK4 app. It uses GDK's clipboard and drag source,
the same mechanism as `stickpile-desktop`, so a result in the spike is a
result for the real client.

```sh
cd /path/to/stickpile
nix develop -c cargo run -p clipspike                     # PNG fixture
nix develop -c cargo run -p clipspike -- --fixture gif    # png | webp | gif | mp4
nix develop -c cargo run -p clipspike -- ~/Pictures/cat.webp
nix develop -c cargo run -p clipspike -- --generate-fixtures   # write all four, print paths
```

Fixtures are generated into `$XDG_CACHE_HOME/clipspike/` (not `/tmp`,
which some apps cannot see): a 640×480 PNG with transparency, a 512×512
lossless WebP (a valid Telegram static sticker), a 12-frame looping GIF and,
if `ffmpeg` is available, a 2 s H.264 MP4 without audio.

The window has:

- **File**: open any file or generate a fixture. The type is sniffed from
  the bytes; the preview shows the first frame.
- **Offer these MIME types**: checkboxes for `image/png`, `image/gif`,
  `image/webp`, `text/uri-list`, `x-special/gnome-copied-files`,
  `text/plain`, offered in that order. Image types are offered when the file
  is that format; `image/png` is also produced by converting any decodable
  still or the first GIF frame. File references carry the absolute
  `file://` URI. An extra checkbox adds a `GdkFileList` value, for which
  GDK itself adds `text/uri-list` and the portal file-transfer types used by
  sandboxed (Flatpak/Snap) apps.
- **Preset**: one click per row of the matrix below, plus "provisional
  default" for the current file kind and for stickers.
- **Copy**: sets a `gdk::ContentProvider::new_union` of
  `ContentProvider::for_bytes`, one per ticked type.
- **Copy and quit after N s**: copies, then exits after N seconds. Paste
  after that to test whether the content outlives its owner (run it once
  with and once without a clipboard manager such as `wl-clip-persist`).
- **Inspect clipboard**: lists the MIME types currently on offer, e.g. to
  check what a clipboard manager re-offers after the owner exited.
- **Drag area**: drag it into a chat window. It offers a `GdkFileList`
  plus `text/uri-list` and `x-special/gnome-copied-files` bytes.
- **Record result**: target app, route (paste / paste after quit / drag /
  data-control), what happened (photo, sticker, file, animation, video,
  text pasted, nothing, error) and notes. **Record** appends a row to
  `clipboard-results.md` in the working directory (`--results FILE` to
  change). The session (Wayland/X11, compositor, running clipboard
  managers) is detected and written into every row.

### Data-control mode

`--wl-data-control` copies through the wlr/ext data-control protocol with
`wl-clipboard-rs`, like `wl-copy`: no window, no keyboard focus needed.
This is the comparison point for the GDK path and the route the CLI's
`stickpile copy` uses.

```sh
# serve until something else is copied (Ctrl+C to stop)
cargo run -p clipspike -- --fixture gif --wl-data-control --mime image/gif --mime text/uri-list
# own the clipboard for 5 s, then exit: paste afterwards to test persistence
cargo run -p clipspike -- cat.png --wl-data-control --quit-after 5
```

Without `--mime`, the provisional default for the file kind is offered.
Record data-control results in the GUI afterwards with route
"paste (wl-data-control)".

### Procedure

For each session (Hyprland first, then a real X11 login such as i3 or
Openbox; XWayland under Hyprland does not count) and each target app:

1. Start clipspike with the fixture for the row.
2. Apply the row's preset, press **Copy**, switch to the chat, paste
   (Ctrl+V), and if the app shows a confirmation box, note what it offers
   (photo / file / sticker) and send with the defaults.
3. Record the result. Repeat for each row, then for the drag route.
4. Persistence: "Copy and quit after 5 s", wait, paste; repeat with
   `wl-clip-persist --clipboard regular` running (and with `cliphist`'s
   `wl-paste --watch cliphist store`, which records history but does not
   re-own the selection).
5. Copy the recorded rows into the tables below and replace the
   "expected" column with the observed result.

## 2. The matrix

Legend for results: **photo** (compressed image), **sticker**, **file**
(document attachment), **animation** (Telegram GIF/MP4 animation or an
animated upload), **video**, **text** (the path or URI was inserted as
text), **nothing**, plus "box" when a confirmation dialog appears first.

Sessions: **Wayland** = Hyprland (owner's machine), **X11** = a real X11
session, **macOS** = `NSPasteboard` with file URL + image representations
(**post-v1**, filled in when the macOS client starts).

### 2.1 Telegram Desktop / AyuGram (Qt 6)

Telegram Desktop's paste and drop handler (`confirmSendingFiles`) reads
`QMimeData::urls()` first: when they are all local files it prepares them
as attachments and only falls back to the image data (`QMimeData::imageData`)
when there are no usable URLs. A `.webp` file of at most 512×512 sent as a
document is turned into a sticker (sticker attribute added at upload).
`QMimeData::urls()` reads `text/uri-list` only; Qt does not parse
`x-special/gnome-copied-files`.

| # | Offer combination (fixture) | Expected (UNVERIFIED) | Wayland (Hyprland) | X11 | macOS (post-v1) |
|---|---|---|---|---|---|
| T1 | PNG data (png) | box → photo | | | |
| T2 | PNG data + uri-list (png) | uri wins: box → photo (compressed by default; "send as file" possible) | | | |
| T3 | uri-list only (png) | box → photo, same as T2 | | | |
| T4 | GIF data (gif) | first frame only → photo (Qt decodes the image, animation lost), unless the build reads raw `image/gif` | | | |
| T5 | GIF data + uri-list (gif) | uri wins: box → animation | | | |
| T6 | uri-list only (gif) | box → animation | | | |
| T7 | MP4 uri-list (mp4) | box → video | | | |
| T8 | 512 px WebP uri-list (webp) | **sticker** (the brief's open question) | | | |
| T9 | 512 px WebP data + uri-list (webp) | uri wins → sticker | | | |
| T10 | gnome-copied-files only (png) | nothing (Qt ignores it) | | | |
| T11 | uri-list + gnome-copied-files (png) | same as T3 | | | |
| T12 | path as text/plain (png) | text: the path is inserted into the input | | | |
| T13 | drag: GdkFileList + uri-list (webp) | sticker | | | |
| T14 | drag: GdkFileList + uri-list (gif / mp4) | animation / video | | | |

### 2.2 Discord / Vesktop (Electron, Chromium)

Chromium on Linux exposes clipboard images to a page's `paste` event as a
`image.png` File (it reads `image/png`; other image types are not
guaranteed to be passed through). Recent Chromium versions also turn a
`text/uri-list` of local files into `clipboardData.files` (the
"clipboard filenames" support), and Discord uploads whatever is in
`clipboardData.files`. Which one wins when both are offered is unknown.

| # | Offer combination (fixture) | Expected (UNVERIFIED) | Wayland (Hyprland) | X11 | macOS (post-v1) |
|---|---|---|---|---|---|
| D1 | PNG data (png) | upload `image.png` | | | |
| D2 | PNG data + uri-list (png) | upload (image or the file; unknown which) | | | |
| D3 | uri-list only (png) | upload of the file, if the Electron version reads filenames; else nothing / text | | | |
| D4 | GIF data (gif) | nothing or a static PNG (Chromium does not hand `image/gif` to pages) | | | |
| D5 | GIF data + uri-list (gif) | animated GIF upload via the file reference | | | |
| D6 | uri-list only (gif) | animated GIF upload (same condition as D3) | | | |
| D7 | MP4 uri-list (mp4) | video upload (same condition as D3) | | | |
| D8 | 512 px WebP uri-list (webp) | file upload shown as an image (no stickers in Discord) | | | |
| D9 | gnome-copied-files only (png) | nothing | | | |
| D10 | uri-list + gnome-copied-files (png) | same as D3 | | | |
| D11 | path as text/plain (png) | text | | | |
| D12 | drag: GdkFileList + uri-list (gif) | animated GIF upload (drop of files works in Electron) | | | |

### 2.3 Browser chat: web.telegram.org or Element in Firefox

Firefox converts clipboard images into a PNG `File` for `paste` events
(animation is lost) and accepts dropped files. Pasting files from a
`text/uri-list` on the clipboard is supported only in recent Firefox
versions on Linux, if at all. web.telegram.org and Element both upload
what they get from `clipboardData.files` or a drop.

| # | Offer combination (fixture) | Expected (UNVERIFIED) | Wayland (Hyprland) | X11 | macOS (post-v1) |
|---|---|---|---|---|---|
| B1 | PNG data (png) | upload as image (web.telegram: photo; Element: image) | | | |
| B2 | PNG data + uri-list (png) | as B1 | | | |
| B3 | uri-list only (png) | nothing or text | | | |
| B4 | GIF data (gif) | static PNG (first frame) | | | |
| B5 | GIF data + uri-list (gif) | static PNG, or the file if Firefox reads filenames | | | |
| B6 | uri-list only (gif) | nothing or text | | | |
| B7 | MP4 uri-list (mp4) | nothing or text | | | |
| B8 | 512 px WebP uri-list (webp) | nothing or text | | | |
| B9 | gnome-copied-files variants (png) | nothing | | | |
| B10 | drag: GdkFileList + uri-list (gif / mp4 / webp) | upload of the file (web.telegram: GIF → animation, WebP → sticker?) | | | |

### 2.4 Wayland persistence

On Wayland the selection is served by the client that set it; when that
client exits, the selection is gone unless a clipboard manager copied the
data and re-offers it. X11 behaves the same unless a manager implements the
`CLIPBOARD_MANAGER` save protocol.

| # | Route | Clipboard manager | Expected (UNVERIFIED) | Wayland (Hyprland) | X11 |
|---|---|---|---|---|---|
| P1 | GDK copy, owner quits after 5 s | none | empty after exit | | |
| P2 | GDK copy, owner quits after 5 s | wl-clip-persist (regular) | persists (all offered types re-offered) | | |
| P3 | GDK copy, owner quits after 5 s | cliphist via `wl-paste --watch` only | empty (history only) | | |
| P4 | data-control, `--quit-after 5` | none | empty after exit | | |
| P5 | data-control, `--quit-after 5` | wl-clip-persist | persists | | |
| P6 | GDK copy, owner stays running (the desktop client's model) | any | persists until replaced | | |

The desktop client is resident (P6) by design, so persistence does not
depend on a clipboard manager. The CLI forks a background server like
`wl-copy` (P4-style, but the process lives until the selection is
replaced).

### 2.5 Questions this spike answers

From the brief (§0), answered by the rows above:

1. **Real Telegram sticker**: which path sends a 512 px WebP as a sticker
   rather than a photo or file? Paste of a file reference (T8, T9), drag
   (T13), or only "attach file"? (The plan's fallback is drag-and-drop
   from the popup.)
2. **File references as attachments**: is `text/uri-list` treated as an
   attachment by each app (T3, D3, B3)? Does `x-special/gnome-copied-files`
   matter anywhere (T10, D9, B9)?
3. **GIFs**: does `image/gif` data keep the animation (T4, D4, B4), or only
   the file reference (T5/T6, D5/D6)?
4. **Videos**: is an MP4 file reference uploaded as a video (T7, D7, B7)?
5. **Wayland persistence**: does the content survive the owner exiting,
   with and without a clipboard manager (P1–P5)? Resident process vs.
   wl-clipboard style background server.
6. **X11**: does anything differ from Wayland (each table's X11 column)?
7. **macOS** (post-v1): which `NSPasteboard` representations (file URL,
   `public.png`, `com.compuserve.gif`) do the same apps prefer?

Additional questions worth noting while testing:

- Does Telegram show a confirmation box on paste, and which option is the
  default? (Affects whether Enter-in-popup then Ctrl+V, Enter is the flow.)
- Does the offer order matter to any app, or does each app pick by its own
  preference?
- Does any app insert the `text/plain` path *in addition* to attaching?
- Flatpak/Snap builds: do file references from `~/.cache` work, or only
  the `GdkFileList` portal types?

## 3. Provisional defaults

Until the matrix is filled in, the clients offer these MIME types, in this
order (`stickpile-core`, `ClipboardOffers::default`):

| Item kind | Offers |
|---|---|
| still (png, jpeg, static webp) | `image/png`, `text/uri-list`, `x-special/gnome-copied-files` |
| animation (gif, animated webp) | `image/gif`, `text/uri-list`, `x-special/gnome-copied-files` |
| video (mp4, webm) | `text/uri-list`, `x-special/gnome-copied-files` |
| sticker (Shift+Enter / sticker mode, Telegram) | `text/uri-list`, `x-special/gnome-copied-files` |

Rationale: image data is the most widely accepted for stills; GIFs and
videos need the file reference to keep their motion; the sticker route
depends on Telegram receiving a 512 px WebP *file*. `image/*` offers carry
the bytes of the chosen variant (converted to PNG if needed); file
references point at the materialised file in the cache,
`$XDG_CACHE_HOME/stickpile/clip/<media>.<variant>/<item name>.<ext>`, so
apps that show the file name show the item's name. The desktop client
also offers `text/plain;charset=utf-8` whenever `text/plain` is configured.

### Changing the offers without rebuilding

The offers are plain configuration in `~/.config/stickpile/config.toml`
(or the home-manager module's `programs.stickpile.settings.clipboard`).
Restart the resident client after editing (`stickpile-desktop quit`, then
start it again, or `systemctl --user restart stickpile-desktop`).

```toml
# Used for every target without an override.
[clipboard.default]
still = ["image/png", "text/uri-list", "x-special/gnome-copied-files"]
animation = ["text/uri-list", "image/gif"]
video = ["text/uri-list"]
sticker = ["text/uri-list"]

# Per-target override (telegram, discord, signal, whatsapp, slack, generic).
# Set all four lists: a list missing here falls back to the built-in
# provisional default (table above), not to [clipboard.default].
[clipboard.targets.telegram]
still = ["image/png"]
animation = ["text/uri-list"]
video = ["text/uri-list"]
sticker = ["text/uri-list"]
```

Allowed entries: `image/png`, `image/gif`, `image/webp`, `image/jpeg`,
`text/uri-list`, `x-special/gnome-copied-files`, `text/plain`. Anything
else is rejected when the config is loaded. Which target a copy is for
comes from the focused window (see `docs/desktop.md`) or Ctrl+T in the
popup.

When the matrix is filled in, the results become the new defaults in
`ClipboardOffers::default` and per-target overrides in the defaults where
apps disagree, and this section is updated.

## 4. Pre-release checklist

Run before every release, on Wayland (Hyprland) and X11, with the real
desktop client (not the spike) against a test server. Use one still, one
GIF, one MP4 and one item with a 512 px WebP sticker variant. Record
pass/fail in the release PR.

Telegram Desktop / AyuGram (focus the chat, open the popup with the
shortcut, check the footer says "Telegram  auto: com.ayugram.desktop"):

- [ ] Still: Enter, Ctrl+V → sent as a photo.
- [ ] GIF: Enter, Ctrl+V → sent as an animation (moves).
- [ ] MP4: Enter, Ctrl+V → sent as a video.
- [ ] Sticker: Shift+Enter on the WebP item, Ctrl+V → sent as a sticker.
- [ ] Drag a GIF from the popup into the chat → animation.

Discord / Vesktop (footer: "Discord  auto: vesktop"):

- [ ] Still: Enter, Ctrl+V → image upload.
- [ ] GIF: Enter, Ctrl+V → animated GIF upload.
- [ ] MP4: Enter, Ctrl+V → video upload.
- [ ] An item over 10 MiB shows the size warning and still copies.

Browser chat in Firefox (web.telegram.org or Element; footer: target from
the window title or "Other / browser"):

- [ ] Still: Enter, Ctrl+V → image upload.
- [ ] GIF: drag from the popup → animated upload.

Clipboard lifetime and focus:

- [ ] Copy, wait 10 minutes, paste → still there (resident process).
- [ ] Copy, then copy text elsewhere, paste → the new text (no stale
      ownership).
- [ ] With the popup open, click another window, press the shortcut →
      popup comes back with that window's target.
- [ ] Offline: stop the server, open the popup → cached results with the
      "Offline" status; a cached item still copies.

Update the matrix above if any result changes (app updates do change paste
behaviour), and adjust the defaults in section 3.
