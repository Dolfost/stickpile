# stickpile — architecture and phase plan

This is deliverable 2 of the project brief: the architecture, the phase plan,
and the decisions and open questions behind them.

## Decisions taken

| Topic | Decision | Why |
|---|---|---|
| Phase 0 | Clients ship with provisional clipboard defaults. The spike tool and the `docs/clipboard.md` checklist are run by hand afterwards, and the offers can be tuned in config without a rebuild. | Nobody can automate pasting into Telegram or Discord. Making the offers configurable keeps the rework cheap. |
| Web framework | **axum 0.8** | It is tower-native (rate limiting, timeouts and headers come as layers), has first-class unix-socket serving for the `tailscale serve` path, has utoipa support, and is the most common choice in the Rust ecosystem. actix-web has its own runtime and middleware model and offers nothing we need on top. |
| Templates | **maud** + htmx (vendored, no JS toolchain) | Templates are type-checked at compile time. htmx fragments are ordinary Rust functions, so full pages and partial responses share code. There are no template files to keep in sync with handlers. |
| Database | **SQLite only in v1** (sqlx). Portable SQL: app-generated TEXT ids, integer timestamps, JSON-as-TEXT. | Your deployment uses SQLite. PostgreSQL is post-v1 behind the same store module. |
| Search | Own in-memory hybrid index (`stickpile-search`), not SQLite FTS5 | It does not depend on the database (PostgreSQL later needs no new search code). The offline client cache reuses the same ranking code. Fuzzy and squash matching (`naahh` → `nah`) and vector fusion live in one place. A personal library of up to about 100k items fits in RAM. |
| CLIP | ViT-B/32 image tower. The text encoder is selectable: `clip` (English) or `multilingual` (sentence-transformers clip-ViT-B-32-multilingual-v1). | Your answer. Both share the same image space, so switching only re-embeds text. |
| Media tools | Pure Rust for stills and GIF (image, gif, libwebp). ffmpeg/ffprobe only for video and animated renditions, always with fixed arguments on server-generated paths, `-protocol_whitelist file`, a timeout and no network. | §9 forbids user-controlled arguments, not ffmpeg itself. Video transcoding without ffmpeg is not realistic. |
| Re-encoding | PNG and GIF are re-encoded losslessly. JPEG and WebP are fully decoded (validated) and then rebuilt from whitelisted segments/chunks. Videos are remuxed with metadata dropped. | A lossy re-encode of a JPEG would add a compression generation, which defeats the quality score and "find a better version". Rebuilding from whitelisted segments still removes metadata, comments, trailing payloads and polyglots. |
| Tailscale auth | The server has a second listener on a **unix socket** (`tailscale serve unix:/state/tailscale.sock`). Identity headers are trusted only on that listener (or on TCP with a loopback peer plus a shared-secret header). | `tailscale serve` cannot add a secret header. A socket in the state directory is reachable only by root and the service user, which is stronger than "loopback", because with rootless podman every local user can reach loopback. |
| CSRF on the tailnet | Requests authenticated by the Tailscale identity header are **not** exempt from cross-site checks: unsafe methods must pass a same-origin check (`Origin` / `Sec-Fetch-Site`). They need no CSRF token. | The proxy attaches the identity to every request from the device, including one a malicious website triggers, so it is an ambient credential like a cookie. The CLI and curl send neither header and are unaffected. |
| Desktop toolkit | gtk4-rs + libadwaita directly. The popup's state machine lives in `stickpile-quick` with no toolkit dependency. | The state machine is testable without a display and reusable by the Android and macOS shells. Relm4 would add a macro layer without making anything easier to test. |
| Global shortcut | The resident GApplication is unique on D-Bus, so running `stickpile-desktop` again toggles the popup. Bind it in the compositor (Hyprland `bind = SUPER, period, exec, stickpile-desktop`). | This works on every compositor. The XDG GlobalShortcuts portal is optional and post-v1. |
| Target detection | Read the focused window (Hyprland IPC, Sway IPC, X11 `_NET_ACTIVE_WINDOW`) when the popup opens, and map its class with configurable rules (AyuGram → Telegram, Vesktop → Discord). | This drives the variant choice and the "cannot be sent to this app" warning. |
| macOS (post-v1) | **SwiftUI shell over `stickpile-core` via UniFFI** | A good quick-search popup on macOS needs a non-activating `NSPanel`, an `NSStatusItem`, `NSPasteboard` writes of several representations, and Carbon or `MASShortcut` hotkeys. These are all one-liners in Swift and awkward or impossible in iced, egui or slint, none of which look native. Tauri needs a JavaScript toolchain. |
| Git packs | libgit2 (`git2`) | gitoxide cannot push yet. |
| Git workflow | Local commits per phase, no push | Your answer. |

## Crates

```
stickpile-api       DTOs, REST paths, search syntax, target-app profiles   (no I/O)
stickpile-search    in-memory hybrid index                                  (server + clients)
stickpile-setfmt    set.toml, set directory, tar                            (server + CLI)
stickpile-media     sniff, sanitise, variants, pHash/dHash, quality score   (server)
stickpile-ai        Enricher trait, CLIP via ort, zero-shot vocabularies    (server)
stickpile-telegram  Bot API sticker import/export                           (server)
stickpile-gitpack   clone/pull/commit/push, webhook signatures              (server)
stickpile-server    axum app: auth, REST, web UI, jobs, stats, admin
stickpile-core      client lib: API client, token, cache, usage queue, clipboard offers, session facade
stickpile-quick     toolkit-free popup state machine + focused-window detection (api types only)
stickpile-cli       `stickpile` binary
stickpile-desktop   GTK4/libadwaita resident popup
spike/clipspike     phase 0 throwaway clipboard tool
```

## Data model (summary)

- **media**: content-addressed by the sha256 of the uploaded bytes. Holds the
  probe facts, pHash/dHash, quality report and embedding. Files live in
  `<data_dir>/media/<sha[0..2]>/<sha>/<variant>.<ext>`. Storage is shared
  across users and sets. That sharing is never observable: near-duplicate
  hints only consider the caller's own items.
- **items**: (set, media) with per-set metadata: name, keywords, emojis, vibe
  and tags, each with a `*_source` of `user` or `ai`. Each item also has a
  position and an export filename stem.
- **sets**: owner, slug, visibility (`private`, `unlisted`, `public`),
  recommended aspect and size, `version`, source (`local`, `git` or
  `telegram`), the git binding and the Telegram pack binding/mapping.
- **usage_events** (idempotent by client id), **ratings** (one per user per
  item), **jobs** (DB-backed queue), **audit_log**, **sessions**,
  **api_tokens**, **invitations**, and **item_tombstones** for client cache
  sync.

## Visibility rules

| Caller | own set | other's private | other's unlisted | other's public |
|---|---|---|---|---|
| read set / item / file by id | ✓ | ✗ (404) | ✓ | ✓ |
| listed in browse | ✓ | ✗ | ✗ | ✓ |
| search, client cache | ✓ | ✗ | ✗ | ✓ |
| edit | ✓ unless read-only git pack | ✗ | ✗ | ✗ |
| like, report usage | ✓ | ✗ | ✓ | ✓ |

Admins get no implicit content access, only the admin API. "Not found" and
"forbidden" both return 404 for content, so ids cannot be probed.

## Phases

| # | Phase | Contents | Done when |
|---|---|---|---|
| 0 | Clipboard spike | `spike/clipspike` (GTK app that offers chosen MIME combinations) and the `docs/clipboard.md` matrix plus release checklist | Tool builds; matrix filled by hand (pending) |
| 1 | Libraries | media, setfmt, gitpack, telegram, ai, search, each with unit tests | `cargo test` green per crate |
| 2 | Server | config, store, auth (sessions, tokens, argon2id, TOTP, tailscale), authorization, REST + OpenAPI, upload/ingest, jobs, import/export, git packs, Telegram, search, stats, ratings, admin API, security headers, CSRF, rate limits | API tests for every endpoint and access rule |
| 3 | Web UI | maud + htmx pages: login, sets, upload, inline editing, AI confirm/override, copy, popular, stats, admin | handler tests per page |
| 4 | Clients (**v1**) | `stickpile-core`, `stickpile` CLI, GTK desktop | core cache/queue/sync tests, quick-model tests, CLI e2e against a real server |
| 5 | Packaging | flake packages, NixOS + home-manager modules with TOML mirror check, container image, quadlets + aorus snippet, CI + release workflows | `nix flake check` green |
| 6 | post-v1 | Android (Kotlin + UniFFI), Unexpected Keyboard fork (separate repo), F-Droid metadata | — |
| 7 | post-v1 | macOS SwiftUI client | — |

## Open questions and risks

1. **Telegram stickers from the clipboard.** The plan assumes Telegram Desktop
   sends a real sticker only for a dropped or attached 512 px WebP file, and
   that a `text/uri-list` file reference on the clipboard counts as an
   attachment. Phase 0 must confirm this on AyuGram (Wayland). If file
   references paste as "send as file", the fallback is to offer drag-and-drop
   from the popup (GTK DragSource with a file list).
2. **Tailscale Services and identity headers.** aorus uses
   `tailscale serve --service=svc:<name>`. Whether Services forward
   `Tailscale-User-Login` must be verified. Without the headers, users fall
   back to local accounts and tokens; nothing else changes.
3. **Wayland clipboard lifetime.** The desktop client is resident, so this is
   handled by design. The CLI `copy` command forks a small server process,
   like `wl-copy`, that lives until the selection is replaced.
4. **Animated `.tgs` stickers** are skipped on Telegram import with a clear
   reason (stretch goal; ask before implementing).
5. **Registration policy** comes from config (Nix is the source of truth).
   The admin panel shows the policy and manages invitations but does not edit
   it.
6. **Model downloads.** The CLIP ONNX files are fetched by Nix as
   fixed-output derivations (`packages.models*`) or by
   `scripts/fetch-models.sh`. They are not vendored. The English pair is
   about 608 MB (fp32, because the quantized CLIP towers lost too much
   accuracy). The multilingual text encoder is a qint8 model plus a dense
   layer. Both together are about 745 MB. CLIP needs about 51 ms per image
   and 7 ms per text on 2 CPU threads, and about 740 MB of RAM.
