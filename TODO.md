# TODO

v1 (server, web UI, CLI, Linux desktop client, Nix and container packaging)
is implemented. This list holds what is left: release chores, checks that
need a person and real chat apps, and the work planned after v1. The
architecture and phase plan are in [docs/plan.md](docs/plan.md).

## Before the first release

- [ ] Turn on private vulnerability reporting (Settings → Code security).
      [SECURITY.md](SECURITY.md) sends reporters there, and it is off.
- [ ] Turn on Dependabot alerts and secret scanning.
- [x] Tag the first release, `v0.2.0` (the tag must match the workspace
      version). A failed release can be rebuilt for the same tag with
      `workflow_dispatch`.
- [x] Make the `ghcr.io/dolfost/stickpile` package public once the first
      image is pushed.

## Verify by hand

Pasting into chat apps cannot be automated. Until these rows have
results, the clients run on the provisional defaults in
[docs/clipboard.md](docs/clipboard.md#3-defaults).

- [ ] **Telegram stickers (T8).** Sticker mode (Ctrl+S, the footer toggle
      or Shift+Enter) copies the 512 px WebP as a file reference. Telegram
      Desktop's source says it becomes a sticker only when "Send as a
      document" is ticked in the send box (or when dropped on "send without
      compression"); compressed, it is a photo on white. Confirm both, and
      the drag from the popup ([plan, question 1](docs/plan.md#open-questions-and-risks)).
- [ ] **Discord / Vesktop** (D rows).
- [ ] **Browser chats** in Firefox and Chromium (B rows).
- [ ] **Wayland persistence** with and without a clipboard manager (P1–P6).
- [ ] **X11**: the X11 column of every table.
- [ ] **Flatpak and Snap builds** of the chat apps: do file references from
      `~/.cache` work, or only the portal file types?
- [ ] Fill in the results, replace the "expected (UNVERIFIED)" columns and
      adjust `ClipboardOffers::default` where an app disagrees.
- [ ] **Tailscale Services.** Does `tailscale serve --service=svc:<name>`
      forward `Tailscale-User-Login`
      ([plan, question 2](docs/plan.md#open-questions-and-risks))? If not,
      document that users behind a Service log in with local accounts or
      tokens.
- [ ] Deploy on aorus from [deploy/aorus](deploy/aorus) and
      [deploy/quadlet](deploy/quadlet), and correct the deployment docs with
      whatever differs.

## After v1

### Android (phase 6)

- [x] Kotlin app over `stickpile-core` through UniFFI
      (`crates/stickpile-android`, `android/`): onboarding with QR pairing,
      the library, settings, a picker keyboard, a content provider for
      other keyboards, a share target, background sync
      ([docs/android.md](docs/android.md)). It has its own touch-first
      bridge instead of the desktop popup's `stickpile-quick` model.
- [x] Pairing by QR code: `POST /api/v1/pairings`, `POST /api/v1/auth/pair`,
      "Add a device" in the web settings, `stickpile login --code`.
- [x] APK in the CI and release workflows (unsigned); draft F-Droid recipe
      and listing (`android/fdroid/`, `fastlane/`).
- [ ] Run it: on an emulator, then on a phone; fill in the insert matrix
      ([docs/android-input.md](docs/android-input.md)).
- [ ] Release signing key; choose F-Droid's signing (its key, or
      reproducible builds with the owner's) before the first submission;
      submit after the three months F-Droid asks for.
- [ ] Logins on a new server address that switch servers only when the
      login works (pairing already does; a password or Tailscale login on a
      new address signs out of the old server first).
- [ ] A stickpile panel in a fork of Unexpected Keyboard (separate
      repository), over the provider contract.
- [ ] WhatsApp sticker packs from the app (WhatsApp's sticker provider).

### macOS (phase 7)

- [ ] SwiftUI shell over `stickpile-core` through UniFFI: a non-activating
      `NSPanel` popup, an `NSStatusItem`, a global hotkey and `NSPasteboard`
      offers.
- [ ] The macOS column of the clipboard matrix.

### Server and web UI

- [ ] Share links, so "Copy link" for animations and videos works for
      people who are not signed in ([docs/web.md](docs/web.md)).
- [ ] PostgreSQL behind the same store module (v1 is SQLite only). Search
      is independent of the database and needs no change.
- [ ] A learned quality component (NIMA on the CLIP image tower, or BRISQUE
      features with an SVR) as one more named part of the quality report
      ([docs/quality.md](docs/quality.md#future-learned-models)).

### Telegram

- [ ] Animated `.tgs` (Lottie) stickers: skipped on import and never
      exported. A stretch goal; agree on the approach before starting.
- [ ] A local Bot API server (`--local`), which returns absolute file paths
      instead of download URLs.
- [ ] Export of custom emoji and mask packs (export always creates regular
      sticker packs today).

### Desktop client

- [ ] Focused-window detection on GNOME and KDE Wayland. Both fall back to
      `default_target` today ([docs/desktop.md](docs/desktop.md)); this
      would need a GNOME Shell extension and a KWin script or D-Bus call.
- [ ] Optional global shortcut through the XDG GlobalShortcuts portal, as an
      alternative to a compositor key binding.

### Repository

- [ ] A script that regenerates [docs/screenshots](docs/screenshots) from a
      seeded demo server, so the README images can follow UI changes. The
      current images were made with scripts that are not in the repository.
