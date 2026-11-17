# Keeping stickers offline

Design note for the offline round (branch `offline`, from `v0.8.0`). The
CLI, the GTK popup, the macOS app, the Android app and its keyboard keep
chosen stickers on the device so they copy, insert and play with the server
unreachable. The web UI is out of scope (no client cache). This page is
rewritten into the final documentation when the work lands.

## Facts this design rests on

- The server's file route (`GET /api/v1/items/{id}/file/{variant}`) has no
  rate limit: the keyed limiters in `security.rs` cover login, upload, AI
  suggestions, rechecks and webhooks only. The downloader still bounds its
  concurrency (4 at once) and obeys a `429` with `Retry-After` if a proxy
  sends one.
- Android keeps its cache in `noBackupFilesDir/cache` (`StickpileApp`),
  which the system never empties (unlike `cacheDir`). Kept files stay
  there; existing users' caches do not move.
- The Settings schema (`settings.rs`: Bool, Int, Choice, Text, in groups)
  expresses the options; the GTK page (`ui/settings.rs`) and the macOS view
  (`settings_page.rs`) already add a non-schema block (warnings), so the
  usage/progress block is additive.
- Measured on a demo library: thumbnails 3-10 KB, Telegram renditions
  ~100-170 KB, originals ~1.9 MB on average and PNG renditions ~7.7 MB (up
  to 17 MB). What a kept sticker costs depends mostly on the target app.

## The cache (rule: one store, additive schema)

`SCHEMA_VERSION` stays 1; everything is additive, so 0.8.0 still opens the
cache.

- `files` gains `kept INTEGER NOT NULL DEFAULT 0` (ALTER TABLE, run when the
  column is missing). `evict` deletes only `kept = 0`; `clear_files`
  ("Clear cache") deletes only `kept = 0`; the confirmed "Remove kept
  files" deletes all files and forgets every keep. The flag is recomputed
  from the plan on every derivation (set for wanted files present, cleared
  for the rest), so what a 0.8.0 client does (it evicts or re-registers a
  kept file without the flag) heals on the next run: the file is wanted,
  missing or unflagged, and is fetched or flagged again.
- `offline_keeps (scope TEXT, id TEXT, created_at INTEGER, PRIMARY KEY
  (scope, id))` with scope `item`, `set` or `library` (id empty): the
  explicit choices. Automatic rules are not stored: they come from the
  options each time.
- `offline_failures (path TEXT PRIMARY KEY, item_id TEXT, error TEXT,
  attempts INTEGER, at INTEGER)`: what failed and why, for status.
- `offline_claims (path TEXT PRIMARY KEY, owner TEXT, at INTEGER)`: a
  process claims a file before downloading it; another process skips a
  fresh claim (stale after 10 minutes), so the CLI and the popup never
  fetch the same file twice. Files arrive as `<path>.part.<owner>` and are
  renamed into place after the size (and the sha256, when the server sends
  the `ETag`) check out.
- Meta key `offline_format` = 1 for later changes (precedent:
  `ITEM_FORMAT`).
- `Cache::reset_account_data` (sign-out, another server) deletes the kept
  files and empties the three tables.

## Options (`[offline]` in the client config, mirrored in home-manager)

| Key | Default | Meaning |
|---|---|---|
| `offline.targets` | `[]` | Chat apps whose renditions are kept; empty: the default target plus the targets in the local copy history |
| `offline.sticker_renditions` | `true` | Also keep the rendition a copy with the sticker flag uses |
| `offline.stills` / `.animations` / `.videos` | `true` / `true` / `false` | Kinds downloaded by pack, library and automatic keeping |
| `offline.max_file_mb` | `0` | Skip a single file larger than this (0: no limit); the budget and the kind switches already bound the total |
| `offline.max_mb` | `1024` | Budget for kept files (0: no limit): ~10,000 stickers for Telegram, a few hundred for targets that copy originals |
| `offline.auto.thumbnails` | `true` | After a sync, fetch every item's thumbnail |
| `offline.auto.used` | `false` | Keep the most recent `offline.auto.used_count` (200) copied stickers |
| `offline.auto.liked` | `false` | Keep liked stickers |
| `offline.auto.own_packs` | `false` | Keep the packs the user owns |
| `offline.mobile_data` | `false` | Android only: also download on metered networks |

A sticker kept by itself is always downloaded, whatever the kind switches
and `max_file_mb` say. A skipped item is reported as skipped with its
reason. Android stores these options next to its display settings and
passes them to the core as an `OfflineOptions` value; the desktop clients
read them from the config.

## Core API (`stickpile_core::offline`, used by every shell)

- `Scope { Item(id), Set(id), Library }`; `Reason` adds the automatic
  rules. `Session::offline_keep(scope)`, `offline_release(scope)`,
  `offline_keeps() -> Vec<Scope>`.
- `needs(item, opts, client) -> Vec<WantedFile>`: the one function that
  says which files a kept item needs (thumbnail; for each target the
  renditions `pick_variant` chooses, with and without the sticker flag; the
  played rendition on clients that play animations; the pack cover).
- `Session::offline_plan(opts) -> Plan`: (keeps and automatic rules) x
  (cached items) x `needs`, in download order (thumbnails, kept stickers,
  packs, library; most used first within a group), with present/missing,
  skipped with reasons, and byte totals.
- `Session::offline_estimate(scope, opts) -> Estimate { files, bytes,
  present_files, present_bytes, skipped }`, shown before keeping a pack or
  the library.
- `Session::offline_run(opts, control) -> OfflineOutcome` drives the
  downloader to completion or until paused, cancelled, the server is
  unreachable, the budget is reached or the disk is full;
  `OfflineControl` pauses, resumes and cancels; a copy's own download goes
  first. `OfflineStatus { state: Idle | Downloading | Paused |
  WaitingForNetwork | LimitReached | DiskFull, files_done, files_total,
  bytes_done, bytes_total, skipped, failed, scopes: Vec<ScopeProgress> }`
  is published while it runs.
- `Session::offline_usage() -> Usage { kept_files, kept_bytes,
  other_files, other_bytes }`; `Session::available_offline(items, target)`
  for the tile marks.
- Offline, `prepare_copy` of a sticker whose file is not on the device
  fails with `CoreError::NotOnDevice` (the shells explain it instead of a
  network error).

## Clients

As in the brief, section 4.9: CLI `stickpile offline keep|release|status
[--dry-run]|sync` (exit codes 0 complete, 2 incomplete, 1 failed); GTK and
macOS: a key and menu entry for the sticker and the pack, an "Offline"
Settings group plus a usage/progress block with pause, cancel and "Remove
kept files", the downloader after each sync, the dimmed unavailable tiles
and the available mark (a tile mark like the others); Android app: keep and
release in the sticker menu and details and on the pack screen, a settings
section, a WorkManager job (unmetered unless `mobile_data`, not on low
storage); the keyboard: keep/release in the long-press menu and the marks.
