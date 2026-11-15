## F-Droid

> The F-Droid notes for the Android app (docs/android.md has a summary).
> Nothing has been submitted. Every F-Droid rule below was read on
> 2026-11-15 at the URL given; quotes are verbatim except that typographic
> quotes and dashes are written in ASCII.

The app is meant for F-Droid's main repository. The draft build recipe is
[`android/fdroid/io.github.dolfost.stickpile.yml`](io.github.dolfost.stickpile.yml),
which goes to fdroiddata as `metadata/io.github.dolfost.stickpile.yml`.
The listing texts are in `fastlane/metadata/android/` in this repository,
where F-Droid reads them.

Sources (short names used below):

| | |
|---|---|
| [IP] | https://f-droid.org/docs/Inclusion_Policy/ |
| [BMR] | https://f-droid.org/docs/Build_Metadata_Reference/ |
| [DESC] | https://f-droid.org/docs/All_About_Descriptions_Graphics_and_Screenshots/ |
| [AF] | https://f-droid.org/docs/Anti-Features/ |
| [QSG] | https://f-droid.org/docs/Submitting_to_F-Droid_Quick_Start_Guide/ |
| [RB] | https://f-droid.org/docs/Reproducible_Builds/ |
| [MRT] | https://gitlab.com/fdroid/fdroiddata/-/raw/master/.gitlab/merge_request_templates/Default.md (the merge request template) |
| [CONTRIB] | https://gitlab.com/fdroid/fdroiddata/-/raw/master/CONTRIBUTING.md |
| [SCN] | https://gitlab.com/fdroid/fdroidserver/-/raw/master/fdroidserver/scanner.py |
| [BLD] | https://gitlab.com/fdroid/fdroidserver/-/raw/master/fdroidserver/build.py |
| [GW] | https://gitlab.com/fdroid/gradlew-fdroid/-/raw/main/gradlew.py (the `gradle` of the build server) |
| [GTL] | https://gitlab.com/fdroid/gradle-transparency-log/-/raw/master/checksums.json |
| [BS] | https://gitlab.com/fdroid/fdroidserver/-/tree/master/buildserver (Dockerfile, provision-apt-get-install, provision-gradle) |
| [SUSS] | https://fdroid.gitlab.io/fdroid-suss/suss.json (the scanner's list of non-free libraries) |
| [NYM] | https://gitlab.com/fdroid/fdroiddata/-/raw/master/metadata/net.nymtech.nymvpn.yml |
| [SC] | https://gitlab.com/fdroid/fdroiddata/-/raw/master/metadata/chat.schildi.android.yml |
| [DC] | https://gitlab.com/fdroid/fdroiddata/-/raw/master/metadata/com.b44t.messenger.yml |
| [FEN] | https://gitlab.com/fdroid/fdroiddata/-/raw/master/metadata/org.mozilla.fennec_fdroid.yml |
| [MUL] | https://gitlab.com/fdroid/fdroiddata/-/raw/master/metadata/net.mullvad.mullvadvpn.yml |

### How F-Droid builds the app

From [BLD] and the recipe: F-Droid clones the repository at the release
commit, runs the recipe's `sudo` (Debian's `rustup`, `gcc`, `libc-dev`) and
`prebuild` (`rustup default 1.95.0`, `rustup target add` for the three
ABIs), deletes `gradlew`, `gradlew.bat` and `gradle-wrapper.jar`, scans the
source tree, runs `build` (`cargo install cargo-ndk@4.1.2 --locked`) and
then `gradle assembleRelease` in `android/app`. That one Gradle run builds
the Rust library and the Kotlin bindings (`cargoNdk`, `uniffiBindings`), as
it does locally. `gradle` there is gradlew-fdroid, which takes the Gradle
version from `android/gradle/wrapper/gradle-wrapper.properties` [GW]. The
NDK comes from the recipe's `ndk: r28c` (28.2.13676358); F-Droid sets
`ANDROID_NDK_HOME` to it, which cargo-ndk reads [BMR, `ndk:`]. F-Droid signs
the APK with its own key unless the reproducible-build setup below is used.

### Checklist

**Licence acceptable and declared: yes.** `LICENSE` and `Cargo.toml`
(`license = "AGPL-3.0-or-later"`); the recipe has `License:
AGPL-3.0-or-later`. [IP]: "All applications in the repository must be Free,
Libre and Open Source Software (FLOSS)". fdroidserver's lint accepts the id
(`'AGPL-3.0-or-later'` is in `APPROVED_LICENSES` of
https://gitlab.com/fdroid/fdroidserver/-/raw/master/fdroidserver/lint.py),
and `io.element.android.x` is in F-Droid under the same licence.

**Builds with free tools only: yes.** [IP]: "The complete application
building process requires a 100% FLOSS toolchain including Debian-packaged
tools." The build uses Gradle (via gradlew-fdroid), the Android Gradle
Plugin from Google's Maven, Debian's OpenJDK, Debian's `rustup`, Rust from
rustup, cargo-ndk built from source, and the NDK that fdroidserver installs
itself for the `ndk:` field. The prebuilt parts are allowed by [IP]: "The
Android SDK, Flutter SDK and Hermes have permission to use official prebuilt
binaries" and "The use of prebuilt FLOSS binaries from PyPIWheels, Nix
cache, Rust/Rustup, Golang and Node.js (current versions) and compilers or
build tools which are not included in Debian can be acceptable." The same
toolchain pattern is in current recipes: [NYM] `apt-get install -y make gcc
libc-dev rustup`, `rustup default "$RUST_VERSION"`, `cargo install
cargo-ndk@4.1.2 cargo-license@0.7.0 --locked`; [SC] `cargo install
cargo-ndk@4.1.2 --locked`; [DC] `apt-get install -y make g++ cmake rustup`.

**No prebuilt binaries in the tree: yes, apart from the Gradle wrapper,
which F-Droid removes.** I applied [SCN]'s file rules (`scan_source`) to
every file tracked at e1da062: the only matches are
`android/gradle/wrapper/gradle-wrapper.jar`, `android/gradlew` and
`android/gradlew.bat`. The scanner removes those without counting an error:

```python
            elif curfile in (
                'gradle-wrapper.jar',
                'gradlew',
                'gradlew.bat',
                'gradle-daemon-jvm.properties',
            ):
                removeproblem(curfile, path_in_build_dir, filepath, json_per_build)
```

(`removeproblem` returns "0 as we deleted the offending file".) The PNG and
SVG files under `docs/screenshots/`, `crates/stickpile-mac/` and
`crates/stickpile-server/assets/` are not flagged (images are only checked
when executable). Every `Cargo.toml` has `Cargo.lock` above it, which the
scanner requires (`DEPFILE = {"Cargo.toml": ["Cargo.lock"], ...}`). There
are no submodules.

**Every dependency free and from an accepted repository: yes, so far.**
`android/settings.gradle.kts` uses `google()` and `mavenCentral()` only.
[IP]: "Applications can download prebuilt FLOSS binaries with specific
conditions from trusted Maven repositories. Those include Maven Central,
Google Maven, OSS Sonatype, OSS JFrog, JitPack.io and Clojars. Those
binaries must still be freely licensed, simply being included in one of
those repositories is not enough." JNA's POM on Maven Central
(https://repo1.maven.org/maven2/net/java/dev/jna/jna/5.19.1/jna-5.19.1.pom)
declares `LGPL-2.1-or-later` and `Apache-2.0`. Rust crates come from
crates.io through the committed `Cargo.lock` (`--locked`). For the libraries
still to be added, the scanner fails the build on any Gradle coordinate in
[SUSS] (data of 2026-11-10); the entries that matter here are
`com.google.mlkit` and `io.github.g00fy2.quickie` (ML Kit, so not for the
QR scanner), `androidx.work:work-gcm`, `androidx.*play-services`,
`com.google.android.gms`, `com.google.android.play:app-update` and the
other Play libraries, and `com(\.google)?\.firebase[.:]`. Their
`gradle_signatures` become `err_gradle_signatures` in [SCN], so a match
fails the build. `com.google.zxing` and `com.journeyapps` are not in it.

**JNA's prebuilt native code: accepted as is; do nothing.** The AAR ships
`jni/<abi>/libjnidispatch.so` for arm64-v8a, armeabi, armeabi-v7a, mips,
mips64, x86 and x86_64 (listed from
https://repo1.maven.org/maven2/net/java/dev/jna/jna/5.19.1/jna-5.19.1.aar);
`abiFilters` keeps three. Evidence that F-Droid takes it from Maven Central
unchanged:
- [NYM] (NymVPN, UniFFI bindings generated with `uniffi-bindgen generate
  --language kotlin`, built with cargo-ndk) uses the same artifact:
  `jna = "5.19.1"` and `jna = { module = "net.java.dev.jna:jna",
  version.ref = "jna" }` in `nym-vpn-android/gradle/libs.versions.toml`,
  `implementation(libs.jna) { artifact { type = "aar" } }` in
  `nym-vpn-android/core/build.gradle.kts` (commit 9f1dd430 of
  github.com/nymtech/nym-vpn-client, the one [NYM] builds). The recipe has
  no srclib, `scandelete` or patch for JNA, and its APKs are even published
  with the developer's signature (`binary:` and `AllowedAPKSigningKeys:`).
- [SC] builds matrix-rust-sdk's UniFFI bindings from source; their
  `buildSrc/src/main/kotlin/Dependencies.kt` (tag sc-sdk-v0.4.25 of
  github.com/SchildiChat/matrix-rust-components-kotlin) has `const val jna =
  "net.java.dev.jna:jna:${DependenciesVersions.jna}@aar"`, and the recipe
  does nothing about JNA.
- [FEN] builds Mozilla's application-services, whose
  `gradle/libs.versions.toml` (tag v160.0) has `jna = "5.18.1"`; Fennec's
  `prebuild.sh` and `build.sh` (gitlab.com/relan/fennecbuild) do not
  mention JNA.
- fdroiddata has no JNA srclib: none of the 614 files in `srclibs/` has
  "jna" in its name (listed through the GitLab API).
- Mechanism: the scanner walks only the checked-out source
  (`os.walk(build_dir, topdown=True)` in [SCN]); Gradle keeps downloaded
  AARs in its cache, outside that tree.

**No tracking, advertising or non-free network service: yes, by design.**
The app talks only to the server the user enters; no analytics, crash
reporting or ads. [IP] forbids "proprietary tracking or advertising
libraries and analytics tools such as Google Play Services and Firebase and
Crashlytics". The server (same repository, AGPL) has optional AI with a
local ONNX model (`[ai]` in docs/configuration.md) and an optional Telegram
bot integration; the app reaches neither. Usage events (`record_use`) go to
the user's own server; the listing says so (see the Tracking judgement
below).

**Useful with a self-hosted free server: yes.** The server is free software
in the same repository, and the app has no default server. Precedent:
`org.jellyfin.mobile` (a client for a self-hosted server) is in F-Droid
with no AntiFeatures
(https://gitlab.com/fdroid/fdroiddata/-/raw/master/metadata/org.jellyfin.mobile.yml).
The listing says up front that a stickpile server is needed.

**Anti-features: none expected.** Against the definitions in [AF] and
[BMR] (`AntiFeatures`):
- TetheredNet: not applied: "This Anti-Feature would not be applied if
  there is a simple configuration option that allows pointing the app to a
  running instance of an alternative, publicly available, self-hostable
  server solution." Here there is no default instance at all; the user
  enters the address of their own server.
- NonFreeNet ("promote or depend entirely on a proprietary network
  service"): the only service is the AGPL server.
- NonFreeDep ("an app that doesn't run, or is not useful, unless you have
  Google Maps installed"): the keyboard works with any app; the per-app
  rendition table (Telegram, Discord, WhatsApp, Slack) only picks a format.
- NonFreeAdd ("promote other Non-Free applications or plugins"): the
  listing names no proprietary app; keep it that way (judgement, see
  below).
- Ads, NonFreeAssets, KnownVuln, NoSourceSince, DisabledAlgorithm: none.
  The app icon is the macOS app's (`crates/stickpile-mac/app/Resources/`),
  redrawn as vector drawables in `android/app/src/main/res/`; both are in
  this repository under its licence (AGPL-3.0-or-later).
- NSFW ([BMR]: "the app contains content that the user may not want to be
  publicized"): the app ships no content; memes come from the user's
  server.
- Tracking ([AF]: "apps that track you and/or report your activity to
  somewhere, either without your permission or by default"): the app
  reports which stickers are used, but only to the server the user chose,
  for the user's own recent/most-used lists. Not found: whether F-Droid
  counts this; the listing discloses it. Safer: a setting to turn usage
  reporting off.

**Gradle wrapper may be committed: yes.** The F-Droid docs say nothing
about it (not found in [IP], [BMR], [QSG]). The tooling handles it: the
scanner removes `gradle-wrapper.jar`, `gradlew` and `gradlew.bat` (quoted
above), [BLD] also deletes `gradlew` and `gradlew.bat` in every Gradle
directory before scanning, and the build runs gradlew-fdroid, whose README
says "Unlike Gradle's _gradlew_, F-Droid's _gradlew-fdroid_ will always
verify the downloaded Gradle binary". It finds the version in
`gradle/wrapper/gradle-wrapper.properties` of the subdir, its parent or
grandparent (`for d in (root_path, root_path.parent,
root_path.parent.parent)` in [GW]), so `android/app` finds
`android/gradle/wrapper/gradle-wrapper.properties` and Gradle 9.6.0. [GTL]
lists `gradle-9.6.0-bin.zip` with sha256 `bbaeb2fe...a529a01`, the same as
our `distributionSha256Sum`.

**Also required, and not met yet:**
- **3 months of development: no.** [IP], Quality Control: "The application
  should have been actively developed for at least 3 months, unless: The
  application has been proven to have some users. The author has been
  maintaining other apps on F-Droid. The application is developed only for
  an event." [MRT]: "Pay special attention to Quality point 6." The first
  commit of this repository is 2026-11-10 and the Android code starts
  2026-11-15. Not found: whether the 3 months count from the project or
  from the Android app. Earliest safe submission: mid-February 2027.
- **Features in the description exist: at submission.** [IP]: "The
  application should be functional and implement all the features
  described in the description." Trim `full_description.txt` to what the
  submitted release really does.
- **Tags and auto-update: yes.** [MRT]: "Releases are tagged and auto update
  is enabled unless there is a special reason." Tags are `vX.Y.Z`; the
  recipe has `UpdateCheckMode: Tags ^v\d+\.\d+\.\d+$`.
- **Upstream metadata with en-US: yes.** [MRT]: "The `en-US` locale must be
  included."
- **Application id free: yes.** `metadata/io.github.dolfost.stickpile.yml`
  and https://f-droid.org/packages/io.github.dolfost.stickpile/ both
  returned 404 on 2026-11-15. [IP]: "It is advised to use an Application ID
  that stems from a domain name owned by the developer." `io.github.dolfost`
  is the owner's GitHub account domain, dolfost.github.io.

### Listing metadata

[DESC] reads `fastlane/metadata/android/<locale>/` at the repository root
(also `<subdir>/fastlane/...`, but "placing the structure anywhere else,
like in the gradle module's root, won't work"). Locales here: `en-US` ("the
F-Droid fallback language") and `uk` (the code F-Droid's own client uses in
https://gitlab.com/fdroid/fdroidclient/-/tree/master/metadata). Limits from
[DESC]: `title.txt` max 50 chars, `short_description.txt` max 80 chars and
mandatory ([QSG]: "less than 80 characters, no trailing dot"),
`full_description.txt` max 4000 chars and mandatory,
`changelogs/<versionCode>.txt` max 500 chars, named after the versionCode
"literally, no padding". fdroidserver counts characters of the decoded text
(`char_limits` in fdroidserver/common.py, read in
`_set_localized_text_entry` of fdroidserver/update.py), so a Cyrillic text
can have more bytes than the limit. "F-Droid will only use metadata files
from the latest release" [DESC]: listing changes appear with the next tag.

`changelogs/6000.txt` assumes the app first ships in 0.6.0. Every later
release may add `changelogs/<versionCode>.txt` (e.g. 6001 for 0.6.1).

Screenshots and graphics (only the icon is made; PNG or JPEG, [DESC]):
- `fastlane/metadata/android/en-US/images/phoneScreenshots/1.png`,
  `2.png`, ... (localized ones may go in `uk/images/phoneScreenshots/`);
- `fastlane/metadata/android/en-US/images/icon.png` (present: the macOS
  app's 512 px icon; [DESC] says "usually 512x512" for the Triple-T
  equivalent);
- `fastlane/metadata/android/en-US/images/featureGraphic.png` (optional,
  landscape, "usually 1024x500").

Screenshots cannot go to fdroiddata ([DESC]: "screenshots may not be added
to the fdroiddata repository"). The client's Latest tab shows only apps
with name, icon, summary, description, licence, a What's New entry, at
least one graphic and at least one of these translated [DESC].

### Submitting (the owner's steps)

1. Wait for the 3 months above, make sure the release does everything the
   description says, and decide the signing question below first (it
   cannot be changed later).
2. Merge this metadata, add screenshots, and tag the release (`v0.6.0`);
   the fastlane files must be in the tagged commit.
3. On GitLab, fork https://gitlab.com/fdroid/fdroiddata ("Please make sure
   your fdroiddata fork is public and your branch is not protected" [MRT])
   and create a branch named `io.github.dolfost.stickpile` [CONTRIB].
4. Copy the recipe to `metadata/io.github.dolfost.stickpile.yml`, set
   `commit:` to the full hash of the tag (`git rev-parse
   'v0.6.0^{commit}'`; [MRT]: "The `commit` field should be the full hash.
   Please don't use tag or branch in commit."), and remove the comments
   (the template says "PLEASE REMOVE ALL COMMENTS BEFORE SUBMITTING TO
   F-DROID DATA!").
5. Check it locally if you can ([CONTRIB] recommends it): `fdroid
   readmeta`, `fdroid rewritemeta io.github.dolfost.stickpile`, `fdroid
   checkupdates --allow-dirty io.github.dolfost.stickpile`, `fdroid lint
   io.github.dolfost.stickpile` (no warnings), `fdroid build -v -l
   io.github.dolfost.stickpile`. [QSG] runs these inside the build server
   container `registry.gitlab.com/fdroid/fdroidserver:buildserver`
   (`docker run ... --entrypoint /bin/bash -v ~/fdroiddata:/build:z -v
   ~/fdroidserver:/home/vagrant/fdroidserver:Z`). Without that, the
   fork's CI pipeline runs the same checks when you push.
6. Push the branch; "All pipelines should pass" and "All warnings and errors
   in the Reports tab should be fixed or explained" [MRT].
7. Open a merge request titled "New app: stickpile" ([MRT]: "should follow
   "New app: app name" format"), fill in the template (what the app does,
   screenshots, why it is unique, the checklist) and keep the `/label
   ~"New App"` line.
8. Answer reviewers quickly ([CONTRIB]). [QSG] lists what they check:
   licence, build system, "if your application uses any pre-compiled
   libraries or binary blobs", non-free dependencies, tracking and
   services, the update process, a build in an isolated environment.
   Expect questions about JNA's `libjnidispatch.so`, the Rust toolchain
   download, the server requirement, reproducible builds, APK size (ABI
   split) and the 3 months.
9. After the merge it takes "somewhere around 24 to 48 hours" to appear
   [QSG]. From then on every `vX.Y.Z` tag is picked up by the update
   checker, which opens the merge request for the new version itself.

### Reproducible builds

What F-Droid needs to publish the owner's signed APK instead of signing its
own build ([RB], "Exclusively publishing (upstream) developer-signed
APKs"): "the addition of the `Binaries` or `Builds.binary` directive to
specify where to get the binaries (APKs) from, and
`AllowedAPKSigningKeys` directive to ensure the expected signing key is
used." F-Droid builds the recipe, copies the signature from the owner's APK
onto its unsigned build and publishes the owner's APK only if that verifies
("the APKs must be completely identical before and after signing (apart
from the signature)"). So the release must attach a signed APK at a
versioned URL (e.g. `Binaries:
https://github.com/Dolfost/stickpile/releases/download/v%v/stickpile-%v.apk`;
the release workflow has no Android job yet) and the recipe gets the
certificate's SHA-256 ([BMR], `AllowedAPKSigningKeys`).

It is not a later option. [QSG]: "one can't easily switch to them later
because Android doesn't allow updates with a different signing key, meaning
users would have to reinstall. So we mainly encourage their use for new
apps." [MRT]: "if you don't enable reproducible build then the apk will be
signed with our key so you can't enable it later. If you can't enable this,
please add the reasons here." Decide before the first submission. Without
it, F-Droid signs with its own key and nothing below is needed.

What in this build can break reproducibility (all from [RB]):
- **Toolchains must match exactly.** "The Rust toolchain should be pinned
  to the same version as upstream"; "Different NDK versions also produce
  different binaries"; JDK: the build server offers "25 from Debian Trixie,
  21 from Debian Trixie, 17 from Debian Bookworm" and the doc shows dex
  differences between Java 17 and 21. The build server image is Debian
  trixie with `default-jdk-headless` [BS], which is OpenJDK 21
  (https://packages.debian.org/trixie/default-jdk-headless, 2:1.21-76);
  the dev shell has JDK 21 as well. cargo-ndk (4.1.2) and Rust (1.95.0)
  match today; not verified: whether rust-overlay's 1.95.0 and rustup's 1.95.0
  are byte-identical toolchains.
- **The NixOS aapt2.** The dev shell makes Gradle use the SDK's build-tools
  36.0.0 `aapt2` instead of the one the Android Gradle Plugin downloads;
  F-Droid uses the downloaded one. Release APKs for `Binaries` must be
  built where the plugin's own aapt2 runs (a CI runner, or the build
  server container above).
- **Rust: embedded paths.** "`--remap-path-prefix=<old>=<new>` can be added
  to strip build paths"; "`CARGO_HOME` path plays an important part too and
  ends up in the built libs". [SC] uses `RUSTFLAGS="--remap-path-prefix=$HOME/.cargo/=.cargo/
  ..."`. Or build in the same paths (F-Droid's server builds in
  `/home/vagrant/build/<app id>`).
- **Rust: parallelism and build id.** "For Rust code, you can set
  `codegen-units = 1`"; "passing `--build-id=none` to the linker" ([NYM]'s
  `nym-vpn-core/Android.mk` passes `-C link-args=-Wl,--build-id=none`).
- **C code** (libwebp, SQLite, ring through the cc crate): embedded paths;
  "The `-ffile-prefix-map` flag can be used to remove embedded build path"
  (for cargo builds, through `CFLAGS_<target>`; not verified).
- **Native library stripping, JNA included.** "Gradle seems to strip shared
  libraries by default, even the app is receiving the shared libraries via
  an AAR library"; "It is important to use the exact NDK version when
  rebuilding". JNA's `libjnidispatch.so` is the same file for both builds,
  but both strip it with their NDK: same NDK, or `doNotStrip`.
- **Timestamps.** "Embedded timestamps are the most common source of
  reproducibility issues". F-Droid sets `SOURCE_DATE_EPOCH` to the commit
  time (`set_FDroidPopen_env` in fdroidserver/common.py); set it the same
  way for the release build.
- **Android Gradle Plugin output.** VCS info: "please upload release APKs
  built after tagging, from a clean tree at the actual tagged commit";
  `baseline.prof` "is not reproducible" sometimes (Compose apps get one);
  if R8 is turned on later, its map id needs the fix [SC] does with
  reproducible-apk-tools.
- **Signing.** "Using `apksigner` from build-tools version 34 produces APKs
  verifiable by `apksigcopier`, but newer versions will fail." The signing
  block must not carry Google's dependency metadata: [SCN] counts block
  `0x504B4453: 'Dependency metadata'` as a problem, and [NYM]'s app sets
  `dependenciesInfo { includeInApk = false }`. Done in
  `android/app/build.gradle.kts` (with `includeInBundle = false`).

### Design decisions this research questions

- **Gradle wrapper jar: keep it.** Removed and replaced by gradlew-fdroid on
  F-Droid; 9.6.0 is in the transparency log (see the checklist).
- **JNA from Maven Central: keep it.** Same artifact and version as [NYM];
  no srclib exists; nothing in the recipe.
- **Gradle Plugin Portal: keep not using it.** [SCN]'s allowed repositories
  include `plugins.gradle.org/m2`, and [MUL] adds it, but [IP]'s list of
  trusted Maven repositories does not name it. If a plugin is only there,
  `gradlePluginPortal()` passes the scanner.
- **Repository layout: fine.** `subdir: android/app` in a repository that
  also holds a server and desktop clients is what [MUL]
  (`subdir: android/app`) and [NYM] (`subdir: nym-vpn-android/app`) do.
  Fastlane at the root is read. Consequence: every `vX.Y.Z` tag is an
  Android release on F-Droid, including server-only releases.
- **Application id: fine** (free, from the owner's GitHub domain).
- **JDK 21: done.** F-Droid's server is trixie, whose default JDK is 21;
  17 exists only "from Debian Bookworm", and "versions not from the
  current Debian stable (Trixie) might become unavailable as time passes,
  so you are encouraged to update as much as possible" [RB]. The dev shell
  and CI build with 21. Never use Gradle JVM toolchains (`jvmToolchain(17)`):
  the server sets `org.gradle.java.installations.auto-download=false` [BS,
  provision-gradle], and the scanner deletes `gradle-daemon-jvm.properties`.
- **Reproducible builds "later": not possible.** Decide before the first
  submission (above).
- **One universal APK: measure it.** [QSG]: "It's not a hard requirement to
  setup ABI split but if the apk is large and ABI split can reduce the apk
  size effectively, this is highly encouraged." F-Droid splits by separate
  build blocks with their own version codes (`VercodeOperation`, as [DC]
  `10 * %c + 1` ...), and checks that each APK has the recipe's version
  code [BLD]; `checkStickpileVersion` allows only one code per version.
  If splits may be needed, choose the version code scheme before the first
  release (codes can only grow).
- **Rust version only in the dev shell.** F-Droid cannot read it, so the
  recipe hardcodes 1.95.0 and the update bot copies it into every new
  build. A `rust-toolchain.toml` at the repository root would let the
  recipe read it as [SC] reads `rust-toolchain.toml` (`rustup default
  $rustVersion`); otherwise bump the recipe by hand (MaintainerNotes).
- **Build time.** Four Rust builds (three ABIs and the host
  `uniffi-bindgen`) must fit the default limit: "The default is 7200 (2
  hours)" [BMR, `timeout`]; [MUL] raises it to 10800.
- **Usage reporting and app names (judgement, no rule found).** See
  Tracking and NonFreeAdd above.
