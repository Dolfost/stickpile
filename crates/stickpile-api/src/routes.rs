//! REST paths. `{id}`-style segments use axum 0.8 syntax and are substituted by
//! clients with [`path`].
//!
//! Authentication for every `/api/v1` route except [`AUTH_TOKEN`],
//! [`AUTH_REGISTER`], [`OPENAPI`] and [`GIT_WEBHOOK`]: `Authorization: Bearer
//! <token>`, the tailnet identity from the trusted `tailscale serve` proxy, or
//! the web session cookie (cookie-authenticated non-GET requests additionally
//! need `X-CSRF-Token`). See docs/api.md.
//! Errors are returned as [`crate::model::ApiError`] JSON with a matching status.

pub const PREFIX: &str = "/api/v1";

pub const HEALTH: &str = "/healthz";
pub const OPENAPI: &str = "/api/v1/openapi.json";

// --- auth & tokens -----------------------------------------------------------
/// POST [`crate::model::TokenRequest`] -> [`crate::model::TokenCreated`]. Password (+TOTP) login for API clients.
pub const AUTH_TOKEN: &str = "/api/v1/auth/token";
/// POST [`crate::model::RegisterRequest`] -> [`crate::model::User`] (201). Unauthenticated;
/// obeys the server's registration policy (closed / invite / open).
pub const AUTH_REGISTER: &str = "/api/v1/auth/register";
/// GET -> [`crate::model::User`]; PATCH [`crate::model::UpdateMe`] -> User
pub const ME: &str = "/api/v1/me";
/// PUT [`crate::model::ChangePassword`] -> 204. Revokes the caller's other web sessions.
pub const ME_PASSWORD: &str = "/api/v1/me/password";
/// POST -> [`crate::model::TotpSetup`]: start TOTP enrolment (pending until confirmed).
pub const ME_TOTP: &str = "/api/v1/me/totp";
/// POST [`crate::model::TotpCode`] -> [`crate::model::User`]: confirm enrolment with a current code.
pub const ME_TOTP_CONFIRM: &str = "/api/v1/me/totp/confirm";
/// POST [`crate::model::TotpCode`] -> [`crate::model::User`]: disable TOTP (needs a current code).
pub const ME_TOTP_DISABLE: &str = "/api/v1/me/totp/disable";
/// GET -> `Vec<ApiToken>`; POST [`crate::model::CreateToken`] -> [`crate::model::TokenCreated`]
pub const TOKENS: &str = "/api/v1/tokens";
/// DELETE: revoke
pub const TOKEN: &str = "/api/v1/tokens/{id}";

// --- sets ----------------------------------------------------------------------
/// GET `?scope=mine|public|all` -> `Vec<SetSummary>`; POST [`crate::model::CreateSet`] -> [`crate::model::Set`]
pub const SETS: &str = "/api/v1/sets";
/// GET -> [`crate::model::SetDetail`]; PATCH [`crate::model::UpdateSet`] -> Set; DELETE
pub const SET: &str = "/api/v1/sets/{id}";
/// PUT [`crate::model::ReorderItems`]: full new order of item ids.
pub const SET_ORDER: &str = "/api/v1/sets/{id}/order";
/// POST multipart (one or more `file` parts, optional `meta` JSON part with
/// `Vec<UploadMeta>` matched by index) -> `Vec<UploadResult>`
pub const SET_ITEMS: &str = "/api/v1/sets/{id}/items";
/// GET -> `application/x-tar` of the set directory (set.toml + numbered files).
pub const SET_EXPORT: &str = "/api/v1/sets/{id}/export.tar";
/// PUT multipart (one file part: a still PNG, JPEG or WebP) -> [`crate::model::Set`]:
/// the set's own cover image, replacing an earlier one. DELETE -> Set: remove it.
pub const SET_COVER: &str = "/api/v1/sets/{id}/cover";
/// GET -> raw bytes of the set's cover image; `{variant}` is `original` or
/// `thumb` ([`crate::model::CoverImage`] has both URLs).
pub const SET_COVER_FILE: &str = "/api/v1/sets/{id}/cover/{variant}";
/// POST multipart: a `set.toml` part plus file parts whose filename is the
/// path relative to the set directory; or a single `archive` part with a tar.
/// Optional query `?into=<set id>` re-imports into an existing own set.
/// -> [`crate::model::ImportReport`]
pub const SETS_IMPORT: &str = "/api/v1/sets/import";
/// POST [`crate::model::CreateGitPack`] -> [`crate::model::GitPackCreated`] (201; the clone runs as a job)
pub const SETS_GITPACK: &str = "/api/v1/sets/gitpack";
/// POST -> [`crate::model::Job`]: re-pull now.
pub const SET_PULL: &str = "/api/v1/sets/{id}/pull";
/// POST, unauthenticated; `X-Stickpile-Webhook-Secret: <secret>`, GitLab
/// `X-Gitlab-Token`, or GitHub/Gitea/Forgejo `X-Hub-Signature-256` (HMAC of
/// the body). Enqueues a pull -> 202 [`crate::model::Job`].
pub const GIT_WEBHOOK: &str = "/api/v1/hooks/git/{id}";
/// POST [`crate::model::TelegramExport`] -> [`crate::model::Job`]
pub const SET_TELEGRAM_EXPORT: &str = "/api/v1/sets/{id}/telegram/export";
/// POST [`crate::model::TelegramImport`] -> [`crate::model::Job`]
pub const TELEGRAM_IMPORT: &str = "/api/v1/telegram/import";

// --- problems ------------------------------------------------------------------
/// GET `?severity=error|warning|info&kind=<ProblemKind>&dismissed=true` ->
/// [`crate::model::SetProblems`]. The set's owner (and admins, read-only).
pub const SET_PROBLEMS: &str = "/api/v1/sets/{id}/problems";
/// PUT: mark the problem as "not a problem" (204); DELETE: undo (204). Owner only.
pub const SET_PROBLEM_DISMISSAL: &str = "/api/v1/sets/{id}/problems/{problem}/dismissal";
/// GET -> `Vec<`[`crate::model::Member`]`>`: the set's contributors (owner and contributors).
pub const SET_MEMBERS: &str = "/api/v1/sets/{id}/members";
/// PUT [`crate::model::PutMember`] -> Member: add a contributor or change
/// their permissions (owner). DELETE: remove them (owner), or leave (the
/// contributor themself). `{username}` is the contributor's username.
pub const SET_MEMBER: &str = "/api/v1/sets/{id}/members/{username}";
/// POST [`crate::model::NewProposal`] (name, description, cover_item_id) ->
/// [`crate::model::Proposal`]: propose changes to someone else's set.
pub const SET_PROPOSALS: &str = "/api/v1/sets/{id}/proposals";
/// POST multipart, as [`SET_ITEMS`] plus an optional `message` part ->
/// Proposal: propose new stickers for someone else's set.
pub const SET_PROPOSAL_ITEMS: &str = "/api/v1/sets/{id}/proposals/items";
/// POST multipart (`file`: a still image, optional `message`) -> Proposal:
/// propose a cover image.
pub const SET_PROPOSAL_COVER: &str = "/api/v1/sets/{id}/proposals/cover";

// --- proposals ---------------------------------------------------------------------
/// GET `?box=incoming|outgoing&status=open|accepted|declined|withdrawn` ->
/// `Vec<`[`crate::model::Proposal`]`>`, newest first.
pub const PROPOSALS: &str = "/api/v1/proposals";
/// GET -> Proposal (its proposer, the set's owner and reviewers).
pub const PROPOSAL: &str = "/api/v1/proposals/{id}";
/// POST [`crate::model::DecideProposal`] -> Proposal: make the change (owner or reviewer).
pub const PROPOSAL_ACCEPT: &str = "/api/v1/proposals/{id}/accept";
/// POST [`crate::model::DecideProposal`] -> Proposal (owner or reviewer).
pub const PROPOSAL_DECLINE: &str = "/api/v1/proposals/{id}/decline";
/// POST -> Proposal (its proposer).
pub const PROPOSAL_WITHDRAW: &str = "/api/v1/proposals/{id}/withdraw";
/// GET -> thumbnail of a proposed sticker (`{n}`: its 0-based position) or,
/// with `{n}` = `cover`, of a proposed cover image, while the proposal is open.
pub const PROPOSAL_THUMB: &str = "/api/v1/proposals/{id}/thumb/{n}";

/// GET `?scope=mine|server&severity=&kind=&dismissed=` -> [`crate::model::ProblemReport`].
/// `server` (every set) is for admins only.
pub const PROBLEMS: &str = "/api/v1/problems";

// --- items -----------------------------------------------------------------------
/// GET `?updated_since=<unix>&cursor=<opaque>&limit=<n>` -> [`crate::model::ItemPage`].
/// Every item the caller may see in search (own, contributed to and public;
/// not unlisted), for client caches.
pub const ITEMS: &str = "/api/v1/items";
/// GET -> [`crate::model::ItemDetail`]; PATCH [`crate::model::UpdateItem`] -> Item; DELETE
pub const ITEM: &str = "/api/v1/items/{id}";
/// GET -> raw bytes of the variant (`Content-Type` from the variant).
/// `{variant}` is a [`crate::model::Variant`] in snake_case.
pub const ITEM_FILE: &str = "/api/v1/items/{id}/file/{variant}";
/// PUT: like; DELETE: unlike. -> [`crate::model::LikeState`]
pub const ITEM_LIKE: &str = "/api/v1/items/{id}/like";
/// GET -> `Vec<ItemSummary>`: same perceptual hash, higher quality, visible to caller.
pub const ITEM_BETTER: &str = "/api/v1/items/{id}/better";
/// POST -> [`crate::model::Job`]: re-run AI enrichment.
pub const ITEM_ENRICH: &str = "/api/v1/items/{id}/enrich";
/// POST [`crate::model::NewProposal`] (name, keywords, emojis, tags, vibe)
/// -> [`crate::model::Proposal`]: propose changes to someone else's sticker.
pub const ITEM_PROPOSALS: &str = "/api/v1/items/{id}/proposals";

// --- AI ----------------------------------------------------------------------------
/// POST multipart (one `file` part: a still image, at most 2 MiB; a small
/// rendering is enough) -> [`crate::model::AiSuggestions`]. Stores nothing.
/// 503 `ai_loading` (with `Retry-After`) while the model loads, `unavailable`
/// when AI is off or failed to load.
pub const AI_SUGGEST: &str = "/api/v1/ai/suggest";

// --- search, usage, stats ----------------------------------------------------------
/// GET `?q=<query>&limit=&offset=&target=<TargetApp>` -> [`crate::model::SearchResults`]
pub const SEARCH: &str = "/api/v1/search";
/// POST [`crate::model::UsageBatch`] -> [`crate::model::UsageAck`]
pub const USAGE: &str = "/api/v1/usage";
/// GET `?scope=server|mine|set&set=<id>&window=day|week|month|all&limit=` -> [`crate::model::Popular`]
pub const POPULAR: &str = "/api/v1/popular";
/// GET -> [`crate::model::UserStats`] for the caller.
pub const STATS_ME: &str = "/api/v1/stats/me";
/// GET -> `Vec<TargetProfile>`
pub const PROFILES: &str = "/api/v1/profiles";
/// GET -> [`crate::model::Job`]
pub const JOB: &str = "/api/v1/jobs/{id}";

// --- admin (role = admin) -----------------------------------------------------------
/// GET -> `Vec<AdminUser>`; POST [`crate::model::AdminCreateUser`] -> AdminUser
pub const ADMIN_USERS: &str = "/api/v1/admin/users";
/// PATCH [`crate::model::AdminUpdateUser`] -> AdminUser; DELETE
pub const ADMIN_USER: &str = "/api/v1/admin/users/{id}";
/// GET -> `Vec<Invitation>`; POST [`crate::model::CreateInvitation`] -> [`crate::model::InvitationCreated`]
pub const ADMIN_INVITATIONS: &str = "/api/v1/admin/invitations";
/// DELETE
pub const ADMIN_INVITATION: &str = "/api/v1/admin/invitations/{id}";
/// GET `?before=<id>&limit=` -> `Vec<AuditEntry>`
pub const ADMIN_AUDIT: &str = "/api/v1/admin/audit";
/// GET -> [`crate::model::StorageUsage`]
pub const ADMIN_STORAGE: &str = "/api/v1/admin/storage";
/// GET `?status=` -> `Vec<Job>`
pub const ADMIN_JOBS: &str = "/api/v1/admin/jobs";

/// Substitute `{name}` segments in order: `path(ITEM_FILE, &[id, "thumb"])`.
pub fn path(template: &str, args: &[&str]) -> String {
    let mut out = String::with_capacity(template.len() + 32);
    let mut args = args.iter();
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let end = rest[start..].find('}').map(|e| start + e).expect("unbalanced {");
        out.push_str(args.next().expect("too few path args"));
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    assert!(args.next().is_none(), "too many path args");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitutes_segments() {
        assert_eq!(path(ITEM_FILE, &["abc", "thumb"]), "/api/v1/items/abc/file/thumb");
        assert_eq!(path(SETS, &[]), "/api/v1/sets");
    }

    #[test]
    #[should_panic]
    fn rejects_missing_args() {
        path(SET, &[]);
    }
}
