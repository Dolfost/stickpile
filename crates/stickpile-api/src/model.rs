//! REST DTOs. Field names are the JSON names. Additive changes only: clients
//! ignore unknown fields, so new optional fields are always safe.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum Role {
    User,
    Admin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum Visibility {
    /// Only the owner.
    #[default]
    Private,
    /// Anyone signed in to this server who has the link (set or item id). Not
    /// listed and not returned by search for non-owners.
    Unlisted,
    /// Listed and searchable by every user on the server.
    Public,
}

/// Coarse media class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum MediaKind {
    /// png, jpeg, single-frame webp/gif
    Still,
    /// animated gif or animated webp
    Animation,
    /// mp4, webm
    Video,
}

/// Container format of the stored original (after sniffing, never from the extension).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum MediaFormat {
    Png,
    Jpeg,
    Webp,
    Gif,
    Mp4,
    Webm,
}

impl MediaFormat {
    pub fn mime(self) -> &'static str {
        match self {
            MediaFormat::Png => "image/png",
            MediaFormat::Jpeg => "image/jpeg",
            MediaFormat::Webp => "image/webp",
            MediaFormat::Gif => "image/gif",
            MediaFormat::Mp4 => "video/mp4",
            MediaFormat::Webm => "video/webm",
        }
    }
    pub fn extension(self) -> &'static str {
        match self {
            MediaFormat::Png => "png",
            MediaFormat::Jpeg => "jpg",
            MediaFormat::Webp => "webp",
            MediaFormat::Gif => "gif",
            MediaFormat::Mp4 => "mp4",
            MediaFormat::Webm => "webm",
        }
    }
}

/// One stored rendition of a media item. Generated on ingest (thumb
/// synchronously, the rest by a background job); clients pick one with
/// [`crate::profiles::TargetProfile`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum Variant {
    /// Sanitised original (metadata stripped, container rewritten).
    Original,
    /// 256 px-bounded still WebP of the first frame, for grids.
    Thumb,
    /// PNG of a still (absent when the original already is PNG — use Original).
    Png,
    /// 512 px static WebP (one side exactly 512, <= 512 KB): Telegram static sticker.
    TgSticker,
    /// 512 px VP9 WebM, <= 3 s, <= 256 KB, no audio: Telegram video sticker (animations/videos).
    TgVideoSticker,
    /// GIF bounded to the Discord upload limit (animations/videos).
    DiscordGif,
    /// H.264 MP4 without audio (animations; videos when the original is WebM).
    Mp4,
}

impl Variant {
    pub const ALL: [Variant; 7] = [
        Variant::Original,
        Variant::Thumb,
        Variant::Png,
        Variant::TgSticker,
        Variant::TgVideoSticker,
        Variant::DiscordGif,
        Variant::Mp4,
    ];
    pub fn as_str(self) -> &'static str {
        match self {
            Variant::Original => "original",
            Variant::Thumb => "thumb",
            Variant::Png => "png",
            Variant::TgSticker => "tg_sticker",
            Variant::TgVideoSticker => "tg_video_sticker",
            Variant::DiscordGif => "discord_gif",
            Variant::Mp4 => "mp4",
        }
    }
    pub fn parse(s: &str) -> Option<Variant> {
        Variant::ALL.into_iter().find(|v| v.as_str() == s)
    }
}

/// Where a metadata field's current value came from. AI values stay marked
/// until the user edits or confirms them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum FieldSource {
    #[default]
    User,
    Ai,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum SetSource {
    Local,
    Git,
    Telegram,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum GitMode {
    /// Read-only in the web UI, "edit upstream" link.
    ReadOnly,
    /// Edits are committed and pushed back (server needs push credentials).
    Push,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    Running,
    Done,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    /// Generate non-thumb variants for a media item.
    Variants,
    /// Compute embedding + AI keywords/emojis/vibe for a media item.
    Enrich,
    GitPull,
    GitPush,
    TelegramImport,
    TelegramExport,
}

/// Chat apps clients can target. `Generic` = browser chats and anything unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum TargetApp {
    Telegram,
    Discord,
    Signal,
    Whatsapp,
    Slack,
    #[default]
    Generic,
}

impl TargetApp {
    pub const ALL: [TargetApp; 6] = [
        TargetApp::Telegram,
        TargetApp::Discord,
        TargetApp::Signal,
        TargetApp::Whatsapp,
        TargetApp::Slack,
        TargetApp::Generic,
    ];
    pub fn as_str(self) -> &'static str {
        match self {
            TargetApp::Telegram => "telegram",
            TargetApp::Discord => "discord",
            TargetApp::Signal => "signal",
            TargetApp::Whatsapp => "whatsapp",
            TargetApp::Slack => "slack",
            TargetApp::Generic => "generic",
        }
    }
    pub fn parse(s: &str) -> Option<TargetApp> {
        TargetApp::ALL.into_iter().find(|t| t.as_str().eq_ignore_ascii_case(s))
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Body of every non-2xx response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ApiError {
    /// Stable machine code: `unauthorized`, `forbidden`, `not_found`,
    /// `bad_request`, `conflict`, `too_large`, `unsupported_media`,
    /// `rate_limited`, `csrf`, `internal`, `unavailable` (AI off),
    /// `ai_loading` (the AI model is loading; see `Retry-After`).
    pub code: String,
    pub message: String,
}

// ---------------------------------------------------------------------------
// Users & auth
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct User {
    pub id: String,
    pub username: String,
    pub display_name: String,
    pub role: Role,
    pub totp_enabled: bool,
    /// `Tailscale-User-Login` this account is bound to, if any.
    pub tailscale_login: Option<String>,
    pub created_at: i64,
    /// Telegram user id that owns the packs this user exports (falls back to
    /// the server's `telegram.default_owner_user_id`).
    #[serde(default)]
    pub telegram_user_id: Option<i64>,
    /// Colour theme of the web UI.
    #[serde(default)]
    pub theme: Theme,
}

/// Colour theme of the web UI, chosen per user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    /// Follow the device's light or dark setting (`prefers-color-scheme`).
    #[default]
    System,
    Light,
    Dark,
}

impl Theme {
    pub const ALL: [Theme; 3] = [Theme::System, Theme::Light, Theme::Dark];
    pub fn as_str(self) -> &'static str {
        match self {
            Theme::System => "system",
            Theme::Light => "light",
            Theme::Dark => "dark",
        }
    }
    /// Exact lowercase names only (`system`, `light`, `dark`).
    pub fn parse(s: &str) -> Option<Theme> {
        Theme::ALL.into_iter().find(|t| t.as_str() == s)
    }
}

/// `PATCH /api/v1/me`: absent = unchanged; `telegram_user_id: null` clears it.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct UpdateMe {
    /// New username (2..=32 of `A-Z a-z 0-9 _ . -`, starting with a letter or
    /// digit; unique ignoring case). A display name equal to the old username
    /// follows it.
    #[serde(default)]
    pub username: Option<String>,
    /// Empty resets the display name to the username.
    pub display_name: Option<String>,
    #[serde(default, with = "double_option")]
    pub telegram_user_id: Option<Option<i64>>,
    /// Colour theme of the web UI: `system`, `light` or `dark`.
    #[serde(default)]
    pub theme: Option<Theme>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct TokenRequest {
    pub username: String,
    pub password: String,
    /// 6-digit TOTP code; required when the account has TOTP enabled.
    pub totp: Option<String>,
    /// Shown in the token list, e.g. "laptop desktop client".
    pub token_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CreateToken {
    pub name: String,
    /// Optional expiry, unix seconds.
    pub expires_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ApiToken {
    pub id: String,
    pub name: String,
    /// First characters of the secret, for recognising it (`spk_ab12…`).
    pub prefix: String,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
    pub expires_at: Option<i64>,
    pub revoked: bool,
}

/// Returned once at creation; the secret is never retrievable again.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct TokenCreated {
    pub token: String,
    pub info: ApiToken,
    pub user: User,
}

/// Self-service registration ([`crate::routes::AUTH_REGISTER`]).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RegisterRequest {
    /// 2..=32 chars of `[A-Za-z0-9_.-]`, starting with a letter or digit.
    pub username: String,
    /// At least 8 characters.
    pub password: String,
    pub display_name: Option<String>,
    /// Required when the server's registration policy is `invite`.
    pub invite_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ChangePassword {
    /// Required when the account already has a password.
    pub current_password: Option<String>,
    pub new_password: String,
}

/// Returned when TOTP enrolment starts; the secret is pending until confirmed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct TotpSetup {
    /// Base32 secret for manual entry.
    pub secret: String,
    /// `otpauth://totp/...` URL (QR code content).
    pub otpauth_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct TotpCode {
    /// Current 6-digit code.
    pub code: String,
}

// ---------------------------------------------------------------------------
// Sets
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct GitBinding {
    pub url: String,
    pub branch: Option<String>,
    pub mode: GitMode,
    pub last_commit: Option<String>,
    pub last_pull_at: Option<i64>,
    pub last_error: Option<String>,
    /// Browser URL for "edit upstream", derived from `url` when recognisable.
    pub web_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Set {
    pub id: String,
    pub owner_id: String,
    pub owner_username: String,
    /// `[a-z0-9-]`, unique per owner; used as export directory name and in `set:` search.
    pub slug: String,
    pub name: String,
    pub description: String,
    pub visibility: Visibility,
    /// Recommended aspect ratio for new items, e.g. (1, 1).
    pub aspect_ratio: Option<(u32, u32)>,
    /// Recommended longest side in pixels for new items (512 for Telegram stickers).
    pub recommended_px: Option<u32>,
    /// `set.toml` version; bumped on every metadata/content change.
    pub version: u64,
    pub source: SetSource,
    pub git: Option<GitBinding>,
    /// Telegram pack name this set is published as, if any.
    pub telegram_pack: Option<String>,
    /// The item chosen as the cover: shown for the set in lists and sent to
    /// Telegram as the pack's thumbnail. `None`: the first item is shown and
    /// Telegram uses the first sticker. A [`Set::cover_image`] wins over it.
    #[serde(default)]
    pub cover_item_id: Option<String>,
    /// The set's own cover image (not an item), if one was uploaded: shown
    /// instead of any item and sent to Telegram as the pack's thumbnail.
    #[serde(default)]
    pub cover_image: Option<CoverImage>,
    /// URL path of the thumbnail shown for the set: the cover image, else the
    /// chosen cover item, else the first item. `None`: an empty set without
    /// a cover image.
    #[serde(default)]
    pub cover_url: Option<String>,
    pub item_count: u64,
    /// True if the caller may edit (owner, and not a read-only git pack).
    pub editable: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SetSummary {
    pub id: String,
    pub slug: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub owner_username: String,
    pub visibility: Visibility,
    pub source: SetSource,
    pub item_count: u64,
    /// The item shown for the set: the chosen cover, else the first item.
    /// A set with a cover image shows that instead (see `cover_url`).
    pub cover_item_id: Option<String>,
    /// URL path of the thumbnail shown for the set (as [`Set::cover_url`]):
    /// the cover image, else the item `cover_item_id`.
    #[serde(default)]
    pub cover_url: Option<String>,
    pub updated_at: i64,
}

/// A set's own cover image, uploaded with PUT [`crate::routes::SET_COVER`]. It
/// is not an item: it is never in item lists, search or counts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CoverImage {
    /// URL path of the grid thumbnail (WebP). Changes with the image, so it
    /// may be cached for good.
    pub thumb_url: String,
    /// URL path of the stored image (sanitised original).
    pub url: String,
    pub format: MediaFormat,
    pub width: u32,
    pub height: u32,
    /// sha256 (hex) of the image as uploaded.
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SetDetail {
    pub set: Set,
    /// In set order.
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CreateSet {
    pub slug: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub visibility: Visibility,
    pub aspect_ratio: Option<(u32, u32)>,
    pub recommended_px: Option<u32>,
}

/// PATCH semantics: absent = unchanged. For nullable fields, `Some(None)` clears.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct UpdateSet {
    pub slug: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub visibility: Option<Visibility>,
    #[serde(default, with = "double_option")]
    pub aspect_ratio: Option<Option<(u32, u32)>>,
    #[serde(default, with = "double_option")]
    pub recommended_px: Option<Option<u32>>,
    /// An item of this set as its cover; `null` goes back to the first item.
    /// Either one also removes the set's cover image: the cover is what was
    /// chosen last. Absent leaves both alone.
    #[serde(default, with = "double_option", skip_serializing_if = "Option::is_none")]
    pub cover_item_id: Option<Option<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ReorderItems {
    /// Every item id of the set, in the new order.
    pub item_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CreateGitPack {
    /// `https://` (or `ssh://`, `git@host:path` when the server has an SSH key).
    pub url: String,
    pub branch: Option<String>,
    /// Sub-directory inside the repository holding `set.toml`; default root.
    pub subdir: Option<String>,
    pub mode: GitMode,
    /// Slug for the new set; default from set.toml / repo name.
    pub slug: Option<String>,
}

/// Returned by [`crate::routes::SETS_GITPACK`] alongside the set: the webhook
/// secret is shown once.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct GitPackCreated {
    pub set: Set,
    pub webhook_url: String,
    pub webhook_secret: String,
    pub job: Job,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ImportReport {
    pub set: Option<Set>,
    pub created: u64,
    pub updated: u64,
    /// Same content hash already in this set; metadata merged.
    pub unchanged: u64,
    pub removed: u64,
    /// Per-file problems (unsupported type, too large, ...). Import continues.
    pub errors: Vec<ImportError>,
    /// Near-duplicates of new items already in the caller's library.
    pub near_duplicates: Vec<NearDuplicate>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ImportError {
    pub file: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct TelegramImport {
    /// Pack short name (`t.me/addstickers/<name>`).
    pub pack_name: String,
    /// Slug for the new set; default derived from the pack name.
    pub slug: Option<String>,
    #[serde(default)]
    pub visibility: Visibility,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct TelegramExport {
    /// Pack short name; must end in `_by_<bot username>`. Default: previously
    /// bound pack, else `<slug>_by_<bot>`.
    pub pack_name: Option<String>,
    /// Pack title; default set name.
    pub title: Option<String>,
}

// ---------------------------------------------------------------------------
// Items & media
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct VariantInfo {
    pub variant: Variant,
    pub mime: String,
    pub width: u32,
    pub height: u32,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MediaInfo {
    pub id: String,
    /// sha256 of the uploaded bytes, lowercase hex: the dedup key.
    pub sha256: String,
    pub kind: MediaKind,
    pub format: MediaFormat,
    pub width: u32,
    pub height: u32,
    pub duration_ms: Option<u64>,
    pub frame_count: Option<u32>,
    pub fps: Option<f32>,
    pub has_alpha: bool,
    pub size_bytes: u64,
    /// 64-bit perceptual hashes as 16 lowercase hex chars.
    pub phash: String,
    pub dhash: String,
    /// 0..=100, `None` until computed.
    pub quality: Option<u8>,
    /// Variants that exist right now (generation may still be running).
    pub variants: Vec<VariantInfo>,
}

/// One entry of a set: a media item plus per-set metadata.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Item {
    pub id: String,
    pub set_id: String,
    pub position: u32,
    /// Export filename stem without the numeric order prefix or extension,
    /// e.g. `cat` (exported as `001-cat.png` via `stickpile_setfmt::export_filename`).
    pub filename: String,
    pub name: String,
    pub name_source: FieldSource,
    pub keywords: Vec<String>,
    pub keywords_source: FieldSource,
    pub emojis: Vec<String>,
    pub emojis_source: FieldSource,
    /// One label from the vibe vocabulary (happy, sad, sarcastic, ...).
    pub vibe: Option<String>,
    pub vibe_source: FieldSource,
    /// User tags (never AI).
    pub tags: Vec<String>,
    pub media: MediaInfo,
    pub likes: u64,
    pub liked_by_me: bool,
    pub copies: u64,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Compact item for grids, search results and caches.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ItemSummary {
    pub id: String,
    pub set_id: String,
    pub set_slug: String,
    pub set_name: String,
    pub owner_username: String,
    pub name: String,
    pub keywords: Vec<String>,
    pub emojis: Vec<String>,
    pub vibe: Option<String>,
    pub tags: Vec<String>,
    pub kind: MediaKind,
    pub format: MediaFormat,
    pub width: u32,
    pub height: u32,
    pub quality: Option<u8>,
    pub likes: u64,
    pub copies: u64,
    pub media_id: String,
    pub sha256: String,
    pub phash: String,
    pub variants: Vec<VariantInfo>,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct QualityComponent {
    /// `resolution`, `jpeg_quality`, `recompression`, `blockiness`, `ringing`,
    /// `sharpness`, `alpha`, `frame_rate`, `palette`.
    pub name: String,
    /// 0..=100 for this component.
    pub score: u8,
    /// Weight used in the total.
    pub weight: f32,
    /// Human-readable measurement, e.g. "estimated JPEG q=62".
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct QualityReport {
    pub score: u8,
    pub components: Vec<QualityComponent>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct NearDuplicate {
    /// The new (or inspected) item.
    pub item_id: String,
    /// Existing item in the caller's library with a close perceptual hash.
    pub existing: ItemSummary,
    /// Hamming distance of the 64-bit pHash (0 = identical image content).
    pub distance: u32,
    pub same_content: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ItemDetail {
    pub item: Item,
    pub set: SetSummary,
    pub quality: Option<QualityReport>,
    /// Other items in the caller's library that look the same.
    pub near_duplicates: Vec<NearDuplicate>,
    /// Profiles that cannot accept this item in any variant, with reasons.
    pub incompatible: Vec<Incompatibility>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Incompatibility {
    pub target: TargetApp,
    pub reason: String,
}

/// Optional per-file metadata in an upload (`meta` multipart part, matched by index).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct UploadMeta {
    pub name: Option<String>,
    pub keywords: Option<Vec<String>>,
    pub emojis: Option<Vec<String>>,
    pub tags: Option<Vec<String>>,
    /// One label, usually from the vibe vocabulary (`tired`, `sarcastic`, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vibe: Option<String>,
    /// Source URL (browser extension, post-v1).
    pub source_url: Option<String>,
    /// Given fields that hold AI suggestions the user has not approved (from
    /// `POST /api/v1/ai/suggest`): any of `keywords`, `emojis`, `vibe`.
    /// They are stored with `*_source: ai`, like enrichment results; every
    /// other given field is the user's.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ai_fields: Vec<String>,
}

/// Zero-shot suggestions for one image (`POST /api/v1/ai/suggest`), chosen
/// like the ones enrichment stores. Nothing is stored.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AiSuggestions {
    /// Best first; empty when nothing in the vocabulary fits.
    pub keywords: Vec<String>,
    pub emojis: Vec<String>,
    /// `None` when the model is unsure.
    pub vibe: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct UploadResult {
    /// Original multipart filename.
    pub file: String,
    pub item: Option<Item>,
    pub error: Option<String>,
    /// True if this exact content was already in the set (no new item created).
    pub already_in_set: bool,
    /// "You already have this in set X."
    pub near_duplicates: Vec<NearDuplicate>,
}

/// PATCH semantics: absent = unchanged. Editing a field marks its source `user`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct UpdateItem {
    pub name: Option<String>,
    pub keywords: Option<Vec<String>>,
    pub emojis: Option<Vec<String>>,
    #[serde(default, with = "double_option")]
    pub vibe: Option<Option<String>>,
    pub tags: Option<Vec<String>>,
    /// Mark AI values as confirmed without changing them:
    /// any of `name`, `keywords`, `emojis`, `vibe`.
    #[serde(default)]
    pub confirm: Vec<String>,
    /// Move to another own set (keeps media, appends at the end).
    pub set_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ItemPage {
    pub items: Vec<ItemSummary>,
    /// Item ids the caller could see before but no longer can (deleted, made private).
    /// Only populated when `updated_since` was given.
    pub removed: Vec<String>,
    pub next_cursor: Option<String>,
    /// Pass as `updated_since` next time.
    pub server_time: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LikeState {
    pub liked: bool,
    pub likes: u64,
}

// ---------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SearchHit {
    pub item: ItemSummary,
    /// Fused score, higher is better; only comparable within one response.
    pub score: f32,
    /// Which signals matched: `name`, `keyword`, `emoji`, `vibe`, `tag`, `set`,
    /// `fuzzy`, `image` (text->image embedding), `semantic` (text->keywords embedding).
    pub matched: Vec<String>,
    /// Set when `target` was given and the item cannot be sent there.
    pub incompatible: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SearchResults {
    pub hits: Vec<SearchHit>,
    pub total: u64,
    /// Normalised echo of the parsed query (see [`crate::query::Query`]'s Display).
    pub query: String,
    /// False if the AI backend was unavailable and only metadata search ran.
    pub semantic: bool,
}

// ---------------------------------------------------------------------------
// Usage, ratings, stats
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct UsageEvent {
    /// Client-generated unique id (UUID or 26-char base32) — makes uploads idempotent.
    pub id: String,
    pub item_id: String,
    /// Unix seconds when the user copied the item (client clock).
    pub copied_at: i64,
    pub target: Option<TargetApp>,
    pub variant: Option<Variant>,
    /// e.g. `desktop/0.1.0`, `cli/0.1.0`, `web`.
    pub client: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct UsageBatch {
    pub events: Vec<UsageEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct UsageAck {
    pub accepted: u64,
    /// Already recorded (same id).
    pub duplicates: u64,
    /// Ids rejected because the item does not exist or is not visible.
    pub rejected: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PopularEntry {
    pub item: ItemSummary,
    pub value: u64,
}

/// Three separate signals, never blended.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Popular {
    /// Copy events in the window.
    pub most_copied: Vec<PopularEntry>,
    /// Total likes (likes are not time-windowed... `value` = likes).
    pub most_liked: Vec<PopularEntry>,
    /// Copies in the last 7 days relative to the prior 4-week weekly average;
    /// `value` = copies in the last 7 days.
    pub trending: Vec<PopularEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct UserStats {
    pub sets: u64,
    pub items: u64,
    pub storage_bytes: u64,
    pub copies_total: u64,
    pub copies_last_7d: u64,
    pub likes_given: u64,
    pub likes_received: u64,
    pub top_items: Vec<PopularEntry>,
    pub top_targets: Vec<(TargetApp, u64)>,
    /// Copies per day for the last 30 days, oldest first.
    pub daily_copies: Vec<u64>,
}

// ---------------------------------------------------------------------------
// Jobs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Job {
    pub id: String,
    pub kind: JobKind,
    pub status: JobStatus,
    pub attempts: u32,
    pub error: Option<String>,
    /// Human-readable target, e.g. set slug or media id.
    pub subject: Option<String>,
    /// Kind-specific result, e.g. `{"set_id": "..."}` after a Telegram import.
    pub result: Option<serde_json::Value>,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
}

// ---------------------------------------------------------------------------
// Admin
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AdminUser {
    pub user: User,
    pub disabled: bool,
    pub has_password: bool,
    pub last_login_at: Option<i64>,
    pub sets: u64,
    pub items: u64,
    pub storage_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AdminCreateUser {
    pub username: String,
    pub display_name: Option<String>,
    pub password: Option<String>,
    pub role: Role,
    pub tailscale_login: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AdminUpdateUser {
    /// Rename the user (same rules as [`UpdateMe::username`]).
    #[serde(default)]
    pub username: Option<String>,
    pub display_name: Option<String>,
    pub role: Option<Role>,
    pub disabled: Option<bool>,
    /// Sets a new password and revokes all sessions.
    pub password: Option<String>,
    #[serde(default, with = "double_option")]
    pub tailscale_login: Option<Option<String>>,
    /// Remove TOTP (lost device).
    #[serde(default)]
    pub reset_totp: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Invitation {
    pub id: String,
    pub role: Role,
    pub created_by: String,
    pub created_at: i64,
    pub expires_at: Option<i64>,
    pub used_by: Option<String>,
    pub used_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CreateInvitation {
    #[serde(default = "default_role")]
    pub role: Role,
    /// Seconds from now; default 7 days.
    pub expires_in: Option<i64>,
}

fn default_role() -> Role {
    Role::User
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct InvitationCreated {
    pub invitation: Invitation,
    /// Registration link containing the one-time code.
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AuditEntry {
    pub id: i64,
    pub at: i64,
    pub actor_id: Option<String>,
    pub actor_username: Option<String>,
    /// e.g. `login`, `login_failed`, `user.create`, `user.update`, `token.create`,
    /// `token.revoke`, `invitation.create`, `set.delete`.
    pub action: String,
    pub target: Option<String>,
    pub ip: Option<String>,
    pub details: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct StorageUsage {
    /// Bytes on disk under the media directory (originals + variants).
    pub media_bytes: u64,
    pub database_bytes: u64,
    pub media_count: u64,
    pub item_count: u64,
    /// Per user: bytes of media referenced by their items (shared media counted for each).
    pub per_user: Vec<(String, u64)>,
}

// ---------------------------------------------------------------------------
// Problems (docs/api.md, "Problems")
// ---------------------------------------------------------------------------

/// How much a [`Problem`] matters. Ordered: `Error` < `Warning` < `Info`, so
/// sorting puts the worst first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ProblemSeverity {
    /// Something is broken or will fail (a Telegram publish, missing files).
    Error,
    /// Probably a mistake worth fixing (duplicates, items Telegram skips).
    Warning,
    /// Not wrong, worth a look (AI hints, unreviewed AI fields).
    Info,
}

impl ProblemSeverity {
    pub const ALL: [ProblemSeverity; 3] = [ProblemSeverity::Error, ProblemSeverity::Warning, ProblemSeverity::Info];
    pub fn as_str(self) -> &'static str {
        match self {
            ProblemSeverity::Error => "error",
            ProblemSeverity::Warning => "warning",
            ProblemSeverity::Info => "info",
        }
    }
    pub fn parse(s: &str) -> Option<ProblemSeverity> {
        ProblemSeverity::ALL.into_iter().find(|v| v.as_str() == s)
    }
}

/// What a [`Problem`] is about. New kinds may be added; clients should show
/// unknown kinds by their `message`. Declared (and sorted) most important
/// first: lists order problems by severity, then by kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ProblemKind {
    /// Two or more items of the set store the same file (same sanitised original).
    ExactDuplicate,
    /// Two items show the same picture: resized, recompressed or slightly cropped.
    NearDuplicate,
    /// Media files of an item are missing (no original or thumbnail).
    MissingFiles,
    /// Generating an item's variants failed or never finished.
    ProcessingFailed,
    /// The last git pull (or push) failed.
    GitFailed,
    /// The last publish to Telegram failed.
    TelegramFailed,
    /// More stickers than a Telegram pack holds.
    TelegramTooMany,
    /// Items the Telegram export skips (cannot become a sticker).
    TelegramUnsupported,
    /// The set changed after its last publish to Telegram.
    TelegramOutdated,
    /// Items without an emoji in a set published to Telegram (they get 🙂).
    NoEmoji,
    /// Emoji entries that are not emoji (Telegram drops them).
    NotEmoji,
    /// Animations and videos longer than a video sticker: only the start is sent.
    TelegramTrimmed,
    /// Items with a very small picture.
    LowResolution,
    /// Two items look alike to the AI (CLIP). A hint, often intentional.
    Similar,
    /// Items that are also in other sets of the same owner.
    InOtherSet,
    /// Items with AI-filled keywords, emojis, vibe or name nobody confirmed.
    AiUnreviewed,
    /// The set has no items.
    EmptySet,
}

impl ProblemKind {
    pub const ALL: [ProblemKind; 17] = [
        ProblemKind::ExactDuplicate,
        ProblemKind::NearDuplicate,
        ProblemKind::MissingFiles,
        ProblemKind::ProcessingFailed,
        ProblemKind::GitFailed,
        ProblemKind::TelegramFailed,
        ProblemKind::TelegramTooMany,
        ProblemKind::TelegramUnsupported,
        ProblemKind::TelegramOutdated,
        ProblemKind::NoEmoji,
        ProblemKind::NotEmoji,
        ProblemKind::TelegramTrimmed,
        ProblemKind::LowResolution,
        ProblemKind::Similar,
        ProblemKind::InOtherSet,
        ProblemKind::AiUnreviewed,
        ProblemKind::EmptySet,
    ];
    pub fn as_str(self) -> &'static str {
        match self {
            ProblemKind::ExactDuplicate => "exact_duplicate",
            ProblemKind::NearDuplicate => "near_duplicate",
            ProblemKind::Similar => "similar",
            ProblemKind::InOtherSet => "in_other_set",
            ProblemKind::MissingFiles => "missing_files",
            ProblemKind::ProcessingFailed => "processing_failed",
            ProblemKind::LowResolution => "low_resolution",
            ProblemKind::AiUnreviewed => "ai_unreviewed",
            ProblemKind::NotEmoji => "not_emoji",
            ProblemKind::NoEmoji => "no_emoji",
            ProblemKind::TelegramTooMany => "telegram_too_many",
            ProblemKind::TelegramUnsupported => "telegram_unsupported",
            ProblemKind::TelegramTrimmed => "telegram_trimmed",
            ProblemKind::TelegramOutdated => "telegram_outdated",
            ProblemKind::TelegramFailed => "telegram_failed",
            ProblemKind::GitFailed => "git_failed",
            ProblemKind::EmptySet => "empty_set",
        }
    }
    pub fn parse(s: &str) -> Option<ProblemKind> {
        ProblemKind::ALL.into_iter().find(|k| k.as_str() == s)
    }
}

/// The fix a client should offer for a [`Problem`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ProblemAction {
    /// Look at the items side by side and delete the ones not wanted.
    DeleteOne,
    /// Look at the items side by side (nothing may need to change).
    Compare,
    /// Open the item(s).
    OpenItem,
    /// Edit the items' emojis.
    EditEmojis,
    /// Confirm or edit the AI-filled fields on the item pages.
    ReviewAi,
    /// Look for a better copy of the picture (`GET /items/{id}/better`).
    FindBetter,
    /// Publish the set to Telegram (again).
    PublishTelegram,
    /// Move items into another set.
    SplitSet,
    /// Pull the git pack again.
    PullGit,
    /// Upload items.
    AddItems,
}

/// An item a [`Problem`] is about.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ProblemItem {
    pub item_id: String,
    /// Empty when `hidden`.
    pub name: String,
    pub kind: MediaKind,
    pub width: u32,
    pub height: u32,
    /// What is wrong with this item in particular, e.g. `"64×64 px"` or the
    /// entries that are not emoji. `None` when `hidden`.
    pub detail: Option<String>,
    /// The caller cannot read the item (an admin looking at another user's
    /// private set): name, detail and files are withheld.
    #[serde(default)]
    pub hidden: bool,
}

/// One problem of a set.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Problem {
    /// `<kind>-<16 hex>`: the kind plus a hash of what makes it a problem
    /// (the affected items and their media, emojis, ...). Stable while
    /// nothing relevant changes; dismissals are keyed by it.
    pub id: String,
    pub kind: ProblemKind,
    pub severity: ProblemSeverity,
    /// Short human-readable description (never contains item names).
    pub message: String,
    pub set_id: String,
    /// Affected items in set order (pairs: the earlier item first).
    pub items: Vec<ProblemItem>,
    pub action: Option<ProblemAction>,
    /// Kind-specific measure: Hamming distance of the perceptual hashes for
    /// `near_duplicate`, cosine similarity for `similar`.
    pub score: Option<f32>,
    /// Marked as "not a problem" by the set's owner.
    pub dismissed: bool,
}

/// Problems by severity. `dismissed` counts the dismissed ones separately
/// (they are not in the other numbers).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ProblemCounts {
    pub error: u32,
    pub warning: u32,
    pub info: u32,
    pub dismissed: u32,
}

impl ProblemCounts {
    /// Problems that are not dismissed.
    pub fn open(&self) -> u32 {
        self.error + self.warning + self.info
    }
}

/// The problems of one set.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SetProblems {
    /// For other users' private sets (admins, server scope) the description
    /// is empty and there is no cover.
    pub set: SetSummary,
    pub owner_id: String,
    /// Worst first; filtered by the request (`severity`, `kind`, `dismissed`).
    pub problems: Vec<Problem>,
    /// Of every problem of the set, regardless of the filters.
    pub counts: ProblemCounts,
    /// The caller may dismiss problems of this set (its owner).
    pub can_dismiss: bool,
    /// The caller may change the set's items (its owner, unless the set is a
    /// read-only git pack): clients offer deleting duplicates, editing emojis.
    #[serde(default)]
    pub can_edit: bool,
}

/// `GET /api/v1/problems`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ProblemReport {
    /// `mine` or `server`.
    pub scope: String,
    /// Sets with at least one problem left after the filters, worst first.
    pub sets: Vec<SetProblems>,
    /// Totals over every set in scope, regardless of the filters.
    pub counts: ProblemCounts,
    /// How many sets were checked.
    pub sets_checked: u32,
    /// AI embeddings were available, so `similar` hints could be made.
    pub similar_available: bool,
    pub generated_at: i64,
}

// ---------------------------------------------------------------------------

/// serde helper: distinguishes "absent" (None) from "null" (Some(None)).
pub mod double_option {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<T: Serialize, S: Serializer>(v: &Option<Option<T>>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            None => s.serialize_none(),
            Some(inner) => inner.serialize(s),
        }
    }

    pub fn deserialize<'de, T: Deserialize<'de>, D: Deserializer<'de>>(d: D) -> Result<Option<Option<T>>, D::Error> {
        Option::<T>::deserialize(d).map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_item_distinguishes_absent_and_null() {
        let absent: UpdateItem = serde_json::from_str("{}").unwrap();
        assert_eq!(absent.vibe, None);
        let null: UpdateItem = serde_json::from_str(r#"{"vibe": null}"#).unwrap();
        assert_eq!(null.vibe, Some(None));
        let set: UpdateItem = serde_json::from_str(r#"{"vibe": "tired"}"#).unwrap();
        assert_eq!(set.vibe, Some(Some("tired".into())));
    }

    #[test]
    fn variant_names_round_trip() {
        for v in Variant::ALL {
            assert_eq!(Variant::parse(v.as_str()), Some(v));
            let json = serde_json::to_string(&v).unwrap();
            assert_eq!(json, format!("\"{}\"", v.as_str()));
        }
    }

    #[test]
    fn problem_names_round_trip() {
        for k in ProblemKind::ALL {
            assert_eq!(ProblemKind::parse(k.as_str()), Some(k));
            assert_eq!(serde_json::to_string(&k).unwrap(), format!("\"{}\"", k.as_str()));
        }
        for s in ProblemSeverity::ALL {
            assert_eq!(ProblemSeverity::parse(s.as_str()), Some(s));
            assert_eq!(serde_json::to_string(&s).unwrap(), format!("\"{}\"", s.as_str()));
        }
        assert!(ProblemSeverity::Error < ProblemSeverity::Info);
    }

    #[test]
    fn target_names_round_trip() {
        for t in TargetApp::ALL {
            assert_eq!(TargetApp::parse(t.as_str()), Some(t));
            assert_eq!(serde_json::to_string(&t).unwrap(), format!("\"{}\"", t.as_str()));
        }
    }
}
