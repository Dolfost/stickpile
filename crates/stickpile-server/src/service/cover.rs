//! A set's own cover image: a picture uploaded for the set that is not one of
//! its items (docs/api.md, docs/web.md).
//!
//! The cover shown for a set is the cover image, else the chosen cover item
//! (`sets.cover_item_id`), else the first item. Choosing an item as the
//! cover (`UpdateSet::cover_item_id`, "Use as cover") removes the image, so
//! the cover is always what was chosen last; uploading an image keeps the
//! chosen item, which is the cover again once the image is removed.
//!
//! Storage: the image goes through the upload pipeline (sniffed, sanitised,
//! content-addressed `media` row with the original and the grid thumbnail)
//! and `sets.cover_media_id` names it. Only stills are taken. No item
//! references it, so it is never in item lists, search, counts or client
//! caches, gets no other variants and no AI enrichment. The orphan-media GC
//! keeps media a set names and reclaims it once the image is replaced,
//! removed or the set is deleted, like a deleted item's media.

use axum::extract::Multipart;
use stickpile_api::routes as r;
use stickpile_api::{CoverImage, MediaFormat, MediaKind, Set, Variant};

use crate::auth::CurrentUser;
use crate::authz;
use crate::db::{SetRow, VariantRow};
use crate::error::{AppError, AppResult};
use crate::service::upload::{self, multipart_error, UploadFile};
use crate::service::{hydrate, sets};
use crate::state::AppState;

/// What a cover image may be, for messages.
pub const ACCEPTED: &str = "a still PNG, JPEG or WebP image";

/// URL path of a variant (`original` or `thumb`) of a set's cover image. The
/// `v` parameter changes with the image, so the URL may be cached for good.
pub fn image_url(set_id: &str, variant: Variant, sha256: &str) -> String {
    let v = sha256.get(..16).unwrap_or(sha256);
    format!("{}?v={v}", r::path(r::SET_COVER_FILE, &[set_id, variant.as_str()]))
}

/// The API view of a cover image from its media facts.
pub fn image_info(set_id: &str, sha256: &str, format: MediaFormat, width: u32, height: u32) -> CoverImage {
    CoverImage {
        thumb_url: image_url(set_id, Variant::Thumb, sha256),
        url: image_url(set_id, Variant::Original, sha256),
        format,
        width,
        height,
        sha256: sha256.to_string(),
    }
}

/// Only stills in the formats every target takes.
pub(crate) fn still_only(p: &stickpile_media::Probe) -> AppResult<()> {
    let what = match p.kind {
        MediaKind::Still if matches!(p.format, MediaFormat::Png | MediaFormat::Jpeg | MediaFormat::Webp) => {
            return Ok(())
        }
        MediaKind::Still => "a GIF",
        MediaKind::Animation => "an animation",
        MediaKind::Video => "a video",
    };
    Err(AppError::unsupported_media(format!(
        "a cover image must be {ACCEPTED}, not {what}"
    )))
}

/// Refuse videos and GIFs by their bytes before anything is decoded (a
/// video would go to ffmpeg first).
pub(crate) async fn sniff_still(file: &UploadFile) -> AppResult<()> {
    let bytes = file.bytes().await?;
    match stickpile_media::sniff(&bytes) {
        Some(MediaFormat::Png | MediaFormat::Jpeg | MediaFormat::Webp) => Ok(()),
        Some(MediaFormat::Mp4 | MediaFormat::Webm) => Err(AppError::unsupported_media(format!(
            "a cover image must be {ACCEPTED}, not a video"
        ))),
        Some(MediaFormat::Gif) => Err(AppError::unsupported_media(format!(
            "a cover image must be {ACCEPTED}, not a GIF"
        ))),
        None => Err(AppError::unsupported_media(format!(
            "unsupported or unrecognised file type: a cover image must be {ACCEPTED}"
        ))),
    }
}

/// Store `file` (stills only) and make it the cover image of `set`. True if
/// the set's cover image changed; `bump` then bumps its version (set.toml
/// names the file). Holds the media lock until the set names the media, so
/// the orphan-media GC cannot take it in between. No authorization.
pub async fn store(state: &AppState, set: &SetRow, file: &UploadFile, bump: bool) -> AppResult<bool> {
    sniff_still(file).await?;
    let recommended = set
        .recommended_px
        .map(|v| v as u32)
        .unwrap_or(state.config.media.default_recommended_px);
    let _guard = state.media_lock.read().await;
    let (media, _) = upload::media_for_file_where(state, file, recommended, still_only).await?;
    let mut tx = sets::begin(&state.db).await?;
    let changed = sqlx::query("UPDATE sets SET cover_media_id = ? WHERE id = ? AND cover_media_id IS NOT ?")
        .bind(&media.id)
        .bind(&set.id)
        .bind(&media.id)
        .execute(&mut *tx)
        .await?
        .rows_affected()
        > 0;
    if changed && bump {
        sets::bump_version(&mut tx, &set.id).await?;
    }
    tx.commit().await?;
    Ok(changed)
}

/// Drop the cover image of `set_id` (the media is left to the GC); true if
/// it had one.
pub async fn clear(conn: &mut sqlx::SqliteConnection, set_id: &str) -> AppResult<bool> {
    Ok(
        sqlx::query("UPDATE sets SET cover_media_id = NULL WHERE id = ? AND cover_media_id IS NOT NULL")
            .bind(set_id)
            .execute(conn)
            .await?
            .rows_affected()
            > 0,
    )
}

/// The one file part of a cover upload (other fields are ignored).
async fn read_file(state: &AppState, mut mp: Multipart) -> AppResult<UploadFile> {
    let max_file = state.config.limits.max_upload_mb * 1024 * 1024;
    let mut found = None;
    while let Some(mut field) = mp.next_field().await.map_err(multipart_error)? {
        if field.file_name().is_none_or(str::is_empty) {
            continue;
        }
        if found.is_some() {
            return Err(AppError::bad_request("a cover image is one file"));
        }
        found = Some(upload::field_to_file(state, &mut field, max_file).await?);
    }
    found
        .filter(|f| f.size > 0)
        .ok_or_else(|| AppError::bad_request("choose an image to upload"))
}

/// Set the cover image from a multipart body with one file part: the whole
/// endpoint (authorization, rate limit as an upload), shared with the web UI.
pub async fn upload_multipart(state: &AppState, viewer: &CurrentUser, set_id: &str, mp: Multipart) -> AppResult<Set> {
    let set = authz::set_for(state, viewer.id(), set_id, authz::Need::EditPack).await?;
    state.limiters.check_upload(viewer.id())?;
    let file = read_file(state, mp).await?;
    store(state, &set, &file, true).await?;
    reload(state, viewer, set_id).await
}

/// Remove the cover image (idempotent); the chosen item, else the first
/// item, is the cover again.
pub async fn remove(state: &AppState, viewer: &CurrentUser, set_id: &str) -> AppResult<Set> {
    authz::set_for(state, viewer.id(), set_id, authz::Need::EditPack).await?;
    let mut tx = sets::begin(&state.db).await?;
    if clear(&mut tx, set_id).await? {
        sets::bump_version(&mut tx, set_id).await?;
    }
    tx.commit().await?;
    reload(state, viewer, set_id).await
}

async fn reload(state: &AppState, viewer: &CurrentUser, set_id: &str) -> AppResult<Set> {
    let row = crate::db::set_by_id(&state.db, set_id)
        .await?
        .ok_or_else(AppError::not_found)?;
    sets::load_api(&state.db, &row, viewer.id()).await
}

/// The stored file of a variant (`original` or `thumb`) of a set's cover
/// image, for a caller who may read the set; otherwise 404.
pub async fn file(state: &AppState, viewer: &CurrentUser, set_id: &str, variant: &str) -> AppResult<CoverFileRow> {
    let set = authz::readable_set(state, viewer.id(), set_id).await?;
    let variant = match Variant::parse(variant) {
        Some(v @ (Variant::Original | Variant::Thumb)) => v,
        _ => return Err(AppError::not_found()),
    };
    let media_id = set.cover_media_id.clone().ok_or_else(AppError::not_found)?;
    let media = crate::db::media_by_id(&state.db, &media_id)
        .await?
        .ok_or_else(AppError::not_found)?;
    let row = variant_row(&state.db, &media_id, variant)
        .await?
        .ok_or_else(AppError::not_found)?;
    Ok(CoverFileRow {
        slug: set.slug.clone(),
        sha256: media.sha256,
        variant,
        row,
    })
}

/// A variant of the cover image with what its response needs.
pub struct CoverFileRow {
    /// The set's slug, for the download name.
    pub slug: String,
    /// Of the image as uploaded (the URL's `v`).
    pub sha256: String,
    pub variant: Variant,
    pub row: VariantRow,
}

pub async fn variant_row(db: &sqlx::SqlitePool, media_id: &str, variant: Variant) -> AppResult<Option<VariantRow>> {
    let rows = hydrate::variant_rows(db, &[media_id.to_string()]).await?;
    Ok(rows
        .get(media_id)
        .and_then(|v| v.iter().find(|r| r.variant() == Some(variant)))
        .cloned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(kind: MediaKind, format: MediaFormat) -> stickpile_media::Probe {
        stickpile_media::Probe {
            format,
            kind,
            width: 10,
            height: 10,
            frame_count: None,
            duration_ms: None,
            fps: None,
            has_alpha: false,
        }
    }

    #[test]
    fn only_stills_make_covers() {
        for f in [MediaFormat::Png, MediaFormat::Jpeg, MediaFormat::Webp] {
            assert!(still_only(&probe(MediaKind::Still, f)).is_ok(), "{f:?}");
        }
        for (kind, format, what) in [
            (MediaKind::Animation, MediaFormat::Webp, "not an animation"),
            (MediaKind::Animation, MediaFormat::Png, "not an animation"),
            (MediaKind::Still, MediaFormat::Gif, "not a GIF"),
            (MediaKind::Video, MediaFormat::Mp4, "not a video"),
        ] {
            let e = still_only(&probe(kind, format)).unwrap_err();
            assert_eq!(e.status, axum::http::StatusCode::UNSUPPORTED_MEDIA_TYPE);
            assert!(e.message.ends_with(what), "{}", e.message);
        }
        assert!(image_url("s", Variant::Thumb, &"ab".repeat(32)).ends_with("/cover/thumb?v=abababababababab"));
    }
}
