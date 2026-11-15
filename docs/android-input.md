# Android insert matrix

On Android the picker keyboard inserts a sticker with
`InputConnection.commitContent`: the chat app's text field lists the MIME
types it accepts (`EditorInfo.contentMimeTypes`), the keyboard commits a
`content://` URI of a type from that list, and the app decides what to do
with it. What each app lists and does cannot be learned from a build. This
page holds the matrix to fill in by hand on a phone, the expectations it
starts from, and the procedure.

> **Status (2026-11-15):** no cell of the matrix has been observed (see
> "Observed on the emulator" for the one app run so far). Every cell is
> **expected (UNVERIFIED)**: the rendition is what `variant_for` picks
> (`crates/stickpile-core/src/insert.rs`, its `PREFERENCES` table) for the
> MIME types the field is assumed to list; the outcome is reasoning from
> how the desktop apps behave (`docs/clipboard.md`) and from the apps'
> public source where named. Treat it as a hypothesis until a cell has a
> result.

## Observed on the emulator

The only app run so far (2026-11-15): AOSP Messaging
(`com.android.messaging`) on the Android Emulator's API 36 x86_64 system
image (the AOSP "default" image, without Google apps), with the debug build
of the `android` branch, signed in to a local test server. This is not a
phone and not one of the apps in the matrix.

- The picker opened in a conversation's message field, read the draft
  `cat` and showed the search's results with their thumbnails. With the
  emulator's hardware keyboard attached (and Android's "show on-screen
  keyboard" off) it showed after a tap on the field.
- The field lists no content types: picking an item never reached
  `commitContent`. The keyboard sent the file with `ACTION_SEND` to
  Messaging, which opened its "Select conversation" screen and logged
  `Cannot send private file content://io.github.dolfost.stickpile.stickers/prepared/...`
  (with an SELinux `getattr` denial on `/data/data/io.github.dolfost.stickpile`):
  it refuses a URI whose file lies in another app's private storage. No
  sticker was sent.
- With the share screen cancelled, the draft `cat` stayed in the field.
  After choosing the conversation on Messaging's share screen the field
  was empty (the cause was not established).
- The keyboard switched back to the previous keyboard after the hand-off
  when the picker had been chosen with the keyboard's globe key; when it
  had been selected with `adb shell ime set`, the system's keyboard
  switcher opened instead (no previous keyboard to go back to).

Not observed: `commitContent` into any field (no app on the image takes
content), and every cell of the matrix below.

## How the keyboard decides

For one pick (docs/android.md, "Choosing what to insert"):

1. `target_for_package(editor package)` names the chat app (Telegram and
   its forks, Discord, Signal and Molly, WhatsApp, Slack; anything else is
   generic).
2. `variant_for(item, target, field's MIME types, sticker)` walks the
   target's row of `PREFERENCES` and takes the first rendition of the item
   whose type the field lists; none means "nothing fits".
3. The keyboard commits it with `commitContent`. When nothing fits or the
   app refuses the commit, it sends the file to the app with `ACTION_SEND`
   (the app's share screen); failing that, it copies the URI to the
   clipboard and says so.

The renditions: the original, a PNG of a still, the 512 px sticker WebP
(`tg_sticker`), the WebM video sticker (`tg_video_sticker`), an MP4 of an
animation or video, a GIF (`discord_gif`). An item has only those the
server made for it.

## The matrix

Columns: what is picked. *Still* is a PNG or JPEG item, *animation* a GIF or
animated WebP, *video* an MP4 or WebM, *sticker* any item picked with "Send
as sticker" (a long press in the keyboard).

| App (package) | Still | Animation | Video | Sticker |
|---|---|---|---|---|
| Telegram (`org.telegram.messenger`) | expected (UNVERIFIED): field lists `image/*`, gets the original; sent as a photo | expected (UNVERIFIED): the original GIF (MP4 when the field lists video first); sent as an animation (GIF) | expected (UNVERIFIED): the original MP4, if the field lists `video/*`; else nothing fits and the share screen opens | expected (UNVERIFIED): the 512 px WebP for a still; whether Telegram sends a committed WebP as a sticker is unknown (on the desktop only "send as file" does, docs/clipboard.md 2.1) |
| WhatsApp (`com.whatsapp`) | expected (UNVERIFIED): field lists images (GIF and PNG, assumed), gets the original or the PNG; sent as a photo | expected (UNVERIFIED): the MP4 if listed (WhatsApp turns GIFs into MP4s), else the GIF | expected (UNVERIFIED): the share screen (the field is assumed to take no video) | expected (UNVERIFIED): as a still; WhatsApp stickers come from sticker packs (phase 6), not from committed images |
| Signal (`org.thoughtcrime.securesms`), Molly (`im.molly.app`) | expected (UNVERIFIED): the original; sent as a photo | expected (UNVERIFIED): the GIF; sent as a GIF | expected (UNVERIFIED): the share screen | expected (UNVERIFIED): as a still (no stickers from a keyboard) |
| Discord (`com.discord`) | expected (UNVERIFIED): the original; uploaded as a file | expected (UNVERIFIED): the GIF (preferred for Discord, docs/clipboard.md D5, D6); animated | expected (UNVERIFIED): the MP4 if listed, else the share screen | expected (UNVERIFIED): as a still |
| Slack (`com.Slack`) | expected (UNVERIFIED): the original or the PNG (Slack takes no WebP still); uploaded as a file | expected (UNVERIFIED): the GIF | expected (UNVERIFIED): the share screen | expected (UNVERIFIED): as a still |
| Google Messages (`com.google.android.apps.messaging`) | expected (UNVERIFIED): generic target: the original; sent as MMS/RCS media | expected (UNVERIFIED): the GIF | expected (UNVERIFIED): the MP4 if listed | expected (UNVERIFIED): as a still |
| A browser text field (Firefox, Chrome) | expected (UNVERIFIED): most web fields list no content types: nothing fits, the share screen opens, else the clipboard | expected (UNVERIFIED): as a still | expected (UNVERIFIED): as a still | expected (UNVERIFIED): as a still |

## Filling it in

You need: a phone with the app installed and signed in to a server that has
a still (PNG), an animated GIF, a short MP4 and a still with a sticker
rendition (an item from a Telegram pack, or one the server made a sticker
for), and the chat apps above.

For each app and each column:

1. Open a chat, tap the message field, switch to the stickpile keyboard.
2. Pick the item (long press, "Send as sticker", for the sticker column).
3. Record:
   - what the keyboard did: inserted, opened the share screen, or copied
     (it says which);
   - what the app did with it: sent as a photo, a GIF, a video, a sticker, a
     file; asked to confirm; refused;
   - for a commit, which rendition went in, if the chat shows it (a GIF
     playing, a video player, a sticker without a bubble).
4. Replace "expected (UNVERIFIED)" with "observed (date, app version)" and
   the result.

When a result contradicts the expectation, change the target's row in
`PREFERENCES` (`crates/stickpile-core/src/insert.rs`); it is the one place
the order lives. The keyboard needs no change.
