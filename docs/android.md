# Android app

> Work in progress on the `android` branch. This page starts with the two
> contracts the app is built against; the user documentation (install,
> onboarding, the keyboard, sharing, settings, F-Droid) follows when the app
> exists.

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

## Bridge interface

The Kotlin app reaches Rust through one UniFFI object, `Library`, exported
by `crates/stickpile-android` (Kotlin package
`io.github.dolfost.stickpile.rust`). It is a small surface over
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
- Errors are one enum, `LibraryError`. Each variant is a kind the app turns
  into its own (translated) text; the `detail` string is for logs, never
  shown as is.
- Paths are absolute strings inside the app's private storage.
- The constructor is synchronous (UniFFI cannot generate async primary
  constructors for Kotlin).

```
object Library
  constructor(dirs: LibraryDirs) throws LibraryError

  // account
  fn server_url() -> String
  async fn set_server_url(url: String) throws          // validated, saved, session reopened
  async fn account() throws -> Account?                // null: signed out
  async fn login_tailnet() throws -> Account?          // null: not on the tailnet
  async fn login_password(username: String, password: String, totp: String?) throws -> Account
  async fn login_pairing(server_url: String, code: String) throws -> Account
  async fn logout() throws

  // reading (offline: from the cache, ItemList.offline = true)
  async fn search(query: String, target: Target, limit: u32) throws -> ItemList
  fn recent(limit: u32) -> List<Item>
  async fn favorites(limit: u32) throws -> ItemList
  async fn most_used(limit: u32) throws -> ItemList
  async fn packs() throws -> PackList
  async fn pack_items(pack_id: String) throws -> ItemList
  fn suggestions(query: String) -> Suggestions?        // stickpile_quick::suggest

  // files
  async fn thumbnail(item_id: String) throws -> String // path of the cached thumbnail
  async fn prepare(item_id: String, editor_package: String,
                   accepted_mime_types: List<String>, sticker: Boolean) throws -> Prepared

  // writing
  fn record_use(item_id: String, variant: String, editor_package: String) throws
  async fn toggle_like(item_id: String) throws -> Boolean   // the new state
  async fn upload(set_id: String, file: UploadFile) throws -> Item  // one file; the app loops and shows progress

  // sync (stickpile_core's SyncPolicy: every 10 minutes, retry after 60 s)
  async fn sync_if_due() throws -> SyncState
  async fn sync_now() throws -> SyncState
  fn sync_state() -> SyncState
  fn pending_usage() -> u32
```

Records and enums:

```
record LibraryDirs { config_file: String, cache_dir: String, token_file: String }
record Account     { username: String, display_name: String?, server_url: String,
                     auth: AuthKind, is_admin: Boolean }
enum   AuthKind    { Token, Tailnet }
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
  Server { status },       // any other HTTP error
  Io, Other
  (every variant also carries detail: String)
}
```

`prepare` picks the rendition with `stickpile_core`'s `target_for_package`
and `variant_for` (below): it never returns a MIME type the field did not
list, and returns `NothingFits` instead.

### Choosing what to insert

Two functions in `stickpile-core`, with unit tests:

- `target_for_package(package) -> TargetApp`: a table of known package
  names (Telegram and its forks, Discord, Signal and Molly, WhatsApp and
  WhatsApp Business, Slack); everything else is `Generic`.
- `variant_for(item, target, accepted_mime_types, sticker) -> Option<Variant>`:
  the best rendition whose MIME type the field accepts, honouring
  wildcards (`image/*`), or none. The preference order per target is a
  table (data, not code), provisional until the by-hand results in
  `docs/android-input.md` fill it in.

The input method then tries, in order: `commitContent` with the prepared
file's provider URI; else an `ACTION_SEND` intent to the editor's package;
else the URI on the clipboard, saying so.

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

Offline the provider answers from the cache, like the app does.

### Files

- `/thumb/<item id>`: `openFile` read-only, the item's thumbnail (WebP).
- `/prepared/<token>/<file name>`: `openFile` read-only, a rendition made by
  `prepare`. Readable by any caller that presents a valid token (see below).

### Calls

- `call("prepare", <item id>, extras)` with extras `package` (string, the
  editor's package), `mime_types` (string array, what the field accepts)
  and `sticker` (boolean). Returns a bundle with `uri` (string,
  `content://io.github.dolfost.stickpile.stickers/prepared/<token>/<file name>`),
  `mime_type` and `label`; or a bundle with `error` (one of
  `nothing_fits`, `not_found`, `not_signed_in`, `unreachable`, `failed`).
  The token is 128 random bits, valid for one hour, kept in memory only.
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
  app's settings ("Keyboards that may read your stickers"). A refused
  package is remembered as pending, so that the settings screen can offer
  it for approval; the refusal is a `SecurityException`.
- `/prepared/<token>/...` is readable by any caller with a valid token: the
  chat app that receives the URI is not on the allowlist and must still
  read the file, so the unguessable token is its capability. URI permission
  grants are not relied on, since on an exported provider they do not
  distinguish callers.
- Path traversal, unknown paths and writes are rejected; files are opened
  read-only.
