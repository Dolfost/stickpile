# Android app

> **Status:** built and tested on the host (Rust tests, JVM unit tests,
> lint), not yet run on a phone or an emulator. What the keyboard inserts
> into each chat app is still to be tried by hand: see
> [android-input.md](android-input.md).

On Android stickpile is not an app you open to copy something. A keyboard
(input method) can insert a picture straight into the focused text field
with `InputConnection.commitContent`, and the field says which MIME types
it accepts. So the app is:

1. **An engine** that owns the account, the sync, the offline cache and the
   files, and serves them to keyboards through a content provider.
2. **A picker keyboard** in the same app: an input method that inserts
   stickers. Switch to it from any keyboard, search or browse, tap a
   sticker, and it switches back. Its keys type only into its own search.
3. **A share target**: "Share -> stickpile" uploads a picture into a set.

Layout: `crates/stickpile-android` (the Rust library, behind UniFFI),
`android/` (the Gradle project, one module `app`), `nix/android-shell.nix`
(`nix develop .#android`).

## Install

It needs Android 8.0 (API 26) or later and a stickpile server
([deployment.md](deployment.md)); there is no public server.

- **From a release:** each GitHub release has
  `stickpile-vX.Y.Z-android-unsigned.apk`. Android installs only signed
  APKs: sign it first ([Release signing](#release-signing)) or build it
  ([Build from source](#build-from-source)).
- **F-Droid:** planned, not submitted yet ([F-Droid](#f-droid)).

The app keeps its files in its private storage, outside backups and device
transfer: the API token never leaves the phone.

## Sign in

1. In the web UI, open Settings, "Add a device". It shows a QR code and a
   code like `7KQ2-M9XA-4FTR`, valid for five minutes and once.
2. In the app, "Scan a QR code" (the camera permission is asked for then),
   or type the server address and the code. Scanning the code with the
   phone's camera app opens a `stickpile://pair` link, which opens
   stickpile. Either way the app asks first: it shows the server's host
   (an internationalised name as punycode), the whole address, the account
   it replaces, and whether the address is plain `http://`; the Pair
   button takes a tap a second after the link shown changed. A link is
   used once: reopening the app from Recents does not offer it again.
3. Without the web UI at hand: type the server address, then "Sign in with
   Tailscale" (on a tailnet, no password) or a username and password (and
   a TOTP code if the account has one).

After signing in, the app says who is signed in where ("Signed in as alice
on stickers.example.org") and walks you through turning on the keyboard.

## The library

The app's main screen: a search field, the tabs Recent, Liked, Packs and
Most used, and a grid. Packs lists each pack with its cover (its own
cover image, else its first sticker).

- **Search** as on the desktop ([search syntax](#search-syntax)); results
  follow the typing. Under the field the app offers what fits what is
  typed: the filter keys with their short forms (`tag: (t:)`, `vibe:
  (v:)`, `e:`, ...), the values of the filter being typed (`vibe:sar` and
  `v:sar` offer `sarcastic`), the emojis for `e:`, and for a bare word the
  tags, vibes and packs containing it (`rea` offers `tag:reaction`). An
  emoji comes with its English name after it, smaller and dimmer: `e:`
  alone offers your emojis, most used first, and `e:sk` the emojis named
  so (💀 skull, ☠️ skull and crossbones, ...), yours first. A tap puts it
  in (an emoji in place of the name). The values come from the offline cache: right after
  signing in they come once the first sync is over, also for what is
  typed already.
- **Search in a pack.** With a pack open (Packs tab), the field says
  "Search in <pack>" and searches only that pack, with the whole syntax
  (`v:tired`, `type:gif`, `-t:nsfw`, `is:liked`, `sort:copies`, words),
  offline too; the pack's name stays above the grid. Clearing the field
  shows the whole pack again; the back arrow leaves the pack, and what is
  typed then searches everything.
- **Safe mode** (the shield) hides the stickers tagged `nsfw` from every
  list and search, and says how many it hid.
- **Sticker info** (the eye) shows or hides the information over each
  tile: an icon for a GIF, another animation or a video and one for NSFW,
  a heart on the ones you liked, the name, the emojis and vibe, the tags,
  and a line with the format and resolution, times copied, likes and the
  quality score. Nothing has a box or a band behind it: the icons and the
  letters have a dark outline, so they show on any sticker. The
  menu's "Sticker info" (also in Settings) turns each piece on or off and
  sets the stickers in a row, 2 to 8, with a slider (with the phone
  upright; held sideways, more of the same size fit). A small tile shows
  only the lines that fit: the name first, then the stats line, the tags,
  and the emojis and vibe. Safe mode and these choices are shared with the
  keyboard.
- **GIFs and videos play** in the grid and the details, looping and
  without sound, instead of showing their first frame. Each one's file is
  downloaded the first time it plays and kept in the offline cache. "Play
  GIFs and videos" (in "Sticker info" and Settings) is Always, Wi-Fi only
  (on mobile data only the ones already downloaded play) or Never.
- **Tap** a sticker to share it; **long press** for its details: the
  pack and owner, the numbers, the vibe, emojis, tags and keywords (a tap
  on one searches for it), and Share, Like or Unlike, and Open in browser
  (its page in the web UI).
- The menu also has the search syntax, Sync now and Settings; pulling the
  grid down syncs too.

### Search syntax

`cat "big chungus"` (words and phrases; typos are forgiven), `e:😂`,
`e:skull` (an emoji whose English name or keyword starts so, typos
forgiven), `vibe:tired`, `tag:reaction`, `set:cats`, `by:alice`, `type:gif` (gif,
png, webp, jpeg, mp4, webm, still, animated, video), `is:mine`,
`is:liked`, `is:public`, `q>70`, `likes>=2`, `copies<5`, `sort:likes`
(relevance, recent, likes, copies, quality); `tag:a tag:b` is either; a
leading `-` leaves out (`-tag:nsfw`). `v:`, `t:`, `s:` and `b:` are short
for `vibe:`, `tag:`, `set:` and `by:` (`v:tired -t:nsfw`). A misspelled
or half-typed filter value searches for the closest one. With a pack (in
the keyboard: a set) open, the search looks only inside it, with all of
this. Under the field, `e:` offers emojis with their names (`e:` alone:
yours; `e:sk`: the emojis named so, yours first); one puts the emoji in.
The app and the keyboard show this list from their menus
([search.md](search.md#query-syntax)).

## The picker keyboard

Turn it on once: the app's walk-through opens the system's keyboard
settings (Settings, System, Keyboard, On-screen keyboard, Manage keyboards
on most phones; the path differs by vendor), where "stickpile stickers" is
switched on. Then switch to it from any keyboard with the keyboard switcher
(the keyboard icon in the navigation bar, or the globe key).

Using it:

- Type a few words with your usual keyboard, then switch: stickpile searches
  for what is in the field (one line, at most 60 characters). It reads only
  plain text fields: not numbers, phone numbers, email addresses, links or
  passwords, not fields that ask keyboards for no suggestions, and not
  incognito fields.
- Or tap its search box: keys of its own come up (English, Ukrainian
  with the globe key, and digits and symbols), and the results follow the
  typing, with the same offers under the box as in the app. What you type
  there goes only into that search, never into the chat. Done, or a tap
  on the grid, puts the keys away. A sticker picked after you changed the
  search leaves the field's text alone.
- Or browse the tabs Recent, Liked, Most used and Sets (each set with its
  cover); the row of emojis narrows the grid. With a set open, the search
  box says "Search in <set>" and what you type searches only that set,
  with the whole syntax (offline too); clearing it shows the whole set
  again. The Results tab, All sets or another tab leave the set, and the
  search then looks everywhere again.
- The shield is safe mode and the menu has the sticker info, the
  stickers in a row (a slider over the top of the grid, which follows it
  as it moves) and the search syntax, all shared with the app. GIFs and
  videos play here too, as the app's "Play GIFs and videos" says.
- Tap a sticker: it is inserted and your keyboard comes back. If the field
  does not take pictures, the chat app's share screen opens with the file;
  failing that, it is copied to the clipboard (the keyboard says which).
- Long press: the sticker's details (pack, numbers, vibe, emojis and tags;
  a tap on one searches for it) with Send, Send as a sticker (Telegram),
  Like or Unlike, and Share.
- The keyboard button at the top goes back to your keyboard at any time.

Its settings (in the app's settings): the keyboard height (the sticker
grid at 60% to 160% of its usual height, never more than half the
screen; it applies the next time the keyboard opens), search what is
typed, delete what was typed when a result from it is inserted, and stay
open after inserting.

### Other keyboards

Another keyboard (such as a patched Unexpected Keyboard) can show your
stickers through the app's content provider ([Provider contract](#provider-contract)).
It must be allowed first: the first time it asks, it is refused and appears
under Settings, "Keyboards that may read your stickers", where you allow
it after a confirmation that names the app (label and package) and what
it can read; the confirmation ignores taps that come through another
app's window drawn over it.

## Sharing to stickpile

Share a picture or a video from a gallery or any app to "stickpile", pick
one of your sets, change the name, keywords and emojis if you like, and
upload. Several files upload one after another with their progress; a file
that fails says why and can be retried. Files larger than 1 GB are left
out.

## Settings

Account (sign out; an account signed in by its tailnet identity has no
token to revoke, so it shows "Signed in through Tailscale" and offers to
change the server instead), sync (sync now, the last sync, uses waiting to
be sent), the offline cache (its size, clear it, which also revokes every
prepared file's URI), the [theme](#theme), the sticker info (safe mode, what the tiles show,
the stickers in a row, when GIFs and videos play), the keyboard's
height, switches and walk-through, the keyboards allowed to read
stickers, and licences.

### Theme

Four choices, for the app and the picker keyboard together: **System**
(the default) follows the phone's light or dark setting, **Dark** and
**White** keep the app dark or light whatever the phone does, and
**OLED** is the dark theme on pure black (the containers, such as the
bars and the keyboard's keys, a little darker than in Dark but still
apart from the black). The app's colours stay the wallpaper's (dynamic
colour) on Android 12 and later. A change shows at once in the app; the
keyboard takes it the next time it opens.

The app syncs in the background every 15 minutes when there is a network
(WorkManager), and when the app or the keyboard opens. Offline, search, the
lists and the thumbnails come from the cache, and stickers inserted before
can be inserted again. When the server stops answering, only the first
list waits for it (5 s by default); for the next five minutes the lists
come from the cache at once, while the server is checked in the
background every 15 seconds, and once it answers they are live again.
Sync now (or pulling the grid down) always tries the server.

## Build from source

```sh
nix develop .#android
cd android
./gradlew assembleDebug        # app/build/outputs/apk/debug/
```

Before compiling Kotlin, the build runs three tasks of
`android/app/build.gradle.kts`:

- `checkStickpileVersion`: `versionName` must equal the workspace version
  in `Cargo.toml`, and `versionCode` must be `major * 1000000 + minor *
  1000 + patch` (0.5.2 is 5002).
- `cargoNdk`: `cargo ndk --release --locked` builds
  `libstickpile_android.so` for arm64-v8a, armeabi-v7a and x86_64 into
  `app/build/rustJniLibs/`.
- `uniffiBindings`: `uniffi-bindgen` (built from
  `crates/stickpile-android` with the `bindgen` feature) reads the
  interface from the built library and writes the Kotlin bindings to
  `app/build/generated/uniffi/kotlin/`. Generated code is never committed.

Versions: Android Gradle Plugin 9.5.1 with Gradle 9.6.0 (the wrapper
downloads it), Kotlin 2.4.21 (the Kotlin Gradle plugin on the build
classpath; the Compose compiler plugin must have the same version), JDK 21
(what F-Droid builds with; the Java and Kotlin target is 17), compileSdk
and targetSdk 37, minSdk 26, NDK 28.2.13676358. The dev shell provides the
SDK, the NDK, cargo-ndk and Rust with the three Android targets. On NixOS the `aapt2` that the plugin
downloads cannot run, so the shell points Gradle at the SDK's own through
`GRADLE_OPTS`; nothing machine-specific is committed.

## Release signing

The release workflow builds the APK unsigned: no signing key is in the
repository or in CI. To install it, the owner signs it with their own key
(`apksigner` and `zipalign` are in the SDK's build tools, e.g.
`$ANDROID_HOME/build-tools/36.0.0/`):

```sh
zipalign -c -v 4 stickpile-vX.Y.Z-android-unsigned.apk      # aligned already?
apksigner sign --ks stickpile-release.jks --out stickpile-vX.Y.Z-android.apk \
    stickpile-vX.Y.Z-android-unsigned.apk
apksigner verify --print-certs stickpile-vX.Y.Z-android.apk
```

Every update must be signed with the same key, or Android refuses to
install it over the old one. An APK from F-Droid is signed with F-Droid's
key unless the build is reproducible (below), so the two cannot update
each other.

## F-Droid

The app is meant for F-Droid's main repository. The draft recipe is
[`android/fdroid/io.github.dolfost.stickpile.yml`](../android/fdroid/io.github.dolfost.stickpile.yml),
the listing texts are in `fastlane/metadata/android/` (English and
Ukrainian), and the compliance checklist with its evidence, the submission
steps and the notes on reproducible builds are in
[`android/fdroid/README.md`](../android/fdroid/README.md). In short:

- Licences, dependencies (Google's Maven and Maven Central only, JNA as it
  is, as other accepted apps do) and the build (Rust from rustup, cargo-ndk,
  the pinned NDK, JDK 21) fit the inclusion policy; no anti-feature is
  expected (the app talks only to the server you configure, and the server
  is free software).
- F-Droid wants about three months of development before a new app;
  this one started in November 2026.
- Before the first submission the owner decides how releases are signed:
  by F-Droid's key, or reproducibly with the owner's key. This cannot be
  changed later.

## Permissions

The app's own manifest asks for `INTERNET` (the server you configure) and
`CAMERA` (scanning the pairing QR code, requested on that screen only).
WorkManager, which runs the background sync, adds `WAKE_LOCK`,
`ACCESS_NETWORK_STATE`, `RECEIVE_BOOT_COMPLETED` and `FOREGROUND_SERVICE`
to the merged manifest, all granted at install time without a prompt; the
owner chose to keep all four (2026-11-15). AndroidX also declares a
signature-level permission of the app's own
(`io.github.dolfost.stickpile.DYNAMIC_RECEIVER_NOT_EXPORTED_PERMISSION`),
which no other app can hold. Nothing else.

No backup and no device transfer: `allowBackup` is off and the backup and
data-extraction rules exclude everything, so the API token never leaves the
device.

## Bridge interface

The Kotlin app reaches Rust through one UniFFI object, `StickpileLibrary`,
exported by `crates/stickpile-android` (Kotlin package
`io.github.dolfost.stickpile.rust`; not `Library`, which would clash with
the `com.sun.jna.Library` that every generated file imports). It is a small surface over
`stickpile_core::Session`; it does not use the desktop popup model.

Rules for the interface:

- `async` Rust functions are Kotlin `suspend` functions, run on the
  library's Tokio runtime (`#[uniffi::export(async_runtime = "tokio")]`).
  Kotlin calls them from coroutines and never blocks the main thread on
  them. Functions that are not `async` read only local state (the config,
  the offline cache) and return quickly.
- Every record and enum that crosses the bridge is defined in
  `crates/stickpile-android`, converted from the `stickpile-api` types. No
  UniFFI derive goes on `stickpile-api` types.
- Errors are one enum, `LibraryError` (Kotlin: the sealed class
  `LibraryException`). Each variant is a kind the app turns into its own
  (translated) text; the `detail` string is for logs, never shown as is.
  URLs in it are cut to scheme, host, port and path: no query (a search's
  text) and no user name or password.
  `LibraryErrorKind` names the same variants without their fields (for
  `SyncState.last_error`).
- The enum `Target` shares its name with Kotlin's default-imported
  `kotlin.annotation.Target`: app code imports
  `io.github.dolfost.stickpile.rust.Target` explicitly.
- Interface drift fails the tests: `crates/stickpile-android/tests/interface.rs`
  checks the exported metadata against this list in every test run, and
  `tests/kotlin.rs` (with `--features bindgen`) checks the generated Kotlin.
- Paths are absolute strings inside the app's private storage.
- The constructor is synchronous (UniFFI cannot generate async primary
  constructors for Kotlin).

```
// free functions, no library needed
fn target_for_package(package: String) -> Target   // stickpile_core::insert, for ranking
fn version() -> String                               // the crate's version
fn client_name() -> String                           // the client name for the server, "android/<version>"

object StickpileLibrary
  constructor(dirs: LibraryDirs) throws LibraryError

  // account
  fn server_url() -> String
  async fn set_server_url(url: String) throws          // checked and normalised, saved, session
                                                       // reopened; another server signs out first
  async fn account() throws -> Account?                // null: signed out; auth = Tailnet: known
                                                       // by its tailnet identity, no token
  async fn login_tailnet() throws -> Account?          // null: not on the tailnet
  async fn login_password(username: String, password: String, totp: String?) throws -> Account
  async fn login_pairing(server_url: String, code: String) throws -> Account
                                                       // exchanges first; the URL is saved only on success
  async fn logout() throws                             // revokes and deletes the token; nothing
                                                       // to do for a tailnet identity

  // reading (offline: from the cache, ItemList.offline = true)
  async fn search(query: String, target: Target, limit: u32) throws -> ItemList
  fn recent(limit: u32) -> List<Item>
  async fn favorites(limit: u32) throws -> ItemList
  async fn most_used(limit: u32) throws -> ItemList
  async fn packs() throws -> PackList
  async fn pack_items(pack_id: String) throws -> ItemList
  async fn pack_search(pack_id: String, query: String) throws -> ItemList
                                                       // the search syntax over one pack's items,
                                                       // searched here (no server search)
  async fn own_packs() throws -> PackList               // the user's own, empty ones included (upload targets)
  fn suggestions(query: String) -> Suggestions?        // stickpile_quick::suggest
  fn hints(query: String) -> List<SearchHint>          // the offers under a search field, each with
                                                       // the whole query it makes (no network)
  fn item_url(item_id: String) -> String?              // the item's page in the web UI

  // files
  async fn thumbnail(item_id: String) throws -> String // path of the cached thumbnail
  async fn pack_cover(pack_id: String) throws -> String?
                                                       // path of a listed pack's cover
  async fn motion(item_id: String, download: Boolean) throws -> Motion?
                                                       // a moving item's rendition to play
  async fn prepare(item_id: String, editor_package: String,
                   accepted_mime_types: List<String>, sticker: Boolean) throws -> Prepared

  // writing
  fn record_use(item_id: String, variant: String, editor_package: String) throws
  async fn toggle_like(item_id: String) throws -> Boolean   // the new state
  async fn upload(set_id: String, file: UploadFile) throws -> Item  // one file; the app loops and shows progress

  // pairing input, no network (stickpile_api::pairing)
  fn parse_pairing_link(text: String) -> PairingLink?  // a scanned stickpile://pair?url=&code=
  fn normalize_pairing_code(input: String) -> String?  // null: not a code (yet); else the display form

  // sync (stickpile_core's SyncPolicy: every 10 minutes, retry after 60 s)
  async fn sync_if_due() throws -> SyncState
  async fn sync_now() throws -> SyncState
  fn sync_state() -> SyncState
  fn pending_usage() -> u32

  // cache (no network)
  fn cache_bytes() throws -> u64                       // thumbnails and prepared files
  fn clear_cache() throws -> u64                       // deletes them, returns the bytes freed;
                                                       // the item list (offline search) stays
```

Records and enums:

```
record LibraryDirs { config_file: String, cache_dir: String, token_file: String }
record Account     { username: String, display_name: String?, server_url: String,
                     auth: AuthKind, is_admin: Boolean }
enum   AuthKind    { Token, Tailnet }
enum   LibraryErrorKind { NotSignedIn, Unauthorized, Unreachable, NotFound, RateLimited,
                          NothingFits, InvalidInput, Server, Io, Other }
enum   Target      { Telegram, Discord, Signal, Whatsapp, Slack, Generic }
enum   MediaKind   { Still, Animation, Video }
enum   MediaFormat { Png, Jpeg, Webp, Gif, Mp4, Webm }
record Item        { id: String, name: String, set_id: String, set_name: String,
                     owner: String, emojis: List<String>, keywords: List<String>,
                     vibe: String?, tags: List<String>, kind: MediaKind,
                     format: MediaFormat, width: u32, height: u32, quality: u32?,
                     likes: u64, copies: u64,
                     nsfw: Boolean,     // tagged nsfw, any case: safe mode hides it
                     liked: Boolean }   // by the account, as the saved favorites say
record ItemList    { items: List<Item>, offline: Boolean }
record Pack        { id: String, slug: String, name: String, owner: String,
                     item_count: u32, mine: Boolean }
record PackList    { packs: List<Pack>, offline: Boolean }
record Suggestions { key: String, words: List<String>, chosen: u32?,
                     labels: List<String> }   // for e: the emoji's name, else ""
enum   HintKind    { Value, Filter, Key }
record SearchHint  { label: String, query: String, chosen: Boolean, kind: HintKind }
enum   MotionKind  { Picture, Video }
record Motion      { path: String, kind: MotionKind, width: u32, height: u32 }
record PairingLink { server_url: String, code: String }
record Prepared    { path: String, mime_type: String, display_name: String,
                     target: Target, variant: String, sticker: Boolean }
record UploadFile  { path: String, name: String?, keywords: List<String>,
                     emojis: List<String> }
record SyncState   { last_sync: i64?, last_error: LibraryErrorKind?,
                     pending_usage: u32, due: Boolean, cache_changed: Boolean }

error  LibraryError {
  NotSignedIn, Unauthorized, Unreachable, NotFound, RateLimited,
  NothingFits,            // prepare: no rendition the field accepts
  InvalidInput { field },  // e.g. a malformed server URL or pairing code
  Server { status: u16 },  // any other HTTP error
  Io, Other
  (every variant also carries detail: String)
}
```

`prepare` picks the rendition with `stickpile_core`'s `target_for_package`
and `variant_for` (below): it never returns a MIME type the field did not
list, and returns `NothingFits` instead. It then has `Session` fetch that
exact rendition into the cache (not `prepare_copy`, which chooses its own)
and names the file `<item name>.<format extension>`. `Prepared.variant` is
the variant's name (`original`, `tg_sticker`, ...), which `record_use`
gets back. Pairing errors: a wrong, expired or used code is
`Unauthorized`; a bad token name is `InvalidInput`.

Behaviour the signatures do not show:

- **Errors.** `CoreError` maps to `LibraryError` as documented in
  `crates/stickpile-android/src/lib.rs`: a login that needs a TOTP code and
  has none is `InvalidInput { field: "totp" }`; HTTP 400, 413, 415 and 422
  are `InvalidInput` naming the call's field (`file` for `upload`,
  `token_name` for pairing, `username` for a password login); 403 and other
  statuses are `Server { status }`; cache and SQLite errors are `Io`. The
  constructor gives `InvalidInput` for a path that is not absolute or a
  config file that is not a valid config.
- **Server URLs.** `set_server_url`, `login_pairing` and
  `parse_pairing_link` check and normalise a server URL the same way
  (`stickpile_api::pairing::server_url`): `http` or `https` with a host; no
  user name, password, query or fragment (`https://my.server@evil.example`
  goes to `evil.example`, so it is refused); the host in lower case and an
  internationalised host in its `xn--` form; no default port; no trailing
  `/`. The URL shown is the one contacted. The same server written another
  way is the same server. An `InvalidInput` for a URL does not repeat it
  (it may hold a password).
- **Logins.** `login_pairing` exchanges the code with a client that has no
  token, for `server_url`; only when that works does it save the URL, write
  the token and reopen the session. A wrong code changes nothing, and the
  old token never reaches the new server. The token is named
  `Android (<device model>)`, or `Android (phone)` when the model is
  unknown. `set_server_url` with a different URL first signs out
  (revoking the token on the old server when it answers; the token file is
  removed either way), so a token is never sent to another server. A
  login that replaces a stored token (`login_password`, `login_tailnet`,
  and `login_pairing` with the current server) first sends the unsent uses
  with the old token, then revokes it on the server, best effort within
  the configured timeout (5 s by default); the login goes on either way. A
  login that fails (a wrong password or code) keeps the stored token.
- **Account.** `account()` is null when no token is stored and when the
  server rejects it; offline it is the cached user, else null when no
  token is stored (a fresh install) and `Unreachable` when one is. Behind
  `tailscale serve` with no token stored, the server may know the phone by
  its tailnet identity: then `account()` is that account with
  `auth = Tailnet`, and `logout()` changes nothing (there is no token; the
  app offers to change the server instead). `display_name` is null when
  blank or equal to the username.
- **Offline favorites.** The library remembers the last favorites list it
  read online (kept up to date by `toggle_like`) and answers from the
  cached items with `offline = true`; `Unreachable` if it was never read
  online. Online it reads (and remembers) up to 200 favorites, or `limit`
  if more, and returns the first `limit`: asking for a few does not
  shorten the list kept for offline.
- **Account changes.** A read that ends after a login, logout or server
  change keeps nothing for the new account or server: its items are not
  remembered for `thumbnail`, `prepare` and `record_use`, and favorites
  are not saved.
- **Sync.** `sync_if_due` returns at once when no sync is due or one is
  running. Both sync calls throw a failure and also record it in
  `last_error`. `last_sync` is the local time of this process's last sync,
  else the cache's sync cursor (the server's clock), and null after an
  account or server change; the policy starts from it, so a restarted app
  does not sync at once. `cache_changed` is true when the sync the call
  ran changed the cached items (new, changed or removed; items the server
  sends again as they were do not count); false from `sync_state` and
  when no sync was due.
- **Search** resolves a half-typed `key:value` filter to its best
  suggestion, as the desktop popup does. The one-letter keys `v:`, `t:`,
  `s:`, `b:` (and `e:`) are `vibe:`, `tag:`, `set:`, `by:` (and
  `emoji:`); the query goes to the server with them written long, so a
  server older than them reads them too. Reads (search, account, favorites, most used, packs, pack
  items, pack search) stop when the calling coroutine is cancelled;
  writes and file fetches run to their end.
- **Pack search.** `pack_search(pack_id, query)` reads the pack's items as
  `pack_items` does (online the server's pack, offline the cached items
  with `offline = true`; the same errors) and searches them in the
  library with the whole syntax (`stickpile_quick::within`, see
  [search.md](search.md#inside-a-pack)): `vibe:tired`, `t:x`,
  `type:gif`, `-tag:nsfw`, `q>70`, `sort:`, words with typos forgiven.
  Words come best first, filters alone keep the pack's order; a word
  also finds the items with it anywhere in the name, a keyword, an emoji
  or a tag. A half-typed filter value searches for its best suggestion,
  the pack's own values first. `is:liked` follows the saved favorites (as
  `Item.liked`), `is:mine` the cached account, `is:public` the pack as
  `packs` or `own_packs` listed it (a pack not listed yet: the
  account's own items count as private). The pack's name is not searched
  as a word; `set:` matches it. A blank query gives the whole pack. No
  request goes to the server's search, so image similarity and vibe
  synonyms are left out.
- **Hints.** `hints(query)` is what a phone offers under a search field
  instead of the desktop's Tab completion; each hint carries the whole
  query it makes. While a `key:value` filter is typed: its values, ranked
  as `suggestions` ranks them, `chosen` on the one the search uses. For
  `e:` (or `emoji:`) the values are emojis labelled with their English
  names (`💀 skull`; the query gets the emoji): the emojis of the cached
  items, most used first, and while a name is typed (`e:sk`) the emojis
  it selects, the items' first (see [search.md](search.md)). Only these
  value labels have a space, so the app's and the keyboard's chips split
  them at the first one (`display/HintLabel.kt`): the emoji at the chip's
  size, the name after it in the secondary colour. While a
  bare word is typed: the tags, vibes and packs containing
  it as filters (`rea` offers `tag:reaction`), those starting with it
  first, then the filter keys the word starts. With nothing typed, or a
  space at the end: the keys `tag:`, `vibe:`, `e:`, `set:`, `by:`,
  `type:`, `is:`, `sort:`; a key's label shows its one-letter form
  (`tag: (t:)`, `vibe: (v:)`, `set: (s:)`, `by: (b:)`), its `query` is the
  long key. A short key being typed (`v:sar`) offers what the long one
  does, and the key stays as typed. A leading `-` stays on what is
  offered; inside an unfinished phrase nothing is. The words come from
  the offline cache, so they follow the last sync: a sync that changed
  the cache returns `cache_changed` true, and the app and the keyboard
  then ask for the offers on screen again (the text as it is), so the
  ones shown while the first sync ran do not stay empty.
- **Item marks.** `nsfw` is the desktop's safe-mode test: a tag `nsfw` in
  any case. `liked` comes from the favorites the library last read (and
  the likes toggled since): false for every item until `favorites` was
  read once for the account, which the app does in the background.
- **Motion.** The thumbnail is a still of the first frame; `motion` gives
  the file the grids play instead. For an animation, a `Picture`: the
  original when it is a GIF or WebP, or the Discord GIF, whichever is
  smaller (an animated picture keeps its transparency), and the MP4 only
  when there is neither. For a video, a `Video`: the MP4 the server made
  (for a WebM or an original that is not plain H.264), else the original.
  Null for a still and for an item without such a rendition. The file is
  the cached rendition `prepare` would use, fetched unless cached; with
  `download` false nothing is fetched and null means not cached (the
  app's "Wi-Fi only"). Like the other file fetches it runs to its end.
- **Pack covers.** `pack_cover` is the pack's own cover image, else its
  cover item's thumbnail (the first item unless one was chosen; offline,
  the first cached item by name), fetched unless cached. Null for a pack
  with neither, and for one `packs` or `own_packs` has not listed since
  the account last changed. Offline and not cached: `Unreachable`.
- **An unreachable server.** Once a request finds the server unreachable
  (no connection, a timeout, or 502 to 504 from a proxy), the reads fail
  as `Unreachable` at once for five minutes instead of each waiting for
  the timeout, so the lists answer from the cache straight away
  (`stickpile_core::ApiClient::with_fail_fast`). While reads come in, the
  server is checked in the background 15 s after the last failure; when
  it answers, the reads go to it again. After five minutes without a
  check one read tries the server itself. What the user asks for tries
  the server whatever it did a moment ago: `sync_now`, `prepare`,
  `toggle_like`, `upload` and the logins.
- **Pairing input.** `parse_pairing_link` returns the server URL
  normalised (above), the one `login_pairing` contacts, and null for a
  link whose URL has a user name or password, a query or a fragment.
  `PairingLink.code` and `normalize_pairing_code` give
  the display form `XXXX-XXXX-XXXX`.
- **Upload.** The file is streamed from disk, never held in memory whole.
  With `name = null` the server names the item after the file
  name (the path's last part), so the app names its temporary copy after
  the shared file.

### Choosing what to insert

Two functions in `stickpile-core`, with unit tests:

- `target_for_package(package) -> TargetApp`: a table of known package
  names (Telegram and its forks, Discord, Signal and Molly, WhatsApp and
  WhatsApp Business, Slack); everything else is `Generic`.
- `variant_for(item, target, accepted_mime_types, sticker) -> Option<InsertChoice>`
  (the rendition's `VariantInfo`, its `MediaFormat`, the MIME type to
  commit it as, and whether it is the sticker rendition):
  the best rendition whose MIME type the field accepts, honouring
  wildcards (`image/*`), or none. The preference order per target is a
  table (data, not code), provisional until the by-hand results in
  `docs/android-input.md` fill it in.

The input method then tries, in order: `commitContent` with the prepared
file's provider URI; else an `ACTION_SEND` intent to the editor's package;
else the URI on the clipboard, saying so. When preparing for the field
fails with `NothingFits` or `Unreachable` (a rendition for sharing may
still be in the cache), or the field refuses the commit, a rendition for
sharing is prepared. The draft the item was found by is deleted only after
the field took the commit. Every hand-off (commit, `ACTION_SEND`,
clipboard) records a use.

## App internals

The Kotlin app (`android/app/src/main/java/io/github/dolfost/stickpile/`)
is one process; these pieces are shared by its parts:

- `StickpileApp` (the `Application`): `StickpileApp.from(context).library`
  is the one `StickpileLibrary`, created on first use with its files in
  `noBackupFilesDir` (`config.toml`, `cache/`, `token`); `.scope` is a
  `CoroutineScope` for work that must outlive a screen.
- `provider.KeyboardAllowlist`: the keyboards the user lets read stickers
  through the provider, each tied to its signing certificates, and the
  pending (refused) ones. It reads the package manager: call it off the
  main thread.
- `ime.ImeSettings`: the picker keyboard's switches (search the draft,
  delete the draft, stay open) and its height (`ime.PickerHeight`).
- `motion.Motions`: the moving renditions for the grids and details of
  the app and the keyboard (the bridge's `motion`, after a short wait so
  that a fling downloads nothing, at most three downloads at once), and
  the decoded animated pictures (`ImageDecoder`, API 28 and later).
  `motion.LoopingVideoView` plays a video in a `TextureView` with a
  muted, looping `MediaPlayer`, only while on screen and at most six at
  once in the process (`motion.Slots`); `motion.MotionPolicy` says when
  an item plays (`display.Autoplay`).
- `sync.SyncScheduler`: `ensureScheduled(context)` at app start and after a
  login, `cancel(context)` after a logout (in the app's scope, also when
  the settings screen went) and when onboarding saved another server
  address.
- `provider.PreparedFiles`: `uriFor(context, prepared)` turns a file from
  `StickpileLibrary.prepare` into a `/prepared/<token>/<name>` provider URI,
  for the input method's `commitContent` and for sharing from the app;
  `revokeAll()` revokes every token at once: after a sign-out and a cache
  clear (settings), after a sign-in as another account or on another
  server, and when onboarding saved another server address (launcher).
- `display.DisplaySettings`: what the grids show and the theme
  (`display.AppTheme`), shared by the app and the keyboard. Every
  activity is a `ui.theme.ThemedActivity`: it lays a night or day
  `uiMode` over its configuration as the theme says (`AppThemes.override`,
  nothing for System), so the window, the system bars and
  `StickpileTheme` agree, and it is made again when the theme changes.
  The keyboard's views get the same through `AppThemes.views`, plus
  `ThemeOverlay.Stickpile.Oled` for OLED; the keyboard makes its view
  again when it opens after a change.
- `ui.common.AccountChanges`: counts the account changes made away from the
  launcher; the launcher watches it while resumed and asks again who is
  signed in when it moved.

Packages: `ui` (onboarding, the library screen, settings, licences),
`ime` (the picker keyboard), `provider` (the content provider below),
`share` (the share target and upload), `sync` (background sync). Strings
are Android resources: English in `res/values/`, Ukrainian in
`res/values-uk/`, one `strings_<area>.xml` per package group.

## Provider contract

The app serves stickers to keyboards (its own picker, and others such as a
patched Unexpected Keyboard) through one content provider. This contract
is versioned: a keyboard checks `contract_version` and treats anything it
does not know as absent.

**Authority:** `io.github.dolfost.stickpile.stickers`.

**Version:** `call("info", null, null)` returns a bundle with
`contract_version` (int, currently `1`) and `app_version` (string).

### Queries

All return a `Cursor`; unknown paths return `null`. Every cursor has an
integer `_id` (the row number) so that list adapters work.

| Path | Rows |
|---|---|
| `/search?q=<query>&package=<editor package>&limit=<n>` | items matching the query, ranked for the editor's app |
| `/recent?limit=<n>` | the items used last on this device |
| `/favorites?limit=<n>` | the items the account liked |
| `/packs` | the packs the account can see |
| `/packs/<pack id>/items` | one pack's items, in pack order |

Item columns: `_id`, `item_id`, `name`, `set_name`, `emojis` (joined, no
separator), `kind` (`still`, `animation` or `video`), `width`, `height`,
`thumbnail_uri` (a `/thumb/` URI below).

Pack columns: `_id`, `pack_id`, `name`, `owner`, `item_count`.

Offline the provider answers from the cache, like the app does. When the
library fails, a query returns an empty cursor whose extras carry `error`
(the codes of `prepare` below). `limit` defaults to 50 and is capped at 200.

### Files

- `/thumb/<item id>`: `openFile` read-only, the item's thumbnail (WebP).
- `/prepared/<token>/<file name>`: `openFile` read-only, a rendition made by
  `prepare`. Readable by any caller that presents a valid token (see below).
  A `query` on it answers the `OpenableColumns` (`_display_name`, `_size`),
  as `FileProvider` does, since receiving apps ask for them.
- `/thumb/` opens and `prepare` may download: they run at most 4 at a time
  for all callers together. A call over the limit waits up to 2 s for a
  slot, then fails (`/thumb/`: `FileNotFoundException`; `prepare`: error
  `failed`); the keyboard may try again.

### Calls

- `call("prepare", <item id>, extras)` with extras `package` (string, the
  editor's package), `mime_types` (string array, what the field accepts)
  and `sticker` (boolean). Returns a bundle with `uri` (string,
  `content://io.github.dolfost.stickpile.stickers/prepared/<token>/<file name>`),
  `mime_type` and `label` (the file name, `<item name>.<ext>`); or a bundle
  with `error` (one of
  `nothing_fits`, `not_found`, `not_signed_in`, `unreachable`, `failed`).
  The token is 128 random bits, valid for one hour, kept in memory only.
  At most 512 tokens live at once, and at most 64 made for one other app
  (by uid); the oldest goes first. All of them are revoked on sign-out, a
  change of server or account, and when the cache is cleared.
- `call("used", <item id>, extras)` with extras `variant` (string, from the
  prepared file) and `package`: records the use (for "recent" and the
  server's counts).

### Access control

Enforced in code at every entry point; no custom permission is used (a
signature-level one cannot cover a keyboard signed by someone else, and
custom permissions have an install-order problem).

- The provider is exported. Calls from the app's own uid are always allowed.
- Any other caller is identified by `Binder.getCallingUid()` and its package
  names, and is allowed only when the user approved that package in the
  app's settings ("Keyboards that may read your stickers") and it is still
  signed with the certificates it had when approved (their SHA-256 is
  stored on approval; a keyboard that rotates its signing key, or another
  app installed under the name, must be approved again). The refusal is a
  `SecurityException`. Only input methods (a package with a service in
  `InputMethodManager.getInputMethodList()`) are remembered as pending, so
  that the settings screen can offer them for approval; any other app is
  refused without being listed.
- `/prepared/<token>/...` is readable by any caller with a valid token: the
  chat app that receives the URI is not on the allowlist and must still
  read the file, so the unguessable token is its capability. URI permission
  grants are not relied on, since on an exported provider they do not
  distinguish callers.
- Path traversal, unknown paths and writes are rejected; files are opened
  read-only. The access check comes before routing, so an unknown path or
  a write from a caller that is not approved is refused like any other.
  `getTypeAnonymous` (asked with the caller's identity cleared) answers
  nothing.
- A `/search` ranks for the app named by `package` (`target_for_package`).
