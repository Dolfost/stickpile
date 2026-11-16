# Discord import and export

stickpile talks to the Discord **REST API** (version 10) as a bot, using
only the guild sticker and emoji routes. It can:

- **export** a set into one Discord server (a "guild") as **stickers** or as
  **emoji**, and keep them in line with the set on later exports;
- **import** a server's stickers and/or emoji into a new set.

The Discord side is `crates/stickpile-discord`; the server uses it
through its platform registry ([In the server](#in-the-server):
configuration, the export page, jobs, API routes). Everything here was
tested against an in-process mock of the Discord API and CDN, never against
Discord itself.

## Bot setup

1. Open the [Discord developer portal](https://discord.com/developers/applications),
   **New Application**, give it a name. On the **Bot** page, **Reset
   Token** and copy the token. The bot needs no privileged gateway intents:
   stickpile uses only REST.
2. Store the token in a file readable only by the service user, and name
   it in the server's configuration (`discord.bot_token_file`,
   [configuration.md](configuration.md#discord)):

   ```sh
   install -m 0400 -o stickpile /dev/stdin /run/secrets/stickpile-discord-token <<< 'MTk4NjIy...'
   ```

3. Invite the bot to the server with the **Create Expressions** permission
   (`CREATE_GUILD_EXPRESSIONS`, `1 << 43`). In the portal: **OAuth2 > URL
   Generator**, scope `bot`, permission **Create Expressions**, then open
   the generated URL. By hand:

   ```
   https://discord.com/oauth2/authorize?client_id=<application id>&scope=bot&permissions=8796093022208
   ```

   Create Expressions lets the bot create stickers and emoji and edit or
   delete the ones it created, which is all stickpile does. **Manage
   Expressions** (`MANAGE_GUILD_EXPRESSIONS`, `1 << 30`, formerly "Manage
   Emojis and Stickers") would let it edit and delete everyone's; it is not
   needed and should not be granted. Since 23 February 2026 Discord requires
   Create Expressions for bots that create stickers or emoji; Manage
   Expressions alone is no longer enough (discord-api-docs, pull request
   8002). Inviting needs the Manage Server permission in that server.
4. Find the server id: in Discord, **User Settings > Advanced > Developer
   Mode** on, then right-click the server icon, **Copy Server ID** (a number
   like `100000000000000001`).

The token is a secret and is passed only as a file path
(`BotToken::from_file`). It travels only in the `Authorization: Bot ...`
header of API requests, marked sensitive; CDN downloads carry no
credentials. It is kept out of `Debug` output, errors (a token file that
cannot be read is named, its content never) and logs. Tests check this.

## Limits

Discord's rules, from the developer docs (resources/sticker,
resources/emoji, reference "Image Data", topics/rate-limits):

| | Stickers | Emoji |
|---|---|---|
| Formats | PNG, APNG, GIF (Lottie: verified or partnered servers only, never produced) | PNG, GIF, JPEG, WebP (AVIF also, not produced); animated GIF or WebP makes an animated emoji |
| File size | 512 KiB | 256 KiB |
| Dimensions | at most 320x320 ("constrained to ... 320 x 320 pixels") | shown at 128x128 ("the 128x128 emoji image") |
| Animation | at most 5 s | |
| Name | 2-30 characters | 2-32 characters of `A-Z a-z 0-9 _` |
| Other fields | description empty or 2-100 characters; tags up to 200 characters | |
| Slots by Server Boost level 0/1/2/3 | 5 / 15 / 30 / 60 | 50 / 100 / 150 / 250 static, and the same number animated |

The slot numbers are not in the developer docs (they only say "five free
sticker slots by default, and each Boost level will grant access to more
slots"). They are the numbers discord.py uses (`Guild.sticker_limit`,
`Guild.emoji_limit`, including the `MORE_STICKERS` feature for at least 60
sticker slots and `MORE_EMOJI` for at least 200 emoji) and the ones
Discord's Server Boosting FAQ is widely quoted with; the FAQ page itself
could not be fetched. The emoji name character set is what the Discord
client enforces; the API reference only says "name".

`requirements(kind)` returns these rules for the server to render files
that fit, and `check_file(kind, bytes)` checks a rendered file (format,
size, dimensions and animation length are read from the file header).

**Rate limits.** Every answer may carry `X-RateLimit-Remaining` and
`X-RateLimit-Reset-After`; when a route has no requests left, the next
request on it waits for the reset. A 429 answer is retried up to 3 times
after its `retry_after` (from the body, else the `Retry-After` header), each
wait capped at 30 s; a global 429 holds back every route. The emoji routes
are limited per server and Discord warns that their headers may be
inaccurate, so 429s are expected there and handled the same way.

## Export

`sync_guild(api, guild_id, kind, items, previous_mapping)` makes the
stickers or the emoji that stickpile created in one server match a set.
Each item is passed already rendered for the kind (see
[Renditions](#renditions)), with its id, a content key (the media hash),
its name, description and emoji.

**Only what stickpile created is touched.** A server's stickers and emoji
are shared with its members. The export keeps a mapping per (set, server,
kind): `item id -> (content key, Discord id)`, persisted by the server like
the Telegram mapping and passed to the next export. Only Discord ids in that
mapping are ever modified or deleted. Stickers and emoji made by people,
other bots, another set, or by this bot before the mapping was lost are left
alone; they still count against the slots.

An export:

1. reads the server (its boost level and features) and its stickers or
   emoji. Mapping entries whose sticker or emoji was deleted by hand are
   forgotten (the item is uploaded again);
2. checks every item's file; an item whose file breaks the rules, or whose
   content key repeats an earlier item's, is skipped with a reason;
3. deletes the mapped stickers or emoji whose item is gone, was skipped, or
   whose content key changed (Discord cannot replace a file, so a changed
   picture is a delete and a new upload). Deletions run first so their
   slots are free again;
4. updates kept ones whose name, description or tags differ (sticker: name,
   description, tags; emoji: name);
5. creates the missing ones in set order while slots are free. Items beyond
   the slot limit are skipped with `no free sticker slot: the server has all
   5 in use (Server Boost level 0)` (static and animated emoji are counted
   separately). An item Discord refuses with HTTP 400 (for example a file it
   cannot read) is skipped with Discord's message, and the export goes on.

Names and tags:

- Sticker name: the item name with whitespace collapsed, cut to 30
  characters; a name shorter than 2 characters becomes `sticker <n>`.
- Sticker description: the given text, cut to 100 characters, empty when
  shorter than 2.
- Sticker tags: the item's first emoji (entries that are not emoji are
  skipped), else `🙂`. The docs call the field "autocomplete/suggestion
  tags" and say the Discord client fills it with a name generated from an
  emoji; discord.py recommends a raw Unicode emoji, which stickpile sends.
  How the Discord client shows such a tag was not checked.
- Emoji name: the item name with accents removed, other characters turned
  into `_`, cut to 32 (`Grumpy cat!` becomes `Grumpy_cat`); a name made of
  no Latin letters or digits becomes `emoji_<n>`. Names are made unique in
  the server (`cat`, `cat_2`, ...) so a person's emoji keeps its name.

Every change carries the audit log reason `stickpile export`.

The result (`SyncReport`) has `added`, `removed`, `updated`, `unchanged`,
`skipped` (`[{key, name, reason}]`), `slots` (`{used, limit}`; static emoji
for an emoji export), `animated_slots` (emoji only), `premium_tier`,
`guild_name`, `url` (`https://discord.com/channels/<server id>`) and the new
`mapping` to store.

**Failures.** Duplicate item ids fail before any call. Any other error
(network, a missing permission, an unknown server, a 5xx) stops the export
with a `SyncError` whose `partial` report holds the mapping as it stands
after the changes already made. The server must store that mapping too:
otherwise the stickers the failed export created would be unknown to the
next one, and it never deletes stickers it does not know.

## Import

`import_guild(api, guild_id, {stickers, emojis, max_file_bytes})` reads the
server and its stickers and/or emoji and downloads each file from the CDN
(no credentials are sent there):

- PNG and APNG stickers from `https://cdn.discordapp.com/stickers/<id>.png`,
  GIF stickers from `https://media.discordapp.net/stickers/<id>.gif` (the
  docs: "Sticker GIFs do not use the CDN base URL");
- static emoji from `https://cdn.discordapp.com/emojis/<id>.png` (WebP as
  fallback), animated emoji from `.../emojis/<id>.webp?animated=true` (the
  docs advise WebP for animations: WebP and AVIF uploads do not convert well
  to GIF).

Each becomes an `ImportedExpression` with the name, the description, the
emoji found in a sticker's tags (other tag words become keywords), whether
it is animated, the sniffed format, the bytes and `created_by_bot`. Skipped,
with a reason: Lottie stickers (`Lottie stickers are not supported`), emoji
managed by an integration, files over `max_file_bytes`, files that are not
PNG, GIF, JPEG or WebP. APNG stickers arrive as APNG; stickpile's ingest
keeps only their first frame (docs/media.md, sanitising rules).

An import must seed the export mapping only from expressions with
`created_by_bot`: a later export of the imported set to the same server then
keeps those instead of uploading copies, and never deletes the ones people
made. (Discord sends the uploader only when the bot has Create or Manage
Expressions.)

## Renditions

The crate checks files but does not render them. The server makes them in
the publish job from each item's stored original (`stickpile_media::pack`,
`service::renditions`); they are not stored as item variants. The sizes and
limits come from the crate's `requirements(kind)`, and every file passes
`check_file` before it is sent (one that does not is skipped with the
crate's reason):

| Kind | Still | Animation or video |
|---|---|---|
| Sticker | PNG, 320x320 (the longest side 320, centred on transparency); at most 512 KiB | APNG, looping, at most 320x320 and 5 s (longer ones are cut), at most 512 KiB |
| Emoji | PNG, 128x128; at most 256 KiB | GIF, looping, at most 128x128 and 10 s (stickpile's own cap: Discord gives none), at most 256 KiB |

A still is lossless RGBA PNG when that fits, else a PNG of 256 colours (at
these sizes the lossless one always fits). An animation is sampled at its
own frame rate, at most 20 frames per second; an APNG has one palette of
256 colours with alpha for every frame and later frames cover only what
changed, a GIF a palette per frame and on/off transparency (pixels at least
half opaque are drawn). Too large: fewer frames per second (15, 12, ...),
then a smaller square (down to a quarter of the side, 5 frames per second);
an item that does not fit even then is skipped with `does not fit the 512
KB limit of Discord stickers, even smaller and at 5 frames per second`.
Videos need ffmpeg on the server. An animation whose frames all look the
same becomes a still sticker. The job result lists what was cut
(`notes`: `cut to the first 5 seconds`) and what was left out (`skipped`).

## In the server

**Configuration** (`[discord]`, [configuration.md](configuration.md#discord)):
`enabled` and `bot_token_file`; the integration is on only when the file
reads as a token (`stickpile-server check-config` warns otherwise, naming
the file, never its content). `api_url`, `cdn_url` and `media_url` point
the tests at the mock.

**Publishing** is the `discord` platform of the publish registry
(`service::discord`, docs/api.md "Publishing"), next to Telegram and
Signal. Only the set's owner publishes. Options:

- `guild_id`: the server id (a number, or a `discord.com/channels/<id>`
  link). Required the first time; afterwards the server the set was
  published to last is used when it is left out.
- `kind`: `stickers` (default) or `emojis`; left out, the kind used last
  for that server.

The `discord_export` job renders the items for the kind, runs
`sync_guild` with the mapping stored for (set, server, kind) and stores the
new mapping in `set_publications.state` (also the partial mapping of a
failed sync: what it created is cleaned up by the next one). The set's
publication is the server's name with the link
`https://discord.com/channels/<server id>`. A publish to another server or
kind keeps the earlier mappings, so publishing back to a server keeps what
stickpile made there. The result has `guild_id`, `guild_name`, `kind`,
`url`, `added`, `removed`, `updated`, `unchanged`, `exported`, `skipped`,
`notes`, `slots`, `animated_slots` and `premium_tier`; the export page
shows it as `3 stickers: 2 added, 1 removed, 0 left out` (or `... emoji:
...`), with what was left out and why.

A 429 or a 5xx answer and network errors are retried by the job queue (up
to 3 attempts, with the mapping kept in between); a missing permission, an
unknown server (`Discord does not know this server, or the bot is not a
member of it`) or another refusal fails the job at once.

**Server ownership.** One bot serves every stickpile user and may edit
everything it created, so the first user who publishes to a server and
kind owns it (`discord_guilds`, migration 0013; the claim is made by the
job). Others get `another stickpile user publishes this kind (stickers or
emoji) to that Discord server` (400 when publishing). The same user may
publish several sets to one server and kind; each set keeps its own
mapping and touches only what it created.

**Import**: `POST /api/v1/discord/import` (`DiscordImport`: `guild_id`,
`stickers` default true, `emojis` default false, optional `slug` and
`visibility`) or the web page `/discord/import` ("Import from Discord", on
the sets list and the upload page) queue a `discord_import` job
(`import_guild`, files capped at `limits.max_upload_mb`). It creates a set
named after the server (slug from its name, `-2`, `-3`, ... when taken;
a slug given in the request must be free, 409 otherwise). Each sticker or
emoji becomes an item through the normal ingest: its name (an emoji's `_`
turned back into spaces), the emoji of a sticker's tags as the item's
emoji and the other tag words as keywords. Lottie stickers, emoji managed
by an integration, files that are too large and duplicates are listed in
`skipped`. The set's source stays `local` (the API's `SetSource` has no
`discord`).

Expressions the bot created seed the new set's mapping, so publishing the
set back to that server keeps them instead of uploading copies, but only
for the user who owns the server's publishing (or for an admin, who adopts
an unowned server and kind this way), and only expressions no other set
maps already. Anyone else gets a plain copy; the result's `bound` says how
many were linked.

**Disabled**: without `discord.enabled` and a readable token the platform
is listed as not configured (publishing to it is a 400), the import route
answers 404 and the import page does not exist, as for Telegram.

## Testing

`crates/stickpile-discord/tests/discord_api.rs` runs the client, the export
and the import against an in-process axum mock of the Discord API and CDN on
`127.0.0.1:0` (`tests/mock_discord`). The mock checks the bot token, keeps
stickers and emoji in memory, validates names, files and sizes, enforces
the slot limits with Discord's error codes (30039, 30008, 30018), answers
429 with `retry_after`, sends the rate-limit headers, injects failures and
records whether the CDN ever saw a token. The tests cover: the token never
in `Debug` output or errors, 429 retries with capped waits, waiting for an
exhausted bucket and for a global limit, sticker and emoji create, modify
and delete, export add/keep/update/remove, slot limits by boost level,
foreign stickers left alone, skipped files and duplicates, emoji slot
counting and unique names, items deleted by hand, a failed export's partial
mapping, and imports. The tests need no network access; the mock is this
crate's reading of the docs, not recorded Discord behaviour.

The server's tests (`crates/stickpile-server/tests/integrations_discord.rs`,
with a copy of the mock in `tests/integrations_support/mock_discord.rs`)
publish through the registry and the export page: create, keep, update and
remove; slot limits; emoji; owner-only publishing and server ownership; the
partial mapping of a failed publish; imports with and without adopting what
the bot made; the import page; and that the token is in no job error,
result, admin page or log line. `tests/renditions.rs` checks the
renditions of stills, animations and (with ffmpeg) a video with
`check_file`.
