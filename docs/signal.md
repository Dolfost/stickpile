# Signal sticker packs

stickpile can:

- **import** a Signal sticker pack from its link into a new set;
- **upload** a set as a new Signal sticker pack, with the credentials of a
  Signal account, and return the pack's link; a set of more than 200
  stickers becomes several packs ([Sets larger than one
  pack](#sets-larger-than-one-pack));
- build a **Signal-ready zip** for people without credentials: the images
  and emoji for Signal Desktop's sticker creator, and a manifest signal-cli
  can upload as it is.

The Signal side is `crates/stickpile-signal`; the server uses it through
its platform registry ([In the server](#in-the-server): configuration, the
export page, jobs, API routes). The format details below come from Signal's
open source code (cited per section) and were checked against encrypted
packs from Signal-Desktop's own test fixtures. The network side (service
and CDN) was tested only against an in-process mock, never against
Signal's servers.

## Links

A pack is identified by a 16-byte id and opened with a 32-byte key, both in
hex:

```
https://signal.art/addstickers/#pack_id=<32 hex>&pack_key=<64 hex>
sgnl://addstickers/?pack_id=<32 hex>&pack_key=<64 hex>
```

In the web form the parameters are in the fragment (after `#`), so they
never reach signal.art; in the app form they are in the query. The trailing
slash is optional in both (Signal-Desktop `ts/util/signalRoutes.std.ts`,
`artAddStickersRoute`). `PackLink::parse` accepts both, either case of hex,
and refuses anything else (wrong host or scheme, missing or malformed
parameters). **The key is the secret**: whoever has the link can read the
pack. `PackLink` and `PackKey` keep the key out of `Debug` output and
errors; only `PackLink::url()` and `app_url()` write it out.

## Import

`import_pack(client, link, max_file_bytes)`:

1. Downloads `https://cdn.signal.org/stickers/<pack id>/manifest.proto`
   (Signal-Desktop `ts/textsecure/WebAPI.preload.ts`,
   `getStickerPackManifest`; the CDN host is `cdn.0` in
   `config/production.json`). Signal's CDN answers **403** for a pack that
   does not exist (signalstickers-client `downloader.py`); that becomes
   "sticker pack not found". Downloads are capped at 1 MiB, Signal-Desktop's
   limit for both kinds of file.
2. Decrypts it (see [Encryption](#encryption)). A MAC mismatch on the
   manifest means the key does not fit: the import fails.
3. Parses the manifest (see [Manifest](#manifest)): title, author, cover,
   and the stickers with their id and emoji.
4. Downloads and decrypts each sticker in manifest order from
   `.../stickers/<pack id>/full/<sticker id>` (`getSticker`). Stickers
   arrive as WebP (what Signal Desktop's creator makes of still images),
   PNG or APNG.
5. Downloads the cover when it is not one of the stickers (Signal Desktop's
   creator uploads a separately chosen cover as an extra file with the next
   id).

The result has the title, author, stickers (`id`, `emoji`, the manifest's
content type, the sniffed format, `animated`, bytes), `cover_id` (the first
sticker when the manifest names none), the separate `cover` if any, and
`skipped` with reasons: a file missing on the CDN, a file whose MAC fails, a
file over `max_file_bytes`, a file that is not an image, an id listed twice.

## Encryption

Every file of a pack, the manifest included, is encrypted with keys derived
from the pack key:

- HKDF-SHA256, salt 32 zero bytes, info `Sticker Pack`, 64 bytes of output:
  the first 32 are the AES-256 key, the last 32 the HMAC-SHA256 key;
- a file is `IV (16 bytes) | AES-256-CBC ciphertext (PKCS#7 padding) |
  HMAC-SHA256 over IV and ciphertext (32 bytes)`;
- the MAC is checked (in constant time) before anything is decrypted.

Sources: Signal-Desktop `ts/Crypto.node.ts` (`deriveStickerPackKey`,
`decryptAttachmentV1`), `sticker-creator/src/util/crypto.ts` (`encrypt`,
`encryptAttachment`: a random 32-byte pack key, a random IV per file);
signalstickers-client `classes/signalcrypto.py` (the same, with HKDF's
default salt, which RFC 5869 defines as zero bytes). The opt-in test
`tests/signal_desktop_fixtures.rs` decrypts the two encrypted packs in
Signal-Desktop's `fixtures/` directory with the keys from its
`ts/test-mock/storage/fixtures.node.ts`; both manifests and stickers
decrypt and parse ("Test Stickerpack" and "Demo Stickerpack", one 512x512
WebP sticker each). The fixture files are not in this repository; the
test's comment says how to run it.

## Manifest

The decrypted manifest is a protobuf (proto2) message:

```proto
message Pack {
  message Sticker {
    optional uint32 id          = 1;
    optional string emoji       = 2;
    optional string contentType = 3;
  }
  optional string  title    = 1;
  optional string  author   = 2;
  optional Sticker cover    = 3;
  repeated Sticker stickers = 4;
}
```

Signal-Android's `StickerResources.proto` has `contentType`; Signal-Desktop's
`protos/Stickers.proto` has the same message without it (unknown fields are
skipped, so each reads the other's manifests). The crate encodes and
decodes it by hand (`Manifest::encode`, `Manifest::decode`), skipping
unknown fields and refusing truncated data and groups.

## Upload

`upload_pack(client, credentials, pack)` creates a **new** pack and returns
its link. Signal packs cannot be changed after upload: publishing a set
again uploads another pack with a new id and key, and the old link keeps
showing the old pack. People who installed the old pack do not get the new
stickers.

Steps (Signal-Desktop `putStickers` and the sticker creator's `encrypt`;
signalstickers-client `uploader.py`; Signal-Server `StickerController`):

1. The pack is checked first (see [Limits](#limits)); nothing is sent when
   it is invalid.
2. `GET https://chat.signal.org/v1/sticker/pack/form/<n>` with HTTP Basic
   auth returns the new pack id and signed S3 upload policies: one for the
   manifest (`stickers/<pack id>/manifest.proto`, at most 10 KiB) and one
   per file (`stickers/<pack id>/full/<i>`, at most 301 KiB). `<n>` is the
   number of stickers, plus one for a separate cover (1 to 201).
3. A fresh random pack key; the manifest and every file are encrypted with
   fresh random IVs.
4. The manifest and then each file are POSTed to `https://cdn.signal.org/`
   as `multipart/form-data` with the policy fields in the order
   Signal-Desktop sends them: `key`, `x-amz-credential`, `acl`,
   `x-amz-algorithm`, `x-amz-date`, `policy`, `x-amz-signature`,
   `Content-Type: application/octet-stream`, then `file`. The CDN gets no
   account credentials.
5. The result is `https://signal.art/addstickers/#pack_id=...&pack_key=...`.

The manifest lists the stickers with ids `0..n` in order, one emoji each and
their content type (`image/webp` or `image/png`). The cover is the first
sticker, or the given cover image: when it equals one of the stickers it
points at that sticker, otherwise it is uploaded as file `n` (as Signal
Desktop's creator does).

**Rate limit.** The service gives an account a burst of 50 upload forms,
then one every 72 minutes (Signal-Server `RateLimiters`, `stickerPack`).
Signal answers 413 (older servers) or 429 with `Retry-After`; the upload
fails with "Signal rate limit reached; try again in N s" and is not
retried. Every pack needs a form of its own, so uploading a set as k packs
([Sets larger than one pack](#sets-larger-than-one-pack)) takes k forms. A
401 or 403 means the credentials are wrong or the device was unlinked.

### Credentials

Uploading needs the credentials of a registered Signal account or of a
device linked to one: the account's ACI (a UUID), the device id and the
device's password. The service authenticates with HTTP Basic, user
`<ACI>.<device id>` (Signal-Server `BasicAuthorizationHeader`,
`AccountAuthenticator`).

**Use a dedicated account.** The password authenticates a device of the
account to Signal's service: anyone who has it can make authenticated
requests to the service as that device. (Messages stay end-to-end
encrypted; reading them needs keys the extracted file below does not
contain.) Signal can tell which account asked for each upload form. So register a separate Signal account (its own phone
number) for stickpile rather than using or linking your personal one, keep
the file readable only by the service user, and remove it if you stop
publishing.

`Credentials::from_file(path)` reads a JSON file in one of these shapes:

```json
{"aci": "8f3a1c2e-1b2c-4d5e-8f90-123456789abc", "device_id": 1, "password": "..."}
```

```json
{"username": "8f3a1c2e-1b2c-4d5e-8f90-123456789abc.1", "password": "..."}
```

or a signal-cli account file itself (`aciAccountData.serviceId`, `deviceId`,
`password`; `uuid` instead of `aciAccountData` in signal-cli's older storage
versions; `serviceEnvironment` must be `LIVE` and `registered` true). Errors
name the file, never its content; the password is kept out of `Debug`
output.

With [signal-cli](https://github.com/AsamK/signal-cli) and a dedicated
number:

```sh
signal-cli -a +15550000000 register          # or: register --voice
signal-cli -a +15550000000 verify 123456
# find the account's data file
jq -r '.accounts[] | "\(.number) \(.path)"' ~/.local/share/signal-cli/data/accounts.json
# copy only what stickpile needs into a file of its own
jq '{aci: .aciAccountData.serviceId, device_id: .deviceId, password: .password}' \
  ~/.local/share/signal-cli/data/<path> \
  | install -m 0400 -o stickpile /dev/stdin /run/secrets/stickpile-signal-credentials.json
```

The signal-cli account file also holds the account's identity private key;
give stickpile the extracted file rather than access to signal-cli's data
directory. signal-cli stores its data in `$XDG_DATA_HOME/signal-cli/data/`
(by default `~/.local/share/signal-cli/data/`). `signal-cli link` makes
signal-cli a linked device of an existing account instead; that hands
stickpile a device of that account, so do it only with an account kept for
this purpose. The credentials stay valid until the device is unlinked or
the account registered again elsewhere.

Signal Desktop users can read the same two values from its storage
(`uuid_id` and `password`, as the signalstickers-client README describes);
that is the personal account's device and is not recommended.

## Limits

| | |
|---|---|
| Stickers per pack | 1 to 200 (Signal-Desktop creator `MAX_STICKERS`; the service issues forms for at most 201 files: 200 and a cover); a larger set is several packs ([below](#sets-larger-than-one-pack)) |
| File size | 300 KiB each (`MAX_STICKER_BYTE_SIZE`; the service allows 301 KiB of ciphertext) |
| Still stickers | PNG or WebP, at most 512x512 (render exactly 512x512; the creator draws every still image onto a 512x512 canvas) |
| Animated stickers | APNG only (not GIF, not animated WebP), square, at most 512x512, looping forever, at most 3 s |
| Emoji | one per sticker; stickpile uses the item's first emoji, else `🙂` |
| Title and author | required; stickpile cuts them to 128 characters so the encrypted manifest stays under the service's 10 KiB |

Sources: Signal-Desktop `sticker-creator/src/constants.ts`,
`util/processImage.ts` (APNG: square, 10 to 512 px, at most 300 KiB, loops
forever; other images resized to 512x512 WebP) and `util/useStickerDropzone.ts`
(PNG, WebP, APNG); Signal's support article "Stickers" (APNG for
animations, 300 KB, 3 seconds, 512x512). `check_sticker(bytes)` checks a
file against these rules from its header; `STICKER_REQUIREMENTS` states
them for the renderer.

## Sets larger than one pack

A Signal pack holds at most 200 stickers. A set with more is uploaded as
several packs, its **parts** (`crates/stickpile-signal/src/parts.rs`, the
same word and titles as for [Telegram](telegram.md#sets-larger-than-one-pack)).
A set of at most 200 stickers is one part and uploads exactly as a single
pack did.

**Splitting.** `split_parts` takes the stickers in set order and fills the
packs one after the other: part 1 gets the first 200, part 2 the next 200,
and so on (450 stickers: 200, 200 and 50). The items that cannot become
stickers are left out before that, once for the whole set, so they take no
place in any part.

**Titles.** Part 1's title is the set's name; part k's is `<name> <k>`
(`Cats`, `Cats 2`, `Cats 3`). Both have at most 128 characters
(`part_title`): a longer name is cut on characters (not bytes), before
` <k>`, so the number stays. Every part has the same author.

**Cover.** Every part gets the set's cover, uploaded with each part as a
file of its own after its stickers; a set without a cover shows each
part's own first sticker.

**No stability rule.** Signal packs cannot be changed, so nothing has to
stay where it was: every publish uploads every part as a new pack with a
new link, and which sticker lands in which part follows only the set's
order at that publish. People who added an older part keep it as it was.

**Uploading.** `upload_parts` checks every part first: when one is invalid
nothing is sent, and the error names the part (`part 2 of 3: sticker 1:
...`). Then it uploads the parts by number, each like a single pack (an
upload form each, see the rate limit above), and stops at the first
failure. It returns the parts uploaded, each with its number, title and
link, and the part that failed with its error; the parts after it are not
tried.

**A failed upload** leaves the parts before it on Signal: stickpile cannot
delete a Signal pack. Their links are only in the failed job's result
([In the server](#in-the-server)), so nobody gets them unless that link is
handed out. Uploads are not resumed: the next publish, or a retry of the
same job, uploads every part anew, and the packs of the failed attempt stay
unused.

## Signal-ready zip

`signal_ready_zip(pack)` (or `signal_ready_files(pack)` for the file list)
builds, with the same checks as an upload:

```
001.webp  002.png  ...   the stickers in pack order
cover.webp               a separate cover, if any
manifest.json            for signal-cli
emoji.txt                "001.webp 😼" per line
README.txt               how to use the folder, title, author, cover
```

- **Signal Desktop**: File > Create/upload sticker pack. The creator takes
  PNG, WebP and APNG files by drag and drop, then asks for one emoji per
  sticker, a title, an author and a cover. It reads no list or manifest, so
  `emoji.txt` says which emoji to pick for each image. Uploading from there
  uses the Signal account linked to Signal Desktop.
- **signal-cli**: `signal-cli -a <number> uploadStickerPack pack.zip` reads
  `manifest.json` from the zip: `{"title", "author", "cover": {"file",
  "emoji", "contentType"}, "stickers": [{"file", "emoji", "contentType"}]}`
  with paths relative to it (signal-cli `StickerUtils.java`,
  `JsonStickerPack.java`). It prints the new pack's link. Without a separate
  cover the manifest names the first sticker's file as the cover, which
  signal-cli uploads once more as the cover file.

The zip is stored without compression (the images are compressed already)
with fixed timestamps, so the same pack gives the same bytes.

**Several packs.** `signal_ready_parts_files(parts, folder)` writes a set
of several parts as one archive with a folder per part, each with the
layout above (its own stickers numbered from `001`, manifest, `emoji.txt`
and `README.txt`), and a `README.txt` at the top naming the folders:

```
cats/  Cats (200 stickers)
cats-2/  Cats 2 (200 stickers)
cats-3/  Cats 3 (50 stickers)
```

The server names the folders like WhatsApp's packs: `<slug>`, `<slug>-2`,
`<slug>-3`, ... ([whatsapp.md](whatsapp.md#from-items-to-stickers)). Each
folder is made into a pack of its own: in Signal Desktop by dragging that
folder's images in, with signal-cli by giving it that folder's
`manifest.json` once the zip is unpacked. One part is written as above,
without a folder.

## TLS

Signal's clients trust only Signal's own root certificate for these hosts:
Signal-Desktop passes `certificateAuthority` from `config/default.json` as
the `ca` of its CDN requests, which replaces Node's default roots, and
signalstickers-client verifies both `chat.signal.org` and `cdn.signal.org`
against the same certificate (`utils/ca/cacert.pem`). The crate pins it
(`SIGNAL_ROOT_CA_PEM`, "Signal Messenger, LLC", valid 2022-01-26 to
2032-01-24, SHA-256 of the DER
`ddb0f92bb95c8d6fd202ea6e8cc5ccd182b544f8cd696f47d580659ddc9df65a`) as the
only trust anchor. `SignalConfig::trust_public_roots` adds the public web
PKI, for the case Signal moves a host to a public CA (the server's
`signal.trust_public_roots`, off by default). That the live hosts present
certificates under this root was not checked: the crate's tests and scripts
never contact Signal.

## In the server

**Configuration** (`[signal]`, [configuration.md](configuration.md#signal)):

- `enabled` turns on pack import from links, which needs no account;
- `credentials_file` (a path to a credentials file, see
  [Credentials](#credentials)) also turns on publishing: Signal counts as
  configured only when the file reads as credentials
  (`stickpile-server check-config` warns otherwise, naming the file, never
  its content);
- `service_url` and `cdn_url` point the tests at the mock;
  `trust_public_roots` see [TLS](#tls).

**Renditions**, made from each item's stored original where they are
needed (the publish job and the zip below; `stickpile_media::pack`,
`service::renditions`), never stored as variants. Sizes and limits come
from the crate's `STICKER_REQUIREMENTS`, and every file passes
`check_sticker` (one that does not is left out with the crate's reason):

- stills: 512x512 (the longest side 512, centred on transparency), WebP,
  lossless when it fits 300 KiB, else lossy at the highest quality that
  fits;
- animations and videos (videos need ffmpeg): APNG looping forever, at
  most 3 s (longer ones are cut, noted as `cut to the first 3 seconds`), at
  most 20 frames per second, one palette of 256 colours with alpha, later
  frames covering only what changed. Too large: fewer frames per second,
  then a smaller square (Signal takes squares up to 512), down to 128x128
  at 5 frames per second; past that the item is left out (`does not fit the
  300 KB limit of Signal stickers, even smaller and at 5 frames per
  second`). An animation whose frames all look the same becomes a still;
- the emoji: the item's first emoji, else `🙂` (`sticker_emoji`); more
  than 200 stickers are several packs
  ([Sets larger than one pack](#sets-larger-than-one-pack)); until 0.8.8
  the rest was left out (`a Signal pack holds at most 200 stickers`);
- the cover: the set's cover image or chosen cover item as a still, else
  the first sticker (of each part); the title is the set's name (part k:
  `<name> <k>`), the author the owner's display name (else the username).

**Publishing** is the `signal` platform of the publish registry
(`service::signal`, docs/api.md "Publishing"); it takes no options. Only
the set's owner publishes. The `signal_export` job uploads a **new** pack
every time (packs cannot be changed), or new packs for every part of a set
of more than 200 stickers: the newest `signal.art` link (part 1's) becomes
the set's publication link (`set_publications`, with the pack id in
`state`), and the export page says so next to the row ("Signal packs
cannot be changed: every publish uploads new packs with new links. People
who added an older pack keep its stickers. A pack holds at most 200
stickers, so a larger set becomes several packs."). Older links keep
working and keep showing the older stickers. The result has `url`,
`title`, `exported`, `skipped` and `notes`; the page shows it as
`3 stickers in a new pack, 0 left out`. A rate limit (a burst of 50 forms,
then one every 72 minutes) and refused credentials fail the job at once
with the crate's message; network errors and 5xx answers are retried.

**Several parts.** For a set of more than one part the result keeps those
fields for part 1 (`url`, `title` and `exported` are part 1's link, the
set's name and part 1's sticker count; `skipped` and `notes` are the whole
set's) and adds `parts`, every part with its `number`, `title`, `url` and
`stickers`:

```json
{"set_id": "...", "title": "Cats", "url": "https://signal.art/addstickers/#pack_id=...&pack_key=...",
 "exported": 200, "skipped": [], "notes": [],
 "parts": [
   {"number": 1, "title": "Cats", "url": "https://signal.art/addstickers/#pack_id=...", "stickers": 200},
   {"number": 2, "title": "Cats 2", "url": "https://signal.art/addstickers/#pack_id=...", "stickers": 200},
   {"number": 3, "title": "Cats 3", "url": "https://signal.art/addstickers/#pack_id=...", "stickers": 50}]}
```

The page shows it as `450 stickers in 3 new packs, 0 left out`. The set's
`Publication` keeps part 1's link as `url` (and the set's name as
`target`), and its `parts` list every part of the last successful publish
(`number`, the title as `name`, `url`, `stickers`; kept in
`set_publications.state`); for one pack `parts` is empty, as from a server
before parts, and a 0.8.8 client reads the publication as before.

**A failed publish of several parts** fails the job with the part and what
happened, e.g. `part 2 of 3: Signal rate limit reached; try again in 4320 s
(part 1 was uploaded; its link is in the job's result)`. The failed job
keeps a result: the fields above, with the parts it uploaded and, last, the
part that failed with its `error` and no `url`. The links are there and not
in the error because errors are logged and a link holds its pack's key.
The set's publication stays that of its last complete publish (`url`,
`parts`, `last_result`); `last_status` and `last_error` say that the last
one failed. The packs uploaded before the failure stay on Signal, unused.
A retried job (network errors, 5xx) uploads every part anew, and its result
is what the last attempt did.

**Import**: `POST /api/v1/signal/import` (`SignalImport`: `link` in
either form, optional `slug` and `visibility`) or the web page
`/signal/import` ("Import a Signal pack", on the sets list and the upload
page) queue a `signal_import` job. A link that does not parse is a 400
before anything is queued. The job (`import_pack`, files capped at
`limits.max_upload_mb`) creates a set named after the pack's title (slug
from it, `-2`, `-3`, ... when taken), adds the stickers in order as
`sticker 1`, `sticker 2`, ... with their emoji, and makes the pack's cover
the set's cover (the cover sticker's item, or the separate cover image).
Stickers that are missing, damaged or duplicates are listed in `skipped`;
APNG stickers keep their first frame (the ingest's rule). The result has
`set_id`, `slug`, `title`, `author`, `imported`, `skipped` and `cover`.
The set's source stays `local` (the API's `SetSource` has no `signal`).

**The link is a secret** (its key opens the pack): it is in the job's
payload only while the job may run and is removed from it when the job
finishes, done or failed; the job has no subject naming the pack, and
neither the result nor errors nor logs contain the key. The web form does
not show a refused link back.

**The Signal-ready zip**: `GET /api/v1/sets/{id}/signal.zip`, for anyone
who can read the set, whether Signal is configured or not, and the
"Signal-ready zip" button in the export page's Download section (which
says, when the server does not publish to Signal, that this is the way to
make a pack). It is `signal_ready_files` of the same renditions plus a
`left-out.txt` naming the items that are not in it, stored without
compression; a set of more than 200 stickers gets one folder per part,
`<slug>/`, `<slug>-2/`, ... ([Several packs](#signal-ready-zip)), with
`README.txt` and `left-out.txt` at the top. The download is
`<slug>-signal.zip` either way. Made on the first request after a change
(that counts as an upload for rate limiting; a large animated set takes a
while), it is kept in `<data_dir>/exports/<set id>/signal/` and served
again while the items, their emoji, the set's name, cover and owner name
are unchanged. A set with nothing to put in it answers 400.

**Disabled**: without `signal.enabled` the import route answers 404 and the
import page does not exist; without readable credentials the platform is
listed as not configured and publishing to it is a 400, as for Telegram.

## Testing

The server's tests (`crates/stickpile-server/tests/integrations_signal.rs`,
with a copy of the mock in `tests/integrations_support/mock_signal.rs`)
publish through the registry (a new pack each time, read back with
`import_pack` and checked with `check_sticker`), the rate limit and wrong
credentials, imports from both link forms with the cover and the link
removed from the finished job, the Signal-ready zip with and without Signal
configured, the import page, and the routes when Signal is off. Sets
larger than one pack (`signal_*` tests, items carrying an emoji of their
own so a pack read back shows which item each sticker is): 200 items as one
pack exactly as before, 450 as three packs with numbered titles and every
item once in set order, `Publication.parts` and the set's cover in every
part, a new link for every part on each publish and no parts once the set
fits one pack, a rate limit at part 2 (the error, the failed job's result
with part 1's link, the publication left as it was), a 5xx at part 2
retried with every part uploaded anew, the zip with a folder per part and
an item left out of the whole set, and a 0.8.8 client's `Publication`
reading the new answer (and a 0.8.8 row read by the new server).

The crate's own tests:

`crates/stickpile-signal/tests/signal_api.rs` runs import, upload and the
zip against an in-process axum mock of Signal's service and CDN on two
`127.0.0.1:0` listeners (`tests/mock_signal`). The mock checks the Basic
auth user and password, issues upload policies like Signal-Server, checks
the CDN form fields and their order, the policy values and the size limits,
stores the encrypted files, answers 403 for missing files and 413 with
`Retry-After` when told to, and fails a chosen form request
(`fail_form_after`). The tests cover: encryption round trips and
MAC failures (unit tests), the manifest round trip and known bytes, both
link forms and bad links, import with missing, damaged and oversized files
and a separate cover, wrong keys, an upload whose link the import reads
back byte for byte, wrong credentials, the rate limit, invalid packs
refused before any request, credentials file formats, secrets kept out of
errors and `Debug`, the pinned certificate's fingerprint, the zip (read
back through its central directory with CRC checks), and parts: 200, 201
and 450 stickers uploaded and read back pack by pack (titles, covers,
order, one form each), a failure at part 2 (part 1 uploaded, part 3 not
tried), an invalid part refused before any request, the zip with a folder
per part; `src/parts.rs` has unit tests for the split and the titles. The
tests need no network access.
