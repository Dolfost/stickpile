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

/// Default of [`Item::text_source`] when a server leaves it out: an empty
/// AI text means "not set" (see `text`).
fn ai_source() -> FieldSource {
    FieldSource::Ai
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
    /// Build the WhatsApp sticker packs (`.wastickers`) of a set for download.
    WhatsappExport,
    /// Import a Discord server's stickers and/or emoji into a new set.
    DiscordImport,
    /// Publish a set to a Discord server as stickers or emoji.
    DiscordExport,
    /// Import a Signal sticker pack from its link into a new set.
    SignalImport,
    /// Upload a set as a new Signal sticker pack.
    SignalExport,
    /// Recompute the parts of one picture a [`Recheck`] asks for. Readers
    /// older than the kind (0.7.0) see [`JobKind::Other`].
    Recheck,
    /// A kind this client does not know (added by a newer server), so a
    /// job list still reads. A server never makes one.
    #[serde(other)]
    Other,
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
    /// The language chosen for the web UI and the clients that follow the
    /// account (`en`, `uk`); `None`: automatic (the browser's or the
    /// system's language).
    #[serde(default)]
    pub language: Option<String>,
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
    /// UI language (`en`, `uk`); `null` goes back to automatic.
    #[serde(default, with = "double_option", skip_serializing_if = "Option::is_none")]
    pub language: Option<Option<String>>,
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

/// A new one-time pairing code ([`crate::routes::PAIRINGS`]). Returned once:
/// the server keeps only a hash of the code.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PairingCreated {
    /// The code in its display form, e.g. `7KQ2-M9XA-4FTR`: 12 characters of
    /// Crockford's base32 in groups of four ([`crate::pairing`]).
    pub code: String,
    /// When the code stops working, unix seconds (five minutes after creation).
    pub expires_at: i64,
    /// `stickpile://pair?url=<server's public URL>&code=<code>`, both values
    /// percent-encoded: the content of the QR code.
    pub pairing_url: String,
}

/// Exchange a pairing code for an API token ([`crate::routes::AUTH_PAIR`]).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PairRequest {
    /// The code as shown or typed: case, spaces and dashes do not matter, and
    /// `O`, `I`, `L` read as `0`, `1`, `1`.
    pub code: String,
    /// Shown in the token list, e.g. "Pixel 8".
    pub token_name: String,
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
    /// True if the caller may change the set's settings (owner, and not a
    /// read-only git pack). Contributors check [`Set::permissions`].
    pub editable: bool,
    /// The caller's relation to the set.
    #[serde(default)]
    pub role: SetRole,
    /// What the caller may do in the set: everything for its owner, the
    /// granted permissions for a contributor, nothing otherwise. All false
    /// for a read-only git pack.
    #[serde(default)]
    pub permissions: Permissions,
    pub created_at: i64,
    pub updated_at: i64,
}

/// A user's relation to a set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum SetRole {
    Owner,
    /// Added by the owner with some [`Permissions`].
    Contributor,
    /// Anyone else who can see the set.
    #[default]
    Viewer,
}

/// What a contributor may do in a set (the owner may do all of it and
/// more: settings, visibility, contributors, deleting the set).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(default)]
pub struct Permissions {
    /// Add stickers (upload, or move one in from another set).
    pub add: bool,
    /// Edit and remove the stickers they added.
    pub edit_own: bool,
    /// Edit everyone's stickers (the owner's and other contributors').
    pub edit_all: bool,
    /// Remove everyone's stickers.
    pub remove_all: bool,
    /// Edit the set's name, description, cover and order.
    pub edit_pack: bool,
    /// Accept or decline proposals for the set.
    pub review: bool,
}

impl Permissions {
    pub const ALL: Permissions = Permissions {
        add: true,
        edit_own: true,
        edit_all: true,
        remove_all: true,
        edit_pack: true,
        review: true,
    };
    pub const NONE: Permissions = Permissions {
        add: false,
        edit_own: false,
        edit_all: false,
        remove_all: false,
        edit_pack: false,
        review: false,
    };
    /// What a newly added contributor gets unless the owner says otherwise.
    pub const CONTRIBUTOR_DEFAULT: Permissions = Permissions {
        add: true,
        edit_own: true,
        edit_all: false,
        remove_all: false,
        edit_pack: false,
        review: false,
    };
    /// Any permission at all.
    pub fn any(&self) -> bool {
        *self != Permissions::NONE
    }
}

/// A contributor of a set, as its owner and contributors see them.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Member {
    pub username: String,
    pub display_name: String,
    pub permissions: Permissions,
    pub added_at: i64,
}

/// Body of PUT [`crate::routes::SET_MEMBER`]: add a contributor or change
/// their permissions.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PutMember {
    pub permissions: Permissions,
}

/// What a proposal asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ProposalKind {
    /// New stickers for the set ([`Proposal::items`]).
    AddItems,
    /// Changes to one sticker: name, keywords, emojis, tags, vibe.
    EditItem,
    /// Changes to the set: name, description, cover.
    EditSet,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ProposalStatus {
    /// Waiting for the owner or a reviewer.
    Open,
    /// Accepted: the change was made.
    Accepted,
    Declined,
    /// Taken back by the proposer.
    Withdrawn,
}

/// Field changes of an [`ProposalKind::EditItem`] or
/// [`ProposalKind::EditSet`] proposal; absent fields stay as they are.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(default)]
pub struct ProposedChanges {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Set description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keywords: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emojis: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    /// `Some(None)` clears the vibe.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "double_option")]
    pub vibe: Option<Option<String>>,
    /// The words in the sticker (`Some("")` clears them); accepted, it is
    /// the owner's text (`text_source: user`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// An item of the set as its cover.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cover_item_id: Option<String>,
}

impl ProposedChanges {
    pub fn is_empty(&self) -> bool {
        *self == ProposedChanges::default()
    }
}

/// A sticker proposed for a set.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ProposedItem {
    pub name: String,
    pub keywords: Vec<String>,
    pub emojis: Vec<String>,
    pub tags: Vec<String>,
    pub vibe: Option<String>,
    /// The words in the picture as the proposer gave them (`text` in the
    /// proposal's `meta`). Empty when they gave none (the server reads the
    /// picture once it is accepted) or an empty one, and from a server
    /// before item texts (0.7.0).
    #[serde(default)]
    pub text: String,
    pub kind: MediaKind,
    pub format: MediaFormat,
    pub width: u32,
    pub height: u32,
    /// URL path of its thumbnail while the proposal is open.
    pub thumb_url: Option<String>,
    /// The item it became once accepted.
    pub item_id: Option<String>,
}

/// A request to change someone's set, from its sender's or its reviewers' side.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Proposal {
    pub id: String,
    pub kind: ProposalKind,
    pub status: ProposalStatus,
    pub set: SetSummary,
    /// The sticker an [`ProposalKind::EditItem`] proposal changes; `None`
    /// once it was removed.
    pub item: Option<ItemSummary>,
    pub proposer_username: String,
    /// The proposer's note.
    pub message: String,
    /// [`ProposalKind::AddItems`]: the stickers, in order.
    pub items: Vec<ProposedItem>,
    pub changes: ProposedChanges,
    /// The values `changes` replace, as they were when it was proposed.
    pub before: ProposedChanges,
    /// URL path of a proposed cover image (thumbnail) while it is open.
    pub cover_image_url: Option<String>,
    pub reviewer_username: Option<String>,
    /// The reviewer's note.
    pub review_note: String,
    pub created_at: i64,
    pub decided_at: Option<i64>,
    /// The caller may accept or decline it.
    pub can_decide: bool,
    /// The caller may withdraw it (its proposer, while open).
    pub can_withdraw: bool,
}

/// Body of POST [`crate::routes::ITEM_PROPOSALS`] and
/// [`crate::routes::SET_PROPOSALS`].
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct NewProposal {
    #[serde(default)]
    pub message: String,
    #[serde(flatten)]
    pub changes: ProposedChanges,
}

/// Body of POST [`crate::routes::PROPOSAL_ACCEPT`] / [`crate::routes::PROPOSAL_DECLINE`].
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DecideProposal {
    #[serde(default)]
    pub note: String,
}

/// `box` of GET [`crate::routes::PROPOSALS`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ProposalBox {
    /// Proposals for the sets the caller owns or reviews.
    #[default]
    Incoming,
    /// The caller's own proposals.
    Outgoing,
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

/// `POST /api/v1/discord/import`: a Discord server's stickers and/or emoji
/// into a new set of the caller.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DiscordImport {
    /// The server ("guild") id, a number like `100000000000000001`.
    pub guild_id: String,
    /// Import the server's stickers (default true).
    #[serde(default = "yes")]
    pub stickers: bool,
    /// Import the server's emoji (default false).
    #[serde(default)]
    pub emojis: bool,
    /// Slug for the new set; default derived from the server's name.
    pub slug: Option<String>,
    #[serde(default)]
    pub visibility: Visibility,
}

fn yes() -> bool {
    true
}

/// `options.discord` of a [`PublishRequest`]: where and as what to publish
/// a set on Discord. Both are strings, as the export page and the CLI send
/// them.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DiscordExport {
    /// The server ("guild") id; default the one this set was published to
    /// last. Required the first time.
    #[serde(default, alias = "guild")]
    pub guild_id: Option<String>,
    /// `stickers` (default) or `emojis`.
    #[serde(default)]
    pub kind: Option<String>,
}

/// `POST /api/v1/signal/import`: a Signal sticker pack into a new set of
/// the caller.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SignalImport {
    /// `https://signal.art/addstickers/#pack_id=...&pack_key=...` or
    /// `sgnl://addstickers/?pack_id=...&pack_key=...`. The key in it opens
    /// the pack: the server uses it for the import only and does not keep
    /// it.
    pub link: String,
    /// Slug for the new set; default derived from the pack's title.
    pub slug: Option<String>,
    #[serde(default)]
    pub visibility: Visibility,
}

// ---------------------------------------------------------------------------
// Publishing to several platforms, WhatsApp packs
// ---------------------------------------------------------------------------

/// `POST /api/v1/sets/{id}/publish`: queue one export job per platform.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PublishRequest {
    /// Platform ids ([`Publication::platform`]): `telegram`, `discord`,
    /// `signal`. Unknown or unconfigured ones fail the whole request (400).
    pub platforms: Vec<String>,
    /// Options per platform id, e.g. `{"telegram": {"pack_name": "cats",
    /// "title": "Cats"}, "discord": {"guild_id": "1000", "kind": "emojis"}}`
    /// (Telegram's are [`TelegramExport`], Discord's [`DiscordExport`];
    /// Signal takes none). A platform without an entry uses its defaults.
    #[serde(default)]
    pub options: std::collections::BTreeMap<String, serde_json::Value>,
}

/// The jobs [`PublishRequest`] queued, in the order of its `platforms`.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PublishJobs {
    pub jobs: Vec<Job>,
}

/// Where and how a set is published on one platform
/// (`GET /api/v1/sets/{id}/publish`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Publication {
    /// Stable id used in the API: `telegram`.
    pub platform: String,
    /// Name for people: `Telegram`.
    pub name: String,
    /// The platform is set up on this server; only configured platforms
    /// can be published to.
    pub configured: bool,
    /// What the set is published as (Telegram: the pack name), if it is.
    pub target: Option<String>,
    /// Link to the published pack.
    pub url: Option<String>,
    /// When the last successful publish finished.
    pub published_at: Option<i64>,
    /// The last finished publish: `done` or `failed`.
    pub last_status: Option<JobStatus>,
    pub last_finished_at: Option<i64>,
    /// Why the last publish failed.
    pub last_error: Option<String>,
    /// The result of the last successful publish (the job's `result`).
    pub last_result: Option<serde_json::Value>,
    /// A publish of this set to the platform that is queued or running.
    pub job: Option<Job>,
    /// Every pack the set is published as, when the platform splits a set
    /// larger than one pack into parts (Telegram 120, Signal 200 stickers
    /// per pack); part 1 is `target`/`url`. Empty for one pack and from
    /// servers older than parts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<PublishedPart>,
}

/// One pack of a set published as several (see [`Publication::parts`]).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PublishedPart {
    /// 1 for the first pack.
    pub number: u32,
    /// The pack's name on the platform (Telegram: `<name>_<k>_by_<bot>`).
    pub name: String,
    /// Link to the pack.
    pub url: Option<String>,
    /// How many stickers the pack holds, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stickers: Option<u32>,
}

/// An item left out of an export, and why.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ExportSkip {
    pub item_id: String,
    pub name: String,
    pub reason: String,
}

/// One sticker of a WhatsApp pack.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct WhatsappSticker {
    pub item_id: String,
    pub name: String,
    /// File name inside the `.wastickers` archive (`01.webp`).
    pub file: String,
    /// 1 to 3 emoji (`🙂` when the item has none).
    pub emojis: Vec<String>,
    pub animated: bool,
    pub size_bytes: u64,
    /// What was changed to fit WhatsApp, e.g. an animation sent as a still.
    #[serde(default)]
    pub note: Option<String>,
}

/// One WhatsApp sticker pack: a `.wastickers` file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct WhatsappPack {
    /// 1-based; packs are numbered in the order of the set.
    pub number: u32,
    /// `contents.json` identifier: `<slug>` or `<slug>-<number>`.
    pub identifier: String,
    /// Pack name (at most 128 characters).
    pub title: String,
    /// Publisher (at most 128 characters): the set owner's display name.
    pub publisher: String,
    /// WhatsApp packs are either all static or all animated.
    pub animated: bool,
    /// File name of the pack's `.wastickers`.
    pub file: String,
    /// Download path of the `.wastickers` alone.
    pub url: String,
    /// The tray icon inside the archive (96x96 PNG).
    pub tray_file: String,
    /// Changes whenever the pack's stickers or tray icon change
    /// (`image_data_version` of `contents.json`).
    pub image_data_version: String,
    pub size_bytes: u64,
    pub stickers: Vec<WhatsappSticker>,
}

/// A finished WhatsApp build of a set.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct WhatsappBuild {
    pub built_at: i64,
    /// The set's `version` when it was built.
    pub set_version: u64,
    /// File name of the download (a zip of every `.wastickers` plus
    /// `contents.json`).
    pub file: String,
    /// Download path of that zip.
    pub url: String,
    pub size_bytes: u64,
    pub packs: Vec<WhatsappPack>,
    /// Items that could not become WhatsApp stickers.
    pub skipped: Vec<ExportSkip>,
}

/// `GET /api/v1/sets/{id}/whatsapp`: the last build and whether it is
/// still up to date.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct WhatsappStatus {
    /// The last finished build, if there is one.
    pub build: Option<WhatsappBuild>,
    /// The build matches the set as it is now (same items, order, emoji,
    /// name and cover).
    pub current: bool,
    /// The caller's build of this set that is queued or running.
    pub job: Option<Job>,
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
    /// The words written in the picture: one string, line breaks kept, at
    /// most [`crate::text::MAX_TEXT_CHARS`] characters. Read by OCR
    /// (`text_source: ai`) or written by a person. Empty from an older server.
    #[serde(default)]
    pub text: String,
    /// An empty `ai` text means "not set": the computer fills it. Any edit by
    /// a person, clearing included, makes it `user`, and the computer never
    /// writes it again.
    #[serde(default = "ai_source")]
    pub text_source: FieldSource,
    /// Who added it to the set (`None`: a removed account, or an older server).
    #[serde(default)]
    pub added_by_username: Option<String>,
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
    /// [`Item::text`]; empty from an older server (and in client caches it
    /// wrote, which clients sync again in full once).
    #[serde(default)]
    pub text: String,
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
    /// The words in the picture ([`Item::text`]). Given (even empty), it is
    /// the user's unless `ai_fields` lists `text`; absent, OCR fills it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Given fields that hold AI suggestions the user has not approved (from
    /// `POST /api/v1/ai/suggest`): any of `keywords`, `emojis`, `vibe`,
    /// `text`. They are stored with `*_source: ai`, like enrichment results;
    /// every other given field is the user's.
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
    /// The words read in the image (OCR), lines joined with `\n`; `None`
    /// when the server has no text reader (or an older server), `Some("")`
    /// when it read no confident text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
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
    /// `Some(None)` clears the vibe; `None` is left out when sent, so a
    /// PATCH without it keeps the vibe.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "double_option")]
    pub vibe: Option<Option<String>>,
    pub tags: Option<Vec<String>>,
    /// The words in the picture; any value, `""` included, makes them the
    /// user's (`text_source: user`), so the computer never fills them again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Mark AI values as confirmed without changing them:
    /// any of `name`, `keywords`, `emojis`, `vibe`, `text`.
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
    /// `text` (the words in the picture), `fuzzy`, `image` (text->image
    /// embedding), `semantic` (text->keywords embedding).
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

/// The values offered while a `key:value` filter is typed (`vibe:sar`,
/// `e:sk`), as the popups offer them under their search field
/// (`GET /api/v1/search/suggest`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SearchSuggestions {
    /// The filter as typed, with its colon and a leading `-`: `vibe:`,
    /// `-tag:`, `e:`.
    pub key: String,
    /// The value typed so far (up to the cursor).
    pub typed: String,
    /// Byte offset in the query where the value starts: taking a word puts
    /// it in place of `typed`.
    pub start: u64,
    /// Best first. For `e:` emojis: taking one puts the emoji in, also
    /// while its name is typed (`e:sk` -> `e:💀`).
    pub words: Vec<String>,
    /// Parallel to `words`: for `e:` each emoji's English name (`skull`
    /// for 💀), else empty.
    pub labels: Vec<String>,
    /// Index in `words` of the value a half-typed or misspelled filter
    /// stands for; none while nothing is typed and for an emoji name,
    /// which the search matches as typed.
    pub chosen: Option<u32>,
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
// Rechecks
// ---------------------------------------------------------------------------

/// What a [`Recheck`] recomputes. Each is chosen on its own; none ever
/// changes a value a person gave (docs/ai.md, "Recheck").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum RecheckPart {
    /// The items' AI keywords (CLIP zero-shot).
    Keywords,
    /// The items' AI emojis (CLIP zero-shot).
    Emojis,
    /// The items' AI vibe (CLIP zero-shot).
    Vibe,
    /// The items' AI text: the words in the picture (OCR).
    Text,
    /// The picture's CLIP embedding (semantic search).
    Embedding,
    /// The picture's quality score and report.
    Quality,
    /// The picture's renditions for the chat apps (variants).
    Renditions,
    /// A part this reader does not know (added by a newer peer).
    #[serde(other)]
    Other,
}

impl RecheckPart {
    /// Every part, in this order.
    pub const ALL: [RecheckPart; 7] = [
        RecheckPart::Keywords,
        RecheckPart::Emojis,
        RecheckPart::Vibe,
        RecheckPart::Text,
        RecheckPart::Embedding,
        RecheckPart::Quality,
        RecheckPart::Renditions,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            RecheckPart::Keywords => "keywords",
            RecheckPart::Emojis => "emojis",
            RecheckPart::Vibe => "vibe",
            RecheckPart::Text => "text",
            RecheckPart::Embedding => "embedding",
            RecheckPart::Quality => "quality",
            RecheckPart::Renditions => "renditions",
            RecheckPart::Other => "other",
        }
    }

    pub fn parse(s: &str) -> Option<RecheckPart> {
        RecheckPart::ALL.into_iter().find(|p| p.as_str() == s.trim())
    }

    /// Needs the CLIP model (the embedding and what is derived from it).
    pub fn needs_clip(self) -> bool {
        matches!(
            self,
            RecheckPart::Keywords | RecheckPart::Emojis | RecheckPart::Vibe | RecheckPart::Embedding
        )
    }
}

/// How a [`Recheck`] treats what is already there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum RecheckMode {
    /// Compute only what is not there: no embedding or reading from the
    /// current model, no quality report, renditions missing or outdated,
    /// an empty field.
    #[default]
    Missing,
    /// Compute every chosen part again and replace the computer's values.
    Refresh,
    /// A mode this reader does not know.
    #[serde(other)]
    Other,
}

impl RecheckMode {
    pub fn as_str(self) -> &'static str {
        match self {
            RecheckMode::Missing => "missing",
            RecheckMode::Refresh => "refresh",
            RecheckMode::Other => "other",
        }
    }
}

/// Whose items a [`Recheck`] covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum RecheckScope {
    /// The items of the sets one user owns.
    User,
    /// Every item on the server (admins, or the server itself).
    Server,
    /// A scope this reader does not know.
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum RecheckState {
    Running,
    /// Every picture was rechecked (some may have failed: `failed`).
    Done,
    /// Stopped by a person; `done` and `failed` count what ran before.
    Cancelled,
    /// Every picture failed.
    Failed,
    /// A state this reader does not know.
    #[serde(other)]
    Other,
}

/// `POST /api/v1/rechecks` and `POST /api/v1/admin/rechecks`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct StartRecheck {
    /// The parts to recompute; empty: every part this server can recompute
    /// now (no CLIP parts without a model, no `text` without a reader).
    #[serde(default)]
    pub parts: Vec<RecheckPart>,
    #[serde(default)]
    pub mode: RecheckMode,
    /// Only this set (one of the caller's own for `POST /api/v1/rechecks`).
    #[serde(default)]
    pub set_id: Option<String>,
}

/// A batch that recomputes metadata of many pictures in the background
/// (one `recheck` job per picture). The counters count pictures.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Recheck {
    pub id: String,
    pub scope: RecheckScope,
    /// Sorted as in [`RecheckPart::ALL`].
    pub parts: Vec<RecheckPart>,
    pub mode: RecheckMode,
    #[serde(default)]
    pub set_id: Option<String>,
    /// Who started it; `None`: the server itself (after a model change, or
    /// for pictures its text reader never read).
    #[serde(default)]
    pub requested_by: Option<String>,
    pub state: RecheckState,
    /// Pictures in the batch.
    #[serde(default)]
    pub total: u64,
    /// Pictures rechecked.
    #[serde(default)]
    pub done: u64,
    /// Pictures whose recheck failed (`GET /api/v1/rechecks/{id}/failures`).
    #[serde(default)]
    pub failed: u64,
    /// Pictures where something was written.
    #[serde(default)]
    pub changed: u64,
    pub created_at: i64,
    #[serde(default)]
    pub finished_at: Option<i64>,
}

/// One picture whose recheck failed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RecheckFailure {
    pub media_id: String,
    /// One of the batch owner's items showing the picture (user scope).
    #[serde(default)]
    pub item_id: Option<String>,
    pub error: String,
    pub at: i64,
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
        // And back: a request that does not change the vibe does not send
        // it (a `null` would clear it), one that clears it sends `null`.
        let text_only = UpdateItem {
            text: Some("hi".into()),
            ..Default::default()
        };
        let v = serde_json::to_value(&text_only).unwrap();
        assert!(v.get("vibe").is_none(), "{v}");
        let back: UpdateItem = serde_json::from_value(v).unwrap();
        assert_eq!(back.vibe, None);
        let clear = UpdateItem {
            vibe: Some(None),
            ..Default::default()
        };
        assert_eq!(serde_json::to_value(&clear).unwrap()["vibe"], serde_json::Value::Null);
    }

    #[test]
    fn an_unknown_job_kind_reads_as_other() {
        let kind: JobKind = serde_json::from_str("\"git_push\"").unwrap();
        assert_eq!(kind, JobKind::GitPush);
        let kind: JobKind = serde_json::from_str("\"made_up_later\"").unwrap();
        assert_eq!(kind, JobKind::Other);
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

    /// The types the text field touched, as stickpile 0.7.0 had them (no
    /// `text`, no `text_source`), to check both directions of rule 5.
    mod v070 {
        use super::super::{FieldSource, MediaFormat, MediaInfo, MediaKind, VariantInfo};
        use serde::Deserialize;

        #[derive(Debug, Deserialize)]
        #[allow(dead_code)]
        pub struct Item {
            pub id: String,
            pub set_id: String,
            pub position: u32,
            pub filename: String,
            pub name: String,
            pub name_source: FieldSource,
            pub keywords: Vec<String>,
            pub keywords_source: FieldSource,
            pub emojis: Vec<String>,
            pub emojis_source: FieldSource,
            pub vibe: Option<String>,
            pub vibe_source: FieldSource,
            pub tags: Vec<String>,
            #[serde(default)]
            pub added_by_username: Option<String>,
            pub media: MediaInfo,
            pub likes: u64,
            pub liked_by_me: bool,
            pub copies: u64,
            pub created_at: i64,
            pub updated_at: i64,
        }

        #[derive(Debug, Deserialize)]
        #[allow(dead_code)]
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

        #[derive(Debug, Deserialize)]
        #[allow(dead_code)]
        pub struct AiSuggestions {
            pub keywords: Vec<String>,
            pub emojis: Vec<String>,
            pub vibe: Option<String>,
        }

        #[derive(Debug, Deserialize)]
        #[allow(dead_code)]
        pub struct UpdateItem {
            pub name: Option<String>,
            pub keywords: Option<Vec<String>>,
            #[serde(default)]
            pub confirm: Vec<String>,
        }

        #[derive(Debug, Deserialize)]
        #[allow(dead_code)]
        pub struct UploadMeta {
            pub name: Option<String>,
            #[serde(default)]
            pub ai_fields: Vec<String>,
        }

        #[derive(Debug, Default, Deserialize)]
        #[serde(default)]
        #[allow(dead_code)]
        pub struct ProposedChanges {
            pub name: Option<String>,
            pub keywords: Option<Vec<String>>,
        }

        /// `JobKind` of 0.7.0: no `Recheck`.
        #[derive(Debug, PartialEq, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum JobKind {
            Variants,
            Enrich,
            GitPull,
            GitPush,
            TelegramImport,
            TelegramExport,
            WhatsappExport,
            DiscordImport,
            DiscordExport,
            SignalImport,
            SignalExport,
            #[serde(other)]
            Other,
        }

        #[derive(Debug, Deserialize)]
        #[allow(dead_code)]
        pub struct Job {
            pub id: String,
            pub kind: JobKind,
            pub status: super::super::JobStatus,
            pub attempts: u32,
            pub error: Option<String>,
            pub subject: Option<String>,
            pub result: Option<serde_json::Value>,
            pub created_at: i64,
            pub started_at: Option<i64>,
            pub finished_at: Option<i64>,
        }
    }

    /// A job list with a recheck job, as a newer server sends it, reads on
    /// 0.7.0: the recheck job is `Other`, the others keep their kinds.
    #[test]
    fn an_older_client_reads_a_recheck_job_as_other() {
        let job = |id: &str, kind: JobKind| Job {
            id: id.into(),
            kind,
            status: JobStatus::Queued,
            attempts: 0,
            error: None,
            subject: Some("m".into()),
            result: None,
            created_at: 1,
            started_at: None,
            finished_at: None,
        };
        let list = serde_json::to_value(vec![job("a", JobKind::Variants), job("b", JobKind::Recheck)]).unwrap();
        assert_eq!(list[1]["kind"], "recheck");
        let old: Vec<v070::Job> = serde_json::from_value(list).unwrap();
        assert_eq!(old[0].kind, v070::JobKind::Variants);
        assert_eq!(old[1].kind, v070::JobKind::Other);
        assert_eq!(old[1].id, "b");
    }

    /// Recheck types read values a newer peer may add, and leave out what
    /// they can.
    #[test]
    fn recheck_types_read_newer_values() {
        for p in RecheckPart::ALL {
            assert_eq!(RecheckPart::parse(p.as_str()), Some(p));
            assert_eq!(serde_json::to_string(&p).unwrap(), format!("\"{}\"", p.as_str()));
        }
        assert_eq!(RecheckPart::parse("bogus"), None);
        let r: StartRecheck = serde_json::from_str(r#"{"parts": ["keywords", "colour"], "mode": "deep"}"#).unwrap();
        assert_eq!(r.parts, [RecheckPart::Keywords, RecheckPart::Other]);
        assert_eq!(r.mode, RecheckMode::Other);
        let r: StartRecheck = serde_json::from_str("{}").unwrap();
        assert_eq!(r, StartRecheck::default());
        assert_eq!(r.mode, RecheckMode::Missing);
        let b: Recheck = serde_json::from_str(
            r#"{"id": "r", "scope": "galaxy", "parts": ["text"], "mode": "refresh", "state": "paused",
                "created_at": 5, "priority": 3}"#,
        )
        .unwrap();
        assert_eq!(
            (b.scope, b.state, b.total, b.set_id, b.requested_by),
            (RecheckScope::Other, RecheckState::Other, 0, None, None)
        );
    }

    fn sample_item() -> Item {
        Item {
            id: "i".into(),
            set_id: "s".into(),
            position: 0,
            filename: "cat".into(),
            name: "cat".into(),
            name_source: FieldSource::User,
            keywords: vec!["cat".into()],
            keywords_source: FieldSource::Ai,
            emojis: vec![],
            emojis_source: FieldSource::User,
            vibe: None,
            vibe_source: FieldSource::User,
            tags: vec![],
            text: "no thoughts\nhead empty".into(),
            text_source: FieldSource::Ai,
            added_by_username: None,
            media: MediaInfo {
                id: "m".into(),
                sha256: "00".into(),
                kind: MediaKind::Still,
                format: MediaFormat::Png,
                width: 1,
                height: 1,
                duration_ms: None,
                frame_count: None,
                fps: None,
                has_alpha: false,
                size_bytes: 1,
                phash: "0".into(),
                dhash: "0".into(),
                quality: None,
                variants: vec![],
            },
            likes: 0,
            liked_by_me: false,
            copies: 0,
            created_at: 0,
            updated_at: 0,
        }
    }

    fn sample_summary() -> ItemSummary {
        ItemSummary {
            id: "i".into(),
            set_id: "s".into(),
            set_slug: "cats".into(),
            set_name: "Cats".into(),
            owner_username: "alice".into(),
            name: "cat".into(),
            keywords: vec![],
            emojis: vec![],
            vibe: None,
            tags: vec![],
            text: "Кіт".into(),
            kind: MediaKind::Still,
            format: MediaFormat::Png,
            width: 1,
            height: 1,
            quality: None,
            likes: 0,
            copies: 0,
            media_id: "m".into(),
            sha256: "00".into(),
            phash: "0".into(),
            variants: vec![],
            updated_at: 0,
        }
    }

    /// Without the keys a 0.7.0 server leaves out.
    fn without(v: impl Serialize, keys: &[&str]) -> serde_json::Value {
        let mut v = serde_json::to_value(v).unwrap();
        for k in keys {
            assert!(v.as_object_mut().unwrap().remove(*k).is_some(), "{k} was there");
        }
        v
    }

    #[test]
    fn answers_of_an_older_server_parse_without_text() {
        let item: Item = serde_json::from_value(without(sample_item(), &["text", "text_source"])).unwrap();
        assert_eq!(item.text, "");
        assert_eq!(item.text_source, FieldSource::Ai, "empty AI text: not set");
        let summary: ItemSummary = serde_json::from_value(without(sample_summary(), &["text"])).unwrap();
        assert_eq!(summary.text, "");
        let s: AiSuggestions = serde_json::from_str(r#"{"keywords": ["cat"], "emojis": [], "vibe": null}"#).unwrap();
        assert_eq!(s.text, None);
        let u: UpdateItem = serde_json::from_str(r#"{"name": "x", "confirm": ["name"]}"#).unwrap();
        assert_eq!(u.text, None);
        let m: UploadMeta = serde_json::from_str(r#"{"name": "x", "ai_fields": ["keywords"]}"#).unwrap();
        assert_eq!(m.text, None);
        let c: ProposedChanges = serde_json::from_str(r#"{"name": "x"}"#).unwrap();
        assert_eq!(c.text, None);
    }

    #[test]
    fn an_older_client_reads_text_and_ignores_it() {
        let old: v070::Item = serde_json::from_value(serde_json::to_value(sample_item()).unwrap()).unwrap();
        assert_eq!(old.name, "cat");
        let old: v070::ItemSummary = serde_json::from_value(serde_json::to_value(sample_summary()).unwrap()).unwrap();
        assert_eq!(old.set_slug, "cats");
        let s = AiSuggestions {
            keywords: vec!["cat".into()],
            text: Some("hi".into()),
            ..Default::default()
        };
        let old: v070::AiSuggestions = serde_json::from_value(serde_json::to_value(s).unwrap()).unwrap();
        assert_eq!(old.keywords, vec!["cat"]);
        // And an older server reads what a new client sends.
        let u = UpdateItem {
            name: Some("x".into()),
            text: Some(String::new()),
            confirm: vec!["text".into()],
            ..Default::default()
        };
        let old: v070::UpdateItem = serde_json::from_value(serde_json::to_value(u).unwrap()).unwrap();
        assert_eq!(old.name.as_deref(), Some("x"));
        let m = UploadMeta {
            name: Some("x".into()),
            text: Some("hi".into()),
            ..Default::default()
        };
        let old: v070::UploadMeta = serde_json::from_value(serde_json::to_value(m).unwrap()).unwrap();
        assert_eq!(old.name.as_deref(), Some("x"));
        let c = ProposedChanges {
            text: Some("hi".into()),
            ..Default::default()
        };
        let old: v070::ProposedChanges = serde_json::from_value(serde_json::to_value(c).unwrap()).unwrap();
        assert_eq!(old.name, None);
        // No DTO refuses unknown fields: that is what makes added fields safe.
        assert!(!include_str!("model.rs").contains(concat!("deny_unknown", "_fields")));
    }

    #[test]
    fn target_names_round_trip() {
        for t in TargetApp::ALL {
            assert_eq!(TargetApp::parse(t.as_str()), Some(t));
            assert_eq!(serde_json::to_string(&t).unwrap(), format!("\"{}\"", t.as_str()));
        }
    }
}
