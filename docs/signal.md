# Signal sticker packs

stickpile can:

- **import** a Signal sticker pack from its link into a new set;
- **upload** a set as a new Signal sticker pack, with the credentials of a
  Signal account, and return the pack's link;
- build a **Signal-ready zip** for people without credentials: the images
  and emoji for Signal Desktop's sticker creator, and a manifest signal-cli
  can upload as it is.

The implementation is `crates/stickpile-signal`. It is a library: the server
does not call it yet. Wiring it into the server is a later step through the
server's platform registry; [What the server will do](#what-the-server-will-do)
describes the plan. The format details below come from Signal's open source
code (cited per section) and were checked against encrypted packs from
Signal-Desktop's own test fixtures. The network side (service and CDN) was
tested only against an in-process mock, never against Signal's servers.

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
retried. A 401 or 403 means the credentials are wrong or the device was
unlinked.

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
| Stickers per pack | 1 to 200 (Signal-Desktop creator `MAX_STICKERS`; the service issues forms for at most 201 files: 200 and a cover) |
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
PKI, for the case Signal moves a host to a public CA. That the live hosts
present certificates under this root was not checked: the crate's tests and
scripts never contact Signal.

## What the server will do

Planned, not implemented in this crate:

- Import: `POST` with a pack link queues a job that calls `import_pack` and
  creates a set (title as name, author noted, stickers in order, each
  sticker's emoji as the item's emoji, cover as the set cover). The link is
  a secret for private packs: the server should not store or log it beyond
  what the job needs.
- Upload: configuration `signal.enabled` and `signal.credentials_file`
  (a path; the integration is on when the file reads as credentials). The
  set owner publishes; the job renders 512x512 stickers (WebP or PNG for
  stills, APNG for animations, none of which the current variants in
  docs/media.md provide), skips items `check_sticker` refuses with the
  reason, uploads, and stores the latest link on the set. Each publish is a
  new pack.
- The Signal-ready zip as a download for any set, with the same renditions
  and no credentials.

## Testing

`crates/stickpile-signal/tests/signal_api.rs` runs import, upload and the
zip against an in-process axum mock of Signal's service and CDN on two
`127.0.0.1:0` listeners (`tests/mock_signal`). The mock checks the Basic
auth user and password, issues upload policies like Signal-Server, checks
the CDN form fields and their order, the policy values and the size limits,
stores the encrypted files, answers 403 for missing files and 413 with
`Retry-After` when told to. The tests cover: encryption round trips and
MAC failures (unit tests), the manifest round trip and known bytes, both
link forms and bad links, import with missing, damaged and oversized files
and a separate cover, wrong keys, an upload whose link the import reads
back byte for byte, wrong credentials, the rate limit, invalid packs
refused before any request, credentials file formats, secrets kept out of
errors and `Debug`, the pinned certificate's fingerprint, and the zip (read
back through its central directory with CRC checks). The tests need no
network access.
