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
2. **A picker keyboard** in the same app: an input method with no typing
   keys. Switch to it from any keyboard, tap a sticker, and it switches back.
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
   stickpile and asks before it signs in.
3. Without the web UI at hand: type the server address, then "Sign in with
   Tailscale" (on a tailnet, no password) or a username and password (and
   a TOTP code if the account has one).

After signing in, the app walks you through turning on the keyboard.

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
  incognito fields. Or browse the tabs Recent, Liked and Sets; the row of
  emojis narrows the grid.
- Tap a sticker: it is inserted and your keyboard comes back. If the field
  does not take pictures, the chat app's share screen opens with the file;
  failing that, it is copied to the clipboard (the keyboard says which).
- Long press: like or unlike, send as a sticker (Telegram), share.
- The keyboard button at the top goes back to your keyboard at any time.

Its settings (in the app's settings): search what is typed, delete what was
typed when a result from it is inserted, and stay open after inserting.

### Other keyboards

Another keyboard (such as a patched Unexpected Keyboard) can show your
stickers through the app's content provider ([Provider contract](#provider-contract)).
It must be allowed first: the first time it asks, it is refused and appears
under Settings, "Keyboards that may read your stickers", where you allow
it.

## Sharing to stickpile

Share a picture or a video from a gallery or any app to "stickpile", pick
one of your sets, change the name, keywords and emojis if you like, and
upload. Several files upload one after another with their progress; a file
that fails says why and can be retried. Files larger than 1 GB are left
out.

## Settings

Account (sign out), sync (sync now, the last sync, uses waiting to be
sent), the offline cache (its size, clear it), the keyboard's switches and
walk-through, the keyboards allowed to read stickers, and licences.

The app syncs in the background every 15 minutes when there is a network
(WorkManager), and when the app or the keyboard opens. Offline, search, the
lists and the thumbnails come from the cache, and stickers inserted before
can be inserted again.

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
object StickpileLibrary
  constructor(dirs: LibraryDirs) throws LibraryError

  // account
  fn server_url() -> String
  async fn set_server_url(url: String) throws          // validated, saved, session reopened;
                                                       // a different URL signs out first
  async fn account() throws -> Account?                // null: signed out
  async fn login_tailnet() throws -> Account?          // null: not on the tailnet
  async fn login_password(username: String, password: String, totp: String?) throws -> Account
  async fn login_pairing(server_url: String, code: String) throws -> Account
                                                       // exchanges first; the URL is saved only on success
  async fn logout() throws

  // reading (offline: from the cache, ItemList.offline = true)
  async fn search(query: String, target: Target, limit: u32) throws -> ItemList
  fn recent(limit: u32) -> List<Item>
  async fn favorites(limit: u32) throws -> ItemList
  async fn most_used(limit: u32) throws -> ItemList
  async fn packs() throws -> PackList
  async fn pack_items(pack_id: String) throws -> ItemList
  async fn own_packs() throws -> PackList               // the user's own, empty ones included (upload targets)
  fn suggestions(query: String) -> Suggestions?        // stickpile_quick::suggest

  // files
  async fn thumbnail(item_id: String) throws -> String // path of the cached thumbnail
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
record Item        { id: String, name: String, set_id: String, set_name: String,
                     emojis: List<String>, keywords: List<String>, vibe: String?,
                     tags: List<String>, kind: MediaKind, width: u32, height: u32,
                     likes: u64, copies: u64 }
record ItemList    { items: List<Item>, offline: Boolean }
record Pack        { id: String, slug: String, name: String, owner: String,
                     item_count: u32, mine: Boolean }
record PackList    { packs: List<Pack>, offline: Boolean }
record Suggestions { key: String, words: List<String>, chosen: u32? }
record PairingLink { server_url: String, code: String }
record Prepared    { path: String, mime_type: String, display_name: String,
                     target: Target, variant: String, sticker: Boolean }
record UploadFile  { path: String, name: String?, keywords: List<String>,
                     emojis: List<String> }
record SyncState   { last_sync: i64?, last_error: LibraryErrorKind?,
                     pending_usage: u32, due: Boolean }

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
- **Logins.** `login_pairing` exchanges the code with a client that has no
  token, for `server_url`; only when that works does it save the URL, write
  the token and reopen the session. A wrong code changes nothing, and the
  old token never reaches the new server. The token is named
  `Android (<device model>)`, or `Android (phone)` when the model is
  unknown. `set_server_url` with a different URL first signs out
  (revoking the token on the old server when it answers; the token file is
  removed either way), so a token is never sent to another server.
- **Account.** `account()` is null when no token is stored and when the
  server rejects it; offline it is the cached user, else null when no
  token is stored (a fresh install) and `Unreachable` when one is. `display_name` is null when blank or equal to the
  username.
- **Offline favorites.** The library remembers the last favorites list it
  read online (kept up to date by `toggle_like`) and answers from the
  cached items with `offline = true`; `Unreachable` if it was never read
  online.
- **Sync.** `sync_if_due` returns at once when no sync is due or one is
  running. Both sync calls throw a failure and also record it in
  `last_error`. `last_sync` is the local time of this process's last sync,
  else the cache's sync cursor (the server's clock), and null after an
  account or server change; the policy starts from it, so a restarted app
  does not sync at once.
- **Search** resolves a half-typed `key:value` filter to its best
  suggestion, as the desktop popup does. Reads (search, account, favorites,
  most used, packs, pack items) stop when the calling coroutine is
  cancelled; writes and file fetches run to their end.
- **Pairing input.** `PairingLink.code` and `normalize_pairing_code` give
  the display form `XXXX-XXXX-XXXX`.
- **Upload.** With `name = null` the server names the item after the file
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
  delete the draft, stay open).
- `sync.SyncScheduler`: `ensureScheduled(context)` at app start and after a
  login, `cancel(context)` after a logout.
- `provider.PreparedFiles`: `uriFor(context, prepared)` turns a file from
  `StickpileLibrary.prepare` into a `/prepared/<token>/<name>` provider URI,
  for the input method's `commitContent` and for sharing from the app;
  `revokeAll()` revokes every token at once (sign-out, a change of server
  or account, clearing the cache).

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
