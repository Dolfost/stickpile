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

- **From a release:** from 0.8.5 on, each GitHub release has
  `stickpile-vX.Y.Z-android.apk`, signed with the release key (certificate
  SHA-256 in [Release signing](#release-signing)); open it on the phone
  and allow the app you opened it from to install apps. Releases up to
  0.8.4 have only `stickpile-vX.Y.Z-android-unsigned.apk`, which Android
  does not install: sign it first or build the app
  ([Build from source](#build-from-source)). Android updates the app only
  from an APK signed with the same key, so an app installed from another
  build (a test build, or a later F-Droid one) has to be uninstalled
  first; that signs the phone out.
- **Samsung phones:** Auto Blocker (on out of the box on newer Galaxy
  phones) blocks every APK from outside the app stores, even with "Install
  unknown apps" allowed ("Unknown app blocked"). Turn it off in Settings,
  Security and privacy, Auto Blocker, install, and turn it on again if you
  like; the installed app keeps working.
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
  (v:)`, `e:`, `text: (x:)`, ...), the values of the filter being typed
  (`vibe:sar` and `v:sar` offer `sarcastic`), the emojis for `e:`, the
  words written in the pictures for `x:` (`x:` alone: those on the most
  stickers; `x:l`: lol, love, ...; shown lower case, without accents or
  apostrophes: `x:п'ят` offers `пятниця`), and for a bare word the
  tags, vibes and packs containing it (`rea` offers `tag:reaction`). An
  emoji comes with its English name after it, smaller and dimmer: `e:`
  alone offers the emojis of your own stickers (not of other people's
  public ones), most used first, and `e:sk` the emojis named so (💀
  skull, ☠️ skull and crossbones, ...), those your stickers use first,
  then others. A tap puts it
  in (an emoji in place of the name). The values come from the offline cache: right after
  signing in they come once the first sync is over, also for what is
  typed already.
- **Results without waiting.** A search never waits for a server that
  does not answer ([offline.md](offline.md), "Reachability and results
  without waiting"). With the server known to be away, the stickers saved
  on this phone show at once under "Offline: showing what is saved on
  this phone."; when the server is slow to answer (or has just gone, and
  nothing knows it yet), they show after 300 ms with the same notice, and
  the server's results take their place when they come (the notice goes
  with it). The server's results replace the others without moving what
  you are on: while a finger is on the grid (pressing a sticker, scrolling)
  they wait until it lifts; the details stay open and a share being
  prepared goes on; at the top the grid stays at the top, so the best
  results come first, and scrolled down, the sticker first in view stays
  where it is while it is still among them. When the server comes back
  (a network appears, the app opens, or the check in the background
  finds it), what is shown loads again in place, from the server, and a
  sync starts. The other lists (Liked, Most used, Packs, a pack and the
  search inside it) are read as before: while the server is not known to
  be away they wait for it (up to the timeout, 5 s by default, when it
  does not answer), and once it is known to be away they answer from the
  phone at once until it is back. Recent never asks the server.
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
  the words in the picture on one line (its lines joined with " / "), and
  a line with the format and resolution, times copied, likes and the
  quality score. Nothing has a box or a band behind it: the icons and the
  letters have a dark outline, so they show on any sticker. The
  menu's "Sticker info" (also in Settings) turns each piece on or off and
  sets the stickers in a row, 2 to 8, with a slider (with the phone
  upright; held sideways, more of the same size fit). "Text in the
  picture" is the one piece that starts off, also after an update, as it
  is drawn over the picture whose words it repeats; the others start on. A
  small tile shows only the lines that fit: the name first, then the stats
  line, the tags, the emojis and vibe, and the text last. Safe mode and
  these choices are shared with the keyboard.
- **GIFs and videos play** in the grid and the details, looping and
  without sound, instead of showing their first frame. Each one's file is
  downloaded the first time it plays and kept in the offline cache. "Play
  GIFs and videos" (in "Sticker info" and Settings) is Always, Wi-Fi only
  (on mobile data only the ones already downloaded play) or Never.
- **Tap** a sticker to share it; **long press** for its details: the
  pack and owner, the numbers, the vibe, emojis, tags and keywords (a tap
  on one searches for it), the words in the picture as they are, line
  breaks kept (when it has any), and Share, Like or Unlike, Keep offline
  ([Keeping stickers offline](#keeping-stickers-offline)), and Open in
  browser (its page in the web UI). The app shows no AI mark on the text:
  the items it gets (`ItemSummary`) do not say who wrote it.
- The menu also has the search syntax, Sync now and Settings; pulling the
  grid down syncs too.

### Search syntax

`cat "big chungus"` (words and phrases, also in the words in the picture;
typos are forgiven), `e:😂`,
`e:skull` (an emoji whose English name or keyword starts so, typos
forgiven), `vibe:tired`, `tag:reaction`, `set:cats`, `by:alice`,
`text:"no thoughts"` (these words in the picture, one after another;
typos and words inside longer words count, and the last may be just its
start: `x:l` finds "lol"), `type:gif` (gif,
png, webp, jpeg, mp4, webm, still, animated, video), `is:mine`,
`is:liked`, `is:public`, `q>70`, `likes>=2`, `copies<5`, `sort:likes`
(relevance, recent, likes, copies, quality); `tag:a tag:b` is either; a
leading `-` leaves out (`-tag:nsfw`). `v:`, `t:`, `s:`, `b:` and `x:` are
short for `vibe:`, `tag:`, `set:`, `by:` and `text:` (`v:tired -t:nsfw`). A misspelled
or half-typed filter value searches for the closest one (a `text:` value
is searched as typed). With a pack (in
the keyboard: a set) open, the search looks only inside it, with all of
this. Under the field, `e:` offers emojis with their names (`e:` alone:
yours; `e:sk`: the emojis named so, yours first); one puts the emoji in.
`x:` offers the words in the pictures, those on the most stickers first;
one puts the word in.
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
  With "Prefer stickers (Telegram)" on, a tap in Telegram or one of its
  forks inserts what Send as a sticker does: a still's 512 px sticker
  WebP, which Telegram sends as a sticker (seen by the owner on a phone,
  2026-11-18), and an animation's or video's WebM video sticker. A
  sticker without a sticker version, or a field that does not take it,
  gets the usual file, as with Send as a sticker; other apps get the
  usual file either way.
- Long press: the sticker's details (pack, numbers, vibe, emojis and tags,
  a tap on one searches for it; the words in the picture, line breaks
  kept; why it is kept offline) with Send (what a tap sends), the other
  way (Send as a sticker, or Send as a picture with "Prefer stickers"
  on), Like or Unlike, Keep offline or Don't keep offline, and Share. The tiles
  carry the offline marks of the app ([Keeping stickers
  offline](#keeping-stickers-offline)); the keyboard has no offline
  settings of its own.
- Its search does not wait for a server that does not answer, as in the
  app ([Results without waiting](#the-library)): the stickers saved on
  the phone show at once when the server is known to be away, else after
  300 ms when it is slow, with "Offline: showing saved stickers." on the
  status line, and the server's results take their place when they come
  (the line goes then). They wait while a finger is on the grid; the
  details, an insert going on and the emoji filter chosen stay; at the
  top the grid stays at the top. Each time the keyboard opens the server
  is checked; when it is back, what is shown loads again from it and the
  app syncs.
- The keyboard button at the top goes back to your keyboard at any time.

Its settings (in the app's settings): the keyboard height (the sticker
grid at 60% to 160% of its usual height, never more than half the
screen; it applies the next time the keyboard opens), prefer stickers
(Telegram; off by default, as the desktop's `prefer_stickers`: a tap
sends the sticker version, a long press still offers the picture), search
what is typed, delete what was typed when a result from it is inserted,
and stay open after inserting.

### Other keyboards

Another keyboard (such as a patched Unexpected Keyboard) can show your
stickers through the app's content provider ([Provider contract](#provider-contract)).
It must be allowed first: the first time it asks, it is refused and appears
under Settings, "Keyboards that may read your stickers", where you allow
it after a confirmation that names the app (label and package) and what
it can read; the confirmation ignores taps that come through another
app's window drawn over it.

## Keeping stickers offline

Stickers kept offline stay on the phone, so that they insert, share and
play with the server unreachable ([offline.md](offline.md) has the
design; everything below is the app's side of it).

- **What to keep.** A sticker: "Keep offline" in its details (a long
  press, in the app and in the keyboard); "Don't keep offline" there takes
  it back. A pack: "Keep offline" over the open pack (the Packs tab); it
  first says what it takes (stickers, files and their size, how much is on
  the phone already, what the options leave out, and whether it fits the
  space for kept files), and keeping a pack also keeps what a sync adds to
  it later; "Kept offline" there stops it, after a confirmation. The whole
  library: the switch in Settings, "Available offline", with the same
  estimate first. The details say why a sticker is kept ("Available
  offline: with its pack, liked"). A sticker kept with its pack or a rule
  can also be kept by itself; then it stays when the pack goes.
- **Automatic rules** (Settings): thumbnails of every sticker (on, so the
  grids are complete offline), the stickers sent last from this phone (off;
  25 to 2000 of them), the ones you liked (off), your own packs (off).
- **What packs, the library and the rules download**: pictures and
  animations (on), videos (off), the sticker versions Telegram sends (on),
  files up to a largest size (no limit by default), within the space for
  kept files (1024 MiB, that is 1 GiB, by default; downloads stop when it
  is full and nothing is deleted to make room). The sliders count in MiB,
  and every size the app shows counts in 1024s (KiB, MiB, GiB; Ukrainian
  КіБ, МіБ, ГіБ), as the CLI does. A sticker kept by itself is always
  downloaded whole. What a sticker needs is the core's: its thumbnail,
  for each chat app it was sent to from this phone (and the one an
  unknown app counts as) the file a share uses and the files the keyboard
  inserts into the usual fields (one that takes any picture, one that
  takes only PNG and GIF, one only PNG; with and without "Send as a
  sticker"), so a kept sticker inserts offline as it does online (a video
  into a picture field as its GIF), and the GIF or video the grids play.
- **When it downloads.** In the background (WorkManager), after each sync
  that went through and after a keep: on Wi-Fi or another network that is
  not metered, unless "Use mobile data" is on, and not while the phone's
  storage is low. No notification and no permission of its own; when the
  system stops the job (the network went, its time ran out) the download
  stops cleanly and goes on later. Settings shows where it stands
  (downloading, waiting for Wi-Fi, the server unreachable, the space or the
  storage full, everything here) with the files and bytes on the phone,
  and Pause (kept across restarts), Resume, Stop and "Download now".
- **Marks.** With the server answering, a small green arrow marks the
  stickers that would also work without it (kept, or saved when they were
  sent or played); "Available offline (a mark)" in "Sticker info" turns
  it off like the other tile marks. With the server unreachable, the
  stickers whose file is not on the phone are dimmed, with a struck-out
  cloud. Choosing one says that it is not on the phone and how to keep it,
  instead of naming a network error. The keyboard marks what a pick would
  insert into the field (or share), as a plain pick or with "Send as a
  sticker" (a video whose only small enough file is its Telegram video
  sticker is not dimmed), whichever of the two a tap makes; with "Prefer
  stickers" on that is what a tap inserts, since the sticker choice falls
  back to the usual files. It starts no download of its own for the
  marks.
- **Clearing.** "Clear" under "Saved pictures" deletes the other saved
  files and says how many kept files stay; "Remove kept files" (asked
  first) deletes the kept ones and forgets what is kept. Signing out does
  both.
- The kept files live with the rest of the cache in `noBackupFilesDir`,
  which the system neither backs up nor empties (unlike `cacheDir`).

## Sharing to stickpile

Share a picture or a video from a gallery or any app to "stickpile", pick
one of your sets, change the name, keywords and emojis if you like, and
upload. "Text in the picture" (several lines, at most 2000 characters)
takes the words written in it, for search: typed, it is yours and the
server never replaces it; left empty, nothing is sent and the server reads
the words itself when it can (OCR, see [ai.md](ai.md#ocr-and-the-text-field)).
"Apply to all" copies keywords and emojis, never the text: each picture
has its own words. Several files upload one after another with their
progress; a file that fails says why and can be retried. Files larger
than 1 GB are left out.

## Settings

Account (sign out; an account signed in by its tailnet identity has no
token to revoke, so it shows "Signed in through Tailscale" and offers to
change the server instead), sync (sync now, the last sync, uses waiting to
be sent), the offline cache (the size of the files kept offline and of
the others; clearing deletes the others, says how many kept files stay,
and revokes every prepared file's URI), the [theme](#theme), the sticker info (safe mode, what the tiles show,
the stickers in a row, when GIFs and videos play), "Available offline"
([Keeping stickers offline](#keeping-stickers-offline)), the keyboard's
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
(WorkManager), and when the app or the keyboard opens; after each sync
that went through, the files kept offline download
([Keeping stickers offline](#keeping-stickers-offline)). Offline, search, the
lists and the thumbnails come from the cache, and stickers inserted before
or kept offline can be inserted again. When the server stops answering, a
search shows what is saved on the phone after 300 ms (at once once the
server is known to be away), and only the first other list waits for it
(5 s by default). Opening the app or the keyboard does not wait for it
either: who is signed in is the account the phone last saw when the
server does not answer within 300 ms, and a token the server refuses
once it answers still signs out. From then on everything comes from the cache at once,
while the server is checked in the background (every 15 seconds for five
minutes, then every minute, and at once when a network appears or the app
or the keyboard opens); once it answers, what is shown loads again from
it and a sync starts. Sync now (or pulling the grid down) always tries
the server.

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

Release APKs (from 0.8.5) are signed with the owner's release key, an
RSA 4096 key in a PKCS12 file, alias `stickpile`, valid until 2056. Its
certificate's SHA-256 is

```
ab7200c912206a158239eb8463d37329785aa838c67f98343b5a44201034e120
```

(`apksigner verify --print-certs stickpile-vX.Y.Z-android.apk` prints it
as "Signer #1 certificate SHA-256 digest"). The key is not in the
repository. The release workflow gets it from two repository secrets,
`ANDROID_KEYSTORE_BASE64` (the PKCS12 file, base64) and
`ANDROID_KEYSTORE_PASSWORD`, and Gradle signs the APK with it
(`android/app/build.gradle.kts`: the environment variables
`STICKPILE_KEYSTORE`, `STICKPILE_KEYSTORE_PASSWORD`, optionally
`STICKPILE_KEY_ALIAS` and `STICKPILE_KEY_PASSWORD`). The workflow fails
when the signed APK carries another certificate than `ANDROID_CERT_SHA256`
or its native libraries are not 16 KB aligned. Without the secrets (a
fork, or before they were set) the APK is published unsigned as
`stickpile-vX.Y.Z-android-unsigned.apk`; to install that one, sign it
with your own key (`apksigner` and `zipalign` are in the SDK's build
tools, e.g. `$ANDROID_HOME/build-tools/36.0.0/`):

```sh
zipalign -c -P 16 -v 4 stickpile-vX.Y.Z-android-unsigned.apk   # aligned already?
apksigner sign --ks my-key.p12 --out stickpile-vX.Y.Z-android.apk \
    stickpile-vX.Y.Z-android-unsigned.apk
apksigner verify --print-certs stickpile-vX.Y.Z-android.apk
```

Every update must be signed with the same key, or Android refuses to
install it over the old one; losing the release key means the app can
never be updated in place again, so the owner keeps a backup of it. An APK from F-Droid is signed with F-Droid's
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
WorkManager, which runs the background sync and the offline downloads
(no permission of their own), adds `WAKE_LOCK`,
`ACCESS_NETWORK_STATE`, `RECEIVE_BOOT_COMPLETED` and `FOREGROUND_SERVICE`
to the merged manifest, all granted at install time without a prompt; the
owner chose to keep all four (2026-11-15). The app itself uses
`ACCESS_NETWORK_STATE` too, for nothing but noticing that a network
appeared, so that a server found away is checked again at once
(`sync.NetworkWatch`); it asks for no permission of its own for it. AndroidX also declares a
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
- A field added to a record goes last with `#[uniffi(default)]`, so that
  Kotlin written before it still builds (`Suggestions.labels`,
  `Item.text`, `UploadFile.text`, `SyncState.synced`). A variant added
  to an enum or to `LibraryError` goes last too (`NotOnDevice`).
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
                                                       // by its tailnet identity, no token; the
                                                       // cached account when the server is slow
                                                       // or away
  async fn account_steps() throws -> AccountAnswer     // the same, and the server's word on a
                                                       // cached account to come (the app's start)
  async fn login_tailnet() throws -> Account?          // null: not on the tailnet
  async fn login_password(username: String, password: String, totp: String?) throws -> Account
  async fn login_pairing(server_url: String, code: String) throws -> Account
                                                       // exchanges first; the URL is saved only on success
  async fn logout() throws                             // revokes and deletes the token; nothing
                                                       // to do for a tailnet identity

  // reading (offline: from the cache, ItemList.offline = true)
  async fn search(query: String, target: Target, limit: u32) throws -> ItemList
                                                       // waits for the server (the provider)
  async fn search_steps(query: String, target: Target, limit: u32) throws -> SearchAnswer
                                                       // never waits for a server that does not
                                                       // answer: the offline results first, the
                                                       // server's to come (the app, the keyboard)
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
  fn clear_cache() throws -> CacheCleared              // deletes the files not kept offline; says
                                                       // what it freed and how many kept files stay;
                                                       // the item list (offline search) stays

  // keeping offline (docs/offline.md; no network unless said)
  fn offline_options() -> OfflineOptions               // in use: those set, else the core's defaults
  fn set_offline_options(options: OfflineOptions)      // the app's settings; kept across a server change
  async fn keep_offline(scope: OfflineScope) throws    // flags what is here; offline_run fetches the rest
  async fn release_offline(scope: OfflineScope) throws // its files become ordinary cached files
  fn offline_keeps() throws -> List<OfflineScope>      // what the user keeps, oldest first
  fn offline_reasons(item_id: String) throws -> List<KeepReason>   // empty: not kept
  async fn offline_estimate(scope: OfflineScope) throws -> OfflineEstimate
  async fn offline_plan() throws -> OfflinePlan        // totals overall and per scope and rule
  fn offline_usage() throws -> OfflineUsage            // kept and other files, the budget
  fn remove_kept_files() throws -> CacheCleared        // the confirmed "Remove kept files"
  fn available_offline(item_ids: List<String>, editor_package: String,
                       accepted_mime_types: List<String>, sticker: Boolean) -> List<Boolean>
                                                       // per item: prepare would work offline
  fn server_unreachable() -> Boolean                   // the lists answer from the cache now

  // the server's reachability (docs/offline.md; no network unless said)
  fn server_reach() -> ServerReach
  async fn server_reach_change(seen: ServerReach) throws -> ServerReach
                                                       // waits until it differs from seen; follows
                                                       // a new server's session
  fn check_server()                                    // checks now, in the background (network);
                                                       // any thread

object PendingSearch                                   // SearchAnswer.pending
  async fn server_results() throws -> ItemList?        // the server's results (offline = false);
                                                       // null: it failed, the offline ones stand;
                                                       // once

object PendingAccount                                  // AccountAnswer.pending
  async fn server_account() throws -> Account?         // the server's word: the account, or null
                                                       // for a refused (revoked) token; throws
                                                       // Unreachable when it gave none (the cached
                                                       // account stands); once
  async fn offline_run() throws -> OfflineOutcome      // the downloader (network); stops when the
                                                       // coroutine is cancelled
  fn offline_status() -> OfflineStatus
  async fn offline_status_change(seen: OfflineStatus?) throws -> OfflineStatus
                                                       // waits until it differs from seen
  fn pause_offline()
  fn resume_offline()
  fn cancel_offline()                                  // ends the run going on
```

Records and enums:

```
record LibraryDirs { config_file: String, cache_dir: String, token_file: String }
record Account     { username: String, display_name: String?, server_url: String,
                     auth: AuthKind, is_admin: Boolean }
record AccountAnswer { account: Account?,           // as account() answers
                     pending: PendingAccount? }     // the server's word to come, or null
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
                     liked: Boolean,    // by the account, as the saved favorites say
                     text: String }     // the words in the picture, "\n" between lines;
                                        // "" for none and from a 0.7.0 server (default "")
record ItemList    { items: List<Item>, offline: Boolean }
record SearchAnswer { list: ItemList,               // to show now
                     pending: PendingSearch? }      // the server's results to come, or null
enum   ServerReach { Unknown, Reachable,
                     Unreachable { since: Timestamp } }  // Kotlin: java.time.Instant
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
                     emojis: List<String>,
                     text: String? }    // the user's words in the picture; null or blank:
                                        // not sent, the server reads them (default null)
record SyncState   { last_sync: i64?, last_error: LibraryErrorKind?,
                     pending_usage: u32, due: Boolean, cache_changed: Boolean,
                     synced: Boolean }      // this call ran a sync that went through
                                            // (default false)

// keeping offline
enum   OfflineScope { Sticker { item_id: String }, Pack { pack_id: String }, Library }
enum   KeepReason  { Sticker, Pack, Library, Used, Liked, OwnPack }
enum   OfflinePart { Thumbnails, Kept { scope: OfflineScope }, Used, Liked, OwnPacks }
record OfflineOptions { sticker_renditions: Boolean = true, stills: Boolean = true,
                     animations: Boolean = true, videos: Boolean = false,
                     max_file_mb: u64 = 0, max_mb: u64 = 1024,
                     auto_thumbnails: Boolean = true, auto_used: Boolean = false,
                     auto_used_count: u32 = 200, auto_liked: Boolean = false,
                     auto_own_packs: Boolean = false, mobile_data: Boolean = false }
record OfflineTotals { files: u64, bytes: u64, present_files: u64, present_bytes: u64 }
record OfflineSkips { kind_off: u64, too_large: u64, not_cached: u64, no_rendition: u64 }
record OfflinePartTotals { part: OfflinePart, name: String?, items: u64, totals: OfflineTotals }
record OfflinePlan { totals: OfflineTotals, items: u64, skipped: OfflineSkips, failed: u64,
                     budget: u64?, over_budget: Boolean, parts: List<OfflinePartTotals> }
record OfflineEstimate { totals: OfflineTotals, items: u64, skipped: OfflineSkips,
                     kept_bytes_after: u64, budget: u64?, fits: Boolean }
record OfflineUsage { kept_files: u64, kept_bytes: u64, other_files: u64, other_bytes: u64,
                     budget: u64? }
record CacheCleared { removed_files: u64, freed_bytes: u64, kept_files: u64, kept_bytes: u64 }
enum   OfflineState { Idle, Downloading, Paused, WaitingForNetwork, LimitReached, DiskFull }
record OfflinePartProgress { part: OfflinePart, name: String?, files_done: u64,
                     files_total: u64, bytes_done: u64, bytes_total: u64 }
record OfflineStatus { state: OfflineState, files_done: u64, files_total: u64,
                     bytes_done: u64, bytes_total: u64, skipped: u64, failed: u64,
                     parts: List<OfflinePartProgress>, message: String?,
                     running: Boolean,      // a run goes on in this process
                     paused: Boolean }
enum   OfflineEnd  { Complete, Cancelled, WaitingForNetwork, LimitReached, DiskFull,
                     Unauthorized }
record OfflineOutcome { end: OfflineEnd, complete: Boolean, status: OfflineStatus }

error  LibraryError {
  NotSignedIn, Unauthorized, Unreachable, NotFound, RateLimited,
  NothingFits,            // prepare: no rendition the field accepts
  InvalidInput { field },  // e.g. a malformed server URL or pairing code
  Server { status: u16 },  // any other HTTP error
  Io, Other,
  NotOnDevice             // the server unreachable and the file not on this phone
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
  config file that is not a valid config. A file that `prepare` or
  `motion` needs, not on this phone with the server unreachable, is
  `NotOnDevice` (the core's `CoreError::NotOnDevice`; up to 0.8.0 it was
  `Unreachable`), so the app says the sticker is not kept offline instead
  of naming a network error. The provider reports it as `unreachable`
  (its contract is unchanged).
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
  token is stored (a fresh install) and `Unreachable` when one is. It
  does not wait for a server that does not answer when the cache knows
  the account (`Session::account_steps`, docs/offline.md): with the
  server believed unreachable the cached user at once, else the server's
  answer if it comes within 300 ms, else the cached user; with nothing
  cached it asks the server and waits, as before. `account_steps()` gives
  the same answer and, when it is the cached user because the server had
  not answered, `pending`: the launcher shows the library at once and,
  when `server_account` then says null (the token was refused, a revoked
  one), goes to onboarding as it would have at the start; when it throws
  (no answer) the account stands. The keyboard's empty Recent tab asks
  `account()`, so it no longer waits for the request's timeout. Behind
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
  suggestion, as the desktop popup does, except `e:` and `text:` values,
  which go as typed (the search matches an emoji name, and the start of
  the last word of a text, typos included, itself). The one-letter keys
  `v:`, `t:`, `s:`, `b:`, `x:` (and `e:`) are `vibe:`, `tag:`, `set:`,
  `by:`, `text:` (and `emoji:`); the query goes to the server with them
  written long, except `e:`, so a
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
  items the cache's account owns (all cached items' while it does not
  know the account), most used first, and while a name is typed (`e:sk`)
  the emojis it selects, those first (see [search.md](search.md)). For
  `x:` (or `text:`) the values are the words written on the cached items,
  read from the cache's search index as they are typed (`x:lo` offers
  `lol`, `love`; [search.md](search.md#suggestions)), each one word as
  the index keeps it (`пятниця` for `x:п'ят`); the search takes the
  value as typed, so `chosen` is only on a word typed whole. Only the
  emoji labels have a space, so the app's and the keyboard's chips split
  them at the first one (`display/HintLabel.kt`): the emoji at the chip's
  size, the name after it in the secondary colour. While a
  bare word is typed: the tags, vibes and packs containing
  it as filters (`rea` offers `tag:reaction`), those starting with it
  first, then the filter keys the word starts (`t` offers `tag:`,
  `text:` and `type:`). With nothing typed, or a space at the end: the
  keys `tag:`, `vibe:`, `e:`, `text:`, `set:`, `by:`, `type:`, `is:`,
  `sort:`; a key's label shows its one-letter form (`tag: (t:)`, `vibe:
  (v:)`, `text: (x:)`, `set: (s:)`, `by: (b:)`), its `query` is the
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
  The rule is the core's `offline::played_rendition` (moved there from
  this crate unchanged), and the library opens its session with
  `with_grid_playback(true)`: a sticker kept offline keeps the file the
  grids play too.
- **Keeping offline** (docs/offline.md). `keep_offline` and
  `release_offline` store the choice in the offline cache (this phone
  only) and flag or release what is on the phone at once; `offline_run`
  downloads the rest, in the core's order, four at a time, and is what
  the app's WorkManager job calls. It ends when everything was tried,
  the server is unreachable (`WaitingForNetwork`; a run tries the server
  even while the lists believe it away), the budget or the storage is full, the sign-in is
  refused, or it is cancelled: by `cancel_offline`, an account or server
  change, "Remove kept files", or the calling coroutine (WorkManager
  stopping the job); a dropped run leaves no partial file. Runs never
  overlap. `pause_offline` holds the downloads until `resume_offline`.
  The status is one per library: `offline_status` reads it,
  `offline_status_change(seen)` waits until it differs from what the
  caller saw (a run's progress, its start and end, a pause, resume or
  cancel). `set_offline_options` takes the app's settings (the config's
  `offline.targets` stay; `auto_used_count` is kept within 1 to 2000) and
  holds them for a new server's session too; until it is called the
  core's defaults apply, the same as `OfflineOptions()`'s in Kotlin
  (`tests/interface.rs` checks them). `mobile_data` is the app's: the
  library does not read it. `clear_cache` deletes only the files not
  kept; `remove_kept_files` deletes the kept ones and forgets the keeps.
- **Offline marks and inserting offline.** `available_offline(ids,
  editor_package, accepted_mime_types, sticker)` says per item whether a
  rendition `prepare` would choose for that field is on this phone (kept
  or cached): `variant_for` over the item's renditions that are here
  (`Session::variant_on_device`). `prepare` with the server unreachable
  uses the best rendition when it is here, else that one, so a mark and
  a pick agree. The app asks with no package and `image/*`, `video/*`
  (what its share uses), the keyboard with the field's package and its
  types plus those. `server_unreachable` is the fail-fast state below
  (`server_reach` is `Unreachable`): the lists answer from the cache now.
- **Pack covers.** `pack_cover` is the pack's own cover image, else its
  cover item's thumbnail (the first item unless one was chosen; offline,
  the first cached item by name), fetched unless cached. Null for a pack
  with neither, and for one `packs` or `own_packs` has not listed since
  the account last changed. Offline and not cached: `Unreachable`.
- **An unreachable server.** Once a request finds the server unreachable
  (no connection, a timeout, or 502 to 504 from a proxy), the reads fail
  as `Unreachable` at once instead of each waiting for the timeout, so
  the lists answer from the cache straight away
  (`stickpile_core::ApiClient::with_fail_fast`, docs/offline.md
  "Reachability"). Meanwhile the server is checked in the background
  every 15 s for five minutes, then every 60 s, whether or not anything
  reads; when it answers, the reads go to it again. What the user asks
  for tries the server whatever it did a moment ago: `sync_now`,
  `prepare`, `toggle_like`, `upload` and the logins. What the library
  knows is `server_reach` (`Unknown` until a request or check, `Reachable`,
  `Unreachable { since }`, the core's `Reach`), and `server_reach_change`
  waits until it differs from what the caller saw (a long poll, cancelled
  with the coroutine; it follows the new session after a server change).
  `check_server` checks at once in the background, from any thread: the
  app calls it when its library screen resumes, the keyboard each time it
  opens, and `sync.NetworkWatch` when a network appears (a default-network
  callback of `ConnectivityManager`, registered when the library opens).
  The library screen and the keyboard follow the value
  (`sync.ServerWatch`): when it turns `Reachable` after an `Unreachable`
  since the last `Reachable` (or while what they show came from the
  cache), they load what they show again in place and the app syncs
  (`StickpileApp.syncBackOnline`, one at a time).
- **Results without waiting.** `search_steps` is `Session::search_steps`:
  with the server believed unreachable the offline cache answers at once
  and nothing is asked (`SearchAnswer.pending` null); else the server is
  asked, and its results come if they are there within 300 ms; if not,
  the offline cache's come first (`list.offline = true`) with
  `pending`, whose `server_results` waits for the server's: they replace
  the offline ones, or null when it failed (the offline ones stand, the
  server is then believed unreachable), or it throws what the server
  answered instead (`Unauthorized`). Cancelling the wait (the coroutine)
  leaves the request running, so that its outcome still tells whether the
  server is there. The app and the keyboard search with it, in the
  generation of the search (a newer search drops the wait), and close the
  `PendingSearch` once answered. `search` stays as it was, waiting for
  the server, for the provider's one answer; `pack_search` and the other
  lists have no second step.
- **Pairing input.** `parse_pairing_link` returns the server URL
  normalised (above), the one `login_pairing` contacts, and null for a
  link whose URL has a user name or password, a query or a fragment.
  `PairingLink.code` and `normalize_pairing_code` give
  the display form `XXXX-XXXX-XXXX`.
- **Upload.** The file is streamed from disk, never held in memory whole.
  With `name = null` the server names the item after the file
  name (the path's last part), so the app names its temporary copy after
  the shared file. A `text` that is not blank goes in the upload's `meta`
  as the user's (`text_source: user`, the server cleans it and refuses
  more than 2000 characters, `InvalidInput` on `file`); a null or blank
  one is left out, so the server's OCR fills it. A 0.7.0 server ignores
  it.

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
  `docs/android-input.md` fill it in; one is in: a still sent with the
  sticker flag goes into Telegram as its 512 px sticker WebP, and
  Telegram sends that as a sticker (the owner, on a phone, 2026-11-18).
  The flag (`sticker`) comes from Send as a sticker in the details, and
  from a tap with "Prefer stickers" on (`ime.SendChoice`). With it, only
  an app that has stickers (Telegram and its forks) gets the sticker
  rendition first, and every other rendition stays after it in the usual
  order; for any other app the flag changes nothing.

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
- `ime.ImeSettings`: the picker keyboard's switches (prefer stickers,
  search the draft, delete the draft, stay open) and its height
  (`ime.PickerHeight`), in the preferences `ime_settings`, which the
  keyboard reads in the same process when it needs them (prefer stickers
  on each tap and long press, the height when it opens).
- `motion.Motions`: the moving renditions for the grids and details of
  the app and the keyboard (the bridge's `motion`, after a short wait so
  that a fling downloads nothing, at most three downloads at once), and
  the decoded animated pictures (`ImageDecoder`, API 28 and later).
  `motion.LoopingVideoView` plays a video in a `TextureView` with a
  muted, looping `MediaPlayer`, only while on screen and at most six at
  once in the process (`motion.Slots`); the players are made, started and
  released on a thread of their own (`stickpile-video`), never on the
  main thread, which a scrolling grid would hold up. `motion.MotionPolicy`
  says when an item plays (`display.Autoplay`).
- The decoded pictures in memory: the app's thumbnails
  (`ui.library.Thumbnails`), the keyboard's (`ime.ThumbnailLoader`, one
  cache for the process, so that the keyboard the system makes again each
  time it is chosen shows them at once) and the moving pictures
  (`motion.Motions`). All three are cleared with the cache and on another
  account, and trimmed when the system is short of memory
  (`StickpileApp.onTrimMemory`, `MemoryTrim`): all of them once nothing
  of the process is on screen.
- `sync.SyncScheduler`: `ensureScheduled(context)` at app start and after a
  login, `cancel(context)` after a logout (in the app's scope, also when
  the settings screen went) and when onboarding saved another server
  address.
- The server's reachability: `sync.NetworkWatch` asks the library to check
  the server when a network appears (registered once, when
  `StickpileApp.library` opens), and `sync.ServerWatch` (pure, tested on
  the JVM) is how the library screen (`ui.library.LibraryViewModel`) and
  the keyboard (`ime.PickerView`, while it is shown) follow the library's
  `server_reach_change`: back after an outage, they load what they show
  again in place and call `StickpileApp.syncBackOnline`, which runs one
  `sync_now` at a time. Both replace a list in place only once no finger
  is on the grid (`LibraryViewModel.pressing`, the keyboard's
  `RecyclerView` touch listener).
- `provider.PreparedFiles`: `uriFor(context, prepared)` turns a file from
  `StickpileLibrary.prepare` into a `/prepared/<token>/<name>` provider URI,
  for the input method's `commitContent` and for sharing from the app;
  `revokeAll()` revokes every token at once: after a sign-out and a cache
  clear (settings), after a sign-in as another account or on another
  server, and when onboarding saved another server address (launcher).
- `display.DisplaySettings`: what the grids show and the theme
  (`display.AppTheme`), shared by the app and the keyboard. The tile
  fields are kept as two sets (`TileField.stored`): those that start on
  and were turned off, and those that start off (`TEXT`) and were turned
  on, so that a field added later starts as it should for everyone. Every
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
- `offline.OfflineSettings`: the offline options (the bridge's
  `OfflineOptions`, whose Kotlin defaults are the core's) and the pause,
  in the preferences `offline` next to `display` (`OfflinePrefs`, a key
  each, so an option added later starts at the core's default).
  `StickpileApp.library` hands them to the library when it opens, the
  settings when they change. `offline.OfflineScheduler` enqueues
  `offline.OfflineWorker`, one unique WorkManager job (`stickpile-offline`,
  an unmetered network unless mobile data is allowed, storage not low),
  after each sync that went through (`sync.CacheChanges`, `SyncState.synced`),
  after a keep and when the options change; the worker runs
  `StickpileLibrary.offline_run` (again when asked during a run, at most
  five times) and nothing while paused. `offline.OfflineProgress` and
  `offline.OfflineMarks` decide what the settings and the tiles say, from
  the library's answers and WorkManager's job (pure, tested on the JVM).

Packages: `ui` (onboarding, the library screen, settings, licences),
`ime` (the picker keyboard), `provider` (the content provider below),
`share` (the share target and upload), `sync` (background sync), `offline`
(keeping stickers offline: its settings, job and marks). Strings
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
