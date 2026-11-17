# WhatsApp sticker packs

WhatsApp has no API to publish sticker packs. A pack reaches WhatsApp
through an app on the phone: WhatsApp's own sample "sticker app" (a pack
bundled into an Android or iOS app) or a sticker maker app that imports
pack files. So stickpile does not publish to WhatsApp; it **builds the
packs as files** that anyone who can read the set downloads:

- on the set's export page, `/sets/{id}/export`, section "WhatsApp sticker
  packs" ([web.md](web.md#export-and-publish));
- with the API: `POST /api/v1/sets/{id}/whatsapp` builds them (a job),
  `GET /api/v1/sets/{id}/whatsapp` reports the last build,
  `GET /api/v1/sets/{id}/whatsapp.zip` and
  `GET /api/v1/sets/{id}/whatsapp/packs/{n}` download it
  ([api.md](api.md#whatsapp-packs));
- with the CLI: `stickpile export <set> --format whatsapp -o FILE`
  ([cli.md](cli.md)).

Code: `crates/stickpile-media/src/whatsapp.rs` (the renditions) and
`crates/stickpile-media/src/anim_webp.rs` (animated WebP),
`crates/stickpile-server/src/service/whatsapp.rs` (the job, packs, storage).

## WhatsApp's rules

From WhatsApp's public repository
[github.com/WhatsApp/stickers](https://github.com/WhatsApp/stickers),
branch `main`, read on 2026-11-16: `Android/README.md` ("Sticker art and app
requirements" and "Modifying the contents.json file"), `iOS/README.md`, the
sample `Android/app/src/main/assets/contents.json`, and the checks the
sample app runs before it hands a pack to WhatsApp,
`Android/app/src/main/java/com/example/samplestickerapp/StickerPackValidator.java`
(constants in brackets).

| Rule | Value | Where |
|---|---|---|
| Stickers per pack | 3 to 30 | README; `STICKER_SIZE_MIN = 3`, `STICKER_SIZE_MAX = 30` |
| Kinds | a pack is all static or all animated, never mixed | README; the validator wants more than one frame in every sticker of an animated pack (`animated_sticker_pack: true`) and exactly one in a static pack |
| Size | exactly 512 x 512 px | README; `IMAGE_WIDTH = IMAGE_HEIGHT = 512` |
| Format | WebP | README |
| Static sticker | at most 100 KB | README; `STATIC_STICKER_FILE_LIMIT_KB = 100`, `KB_IN_BYTES = 1024` |
| Animated sticker | at most 500 KB | README; `ANIMATED_STICKER_FILE_LIMIT_KB = 500` |
| Animated frames | each at least 8 ms | README; `ANIMATED_STICKER_FRAME_DURATION_MIN = 8` |
| Animated length | at most 10 s | README; `ANIMATED_STICKER_TOTAL_DURATION_MAX = 10 * 1000` |
| Tray icon | PNG, 96 x 96 px, at most 50 KB, static | README; the validator accepts 24 to 512 px (`TRAY_IMAGE_DIMENSION_MIN/MAX`) and `TRAY_IMAGE_FILE_SIZE_MAX_KB = 50` |
| Emoji | 1 to 3 per sticker | Android README ("required", "up to a maximum of three"); `EMOJI_MIN_LIMIT = 1`, `EMOJI_MAX_LIMIT = 3`. The iOS README calls them optional, at most 3 |
| Name, publisher | at most 128 characters each | README; `CHAR_COUNT_MAX = 128` |
| Identifier | at most 128 characters of `a-z A-Z 0-9 _ - .` and space | README |
| `image_data_version` | changes whenever the pack's images change | README |
| Accessibility text | optional, at most 125 (static) or 255 (animated) characters | README; stickpile does not write it |

The README also asks that the first frame of an animated sticker "say it
all", because WhatsApp ends the animation on it. stickpile cannot judge
that; it keeps the item's own first frame first.

## The `.wastickers` layout

WhatsApp does not define a file format for packs. `.wastickers` is the
convention of third-party sticker apps: [Sticker Maker by Viko &
Co.](https://fileinfo.com/extension/wastickers) opens these files, and
[sticker-convert](https://github.com/laggykiller/sticker-convert)
(`src/sticker_convert/uploaders/upload_whatsapp.py`) writes them for that
app. It is a ZIP archive with, at its root, `title.txt` (the pack's name),
`author.txt` (its publisher), the tray icon and the stickers.
sticker-convert names the tray icon `cover.png`; stickpile does the same.

stickpile writes, per pack, with stored (uncompressed) entries:

```
title.txt     pack name, UTF-8, no newline
author.txt    publisher, UTF-8, no newline
cover.png     tray icon, 96x96 PNG
01.webp       the stickers in set order, 512x512 WebP
02.webp
...
```

Not verified on a phone: no device was at hand, so importing these files
into Sticker Maker or WhatsApp is untested. The stickers themselves are
checked against every rule of the validator above by the tests.

The download (`whatsapp.zip`, named `<slug>-whatsapp.zip`) holds every pack
plus `contents.json`:

```
contents.json            the packs in the layout of WhatsApp's sample app
<slug>.wastickers        one pack, or
<slug>-1.wastickers      when the set is split, pack 1, 2, ...
<slug>-2.wastickers
```

`contents.json` has the sample app's fields (`identifier`, `name`,
`publisher`, `tray_image_file`, `image_data_version`, `avoid_cache`,
`animated_sticker_pack`, the empty link fields, and `stickers` with
`image_file` and `emojis`) plus `wastickers`, the file that holds the
pack's images. An app that bundles packs, like WhatsApp's sample, can read
it as it is.

## From items to stickers

Every item is made from its stored original:

- **Stills** (and the first frame of animations that do not move): scaled
  so the longer side is exactly 512 px (up or down, Lanczos3), centred on a
  transparent 512 x 512 square. Lossless WebP when that is at most 100 KB,
  else lossy WebP (sharp RGB to YUV) at the highest quality up to 95 that
  fits, found by binary search. An image that does not fit even at quality
  1 is left out (512 x 512 noise still fits, so this does not happen in
  practice).
- **Animations** (GIF, animated WebP): decoded in Rust like for the other
  renditions, resampled to a constant frame rate, each frame squared like a
  still, and encoded with libwebp's animation encoder (lossy, looping, a
  transparent background). Frame rate and quality are tried in this order
  until the file is at most 500 KB: 15 fps at quality 75, 12/60, 10/45,
  8/30, 6/20, 5/10 (never above the animation's own rate; frames are at
  least 66 ms, far above the 8 ms minimum). Only the first 10 s are kept,
  and such a sticker carries the note "cut to the first 10 seconds". When
  the first attempt is more than 12 times too large, the rest is skipped. An
  animation that never fits is left out ("does not fit WhatsApp's 500 KB
  limit for animated stickers, even at 5 frames per second"); animations
  whose frames all look the same become stills.
- **Videos** (MP4, WebM): ffmpeg decodes the first 10 s at up to 15 fps,
  scaled so the longer side is 512, into raw RGBA frames (VP9 WebM with
  alpha keeps its transparency); then as for animations. Without ffmpeg on
  the server, videos are left out ("animated stickers from videos need
  ffmpeg on the server").

**Packs.** Stills and animated stickers go into separate packs:

1. One or two animated stickers cannot make a pack of their own, so they
   are made into stills (their first frame, a video's frame at about 10 %)
   and join the still ones, noted "sent as a still sticker: a WhatsApp pack
   needs at least 3 animated stickers".
2. One or two still stickers next to at least three animated ones are left
   out, with the reason.
3. If neither kind has 3 stickers, the job fails: "nothing to export: a
   WhatsApp pack needs at least 3 stickers, ...".
4. Each kind is split into as few packs as possible, as even as possible:
   31 stickers make 16 + 15, never 30 + 1. Still packs come first; packs
   are numbered from 1 in set order.

**Names.** One pack: the set's name, identifier `<slug>`, file
`<slug>.wastickers`. Several: `<name> (2/3)`, `<slug>-2`,
`<slug>-2.wastickers`. The publisher is the set owner's display name (else
their username). Both are cut to 128 characters.

**Emoji.** The item's emoji, without entries that are not emoji (words
typed into the field), the first three; an item without any gets `🙂`,
like on Telegram.

**Tray icon.** Pack 1 shows the set's cover image or chosen cover item (its
thumbnail), if it has one; every other pack, and pack 1 of a set without a
cover, shows its own first sticker. It is scaled to fit 96 x 96 and centred
on transparency, as PNG (well below 50 KB: the raw pixels of a 96 x 96 image are 36 KiB).

**`image_data_version`** is the first 16 hex digits of a sha256 over the
pack's tray icon, stickers and emoji, so it changes exactly when they do.

## Storage and reuse

A build is kept in `<data_dir>/exports/<set id>/whatsapp/`: the zip and
`manifest.json` (the `WhatsappBuild` and a fingerprint of what it was made
from: the items' ids, media, names and emoji in order, the set's name,
slug, cover, the owner's name, and whether ffmpeg is available). A new
build of an unchanged set returns the stored one (`reused: true`); the
status's `current` says whether the stored one still matches. A new build
replaces the files atomically, so downloads in progress finish. The
periodic cleanup removes the exports of deleted sets. Everything under
`exports/` can be made again; backups need not include it.

## Tests

- `crates/stickpile-media/tests/pipeline.rs` (`whatsapp_*`): animated
  stickers from GIF and WebP animations and from MP4 and VP9 WebM (with
  alpha) videos, a 12 s video cut to 10 s, the limits of the validator
  (512 x 512, 500 KB, frames of at least 8 ms, at most 10 s), the still
  fallback, padding of a non-square still, and an animation of noise that
  fits at no rate (`TooLarge`). Unit tests cover the frame plan, squaring,
  the 100 KB squeeze, the tray icon and the libwebp encoder.
- `crates/stickpile-server/tests/export.rs`: a set of 32 items split into
  two packs of 16, every sticker and tray icon checked like the validator,
  emoji filtered to 3, the wide item padded, `contents.json`, single pack
  downloads, reuse of an unchanged build and a rebuild after an edit, an
  animated pack leaving out a lonely still, a set of two failing, a queued
  build returned again, readers allowed and others 404, the cleanup after
  the set is deleted.
