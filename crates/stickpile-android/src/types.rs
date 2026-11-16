//! The records and enums of the interface, each converted from its
//! `stickpile-api` counterpart (no UniFFI derive goes on those).

use stickpile_api::{ItemSummary, Role, SetSummary, TargetApp, User};

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
    /// highlighted); `None` while no value is typed.
    pub chosen: Option<u32>,
}

impl From<stickpile_quick::suggest::Suggestions> for Suggestions {
    fn from(s: stickpile_quick::suggest::Suggestions) -> Self {
        let chosen = s.chosen().and_then(|i| u32::try_from(i).ok());
        Suggestions {
            key: s.key,
            words: s.words,
            chosen,
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
    /// What to show: `sarcastic`, `tag:reaction`, `vibe:`.
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
}

/// A pack made ready for WhatsApp (`StickpileLibrary.whatsapp_packs`).
/// WhatsApp takes 3 to 30 stickers of one kind (still or animated) per
/// sticker pack, so the server may split a pack into several parts; each
/// part is one sticker pack in WhatsApp, added on its own.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct WhatsappPacks {
    /// The parts in order (part 1 first).
    pub parts: Vec<WhatsappPart>,
    /// Items of the pack the server could not make into WhatsApp stickers,
    /// left out of every part.
    pub skipped: u32,
    /// The server could not be reached: these are the parts unpacked on
    /// this device before.
    pub offline: bool,
}

/// One part of a pack: one WhatsApp sticker pack, unpacked on this device,
/// as the app's WhatsApp provider serves it.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct WhatsappPart {
    /// The stickpile pack it comes from.
    pub pack_id: String,
    /// 1-based.
    pub number: u32,
    /// What WhatsApp knows it by, unique in the app: `<pack id>-<number>`
    /// (the provider's `sticker_pack_identifier`).
    pub identifier: String,
    /// At most 128 characters; `<pack name> (2/3)` for a pack in parts.
    pub name: String,
    /// The pack owner's display name, at most 128 characters.
    pub publisher: String,
    /// All its stickers move (else none does).
    pub animated: bool,
    /// Changes whenever its stickers, their emoji or its tray icon change
    /// (the provider's `image_data_version`): WhatsApp then reloads it.
    pub version: String,
    /// The directory holding its files: absolute.
    pub dir: String,
    /// The tray icon's file name in `dir` (a 96 x 96 PNG).
    pub tray_file: String,
    /// In order.
    pub stickers: Vec<WhatsappSticker>,
}

/// One sticker of a part.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct WhatsappSticker {
    /// Its file name in the part's `dir` (a 512 x 512 WebP).
    pub file: String,
    /// 1 to 3 emoji.
    pub emojis: Vec<String>,
}

/// What `StickpileLibrary.whatsapp_packs` is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum WhatsappStep {
    /// Asking the server for the packs, and waiting while it makes them.
    Building,
    /// Downloading and unpacking `part` of `parts`.
    Downloading,
}

/// How far `StickpileLibrary.whatsapp_packs` is (`whatsapp_progress`).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct WhatsappProgress {
    pub step: WhatsappStep,
    /// While downloading, the part (1-based); else 0.
    pub part: u32,
    /// While downloading, how many parts there are; else 0.
    pub parts: u32,
}
