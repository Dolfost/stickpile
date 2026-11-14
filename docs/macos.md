# macOS app (`Stickpile.app`)

The same quick-search popup as the Linux client ([desktop.md](desktop.md)),
as a menu bar app: press the shortcut in any chat app, type, press Enter, and
paste. Tabs, keys, settings, target detection and the `config.toml` keys are
the Linux client's.

## Install

It needs macOS 13 or later, on Apple silicon or Intel.

**Homebrew.** This repository is a tap with a cask for the app:

```sh
brew tap dolfost/stickpile https://github.com/Dolfost/stickpile
brew install --cask stickpile
```

`brew upgrade --cask stickpile` updates it after a release; the release
workflow moves the cask (`Casks/stickpile.rb`) to every new version.
`brew uninstall --zap --cask stickpile` also removes the config, the token,
the cache and the log.

**By hand.** Download `Stickpile-vX.Y.Z-arm64-macos.zip` (Apple silicon) or
`...-x86_64-macos.zip` (Intel) from the
[releases](https://github.com/Dolfost/stickpile/releases), unzip it and move
`Stickpile.app` to `/Applications`.

The app is signed ad hoc, not notarized by Apple. The cask clears the
quarantine flag after installing, so it starts at once. A downloaded copy is
blocked on its first launch: open it once anyway (System Settings, Privacy &
Security, "Open Anyway" under the message about Stickpile), or, in a
terminal:

```sh
xattr -dr com.apple.quarantine /Applications/Stickpile.app
```

Stickpile has no Dock icon. Its menu bar icon opens the popup and has the
settings that only exist on macOS: the shortcut and "Launch at login".

## First start

1. Click the menu bar icon, then Settings..., and set **Server URL**.
2. Log in on the **Me** tab (6), or with the CLI: `stickpile login`. Both use
   the same token file.
3. Press the shortcut (**Option+Space** by default) in a chat app.

## Files

| | |
|---|---|
| config | `~/Library/Application Support/stickpile/config.toml` ([configuration.md](configuration.md)) |
| token | `~/Library/Application Support/stickpile/token` |
| cache | `~/Library/Caches/stickpile` |
| log | `~/Library/Logs/stickpile/stickpile.log` |

The CLI for macOS (in the same release, `stickpile-vX.Y.Z-arm64-macos.tar.gz`)
reads the same config and token.

## Keys

The Linux popup's keys ([desktop.md](desktop.md#keys)), with Cmd for Ctrl:
Cmd+T cycles the target app, Cmd+S flips sticker mode, Cmd+D likes the item,
and Cmd+1..7 switch tabs (Alt+1..7 on Linux). Control works as Ctrl too. The
vim keys of Normal mode work with a Cyrillic layout active.

## Target detection

When the shortcut is pressed, the app reads the frontmost application's
bundle id and name and matches them against `focus.rules`, like a window
class on Linux. The default rules already cover the Mac apps:

| app | bundle id | target |
|---|---|---|
| Telegram Desktop (direct download) | `com.tdesktop.Telegram` | Telegram |
| Telegram Desktop (Mac App Store, "Telegram Lite") | `org.telegram.desktop` | Telegram |
| Telegram for macOS (the Swift app) | `ru.keepcoder.Telegram` | Telegram |
| Discord | `com.hnc.Discord` | Discord |
| Signal, WhatsApp, Slack | `org.whispersystems.signal-desktop`, `net.whatsapp.WhatsApp`, `com.tinyspeck.slackmacgap` | Signal, WhatsApp, Slack |

The popup's footer shows the bundle id it matched ("auto: com.tdesktop.Telegram").

## What goes on the pasteboard

The `[clipboard]` offers of the config, the same MIME lists as on Linux, are
written as one pasteboard item:

| MIME type | pasteboard type |
|---|---|
| `image/png`, `image/gif`, `image/webp`, `image/jpeg` | `public.png`, `com.compuserve.gif`, `org.webmproject.webp`, `public.jpeg` |
| `text/uri-list` | `public.file-url` (the copied file in the cache, as Finder copies a file) |
| `text/plain` | `public.utf8-plain-text` |
| `x-special/gnome-copied-files` | left out (GNOME only) |
| `application/x-sticker`, `application/x-no-dialog` | `com.trolltech.anymime.application--x-sticker`, `...--x-no-dialog` (how Qt stores a MIME type, so a Qt app reads them back) |

What the apps do with it (from their source; still to be confirmed by hand,
see [clipboard.md](clipboard.md#6-macos)):

- **Telegram Desktop** (Qt) takes the file URL when there is one: a still is
  sent as a photo, a GIF as an animation, a video as a video. It only falls
  back to the image data when the file cannot be read. A WebP becomes a
  sticker only through "send as file".
- **Discord** (Chromium) also takes the file when there is one and uploads it
  as it is, so GIFs stay animated. Image data alone would be a still PNG.

## Build from source

On a Mac with the Command Line Tools (`xcode-select --install`) and Rust
([rustup](https://rustup.rs)):

```sh
crates/stickpile-mac/app/build.sh            # dist/Stickpile.app, dist/Stickpile.zip
TARGET=x86_64-apple-darwin crates/stickpile-mac/app/build.sh   # for Intel
```

The script builds the Rust side (`cargo build -p stickpile-mac`), generates
its Swift interface with UniFFI, compiles the Swift app with `swiftc` and
assembles and signs the bundle.

## How it works

The popup's behaviour is Rust, shared with the Linux client: the
`stickpile-quick` model (tabs, modes, keys, selection, target app) and the
display-free modules of `stickpile-desktop` (the backend over
`stickpile-core`, key mapping, the Me and Settings decisions). `stickpile-mac`
drives them the way the GTK shell does and offers them to Swift through
UniFFI (`crates/stickpile-mac/src/bridge.rs`): Swift draws a `ViewState` and
reports input, Rust answers with new states, thumbnails and pasteboard
contents. The Swift side (`crates/stickpile-mac/app/`) is the window, the
grid, the pages, the menu bar item, the global shortcut (Carbon
`RegisterEventHotKey`, no Accessibility permission) and the pasteboard.

The popup is a non-activating panel: the chat app stays the active app, so
after a copy the keyboard focus is back in its message box.
