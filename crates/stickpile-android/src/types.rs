//! The records and enums of the interface, each converted from its
//! `stickpile-api` counterpart (no UniFFI derive goes on those).

use stickpile_api::{ItemSummary, Role, SetSummary, TargetApp, User};
use stickpile_core::offline;

/// Where the library keeps its files: absolute paths inside the app's
/// private storage.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct LibraryDirs {
    /// The client config (`config.toml`, the desktop's format). Need not
    /// exist: missing means the defaults. `StickpileLibrary.set_server_url`
    /// writes it.
    pub config_file: String,
    /// The offline cache: the item database, thumbnails, prepared files.
    pub cache_dir: String,
    /// The API token, one line, mode 0600. Need not exist: signed out.
    pub token_file: String,
}

/// How requests are authenticated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum AuthKind {
    /// By the stored API token.
    Token,
    /// Without a token: the server knows this device by its tailnet
    /// identity (behind `tailscale serve`).
    Tailnet,
}

/// The account the library is signed in to.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct Account {
    pub username: String,
    /// `None` when the account has none of its own (it equals the username).
    pub display_name: Option<String>,
    /// The server the account is on.
    pub server_url: String,
    pub auth: AuthKind,
    pub is_admin: bool,
}

impl Account {
    pub(crate) fn new(user: &User, server_url: &str, auth: AuthKind) -> Account {
        let display = user.display_name.trim();
        Account {
            username: user.username.clone(),
            display_name: (!display.is_empty() && display != user.username).then(|| display.to_string()),
            server_url: server_url.to_string(),
            auth,
            is_admin: user.role == Role::Admin,
        }
    }
}

/// The chat app a picture goes to; it decides which rendition fits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum Target {
    Telegram,
    Discord,
    Signal,
    Whatsapp,
    Slack,
    /// Anything else.
    Generic,
}

impl From<TargetApp> for Target {
    fn from(t: TargetApp) -> Self {
        match t {
            TargetApp::Telegram => Target::Telegram,
            TargetApp::Discord => Target::Discord,
            TargetApp::Signal => Target::Signal,
            TargetApp::Whatsapp => Target::Whatsapp,
            TargetApp::Slack => Target::Slack,
            TargetApp::Generic => Target::Generic,
        }
    }
}

impl From<Target> for TargetApp {
    fn from(t: Target) -> Self {
        match t {
            Target::Telegram => TargetApp::Telegram,
            Target::Discord => TargetApp::Discord,
            Target::Signal => TargetApp::Signal,
            Target::Whatsapp => TargetApp::Whatsapp,
            Target::Slack => TargetApp::Slack,
            Target::Generic => TargetApp::Generic,
        }
    }
}

/// What an item is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum MediaKind {
    /// A picture: PNG, JPEG, a WebP or GIF of one frame.
    Still,
    /// An animated GIF or WebP.
    Animation,
    /// An MP4 or WebM.
    Video,
}

impl From<stickpile_api::MediaKind> for MediaKind {
    fn from(k: stickpile_api::MediaKind) -> Self {
        match k {
            stickpile_api::MediaKind::Still => MediaKind::Still,
            stickpile_api::MediaKind::Animation => MediaKind::Animation,
            stickpile_api::MediaKind::Video => MediaKind::Video,
        }
    }
}

/// The file format of an item's original.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum MediaFormat {
    Png,
    Jpeg,
    Webp,
    Gif,
    Mp4,
    Webm,
}

impl From<stickpile_api::MediaFormat> for MediaFormat {
    fn from(f: stickpile_api::MediaFormat) -> Self {
        match f {
            stickpile_api::MediaFormat::Png => MediaFormat::Png,
            stickpile_api::MediaFormat::Jpeg => MediaFormat::Jpeg,
            stickpile_api::MediaFormat::Webp => MediaFormat::Webp,
            stickpile_api::MediaFormat::Gif => MediaFormat::Gif,
            stickpile_api::MediaFormat::Mp4 => MediaFormat::Mp4,
            stickpile_api::MediaFormat::Webm => MediaFormat::Webm,
        }
    }
}

/// A sticker or meme.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct Item {
    pub id: String,
    pub name: String,
    /// The pack (set) it is in.
    pub set_id: String,
    pub set_name: String,
    /// The pack owner's username.
    pub owner: String,
    pub emojis: Vec<String>,
    pub keywords: Vec<String>,
    /// One label such as `happy` or `sarcastic`, if it has one.
    pub vibe: Option<String>,
    pub tags: Vec<String>,
    pub kind: MediaKind,
    /// The original's format.
    pub format: MediaFormat,
    /// Of the original, in pixels (0 when unknown).
    pub width: u32,
    pub height: u32,
    /// The server's quality score, 0 to 100, when it has one.
    pub quality: Option<u32>,
    /// Likes by all users.
    pub likes: u64,
    /// Times it was sent, by all users.
    pub copies: u64,
    /// Tagged `nsfw` (in any case): what safe mode hides.
    pub nsfw: bool,
    /// The signed-in account liked it, as far as this device knows: the
    /// favorites last read (`StickpileLibrary.favorites`) and the likes
    /// changed here since. False until the favorites were read once.
    pub liked: bool,
    /// The words written in the picture, line breaks kept (`\n`); read by
    /// the server (OCR) or written by a person. Empty when it has none,
    /// and from a server before the text field (0.7.0).
    #[uniffi(default)]
    pub text: String,
}

impl Item {
    /// The item with `liked` set from the account's known favorites.
    pub(crate) fn new(i: &ItemSummary, liked: bool) -> Self {
        Item {
            id: i.id.clone(),
            name: i.name.clone(),
            set_id: i.set_id.clone(),
            set_name: i.set_name.clone(),
            owner: i.owner_username.clone(),
            emojis: i.emojis.clone(),
            keywords: i.keywords.clone(),
            vibe: i.vibe.clone(),
            tags: i.tags.clone(),
            kind: i.kind.into(),
            format: i.format.into(),
            width: i.width,
            height: i.height,
            quality: i.quality.map(u32::from),
            likes: i.likes,
            copies: i.copies,
            nsfw: stickpile_quick::model::is_nsfw(i),
            liked,
            text: i.text.clone(),
        }
    }
}

/// A list of items.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ItemList {
    pub items: Vec<Item>,
    /// The server could not be reached and the offline cache answered.
    pub offline: bool,
}

/// What `StickpileLibrary.search_steps` answers: the results to show now,
/// and the server's to come when these are the offline cache's because the
/// server had not answered yet (docs/offline.md, "Results without
/// waiting").
#[derive(Debug, Clone, uniffi::Record)]
pub struct SearchAnswer {
    /// The results to show now: the server's if it answered in time, else
    /// the offline cache's (`offline = true`).
    pub list: ItemList,
    /// The server's answer to wait for, which replaces `list`; `None` when
    /// nothing is to come (the server answered, or it is believed
    /// unreachable and was not asked).
    pub pending: Option<std::sync::Arc<crate::reach::PendingSearch>>,
}

/// What the library knows of the server (`stickpile_core::Reach`): a
/// request or a background check found it there or not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ServerReach {
    /// Nothing asked yet, or an outage forgotten because the user asked
    /// for something (a sync, a share): the next request tries the server.
    Unknown,
    /// The server answered the last request or check.
    Reachable,
    /// A request found no server and nothing answered since: reads answer
    /// from the offline cache at once, and the server is checked in the
    /// background.
    Unreachable {
        /// When the outage was noticed (its first failure).
        since: std::time::SystemTime,
    },
}

impl From<stickpile_core::Reach> for ServerReach {
    fn from(r: stickpile_core::Reach) -> Self {
        match r {
            stickpile_core::Reach::Unknown => ServerReach::Unknown,
            stickpile_core::Reach::Reachable => ServerReach::Reachable,
            stickpile_core::Reach::Unreachable { since } => ServerReach::Unreachable { since },
        }
    }
}

/// A pack (a set of items, as the server calls it).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct Pack {
    pub id: String,
    pub slug: String,
    pub name: String,
    /// The owner's username.
    pub owner: String,
    pub item_count: u32,
    /// The signed-in account owns it.
    pub mine: bool,
}

impl Pack {
    pub(crate) fn new(s: &SetSummary, me: Option<&str>) -> Pack {
        Pack {
            id: s.id.clone(),
            slug: s.slug.clone(),
            name: s.name.clone(),
            owner: s.owner_username.clone(),
            item_count: u32::try_from(s.item_count).unwrap_or(u32::MAX),
            mine: me == Some(s.owner_username.as_str()),
        }
    }
}

/// A list of packs.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PackList {
    pub packs: Vec<Pack>,
    /// The server could not be reached and the offline cache answered.
    pub offline: bool,
}

/// The values offered under the search field while a `key:value` filter is
/// typed (`vibe:sar`).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct Suggestions {
    /// The filter as typed, with its colon and a leading `-`: `vibe:`, `-tag:`.
    pub key: String,
    /// Best first. Accepting one replaces the typed value with it.
    pub words: Vec<String>,
    /// The index in `words` of the value the search uses now (draw it
    /// highlighted); `None` while no value is typed, and for an emoji name
    /// (`e:sk`, searched as typed).
    pub chosen: Option<u32>,
    /// Parallel to `words`: for `e:` the emoji's English name (`skull` for
    /// 💀), else empty.
    #[uniffi(default)]
    pub labels: Vec<String>,
}

impl From<stickpile_quick::suggest::Suggestions> for Suggestions {
    fn from(s: stickpile_quick::suggest::Suggestions) -> Self {
        let chosen = s.chosen().and_then(|i| u32::try_from(i).ok());
        Suggestions {
            key: s.key,
            words: s.words,
            chosen,
            labels: s.labels,
        }
    }
}

/// What a search hint offers (`StickpileLibrary.hints`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum HintKind {
    /// A value of the filter being typed (`sarcastic` for `vibe:sar`, an
    /// emoji for `e:`).
    Value,
    /// A whole filter for the word being typed (`tag:reaction` for `rea`).
    Filter,
    /// A filter key to type a value after (`tag:`).
    Key,
}

/// One offer under the search field while a query is typed.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SearchHint {
    /// What to show: `sarcastic`, `tag:reaction`, `vibe:`; an emoji with
    /// its English name, `💀 skull` (the query gets the emoji alone). Of
    /// the [`HintKind::Value`] labels only these have a space (the other
    /// values are one word), so the apps draw the name apart from the
    /// emoji at the first one.
    pub label: String,
    /// The whole query once it is taken: the field's new text. Values and
    /// filters end with a space (the next word starts), keys with their
    /// colon (their values are offered next).
    pub query: String,
    /// The search uses this value now (a half-typed or misspelled value
    /// searches for its best match): draw it highlighted.
    pub chosen: bool,
    pub kind: HintKind,
}

/// How a moving item's rendition plays (`StickpileLibrary.motion`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum MotionKind {
    /// An animated GIF or WebP: `ImageDecoder` animates it (API 28 and later).
    Picture,
    /// An MP4 or WebM, played in a media player without sound.
    Video,
}

/// A rendition of a moving item to play in place of its still thumbnail.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct Motion {
    /// The file, in the cache: absolute.
    pub path: String,
    pub kind: MotionKind,
    /// Of the rendition, in pixels (0 when unknown).
    pub width: u32,
    pub height: u32,
}

/// What a pairing link (the QR code) says.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PairingLink {
    /// The server's URL, `http://` or `https://`, without a trailing `/`.
    pub server_url: String,
    /// The code in its display form (`7KQ2-M9XA-4FTR`).
    pub code: String,
}

/// A rendition of an item as a file, ready to be inserted.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct Prepared {
    /// The file, in the cache: absolute.
    pub path: String,
    /// The MIME type to commit it as; always one the field accepts.
    pub mime_type: String,
    /// The file's name, `<item name>.<extension>` (`grumpy cat.png`).
    pub display_name: String,
    /// The chat app the editor's package was taken for.
    pub target: Target,
    /// The rendition's name (`original`, `png`, `tg_sticker`,
    /// `tg_video_sticker`, `discord_gif`, `mp4`); give it back to
    /// `StickpileLibrary.record_use`.
    pub variant: String,
    /// It is the sticker rendition (asked for, and the app has stickers).
    pub sticker: bool,
}

/// One file to upload.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct UploadFile {
    /// The file: absolute. Its name (with the extension) is the upload's
    /// file name.
    pub path: String,
    /// The item's name; `None`: the server names it after the file.
    pub name: Option<String>,
    pub keywords: Vec<String>,
    pub emojis: Vec<String>,
    /// The words in the picture, as the person typed them (line breaks
    /// kept): the user's text. `None` or blank: not sent, so the server
    /// reads them itself when it can (OCR). At most 2000 characters.
    #[uniffi(default)]
    pub text: Option<String>,
}

/// Where the background sync stands.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SyncState {
    /// When the last sync went through, unix seconds; `None`: not since the
    /// account or server changed. After a restart of the app this is the
    /// server's time of that sync (the cache's sync cursor).
    pub last_sync: Option<i64>,
    /// Why the last attempt failed; `None` when it went through (or none
    /// was made since the library was opened).
    pub last_error: Option<crate::LibraryErrorKind>,
    /// Uses recorded on this device and not yet sent to the server.
    pub pending_usage: u32,
    /// A sync is due: `StickpileLibrary.sync_if_due` would sync now.
    pub due: bool,
    /// The sync this call ran changed the offline cache (items new,
    /// changed or removed): what `hints` and `suggestions` offer may be
    /// different now, so offers on screen are asked for again. False from
    /// `sync_state`, and when no sync was due.
    pub cache_changed: bool,
    /// This call ran a sync, and it went through: the offline downloads
    /// follow it. False from `sync_state`, and when no sync was due.
    #[uniffi(default)]
    pub synced: bool,
}

// -- keeping stickers offline (docs/offline.md) ------------------------------

/// What can be kept on this phone (`StickpileLibrary.keep_offline`). The
/// choice is stored in the offline cache, on this phone only.
#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum OfflineScope {
    /// One sticker: always downloaded whole, whatever the per-kind options
    /// say.
    Sticker { item_id: String },
    /// A pack: its stickers now, and those a sync adds later.
    Pack { pack_id: String },
    /// Everything the account can see, now and later.
    Library,
}

impl From<OfflineScope> for offline::Scope {
    fn from(s: OfflineScope) -> Self {
        match s {
            OfflineScope::Sticker { item_id } => offline::Scope::Item(item_id),
            OfflineScope::Pack { pack_id } => offline::Scope::Set(pack_id),
            OfflineScope::Library => offline::Scope::Library,
        }
    }
}

impl From<offline::Scope> for OfflineScope {
    fn from(s: offline::Scope) -> Self {
        match s {
            offline::Scope::Item(item_id) => OfflineScope::Sticker { item_id },
            offline::Scope::Set(pack_id) => OfflineScope::Pack { pack_id },
            offline::Scope::Library => OfflineScope::Library,
        }
    }
}

/// Why a sticker is kept on this phone. Reasons combine: it stays until the
/// last one is gone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum KeepReason {
    /// Kept by itself ([`OfflineScope::Sticker`]).
    Sticker,
    /// Its pack is kept.
    Pack,
    /// The library is kept.
    Library,
    /// One of the stickers sent last from this phone (`auto_used`).
    Used,
    /// Liked by the account (`auto_liked`).
    Liked,
    /// In a pack the account owns (`auto_own_packs`).
    OwnPack,
}

impl From<offline::Reason> for KeepReason {
    fn from(r: offline::Reason) -> Self {
        match r {
            offline::Reason::Item => KeepReason::Sticker,
            offline::Reason::Set => KeepReason::Pack,
            offline::Reason::Library => KeepReason::Library,
            offline::Reason::Used => KeepReason::Used,
            offline::Reason::Liked => KeepReason::Liked,
            offline::Reason::OwnPack => KeepReason::OwnPack,
        }
    }
}

/// What totals and progress are counted for: a scope the user keeps, or an
/// automatic rule that is on.
#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum OfflinePart {
    /// `auto_thumbnails`: every sticker's thumbnail.
    Thumbnails,
    Kept {
        scope: OfflineScope,
    },
    /// `auto_used`.
    Used,
    /// `auto_liked`.
    Liked,
    /// `auto_own_packs`.
    OwnPacks,
}

impl From<offline::PlanScope> for OfflinePart {
    fn from(s: offline::PlanScope) -> Self {
        match s {
            offline::PlanScope::Thumbnails => OfflinePart::Thumbnails,
            offline::PlanScope::Kept(scope) => OfflinePart::Kept { scope: scope.into() },
            offline::PlanScope::Used => OfflinePart::Used,
            offline::PlanScope::Liked => OfflinePart::Liked,
            offline::PlanScope::OwnPacks => OfflinePart::OwnPacks,
        }
    }
}

/// Files and bytes wanted, and how much of it is on this phone. Sizes as
/// the stickers' metadata says (an unknown size counts 0).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, uniffi::Record)]
pub struct OfflineTotals {
    pub files: u64,
    pub bytes: u64,
    pub present_files: u64,
    pub present_bytes: u64,
}

impl From<offline::Totals> for OfflineTotals {
    fn from(t: offline::Totals) -> Self {
        OfflineTotals {
            files: t.files,
            bytes: t.bytes,
            present_files: t.present_files,
            present_bytes: t.present_bytes,
        }
    }
}

/// What the options leave out, counted by reason: skipped, not failed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, uniffi::Record)]
pub struct OfflineSkips {
    /// Stickers of a kind switched off (`stills`, `animations`, `videos`),
    /// kept only with a pack, the library or an automatic rule.
    pub kind_off: u64,
    /// Files larger than `max_file_mb`.
    pub too_large: u64,
    /// Stickers kept by themselves that the offline cache does not hold
    /// yet (a sync brings them).
    pub not_cached: u64,
    /// Stickers with no file a copy could use.
    pub no_rendition: u64,
}

impl From<&[offline::Skipped]> for OfflineSkips {
    fn from(skipped: &[offline::Skipped]) -> Self {
        let mut out = OfflineSkips::default();
        for s in skipped {
            match s.reason {
                offline::SkipReason::KindOff { .. } => out.kind_off += 1,
                offline::SkipReason::TooLarge { .. } => out.too_large += 1,
                offline::SkipReason::NotCached => out.not_cached += 1,
                offline::SkipReason::NoRendition => out.no_rendition += 1,
            }
        }
        out
    }
}

/// The totals of one kept scope or automatic rule.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct OfflinePartTotals {
    pub part: OfflinePart,
    /// The kept sticker's or pack's name, from the offline cache; `None`
    /// for the library, the rules, and what the cache does not hold.
    pub name: Option<String>,
    /// Stickers it keeps (skipped ones left out).
    pub items: u64,
    pub totals: OfflineTotals,
}

/// What is kept on this phone and what that means in files
/// (`StickpileLibrary.offline_plan`).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct OfflinePlan {
    pub totals: OfflineTotals,
    /// Stickers kept for at least one reason, skipped ones included.
    pub items: u64,
    pub skipped: OfflineSkips,
    /// Wanted files whose download failed (the reasons are logged).
    pub failed: u64,
    /// `max_mb` in bytes; `None`: no limit.
    pub budget: Option<u64>,
    /// The files wanted are more than the budget allows.
    pub over_budget: bool,
    /// Per kept scope and automatic rule, in display order.
    pub parts: Vec<OfflinePartTotals>,
}

impl From<offline::Plan> for OfflinePlan {
    fn from(p: offline::Plan) -> Self {
        OfflinePlan {
            totals: p.totals.into(),
            items: p.items,
            skipped: p.skipped.as_slice().into(),
            failed: p.failures.len() as u64,
            budget: p.budget,
            over_budget: p.over_budget(),
            parts: p
                .scopes
                .into_iter()
                .map(|s| OfflinePartTotals {
                    part: s.scope.into(),
                    name: s.name,
                    items: s.items,
                    totals: s.totals.into(),
                })
                .collect(),
        }
    }
}

/// What keeping a scope would take, shown before keeping a pack or the
/// library is confirmed (`StickpileLibrary.offline_estimate`).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct OfflineEstimate {
    /// The scope's own files: how many, how large, how much of it is on
    /// this phone already.
    pub totals: OfflineTotals,
    /// Stickers it would keep (skipped ones left out).
    pub items: u64,
    pub skipped: OfflineSkips,
    /// Bytes every kept file would take with the scope kept too.
    pub kept_bytes_after: u64,
    /// `max_mb` in bytes; `None`: no limit.
    pub budget: Option<u64>,
    /// `kept_bytes_after` is within the budget.
    pub fits: bool,
}

impl From<offline::Estimate> for OfflineEstimate {
    fn from(e: offline::Estimate) -> Self {
        OfflineEstimate {
            fits: e.fits(),
            totals: e.totals.into(),
            items: e.items,
            skipped: e.skipped.as_slice().into(),
            kept_bytes_after: e.kept_bytes_after,
            budget: e.budget,
        }
    }
}

/// The files on this phone: those kept offline, and the others (thumbnails
/// and sent files the cache size limit governs).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, uniffi::Record)]
pub struct OfflineUsage {
    pub kept_files: u64,
    pub kept_bytes: u64,
    pub other_files: u64,
    pub other_bytes: u64,
    /// `max_mb` in bytes; `None`: no limit.
    pub budget: Option<u64>,
}

impl From<offline::OfflineUsage> for OfflineUsage {
    fn from(u: offline::OfflineUsage) -> Self {
        OfflineUsage {
            kept_files: u.kept_files,
            kept_bytes: u.kept_bytes,
            other_files: u.other_files,
            other_bytes: u.other_bytes,
            budget: u.budget,
        }
    }
}

/// What clearing deleted, and the kept files that stay
/// (`StickpileLibrary.clear_cache`, `remove_kept_files`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, uniffi::Record)]
pub struct CacheCleared {
    pub removed_files: u64,
    pub freed_bytes: u64,
    /// Kept files left on this phone afterwards.
    pub kept_files: u64,
    pub kept_bytes: u64,
}

impl From<stickpile_core::cache::EvictReport> for CacheCleared {
    fn from(r: stickpile_core::cache::EvictReport) -> Self {
        CacheCleared {
            removed_files: r.removed_files as u64,
            freed_bytes: r.freed_bytes,
            kept_files: r.kept_files as u64,
            kept_bytes: r.kept_bytes,
        }
    }
}

/// What the offline downloader is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum OfflineState {
    /// Not downloading: finished, cancelled, or not started.
    Idle,
    Downloading,
    /// Paused (`pause_offline`).
    Paused,
    /// The server could not be reached; the next run after a sync goes on.
    WaitingForNetwork,
    /// The next file would go over `max_mb`; nothing was deleted.
    LimitReached,
    /// The phone's storage is full; nothing was deleted.
    DiskFull,
}

impl From<offline::OfflineState> for OfflineState {
    fn from(s: offline::OfflineState) -> Self {
        match s {
            offline::OfflineState::Idle => OfflineState::Idle,
            offline::OfflineState::Downloading => OfflineState::Downloading,
            offline::OfflineState::Paused => OfflineState::Paused,
            offline::OfflineState::WaitingForNetwork => OfflineState::WaitingForNetwork,
            offline::OfflineState::LimitReached => OfflineState::LimitReached,
            offline::OfflineState::DiskFull => OfflineState::DiskFull,
        }
    }
}

/// The progress of one kept scope or automatic rule.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct OfflinePartProgress {
    pub part: OfflinePart,
    pub name: Option<String>,
    pub files_done: u64,
    pub files_total: u64,
    pub bytes_done: u64,
    pub bytes_total: u64,
}

/// Where the offline downloader stands (`StickpileLibrary.offline_status`).
/// "Done" counts the wanted files on this phone, those there before the
/// run included.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct OfflineStatus {
    /// As the last run left it: `Downloading` stays after a run that was
    /// stopped from outside (see `running`).
    pub state: OfflineState,
    pub files_done: u64,
    pub files_total: u64,
    pub bytes_done: u64,
    pub bytes_total: u64,
    /// Stickers and files the options leave out.
    pub skipped: u64,
    /// Wanted files whose download failed.
    pub failed: u64,
    pub parts: Vec<OfflinePartProgress>,
    /// What stopped or holds the run, in English, for logs.
    pub message: Option<String>,
    /// A run (`offline_run`) is going on in this process now.
    pub running: bool,
    /// Downloads are paused (`pause_offline`) until `resume_offline`.
    pub paused: bool,
}

impl OfflineStatus {
    pub(crate) fn new(s: &offline::OfflineStatus, running: bool, paused: bool) -> OfflineStatus {
        OfflineStatus {
            state: s.state.into(),
            files_done: s.files_done,
            files_total: s.files_total,
            bytes_done: s.bytes_done,
            bytes_total: s.bytes_total,
            skipped: s.skipped,
            failed: s.failed,
            parts: s
                .scopes
                .iter()
                .map(|p| OfflinePartProgress {
                    part: p.scope.clone().into(),
                    name: p.name.clone(),
                    files_done: p.files_done,
                    files_total: p.files_total,
                    bytes_done: p.bytes_done,
                    bytes_total: p.bytes_total,
                })
                .collect(),
            message: s.message.clone(),
            running,
            paused,
        }
    }
}

/// How a run of the offline downloader ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum OfflineEnd {
    /// Every file was tried: on this phone, or failed.
    Complete,
    /// Cancelled (`cancel_offline`, an account change, "Remove kept files").
    Cancelled,
    /// The server could not be reached.
    WaitingForNetwork,
    /// `max_mb` is reached.
    LimitReached,
    /// The phone's storage is full.
    DiskFull,
    /// The server refused the sign-in.
    Unauthorized,
}

impl From<offline::RunEnd> for OfflineEnd {
    fn from(e: offline::RunEnd) -> Self {
        match e {
            offline::RunEnd::Complete => OfflineEnd::Complete,
            offline::RunEnd::Cancelled => OfflineEnd::Cancelled,
            offline::RunEnd::WaitingForNetwork => OfflineEnd::WaitingForNetwork,
            offline::RunEnd::LimitReached => OfflineEnd::LimitReached,
            offline::RunEnd::DiskFull => OfflineEnd::DiskFull,
            offline::RunEnd::Unauthorized => OfflineEnd::Unauthorized,
        }
    }
}

/// What a run did (`StickpileLibrary.offline_run`).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct OfflineOutcome {
    pub end: OfflineEnd,
    /// Every wanted file is on this phone.
    pub complete: bool,
    /// The last status of the run.
    pub status: OfflineStatus,
}

/// The options of keeping offline, which the app keeps in its settings
/// and gives the library (`set_offline_options`). The defaults are the
/// core's (`[offline]`, docs/offline.md); `tests/interface.rs` checks that
/// they agree. The chat apps whose files are kept are not here: those of
/// the config (none on Android), else this phone's default and those
/// stickers were sent to from it.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct OfflineOptions {
    /// Also keep the file sent as a sticker (Telegram's sticker WebP or
    /// WebM).
    #[uniffi(default = true)]
    pub sticker_renditions: bool,
    /// Download stills when keeping a pack, the library or by a rule. A
    /// sticker kept by itself is always downloaded.
    #[uniffi(default = true)]
    pub stills: bool,
    /// The same for animations.
    #[uniffi(default = true)]
    pub animations: bool,
    /// The same for videos.
    #[uniffi(default = false)]
    pub videos: bool,
    /// Skip a file larger than this many MiB when keeping a pack, the
    /// library or by a rule (0: no limit).
    #[uniffi(default = 0)]
    pub max_file_mb: u64,
    /// The most space kept files may take, in MiB (0: no limit).
    #[uniffi(default = 1024)]
    pub max_mb: u64,
    /// Keep every sticker's thumbnail.
    #[uniffi(default = true)]
    pub auto_thumbnails: bool,
    /// Keep the stickers sent last from this phone...
    #[uniffi(default = false)]
    pub auto_used: bool,
    /// ...this many (1 to 2000).
    #[uniffi(default = 200)]
    pub auto_used_count: u32,
    /// Keep the stickers the account liked.
    #[uniffi(default = false)]
    pub auto_liked: bool,
    /// Keep the packs the account owns.
    #[uniffi(default = false)]
    pub auto_own_packs: bool,
    /// Also download on metered networks (mobile data). The app decides
    /// when the downloader runs with it; the library does not read it.
    #[uniffi(default = false)]
    pub mobile_data: bool,
}

impl Default for OfflineOptions {
    /// The core's defaults.
    fn default() -> Self {
        OfflineOptions::from(&offline::OfflineOptions::default())
    }
}

impl From<&offline::OfflineOptions> for OfflineOptions {
    fn from(o: &offline::OfflineOptions) -> Self {
        OfflineOptions {
            sticker_renditions: o.sticker_renditions,
            stills: o.stills,
            animations: o.animations,
            videos: o.videos,
            max_file_mb: o.max_file_mb,
            max_mb: o.max_mb,
            auto_thumbnails: o.auto.thumbnails,
            auto_used: o.auto.used,
            auto_used_count: o.auto.used_count,
            auto_liked: o.auto.liked,
            auto_own_packs: o.auto.own_packs,
            mobile_data: o.mobile_data,
        }
    }
}

impl OfflineOptions {
    /// These options over `base` (the config's `[offline]`): its `targets`
    /// stay; the used count is kept within 1 to 2000.
    pub(crate) fn over(&self, base: &offline::OfflineOptions) -> offline::OfflineOptions {
        offline::OfflineOptions {
            targets: base.targets.clone(),
            sticker_renditions: self.sticker_renditions,
            stills: self.stills,
            animations: self.animations,
            videos: self.videos,
            max_file_mb: self.max_file_mb,
            max_mb: self.max_mb,
            auto: offline::AutoKeepConfig {
                thumbnails: self.auto_thumbnails,
                used: self.auto_used,
                used_count: self.auto_used_count.clamp(1, stickpile_core::config::AUTO_USED_MAX),
                liked: self.auto_liked,
                own_packs: self.auto_own_packs,
            },
            mobile_data: self.mobile_data,
        }
    }
}
