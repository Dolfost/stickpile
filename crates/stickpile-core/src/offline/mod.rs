//! Keeping stickers on the device (docs/offline.md): the user keeps a
//! sticker, a pack or the whole library, automatic rules keep more, and the
//! background downloader fetches the files so they copy, insert and play
//! with the server unreachable.
//!
//! * One store: kept files are ordinary cache files (`files/thumb/...`,
//!   `clip/<media>.<variant>/...`), found by the lookup
//!   [`Session::prepare_copy`], [`Session::fetch_variant`] and
//!   [`Session::thumbnail`] already use. They carry the `kept` flag in the
//!   cache's `files` table: the size cap never evicts them and "Clear cache"
//!   leaves them ([`crate::cache::Cache::evict`]).
//! * What to keep is stored ([`Scope`], in `offline_keeps`); what that
//!   means in files is derived every time ([`Plan`]): (kept scopes and
//!   automatic rules) x (cached items) x [`needs`], minus what is present.
//!   A sync that adds an item to a kept pack so adds its files; one that
//!   removes an item makes its kept files go ([`Session::offline_refresh`]).
//! * [`Session::offline_run`] downloads the missing files, driven by the
//!   shells ([`OfflineControl`] pauses, resumes, cancels and reports
//!   [`OfflineStatus`]).
//!
//! [`Session::prepare_copy`]: crate::Session::prepare_copy
//! [`Session::fetch_variant`]: crate::Session::fetch_variant
//! [`Session::thumbnail`]: crate::Session::thumbnail
//! [`Session::offline_refresh`]: crate::Session::offline_refresh
//! [`Session::offline_run`]: crate::Session::offline_run

pub(crate) mod run;
pub(crate) mod session;

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};
use stickpile_api::routes::{self, path};
use stickpile_api::{ItemSummary, MediaKind, TargetApp, Variant, VariantInfo};

pub use crate::config::{AutoKeepConfig, OfflineConfig as OfflineOptions};
pub use run::{OfflineControl, OfflineOutcome, OfflineState, OfflineStatus, RunEnd, ScopeProgress};
pub use session::{Estimate, OfflineUsage};

use crate::cache::safe_component;
use crate::session::{clip_rel, pick_variant};

const MIB: u64 = 1024 * 1024;

// -- what the user keeps -----------------------------------------------------

/// What the user chose to keep on this device. Stored in the cache
/// (`offline_keeps`), not on the server and not synced.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "scope", content = "id", rename_all = "lowercase")]
pub enum Scope {
    /// One sticker, by item id. Always downloaded whole, whatever the
    /// per-kind options say.
    Item(String),
    /// A pack, by set id: its items now and whatever a sync adds later.
    Set(String),
    /// Everything the cache knows this account can see, now and later.
    Library,
}

impl Scope {
    /// `(scope, id)` as stored: `item`/`set` with the id, `library` with "".
    pub fn key(&self) -> (&'static str, &str) {
        match self {
            Scope::Item(id) => ("item", id),
            Scope::Set(id) => ("set", id),
            Scope::Library => ("library", ""),
        }
    }

    /// The scope stored as `(scope, id)`; `None` for one this version does
    /// not know.
    pub fn from_key(kind: &str, id: &str) -> Option<Scope> {
        match kind {
            "item" if !id.is_empty() => Some(Scope::Item(id.to_string())),
            "set" if !id.is_empty() => Some(Scope::Set(id.to_string())),
            "library" => Some(Scope::Library),
            _ => None,
        }
    }
}

/// Why an item is kept. Reasons combine: an item stays until its last
/// reason is gone, and then its files go back to ordinary
/// least-recently-used life (they are not deleted at once).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    /// Kept by itself ([`Scope::Item`]).
    Item,
    /// Its pack is kept ([`Scope::Set`]).
    Set,
    /// The library is kept ([`Scope::Library`]).
    Library,
    /// One of the most recently copied (`offline.auto.used`).
    Used,
    /// Liked (`offline.auto.liked`).
    Liked,
    /// In a pack the account owns (`offline.auto.own_packs`).
    OwnPack,
}

impl Reason {
    /// The download group of an item kept for this reason.
    fn group(self) -> Group {
        match self {
            Reason::Item | Reason::Used | Reason::Liked => Group::Stickers,
            Reason::Set | Reason::OwnPack => Group::Packs,
            Reason::Library => Group::Library,
        }
    }
}

/// Download order: thumbnails, then stickers (kept by themselves, used,
/// liked), then packs, then the library; most used first within a group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Group {
    Thumbnails,
    Stickers,
    Packs,
    Library,
}

/// What progress and totals are reported for: each kept [`Scope`] and
/// each automatic rule that is on.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanScope {
    /// `offline.auto.thumbnails`: every item's thumbnail.
    Thumbnails,
    /// A scope the user keeps.
    Kept(Scope),
    /// `offline.auto.used`.
    Used,
    /// `offline.auto.liked`.
    Liked,
    /// `offline.auto.own_packs`.
    OwnPacks,
}

// -- which files a kept item needs -------------------------------------------

/// What a device does with kept stickers: the chat apps it copies to
/// (resolved: [`OfflineOptions::targets`], or the default target and the
/// targets of the copy history, [`crate::Session::offline_device`]),
/// whether its grids play animations and whether it inserts into text
/// fields (the Android app and its keyboard do both; the desktop popups
/// show thumbnails and copy).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    pub targets: Vec<TargetApp>,
    pub plays_motion: bool,
    #[serde(default)]
    pub inserts: bool,
}

/// What a kept file is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum FileRole {
    /// The grid's thumbnail ([`crate::Session::thumbnail`]).
    Thumbnail,
    /// What a copy to `target` uses, with or without the sticker flag
    /// ([`crate::Session::prepare_copy`]).
    Copy { target: TargetApp, sticker: bool },
    /// What an insert into a text field of `target` uses, with or without
    /// the sticker flag, for one of [`INSERT_FIELDS`] ([`crate::variant_for`]).
    Insert { target: TargetApp, sticker: bool },
    /// What the grid plays ([`played_rendition`]).
    Played,
    /// A kept pack's cover in the pack lists ([`crate::Session::pack_cover`]).
    Cover,
}

/// The text fields an inserting device ([`Device::inserts`]) keeps a kept
/// sticker's insert for, as a field lists the MIME types it takes
/// (`EditorInfoCompat.getContentMimeTypes`); what [`crate::variant_for`]
/// picks depends only on which types are covered, so each entry stands for
/// every list that covers the same:
///
/// - `image/*`: also the usual chat field (`image/gif, image/*, image/jpg,
///   image/png, image/webp`): a still's original, an animation's GIF or
///   WebP, a video's GIF.
/// - `image/png, image/gif`: a WebP still's PNG, a WebP animation's GIF.
/// - `image/png`: a field for stills only.
/// - `image/*, video/*`: what the keyboard shares when the field takes
///   nothing (`InsertFlow.SHARE_TYPES`), and a field that takes videos.
///
/// Which lists the chat apps really give is not recorded yet
/// (docs/android-input.md); a field that covers other types may insert
/// another rendition offline, or share.
pub const INSERT_FIELDS: &[&[&str]] = &[
    &["image/*"],
    &["image/png", "image/gif"],
    &["image/png"],
    &["image/*", "video/*"],
];

/// How a sticker whose file is not on the device stands with keeping
/// offline ([`crate::CoreError::NotOnDevice`]): what a shell tells the user
/// to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Keeping {
    /// The sticker is not kept offline: keep it while the server is
    /// reachable.
    NotKept,
    /// The sticker is kept and so is this file, which is not downloaded
    /// yet (or its download failed): run the downloader while the server
    /// is reachable.
    NotYet,
    /// The sticker is kept, but this file is not among what it keeps: a
    /// chat app that is not among the device's targets (`offline.targets`,
    /// or the default target and the copy history), or the sticker
    /// rendition with `offline.sticker_renditions` off.
    OtherFile,
    /// The sticker is kept with a pack, the library or an automatic rule,
    /// and the per-kind options or `offline.max_file_mb` leave this file
    /// out ([`skip_reason`]).
    LeftOut,
}

/// A file a kept item (or pack) needs, where the copy path looks for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WantedFile {
    /// Path relative to the cache directory: `files/thumb/<media>.webp`,
    /// `clip/<media>.<variant>/<name>.<ext>`, `files/cover/<...>.webp`.
    pub rel: String,
    /// The item it belongs to (for a pack's own cover image: the set).
    pub item_id: String,
    /// The rendition (`thumb` for thumbnails and covers).
    pub variant: Variant,
    /// Expected size in bytes from the item's metadata (0: unknown).
    pub size: u64,
    /// What it is for; several when one file serves several targets.
    pub roles: Vec<FileRole>,
    /// For a pack's own cover image: the URL path to fetch it from.
    /// `None`: the item's file route for `variant`.
    pub url: Option<String>,
}

impl WantedFile {
    fn thumbnail(item: &ItemSummary, role: FileRole) -> WantedFile {
        WantedFile {
            rel: thumb_rel(item),
            item_id: item.id.clone(),
            variant: Variant::Thumb,
            size: item
                .variants
                .iter()
                .find(|v| v.variant == Variant::Thumb)
                .map_or(0, |v| v.size_bytes),
            roles: vec![role],
            url: None,
        }
    }

    fn rendition(item: &ItemSummary, info: &VariantInfo, role: FileRole) -> WantedFile {
        WantedFile {
            rel: clip_rel(item, info),
            item_id: item.id.clone(),
            variant: info.variant,
            size: info.size_bytes,
            roles: vec![role],
            url: None,
        }
    }

    /// The URL path the downloader fetches.
    pub fn url_path(&self) -> String {
        match &self.url {
            Some(url) => url.clone(),
            None => path(routes::ITEM_FILE, &[&self.item_id, self.variant.as_str()]),
        }
    }
}

/// The cache path of an item's thumbnail (as [`crate::Session::thumbnail`]).
pub fn thumb_rel(item: &ItemSummary) -> String {
    format!("files/thumb/{}.webp", safe_component(&item.media_id))
}

/// The cache path of a pack's own cover image (as
/// [`crate::Session::pack_cover`]); `None` for a URL that is not one.
pub fn cover_rel(set_id: &str, cover_url: &str) -> Option<String> {
    let prefix = format!("{}/", path(routes::SET_COVER, &[set_id]));
    cover_url
        .starts_with(&prefix)
        .then(|| format!("files/cover/{}.webp", safe_component(cover_url)))
}

/// How the grid plays a moving item ([`played_rendition`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Playback {
    /// As an animated picture (GIF or WebP; transparency kept).
    Picture,
    /// In a media player, without sound (MP4 or WebM).
    Video,
}

/// The rendition a grid plays in place of a moving item's still thumbnail,
/// and how; `None` for a still, or a moving item without a playable
/// rendition. The Android app's rule (moved here from the Android crate's
/// `motion::choose`, unchanged):
///
/// - An animation plays as a picture (`ImageDecoder` animates a GIF or an
///   animated WebP, transparency kept): the original when it is a GIF or a
///   WebP, or the Discord GIF, whichever is smaller. The MP4 (flattened on
///   white) only when there is neither.
/// - A video plays in a media player, without sound: the MP4 the server made
///   when there is one (the original is a WebM, or not plain H.264), else
///   the original, when it is an MP4 or a WebM.
pub fn played_rendition(item: &ItemSummary) -> Option<(&VariantInfo, Playback)> {
    let find = |v: Variant| item.variants.iter().find(|i| i.variant == v);
    let picture = |i: &&VariantInfo| matches!(i.mime.as_str(), "image/gif" | "image/webp");
    let video = |i: &&VariantInfo| matches!(i.mime.as_str(), "video/mp4" | "video/webm");
    match item.kind {
        MediaKind::Still => None,
        MediaKind::Animation => [find(Variant::Original), find(Variant::DiscordGif)]
            .into_iter()
            .flatten()
            .filter(picture)
            .min_by_key(|i| i.size_bytes)
            .map(|i| (i, Playback::Picture))
            .or_else(|| find(Variant::Mp4).filter(video).map(|i| (i, Playback::Video))),
        MediaKind::Video => find(Variant::Mp4)
            .or_else(|| find(Variant::Original))
            .filter(video)
            .map(|i| (i, Playback::Video)),
    }
}

/// The files a kept item needs on `device`, thumbnail first: its
/// thumbnail; for each target, the rendition a copy chooses
/// ([`pick_variant`], as [`crate::Session::prepare_copy`] does), and with
/// [`OfflineOptions::sticker_renditions`] also the one it chooses with the
/// sticker flag; on a device that inserts into text fields, for each
/// target and each of [`INSERT_FIELDS`] the rendition an insert picks
/// ([`crate::variant_for`], with and without the sticker flag likewise);
/// on a device that plays animations, the [`played_rendition`]. One entry
/// per file (a file several targets use lists every role). The per-kind
/// options and `max_file_mb` are not applied here: they decide what pack,
/// library and automatic keeping download ([`skip_reason`]); a sticker
/// kept by itself gets all of these.
pub fn needs(item: &ItemSummary, opts: &OfflineOptions, device: &Device) -> Vec<WantedFile> {
    let mut out: Vec<WantedFile> = vec![WantedFile::thumbnail(item, FileRole::Thumbnail)];
    let mut add = |file: WantedFile| match out.iter_mut().find(|f| f.rel == file.rel) {
        Some(existing) => {
            for role in file.roles {
                if !existing.roles.contains(&role) {
                    existing.roles.push(role);
                }
            }
        }
        None => out.push(file),
    };
    let flags: &[bool] = if opts.sticker_renditions {
        &[false, true]
    } else {
        &[false]
    };
    for &target in &device.targets {
        for &sticker in flags {
            if let Ok((info, is_sticker, _)) = pick_variant(item, target, sticker) {
                // With the sticker flag on a target without stickers the
                // copy is the plain one, listed already.
                if sticker && !is_sticker {
                    continue;
                }
                add(WantedFile::rendition(
                    item,
                    &info,
                    FileRole::Copy {
                        target,
                        sticker: is_sticker,
                    },
                ));
            }
        }
    }
    if device.inserts {
        let fields: Vec<Vec<String>> = INSERT_FIELDS
            .iter()
            .map(|f| f.iter().map(|m| m.to_string()).collect())
            .collect();
        for &target in &device.targets {
            for field in &fields {
                for &sticker in flags {
                    if let Some(choice) = crate::insert::variant_for(item, target, field, sticker) {
                        add(WantedFile::rendition(
                            item,
                            &choice.variant,
                            FileRole::Insert {
                                target,
                                sticker: choice.sticker,
                            },
                        ));
                    }
                }
            }
        }
    }
    if device.plays_motion {
        if let Some((info, _)) = played_rendition(item) {
            add(WantedFile::rendition(item, info, FileRole::Played));
        }
    }
    out
}

/// Why a file or an item is not downloaded. Reported as skipped, not as
/// failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum SkipReason {
    /// Its kind is switched off (`offline.stills`, `.animations`,
    /// `.videos`) and it is kept only with a pack, the library or an
    /// automatic rule.
    KindOff { kind: MediaKind },
    /// The rendition is larger than `offline.max_file_mb`.
    TooLarge { size: u64, limit: u64 },
    /// Kept by itself, but not in the cache (not synced yet, or not one the
    /// cache lists): nothing is known to download.
    NotCached,
    /// The item has no rendition a copy could use.
    NoRendition,
}

impl std::fmt::Display for SkipReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SkipReason::KindOff { kind } => {
                let name = match kind {
                    MediaKind::Still => "stills",
                    MediaKind::Animation => "animations",
                    MediaKind::Video => "videos",
                };
                write!(f, "{name} are not kept with packs, the library or automatic rules")
            }
            SkipReason::TooLarge { size, limit } => write!(
                f,
                "{:.1} MiB is over the limit of {} MiB per file",
                *size as f64 / MIB as f64,
                limit / MIB
            ),
            SkipReason::NotCached => f.write_str("not in the offline cache yet"),
            SkipReason::NoRendition => f.write_str("no rendition to keep"),
        }
    }
}

/// Whether the per-kind options keep an item of `kind`.
pub fn kind_kept(opts: &OfflineOptions, kind: MediaKind) -> bool {
    match kind {
        MediaKind::Still => opts.stills,
        MediaKind::Animation => opts.animations,
        MediaKind::Video => opts.videos,
    }
}

/// Why `file` of `item` is not downloaded under `opts`, if it is not. A
/// sticker kept by itself (`explicit`) is always downloaded; thumbnails and
/// covers always are (the grids need them, and they are small).
pub fn skip_reason(item: &ItemSummary, file: &WantedFile, opts: &OfflineOptions, explicit: bool) -> Option<SkipReason> {
    if explicit
        || file
            .roles
            .iter()
            .all(|r| matches!(r, FileRole::Thumbnail | FileRole::Cover))
    {
        return None;
    }
    if !kind_kept(opts, item.kind) {
        return Some(SkipReason::KindOff { kind: item.kind });
    }
    let limit = opts.max_file_mb.saturating_mul(MIB);
    if limit > 0 && file.size > limit {
        return Some(SkipReason::TooLarge { size: file.size, limit });
    }
    None
}

// -- the plan ------------------------------------------------------------------

/// Byte and file counts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Totals {
    /// Files wanted.
    pub files: u64,
    /// Their bytes (as the item metadata says; unknown sizes count 0).
    pub bytes: u64,
    /// Of those, on the device.
    pub present_files: u64,
    pub present_bytes: u64,
}

impl Totals {
    fn add(&mut self, size: u64, present: bool) {
        self.files += 1;
        self.bytes += size;
        if present {
            self.present_files += 1;
            self.present_bytes += size;
        }
    }

    /// Files still to download.
    pub fn missing_files(&self) -> u64 {
        self.files - self.present_files
    }

    /// Bytes still to download.
    pub fn missing_bytes(&self) -> u64 {
        self.bytes - self.present_bytes
    }
}

/// One file of a [`Plan`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedFile {
    pub file: WantedFile,
    /// On the device (registered, of the expected size).
    pub present: bool,
    pub group: Group,
    /// Indexes into [`Plan::scopes`] of the scopes that want it.
    pub scopes: Vec<usize>,
}

/// An item, or one of its files, that is not downloaded, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Skipped {
    pub item_id: String,
    /// The item's name ("" for an item not in the cache).
    pub name: String,
    /// The rendition skipped; `None`: the whole item.
    pub variant: Option<Variant>,
    pub reason: SkipReason,
}

/// Totals of one scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopeTotals {
    pub scope: PlanScope,
    /// The kept sticker's or pack's name, from the cache (`None` for the
    /// library, the automatic rules, and what the cache does not hold).
    pub name: Option<String>,
    /// Items it keeps (a pack's or the library's: those not skipped).
    pub items: u64,
    pub totals: Totals,
}

/// A download that failed, with its reason (`offline_failures`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OfflineFailure {
    /// Cache-relative path.
    pub path: String,
    pub item_id: String,
    /// What went wrong (the server's message, a size or checksum mismatch).
    pub error: String,
    /// Attempts so far, over all runs.
    pub attempts: u32,
    /// When it last failed (unix seconds).
    pub at: i64,
}

/// What is kept and what that means in files, derived from the cache:
/// (kept scopes and automatic rules) x (cached items) x [`needs`]. Never
/// stored; [`crate::Session::offline_plan`] derives it again each time.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    /// Every file wanted, in download order, each once.
    pub files: Vec<PlannedFile>,
    /// Items and renditions not downloaded, with the reason.
    pub skipped: Vec<Skipped>,
    /// Over all files.
    pub totals: Totals,
    /// Per kept scope and automatic rule.
    pub scopes: Vec<ScopeTotals>,
    /// Items kept for at least one reason (skipped ones included).
    pub items: u64,
    /// `offline.max_mb` in bytes; `None`: no limit.
    pub budget: Option<u64>,
    /// Wanted files whose download failed, with the reason.
    pub failures: Vec<OfflineFailure>,
}

impl Plan {
    /// Whether everything wanted is on the device.
    pub fn complete(&self) -> bool {
        self.totals.files == self.totals.present_files
    }

    /// The files still to download, in order.
    pub fn missing(&self) -> impl Iterator<Item = &PlannedFile> {
        self.files.iter().filter(|f| !f.present)
    }

    /// Whether the files wanted exceed the budget.
    pub fn over_budget(&self) -> bool {
        self.budget.is_some_and(|b| self.totals.bytes > b)
    }
}

/// A pack's cover as the server lists it (cached in `meta` as
/// `offline_covers`, refreshed by [`crate::Session::offline_run`]).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CoverInfo {
    /// `SetSummary::cover_url`.
    #[serde(default)]
    pub url: Option<String>,
    /// `SetSummary::cover_item_id`.
    #[serde(default)]
    pub item: Option<String>,
}

/// Everything a plan is derived from.
pub(crate) struct PlanInput<'a> {
    pub items: &'a [ItemSummary],
    pub keeps: &'a [Scope],
    pub opts: &'a OfflineOptions,
    pub device: &'a Device,
    /// The account's username (own packs).
    pub me: Option<&'a str>,
    /// Most recently copied item ids, newest first (`auto.used`, already
    /// cut to `used_count`).
    pub used: &'a [String],
    /// Liked item ids (`auto.liked`).
    pub liked: &'a HashSet<String>,
    /// Copies per item on this device (order within a group).
    pub counts: &'a HashMap<String, u64>,
    pub covers: &'a BTreeMap<String, CoverInfo>,
    /// Whether a file is on the device: (rel, expected size).
    pub present: &'a dyn Fn(&str, u64) -> bool,
}

/// The reasons each cached item is kept for, by item id.
pub(crate) fn reasons(input: &PlanInput<'_>) -> HashMap<String, Vec<Reason>> {
    let mut out: HashMap<String, Vec<Reason>> = HashMap::new();
    let mut add = |id: &str, r: Reason| {
        let v = out.entry(id.to_string()).or_default();
        if !v.contains(&r) {
            v.push(r);
        }
    };
    let library = input.keeps.contains(&Scope::Library);
    let sets: HashSet<&str> = input
        .keeps
        .iter()
        .filter_map(|s| match s {
            Scope::Set(id) => Some(id.as_str()),
            _ => None,
        })
        .collect();
    for s in input.keeps {
        if let Scope::Item(id) = s {
            add(id, Reason::Item);
        }
    }
    let auto = &input.opts.auto;
    if auto.used {
        for id in input.used {
            add(id, Reason::Used);
        }
    }
    if auto.liked {
        for id in input.liked {
            add(id, Reason::Liked);
        }
    }
    for item in input.items {
        if sets.contains(item.set_id.as_str()) {
            add(&item.id, Reason::Set);
        }
        if library {
            add(&item.id, Reason::Library);
        }
        if auto.own_packs && input.me.is_some_and(|me| me == item.owner_username) {
            add(&item.id, Reason::OwnPack);
        }
    }
    for v in out.values_mut() {
        v.sort();
    }
    out
}

/// The scopes progress is reported for, in display order.
fn plan_scopes(input: &PlanInput<'_>) -> Vec<PlanScope> {
    let mut out = Vec::new();
    if input.opts.auto.thumbnails {
        out.push(PlanScope::Thumbnails);
    }
    out.extend(input.keeps.iter().cloned().map(PlanScope::Kept));
    if input.opts.auto.used {
        out.push(PlanScope::Used);
    }
    if input.opts.auto.liked {
        out.push(PlanScope::Liked);
    }
    if input.opts.auto.own_packs {
        out.push(PlanScope::OwnPacks);
    }
    out
}

/// Whether an item kept for `reasons` belongs to `scope`.
fn in_scope(scope: &PlanScope, item: &ItemSummary, reasons: &[Reason]) -> bool {
    match scope {
        PlanScope::Thumbnails => false,
        PlanScope::Kept(Scope::Item(id)) => *id == item.id,
        PlanScope::Kept(Scope::Set(id)) => *id == item.set_id,
        PlanScope::Kept(Scope::Library) => true,
        PlanScope::Used => reasons.contains(&Reason::Used),
        PlanScope::Liked => reasons.contains(&Reason::Liked),
        PlanScope::OwnPacks => reasons.contains(&Reason::OwnPack),
    }
}

/// Collects a plan's files, each once.
struct Builder<'a> {
    plan: Plan,
    index: HashMap<String, usize>,
    present: &'a dyn Fn(&str, u64) -> bool,
}

impl Builder<'_> {
    /// Add `file` (or the roles and scopes it adds to the same file).
    fn push(&mut self, file: WantedFile, group: Group, scope_ix: &[usize]) {
        let ix = match self.index.get(&file.rel) {
            Some(&ix) => {
                let existing = &mut self.plan.files[ix].file;
                for role in &file.roles {
                    if !existing.roles.contains(role) {
                        existing.roles.push(*role);
                    }
                }
                ix
            }
            None => {
                let present = (self.present)(&file.rel, file.size);
                self.index.insert(file.rel.clone(), self.plan.files.len());
                self.plan.files.push(PlannedFile {
                    file,
                    present,
                    group,
                    scopes: Vec::new(),
                });
                self.plan.files.len() - 1
            }
        };
        for &s in scope_ix {
            if !self.plan.files[ix].scopes.contains(&s) {
                self.plan.files[ix].scopes.push(s);
            }
        }
    }
}

/// Derive the plan (see [`Plan`]).
pub(crate) fn derive(input: &PlanInput<'_>) -> Plan {
    let reasons = reasons(input);
    let scopes = plan_scopes(input);
    let by_id: HashMap<&str, &ItemSummary> = input.items.iter().map(|i| (i.id.as_str(), i)).collect();
    let mut b = Builder {
        plan: Plan {
            budget: (input.opts.max_mb > 0).then(|| input.opts.max_mb.saturating_mul(MIB)),
            scopes: scopes
                .iter()
                .map(|s| ScopeTotals {
                    scope: s.clone(),
                    name: match s {
                        PlanScope::Kept(Scope::Item(id)) => by_id.get(id.as_str()).map(|i| i.name.clone()),
                        PlanScope::Kept(Scope::Set(id)) => {
                            input.items.iter().find(|i| i.set_id == *id).map(|i| i.set_name.clone())
                        }
                        _ => None,
                    },
                    items: 0,
                    totals: Totals::default(),
                })
                .collect(),
            ..Plan::default()
        },
        index: HashMap::new(),
        present: input.present,
    };

    // Most used first: copies on this device, then over all users, then name.
    let rank = |a: &ItemSummary, b: &ItemSummary| {
        let count = |i: &ItemSummary| input.counts.get(&i.id).copied().unwrap_or(0);
        count(b)
            .cmp(&count(a))
            .then(b.copies.cmp(&a.copies))
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.id.cmp(&b.id))
    };

    // 1. Thumbnails of every item.
    if input.opts.auto.thumbnails {
        let mut all: Vec<&ItemSummary> = input.items.iter().collect();
        all.sort_by(|a, b| rank(a, b));
        for item in all {
            b.push(
                WantedFile::thumbnail(item, FileRole::Thumbnail),
                Group::Thumbnails,
                &[0],
            );
        }
    }

    // Items kept by themselves but not cached: nothing known to download.
    for s in input.keeps {
        if let Scope::Item(id) = s {
            if !by_id.contains_key(id.as_str()) {
                b.plan.skipped.push(Skipped {
                    item_id: id.clone(),
                    name: String::new(),
                    variant: None,
                    reason: SkipReason::NotCached,
                });
            }
        }
    }

    // The packs kept explicitly or as own packs: their covers go before
    // their items.
    let mut kept_sets: Vec<&str> = input
        .keeps
        .iter()
        .filter_map(|s| match s {
            Scope::Set(id) => Some(id.as_str()),
            _ => None,
        })
        .collect();
    if input.opts.auto.own_packs {
        if let Some(me) = input.me {
            let mut own: Vec<&str> = input
                .items
                .iter()
                .filter(|i| i.owner_username == me)
                .map(|i| i.set_id.as_str())
                .collect();
            own.sort();
            own.dedup();
            kept_sets.extend(own);
        }
    }
    let cover_files = |b: &mut Builder<'_>, set_id: &str| {
        let scope_ix: Vec<usize> = scopes
            .iter()
            .enumerate()
            .filter(|(_, s)| match s {
                PlanScope::Kept(Scope::Set(id)) => id == set_id,
                PlanScope::OwnPacks => input
                    .items
                    .iter()
                    .any(|i| i.set_id == set_id && input.me == Some(i.owner_username.as_str())),
                _ => false,
            })
            .map(|(i, _)| i)
            .collect();
        let cover = input.covers.get(set_id);
        if let Some(url) = cover.and_then(|c| c.url.as_deref()) {
            if let Some(rel) = cover_rel(set_id, url) {
                let file = WantedFile {
                    rel,
                    item_id: set_id.to_string(),
                    variant: Variant::Thumb,
                    size: 0,
                    roles: vec![FileRole::Cover],
                    url: Some(url.to_string()),
                };
                b.push(file, Group::Packs, &scope_ix);
            }
        }
        // The cover item's thumbnail: the one the server names, and the
        // first item by name, which the offline pack lists show.
        let mut members: Vec<&ItemSummary> = input.items.iter().filter(|i| i.set_id == set_id).collect();
        members.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
        let named = cover
            .and_then(|c| c.item.as_deref())
            .and_then(|id| by_id.get(id).copied());
        for item in named.into_iter().chain(members.first().copied()) {
            b.push(WantedFile::thumbnail(item, FileRole::Cover), Group::Packs, &scope_ix);
        }
    };

    // 2. Kept items by group, most used first within it.
    let mut kept: Vec<(Group, &ItemSummary, &Vec<Reason>)> = reasons
        .iter()
        .filter_map(|(id, rs)| {
            let item = *by_id.get(id.as_str())?;
            let group = rs.iter().map(|r| r.group()).min()?;
            Some((group, item, rs))
        })
        .collect();
    kept.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| rank(a.1, b.1)));
    b.plan.items = kept.len() as u64;

    let mut covers_done: HashSet<&str> = HashSet::new();
    for (group, item, rs) in kept {
        if group == Group::Packs
            && kept_sets.contains(&item.set_id.as_str())
            && covers_done.insert(item.set_id.as_str())
        {
            cover_files(&mut b, &item.set_id);
        }
        let explicit = rs.contains(&Reason::Item);
        let scope_ix: Vec<usize> = scopes
            .iter()
            .enumerate()
            .filter(|(_, s)| in_scope(s, item, rs))
            .map(|(i, _)| i)
            .collect();
        let files = needs(item, input.opts, input.device);
        if files.iter().all(|f| f.roles == [FileRole::Thumbnail]) {
            // Only a thumbnail: no rendition a copy could use.
            b.plan.skipped.push(Skipped {
                item_id: item.id.clone(),
                name: item.name.clone(),
                variant: None,
                reason: SkipReason::NoRendition,
            });
        }
        let mut item_skipped = false;
        for file in files {
            match skip_reason(item, &file, input.opts, explicit) {
                None => b.push(file, group, &scope_ix),
                Some(reason @ SkipReason::KindOff { .. }) => {
                    if !item_skipped {
                        item_skipped = true;
                        b.plan.skipped.push(Skipped {
                            item_id: item.id.clone(),
                            name: item.name.clone(),
                            variant: None,
                            reason,
                        });
                    }
                }
                Some(reason) => b.plan.skipped.push(Skipped {
                    item_id: item.id.clone(),
                    name: item.name.clone(),
                    variant: Some(file.variant),
                    reason,
                }),
            }
        }
        if !item_skipped {
            for &s in &scope_ix {
                b.plan.scopes[s].items += 1;
            }
        }
    }
    // Kept packs none of whose items were placed yet still show their cover.
    for set_id in kept_sets {
        if covers_done.insert(set_id) {
            cover_files(&mut b, set_id);
        }
    }

    let mut plan = b.plan;
    let mut totals = Totals::default();
    for f in &plan.files {
        totals.add(f.file.size, f.present);
        for &s in &f.scopes {
            plan.scopes[s].totals.add(f.file.size, f.present);
        }
    }
    plan.totals = totals;
    plan
}

/// The media component of a cache path (`files/thumb/<m>.webp`,
/// `clip/<m>.<variant>.../<name>`); `None` for other paths (covers).
pub(crate) fn media_of(rel: &str) -> Option<&str> {
    if let Some(rest) = rel.strip_prefix("files/thumb/") {
        return rest.strip_suffix(".webp");
    }
    let rest = rel.strip_prefix("clip/")?;
    let dir = rest.split('/').next()?;
    dir.split('.').next()
}

/// The kept files to delete because their item is gone: a kept thumbnail or
/// rendition whose media no cached item has any more (a sync removed the
/// item, or its picture was replaced). `media` is the `media_id` of every
/// cached item, those this version cannot read included
/// ([`crate::cache::Cache::item_media`]).
pub(crate) fn orphaned<'a>(kept: impl Iterator<Item = &'a String>, media: &HashSet<String>) -> HashSet<String> {
    let media: HashSet<String> = media.iter().map(|m| safe_component(m)).collect();
    kept.filter(|rel| media_of(rel).is_some_and(|m| !media.contains(m)))
        .cloned()
        .collect()
}
