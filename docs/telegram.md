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
"visibility": "private"}` runs `import_pack`:

1. `getStickerSet` reads the pack.
2. Each sticker is fetched in pack order with `getFile` and a download from
   `<api_url>/file/bot<token>/<file_path>`. The download streams with a byte
   cap.
3. Static stickers arrive as WebP and video stickers as WebM (VP9). Each
   becomes an item in pack order and goes through normal ingest: sniffing,
   variants and enrichment.
4. A sticker's emoji becomes the item's `emojis`. The Bot API exposes only
   one emoji per sticker and no keywords, so the import can carry no more.

Skipped stickers are listed in the job report with a reason:

| Case | Reason |
|---|---|
| Animated stickers (`.tgs`, Lottie) | `animated .tgs stickers are not supported (stretch goal)` |
| File over the media upload limit | `file is larger than N bytes` |

If you later export an imported pack that your bot owns, the server can seed
the export mapping from the import. Each imported sticker's
`file_unique_id`, together with the item's media hash, identifies the
sticker. The first export then keeps the existing stickers instead of
re-uploading them.

## Export

`POST /api/v1/sets/{id}/telegram/export` with `{"pack_name": null, "title":
null}` runs `export_pack`. By default it uses the previously bound pack name
(or `<slug>_by_<bot>`) and the set name as title. Every item is exported as
its `tg_sticker` variant (512 px static WebP) or its `tg_video_sticker`
variant (512 px VP9 WebM, at most 3 s, no audio).

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

The pack is never empty along the way. If nothing can be kept (for example
the mapping was lost), the last old sticker is swapped with
`replaceStickerInSet` instead of being deleted.

Items are adapted to what Telegram accepts:

- An item without emoji gets `🙂`, because Telegram requires at least one.
  At most 20 emoji are sent.
- At most 20 keywords are sent, with no more than 64 characters in total.
- Titles are cut to 64 characters.
- Items with identical media are exported once, because Telegram ignores a
  second identical file in a pack.

The new mapping is returned in the job result and stored on the set.

## Limitations

- **Animated `.tgs` stickers** are a stretch goal. They are skipped on
  import, and stickpile does not produce them on export.
- **120 stickers per pack.** Telegram's limit for regular packs is 120
  (`addStickerToSet`). Exporting a larger set fails before any change is
  made, so split the set.
- **Rate limits.** Telegram answers floods with 429 and `retry_after`. Each
  call is retried up to 3 times after waiting `retry_after`, capped at 30 s
  per wait. A full re-export of a large pack makes a few hundred calls and
  can take minutes. It runs as a background job.
- **Download size.** The public Bot API serves files up to 20 MB. A local
  Bot API server (`--local`) returns absolute file paths, which are not
  supported.
- **Custom emoji and mask packs** can be imported like regular packs.
  Export always creates regular sticker packs.
- **Ownership.** Packs belong to the owner user id that created them. The
  bot can edit them, but stickpile cannot move a pack to a different owner.

## Testing

`crates/stickpile-telegram/tests/bot_api.rs` runs everything against an
in-process axum mock of the Bot API on `127.0.0.1:0`. The mock keeps
in-memory pack state, serves file downloads, injects 429 answers, derives
`file_unique_id` from the file content as Telegram does, and refuses to
empty a pack. The tests need no network access.
