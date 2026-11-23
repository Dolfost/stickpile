# Telegram import and export

stickpile talks to the Telegram **Bot API**, using only the sticker methods.
It can:

- **import** an existing sticker pack by name into a new set;
- **export** a set as a sticker pack that a bot owns: it creates the pack the
  first time and updates it on later exports.

After an import, stickpile is the source of truth for packs you already
have. The implementation is `crates/stickpile-telegram`.

## Bot setup

1. In Telegram, talk to [@BotFather](https://t.me/BotFather): `/newbot`,
   choose a name and a username (it must end in `bot`, for example
   `my_stickpile_bot`). BotFather replies with a token like
   `123456789:AA…`.
2. Store the token in a file readable only by the service user:

   ```sh
   install -m 0400 -o stickpile /dev/stdin /run/secrets/stickpile-telegram-token <<< '123456789:AA…'
   ```

3. Find the numeric Telegram user id that should own exported packs. You
   can ask [@userinfobot](https://t.me/userinfobot), for example. The owner
   must have started a chat with your bot at least once (`/start`), or
   Telegram refuses to create packs for them.
4. Configure the server:

   ```toml
   [telegram]
   enabled = true
   bot_token_file = "/run/secrets/stickpile-telegram-token"
   # api_url = "https://api.telegram.org"   # or a self-hosted Bot API server
   default_owner_user_id = 123456789          # users can set their own in the web UI
   ```

The integration is on when `telegram.enabled` is set **and** the token file
is readable and not empty. Otherwise both endpoints answer `404`, and
`stickpile-server check-config` prints a warning. Each user sets the
Telegram user id that should own their exported packs with
`PATCH /api/v1/me` (`{"telegram_user_id": 123456789}`, `null` clears it) or
in the web UI. `default_owner_user_id` is the fallback for users who have
not set one.

The token is a secret and is passed only as a file path, never inline. It is
part of every Bot API URL, so stickpile strips URLs from HTTP errors, keeps
the token out of `Debug` output, and never logs it. A test checks this.

## Pack names

Telegram requires the short name of a bot-created pack to:

- be 1–64 characters of `A–Z a–z 0–9 _`;
- start with a letter and contain no `__`;
- end in `_by_<bot username>` (case-insensitive).

`valid_pack_name(name, bot)` checks these rules. `default_pack_name(slug,
bot)` derives one from a set slug, for example `my-cats` becomes
`my_cats_by_my_stickpile_bot`. The pack can then be added at
`https://t.me/addstickers/<name>`.

Import works with **any** public pack name, from any creator. Export works
only with packs created by your bot, because Telegram allows a bot to modify
only its own packs.

## Import

`POST /api/v1/telegram/import` with `{"pack_name": "…", "slug": null,
"visibility": "private"}` queues a `telegram_import` job and returns it.
`pack_name` may also be a `https://t.me/addstickers/<name>` link. The slug
defaults to the pack name slugified (`Animal_Friends` becomes
`animal-friends`). A slug you already use is `409` right away. The request
counts against `limits.upload_per_minute`. The job runs `import_pack`:

1. `getStickerSet` reads the pack.
2. Each sticker is fetched in pack order with `getFile` and a download from
   `<api_url>/file/bot<token>/<file_path>`. The download streams with a byte
   cap.
3. Static stickers arrive as WebP and video stickers as WebM (VP9). Each
   becomes an item in pack order and goes through normal ingest: sniffing,
   variants and enrichment.
4. A sticker's emoji becomes the item's `emojis` (user-sourced). The Bot API
   exposes only one emoji per sticker and no keywords, so the import can
   carry no more.
5. Everything is downloaded before the set is created, so a job retried
   after a network error starts clean. The new set has `source = telegram`,
   the pack title as its name, aspect ratio 1:1 and 512 px recommended
   size. Items are named `sticker 1`, `sticker 2`, … in pack order.

The job result has `set_id`, `slug`, `pack_name`, `title`, `url`
(`https://t.me/addstickers/<name>`), `imported`, `skipped`
(`[{file_unique_id, reason}]`) and `bound` (see below). An unknown pack
fails the job with "sticker pack not found".

Skipped stickers are listed in the job report with a reason:

| Case | Reason |
|---|---|
| Animated stickers (`.tgs`, Lottie) | `animated .tgs stickers are not supported (stretch goal)` |
| File over the media upload limit | `file is larger than N bytes` |
| Same file as an earlier sticker of the pack | `the same file as an earlier sticker of the pack` |
| Rejected by ingest (sniffing, limits) | the ingest error, e.g. `unsupported or unrecognised file type` |

If the imported pack was made by your bot (its name ends in
`_by_<bot username>`, checked with `getMe`) and you may publish to it (see
[Pack ownership](#pack-ownership)), the set is **bound** to it
(`Set.telegram_pack`). Anyone else gets an unbound copy. The export mapping is seeded from the import: each
imported sticker's `file_unique_id`, together with the item's media hash,
identifies the sticker. The first export then keeps the existing stickers
instead of re-uploading them.

## Pack ownership

One bot serves every user, and Telegram lets a bot edit every pack it ever
created, whoever asked for it. So stickpile itself decides who may publish
to a pack, in the `telegram_packs` table (migration
`0003_telegram_pack_owners.sql`):

- The first export to a new pack name records the exporting user as its
  owner. Only that user can export to it again or bind it on import. Pack
  names compare case-insensitively, as on Telegram.
- Another user's export to that name fails with "belongs to another
  stickpile user; choose another pack name". Their import of the pack is an
  unbound copy. The same applies when two users' sets share a slug and so
  derive the same default pack name: the second one has to pick a name.
- A pack that exists on Telegram but was not created through this server
  (made with the bot elsewhere, or before the database was reset) can only
  be adopted by an **admin**, by importing it or exporting to it. A regular
  user gets "already exists but was not published through this server".
- A pack follows exactly one set. Publishing another of your sets to the
  same pack moves the binding to that set.
- A name stays claimed even if the first export failed after the claim, or
  the set was deleted, so nobody else can take over the pack. Deleting the
  user releases their claims; only an admin can adopt the packs after that.

The migration gives existing packs to the owner of the earliest set bound to
each, and unbinds any later sets bound to the same pack.

## Where publishing lives

Telegram is the first platform of the publish registry
(`crates/stickpile-server/src/service/publish.rs`), next to Discord
([discord.md](discord.md)) and Signal ([signal.md](signal.md)). In the web UI, publishing lives on the set's export
page, `/sets/{id}/export` ([web.md](web.md#export-and-publish)): its
Publish section has a row for Telegram with the pack's link, when it was
last published and what that changed, the last error, the pack name and
title fields and the Telegram user id hint, and a checkbox to publish to
Telegram together with other platforms. The set page's "Publish to
Telegram" / "Update Telegram pack" button leads there; the older form at
`/sets/{id}/telegram` still works.

In the API, `POST /api/v1/sets/{id}/publish` with `{"platforms":
["telegram"], "options": {"telegram": {"pack_name": ..., "title": ...}}}`
queues the same `telegram_export` job as the route below, which stays as it
is; `GET /api/v1/sets/{id}/publish` reports where the set is published
([api.md](api.md#publishing)). Every finished `telegram_export` job, from
either route, is recorded with the set (`set_publications`, migration 0012):
when it was last published, that result and the last error. The pack
binding itself stays in `sets.telegram_*`.

## Export

`POST /api/v1/sets/{id}/telegram/export` with `{"pack_name": null, "title":
null}` (the set's owner only, read-only git packs included) queues a
`telegram_export` job and returns it. Without a Telegram user id for the
caller (their own, else `telegram.default_owner_user_id`) the request is a
`400` that says how to set one. The job runs `export_pack`. By default it
uses the previously bound pack name (or `<slug>_by_<bot>`) and the set name
as title. A given pack name without the `_by_<bot username>` suffix gets it
appended (`test` becomes `test_by_<bot>`). A name that ends in another bot's
suffix (`x_by_other_bot`) fails the job with an explanation, because a bot
can edit only its own packs.

Every item is exported as its `tg_sticker` variant (512 px static WebP) for
stills, or its `tg_video_sticker` variant (512 px VP9 WebM, at most 3 s, no
audio) for animations and videos. `tg_sticker` is lossless, made for
chats; one over Telegram's 512 KB limit for pack stickers is re-encoded
lossy for the pack (docs/media.md, section 3), the stored file stays as it
is. If the `variants` job has not produced the
variant yet, the export job renders it itself. Items that cannot become
stickers are skipped and listed in the result, for example an animation
without ffmpeg on the server, or a video that cannot fit Telegram's limits.
A set with no exportable item fails the job.

The job result has `pack_name`, `url` (`https://t.me/addstickers/<name>`),
`title`, `created_pack`, `added`, `removed`, `updated`, `reordered`,
`thumbnail` (the pack has the set's cover as its thumbnail),
`thumbnail_changed`, `exported` and `skipped` (`[{item_id, name, reason}]`). The pack name and the
new mapping are stored on the set (`sets.telegram_pack`,
`sets.telegram_mapping`). Exporting under a different pack name starts that
pack with an empty mapping.

**First export.** The pack does not exist yet: `getStickerSet` answers
`STICKERSET_INVALID`. `createNewStickerSet` creates it with the first 50
items, and `addStickerToSet` adds the rest one by one.

**Later exports** compare the set with the pack, using the mapping the
server stored after the previous export
(`item id → (media hash, file_unique_id)`):

1. Stickers whose item was deleted, whose media changed, or that the mapping
   does not know (for example added by hand in @Stickers) are deleted.
2. Missing items are added. Telegram appends new stickers at the end, and
   their ids are read back with `getStickerSet`.
3. Emoji list and keywords are set again on every sticker that stayed. The
   Bot API cannot report keywords, so a change cannot be detected; this
   costs two cheap calls per sticker.
4. `setStickerPositionInSet` fixes positions so the pack order matches the
   set order.
5. The title is updated if it differs.
6. The thumbnail follows the set's cover (below).

The pack is never empty along the way. If nothing can be kept (for example
the mapping was lost), the last old sticker is swapped with
`replaceStickerInSet` instead of being deleted.

**Thumbnail.** A set with a cover image of its own or a chosen cover item
(set settings, or "Use as cover" in an item's menu) gets it as the pack's
thumbnail with `setStickerSetThumbnail`, scaled to fit 100×100, centred on
a transparent square of exactly that size, as PNG. A cover image (a still
uploaded for the set, never a sticker of the pack) is rendered from its
stored original, a cover item from its grid thumbnail (a still frame for
animations and videos). The key of what was sent (`image:` and the image's
hash, or the cover item and its media hash) is stored on the set
(`sets.telegram_thumbnail`), so the thumbnail is uploaded only when the
cover changes: a new image, another item, or the image removed (back to the
chosen item). Going back to "First item" with no image drops the thumbnail
stickpile set, and Telegram shows the first sticker again. A thumbnail set
outside stickpile is left alone while the set has no cover. Like everything
else, it reaches Telegram on the next publish ("Update Telegram pack"), not
at once.

Items are adapted to what Telegram accepts:

- Emoji are sent on every publish: with new stickers, and with
  `setStickerEmojiList` for the ones that stay, so edits reach the pack.
  Entries that are not emoji (a word typed into the field) are left out,
  because Telegram would refuse them and fail the export. An item without
  emoji gets `🙂`, because Telegram requires at least one. At most 20 emoji
  are sent.
- At most 20 keywords are sent, with no more than 64 characters in total.
- Titles are cut to 64 characters.
- Items with identical media are exported once, because Telegram ignores a
  second identical file in a pack.

The new mapping is returned in the job result and stored on the set.

## Sets larger than one pack

A Telegram pack holds at most 120 stickers (`MAX_PACK_STICKERS`). A set
with more is published as several packs, its **parts**; later exports keep
every part in step with the set. The `parts` module of
`crates/stickpile-telegram` does this: `plan_parts` decides where each
sticker goes, `export_parts` carries the plan out, and `part_pack_name` and
`part_title` name the parts. A set of at most 120 stickers is one part, and
exports exactly as a single pack did.

**Names and titles.** Part 1 keeps the name a single pack has,
`<name>_by_<bot>` (the default from the slug, or a name chosen at the first
export). Part k is `<name>_<k>_by_<bot>`, with the suffix written as in
part 1's name: `cats_by_my_bot` is followed by `cats_2_by_my_bot`,
`cats_3_by_my_bot` and so on. When the whole would pass 64 characters,
`<name>` is shortened, and a `_` left at its end goes too, so every part's
name is a valid pack name (it starts with a letter and has no `__`). In the
one case where the shortened name would be part 1's own (a 64-character
part 1 name ending in `_2_by_<bot>`), it is shortened once more. Part 1's
title is the set's name; part k's is `<set name> <k>`. Both have at most 64
characters: a longer name is cut on characters (not bytes, so Cyrillic is
cut cleanly), before ` <k>`, so the number stays. Every part gets the set's
cover as its thumbnail.

**Planning.** Before any call, `plan_parts` assigns the set's exportable
items, in set order, to parts, from the mapping each part stored after the
previous export:

1. Items with the same media are exported once for the whole set, so a
   duplicate never lands in a second pack. The one exported is the one
   already published with that media, else one already published, else the
   first in set order; the plan lists the others as duplicates. Items that
   cannot become stickers (no rendition) are not among the items: like a
   removed item, a published one frees its place.
2. A published sticker stays in the part it was first published to, the
   part whose mapping has its item. Moving an item in the set across a part
   boundary does not move its sticker to another pack, and an item whose
   media changed is replaced within its part.
3. New stickers fill the first part with room, in set order: the existing
   parts by number, then new parts, numbered from the lowest unused number.
   A part emptied in the same export takes new stickers first.
4. The order inside a part follows the set.
5. A part left without stickers is planned for deletion, part 1 included:
   the other parts keep their stickers.

A set without exportable items is an error, never a plan that deletes every
part.

**Repack**, only on request, assigns every item again in set order: part 1
gets the first 120, part 2 the next 120, and so on. The plan counts the
published stickers that change part (`moved`), the number to show in a
confirmation before the repack runs, and deletes the parts no longer
needed.

**Exporting.** `export_parts` makes and checks every name first, so an
invalid one fails before any call. Then it exports the planned parts by
number, each like a single pack ([Export](#export): created or updated,
never emptied, with the cover as thumbnail), and hands each part's new
state (its mapping and thumbnail key) to the caller to store as soon as
that part is done. After every part is updated, it deletes the parts left
empty with `deleteStickerSet` (Bot API 6.6, which deletes a set the bot
created); an answer that the pack does not exist (`STICKERSET_INVALID`)
counts as deleted. The caller then forgets the part. The report lists every part with its number,
name, link (`https://t.me/addstickers/<name>`), title, number of stickers,
`created_pack`, `added`, `removed`, `updated`, `reordered`, `thumbnail`,
`thumbnail_changed`, `deleted` and the error where the export stopped, and
sums them up. Part 1's fields are those of a single pack's export.

**Failure.** The export stops at the first failure and then deletes
nothing. Each part's state is stored as soon as that part is done, and the
part that failed stores its state as far as it got
(`export_pack_resumable`): the stickers it kept, the ones it added as far as
reading the pack back confirms them, and the previous entries of its other
items, so that they keep their part. When Telegram's answer to a call was
lost, the pack read back shows whether the call took effect. The next
export continues from there:

- Names follow from the part number, so it finds the same packs and never
  creates a second pack for a part.
- A sticker the stored state does not know is deleted before the missing
  ones are added, so none is doubled; at worst a sticker is uploaded again.
- A part known to exist without a stored mapping (its first export failed
  before any sticker was confirmed) is passed in with an empty state, and
  is then filled or deleted, never left behind on Telegram.
- A part whose pack was deleted by hand on Telegram is created again, as a
  single pack is.
- A repack that stopped is finished by the next repack (a retried job
  repeats it). An ordinary export after it does not finish it, but also
  leaves every sticker in exactly one pack: if an item ends up in two
  parts' mappings (the repack stopped after an earlier part took the
  sticker over), the lower part keeps it and the other drops its copy.

**Server side** (design, being built):

- **Ownership per part.** Every part's name is registered to the set's
  owner in `telegram_packs` before its pack is created
  ([Pack ownership](#pack-ownership)). A part name that belongs to someone
  else, is bound to another set, or exists on Telegram for another bot
  fails the export with a clear message before any change.
- **Repack surfaces.** "Repack the parts" on the export page, `--repack` in
  the CLI and `repack: true` in the API, after a confirmation that says how
  many stickers move.
- **Results and compatibility.** The job's result keeps its fields for
  part 1 and adds `parts`. `Set.telegram_pack` stays part 1's name, so
  older clients read a set as before; the migration keeps every existing
  binding as part 1.
- **Other platforms.** Signal packs hold 200 stickers and are split the
  same way (each publish uploads every part anew). Discord is unchanged (a
  server's slots are its own), and importing a Telegram pack still makes
  one set.

## Limitations

- **Animated `.tgs` stickers** are a stretch goal. They are skipped on
  import, and stickpile does not produce them on export.
- **120 stickers per pack.** Telegram's limit for regular packs is 120
  (`addStickerToSet`). A larger set is published as several packs
  ([Sets larger than one pack](#sets-larger-than-one-pack)).
- **Rate limits.** Telegram answers floods with 429 and `retry_after`. Each
  call is retried up to 3 times after waiting `retry_after`, capped at 30 s
  per wait. A full re-export of a large pack makes a few hundred calls and
  can take minutes. It runs as a background job.
- **Download size.** The public Bot API serves files up to 20 MB. A local
  Bot API server (`--local`) returns absolute file paths, which are not
  supported.
- **Custom emoji and mask packs** can be imported like regular packs.
  Export always creates regular sticker packs.
- **Ownership.** On Telegram, a pack belongs to the Telegram user id it was
  created for. The bot can edit it, but stickpile cannot move it to a
  different Telegram owner. Within stickpile, see
  [Pack ownership](#pack-ownership).

## Testing

`crates/stickpile-server/tests/integrations_telegram.rs` tests the server
side (import with skipped `.tgs` and duplicates, binding and seeding,
export create and update, ordering, emoji, titles, owner resolution, pack
name validation, pack ownership between users and admin adoption, the
ownership migration, the disabled integration) against a compact mock in
`tests/integrations_support/mock_bot.rs`.

`crates/stickpile-telegram/tests/bot_api.rs` runs everything against an
in-process axum mock of the Bot API on `127.0.0.1:0`. The mock keeps
in-memory pack state, serves file downloads, injects 429 answers, derives
`file_unique_id` from the file content as Telegram does, and refuses to
empty a pack. It checks Telegram's pack name and title rules, 1 to 50
stickers in `createNewStickerSet` and 120 per pack, and deletes only packs
of its own bot. It can also fail a method after a number of calls, before
or after the call takes effect, to test exports that stop half-way.
`tests/parts.rs` exports sets of several parts against the same mock:
from scratch, again unchanged, failing between parts and in the middle of
one (then continuing without a duplicate pack and with every sticker
exactly once), deleting emptied parts after the others, a repack, a part
deleted by hand, and a single pack bound before parts existed. The planner,
the names and the titles have unit tests in `src/parts.rs`. The tests need
no network access.
