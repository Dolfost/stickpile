# Keeping stickers offline

The CLI, the GTK popup, the macOS app, the Android app and its keyboard
keep chosen stickers on the device, so that they copy, insert and play with
the server unreachable. The user keeps a sticker, a pack or the whole
library ("Keep offline"); automatic rules keep more; a downloader fetches
the files in the background. The web UI has no client cache and is out of
scope.

Everything below up to "Clients" is implemented in `stickpile-core`
(`stickpile_core::offline`, the cache in `cache.rs`) and tested there
(`crates/stickpile-core/src/tests/offline.rs`). The client sections are the
plan for the shells, which only draw, forward input and decide when the
platform allows a download.

## Facts this rests on

- The server's file route (`GET /api/v1/items/{id}/file/{variant}`) has no
  rate limit: the keyed limiters in `security.rs` cover login, upload, AI
  suggestions, rechecks and webhooks only. It sends a strong `ETag` (the
  file's sha256), immutable caching and `Range`. The downloader still
  bounds its concurrency (4 at once) and obeys a `429` with `Retry-After`
  if a proxy sends one.
- Android keeps its cache in `noBackupFilesDir/cache` (`StickpileApp`),
  which the system never empties (unlike `cacheDir`). Kept files stay
  there; existing users' caches do not move.
- Measured on a demo library: thumbnails 3-10 KB, Telegram renditions
  ~100-170 KB, originals ~1.9 MB on average and PNG renditions ~7.7 MB (up
  to 17 MB). What a kept sticker costs depends mostly on the target app.

## What can be kept, and why an item is kept

- `Scope::Item(id)`: one sticker. Always downloaded whole, whatever the
  per-kind options say: an explicit choice beats a general rule.
- `Scope::Set(id)`: a pack, its items now and whatever a sync adds later.
- `Scope::Library`: everything the cache knows the account can see, now
  and later.

The choices live in the cache (`offline_keeps`), on this device only: not
on the server, not synced. Automatic rules come from the options each time
and are not stored. An item is kept for every `Reason` that applies
(`Item`, `Set`, `Library`, `Used`, `Liked`, `OwnPack`); it stays until the
last one is gone, and then its files go back to ordinary
least-recently-used life (they are not deleted at once).

## Which files a kept item needs

`offline::needs(item, options, device)` is the one function that decides
it, for the plan, the estimate and every client:

- its thumbnail (`files/thumb/<media>.webp`);
- for each target app of the device (`offline.targets`; when empty, the
  client's `default_target` plus the targets in the local copy history),
  the rendition a copy chooses (`session::pick_variant`, which uses
  `stickpile_api::profiles::choose_variant`), and with
  `offline.sticker_renditions` also the one it chooses with the sticker
  flag (`clip/<media>.<variant>/<name>.<ext>`);
- on a device that inserts into text fields (the Android keyboard:
  `Session::with_inserts(true)`), for each target app the rendition an
  insert picks (`crate::variant_for`, limited by the MIME types the field
  lists), with and without the sticker flag likewise, for each of the
  fields of `offline::INSERT_FIELDS`: `image/*` (also the usual chat
  field, `image/gif, image/*, image/jpg, image/png, image/webp`), `image/png,
  image/gif`, `image/png`, and `image/*, video/*` (the keyboard's share
  sheet). So a kept sticker inserts offline as it does online: a video
  into an image field as its GIF, a WebP still into a field that takes
  PNG and GIF as its PNG, a WebP animation there as its GIF. On the
  behaviour review's items (one of each shape, the sizes measured on the
  demo library, `tests/offline_clients.rs`), summed over six chat apps,
  this takes 62.2 MB instead of 37.0 MB (+68%): a WebP still and a WebP
  animation 0.6 MB more per app (their PNG, their GIF), a video 1.5 MB more
  (its GIF); PNG stills and GIF animations nothing more. Which lists the
  chat apps really give is not recorded yet (docs/android-input.md); a
  field that covers other types may insert another rendition offline;
- on a device whose grids play animations (the Android app:
  `Session::with_grid_playback(true)`), the rendition played,
  `offline::played_rendition`: the Android rule moved into the core
  unchanged (an animation plays its smaller GIF or WebP picture, else its
  MP4; a video its MP4, else its MP4 or WebM original).

A kept pack also keeps its cover: the pack's own cover image when the
server lists one (`files/cover/...`, learnt by the downloader from
`GET /sets` and kept in `meta` as `offline_covers`), and the thumbnail of
its cover item and of its first item by name (what the offline pack lists
show).

The per-kind options and `offline.max_file_mb` then decide what pack,
library and automatic keeping download (`offline::skip_reason`):
renditions of a kind switched off, or larger than the limit, are skipped
and reported as skipped with the reason (`SkipReason`), not as failed.
Thumbnails and covers are always kept (the grids need them; they are
small), so a pack kept with videos off still shows its videos, dimmed.

## The cache (one store, additive schema)

Kept files are ordinary cache files, found by the lookup
`Session::prepare_copy`, `fetch_variant` and `thumbnail` already use: a
kept file is useful because the copy path finds it without knowing it was
kept. `SCHEMA_VERSION` stays 1 and every change is additive
(`cache::OFFLINE_FORMAT` = 1, in `meta` as `offline_format`), so a client
up to 0.8.0 still opens and uses the cache.

- `files.kept INTEGER NOT NULL DEFAULT 0`, added with `ALTER TABLE` when the
  column is missing (in an immediate transaction, so two processes opening
  the cache at once do not both add it), with a partial index: 0 an
  ordinary file, 1 kept, 2 kept before a reset and being deleted (see
  below).
- `offline_keeps (scope, id, created_at)`: the user's choices (`item`,
  `set`, or `library` with an empty id).
- `offline_failures (path, item_id, error, attempts, at)`: downloads that
  failed and why.
- `offline_claims (path, owner, at)`: which process is downloading a file.
- `offline_peers (id, at, json)`: the offline options and device of each
  process that brought the kept files in line in the last 24 hours (one
  row per `Session`, gone when it ends).
- `meta`: `offline_owner` (the account the kept data belongs to, set once a
  sync completed), `offline_epoch` (moves on every reset),
  `offline_kept_epoch` (the epoch the kept files belong to),
  `offline_liked` and `offline_covers` (what the downloader last learnt
  from the server, for plans made offline).

How a kept file is protected:

- `Cache::evict` (the `cache_max_mb` cap) only deletes files with
  `kept = 0`, and only they count against the cap: `offline.max_mb`
  bounds the kept files.
- `Cache::clear_files` ("Clear cache") deletes only files that are not
  kept; its report says how many kept files stay (`kept_files`,
  `kept_bytes`).
- `Cache::offline_reset` (the confirmed "Remove kept files", and part of
  sign-out) deletes the kept files and forgets every keep, failure and
  claim. In one transaction it forgets, moves the epoch and marks the kept
  files 2 ("being deleted"); then it deletes them. A process that dies in
  between leaves them marked, and the next open of the cache, refresh or
  reset deletes them; one that ran the reset's transaction without marking
  them (moving the epoch past `offline_kept_epoch`) is caught the same way:
  every transaction that flags a file first marks the kept files of an
  older epoch.
- A file is deleted only together with its row: the check, the unlink and
  the row's deletion are one immediate transaction. A file that cannot be
  deleted keeps its row (and its flag), so a later clear, eviction or
  reset tries again; the reports count only files that went.
- The flag is recomputed from the plan on every derivation
  (`Session::offline_refresh`, run after every sync, keep and release, and
  at the start of every download run): wanted files that are present are
  flagged, flagged files nothing wants any more are released to the LRU
  (as used now), and the kept files of items a sync removed (no cached
  item has their media any more, also counting the items this version
  cannot read) are deleted.
- A refresh releases only what no process on the cache keeps: the CLI and
  the popup may run with other options (one read the config before it was
  edited). Each refresh notes its options and device in `offline_peers`
  and flags, and keeps from release, what the plans of the other processes
  noted there in the last 24 hours want (when their options cannot be
  read, from a newer client, it releases nothing). It releases only files
  that were kept when its plan was made, and nothing at all when the epoch
  moved meanwhile (a reset: it derives the plan again). Kept files of
  items this version cannot read (a newer client stored them) stay kept.
  A process whose options are older than another's so never hands the
  other's kept files to the size cap; the price is that a file only an
  ended process wanted stays kept until its row is 24 hours old (or the
  process ended cleanly).
- A kept file's `last_access` is the largest value SQLite holds. The new
  client never reads it for kept files; a client up to 0.8.0, which evicts
  by `last_access` alone, so takes every other file first.

What an older client does to the new cache, and how it heals:

| 0.8.0 does | Effect | Healed by |
|---|---|---|
| opens it | `CREATE TABLE IF NOT EXISTS` and `user_version = 1` change nothing | nothing to heal |
| evicts under its `cache_max_mb` (kept files count there) | other files go first; with more pressure kept files are deleted | the next refresh finds them missing; the next run downloads them again |
| "Clear cache" (Android `clear_cache`) | every file deleted | the same |
| registers a file again | the row keeps `kept`, `last_access` is lowered | the next refresh sets it back |
| a wrong-size or missing file | its row is dropped | the next run downloads it |
| signs out or changes account | items, the sync cursor and `me` go, the offline tables stay | the next refresh, sync or login (`Cache::set_me`) sees `offline_owner` differ from the cached account (or none) and resets the offline data: kept files deleted, keeps forgotten |
| signs out and in again as the same account | as above, then `me` is back at once | the same, as long as 0.8.0 has not synced since: the items and the sync cursor are gone while `offline_owner` is set (it is set only after a completed sync, so a new login's first sync does not look like this) |
| signs out, in again as the same account, and syncs | items, cursor and `me` are back | cannot be told from an ordinary 0.8.0 sync: the kept files of that same account stay kept |
| writes `me` or an item this version cannot read (a newer client) | `Cache::me` or the item is unreadable | not a sign-out: the account's id is read from the raw JSON, and a kept file's item counts as there while its row is |

Running the released 0.8.0 CLI on a cache the new client wrote is a
manual test: `real_0_8_0_cli_on_a_cache_the_new_client_wrote` and
`review_real_0_8_0_cli_upgrade_touch_and_sign_out` in `tests/offline.rs`
(ignored; they need `STICKPILE_080_CLI`, the binary of `nix build
github:Dolfost/stickpile/v0.8.0#stickpile-cli`).

## Options

`[offline]` in the client config (`ClientConfig::offline`,
`config::OfflineConfig`, also exported as `offline::OfflineOptions`),
mirrored in the home-manager module. Android keeps its own copy next to
its display settings and passes it with `Session::set_offline_options`.

| Key | Default | Meaning |
|---|---|---|
| `offline.targets` | `[]` | Chat apps whose renditions are kept; empty: `default_target` plus the targets in the copy history. A list: config file only, not on the Settings page |
| `offline.sticker_renditions` | `true` | Also keep the rendition a copy with the sticker flag uses |
| `offline.stills` / `.animations` / `.videos` | `true` / `true` / `false` | Kinds downloaded by pack, library and automatic keeping |
| `offline.max_file_mb` | `0` | Skip a single rendition larger than this (0: no limit), for pack, library and automatic keeping |
| `offline.max_mb` | `1024` | Budget for kept files (0: no limit): ~10,000 stickers for Telegram, a few hundred for targets that copy originals |
| `offline.auto.thumbnails` | `true` | Keep every item's thumbnail (fetched after each sync) |
| `offline.auto.used` | `false` | Keep the most recently copied stickers of this device... |
| `offline.auto.used_count` | `200` | ...this many (1-2000) |
| `offline.auto.liked` | `false` | Keep the stickers the account liked (listed from the server by each run) |
| `offline.auto.own_packs` | `false` | Keep the packs the account owns |
| `offline.mobile_data` | `false` | Android only: also download on metered networks |

The Settings page's group "Offline" (`settings::settings_schema`) has every
key but `offline.targets` (a list) and `offline.mobile_data` (Android's
own).

## Core API

In `stickpile_core::offline` and on `Session`; all of it is `Send + Sync`
and works without a server unless said otherwise.

| Call | What it does |
|---|---|
| `Session::with_grid_playback(bool)` | The client's grids play animations (Android): keep the played rendition too |
| `Session::with_inserts(bool)` | The client inserts into text fields (the Android keyboard): keep what an insert into the usual fields picks too |
| `Session::offline_options()` / `set_offline_options(opts)` | The options in use (`[offline]` of the config, or what the shell set) |
| `Session::offline_device()` | The resolved targets, playback and inserting, as `needs` gets them |
| `Session::offline_keep(&scope)` / `offline_release(&scope)` | Keep or release a scope; flags or releases what is on the device at once |
| `Session::offline_keeps()` | The kept scopes, oldest first |
| `Session::offline_reasons(&item)` | Why an item is kept (empty: it is not) |
| `Session::offline_plan()` | The `Plan`: every wanted file in download order with present/missing, the skipped items with reasons, totals overall and per scope, the budget, the failures. Read-only |
| `Session::offline_refresh()` | The plan, and the cache brought in line with it (flags, releases, deletions of removed items' files) |
| `Session::offline_estimate(&scope)` | What keeping a scope would take: files, bytes, how much is present, what is skipped, the kept bytes afterwards, the budget (`Estimate::fits`) |
| `Session::offline_run(&control)` | The downloader, until complete, cancelled, the server unreachable, the budget reached or the disk full; waits while paused. Returns `OfflineOutcome { end: RunEnd, status }` |
| `OfflineControl::{new, pause, resume, cancel, is_paused, status, subscribe}` | Controls a run and carries its status (a `tokio::sync::watch` channel, runtime-independent) |
| `Session::offline_usage()` | `OfflineUsage { kept_files, kept_bytes, other_files, other_bytes, budget }` |
| `Session::clear_cache()` | "Clear cache": the files that are not kept |
| `Session::offline_remove_all()` | The confirmed "Remove kept files": kept files deleted, keeps forgotten |
| `Session::available_offline(&items, target, sticker)` | Per item, whether the copy's file is on the device (tile marks); touches nothing |
| `Session::variant_on_device(&item, variant)` | Whether one rendition is on the device (the keyboard's choice) |
| `offline::needs`, `played_rendition`, `skip_reason`, `kind_kept`, `thumb_rel`, `cover_rel` | The rules above, for display and tests |

`OfflineStatus { state, files_done, files_total, bytes_done, bytes_total,
skipped, failed, scopes: Vec<ScopeProgress>, message }`, where `state` is
`Idle | Downloading | Paused | WaitingForNetwork | LimitReached |
DiskFull`, "done" counts the wanted files on the device (those there
before the run included), each `ScopeProgress` covers one kept scope or
automatic rule (`PlanScope::{Thumbnails, Kept(scope), Used, Liked,
OwnPacks}`, with the kept sticker's or pack's `name` from the cache) and
`message` is the English reason of a stop (for logs; the state says
which). The plan's `ScopeTotals` carry the same scopes and names, with the
number of items each keeps. `RunEnd` is `Complete` (everything tried; `failed` may
be non-zero), `Cancelled`, `WaitingForNetwork`, `LimitReached`, `DiskFull`
or `Unauthorized`; `OfflineOutcome::is_complete` means every wanted file
is on the device.

With the server unreachable, `prepare_copy` and `fetch_variant` of a file
that is not on the device fail with `CoreError::NotOnDevice { item,
variant, cause, keeping }` (`is_offline()` is true for it too,
`is_not_on_device()` tells it apart), so a shell explains it instead of
naming a network error. `keeping` (`offline::Keeping`) says what to tell
the user: `NotKept` (keep it while online), `NotYet` (kept, this file too,
not downloaded yet: run the downloader while online), `OtherFile` (kept,
but not this file: another chat app than the device's targets, or the
sticker rendition with `offline.sticker_renditions` off) or `LeftOut`
(kept with a pack, the library or a rule, whose per-kind options or
`offline.max_file_mb` leave the file out). Before failing, `prepare_copy`
copies the best rendition on the device that the target takes (sticker
first when asked; the Android keyboard's `prepare` does the same with
`variant_for`), with the warning `session::COPIED_ANOTHER_FILE` (fixed,
translated in the catalog).

## The downloader

`Session::offline_run` derives the plan, then fetches the missing files in
its order: thumbnails, then stickers (kept by themselves, used, liked),
then packs (with their covers first), then the library; most used first
within a group (copies on this device, then over all users).

- Concurrency 4. Before a file is fetched it is claimed in
  `offline_claims`; another process (the CLI next to the popup) skips a
  fresh claim and looks again later, so no file is fetched twice. A claim
  is renewed every 30 s while its download runs and is stale 120 s after
  that. The part files of a dead process go when a run starts (under
  `files/thumb`, `files/cover` and `clip/`, those of files no live claim
  holds), on "Clear cache" and "Remove kept files", and when their file is
  claimed next. Only names that are exactly `<file>.part.<owner>` count.
- A file is written to `<path>.part.<owner>`, synced to disk and renamed
  into place once its size equals `size_bytes` and its sha256 the server's
  `ETag`, so an interrupted download leaves nothing that looks complete;
  the part file is deleted when the download ends any other way, also when
  the run's future is dropped. Partial bytes are not reused: the next run
  fetches the file again.
- The rename happens in the immediate transaction that checks the epoch
  and registers the file as kept (`Cache::install_kept_file`): a reset
  either comes first (the epoch moved: only the part file is deleted, the
  run ends `Cancelled`) or after (it finds the file kept and deletes it).
  A download a reset made stale never replaces nor deletes a file another
  process kept since; a file found on the device already is registered the
  same way and never deleted. Before each try, also each retry, the run
  checks the epoch and its claim again.
- A `429` makes every download wait as `Retry-After` says (30 s without
  one, at most 10 minutes; at most 5 times per file). Other errors are
  retried after 2 s and 4 s; the third failure counts the file as failed
  (`offline_failures`, the reason kept), and the run goes on with the rest.
  A file that failed in an earlier run gets one try per run.
- The server unreachable: the run stops at once (`WaitingForNetwork`), the
  running downloads too; the next run after a successful sync goes on. A
  rejected token stops it (`Unauthorized`).
- The budget: before a file starts, the kept bytes, the running downloads
  and the file must fit in `offline.max_mb`; when they do not, no download
  starts any more, the running ones finish, and the status says
  `LimitReached` with the budget. Nothing is deleted to make room.
- A full disk (`ENOSPC`, `EDQUOT`, or SQLite's "disk is full") stops the run
  (`DiskFull`) without a retry and without deleting anything.
- Pause stops the running downloads (they start again first on resume) and
  holds the run; it stays paused across runs until resumed. Cancel ends the
  run; the next one starts afresh.
- A copy goes first: while a download the user asked for runs (a copy, an
  insert, a thumbnail or cover on screen), no background download starts
  and the running ones hold between chunks (at most 30 s at a time).

## Sign-out and another server

`Cache::reset_account_data` (logout, another account, another server)
deletes the kept files and empties the keep, failure and claim tables, with
the rest of the account's data. A download still running throws its file
away (`offline_epoch` moved).

A client up to 0.8.0 does not know the offline data: its sign-out drops
the items, the sync cursor and the cached account, and leaves the kept
files. The new client completes it (`Cache::offline_check_owner`, run by
every refresh and by `Cache::set_me`, so before a login stores its
account): it resets the offline data when no account is cached, when
another one is, or when the same one is but the items and the sync cursor
are gone (0.8.0 signed out and in again; its login stores the account at
once, and `offline_owner` is set only after a completed sync). After a
0.8.0 sign-out, a sign-in as the same account and a 0.8.0 sync, nothing
tells the cache from an ordinary 0.8.0 sync: that account's kept files
stay. A cached account this version cannot read is not taken for a
sign-out.

## Clients

The plan for the shells, as in the brief, section 4.9:

- **CLI**: `stickpile offline keep <item>`, `--set <pack>`, `--all`;
  `stickpile offline release` with the same forms; `stickpile offline
  status` (what is kept, `offline_usage`, the plan's totals per scope,
  skipped and failures; the estimate for a scope with `--dry-run`);
  `stickpile offline sync` to sync and run the downloader to completion
  with progress, since the CLI is not resident. Exit codes: 0 complete
  (`is_complete`), 2 incomplete (paused, limit, network, failures), 1
  failed (an error). Explain `CoreError::NotOnDevice` in `stickpile copy`.
- **GTK popup and macOS app**: a key and a menu entry to keep or release
  the selected sticker (`offline_reasons` tells which to show), the same
  for a pack in the packs view (with the estimate first); the "Offline"
  Settings group (generic, from the schema) plus a block with the library
  switch, `offline_usage`, the progress (`OfflineControl::subscribe`) with
  pause and cancel, and "Remove kept files" (confirmed,
  `offline_remove_all`); "Clear cache" says how many kept files stay. The
  resident process runs `offline_run` after each successful sync
  (`SyncPolicy`) and after a keep. Tiles not `available_offline` are dimmed
  when the server is unreachable (the others are drawn plain then), and a
  mark shows what is available offline only while it is reachable (a tile
  mark like the others), as on Android. `NotOnDevice` is explained,
  translated; for a sticker that is kept (`offline_reasons` not empty) the
  message names the missing rendition and the app instead of advising `K`,
  which would release it. The estimate and the Settings block count what
  is left out by reason, as Android does: stickers of a kind switched off,
  files over `max_file_mb`, and the other reasons by name. Sizes are shown
  in powers of 1024 (KiB, MiB, GiB) in every client.
- **Android app**: keep and release in the sticker's menu and details and
  on the pack screen; a settings section that stores `OfflineOptions` and
  passes them with `set_offline_options`; `with_grid_playback(true)`; a
  WorkManager job that runs `offline_run` (unmetered unless `mobile_data`,
  not on low storage), cancelled by dropping it. `cache_bytes` and
  `clear_cache` move to `offline_usage` and `Session::clear_cache`;
  `motion::choose` moves to `offline::played_rendition`;
  `LibraryError` gets its own kind for `NotOnDevice` (today it maps to
  `Unreachable`).
- **Android keyboard**: keep and release in the long-press menu, the marks
  (whether a rendition it would insert or share is on the phone, for a
  plain pick or for "Send as a sticker"), no settings of its own, no
  downloads beyond what a pick needs.
- **home-manager module**: the `[offline]` options, mirrored (done).

## Reachability and results without waiting (design, being built)

A search never waits for a server that does not answer. In
`stickpile-core`, built once and used by the GTK popup, the macOS app,
the Android app and the keyboard (the CLI and the web UI keep their
behaviour):

- **Reachability for every client.** The fail-fast reachability of
  `ApiClient::with_fail_fast` is used by every client that shows
  results, and it publishes its state as a watch value: `Unknown`,
  `Reachable`, `Unreachable { since }`. While the server is believed
  unreachable a background task probes `GET /healthz` every 15 s for 5
  minutes, then every 60 s, whether or not anything reads; it starts on
  any offline failure and stops when the server answers. A shell can ask
  for a probe at once: when the popup or the app opens, and on Android
  when a network appears (`ConnectivityManager`; no new permission).
- **Two-step search.** With the state `Unreachable`, a search answers
  from the offline index at once, marked offline. With `Reachable` or
  `Unknown` it asks the server; if the server has not answered after
  300 ms, the offline results are handed over first and the server's
  follow in the same generation when they come. A failure then marks the
  state `Unreachable` and the offline results stay. One core API yields
  the offline answer and then, maybe, the server's; the popup model and
  the Android bridge consume it.
- **Short connect timeout.** Searches and other interactive reads connect
  within 2 s, so a peer that is gone is recognised quickly; `timeout_secs`
  still bounds the rest, and uploads, imports and file downloads keep
  their timeouts.
- **Back online.** When the watch value turns `Reachable`, the shells run
  the current search again online, start a sync and drop the offline
  notice.
- **Results do not jump.** The server's results replace the offline ones
  in place: the selected sticker stays selected (by id) when it is still
  in them, a copy or insert in progress is not dropped, and the grid is
  not reordered under a pointer or a finger pressing a tile. The server
  stays authoritative: its results include semantic and image matches
  and items the cache does not hold.
